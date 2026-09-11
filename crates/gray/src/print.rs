//! Print mode: one-shot execution of a user prompt, streaming events to stdout
//! and recording the conversation to a JSONL session.

use std::collections::HashMap;
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use gray_core::agent::ToolContext;
use gray_core::event::AgentEvent;
use gray_core::message::Message;
use gray_core::redaction::{redact_for_disclosure, redact_message};
use gray_session::{JsonlSessionStore, SessionId, SessionMeta};

use crate::build_agent;
use crate::config::Config;

/// Tracking state for active tool call during streaming.
#[derive(Debug, Clone, Default)]
pub struct ActiveToolCall {
    pub name: String,
    pub args: Option<serde_json::Value>,
}

/// Renders a single AgentEvent with active tool tracking and CWD context.
///
/// In-flight calls are keyed by call id so concurrent starts/ends/results
/// can't clobber each other (single-slot tracking swapped names/args on
/// parallel calls).
pub fn render_event_with_context<W: Write>(
    w: &mut W,
    event: &AgentEvent,
    cwd: Option<&Path>,
    in_flight: &mut HashMap<String, ActiveToolCall>,
) -> std::io::Result<()> {
    match event {
        AgentEvent::Start => Ok(()),
        AgentEvent::TextDelta { delta } => {
            write!(w, "{delta}")?;
            w.flush()
        }
        AgentEvent::ThinkingDelta { delta } => {
            // Same dim+italic treatment as the REPL (pi's thinking style).
            write!(w, "\x1b[2m\x1b[3m{delta}\x1b[0m")?;
            w.flush()
        }
        AgentEvent::ToolCallStart { id, name } => {
            in_flight.insert(
                id.clone(),
                ActiveToolCall {
                    name: name.clone(),
                    args: None,
                },
            );
            Ok(())
        }
        AgentEvent::ToolCallProgress { id, name, .. } => {
            in_flight.entry(id.clone()).or_insert(ActiveToolCall {
                name: name.clone(),
                args: None,
            });
            Ok(())
        }
        AgentEvent::ToolCallEnd { id, args } => {
            let entry = in_flight.entry(id.clone()).or_insert(ActiveToolCall {
                name: "tool".to_string(),
                args: None,
            });
            entry.args = Some(args.clone());
            if entry.name.is_empty() {
                entry.name = "tool".to_string();
            }
            let name = entry.name.as_str();
            writeln!(
                w,
                "\n{}",
                crate::tool_fmt::format_tool_call_header_plain(name, args, cwd)
            )?;
            w.flush()
        }
        AgentEvent::ToolResult {
            id,
            output,
            is_error,
            ..
        } => {
            let tool = in_flight.remove(id).unwrap_or_default();
            let name = if tool.name.is_empty() {
                "tool"
            } else {
                tool.name.as_str()
            };
            let res = crate::tool_fmt::format_tool_result_plain_with_context(
                name,
                tool.args.as_ref(),
                output,
                *is_error,
                cwd,
            );
            if !res.is_empty() {
                write!(w, "{res}")?;
            }
            w.flush()
        }
        AgentEvent::StepUsage { .. } => Ok(()),
        // Codex steal: retry notices go to the same stream, dim, never fatal.
        AgentEvent::StreamError { message, details } => {
            // Provider retry notices can echo request details: scrub first.
            let message = scrub_error_text(message);
            let details = scrub_error_text(details);
            if details.is_empty() {
                writeln!(w, "\n\x1b[2m⚠ {message}\x1b[0m")?;
            } else {
                writeln!(w, "\n\x1b[2m⚠ {message}\n└ {details}\x1b[0m")?;
            }
            w.flush()
        }
        AgentEvent::TurnEnd { usage, .. } => {
            if usage.total() > 0 {
                writeln!(
                    w,
                    "\n\x1b[2m\u{2b22} {} tok\x1b[0m",
                    crate::repl::fmt_usage(usage.total())
                )?;
            }
            w.flush()
        }
    }
}

/// Executes a prompt in one-shot print mode, printing events to stdout and persisting the session.
pub async fn run_print_mode(config: &Config, prompt: &str) -> anyhow::Result<()> {
    run_print_mode_with_session(config, prompt, None, false).await
}

