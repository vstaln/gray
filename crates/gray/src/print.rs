//! Print mode: one-shot execution of a user prompt, streaming events to stdout
//! and recording the conversation to a JSONL session.

use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use gray_core::agent::{PermissionMode, ToolContext};
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

/// Renders a single AgentEvent to a writer according to CLI display conventions.
pub fn render_event<W: Write>(w: &mut W, event: &AgentEvent) -> std::io::Result<()> {
    let mut current_tool = None;
    render_event_with_context(w, event, None, &mut current_tool)
}

/// Renders a single AgentEvent with active tool tracking and CWD context.
pub fn render_event_with_context<W: Write>(
    w: &mut W,
    event: &AgentEvent,
    cwd: Option<&Path>,
    current_tool: &mut Option<ActiveToolCall>,
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
        AgentEvent::ToolCallStart { name, .. } => {
            *current_tool = Some(ActiveToolCall {
                name: name.clone(),
                args: None,
            });
            Ok(())
        }
        AgentEvent::ToolCallEnd { args, .. } => {
            let name = current_tool
                .as_ref()
                .map(|t| t.name.clone())
                .unwrap_or_else(|| "tool".to_string());
            if let Some(t) = current_tool {
                t.args = Some(args.clone());
            }
            writeln!(
                w,
                "\n{}",
                crate::tool_fmt::format_tool_call_header_plain(&name, args, cwd)
            )?;
            w.flush()
        }
        AgentEvent::ToolResult {
            output, is_error, ..
        } => {
            let tool = current_tool.take().unwrap_or_default();
            let res = crate::tool_fmt::format_tool_result_plain_with_context(
                &tool.name,
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
    // Unified with Config::resolve: explicit env (canonical
    // `GRAY_PERMISSION`, alias `GRAY_PERMISSIONS`) wins via
    // config.permissions; saved file is the fallback.
    let permissions = config.permissions.clone().or_else(|| {
        crate::setup::load_saved_config_at(
            &crate::setup::saved_config_path()
                .unwrap_or_else(|_| std::path::PathBuf::from("/dev/null")),
        )
        .permissions
    });
    let ctx = ToolContext {
        cwd: cwd.clone(),
        cancel,
        questions: None,
        session_id: None, // one-shot print mode has no session
        permission: PermissionMode::resolve(true), // auto for -p unless GRAY_PERMISSION=ask
        // (guard Prompt verdicts fail closed at the tool seam in this mode —
        // no TTY to ask on, so risky commands deny instead of auto-running).
        approvals: Some(gray_core::approvals::ApprovalGate::new(
            permissions
                .as_deref()
                .unwrap_or(gray_core::approvals::MODE_AUTO),
        )),
    };

    let mut agent = build_agent(config, &cwd, resume_target.as_ref().map(|s| s.as_str())).await?;
    if !history.is_empty() {
        agent = agent.with_messages(history);
    }
    for w in crate::take_profile_warnings() {
        eprintln!("warning: {w}");
    }

    let user_msg = Message::user(prompt);
    // Stream events live so piped output isn't all-or-nothing.
    let result = {
        let stdout = std::io::stdout();
        let mut current_tool = None;
        let mut on_event = |ev: &AgentEvent| {
            if let Err(e) =
                render_event_with_context(&mut stdout.lock(), ev, Some(&cwd), &mut current_tool)
            {
                eprintln!("render error: {e}");
            }
        };
        agent
            .run_streaming(user_msg, ctx, &mut on_event)
            .await
            .map_err(|e| {
                let msg = crate::repl::format_core_error(&e, &config.base_url);
                anyhow::anyhow!(scrub_error_text(&msg))
            })?
    };
    drop(result);

    // Persist session to JSONL store: a resumed session keeps its file
    // (only the new turn is appended); otherwise a fresh session is created.
    if let Some(sid) = &resume_target {
        append_new_messages(&store, sid, initial_count, agent.messages()).await?;
    } else {
        save_session(
            &store,
            config.model.as_deref().unwrap_or("unset"),
            &cwd,
            agent.messages(),
        )
        .await?;
    }

    // Cron (or other plugin-initiated) `host/say` lines queued mid-turn.
    for line in crate::host::take_host_say() {
        println!("{line}");
    }

    // Print mode has no later turn: background tasks would orphan, so stop
    // the one-shot session unconditionally (exit code unaffected).
    let _ = crate::shell_drain::shutdown_shell_session("nosession").await;

    Ok(())
}

/// Scrub provider/error text before it reaches stderr or a receipt.
pub(crate) fn scrub_error_text(s: &str) -> String {
    redact_for_disclosure(s).into_text()
}

/// Appends only messages at index `prior_count..` to an existing session
/// (print-mode `--session`/`-c` continuation in place — never a new file).
pub async fn append_new_messages(
    store: &JsonlSessionStore,
    sid: &SessionId,
    prior_count: usize,
    messages: &[Message],
) -> anyhow::Result<()> {
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
}
