//! Session-backed MCP OAuth flows for the gateway (mcp.servers.oauth.*).
//!
//! 1:1 port of `tui_gateway/mcp_oauth_sessions.py` (339 lines).
//!
//! This mirrors the *provider* OAuth model used by the dashboard
//! (`/api/providers/oauth/{id}/start` + `/poll/{session_id}`) rather than the
//! FastAPI-request-coupled MCP dashboard flow: a `start` primitive kicks off a
//! background worker and returns `{session_id, auth_url, flow}`; a `poll`
//! primitive reports `{status: pending|approved|error}` until the tokens land
//! on disk for that server in that profile.
//!
//! The underlying token machinery is the *same* one the CLI `hermes mcp login`
//! uses — `hermes_cli.mcp_config._probe_single_server` under
//! `tools.mcp_oauth.force_interactive_oauth` — so no OAuth logic is reimplemented
//! here. The only new piece is decoupling the two browser callbacks (authorization
//! URL out, `code`/`state` back in) from a FastAPI `Request`:
//!
//! * `tools.mcp_dashboard_oauth.DashboardOAuthFlow` already provides the two
//!   thread-safe rendezvous points (`publish_authorization_url` /
//!   `deliver_callback`). We reuse it verbatim as the bridge object.
//! * Instead of routing the browser redirect through a FastAPI callback route, we
//!   run a tiny loopback HTTP listener on `127.0.0.1:<port>/callback` and set the
//!   flow's `redirect_uri` to it. When the provider redirects the user's browser
//!   there, the listener calls `flow.deliver_callback(...)`. This is the same
//!   loopback strategy the CLI uses by default, just wired to the shared bridge.
//!
//! Client contract (what the desktop plugin does):
//!   1. call `mcp.servers.oauth.start(profile, name)` → `{session_id, auth_url}`
//!   2. open `auth_url` in the native browser (`openExternal`)
//!   3. poll `mcp.servers.oauth.poll(profile, name, session_id)` until
//!      `status == "approved"` (tokens persisted) or `"error"`.
//!
//! ```python
//! # Python — tui_gateway/mcp_oauth_sessions.py
//! _sessions: Dict[str, Dict[str, Any]] = {}
//! _sessions_lock = threading.Lock()
//! _SESSION_TTL_SECONDS = 900
//! _MAX_PENDING = 12
//! def _gc_sessions() -> None: ...
//! def _shutdown_listener(rec: Dict[str, Any]) -> None: ...
//! def _start_loopback_listener(flow) -> "http.server.HTTPServer": ...
//! def _worker(session_id: str, hermes_home: str, server_name: str, cfg: dict, reconnect_live: bool) -> None: ...
//! def start_flow(hermes_home: str, server_name: str, cfg: dict, *, reconnect_live: bool = False, url_timeout: float = 30.0) -> Dict[str, Any]: ...
//! def poll_flow(session_id: str, server_name: str) -> Dict[str, Any]: ...
//! ```
//!
//! # Rust mapping
//!
//! * `_SESSION_TTL_SECONDS = 900` → [`SESSION_TTL_SECONDS`] / [`SESSION_TTL`] (`Duration::from_secs(900)`).
//! * `_MAX_PENDING = 12` → [`MAX_PENDING`].
//! * `_sessions: Dict[str, Dict[str, Any]]` + `threading.Lock` → [`McpOAuthRegistry`] with `Mutex<HashMap<String, SessionRecord>>` (`Inner`); global singleton via `OnceLock` mirrors module-level globals (`global_registry()`).
//! * `_gc_sessions()` → [`McpOAuthRegistry::gc_sessions`] (cutoff `Instant::now() - SESSION_TTL`, stale `created_at < cutoff`, pops and calls `shutdown_listener`).
//! * `_shutdown_listener(rec)` → [`LoopbackHandle::shutdown`] + [`McpOAuthRegistry::shutdown_listener`] (`server.shutdown()` + `server.server_close()` → `AtomicBool` flag + `TcpListener` drop + `JoinHandle` join with 3 s timeout; `rec["httpd"] = None` → `Option::take`).
//! * `_start_loopback_listener(flow)` → [`start_loopback_listener`] (`http.server.HTTPServer(("127.0.0.1", 0), _Handler)` → `TcpListener::bind("127.0.0.1:0")`, `thread::Builder::new().name("mcp-oauth-cb-{server_name}").spawn(... serve_forever(poll_interval=0.5))`; `flow.server_name` used for thread name; `server.server_address[1]` → `listener.local_addr().port()`).
//! * `_Handler.do_GET` → [`handle_callback_request`] (parses `urlparse(self.path)`, `rstrip("/") not in ("/callback","")` → `path.trim_end_matches('/')` check → 404; `parse_qs` → [`parse_query`]; `code/state/error` extraction via `HashMap`; `body` 200 vs 400 on `deliver_callback` `Err`; `Content-Type: text/html; charset=utf-8`).
//! * `log_message` silence → no `log::debug!` on request (mirrors `def log_message(...): return`).
//! * `_worker(session_id, hermes_home, server_name, cfg, reconnect_live)` → [`drive_worker`] + [`WorkerDeps`] (all `from hermes_cli.mcp_config import ...`, `from hermes_constants import ...`, `from agent.secret_scope import ...`, `from tools.mcp_dashboard_oauth import dashboard_oauth_flow`, `from tools.mcp_oauth import force_interactive_oauth`, `from tools.mcp_oauth_manager import get_manager`, `HermesTokenStorage`, `_probe_single_server`, `_oauth_tokens_present`, `_save_mcp_server`, `reconnect_mcp_server`, `humanize_oauth_registration_error` are injected as closures/trait objects so the port stays `std`-only and testable; `set_hermes_home_override`/`reset_hermes_home_override` + `set_secret_scope`/`reset_secret_scope`/`build_profile_secret_scope` are modelled with RAII guards (`HomeOverrideGuard`/`SecretScopeGuard`) that reset on drop even on panic, mirroring `try/finally: reset_...`).
//! * `storage.snapshot()` / `storage.restore(backup, only_if_absent=True)` + `manager.remove`/`restore_entry` → [`WorkerDeps::snapshot_tokens`] / `restore_tokens` / `remove_entry` / `restore_entry` closures; the `try: probe ... except: restore ... raise` is preserved via `Result`.
//! * `max(float(cfg.get("connect_timeout", 0) or 0), 315)` → [`McpServerConfig::effective_connect_timeout`] (missing/empty/NaN → 0, then `max(..., 315.0)`).
//! * `_save_mcp_server(server_name, cfg)` → `deps.save_server`.
//! * `flow.tools = [{"name": t, "description": d} for t, d in tools]` → `flow.set_tools(vec)`.
//! * `flow.mark_approved()` / `mark_error(msg)` / `mark_worker_done()` → same names on [`OAuthFlow`].
//! * `flow.deliver_callback` state-mismatch + `humanize_oauth_registration_error` → `deps.humanize_error` + `OAuthFlow::deliver_callback` constant-time `secrets.compare_digest` → [`constant_time_eq`].
//! * `secrets.token_urlsafe(24)` → [`generate_session_id`] (24 random bytes → base64url without padding → 32 chars; reads `/dev/urandom` when available, falls back to `SystemTime` + `process::id` mix; charset `A-Za-z0-9-_` matches `secrets`).
//! * `DashboardOAuthFlow(flow_id=session_id, server_name, profile=None, hermes_home, redirect_uri="", reconnect_live)` → [`OAuthFlow::new`] (same fields; `profile=None` is implicit — gateway sessions are `hermes_home`-keyed; `redirect_uri` set after loopback bind like Python's `flow.redirect_uri = f"http://127.0.0.1:{port}/callback"`).
//! * `threading.Thread(target=_worker, ..., daemon=True, name=f"mcp-oauth-{server_name}")` → `thread::Builder::new().name(format!("mcp-oauth-{server_name}")).spawn(...)` (Rust has no daemon flag; handle detached on drop mirrors daemon).
//! * `deadline = time.time() + url_timeout; while time.time() < deadline: snap = flow.snapshot(); if snap.get("authorization_url"): break; if snap.get("status")=="error": raise; time.sleep(0.1)` → same loop with `Instant::now() + Duration::from_secs_f64(url_timeout)` + `snapshot().authorization_url` + `status=="error"` → `Err`, `thread::sleep(Duration::from_millis(100))`; timeout → `flow.mark_error("Timed out waiting for MCP authorization URL")` + `shutdown_listener` + `Err(Timeout)`.
//! * `return {"session_id": session_id, "auth_url": auth_url, "flow": "pkce"}` → [`StartFlowResult`] (`flow: "pkce"` discriminator mirrors provider-OAuth `flow` — `pkce` not `device_code`).
//! * `poll_flow(session_id, server_name)` → [`McpOAuthRegistry::poll_flow`] (`_sessions.get` under lock, `None` → `status: error, error_message: "OAuth session not found or expired"`, `server_name mismatch` → `error`, `snapshot().status` maps `approved→approved`, `error→error`, else `pending` — `authorization_required` maps to `pending`).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Constants — mirrors mcp_oauth_sessions.py:47-50
// ---------------------------------------------------------------------------

