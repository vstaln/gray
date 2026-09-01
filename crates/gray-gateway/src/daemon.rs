use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use crate::config::{GatewayConfig, Platform, load_gateway_config};
use crate::platform::{BasePlatformAdapter, MessageEvent, SendResult, truncate_message};
use crate::session::{build_session_key, shared_store, FileGatewayStore};

use crate::telegram::TelegramAdapter;
use crate::discord::DiscordAdapter;
use crate::slack::SlackAdapter;

type Adapter = Arc<dyn BasePlatformAdapter>;

pub struct GatewayRunner {
    pub config: GatewayConfig,
    pub adapters: HashMap<Platform, Adapter>,
    pub store: Arc<FileGatewayStore>,
    /// Per-session cancellation for /stop (hermes parity).
    cancel_tokens: Mutex<HashMap<String, tokio_util::sync::CancellationToken>>,
}

impl GatewayRunner {
    pub fn from_config(config: GatewayConfig) -> anyhow::Result<Self> {
        let store = shared_store();
        let mut adapters: HashMap<Platform, Adapter> = HashMap::new();
        for (plat, cfg) in &config.platforms {
            if !cfg.enabled { continue; }
            let adapter: Adapter = match plat {
                Platform::Telegram => Arc::new(TelegramAdapter::new(cfg.clone())?),
                Platform::Discord => Arc::new(DiscordAdapter::new(cfg.clone())?),
                Platform::Slack => Arc::new(SlackAdapter::new(cfg.clone())?),
            };
            adapters.insert(*plat, adapter);
        }
        Ok(Self { config, adapters, store, cancel_tokens: Mutex::new(HashMap::new()) })
    }

    /// Handle inbound MessageEvent: resolve session -> run Agent -> send reply.
    /// Intercepts the hermes slash commands /reset /status /stop.
    pub async fn handle_inbound(&self, ev: MessageEvent) -> anyhow::Result<SendResult> {
        let platform = ev.source.platform;
        let chat_id = ev.source.chat_id.clone();
        let key = build_session_key(&ev.source, self.config.group_per_user, self.config.thread_per_user);
        log::info!("gateway inbound {platform} chat={chat_id} key={key} text={:?}", &ev.text[..ev.text.len().min(80)]);

        let reply_text = match ev.text.trim() {
            "/reset" => {
                let sid = self.store.reset(&key);
                format!("Session reset ({sid}).")
            }
            "/status" => {
                let sid = self.store.get(&key).unwrap_or_else(|| "(none yet)".into());
                format!(
                    "session {sid}\nmodel {}\ngroup_per_user={}",
                    self.resolve_model().unwrap_or_else(|| "unconfigured".into()),
                    self.config.group_per_user
                )
            }
            "/stop" => {
                let sid = self.store.get(&key).unwrap_or_default();
                let stopped = self.cancel_tokens.lock().unwrap().get(&sid).map(|t| t.cancel()).is_some();
                if stopped { "Stop requested~".into() } else { "Nothing running.".into() }
            }
            text => {
                let sid_str = self.store.get_or_create(&key, &ev.source);
                // Typing indicator while the agent works (hermes persistent typing, 8s loop).
                let done = Arc::new(tokio::sync::Notify::new());
                let typing_task = self.adapters.get(&platform).map(|a| {
                    let (a, chat, d) = (Arc::clone(a), chat_id.clone(), Arc::clone(&done));
                    tokio::spawn(async move {
                        loop {
                            tokio::select! {
                                _ = d.notified() => break,
                                _ = tokio::time::sleep(Duration::from_secs(8)) => a.send_typing(&chat).await,
                            }
                        }
                    })
                });

                let result = self.run_agent_for_event(&sid_str, text, &ev).await;
                done.notify_one();
                if let Some(t) = typing_task { let _ = t.await; }
                match result {
                    Ok(t) => t,
                    Err(e) => {
                        log::warn!("gateway agent error: {e}");
                        format!("error: {e}")
                    }
                }
            }
        };

        let max = match platform { Platform::Telegram => 4096, Platform::Discord => 2000, Platform::Slack => 39000 };
        let truncated = truncate_message(&reply_text, max);

        if let Some(adapter) = self.adapters.get(&platform) {
            let res = adapter.send(&chat_id, &truncated).await;
            if !res.success { log::warn!("gateway send failed: {:?}", res.error); }
            Ok(res)
        } else {
            log::warn!("no adapter for {platform}, dropping reply");
            Ok(SendResult { success: false, message_id: None, error: Some(format!("no adapter for {platform}")), retryable: false })
        }
    }

