//! QQBot platform package.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/platforms/qqbot/__init__.py` (91 LOC).
//!
//! Re-exports the main adapter symbols from `adapter.py` (the original
//! `qqbot.py`) so that **all existing import paths remain unchanged**:
//! ```text
//! from gateway.platforms.qqbot import QQAdapter          # works
//! from gateway.platforms.qqbot import check_qq_requirements  # works
//! ```
//!
//! New modules:
//!     - `constants` — shared constants (API URLs, timeouts, message types)
//!     - `utils` — User-Agent builder, config helpers
//!     - `crypto` — AES-256-GCM key generation and decryption
//!     - `onboard` — QR-code scan-to-configure flow
//!
//! Python source docstring (preserved):
//! ```text
//! QQBot platform package.
//!
//! Re-exports the main adapter symbols from ``adapter.py`` (the original
//! ``qqbot.py``) so that **all existing import paths remain unchanged**::
//!
//!     from gateway.platforms.qqbot import QQAdapter          # works
//!     from gateway.platforms.qqbot import check_qq_requirements  # works
//!
//! New modules:
//!     - ``constants`` — shared constants (API URLs, timeouts, message types)
//!     - ``utils`` — User-Agent builder, config helpers
//!     - ``crypto`` — AES-256-GCM key generation and decryption
//!     - ``onboard`` — QR-code scan-to-configure flow
//! ```
//!
//! Python re-exports (preserved verbatim):
//! ```python
//! # -- Adapter (original qqbot.py) ------------------------------------------
//! from .adapter import (  # noqa: F401
//!     QQAdapter,
//!     QQCloseError,
//!     check_qq_requirements,
//!     _coerce_list,
//!     _ssrf_redirect_guard,
//! )
//!
//! # -- Onboard (QR-code scan-to-configure) -----------------------------------
//! from .onboard import (  # noqa: F401
//!     BindStatus,
//!     build_connect_url,
//!     qr_register,
//! )
//! from .crypto import decrypt_secret, generate_bind_key  # noqa: F401
//!
//! # -- Utils -----------------------------------------------------------------
//! from .utils import build_user_agent, get_api_headers, coerce_list  # noqa: F401
//!
//! # -- Chunked upload --------------------------------------------------------
//! from .chunked_upload import (  # noqa: F401
//!     ChunkedUploader,
//!     UploadDailyLimitExceededError,
//!     UploadFileTooLargeError,
//! )
//!
//! # -- Inline keyboards ------------------------------------------------------
//! from .keyboards import (  # noqa: F401
//!     ApprovalRequest,
//!     ApprovalSender,
//!     InlineKeyboard,
//!     InteractionEvent,
//!     build_approval_keyboard,
//!     build_approval_text,
//!     build_update_prompt_keyboard,
//!     parse_approval_button_data,
//!     parse_interaction_event,
//!     parse_update_prompt_button_data,
//! )
//!
//! __all__ = [
//!     # adapter
//!     "QQAdapter",
//!     "QQCloseError",
//!     "check_qq_requirements",
//!     "_coerce_list",
//!     "_ssrf_redirect_guard",
//!     # onboard
//!     "BindStatus",
//!     "build_connect_url",
//!     "qr_register",
//!     # crypto
//!     "decrypt_secret",
//!     "generate_bind_key",
//!     # utils
//!     "build_user_agent",
//!     "get_api_headers",
//!     "coerce_list",
//!     # chunked upload
//!     "ChunkedUploader",
//!     "UploadDailyLimitExceededError",
//!     "UploadFileTooLargeError",
//!     # keyboards
//!     "ApprovalRequest",
//!     "ApprovalSender",
//!     "InlineKeyboard",
//!     "InteractionEvent",
//!     "build_approval_keyboard",
//!     "build_approval_text",
//!     "build_update_prompt_keyboard",
//!     "parse_approval_button_data",
//!     "parse_interaction_event",
//!     "parse_update_prompt_button_data",
//! ]
//! ```
//!
//! Rust notes:
//! - Until `adapter`, `onboard`, `crypto`, `utils`, `chunked_upload`, `keyboards`
//!   are ported as `crate::qqbot_adapter` / `crate::qqbot_onboard` etc., this
//!   module documents the unified public surface and exposes `ALL` plus stub
//!   types/fns for 1:1 discoverability.
//!   Re-exports will be wired as `pub use crate::qqbot_adapter::{QQAdapter, ...}`
//!   and `pub use crate::qqbot_onboard::{BindStatus, ...}` etc. once those
//!   modules exist.
//!   ponytail: stub surface until submodules land; wire pub use when modules land.
//!
//! Mapping:
//! - `from .adapter import QQAdapter` → [`QQAdapter`]
//! - `from .adapter import QQCloseError` → [`QQCloseError`]
//! - `from .adapter import check_qq_requirements` → [`check_qq_requirements`]
//! - `from .adapter import _coerce_list` → [`_coerce_list`]
//! - `from .adapter import _ssrf_redirect_guard` → [`_ssrf_redirect_guard`]
//! - `from .onboard import BindStatus` → [`BindStatus`]
//! - `from .onboard import build_connect_url` → [`build_connect_url`]
//! - `from .onboard import qr_register` → [`qr_register`]
//! - `from .crypto import decrypt_secret` → [`decrypt_secret`]
//! - `from .crypto import generate_bind_key` → [`generate_bind_key`]
//! - `from .utils import build_user_agent` → [`build_user_agent`]
//! - `from .utils import get_api_headers` → [`get_api_headers`]
//! - `from .utils import coerce_list` → [`coerce_list`]
//! - `from .chunked_upload import ChunkedUploader` → [`ChunkedUploader`]
//! - `from .chunked_upload import UploadDailyLimitExceededError` → [`UploadDailyLimitExceededError`]
//! - `from .chunked_upload import UploadFileTooLargeError` → [`UploadFileTooLargeError`]
//! - `from .keyboards import ApprovalRequest` → [`ApprovalRequest`]
//! - `from .keyboards import ApprovalSender` → [`ApprovalSender`]
//! - `from .keyboards import InlineKeyboard` → [`InlineKeyboard`]
//! - `from .keyboards import InteractionEvent` → [`InteractionEvent`]
//! - `from .keyboards import build_approval_keyboard` → [`build_approval_keyboard`]
//! - `from .keyboards import build_approval_text` → [`build_approval_text`]
//! - `from .keyboards import build_update_prompt_keyboard` → [`build_update_prompt_keyboard`]
//! - `from .keyboards import parse_approval_button_data` → [`parse_approval_button_data`]
//! - `from .keyboards import parse_interaction_event` → [`parse_interaction_event`]
//! - `from .keyboards import parse_update_prompt_button_data` → [`parse_update_prompt_button_data`]
//! - `__all__` → [`ALL`] / [`__ALL__`]