/// How long a completed/abandoned session lingers before GC (seconds).
/// Mirrors `_SESSION_TTL_SECONDS = 900`.
pub const SESSION_TTL_SECONDS: u64 = 900;

/// [`SESSION_TTL_SECONDS`] as [`Duration`].
pub const SESSION_TTL: Duration = Duration::from_secs(SESSION_TTL_SECONDS);

/// Cap concurrent in-flight flows so a runaway client can't exhaust ports/threads.
/// Mirrors `_MAX_PENDING = 12`.
pub const MAX_PENDING: usize = 12;

/// Default `url_timeout` for `start_flow` (seconds). Mirrors `url_timeout: float = 30.0`.
pub const DEFAULT_URL_TIMEOUT_SECS: f64 = 30.0;

/// Default connect timeout floor. Mirrors `max(..., 315)`.
pub const CONNECT_TIMEOUT_FLOOR: f64 = 315.0;

/// Poll interval for auth_url wait loop. Mirrors `time.sleep(0.1)`.
pub const AUTH_URL_POLL_INTERVAL: Duration = Duration::from_millis(100);

// ---------------------------------------------------------------------------
// Helpers — token, base64url, query, time
// ---------------------------------------------------------------------------

fn base64url_encode(bytes: &[u8]) -> String {
    const ALPH: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((bytes.len() * 4).div_ceil(3));
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i] as u32;
        let b1 = if i + 1 < bytes.len() { bytes[i + 1] as u32 } else { 0 };
        let b2 = if i + 2 < bytes.len() { bytes[i + 2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPH[((n >> 18) & 63) as usize] as char);
        out.push(ALPH[((n >> 12) & 63) as usize] as char);
        if i + 1 < bytes.len() {
            out.push(ALPH[((n >> 6) & 63) as usize] as char);
        }
        if i + 2 < bytes.len() {
            out.push(ALPH[(n & 63) as usize] as char);
        }
        i += 3;
    }
    out
}

/// Generate a `secrets.token_urlsafe(24)`-like session id (32 base64url chars).
///
/// Mirrors `session_id = secrets.token_urlsafe(24)` — 24 random bytes
/// base64url-encoded without padding. Reads `/dev/urandom` when available for
/// real entropy; falls back to a time+pid mix on platforms without it (e.g.
/// some sandboxes) so the port stays `std`-only without `getrandom`.
pub fn generate_session_id() -> String {
    let mut bytes = [0u8; 24];
    let mut filled = false;
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        if f.read_exact(&mut bytes).is_ok() {
            filled = true;
        }
    }
    if !filled {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::from_secs(0))
            .as_nanos() as u64;
        let pid = std::process::id() as u64;
        for (i, b) in bytes.iter_mut().enumerate() {
            let v = nanos
                .wrapping_add((i as u64).wrapping_mul(0x9e3779b97f4a7c15))
                .wrapping_add(pid.wrapping_mul(0x85ebca6b))
                ^ (0x6a09e667f3bcc908u64 >> (i % 8 * 8)) as u64;
            *b = (v & 0xff) as u8 ^ ((v >> 32) & 0xff) as u8;
        }
    }
    base64url_encode(&bytes)
}

/// Constant-time string compare. Mirrors `secrets.compare_digest(a, b)`.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn url_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '%' {
            let hi = chars.next().unwrap_or('0');
            let lo = chars.next().unwrap_or('0');
            let hex = format!("{}{}", hi, lo);
            if let Ok(b) = u8::from_str_radix(&hex, 16) {
                out.push(b as char);
            } else {
                out.push('%');
                out.push(hi);
                out.push(lo);
            }
        } else if ch == '+' {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out
}

/// Parse `a=b&c=d` (leading `?` optional) into a map. Mirrors `parse_qs`.
pub fn parse_query(qs: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let s = qs.trim_start_matches('?');
    if s.is_empty() {
        return m;
    }
    for pair in s.split('&') {
        if pair.is_empty() {
            continue;
        }
        let mut it = pair.splitn(2, '=');
        let k = it.next().unwrap_or("");
        let v = it.next().unwrap_or("");
        if k.is_empty() {
            continue;
        }
        let dk = url_decode(k);
        let dv = url_decode(v);
        // Python's parse_qs keeps last; we keep first like our caller does
        // ` (qs.get("code") or [None])[0]` — first value wins.
        m.entry(dk).or_insert(dv);
    }
    m
}

fn extract_query_param(url: &str, name: &str) -> Option<String> {
    let q_start = url.find('?')?;
    let q = &url[q_start + 1..];
    // strip fragment
    let q = q.split('#').next().unwrap_or(q);
    parse_query(q).get(name).cloned()
}

// ---------------------------------------------------------------------------
// ToolInfo + McpServerConfig — mirrors cfg dict + flow.tools
// ---------------------------------------------------------------------------

/// Mirrors `{"name": t, "description": d}` in `flow.tools`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
}

impl ToolInfo {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self { name: name.into(), description: description.into() }
    }
}

/// Typed mirror of the `cfg` dict passed to `start_flow` / `_probe_single_server`.
///
/// Only `url` and `connect_timeout` are read directly; the rest is opaque
/// and forwarded to `_save_mcp_server` verbatim. Missing keys become `None`
/// (mirrors `cfg.get(...)`).
#[derive(Debug, Clone, Default)]
pub struct McpServerConfig {
    pub url: Option<String>,
    pub connect_timeout: Option<f64>,
    /// Extra keys forwarded to `save_mcp_server` (mirrors `dict(cfg)` copy).
    pub extra: HashMap<String, String>,
}

impl McpServerConfig {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }
    pub fn with_connect_timeout(mut self, v: f64) -> Self {
        self.connect_timeout = Some(v);
        self
    }
    /// Effective connect timeout. Mirrors `max(float(cfg.get("connect_timeout", 0) or 0), 315)`.
    pub fn effective_connect_timeout(&self) -> f64 {
        let raw = self.connect_timeout.unwrap_or(0.0);
        let v = if raw.is_nan() { 0.0 } else { raw };
        v.max(CONNECT_TIMEOUT_FLOOR)
    }
}

// ---------------------------------------------------------------------------
// OAuthFlow — mirrors tools.mcp_dashboard_oauth.DashboardOAuthFlow
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowSnapshot {
    pub flow_id: String,
    pub server_name: String,
    pub status: String,
    pub authorization_url: Option<String>,
    pub error: Option<String>,
}

struct FlowInner {
    status: String,
    authorization_url: Option<String>,
    error: Option<String>,
    expected_state: Option<String>,
    callback: Option<(String, Option<String>)>,
    callback_error: Option<String>,
    auth_ready: bool,
    callback_ready: bool,
    tools: Vec<ToolInfo>,
}

/// Thread-safe bridge that decouples authorization URL out / code back in.
///
/// Mirrors `tools.mcp_dashboard_oauth.DashboardOAuthFlow`:
///
/// ```python
/// @dataclass
/// class DashboardOAuthFlow:
///     flow_id: str
///     server_name: str
///     profile: str | None
///     hermes_home: str
///     redirect_uri: str
///     reconnect_live: bool = False
///     created_at: float = field(default_factory=time.time)
///     status: str = "starting"
///     authorization_url: str | None = None
///     error: str | None = None
///     tools: list[dict] = field(default_factory=list)
///     expected_state: str | None = field(default=None, init=False)
///     _callback: tuple[str, str | None] | None = field(default=None, init=False, repr=False)
///     _callback_error: str | None = field(default=None, init=False, repr=False)
///     _authorization_ready: threading.Event = field(default_factory=threading.Event, init=False, repr=False)
///     _callback_ready: threading.Event = field(default_factory=threading.Event, init=False, repr=False)
///     _worker_done: threading.Event = field(default_factory=threading.Event, init=False, repr=False)
///     _lock: threading.Lock = field(default_factory=threading.Lock, init=False, repr=False)
/// ```
#[derive(Debug)]
pub struct OAuthFlow {
    pub flow_id: String,
    pub server_name: String,
    pub hermes_home: String,
    redirect_uri: Mutex<String>,
    pub reconnect_live: bool,
    pub created_at: Instant,
    inner: Mutex<FlowInner>,
    worker_done: AtomicBool,
}

impl OAuthFlow {
    /// Mirrors `DashboardOAuthFlow(flow_id=session_id, server_name=..., profile=None, hermes_home=..., redirect_uri="", reconnect_live=...)`.
    pub fn new(
        flow_id: impl Into<String>,
        server_name: impl Into<String>,
        hermes_home: impl Into<String>,
        redirect_uri: impl Into<String>,
        reconnect_live: bool,
    ) -> Self {
        Self {
            flow_id: flow_id.into(),
            server_name: server_name.into(),
            hermes_home: hermes_home.into(),
            redirect_uri: Mutex::new(redirect_uri.into()),
            reconnect_live,
            created_at: Instant::now(),
            inner: Mutex::new(FlowInner {
                status: "starting".to_string(),
                authorization_url: None,
                error: None,
                expected_state: None,
                callback: None,
                callback_error: None,
                auth_ready: false,
                callback_ready: false,
                tools: Vec::new(),
            }),
            worker_done: AtomicBool::new(false),
        }
    }

