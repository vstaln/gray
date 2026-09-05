//! Agent turns and cron delivery for the gateway (move-only split).
//!
//! [`GatewayRunner::run_agent`] runs one agent turn against the persisted
//! session and streams deltas to the caller; `run_cron_job` (with the
//! `cron` feature) runs a due cron job fresh and fans its output out to
//! home channels.

use crate::authz::GatedExecutor;
use crate::config::Platform;
use crate::daemon::GatewayRunner;
use crate::daemon_stream::ProgressMsg;
use crate::session::{SessionSource, build_session_key};

impl GatewayRunner {
    /// Same agent every entry point builds: thin surface wrapper over
    /// [`gray_plugin::builder::build_agent`] (the single profile-aware
    /// builder for REPL, `-p`, gateway, and cron — F8 resolved the `gray →
    /// gray-gateway` cycle by moving it to the lowest common crate).
    /// Surface policy owned here: provider config, gateway system prompt,
    /// [`GatedExecutor`] wrapping, and warn-and-skip on sidecar spawn
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
        let host_handler = cron_host_handler(cwd.clone());
        let denied = self.config.denied_tools.clone();
        let agent = gray_plugin::builder::build_agent(gray_plugin::builder::BuilderOptions {
            model,
            api_key: api_key.unwrap_or_default(),
            base_url: base_url.unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string()),
            reasoning_effort: None,
            session_id: session_id.map(str::to_string),
            cwd,
            system_prompt: gray_plugin::builder::SystemPrompt::Literal(load_system_prompt()),
            extra_tools: Vec::new(),
            host_handler: Some(host_handler),
            profile_path: "gray.yml".to_string(),
            abort_on_spawn_failure: false,
            // Advertise the full registry: denials belong to GatedExecutor so the
            // model gets the gate's accurate reason instead of "does not exist".
            wrap_executor: Some(Box::new(
                move |inner: Box<dyn gray_core::agent::ToolExecutor>| {
                    Box::new(GatedExecutor::new(inner, denied))
                        as Box<dyn gray_core::agent::ToolExecutor>
                },
            )),
        })
        .await?;
        for w in gray_plugin::builder::take_builder_warnings() {
            log::warn!(target: "gray_gateway", "{w}");
        }
        Ok(agent.with_messages(prior))
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
        use gray_core::agent::{PermissionMode, ToolContext};
        use gray_core::event::AgentEvent;
        use gray_session::{JsonlSessionStore, SessionId, SessionMeta, default_root};

        let root = default_root().unwrap_or_else(|| std::path::PathBuf::from(".gray/sessions"));
        let store = JsonlSessionStore::new(root);
        let sid = SessionId::new(sid_str.to_string());

        let prior_messages: Vec<Message> = match store.load(&sid).await {
            Ok((_meta, entries)) => entries.into_iter().map(|e| e.message).collect(),
            Err(_) => {
                let model = self
                    .resolve_model()
                    .unwrap_or_else(|| "unknown".to_string());
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let meta = SessionMeta::new(sid.clone(), now_millis(), cwd, model);
                let _ = store.create(meta).await;
                Vec::new()
            }
        };
        let prior_len = prior_messages.len();
        let mut agent = self.build_agent(prior_messages, Some(sid_str)).await?;

        // Cancel token registered under the session key so /stop and interrupts can abort.
        let token = tokio_util::sync::CancellationToken::new();
        self.cancel_tokens
            .lock()
            .unwrap()
            .insert(key.to_string(), token.clone());
        let ctx = ToolContext {
            cwd: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            cancel: token,
            questions: None, // no interactive user → request_user_input is denied anyway
            session_id: Some(sid_str.to_string()),
            permission: PermissionMode::resolve(false),
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
        self.cancel_tokens.lock().unwrap().remove(key);

        // Persist whatever the agent produced (also on cancel — partial turns are still history).
        for m in agent.messages().iter().skip(prior_len) {
            let _ = store.append(&sid, m).await;
        }
        run?;

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

    /// Run a due cron job through the agent and deliver its output to every
    /// platform's home channel. Output is
    /// also saved under `~/.gray/cron/output/` so nothing is lost when no
    /// home channel is configured.
    #[cfg(feature = "cron")]
    pub async fn run_cron_job(&self, job: &gray_cron::CronJob) {
        // Session keyed through build_session_key (never hand-built): the
        // "platform" is the first home-channel platform, chat_type "cron".
        let platform = Platform::ALL
            .into_iter()
            .find(|p| self.adapters.contains_key(p) && self.router.home_channel(*p).is_some());
        let src = SessionSource {
            platform: platform.unwrap_or(Platform::Telegram),
            chat_id: job.id.clone(),
            chat_type: "cron".into(),
            user_id: None,
            thread_id: None,
            scope_id: None,
            message_id: None,
        };
        let key = build_session_key(&src, false, false);
        // Cron jobs start fresh each run (isolated per-run session).
        let sid = self.store.reset(&key);
        log::info!("gateway cron '{}' ({}) running as {sid}", job.name, job.id);
        let output = match self.run_agent(&sid, &key, &job.prompt, None).await {
            Ok(t) => t,
            Err(e) => format!("cron job '{}' failed: {e}", job.name),
        };
        save_cron_output(&job.id, &job.name, &output);
        let text = format!("⏰ {}\n\n{}", job.name, output);
        if platform.is_none() {
            log::info!(
                "gateway cron '{}' done (no home_channel; output saved locally)",
                job.name
            );
            return;
        }
        for (p, r) in self.router.deliver_home_all(&text).await {
            if r.success {
                log::info!("gateway cron '{}' delivered to {p} home", job.name);
            } else {
                log::warn!(
                    "gateway cron '{}' delivery to {p} failed: {:?}",
                    job.name,
                    r.error
                );
            }
        }
    }
}

/// Plugin→host handler for gateway-spawned sidecars (`host/run`/`host/say`).
/// `host/run` replays the prompt through a fresh `gray -p` child of the
/// running binary (shared runner, no new deps); `host/say` is logged + saved
/// under `cron/output`. Home-channel fan-out stays with the legacy
/// `run_cron_job` path until the cron sidecar goes persistent (owed — the
/// per-turn sidecars here live only for the turn, so the `daemon_boot`
/// ticker remains the primary gateway firer; all tickers claim atomically).
fn cron_host_handler(cwd: std::path::PathBuf) -> gray_plugin::HostHandler {
    std::sync::Arc::new(move |method: String, params: serde_json::Value| {
        let cwd = cwd.clone();
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
                        let prompt = params
                            .get("prompt")
                            .and_then(|p| p.as_str())
                            .unwrap_or("")
                            .to_string();
                        if prompt.trim().is_empty() {
                            return serde_json::json!({"error": "host/run: missing prompt"});
                        }
                        gray_plugin::host::run_prompt_child(&cwd, &prompt).await
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

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
