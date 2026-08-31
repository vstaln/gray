//! BasePlatformAdapter
use async_trait::async_trait;
use crate::config::Platform;
#[derive(Debug, Clone)] pub struct MessageEvent { pub text: String, pub message_id: Option<String>, pub source: crate::session::SessionSource, pub media_urls: Vec<String> }
#[derive(Debug, Clone)] pub struct SendResult { pub success: bool, pub message_id: Option<String>, pub error: Option<String>, pub retryable: bool }
#[async_trait] pub trait BasePlatformAdapter: Send+Sync {
    fn platform(&self) -> Platform;
    async fn connect(&mut self) -> anyhow::Result<()>;
    async fn disconnect(&mut self) -> anyhow::Result<()>;
    async fn send(&self, chat: &str, text: &str) -> SendResult;
}
pub fn utf16_len(s: &str) -> usize { s.encode_utf16().count() }
pub fn truncate_message(s: &str, max_utf16: usize) -> String {
    if utf16_len(s) <= max_utf16 { return s.to_string(); }
    let mut buf = String::new(); let mut len = 0;
    for c in s.chars() { let cl = c.len_utf16(); if len + cl > max_utf16 - 3 { buf.push_str("…"); break; } buf.push(c); len += cl; }
    buf
}