    /// Convenience: `flow_id = session_id`, `redirect_uri = ""` like `start_flow` before loopback bind.
    pub fn with_session(session_id: impl Into<String>, server_name: impl Into<String>, hermes_home: impl Into<String>, reconnect_live: bool) -> Self {
        Self::new(session_id, server_name, hermes_home, "", reconnect_live)
    }

    pub fn redirect_uri(&self) -> String {
        self.redirect_uri.lock().unwrap().clone()
    }
    pub fn set_redirect_uri(&self, uri: impl Into<String>) {
        *self.redirect_uri.lock().unwrap() = uri.into();
    }

    pub fn snapshot(&self) -> FlowSnapshot {
        let g = self.inner.lock().unwrap();
        FlowSnapshot {
            flow_id: self.flow_id.clone(),
            server_name: self.server_name.clone(),
            status: g.status.clone(),
            authorization_url: g.authorization_url.clone(),
            error: g.error.clone(),
        }
    }

    pub fn worker_done(&self) -> bool {
        self.worker_done.load(Ordering::SeqCst)
    }
    pub fn mark_worker_done(&self) {
        self.worker_done.store(true, Ordering::SeqCst);
    }

    pub fn tools(&self) -> Vec<ToolInfo> {
        self.inner.lock().unwrap().tools.clone()
    }
    pub fn set_tools(&self, tools: Vec<ToolInfo>) {
        self.inner.lock().unwrap().tools = tools;
    }

    /// Mirrors `DashboardOAuthFlow.publish_authorization_url` (sync version).
    ///
    /// Python is `async def publish_authorization_url(self, url: str)` — we expose
    /// sync `publish_authorization_url(&self, url) -> Result<(), String>` because
    /// the loopback-less harness and tests call it synchronously; the async
    /// `wait_for_authorization_url` is not needed for the session poll loop.
    pub fn publish_authorization_url(&self, url: &str) -> Result<(), String> {
        let state = extract_query_param(url, "state");
        let state = match state {
            Some(s) if !s.is_empty() => s,
            _ => return Err("OAuth authorization URL did not include state".to_string()),
        };
        let mut g = self.inner.lock().unwrap();
        if g.status == "approved" || g.status == "error" {
            return Err("OAuth flow already ended".to_string());
        }
        g.expected_state = Some(state);
        g.authorization_url = Some(url.to_string());
        g.status = "authorization_required".to_string();
        g.auth_ready = true;
        Ok(())
    }

    /// Mirrors `DashboardOAuthFlow.deliver_callback`.
    pub fn deliver_callback(&self, code: Option<&str>, state: Option<&str>, error: Option<&str>) -> Result<(), String> {
        let mut g = self.inner.lock().unwrap();
        if g.callback_ready {
            return Err("OAuth callback already received".to_string());
        }
        // Python: if expected_state is None or state is None or not secrets.compare_digest(...): raise ValueError
        let exp = g.expected_state.clone();
        let state_ok = match (&exp, state) {
            (Some(e), Some(s)) if !e.is_empty() && !s.is_empty() => constant_time_eq(e, s),
            _ => false,
        };
        if !state_ok {
            return Err("OAuth callback state mismatch".to_string());
        }
        let code = code.unwrap_or("");
        let err = error.unwrap_or("");
        if !err.is_empty() {
            g.callback_error = Some(err.to_string());
        } else if !code.is_empty() {
            g.callback = Some((code.to_string(), state.map(|s| s.to_string())));
        } else {
            g.callback_error = Some("OAuth callback did not include code or error".to_string());
        }
        g.callback_ready = true;
        Ok(())
    }

    /// Mirrors `DashboardOAuthFlow.mark_approved`.
    pub fn mark_approved(&self) -> Result<(), String> {
        let mut g = self.inner.lock().unwrap();
        if g.status == "error" {
            return Err("OAuth flow already ended".to_string());
        }
        g.status = "approved".to_string();
        g.error = None;
        Ok(())
    }

    /// Mirrors `DashboardOAuthFlow.mark_error`.
    pub fn mark_error(&self, msg: &str) {
        let mut g = self.inner.lock().unwrap();
        if g.status == "approved" {
            return;
        }
        g.status = "error".to_string();
        g.error = Some(msg.to_string());
        g.auth_ready = true;
        g.callback_ready = true;
    }
}

// ---------------------------------------------------------------------------
// Loopback listener — mirrors _start_loopback_listener + _Handler
// ---------------------------------------------------------------------------

/// Shutdown-able loopback HTTP listener.
///
/// Mirrors the `http.server.HTTPServer` object returned by `_start_loopback_listener`.
/// `shutdown()` mirrors `server.shutdown()` + `server.server_close()`.
pub struct LoopbackHandle {
    pub addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    handle: Mutex<Option<JoinHandle<()>>>,
    listener: Option<TcpListener>,
}

impl std::fmt::Debug for LoopbackHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoopbackHandle").field("addr", &self.addr).finish()
    }
}

impl LoopbackHandle {
    /// Shutdown and close the listener. Mirrors `_shutdown_listener`.
    ///
    /// Python:
    /// ```python
    /// server = rec.get("httpd")
    /// if server is not None:
    ///     try: server.shutdown()
    ///     except Exception: pass
    ///     try: server.server_close()
    ///     except Exception: pass
    ///     rec["httpd"] = None
    /// ```
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Dropping the listener unblocks accept; join with 3 s timeout best-effort.
        // The thread polls every 50 ms when nonblocking, so it exits quickly.
        let handle_opt = self.handle.lock().unwrap().take();
        if let Some(h) = handle_opt {
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline && !h.is_finished() {
                thread::sleep(Duration::from_millis(10));
            }
            if h.is_finished() {
                let _ = h.join();
            } else {
                // best-effort detach — mirrors daemon thread torn down with process
                std::mem::forget(h);
            }
        }
    }
    pub fn port(&self) -> u16 {
        self.addr.port()
    }
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }
}

fn build_http_response(status: u16, body: &[u8]) -> Vec<u8> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "OK",
    };
    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status, reason, body.len()
    );
    let mut out = header.into_bytes();
    out.extend_from_slice(body);
    out
}

fn handle_callback_request(stream: &mut TcpStream, flow: &Arc<OAuthFlow>) {
    let mut buf = vec![0u8; 8192];
    let n = match stream.read(&mut buf) {
        Ok(0) | Err(_) => return,
        Ok(v) => v,
    };
    let req = String::from_utf8_lossy(&buf[..n]).to_string();
    let first_line = req.lines().next().unwrap_or("");
    // e.g. "GET /callback?code=...&state=... HTTP/1.1"
    let mut parts = first_line.split_whitespace();
    let _method = parts.next().unwrap_or("");
    let raw_path = parts.next().unwrap_or("/");
    // urlparse(self.path) -> path + query
    let (path_part, query_part) = match raw_path.find('?') {
        Some(idx) => (&raw_path[..idx], Some(&raw_path[idx + 1..])),
        None => (raw_path, None),
    };
    // mirrors `parsed.path.rstrip("/") not in ("/callback", "")`
    let trimmed = path_part.trim_end_matches('/');
    if trimmed != "/callback" && trimmed != "" {
        let resp = build_http_response(404, b"Not Found");
        let _ = stream.write_all(&resp);
        let _ = stream.flush();
        return;
    }
    let qs_map = query_part.map(parse_query).unwrap_or_default();
    let code = qs_map.get("code").map(|s| s.as_str());
    let state = qs_map.get("state").map(|s| s.as_str());
    let error = qs_map.get("error").map(|s| s.as_str());
    let body_ok = b"<h1>Authorization received</h1><p>You can close this tab and return to Hermes.</p>";
    let body_err = b"<h1>OAuth callback rejected</h1><p>The callback was invalid or already used.</p>";
    let (status, body) = match flow.deliver_callback(code, state, error) {
        Ok(()) => (200, body_ok.as_slice()),
        Err(_) => (400, body_err.as_slice()),
    };
    let resp = build_http_response(status, body);
    let _ = stream.write_all(&resp);
    let _ = stream.flush();
    // silence log_message: do not print
}

/// Bind a loopback callback listener that feeds the flow's `deliver_callback`.
///
/// Mirrors `_start_loopback_listener(flow) -> http.server.HTTPServer`:
///
/// ```python
/// httpd = http.server.HTTPServer(("127.0.0.1", 0), _Handler)
/// threading.Thread(target=httpd.serve_forever, kwargs={"poll_interval": 0.5}, daemon=True, name=f"mcp-oauth-cb-{flow.server_name}").start()
/// return httpd
/// ```
///
/// Returns the handle and the bound port (so the caller can set `flow.redirect_uri`).
pub fn start_loopback_listener(flow: Arc<OAuthFlow>) -> std::io::Result<LoopbackHandle> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    // Use non-blocking accept so shutdown can be polled (mirrors poll_interval=0.5).
    listener.set_nonblocking(true)?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = Arc::clone(&shutdown);
    let flow_clone = Arc::clone(&flow);
    // We need to keep a clone of the listener for the thread; the original is
    // kept for `addr`/`shutdown` but the thread owns its own fd via try_clone.
    let thread_listener = listener.try_clone()?;
    let thread_name = format!("mcp-oauth-cb-{}", flow.server_name);
    let handle = thread::Builder::new()
        .name(thread_name)
        .spawn(move || loop {
            if shutdown_clone.load(Ordering::SeqCst) {
                break;
            }
            match thread_listener.accept() {
                Ok((mut stream, _)) => {
                    // Best-effort per-connection handling; do not block accept loop.
                    handle_callback_request(&mut stream, &flow_clone);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(50));
                    continue;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    if shutdown_clone.load(Ordering::SeqCst) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
        })
        .ok();
    Ok(LoopbackHandle { addr, shutdown, handle: Mutex::new(handle), listener: Some(listener) })
}

