//! Agent turns for the gateway (move-only split).
//!
//! [`GatewayRunner::run_agent`] runs one agent turn against the persisted
//! session and streams deltas to the caller. Cron delivery lives in the
//! external cron plugin now (in-gateway firing removed with `gray-cron`).

use std::sync::Arc;

use crate::authz::DenyExecutor;
use crate::daemon::GatewayRunner;
use crate::daemon_stream::ProgressMsg;

impl GatewayRunner {
    /// Same agent every entry point builds: thin surface wrapper over
    /// [`gray_plugin::builder::build_agent`] (the single profile-aware
    /// builder for REPL, `-p`, gateway, and cron — F8 resolved the `gray →
    /// gray-gateway` cycle by moving it to the lowest common crate).
    /// Surface policy owned here: provider config, gateway system prompt,
    /// [`DenyExecutor`] wrapping, and warn-and-skip on sidecar spawn
    /// failure (the daemon must stay up). `session_id` pins the Responses
    /// cache shard per session instead of colliding all daemon sessions on
    /// one per-process key.
    async fn build_agent(
        &self,
        prior: Vec<gray_core::Message>,
        session_id: Option<&str>,
    ) -> anyhow::Result<gray_core::agent::Agent> {
        let (base_url, api_key, model) = self.resolve_provider_config();
        let model = model.ok_or_else(|| {
            anyhow::anyhow!("no model configured — set ~/.gray/config.json model")
        })?;
        // Every sidecar gets the host runner so plugin-initiated `host/run`
        // (cron fires) / `host/say` don't fall back to loud `{"error":…}`.
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let host_handler = cron_host_handler();
        let agent = gray_plugin::builder::build_agent(gray_plugin::builder::BuilderOptions {
            model,
            api_key: api_key.unwrap_or_default(),
            base_url: base_url.unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string()),
            reasoning_effort: None,
            // Window unknown outside the gray crate; overflow recovery still guards.
            context_window: None,
            session_id: session_id.map(str::to_string),
            cwd,
            system_prompt: gray_plugin::builder::SystemPrompt::Literal(load_system_prompt()),
            extra_tools: Vec::new(),
            host_handler: Some(host_handler),
            profile_path: "gray.yml".to_string(),
            abort_on_spawn_failure: false,
            // Advertise the full registry: denials belong to DenyExecutor so the
            // model gets the gate's accurate reason instead of "does not exist".
            wrap_executor: Some(Box::new(
                |_inner: std::sync::Arc<dyn gray_core::agent::ToolExecutor>| {
                    std::sync::Arc::new(DenyExecutor)
                        as std::sync::Arc<dyn gray_core::agent::ToolExecutor>
                },
            )),
        })
        .await?;
        for w in gray_plugin::builder::take_builder_warnings() {
            log::warn!(target: "gray_gateway", "{w}");
        }
        // Bash + sleep self-bound at 600 s; keep the agent timeout above them.
        Ok(agent
            .with_tool_timeout(std::time::Duration::from_secs(610))
            .with_messages(prior))
    }

    /// Run one agent turn in session `sid`, forwarding tool events to `sink`
    /// for the progress bubble. The final answer is returned, never streamed.
    /// Persists the full turn (tool calls included).
    pub(crate) async fn run_agent(
        &self,
        sid_str: &str,
        key: &str,
        text: &str,
        sink: Option<tokio::sync::mpsc::UnboundedSender<ProgressMsg>>,
    ) -> anyhow::Result<String> {
        use gray_core::Message;
        use gray_core::agent::ToolContext;
        use gray_core::event::AgentEvent;
        use gray_session::{JsonlSessionStore, SessionId, SessionMeta, default_root};

        // Serialize turns per key for the whole body (load → build → run →
        // persist → token removal): no overlapping runs, no cross-run token
        // removal, no interleaved session appends.
        let run_lock = self
            .run_locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _run_guard = run_lock.lock().await;

        let root = default_root().unwrap_or_else(|| std::path::PathBuf::from(".gray/sessions"));
        let store = JsonlSessionStore::new(root);
        let sid = SessionId::new(sid_str.to_string());

        let prior_messages: Vec<Message> = match store.load(&sid).await {
            Ok((_meta, entries)) => entries.into_iter().map(|e| e.message).collect(),
            // Only a missing session is created: corruption or I/O errors
            // must surface, not be paved over with a blank history.
            Err(gray_session::SessionError::NotFound(_)) => {
                let model = self
                    .resolve_model()
                    .unwrap_or_else(|| "unknown".to_string());
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let meta = SessionMeta::new(
                    sid.clone(),
                    chrono::Utc::now().timestamp_millis() as u64,
                    cwd,
                    model,
                );
                match store.create(meta).await {
                    Ok(_) => Vec::new(),
                    // Another writer won the race between load-NotFound and
                    // create: use their history, never a blank in-memory one.
                    Err(gray_session::SessionError::AlreadyExists(_)) => {
                        match store.load(&sid).await {
                            Ok((_meta, entries)) => {
                                entries.into_iter().map(|e| e.message).collect()
                            }
                            Err(e) => {
                                return Err(anyhow::anyhow!(
                                    "gateway cannot re-load raced session: {e:#}"
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        return Err(anyhow::anyhow!("gateway cannot create session: {e:#}"));
                    }
                }
            }
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "gateway cannot load session history: {e:#}"
                ));
            }
        };
        let prior_len = prior_messages.len();
        let mut agent = self.build_agent(prior_messages, Some(sid_str)).await?;

        // Cancel token registered under the session key so /stop and interrupts can abort.
        let token = tokio_util::sync::CancellationToken::new();
        self.cancel_tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_string(), token.clone());
        let ctx = ToolContext {
            cwd: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            cancel: token,
            session_id: Some(sid_str.to_string()),
        };
        let mut on_event = |e: &AgentEvent| {
            if let Some(tx) = &sink {
                match e {
                    AgentEvent::ToolCallStart { id, name } => {
                        let _ = tx.send(ProgressMsg::ToolStart {
                            id: id.clone(),
                            name: name.clone(),
                        });
                    }
                    AgentEvent::ToolCallEnd { id, args, .. } => {
                        let _ = tx.send(ProgressMsg::ToolEnd {
                            id: id.clone(),
                            args: args.clone(),
                        });
                    }
                    _ => {}
                }
            }
        };
        let run = agent
            .run_streaming(Message::user(text.to_string()), ctx, &mut on_event)
            .await
            .map_err(|e| anyhow::anyhow!("agent run: {e}"));
        self.cancel_tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);

        // Persist whatever the agent produced (also on cancel — partial turns are still history).
        // A shrink below the cursor means in-loop compaction ran: persist the
        // active transcript behind a boundary marker instead of skipping it.
        let to_persist = messages_to_persist(self.config.persist_redacted, agent.messages());
        let persist_error = if to_persist.len() < prior_len {
            store
                .append_compaction_replacement(&sid, &to_persist)
                .await
                .err()
        } else {
            let mut err = None;
            for m in to_persist.iter().skip(prior_len) {
                if let Err(e) = store.append(&sid, m).await {
                    err = Some(e);
                    break;
                }
            }
            err
        };
        run?;
        if let Some(e) = persist_error {
            return Err(anyhow::anyhow!("gateway cannot persist session: {e:#}"));
        }

        let mut reply = agent
            .messages()
            .iter()
            .rev()
            .find(|m| m.role == gray_core::Role::Assistant)
            .map(|m| m.text_content())
            .unwrap_or_default();
        if reply.trim().is_empty() {
            reply = "(no reply)".to_string();
        }
        Ok(reply)
    }
}

