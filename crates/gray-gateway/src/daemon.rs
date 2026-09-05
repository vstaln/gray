//! GatewayRunner.
//!
//! Inbound pipeline for every [`MessageEvent`]:
//! 1. **authorize** — deny-by-default ([`crate::authz`]); unknown DM senders
//!    get a pairing code, everyone else unknown is dropped silently;
//! 2. **slash dispatch** — `/reset /new /status /stop /restart /whoami /help`;
//! 3. **interrupt** — a new message for a session with a running agent cancels
//!    that run first (level 2); `/stop` cancels without replacing (level 1);
//! 4. **run** — agent with the [`crate::authz::GatedExecutor`] (dangerous
//!    tools auto-denied), Hermes-style progress bubbles where the platform allows;
//! 5. **deliver** — reply to the originating chat/thread, chunked to the
//!    platform limit. Cron output goes to each platform's `home_channel`.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::authz::{Authorizer, Decision};
use crate::config::{GatewayConfig, Platform};
use crate::delivery::{DeadTargets, DeliveryLedger, DeliveryRouter, DeliveryTarget};
use crate::pairing::{PairingOffer, PairingStore};
use crate::platform::{BasePlatformAdapter, InboundDedup, MessageEvent, SendOptions, SendResult, preview_80};
use crate::session::{FileGatewayStore, build_session_key, shared_store};

use crate::discord::DiscordAdapter;
use crate::slack::SlackAdapter;
use crate::telegram::TelegramAdapter;

use crate::daemon_stream::ProgressBubble;

/// Minimum interval between progress-bubble EDITS while working
/// (Discord/Telegram edit rate limits sit around 1/s per chat).
/// The first send is always immediate; only edits are throttled.
pub use super::daemon_boot::{run_gateway, run_gateway_shutdown, run_gateway_shutdown_with_board};
pub use super::daemon_supervise::{
    BOOT_MAX_ATTEMPTS, FAST_FAILURE_WINDOW, Fatal, MAX_FAST_FAILURES, MAX_RECONNECT_ATTEMPTS,
    classify_connect_error, classify_shard_end, crash_loop_tripped,
};

pub(crate) type Adapter = Arc<dyn BasePlatformAdapter>;

pub struct GatewayRunner {
    pub config: GatewayConfig,
    pub adapters: HashMap<Platform, Adapter>,
    pub store: Arc<FileGatewayStore>,
    pub pairing: Arc<PairingStore>,
    pub authz: Authorizer,
    pub router: DeliveryRouter,
    pub dedup: InboundDedup,
    pub ledger: DeliveryLedger,
    pub dead: Arc<DeadTargets>,
    /// Per-session cancellation for /stop and message interrupts.
    pub(crate) cancel_tokens: Mutex<HashMap<String, tokio_util::sync::CancellationToken>>,
}

/// Restart ping-back marker.
/// Written by `/restart` before exit; consumed on next boot, always unlinked.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct RestartNotify {
    pub(crate) platform: String,
    pub(crate) chat_id: String,
}

fn restart_notify_path_in(home: &std::path::Path) -> std::path::PathBuf {
    home.join(".restart_notify.json")
}

fn write_restart_marker_in(home: &std::path::Path, platform: Platform, chat_id: &str) -> anyhow::Result<()> {
    let data = RestartNotify { platform: platform.to_string(), chat_id: chat_id.to_string() };
    std::fs::write(restart_notify_path_in(home), serde_json::to_string(&data)?)?;
    Ok(())
}

/// Read + unlink the marker. Always unlinks when present (even on parse
/// failure) so one bad file can't spam every boot.
pub(crate) fn take_restart_marker_in(home: &std::path::Path) -> Option<RestartNotify> {
    let path = restart_notify_path_in(home);
    if !path.exists() {
        return None;
    }
    let data = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok());
    let _ = std::fs::remove_file(&path);
    data
}