// ---------------------------------------------------------------------------
// Public surface — mirrors `__all__` (26 entries)
// ---------------------------------------------------------------------------

/// Unified public surface, mirroring Python `__all__` (26 entries).
pub const ALL: &[&str] = &[
    // adapter
    "QQAdapter",
    "QQCloseError",
    "check_qq_requirements",
    "_coerce_list",
    "_ssrf_redirect_guard",
    // onboard
    "BindStatus",
    "build_connect_url",
    "qr_register",
    // crypto
    "decrypt_secret",
    "generate_bind_key",
    // utils
    "build_user_agent",
    "get_api_headers",
    "coerce_list",
    // chunked upload
    "ChunkedUploader",
    "UploadDailyLimitExceededError",
    "UploadFileTooLargeError",
    // keyboards
    "ApprovalRequest",
    "ApprovalSender",
    "InlineKeyboard",
    "InteractionEvent",
    "build_approval_keyboard",
    "build_approval_text",
    "build_update_prompt_keyboard",
    "parse_approval_button_data",
    "parse_interaction_event",
    "parse_update_prompt_button_data",
];

/// Alias matching Python `__all__` name for grep discoverability.
pub const __ALL__: &[&str] = ALL;

// Re-exports (future):
// Once `crate::qqbot_adapter`, `crate::qqbot_onboard`, `crate::qqbot_crypto`,
// `crate::qqbot_utils`, `crate::qqbot_chunked_upload`, `crate::qqbot_keyboards`
// exist, wire:
//   pub use crate::qqbot_adapter::{QQAdapter, QQCloseError, check_qq_requirements, _coerce_list, _ssrf_redirect_guard};
//   pub use crate::qqbot_onboard::{BindStatus, build_connect_url, qr_register};
//   pub use crate::qqbot_crypto::{decrypt_secret, generate_bind_key};
//   pub use crate::qqbot_utils::{build_user_agent, get_api_headers, coerce_list};
//   pub use crate::qqbot_chunked_upload::{ChunkedUploader, UploadDailyLimitExceededError, UploadFileTooLargeError};
//   pub use crate::qqbot_keyboards::{ApprovalRequest, ApprovalSender, InlineKeyboard, InteractionEvent, build_approval_keyboard, build_approval_text, build_update_prompt_keyboard, parse_approval_button_data, parse_interaction_event, parse_update_prompt_button_data};
// Until then this module exposes `ALL` plus stub types/fns below.

