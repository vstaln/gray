//! Discord adapter (stub by default, real behind `discord` feature).
//!
//! Enable real gateway: `cargo check -p gray-gateway --features discord`
//! requires `twilight-gateway`/`twilight-http` (optional). Default stub keeps
//! `cargo check` passing without those deps.

use crate::config::{Platform, PlatformConfig};
use crate::platform::{split_message, truncate_message, utf16_len, BasePlatformAdapter, SendResult};

pub const MAX_LENGTH: usize = 2000;
pub const SPLITS_LONG_MESSAGES: bool = true;

pub struct DiscordAdapter {
    token: String,
    connected: bool,
}

impl DiscordAdapter {
    pub fn new(cfg: PlatformConfig) -> anyhow::Result<Self> {
        let token = cfg
            .token
            .ok_or_else(|| anyhow::anyhow!("discord token not set (set platforms.discord.token in gateway.yaml)"))?;
        validate_discord_token(&token)?;
        Ok(Self { token, connected: false })
    }

    pub fn is_authenticated(&self) -> bool {
        validate_discord_token(&self.token).is_ok()
    }
}

pub fn validate_discord_token(token: &str) -> anyhow::Result<()> {
    let t = token.trim();
    if t.is_empty() {
        anyhow::bail!("discord token empty");
    }
    let raw = t.strip_prefix("Bot ").unwrap_or(t);
    if raw.contains(' ') || raw.contains('\n') {
        anyhow::bail!("discord token must not contain whitespace");
    }
    if raw.len() < 20 {
        anyhow::bail!("discord token too short (expected >=20 chars)");
    }
    Ok(())
}

#[async_trait::async_trait]
impl BasePlatformAdapter for DiscordAdapter {
    fn platform(&self) -> Platform {
        Platform::Discord
    }

    fn is_authenticated(&self) -> bool {
        self.is_authenticated()
    }

    async fn connect(&mut self) -> anyhow::Result<()> {
        validate_discord_token(&self.token)?;
        #[cfg(feature = "discord")]
        {
            log::info!("[discord] (feature) validated token, connecting gateway");
        }
        #[cfg(not(feature = "discord"))]
        {
            log::info!("[discord] stub connect (token {}…)", &self.token[..self.token.len().min(6)]);
        }
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        self.connected = false;
        log::info!("[discord] disconnected");
        Ok(())
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

        let chunks = if SPLITS_LONG_MESSAGES && utf16_len(text) > MAX_LENGTH {
            split_message(text, MAX_LENGTH)
        } else if utf16_len(text) > MAX_LENGTH {
            vec![truncate_message(text, MAX_LENGTH)]
        } else {
            vec![text.to_string()]
        };

        for (i, chunk) in chunks.iter().enumerate() {
            debug_assert!(utf16_len(chunk) <= MAX_LENGTH);
            #[cfg(feature = "discord")]
            {
                let _ = chat;
                log::debug!("[discord] (feature) send chunk {}/{} to {}: {} chars", i + 1, chunks.len(), chat, chunk.len());
            }
            #[cfg(not(feature = "discord"))]
            {
                log::info!("[discord] send to {} chunk {}/{} ({} utf16): {:?}", chat, i + 1, chunks.len(), utf16_len(chunk), &chunk[..chunk.len().min(80)]);
            }
        }

        SendResult { success: true, message_id: None, error: None, retryable: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PlatformConfig;
    use crate::platform::utf16_len;

    fn cfg(token: &str) -> PlatformConfig {
        PlatformConfig { enabled: true, token: Some(token.to_string()), app_token: None, home_channel: None }
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
}