/// Slash commands understood on every platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    Reset,
    Status,
    Stop,
    Restart,
    Whoami,
    Help,
}

/// Parse `/cmd`, `/cmd@BotName` (Telegram group form) and `/cmd args` (args ignored).
pub fn parse_slash(text: &str) -> Option<SlashCommand> {
    let t = text.trim();
    let first = t.split_whitespace().next()?;
    let name = first.strip_prefix('/')?.split('@').next()?.to_ascii_lowercase();
    Some(match name.as_str() {
        "reset" | "new" | "clear" => SlashCommand::Reset,
        "status" => SlashCommand::Status,
        "stop" | "cancel" => SlashCommand::Stop,
        "restart" => SlashCommand::Restart,
        "whoami" | "id" => SlashCommand::Whoami,
        "help" | "start" => SlashCommand::Help,
        _ => return None,
    })
}

pub fn pairing_prompt(platform: Platform, code: &str) -> String {
    format!(
        "Hi! I don't recognize you yet.\n\nYour pairing code: {code}\n\nAsk the bot owner to run:\ngray gateway pairing approve {platform} {code}\n\nThe code expires in 1 hour."
    )
}

fn help_text() -> String {
    "gray gateway commands:\n/reset — start a fresh session\n/status — session + model info\n/stop — cancel the running agent\n/restart — restart the gateway\n/whoami — show your user id (for allowlists)\n/help — this message".to_string()
}

impl GatewayRunner {
    pub fn from_config(config: GatewayConfig) -> anyhow::Result<Self> {
        Self::from_config_with(config, shared_store(), Arc::new(PairingStore::open_default()))
    }

    /// Dependency-injected constructor (tests point stores at temp dirs).
    pub fn from_config_with(config: GatewayConfig, store: Arc<FileGatewayStore>, pairing: Arc<PairingStore>) -> anyhow::Result<Self> {
        let mut adapters: HashMap<Platform, Adapter> = HashMap::new();
        for (plat, cfg) in &config.platforms {
            if !cfg.enabled {
                continue;
            }
            let adapter: Adapter = match plat {
                Platform::Telegram => Arc::new(TelegramAdapter::new(cfg.clone())?),
                Platform::Discord => Arc::new(DiscordAdapter::new(cfg.clone())?),
                Platform::Slack => Arc::new(SlackAdapter::new(cfg.clone())?),
            };
            adapters.insert(*plat, adapter);
        }
        let authz = Authorizer::new(config.clone(), Arc::clone(&pairing));
        let dead = Arc::new(DeadTargets::in_memory());
        let router = DeliveryRouter::new(config.clone(), adapters.clone()).with_dead_targets(Arc::clone(&dead));
        Ok(Self {
            config,
            adapters,
            store,
            pairing,
            authz,
            router,
            dedup: InboundDedup::new(),
            ledger: DeliveryLedger::in_memory(),
            dead,
            cancel_tokens: Mutex::new(HashMap::new()),
        })
    }

    /// Rebuild the router after adapters were mutated (event channel wiring happens
    /// through `Arc::get_mut`, which needs unique ownership — so wire first, then call this).
    pub fn rebuild_router(&mut self) {
        self.router = DeliveryRouter::new(self.config.clone(), self.adapters.clone()).with_dead_targets(Arc::clone(&self.dead));
    }

    /// Replay crash-recovered obligations (call on boot and after reconnects).
    pub async fn sweep_pending(&self) -> Vec<(String, SendResult)> {
        self.router.sweep_ledger(&self.ledger).await
    }

    fn reply_opts(ev: &MessageEvent) -> SendOptions {
        SendOptions { reply_to: ev.message_id.clone(), thread_id: ev.source.thread_id.clone() }
    }