// ---------------------------------------------------------------------------
// Stub types — adapter (original qqbot.py)
// ---------------------------------------------------------------------------

/// QQ Bot platform adapter. Mirrors `gateway/platforms/qqbot/adapter.py::QQAdapter`.
#[derive(Debug, Clone, Default)]
pub struct QQAdapter {
    /// Placeholder config snapshot.
    pub config: Option<serde_json::Value>,
}

/// Raised when QQ WebSocket closes. Mirrors `QQCloseError` with `code` + `reason`.
#[derive(Debug, Clone)]
pub struct QQCloseError {
    pub code: Option<i32>,
    pub reason: String,
}

impl std::fmt::Display for QQCloseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WebSocket closed (code={:?}, reason={})", self.code, self.reason)
    }
}
impl std::error::Error for QQCloseError {}

/// Check if QQ runtime dependencies are available. Mirrors `check_qq_requirements() -> bool`.
pub fn check_qq_requirements() -> bool {
    // ponytail: stub returns false until adapter http deps land
    false
}

/// Coerce config values into a trimmed string list. Mirrors `adapter._coerce_list`.
pub fn _coerce_list(value: &serde_json::Value) -> Vec<String> {
    coerce_list(value)
}

/// SSRF redirect guard for QQ HTTP client. Mirrors `gateway/platforms/base._ssrf_redirect_guard` re-exported via adapter.
pub fn _ssrf_redirect_guard(_url: &str) -> Result<(), String> {
    // ponytail: stub validates via url crate when wired; accept all for now with empty check
    Ok(())
}

// ---------------------------------------------------------------------------
// Stub types — onboard (QR-code scan-to-configure)
// ---------------------------------------------------------------------------

/// Status codes returned by poll. Mirrors `gateway/platforms/qqbot/onboard.py::BindStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(i32)]
pub enum BindStatus {
    None = 0,
    Pending = 1,
    Completed = 2,
    Expired = 3,
}

impl BindStatus {
    pub fn from_i32(v: i32) -> Self {
        match v {
            1 => Self::Pending,
            2 => Self::Completed,
            3 => Self::Expired,
            _ => Self::None,
        }
    }
}

/// Build the QR-code target URL for a given task_id. Mirrors `build_connect_url`.
pub fn build_connect_url(task_id: &str) -> String {
    // Mirrors QR_URL_TEMPLATE.format(task_id=quote(task_id))
    // ponytail: stub encodes minimal; real impl uses urlencoding when onboard crate lands
    format!(
        "https://q.qq.com/qrcode/{}",
        urlencoding_stub(task_id)
    )
}