    async fn run_agent_for_event(&self, sid_str: &str, text: &str, _ev: &MessageEvent) -> anyhow::Result<String> {
        use gray_core::Message;
        use gray_core::agent::{Agent, ToolContext};
        use gray_provider::OpenAiProvider;
        use gray_session::{SessionId, JsonlSessionStore, SessionMeta, default_root};
        use gray_tools::Registry;

        let root = default_root().unwrap_or_else(|| std::path::PathBuf::from(".gray/sessions"));
        let store = JsonlSessionStore::new(root);
        let sid = SessionId::new(sid_str.to_string());

        let prior_messages: Vec<Message> = match store.load(&sid).await {
            Ok((_meta, entries)) => entries.into_iter().map(|e| e.message).collect(),
            Err(_) => {
                let model = self.resolve_model().unwrap_or_else(|| "unknown".to_string());
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let meta = SessionMeta::new(sid.clone(), now_millis(), cwd, model);
                let _ = store.create(meta).await;
                Vec::new()
            }
        };

        let (base_url, api_key, model) = self.resolve_provider_config();

        let model = model.ok_or_else(|| anyhow::anyhow!("no model configured — set ~/.gray/config.json model"))?;
        let api_key = api_key.unwrap_or_default();
        let base_url = base_url.unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string());

        let provider = OpenAiProvider::builder(&api_key, &model).base_url(&base_url).build()
            .map_err(|e| anyhow::anyhow!("provider init: {e}"))?;
        let registry = Registry::builtin();
        let tool_defs = registry.defs();

        let system = load_system_prompt();

        let mut agent = Agent::new(Box::new(provider), Box::new(registry))
            .with_system(system)
            .with_tools(tool_defs)
            .with_messages(prior_messages);

        // Cancel token registered so /stop can abort the run (hermes parity).
        let token = tokio_util::sync::CancellationToken::new();
        self.cancel_tokens.lock().unwrap().insert(sid_str.to_string(), token.clone());
        let ctx = ToolContext { cancel: token, ..Default::default() };
        let run = agent.run(Message::user(text.to_string()), ctx).await
            .map_err(|e| anyhow::anyhow!("agent run: {e}"));
        self.cancel_tokens.lock().unwrap().remove(sid_str);
        run?;

        let mut reply = String::new();
        if let Some(last) = agent.messages().iter().rev().find(|m| m.role == gray_core::Role::Assistant) {
            reply = last.text_content();
        }
        if reply.is_empty() { reply = "(no reply)".to_string(); }

        // persist turn
        let _ = store.append(&sid, &Message::user(text.to_string())).await;
        let _ = store.append(&sid, &Message::assistant(reply.clone())).await;

        Ok(reply)
    }

    fn resolve_model(&self) -> Option<String> {
        let (_, _, m) = self.resolve_provider_config();
        m
    }

    fn resolve_provider_config(&self) -> (Option<String>, Option<String>, Option<String>) {
        let saved = load_saved_config();
        let base_url = std::env::var("GRAY_BASE_URL").ok().or(saved.as_ref().and_then(|s| s.base_url.clone()));
        let api_key = std::env::var("GRAY_API_KEY").ok().or_else(|| std::env::var("OPENAI_API_KEY").ok()).or(saved.as_ref().and_then(|s| s.api_key.clone()));
        let model = std::env::var("GRAY_MODEL").ok().or(saved.as_ref().and_then(|s| s.model.clone()));
        (base_url, api_key, model)
    }
}

fn load_saved_config() -> Option<SavedConfig> {
    let base = std::env::var("GRAY_HOME").or_else(|_| std::env::var("HOME").map(|h| format!("{h}/.gray"))).ok()?;
    let path = std::path::PathBuf::from(base).join("config.json");
    std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok())
}

#[derive(serde::Deserialize)]
struct SavedConfig { base_url: Option<String>, api_key: Option<String>, model: Option<String> }