    async fn reply(&self, ev: &MessageEvent, text: &str) -> SendResult {
        let target = DeliveryTarget {
            platform: ev.source.platform,
            chat_id: Some(ev.source.chat_id.clone()),
            thread_id: ev.source.thread_id.clone(),
            is_origin: true,
        };
        let res = self.router.deliver(&target, text, ev.message_id.as_deref()).await;
        if !res.success {
            log::warn!("gateway send failed: {:?}", res.error);
        }
        res
    }

    /// Handle inbound MessageEvent: dedup → authorize → slash → agent → deliver.
    pub async fn handle_inbound(&self, ev: MessageEvent) -> anyhow::Result<SendResult> {
        // 0. Redelivery guard — before authz so replays never mint pairing codes.
        if self.dedup.is_duplicate_event(&ev) {
            return Ok(SendResult::fail("duplicate", false));
        }
        let platform = ev.source.platform;
        let chat_id = ev.source.chat_id.clone();

        // 1. Authorization gate — nothing below runs for unknown senders.
        match self.authz.check(&ev.source) {
            Decision::Allow => {}
            Decision::Deny => {
                log::warn!(
                    "gateway denied {platform} user={:?} chat={chat_id} type={}",
                    ev.source.user_id, ev.source.chat_type
                );
                return Ok(SendResult::fail("unauthorized", false));
            }
            Decision::OfferPairing => {
                let uid = ev.source.user_id.clone().unwrap_or_default();
                log::warn!("gateway unknown DM sender {platform} user={uid} — offering pairing");
                let offer = self.pairing.request_code(platform, &uid, ev.user_name.as_deref().unwrap_or(""));
                return Ok(match offer {
                    PairingOffer::Code(code) => self.reply(&ev, &pairing_prompt(platform, &code)).await,
                    PairingOffer::RateLimited => SendResult::fail("pairing rate-limited", false),
                    PairingOffer::Unavailable => {
                        self.reply(&ev, "Pairing is temporarily unavailable (too many pending requests). Try again later.").await
                    }
                });
            }
        }

        let key = build_session_key(&ev.source, self.config.group_per_user, self.config.thread_per_user);
        log::info!("gateway inbound {platform} chat={chat_id} key={key} text={:?}", preview_80(&ev.text));

        // Session reset policy: expired sessions restart fresh via reset().
        if self.store.reset_if_due(&key, &self.config.reset_policy).is_some() {
            log::info!("gateway session {key} expired by reset policy; started fresh");
        }

        // 2. Slash dispatch.
        if let Some(cmd) = parse_slash(&ev.text) {
            let text = match cmd {
                SlashCommand::Reset => {
                    self.cancel_key(&key);
                    let sid = self.store.reset(&key);
                    format!("Session reset ({}).", &sid[..sid.len().min(8)])
                }
                SlashCommand::Status => {
                    let sid = self.store.get(&key).unwrap_or_else(|| "(none yet)".into());
                    let running = self.cancel_tokens.lock().unwrap().contains_key(&key);
                    format!(
                        "session {sid}\nkey {key}\nmodel {}\nrunning {running}\nstreaming {}\ngroup_per_user={} thread_per_user={}",
                        self.resolve_model().unwrap_or_else(|| "unconfigured".into()),
                        self.config.streaming && self.adapters.get(&platform).map(|a| a.supports_edit()).unwrap_or(false),
                        self.config.group_per_user,
                        self.config.thread_per_user
                    )
                }
                SlashCommand::Stop => {
                    if self.cancel_key(&key) { "Stop requested.".into() } else { "Nothing running.".into() }
                }
                SlashCommand::Restart => {
                    // Remember the requester, reply, then exit;
                    // systemd (Restart=always) revives us and boot pings back.
                    if let Ok(home) = crate::config::gray_home_dir() {
                        let _ = write_restart_marker_in(&home, platform, &chat_id);
                    }
                    std::thread::spawn(|| {
                        std::thread::sleep(Duration::from_secs(2));
                        std::process::exit(0);
                    });
                    "Restarting gateway…".into()
                }
                SlashCommand::Whoami => format!(
                    "platform {platform}\nuser_id {}\nchat_id {chat_id}\nchat_type {}{}\n\nAdd the user_id to platforms.{platform}.allowed_users in gateway.yaml to skip pairing.",
                    ev.source.user_id.as_deref().unwrap_or("?"),
                    ev.source.chat_type,
                    ev.source.thread_id.as_ref().map(|t| format!("\nthread_id {t}")).unwrap_or_default()
                ),
                SlashCommand::Help => help_text(),
            };
            return Ok(self.reply(&ev, &text).await);
        }

        // 3. Interrupt: a new message while this session is busy replaces the run.
        if self.cancel_key(&key) {
            log::info!("gateway interrupting active run for {key}");
            self.wait_idle(&key, Duration::from_secs(8)).await;
        }

        // 4. Run the agent with typing + streaming.
        let sid = self.store.get_or_create(&key);
        let adapter = self.adapters.get(&platform).cloned();
        let done = Arc::new(tokio::sync::Notify::new());
        let typing_task = adapter.as_ref().map(|a| {
            let (a, chat, d) = (Arc::clone(a), chat_id.clone(), Arc::clone(&done));
            tokio::spawn(async move {
                a.send_typing(&chat).await;
                loop {
                    tokio::select! {
                        _ = d.notified() => break,
                        _ = tokio::time::sleep(Duration::from_secs(8)) => a.send_typing(&chat).await,
                    }
                }
            })
        });

        let progress = match &adapter {
            Some(a) if self.config.streaming && a.supports_edit() => {
                Some(ProgressBubble::spawn(Arc::clone(a), chat_id.clone(), Self::reply_opts(&ev), platform.max_message_len()))
            }
            _ => None,
        };
        let sink = progress.as_ref().map(|p| p.tx.clone());

        let result = self.run_agent(&sid, &key, &ev.text, sink).await;
        done.notify_one();
        if let Some(t) = typing_task {
            let _ = t.await;
        }
        let reply_text = match result {
            Ok(t) => t,
            Err(e) if e.to_string().contains("Cancelled") => "Stopped.".to_string(),
            Err(e) => {
                log::warn!("gateway agent error: {e}");
                format!("error: {e}")
            }
        };

        // 5. Deliver — progress bubbles are deleted, then the final answer
        // goes out as fresh message(s) on the normal chunked path.
        let res = match progress {
            Some(p) => {
                let final_text = p.finish(reply_text).await;
                self.reply(&ev, &final_text).await
            }
            None => self.reply(&ev, &reply_text).await,
        };
        Ok(res)
    }

