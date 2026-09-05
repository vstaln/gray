//! Discord adapter — stub by default, real gateway+REST behind `discord` feature.
//!
//! Enable: `cargo check -p gray-gateway --features discord` (twilight 0.16).
//! Features: inbound messages via twilight-gateway, replies via
//! twilight-http with reply-on-first-chunk, persistent typing loop, slash
//! commands /ask /reset /status /stop.

use crate::config::{Platform, PlatformConfig};
use crate::platform::{
    BasePlatformAdapter, MessageEvent, SendOptions, SendResult, check_token_shape, utf16_len,
};
use crate::session::SessionSource;
use crate::status::GatewayStatusBoard;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;

pub const MAX_LENGTH: usize = 2000;

/// OAuth2 invite permissions: View Channels + Send Messages + Read Message History.
pub const INVITE_PERMISSIONS: u64 = 1024 + 2048 + 65536;

/// Recover the Application (client) ID from a bot token: the first
/// dot-segment is the base64-encoded ID (classic Discord token layout).
/// Returns None for tokens that don't follow that layout.
pub fn client_id_from_token(token: &str) -> Option<String> {
    let raw = token.trim().strip_prefix("Bot ").unwrap_or(token.trim());
    let seg = raw.split('.').next()?;
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(seg)
        .ok()?;
    let id = String::from_utf8(bytes).ok()?;
    if id.chars().all(|c| c.is_ascii_digit()) && (15..=21).contains(&id.len()) {
        Some(id)
    } else {
        None
    }
}

/// Build the OAuth2 invite URL for `client_id` (Application ID, numeric).
/// Normally you never pass one by hand: the ID is derived from
/// `platforms.discord.token` via `client_id_from_token`.
pub fn invite_url(client_id: &str) -> anyhow::Result<String> {
    let id = client_id.trim();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) || id.len() < 15 {
        anyhow::bail!(
            "discord client_id must be the numeric Application ID (portal → General Information)"
        );
    }
    Ok(format!(
        "https://discord.com/oauth2/authorize?client_id={id}&permissions={}&scope=bot+applications.commands",
        INVITE_PERMISSIONS
    ))
}

/// Slash commands we register and handle (/ask /reset /status /stop).
#[cfg(feature = "discord")]
const SLASH_COMMANDS: [(&str, &str); 4] = [
    ("ask", "Send a prompt to gray"),
    ("reset", "Reset your gray session"),
    ("status", "Show gray session status"),
    ("stop", "Stop the running gray agent"),
];

#[cfg(feature = "discord")]
type HttpClient = std::sync::Arc<twilight_http::Client>;
#[cfg(not(feature = "discord"))]
type HttpClient = ();

pub struct DiscordAdapter {
    token: String,
    client: Mutex<Option<HttpClient>>,
    event_tx: Mutex<Option<UnboundedSender<MessageEvent>>>,
    /// Last inbound message id per channel — reply target for the first chunk.
    #[cfg_attr(not(feature = "discord"), allow(dead_code))]
    last_inbound: Mutex<HashMap<String, u64>>,
    /// Bot name from `current_user` (set on connect, read by the boot card).
    identity: Mutex<Option<String>>,
    /// Live shard task. `connect()` stores it, `disconnect()` aborts it, and
    /// the supervisor re-enters the reconnect ladder when it dies.
    shard: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Status board for staged connect progress (wired by the daemon; None for send-only).
    board: Mutex<Option<GatewayStatusBoard>>,
}

/// How long `connect()` waits for the first Ready before failing.
#[cfg(any(feature = "discord", test))]
pub(crate) const READY_TIMEOUT_SECS: u64 = 30;

/// Wait for the first Ready event; timeout surfaces as a retryable error.
#[cfg(feature = "discord")]
pub(crate) async fn wait_for_ready(rx: tokio::sync::oneshot::Receiver<()>) -> anyhow::Result<()> {
    wait_for_ready_with(rx, std::time::Duration::from_secs(READY_TIMEOUT_SECS)).await
}

