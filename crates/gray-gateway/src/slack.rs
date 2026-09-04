//! Slack adapter — stub by default, real Socket Mode behind `slack` feature.
//!
//! Slack uses two tokens in Socket Mode:
//! - `token` = bot token `xoxb-...` (Web API: chat.postMessage / chat.update / auth.test)
//! - `app_token` = app-level token `xapp-...` (Socket Mode websocket, `connections:write`)
//! Bot-token-only mode still works for *sending* (`gray send slack:C123 …`);
//! inbound requires the app token.
//!
//! Enable: `cargo check -p gray-gateway --features slack` (slack-morphism 2, hyper+rustls).
//! Features (hermes `plugins/platforms/slack/adapter.py` parity): channels,
//! DMs (`im`), group DMs (`mpim`), thread replies (`thread_ts` → session
//! `thread_id`), 39000-unit chunking, edit-in-place streaming, bot-echo
//! suppression via `auth.test` user id. Socket Mode reconnects are handled by
//! slack-morphism's client manager; we additionally retry the initial
//! connection with exponential backoff.
//!
//! Required app scopes: `chat:write`, `channels:history`, `groups:history`,
//! `im:history`, `mpim:history`, `app_mentions:read`; events: `message.im`,
//! `message.channels`, `message.groups`, `message.mpim`.

use crate::config::{Platform, PlatformConfig};
use crate::platform::{check_token_shape, utf16_len, BasePlatformAdapter, MessageEvent, SendOptions, SendResult};
use crate::session::SessionSource;
use std::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;

pub const MAX_LENGTH: usize = 39000;

#[cfg(feature = "slack")]
type WebClient = std::sync::Arc<slack_morphism::hyper_tokio::SlackHyperClient>;
#[cfg(not(feature = "slack"))]
type WebClient = ();

pub struct SlackAdapter {
    bot_token: String,
    app_token: Option<String>,
    client: Mutex<Option<WebClient>>,
    event_tx: Mutex<Option<UnboundedSender<MessageEvent>>>,
    /// Socket Mode task; aborted on disconnect.
    listener: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Bot name from `auth.test` (set on connect, read by the boot card).
    identity: Mutex<Option<String>>,
}

impl SlackAdapter {
    pub fn new(cfg: PlatformConfig) -> anyhow::Result<Self> {
        let bot_token = cfg
            .token
            .ok_or_else(|| anyhow::anyhow!("slack token not set (set platforms.slack.token to xoxb-... in gateway.yaml)"))?;
        validate_slack_bot_token(&bot_token)?;
        let app_token = cfg.app_token.clone().map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        if let Some(ref t) = app_token {
            validate_slack_app_token(t)?;
        }
        Ok(Self {
            bot_token: bot_token.trim().to_string(),
            app_token,
            client: Mutex::new(None),
            event_tx: Mutex::new(None),
            listener: Mutex::new(None),
            identity: Mutex::new(None),
        })
    }

    pub fn is_authenticated(&self) -> bool {
        validate_slack_bot_token(&self.bot_token).is_ok()
            && self.app_token.as_ref().map(|t| validate_slack_app_token(t).is_ok()).unwrap_or(true)
    }

    pub fn has_socket_mode(&self) -> bool {
        self.app_token.is_some()
    }
}

pub fn validate_slack_bot_token(token: &str) -> anyhow::Result<()> {
    let t = check_token_shape(token, "slack bot token")?;
    if !(t.starts_with("xoxb-") || t.starts_with("xoxp-")) {
        anyhow::bail!("slack token must start with xoxb- (bot) or xoxp- (user); got prefix {:?}", &t[..t.len().min(5)]);
    }
    if t.len() < 10 {
        anyhow::bail!("slack token too short");
    }
    Ok(())
}

pub fn validate_slack_app_token(token: &str) -> anyhow::Result<()> {
    let t = check_token_shape(token, "slack app token")?;
    if !t.starts_with("xapp-") {
        anyhow::bail!("slack app_token must start with xapp- (Socket Mode); got {:?}", &t[..t.len().min(5)]);
    }
    if t.len() < 10 {
        anyhow::bail!("slack app_token too short");
    }
    Ok(())
}