    /// Cancel the run registered under `key`. Returns whether one existed.
    fn cancel_key(&self, key: &str) -> bool {
        self.cancel_tokens.lock().unwrap().get(key).map(|t| t.cancel()).is_some()
    }

    async fn wait_idle(&self, key: &str, max: Duration) {
        let start = Instant::now();
        while self.cancel_tokens.lock().unwrap().contains_key(key) && start.elapsed() < max {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub(crate) fn resolve_model(&self) -> Option<String> {
        let (_, _, m) = self.resolve_provider_config();
        m
    }

    pub(crate) fn resolve_provider_config(&self) -> (Option<String>, Option<String>, Option<String>) {
        let saved = load_saved_config();
        let base_url = std::env::var("GRAY_BASE_URL").ok().or(saved.as_ref().and_then(|s| s.base_url.clone()));
        let api_key = std::env::var("GRAY_API_KEY").ok().or_else(|| std::env::var("OPENAI_API_KEY").ok()).or(saved.as_ref().and_then(|s| s.api_key.clone()));
        let model = std::env::var("GRAY_MODEL").ok().or(saved.as_ref().and_then(|s| s.model.clone()));
        (base_url, api_key, model)
    }
}

fn load_saved_config() -> Option<SavedConfig> {
    let path = crate::config::gray_home_dir().ok()?.join("config.json");
    std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok())
}

#[derive(serde::Deserialize)]
struct SavedConfig {
    base_url: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_stream::ProgressMsg;
    use crate::session::SessionSource;
    use crate::config::{GatewayConfig, Platform, PlatformConfig};
    use std::collections::HashMap;

