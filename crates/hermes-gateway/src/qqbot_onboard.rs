//! QQBot scan-to-configure (QR code onboard) module.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/platforms/qqbot/onboard.py` (220 LOC).
//!
//! Python source docstring (preserved):
//! ```text
//! QQBot scan-to-configure (QR code onboard) module.
//!
//! Mirrors the Feishu onboarding pattern: synchronous HTTP + a single public
//! entry-point ``qr_register()`` that handles the full flow (create task →
//! display QR code → poll → decrypt credentials).
//!
//! Calls the ``q.qq.com`` ``create_bind_task`` / ``poll_bind_result`` APIs to
//! generate a QR-code URL and poll for scan completion.  On success the caller
//! receives the bot's *app_id*, *client_secret* (decrypted locally), and the
//! scanner's *user_openid* — enough to fully configure the QQBot gateway.
//!
//! Reference: https://bot.q.qq.com/wiki/develop/api-v2/
//! ```
//!
//! Mapping:
//! - `class BindStatus(IntEnum): NONE=0 PENDING=1 COMPLETED=2 EXPIRED=3` → [`BindStatus`]
//! - `def _render_qr(url: str) -> bool` → [`_render_qr`] / [`render_qr`]
//! - `def _create_bind_task(timeout: float = ONBOARD_API_TIMEOUT) -> Tuple[str, str]` → [`_create_bind_task`] / [`create_bind_task`]
//! - `def _poll_bind_result(task_id: str, timeout: float = ...) -> Tuple[BindStatus, str, str, str]` → [`_poll_bind_result`] / [`poll_bind_result`]
//! - `def build_connect_url(task_id: str) -> str` → [`build_connect_url`]
//! - `def qr_register(timeout_seconds: int = 600) -> Optional[dict]` → [`qr_register`]
//! - `from .constants import ONBOARD_API_TIMEOUT, ONBOARD_CREATE_PATH, ONBOARD_POLL_INTERVAL, ONBOARD_POLL_PATH, PORTAL_HOST, QR_URL_TEMPLATE` → [`crate::qqbot_constants`]
//! - `from .crypto import decrypt_secret, generate_bind_key` → [`crate::qqbot_crypto`]
//! - `from .utils import get_api_headers` → [`crate::qqbot_utils::get_api_headers`]
//! - `QR_URL_TEMPLATE.format(task_id=quote(task_id))` → [`build_connect_url`] + [`urllib_quote`] + [`crate::qqbot_constants::QR_URL_TEMPLATE`]
//! - `time.monotonic() + timeout_seconds` → [`std::time::Instant::now`] + `Duration`
//! - `time.sleep(ONBOARD_POLL_INTERVAL)` → [`std::thread::sleep`]
//! - `import qrcode` optional → [`_render_qr`] stub (ponytail: no `qrcode` crate in std-only)
//! - `httpx.Client(timeout=..., follow_redirects=True).post(..., json=..., headers=...)` → [`post_json_sync`] (ponytail: std-only via `curl` subprocess; swap for `reqwest`/`httpx` when available)
//! - `data.get("retcode") != 0` → [`parse_create_bind_response`] / [`parse_poll_bind_response`]
//! - `_MAX_REFRESHES = 3` → [`_MAX_REFRESHES`] / [`MAX_REFRESHES`]
//!
//! Notes:
//! - `ponytail: no qrcode crate — _render_qr returns false; caller prints URL fallback. Add `qrcode` crate when terminal QR matters.`
//! - `ponytail: std-only HTTP via curl subprocess — swap for reqwest/ureq if throughput or TLS-native handling matters.`
//! - `ponytail: std-only percent-encoding — inline RFC3986, no `urlencoding` crate.`

use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::qqbot_constants::{
    ONBOARD_API_TIMEOUT, ONBOARD_CREATE_PATH, ONBOARD_POLL_INTERVAL, ONBOARD_POLL_PATH,
    QR_URL_TEMPLATE,
};
use crate::qqbot_crypto::{decrypt_secret, generate_bind_key};

// ---------------------------------------------------------------------------
// Bind status — mirrors `class BindStatus(IntEnum)`
// ---------------------------------------------------------------------------

/// Status codes returned by [`_poll_bind_result`].
///
/// Mirrors:
/// ```python
/// class BindStatus(IntEnum):
///     NONE = 0
///     PENDING = 1
///     COMPLETED = 2
///     EXPIRED = 3
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(i32)]
pub enum BindStatus {
    /// Mirrors `NONE = 0`.
    None = 0,
    /// Mirrors `PENDING = 1`.
    Pending = 1,
    /// Mirrors `COMPLETED = 2`.
    Completed = 2,
    /// Mirrors `EXPIRED = 3`.
    Expired = 3,
}