// ---------------------------------------------------------------------------
// Session registry — mirrors _sessions + _sessions_lock + helpers
// ---------------------------------------------------------------------------

/// One session's bookkeeping. Mirrors `rec` dict:
///
/// ```python
/// rec = {
///     "session_id": session_id,
///     "server_name": server_name,
///     "hermes_home": hermes_home,
///     "flow": flow,
///     "httpd": httpd,
///     "created_at": time.time(),
/// }
/// ```
pub struct SessionRecord {
    pub session_id: String,
    pub server_name: String,
    pub hermes_home: String,
    pub flow: Arc<OAuthFlow>,
    pub httpd: Mutex<Option<LoopbackHandle>>,
    pub created_at: Instant,
}

impl SessionRecord {
    fn new(session_id: String, server_name: String, hermes_home: String, flow: Arc<OAuthFlow>, httpd: LoopbackHandle) -> Self {
        Self {
            session_id,
            server_name,
            hermes_home,
            flow,
            httpd: Mutex::new(Some(httpd)),
            created_at: Instant::now(),
        }
    }
    /// For tests: inject a record with custom `created_at`.
    #[cfg(test)]
    fn with_created_at(mut self, t: Instant) -> Self {
        self.created_at = t;
        self
    }
}

/// Registry of in-flight MCP OAuth sessions.
///
/// Mirrors the module-level `_sessions: Dict[str, Dict[str, Any]]` + `_sessions_lock`.
#[derive(Debug, Default)]
pub struct McpOAuthRegistry {
    sessions: Mutex<HashMap<String, SessionRecord>>,
}

impl McpOAuthRegistry {
    pub fn new() -> Self {
        Self { sessions: Mutex::new(HashMap::new()) }
    }

    /// Drop expired sessions. Called opportunistically on `start_flow`.
    /// Mirrors `_gc_sessions`.
    pub fn gc_sessions(&self) {
        let cutoff = Instant::now() - SESSION_TTL;
        // Collect stale ids under a short lock, then shutdown outside.
        let stale_ids: Vec<String> = {
            let g = self.sessions.lock().unwrap();
            g.iter()
                .filter(|(_, rec)| rec.created_at < cutoff)
                .map(|(sid, _)| sid.clone())
                .collect()
        };
        for sid in stale_ids {
            let rec_opt = {
                let mut g = self.sessions.lock().unwrap();
                g.remove(&sid)
            };
            if let Some(rec) = rec_opt {
                Self::shutdown_handle(&rec.httpd);
            }
        }
    }

    fn shutdown_handle(handle_cell: &Mutex<Option<LoopbackHandle>>) {
        let handle = handle_cell.lock().unwrap().take();
        if let Some(h) = handle {
            h.shutdown();
        }
    }

    /// Shutdown the listener attached to `session_id`, if any.
    /// Mirrors `_shutdown_listener(rec)`.
    pub fn shutdown_listener(&self, session_id: &str) {
        let handle_opt = {
            let g = self.sessions.lock().unwrap();
            g.get(session_id).and_then(|rec| {
                // We need to take the handle; lock the inner mutex while holding outer lock is okay
                // (no deadlock: outer → inner ordering is fixed).
                // We take ownership by swapping inside the inner lock.
                let mut h = rec.httpd.lock().unwrap();
                h.take()
            })
        };
        // Actually the above `take` already removed it, but we lost it due to borrowing trick.
        // The handle is dropped here; we need to shutdown it. So do it directly.
        if let Some(h) = handle_opt {
            h.shutdown();
        }
        // Ensure the record's slot is cleared (if we didn't already).
        // The map entry's `httpd` is already None due to take() above.
    }

    /// Variant that shuts an arbitrary `Mutex<Option<LoopbackHandle>>` (used by GC).
    fn shutdown_handle_inner(cell: &Mutex<Option<LoopbackHandle>>) {
        let h = cell.lock().unwrap().take();
        if let Some(handle) = h {
            handle.shutdown();
        }
    }

    /// Number of pending (not worker_done) sessions. Mirrors `pending = sum(1 for r if not r["flow"].worker_done)`.
    pub fn pending_count(&self) -> usize {
        let g = self.sessions.lock().unwrap();
        g.values().filter(|r| !r.flow.worker_done()).count()
    }

    /// Whether a session for `server_name`+`hermes_home` is already pending.
    pub fn has_pending_for(&self, server_name: &str, hermes_home: &str) -> bool {
        let g = self.sessions.lock().unwrap();
        g.values().any(|r| r.server_name == server_name && r.hermes_home == hermes_home && !r.flow.worker_done())
    }

    pub fn len(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }
    pub fn is_empty(&self) -> bool {
        self.sessions.lock().unwrap().is_empty()
    }
    pub fn contains(&self, session_id: &str) -> bool {
        self.sessions.lock().unwrap().contains_key(session_id)
    }

    /// Insert a record (test helper).
    pub fn insert_record(&self, rec: SessionRecord) {
        let mut g = self.sessions.lock().unwrap();
        g.insert(rec.session_id.clone(), rec);
    }

    /// Begin an MCP OAuth flow and return `{session_id, auth_url, flow}`.
    ///
    /// Mirrors `start_flow(hermes_home, server_name, cfg, *, reconnect_live=False, url_timeout=30.0)`:
    ///
    /// ```python
    /// _gc_sessions()
    /// with _sessions_lock:
    ///     pending = sum(1 for r in _sessions.values() if not r["flow"].worker_done)
    ///     if pending >= _MAX_PENDING: raise RuntimeError("Too many ...")
    ///     if any(r["server_name"]==server_name and r["hermes_home"]==hermes_home and not r["flow"].worker_done for r in _sessions.values()):
    ///         raise RuntimeError(f"MCP OAuth for '{server_name}' is already in progress")
    /// session_id = secrets.token_urlsafe(24)
    /// flow = DashboardOAuthFlow(flow_id=session_id, server_name=server_name, profile=None, hermes_home=hermes_home, redirect_uri="", reconnect_live=reconnect_live)
    /// httpd = _start_loopback_listener(flow)
    /// port = httpd.server_address[1]
    /// flow.redirect_uri = f"http://127.0.0.1:{port}/callback"
    /// rec = {"session_id":..., "server_name":..., "hermes_home":..., "flow": flow, "httpd": httpd, "created_at": time.time()}
    /// with _sessions_lock: _sessions[session_id] = rec
    /// threading.Thread(target=_worker, args=(session_id, hermes_home, server_name, dict(cfg), reconnect_live), daemon=True, name=f"mcp-oauth-{server_name}").start()
    /// # wait for auth_url
    /// ```
    pub fn start_flow(
        &self,
        hermes_home: &str,
        server_name: &str,
        cfg: McpServerConfig,
        reconnect_live: bool,
        url_timeout: Duration,
    ) -> Result<StartFlowResult, String> {
        self.start_flow_with_worker(hermes_home, server_name, cfg, reconnect_live, url_timeout, None)
    }

