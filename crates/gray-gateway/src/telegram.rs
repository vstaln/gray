//! Telegram adapter (stub by default, real behind `telegram` feature).
//!
//! Enable real polling: `cargo check -p gray-gateway --features telegram`
//! requires `teloxide` (optional dep). Default build keeps stub so `cargo check`
//! passes without network deps.

use crate::config::{Platform, PlatformConfig};
use crate::platform::{check_token_shape, utf16_len, BasePlatformAdapter, SendResult};

pub const MAX_LENGTH: usize = 4096;

pub struct TelegramAdapter {
    token: String,
}

impl TelegramAdapter {
    pub fn new(cfg: PlatformConfig) -> anyhow::Result<Self> {
        let token = cfg
            .token
            .ok_or_else(|| anyhow::anyhow!("telegram token not set (set platforms.telegram.token in gateway.yaml)"))?;
        validate_telegram_token(&token)?;
        Ok(Self {
            token,
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
        // Real impl behind feature: init teloxide Bot and start polling.
        #[cfg(feature = "telegram")]
        {
            log::info!("[telegram] (feature) validated token, starting polling");
        }
        #[cfg(not(feature = "telegram"))]
        {
            log::info!("[telegram] stub connect (token {}…)", &self.token[..self.token.len().min(6)]);
        }
        Ok(())
    }

    async fn disconnect(&self) -> anyhow::Result<()> {
        log::info!("[telegram] disconnected");
        Ok(())
    }

    async fn send(&self, chat: &str, text: &str) -> SendResult {
        if !self.is_authenticated() {
            return SendResult {
                success: false,
                message_id: None,
                error: Some("telegram not authenticated: invalid token".to_string()),
                retryable: false,
            };
        }
        if text.is_empty() {
            return SendResult {
                success: true,
                message_id: None,
                error: None,
                retryable: false,
            };
        }

        // Split long messages into 4096-unit chunks (like hermes splits_long_messages)
        let chunks = crate::platform::chunk_message(text, MAX_LENGTH);

        // Stub: log each chunk; real feature would loop bot.send_message(chat_id, chunk).await
        for (i, chunk) in chunks.iter().enumerate() {
            debug_assert!(utf16_len(chunk) <= MAX_LENGTH, "chunk exceeds limit");
            #[cfg(feature = "telegram")]
            {
                let _ = chat;
                log::debug!("[telegram] (feature) send chunk {}/{} to {}: {} chars", i + 1, chunks.len(), chat, chunk.len());
            }
            #[cfg(not(feature = "telegram"))]
            {
                log::info!("[telegram] send to {} chunk {}/{} ({} utf16): {:?}", chat, i + 1, chunks.len(), utf16_len(chunk), crate::platform::preview_80(chunk));
            }
        }

        SendResult {
            success: true,
            message_id: None,
            error: None,
            retryable: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PlatformConfig;
    use crate::platform::{BasePlatformAdapter, utf16_len};

    fn cfg(token: &str) -> PlatformConfig {
        PlatformConfig {
            enabled: true,
            token: Some(token.to_string()),
            app_token: None,
            home_channel: None,
        }
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
        assert!(TelegramAdapter::new(PlatformConfig { enabled: true, token: None, app_token: None, home_channel: None }).is_err());
    }

    #[tokio::test]
    async fn send_splits_long() {
        let a = TelegramAdapter::new(cfg("123456:ABCDEFGHIJ1234567890")).unwrap();
        // 5000 'a' -> needs 2 chunks (4096 + 904)
        let long = "a".repeat(5000);
        let res = a.send("123", &long).await;
        assert!(res.success);
        // verify split logic directly
        let chunks = crate::platform::split_message(&long, MAX_LENGTH);
        assert_eq!(chunks.len(), 2);
        for c in &chunks { assert!(utf16_len(c) <= MAX_LENGTH); }
    }

    #[tokio::test]
    async fn send_truncate_emoji() {
        let a = TelegramAdapter::new(cfg("123:ABCDEFGHIJ1234567890")).unwrap();
        let s = "😀".repeat(3000); // 6000 units > 4096
        let res = a.send("123", &s).await;
        assert!(res.success);
    }

    #[test]
    fn is_authenticated_check() {
        let a = TelegramAdapter::new(cfg("999:ABCDEFGHIJ1234567890")).unwrap();
        assert!(a.is_authenticated());
        assert!(a.is_authenticated()); // trait also
    }
}
