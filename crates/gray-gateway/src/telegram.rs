//! Telegram adapter — stub by default, real long-polling behind `telegram` feature.
//!
//! Enable: `cargo check -p gray-gateway --features telegram` (teloxide 0.17, rustls).
//! Features: inbound
//! text/captions via `getUpdates` long-polling with exponential-backoff
//! reconnect, replies chunked at 4096 utf16 units with reply-on-first-chunk,
//! forum-topic threads, persistent typing action, edit-in-place streaming,
//! flood-control (`RetryAfter`) honoured on send.
//!
//! Authorization is NOT done here: every event carries `user_id` and the
//! runner's [`crate::authz::Authorizer`] decides (deny-by-default,
//! `TELEGRAM_ALLOWED_USERS` / `allowed_users` / pairing).

use crate::config::{Platform, PlatformConfig};
use crate::platform::{check_token_shape, utf16_len, BasePlatformAdapter, MessageEvent, SendOptions, SendResult};
use crate::session::SessionSource;
use crate::status::GatewayStatusBoard;
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicBool;
#[cfg(feature = "telegram")]
use std::sync::atomic::Ordering;
#[cfg(feature = "telegram")]
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

pub const MAX_LENGTH: usize = 4096;

/// Heartbeat (`get_me`) interval; after [`HEARTBEAT_MAX_MISSES`]
/// consecutive failures the poller is aborted + respawned with backoff.
pub const HEARTBEAT_INTERVAL_SECS: u64 = 60;
pub const HEARTBEAT_MAX_MISSES: u32 = 2;

/// True once missed heartbeats hit the respawn threshold.
pub fn heartbeat_should_respawn(consecutive_failures: u32) -> bool {
    consecutive_failures >= HEARTBEAT_MAX_MISSES
}

/// Heartbeat `get_me` failure policy: Terminal (revoked token) never
/// respawns, Retryable respawns once misses hit the threshold.
/// Routes through the daemon's [`crate::daemon::classify_connect_error`].
pub fn heartbeat_should_respawn_error(err: &str, consecutive_failures: u32) -> bool {
    match crate::daemon::classify_connect_error(err) {
        crate::daemon::Fatal::Terminal(_) => false,
        crate::daemon::Fatal::Retryable(_) => heartbeat_should_respawn(consecutive_failures),
    }
}

/// Fold a sequence of probe outcomes (`true` = ok) into
/// (final miss count, respawn tripped). A success clears the count.
pub fn drive_heartbeat<I: IntoIterator<Item = bool>>(probes: I) -> (u32, bool) {
    let mut misses = 0u32;
    for ok in probes {
        if ok {
            misses = 0;
        } else {
            misses += 1;
            if heartbeat_should_respawn(misses) {
                return (misses, true);
            }
        }
    }
    (misses, heartbeat_should_respawn(misses))
}

#[cfg(feature = "telegram")]
type BotClient = teloxide::Bot;
#[cfg(not(feature = "telegram"))]
type BotClient = ();

pub struct TelegramAdapter {
    token: String,
    client: Mutex<Option<BotClient>>,
    event_tx: Mutex<Option<UnboundedSender<MessageEvent>>>,
    /// Polling task handle so `disconnect` can stop it (shared with the
    /// heartbeat task, which respawns the poller after missed heartbeats).
    poller: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// Heartbeat task handle so `disconnect` can stop it.
    heartbeat: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Bot name from `get_me` (set on connect, read by the boot card).
    identity: Mutex<Option<String>>,
    /// Status board for staged connect progress (wired by the daemon; None for send-only).
    board: Mutex<Option<GatewayStatusBoard>>,
    /// False until the first `connect()` (cold boot drops the pending queue,
    /// reconnects resume the live queue — see `poller_initial_offset`).
    #[cfg_attr(not(feature = "telegram"), allow(dead_code))]
    booted: AtomicBool,
}

/// Cold boot drops the pending queue (start at the live head, offset -1);
/// reconnects resume the live queue unoffset so the server redelivers.
#[cfg_attr(not(feature = "telegram"), allow(dead_code))]
pub(crate) fn poller_initial_offset(drop_pending: bool) -> Option<i32> {
    drop_pending.then_some(-1)
}

