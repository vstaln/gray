//! Agent turns and cron delivery for the gateway (move-only split).
//!
//! [`GatewayRunner::run_agent`] runs one agent turn against the persisted
//! session and streams deltas to the caller; `run_cron_job` (with the
//! `cron` feature) runs a due cron job fresh and fans its output out to
//! home channels.

use crate::authz::GatedExecutor;
use crate::config::Platform;
use crate::daemon::GatewayRunner;
use crate::daemon_stream::StreamMsg;
use crate::session::{SessionSource, build_session_key};

impl GatewayRunner {
    /// Same agent every entry point builds (p2-7): provider + full
    /// registry + `gray.yml` sidecar hooks. The REPL/`-p` spelling of this
    /// lives in `gray::build_agent` (skills + cache-key pinning are
    /// interactive-only); a shared crate is still owed to truly unify them
    /// (`gray → gray-gateway` edge forbids calling it from here).
    async fn build_agent(
        &self,
        prior: Vec<gray_core::Message>,
    ) -> anyhow::Result<gray_core::agent::Agent> {
        use gray_core::agent::Agent;
        use gray_provider::OpenAiProvider;
        use gray_tools::Registry;

        let (base_url, api_key, model) = self.resolve_provider_config();
        let model = model.ok_or_else(|| {
            anyhow::anyhow!("no model configured — set ~/.gray/config.json model")
        })?;
        let api_key = api_key.unwrap_or_default();
        let base_url = base_url.unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string());
        let provider = OpenAiProvider::builder(&api_key, &model)
            .base_url(&base_url)
            .build()
            .map_err(|e| anyhow::anyhow!("provider init: {e}"))?;

        let registry = Registry::builtin();
        // Advertise the full registry: denials belong to GatedExecutor so the
        // model gets the gate's accurate reason instead of "does not exist".
        let tool_defs = registry.defs();
        let executor = GatedExecutor::new(Box::new(registry), self.config.denied_tools.clone());

        // Sidecar hooks from the same `gray.yml` profile the REPL reads;
        // spawn failures warn + continue (the daemon must stay up).
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let mut plugins: Vec<std::sync::Arc<dyn gray_plugin::Plugin>> = Vec::new();
        match gray_plugin::profile::load_entries("gray.yml") {
            Ok(entries) => {
                for e in &entries {
                    if let gray_plugin::profile::PluginEntry::Sidecar(spec) = e {
                        match gray_plugin::sidecar::SidecarPlugin::spawn(spec.0.clone()).await {
                            Ok(p) => plugins.push(std::sync::Arc::new(p)),
                            Err(err) => log::warn!(target: "gray_gateway", "sidecar spawn failed, skipping: {err:#}"),
                        }
                    }
                }
            }
            Err(_) => {}
        }
        let hooks =
            gray_plugin::PluginHookAdapter::for_plugins(&plugins, &cwd.to_string_lossy());

        Ok(Agent::new(Box::new(provider), Box::new(executor))
            .with_system(load_system_prompt())
            .with_tools(tool_defs)
            .with_hooks(hooks)
            .with_messages(prior))
    }

    /// Run one agent turn in session `sid`, streaming text deltas to `sink`.
    /// Returns the final assistant text. Persists the full turn (tool calls included).
    pub(crate) async fn run_agent(
        &self,
        sid_str: &str,
        key: &str,
        text: &str,
        sink: Option<tokio::sync::mpsc::UnboundedSender<StreamMsg>>,
    ) -> anyhow::Result<String> {
        use gray_core::Message;
        use gray_core::agent::ToolContext;
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
        let mut agent = self.build_agent(prior_messages).await?;

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
        };
        let mut on_event = |e: &AgentEvent| {
            if let Some(tx) = &sink {
                match e {
                    AgentEvent::TextDelta { delta } => {
                        let _ = tx.send(StreamMsg::Delta(delta.clone()));
                    }
                    // Text before a tool call is narration; the reply is what comes after.
                    AgentEvent::ToolResult { .. } => {
                        let _ = tx.send(StreamMsg::Reset);
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