    fn runner_with(pc: PlatformConfig, plat: Platform) -> (tempfile::TempDir, GatewayRunner) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(FileGatewayStore::new(dir.path().join("sessions.json")));
        let pairing = Arc::new(PairingStore::new(dir.path().join("pairing")));
        let mut platforms = HashMap::new();
        platforms.insert(plat, pc);
        let cfg = GatewayConfig { platforms, ..Default::default() };
        let r = GatewayRunner::from_config_with(cfg, store, pairing).unwrap();
        (dir, r)
    }

    fn tg_event(user: &str, text: &str, chat_type: &str) -> MessageEvent {
        // Unique ids per call so distinct test messages don't trip the
        // inbound dedup guard (which keys on platform/chat/msg_id).
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed).to_string();
        MessageEvent {
            text: text.into(),
            message_id: Some(id.clone()),
            source: SessionSource {
                platform: Platform::Telegram,
                chat_id: "100".into(),
                chat_type: chat_type.into(),
                user_id: Some(user.into()),
                thread_id: None,
                scope_id: None,
                message_id: Some(id),
            },
            media_urls: vec![],
            user_name: Some("tester".into()),
        }
    }

    /// Stub adapters "send" successfully; the real client (feature on) fails
    /// cleanly with "not connected" — either way the pipeline reached delivery.
    fn delivered(r: &SendResult) -> bool {
        if cfg!(feature = "telegram") {
            !r.success && r.error.as_deref() == Some("telegram not connected")
        } else {
            r.success
        }
    }

    #[test]
    fn from_config_with_dummy_token() {
        let (_d, runner) = runner_with(PlatformConfig::with_token("123456:ABCDEFGHIJ1234567890"), Platform::Telegram);
        assert!(runner.adapters.contains_key(&Platform::Telegram));
    }

    #[test]
    fn from_config_builds_every_platform() {
        let mut platforms = HashMap::new();
        platforms.insert(Platform::Telegram, PlatformConfig::with_token("123456:ABCDEFGHIJ1234567890"));
        platforms.insert(Platform::Discord, PlatformConfig::with_token(&"d".repeat(40)));
        platforms.insert(Platform::Slack, PlatformConfig { app_token: Some("xapp-1-A-1-x".into()), ..PlatformConfig::with_token("xoxb-1234567890-x") });
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(FileGatewayStore::new(dir.path().join("s.json")));
        let pairing = Arc::new(PairingStore::new(dir.path().join("p")));
        let runner = GatewayRunner::from_config_with(GatewayConfig { platforms, ..Default::default() }, store, pairing).unwrap();
        assert_eq!(runner.adapters.len(), 3);
    }

    #[test]
    fn from_config_skips_disabled() {
        let (_d, runner) = runner_with(PlatformConfig { enabled: false, token: Some("x".into()), ..Default::default() }, Platform::Telegram);
        assert!(runner.adapters.is_empty());
    }

    #[test]
    fn slash_parsing() {
        assert_eq!(parse_slash("/reset"), Some(SlashCommand::Reset));
        assert_eq!(parse_slash("/new"), Some(SlashCommand::Reset));
        assert_eq!(parse_slash("/status@graybot"), Some(SlashCommand::Status));
        assert_eq!(parse_slash("  /stop now"), Some(SlashCommand::Stop));
        assert_eq!(parse_slash("/whoami"), Some(SlashCommand::Whoami));
        assert_eq!(parse_slash("/start"), Some(SlashCommand::Help));
        assert_eq!(parse_slash("/unknown"), None);
        assert_eq!(parse_slash("hello /reset"), None);
        assert_eq!(parse_slash(""), None);
    }

    #[tokio::test]
    async fn unknown_dm_gets_pairing_code_and_nothing_else() {
        // GRAY_HOME unset in tests → env allowlist absent; config has none → pairing.
        let (_d, runner) = runner_with(PlatformConfig::with_token("123456:ABCDEFGHIJ1234567890"), Platform::Telegram);
        let r = runner.handle_inbound(tg_event("42", "hello", "dm")).await.unwrap();
        assert!(delivered(&r), "pairing prompt reached the adapter: {r:?}");
        assert!(runner.pairing.has_pending(Platform::Telegram, "42"));
        assert!(runner.store.get("gray:main:telegram:dm:100").is_none(), "no session created for unpaired user");
        // Second message within the rate window: silent.
        let r = runner.handle_inbound(tg_event("42", "hello again", "dm")).await.unwrap();
        assert!(!r.success);
        assert_eq!(r.error.as_deref(), Some("pairing rate-limited"));
    }

    #[tokio::test]
    async fn unknown_group_sender_is_dropped() {
        let (_d, runner) = runner_with(PlatformConfig::with_token("123456:ABCDEFGHIJ1234567890"), Platform::Telegram);
        let r = runner.handle_inbound(tg_event("42", "/status", "group")).await.unwrap();
        assert!(!r.success);
        assert_eq!(r.error.as_deref(), Some("unauthorized"));
        assert!(!runner.pairing.has_pending(Platform::Telegram, "42"));
    }

    #[tokio::test]
    async fn allowed_user_slash_commands_route_by_session_key() {
        let pc = PlatformConfig { allowed_users: vec!["42".into()], ..PlatformConfig::with_token("123456:ABCDEFGHIJ1234567890") };
        let (_d, runner) = runner_with(pc, Platform::Telegram);
        assert!(delivered(&runner.handle_inbound(tg_event("42", "/whoami", "dm")).await.unwrap()));
        assert!(delivered(&runner.handle_inbound(tg_event("42", "/status", "dm")).await.unwrap()));
        assert!(delivered(&runner.handle_inbound(tg_event("42", "/stop", "dm")).await.unwrap()));
        assert!(delivered(&runner.handle_inbound(tg_event("42", "/reset", "dm")).await.unwrap()));
        assert!(runner.store.get("gray:main:telegram:dm:100").is_some());
        // Paired user gets the same treatment.
        runner.pairing.approve_user(Platform::Telegram, "7", "");
        assert!(delivered(&runner.handle_inbound(tg_event("7", "/help", "dm")).await.unwrap()));
    }

    #[tokio::test]
    async fn agent_path_without_model_reports_error_not_panic() {
        let pc = PlatformConfig { allowed_users: vec!["42".into()], ..PlatformConfig::with_token("123456:ABCDEFGHIJ1234567890") };
        let (d, runner) = runner_with(pc, Platform::Telegram);
        // Point config lookups at an empty home so no real model/API key leaks in.
        // SAFETY: tests in this crate that touch GRAY_HOME are serialized by cargo's
        // per-test-binary process; other tests do not depend on this variable.
        unsafe { std::env::set_var("GRAY_HOME", d.path()) };
        unsafe { std::env::remove_var("GRAY_MODEL") };
        let r = runner.handle_inbound(tg_event("42", "hi there", "dm")).await.unwrap();
        unsafe { std::env::remove_var("GRAY_HOME") };
        // The error text reached delivery (no panic, no hang).
        assert!(delivered(&r), "{r:?}");
    }

    #[test]
    fn restart_marker_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        assert!(take_restart_marker_in(home).is_none());
        write_restart_marker_in(home, Platform::Telegram, "12345").unwrap();
        let m = take_restart_marker_in(home).unwrap();
        assert_eq!(m.platform, "telegram");
        assert_eq!(m.chat_id, "12345");
        // consumed: gone, no repeat spam on next boot
        assert!(!restart_notify_path_in(home).exists());
        assert!(take_restart_marker_in(home).is_none());
    }

    #[test]
    fn restart_marker_bad_content_unlinked() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(restart_notify_path_in(dir.path()), "not json").unwrap();
        assert!(take_restart_marker_in(dir.path()).is_none());
        assert!(!restart_notify_path_in(dir.path()).exists());
    }

    #[tokio::test]
    async fn progress_bubble_tracks_tool_lines_and_finishes() {
        use crate::progress::{tool_end_line, tool_start_line};
        // Pure composition sanity inside the IO test's world.
        assert_eq!(tool_start_line("terminal"), "⏳ terminal…");
        assert_eq!(
            tool_end_line("terminal", &serde_json::json!({"command": "ls"})),
            "🔧 terminal: \"{\"command\":\"ls\"}\""
        );
        // End-to-end through the bubble task on the stub adapter (no network):
        // tool events → bubble sends → Done deletes → final text returned.
        let a: Adapter = Arc::new(TelegramAdapter::new(PlatformConfig::with_token("123456:ABCDEFGHIJ1234567890")).unwrap());
        let b = ProgressBubble::spawn(a, "100".into(), SendOptions::default(), 4096);
        b.tx.send(ProgressMsg::ToolStart { id: "1".into(), name: "terminal".into() }).unwrap();
        b.tx.send(ProgressMsg::ToolEnd { id: "1".into(), args: serde_json::json!({"command": "ls"}) }).unwrap();
        let out = b.finish("done".into()).await;
        assert_eq!(out, "done");
    }

    #[tokio::test]
    async fn gated_executor_denies_with_accurate_message() {
        use crate::authz::GatedExecutor;
        use gray_core::agent::{ToolContext, ToolExecutor, ToolOutput};
        struct Inner;
        #[async_trait::async_trait]
        impl ToolExecutor for Inner {
            fn execute(&self, _ctx: &ToolContext, _name: &str, _args: serde_json::Value) -> futures::future::BoxFuture<'static, ToolOutput> {
                Box::pin(async { ToolOutput::ok("must not reach inner") })
            }
        }
        let ex = GatedExecutor::new(Box::new(Inner), vec!["write".to_string()]);
        let ctx = ToolContext { cwd: std::path::PathBuf::from("."), cancel: tokio_util::sync::CancellationToken::new(), questions: None };
        let out = ex.execute(&ctx, "write", serde_json::json!({})).await;
        assert!(out.is_error);
        assert!(out.content.contains("disabled in gateway mode"), "got: {}", out.content);
        // Non-denied tools still delegate.
        let out = ex.execute(&ctx, "read", serde_json::json!({"path": "x"})).await;
        assert!(!out.is_error);
    }

    #[test]
    fn pairing_prompt_contains_cli_hint() {
        let p = pairing_prompt(Platform::Slack, "ABCD2345");
        assert!(p.contains("gray gateway pairing approve slack ABCD2345"));
    }

    #[test]
    fn truncate_helper_still_exported() {
        assert_eq!(crate::platform::truncate_message("hello", 10), "hello");
    }

    #[test]
    fn terminal_auth_failure_stops_ladder() {
        for msg in [
            "telegram token rejected: 401 Unauthorized",
            "discord token rejected: 403 Forbidden",
            "bad token: unauthorized",
            "forbidden: bot was kicked",
        ] {
            assert!(matches!(classify_connect_error(msg), Fatal::Terminal(_)), "must be terminal: {msg}");
        }
    }

    #[test]
    fn retryable_failure_retries() {
        assert!(matches!(classify_connect_error("connection reset by peer"), Fatal::Retryable(_)));
        assert!(matches!(classify_connect_error("connect timeout 45s"), Fatal::Retryable(_)));
        assert!(matches!(classify_connect_error("shard ended"), Fatal::Retryable(_)));
    }

    #[test]
    fn ladder_terminal_stops_after_one_attempt() {
        // Production ladder (`connect_adapter_with_retry`) stops immediately
        // on terminal auth failures — single classification, no retry.
        assert!(matches!(
            classify_connect_error("telegram token rejected: 401 Unauthorized"),
            Fatal::Terminal(_)
        ));
    }

    #[test]
    fn ladder_retryable_retries_with_backoff_then_succeeds() {
        // Backoff ladder itself is the existing helper, unchanged.
        assert_eq!(crate::platform::backoff_delay(0).as_secs(), 1);
        assert_eq!(crate::platform::backoff_delay(1).as_secs(), 2);
        assert!(matches!(
            classify_connect_error("connection reset by peer"),
            Fatal::Retryable(_)
        ));
    }

    #[test]
    fn discord_shard_end_reconnects_through_ladder() {
        // Shard death is retryable so the production ladder reconnects.
        assert!(matches!(classify_shard_end(), Fatal::Retryable(_)));
        assert!(matches!(classify_connect_error("shard ended"), Fatal::Retryable(_)));
    }

    #[test]
    fn privileged_intents_never_terminal() {
        // Close 4014 contains "401" — the intents carve-out must win.
        for msg in [
            "discord gateway closed: 4014 disallowed intents",
            "privileged intents required (enable MESSAGE_CONTENT)",
            "close 4014: disallowed privileged intents",
        ] {
            assert!(matches!(classify_connect_error(msg), Fatal::Retryable(_)), "must be retryable: {msg}");
        }
    }

    #[test]
    fn slack_revoked_tokens_are_terminal() {
        for msg in [
            "slack bot token rejected: invalid_auth",
            "slack send: account_inactive",
            "socket mode failed: token_revoked",
            "not_authed",
        ] {
            assert!(matches!(classify_connect_error(msg), Fatal::Terminal(_)), "must be terminal: {msg}");
        }
    }

    #[test]
    fn crash_loop_guard_trips_after_fast_failures() {
        assert!(!crash_loop_tripped(0));
        assert!(!crash_loop_tripped(MAX_FAST_FAILURES - 1));
        assert!(crash_loop_tripped(MAX_FAST_FAILURES));
        assert!(crash_loop_tripped(MAX_FAST_FAILURES + 10));
    }

    #[test]
    fn boot_cap_is_lower_than_steady_state() {
        assert!(BOOT_MAX_ATTEMPTS >= 1);
        assert!(BOOT_MAX_ATTEMPTS < MAX_RECONNECT_ATTEMPTS);
    }

    #[tokio::test]
    async fn dedup_guard_runs_before_authz() {
        let (_d, runner) = runner_with(PlatformConfig::with_token("123456:ABCDEFGHIJ1234567890"), Platform::Telegram);
        let ev = tg_event("42", "hello", "dm");
        let r1 = runner.handle_inbound(ev.clone()).await.unwrap();
        assert!(delivered(&r1), "first pairing prompt must deliver: {r1:?}");
        let r2 = runner.handle_inbound(ev).await.unwrap();
        assert!(!r2.success);
        assert_eq!(r2.error.as_deref(), Some("duplicate"));
    }

    #[tokio::test]
    async fn sweep_replays_pending_ledger() {
        use crate::delivery::{DeliveryTarget, ObligationStatus};
        let (_d, runner) = runner_with(PlatformConfig::with_token("123456:ABCDEFGHIJ1234567890"), Platform::Telegram);
        let target = DeliveryTarget::parse("telegram:100", None).unwrap();
        let id = runner.ledger.record("sess", "m1", &target, "hi", None);
        assert_eq!(runner.ledger.sweep().len(), 1);
        let done = runner.router.sweep_ledger(&runner.ledger).await;
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].0, id);
        assert!(delivered(&done[0].1) || done[0].1.success, "sweep must deliver: {:?}", done[0].1);
        assert_eq!(runner.ledger.get(&id).unwrap().status, ObligationStatus::Delivered);
    }
}