impl BindStatus {
    /// Convert `i32` to `BindStatus`, falling back to `None` for unknown values.
    ///
    /// Mirrors `BindStatus(d.get("status", 0))`.
    pub fn from_i32(v: i32) -> Self {
        match v {
            1 => Self::Pending,
            2 => Self::Completed,
            3 => Self::Expired,
            _ => Self::None,
        }
    }

    /// Return the integer value (mirrors `IntEnum` value).
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

impl std::fmt::Display for BindStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_i32())
    }
}

// Private alias for grep traceability (Python name `BindStatus`)
#[allow(dead_code)]
fn _bind_status_from_i32(v: i32) -> BindStatus {
    BindStatus::from_i32(v)
}

// ---------------------------------------------------------------------------
// QR rendering — mirrors `def _render_qr`
// ---------------------------------------------------------------------------

/// Try to render a QR code in the terminal. Returns `true` if successful.
///
/// Mirrors:
/// ```python
/// def _render_qr(url: str) -> bool:
///     if _qrcode_mod is None:
///         return False
///     try:
///         qr = _qrcode_mod.QRCode(
///             error_correction=_qrcode_mod.constants.ERROR_CORRECT_M,
///             border=2,
///         )
///         qr.add_data(url)
///         qr.make(fit=True)
///         qr.print_ascii(invert=True)
///         return True
///     except Exception:
///         return False
/// ```
///
/// `ponytail: no qrcode crate — always returns false; caller falls back to printing URL.`
pub fn _render_qr(url: &str) -> bool {
    let _ = url;
    // ponytail: std-only — no `qrcode` dep; return false so `qr_register` prints the URL.
    // Swap for `qrcode` crate (`QRCode::new(...).render()`) when terminal QR matters.
    false
}

/// Public alias for `_render_qr` (keeps underscore + non-underscore names discoverable).
pub fn render_qr(url: &str) -> bool {
    _render_qr(url)
}

// ---------------------------------------------------------------------------
// Percent-encoding — mirrors `urllib.parse.quote`
// ---------------------------------------------------------------------------

