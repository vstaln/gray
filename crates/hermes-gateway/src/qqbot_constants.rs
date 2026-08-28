//! QQBot package-level constants shared across adapter, onboard, and other modules.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/platforms/qqbot/constants.py` (74 LOC).
//!
//! Python source docstring (preserved):
//! ```text
//! QQBot package-level constants shared across adapter, onboard, and other modules.
//! ```
//!
//! Mapping:
//! - `QQBOT_VERSION = "1.1.0"` → [`QQBOT_VERSION`]
//! - `PORTAL_HOST = os.getenv("QQ_PORTAL_HOST", "q.qq.com")` → [`PORTAL_HOST`] (default) + [`portal_host()`] (runtime env) + [`QQ_PORTAL_HOST_ENV`] / [`DEFAULT_PORTAL_HOST`]
//! - `API_BASE = "https://api.sgroup.qq.com"` → [`API_BASE`]
//! - `TOKEN_URL = "https://bots.qq.com/app/getAppAccessToken"` → [`TOKEN_URL`]
//! - `GATEWAY_URL_PATH = "/gateway"` → [`GATEWAY_URL_PATH`]
//! - `ONBOARD_CREATE_PATH = "/lite/create_bind_task"` → [`ONBOARD_CREATE_PATH`]
//! - `ONBOARD_POLL_PATH = "/lite/poll_bind_result"` → [`ONBOARD_POLL_PATH`]
//! - `QR_URL_TEMPLATE = "https://q.qq.com/qqbot/openclaw/connect.html?task_id={task_id}&_wv=2&source=hermes"` → [`QR_URL_TEMPLATE`] + [`qr_url()`]
//! - `DEFAULT_API_TIMEOUT = 30.0` → [`DEFAULT_API_TIMEOUT`]
//! - `FILE_UPLOAD_TIMEOUT = 120.0` → [`FILE_UPLOAD_TIMEOUT`]
//! - `CONNECT_TIMEOUT_SECONDS = 20.0` → [`CONNECT_TIMEOUT_SECONDS`]
//! - `RECONNECT_BACKOFF = [2, 5, 10, 30, 60]` → [`RECONNECT_BACKOFF`]
//! - `MAX_RECONNECT_ATTEMPTS = 100` → [`MAX_RECONNECT_ATTEMPTS`]
//! - `RATE_LIMIT_DELAY = 60` → [`RATE_LIMIT_DELAY`]
//! - `QUICK_DISCONNECT_THRESHOLD = 5.0` → [`QUICK_DISCONNECT_THRESHOLD`]
//! - `MAX_QUICK_DISCONNECT_COUNT = 3` → [`MAX_QUICK_DISCONNECT_COUNT`]
//! - `ONBOARD_POLL_INTERVAL = 2.0` → [`ONBOARD_POLL_INTERVAL`]
//! - `ONBOARD_API_TIMEOUT = 10.0` → [`ONBOARD_API_TIMEOUT`]
//! - `MAX_MESSAGE_LENGTH = 4000` → [`MAX_MESSAGE_LENGTH`]
//! - `DEDUP_WINDOW_SECONDS = 300` → [`DEDUP_WINDOW_SECONDS`]
//! - `DEDUP_MAX_SIZE = 1000` → [`DEDUP_MAX_SIZE`]
//! - `MSG_TYPE_TEXT = 0` → [`MSG_TYPE_TEXT`]
//! - `MSG_TYPE_MARKDOWN = 2` → [`MSG_TYPE_MARKDOWN`]
//! - `MSG_TYPE_MEDIA = 7` → [`MSG_TYPE_MEDIA`]
//! - `MSG_TYPE_INPUT_NOTIFY = 6` → [`MSG_TYPE_INPUT_NOTIFY`]
//! - `MEDIA_TYPE_IMAGE = 1` → [`MEDIA_TYPE_IMAGE`]
//! - `MEDIA_TYPE_VIDEO = 2` → [`MEDIA_TYPE_VIDEO`]
//! - `MEDIA_TYPE_VOICE = 3` → [`MEDIA_TYPE_VOICE`]
//! - `MEDIA_TYPE_FILE = 4` → [`MEDIA_TYPE_FILE`]

