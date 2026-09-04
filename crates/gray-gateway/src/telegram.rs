//! Telegram adapter — stub by default, real long-polling behind `telegram` feature.
//!
//! Enable: `cargo check -p gray-gateway --features telegram` (teloxide 0.17, rustls).
//! Features (hermes `plugins/platforms/telegram/adapter.py` parity): inbound
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
use std::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;

pub const MAX_LENGTH: usize = 4096;

#[cfg(feature = "telegram")]
type BotClient = teloxide::Bot;
#[cfg(not(feature = "telegram"))]
type BotClient = ();

pub struct TelegramAdapter {
    token: String,
    client: Mutex<Option<BotClient>>,
    event_tx: Mutex<Option<UnboundedSender<MessageEvent>>>,
    /// Polling task handle so `disconnect` can stop it.
    poller: Mutex<Option<tokio::task::JoinHandle<()>>>,
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
            poller: Mutex::new(None),
        })
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
/// `thread_id`, so `thread_per_user` applies (hermes `thread_id=message_thread_id`).
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
fn spawn_poller(bot: teloxide::Bot, tx: UnboundedSender<MessageEvent>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        use teloxide::prelude::*;
        use teloxide::types::{AllowedUpdate, UpdateKind};

        let mut offset: Option<i32> = None;
        let mut failures: u32 = 0;
        loop {
            let mut req = bot.get_updates().timeout(30).allowed_updates(vec![AllowedUpdate::Message]);
            if let Some(o) = offset {
                req = req.offset(o);
            }
            match req.await {
                Ok(updates) => {
                    failures = 0;
                    for upd in updates {
                        offset = Some(upd.id.as_offset());
                        let UpdateKind::Message(m) = upd.kind else { continue };
                        // Ignore other bots (hermes: skip bot senders) — prevents loops.
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

    async fn connect(&self) -> anyhow::Result<()> {
        validate_telegram_token(&self.token)?;
        #[cfg(feature = "telegram")]
        {
            use teloxide::prelude::*;
            let bot = teloxide::Bot::new(self.token.clone());
            // Fail fast on a rejected token (hermes get_me parity).
            let me = bot.get_me().await.map_err(|e| anyhow::anyhow!("telegram token rejected: {e}"))?;
            log::info!("[telegram] authenticated as @{}", me.username());
            *self.client.lock().unwrap() = Some(bot.clone());
            let tx = self.event_tx.lock().unwrap().clone();
            match tx {
                Some(tx) => {
                    let handle = spawn_poller(bot, tx);
                    if let Some(old) = self.poller.lock().unwrap().replace(handle) {
                        old.abort();
                    }
                    log::info!("[telegram] long-polling started");
                }
                None => log::info!("[telegram] send-only mode (no event channel wired)"),
            }
        }
        #[cfg(not(feature = "telegram"))]
        {
            log::info!("[telegram] stub connect (token {}…)", &self.token[..self.token.len().min(6)]);
        }
        Ok(())
    }

    async fn disconnect(&self) -> anyhow::Result<()> {
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

        // Split long messages into 4096-unit chunks (hermes split_long_messages).
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