/// Map Slack `channel_type` to session chat type: `im` → dm, `mpim`/`group`/`channel` → channel.
#[cfg_attr(not(feature = "slack"), allow(dead_code))]
pub(crate) fn chat_type_for(channel_type: Option<&str>, channel_id: &str) -> &'static str {
    match channel_type {
        Some("im") => "dm",
        Some("mpim") | Some("group") | Some("channel") => "channel",
        // channel_type missing: infer from id prefix (D = DM).
        _ if channel_id.starts_with('D') => "dm",
        _ => "channel",
    }
}

#[cfg_attr(not(feature = "slack"), allow(dead_code))]
pub(crate) fn source_for(
    team_id: &str,
    channel_id: &str,
    chat_type: &str,
    user_id: Option<&str>,
    thread_ts: Option<&str>,
    ts: &str,
) -> SessionSource {
    SessionSource {
        platform: Platform::Slack,
        chat_id: channel_id.to_string(),
        chat_type: chat_type.to_string(),
        user_id: user_id.map(str::to_string),
        thread_id: thread_ts.map(str::to_string),
        scope_id: (!team_id.is_empty()).then(|| team_id.to_string()),
        message_id: Some(ts.to_string()),
    }
}

/// Parse `C123` / `C123:1700000000.000100` (channel + optional thread_ts).
pub fn parse_chat_target(chat: &str) -> anyhow::Result<(String, Option<String>)> {
    let (c, t) = match chat.split_once(':') {
        Some((c, t)) => (c.trim(), Some(t.trim().to_string())),
        None => (chat.trim(), None),
    };
    if c.is_empty() || !c.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        anyhow::bail!("invalid slack channel id {chat:?} (expected e.g. C0123456789 or D0123456789)");
    }
    Ok((c.to_string(), t.filter(|t| !t.is_empty())))
}

#[cfg(feature = "slack")]
mod live {
    use super::*;
    use slack_morphism::hyper_tokio::SlackHyperClient;
    use slack_morphism::prelude::*;
    use std::sync::Arc;

    /// Shared with the Socket Mode callbacks through slack-morphism's user state.
    pub struct ListenerState {
        pub tx: UnboundedSender<MessageEvent>,
        pub bot_user_id: String,
    }

    pub async fn on_push_event(
        event: SlackPushEventCallback,
        _client: Arc<SlackHyperClient>,
        states: SlackClientEventsUserState,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let SlackEventCallbackBody::Message(m) = event.event else { return Ok(()) };
        // Skip edits/deletes/joins/bot posts — only fresh human text.
        if m.subtype.is_some() || m.sender.bot_id.is_some() {
            return Ok(());
        }
        let Some(channel) = m.origin.channel.as_ref() else { return Ok(()) };
        let text = m.content.as_ref().and_then(|c| c.text.clone()).unwrap_or_default();
        if text.trim().is_empty() {
            return Ok(());
        }
        let user_id = m.sender.user.as_ref().map(|u| u.to_string());
        let guard = states.read().await;
        let Some(st) = guard.get_user_state::<ListenerState>() else { return Ok(()) };
        if user_id.as_deref() == Some(st.bot_user_id.as_str()) {
            return Ok(());
        }
        let channel_id = channel.to_string();
        let ctype = m.origin.channel_type.as_ref().map(|t| t.to_string());
        let chat_type = chat_type_for(ctype.as_deref(), &channel_id);
        let thread_ts = m.origin.thread_ts.as_ref().map(|t| t.to_string());
        let ts = m.origin.ts.to_string();
        let user_name = m.sender.user_profile.as_ref().and_then(|p| p.display_name.clone().or(p.real_name.clone()));
        let ev = MessageEvent {
            text,
            message_id: Some(ts.clone()),
            source: source_for(&event.team_id.to_string(), &channel_id, chat_type, user_id.as_deref(), thread_ts.as_deref(), &ts),
            media_urls: vec![],
            user_name,
        };
        let _ = st.tx.send(ev);
        Ok(())
    }