/// Print mode with `--session <id>` / `-c` support: a resolved session is
/// continued in place (new messages appended to its JSONL); otherwise a fresh
/// session is created. Bogus ids fail with the same `no session matching`
/// error as `gray resume <id>` (exit 1), before any model work starts.
pub async fn run_print_mode_with_session(
    config: &Config,
    prompt: &str,
    session: Option<&str>,
    continue_last: bool,
) -> anyhow::Result<()> {
    crate::setup::set_user_context_window(config.context_window);
    crate::setup::set_user_reserve_tokens(config.context_reserve);
    crate::setup::set_user_keep_recent_tokens(config.context_keep);
    let cwd = std::env::current_dir()?;
    let store = JsonlSessionStore::default();
    // Explicit `--session` wins over `-c` (same precedence as the REPL).
    let resume_target: Option<SessionId> = match session {
        Some(raw) => Some(crate::resume::resolve_session_strict(&store, raw, false).await?),
        None if continue_last => crate::resume::latest_session_anywhere(&store).await,
        None => None,
    };
    let history: Vec<Message> = match &resume_target {
        Some(sid) => {
            let (_, entries) = store
                .load(sid)
                .await
                .map_err(|e| anyhow::anyhow!("could not resume session {}: {e}", sid.as_str()))?;
            entries.into_iter().map(|e| e.message).collect()
        }
        None => Vec::new(),
    };
    let initial_count = history.len();
    let cancel = tokio_util::sync::CancellationToken::new();
    let ctx = ToolContext {
        cwd: cwd.clone(),
        cancel: cancel.clone(),
        session_id: None, // one-shot print mode has no session
    };

    let mut agent = build_agent(config, &cwd, resume_target.as_ref().map(|s| s.as_str())).await?;
    if !history.is_empty() {
        agent = agent.with_messages(history);
    }
    for w in crate::take_profile_warnings() {
        eprintln!("warning: {w}");
    }

    let user_msg = Message::user(prompt);
    // SIGINT only signals the shared token — never wrap the run in a
    // select! that would drop it. Aborted once the run returns.
    let sigint_cancel = cancel.clone();
    let sigint_task = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            sigint_cancel.cancel();
        }
    });
    // Stream events live so piped output isn't all-or-nothing. The first
    // stdout error cancels paid model/tool work and stops further writes
    // (recorded once, never spammed per event).
    let stdout = std::io::stdout();
    let mut in_flight: HashMap<String, ActiveToolCall> = HashMap::new();
    let mut render_err: Option<std::io::Error> = None;
    let run_result = {
        let render_cancel = cancel.clone();
        let mut on_event = |ev: &AgentEvent| {
            if render_err.is_some() {
                return;
            }
            if let Err(e) =
                render_event_with_context(&mut stdout.lock(), ev, Some(&cwd), &mut in_flight)
            {
                render_cancel.cancel();
                render_err = Some(e);
            }
        };
        agent
            .run_streaming(user_msg, ctx, &mut on_event)
            .await
            .map_err(|e| {
                let msg = crate::repl::format_core_error(&e, &config.base_url);
                anyhow::anyhow!(scrub_error_text(&msg))
            })
    };
    sigint_task.abort();

    // F14: no `?` between run completion and finalization — always attempt
    // session persistence AND shell teardown, then propagate the original
    // error combined with any persistence error.
    let persist_result: anyhow::Result<()> = if let Some(sid) = &resume_target {
        append_new_messages(&store, sid, initial_count, agent.messages()).await
    } else {
        save_session(
            &store,
            config.model.as_deref().unwrap_or("unset"),
            &cwd,
            agent.messages(),
        )
        .await
        .map(|_| ())
    };

    // Print mode has no later turn: background tasks would orphan, so stop
    // the one-shot session unconditionally (exit code unaffected).
    let _ = crate::shell_drain::shutdown_shell_session("nosession").await;

    let render_broken = render_err
        .as_ref()
        .is_some_and(|e| e.kind() == ErrorKind::BrokenPipe);
    // Cron (or other plugin-initiated) `host/say` lines queued mid-turn.
    // Fallible writeln on a locked handle: stdout may already be closed.
    if render_broken {
        let _ = crate::host::take_host_say();
    } else {
        let lines = crate::host::take_host_say();
        if !lines.is_empty() {
            let mut out = stdout.lock();
            for line in &lines {
                if writeln!(out, "{line}").is_err() {
                    break;
                }
            }
            let _ = out.flush();
        }
    }

    if render_broken {
        // Graceful BrokenPipe: the consumer went away. Paid work is already
        // cancelled, history persisted, shell torn down. Swallow a
        // cancellation (it is our own stop), keep any other error.
        match (&run_result, &persist_result) {
            (_, Err(p)) => return Err(anyhow::anyhow!("{p:#}")),
            (Ok(_), Ok(_)) => return Ok(()),
            (Err(r), Ok(_)) if r.to_string().to_lowercase().contains("cancel") => {
                return Ok(());
            }
            (Err(_), Ok(_)) => return run_result.map(|_| ()),
        }
    }
    if let Some(e) = render_err {
        // Non-pipe stdout failure: surface it, combined with run/persist.
        return match (run_result, persist_result) {
            (Err(r), Err(p)) => Err(anyhow::anyhow!(
                "{r:#}; stdout write failed: {e}; also failed to persist session: {p:#}"
            )),
            (Err(r), _) => Err(anyhow::anyhow!("{r:#}; stdout write failed: {e}")),
            (_, Err(p)) => Err(anyhow::anyhow!(
                "stdout write failed: {e}; also failed to persist session: {p:#}"
            )),
            _ => Err(anyhow::anyhow!("stdout write failed: {e}")),
        };
    }
    match (run_result, persist_result) {
        (Err(r), Err(p)) => Err(anyhow::anyhow!(
            "{r:#}; also failed to persist session: {p:#}"
        )),
        (Err(r), _) => Err(r),
        (_, Err(p)) => Err(p),
        (Ok(_), Ok(_)) => Ok(()),
    }
}