impl TelegramAdapter {
    pub fn new(cfg: PlatformConfig) -> anyhow::Result<Self> {
        let token = cfg
            .token
            .ok_or_else(|| anyhow::anyhow!("telegram token not set (set platforms.telegram.token in gateway.yaml)"))?;
        validate_telegram_token(&token)?;
        Ok(Self {
            token: token.trim().to_string(),
            client: Mutex::new(None),
            event_tx: Mutex::new(None),
            poller: Arc::new(Mutex::new(None)),
            heartbeat: Mutex::new(None),
            identity: Mutex::new(None),
            board: Mutex::new(None),
            booted: AtomicBool::new(false),
        })
    }

    fn stage(&self, stage: &'static str) {
        if let Some(b) = self.board.lock().unwrap().clone() {
            b.mark_stage(Platform::Telegram, stage);
        }
    }

    pub fn is_authenticated(&self) -> bool {
        validate_telegram_token(&self.token).is_ok()
    }
}

pub fn validate_telegram_token(token: &str) -> anyhow::Result<()> {
    let t = check_token_shape(token, "telegram token")?;
    // Telegram bot tokens are "<digits>:<alphanumeric_maybe_with_dash_underscore>"
    let Some((id_part, secret)) = t.split_once(':') else {
        anyhow::bail!("telegram token must be in form <id>:<secret> (e.g. 123456:ABC...)");
    };
    if id_part.is_empty() || !id_part.chars().all(|c| c.is_ascii_digit()) {
        anyhow::bail!("telegram token id part must be digits before ':'");
    }
    if secret.len() < 10 {
        anyhow::bail!("telegram token secret part too short");
    }
    Ok(())
}

/// Chat-type string for session keys: private → `dm`, everything else → `group`
/// (Telegram channels post as the channel, no per-user sender).
#[cfg_attr(not(feature = "telegram"), allow(dead_code))]
pub(crate) fn chat_type_for(is_private: bool, is_channel: bool) -> &'static str {
    if is_private {
        "dm"
    } else if is_channel {
        "channel"
    } else {
        "group"
    }
}

/// Build the session source. Forum topics (supergroup threads) become
/// `thread_id`, so `thread_per_user` applies.
#[cfg_attr(not(feature = "telegram"), allow(dead_code))]
pub(crate) fn source_for(
    chat_id: i64,
    chat_type: &str,
    user_id: Option<u64>,
    thread_id: Option<i32>,
    message_id: i32,
) -> SessionSource {
    SessionSource {
        platform: Platform::Telegram,
        chat_id: chat_id.to_string(),
        chat_type: chat_type.to_string(),
        user_id: user_id.map(|u| u.to_string()),
        thread_id: thread_id.map(|t| t.to_string()),
        scope_id: None,
        message_id: Some(message_id.to_string()),
    }
}

/// Parse `chat` (and optional `:thread`) as used by `gray send telegram:<chat>`.
pub fn parse_chat_target(chat: &str) -> anyhow::Result<(i64, Option<i32>)> {
    let (c, t) = match chat.split_once(':') {
        Some((c, t)) => (c, Some(t)),
        None => (chat, None),
    };
    let cid: i64 = c.trim().parse().map_err(|_| anyhow::anyhow!("invalid telegram chat id {chat:?} (expected integer, e.g. 123456 or -1001234567890)"))?;
    let tid = match t {
        Some(t) => Some(t.trim().parse::<i32>().map_err(|_| anyhow::anyhow!("invalid telegram thread id in {chat:?}"))?),
        None => None,
    };
    Ok((cid, tid))
}