fn urlencoding_stub(s: &str) -> String {
    // minimal percent-encoding for non-alphanum (ponytail: naive, replace with urlencoding crate when onboard lands)
    let mut out = String::new();
    for b in s.bytes() {
        let c = b as char;
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
            out.push(c);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

/// Run the QQBot scan-to-configure QR registration flow. Mirrors `qr_register`.
pub fn qr_register(_timeout_seconds: Option<u64>) -> Option<serde_json::Value> {
    // ponytail: stub returns None until onboard http flow lands
    None
}

// ---------------------------------------------------------------------------
// Stub types — crypto (AES-256-GCM)
// ---------------------------------------------------------------------------

/// Generate a 256-bit random AES key as base64. Mirrors `generate_bind_key() -> str`.
pub fn generate_bind_key() -> String {
    // ponytail: stub returns empty until crypto crate lands; real impl uses base64(os.urandom(32))
    String::new()
}

/// Decrypt a base64-encoded AES-256-GCM ciphertext. Mirrors `decrypt_secret`.
pub fn decrypt_secret(_encrypted_base64: &str, _key_base64: &str) -> Result<String, String> {
    // ponytail: stub errors until cryptography/AES-GCM wiring lands
    Err("decrypt_secret not yet implemented (stub)".to_string())
}

// ---------------------------------------------------------------------------
// Stub types — utils (User-Agent, config coercion)
// ---------------------------------------------------------------------------

/// Build a descriptive User-Agent string. Mirrors `build_user_agent() -> str`.
pub fn build_user_agent() -> String {
    // ponytail: stub fixed version until constants.version lands
    "QQBotAdapter/1.0.0 (Hermes/0.1.1)".to_string()
}

/// Return standard HTTP headers for QQBot API requests. Mirrors `get_api_headers()`.
pub fn get_api_headers() -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    m.insert("Content-Type".to_string(), "application/json".to_string());
    m.insert("Accept".to_string(), "application/json".to_string());
    m.insert("User-Agent".to_string(), build_user_agent());
    m
}

/// Coerce config values into a trimmed string list. Mirrors `utils.coerce_list`.
pub fn coerce_list(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::Null => vec![],
        serde_json::Value::String(s) => s
            .split(',')
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .collect(),
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|v| {
                let s = match v {
                    serde_json::Value::String(s) => s.clone(),
                    _ => v.to_string().trim_matches('"').to_string(),
                };
                let t = s.trim().to_string();
                if t.is_empty() { None } else { Some(t) }
            })
            .collect(),
        _ => {
            let s = value.to_string().trim_matches('"').to_string();
            let t = s.trim().to_string();
            if t.is_empty() { vec![] } else { vec![t] }
        }
    }
}

// ---------------------------------------------------------------------------
// Stub types — chunked upload
// ---------------------------------------------------------------------------

/// Run the prepare → PUT parts → complete sequence. Mirrors `ChunkedUploader`.
#[derive(Debug, Clone, Default)]
pub struct ChunkedUploader {
    pub log_tag: String,
}

/// Raised when upload_prepare returns daily-limit biz_code. Mirrors `UploadDailyLimitExceededError`.
#[derive(Debug, Clone)]
pub struct UploadDailyLimitExceededError {
    pub file_name: String,
    pub file_size: i64,
    pub message: String,
}

impl std::fmt::Display for UploadDailyLimitExceededError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for UploadDailyLimitExceededError {}

/// Raised when a file exceeds the platform per-file size limit. Mirrors `UploadFileTooLargeError`.
#[derive(Debug, Clone)]
pub struct UploadFileTooLargeError {
    pub file_name: String,
    pub file_size: i64,
    pub limit_bytes: i64,
    pub message: String,
}

impl std::fmt::Display for UploadFileTooLargeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for UploadFileTooLargeError {}

// ---------------------------------------------------------------------------
// Stub types — inline keyboards
// ---------------------------------------------------------------------------

/// Structured approval-request display data. Mirrors `keyboards.ApprovalRequest`.
#[derive(Debug, Clone, Default)]
pub struct ApprovalRequest {
    pub session_key: String,
    pub title: String,
    pub description: String,
    pub command_preview: String,
    pub cwd: String,
    pub tool_name: String,
    pub severity: String,
    pub timeout_sec: i64,
    pub allow_permanent: bool,
}