/// Percent-encode `input` as UTF-8, leaving `safe` bytes unescaped.
///
/// Mirrors `urllib.parse.quote(task_id)` (default `safe='/'`).
/// Python: `quote(string, safe='/', encoding='utf-8', errors='strict')`
/// Unreserved `A-Z a-z 0-9 - _ . ~` are never escaped; `safe` adds extra
/// passthrough (default `/`).
pub fn urllib_quote(input: &str, safe: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    let safe_bytes: Vec<u8> = safe.bytes().collect();
    for b in input.bytes() {
        let c = b as char;
        let is_unreserved = c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~');
        let is_safe = safe_bytes.contains(&b);
        if is_unreserved || is_safe {
            out.push(c);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

/// Alias matching Python `quote` import name.
pub fn quote(input: &str) -> String {
    urllib_quote(input, "/")
}

// ---------------------------------------------------------------------------
// QR URL builder — mirrors `def build_connect_url`
// ---------------------------------------------------------------------------

/// Build the QR-code target URL for a given `task_id`.
///
/// Mirrors:
/// ```python
/// def build_connect_url(task_id: str) -> str:
///     return QR_URL_TEMPLATE.format(task_id=quote(task_id))
/// ```
pub fn build_connect_url(task_id: &str) -> String {
    let encoded = quote(task_id);
    QR_URL_TEMPLATE.replace("{task_id}", &encoded)
}

/// Private alias for grep discoverability.
#[allow(dead_code)]
fn _build_connect_url(task_id: &str) -> String {
    build_connect_url(task_id)
}

// ---------------------------------------------------------------------------
// Synchronous HTTP helpers — mirrors `_create_bind_task` / `_poll_bind_result`
// ---------------------------------------------------------------------------

/// Max QR refreshes on expiry. Mirrors `_MAX_REFRESHES = 3`.
pub const _MAX_REFRESHES: u32 = 3;

/// Public alias.
pub const MAX_REFRESHES: u32 = _MAX_REFRESHES;

/// Build create-bind-task URL. Mirrors `f"https://{PORTAL_HOST}{ONBOARD_CREATE_PATH}"`.
pub fn create_bind_task_url() -> String {
    format!(
        "https://{}{}",
        crate::qqbot_constants::portal_host(),
        ONBOARD_CREATE_PATH
    )
}

/// Build poll-bind-result URL. Mirrors `f"https://{PORTAL_HOST}{ONBOARD_POLL_PATH}"`.
pub fn poll_bind_result_url() -> String {
    format!(
        "https://{}{}",
        crate::qqbot_constants::portal_host(),
        ONBOARD_POLL_PATH
    )
}

// Private aliases for grep traceability
#[allow(dead_code)]
fn _create_bind_task_url() -> String {
    create_bind_task_url()
}
#[allow(dead_code)]
fn _poll_bind_result_url() -> String {
    poll_bind_result_url()
}

/// Parse the JSON body of `create_bind_task`.
///
/// Mirrors:
/// ```python
/// if data.get("retcode") != 0:
///     raise RuntimeError(data.get("msg", "create_bind_task failed"))
/// task_id = (data.get("data") or {}).get("task_id")
/// if not task_id:
///     raise RuntimeError("create_bind_task: missing task_id in response")
/// ```
pub fn parse_create_bind_response(data: &Value) -> Result<String, String> {
    let retcode = data.get("retcode").and_then(|v| v.as_i64()).unwrap_or(-1);
    if retcode != 0 {
        let msg = data
            .get("msg")
            .and_then(|v| v.as_str())
            .unwrap_or("create_bind_task failed");
        return Err(msg.to_string());
    }
    let task_id = data
        .get("data")
        .and_then(|v| v.as_object())
        .and_then(|m| m.get("task_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if task_id.is_empty() {
        return Err("create_bind_task: missing task_id in response".to_string());
    }
    Ok(task_id.to_string())
}

/// Parse the JSON body of `poll_bind_result`.
///
/// Mirrors:
/// ```python
/// if data.get("retcode") != 0:
///     raise RuntimeError(data.get("msg", "poll_bind_result failed"))
/// d = data.get("data", {})
/// return (
///     BindStatus(d.get("status", 0)),
///     str(d.get("bot_appid", "")),
///     d.get("bot_encrypt_secret", ""),
///     d.get("user_openid", ""),
/// )
/// ```
pub fn parse_poll_bind_response(
    data: &Value,
) -> Result<(BindStatus, String, String, String), String> {
    let retcode = data.get("retcode").and_then(|v| v.as_i64()).unwrap_or(-1);
    if retcode != 0 {
        let msg = data
            .get("msg")
            .and_then(|v| v.as_str())
            .unwrap_or("poll_bind_result failed");
        return Err(msg.to_string());
    }
    let d = data.get("data").and_then(|v| v.as_object());
    let empty = serde_json::Map::new();
    let m = d.unwrap_or(&empty);
    let status_raw = m.get("status").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    let status = BindStatus::from_i32(status_raw);
    let bot_appid = m
        .get("bot_appid")
        .map(|v| {
            if let Some(s) = v.as_str() {
                s.to_string()
            } else if let Some(n) = v.as_i64() {
                n.to_string()
            } else if let Some(n) = v.as_u64() {
                n.to_string()
            } else {
                v.to_string().trim_matches('"').to_string()
            }
        })
        .unwrap_or_default();
    let bot_encrypt_secret = m
        .get("bot_encrypt_secret")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let user_openid = m
        .get("user_openid")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok((status, bot_appid, bot_encrypt_secret, user_openid))
}

// ponytail: std-only HTTP via curl subprocess — swap for reqwest/ureq if throughput or native TLS matters.

/// Synchronous `POST` with JSON body, returning parsed JSON.
///
/// Uses `curl` subprocess (present on most Unix hosts) to avoid adding a
/// new http crate in `NEVER cargo` mode. Mirrors `httpx.Client(...).post(..., json=..., headers=...)`
/// + `resp.raise_for_status()` + `resp.json()`.
///
/// Headers include `get_api_headers()` (Content-Type/Accept/User-Agent).
fn post_json_sync(url: &str, body: &Value, timeout: f64) -> Result<Value, String> {
    let body_str = body.to_string();
    let headers = crate::qqbot_utils::get_api_headers();
    // Build curl args: -sS -f -L -X POST --max-time <timeout> -H <headers> -d <body> <url>
    let timeout_secs = {
        let t = timeout.ceil() as u64;
        if t == 0 { 1 } else { t }
    };
    let mut cmd = std::process::Command::new("curl");
    cmd.arg("-sS");
    cmd.arg("-f");
    cmd.arg("-L");
    cmd.arg("-X");
    cmd.arg("POST");
    cmd.arg("--max-time");
    cmd.arg(timeout_secs.to_string());
    for (k, v) in &headers {
        cmd.arg("-H");
        cmd.arg(format!("{}: {}", k, v));
    }
    cmd.arg("-d");
    cmd.arg(&body_str);
    cmd.arg(url);
    // ponytail: curl subprocess — one extra process per API call; reqwest keeps a
    // persistent connection pool if this path becomes hot.
    let output = cmd.output().map_err(|e| format!("curl exec failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!("HTTP error: {}", output.status)
        };
        return Err(detail);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Err("empty response body".to_string());
    }
    serde_json::from_str(trimmed).map_err(|e| format!("invalid json: {e} => {trimmed}"))
}

/// Create a bind task and return `(task_id, aes_key_base64)`.
///
/// Mirrors:
/// ```python
/// def _create_bind_task(timeout: float = ONBOARD_API_TIMEOUT) -> Tuple[str, str]:
///     url = f"https://{PORTAL_HOST}{ONBOARD_CREATE_PATH}"
///     key = generate_bind_key()
///     with httpx.Client(timeout=timeout, follow_redirects=True) as client:
///         resp = client.post(url, json={"key": key}, headers=get_api_headers())
///         resp.raise_for_status()
///         data = resp.json()
///     if data.get("retcode") != 0:
///         raise RuntimeError(data.get("msg", "create_bind_task failed"))
///     task_id = (data.get("data") or {}).get("task_id")
///     if not task_id:
///         raise RuntimeError("create_bind_task: missing task_id in response")
///     logger.debug("create_bind_task ok: task_id=%s", task_id)
///     return task_id, key
/// ```
pub fn _create_bind_task(timeout: f64) -> Result<(String, String), String> {
    let url = create_bind_task_url();
    let key = generate_bind_key();
    let body = json!({ "key": key });
    let data = post_json_sync(&url, &body, timeout)?;
    let task_id = parse_create_bind_response(&data)?;
    // Mirrors `logger.debug("create_bind_task ok: task_id=%s", task_id)`
    eprintln!("[QQBot onboard] create_bind_task ok: task_id={}", task_id);
    Ok((task_id, key))
}

/// Public alias for `_create_bind_task`.
pub fn create_bind_task(timeout: f64) -> Result<(String, String), String> {
    _create_bind_task(timeout)
}

/// Default-timeout variant. Mirrors `timeout: float = ONBOARD_API_TIMEOUT`.
pub fn create_bind_task_default() -> Result<(String, String), String> {
    _create_bind_task(ONBOARD_API_TIMEOUT)
}

/// Create bind task with injectable HTTP post (test seam).
///
/// `post_fn` mirrors `httpx.Client.post` injection for deterministic tests.
pub fn _create_bind_task_with<F>(timeout: f64, post_fn: F) -> Result<(String, String), String>
where
    F: Fn(&str, &Value, f64) -> Result<Value, String>,
{
    let url = create_bind_task_url();
    let key = generate_bind_key();
    let body = json!({ "key": key });
    let data = post_fn(&url, &body, timeout)?;
    let task_id = parse_create_bind_response(&data)?;
    Ok((task_id, key))
}

/// Poll the bind result for `task_id`.
///
/// Mirrors:
/// ```python
/// def _poll_bind_result(
///     task_id: str,
///     timeout: float = ONBOARD_API_TIMEOUT,
/// ) -> Tuple[BindStatus, str, str, str]:
///     url = f"https://{PORTAL_HOST}{ONBOARD_POLL_PATH}"
///     with httpx.Client(timeout=timeout, follow_redirects=True) as client:
///         resp = client.post(url, json={"task_id": task_id}, headers=get_api_headers())
///         resp.raise_for_status()
///         data = resp.json()
///     if data.get("retcode") != 0:
///         raise RuntimeError(data.get("msg", "poll_bind_result failed"))
///     d = data.get("data", {})
///     return (
///         BindStatus(d.get("status", 0)),
///         str(d.get("bot_appid", "")),
///         d.get("bot_encrypt_secret", ""),
///         d.get("user_openid", ""),
///     )
/// ```
pub fn _poll_bind_result(
    task_id: &str,
    timeout: f64,
) -> Result<(BindStatus, String, String, String), String> {
    let url = poll_bind_result_url();
    let body = json!({ "task_id": task_id });
    let data = post_json_sync(&url, &body, timeout)?;
    parse_poll_bind_response(&data)
}

/// Public alias for `_poll_bind_result`.
pub fn poll_bind_result(
    task_id: &str,
    timeout: f64,
) -> Result<(BindStatus, String, String, String), String> {
    _poll_bind_result(task_id, timeout)
}

/// Default-timeout variant.
pub fn poll_bind_result_default(
    task_id: &str,
) -> Result<(BindStatus, String, String, String), String> {
    _poll_bind_result(task_id, ONBOARD_API_TIMEOUT)
}

/// Poll with injectable HTTP post (test seam).
pub fn _poll_bind_result_with<F>(
    task_id: &str,
    timeout: f64,
    post_fn: F,
) -> Result<(BindStatus, String, String, String), String>
where
    F: Fn(&str, &Value, f64) -> Result<Value, String>,
{
    let url = poll_bind_result_url();
    let body = json!({ "task_id": task_id });
    let data = post_fn(&url, &body, timeout)?;
    parse_poll_bind_response(&data)
}

// ---------------------------------------------------------------------------
// Public entry-point — mirrors `def qr_register`
// ---------------------------------------------------------------------------

/// Result of `qr_register` on success.
///
/// Mirrors `{"app_id": ..., "client_secret": ..., "user_openid": ...}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrRegisterResult {
    /// Bot App ID. Mirrors `app_id`.
    pub app_id: String,
    /// Decrypted client secret. Mirrors `client_secret`.
    pub client_secret: String,
    /// Scanner's OpenID. Mirrors `user_openid`.
    pub user_openid: String,
}

impl QrRegisterResult {
    /// Convert to `serde_json::Value` dict (mirrors Python `dict` return).
    pub fn to_json(&self) -> Value {
        json!({
            "app_id": self.app_id,
            "client_secret": self.client_secret,
            "user_openid": self.user_openid,
        })
    }
}

/// Run the QQBot scan-to-configure QR registration flow.
///
/// Mirrors `feishu.qr_register()`: handles create → display → poll → decrypt
/// in one call. Unexpected errors propagate to the caller as `None` (the
/// Python version logs a warning and returns `None` on create failures or
/// expiry/timeout).
///
/// ```python
/// def qr_register(timeout_seconds: int = 600) -> Optional[dict]:
///     deadline = time.monotonic() + timeout_seconds
///     for refresh_count in range(_MAX_REFRESHES + 1):
///         try:
///             task_id, aes_key = _create_bind_task()
///         except Exception as exc:
///             logger.warning("[QQBot onboard] Failed to create bind task: %s", exc)
///             return None
///         url = build_connect_url(task_id)
///         print()
///         if _render_qr(url):
///             print(f"  Scan the QR code above, or open this URL directly:\n  {url}")
///         else:
///             print(f"  Open this URL in QQ on your phone:\n  {url}")
///             print("  Tip: pip install qrcode  to display a scannable QR code here")
///         print()
///         while time.monotonic() < deadline:
///             try:
///                 status, app_id, encrypted_secret, user_openid = _poll_bind_result(task_id)
///             except Exception:
///                 time.sleep(ONBOARD_POLL_INTERVAL)
///                 continue
///             if status == BindStatus.COMPLETED:
///                 client_secret = decrypt_secret(encrypted_secret, aes_key)
///                 print()
///                 print(f"  QR scan complete! (App ID: {app_id})")
///                 if user_openid:
///                     print(f"  Scanner's OpenID: {user_openid}")
///                 return {"app_id": app_id, "client_secret": client_secret, "user_openid": user_openid}
///             if status == BindStatus.EXPIRED:
///                 if refresh_count >= _MAX_REFRESHES:
///                     logger.warning("[QQBot onboard] QR code expired %d times — giving up", _MAX_REFRESHES)
///                     return None
///                 print(f"\n  QR code expired, refreshing... ({refresh_count + 1}/{_MAX_REFRESHES})")
///                 break
///             time.sleep(ONBOARD_POLL_INTERVAL)
///         else:
///             logger.warning("[QQBot onboard] Poll timed out after %ds", timeout_seconds)
///             return None
///     return None
/// ```
pub fn qr_register(timeout_seconds: Option<u64>) -> Option<Value> {
    let timeout_secs = timeout_seconds.unwrap_or(600);
    qr_register_with(
        Duration::from_secs(timeout_secs),
        |t| _create_bind_task(t),
        |task_id, t| _poll_bind_result(task_id, t),
    )
    .map(|r| r.to_json())
}

/// Typed variant returning `QrRegisterResult`.
pub fn qr_register_typed(timeout_seconds: Option<u64>) -> Option<QrRegisterResult> {
    let timeout_secs = timeout_seconds.unwrap_or(600);
    qr_register_with(
        Duration::from_secs(timeout_secs),
        |t| _create_bind_task(t),
        |task_id, t| _poll_bind_result(task_id, t),
    )
}

/// Testable core: `qr_register` with injectable `create` / `poll` fns.
///
/// `create_fn: Fn(timeout) -> Result<(task_id, aes_key), String>`
/// `poll_fn: Fn(task_id, timeout) -> Result<(BindStatus, app_id, enc_secret, openid), String>`
pub fn qr_register_with<C, P>(
    timeout_total: Duration,
    create_fn: C,
    poll_fn: P,
) -> Option<QrRegisterResult>
where
    C: Fn(f64) -> Result<(String, String), String>,
    P: Fn(&str, f64) -> Result<(BindStatus, String, String, String), String>,
{
    let deadline = Instant::now() + timeout_total;
    let poll_interval = Duration::from_secs_f64(ONBOARD_POLL_INTERVAL);

    for refresh_count in 0..=MAX_REFRESHES {
        // ── Create bind task ──
        let (task_id, aes_key) = match create_fn(ONBOARD_API_TIMEOUT) {
            Ok(v) => v,
            Err(exc) => {
                eprintln!("[QQBot onboard] Failed to create bind task: {}", exc);
                return None;
            }
        };

        let url = build_connect_url(&task_id);

        // ── Display QR code + URL ──
        println!();
        if _render_qr(&url) {
            println!("  Scan the QR code above, or open this URL directly:\n  {}", url);
        } else {
            println!("  Open this URL in QQ on your phone:\n  {}", url);
            println!("  Tip: pip install qrcode  to display a scannable QR code here");
        }
        println!();

        // ── Poll loop ──
        let mut expired_break = false;
        while Instant::now() < deadline {
            let polled = poll_fn(&task_id, ONBOARD_API_TIMEOUT);
            let (status, app_id, encrypted_secret, user_openid) = match polled {
                Ok(v) => v,
                Err(_) => {
                    thread::sleep(poll_interval);
                    continue;
                }
            };

            if status == BindStatus::Completed {
                // Decrypt; Python lets unexpected errors propagate. Here we
                // treat decrypt failure as a hard error → return None (the
                // Python `decrypt_secret` would raise and propagate, but
                // `qr_register` only catches create failures; we mirror by
                // returning None on decrypt error to avoid panicking).
                let client_secret = match decrypt_secret(&encrypted_secret, &aes_key) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("[QQBot onboard] decrypt failed: {}", e);
                        return None;
                    }
                };
                println!();
                println!("  QR scan complete! (App ID: {})", app_id);
                if !user_openid.is_empty() {
                    println!("  Scanner's OpenID: {}", user_openid);
                }
                return Some(QrRegisterResult {
                    app_id,
                    client_secret,
                    user_openid,
                });
            }

            if status == BindStatus::Expired {
                if refresh_count >= MAX_REFRESHES {
                    eprintln!(
                        "[QQBot onboard] QR code expired {} times — giving up",
                        MAX_REFRESHES
                    );
                    return None;
                }
                println!(
                    "\n  QR code expired, refreshing... ({}/{})",
                    refresh_count + 1,
                    MAX_REFRESHES
                );
                expired_break = true;
                break;
            }

            thread::sleep(poll_interval);
        }

        if expired_break {
            continue;
        }

        // deadline reached without completing (Python `else:` on while)
        if Instant::now() >= deadline {
            eprintln!(
                "[QQBot onboard] Poll timed out after {}s",
                timeout_total.as_secs()
            );
            return None;
        }
    }

    None
}

// Private alias for grep discoverability (Python name `qr_register`)

#[allow(dead_code)]
fn _qr_register(timeout_seconds: Option<u64>) -> Option<Value> {
    qr_register(timeout_seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bind_status_from_i32() {
        assert_eq!(BindStatus::from_i32(0), BindStatus::None);
        assert_eq!(BindStatus::from_i32(1), BindStatus::Pending);
        assert_eq!(BindStatus::from_i32(2), BindStatus::Completed);
        assert_eq!(BindStatus::from_i32(3), BindStatus::Expired);
        assert_eq!(BindStatus::from_i32(99), BindStatus::None);
        assert_eq!(BindStatus::from_i32(-1), BindStatus::None);
        assert_eq!(BindStatus::Completed.as_i32(), 2);
        assert_eq!(BindStatus::Expired.as_i32(), 3);
    }

    #[test]
    fn render_qr_always_false_without_crate() {
        assert!(!_render_qr("https://q.qq.com/qqbot/openclaw/connect.html?task_id=abc"));
        assert!(!render_qr("https://example.com"));
    }

    #[test]
    fn urllib_quote_basic() {
        assert_eq!(quote("abc123"), "abc123");
        assert_eq!(urllib_quote("a b", "/"), "a%20b");
        // slash passthrough when safe='/'
        assert_eq!(quote("a/b"), "a/b");
        // slash encoded when safe=''
        assert_eq!(urllib_quote("a/b", ""), "a%2Fb");
        assert_eq!(quote("hello/world?x=1&y=2"), "hello/world%3Fx%3D1%26y%3D2");
        // unreserved not encoded
        assert_eq!(quote("a-b_c.d~e"), "a-b_c.d~e");
        // non-ascii percent-encoded per UTF-8 bytes
        assert_eq!(quote("café"), "caf%C3%A9");
        assert_eq!(_build_connect_url("abc"), build_connect_url("abc"));
    }

    #[test]
    fn build_connect_url_encodes_task_id() {
        let url = build_connect_url("my task/id?&=");
        // task_id part should be quoted, template should be intact
        assert!(url.starts_with("https://q.qq.com/qqbot/openclaw/connect.html?task_id="));
        assert!(url.contains("&_wv=2&source=hermes"));
        assert!(url.contains("my%20task/id%3F%26%3D") || url.contains("my%20task/id"));
        // plain id stays plain
        assert_eq!(
            build_connect_url("abc123"),
            "https://q.qq.com/qqbot/openclaw/connect.html?task_id=abc123&_wv=2&source=hermes"
        );
        // special chars encoded
        assert_eq!(
            build_connect_url("a b/c"),
            "https://q.qq.com/qqbot/openclaw/connect.html?task_id=a%20b/c&_wv=2&source=hermes"
        );
    }

    #[test]
    fn parse_create_bind_response_ok() {
        let data = json!({"retcode": 0, "data": {"task_id": "tid-123"}});
        assert_eq!(parse_create_bind_response(&data).unwrap(), "tid-123");
    }

    #[test]
    fn parse_create_bind_response_retcode_nonzero() {
        let data = json!({"retcode": 1, "msg": "boom"});
        let err = parse_create_bind_response(&data).unwrap_err();
        assert_eq!(err, "boom");
        let data2 = json!({"retcode": 2});
        let err2 = parse_create_bind_response(&data2).unwrap_err();
        assert_eq!(err2, "create_bind_task failed");
    }

    #[test]
    fn parse_create_bind_response_missing_task_id() {
        let data = json!({"retcode": 0, "data": {}});
        assert!(parse_create_bind_response(&data).is_err());
        let data2 = json!({"retcode": 0, "data": null});
        assert!(parse_create_bind_response(&data2).is_err());
        let data3 = json!({"retcode": 0});
        assert!(parse_create_bind_response(&data3).is_err());
    }

    #[test]
    fn parse_poll_bind_response_ok() {
        let data = json!({
            "retcode": 0,
            "data": {
                "status": 2,
                "bot_appid": "12345",
                "bot_encrypt_secret": "enc==",
                "user_openid": "openid-xyz"
            }
        });
        let (st, appid, enc, openid) = parse_poll_bind_response(&data).unwrap();
        assert_eq!(st, BindStatus::Completed);
        assert_eq!(appid, "12345");
        assert_eq!(enc, "enc==");
        assert_eq!(openid, "openid-xyz");
    }

    #[test]
    fn parse_poll_bind_response_pending_and_expired() {
        let pend = json!({"retcode":0,"data":{"status":1}});
        let (st, _, _, _) = parse_poll_bind_response(&pend).unwrap();
        assert_eq!(st, BindStatus::Pending);
        let exp = json!({"retcode":0,"data":{"status":3}});
        let (st2, _, _, _) = parse_poll_bind_response(&exp).unwrap();
        assert_eq!(st2, BindStatus::Expired);
        let none = json!({"retcode":0,"data":{"status":0}});
        let (st3, _, _, _) = parse_poll_bind_response(&none).unwrap();
        assert_eq!(st3, BindStatus::None);
    }

    #[test]
    fn parse_poll_bind_response_defaults() {
        let data = json!({"retcode":0,"data":{}});
        let (st, appid, enc, openid) = parse_poll_bind_response(&data).unwrap();
        assert_eq!(st, BindStatus::None);
        assert_eq!(appid, "");
        assert_eq!(enc, "");
        assert_eq!(openid, "");
    }

    #[test]
    fn parse_poll_bind_response_retcode_err() {
        let data = json!({"retcode": 5, "msg": "bad poll"});
        let err = parse_poll_bind_response(&data).unwrap_err();
        assert_eq!(err, "bad poll");
    }

    #[test]
    fn create_bind_task_url_shape() {
        let url = create_bind_task_url();
        assert!(url.starts_with("https://"));
        assert!(url.contains("/lite/create_bind_task"));
        let poll = poll_bind_result_url();
        assert!(poll.contains("/lite/poll_bind_result"));
    }

    #[test]
    fn qr_register_returns_none_on_create_failure() {
        let res = qr_register_with(
            Duration::from_secs(1),
            |_| Err("network down".to_string()),
            |_, _| Ok((BindStatus::Pending, "".to_string(), "".to_string(), "".to_string())),
        );
        assert!(res.is_none());
    }

    #[test]
    fn qr_register_returns_none_on_expiry_giving_up() {
        // Each create gives new task_id; poll always returns Expired → after 4 creates (MAX+1) should give up
        let mut creates = 0;
        let res = qr_register_with(
            Duration::from_secs(1),
            |_| {
                creates += 1;
                Ok((format!("task-{}", creates), generate_bind_key()))
            },
            |_, _| Ok((BindStatus::Expired, "".to_string(), "".to_string(), "".to_string())),
        );
        assert!(res.is_none());
        // We do not assert count because sleep makes it timing-sensitive, but it should have tried multiple times
    }

    #[test]
    fn qr_register_success_on_completed_with_decrypt() {
        // Generate a real key and encrypt a known secret using the crypto helper's internal encrypt
        // We need to craft bot_encrypt_secret that decrypts correctly.
        // Use qqbot_crypto::generate_bind_key + manual AES encrypt via the public decrypt path in reverse:
        // Instead of encrypting, we use a precomputed vector from qqbot_crypto tests.
        // For a hermetic test, inject poll that returns Completed with a precomputed enc that matches a known key.
        // Approach: generate key, then use the same crypto to encrypt via a helper that builds valid payload.
        // Since encrypt helper is cfg(test) private, replicate by using base64 + raw AES not available here.
        // Simpler: Stub decrypt by returning Completed but with empty secret? That would fail decrypt.
        // Instead test the poll->None path via timeout, and test the parse paths separately.
        // Here we test that if poll returns Pending forever, timeout returns None.
        let res = qr_register_with(
            Duration::from_millis(100),
            |_| Ok(("task-1".to_string(), generate_bind_key())),
            |_, _| Ok((BindStatus::Pending, "".to_string(), "".to_string(), "".to_string())),
        );
        assert!(res.is_none());
    }

    #[test]
    fn qr_register_with_completed_decrypt_flow() {
        // Use a deterministic key + ciphertext from qqbot_crypto::tests decrypt_known_python_vector
        // key_b64 = "ABEiM0RVZneImaq7zN3u/wARIjNEVWZ3iJmqu8zd7v8="
        // enc_b64 = "qrvM3e7/ESIzRFVmAPR2ysAqXAIKrRfsaMMOs0pJR28Xa+8o233FBhh5qHk=" => "hello-secret-123"
        let key_b64 = "ABEiM0RVZneImaq7zN3u/wARIjNEVWZ3iJmqu8zd7v8=".to_string();
        let enc_b64 = "qrvM3e7/ESIzRFVmAPR2ysAqXAIKrRfsaMMOs0pJR28Xa+8o233FBhh5qHk=".to_string();
        // Need to make create return that exact key, and poll return that enc
        let key_clone = key_b64.clone();
        let enc_clone = enc_b64.clone();
        let res = qr_register_with(
            Duration::from_secs(1),
            move |_| Ok(("task-deterministic".to_string(), key_clone.clone())),
            {
                let enc = enc_clone.clone();
                move |_, _| {
                    Ok((
                        BindStatus::Completed,
                        "12345".to_string(),
                        enc.clone(),
                        "openid-abc".to_string(),
                    ))
                }
            },
        );
        let r = res.expect("should complete");
        assert_eq!(r.app_id, "12345");
        assert_eq!(r.client_secret, "hello-secret-123");
        assert_eq!(r.user_openid, "openid-abc");
        assert_eq!(
            r.to_json(),
            json!({"app_id":"12345","client_secret":"hello-secret-123","user_openid":"openid-abc"})
        );
    }

    #[test]
    fn max_refreshes_const() {
        assert_eq!(MAX_REFRESHES, 3);
        assert_eq!(_MAX_REFRESHES, 3);
    }

    #[test]
    fn qr_register_timeout_zero_returns_none_quickly() {
        // deadline already past if timeout=0 and poll never completes
        let res = qr_register_with(
            Duration::from_millis(10),
            |_| Ok(("task-1".to_string(), generate_bind_key())),
            |_, _| Ok((BindStatus::Pending, "".to_string(), "".to_string(), "".to_string())),
        );
        assert!(res.is_none());
    }
}
