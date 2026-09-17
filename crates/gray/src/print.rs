//! Print mode: one-shot execution of a user prompt, streaming events to stdout
//! and recording the conversation to a JSONL session.

use std::collections::HashMap;
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::session_store::{JsonlSessionStore, SessionId, SessionMeta};
use gray_core::agent::ToolContext;
use gray_core::event::AgentEvent;
use gray_core::message::Message;
use gray_core::redaction::{redact_for_disclosure, redact_message};

use crate::build_agent;
use crate::config::Config;

/// Wall-clock milliseconds since the Unix epoch (0 on a pre-epoch clock).
pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

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
    run_print_inner(config, prompt, session, continue_last, None).await
}

/// Machine-readable print mode. Never forwards reasoning, tool arguments/results,
/// provider error bodies, or plugin host/say output to the transport.
pub async fn run_print_mode_json(
    config: &Config,
    prompt: &str,
    session: Option<&str>,
    continue_last: bool,
    max_requests: Option<u32>,
    input_price: Option<f64>,
    output_price: Option<f64>,
) -> anyhow::Result<()> {
    let mut output = JsonOutput {
        turn_id: uuid::Uuid::new_v4().to_string(),
        session_id: None,
        text: String::new(),
        usage: gray_core::event::Usage::default(),
        meter: None,
    };
    let result = match crate::print_meter::Meter::new(
        max_requests.unwrap_or(32),
        input_price,
        output_price,
        config.max_cost_micros,
    ) {
        Ok(meter) => {
            output.meter = Some(meter);
            run_print_inner(config, prompt, session, continue_last, Some(&mut output)).await
        }
        Err(error) => Err(error),
    };
    let mut row = match &result {
        Ok(()) => serde_json::json!({"type": "result", "text": output.text, "usage": output.usage}),
        Err(_) => serde_json::json!({"type": "error", "code": "turn_failed",
            "message": "Agent turn failed; actions may already have occurred. Do not automatically retry."}),
    };
    if let Some(meter) = &output.meter {
        row["accounting"] = serde_json::to_value(meter.snapshot())?;
    }
    output.write(row)?;
    // The detailed human-mode error may contain provider context. Keep the JSON
    // error and process exit consistent without copying that context to stderr.
    result.map_err(|_| anyhow::anyhow!("agent turn failed (see JSON error record)"))
}

struct JsonOutput {
    turn_id: String,
    session_id: Option<String>,
    text: String,
    usage: gray_core::event::Usage,
    meter: Option<crate::print_meter::Meter>,
}

impl JsonOutput {
    fn write(&self, mut row: serde_json::Value) -> std::io::Result<()> {
        row["protocol"] = 1.into();
        row["turn_id"] = self.turn_id.clone().into();
        row["session_id"] = self.session_id.clone().into();
        let mut out = std::io::stdout().lock();
        writeln!(out, "{row}")?;
        out.flush()
    }

    fn event(&mut self, event: &AgentEvent) -> std::io::Result<()> {
        let phase = match event {
            AgentEvent::Start => "generating",
            AgentEvent::ToolCallStart { .. } => "tool_started",
            AgentEvent::ToolResult { .. } => "tool_finished",
            AgentEvent::StreamError { .. } => "provider_retry",
            AgentEvent::TurnEnd { usage, .. } => {
                self.usage = *usage;
                "persisting"
            }
            _ => return Ok(()),
        };
        self.write(serde_json::json!({"type": "progress", "phase": phase}))
    }
}

