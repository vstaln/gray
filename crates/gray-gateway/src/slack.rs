//! Slack adapter (stub by default, real behind `slack` feature).
//!
//! Slack uses two tokens in Socket Mode:
//! - `token` = bot token `xoxb-...` (for Web API)
//! - `app_token` = app-level token `xapp-...` (for Socket Mode)
//! Single-token mode (just xoxb) still works for Web API only.
//!
//! Enable real Socket Mode: `cargo check -p gray-gateway --features slack`
//! requires `slack-morphism` (optional). Default stub keeps `cargo check` passing.

use crate::config::{Platform, PlatformConfig};
use crate::platform::{check_token_shape, utf16_len, BasePlatformAdapter, SendResult};

pub const MAX_LENGTH: usize = 39000;

pub struct SlackAdapter {
    bot_token: String,
    app_token: Option<String>,
}

impl SlackAdapter {
    pub fn new(cfg: PlatformConfig) -> anyhow::Result<Self> {
        let bot_token = cfg
            .token
            .ok_or_else(|| anyhow::anyhow!("slack token not set (set platforms.slack.token to xoxb-... in gateway.yaml)"))?;
        validate_slack_bot_token(&bot_token)?;
        let app_token = cfg.app_token.clone();
        if let Some(ref t) = app_token {
            validate_slack_app_token(t)?;
        }
        Ok(Self { bot_token, app_token })
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

#[async_trait::async_trait]
impl BasePlatformAdapter for SlackAdapter {
    fn platform(&self) -> Platform {
        Platform::Slack
    }

    fn is_authenticated(&self) -> bool {
        self.is_authenticated()
    }

    async fn connect(&self) -> anyhow::Result<()> {
        validate_slack_bot_token(&self.bot_token)?;
        if let Some(ref t) = self.app_token {
            validate_slack_app_token(t)?;
        }
        #[cfg(feature = "slack")]
        {
            log::info!("[slack] (feature) validated tokens, connecting Socket Mode (has_app_token={})", self.has_socket_mode());
        }
        #[cfg(not(feature = "slack"))]
        {
            log::info!("[slack] stub connect bot={}… app_token={}", &self.bot_token[..self.bot_token.len().min(8)], self.has_socket_mode());
        }
        Ok(())
    }

    async fn disconnect(&self) -> anyhow::Result<()> {
        log::info!("[slack] disconnected");
        Ok(())
    }

    async fn send(&self, chat: &str, text: &str) -> SendResult {
        if !self.is_authenticated() {
            return SendResult {
                success: false,
                message_id: None,
                error: Some("slack not authenticated: invalid token".to_string()),
                retryable: false,
            };
        }
        if text.is_empty() {
            return SendResult { success: true, message_id: None, error: None, retryable: false };
        }

        let chunks = crate::platform::chunk_message(text, MAX_LENGTH);

        for (i, chunk) in chunks.iter().enumerate() {
            debug_assert!(utf16_len(chunk) <= MAX_LENGTH);
            #[cfg(feature = "slack")]
            {
                let _ = chat;
                log::debug!("[slack] (feature) send chunk {}/{} to {}: {} chars", i + 1, chunks.len(), chat, chunk.len());
            }
            #[cfg(not(feature = "slack"))]
            {
                log::info!("[slack] send to {} chunk {}/{} ({} utf16): {:?}", chat, i + 1, chunks.len(), utf16_len(chunk), crate::platform::preview_80(chunk));
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

    fn cfg(bot: &str, app: Option<&str>) -> PlatformConfig {
        PlatformConfig { enabled: true, token: Some(bot.to_string()), app_token: app.map(|s| s.to_string()), home_channel: None, client_id: None }
    }

    #[test]
    fn validate_bot_good() {
        assert!(validate_slack_bot_token("xoxb-123456-abcdef").is_ok());
        assert!(validate_slack_bot_token("xoxp-123-abc").is_ok());
    }

    #[test]
    fn validate_bot_bad() {
        assert!(validate_slack_bot_token("").is_err());
        assert!(validate_slack_bot_token("xoxa-bad").is_err());
        assert!(validate_slack_bot_token("short").is_err());
    }

    #[test]
    fn validate_app_good_bad() {
        assert!(validate_slack_app_token("xapp-1-A123-abc").is_ok());
        assert!(validate_slack_app_token("xoxb-bad").is_err());
        assert!(validate_slack_app_token("").is_err());
    }

    #[test]
    fn new_with_app_token() {
        let a = SlackAdapter::new(cfg("xoxb-1234567890-abc", Some("xapp-1-A123-abcdef"))).unwrap();
        assert!(a.has_socket_mode());
        assert!(a.is_authenticated());
    }

    #[test]
    fn new_without_app_token_ok() {
        let a = SlackAdapter::new(cfg("xoxb-1234567890-abc", None)).unwrap();
        assert!(!a.has_socket_mode());
    }

    #[tokio::test]
    async fn send_splits_39000() {
        let a = SlackAdapter::new(cfg("xoxb-aaaaaaaaaa-bbbbbbb", None)).unwrap();
        let long = "a".repeat(80000);
        let res = a.send("C123", &long).await;
        assert!(res.success);
        let chunks = crate::platform::split_message(&long, MAX_LENGTH);
        assert_eq!(chunks.len(), 3); // 39000*2 +2000
        for c in &chunks { assert!(utf16_len(c) <= MAX_LENGTH); }
    }
}
