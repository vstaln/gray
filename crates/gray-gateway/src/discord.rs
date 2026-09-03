//! Discord adapter — stub by default, real gateway+REST behind `discord` feature.
//!
//! Enable: `cargo check -p gray-gateway --features discord` (twilight 0.16).
//! Features (hermes parity): inbound messages via twilight-gateway, replies via
//! twilight-http with reply-on-first-chunk, persistent typing loop, slash
//! commands /ask /reset /status /stop.

use crate::config::{Platform, PlatformConfig};
use crate::platform::{check_token_shape, utf16_len, BasePlatformAdapter, MessageEvent, SendResult};
use crate::session::SessionSource;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;

pub const MAX_LENGTH: usize = 2000;

/// OAuth2 invite permissions: View Channels + Send Messages + Read Message History.
pub const INVITE_PERMISSIONS: u64 = 1024 + 2048 + 65536;

/// Build the OAuth2 invite URL for `client_id` (Application ID, numeric).
/// Copy from the Developer Portal (General Information) into
/// `~/.gray/gateway.yaml` as `platforms.discord.client_id`.
pub fn invite_url(client_id: &str) -> anyhow::Result<String> {
    let id = client_id.trim();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) || id.len() < 15 {
        anyhow::bail!("discord client_id must be the numeric Application ID (portal → General Information)");
    }
    Ok(format!(
        "https://discord.com/oauth2/authorize?client_id={id}&permissions={}&scope=bot+applications.commands",
        INVITE_PERMISSIONS
    ))
}

/// Slash commands we register and handle (hermes: /ask /reset /status /stop).
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
    /// Last inbound message id per channel — reply target for the first chunk (hermes reply_to_mode=first).
    #[cfg_attr(not(feature = "discord"), allow(dead_code))]
    last_inbound: Mutex<HashMap<String, u64>>,
}

impl DiscordAdapter {
    pub fn new(cfg: PlatformConfig) -> anyhow::Result<Self> {
        let token = cfg
            .token
            .ok_or_else(|| anyhow::anyhow!("discord token not set (set platforms.discord.token in gateway.yaml)"))?;
        validate_discord_token(&token)?;
        Ok(Self {
            token: token.trim().trim_start_matches("Bot ").to_string(),
            client: Mutex::new(None),
            event_tx: Mutex::new(None),
            last_inbound: Mutex::new(HashMap::new()),
        })
    }

    pub fn is_authenticated(&self) -> bool {
        validate_discord_token(&self.token).is_ok()
    }
}

pub fn validate_discord_token(token: &str) -> anyhow::Result<()> {
    let t = token.trim();
    let raw = t.strip_prefix("Bot ").unwrap_or(t);
    check_token_shape(raw, "discord token")?;
    if raw.len() < 20 {
        anyhow::bail!("discord token too short (expected >=20 chars)");
    }
    Ok(())
}