/// Send an approval-request message with an inline keyboard. Mirrors `ApprovalSender`.
#[derive(Debug, Clone, Default)]
pub struct ApprovalSender {
    pub log_tag: String,
}

/// Top-level keyboard payload — goes into MessageToCreate.keyboard. Mirrors `InlineKeyboard`.
#[derive(Debug, Clone, Default)]
pub struct InlineKeyboard {
    pub content: serde_json::Value,
}

/// Parsed INTERACTION_CREATE event payload. Mirrors `InteractionEvent`.
#[derive(Debug, Clone, Default)]
pub struct InteractionEvent {
    pub id: String,
    pub r#type: i32,
    pub chat_type: i32,
    pub scene: String,
    pub group_openid: String,
    pub group_member_openid: String,
    pub user_openid: String,
    pub channel_id: String,
    pub guild_id: String,
    pub button_data: String,
    pub button_id: String,
    pub resolver_user_id: String,
}

impl InteractionEvent {
    /// Best available operator openid (group → member; c2c → user). Mirrors `operator_openid` property.
    pub fn operator_openid(&self) -> &str {
        if !self.group_member_openid.is_empty() {
            &self.group_member_openid
        } else if !self.user_openid.is_empty() {
            &self.user_openid
        } else {
            &self.resolver_user_id
        }
    }
}

/// Build the approval keyboard. Mirrors `build_approval_keyboard`.
pub fn build_approval_keyboard(session_key: &str, _allow_permanent: Option<bool>) -> InlineKeyboard {
    let _ = session_key;
    InlineKeyboard::default()
}

/// Render an ApprovalRequest into the message body. Mirrors `build_approval_text`.
pub fn build_approval_text(req: &ApprovalRequest) -> String {
    // ponytail: minimal stub; real impl renders markdown with command_preview/cwd branches
    if req.command_preview.is_empty() && req.cwd.is_empty() {
        format!("Approval: {}", req.title)
    } else {
        format!("Command approval: {}", req.command_preview)
    }
}

/// Build a Yes/No keyboard for update confirmation prompts. Mirrors `build_update_prompt_keyboard`.
pub fn build_update_prompt_keyboard() -> InlineKeyboard {
    InlineKeyboard::default()
}

/// Parse approval button_data into (session_key, decision). Mirrors `parse_approval_button_data`.
pub fn parse_approval_button_data(button_data: &str) -> Option<(String, String)> {
    // Pattern: approve:<session_key>:<decision> where decision = allow-once|allow-always|deny
    let prefix = "approve:";
    if !button_data.starts_with(prefix) {
        return None;
    }
    let rest = &button_data[prefix.len()..];
    for suffix in ["allow-once", "allow-always", "deny"] {
        let needle = format!(":{}", suffix);
        if rest.ends_with(&needle) {
            let session_key = rest[..rest.len() - needle.len()].to_string();
            if session_key.is_empty() {
                return None;
            }
            return Some((session_key, suffix.to_string()));
        }
    }
    None
}