// ---------------------------------------------------------------------------
// QQBot adapter version — bump on functional changes to the adapter package.
// ---------------------------------------------------------------------------

/// QQBot adapter version. Mirrors `QQBOT_VERSION = "1.1.0"`.
pub const QQBOT_VERSION: &str = "1.1.0";

// ---------------------------------------------------------------------------
// API endpoints
// ---------------------------------------------------------------------------

/// Env var name for portal host override. Mirrors `os.getenv("QQ_PORTAL_HOST", ...)`.
pub const QQ_PORTAL_HOST_ENV: &str = "QQ_PORTAL_HOST";

/// Alias for grep-ability.
pub const PORTAL_HOST_ENV: &str = QQ_PORTAL_HOST_ENV;

/// Default portal host when `QQ_PORTAL_HOST` is unset. Mirrors `"q.qq.com"`.
pub const DEFAULT_PORTAL_HOST: &str = "q.qq.com";

/// Default portal host value. Mirrors `PORTAL_HOST` default (`"q.qq.com"`).
/// For runtime env-aware value, use [`portal_host()`].
pub const PORTAL_HOST: &str = DEFAULT_PORTAL_HOST;

/// Resolve portal host from env at runtime.
///
/// Mirrors:
/// ```python
/// PORTAL_HOST = os.getenv("QQ_PORTAL_HOST", "q.qq.com")
/// ```
pub fn portal_host() -> String {
    let raw = std::env::var(QQ_PORTAL_HOST_ENV).unwrap_or_default();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        DEFAULT_PORTAL_HOST.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Private alias mirroring Python `PORTAL_HOST` name for traceability.
#[allow(dead_code)]
fn _portal_host() -> String {
    portal_host()
}

/// API base URL. Mirrors `API_BASE = "https://api.sgroup.qq.com"`.
pub const API_BASE: &str = "https://api.sgroup.qq.com";

/// Token URL. Mirrors `TOKEN_URL = "https://bots.qq.com/app/getAppAccessToken"`.
pub const TOKEN_URL: &str = "https://bots.qq.com/app/getAppAccessToken";

/// Gateway URL path. Mirrors `GATEWAY_URL_PATH = "/gateway"`.
pub const GATEWAY_URL_PATH: &str = "/gateway";

/// QR-code onboard create path (on the portal host). Mirrors `ONBOARD_CREATE_PATH = "/lite/create_bind_task"`.
pub const ONBOARD_CREATE_PATH: &str = "/lite/create_bind_task";

/// QR-code onboard poll path (on the portal host). Mirrors `ONBOARD_POLL_PATH = "/lite/poll_bind_result"`.
pub const ONBOARD_POLL_PATH: &str = "/lite/poll_bind_result";

/// QR URL template with `{task_id}` placeholder. Mirrors `QR_URL_TEMPLATE`.
pub const QR_URL_TEMPLATE: &str =
    "https://q.qq.com/qqbot/openclaw/connect.html?task_id={task_id}&_wv=2&source=hermes";

/// Build QR connect URL for a given task_id. Mirrors `QR_URL_TEMPLATE.format(task_id=task_id)`.
pub fn qr_url(task_id: &str) -> String {
    QR_URL_TEMPLATE.replace("{task_id}", task_id)
}

/// Private alias for traceability.
#[allow(dead_code)]
fn _qr_url(task_id: &str) -> String {
    qr_url(task_id)
}

// ---------------------------------------------------------------------------
// Timeouts & retry
// ---------------------------------------------------------------------------

/// Default API timeout (seconds). Mirrors `DEFAULT_API_TIMEOUT = 30.0`.
pub const DEFAULT_API_TIMEOUT: f64 = 30.0;

/// File upload timeout (seconds). Mirrors `FILE_UPLOAD_TIMEOUT = 120.0`.
pub const FILE_UPLOAD_TIMEOUT: f64 = 120.0;

/// Connect timeout (seconds). Mirrors `CONNECT_TIMEOUT_SECONDS = 20.0`.
pub const CONNECT_TIMEOUT_SECONDS: f64 = 20.0;

/// Reconnect backoff sequence (seconds). Mirrors `RECONNECT_BACKOFF = [2, 5, 10, 30, 60]`.
pub const RECONNECT_BACKOFF: [u64; 5] = [2, 5, 10, 30, 60];

/// Max reconnect attempts. Mirrors `MAX_RECONNECT_ATTEMPTS = 100`.
pub const MAX_RECONNECT_ATTEMPTS: u32 = 100;

/// Rate-limit delay (seconds). Mirrors `RATE_LIMIT_DELAY = 60`.
pub const RATE_LIMIT_DELAY: u64 = 60;

/// Quick-disconnect threshold (seconds). Mirrors `QUICK_DISCONNECT_THRESHOLD = 5.0`.
pub const QUICK_DISCONNECT_THRESHOLD: f64 = 5.0;

/// Max quick-disconnect count before backoff. Mirrors `MAX_QUICK_DISCONNECT_COUNT = 3`.
pub const MAX_QUICK_DISCONNECT_COUNT: u32 = 3;

/// Onboard poll interval (seconds). Mirrors `ONBOARD_POLL_INTERVAL = 2.0`.
pub const ONBOARD_POLL_INTERVAL: f64 = 2.0;

/// Onboard API timeout (seconds). Mirrors `ONBOARD_API_TIMEOUT = 10.0`.
pub const ONBOARD_API_TIMEOUT: f64 = 10.0;

// ---------------------------------------------------------------------------
// Message limits
// ---------------------------------------------------------------------------

/// Max message length. Mirrors `MAX_MESSAGE_LENGTH = 4000`.
pub const MAX_MESSAGE_LENGTH: usize = 4000;

/// Dedup window (seconds). Mirrors `DEDUP_WINDOW_SECONDS = 300`.
pub const DEDUP_WINDOW_SECONDS: u64 = 300;

/// Dedup max size (entries). Mirrors `DEDUP_MAX_SIZE = 1000`.
pub const DEDUP_MAX_SIZE: usize = 1000;

// ---------------------------------------------------------------------------
// QQ Bot message types
// ---------------------------------------------------------------------------

/// Text message type. Mirrors `MSG_TYPE_TEXT = 0`.
pub const MSG_TYPE_TEXT: i32 = 0;

/// Markdown message type. Mirrors `MSG_TYPE_MARKDOWN = 2`.
pub const MSG_TYPE_MARKDOWN: i32 = 2;

/// Media message type. Mirrors `MSG_TYPE_MEDIA = 7`.
pub const MSG_TYPE_MEDIA: i32 = 7;

/// Input-notify message type. Mirrors `MSG_TYPE_INPUT_NOTIFY = 6`.
pub const MSG_TYPE_INPUT_NOTIFY: i32 = 6;

// ---------------------------------------------------------------------------
// QQ Bot file media types
// ---------------------------------------------------------------------------

/// Image media type. Mirrors `MEDIA_TYPE_IMAGE = 1`.
pub const MEDIA_TYPE_IMAGE: i32 = 1;

/// Video media type. Mirrors `MEDIA_TYPE_VIDEO = 2`.
pub const MEDIA_TYPE_VIDEO: i32 = 2;

/// Voice media type. Mirrors `MEDIA_TYPE_VOICE = 3`.
pub const MEDIA_TYPE_VOICE: i32 = 3;

/// File media type. Mirrors `MEDIA_TYPE_FILE = 4`.
pub const MEDIA_TYPE_FILE: i32 = 4;