#[cfg(feature = "telegram")]
fn spawn_poller(bot: teloxide::Bot, tx: UnboundedSender<MessageEvent>, drop_pending: bool, first_ok: Option<tokio::sync::oneshot::Sender<()>>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        use teloxide::prelude::*;
        use teloxide::types::{AllowedUpdate, UpdateKind};

        let mut first_ok = first_ok;
        let mut offset: Option<i32> = poller_initial_offset(drop_pending);
        let mut failures: u32 = 0;
        loop {
            let mut req = bot.get_updates().timeout(30).allowed_updates(vec![AllowedUpdate::Message]);
            if let Some(o) = offset {
                req = req.offset(o);
            }
            match req.await {
                Ok(updates) => {
                    failures = 0;
                    // First successful batch unblocks `connect()` ("confirming").
                    if let Some(tx) = first_ok.take() {
                        let _ = tx.send(());
                    }
                    for upd in updates {
                        offset = Some(upd.id.as_offset());
                        let UpdateKind::Message(m) = upd.kind else { continue };
                        // Ignore other bots — prevents loops.
                        if m.from.as_ref().map(|u| u.is_bot).unwrap_or(false) {
                            continue;
                        }
                        let text = m.text().or_else(|| m.caption()).unwrap_or("").to_string();
                        if text.is_empty() {
                            continue;
                        }
                        let chat_type = chat_type_for(m.chat.is_private(), m.chat.is_channel());
                        let user_id = m.from.as_ref().map(|u| u.id.0);
                        let user_name = m.from.as_ref().map(|u| {
                            u.username.clone().map(|n| format!("@{n}")).unwrap_or_else(|| u.first_name.clone())
                        });
                        // Only forum topics carry a meaningful thread id.
                        let thread_id = if m.is_topic_message { m.thread_id.map(|t| t.0 .0) } else { None };
                        let ev = MessageEvent {
                            text,
                            message_id: Some(m.id.0.to_string()),
                            source: source_for(m.chat.id.0, chat_type, user_id, thread_id, m.id.0),
                            media_urls: vec![],
                            user_name,
                        };
                        if tx.send(ev).is_err() {
                            log::info!("[telegram] event channel closed, stopping poller");
                            return;
                        }
                    }
                }
                Err(e) => {
                    failures += 1;
                    let delay = match &e {
                        teloxide::RequestError::RetryAfter(s) => s.duration(),
                        _ => crate::platform::backoff_delay(failures),
                    };
                    log::warn!("[telegram] getUpdates failed ({failures}): {e}; retry in {delay:?}");
                    tokio::time::sleep(delay).await;
                }
            }
        }
    })
}

#[cfg(feature = "telegram")]
fn spawn_heartbeat(
    bot: teloxide::Bot,
    tx: UnboundedSender<MessageEvent>,
    poller: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
) -> tokio::task::JoinHandle<()> {
    // NOTE: TCP keepalive would need a custom reqwest client, but
    // `teloxide::Bot::new` doesn't expose socket options (and no new deps
    // are in scope), so liveness is covered by this `get_me` heartbeat.
    tokio::spawn(async move {
        use teloxide::prelude::*;
        let mut misses = 0u32;
        let mut respawns = 0u32;
        loop {
            tokio::time::sleep(Duration::from_secs(HEARTBEAT_INTERVAL_SECS)).await;
            match bot.get_me().await {
                Ok(_) => {
                    misses = 0;
                    respawns = 0;
                }
                Err(e) => {
                    let msg = e.to_string();
                    // Revoked token is Terminal: stop respawning, don't loop forever.
                    if matches!(crate::daemon::classify_connect_error(&msg), crate::daemon::Fatal::Terminal(_)) {
                        log::error!("[telegram] heartbeat terminal (not respawning): {e}");
                        return;
                    }
                    misses += 1;
                    log::warn!("[telegram] heartbeat failed ({misses}): {e}");
                    if heartbeat_should_respawn_error(&msg, misses) {
                        let d = crate::platform::backoff_delay(respawns);
                        respawns += 1;
                        log::warn!("[telegram] heartbeat missed {misses}x, respawning poller in {d:?}");
                        tokio::time::sleep(d).await;
                        if let Some(old) = poller.lock().unwrap().take() {
                            old.abort();
                        }
                        // Respawn resumes the live queue (cold-boot drop happened once).
                        *poller.lock().unwrap() = Some(spawn_poller(bot.clone(), tx.clone(), false, None));
                        misses = 0;
                    }
                }
            }
        }
    })
}