/// Scrub provider/error text before it reaches stderr or a receipt.
pub(crate) fn scrub_error_text(s: &str) -> String {
    redact_for_disclosure(s).into_text()
}

/// Appends only messages at index `prior_count..` to an existing session
/// (print-mode `--session`/`-c` continuation in place — never a new file).
/// When in-loop compaction shrank history below the cursor, persists the
/// whole active transcript behind a boundary marker instead of skipping it.
pub async fn append_new_messages(
    store: &JsonlSessionStore,
    sid: &SessionId,
    prior_count: usize,
    messages: &[Message],
) -> anyhow::Result<()> {
    if messages.len() < prior_count {
        // Same redaction as the normal path: secrets never land in the file.
        let redacted: Vec<Message> = messages.iter().map(redact_message).collect();
        return store
            .append_compaction_replacement(sid, &redacted)
            .await
            .map_err(|e| anyhow::anyhow!("failed to persist compacted session: {e}"));
    }
    for msg in &messages[prior_count.min(messages.len())..] {
        // Durable JSONL: persist the redacted copy (secrets/paths never land
        // in the session file); the live turn keeps the raw text.
        let redacted = redact_message(msg);
        store
            .append(sid, &redacted)
            .await
            .map_err(|e| anyhow::anyhow!("failed to append message to session: {e}"))?;
    }
    Ok(())
}

