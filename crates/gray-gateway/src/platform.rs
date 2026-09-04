//! BasePlatformAdapter + utf16 helpers + truncation/splitting

use async_trait::async_trait;
use crate::config::Platform;

#[derive(Debug, Clone)]
pub struct MessageEvent {
    pub text: String,
    pub message_id: Option<String>,
    pub source: crate::session::SessionSource,
    pub media_urls: Vec<String>,
    /// Display name of the sender, for pairing prompts / operator listings.
    pub user_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SendResult {
    pub success: bool,
    pub message_id: Option<String>,
    pub error: Option<String>,
    pub retryable: bool,
}

impl SendResult {
    pub fn ok(message_id: Option<String>) -> Self {
        Self { success: true, message_id, error: None, retryable: false }
    }
    pub fn fail(error: impl Into<String>, retryable: bool) -> Self {
        Self { success: false, message_id: None, error: Some(error.into()), retryable }
    }
}

/// Delivery hints for [`BasePlatformAdapter::send_ext`].
#[derive(Debug, Clone, Default)]
pub struct SendOptions {
    /// Platform message id to reply to (first chunk only — hermes `reply_to_mode=first`).
    pub reply_to: Option<String>,
    /// Thread to post into (Slack `thread_ts`, Telegram forum topic id).
    pub thread_id: Option<String>,
}

#[async_trait]
pub trait BasePlatformAdapter: Send + Sync {
    fn platform(&self) -> Platform;
    fn is_authenticated(&self) -> bool;
    /// Connect. When no event channel was wired via [`set_event_tx`] the
    /// adapter must come up in *send-only* mode (no inbound polling) so the
    /// `gray send` CLI can reuse the same code path.
    async fn connect(&self) -> anyhow::Result<()>;
    async fn disconnect(&self) -> anyhow::Result<()>;
    /// Bot display name learned during [`BasePlatformAdapter::connect`]
    /// (`get_me` / `current_user` / `auth.test`). Shown on the REPL boot card
    /// as `connected as <name>`. None for stubs or before connect.
    fn bot_identity(&self) -> Option<String> {
        None
    }
    async fn send(&self, chat: &str, text: &str) -> SendResult;

    /// Send with reply/thread hints. Default ignores the hints.
    async fn send_ext(&self, chat: &str, text: &str, _opts: &SendOptions) -> SendResult {
        self.send(chat, text).await
    }

    /// Wire the inbound event channel. Default no-op (stub adapters never receive).
    fn set_event_tx(&mut self, _tx: tokio::sync::mpsc::UnboundedSender<MessageEvent>) {}

    /// Best-effort typing indicator (Discord typing trigger). Default no-op.
    async fn send_typing(&self, _chat: &str) {}

    /// Whether [`edit_message`] works (streaming edit-in-place). Default false.
    fn supports_edit(&self) -> bool {
        false
    }

    /// Replace the text of a previously sent message (must fit in one chunk).
    async fn edit_message(&self, _chat: &str, _message_id: &str, _text: &str) -> SendResult {
        SendResult::fail(format!("{} does not support message edits", self.platform()), false)
    }
}

/// Exponential reconnect backoff: 1s, 2s, 4s … capped at 60s.
pub fn backoff_delay(attempt: u32) -> std::time::Duration {
    let secs = 1u64.checked_shl(attempt.min(6)).unwrap_or(64).min(60);
    std::time::Duration::from_secs(secs)
}

/// Count UTF-16 code units (Telegram/Discord limits are in utf16).
pub fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// First ~80 bytes of `s` for log previews, never splitting a char.
/// (`&s[..s.len().min(80)]` panics on multi-byte UTF-8 at the boundary.)
pub fn preview_80(s: &str) -> &str {
    if s.len() <= 80 {
        return s;
    }
    let mut i = 80;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    &s[..i]
}

/// Shared token preamble: trim, reject empty/whitespace. Returns trimmed token.
pub fn check_token_shape(token: &str, what: &str) -> anyhow::Result<String> {
    let t = token.trim();
    if t.is_empty() {
        anyhow::bail!("{what} empty");
    }
    if t.contains(' ') || t.contains('\n') {
        anyhow::bail!("{what} must not contain whitespace");
    }
    Ok(t.to_string())
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
/// Keeps char boundaries; does not attempt word wrap.
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

/// Like [`split_message`] but prefers breaking at the last newline (then
/// space) inside the window so code blocks and paragraphs stay readable.
/// Falls back to a hard split when no boundary exists.
pub fn split_message_smart(s: &str, max_utf16: usize) -> Vec<String> {
    if max_utf16 == 0 || s.is_empty() {
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
        let window = prefix_within_utf16_limit(remaining, max_utf16);
        let mut cut = window.len();
        // Only accept a soft boundary if it keeps at least half the window.
        let min_keep = window.len() / 2;
        if let Some(i) = window.rfind('\n').filter(|i| *i >= min_keep) {
            cut = i + 1;
        } else if let Some(i) = window.rfind(' ').filter(|i| *i >= min_keep) {
            cut = i + 1;
        }
        if cut == 0 {
            cut = remaining.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        }
        out.push(remaining[..cut].to_string());
        remaining = &remaining[cut..];
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

    #[test]
    fn preview_80_never_splits_char() {
        assert_eq!(preview_80("hi"), "hi");
        assert_eq!(preview_80(&"a".repeat(80)), "a".repeat(80));
        // 79 ascii + emoji: byte 80 is mid-emoji, old `&s[..80]` panicked here
        let s = "a".repeat(79) + "😀";
        let p = preview_80(&s);
        assert!(p.len() <= 80);
        assert!(s.is_char_boundary(p.len()));
        assert_eq!(p, "a".repeat(79));
    }

    #[test]
    fn smart_split_prefers_newlines() {
        let s = format!("{}\n{}", "a".repeat(60), "b".repeat(60));
        let chunks = split_message_smart(&s, 100);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], format!("{}\n", "a".repeat(60)));
        assert_eq!(chunks.concat(), s);
        for c in &chunks { assert!(utf16_len(c) <= 100); }
    }

    #[test]
    fn smart_split_hard_fallback_and_emoji() {
        let s = "😀".repeat(10);
        let chunks = split_message_smart(&s, 5);
        assert_eq!(chunks.concat(), s);
        for c in &chunks { assert!(utf16_len(c) <= 5); }
        let s = "x".repeat(250);
        assert_eq!(split_message_smart(&s, 100).len(), 3);
    }

    #[test]
    fn backoff_caps() {
        assert_eq!(backoff_delay(0).as_secs(), 1);
        assert_eq!(backoff_delay(3).as_secs(), 8);
        assert_eq!(backoff_delay(20).as_secs(), 60);
    }
}
