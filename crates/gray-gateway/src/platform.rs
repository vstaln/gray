//! BasePlatformAdapter + utf16 helpers + truncation/splitting

use async_trait::async_trait;
use crate::config::Platform;

#[derive(Debug, Clone)]
pub struct MessageEvent {
    pub text: String,
    pub message_id: Option<String>,
    pub source: crate::session::SessionSource,
    pub media_urls: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SendResult {
    pub success: bool,
    pub message_id: Option<String>,
    pub error: Option<String>,
    pub retryable: bool,
}

#[async_trait]
pub trait BasePlatformAdapter: Send + Sync {
    fn platform(&self) -> Platform;
    fn is_authenticated(&self) -> bool;
    async fn connect(&self) -> anyhow::Result<()>;
    async fn disconnect(&self) -> anyhow::Result<()>;
    async fn send(&self, chat: &str, text: &str) -> SendResult;

    /// Wire the inbound event channel. Default no-op (stub adapters never receive).
    fn set_event_tx(&mut self, _tx: tokio::sync::mpsc::UnboundedSender<MessageEvent>) {}

    /// Best-effort typing indicator (Discord typing trigger). Default no-op.
    async fn send_typing(&self, _chat: &str) {}
}

/// Count UTF-16 code units (Telegram/Discord limits are in utf16).
pub fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Longest prefix of `s` whose utf16_len <= limit, without slicing a char.
pub fn prefix_within_utf16_limit(s: &str, limit: usize) -> String {
    if utf16_len(s) <= limit {
        return s.to_string();
    }
    // Linear scan — max 39000 chars, negligible; correct for surrogate pairs.
    let mut cur = String::new();
    let mut cu = 0usize;
    for c in s.chars() {
        let cl = c.len_utf16();
        if cu + cl > limit {
            break;
        }
        cur.push(c);
        cu += cl;
    }
    cur
}

/// Truncate to max_utf16 with trailing ellipsis (1 unit) if overflow.
pub fn truncate_message(s: &str, max_utf16: usize) -> String {
    if utf16_len(s) <= max_utf16 {
        return s.to_string();
    }
    if max_utf16 == 0 {
        return String::new();
    }
    // reserve 1 for "…"
    let prefix = prefix_within_utf16_limit(s, max_utf16 - 1);
    format!("{prefix}…")
}

/// Split text into chunks each <= max_utf16 (measured in utf16 units).
/// Keeps char boundaries; does not attempt word wrap (ponytail: simplest).
pub fn split_message(s: &str, max_utf16: usize) -> Vec<String> {
    if max_utf16 == 0 {
        return vec![];
    }
    if s.is_empty() {
        return vec![];
    }
    if utf16_len(s) <= max_utf16 {
        return vec![s.to_string()];
    }
    let mut out = Vec::new();
    let mut remaining = s;
    while !remaining.is_empty() {
        if utf16_len(remaining) <= max_utf16 {
            out.push(remaining.to_string());
            break;
        }
        let chunk = prefix_within_utf16_limit(remaining, max_utf16);
        // Ensure progress even if single char exceeds limit (e.g. limit 1 but char needs 2 units like emoji)
        let take_len = if chunk.is_empty() {
            // take one char regardless of overflow
            let c = remaining.chars().next().unwrap();
            c.len_utf8()
        } else {
            chunk.len()
        };
        out.push(remaining[..take_len].to_string());
        remaining = &remaining[take_len..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_len_ascii() {
        assert_eq!(utf16_len("hello"), 5);
    }

    #[test]
    fn utf16_len_emoji_is_two() {
        // 😀 is outside BMP -> surrogate pair -> 2 units
        assert_eq!(utf16_len("😀"), 2);
        assert_eq!(utf16_len("a😀b"), 4);
    }

    #[test]
    fn truncate_ascii() {
        assert_eq!(truncate_message("hello world", 5), "hell…");
        assert_eq!(truncate_message("hi", 10), "hi");
    }

    #[test]
    fn truncate_emoji_respects_units() {
        // "a😀b" len 4; max 3 -> prefix "a" (1) then ellipsis, because "a😀" would be 3 units already but need reserve 1
        let s = "a😀b";
        let t = truncate_message(s, 3);
        assert!(utf16_len(&t) <= 3);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn truncate_empty_and_zero() {
        assert_eq!(truncate_message("", 5), "");
        assert_eq!(truncate_message("hello", 0), "");
    }

    #[test]
    fn prefix_emoji_boundary() {
        let s = "😀😀😀"; // 6 units
        let p = prefix_within_utf16_limit(s, 3);
        // can fit only 1 emoji (2 units) within 3
        assert_eq!(utf16_len(&p), 2);
        assert_eq!(p, "😀");
    }

    #[test]
    fn split_basic() {
        let s = "a".repeat(5000);
        let chunks = split_message(&s, 2000);
        assert_eq!(chunks.len(), 3);
        for c in &chunks {
            assert!(utf16_len(c) <= 2000);
        }
        assert_eq!(chunks.join(""), s);
    }

    #[test]
    fn split_emoji_no_panic() {
        let s = "😀".repeat(10); // 20 units
        let chunks = split_message(&s, 5);
        for c in &chunks {
            assert!(utf16_len(c) <= 5);
        }
        assert_eq!(chunks.join(""), s);
    }

    #[test]
    fn split_preserves_content() {
        let s = "hello world ".repeat(100);
        let chunks = split_message(&s, 100);
        assert_eq!(chunks.join(""), s);
    }
}
