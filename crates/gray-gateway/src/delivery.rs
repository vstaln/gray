//! Delivery routing (hermes `gateway/delivery.py`).
//!
//! Targets:
//! - `origin`                → back to the originating chat (reply-to + thread preserved)
//! - `telegram` / `slack`…  → that platform's `home_channel`
//! - `telegram:123456`       → explicit chat; `slack:C123:1700.1` adds a thread
//!
//! [`DeliveryRouter`] fans a message out through live adapters; [`send_once`]
//! builds a throw-away send-only adapter for the `gray send` CLI.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::config::{GatewayConfig, Platform};
use crate::platform::{BasePlatformAdapter, SendOptions, SendResult};
use crate::session::SessionSource;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryTarget {
    pub platform: Platform,
    /// `None` → use the platform's `home_channel`.
    pub chat_id: Option<String>,
    pub thread_id: Option<String>,
    pub is_origin: bool,
}

impl DeliveryTarget {
    /// Parse a target string. `origin` requires an originating source.
    pub fn parse(target: &str, origin: Option<&SessionSource>) -> anyhow::Result<Self> {
        let t = target.trim();
        if t.is_empty() {
            anyhow::bail!("empty delivery target");
        }
        if t.eq_ignore_ascii_case("origin") {
            let Some(o) = origin else { anyhow::bail!("`origin` target needs an originating message") };
            return Ok(Self { platform: o.platform, chat_id: Some(o.chat_id.clone()), thread_id: o.thread_id.clone(), is_origin: true });
        }
        let mut parts = t.splitn(3, ':');
        let plat = parts.next().unwrap_or_default();
        let platform: Platform = plat.parse().map_err(|_| anyhow::anyhow!("unknown platform {plat:?} in target {t:?} (telegram|discord|slack)"))?;
        let chat_id = parts.next().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        let thread_id = parts.next().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        Ok(Self { platform, chat_id, thread_id, is_origin: false })
    }

    pub fn to_target_string(&self) -> String {
        if self.is_origin {
            return "origin".into();
        }
        match (&self.chat_id, &self.thread_id) {
            (Some(c), Some(t)) => format!("{}:{c}:{t}", self.platform),
            (Some(c), None) => format!("{}:{c}", self.platform),
            _ => self.platform.to_string(),
        }
    }
}

pub struct DeliveryRouter {
    config: GatewayConfig,
    adapters: HashMap<Platform, Arc<dyn BasePlatformAdapter>>,
}

impl DeliveryRouter {
    pub fn new(config: GatewayConfig, adapters: HashMap<Platform, Arc<dyn BasePlatformAdapter>>) -> Self {
        Self { config, adapters }
    }

    pub fn home_channel(&self, platform: Platform) -> Option<String> {
        self.config
            .platforms
            .get(&platform)
            .and_then(|c| c.home_channel.clone())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Resolve the concrete chat for a target (explicit or home channel).
    pub fn resolve_chat(&self, target: &DeliveryTarget) -> anyhow::Result<String> {
        if let Some(c) = &target.chat_id {
            return Ok(c.clone());
        }
        self.home_channel(target.platform)
            .ok_or_else(|| anyhow::anyhow!("no home_channel configured for {} (set platforms.{}.home_channel)", target.platform, target.platform))
    }

    /// Deliver `text` to `target`. Chunking is the adapter's job.
    pub async fn deliver(&self, target: &DeliveryTarget, text: &str, reply_to: Option<&str>) -> SendResult {
        let Some(adapter) = self.adapters.get(&target.platform) else {
            return SendResult::fail(format!("no live adapter for {}", target.platform), false);
        };
        let chat = match self.resolve_chat(target) {
            Ok(c) => c,
            Err(e) => return SendResult::fail(e.to_string(), false),
        };
        let opts = SendOptions { reply_to: reply_to.map(str::to_string), thread_id: target.thread_id.clone() };
        let res = tokio::time::timeout(Duration::from_secs(60), adapter.send_ext(&chat, text, &opts)).await;
        match res {
            Ok(r) => r,
            Err(_) => SendResult::fail(format!("delivery to {} timed out", target.to_target_string()), true),
        }
    }

    /// Home-channel broadcast (cron output, boot notices): every enabled
    /// platform with a `home_channel`. Returns per-platform results.
    pub async fn deliver_home_all(&self, text: &str) -> Vec<(Platform, SendResult)> {
        let mut out = Vec::new();
        for plat in self.adapters.keys().copied() {
            if self.home_channel(plat).is_none() {
                continue;
            }
            let target = DeliveryTarget { platform: plat, chat_id: None, thread_id: None, is_origin: false };
            let r = self.deliver(&target, text, None).await;
            out.push((plat, r));
        }
        out
    }
}

/// Build an adapter for `platform` from config (no event channel → send-only).
pub fn build_adapter(config: &GatewayConfig, platform: Platform) -> anyhow::Result<Arc<dyn BasePlatformAdapter>> {
    let pc = config
        .platforms
        .get(&platform)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("{platform} is not configured in gateway.yaml"))?;
    let a: Arc<dyn BasePlatformAdapter> = match platform {
        Platform::Telegram => Arc::new(crate::telegram::TelegramAdapter::new(pc)?),
        Platform::Discord => Arc::new(crate::discord::DiscordAdapter::new(pc)?),
        Platform::Slack => Arc::new(crate::slack::SlackAdapter::new(pc)?),
    };
    Ok(a)
}

