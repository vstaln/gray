//! GatewayRunner (hermes `gateway/run.py` parity, OpenClaw control-plane model).
//!
//! Inbound pipeline for every [`MessageEvent`]:
//! 1. **authorize** — deny-by-default ([`crate::authz`]); unknown DM senders
//!    get a pairing code, everyone else unknown is dropped silently;
//! 2. **slash dispatch** — `/reset /new /status /stop /restart /whoami /help`;
//! 3. **interrupt** — a new message for a session with a running agent cancels
//!    that run first (level 2); `/stop` cancels without replacing (level 1);
//! 4. **run** — agent with the [`crate::authz::GatedExecutor`] (dangerous
//!    tools auto-denied), streaming edit-in-place where the platform allows;
//! 5. **deliver** — reply to the originating chat/thread, chunked to the
//!    platform limit. Cron output goes to each platform's `home_channel`.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::authz::{Authorizer, Decision, GatedExecutor};
use crate::config::{GatewayConfig, Platform, load_gateway_config};
use crate::delivery::{DeliveryRouter, DeliveryTarget};
use crate::pairing::{PairingOffer, PairingStore};
use crate::platform::{BasePlatformAdapter, MessageEvent, SendOptions, SendResult, preview_80, split_message_smart, utf16_len};
use crate::session::{build_session_key, shared_store, FileGatewayStore, SessionSource};

use crate::discord::DiscordAdapter;
use crate::slack::SlackAdapter;
use crate::telegram::TelegramAdapter;

type Adapter = Arc<dyn BasePlatformAdapter>;

/// Minimum interval between edit-in-place updates while streaming
/// (Telegram/Discord edit rate limits sit around 1/s per chat).
const STREAM_EDIT_INTERVAL: Duration = Duration::from_millis(1500);
/// Don't create the placeholder message until this much text exists.
const STREAM_MIN_CHARS: usize = 24;
const STREAM_CURSOR: &str = " ▍";

pub struct GatewayRunner {
    pub config: GatewayConfig,
    pub adapters: HashMap<Platform, Adapter>,
    pub store: Arc<FileGatewayStore>,
    pub pairing: Arc<PairingStore>,
    pub authz: Authorizer,
    pub router: DeliveryRouter,
    /// Per-session cancellation for /stop and message interrupts (hermes parity).
    cancel_tokens: Mutex<HashMap<String, tokio_util::sync::CancellationToken>>,
}

/// Restart ping-back marker (hermes parity: `~/.hermes/.restart_notify.json`).
/// Written by `/restart` before exit; consumed on next boot, always unlinked.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct RestartNotify {
    platform: String,
    chat_id: String,
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
fn take_restart_marker_in(home: &std::path::Path) -> Option<RestartNotify> {
    let path = restart_notify_path_in(home);
    if !path.exists() {
        return None;
    }
    let data = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok());
    let _ = std::fs::remove_file(&path);
    data
}