async fn run_print_inner(
    config: &Config,
    prompt: &str,
    session: Option<&str>,
    continue_last: bool,
    mut json: Option<&mut JsonOutput>,
) -> anyhow::Result<()> {
    crate::setup::set_user_context_window(config.context_window);
    crate::setup::set_user_reserve_tokens(config.context_reserve);
    crate::setup::set_user_keep_recent_tokens(config.context_keep);
    let cwd = std::env::current_dir()?;
    let store = JsonlSessionStore::default();
    // Explicit `--session` wins over `-c` (same precedence as the REPL).
    let mut resume_target: Option<SessionId> = match session {
        Some(raw) if json.is_some() && uuid::Uuid::parse_str(raw).is_ok() => {
            let id = SessionId::new(raw);
            store.maintain(&id).await?;
            Some(id)
        }
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
    if let Some(output) = json.as_deref_mut() {
        if resume_target.is_none() {
            resume_target = Some(
                save_session(
                    &store,
                    config.model.as_deref().unwrap_or("unset"),
                    &cwd,
                    &[],
                )
                .await?,
            );
        }
        output.session_id = resume_target.as_ref().map(|id| id.as_str().to_owned());
    }
    let initial_count = history.len();
    let cancel = tokio_util::sync::CancellationToken::new();
    let ctx = ToolContext {
        cwd: cwd.clone(),
        cancel: cancel.clone(),
        session_id: resume_target.as_ref().map(|id| id.as_str().to_owned()),
    };

    // One-shot run = one turn: only max_turns=0 (nonsensical but
    // explicit) and an already-blown wall clock can stop it pre-run.
    // max_turns=0 is rejected at resolve, so this is dead-simple.
    if let Some(max) = config.max_wall_secs
        && crate::turn_caps::process_start().elapsed().as_secs() >= max
    {
        anyhow::bail!("max wall time {max}s reached — stopping (--max-wall-secs SECS)");
    }
    let mut agent = build_agent(config, &cwd, resume_target.as_ref().map(|s| s.as_str())).await?;
    if let Some(meter) = json.as_ref().and_then(|output| output.meter.as_ref()) {
        agent = agent.map_provider(|provider| meter.wrap(provider));
    }
    if !history.is_empty() {
        agent = agent.with_messages(history);
    }
    for w in crate::take_profile_warnings() {
        eprintln!("warning: {w}");
    }

    let history_revision = agent.history_revision();
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
            let rendered = match json.as_deref_mut() {
                Some(output) => output.event(ev),
                None => {
                    render_event_with_context(&mut stdout.lock(), ev, Some(&cwd), &mut in_flight)
                }
            };
            if let Err(e) = rendered {
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
    if run_result.is_ok()
        && let Some(output) = json.as_deref_mut()
    {
        // Read the live final assistant message, not redacted JSONL and not
        // an exact-text search that could select a previous identical turn.
        if let Some(message) = agent
            .messages()
            .last()
            .filter(|m| m.role == gray_core::message::Role::Assistant)
        {
            output.text = redact_message(message).text_content();
        }
    }

    // F14: no `?` between run completion and finalization — always attempt
    // session persistence AND shell teardown, then propagate the original
    // error combined with any persistence error.
    let persist_result: anyhow::Result<()> = if let Some(sid) = &resume_target {
        append_new_messages(
            &store,
            sid,
            initial_count,
            agent.messages(),
            agent.history_revision() != history_revision,
        )
        .await
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

    let render_broken = render_err
        .as_ref()
        .is_some_and(|e| e.kind() == ErrorKind::BrokenPipe);
    // Cron (or other plugin-initiated) `host/say` lines queued mid-turn.
    // Fallible writeln on a locked handle: stdout may already be closed.
    if render_broken || json.is_some() {
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
/// When in-loop compaction rewrote history (even if it grew past the cursor),
/// persists the whole active transcript behind a replacement boundary.
pub async fn append_new_messages(
    store: &JsonlSessionStore,
    sid: &SessionId,
    prior_count: usize,
    messages: &[Message],
    history_rewritten: bool,
) -> anyhow::Result<()> {
    if history_rewritten || messages.len() < prior_count {
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
    let timestamp = now_millis();

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

#[path = "print_tests.rs"]
#[cfg(test)]
mod tests;