/// One-shot delivery for scripts: `gray send telegram:123 "text"`.
/// Connects a send-only adapter, sends, disconnects. Uses the persisted
/// gateway.yaml token, so the daemon does not need to be running.
pub async fn send_once(config: &GatewayConfig, target: &str, text: &str) -> anyhow::Result<()> {
    let target = DeliveryTarget::parse(target, None)?;
    if text.trim().is_empty() {
        anyhow::bail!("refusing to send an empty message");
    }
    let adapter = build_adapter(config, target.platform)?;
    tokio::time::timeout(Duration::from_secs(30), adapter.connect())
        .await
        .map_err(|_| anyhow::anyhow!("{} connect timed out", target.platform))??;
    let mut adapters = HashMap::new();
    adapters.insert(target.platform, Arc::clone(&adapter));
    let router = DeliveryRouter::new(config.clone(), adapters);
    let res = router.deliver(&target, text, None).await;
    let _ = adapter.disconnect().await;
    if res.success {
        Ok(())
    } else {
        anyhow::bail!("send failed: {}", res.error.unwrap_or_else(|| "unknown error".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PlatformConfig;

    fn src() -> SessionSource {
        SessionSource {
            platform: Platform::Slack,
            chat_id: "C1".into(),
            chat_type: "channel".into(),
            user_id: Some("U1".into()),
            thread_id: Some("17.1".into()),
            scope_id: Some("T1".into()),
            message_id: Some("17.2".into()),
        }
    }

    #[test]
    fn parse_targets() {
        let t = DeliveryTarget::parse("telegram:123", None).unwrap();
        assert_eq!(t, DeliveryTarget { platform: Platform::Telegram, chat_id: Some("123".into()), thread_id: None, is_origin: false });
        assert_eq!(t.to_target_string(), "telegram:123");
        let t = DeliveryTarget::parse("Slack:C1:17.5", None).unwrap();
        assert_eq!(t.thread_id.as_deref(), Some("17.5"));
        assert_eq!(t.to_target_string(), "slack:C1:17.5");
        let t = DeliveryTarget::parse("discord", None).unwrap();
        assert!(t.chat_id.is_none());
        assert_eq!(t.to_target_string(), "discord");
        assert!(DeliveryTarget::parse("matrix:1", None).is_err());
        assert!(DeliveryTarget::parse("", None).is_err());
        assert!(DeliveryTarget::parse("origin", None).is_err());
        let o = DeliveryTarget::parse("origin", Some(&src())).unwrap();
        assert!(o.is_origin);
        assert_eq!(o.chat_id.as_deref(), Some("C1"));
        assert_eq!(o.thread_id.as_deref(), Some("17.1"));
    }

    fn cfg_with_home() -> GatewayConfig {
        let mut cfg = GatewayConfig::default();
        cfg.platforms.insert(
            Platform::Telegram,
            PlatformConfig { home_channel: Some("555".into()), ..PlatformConfig::with_token("123456:ABCDEFGHIJ1234567890") },
        );
        cfg.platforms.insert(Platform::Discord, PlatformConfig::with_token(&"x".repeat(40)));
        cfg
    }

    #[tokio::test]
    async fn router_resolves_home_and_explicit() {
        let cfg = cfg_with_home();
        let mut adapters: HashMap<Platform, Arc<dyn BasePlatformAdapter>> = HashMap::new();
        adapters.insert(Platform::Telegram, build_adapter(&cfg, Platform::Telegram).unwrap());
        adapters.insert(Platform::Discord, build_adapter(&cfg, Platform::Discord).unwrap());
        let router = DeliveryRouter::new(cfg, adapters);
        let home = DeliveryTarget::parse("telegram", None).unwrap();
        assert_eq!(router.resolve_chat(&home).unwrap(), "555");
        let explicit = DeliveryTarget::parse("telegram:1", None).unwrap();
        assert_eq!(router.resolve_chat(&explicit).unwrap(), "1");
        // Discord has no home channel: error, not a silent drop.
        let dh = DeliveryTarget::parse("discord", None).unwrap();
        assert!(router.resolve_chat(&dh).is_err());
        let r = router.deliver(&dh, "hi", None).await;
        assert!(!r.success);
        // Unknown platform adapter.
        let sl = DeliveryTarget::parse("slack:C1", None).unwrap();
        assert!(!router.deliver(&sl, "hi", None).await.success);
        // Home broadcast only hits platforms with a home channel.
        let results = router.deliver_home_all("boot").await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, Platform::Telegram);
    }

    #[tokio::test]
    async fn send_once_validates() {
        let cfg = cfg_with_home();
        assert!(send_once(&cfg, "telegram:1", "   ").await.is_err());
        assert!(send_once(&cfg, "slack:C1", "hi").await.is_err()); // not configured
        assert!(send_once(&cfg, "nope:1", "hi").await.is_err());
        // Stub adapters "connect" offline; with the real feature this would need network.
        #[cfg(not(feature = "telegram"))]
        assert!(send_once(&cfg, "telegram:1", "hi").await.is_ok());
    }
}