#[cfg(any(feature = "discord", test))]
pub(crate) async fn wait_for_ready_with(
    rx: tokio::sync::oneshot::Receiver<()>,
    timeout: std::time::Duration,
) -> anyhow::Result<()> {
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => anyhow::bail!("discord shard ended before ready"),
        Err(_) => anyhow::bail!(
            "timed out waiting for discord ready (timeout {}s)",
            timeout.as_secs()
        ),
    }
}

impl DiscordAdapter {
    pub fn new(cfg: PlatformConfig) -> anyhow::Result<Self> {
        let token = cfg.token.ok_or_else(|| {
            anyhow::anyhow!("discord token not set (set platforms.discord.token in gateway.yaml)")
        })?;
        validate_discord_token(&token)?;
        Ok(Self {
            token: token.trim().trim_start_matches("Bot ").to_string(),
            client: Mutex::new(None),
            event_tx: Mutex::new(None),
            last_inbound: Mutex::new(HashMap::new()),
            identity: Mutex::new(None),
            shard: Mutex::new(None),
            board: Mutex::new(None),
        })
    }

    fn stage(&self, stage: &'static str) {
        if let Some(b) = self.board.lock().unwrap().clone() {
            b.mark_stage(Platform::Discord, stage);
        }
    }

    pub fn is_authenticated(&self) -> bool {
        validate_discord_token(&self.token).is_ok()
    }

    /// Whether a shard task is currently stored (for tests/supervision).
    pub fn has_shard(&self) -> bool {
        self.shard.lock().unwrap().is_some()
    }
}

/// Retry a fallible future with a per-attempt timeout. First Ok wins;
/// otherwise the last error, annotated with the attempt count. Keeps slow
/// steps (token validation) fail-fast with log breadcrumbs instead of one
/// long silent hang against the daemon's outer timeout.
#[cfg(any(feature = "discord", test))]
async fn retry_with_timeout<F, Fut, T>(
    attempts: u32,
    per_attempt: std::time::Duration,
    label: &str,
    mut f: F,
) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    let mut last = anyhow::anyhow!("{label}: no attempts ran");
    for n in 1..=attempts.max(1) {
        match tokio::time::timeout(per_attempt, f()).await {
            Ok(Ok(v)) => return Ok(v),
            Ok(Err(e)) => {
                log::warn!("[discord] {label} failed (attempt {n}): {e:#}");
                last = e;
            }
            Err(_) => {
                log::warn!(
                    "[discord] {label} timed out after {}s (attempt {n})",
                    per_attempt.as_secs()
                );
                last = anyhow::anyhow!("{label} timed out");
            }
        }
    }
    Err(anyhow::anyhow!("{label} failed: {last:#}"))
}

pub fn validate_discord_token(token: &str) -> anyhow::Result<()> {
    let raw = check_token_shape(token.strip_prefix("Bot ").unwrap_or(token), "discord token")?;
    if raw.len() < 20 {
        anyhow::bail!("discord token too short (expected >=20 chars)");
    }
    Ok(())
}

/// Guild trigger + cleanup (legacy parity): `Some(clean text)` only on
/// @mention or reply-to-bot (resolved inline by the gateway), else `None`
/// meaning stay silent. DMs bypass this entirely.
#[cfg_attr(not(feature = "discord"), allow(dead_code))]
fn guild_answer(
    mentioned: bool,
    reply_to_bot: bool,
    content: &str,
    bot_id: &str,
) -> Option<String> {
    if !(mentioned || reply_to_bot) {
        return None;
    }
    let text = content
        .replace(&format!("<@{bot_id}>"), "")
        .replace(&format!("<@!{bot_id}>"), "")
        .trim()
        .to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg_attr(not(feature = "discord"), allow(dead_code))]