#[cfg(feature = "telegram")]
async fn send_one(
    bot: &teloxide::Bot,
    chat: teloxide::types::ChatId,
    text: &str,
    reply_to: Option<i32>,
    thread: Option<i32>,
) -> Result<i32, teloxide::RequestError> {
    use teloxide::prelude::*;
    use teloxide::types::{MessageId, ReplyParameters, ThreadId};
    // One retry on flood control; anything else surfaces to the caller.
    for attempt in 0..2 {
        let mut req = bot.send_message(chat, text);
        if let Some(r) = reply_to {
            req = req.reply_parameters(ReplyParameters::new(MessageId(r)));
        }
        if let Some(t) = thread {
            req = req.message_thread_id(ThreadId(MessageId(t)));
        }
        match req.await {
            Ok(m) => return Ok(m.id.0),
            Err(teloxide::RequestError::RetryAfter(s)) if attempt == 0 => {
                log::warn!("[telegram] flood control, sleeping {:?}", s.duration());
                tokio::time::sleep(s.duration()).await;
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("send_one loop always returns")
}

#[async_trait::async_trait]
impl BasePlatformAdapter for TelegramAdapter {
    fn platform(&self) -> Platform {
        Platform::Telegram
    }

    fn is_authenticated(&self) -> bool {
        self.is_authenticated()
    }

    fn bot_identity(&self) -> Option<String> {
        self.identity.lock().ok().and_then(|g| g.clone())
    }

    async fn connect(&self) -> anyhow::Result<()> {
        self.stage("validating token");
        validate_telegram_token(&self.token)?;
        #[cfg(feature = "telegram")]
        {
            use teloxide::prelude::*;
            let bot = teloxide::Bot::new(self.token.clone());
            self.stage("identifying");
            // Fail fast on a rejected token.
            let me = bot.get_me().await.map_err(|e| anyhow::anyhow!("telegram token rejected: {e}"))?;
            log::info!("[telegram] authenticated as @{}", me.username());
            *self.identity.lock().unwrap() = Some(format!("@{}", me.username()));
            *self.client.lock().unwrap() = Some(bot.clone());
            self.stage("clearing webhook");
            // Polling and webhooks are mutually exclusive: clear any webhook so
            // getUpdates delivers. Best-effort — the poller surfaces real errors.
            if let Err(e) = bot.delete_webhook().await {
                log::warn!("[telegram] delete_webhook failed (continuing): {e}");
            }
            let tx = self.event_tx.lock().unwrap().clone();
            match tx {
                Some(tx) => {
                    // Cold boot drops the pending queue, reconnects resume it.
                    let drop_pending = !self.booted.swap(true, Ordering::Relaxed);
                    self.stage("polling");
                    let (ok_tx, ok_rx) = tokio::sync::oneshot::channel();
                    let handle = spawn_poller(bot.clone(), tx.clone(), drop_pending, Some(ok_tx));
                    if let Some(old) = self.poller.lock().unwrap().replace(handle) {
                        old.abort();
                    }
                    let hb = spawn_heartbeat(bot, tx, Arc::clone(&self.poller));
                    if let Some(old) = self.heartbeat.lock().unwrap().replace(hb) {
                        old.abort();
                    }
                    self.stage("confirming");
                    ok_rx.await.map_err(|_| anyhow::anyhow!("telegram poller ended before first updates batch"))?;
                    log::info!("[telegram] long-polling started");
                }
                None => log::info!("[telegram] send-only mode (no event channel wired)"),
            }
        }
        #[cfg(not(feature = "telegram"))]
        {
            self.stage("identifying");
            self.stage("clearing webhook");
            self.stage("polling");
            self.stage("confirming");
            log::info!("[telegram] stub connect (token {}…)", &self.token[..self.token.len().min(6)]);
        }
        Ok(())
    }

    async fn disconnect(&self) -> anyhow::Result<()> {
        if let Some(h) = self.heartbeat.lock().unwrap().take() {
            h.abort();
        }
        if let Some(h) = self.poller.lock().unwrap().take() {
            h.abort();
        }
        *self.client.lock().unwrap() = None;
        log::info!("[telegram] disconnected");
        Ok(())
    }

    fn set_event_tx(&mut self, tx: UnboundedSender<MessageEvent>) {
        *self.event_tx.lock().unwrap() = Some(tx);
    }

    fn set_status_board(&self, board: GatewayStatusBoard) {
        *self.board.lock().unwrap() = Some(board);
    }

    async fn send_typing(&self, chat: &str) {
        #[cfg(feature = "telegram")]
        {
            use teloxide::prelude::*;
            use teloxide::types::ChatAction;
            let bot = self.client.lock().unwrap().clone();
            if let (Some(bot), Ok((cid, _))) = (bot, parse_chat_target(chat)) {
                let _ = bot.send_chat_action(ChatId(cid), ChatAction::Typing).await;
            }
        }
        #[cfg(not(feature = "telegram"))]
        let _ = chat;
    }

    fn supports_edit(&self) -> bool {
        cfg!(feature = "telegram")
    }

    async fn edit_message(&self, chat: &str, message_id: &str, text: &str) -> SendResult {
        #[cfg(feature = "telegram")]
        {
            use teloxide::prelude::*;
            use teloxide::types::MessageId;
            let Some(bot) = self.client.lock().unwrap().clone() else {
                return SendResult::fail("telegram not connected", false);
            };
            let Ok((cid, _)) = parse_chat_target(chat) else {
                return SendResult::fail(format!("invalid telegram chat id {chat:?}"), false);
            };
            let Ok(mid) = message_id.parse::<i32>() else {
                return SendResult::fail(format!("invalid telegram message id {message_id:?}"), false);
            };
            if utf16_len(text) > MAX_LENGTH {
                return SendResult::fail("edit text exceeds 4096 utf16 units", false);
            }
            return match bot.edit_message_text(ChatId(cid), MessageId(mid), text).await {
                Ok(_) => SendResult::ok(Some(message_id.to_string())),
                // "message is not modified" is a no-op success for our purposes.
                Err(e) if e.to_string().contains("not modified") => SendResult::ok(Some(message_id.to_string())),
                Err(e) => SendResult::fail(format!("telegram edit: {e}"), true),
            };
        }
        #[cfg(not(feature = "telegram"))]
        {
            log::info!("[telegram] edit {chat}/{message_id} ({} utf16)", utf16_len(text));
            SendResult::ok(Some(message_id.to_string()))
        }
    }

    async fn delete_message(&self, chat: &str, message_id: &str) -> SendResult {
        #[cfg(feature = "telegram")]
        {
            use teloxide::prelude::*;
            use teloxide::types::MessageId;
            let Some(bot) = self.client.lock().unwrap().clone() else {
                return SendResult::fail("telegram not connected", false);
            };
            let Ok((cid, _)) = parse_chat_target(chat) else {
                return SendResult::fail(format!("invalid telegram chat id {chat:?}"), false);
            };
            let Ok(mid) = message_id.parse::<i32>() else {
                return SendResult::fail(format!("invalid telegram message id {message_id:?}"), false);
            };
            return match bot.delete_message(ChatId(cid), MessageId(mid)).await {
                Ok(_) => SendResult::ok(Some(message_id.to_string())),
                Err(e) => SendResult::fail(format!("telegram delete: {e}"), true),
            };
        }
        #[cfg(not(feature = "telegram"))]
        {
            log::info!("[telegram] delete {chat}/{message_id}");
            SendResult::ok(Some(message_id.to_string()))
        }
    }

    async fn send(&self, chat: &str, text: &str) -> SendResult {
        self.send_ext(chat, text, &SendOptions::default()).await
    }

    async fn send_ext(&self, chat: &str, text: &str, opts: &SendOptions) -> SendResult {
        if !self.is_authenticated() {
            return SendResult::fail("telegram not authenticated: invalid token", false);
        }
        if text.is_empty() {
            return SendResult::ok(None);
        }

        // Split long messages into 4096-unit chunks.
        let chunks = crate::platform::split_message_smart(text, MAX_LENGTH);

        #[cfg(feature = "telegram")]
        {
            use teloxide::types::ChatId;
            let Some(bot) = self.client.lock().unwrap().clone() else {
                return SendResult::fail("telegram not connected", false);
            };
            let (cid, target_thread) = match parse_chat_target(chat) {
                Ok(v) => v,
                Err(e) => return SendResult::fail(e.to_string(), false),
            };
            let thread = opts.thread_id.as_deref().and_then(|t| t.parse::<i32>().ok()).or(target_thread);
            let reply_to = opts.reply_to.as_deref().and_then(|r| r.parse::<i32>().ok());
            let mut last_id = None;
            for (i, chunk) in chunks.iter().enumerate() {
                debug_assert!(utf16_len(chunk) <= MAX_LENGTH, "chunk exceeds limit");
                let r = if i == 0 { reply_to } else { None };
                match send_one(&bot, ChatId(cid), chunk, r, thread).await {
                    Ok(id) => last_id = Some(id.to_string()),
                    // Reply target vanished → retry the first chunk without reply.
                    Err(e) if i == 0 && r.is_some() && e.to_string().contains("not found") => {
                        match send_one(&bot, ChatId(cid), chunk, None, thread).await {
                            Ok(id) => last_id = Some(id.to_string()),
                            Err(e) => return SendResult::fail(format!("telegram send: {e}"), true),
                        }
                    }
                    Err(e) => {
                        log::warn!("[telegram] send chunk {}/{} failed: {e}", i + 1, chunks.len());
                        return SendResult::fail(format!("telegram send: {e}"), true);
                    }
                }
            }
            return SendResult::ok(last_id);
        }
        #[cfg(not(feature = "telegram"))]
        {
            for (i, chunk) in chunks.iter().enumerate() {
                debug_assert!(utf16_len(chunk) <= MAX_LENGTH, "chunk exceeds limit");
                log::info!(
                    "[telegram] send to {} chunk {}/{} ({} utf16, reply_to={:?}, thread={:?}): {:?}",
                    chat, i + 1, chunks.len(), utf16_len(chunk), opts.reply_to, opts.thread_id,
                    crate::platform::preview_80(chunk)
                );
            }
            SendResult::ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PlatformConfig;
    use crate::platform::{utf16_len, BasePlatformAdapter};
    use crate::session::build_session_key;

    fn cfg(token: &str) -> PlatformConfig {
        PlatformConfig::with_token(token)
    }

    #[test]
    fn validate_good_token() {
        assert!(validate_telegram_token("123456:ABCdefGHIjklMNOpqr-123").is_ok());
    }

    #[test]
    fn validate_bad_tokens() {
        assert!(validate_telegram_token("").is_err());
        assert!(validate_telegram_token("notoken").is_err());
        assert!(validate_telegram_token("abc:short").is_err());
        assert!(validate_telegram_token("abc:1234567890123").is_err());
        assert!(validate_telegram_token("123456: short with space").is_err());
    }

    #[test]
    fn new_rejects_invalid() {
        assert!(TelegramAdapter::new(cfg("bad")).is_err());
        assert!(TelegramAdapter::new(cfg("123:short")).is_err());
        assert!(TelegramAdapter::new(PlatformConfig { enabled: true, token: None, ..Default::default() }).is_err());
    }

    #[tokio::test]
    async fn send_splits_long() {
        let a = TelegramAdapter::new(cfg("123456:ABCDEFGHIJ1234567890")).unwrap();
        // 5000 'a' -> needs 2 chunks (4096 + 904)
        let long = "a".repeat(5000);
        let chunks = crate::platform::split_message_smart(&long, MAX_LENGTH);
        assert_eq!(chunks.len(), 2);
        for c in &chunks { assert!(utf16_len(c) <= MAX_LENGTH); }
        // Live send only when the real client is compiled in and connected.
        #[cfg(not(feature = "telegram"))]
        {
            let res = a.send("123", &long).await;
            assert!(res.success);
        }
        #[cfg(feature = "telegram")]
        {
            let res = a.send("123", &long).await;
            assert!(!res.success, "not connected → must fail cleanly");
            assert_eq!(res.error.as_deref(), Some("telegram not connected"));
        }
    }

    #[tokio::test]
    async fn send_truncate_emoji() {
        let a = TelegramAdapter::new(cfg("123:ABCDEFGHIJ1234567890")).unwrap();
        let s = "😀".repeat(3000); // 6000 units > 4096
        for c in crate::platform::split_message_smart(&s, MAX_LENGTH) {
            assert!(utf16_len(&c) <= MAX_LENGTH);
        }
        #[cfg(not(feature = "telegram"))]
        assert!(a.send("123", &s).await.success);
        #[cfg(feature = "telegram")]
        assert!(!a.send("123", &s).await.success);
    }

    #[test]
    fn is_authenticated_check() {
        let a = TelegramAdapter::new(cfg("999:ABCDEFGHIJ1234567890")).unwrap();
        assert!(a.is_authenticated());
        assert!(BasePlatformAdapter::is_authenticated(&a));
        assert_eq!(a.platform(), Platform::Telegram);
    }

    #[test]
    fn session_key_routing() {
        // DM: keyed by chat only (user ignored).
        let dm = source_for(42, "dm", Some(42), None, 7);
        assert_eq!(build_session_key(&dm, true, false), "gray:main:telegram:dm:42");
        // Supergroup without topics: per-user by default.
        let g = source_for(-1001, "group", Some(42), None, 8);
        assert_eq!(build_session_key(&g, true, false), "gray:main:telegram:group:-1001:42");
        assert_eq!(build_session_key(&g, false, false), "gray:main:telegram:group:-1001");
        // Forum topic: thread id in key; thread_per_user governs user suffix.
        let t = source_for(-1001, "group", Some(42), Some(99), 9);
        assert_eq!(build_session_key(&t, true, false), "gray:main:telegram:group:-1001:thread_99");
        assert_eq!(build_session_key(&t, true, true), "gray:main:telegram:group:-1001:thread_99:42");
        assert_eq!(chat_type_for(true, false), "dm");
        assert_eq!(chat_type_for(false, true), "channel");
        assert_eq!(chat_type_for(false, false), "group");
    }

    #[test]
    fn parse_targets() {
        assert_eq!(parse_chat_target("123").unwrap(), (123, None));
        assert_eq!(parse_chat_target("-1001234:55").unwrap(), (-1001234, Some(55)));
        assert!(parse_chat_target("abc").is_err());
        assert!(parse_chat_target("1:x").is_err());
    }

    #[test]
    fn heartbeat_respawns_after_two_consecutive_failures() {
        assert_eq!(HEARTBEAT_INTERVAL_SECS, 60);
        assert_eq!(HEARTBEAT_MAX_MISSES, 2);
        assert!(!heartbeat_should_respawn(0));
        assert!(!heartbeat_should_respawn(1));
        assert!(heartbeat_should_respawn(2));
        assert!(heartbeat_should_respawn(3));
        // One success clears the miss count.
        assert_eq!(drive_heartbeat([true, false, true, false]), (1, false));
        // Two in a row trips respawn.
        assert_eq!(drive_heartbeat([true, false, false]), (2, true));
        assert_eq!(drive_heartbeat([false, false]), (2, true));
        assert_eq!(drive_heartbeat([true]), (0, false));
    }

    #[test]
    fn heartbeat_terminal_never_respawns_retryable_does() {
        // Revoked token: Terminal → no respawn even past the miss threshold.
        for err in ["401 Unauthorized", "telegram token rejected: forbidden", "invalid token"] {
            assert!(!heartbeat_should_respawn_error(err, 2), "terminal must not respawn: {err}");
            assert!(!heartbeat_should_respawn_error(err, 10), "terminal must not respawn: {err}");
        }
        // Transient: Retryable → respawn once misses hit the threshold.
        assert!(!heartbeat_should_respawn_error("connection reset by peer", 1));
        assert!(heartbeat_should_respawn_error("connection reset by peer", 2));
        assert!(heartbeat_should_respawn_error("connect timeout 45s", 3));
    }

    #[test]
    fn poller_offset_flag_cold_boot_drops_reconnect_resumes() {
        assert_eq!(poller_initial_offset(true), Some(-1), "cold boot starts at live head");
        assert_eq!(poller_initial_offset(false), None, "reconnect resumes the live queue");
    }

    #[tokio::test]
    async fn stub_connect_walks_stages() {
        // Stub-only: no network, connect still walks the staged path.
        #[cfg(not(feature = "telegram"))]
        {
            use crate::status::{GatewayStatusBoard, PlatformConnState};
            let a = TelegramAdapter::new(cfg("123456:ABCDEFGHIJ1234567890")).unwrap();
            let board = GatewayStatusBoard::new(&[Platform::Telegram]);
            a.set_status_board(board.clone());
            a.connect().await.unwrap();
            assert_eq!(
                board.snapshot()[0].1,
                PlatformConnState::Connecting { stage: "confirming" },
                "stub ends on the last pre-connected stage; the daemon marks connected"
            );
        }
    }

    #[tokio::test]
    async fn edit_and_typing_are_safe_without_connection() {
        let a = TelegramAdapter::new(cfg("999:ABCDEFGHIJ1234567890")).unwrap();
        a.send_typing("123").await; // must not panic
        let r = a.edit_message("123", "5", "hi").await;
        #[cfg(not(feature = "telegram"))]
        assert!(r.success);
        #[cfg(feature = "telegram")]
        assert!(!r.success);
        assert!(a.disconnect().await.is_ok());
    }
}
