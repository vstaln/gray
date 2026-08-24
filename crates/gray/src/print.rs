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

/// Renders a single AgentEvent to a writer according to CLI display conventions.
pub fn render_event<W: Write>(w: &mut W, event: &AgentEvent) -> std::io::Result<()> {
    match event {
        AgentEvent::Start => Ok(()),
        AgentEvent::TextDelta { delta } => {
            write!(w, "{delta}")?;
            w.flush()
        }
        AgentEvent::ToolCallStart { name, .. } => {
            writeln!(w, "[tool] {name}")?;
            w.flush()
        }
        AgentEvent::ToolCallEnd { .. } => Ok(()),
        AgentEvent::ToolResult { output, is_error, .. } => {
            if *is_error {
                writeln!(w, "! {output}")?;
                w.flush()?;
            }
            Ok(())
        }
        AgentEvent::TurnEnd { .. } => {
            writeln!(w)?;
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
    let events = agent
        .run(user_msg, ctx)
        .await
        .map_err(|e| anyhow::anyhow!("agent execution failed: {e}"))?;

    let mut stdout = std::io::stdout();
    for event in &events {
        render_event(&mut stdout, event)?;
    }

    // Persist session to JSONL store
    let store = JsonlSessionStore::default();
    save_session(&store, &config.model, &cwd, agent.messages()).await?;

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

        render_event(&mut buf, &AgentEvent::Start).unwrap();
        assert_eq!(buf, b"");

        render_event(&mut buf, &AgentEvent::text_delta("Hello, ")).unwrap();
        render_event(&mut buf, &AgentEvent::text_delta("world!")).unwrap();
        assert_eq!(buf, b"Hello, world!");
        buf.clear();

        render_event(
            &mut buf,
            &AgentEvent::tool_call_start("call_1", "read"),
        )
        .unwrap();
        assert_eq!(buf, b"[tool] read\n");
        buf.clear();

        render_event(
            &mut buf,
            &AgentEvent::tool_call_end("call_1", json!({"path": "file.txt"})),
        )
        .unwrap();
        assert_eq!(buf, b"");

        // Non-error tool result produces no stdout
        render_event(
            &mut buf,
            &AgentEvent::tool_result("call_1", "file contents", false),
        )
        .unwrap();
        assert_eq!(buf, b"");

        // Error tool result is prefixed with '!'
        render_event(
            &mut buf,
            &AgentEvent::tool_result("call_1", "file not found", true),
        )
        .unwrap();
        assert_eq!(buf, b"! file not found\n");
        buf.clear();

        // Turn end outputs a newline
        render_event(
            &mut buf,
            &AgentEvent::turn_end(StopReason::EndTurn, Usage::new(10, 20)),
        )
        .unwrap();
        assert_eq!(buf, b"\n");
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