/// Slash commands understood on every platform (hermes: /reset /status /stop /restart).
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
        let router = DeliveryRouter::new(config.clone(), adapters.clone());
        Ok(Self { config, adapters, store, pairing, authz, router, cancel_tokens: Mutex::new(HashMap::new()) })
    }

    /// Rebuild the router after adapters were mutated (event channel wiring happens
    /// through `Arc::get_mut`, which needs unique ownership — so wire first, then call this).
    pub fn rebuild_router(&mut self) {
        self.router = DeliveryRouter::new(self.config.clone(), self.adapters.clone());
    }

    fn reply_opts(ev: &MessageEvent) -> SendOptions {
        SendOptions { reply_to: ev.message_id.clone(), thread_id: ev.source.thread_id.clone() }
    }

    async fn reply(&self, ev: &MessageEvent, text: &str) -> SendResult {
        let Some(adapter) = self.adapters.get(&ev.source.platform) else {
            log::warn!("no adapter for {}, dropping reply", ev.source.platform);
            return SendResult::fail(format!("no adapter for {}", ev.source.platform), false);
        };
        let res = adapter.send_ext(&ev.source.chat_id, text, &Self::reply_opts(ev)).await;
        if !res.success {
            log::warn!("gateway send failed: {:?}", res.error);
        }
        res
    }

    /// Handle inbound MessageEvent: authorize → slash → agent → deliver.
    pub async fn handle_inbound(&self, ev: MessageEvent) -> anyhow::Result<SendResult> {
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
                    // hermes parity: remember the requester, reply, then exit;
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

        let streamer = match &adapter {
            Some(a) if self.config.streaming && a.supports_edit() => {
                Some(Streamer::spawn(Arc::clone(a), chat_id.clone(), Self::reply_opts(&ev), platform.max_message_len()))
            }
            _ => None,
        };
        let sink = streamer.as_ref().map(|s| s.tx.clone());

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

        // 5. Deliver — streaming finalizes in place, otherwise chunked send.
        let res = match streamer {
            Some(s) => s.finish(reply_text).await,
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

    fn build_agent(&self, prior: Vec<gray_core::Message>) -> anyhow::Result<gray_core::agent::Agent> {
        use gray_core::agent::Agent;
        use gray_provider::OpenAiProvider;
        use gray_tools::Registry;

        let (base_url, api_key, model) = self.resolve_provider_config();
        let model = model.ok_or_else(|| anyhow::anyhow!("no model configured — set ~/.gray/config.json model"))?;
        let api_key = api_key.unwrap_or_default();
        let base_url = base_url.unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string());
        let provider = OpenAiProvider::builder(&api_key, &model).base_url(&base_url).build().map_err(|e| anyhow::anyhow!("provider init: {e}"))?;

        let registry = Registry::builtin();
        // Advertise only tools the gate will actually let through.
        let tool_defs: Vec<_> = registry
            .defs()
            .into_iter()
            .filter(|d| crate::authz::tool_call_allowed(&self.config.denied_tools, &d.name, &serde_json::Value::Null).is_ok())
            .collect();
        let executor = GatedExecutor::new(Box::new(registry), self.config.denied_tools.clone());

        Ok(Agent::new(Box::new(provider), Box::new(executor))
            .with_system(load_system_prompt())
            .with_tools(tool_defs)
            .with_messages(prior))
    }

    /// Run one agent turn in session `sid`, streaming text deltas to `sink`.
    /// Returns the final assistant text. Persists the full turn (tool calls included).
    async fn run_agent(&self, sid_str: &str, key: &str, text: &str, sink: Option<tokio::sync::mpsc::UnboundedSender<StreamMsg>>) -> anyhow::Result<String> {
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
                let model = self.resolve_model().unwrap_or_else(|| "unknown".to_string());
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let meta = SessionMeta::new(sid.clone(), now_millis(), cwd, model);
                let _ = store.create(meta).await;
                Vec::new()
            }
        };
        let prior_len = prior_messages.len();
        let mut agent = self.build_agent(prior_messages)?;

        // Cancel token registered under the session key so /stop and interrupts can abort.
        let token = tokio_util::sync::CancellationToken::new();
        self.cancel_tokens.lock().unwrap().insert(key.to_string(), token.clone());
        let ctx = ToolContext {
            cwd: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            cancel: token,
            questions: None, // no interactive user → request_user_input is denied anyway
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
        let run = agent.run_streaming(Message::user(text.to_string()), ctx, &mut on_event).await.map_err(|e| anyhow::anyhow!("agent run: {e}"));
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
    /// platform's home channel (hermes `DeliveryRouter` cron path). Output is
    /// also saved under `~/.gray/cron/output/` so nothing is lost when no
    /// home channel is configured.
    pub async fn run_cron_job(&self, job: &gray_cron::CronJob) {
        // Session keyed through build_session_key (never hand-built): the
        // "platform" is the first home-channel platform, chat_type "cron".
        let platform = Platform::ALL.into_iter().find(|p| self.adapters.contains_key(p) && self.router.home_channel(*p).is_some());
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
        // Cron jobs start fresh each run (hermes: isolated per-run session).
        let sid = self.store.reset(&key);
        log::info!("gateway cron '{}' ({}) running as {sid}", job.name, job.id);
        let output = match self.run_agent(&sid, &key, &job.prompt, None).await {
            Ok(t) => t,
            Err(e) => format!("cron job '{}' failed: {e}", job.name),
        };
        save_cron_output(&job.id, &job.name, &output);
        let text = format!("⏰ {}\n\n{}", job.name, output);
        if platform.is_none() {
            log::info!("gateway cron '{}' done (no home_channel; output saved locally)", job.name);
            return;
        }
        for (p, r) in self.router.deliver_home_all(&text).await {
            if r.success {
                log::info!("gateway cron '{}' delivered to {p} home", job.name);
            } else {
                log::warn!("gateway cron '{}' delivery to {p} failed: {:?}", job.name, r.error);
            }
        }
    }

    /// hermes parity (`gateway/run.py` boot sequence): ping the `/restart`
    /// requester, then DM each platform's `home_channel`. Sends are timeout-
    /// bounded so a flood-control sleep can't freeze boot.
    pub async fn send_startup_notifications(&self) {
        if let Ok(home) = crate::config::gray_home_dir() {
            if let Some(m) = take_restart_marker_in(&home) {
                match m.platform.parse::<Platform>() {
                    Ok(p) if self.adapters.contains_key(&p) => {
                        let target = DeliveryTarget { platform: p, chat_id: Some(m.chat_id.clone()), thread_id: None, is_origin: false };
                        let r = self.router.deliver(&target, "♻ Gateway restarted successfully. Your session continues.", None).await;
                        if r.success {
                            log::info!("gateway restart ping sent to {p}:{}", m.chat_id);
                        } else {
                            log::warn!("gateway restart ping failed: {:?}", r.error);
                        }
                    }
                    _ => log::warn!("gateway restart marker: no live adapter for '{}'", m.platform),
                }
            }
        }
        // Settle beat (hermes: 1s helps fresh reconnect deliveries).
        tokio::time::sleep(Duration::from_secs(1)).await;
        for (plat, r) in self.router.deliver_home_all("● Gray gateway online.").await {
            if r.success {
                log::info!("gateway online notice sent to {plat}");
            } else {
                log::warn!("gateway online notice failed for {plat}: {:?}", r.error);
            }
        }
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

fn save_cron_output(job_id: &str, name: &str, output: &str) {
    let Ok(home) = crate::config::gray_home_dir() else { return };
    let dir = home.join("cron").join("output");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let path = dir.join(format!("{job_id}-{ts}.md"));
    let _ = std::fs::write(path, format!("# {name}\n\n{output}\n"));
}

// ---------------------------------------------------------------------------
// Streaming (edit-in-place) reply
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum StreamMsg {
    Delta(String),
    /// A tool ran; whatever was streamed so far was narration, start over.
    Reset,
    Done(String),
}

struct Streamer {
    tx: tokio::sync::mpsc::UnboundedSender<StreamMsg>,
    task: tokio::task::JoinHandle<SendResult>,
}

impl Streamer {
    fn spawn(adapter: Adapter, chat: String, opts: SendOptions, max: usize) -> Self {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<StreamMsg>();
        let task = tokio::spawn(async move {
            let mut buf = String::new();
            let mut msg_id: Option<String> = None;
            let mut last_edit = Instant::now() - STREAM_EDIT_INTERVAL;
            let mut final_text: Option<String> = None;
            while let Some(m) = rx.recv().await {
                match m {
                    StreamMsg::Delta(d) => buf.push_str(&d),
                    StreamMsg::Reset => buf.clear(),
                    StreamMsg::Done(t) => {
                        final_text = Some(t);
                        break;
                    }
                }
                let preview = format!("{}{STREAM_CURSOR}", buf.trim_end());
                // Stop live-updating once the reply outgrows one message; the final
                // chunked send handles it.
                if buf.trim().chars().count() < STREAM_MIN_CHARS || utf16_len(&preview) > max {
                    continue;
                }
                if last_edit.elapsed() < STREAM_EDIT_INTERVAL {
                    continue;
                }
                match &msg_id {
                    None => {
                        let r = adapter.send_ext(&chat, &preview, &opts).await;
                        if r.success {
                            msg_id = r.message_id;
                        }
                    }
                    Some(id) => {
                        let _ = adapter.edit_message(&chat, id, &preview).await;
                    }
                }
                last_edit = Instant::now();
            }
            let text = final_text.unwrap_or_else(|| if buf.is_empty() { "(no reply)".into() } else { buf.clone() });
            finalize_stream(adapter.as_ref(), &chat, &opts, msg_id.as_deref(), &text, max).await
        });
        Self { tx, task }
    }

    async fn finish(self, text: String) -> SendResult {
        let _ = self.tx.send(StreamMsg::Done(text));
        drop(self.tx);
        match self.task.await {
            Ok(r) => r,
            Err(e) => SendResult::fail(format!("stream task failed: {e}"), true),
        }
    }
}

/// Final delivery for a streamed reply: overwrite the placeholder with the first
/// chunk, then send any remaining chunks as normal messages. Falls back to a
/// plain send if the edit fails (e.g. placeholder deleted).
async fn finalize_stream(adapter: &dyn BasePlatformAdapter, chat: &str, opts: &SendOptions, msg_id: Option<&str>, text: &str, max: usize) -> SendResult {
    let chunks = split_message_smart(text, max);
    let Some(first) = chunks.first() else {
        return SendResult::ok(msg_id.map(str::to_string));
    };
    let mut rest_start = 0;
    let mut last = SendResult::ok(msg_id.map(str::to_string));
    if let Some(id) = msg_id {
        let r = adapter.edit_message(chat, id, first).await;
        if r.success {
            rest_start = 1;
            last = r;
        }
    }
    for (i, chunk) in chunks.iter().enumerate().skip(rest_start) {
        // Only the very first message replies to the origin.
        let o = if i == 0 { opts.clone() } else { SendOptions { reply_to: None, thread_id: opts.thread_id.clone() } };
        last = adapter.send_ext(chat, chunk, &o).await;
        if !last.success {
            return last;
        }
    }
    last
}

// ---------------------------------------------------------------------------

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

fn load_system_prompt() -> String {
    let base = crate::config::gray_home_dir().map(|b| b.join("AGENTS.md")).unwrap_or_else(|_| std::path::PathBuf::from("AGENTS.md"));
    // migrate legacy sys.md if needed (same one-time path as lib.rs)
    if !base.exists() {
        if let Some(parent) = base.parent() {
            let legacy = parent.join("sys.md");
            if let Ok(body) = std::fs::read_to_string(&legacy) {
                let _ = std::fs::write(&base, &body);
            }
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
    std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// CLI entry: run until SIGINT/SIGTERM.
pub async fn run_gateway() -> anyhow::Result<()> {
    let token = tokio_util::sync::CancellationToken::new();
    let res = run_gateway_inner(token.clone()).await;
    token.cancel();
    res
}

/// Like [`run_gateway`], but also exits when `shutdown` resolves (REPL `/gateway stop`).
pub async fn run_gateway_shutdown(shutdown: tokio::sync::oneshot::Receiver<()>) -> anyhow::Result<()> {
    let token = tokio_util::sync::CancellationToken::new();
    let t = token.clone();
    let relay = tokio::spawn(async move {
        let _ = shutdown.await;
        t.cancel();
    });
    let res = run_gateway_inner(token.clone()).await;
    token.cancel();
    let _ = relay.await;
    res
}

async fn run_gateway_inner(token: tokio_util::sync::CancellationToken) -> anyhow::Result<()> {
    let cfg = load_gateway_config();
    let mut runner = GatewayRunner::from_config(cfg)?;
    if runner.adapters.is_empty() {
        anyhow::bail!("no gateway platforms enabled — edit ~/.gray/gateway.yaml");
    }
    // Warn loudly when a platform has no operator allowlist: everyone will pair.
    for (plat, pc) in &runner.config.platforms {
        if pc.enabled && pc.allowed_users.is_empty() && std::env::var(plat.allowed_users_env()).is_err() {
            log::warn!(
                "gateway {plat}: no allowed_users / {} set — unknown DMs get a pairing code, groups are ignored (dm_policy={:?})",
                plat.allowed_users_env(), pc.dm_policy
            );
        }
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<MessageEvent>();
    // The router holds clones of the adapter Arcs; drop it so get_mut works, then rebuild.
    runner.router = DeliveryRouter::new(runner.config.clone(), HashMap::new());
    for adapter in runner.adapters.values_mut() {
        match Arc::get_mut(adapter) {
            Some(a) => a.set_event_tx(tx.clone()),
            None => log::warn!("gateway: could not wire event channel (adapter shared)"),
        }
    }
    runner.rebuild_router();
    for (plat, adapter) in runner.adapters.iter() {
        let res = tokio::time::timeout(Duration::from_secs(45), adapter.connect()).await;
        match res {
            Ok(Ok(())) => log::info!("gateway {plat} connected"),
            Ok(Err(e)) => log::warn!("gateway {plat} connect failed: {e}"),
            Err(_) => log::warn!("gateway {plat} connect timeout 45s"),
        }
    }

    runner.send_startup_notifications().await;

    let runner = Arc::new(runner);
    // Agent futures are !Send (gray-core run_streaming sink), so handle events on a
    // dedicated LocalSet thread; spawn_local per event keeps /stop responsive mid-run.
    // The thread exits when `token` cancels, dropping adapters (closing connections).
    let _worker = {
        let token = token.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("gateway runtime");
            rt.block_on(tokio::task::LocalSet::new().run_until(async move {
                // Cron: run due jobs here and deliver to home channels.
                if runner.config.cron_delivery {
                    let r = Arc::clone(&runner);
                    tokio::task::spawn_local(async move {
                        let scheduler = gray_cron::Scheduler::from_active();
                        let mut interval = tokio::time::interval(Duration::from_secs(60));
                        loop {
                            interval.tick().await;
                            let due = match scheduler.scan_due_jobs() {
                                Ok(d) => d,
                                Err(e) => {
                                    log::warn!("gateway cron scan failed: {e}");
                                    continue;
                                }
                            };
                            // Sequential inline dispatch — no dedup guard needed
                            // (a claim would always release before the next scan).
                            for job in due {
                                let _ = gray_cron::store::update_job_run(&job.id, chrono::Utc::now());
                                r.run_cron_job(&job).await;
                            }
                        }
                    });
                }
                loop {
                    tokio::select! {
                        ev = rx.recv() => match ev {
                            Some(ev) => {
                                let r = Arc::clone(&runner);
                                tokio::task::spawn_local(async move {
                                    if let Err(e) = r.handle_inbound(ev).await { log::warn!("gateway handle error: {e}"); }
                                });
                            }
                            None => break,
                        },
                        _ = token.cancelled() => break,
                    }
                }
            }));
        })
    };

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        tokio::select! {
            _ = token.cancelled() => {},
            _ = sigterm.recv() => {},
            _ = sigint.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        tokio::select! {
            _ = token.cancelled() => {},
            _ = tokio::signal::ctrl_c() => {},
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
        MessageEvent {
            text: text.into(),
            message_id: Some("1".into()),
            source: SessionSource {
                platform: Platform::Telegram,
                chat_id: "100".into(),
                chat_type: chat_type.into(),
                user_id: Some(user.into()),
                thread_id: None,
                scope_id: None,
                message_id: Some("1".into()),
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
    async fn finalize_stream_chunks_after_edit() {
        let a: Adapter = Arc::new(TelegramAdapter::new(PlatformConfig::with_token("123456:ABCDEFGHIJ1234567890")).unwrap());
        let long = "x".repeat(5000);
        // Stub edit succeeds → first chunk edited, second sent.
        let r = finalize_stream(a.as_ref(), "100", &SendOptions::default(), Some("9"), &long, 4096).await;
        #[cfg(not(feature = "telegram"))]
        assert!(r.success);
        #[cfg(feature = "telegram")]
        assert!(!r.success); // not connected
        // Empty text with a placeholder is a no-op success.
        let r = finalize_stream(a.as_ref(), "100", &SendOptions::default(), Some("9"), "", 4096).await;
        assert!(r.success);
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
}