/// Saves the accumulated messages to the session store.
pub async fn save_session(
    store: &JsonlSessionStore,
    model: &str,
    cwd: &Path,
    messages: &[Message],
) -> anyhow::Result<SessionId> {
    let session_id = SessionId::generate();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let meta = SessionMeta::new(
        session_id.clone(),
        timestamp,
        cwd.to_path_buf(),
        model.to_string(),
    );
    store.create(meta).await?;

    for msg in messages {
        // Durable JSONL: persist the redacted copy (see append_new_messages).
        let redacted = redact_message(msg);
        store
            .append(&session_id, &redacted)
            .await
            .map_err(|e| anyhow::anyhow!("failed to append message to session: {e}"))?;
    }

    Ok(session_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn append_continues_session_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path());
        let sid = save_session(&store, "m", dir.path(), &[Message::user("first")])
            .await
            .unwrap();
        let before = store.load(&sid).await.unwrap().1.len();
        // Prior history + one new turn: only the new message lands in the file.
        let with_new = vec![Message::user("first"), Message::user("second")];
        append_new_messages(&store, &sid, before, &with_new)
            .await
            .unwrap();
        let (_, entries) = store.load(&sid).await.unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].message.text_content(), "second");
        // Same file, not a new session.
        assert_eq!(store.list().await.len(), 1);
    }

    #[tokio::test]
    async fn append_bogus_session_errors() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path());
        let err = append_new_messages(&store, &SessionId::new("bogus"), 0, &[Message::user("x")])
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("failed to append message to session"),
            "unexpected error: {err:#}"
        );
    }

    #[tokio::test]
    async fn saved_sessions_redact_secrets_and_paths() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path());
        let sid = save_session(
            &store,
            "m",
            dir.path(),
            &[
                Message::user(
                    "read /Users/hunter/src/app/main.rs with ZAI_API_KEY=supersecretvalue12345",
                ),
                // Secret-free: persists verbatim (resume fidelity).
                Message::user("read crates/foo/src/main.rs"),
            ],
        )
        .await
        .unwrap();
        let (_, entries) = store.load(&sid).await.unwrap();
        let text = entries[0].message.text_content();
        assert!(!text.contains("/Users/hunter"), "{text}");
        assert!(!text.contains("supersecretvalue12345"), "{text}");
        assert_eq!(
            entries[1].message.text_content(),
            "read crates/foo/src/main.rs"
        );
    }

    #[test]
    fn error_surfaces_are_scrubbed_before_display() {
        let scrubbed = scrub_error_text("auth failed: ZAI_API_KEY=supersecretvalue12345");
        assert!(!scrubbed.contains("supersecretvalue12345"), "{scrubbed}");
        assert!(scrubbed.contains("<redacted>"), "{scrubbed}");
    }

    #[test]
    fn concurrent_tool_calls_track_by_id() {
        use gray_core::event::AgentEvent;
        let mut in_flight = HashMap::new();
        let mut out = Vec::new();
        let a = serde_json::json!({"x": 1});
        let b = serde_json::json!({"y": 2});
        render_event_with_context(
            &mut out,
            &AgentEvent::tool_call_start("id1", "alpha"),
            None,
            &mut in_flight,
        )
        .unwrap();
        render_event_with_context(
            &mut out,
            &AgentEvent::tool_call_start("id2", "beta"),
            None,
            &mut in_flight,
        )
        .unwrap();
        render_event_with_context(
            &mut out,
            &AgentEvent::tool_call_end("id1", a.clone()),
            None,
            &mut in_flight,
        )
        .unwrap();
        // Second call's end must not clobber the first call's name/args.
        assert_eq!(in_flight["id1"].name, "alpha");
        assert_eq!(in_flight["id1"].args.as_ref(), Some(&a));
        render_event_with_context(
            &mut out,
            &AgentEvent::tool_call_end("id2", b.clone()),
            None,
            &mut in_flight,
        )
        .unwrap();
        assert_eq!(in_flight["id2"].name, "beta");
        render_event_with_context(
            &mut out,
            &AgentEvent::tool_result("id1", "ok-a", false),
            None,
            &mut in_flight,
        )
        .unwrap();
        // id2 survives id1's result (no single-slot take() wiping both).
        assert!(in_flight.contains_key("id2"));
        assert!(!in_flight.contains_key("id1"));
        render_event_with_context(
            &mut out,
            &AgentEvent::tool_result("id2", "ok-b", false),
            None,
            &mut in_flight,
        )
        .unwrap();
        assert!(in_flight.is_empty());
    }

    #[test]
    fn render_error_propagates_for_retry_policy() {
        use std::io::{Error, ErrorKind};
        struct Fail;
        impl std::io::Write for Fail {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(Error::new(ErrorKind::BrokenPipe, "closed"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(Error::new(ErrorKind::BrokenPipe, "closed"))
            }
        }
        let mut in_flight = HashMap::new();
        let err = render_event_with_context(
            &mut Fail,
            &gray_core::event::AgentEvent::text_delta("hi"),
            None,
            &mut in_flight,
        )
        .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::BrokenPipe);
    }
}