    pub fn on_error(
        err: Box<dyn std::error::Error + Send + Sync>,
        _client: Arc<SlackHyperClient>,
        _states: SlackClientEventsUserState,
    ) -> HttpStatusCode {
        log::warn!("[slack] listener error: {err}");
        HttpStatusCode::OK
    }

    pub fn spawn_socket_mode(
        client: Arc<SlackHyperClient>,
        app_token: String,
        state: ListenerState,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let callbacks = SlackSocketModeListenerCallbacks::new().with_push_events(on_push_event);
            let env = Arc::new(
                SlackClientEventsListenerEnvironment::new(client).with_error_handler(on_error).with_user_state(state),
            );
            let listener = SlackClientSocketModeListener::new(&SlackClientSocketModeConfig::new(), env, callbacks);
            let token = SlackApiToken::new(app_token.into());
            let mut attempt = 0u32;
            loop {
                match listener.listen_for(&token).await {
                    Ok(()) => break,
                    Err(e) => {
                        attempt += 1;
                        let d = crate::platform::backoff_delay(attempt);
                        log::warn!("[slack] socket mode connect failed ({attempt}): {e}; retry in {d:?}");
                        tokio::time::sleep(d).await;
                    }
                }
            }
            log::info!("[slack] socket mode connected");
            // `serve` blocks until shutdown and lets slack-morphism reconnect on its own.
            let _ = listener.serve().await;
        })
    }
}

#[async_trait::async_trait]
impl BasePlatformAdapter for SlackAdapter {
    fn platform(&self) -> Platform {
        Platform::Slack
    }

    fn is_authenticated(&self) -> bool {
        self.is_authenticated()
    }

    fn bot_identity(&self) -> Option<String> {
        self.identity.lock().ok().and_then(|g| g.clone())
    }

    async fn connect(&self) -> anyhow::Result<()> {
        validate_slack_bot_token(&self.bot_token)?;
        if let Some(ref t) = self.app_token {
            validate_slack_app_token(t)?;
        }
        #[cfg(feature = "slack")]
        {
            use slack_morphism::hyper_tokio::SlackClientHyperConnector;
            use slack_morphism::prelude::*;
            let client = std::sync::Arc::new(SlackClient::new(SlackClientHyperConnector::new()?));
            let token = SlackApiToken::new(self.bot_token.clone().into());
            let me = client
                .open_session(&token)
                .auth_test()
                .await
                .map_err(|e| anyhow::anyhow!("slack bot token rejected: {e}"))?;
            log::info!("[slack] authenticated as {} in {}", me.user.as_deref().unwrap_or("?"), me.team);
            *self.identity.lock().unwrap() = me.user.clone().filter(|s| !s.trim().is_empty());
            *self.client.lock().unwrap() = Some(client.clone());
            let tx = self.event_tx.lock().unwrap().clone();
            match (tx, self.app_token.clone()) {
                (Some(tx), Some(app)) => {
                    let state = live::ListenerState { tx, bot_user_id: me.user_id.to_string() };
                    let handle = live::spawn_socket_mode(client, app, state);
                    if let Some(old) = self.listener.lock().unwrap().replace(handle) {
                        old.abort();
                    }
                }
                (Some(_), None) => log::warn!("[slack] no app_token (xapp-…): inbound disabled, send-only"),
                (None, _) => log::info!("[slack] send-only mode (no event channel wired)"),
            }
        }
        #[cfg(not(feature = "slack"))]
        {
            log::info!("[slack] stub connect bot={}… app_token={}", &self.bot_token[..self.bot_token.len().min(8)], self.has_socket_mode());
        }
        Ok(())
    }

    async fn disconnect(&self) -> anyhow::Result<()> {
        if let Some(h) = self.listener.lock().unwrap().take() {
            h.abort();
        }
        *self.client.lock().unwrap() = None;
        log::info!("[slack] disconnected");
        Ok(())
    }