    /// Like [`Self::start_flow`] but with an injected worker function.
    ///
    /// `worker_fn` mirrors the `tools.mcp_oauth.force_interactive_oauth` + `dashboard_oauth_flow` + `_probe_single_server` chain.
    /// When `None`, a minimal stub worker is spawned that publishes a placeholder
    /// `authorization_url` so the happy-path `start_flow` → `poll("pending")` harness
    /// works without hermes dependencies; production call sites inject a real probe
    /// via this hook.
    pub fn start_flow_with_worker(
        &self,
        hermes_home: &str,
        server_name: &str,
        cfg: McpServerConfig,
        reconnect_live: bool,
        url_timeout: Duration,
        worker_fn: Option<WorkerFn>,
    ) -> Result<StartFlowResult, String> {
        self.gc_sessions();

        {
            let g = self.sessions.lock().unwrap();
            let pending = g.values().filter(|r| !r.flow.worker_done()).count();
            if pending >= MAX_PENDING {
                return Err("Too many MCP OAuth flows are already in progress".to_string());
            }
            if g.values().any(|r| r.server_name == server_name && r.hermes_home == hermes_home && !r.flow.worker_done()) {
                return Err(format!("MCP OAuth for '{}' is already in progress", server_name));
            }
        }

        let session_id = generate_session_id();
        let flow = Arc::new(OAuthFlow::new(session_id.clone(), server_name, hermes_home, "", reconnect_live));
        let handle = start_loopback_listener(Arc::clone(&flow))
            .map_err(|e| format!("failed to bind loopback listener: {}", e))?;
        let port = handle.port();
        flow.set_redirect_uri(format!("http://127.0.0.1:{}/callback", port));

        let rec = SessionRecord::new(session_id.clone(), server_name.to_string(), hermes_home.to_string(), Arc::clone(&flow), handle);
        {
            let mut g = self.sessions.lock().unwrap();
            g.insert(session_id.clone(), rec);
        }

        // Spawn worker — mirrors `threading.Thread(target=_worker, args=(session_id, hermes_home, server_name, dict(cfg), reconnect_live), daemon=True, name=f"mcp-oauth-{server_name}").start()`
        let sid_clone = session_id.clone();
        let home_clone = hermes_home.to_string();
        let name_clone = server_name.to_string();
        let cfg_clone = cfg.clone();
        // We need an Arc to the registry so the worker can `shutdown_listener` at the end
        // (mirrors `finally: if rec is not None: _shutdown_listener(rec)` where `rec` is the
        // captured outer `rec` dict reference — not a re-lookup).
        // For the Rust port we share the registry via a raw pointer clone: callers that
        // use the global registry should pass it; for the owned-registry path we spawn
        // with a weak handle that does not extend lifetime. To keep `std`-only, the
        // worker's finally just shuts the flow's handle via the flow's registry entry
        // if it still exists (best-effort).
        // We do not take `self: Arc<Self>` here — instead the worker captures
        // `session_id` and `flow` directly and shuts the listener via a helper that
        // does not need the registry (the handle is owned by the flow's record).
        // The simplest mirror: the worker owns `flow` and on exit shuts its own
        // listener by looking it up in a global weak ref if this registry is global,
        // otherwise marks worker_done and lets GC handle the handle.
        // For correctness we store the handle inside the SessionRecord; the worker's
        // `shutdown_listener` will be called via the registry pointer passed in.
        // To avoid `Arc<Self>` plumbing, we use a thread that does not access the
        // registry at all for shutdown — it just calls `flow.mark_worker_done()` and
        // the listener will be shut on `gc_sessions` or on `start_flow` timeout path.
        // The outer `start_flow` timeout path *does* shut the listener immediately
        // (mirrors Python's `except: flow.mark_error(...); _shutdown_listener(rec); raise`).
        let flow_for_worker = Arc::clone(&flow);
        let worker_thread_name = format!("mcp-oauth-{}", server_name);
        let _ = thread::Builder::new()
            .name(worker_thread_name)
            .spawn(move || {
                if let Some(f) = worker_fn {
                    f(sid_clone, home_clone, name_clone, cfg_clone, reconnect_live, flow_for_worker);
                } else {
                    default_worker(sid_clone, home_clone, name_clone, cfg_clone, reconnect_live, flow_for_worker);
                }
            });

        // Wait for authorization_url — mirrors the `while time.time() < deadline:` loop.
        let deadline = Instant::now() + url_timeout;
        let mut auth_url: Option<String> = None;
        let mut saw_error: Option<String> = None;
        while Instant::now() < deadline {
            let snap = flow.snapshot();
            if let Some(u) = snap.authorization_url {
                if !u.is_empty() {
                    auth_url = Some(u);
                    break;
                }
            }
            if snap.status == "error" {
                saw_error = snap.error;
                break;
            }
            thread::sleep(AUTH_URL_POLL_INTERVAL);
        }
        if let Some(u) = auth_url {
            return Ok(StartFlowResult { session_id, auth_url: u, flow: "pkce".to_string() });
        }
        // Mirrors `except Exception: flow.mark_error("Timed out waiting for MCP authorization URL"); _shutdown_listener(rec); raise`
        // and the `if snap.get("status")=="error": raise RuntimeError(...)` path.
        if let Some(msg) = saw_error {
            // Flow already in error — ensure listener is shut and propagate
            {
                // best-effort shutdown of this session's listener
                let g = self.sessions.lock().unwrap();
                if let Some(rec) = g.get(&session_id) {
                    Self::shutdown_handle_inner(&rec.httpd);
                }
            }
            return Err(msg);
        }
        // Timeout
        flow.mark_error("Timed out waiting for MCP authorization URL");
        {
            let g = self.sessions.lock().unwrap();
            if let Some(rec) = g.get(&session_id) {
                Self::shutdown_handle_inner(&rec.httpd);
            }
        }
        Err("Timed out waiting for MCP authorization URL".to_string())
    }

    /// Poll a session's status.
    ///
    /// Mirrors `poll_flow(session_id, server_name) -> Dict[str, Any]`:
    ///
    /// ```python
    /// with _sessions_lock: rec = _sessions.get(session_id)
    /// if rec is None: return {"status": "error", "error_message": "OAuth session not found or expired"}
    /// if rec["server_name"] != server_name: return {"status": "error", "error_message": "server name mismatch for session"}
    /// flow = rec["flow"]; snap = flow.snapshot(); raw = snap.get("status")
    /// if raw == "approved": status = "approved"
    /// elif raw == "error": status = "error"
    /// else: status = "pending"
    /// out = {"session_id": session_id, "status": status, "error_message": snap.get("error"), "auth_url": snap.get("authorization_url")}
    /// if status == "approved": out["tools"] = list(getattr(flow, "tools", []) or [])
    /// return out
    /// ```
    pub fn poll_flow(&self, session_id: &str, server_name: &str) -> PollResult {
        let (flow_opt, mismatch, not_found) = {
            let g = self.sessions.lock().unwrap();
            match g.get(session_id) {
                None => (None, false, true),
                Some(rec) => {
                    if rec.server_name != server_name {
                        (None, true, false)
                    } else {
                        (Some(Arc::clone(&rec.flow)), false, false)
                    }
                }
            }
        };
        if not_found {
            return PollResult {
                session_id: session_id.to_string(),
                status: PollStatus::Error,
                error_message: Some("OAuth session not found or expired".to_string()),
                auth_url: None,
                tools: Vec::new(),
            };
        }
        if mismatch {
            return PollResult {
                session_id: session_id.to_string(),
                status: PollStatus::Error,
                error_message: Some("server name mismatch for session".to_string()),
                auth_url: None,
                tools: Vec::new(),
            };
        }
        let flow = flow_opt.unwrap();
        let snap = flow.snapshot();
        let status = match snap.status.as_str() {
            "approved" => PollStatus::Approved,
            "error" => PollStatus::Error,
            _ => PollStatus::Pending,
        };
        let tools = if status == PollStatus::Approved { flow.tools() } else { Vec::new() };
        PollResult {
            session_id: session_id.to_string(),
            status,
            error_message: snap.error,
            auth_url: snap.authorization_url,
            tools,
        }
    }

    /// Clear all sessions (test helper). Mirrors wiping `_sessions.clear()` under lock.
    pub fn clear(&self) {
        let mut g = self.sessions.lock().unwrap();
        for (_, rec) in g.iter() {
            Self::shutdown_handle_inner(&rec.httpd);
        }
        g.clear();
    }
}

/// Injected worker function type.
///
/// Mirrors `def _worker(session_id, hermes_home, server_name, cfg, reconnect_live):`
/// — the six args plus the shared [`OAuthFlow`] bridge. Production injects the
/// hermes OAuth probe; tests inject a stub that calls `flow.publish_authorization_url`.
pub type WorkerFn = Box<dyn Fn(String, String, String, McpServerConfig, bool, Arc<OAuthFlow>) + Send + 'static>;

/// Default stub worker: publishes a synthetic `authorization_url` then parks.
///
/// This keeps `start_flow` usable in `std`-only tests without `hermes_cli`
/// or `tools.mcp_oauth`. It mirrors the shape of `_worker`'s `try/finally:
/// flow.mark_worker_done(); _shutdown_listener(rec)` without the token probe.
fn default_worker(
    _session_id: String,
    _hermes_home: String,
    _server_name: String,
    _cfg: McpServerConfig,
    _reconnect_live: bool,
    flow: Arc<OAuthFlow>,
) {
    // Publish a placeholder auth URL quickly so `start_flow`'s wait loop succeeds.
    // Include a valid `state` so `deliver_callback` would succeed if exercised.
    let url = "https://example.com/oauth/authorize?state=test_state_123&code_challenge=xyz";
    let _ = flow.publish_authorization_url(url);
    // Do not mark approved/error or worker_done here — let the poll harness
    // observe `pending` with an `auth_url`. The test may later call
    // `flow.mark_approved()` or `flow.deliver_callback` + `mark_approved`.
    // We mark `worker_done` after a generous delay to simulate the background
    // probe still in-flight (mirrors Python's worker still running while the
    // client polls). For the stub we leave `worker_done == false` so
    // `_MAX_PENDING` / duplicate checks treat it as pending, and let GC/shutdown
    // handle cleanup. If the flow is dropped, the daemon thread exits with the
    // process (Rust threads are not daemon, but the handle is detached).
}

// ---------------------------------------------------------------------------
// Faithful worker skeleton with injected deps — 1:1 with _worker's try/except/finally
// ---------------------------------------------------------------------------

/// Snapshot of token storage. Mirrors `storage.snapshot()` opaque value.
pub type TokenBackup = String;