/// Plugin→host handler for gateway-spawned sidecars (`host/run`/`host/say`).
/// `host/run` replays the prompt through a fresh `gray -p` child of the
/// running binary (shared runner, no new deps); `host/say` is logged + saved
/// under `cron/output` (kept: the external cron plugin reports here).
fn cron_host_handler() -> gray_plugin::HostHandler {
    std::sync::Arc::new(move |method: String, params: serde_json::Value| {
        let fut: std::pin::Pin<Box<dyn std::future::Future<Output = serde_json::Value> + Send>> =
            Box::pin(async move {
                match method.as_str() {
                    gray_plugin::HOST_SAY => {
                        let text = params
                            .get("text")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        if !text.trim().is_empty() {
                            log::info!(target: "gray_gateway", "cron sidecar says: {text}");
                            save_cron_output("sidecar", "cron", &text);
                        }
                        serde_json::json!({"ok": true})
                    }
                    gray_plugin::HOST_RUN => {
                        // Containment: a gateway-driven nested `gray -p`
                        // child would inherit env/cwd with no executor
                        // policy, principal, cancel, or recursion fence.
                        // Re-enable only with an immutable policy snapshot
                        // and depth bound (audit 5.5).
                        serde_json::json!({
                            "error": "host/run disabled in gateway mode until policy-fenced"
                        })
                    }
                    _ => serde_json::json!({"error": format!("unknown host method {method}")}),
                }
            });
        fut
    })
}