fn load_system_prompt() -> String {
    let base = std::env::var("GRAY_HOME").or_else(|_| std::env::var("HOME").map(|h| format!("{h}/.gray"))).map(|b| std::path::PathBuf::from(b).join("AGENTS.md")).unwrap_or_else(|_| std::path::PathBuf::from("AGENTS.md"));
    // migrate legacy sys.md if needed (same one-time path as lib.rs)
    if !base.exists() {
        if let Some(parent) = base.parent() {
            let legacy = parent.join("sys.md");
            if let Ok(body) = std::fs::read_to_string(&legacy) {
                let _ = std::fs::write(&base, &body);
            }
        }
    }
    std::fs::read_to_string(&base).unwrap_or_else(|_| r#"You are gray, a minimal agent running on the user's machine.
You help by using tools: read files, run commands, edit code, search.

Guidelines:
- Be concise.
- Read surrounding code, types, and tests before changing anything; match existing patterns.
- Give error and edge cases the same care as happy paths; fix root causes.
- Verify by building and testing; only claim what you actually ran.
- When referencing files or URLs in responses, format them with absolute paths or file:// links (e.g. file:///path/to/file or [label](file:///path/to/file)) and standard web URLs so they are clickable in the terminal.
- Keep going until done or truly blocked. A failed tool call means try differently, not give up."#.to_string())
}

fn now_millis() -> u64 { std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0) }

pub async fn run_gateway() -> anyhow::Result<()> {
    let cfg = load_gateway_config();
    let mut runner = GatewayRunner::from_config(cfg)?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<MessageEvent>();
    for adapter in runner.adapters.values_mut() {
        if let Some(a) = Arc::get_mut(adapter) { a.set_event_tx(tx.clone()); }
    }
    for (plat, adapter) in runner.adapters.iter_mut() {
        let res = tokio::time::timeout(Duration::from_secs(45), adapter.connect()).await;
        match res {
            Ok(Ok(())) => log::info!("gateway {plat} connected"),
            Ok(Err(e)) => log::warn!("gateway {plat} connect failed: {e}"),
            Err(_) => log::warn!("gateway {plat} connect timeout 45s"),
        }
    }
    if runner.adapters.is_empty() { anyhow::bail!("no gateway platforms enabled — edit ~/.gray/gateway.yaml"); }

    let runner = Arc::new(runner);
    // Agent futures are !Send (gray-core run_streaming sink), so handle events on a
    // dedicated LocalSet thread; spawn_local per event keeps /stop responsive mid-run.
    let _worker = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("gateway runtime");
        rt.block_on(tokio::task::LocalSet::new().run_until(async move {
            while let Some(ev) = rx.recv().await {
                let r = Arc::clone(&runner);
                tokio::task::spawn_local(async move {
                    if let Err(e) = r.handle_inbound(ev).await { log::warn!("gateway handle error: {e}"); }
                });
            }
        }));
    });

    #[cfg(unix)] {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        tokio::select! { _ = sigterm.recv() => {}, _ = sigint.recv() => {} }
    }
    #[cfg(not(unix))] { tokio::signal::ctrl_c().await?; }
    // ponytail: no graceful shutdown — adapters hold event_tx clones so rx never
    // closes; process exit reaps the worker thread. Add drain if restart-in-process lands.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{GatewayConfig, Platform, PlatformConfig};
    use std::collections::HashMap;
    #[test]
    fn from_config_with_dummy_token() {
        let mut platforms = HashMap::new();
        platforms.insert(Platform::Telegram, PlatformConfig{ enabled: true, token: Some("123456:ABCDEFGHIJ1234567890".into()), app_token: None, home_channel: None });
        let cfg = GatewayConfig{ platforms, group_per_user: true, thread_per_user: false };
        let runner = GatewayRunner::from_config(cfg).unwrap();
        assert!(runner.adapters.contains_key(&Platform::Telegram));
    }
    #[test]
    fn from_config_skips_disabled() {
        let mut platforms = HashMap::new();
        platforms.insert(Platform::Telegram, PlatformConfig{ enabled: false, token: Some("x".into()), app_token: None, home_channel: None });
        let cfg = GatewayConfig{ platforms, ..Default::default() };
        let runner = GatewayRunner::from_config(cfg).unwrap();
        assert!(runner.adapters.is_empty());
    }
}