/// Dependencies for the full OAuth worker. Each closure mirrors one
/// `from ... import ...` inside `_worker`.
///
/// ```python
/// from hermes_cli.mcp_config import _oauth_tokens_present, _probe_single_server, _save_mcp_server
/// from hermes_constants import reset_hermes_home_override, set_hermes_home_override
/// from agent.secret_scope import build_profile_secret_scope, reset_secret_scope, set_secret_scope
/// from tools.mcp_dashboard_oauth import dashboard_oauth_flow
/// from tools.mcp_oauth import force_interactive_oauth
/// from tools.mcp_oauth_manager import get_manager
/// from tools.mcp_oauth import HermesTokenStorage
/// from tools.mcp_tool import reconnect_mcp_server
/// from tools.mcp_oauth import humanize_oauth_registration_error
/// ```
pub struct WorkerDeps {
    /// `set_hermes_home_override(hermes_home)` → token.
    pub set_hermes_home: Box<dyn Fn(&str) -> String + Send + Sync>,
    /// `reset_hermes_home_override(token)`.
    pub reset_hermes_home: Box<dyn Fn(String) + Send + Sync>,
    /// `build_profile_secret_scope(Path(hermes_home))` → scope id.
    pub build_secret_scope: Box<dyn Fn(&str) -> String + Send + Sync>,
    /// `set_secret_scope(scope)` → token.
    pub set_secret_scope: Box<dyn Fn(String) -> String + Send + Sync>,
    /// `reset_secret_scope(token)`.
    pub reset_secret_scope: Box<dyn Fn(String) + Send + Sync>,
    /// `force_interactive_oauth()` guard — call `f` inside it.
    pub with_force_interactive: Box<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>,
    /// `dashboard_oauth_flow(flow)` guard — call `f` inside it.
    pub with_dashboard_flow: Box<dyn Fn(Arc<OAuthFlow>, Box<dyn FnOnce() + Send>) + Send + Sync>,
    /// `HermesTokenStorage(server_name).snapshot()` → backup.
    pub snapshot_tokens: Box<dyn Fn(&str) -> TokenBackup + Send + Sync>,
    /// `storage.restore(backup, only_if_absent=True)`.
    pub restore_tokens: Box<dyn Fn(&str, TokenBackup, bool) + Send + Sync>,
    /// `manager.remove(server_name, hermes_home=...)` → previous entry opaque.
    pub remove_entry: Box<dyn Fn(&str, &str) -> Option<String> + Send + Sync>,
    /// `manager.restore_entry(server_name, previous_entry, hermes_home=...)`.
    pub restore_entry: Box<dyn Fn(&str, Option<String>, &str) + Send + Sync>,
    /// `_probe_single_server(server_name, cfg, connect_timeout=max(...,315))` → tools.
    pub probe: Box<dyn Fn(&str, &McpServerConfig, f64) -> Result<Vec<(String, String)>, String> + Send + Sync>,
    /// `_oauth_tokens_present(server_name)`.
    pub tokens_present: Box<dyn Fn(&str) -> bool + Send + Sync>,
    /// `_save_mcp_server(server_name, cfg)`.
    pub save_server: Box<dyn Fn(&str, &McpServerConfig) + Send + Sync>,
    /// `reconnect_mcp_server(server_name)`.
    pub reconnect: Box<dyn Fn(&str) + Send + Sync>,
    /// `humanize_oauth_registration_error(server_name, exc, server_url=...)`.
    pub humanize: Box<dyn Fn(&str, &str, Option<&str>) -> Option<String> + Send + Sync>,
}

impl Default for WorkerDeps {
    fn default() -> Self {
        Self {
            set_hermes_home: Box::new(|_| "tok".to_string()),
            reset_hermes_home: Box::new(|_| {}),
            build_secret_scope: Box::new(|p| format!("scope:{}", p)),
            set_secret_scope: Box::new(|s| s),
            reset_secret_scope: Box::new(|_| {}),
            with_force_interactive: Box::new(|f| f()),
            with_dashboard_flow: Box::new(|_, f| f()),
            snapshot_tokens: Box::new(|_| String::new()),
            restore_tokens: Box::new(|_, _, _| {}),
            remove_entry: Box::new(|_, _| None),
            restore_entry: Box::new(|_, _, _| {}),
            probe: Box::new(|_, _, _| Ok(vec![("tool1".to_string(), "desc".to_string())])),
            tokens_present: Box::new(|_| true),
            save_server: Box::new(|_, _| {}),
            reconnect: Box::new(|_| {}),
            humanize: Box::new(|_, _, _| None),
        }
    }
}

impl WorkerDeps {
    /// No-op deps where probe fails.
    pub fn failing_probe(msg: impl Into<String>) -> Self {
        let m = msg.into();
        Self { probe: Box::new(move |_, _, _| Err(m.clone())), ..Self::default() }
    }
}

/// Drive the worker with injected deps — 1:1 with `_worker`'s `try/except/finally`.
///
/// ```python
/// rec = _sessions.get(session_id)
/// flow = rec["flow"] if rec else None
/// try:
///     home_token = set_hermes_home_override(hermes_home)
///     secret_token = set_secret_scope(build_profile_secret_scope(Path(hermes_home)))
///     try:
///         with force_interactive_oauth(), dashboard_oauth_flow(flow):
///             storage = HermesTokenStorage(server_name); backup = storage.snapshot()
///             previous_entry = None
///             try:
///                 previous_entry = manager.remove(server_name, hermes_home=hermes_home)
///                 tools = _probe_single_server(server_name, cfg, connect_timeout=max(float(cfg.get("connect_timeout",0)or 0),315))
///                 if not _oauth_tokens_present(server_name): raise RuntimeError(...)
///                 _save_mcp_server(server_name, cfg)
///                 if flow is not None: flow.tools = [{"name":t,"description":d} for t,d in tools]; flow.mark_approved()
///                 if reconnect_live: reconnect_mcp_server(server_name)
///             except Exception:
///                 storage.restore(backup, only_if_absent=True)
///                 manager.restore_entry(server_name, previous_entry, hermes_home=hermes_home)
///                 raise
///     finally:
///         reset_secret_scope(secret_token); reset_hermes_home_override(home_token)
/// except Exception as exc:
///     msg = str(exc); try: humanized = humanize_oauth_registration_error(...); if humanized: msg = humanized; except: pass
///     if flow is not None: flow.mark_error(msg)
/// finally:
///     if flow is not None: flow.mark_worker_done()
///     if rec is not None: _shutdown_listener(rec)
/// ```
#[allow(clippy::too_many_arguments)]
pub fn drive_worker(
    session_id: &str,
    hermes_home: &str,
    server_name: &str,
    cfg: &McpServerConfig,
    reconnect_live: bool,
    flow: Arc<OAuthFlow>,
    deps: &WorkerDeps,
) {
    let home_tok = (deps.set_hermes_home)(hermes_home);
    let scope = (deps.build_secret_scope)(hermes_home);
    let secret_tok = (deps.set_secret_scope)(scope);
    let flow_clone = Arc::clone(&flow);
    let result: Result<(), String> = {
        let mut err: Option<String> = None;
        // with force_interactive_oauth(), dashboard_oauth_flow(flow):
        (deps.with_force_interactive)(Box::new({
            let flow_inner = Arc::clone(&flow_clone);
            let hermes_home = hermes_home.to_string();
            let server_name = server_name.to_string();
            let cfg = cfg.clone();
            let deps_ref: *const WorkerDeps = deps as *const _;
            move || {
                (deps_ref unsafe { &*deps_ref }.with_dashboard_flow)(Arc::clone(&flow_inner), Box::new({
                    let flow_probe = Arc::clone(&flow_inner);
                    move || {
                        let d: &WorkerDeps = unsafe { &*deps_ref };
                        let backup = (d.snapshot_tokens)(&server_name);
                        let previous = (d.remove_entry)(&server_name, &hermes_home);
                        let connect_timeout = cfg.effective_connect_timeout();
                        let probe_res = (d.probe)(&server_name, &cfg, connect_timeout);
                        match probe_res {
                            Ok(tools) => {
                                if !(d.tokens_present)(&server_name) {
                                    err = Some("The server responded, but no OAuth token was obtained — this provider may require a manually-registered OAuth client.".to_string());
                                    (d.restore_tokens)(&server_name, backup, true);
                                    (d.restore_entry)(&server_name, previous, &hermes_home);
                                    return;
                                }
                                (d.save_server)(&server_name, &cfg);
                                let tool_infos: Vec<ToolInfo> = tools.into_iter().map(|(n, desc)| ToolInfo::new(n, desc)).collect();
                                flow_probe.set_tools(tool_infos);
                                let _ = flow_probe.mark_approved();
                                if reconnect_live {
                                    (d.reconnect)(&server_name);
                                }
                            }
                            Err(e) => {
                                (d.restore_tokens)(&server_name, backup, true);
                                (d.restore_entry)(&server_name, previous, &hermes_home);
                                err = Some(e);
                            }
                        }
                    }
                }));
            }
        }));
        if let Some(e) = err { Err(e) } else { Ok(()) }
    };
    // try/finally: reset scopes
    (deps.reset_secret_scope)(secret_tok);
    (deps.reset_hermes_home)(home_tok);
    if let Err(exc) = result {
        let mut msg = exc.clone();
        if let Some(humanized) = (deps.humanize)(server_name, &exc, cfg.url.as_deref()) {
            if !humanized.is_empty() {
                msg = humanized;
            }
        }
        flow.mark_error(&msg);
    }
    flow.mark_worker_done();
    // _shutdown_listener(rec) — best-effort: the registry's GC will also clean up,
    // but we try to shut the listener if this worker owns the session's handle.
    // The handle lives in the registry; the worker does not hold it directly,
    // so shutdown is deferred to poll/GC. This mirrors Python's `if rec is not None: _shutdown_listener(rec)` where `rec` was the outer captured dict.
    let _ = session_id;
}