fn save_cron_output(job_id: &str, name: &str, output: &str) {
    let Ok(home) = crate::config::gray_home_dir() else {
        return;
    };
    let dir = home.join("cron").join("output");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let path = dir.join(format!("{job_id}-{ts}.md"));
    let _ = std::fs::write(path, format!("# {name}\n\n{output}\n"));
}

// ---------------------------------------------------------------------------

fn load_system_prompt() -> String {
    let base = crate::config::gray_home_dir()
        .map(|b| b.join("AGENTS.md"))
        .unwrap_or_else(|_| std::path::PathBuf::from("AGENTS.md"));
    // migrate legacy sys.md if needed (same one-time path as lib.rs)
    if !base.exists()
        && let Some(parent) = base.parent()
    {
        let legacy = parent.join("sys.md");
        if let Ok(body) = std::fs::read_to_string(&legacy) {
            let _ = std::fs::write(&base, &body);
        }
    }
    let body = std::fs::read_to_string(&base).unwrap_or_else(|_| {
        r#"You are gray, a minimal agent running on the user's machine.
You help by using tools: read files, run commands, edit code, search.

Guidelines:
- Be concise.
- Read surrounding code, types, and tests before changing anything; match existing patterns.
- Give error and edge cases the same care as happy paths; fix root causes.
- Verify by building and testing; only claim what you actually ran.
- When referencing files or URLs in responses, format them with absolute paths or file:// links (e.g. file:///path/to/file or [label](file:///path/to/file)) and standard web URLs so they are clickable in the terminal.
- Keep going until done or truly blocked. A failed tool call means try differently, not give up."#
            .to_string()
    });
    format!(
        "{body}\n\n# Gateway mode\nYou are talking through a chat platform (Telegram/Discord/Slack), not a terminal.\n- Nobody can answer interactive prompts; destructive shell commands are auto-denied by policy — say so instead of retrying.\n- Keep replies short; long output is split into multiple messages.\n- Plain text or light markdown only; no ANSI escapes."
    )
}

/// Persist-time scrub: raw by default (exact replay fidelity), redacted when
/// the operator opts in via `gateway.yaml: persist_redacted: true`. Print
/// mode always scrubs; this closes the daemon gap (audit F4).
fn messages_to_persist(redact: bool, messages: &[gray_core::Message]) -> Vec<gray_core::Message> {
    if redact {
        messages
            .iter()
            .map(gray_core::redaction::redact_message)
            .collect()
    } else {
        messages.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gray_core::message::{ContentBlock, Message, Role};

    #[test]
    fn persist_redacted_scrubs_but_keeps_replay_shape() {
        let secret_msg = Message::new(
            Role::Assistant,
            vec![
                ContentBlock::tool_use(
                    "c1",
                    "bash",
                    serde_json::json!({"command": "curl -H \"Authorization: Bearer qqq12345\" https://x"}),
                ),
                ContentBlock::text("plain prose about /tmp/build"),
            ],
        );
        // Default: byte-identical raw persistence.
        assert_eq!(
            messages_to_persist(false, std::slice::from_ref(&secret_msg)),
            vec![secret_msg.clone()]
        );
        // Opt-in: secret scrubbed, args stay parseable JSON, prose survives.
        let scrubbed = messages_to_persist(true, std::slice::from_ref(&secret_msg));
        let ContentBlock::ToolUse { args, .. } = &scrubbed[0].content[0] else {
            panic!("expected ToolUse");
        };
        let cmd = args.get("command").and_then(|v| v.as_str()).unwrap();
        assert!(!cmd.contains("qqq12345"), "{cmd}");
        assert!(cmd.contains("<redacted>"), "{cmd}");
        assert_eq!(scrubbed[0].content[1], secret_msg.content[1]);
    }
}