#[cfg_attr(not(feature = "discord"), allow(dead_code))]
fn source_for(msg_channel: u64, guild: Option<u64>, user_id: u64, message_id: u64) -> SessionSource {
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
fn spawn_shard(token: String, http: std::sync::Arc<twilight_http::Client>, tx: UnboundedSender<MessageEvent>, last_inbound: std::sync::Arc<Mutex<HashMap<String, u64>>>) {
    tokio::spawn(async move {
        use twilight_gateway::{Event, EventTypeFlags, Intents, Shard, ShardId, StreamExt};
        use twilight_model::application::interaction::InteractionData;

        let intents = Intents::GUILD_MESSAGES | Intents::DIRECT_MESSAGES | Intents::MESSAGE_CONTENT;
        let mut shard = Shard::new(ShardId::ONE, token, intents);
        let mut app_id = None;
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
                    // Register global slash commands (hermes _register_slash_commands).
                    let commands: Vec<twilight_model::application::command::Command> = SLASH_COMMANDS
                        .iter()
                        .map(|(name, desc)| {
                            let b = twilight_util::builder::command::CommandBuilder::new(
                                *name, *desc, twilight_model::application::command::CommandType::ChatInput);
                            if *name == "ask" {
                                b.option(twilight_util::builder::command::StringBuilder::new("prompt", "What to ask gray").required(true)).build()
                            } else {
                                b.build()
                            }
                        })
                        .collect();
                    match http.interaction(r.application.id).set_global_commands(&commands).await {
                        Ok(_) => log::info!("[discord] registered {} slash commands", commands.len()),
                        Err(e) => log::warn!("[discord] slash command registration failed: {e}"),
                    }
                }
                Event::MessageCreate(msg) => {
                    let m = msg.0;
                    if m.author.bot { continue; }
                    let content = m.content.clone();
                    if content.is_empty() { continue; }
                    let cid = m.channel_id.get();
                    last_inbound.lock().unwrap().insert(cid.to_string(), m.id.get());
                    let ev = MessageEvent {
                        text: content,
                        message_id: Some(m.id.get().to_string()),
                        source: source_for(cid, m.guild_id.map(|g| g.get()), m.author.id.get(), m.id.get()),
                        media_urls: vec![],
                    };
                    let _ = tx.send(ev);
                }
                Event::InteractionCreate(interaction) => {
                    let Some(app) = app_id else { continue };
                    let Some(ref data) = interaction.0.data else { continue };
                    let InteractionData::ApplicationCommand(cmd) = data else { continue };
                    let Some(channel_id) = interaction.0.channel.as_ref().map(|c| c.id) else { continue };
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
                    // Ack the interaction so Discord doesn't show "failure" (hermes responds immediately).
                    use twilight_model::http::interaction::{InteractionResponse, InteractionResponseData, InteractionResponseType};
                    let resp = InteractionResponse {
                        kind: InteractionResponseType::ChannelMessageWithSource,
                        data: Some(InteractionResponseData {
                            content: if name == "ask" { Some(format!("🤖 {text}")) } else { Some("…".into()) },
                            ..Default::default()
                        }),
                    };
                    if let Err(e) = http.interaction(app).create_response(interaction.0.id, &interaction.0.token, &resp).await {
                        log::warn!("[discord] interaction ack failed: {e}");
                    }
                    let ev = MessageEvent {
                        text,
                        message_id: Some(interaction.0.id.get().to_string()),
                        source: source_for(cid, interaction.0.guild_id.map(|g| g.get()), user_id, interaction.0.id.get()),
                        media_urls: vec![],
                    };
                    let _ = tx.send(ev);
                }
                Event::GatewayClose(frame) => log::warn!("[discord] gateway closed: {frame:?}"),
                _ => {}
            }
        }
        log::warn!("[discord] shard ended");
    });
}

#[async_trait::async_trait]
impl BasePlatformAdapter for DiscordAdapter {
    fn platform(&self) -> Platform {
        Platform::Discord
    }

    fn is_authenticated(&self) -> bool {
        self.is_authenticated()
    }

    async fn connect(&self) -> anyhow::Result<()> {
        validate_discord_token(&self.token)?;
        #[cfg(feature = "discord")]
        {
            let http = std::sync::Arc::new(twilight_http::Client::new(self.token.clone()));
            // Validate the token early so misconfigurations fail fast (hermes get_me parity).
            if let Err(e) = http.current_user().await {
                anyhow::bail!("discord token rejected: {e}");
            }
            *self.client.lock().unwrap() = Some(http.clone());
            let tx = self.event_tx.lock().unwrap().clone()
                .ok_or_else(|| anyhow::anyhow!("discord adapter started before set_event_tx"))?;
            spawn_shard(self.token.clone(), http, tx, std::sync::Arc::new(Mutex::new(self.last_inbound.lock().unwrap().clone())));
            log::info!("[discord] gateway connected, slash commands registered on Ready");
        }
        #[cfg(not(feature = "discord"))]
        {
            log::info!("[discord] stub connect (token {}…)", &self.token[..self.token.len().min(6)]);
        }
        Ok(())
    }