fn source_for(
    msg_channel: u64,
    guild: Option<u64>,
    user_id: u64,
    message_id: u64,
) -> SessionSource {
    SessionSource {
        platform: Platform::Discord,
        chat_id: msg_channel.to_string(),
        chat_type: if guild.is_some() { "group" } else { "dm" }.to_string(),
        user_id: Some(user_id.to_string()),
        // Threads key by their own channel id, so no explicit thread tracking needed.
        thread_id: None,
        scope_id: guild.map(|g| g.to_string()),
        message_id: Some(message_id.to_string()),
    }
}

#[cfg(feature = "discord")]
fn spawn_shard(
    token: String,
    http: std::sync::Arc<twilight_http::Client>,
    tx: UnboundedSender<MessageEvent>,
    last_inbound: std::sync::Arc<Mutex<HashMap<String, u64>>>,
    ready_tx: Option<tokio::sync::oneshot::Sender<()>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ready_tx = ready_tx;
        use twilight_gateway::{Event, EventTypeFlags, Intents, Shard, ShardId, StreamExt};
        use twilight_model::application::interaction::InteractionData;

        let intents = Intents::GUILD_MESSAGES | Intents::DIRECT_MESSAGES | Intents::MESSAGE_CONTENT;
        let mut shard = Shard::new(ShardId::ONE, token, intents);
        let mut app_id = None;
        let mut bot_id: Option<twilight_model::id::Id<twilight_model::id::marker::UserMarker>> =
            None;
        while let Some(item) = shard.next_event(EventTypeFlags::all()).await {
            let event = match item {
                Ok(e) => e,
                Err(e) => {
                    log::warn!("[discord] shard event error: {e}");
                    continue;
                }
            };
            match event {
                Event::Ready(r) => {
                    app_id = Some(r.application.id);
                    bot_id = Some(r.user.id);
                    // First Ready unblocks `connect()` (which bounds it with a timeout).
                    if let Some(tx) = ready_tx.take() {
                        let _ = tx.send(());
                    }
                    // Register global slash commands.
                    let commands: Vec<twilight_model::application::command::Command> =
                        SLASH_COMMANDS
                            .iter()
                            .map(|(name, desc)| {
                                let b = twilight_util::builder::command::CommandBuilder::new(
                                    *name,
                                    *desc,
                                    twilight_model::application::command::CommandType::ChatInput,
                                );
                                if *name == "ask" {
                                    b.option(
                                        twilight_util::builder::command::StringBuilder::new(
                                            "prompt",
                                            "What to ask gray",
                                        )
                                        .required(true),
                                    )
                                    .build()
                                } else {
                                    b.build()
                                }
                            })
                            .collect();
                    match http
                        .interaction(r.application.id)
                        .set_global_commands(&commands)
                        .await
                    {
                        Ok(_) => {
                            log::info!("[discord] registered {} slash commands", commands.len())
                        }
                        Err(e) => log::warn!("[discord] slash command registration failed: {e}"),
                    }
                }
                Event::MessageCreate(msg) => {
                    let m = msg.0;
                    if m.author.bot {
                        continue;
                    }
                    let mut content = m.content.clone();
                    if content.is_empty() {
                        continue;
                    }
                    if m.guild_id.is_some() {
                        let Some(bot) = bot_id else { continue };
                        let mentioned = m.mentions.iter().any(|u| u.id == bot);
                        let reply_to_bot = m
                            .referenced_message
                            .as_deref()
                            .is_some_and(|r| r.author.id == bot);
                        let Some(text) =
                            guild_answer(mentioned, reply_to_bot, &content, &bot.to_string())
                        else {
                            continue;
                        };
                        content = text;
                    }
                    let cid = m.channel_id.get();
                    last_inbound
                        .lock()
                        .unwrap()
                        .insert(cid.to_string(), m.id.get());
                    let ev = MessageEvent {
                        text: content,
                        message_id: Some(m.id.get().to_string()),
                        source: source_for(
                            cid,
                            m.guild_id.map(|g| g.get()),
                            m.author.id.get(),
                            m.id.get(),
                        ),
                        media_urls: vec![],
                        user_name: Some(m.author.name.clone()),
                    };
                    let _ = tx.send(ev);
                }
                Event::InteractionCreate(interaction) => {
                    let Some(app) = app_id else { continue };
                    let Some(ref data) = interaction.0.data else {
                        continue;
                    };
                    let InteractionData::ApplicationCommand(cmd) = data else {
                        continue;
                    };
                    let Some(channel_id) = interaction.0.channel.as_ref().map(|c| c.id) else {
                        continue;
                    };
                    let user_id = interaction.0.author_id().map(|u| u.get()).unwrap_or(0);
                    let name = cmd.name.as_str();
                    let cid = channel_id.get();
                    let text = match name {
                        "ask" => cmd.options.iter()
                            .find(|o| o.name == "prompt")
                            .and_then(|o| match &o.value {
                                twilight_model::application::interaction::application_command::CommandOptionValue::String(s) => Some(s.clone()),
                                _ => None,
                            })
                            .unwrap_or_default(),
                        other => format!("/{other}"),
                    };
                    if text.is_empty() {
                        log::warn!("[discord] /ask without prompt from {user_id}");
                        continue;
                    }
                    // Ack the interaction immediately so Discord doesn't show "failure".
                    use twilight_model::http::interaction::{
                        InteractionResponse, InteractionResponseData, InteractionResponseType,
                    };
                    let resp = InteractionResponse {
                        kind: InteractionResponseType::ChannelMessageWithSource,
                        data: Some(InteractionResponseData {
                            content: if name == "ask" {
                                Some(format!("🤖 {text}"))
                            } else {
                                Some("…".into())
                            },
                            ..Default::default()
                        }),
                    };
                    if let Err(e) = http
                        .interaction(app)
                        .create_response(interaction.0.id, &interaction.0.token, &resp)
                        .await
                    {
                        log::warn!("[discord] interaction ack failed: {e}");
                    }
                    let ev = MessageEvent {
                        text,
                        message_id: Some(interaction.0.id.get().to_string()),
                        source: source_for(
                            cid,
                            interaction.0.guild_id.map(|g| g.get()),
                            user_id,
                            interaction.0.id.get(),
                        ),
                        media_urls: vec![],
                        user_name: interaction.0.author().map(|u| u.name.clone()),
                    };
                    let _ = tx.send(ev);
                }
                Event::GatewayClose(frame) => log::warn!("[discord] gateway closed: {frame:?}"),
                _ => {}
            }
        }
        // A dropped shard is never auth: the supervisor re-enters the
        // daemon's `connect_adapter_with_retry` ladder via this classification.
        let _ = crate::daemon::classify_shard_end();
        log::warn!("[discord] shard ended");
    })
}