    fn set_event_tx(&mut self, tx: UnboundedSender<MessageEvent>) {
        *self.event_tx.lock().unwrap() = Some(tx);
    }

    fn supports_edit(&self) -> bool {
        cfg!(feature = "slack")
    }

    async fn edit_message(&self, chat: &str, message_id: &str, text: &str) -> SendResult {
        #[cfg(feature = "slack")]
        {
            use slack_morphism::prelude::*;
            let Some(client) = self.client.lock().unwrap().clone() else {
                return SendResult::fail("slack not connected", false);
            };
            let Ok((channel, _)) = parse_chat_target(chat) else {
                return SendResult::fail(format!("invalid slack channel {chat:?}"), false);
            };
            if utf16_len(text) > MAX_LENGTH {
                return SendResult::fail("edit text exceeds 39000 utf16 units", false);
            }
            let token = SlackApiToken::new(self.bot_token.clone().into());
            let req = SlackApiChatUpdateRequest::new(
                SlackChannelId(channel),
                SlackMessageContent::new().with_text(text.to_string()),
                SlackTs(message_id.to_string()),
            );
            return match client.open_session(&token).chat_update(&req).await {
                Ok(r) => SendResult::ok(Some(r.ts.to_string())),
                Err(e) => SendResult::fail(format!("slack edit: {e}"), true),
            };
        }
        #[cfg(not(feature = "slack"))]
        {
            log::info!("[slack] edit {chat}/{message_id} ({} utf16)", utf16_len(text));
            SendResult::ok(Some(message_id.to_string()))
        }
    }

    async fn send(&self, chat: &str, text: &str) -> SendResult {
        self.send_ext(chat, text, &SendOptions::default()).await
    }