/// Parse a raw INTERACTION_CREATE dispatch payload. Mirrors `parse_interaction_event`.
pub fn parse_interaction_event(raw: &serde_json::Value) -> InteractionEvent {
    let empty = serde_json::Map::new();
    let obj = raw.as_object().unwrap_or(&empty);
    let data = obj.get("data").and_then(|v| v.as_object()).unwrap_or(&empty);
    let resolved = data.get("resolved").and_then(|v| v.as_object()).unwrap_or(&empty);
    let scene_code = obj.get("chat_type").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    let scene = match scene_code {
        0 => "guild",
        1 => "group",
        2 => "c2c",
        _ => "",
    }
    .to_string();
    InteractionEvent {
        id: obj.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        r#type: data.get("type").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
        chat_type: scene_code,
        scene,
        group_openid: obj.get("group_openid").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        group_member_openid: obj.get("group_member_openid").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        user_openid: obj.get("user_openid").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        channel_id: obj.get("channel_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        guild_id: obj.get("guild_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        button_data: resolved.get("button_data").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        button_id: resolved.get("button_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        resolver_user_id: resolved.get("user_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
    }
}

/// Parse update-prompt button_data into 'y' or 'n'. Mirrors `parse_update_prompt_button_data`.
pub fn parse_update_prompt_button_data(button_data: &str) -> Option<String> {
    match button_data {
        "update_prompt:y" => Some("y".to_string()),
        "update_prompt:n" => Some("n".to_string()),
        _ => None,
    }
}

// Private aliases mirroring Python's underscore-prefixed helpers for traceability
#[allow(dead_code)]
fn _coerce_list_impl(value: &serde_json::Value) -> Vec<String> {
    coerce_list(value)
}

#[allow(dead_code)]
fn _build_connect_url(task_id: &str) -> String {
    build_connect_url(task_id)
}

#[allow(dead_code)]
fn _generate_bind_key() -> String {
    generate_bind_key()
}

#[allow(dead_code)]
fn _decrypt_secret(a: &str, b: &str) -> Result<String, String> {
    decrypt_secret(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_matches_python() {
        assert_eq!(
            ALL,
            [
                "QQAdapter",
                "QQCloseError",
                "check_qq_requirements",
                "_coerce_list",
                "_ssrf_redirect_guard",
                "BindStatus",
                "build_connect_url",
                "qr_register",
                "decrypt_secret",
                "generate_bind_key",
                "build_user_agent",
                "get_api_headers",
                "coerce_list",
                "ChunkedUploader",
                "UploadDailyLimitExceededError",
                "UploadFileTooLargeError",
                "ApprovalRequest",
                "ApprovalSender",
                "InlineKeyboard",
                "InteractionEvent",
                "build_approval_keyboard",
                "build_approval_text",
                "build_update_prompt_keyboard",
                "parse_approval_button_data",
                "parse_interaction_event",
                "parse_update_prompt_button_data",
            ]
        );
        assert_eq!(__ALL__, ALL);
        assert_eq!(ALL.len(), 26);
    }

    #[test]
    fn parse_approval_roundtrip() {
        let data = "approve:agent:main:qqbot:c2c:OPENID123:allow-once";
        let (sk, dec) = parse_approval_button_data(data).unwrap();
        assert_eq!(sk, "agent:main:qqbot:c2c:OPENID123");
        assert_eq!(dec, "allow-once");
        assert!(parse_approval_button_data("not-approval").is_none());
        assert!(parse_approval_button_data("approve:only-one-part").is_none());
    }

    #[test]
    fn parse_update_prompt() {
        assert_eq!(parse_update_prompt_button_data("update_prompt:y"), Some("y".to_string()));
        assert_eq!(parse_update_prompt_button_data("update_prompt:n"), Some("n".to_string()));
        assert_eq!(parse_update_prompt_button_data("update_prompt:x"), None);
        assert_eq!(parse_update_prompt_button_data(""), None);
    }

    #[test]
    fn coerce_list_variants() {
        assert_eq!(coerce_list(&serde_json::Value::Null), Vec::<String>::new());
        assert_eq!(
            coerce_list(&serde_json::json!("a, b ,c")),
            vec!["a", "b", "c"]
        );
        assert_eq!(
            coerce_list(&serde_json::json!(["x", " y "])),
            vec!["x", "y"]
        );
        assert_eq!(_coerce_list(&serde_json::json!("a,b")), vec!["a", "b"]);
    }

    #[test]
    fn bind_status_conversion() {
        assert_eq!(BindStatus::from_i32(0), BindStatus::None);
        assert_eq!(BindStatus::from_i32(2), BindStatus::Completed);
        assert_eq!(BindStatus::from_i32(99), BindStatus::None);
    }

    #[test]
    fn headers_contain_user_agent() {
        let h = get_api_headers();
        assert_eq!(h.get("Content-Type").map(|s| s.as_str()), Some("application/json"));
        assert!(h.get("User-Agent").unwrap().contains("QQBotAdapter"));
    }
}
