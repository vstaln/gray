use std::collections::HashMap;
use std::sync::Arc;
use crate::config::{GatewayConfig, Platform, load_gateway_config};
use crate::platform::{BasePlatformAdapter, MessageEvent, SendResult, truncate_message};
use crate::session::{build_session_key, shared_store, GatewaySessionStore};

use crate::telegram::TelegramAdapter;
use crate::discord::DiscordAdapter;
use crate::slack::SlackAdapter;

pub struct GatewayRunner { pub config: GatewayConfig, pub adapters: HashMap<Platform, Box<dyn BasePlatformAdapter>>, pub store: Arc<dyn GatewaySessionStore> }

impl GatewayRunner {
    pub fn from_config(config: GatewayConfig) -> anyhow::Result<Self> {
        let store = shared_store();
        let mut adapters: HashMap<Platform, Box<dyn BasePlatformAdapter>> = HashMap::new();
        for (plat, cfg) in &config.platforms {
            if !cfg.enabled { continue; }
            let adapter: Box<dyn BasePlatformAdapter> = match plat {
                Platform::Telegram => Box::new(TelegramAdapter::new(cfg.clone())?),
                Platform::Discord => Box::new(DiscordAdapter::new(cfg.clone())?),
                Platform::Slack => Box::new(SlackAdapter::new(cfg.clone())?),
            };
            adapters.insert(*plat, adapter);
        }
        Ok(Self{config, adapters, store})
    }

    pub async fn start(&mut self) -> anyhow::Result<()> {
        for (plat, adapter) in self.adapters.iter_mut() {
            let res = tokio::time::timeout(std::time::Duration::from_secs(45), adapter.connect()).await;
            match res {
                Ok(Ok(())) => log::info!("gateway {plat} connected"),
                Ok(Err(e)) => log::warn!("gateway {plat} connect failed: {e}"),
                Err(_) => log::warn!("gateway {plat} connect timeout 45s"),
            }
        }
        if self.adapters.is_empty() { anyhow::bail!("no gateway platforms enabled — edit ~/.gray/gateway.yaml"); }
        #[cfg(unix)] {
            let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
            tokio::select! { _ = sigterm.recv() => {}, _ = sigint.recv() => {} }
        }
        #[cfg(not(unix))] { tokio::signal::ctrl_c().await?; }
        self.stop().await
    }

    pub async fn stop(&mut self) -> anyhow::Result<()> {
        for (_, a) in self.adapters.iter_mut() { let _ = a.disconnect().await; }
        Ok(())
    }

    /// Handle inbound MessageEvent: resolve session -> run Agent -> send reply with truncation.
    pub async fn handle_inbound(&self, ev: MessageEvent) -> anyhow::Result<SendResult> {
        let platform = ev.source.platform;
        let chat_id = ev.source.chat_id.clone();
        let key = build_session_key(&ev.source, self.config.group_per_user, self.config.thread_per_user);
        let sid_str = self.store.get_or_create(&key, &ev.source);
        log::info!("gateway inbound {platform} chat={chat_id} key={key} sid={sid_str} text_len={}", ev.text.len());

        // Session store: load existing messages or create new session file
        let reply_text = match self.run_agent_for_event(&sid_str, &ev).await {
            Ok(t) => t,
            Err(e) => {
                log::warn!("gateway agent error: {e}");
                format!("error: {e}")
            }
        };

        let max = match platform { Platform::Telegram => 4096, Platform::Discord => 2000, Platform::Slack => 39000 };
        let truncated = truncate_message(&reply_text, max);

        // Find adapter to send back
        if let Some(adapter) = self.adapters.get(&platform) {
            // chunk if still over? truncate_message already cuts, but telegram needs split
            // ponytail: single truncation is enough for daemon wiring
            let res = adapter.send(&chat_id, &truncated).await;
            if !res.success { log::warn!("gateway send failed: {:?}", res.error); }
            Ok(res)
        } else {
            log::warn!("no adapter for {platform}, dropping reply");
            Ok(SendResult{ success: false, message_id: None, error: Some(format!("no adapter for {platform}")), retryable: false })
        }
    }

    async fn run_agent_for_event(&self, sid_str: &str, ev: &MessageEvent) -> anyhow::Result<String> {
        use gray_core::Message;
        use gray_core::agent::{Agent, ToolContext};
        use gray_provider::OpenAiProvider;
        use gray_session::{SessionId, SessionStore, JsonlSessionStore, SessionMeta, default_root};
        use gray_tools::Registry;

        // Resolve session history
        let root = default_root().unwrap_or_else(|| std::path::PathBuf::from(".gray/sessions"));
        let store = JsonlSessionStore::new(root);
        let sid = SessionId::new(sid_str.to_string());

        let prior_messages: Vec<Message> = match store.load(&sid).await {
            Ok((_meta, entries)) => entries.into_iter().map(|e| e.message).collect(),
            Err(_) => {
                // create new session file
                let model = self.resolve_model().unwrap_or_else(|| "unknown".to_string());
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let meta = SessionMeta::new(sid.clone(), now_millis(), cwd, model);
                let _ = store.create(meta).await;
                Vec::new()
            }
        };

        // Build Config from GRAY_HOME/config.json + env (mirrors gray::config::Config but local)
        let (base_url, api_key, model) = self.resolve_provider_config();

        let model = model.ok_or_else(|| anyhow::anyhow!("no model configured — set ~/.gray/config.json model"))?;
        let api_key = api_key.unwrap_or_default();
        let base_url = base_url.unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string());

        let provider = OpenAiProvider::builder(&api_key, &model).base_url(&base_url).build()
            .map_err(|e| anyhow::anyhow!("provider init: {e}"))?;
        let registry = Registry::builtin();
        let tool_defs = registry.defs();

        // System prompt: try GRAY_HOME/sys.md else default
        let system = load_system_prompt();

        let mut agent = Agent::new(Box::new(provider), Box::new(registry))
            .with_system(system)
            .with_tools(tool_defs)
            .with_messages(prior_messages);

        let ctx = ToolContext{ cwd: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")), cancel: tokio_util::sync::CancellationToken::new() };
        let events = agent.run(Message::user(ev.text.clone()), ctx).await
            .map_err(|e| anyhow::anyhow!("agent run: {e}"))?;

        // Extract last assistant text
        let mut reply = String::new();
        for ev in events.iter().rev() {
            if let gray_core::event::AgentEvent::TurnEnd{..} = ev { break; }
        }
        // simpler: last assistant message
        if let Some(last) = agent.messages().iter().rev().find(|m| m.role == gray_core::Role::Assistant) {
            reply = last.text_content();
        }
        if reply.is_empty() { reply = "(no reply)".to_string(); }

        // persist turn
        let _ = store.append(&sid, &Message::user(ev.text.clone())).await;
        let _ = store.append(&sid, &Message::assistant(reply.clone())).await;

        Ok(reply)
    }

    fn resolve_model(&self) -> Option<String> {
        let (_, _, m) = self.resolve_provider_config();
        m
    }

    fn resolve_provider_config(&self) -> (Option<String>, Option<String>, Option<String>) {
        // Try env then ~/.gray/config.json
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
    let base = std::env::var("GRAY_HOME").or_else(|_| std::env::var("HOME").map(|h| format!("{h}/.gray"))).map(|b| std::path::PathBuf::from(b).join("sys.md")).unwrap_or_else(|_| std::path::PathBuf::from("sys.md"));
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
    runner.start().await
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