    async fn send_ext(&self, chat: &str, text: &str, opts: &SendOptions) -> SendResult {
        if !self.is_authenticated() {
            return SendResult::fail("slack not authenticated: invalid token", false);
        }
        if text.is_empty() {
            return SendResult::ok(None);
        }

        let chunks = crate::platform::split_message_smart(text, MAX_LENGTH);

        #[cfg(feature = "slack")]
        {
            use slack_morphism::prelude::*;
            let Some(client) = self.client.lock().unwrap().clone() else {
                return SendResult::fail("slack not connected", false);
            };
            let (channel, target_thread) = match parse_chat_target(chat) {
                Ok(v) => v,
                Err(e) => return SendResult::fail(e.to_string(), false),
            };
            let thread = opts.thread_id.clone().or(target_thread);
            let token = SlackApiToken::new(self.bot_token.clone().into());
            let session = client.open_session(&token);
            let mut last_ts = None;
            for (i, chunk) in chunks.iter().enumerate() {
                debug_assert!(utf16_len(chunk) <= MAX_LENGTH);
                let mut req = SlackApiChatPostMessageRequest::new(
                    SlackChannelId(channel.clone()),
                    SlackMessageContent::new().with_text(chunk.clone()),
                );
                if let Some(t) = thread.as_ref() {
                    req = req.with_thread_ts(SlackTs(t.clone()));
                }
                match session.chat_post_message(&req).await {
                    Ok(r) => last_ts = Some(r.ts.to_string()),
                    Err(e) => {
                        log::warn!("[slack] send chunk {}/{} failed: {e}", i + 1, chunks.len());
                        return SendResult::fail(format!("slack send: {e}"), true);
                    }
                }
            }
            return SendResult::ok(last_ts);
        }
        #[cfg(not(feature = "slack"))]
        {
            for (i, chunk) in chunks.iter().enumerate() {
                debug_assert!(utf16_len(chunk) <= MAX_LENGTH);
                log::info!(
                    "[slack] send to {} chunk {}/{} ({} utf16, thread={:?}): {:?}",
                    chat, i + 1, chunks.len(), utf16_len(chunk), opts.thread_id,
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

    fn cfg(bot: &str, app: Option<&str>) -> PlatformConfig {
        PlatformConfig { app_token: app.map(str::to_string), ..PlatformConfig::with_token(bot) }
    }

    #[test]
    fn validate_tokens() {
        assert!(validate_slack_bot_token("xoxb-1234567890-abc").is_ok());
        assert!(validate_slack_bot_token("xoxp-1234567890-abc").is_ok());
        assert!(validate_slack_bot_token("xapp-1234567890").is_err());
        assert!(validate_slack_bot_token("").is_err());
        assert!(validate_slack_bot_token("xoxb-").is_err());
        assert!(validate_slack_app_token("xapp-1-A123-456-abc").is_ok());
        assert!(validate_slack_app_token("xoxb-1234567890").is_err());
        assert!(validate_slack_app_token("xapp-1 2").is_err());
    }

    #[test]
    fn new_and_socket_mode_flag() {
        let a = SlackAdapter::new(cfg("xoxb-1234567890-abc", None)).unwrap();
        assert!(a.is_authenticated());
        assert!(!a.has_socket_mode());
        let b = SlackAdapter::new(cfg("xoxb-1234567890-abc", Some("xapp-1-A1-2-abc"))).unwrap();
        assert!(b.has_socket_mode());
        assert!(SlackAdapter::new(cfg("xoxb-1234567890-abc", Some("bad"))).is_err());
        assert!(SlackAdapter::new(cfg("nope", None)).is_err());
        assert_eq!(b.platform(), Platform::Slack);
    }

    #[tokio::test]
    async fn send_splits() {
        let a = SlackAdapter::new(cfg("xoxb-1234567890-abc", None)).unwrap();
        let long = "a".repeat(80_000);
        let chunks = crate::platform::split_message_smart(&long, MAX_LENGTH);
        assert_eq!(chunks.len(), 3); // 39000*2 + 2000
        for c in &chunks { assert!(utf16_len(c) <= MAX_LENGTH); }
        #[cfg(not(feature = "slack"))]
        assert!(a.send("C123", &long).await.success);
        #[cfg(feature = "slack")]
        {
            let r = a.send("C123", &long).await;
            assert!(!r.success);
            assert_eq!(r.error.as_deref(), Some("slack not connected"));
        }
    }

    #[test]
    fn session_key_routing() {
        let dm = source_for("T1", "D42", "dm", Some("U1"), None, "1.0");
        assert_eq!(build_session_key(&dm, true, false), "gray:main:slack:dm:T1:D42");
        let ch = source_for("T1", "C9", "channel", Some("U1"), None, "1.0");
        assert_eq!(build_session_key(&ch, true, false), "gray:main:slack:channel:T1:C9:U1");
        let th = source_for("T1", "C9", "channel", Some("U1"), Some("1700.5"), "1701.0");
        assert_eq!(build_session_key(&th, true, false), "gray:main:slack:channel:T1:C9:thread_1700.5");
        assert_eq!(build_session_key(&th, true, true), "gray:main:slack:channel:T1:C9:thread_1700.5:U1");
        assert_eq!(chat_type_for(Some("im"), "D1"), "dm");
        assert_eq!(chat_type_for(Some("mpim"), "G1"), "channel");
        assert_eq!(chat_type_for(None, "D1"), "dm");
        assert_eq!(chat_type_for(None, "C1"), "channel");
    }

    #[test]
    fn parse_targets() {
        assert_eq!(parse_chat_target("C123").unwrap(), ("C123".into(), None));
        assert_eq!(parse_chat_target("C123:1700.1").unwrap(), ("C123".into(), Some("1700.1".into())));
        assert!(parse_chat_target("").is_err());
        assert!(parse_chat_target("#general").is_err());
    }

    #[tokio::test]
    async fn edit_without_connection_is_safe() {
        let a = SlackAdapter::new(cfg("xoxb-1234567890-abc", None)).unwrap();
        a.send_typing("C1").await;
        let r = a.edit_message("C1", "1.0", "x").await;
        #[cfg(not(feature = "slack"))]
        assert!(r.success);
        #[cfg(feature = "slack")]
        assert!(!r.success);
        assert!(a.disconnect().await.is_ok());
    }
}