    async fn disconnect(&self) -> anyhow::Result<()> {
        *self.client.lock().unwrap() = None;
        log::info!("[discord] disconnected");
        Ok(())
    }

    fn set_event_tx(&mut self, tx: UnboundedSender<MessageEvent>) {
        *self.event_tx.lock().unwrap() = Some(tx);
    }

    async fn send_typing(&self, chat: &str) {
        #[cfg(feature = "discord")]
        {
            let client = self.client.lock().unwrap().clone();
            if let (Some(client), Ok(cid)) = (client, chat.parse::<u64>()) {
                let _ = client.create_typing_trigger(twilight_model::id::Id::new(cid)).await;
            }
        }
        #[cfg(not(feature = "discord"))]
        let _ = chat;
    }

    async fn send(&self, chat: &str, text: &str) -> SendResult {
        if !self.is_authenticated() {
            return SendResult {
                success: false,
                message_id: None,
                error: Some("discord not authenticated: invalid token".to_string()),
                retryable: false,
            };
        }
        if text.is_empty() {
            return SendResult { success: true, message_id: None, error: None, retryable: false };
        }

        let chunks = crate::platform::chunk_message(text, MAX_LENGTH);

        #[cfg(feature = "discord")]
        {
            let Some(client) = self.client.lock().unwrap().clone() else {
                return SendResult { success: false, message_id: None, error: Some("discord not connected".into()), retryable: false };
            };
            let Ok(cid) = chat.parse::<u64>() else {
                return SendResult { success: false, message_id: None, error: Some(format!("invalid discord channel id {chat:?}")), retryable: false };
            };
            let channel = twilight_model::id::Id::new(cid);
            let reply_to = self.last_inbound.lock().unwrap().get(chat).copied(); // hermes reply_to_mode=first
            for (i, chunk) in chunks.iter().enumerate() {
                debug_assert!(utf16_len(chunk) <= MAX_LENGTH);
                let create = client.create_message(channel).content(chunk.as_str());
                let create = if i == 0 {
                    if let Some(mid) = reply_to { create.reply(twilight_model::id::Id::new(mid)) } else { create }
                } else { create };
                if let Err(e) = create.await {
                    log::warn!("[discord] send chunk {}/{} failed: {e}", i + 1, chunks.len());
                    return SendResult { success: false, message_id: None, error: Some(format!("discord send: {e}")), retryable: true };
                }
            }
            return SendResult { success: true, message_id: None, error: None, retryable: false };
        }
        #[cfg(not(feature = "discord"))]
        {
            for (i, chunk) in chunks.iter().enumerate() {
                debug_assert!(utf16_len(chunk) <= MAX_LENGTH);
                log::info!("[discord] send to {} chunk {}/{} ({} utf16): {:?}", chat, i + 1, chunks.len(), utf16_len(chunk), crate::platform::preview_80(chunk));
            }
            SendResult { success: true, message_id: None, error: None, retryable: false }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PlatformConfig;
    use crate::platform::utf16_len;

    fn cfg(token: &str) -> PlatformConfig {
        PlatformConfig { enabled: true, token: Some(token.to_string()), app_token: None, home_channel: None, client_id: None }
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
        assert!(res.success);
        let chunks = crate::platform::split_message(&long, MAX_LENGTH);
        assert_eq!(chunks.len(), 3); // 2000*2 +1000
        for c in &chunks { assert!(utf16_len(c) <= MAX_LENGTH); }
    }

    #[test]
    fn is_auth() {
        let a = DiscordAdapter::new(cfg(&"y".repeat(30))).unwrap();
        assert!(a.is_authenticated());
    }

    #[test]
    fn invite_url_good_bad() {
        let url = invite_url("123456789012345678").unwrap();
        assert!(url.starts_with("https://discord.com/oauth2/authorize?client_id=123456789012345678"));
        assert!(url.contains("scope=bot+applications.commands"));
        assert!(invite_url("").is_err());
        assert!(invite_url("notanid").is_err());
        assert!(invite_url("123").is_err());
    }
}