#[async_trait::async_trait]
impl BasePlatformAdapter for DiscordAdapter {
    fn platform(&self) -> Platform {
        Platform::Discord
    }

    fn is_authenticated(&self) -> bool {
        self.is_authenticated()
    }

    fn bot_identity(&self) -> Option<String> {
        self.identity.lock().ok().and_then(|g| g.clone())
    }

    async fn connect(&self) -> anyhow::Result<()> {
        self.stage("validating token");
        validate_discord_token(&self.token)?;
        #[cfg(feature = "discord")]
        {
            let http = std::sync::Arc::new(twilight_http::Client::new(self.token.clone()));
            // Validate the token early so misconfigurations fail fast.
            // Bounded attempts: a hung API call must surface in seconds, not sit
            // on "validating token" until the daemon's 45s outer timeout.
            // The display name is user-chosen at bot creation — the boot card shows it verbatim.
            let http2 = http.clone();
            let me = retry_with_timeout(
                3,
                std::time::Duration::from_secs(15),
                "discord token validation",
                move || {
                    let http2 = http2.clone();
                    async move {
                        http2
                            .current_user()
                            .await
                            .map_err(|e| anyhow::anyhow!("discord token rejected: {e}"))?
                            .model()
                            .await
                            .map_err(|e| anyhow::anyhow!("discord token rejected: {e}"))
                    }
                },
            )
            .await?;
            *self.identity.lock().unwrap() = Some(
                me.global_name
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| me.name.clone()),
            );
            *self.client.lock().unwrap() = Some(http.clone());
            // Bind first: the match scrutinee temporary would hold the
            // non-Send guard across the ready await below.
            let tx = self.event_tx.lock().unwrap().clone();
            match tx {
                Some(tx) => {
                    self.stage("connecting gateway");
                    if let Some(old) = self.shard.lock().unwrap().take() {
                        old.abort();
                    }
                    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
                    let h = spawn_shard(
                        self.token.clone(),
                        http,
                        tx,
                        std::sync::Arc::new(Mutex::new(self.last_inbound.lock().unwrap().clone())),
                        Some(ready_tx),
                    );
                    *self.shard.lock().unwrap() = Some(h);
                    self.stage("waiting for ready");
                    if let Err(e) = wait_for_ready(ready_rx).await {
                        if let Some(h) = self.shard.lock().unwrap().take() {
                            h.abort();
                        }
                        return Err(e);
                    }
                    log::info!("[discord] gateway connected, slash commands registered on Ready");
                }
                None => log::info!("[discord] send-only mode (no event channel wired)"),
            }
        }
        #[cfg(not(feature = "discord"))]
        {
            self.stage("connecting gateway");
            if let Some(old) = self.shard.lock().unwrap().take() {
                old.abort();
            }
            // Stub shard: pends forever so disconnect-abort is testable without network.
            // On real shards death the task exits and the next supervised
            // `connect_adapter_with_retry` restarts it (see daemon ladder).
            *self.shard.lock().unwrap() =
                Some(tokio::spawn(async { std::future::pending::<()>().await }));
            self.stage("waiting for ready");
            log::info!(
                "[discord] stub connect (token {}…)",
                &self.token[..self.token.len().min(6)]
            );
        }
        Ok(())
    }

    async fn disconnect(&self) -> anyhow::Result<()> {
        if let Some(h) = self.shard.lock().unwrap().take() {
            h.abort();
        }
        *self.client.lock().unwrap() = None;
        log::info!("[discord] disconnected");
        Ok(())
    }

    fn set_event_tx(&mut self, tx: UnboundedSender<MessageEvent>) {
        *self.event_tx.lock().unwrap() = Some(tx);
    }

    fn set_status_board(&self, board: GatewayStatusBoard) {
        *self.board.lock().unwrap() = Some(board);
    }

    async fn send_typing(&self, chat: &str) {
        #[cfg(feature = "discord")]
        {
            let client = self.client.lock().unwrap().clone();
            if let (Some(client), Ok(cid)) = (client, chat.parse::<u64>()) {
                let _ = client
                    .create_typing_trigger(twilight_model::id::Id::new(cid))
                    .await;
            }
        }
        #[cfg(not(feature = "discord"))]
        let _ = chat;
    }

    fn supports_edit(&self) -> bool {
        cfg!(feature = "discord")
    }

    async fn edit_message(&self, chat: &str, message_id: &str, text: &str) -> SendResult {
        #[cfg(feature = "discord")]
        {
            let Some(client) = self.client.lock().unwrap().clone() else {
                return SendResult::fail("discord not connected", false);
            };
            let (Ok(cid), Ok(mid)) = (chat.parse::<u64>(), message_id.parse::<u64>()) else {
                return SendResult::fail(
                    format!("invalid discord ids {chat:?}/{message_id:?}"),
                    false,
                );
            };
            if utf16_len(text) > MAX_LENGTH {
                return SendResult::fail("edit text exceeds 2000 utf16 units", false);
            }
            return match client
                .update_message(
                    twilight_model::id::Id::new(cid),
                    twilight_model::id::Id::new(mid),
                )
                .content(Some(text))
                .await
            {
                Ok(_) => SendResult::ok(Some(message_id.to_string())),
                Err(e) => SendResult::fail(format!("discord edit: {e}"), true),
            };
        }
        #[cfg(not(feature = "discord"))]
        {
            log::info!(
                "[discord] edit {chat}/{message_id} ({} utf16)",
                utf16_len(text)
            );
            SendResult::ok(Some(message_id.to_string()))
        }
    }

    async fn delete_message(&self, chat: &str, message_id: &str) -> SendResult {
        #[cfg(feature = "discord")]
        {
            let Some(client) = self.client.lock().unwrap().clone() else {
                return SendResult::fail("discord not connected", false);
            };
            let (Ok(cid), Ok(mid)) = (chat.parse::<u64>(), message_id.parse::<u64>()) else {
                return SendResult::fail(
                    format!("invalid discord ids {chat:?}/{message_id:?}"),
                    false,
                );
            };
            return match client
                .delete_message(
                    twilight_model::id::Id::new(cid),
                    twilight_model::id::Id::new(mid),
                )
                .await
            {
                Ok(_) => SendResult::ok(Some(message_id.to_string())),
                Err(e) => SendResult::fail(format!("discord delete: {e}"), true),
            };
        }
        #[cfg(not(feature = "discord"))]
        {
            log::info!("[discord] delete {chat}/{message_id}");
            SendResult::ok(Some(message_id.to_string()))
        }
    }

    async fn send(&self, chat: &str, text: &str) -> SendResult {
        // Default reply target: last inbound message in this channel.
        let reply_to = self
            .last_inbound
            .lock()
            .unwrap()
            .get(chat)
            .map(|m| m.to_string());
        self.send_ext(
            chat,
            text,
            &SendOptions {
                reply_to,
                thread_id: None,
            },
        )
        .await
    }

    async fn send_ext(&self, chat: &str, text: &str, opts: &SendOptions) -> SendResult {
        if !self.is_authenticated() {
            return SendResult::fail("discord not authenticated: invalid token", false);
        }
        if text.is_empty() {
            return SendResult::ok(None);
        }

        let chunks = crate::platform::split_message_smart(text, MAX_LENGTH);

        #[cfg(feature = "discord")]
        {
            let Some(client) = self.client.lock().unwrap().clone() else {
                return SendResult::fail("discord not connected", false);
            };
            // Threads are channels on Discord: `thread_id` overrides the target channel.
            let target = opts.thread_id.as_deref().unwrap_or(chat);
            let Ok(cid) = target.parse::<u64>() else {
                return SendResult::fail(format!("invalid discord channel id {target:?}"), false);
            };
            let channel = twilight_model::id::Id::new(cid);
            let reply_to = opts.reply_to.as_deref().and_then(|r| r.parse::<u64>().ok());
            let mut last_id = None;
            for (i, chunk) in chunks.iter().enumerate() {
                debug_assert!(utf16_len(chunk) <= MAX_LENGTH);
                let create = client.create_message(channel).content(chunk.as_str());
                let create = match (i, reply_to) {
                    (0, Some(mid)) => create.reply(twilight_model::id::Id::new(mid)),
                    _ => create,
                };
                match create.await {
                    Ok(resp) => {
                        if let Ok(m) = resp.model().await {
                            last_id = Some(m.id.get().to_string());
                        }
                    }
                    Err(e) => {
                        log::warn!(
                            "[discord] send chunk {}/{} failed: {e}",
                            i + 1,
                            chunks.len()
                        );
                        return SendResult::fail(format!("discord send: {e}"), true);
                    }
                }
            }
            return SendResult::ok(last_id);
        }
        #[cfg(not(feature = "discord"))]
        {
            for (i, chunk) in chunks.iter().enumerate() {
                debug_assert!(utf16_len(chunk) <= MAX_LENGTH);
                log::info!(
                    "[discord] send to {} chunk {}/{} ({} utf16, reply_to={:?}): {:?}",
                    chat,
                    i + 1,
                    chunks.len(),
                    utf16_len(chunk),
                    opts.reply_to,
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
    use crate::platform::utf16_len;

    fn cfg(token: &str) -> PlatformConfig {
        PlatformConfig::with_token(token)
    }

    #[test]
    fn validate_good() {
        assert!(validate_discord_token(&"a".repeat(50)).is_ok());
        assert!(validate_discord_token(&format!("Bot {}", "b".repeat(50))).is_ok());
    }

    #[test]
    fn validate_bad() {
        assert!(validate_discord_token("").is_err());
        assert!(validate_discord_token("short").is_err());
        assert!(validate_discord_token("has space in token abcdefghijklmnopqrst").is_err());
    }

    #[tokio::test]
    async fn send_splits() {
        let a = DiscordAdapter::new(cfg(&"x".repeat(50))).unwrap();
        let long = "a".repeat(5000);
        let res = a.send("chan1", &long).await;
        // Stub logs and succeeds; the real client fails cleanly when not connected.
        #[cfg(not(feature = "discord"))]
        assert!(res.success);
        #[cfg(feature = "discord")]
        assert!(!res.success && res.error.as_deref() == Some("discord not connected"));
        let chunks = crate::platform::split_message(&long, MAX_LENGTH);
        assert_eq!(chunks.len(), 3); // 2000*2 +1000
        for c in &chunks {
            assert!(utf16_len(c) <= MAX_LENGTH);
        }
    }

    #[test]
    fn is_auth() {
        let a = DiscordAdapter::new(cfg(&"y".repeat(30))).unwrap();
        assert!(a.is_authenticated());
    }

    #[test]
    fn invite_url_good_bad() {
        let url = invite_url("123456789012345678").unwrap();
        assert!(
            url.starts_with("https://discord.com/oauth2/authorize?client_id=123456789012345678")
        );
        assert!(url.contains("scope=bot+applications.commands"));
        assert!(invite_url("").is_err());
        assert!(invite_url("notanid").is_err());
        assert!(invite_url("123").is_err());
    }

    #[test]
    fn client_id_from_token_roundtrip() {
        use base64::Engine as _;
        let id = "123456789012345678";
        let tok = format!(
            "{}.fake.sig",
            base64::engine::general_purpose::URL_SAFE.encode(id)
        );
        assert_eq!(client_id_from_token(&tok), Some(id.to_string()));
        assert_eq!(
            client_id_from_token(&("Bot ".to_string() + &tok)),
            Some(id.to_string())
        );
        assert_eq!(client_id_from_token("short"), None);
        assert_eq!(client_id_from_token(""), None);
    }

    #[test]
    fn guild_answer_gate() {
        assert_eq!(
            guild_answer(true, false, "<@123> hello", "123").as_deref(),
            Some("hello")
        );
        assert_eq!(
            guild_answer(false, true, "reply hi", "123").as_deref(),
            Some("reply hi")
        );
        assert_eq!(guild_answer(false, false, "noise", "123"), None);
        assert_eq!(guild_answer(true, false, "<@!123>", "123"), None); // mention-only
        assert_eq!(
            guild_answer(true, false, "hey <@123> yo", "123").as_deref(),
            Some("hey  yo")
        );
    }

    #[test]
    fn session_key_routing() {
        use crate::session::build_session_key;
        let dm = source_for(100, None, 7, 1);
        assert_eq!(
            build_session_key(&dm, true, false),
            "gray:main:discord:dm:100"
        );
        let g = source_for(200, Some(999), 7, 2);
        assert_eq!(
            build_session_key(&g, true, false),
            "gray:main:discord:group:999:200:7"
        );
        assert_eq!(
            build_session_key(&g, false, false),
            "gray:main:discord:group:999:200"
        );
    }

    #[test]
    fn shard_end_is_retryable_via_ladder() {
        // A dropped shard is never auth: it must re-enter the supervised ladder.
        assert!(matches!(
            crate::daemon::classify_shard_end(),
            crate::daemon::Fatal::Retryable(_)
        ));
    }

    #[tokio::test]
    async fn disconnect_aborts_stored_shard() {
        let a = DiscordAdapter::new(cfg(&"x".repeat(50))).unwrap();
        // Inject directly so the test needs no network (feature builds try real connect).
        *a.shard.lock().unwrap() = Some(tokio::spawn(async { std::future::pending::<()>().await }));
        assert!(a.has_shard(), "shard must be stored");
        a.disconnect().await.unwrap();
        assert!(
            !a.has_shard(),
            "disconnect must abort/take the stored handle"
        );
    }

    #[test]
    fn ready_timeout_is_30s() {
        assert_eq!(READY_TIMEOUT_SECS, 30);
    }

    #[tokio::test]
    async fn ready_wait_resolves_on_first_ready() {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        tx.send(()).unwrap();
        assert!(
            wait_for_ready_with(rx, std::time::Duration::from_secs(5))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn ready_wait_timeout_is_retryable_not_terminal() {
        // Sender kept alive so the wait actually times out (no instant cancel).
        let (_keep, rx) = tokio::sync::oneshot::channel::<()>();
        let err = wait_for_ready_with(rx, std::time::Duration::from_millis(10))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("ready"), "error surfaces: {err}");
        assert!(
            matches!(
                crate::daemon::classify_connect_error(&err.to_string()),
                crate::daemon::Fatal::Retryable(_)
            ),
            "ready timeout must retry, never terminal: {err}"
        );
    }

    #[tokio::test]
    async fn stub_connect_walks_stages_for_supervision() {
        // Stub-only: no network, connect still walks the staged path.
        #[cfg(not(feature = "discord"))]
        {
            use crate::status::{GatewayStatusBoard, PlatformConnState};
            let a = DiscordAdapter::new(cfg(&"x".repeat(50))).unwrap();
            let board = GatewayStatusBoard::new(&[Platform::Discord]);
            a.set_status_board(board.clone());
            a.connect().await.unwrap();
            assert!(a.has_shard(), "connect must store the shard task");
            assert_eq!(
                board.snapshot()[0].1,
                PlatformConnState::Connecting {
                    stage: "waiting for ready"
                },
                "stub ends on the last pre-connected stage; the daemon marks connected"
            );
            a.disconnect().await.unwrap();
            assert!(!a.has_shard());
        }
    }

    #[tokio::test]
    async fn stub_connect_stores_shard_for_supervision() {
        // Stub-only: no network, connect must store the shard task.
        #[cfg(not(feature = "discord"))]
        {
            let a = DiscordAdapter::new(cfg(&"x".repeat(50))).unwrap();
            a.connect().await.unwrap();
            assert!(a.has_shard(), "connect must store the shard task");
            a.disconnect().await.unwrap();
            assert!(!a.has_shard());
        }
    }

    #[tokio::test]
    async fn retry_with_timeout_succeeds_after_flakes() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let calls = std::sync::Arc::new(AtomicU32::new(0));
        let r = retry_with_timeout(3, std::time::Duration::from_secs(5), "t", {
            let calls = calls.clone();
            move || {
                let calls = calls.clone();
                async move {
                    let n = calls.fetch_add(1, Ordering::SeqCst);
                    if n < 2 {
                        anyhow::bail!("flake {n}")
                    } else {
                        Ok(42)
                    }
                }
            }
        })
        .await;
        assert_eq!(r.unwrap(), 42);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_with_timeout_bounds_a_hang() {
        let before = std::time::Instant::now();
        let r = retry_with_timeout(2, std::time::Duration::from_millis(50), "t", || async {
            std::future::pending::<anyhow::Result<()>>().await
        })
        .await;
        assert!(r.is_err(), "perpetual hang must surface as Err");
        assert!(
            before.elapsed() < std::time::Duration::from_secs(5),
            "two 50ms attempts must not hang: {:?}",
            before.elapsed()
        );
    }
}
