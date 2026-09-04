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
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};

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

// ---------------------------------------------------------------------------
// Delivery ledger: pending → delivered/failed obligations, JSON-persisted.
// ---------------------------------------------------------------------------

/// Max send attempts per obligation before it is abandoned.
pub const MAX_DELIVERY_ATTEMPTS: u32 = 3;

/// Stable id for an outbound obligation: `hash(session_key + message_ref + content)`.
pub fn obligation_id(session_key: &str, message_ref: &str, content: &str) -> String {
    let mut h = Sha256::new();
    h.update(session_key.as_bytes());
    h.update(b"\x00");
    h.update(message_ref.as_bytes());
    h.update(b"\x00");
    h.update(content.as_bytes());
    format!("{:x}", h.finalize())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObligationStatus {
    Pending,
    Delivered,
    Failed,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeliveryObligation {
    pub id: String,
    pub session_key: String,
    /// Resolved target string (`platform:chat[:thread]`).
    pub target: String,
    pub text: String,
    pub reply_to: Option<String>,
    pub attempts: u32,
    pub retryable: bool,
    pub status: ObligationStatus,
    pub last_error: Option<String>,
    pub updated_at: i64,
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Crash-safe outbox: record BEFORE send, mark after, replay via [`sweep`].
/// Persists like [`crate::session::FileGatewayStore`] (whole-file JSON).
pub struct DeliveryLedger {
    path: Option<PathBuf>,
    lock: Mutex<HashMap<String, DeliveryObligation>>,
}

impl DeliveryLedger {
    pub fn new(path: PathBuf) -> Self {
        let map = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Self { path: Some(path), lock: Mutex::new(map) }
    }

    pub fn in_memory() -> Self {
        Self { path: None, lock: Mutex::new(HashMap::new()) }
    }

    pub fn open_default() -> Self {
        match crate::config::gray_home_dir().map(|h| h.join("delivery_ledger.json")) {
            Ok(p) => Self::new(p),
            Err(_) => Self::in_memory(),
        }
    }

    fn persist(&self) {
        let Some(path) = &self.path else { return };
        if let Ok(map) = self.lock.lock()
            && let Ok(s) = serde_json::to_string_pretty(&*map)
        {
            if let Some(p) = path.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            let _ = std::fs::write(path, s);
        }
    }

    /// Record a pending obligation BEFORE sending. Idempotent on the id;
    /// starts replayable (`retryable: true`) until a send proves otherwise.
    pub fn record(&self, session_key: &str, message_ref: &str, target: &DeliveryTarget, text: &str, reply_to: Option<&str>) -> String {
        let id = obligation_id(session_key, message_ref, text);
        {
            let mut map = self.lock.lock().unwrap();
            map.entry(id.clone()).or_insert_with(|| DeliveryObligation {
                id: id.clone(),
                session_key: session_key.to_string(),
                target: target.to_target_string(),
                text: text.to_string(),
                reply_to: reply_to.map(str::to_string),
                attempts: 0,
                retryable: true,
                status: ObligationStatus::Pending,
                last_error: None,
                updated_at: now_ts(),
            });
        }
        self.persist();
        id
    }

    pub fn get(&self, id: &str) -> Option<DeliveryObligation> {
        self.lock.lock().unwrap().get(id).cloned()
    }

    pub fn mark_delivered(&self, id: &str) {
        {
            let mut map = self.lock.lock().unwrap();
            if let Some(o) = map.get_mut(id) {
                o.status = ObligationStatus::Delivered;
                o.updated_at = now_ts();
            }
        }
        self.persist();
    }

    /// Record a failed attempt. Non-retryable errors and attempt #3+ are
    /// terminal (abandoned + logged); [`SendResult::retryable`] is the ONLY
    /// replay gate.
    pub fn mark_failed(&self, id: &str, error: &str, retryable: bool) {
        {
            let mut map = self.lock.lock().unwrap();
            if let Some(o) = map.get_mut(id) {
                o.attempts += 1;
                o.retryable = retryable;
                o.last_error = Some(error.to_string());
                o.updated_at = now_ts();
                if !retryable || o.attempts >= MAX_DELIVERY_ATTEMPTS {
                    o.status = ObligationStatus::Failed;
                    log::warn!("delivery ledger abandoning {id} after {} attempts: {error}", o.attempts);
                }
            }
        }
        self.persist();
    }

    /// Replay candidates: pending, still retryable, attempts left.
    /// Feed to [`DeliveryRouter::sweep_ledger`] on boot / after reconnect.
    pub fn sweep(&self) -> Vec<DeliveryObligation> {
        self.lock
            .lock()
            .unwrap()
            .values()
            .filter(|o| o.status == ObligationStatus::Pending && o.retryable && o.attempts < MAX_DELIVERY_ATTEMPTS)
            .cloned()
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Dead targets: chats that hard-fail sends, parked until a send succeeds.
// ---------------------------------------------------------------------------

/// Fatal-send substrings (matched case-insensitively, no taxonomy).
const DEAD_HINTS: &[&str] = &["not found", "forbidden", "kicked", "blocked", "deactivated"];

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeadEntry {
    pub reason: String,
    pub ts: i64,
}

/// `"platform:chat" -> {reason, ts}`, persisted to `dead_targets.json`.
pub struct DeadTargets {
    path: Option<PathBuf>,
    lock: Mutex<HashMap<String, DeadEntry>>,
}

impl DeadTargets {
    pub fn new(path: PathBuf) -> Self {
        let map = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Self { path: Some(path), lock: Mutex::new(map) }
    }

    pub fn in_memory() -> Self {
        Self { path: None, lock: Mutex::new(HashMap::new()) }
    }

    pub fn open_default() -> Self {
        match crate::config::gray_home_dir().map(|h| h.join("dead_targets.json")) {
            Ok(p) => Self::new(p),
            Err(_) => Self::in_memory(),
        }
    }

    fn persist(&self) {
        let Some(path) = &self.path else { return };
        if let Ok(map) = self.lock.lock()
            && let Ok(s) = serde_json::to_string_pretty(&*map)
        {
            if let Some(p) = path.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            let _ = std::fs::write(path, s);
        }
    }

    pub fn dead_key(platform: Platform, chat: &str) -> String {
        format!("{platform}:{chat}")
    }

    /// Substring match only — no taxonomy.
    pub fn is_dead_error(err: &str) -> bool {
        let e = err.to_ascii_lowercase();
        DEAD_HINTS.iter().any(|h| e.contains(h))
    }

    pub fn is_dead(&self, key: &str) -> bool {
        self.lock.lock().unwrap().contains_key(key)
    }

    pub fn mark(&self, key: &str, reason: &str) {
        {
            let mut map = self.lock.lock().unwrap();
            map.insert(key.to_string(), DeadEntry { reason: reason.to_string(), ts: now_ts() });
        }
        self.persist();
    }

    pub fn clear(&self, key: &str) {
        {
            self.lock.lock().unwrap().remove(key);
        }
        self.persist();
    }
}

pub struct DeliveryRouter {
    config: GatewayConfig,
    adapters: HashMap<Platform, Arc<dyn BasePlatformAdapter>>,
    dead: Option<Arc<DeadTargets>>,
}

impl DeliveryRouter {
    pub fn new(config: GatewayConfig, adapters: HashMap<Platform, Arc<dyn BasePlatformAdapter>>) -> Self {
        Self { config, adapters, dead: None }
    }

    /// Attach dead-target tracking: [`deliver`] skips dead chats, clears on success.
    pub fn with_dead_targets(mut self, dead: Arc<DeadTargets>) -> Self {
        self.dead = Some(dead);
        self
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
        let key = DeadTargets::dead_key(target.platform, &chat);
        if self.dead.as_ref().is_some_and(|d| d.is_dead(&key)) {
            return SendResult::fail(format!("skipping dead target {key}"), false);
        }
        let opts = SendOptions { reply_to: reply_to.map(str::to_string), thread_id: target.thread_id.clone() };
        let res = Self::send_to(adapter.as_ref(), &target.to_target_string(), &chat, text, &opts).await;
        self.note_result(&key, &res);
        res
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

    /// Raw send with timeout; no dead-target bookkeeping (see [`deliver`]).
    async fn send_to(adapter: &dyn BasePlatformAdapter, what: &str, chat: &str, text: &str, opts: &SendOptions) -> SendResult {
        let res = tokio::time::timeout(Duration::from_secs(60), adapter.send_ext(chat, text, opts)).await;
        match res {
            Ok(r) => r,
            Err(_) => SendResult::fail(format!("delivery to {what} timed out"), true),
        }
    }

    /// Dead-target bookkeeping for a finished send: success self-heals,
    /// fatal errors park the chat.
    fn note_result(&self, key: &str, res: &SendResult) {
        let Some(dead) = &self.dead else { return };
        if res.success {
            dead.clear(key);
        } else if let Some(e) = &res.error
            && DeadTargets::is_dead_error(e)
        {
            dead.mark(key, e);
        }
    }

    /// Record-then-send: the obligation hits the ledger BEFORE the send so a
    /// crash between the two still replays via [`sweep_ledger`].
    pub async fn deliver_recorded(
        &self,
        ledger: &DeliveryLedger,
        session_key: &str,
        message_ref: &str,
        target: &DeliveryTarget,
        text: &str,
        reply_to: Option<&str>,
    ) -> (String, SendResult) {
        let id = ledger.record(session_key, message_ref, target, text, reply_to);
        let res = self.deliver(target, text, reply_to).await;
        if res.success {
            ledger.mark_delivered(&id);
        } else {
            ledger.mark_failed(&id, res.error.as_deref().unwrap_or("unknown error"), res.retryable);
        }
        (id, res)
    }

    /// Replay [`DeliveryLedger::sweep`] candidates (call on boot and after
    /// adapter reconnect). Probes dead targets too — a reconnect may have
    /// fixed the cause, and success self-heals via [`note_result`].
    pub async fn sweep_ledger(&self, ledger: &DeliveryLedger) -> Vec<(String, SendResult)> {
        let mut out = Vec::new();
        for ob in ledger.sweep() {
            let target = match DeliveryTarget::parse(&ob.target, None) {
                Ok(t) => t,
                Err(e) => {
                    let err = format!("bad ledger target {}: {e}", ob.target);
                    ledger.mark_failed(&ob.id, &err, false);
                    out.push((ob.id, SendResult::fail(err, false)));
                    continue;
                }
            };
            let Some(adapter) = self.adapters.get(&target.platform) else {
                let err = format!("no live adapter for {}", target.platform);
                ledger.mark_failed(&ob.id, &err, false);
                out.push((ob.id.clone(), SendResult::fail(err, false)));
                continue;
            };
            let chat = match self.resolve_chat(&target) {
                Ok(c) => c,
                Err(e) => {
                    let err = e.to_string();
                    ledger.mark_failed(&ob.id, &err, false);
                    out.push((ob.id.clone(), SendResult::fail(err, false)));
                    continue;
                }
            };
            let opts = SendOptions { reply_to: ob.reply_to.clone(), thread_id: target.thread_id.clone() };
            let res = Self::send_to(adapter.as_ref(), &target.to_target_string(), &chat, &ob.text, &opts).await;
            let key = DeadTargets::dead_key(target.platform, &chat);
            self.note_result(&key, &res);
            if res.success {
                ledger.mark_delivered(&ob.id);
            } else {
                ledger.mark_failed(&ob.id, res.error.as_deref().unwrap_or("unknown error"), res.retryable);
            }
            out.push((ob.id, res));
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
    use std::sync::Mutex;

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

    // --- delivery ledger + dead targets (workstream A-gw1) ---

    /// Scriptable adapter: pops a canned result per send, counts calls.
    struct ScriptAdapter {
        plat: Platform,
        script: Mutex<Vec<SendResult>>,
        calls: Mutex<usize>,
    }

    impl ScriptAdapter {
        /// Results are consumed in order (first element = first send).
        fn with_script(plat: Platform, script: Vec<SendResult>) -> Self {
            Self { plat, script: Mutex::new(script.into_iter().rev().collect()), calls: Mutex::new(0) }
        }
        fn calls(&self) -> usize {
            *self.calls.lock().unwrap()
        }
    }

    #[async_trait::async_trait]
    impl BasePlatformAdapter for ScriptAdapter {
        fn platform(&self) -> Platform {
            self.plat
        }
        fn is_authenticated(&self) -> bool {
            true
        }
        async fn connect(&self) -> anyhow::Result<()> {
            Ok(())
        }
        async fn disconnect(&self) -> anyhow::Result<()> {
            Ok(())
        }
        async fn send(&self, _chat: &str, _text: &str) -> SendResult {
            *self.calls.lock().unwrap() += 1;
            self.script.lock().unwrap().pop().unwrap_or(SendResult::ok(None))
        }
    }

    fn script_router(script: Vec<SendResult>) -> (Arc<ScriptAdapter>, DeliveryRouter) {
        let adapter = Arc::new(ScriptAdapter::with_script(Platform::Telegram, script));
        let mut adapters: HashMap<Platform, Arc<dyn BasePlatformAdapter>> = HashMap::new();
        adapters.insert(Platform::Telegram, adapter.clone());
        (adapter, DeliveryRouter::new(GatewayConfig::default(), adapters))
    }

    #[test]
    fn obligation_id_stable_and_content_bound() {
        let a = obligation_id("sess", "ref1", "hello");
        assert_eq!(a, obligation_id("sess", "ref1", "hello"));
        assert_ne!(a, obligation_id("sess", "ref1", "hello!"));
        assert_ne!(a, obligation_id("sess", "ref2", "hello"));
        assert_ne!(a, obligation_id("other", "ref1", "hello"));
    }

    #[tokio::test]
    async fn ledger_record_deliver_sweep_abandon() {
        let (_a, router) = script_router(vec![
            SendResult::ok(Some("m1".into())),
            SendResult::fail("timeout", true),
        ]);
        let ledger = DeliveryLedger::in_memory();
        let target = DeliveryTarget::parse("telegram:123", None).unwrap();

        // Record BEFORE send → pending.
        let id = ledger.record("sess", "m1", &target, "hi", None);
        let ob = ledger.get(&id).unwrap();
        assert_eq!(ob.status, ObligationStatus::Pending);
        assert_eq!(ob.attempts, 0);

        // Success path clears the obligation.
        let (id2, res) = router.deliver_recorded(&ledger, "sess", "m2", &target, "hi", None).await;
        assert!(res.success);
        assert_eq!(ledger.get(&id2).unwrap().status, ObligationStatus::Delivered);
        assert!(ledger.sweep().iter().all(|o| o.id != id2));

        // Retryable failure stays pending and shows up in sweep.
        let (id3, res) = router.deliver_recorded(&ledger, "sess", "m3", &target, "hi", None).await;
        assert!(!res.success);
        let ob = ledger.get(&id3).unwrap();
        assert_eq!(ob.status, ObligationStatus::Pending);
        assert_eq!(ob.attempts, 1);
        assert!(ledger.sweep().iter().any(|o| o.id == id3));

        // Attempts 2..3 → abandoned (failed) and gone from sweep.
        ledger.mark_failed(&id3, "timeout", true);
        assert_eq!(ledger.get(&id3).unwrap().status, ObligationStatus::Pending);
        ledger.mark_failed(&id3, "timeout", true);
        let ob = ledger.get(&id3).unwrap();
        assert_eq!(ob.status, ObligationStatus::Failed);
        assert_eq!(ob.attempts, 3);
        assert!(ledger.sweep().iter().all(|o| o.id != id3));
    }

    #[tokio::test]
    async fn ledger_non_retryable_fails_fast() {
        let (_a, router) = script_router(vec![SendResult::fail("no live adapter", false)]);
        let ledger = DeliveryLedger::in_memory();
        let target = DeliveryTarget::parse("telegram:123", None).unwrap();
        let (id, res) = router.deliver_recorded(&ledger, "sess", "m9", &target, "hi", None).await;
        assert!(!res.success);
        let ob = ledger.get(&id).unwrap();
        assert_eq!(ob.status, ObligationStatus::Failed);
        assert!(ledger.sweep().iter().all(|o| o.id != id));
    }

    #[tokio::test]
    async fn ledger_sweep_replays_and_marks() {
        let (_a, router) = script_router(vec![SendResult::ok(None)]);
        let ledger = DeliveryLedger::in_memory();
        let target = DeliveryTarget::parse("telegram:123", None).unwrap();
        // Simulate a crash between record and send: pending, never attempted.
        let id = ledger.record("sess", "crash", &target, "pending text", None);
        assert_eq!(ledger.sweep().len(), 1);
        let done = router.sweep_ledger(&ledger).await;
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].0, id);
        assert!(done[0].1.success);
        assert_eq!(ledger.get(&id).unwrap().status, ObligationStatus::Delivered);
        assert!(ledger.sweep().is_empty());
    }

    #[test]
    fn dead_error_classification_is_substring_only() {
        for e in ["chat not found", "FORBIDDEN: bot was kicked", "blocked by user", "user deactivated", "kicked from group"] {
            assert!(DeadTargets::is_dead_error(e), "{e}");
        }
        for e in ["timeout", "not connected", "no live adapter", ""] {
            assert!(!DeadTargets::is_dead_error(e), "{e}");
        }
    }

    #[tokio::test]
    async fn dead_targets_skip_and_self_heal() {
        let (adapter, router) = script_router(vec![
            SendResult::fail("forbidden: bot was kicked", false),
            SendResult::ok(None),
        ]);
        let dead = Arc::new(DeadTargets::in_memory());
        let router = router.with_dead_targets(Arc::clone(&dead));
        let target = DeliveryTarget::parse("telegram:123", None).unwrap();

        // Fatal send error marks the target dead.
        let r = router.deliver(&target, "hi", None).await;
        assert!(!r.success);
        assert!(dead.is_dead("telegram:123"));

        // Hot path skips dead entries without touching the adapter.
        let calls = adapter.calls();
        let r = router.deliver(&target, "hi", None).await;
        assert!(!r.success);
        assert_eq!(adapter.calls(), calls);

        // The sweep (boot/reconnect) path probes anyway; success self-heals.
        let ledger = DeliveryLedger::in_memory();
        let id = ledger.record("sess", "heal", &target, "hi", None);
        let done = router.sweep_ledger(&ledger).await;
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].0, id);
        assert!(done[0].1.success);
        assert!(!dead.is_dead("telegram:123"));
        assert_eq!(ledger.get(&id).unwrap().status, ObligationStatus::Delivered);
    }

    #[test]
    fn ledger_and_dead_targets_persist_to_json() {
        let dir = tempfile::tempdir().unwrap();
        let lpath = dir.path().join("delivery_ledger.json");
        let dpath = dir.path().join("dead_targets.json");
        let ledger = DeliveryLedger::new(lpath.clone());
        let target = DeliveryTarget::parse("telegram:123", None).unwrap();
        let id = ledger.record("sess", "persist", &target, "hi", None);
        ledger.mark_failed(&id, "timeout", true);
        let dead = DeadTargets::new(dpath.clone());
        dead.mark("telegram:123", "forbidden");
        assert!(lpath.exists());
        assert!(dpath.exists());
        // Reloaded state survives the round trip.
        let ledger2 = DeliveryLedger::new(lpath);
        let ob = ledger2.get(&id).unwrap();
        assert_eq!(ob.attempts, 1);
        assert_eq!(ob.status, ObligationStatus::Pending);
        let dead2 = DeadTargets::new(dpath);
        assert!(dead2.is_dead("telegram:123"));
        dead2.clear("telegram:123");
        assert!(!dead2.is_dead("telegram:123"));
    }
}