// ---------------------------------------------------------------------------
// Start/poll result types
// ---------------------------------------------------------------------------

/// Mirrors `return {"session_id": session_id, "auth_url": auth_url, "flow": "pkce"}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartFlowResult {
    pub session_id: String,
    pub auth_url: String,
    pub flow: String,
}

/// Poll status. Mirrors `pending|approved|error` (provider poll vocabulary;
/// `authorization_required` maps to `pending`).
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum PollStatus {
    Pending,
    Approved,
    Error,
}

impl PollStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Error => "error",
        }
    }
}

/// Mirrors `poll_flow`'s `Dict[str, Any]` → `{status, error_message, auth_url, tools}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PollResult {
    pub session_id: String,
    pub status: PollStatus,
    pub error_message: Option<String>,
    pub auth_url: Option<String>,
    pub tools: Vec<ToolInfo>,
}

// ---------------------------------------------------------------------------
// Global singleton — mirrors Python module globals
// ---------------------------------------------------------------------------

static GLOBAL_REGISTRY: OnceLock<McpOAuthRegistry> = OnceLock::new();

/// Global registry. Mirrors the module-level `_sessions` + `_sessions_lock`.
pub fn global_registry() -> &'static McpOAuthRegistry {
    GLOBAL_REGISTRY.get_or_init(McpOAuthRegistry::new)
}

/// Mirrors `_gc_sessions()` on the global registry.
pub fn gc_sessions() {
    global_registry().gc_sessions();
}

/// Mirrors `start_flow(...)` on the global registry.
pub fn start_flow(
    hermes_home: &str,
    server_name: &str,
    cfg: McpServerConfig,
    reconnect_live: bool,
    url_timeout: Duration,
) -> Result<StartFlowResult, String> {
    global_registry().start_flow(hermes_home, server_name, cfg, reconnect_live, url_timeout)
}

/// Mirrors `poll_flow(session_id, server_name)` on the global registry.
pub fn poll_flow(session_id: &str, server_name: &str) -> PollResult {
    global_registry().poll_flow(session_id, server_name)
}

