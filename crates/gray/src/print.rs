//! Print mode: one-shot execution of a user prompt, streaming events to stdout
//! and recording the conversation to a JSONL session.

use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use gray_core::agent::ToolContext;
use gray_core::event::AgentEvent;
use gray_core::message::Message;
use gray_session::{JsonlSessionStore, SessionId, SessionMeta, SessionStore};

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
            let name = current_tool.as_ref().map(|t| t.name.clone()).unwrap_or_else(|| "tool".to_string());
            if let Some(t) = current_tool {
                t.args = Some(args.clone());
            }
            writeln!(w, "\n{}", crate::tool_fmt::format_tool_call_header_plain(&name, args, cwd))?;
            w.flush()
        }
        AgentEvent::ToolResult { output, is_error, .. } => {
            let tool = current_tool.take().unwrap_or_default();
            let res = crate::tool_fmt::format_tool_result_plain_with_context(&tool.name, tool.args.as_ref(), output, *is_error, cwd);
            if !res.is_empty() {
                write!(w, "{res}")?;
            }
            w.flush()
        }
        AgentEvent::TurnEnd { usage, .. } => {
            if usage.total() > 0 {
                writeln!(w, "\n\x1b[2m\u{2b22} {} tok\x1b[0m", crate::repl::fmt_usage(usage.total()))?;
            }
            w.flush()
        }
    }
}

/// Executes a prompt in one-shot print mode, printing events to stdout and persisting the session.
pub async fn run_print_mode(config: &Config, prompt: &str) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let cancel = tokio_util::sync::CancellationToken::new();
    let ctx = ToolContext {
        cwd: cwd.clone(),
        cancel,
    };

    let mut agent = build_agent(config, &cwd)?;

    let user_msg = Message::user(prompt);
    // Stream events live so piped output isn't all-or-nothing.
    let result = {
        let stdout = std::io::stdout();
        let mut current_tool = None;
        let mut on_event = |ev: &AgentEvent| {
            if let Err(e) = render_event_with_context(&mut stdout.lock(), ev, Some(&cwd), &mut current_tool) {
                eprintln!("render error: {e}");
            }
        };
        agent
            .run_streaming(user_msg, ctx, &mut on_event)
            .await
            .map_err(|e| anyhow::anyhow!("agent execution failed: {e}"))?
    };
    drop(result);

    // Persist session to JSONL store
    let store = JsonlSessionStore::default();
    save_session(&store, config.model.as_deref().unwrap_or("unset"), &cwd, agent.messages()).await?;

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
    store.create(meta).await;

    for msg in messages {
        store
            .append(&session_id, msg)
            .await
            .map_err(|e| anyhow::anyhow!("failed to append message to session: {e}"))?;
    }

    Ok(session_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gray_core::event::{StopReason, Usage};
    use serde_json::json;

    #[test]
    fn render_event_formats_correctly() {
        let mut buf = Vec::new();
        let mut tool = None;

        render_event_with_context(&mut buf, &AgentEvent::Start, None, &mut tool).unwrap();
        assert_eq!(buf, b"");

        render_event_with_context(&mut buf, &AgentEvent::text_delta("Hello, "), None, &mut tool).unwrap();
        render_event_with_context(&mut buf, &AgentEvent::text_delta("world!"), None, &mut tool).unwrap();
        assert_eq!(buf, b"Hello, world!");
        buf.clear();

        render_event_with_context(&mut buf, &AgentEvent::tool_call_start("call_1", "read"), None, &mut tool).unwrap();
        assert!(buf.is_empty());
        buf.clear();

        render_event_with_context(&mut buf, &AgentEvent::tool_call_end("call_1", json!({"path": "file.txt"})), None, &mut tool).unwrap();
        assert!(!buf.is_empty());
        buf.clear();

        render_event_with_context(&mut buf, &AgentEvent::tool_result("call_1", "file contents", false), None, &mut tool).unwrap();
        assert_eq!(buf, b"");
        buf.clear();

        let mut tool2 = Some(ActiveToolCall {
            name: "read".to_string(),
            args: None,
        });
        render_event_with_context(&mut buf, &AgentEvent::tool_result("call_1", "file not found", true), None, &mut tool2).unwrap();
        assert!(!buf.is_empty());
        buf.clear();

        render_event_with_context(&mut buf, &AgentEvent::turn_end(StopReason::EndTurn, Usage::new(10, 20)), None, &mut tool).unwrap();
        assert!(!buf.is_empty());
    }

    #[tokio::test]
    async fn save_session_persists_all_messages() {
        let tmp = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(tmp.path());
        let msgs = vec![
            Message::user("tell me a joke"),
            Message::assistant("why did the chicken cross the road?"),
        ];

        let session_id = save_session(&store, "test-model", tmp.path(), &msgs)
            .await
            .expect("save_session should succeed");

        let (meta, entries) = store
            .load(&session_id)
            .await
            .expect("load should succeed");
        assert_eq!(meta.id, session_id);
        assert_eq!(meta.model, "test-model");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].message, msgs[0]);
        assert_eq!(entries[1].message, msgs[1]);
    }
}