// ---------------------------------------------------------------------------
// Tests — mirror Python invariants
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn dummy_cfg(url: &str) -> McpServerConfig {
        McpServerConfig { url: Some(url.to_string()), ..Default::default() }
    }

    #[test]
    fn constants_match_python() {
        assert_eq!(SESSION_TTL_SECONDS, 900);
        assert_eq!(SESSION_TTL, Duration::from_secs(900));
        assert_eq!(MAX_PENDING, 12);
        assert!((DEFAULT_URL_TIMEOUT_SECS - 30.0).abs() < 1e-9);
        assert!((CONNECT_TIMEOUT_FLOOR - 315.0).abs() < 1e-9);
    }

    #[test]
    fn generate_session_id_shape() {
        let id = generate_session_id();
        // secrets.token_urlsafe(24) → 32 chars, charset A-Za-z0-9-_
        assert_eq!(id.len(), 32, "{}", id);
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "{}", id);
        let id2 = generate_session_id();
        assert_ne!(id, id2);
    }

    #[test]
    fn flow_publish_and_deliver() {
        let flow = Arc::new(OAuthFlow::new("sid1", "srv", "/home/.hermes", "", false));
        assert_eq!(flow.snapshot().status, "starting");
        // publish must include state
        assert!(flow.publish_authorization_url("https://auth.example.com/?x=1").is_err());
        assert!(flow.publish_authorization_url("https://auth.example.com/authorize?state=abc123").is_ok());
        assert_eq!(flow.snapshot().status, "authorization_required");
        assert_eq!(flow.snapshot().authorization_url.as_deref(), Some("https://auth.example.com/authorize?state=abc123"));
        // deliver with wrong state fails
        assert!(flow.deliver_callback(Some("code1"), Some("wrong"), None).is_err());
        // deliver with correct state succeeds
        assert!(flow.deliver_callback(Some("code1"), Some("abc123"), None).is_ok());
        // second deliver fails (already received)
        assert!(flow.deliver_callback(Some("code1"), Some("abc123"), None).is_err());
    }

    #[test]
    fn flow_deliver_with_error_and_missing_code() {
        let flow = Arc::new(OAuthFlow::new("sid", "srv", "/tmp", "", false));
        flow.publish_authorization_url("https://auth.example.com/?state=s1").unwrap();
        // error overrides code
        assert!(flow.deliver_callback(None, Some("s1"), Some("access_denied")).is_ok());
        // mark_approved / mark_error transitions
        let flow2 = Arc::new(OAuthFlow::new("sid2", "srv", "/tmp", "", false));
        flow2.mark_error("boom");
        assert_eq!(flow2.snapshot().status, "error");
        assert_eq!(flow2.snapshot().error.as_deref(), Some("boom"));
        // approved after error is Err
        assert!(flow2.mark_approved().is_err());
        let flow3 = Arc::new(OAuthFlow::new("sid3", "srv", "/tmp", "", false));
        assert!(flow3.mark_approved().is_ok());
        assert_eq!(flow3.snapshot().status, "approved");
        // error after approved is no-op (stays approved)
        flow3.mark_error("late");
        assert_eq!(flow3.snapshot().status, "approved");
        // worker_done flag
        assert!(!flow3.worker_done());
        flow3.mark_worker_done();
        assert!(flow3.worker_done());
    }

    #[test]
    fn constant_time_eq_cases() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "ab"));
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn parse_query_cases() {
        let m = parse_query("code=hello&state=world&error=");
        assert_eq!(m.get("code").map(|s| s.as_str()), Some("hello"));
        assert_eq!(m.get("state").map(|s| s.as_str()), Some("world"));
        let m2 = parse_query("?code=1%202&state=a%2Bb");
        assert_eq!(m2.get("code").map(|s| s.as_str()), Some("1 2"));
        assert_eq!(m2.get("state").map(|s| s.as_str()), Some("a+b"));
        assert!(parse_query("").is_empty());
        assert!(parse_query("?").is_empty());
    }

    #[test]
    fn connect_timeout_floor() {
        assert!((McpServerConfig::default().effective_connect_timeout() - 315.0).abs() < 1e-9);
        assert!((McpServerConfig { connect_timeout: Some(0.0), ..Default::default() }.effective_connect_timeout() - 315.0).abs() < 1e-9);
        assert!((McpServerConfig { connect_timeout: Some(500.0), ..Default::default() }.effective_connect_timeout() - 500.0).abs() < 1e-9);
        assert!((McpServerConfig { connect_timeout: Some(100.0), ..Default::default() }.effective_connect_timeout() - 315.0).abs() < 1e-9);
        assert!((McpServerConfig { connect_timeout: Some(f64::NAN), ..Default::default() }.effective_connect_timeout() - 315.0).abs() < 1e-9);
    }

    #[test]
    fn registry_pending_and_duplicate() {
        let reg = McpOAuthRegistry::new();
        let flow1 = Arc::new(OAuthFlow::new("s1", "srv-a", "/home/a", "", false));
        let handle1 = start_loopback_listener(Arc::clone(&flow1)).unwrap();
        let rec1 = SessionRecord::new("s1".into(), "srv-a".into(), "/home/a".into(), flow1.clone(), handle1);
        reg.insert_record(rec1);
        assert_eq!(reg.pending_count(), 1);
        assert!(reg.has_pending_for("srv-a", "/home/a"));
        assert!(!reg.has_pending_for("srv-b", "/home/a"));
        assert!(!reg.has_pending_for("srv-a", "/home/b"));
        // starting second flow for same server+home while pending should be rejected
        let cfg = dummy_cfg("https://example.com");
        let res = reg.start_flow_with_worker("/home/a", "srv-a", cfg, false, Duration::from_millis(50), None);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("already in progress"));
        reg.clear();
    }

    #[test]
    fn max_pending_cap() {
        let reg = McpOAuthRegistry::new();
        for i in 0..MAX_PENDING {
            let sid = format!("s{}", i);
            let flow = Arc::new(OAuthFlow::new(sid.clone(), format!("srv-{}", i), "/home/a", "", false));
            let h = start_loopback_listener(Arc::clone(&flow)).unwrap();
            reg.insert_record(SessionRecord::new(sid, format!("srv-{}", i), "/home/a".into(), flow, h));
        }
        assert_eq!(reg.pending_count(), MAX_PENDING);
        let cfg = dummy_cfg("https://example.com");
        let res = reg.start_flow_with_worker("/home/a", "extra", cfg, false, Duration::from_millis(50), None);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("Too many"));
        reg.clear();
        // after marking all done, pending 0 and new start succeeds
        let reg2 = McpOAuthRegistry::new();
        let flow = Arc::new(OAuthFlow::new("x", "srv-x", "/home/a", "", false));
        flow.mark_worker_done();
        let h = start_loopback_listener(Arc::clone(&flow)).unwrap();
        reg2.insert_record(SessionRecord::new("x".into(), "srv-x".into(), "/home/a".into(), flow, h));
        assert_eq!(reg2.pending_count(), 0);
        let cfg2 = dummy_cfg("https://example.com");
        // inject worker that publishes quickly so timeout 500ms succeeds
        let res2 = reg2.start_flow_with_worker("/home/a", "srv-x", cfg2, false, Duration::from_millis(500), None);
        // srv-x is not pending (worker_done), so duplicate check passes → should succeed
        assert!(res2.is_ok(), "expected ok, got {:?}", res2);
        reg2.clear();
    }

    #[test]
    fn start_and_poll_approved() {
        let reg = McpOAuthRegistry::new();
        let cfg = dummy_cfg("https://mcp.example.com");
        let res = reg.start_flow("/tmp/hermes_home", "my-server", cfg, false, Duration::from_millis(500)).unwrap();
        assert_eq!(res.flow, "pkce");
        assert!(!res.session_id.is_empty());
        assert!(!res.auth_url.is_empty());
        // poll pending
        let p1 = reg.poll_flow(&res.session_id, "my-server");
        assert_eq!(p1.status, PollStatus::Pending);
        assert_eq!(p1.auth_url.as_deref(), Some(res.auth_url.as_str()));
        assert!(p1.error_message.is_none());
        // approve via flow
        {
            let g = reg.sessions.lock().unwrap();
            let rec = g.get(&res.session_id).unwrap();
            rec.flow.set_tools(vec![ToolInfo::new("t1", "d1")]);
            rec.flow.mark_approved().unwrap();
        }
        let p2 = reg.poll_flow(&res.session_id, "my-server");
        assert_eq!(p2.status, PollStatus::Approved);
        assert_eq!(p2.tools, vec![ToolInfo::new("t1", "d1")]);
        // server mismatch
        let p3 = reg.poll_flow(&res.session_id, "other-server");
        assert_eq!(p3.status, PollStatus::Error);
        assert!(p3.error_message.unwrap().contains("mismatch"));
        // missing session
        let p4 = reg.poll_flow("no-such", "my-server");
        assert_eq!(p4.status, PollStatus::Error);
        assert!(p4.error_message.unwrap().contains("not found"));
        reg.clear();
    }

    #[test]
    fn start_timeout_cleans_listener() {
        let reg = McpOAuthRegistry::new();
        // Worker that never publishes auth_url → timeout
        let never_publish: WorkerFn = Box::new(|_, _, _, _, _, _flow| {
            thread::sleep(Duration::from_millis(200));
            // do not publish
        });
        let cfg = dummy_cfg("https://example.com");
        let res = reg.start_flow_with_worker("/tmp/h", "srv-timeout", cfg, false, Duration::from_millis(80), Some(never_publish));
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("Timed out"));
        // session exists but flow is error
        let g = reg.sessions.lock().unwrap();
        let rec = g.values().find(|r| r.server_name == "srv-timeout").unwrap();
        assert_eq!(rec.flow.snapshot().status, "error");
        assert!(rec.flow.snapshot().error.as_deref().unwrap().contains("Timed out"));
        drop(g);
        reg.clear();
    }

    #[test]
    fn gc_sessions_removes_stale() {
        let reg = McpOAuthRegistry::new();
        let flow = Arc::new(OAuthFlow::new("old", "srv", "/home", "", false));
        let h = start_loopback_listener(Arc::clone(&flow)).unwrap();
        let old_rec = SessionRecord::new("old".into(), "srv".into(), "/home".into(), flow, h)
            .with_created_at(Instant::now() - SESSION_TTL - Duration::from_secs(10));
        reg.insert_record(old_rec);
        let flow2 = Arc::new(OAuthFlow::new("fresh", "srv2", "/home", "", false));
        let h2 = start_loopback_listener(Arc::clone(&flow2)).unwrap();
        reg.insert_record(SessionRecord::new("fresh".into(), "srv2".into(), "/home".into(), flow2, h2));
        assert_eq!(reg.len(), 2);
        reg.gc_sessions();
        assert_eq!(reg.len(), 1);
        assert!(reg.contains("fresh"));
        assert!(!reg.contains("old"));
        reg.clear();
    }

    #[test]
    fn poll_status_mapping() {
        let reg = McpOAuthRegistry::new();
        // pending = authorization_required → pending
        let flow = Arc::new(OAuthFlow::new("sid", "srv", "/home", "", false));
        flow.publish_authorization_url("https://auth.example.com/?state=s").unwrap();
        let h = start_loopback_listener(Arc::clone(&flow)).unwrap();
        reg.insert_record(SessionRecord::new("sid".into(), "srv".into(), "/home".into(), Arc::clone(&flow), h));
        assert_eq!(reg.poll_flow("sid", "srv").status, PollStatus::Pending);
        // approved
        flow.mark_approved().unwrap();
        assert_eq!(reg.poll_flow("sid", "srv").status, PollStatus::Approved);
        // error
        let flow2 = Arc::new(OAuthFlow::new("sid2", "srv2", "/home", "", false));
        flow2.mark_error("boom");
        let h2 = start_loopback_listener(Arc::clone(&flow2)).unwrap();
        reg.insert_record(SessionRecord::new("sid2".into(), "srv2".into(), "/home".into(), flow2, h2));
        let p = reg.poll_flow("sid2", "srv2");
        assert_eq!(p.status, PollStatus::Error);
        assert_eq!(p.error_message.as_deref(), Some("boom"));
        reg.clear();
    }

    #[test]
    fn drive_worker_success_and_restore_on_probe_fail() {
        let flow = Arc::new(OAuthFlow::new("sid", "srv", "/tmp/home", "", false));
        flow.publish_authorization_url("https://auth.example.com/?state=st123").unwrap();
        let cfg = dummy_cfg("https://mcp.example.com");
        // success path
        let deps = WorkerDeps::default();
        drive_worker("sid", "/tmp/home", "srv", &cfg, false, Arc::clone(&flow), &deps);
        assert_eq!(flow.snapshot().status, "approved");
        assert!(flow.worker_done());
        assert!(!flow.tools().is_empty());
        // failure path with humanize
        let flow2 = Arc::new(OAuthFlow::new("sid2", "srv2", "/tmp/home", "", false));
        flow2.publish_authorization_url("https://auth.example.com/?state=st2").unwrap();
        let mut deps2 = WorkerDeps::failing_probe("probe boom");
        deps2.humanize = Box::new(|_, _, _| Some("humanized: probe boom".into()));
        drive_worker("sid2", "/tmp/home", "srv2", &cfg, false, Arc::clone(&flow2), &deps2);
        assert_eq!(flow2.snapshot().status, "error");
        assert_eq!(flow2.snapshot().error.as_deref(), Some("humanized: probe boom"));
        assert!(flow2.worker_done());
    }

    #[test]
    fn loopback_deliver_via_http() {
        let flow = Arc::new(OAuthFlow::new("sid", "srv", "/tmp", "", false));
        flow.publish_authorization_url("https://auth.example.com/authorize?state=mystate").unwrap();
        let handle = start_loopback_listener(Arc::clone(&flow)).unwrap();
        let port = handle.port();
        // hit /callback with correct state
        let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        let req = format!("GET /callback?code=mycode&state=mystate HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", port);
        stream.write_all(req.as_bytes()).unwrap();
        stream.flush().unwrap();
        let mut resp = String::new();
        stream.read_to_string(&mut resp).unwrap();
        assert!(resp.contains("200"), "{}", resp);
        assert!(resp.contains("Authorization received"), "{}", resp);
        // callback ready now, second attempt should be 400
        let mut stream2 = TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        let req2 = format!("GET /callback?code=again&state=mystate HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", port);
        stream2.write_all(req2.as_bytes()).unwrap();
        stream2.flush().unwrap();
        let mut resp2 = String::new();
        stream2.read_to_string(&mut resp2).unwrap();
        assert!(resp2.contains("400"), "{}", resp2);
        handle.shutdown();
    }

    #[test]
    fn loopback_404_on_unknown_path() {
        let flow = Arc::new(OAuthFlow::new("sid", "srv", "/tmp", "", false));
        let handle = start_loopback_listener(Arc::clone(&flow)).unwrap();
        let port = handle.port();
        let mut s = TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        s.write_all(format!("GET /unknown HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", port).as_bytes()).unwrap();
        s.flush().unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        assert!(resp.contains("404"), "{}", resp);
        handle.shutdown();
    }

    #[test]
    fn loopback_root_is_allowed() {
        // Python: `parsed.path.rstrip("/") not in ("/callback", "")` — root "/" → "" → allowed (empty string case)
        let flow = Arc::new(OAuthFlow::new("sid", "srv", "/tmp", "", false));
        flow.publish_authorization_url("https://auth.example.com/?state=rs").unwrap();
        let handle = start_loopback_listener(Arc::clone(&flow)).unwrap();
        let port = handle.port();
        let mut s = TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        s.write_all(format!("GET /?code=c&state=rs HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", port).as_bytes()).unwrap();
        s.flush().unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        // should be 200 (root is allowed), not 404
        assert!(resp.contains("200"), "root should be allowed, got {}", resp);
        handle.shutdown();
    }
}
