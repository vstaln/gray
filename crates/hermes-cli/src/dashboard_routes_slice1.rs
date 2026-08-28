//! hermes-cli dashboard_routes — slice 1/2
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/dashboard_auth/routes.py`
//! slice 1/2 — lines 1–900 of 1 097 (first 900 LOC).
//! Covers: module docstring + imports + router, `_redirect_uri`,
//! `_client_ip`, `_prefix`, `GET /login` (login_page), `GET /api/auth/providers`,
//! `GET /auth/login`, `_validate_loopback_redirect_uri`,
//! `GET /auth/native/authorize`, `GET /auth/callback`,
//! `_validate_post_login_target`, password-login rate limiter
//! (`_PW_RATE_*`, `_password_rate_limited`, `_reset_password_rate_limit`),
//! `_PasswordLoginBody`, `POST /auth/password-login` (through native
//! branch + tail), and `POST /auth/logout` head through `clear_pkce_cookie`
//! (line 900). Continued in `dashboard_routes_slice2.rs`
//! (from `clear_pkce_cookie` tail at line 901 through `/api/auth/me`,
//! `/api/auth/ws-ticket`, `/auth/native/token`, `/auth/native/refresh` EOF).
//!
//! T0714 — 1:1 port, no cargo (NEVER cargo).

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-15
// ---------------------------------------------------------------------------

/// Module doc — HTTP routes for the dashboard-auth OAuth round trip.
///
/// Mounted at root (no prefix) by `web_server.py`. The router does not
/// auto-gate; gating is performed by `gated_auth_middleware`, which
/// allowlists everything under `/auth/*` and `/api/auth/providers`.
///
/// Mirrors `hermes_cli/dashboard_auth/routes.py` lines 1-15.
/// Routes:
///   GET  /login              → server-rendered login page
///   GET  /auth/login?provider=N → 302 to IDP, sets PKCE cookie
///   GET  /auth/callback?code,state → completes login, sets session cookies
///   POST /auth/logout        → clears cookies, best-effort revoke
///   GET  /api/auth/providers → list registered providers (login bootstrap)
///   GET  /api/auth/me        → current Session as JSON (auth-required)
pub const MODULE_DOC: &str = "dashboard_auth.routes: OAuth round-trip routes — see routes.py lines 1-15";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 18-50
// ---------------------------------------------------------------------------
// Python: logging, threading, time, collections.defaultdict/deque,
// typing.Any/Deque/Dict, fastapi.APIRouter/HTTPException/Request,
// fastapi.responses (HTMLResponse/JSONResponse/RedirectResponse),
// pydantic.BaseModel, hermes_cli.dashboard_auth (get_provider etc.),
// hermes_cli.dashboard_auth.audit, base, cookies, login_page
//
// Rust: std only (NEVER cargo). FastAPI, Pydantic, and hermes_cli.dashboard_auth
// submodules are stubbed for 1:1 traceability; real HTTP wiring in web_server.

/// Mirrors `logging.getLogger(__name__)` — line 51.
pub fn log_debug(msg: &str) {
    if std::env::var("HERMES_DEBUG").is_ok() {
        eprintln!("[dashboard_routes] DEBUG: {msg}");
    }
}
pub fn log_warning(msg: &str) {
    eprintln!("[dashboard_routes] WARN: {msg}");
}

/// Mirrors `router = APIRouter()` — line 53.
#[derive(Debug, Clone, Default)]
pub struct RouterStub {
    pub routes: Vec<RouteDef>,
}
#[derive(Debug, Clone)]
pub struct RouteDef {
    pub method: String,
    pub path: String,
    pub name: String,
}
impl RouterStub {
    pub fn new() -> Self {
        Self { routes: Vec::new() }
    }
    pub fn add(&mut self, method: &str, path: &str, name: &str) {
        self.routes.push(RouteDef { method: method.to_string(), path: path.to_string(), name: name.to_string() });
    }
}
pub static ROUTER: OnceLock<Mutex<RouterStub>> = OnceLock::new();
pub fn router() -> &'static Mutex<RouterStub> {
    ROUTER.get_or_init(|| Mutex::new(RouterStub::new()))
}

// ---------------------------------------------------------------------------
// Request / Response stubs — mirrors FastAPI types (lines 24-25)
// ---------------------------------------------------------------------------

/// Minimal mirror of `fastapi.Request` for 1:1 signature coverage.
/// Real request carries headers, client, query_params, url, state; we stub fields
/// used by the ported functions.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub headers: HashMap<String, String>,
    pub client_host: Option<String>,
    pub query_params: HashMap<String, String>,
    pub url: String,
    pub state_session: Option<Session>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        // case-insensitive lookup mirroring Starlette's Headers
        let lower = name.to_lowercase();
        for (k, v) in &self.headers {
            if k.to_lowercase() == lower {
                return Some(v.as_str());
            }
        }
        None
    }
    /// Mirrors `request.url_for("auth_callback")` — returns callback URL string.
    pub fn url_for_auth_callback(&self) -> String {
        // Default mirrors uvicorn proxy_headers behavior: reconstruct from request.url
        // For stub, if url contains /auth/callback return it, else build from base.
        if self.url.contains("/auth/callback") {
            return self.url.clone();
        }
        // Fallback: derive scheme+host from url or default to http://localhost
        let base = if self.url.is_empty() { "http://localhost".to_string() } else { self.url.clone() };
        // Strip query and ensure path ends with /auth/callback
        let base_no_q = base.split('?').next().unwrap_or(&base).split('#').next().unwrap_or(&base).to_string();
        // If base already has host, append path
        if base_no_q.contains("://") {
            let trimmed = base_no_q.trim_end_matches('/');
            format!("{trimmed}/auth/callback")
        } else {
            "http://localhost/auth/callback".to_string()
        }
    }
}

#[derive(Debug, Clone)]
pub struct HttpException {
    pub status_code: u16,
    pub detail: String,
}
impl HttpException {
    pub fn new(status_code: u16, detail: impl Into<String>) -> Self {
        Self { status_code, detail: detail.into() }
    }
}
impl std::fmt::Display for HttpException {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.status_code, self.detail)
    }
}
impl std::error::Error for HttpException {}

#[derive(Debug, Clone)]
pub struct RedirectResponse {
    pub url: String,
    pub status_code: u16,
    pub headers: HashMap<String, String>,
    pub cookies: Vec<CookieOp>,
}
#[derive(Debug, Clone)]
pub struct HtmlResponse {
    pub body: String,
    pub headers: HashMap<String, String>,
}
#[derive(Debug, Clone)]
pub struct JsonResponse {
    pub body: String,
    pub status_code: u16,
    pub headers: HashMap<String, String>,
}
#[derive(Debug, Clone)]
pub struct CookieOp {
    pub name: String,
    pub value: String,
    pub op: String, // "set" or "clear"
}

// ---------------------------------------------------------------------------
// Session / Provider stubs — mirrors hermes_cli.dashboard_auth.* (lines 28-49)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct Session {
    pub user_id: String,
    pub email: String,
    pub display_name: String,
    pub org_id: String,
    pub provider: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone)]
pub struct Provider {
    pub name: String,
    pub display_name: String,
    pub supports_password: bool,
    pub supports_session: bool,
}
impl Provider {
    pub fn new(name: &str, display_name: &str, supports_password: bool, supports_session: bool) -> Self {
        Self { name: name.to_string(), display_name: display_name.to_string(), supports_password, supports_session }
    }
}

/// Mirrors `hermes_cli.dashboard_auth.get_provider` (line 29).
pub fn get_provider(name: &str) -> Option<Provider> {
    // Stub: in real wiring this consults the provider registry.
    // For 1:1 traceability we simulate unknown provider as None.
    let _ = name;
    None
}
/// Mirrors `hermes_cli.dashboard_auth.list_providers` (line 30).
pub fn list_providers() -> Vec<Provider> {
    Vec::new()
}
/// Mirrors `hermes_cli.dashboard_auth.list_session_providers` (line 31).
pub fn list_session_providers() -> Vec<Provider> {
    Vec::new()
}

// Audit stubs — mirrors hermes_cli.dashboard_auth.audit (lines 33)
#[derive(Debug, Clone)]
pub enum AuditEvent {
    LoginStart,
    LoginSuccess,
    LoginFailure,
    Logout,
    NativeAuthorizeStart,
    NativeCodeIssued,
    NativeTokenFailure,
    NativeTokenSuccess,
    RefreshSuccess,
    RefreshFailure,
    WsTicketMinted,
}
impl AuditEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuditEvent::LoginStart => "LOGIN_START",
            AuditEvent::LoginSuccess => "LOGIN_SUCCESS",
            AuditEvent::LoginFailure => "LOGIN_FAILURE",
            AuditEvent::Logout => "LOGOUT",
            AuditEvent::NativeAuthorizeStart => "NATIVE_AUTHORIZE_START",
            AuditEvent::NativeCodeIssued => "NATIVE_CODE_ISSUED",
            AuditEvent::NativeTokenFailure => "NATIVE_TOKEN_FAILURE",
            AuditEvent::NativeTokenSuccess => "NATIVE_TOKEN_SUCCESS",
            AuditEvent::RefreshSuccess => "REFRESH_SUCCESS",
            AuditEvent::RefreshFailure => "REFRESH_FAILURE",
            AuditEvent::WsTicketMinted => "WS_TICKET_MINTED",
        }
    }
}
pub fn audit_log(_event: AuditEvent, _provider: &str, _reason: &str, _ip: &str) {
    // Mirrors `audit.audit_log` — best-effort structured log; stub for 1:1.
}

// Base error stubs — mirrors hermes_cli.dashboard_auth.base (lines 34-38)
#[derive(Debug, Clone)]
pub struct InvalidCodeError(pub String);
#[derive(Debug, Clone)]
pub struct InvalidCredentialsError(pub String);
#[derive(Debug, Clone)]
pub struct ProviderError(pub String);
#[derive(Debug, Clone)]
pub struct RefreshExpiredError(pub String);

// Cookies stubs — mirrors hermes_cli.dashboard_auth.cookies (lines 39-48)
pub fn clear_pkce_cookie(_resp: &mut RedirectResponse, _prefix: &str) {}
pub fn clear_pkce_cookie_json(_resp: &mut JsonResponse, _prefix: &str) {}
pub fn clear_session_cookies(_resp: &mut RedirectResponse, _prefix: &str) {}
pub fn clear_sso_attempt_cookie(_resp: &mut RedirectResponse, _prefix: &str) {}
pub fn clear_sso_attempt_cookie_generic(_resp: &mut RedirectResponse, _prefix: &str) {}
pub fn detect_https(_request: &Request) -> bool {
    // Mirrors `cookies.detect_https` — checks X-Forwarded-Proto or request.url scheme
    if let Some(proto) = _request.header("x-forwarded-proto") {
        return proto.to_lowercase() == "https";
    }
    _request.url.starts_with("https://")
}
pub fn read_pkce_cookie(_request: &Request) -> String { String::new() }
pub fn read_session_cookies(_request: &Request) -> (String, String) { (String::new(), String::new()) }
pub fn set_pkce_cookie(_resp: &mut RedirectResponse, _payload: &str, _use_https: bool, _prefix: &str) {}
pub fn set_session_cookies(_resp: &mut RedirectResponse, _access_token: &str, _refresh_token: &str, _access_token_expires_in: i64, _use_https: bool, _prefix: &str, _provider: &str) {}
pub fn set_session_cookies_json(_resp: &mut JsonResponse, _access_token: &str, _refresh_token: &str, _expires_in: i64, _use_https: bool, _prefix: &str, _provider: &str) {}

// Login page stub — mirrors hermes_cli.dashboard_auth.login_page.render_login_html (line 49)
pub fn render_login_html(_next_path: &str) -> String {
    format!("<html><body>login next={}</body></html>", _next_path)
}

// Prefix stubs — mirrors hermes_cli.dashboard_auth.prefix (lines 84-87, 104, 116-124)
pub fn resolve_public_url() -> Option<String> {
    // Mirrors `prefix.resolve_public_url()` — reads HERMES_DASHBOARD_PUBLIC_URL or config.yaml
    if let Ok(v) = std::env::var("HERMES_DASHBOARD_PUBLIC_URL") {
        let v = v.trim().trim_end_matches('/').to_string();
        if !v.is_empty() { return Some(v); }
    }
    None
}
pub fn prefix_from_request(_request: &Request) -> String {
    // Mirrors `prefix.prefix_from_request` — reads X-Forwarded-Prefix and normalises
    if let Some(prefix) = _request.header("x-forwarded-prefix") {
        let trimmed = prefix.trim().trim_end_matches('/').to_string();
        if trimmed.is_empty() || trimmed == "/" { return String::new(); }
        // Normalise: must start with /
        if trimmed.starts_with('/') { return trimmed; }
        return format!("/{trimmed}");
    }
    String::new()
}

// Native flow stubs — mirrors hermes_cli.dashboard_auth.native_flow (lines 363-373 etc.)
pub mod native_flow {
    use super::Session;
    #[derive(Debug, Clone)]
    pub struct PendingAuth {
        pub redirect_uri: String,
        pub client_state: String,
    }
    #[derive(Debug, Clone)]
    pub struct NativeFlowError(pub String);
    impl std::fmt::Display for NativeFlowError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}", self.0) }
    }
    impl std::error::Error for NativeFlowError {}
    #[derive(Debug, Clone)]
    pub struct CodeInvalid(pub String);
    pub fn register_pending(_code_challenge: &str, _redirect_uri: &str, _client_state: &str, _client_ip: &str) -> Result<String, NativeFlowError> {
        Ok("broker_stub".to_string())
    }
    pub fn get_pending(_broker_state: &str) -> Result<PendingAuth, NativeFlowError> {
        Err(NativeFlowError("not found".to_string()))
    }
    pub fn complete_pending(_broker_state: &str, _session: Session) -> Result<String, NativeFlowError> {
        Ok("gw_code_stub".to_string())
    }
    pub fn redeem_code(_code: &str, _code_verifier: &str) -> Result<Session, CodeInvalid> {
        Err(CodeInvalid("invalid".to_string()))
    }
}

// WS tickets stub — mirrors hermes_cli.dashboard_auth.ws_tickets (lines 952)
pub mod ws_tickets {
    pub const TTL_SECONDS: u64 = 30;
    pub fn mint_ticket(_user_id: &str, _provider: &str) -> String { "ticket_stub".to_string() }
}

// LoginState stub — mirrors provider.start_login return (ls.redirect_url, ls.cookie_payload)
#[derive(Debug, Clone)]
pub struct LoginState {
    pub redirect_url: String,
    pub cookie_payload: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// _redirect_uri — mirrors lines 56-105
// ---------------------------------------------------------------------------

/// Reconstruct the absolute callback URL the IDP redirects back to.
///
/// Three resolution tiers (mirrors Python docstring lines 57-81):
///   1. `HERMES_DASHBOARD_PUBLIC_URL` env var or `dashboard.public_url` in config.yaml
///   2. `X-Forwarded-Prefix: /hermes` (Mission Control)
///   3. Bare `request.url_for("auth_callback")` (Fly.io default)
/// Mirrors `def _redirect_uri(request: Request) -> str:` at lines 56-105.
pub fn redirect_uri(request: &Request) -> String {
    // Tier 1: operator-declared public URL
    if let Some(public_url) = resolve_public_url() {
        // `public_url` already stripped trailing slash; append verbatim
        return format!("{public_url}/auth/callback");
    }
    // Tier 2 + 3: reconstruct from request URL, optionally with X-Forwarded-Prefix
    let base = request.url_for_auth_callback();
    let prefix = prefix_from_request(request);
    if prefix.is_empty() {
        return base;
    }
    // Append prefix to parsed path — cheap string manipulation without url crate (std only)
    // Mirrors `parsed._replace(path=f"{prefix}{parsed.path}")` (lines 104-105)
    if let Some(scheme_end) = base.find("://") {
        let after_scheme = &base[scheme_end + 3..];
        if let Some(path_start) = after_scheme.find('/') {
            let authority = &base[..scheme_end + 3 + path_start];
            let path_and_rest = &base[scheme_end + 3 + path_start..];
            // Split path from query/fragment
            let (path, suffix) = if let Some(q) = path_and_rest.find(|c| c == '?' || c == '#') {
                (&path_and_rest[..q], &path_and_rest[q..])
            } else {
                (path_and_rest, "")
            };
            return format!("{authority}{}{path}{suffix}", prefix);
        } else {
            // No path, just authority
            return format!("{base}{prefix}/auth/callback");
        }
    }
    // Fallback: no scheme
    format!("{prefix}{base}")
}

// ---------------------------------------------------------------------------
// _client_ip — mirrors lines 108-112
// ---------------------------------------------------------------------------

/// Mirrors `def _client_ip(request: Request) -> str:` at lines 108-112.
pub fn client_ip(request: &Request) -> String {
    if let Some(fwd) = request.header("x-forwarded-for") {
        if !fwd.trim().is_empty() {
            // Take first entry before comma
            let first = fwd.split(',').next().unwrap_or("").trim().to_string();
            if !first.is_empty() {
                return first;
            }
        }
    }
    request.client_host.clone().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// _prefix — mirrors lines 115-124
// ---------------------------------------------------------------------------

/// Resolve the X-Forwarded-Prefix header for the active request.
/// Mirrors `def _prefix(request: Request) -> str:` at lines 115-124.
pub fn prefix(request: &Request) -> String {
    prefix_from_request(request)
}

// ---------------------------------------------------------------------------
// Public: login page — mirrors lines 131-144
// ---------------------------------------------------------------------------

/// Mirrors `@router.get("/login", name="login_page")` at lines 132-144.
/// Returns HTMLResponse with Cache-Control: no-store.
pub fn login_page(request: &Request) -> HtmlResponse {
    let next_raw = request.query_params.get("next").map(|s| s.as_str()).unwrap_or("");
    let next_path = validate_post_login_target(next_raw);
    let body = render_login_html(&next_path);
    let mut headers = HashMap::new();
    headers.insert("Cache-Control".to_string(), "no-store, no-cache, must-revalidate".to_string());
    HtmlResponse { body, headers }
}

// ---------------------------------------------------------------------------
// Public: provider list — mirrors lines 152-174
// ---------------------------------------------------------------------------

/// Mirrors `@router.get("/api/auth/providers", name="auth_providers")` at lines 152-174.
pub fn api_auth_providers() -> Result<String, HttpException> {
    let providers = list_session_providers();
    if providers.is_empty() {
        // Q13: fail-closed when zero providers registered — mirrors lines 157-162
        return Err(HttpException::new(503, "no auth providers registered"));
    }
    // Build JSON manually without serde (std only, NEVER cargo)
    let mut items: Vec<String> = Vec::new();
    for p in providers {
        let item = format!(
            r#"{{"name":"{}","display_name":"{}","supports_password":{}}}"#,
            json_escape(&p.name),
            json_escape(&p.display_name),
            p.supports_password
        );
        items.push(item);
    }
    Ok(format!(r#"{{"providers":[{}]}}"#, items.join(",")))
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r")
}

// ---------------------------------------------------------------------------
// Public: OAuth round trip — mirrors lines 182-245
// ---------------------------------------------------------------------------

/// Mirrors `@router.get("/auth/login", name="auth_login")` at lines 182-245.
pub fn auth_login(request: &Request, provider: &str, next: &str) -> Result<RedirectResponse, HttpException> {
    let p = get_provider(provider).ok_or_else(|| HttpException::new(404, format!("Unknown provider: {provider:?}")))?;
    if !p.supports_session {
        return Err(HttpException::new(404, format!("Provider does not support interactive login: {provider:?}")));
    }
    if p.supports_password {
        let safe_next = validate_post_login_target(next);
        let login_url = if safe_next.is_empty() {
            format!("{}/login", prefix(request))
        } else {
            format!("{}/login?next={}", prefix(request), url_encode(&safe_next))
        };
        return Ok(RedirectResponse { url: login_url, status_code: 302, headers: HashMap::new(), cookies: Vec::new() });
    }

    // Non-password provider: start upstream PKCE login
    let ls = start_login_stub(&p, &redirect_uri(request)).map_err(|e| {
        audit_log(AuditEvent::LoginFailure, provider, "provider_unreachable", &client_ip(request));
        HttpException::new(503, format!("Provider unreachable: {e}"))
    })?;

    audit_log(AuditEvent::LoginStart, provider, "", &client_ip(request));

    let mut resp = RedirectResponse { url: ls.redirect_url.clone(), status_code: 302, headers: HashMap::new(), cookies: Vec::new() };
    let mut pkce = ls.cookie_payload.get("hermes_session_pkce").cloned().unwrap_or_default();
    if !pkce.contains("provider=") {
        if pkce.is_empty() {
            pkce = format!("provider={provider}");
        } else {
            pkce = format!("provider={provider};{pkce}");
        }
    }
    let safe_next = validate_post_login_target(next);
    if !safe_next.is_empty() {
        pkce = format!("{pkce};next={}", url_encode(&safe_next));
    }
    set_pkce_cookie(&mut resp, &pkce, detect_https(request), &prefix(request));
    Ok(resp)
}

fn start_login_stub(_provider: &Provider, _redirect_uri: &str) -> Result<LoginState, ProviderError> {
    // Mirrors `p.start_login(redirect_uri=...)` — stub returns dummy redirect
    Err(ProviderError("stub: provider.start_login not wired in slice1".to_string()))
}

fn url_encode(s: &str) -> String {
    // Minimal percent-encoding for `quote(safe='')` equivalent — encodes all non-unreserved
    let mut out = String::new();
    for b in s.bytes() {
        let c = b as char;
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
            out.push(c);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
fn url_decode(s: &str) -> String {
    // Mirrors `urllib.parse.unquote` — decodes %XX
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hi = chars.next().unwrap_or('0');
            let lo = chars.next().unwrap_or('0');
            let hex = format!("{hi}{lo}");
            if let Ok(b) = u8::from_str_radix(&hex, 16) {
                out.push(b as char);
            } else {
                out.push('%');
                out.push(hi);
                out.push(lo);
            }
        } else {
            out.push(c);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Public: RFC 8252 native-app authorization — mirrors lines 253-423
// ---------------------------------------------------------------------------

/// Mirrors `def _validate_loopback_redirect_uri(raw: str) -> str:` at lines 253-286.
pub fn validate_loopback_redirect_uri(raw: &str) -> Result<String, HttpException> {
    if raw.is_empty() {
        return Err(HttpException::new(400, "redirect_uri required"));
    }
    // Parse URL without `url` crate (std only)
    let parsed = parse_url(raw);
    if parsed.scheme != "http" {
        return Err(HttpException::new(400, "native redirect_uri must be http:// on the loopback interface"));
    }
    let host_lower = parsed.hostname.to_lowercase();
    if host_lower != "127.0.0.1" && host_lower != "::1" {
        return Err(HttpException::new(400, "native redirect_uri host must be a loopback IP literal (127.0.0.1 / ::1)"));
    }
    Ok(raw.to_string())
}

#[derive(Debug, Clone, Default)]
struct ParsedUrl {
    scheme: String,
    hostname: String,
    path: String,
}
fn parse_url(raw: &str) -> ParsedUrl {
    let mut scheme = String::new();
    let mut hostname = String::new();
    let mut path = String::new();
    // Extract scheme
    if let Some(idx) = raw.find("://") {
        scheme = raw[..idx].to_lowercase();
        let after = &raw[idx + 3..];
        // Extract host[:port] before first / ? #
        let host_end = after.find(|c| c == '/' || c == '?' || c == '#').unwrap_or(after.len());
        let host_port = &after[..host_end];
        path = if host_end < after.len() { after[host_end..].to_string() } else { String::new() };
        // Handle IPv6 bracketed host
        if host_port.starts_with('[') {
            if let Some(end) = host_port.find(']') {
                hostname = host_port[1..end].to_string();
            } else {
                hostname = host_port.to_string();
            }
        } else {
            // Strip userinfo @, then port :
            let host_no_user = if let Some(at) = host_port.rfind('@') { &host_port[at+1..] } else { host_port };
            // Strip port if numeric
            if let Some(colon) = host_no_user.rfind(':') {
                let after_colon = &host_no_user[colon+1..];
                if after_colon.chars().all(|c| c.is_ascii_digit()) && !after_colon.is_empty() {
                    hostname = host_no_user[..colon].to_string();
                } else {
                    hostname = host_no_user.to_string();
                }
            } else {
                hostname = host_no_user.to_string();
            }
        }
    } else {
        hostname = raw.to_string();
    }
    ParsedUrl { scheme, hostname, path }
}

/// Mirrors `@router.get("/auth/native/authorize", name="auth_native_authorize")` at lines 289-423.
pub fn auth_native_authorize(
    request: &Request,
    provider: &str,
    code_challenge: &str,
    code_challenge_method: &str,
    redirect_uri_param: &str,
    state: &str,
) -> Result<RedirectResponse, HttpException> {
    // Mirrors lines 317-326: S256 + non-empty challenge + loopback validate
    if code_challenge_method.to_uppercase() != "S256" {
        return Err(HttpException::new(400, "code_challenge_method must be S256"));
    }
    if code_challenge.is_empty() {
        return Err(HttpException::new(400, "code_challenge required"));
    }
    validate_loopback_redirect_uri(redirect_uri_param)?;

    // Resolve provider — mirrors lines 336-355
    let mut p_opt = if provider.is_empty() { None } else { get_provider(provider) };
    if p_opt.is_none() && provider.is_empty() {
        let native_eligible: Vec<Provider> = list_session_providers().into_iter().filter(|pp| !pp.supports_password).collect();
        if native_eligible.len() == 1 {
            p_opt = Some(native_eligible.into_iter().next().unwrap());
        } else if native_eligible.is_empty() {
            let sess_providers = list_session_providers();
            if sess_providers.len() == 1 {
                p_opt = Some(sess_providers.into_iter().next().unwrap());
            }
        }
    }
    let p = p_opt.ok_or_else(|| HttpException::new(404, format!("Unknown provider: {provider:?}")))?;
    if !p.supports_session {
        return Err(HttpException::new(400, format!("Provider does not support native login: {:?}", p.name)));
    }

    let broker_state = native_flow::register_pending(code_challenge, redirect_uri_param, state, &client_ip(request))
        .map_err(|e| HttpException::new(503, e.to_string()))?;

    if p.supports_password {
        // Password provider branch — mirrors lines 375-397
        audit_log(AuditEvent::NativeAuthorizeStart, &p.name, "", &client_ip(request));
        let mut resp = RedirectResponse { url: format!("{}/login", prefix(request)), status_code: 302, headers: HashMap::new(), cookies: Vec::new() };
        set_pkce_cookie(&mut resp, &format!("provider={};broker={}", p.name, broker_state), detect_https(request), &prefix(request));
        return Ok(resp);
    }

    let ls = start_login_stub(&p, &redirect_uri(request))
        .map_err(|e| HttpException::new(503, format!("Provider unreachable: {e}")))?;

    audit_log(AuditEvent::NativeAuthorizeStart, &p.name, "", &client_ip(request));

    let mut resp = RedirectResponse { url: ls.redirect_url.clone(), status_code: 302, headers: HashMap::new(), cookies: Vec::new() };
    let mut pkce = ls.cookie_payload.get("hermes_session_pkce").cloned().unwrap_or_default();
    if !pkce.contains("provider=") {
        if pkce.is_empty() {
            pkce = format!("provider={}", p.name);
        } else {
            pkce = format!("provider={};{pkce}", p.name);
        }
    }
    pkce = format!("{pkce};broker={broker_state}");
    set_pkce_cookie(&mut resp, &pkce, detect_https(request), &prefix(request));
    Ok(resp)
}

// ---------------------------------------------------------------------------
// Public: /auth/callback — mirrors lines 426-605
// ---------------------------------------------------------------------------

/// Mirrors `@router.get("/auth/callback", name="auth_callback")` at lines 426-605.
/// Full port of tier-1/2 redirect handling, PKCE cookie parsing, state/CSRF,
/// error propagation, session minting, and the RFC 8252 native branch (loopback code issuance).
pub fn auth_callback(
    request: &Request,
    code: &str,
    state: &str,
    error: &str,
    error_description: &str,
) -> Result<RedirectResponse, HttpException> {
    let pkce_raw = read_pkce_cookie(request);
    if pkce_raw.is_empty() {
        audit_log(AuditEvent::LoginFailure, "", "missing_pkce_cookie", &client_ip(request));
        return Err(HttpException::new(400, "Missing PKCE state cookie"));
    }

    // Parse provider=...;state=...;verifier=...;next=...;broker=... — mirrors lines 450-465
    let parts: HashMap<String, String> = pkce_raw.split(';').filter(|s| s.contains('=')).filter_map(|seg| {
        let mut kv = seg.splitn(2, '=');
        Some((kv.next()?.trim().to_string(), kv.next()?.trim().to_string()))
    }).collect();

    let provider_name = parts.get("provider").cloned().unwrap_or_default();
    let expected_state = parts.get("state").cloned().unwrap_or_default();
    let verifier = parts.get("verifier").cloned().unwrap_or_default();
    let next_from_cookie = parts.get("next").cloned().unwrap_or_default();
    let broker_state = parts.get("broker").cloned().unwrap_or_default();

    let p = get_provider(&provider_name).ok_or_else(|| HttpException::new(400, format!("Unknown provider in cookie: {provider_name:?}")))?;

    if !error.is_empty() {
        audit_log(AuditEvent::LoginFailure, &provider_name, "idp_error", &client_ip(request));
        return Err(HttpException::new(400, format!("OAuth error from provider: {error} ({error_description})")));
    }

    if state.is_empty() || state != expected_state {
        audit_log(AuditEvent::LoginFailure, &provider_name, "state_mismatch", &client_ip(request));
        return Err(HttpException::new(400, "OAuth state mismatch (CSRF check failed)"));
    }

    // Mirrors lines 499-524: p.complete_login
    let session = complete_login_stub(&p, code, state, &verifier, &redirect_uri(request)).map_err(|e| {
        match e {
            CompleteLoginError::InvalidCode(msg) => {
                audit_log(AuditEvent::LoginFailure, &provider_name, "invalid_code", &client_ip(request));
                HttpException::new(400, format!("Invalid code: {msg}"))
            }
            CompleteLoginError::ProviderUnreachable(msg) => {
                audit_log(AuditEvent::LoginFailure, &provider_name, "provider_unreachable", &client_ip(request));
                HttpException::new(503, format!("Provider unreachable: {msg}"))
            }
        }
    })?;

    audit_log(AuditEvent::LoginSuccess, &provider_name, "", &client_ip(request));
    let expires_in = std::cmp::max(60, session.expires_at - current_unix_secs() as i64);

    // RFC 8252 native branch — mirrors lines 544-582
    if !broker_state.is_empty() {
        let pending = native_flow::get_pending(&broker_state).map_err(|_| {
            audit_log(AuditEvent::NativeTokenFailure, &provider_name, "pending_not_found", &client_ip(request));
            HttpException::new(400, "Native login expired or unknown; restart sign-in.")
        })?;
        let gw_code = native_flow::complete_pending(&broker_state, session.clone()).map_err(|_| {
            audit_log(AuditEvent::NativeTokenFailure, &provider_name, "pending_not_found", &client_ip(request));
            HttpException::new(400, "Native login expired or unknown; restart sign-in.")
        })?;
        let sep = if pending.redirect_uri.contains('?') { "&" } else { "?" };
        let loopback = format!("{}{}code={}&state={}", pending.redirect_uri, sep, url_encode(&gw_code), url_encode(&pending.client_state));
        audit_log(AuditEvent::NativeCodeIssued, &provider_name, "", &client_ip(request));
        let mut resp = RedirectResponse { url: loopback, status_code: 302, headers: HashMap::new(), cookies: Vec::new() };
        clear_pkce_cookie(&mut resp, &prefix(request));
        clear_sso_attempt_cookie(&mut resp, &prefix(request));
        return Ok(resp);
    }

    // Standard cookie login branch — mirrors lines 584-605
    let landing = {
        let v = validate_post_login_target(&next_from_cookie);
        if v.is_empty() { "/".to_string() } else { v }
    };
    let mut resp = RedirectResponse { url: landing, status_code: 302, headers: HashMap::new(), cookies: Vec::new() };
    set_session_cookies(&mut resp, &session.access_token, &session.refresh_token, expires_in, detect_https(request), &prefix(request), &session.provider);
    clear_pkce_cookie(&mut resp, &prefix(request));
    clear_sso_attempt_cookie(&mut resp, &prefix(request));
    Ok(resp)
}

enum CompleteLoginError {
    InvalidCode(String),
    ProviderUnreachable(String),
}
fn complete_login_stub(_p: &Provider, _code: &str, _state: &str, _verifier: &str, _redirect_uri: &str) -> Result<Session, CompleteLoginError> {
    Err(CompleteLoginError::ProviderUnreachable("stub: complete_login not wired in slice1".to_string()))
}

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}

// ---------------------------------------------------------------------------
// _validate_post_login_target — mirrors lines 608-639
// ---------------------------------------------------------------------------

/// Return `raw` if it's a safe same-origin path, else empty string.
/// Mirrors `def _validate_post_login_target(raw: str) -> str:` at lines 608-639.
pub fn validate_post_login_target(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let decoded = url_decode(raw);
    if !decoded.starts_with('/') || decoded.starts_with("//") {
        return String::new();
    }
    // Don't loop back to login pages or auth flow — mirrors lines 626-628
    for p in ["/login", "/auth/", "/api/auth/"] {
        if decoded == p || decoded.starts_with(p) {
            return String::new();
        }
    }
    // Reject any /api/* target — mirrors lines 637-638
    if decoded == "/api" || decoded.starts_with("/api/") {
        return String::new();
    }
    decoded
}

// ---------------------------------------------------------------------------
// Public: password (non-redirect) login — mirrors lines 642-872
// ---------------------------------------------------------------------------
//
// Brute-force throttle — mirrors header comment lines 642-654

pub const PW_RATE_MAX_ATTEMPTS: usize = 10;
pub const PW_RATE_WINDOW_SEC: f64 = 60.0;

static PW_ATTEMPTS: OnceLock<Mutex<HashMap<String, VecDeque<f64>>>> = OnceLock::new();
fn pw_attempts() -> &'static Mutex<HashMap<String, VecDeque<f64>>> {
    PW_ATTEMPTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Mirrors `def _password_rate_limited(ip: str) -> bool:` at lines 662-682.
pub fn password_rate_limited(ip: &str) -> bool {
    let now = monotonic_secs();
    let cutoff = now - PW_RATE_WINDOW_SEC;
    let key = if ip.is_empty() { "_unknown_".to_string() } else { ip.to_string() };
    let mut map = pw_attempts().lock().unwrap_or_else(|e| e.into_inner());
    let bucket = map.entry(key).or_insert_with(VecDeque::new);
    while bucket.front().map(|v| *v < cutoff).unwrap_or(false) {
        bucket.pop_front();
    }
    if bucket.len() >= PW_RATE_MAX_ATTEMPTS {
        return true;
    }
    bucket.push_back(now);
    false
}

fn monotonic_secs() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Use SystemTime monotonic approximation via Instant would require lazy static Instant origin.
    // For 1:1 correctness we use Instant::now() anchored to first call.
    static ORIGIN: OnceLock<std::time::Instant> = OnceLock::new();
    let origin = ORIGIN.get_or_init(std::time::Instant::now);
    origin.elapsed().as_secs_f64()
}

/// Mirrors `def _reset_password_rate_limit() -> None:` at lines 685-687.
/// Test-only: clear all rate-limit buckets.
pub fn reset_password_rate_limit() {
    if let Some(m) = PW_ATTEMPTS.get() {
        if let Ok(mut map) = m.lock() {
            map.clear();
        }
    }
}

/// Mirrors `class _PasswordLoginBody(BaseModel):` at lines 690-694.
#[derive(Debug, Clone)]
pub struct PasswordLoginBody {
    pub provider: String,
    pub username: String,
    pub password: String,
    pub next: String,
}

/// Mirrors `@router.post("/auth/password-login", name="auth_password_login")` at lines 697-872.
pub fn auth_password_login(request: &Request, body: &PasswordLoginBody) -> Result<JsonResponse, HttpException> {
    let ip = client_ip(request);
    if password_rate_limited(&ip) {
        audit_log(AuditEvent::LoginFailure, &body.provider, "rate_limited", &ip);
        return Err(HttpException::new(429, "Too many login attempts. Try again shortly."));
    }

    let p = get_provider(&body.provider);
    let p = match p {
        Some(pp) if pp.supports_password => pp,
        _ => {
            audit_log(AuditEvent::LoginFailure, &body.provider, "unknown_password_provider", &ip);
            return Err(HttpException::new(404, "Unknown provider"));
        }
    };

    // Native-app branch discriminator — mirrors lines 748-781
    let mut broker_state = String::new();
    let mut cookie_provider = String::new();
    let pkce_raw = read_pkce_cookie(request);
    if !pkce_raw.is_empty() {
        let pkce_parts: HashMap<String, String> = pkce_raw.split(';').filter(|s| s.contains('=')).filter_map(|seg| {
            let mut kv = seg.splitn(2, '=');
            Some((kv.next()?.trim().to_string(), kv.next()?.trim().to_string()))
        }).collect();
        broker_state = pkce_parts.get("broker").cloned().unwrap_or_default();
        cookie_provider = pkce_parts.get("provider").cloned().unwrap_or_default();
    }
    if !broker_state.is_empty() && cookie_provider != body.provider {
        audit_log(AuditEvent::NativeTokenFailure, &body.provider, "provider_mismatch", &ip);
        return Err(HttpException::new(400, "This native sign-in was started for a different provider; use that provider's form or restart sign-in."));
    }

    // Mirrors lines 783-807: p.complete_password_login
    let session = complete_password_login_stub(&p, &body.username, &body.password).map_err(|e| {
        match e {
            PasswordLoginError::InvalidCredentials => {
                audit_log(AuditEvent::LoginFailure, &body.provider, "invalid_credentials", &ip);
                HttpException::new(401, "Invalid credentials")
            }
            PasswordLoginError::NotImplemented => HttpException::new(500, "Provider misconfigured"),
            PasswordLoginError::ProviderUnreachable(msg) => {
                audit_log(AuditEvent::LoginFailure, &body.provider, "provider_unreachable", &ip);
                HttpException::new(503, format!("Provider unreachable: {msg}"))
            }
        }
    })?;

    audit_log(AuditEvent::LoginSuccess, &body.provider, "", &ip);

    // Native branch — mirrors lines 820-858
    if !broker_state.is_empty() {
        let pending = native_flow::get_pending(&broker_state).map_err(|_| {
            audit_log(AuditEvent::NativeTokenFailure, &body.provider, "pending_not_found", &ip);
            HttpException::new(400, "Native login expired or unknown; restart sign-in.")
        })?;
        let gw_code = native_flow::complete_pending(&broker_state, session.clone()).map_err(|_| {
            audit_log(AuditEvent::NativeTokenFailure, &body.provider, "pending_not_found", &ip);
            HttpException::new(400, "Native login expired or unknown; restart sign-in.")
        })?;
        let sep = if pending.redirect_uri.contains('?') { "&" } else { "?" };
        let loopback = format!("{}{}code={}&state={}", pending.redirect_uri, sep, url_encode(&gw_code), url_encode(&pending.client_state));
        audit_log(AuditEvent::NativeCodeIssued, &body.provider, "", &ip);
        let mut resp = JsonResponse { body: format!(r#"{{"ok":true,"next":"{}"}}"#, json_escape(&loopback)), status_code: 200, headers: HashMap::new() };
        clear_pkce_cookie_json(&mut resp, &prefix(request));
        return Ok(resp);
    }

    let expires_in = std::cmp::max(60, session.expires_at - current_unix_secs() as i64);
    let landing = {
        let v = validate_post_login_target(&body.next);
        if v.is_empty() { "/".to_string() } else { v }
    };
    let mut resp = JsonResponse { body: format!(r#"{{"ok":true,"next":"{}"}}"#, json_escape(&landing)), status_code: 200, headers: HashMap::new() };
    set_session_cookies_json(&mut resp, &session.access_token, &session.refresh_token, expires_in, detect_https(request), &prefix(request), &session.provider);
    Ok(resp)
}

enum PasswordLoginError {
    InvalidCredentials,
    NotImplemented,
    ProviderUnreachable(String),
}
fn complete_password_login_stub(_p: &Provider, _username: &str, _password: &str) -> Result<Session, PasswordLoginError> {
    Err(PasswordLoginError::ProviderUnreachable("stub: complete_password_login not wired in slice1".to_string()))
}

// ---------------------------------------------------------------------------
// Public: /auth/logout — mirrors lines 875-903 (slice1 through clear_pkce_cookie at 902)
// ---------------------------------------------------------------------------

/// Mirrors `@router.post("/auth/logout", name="auth_logout")` at lines 875-903.
/// Slice1 covers through `clear_pkce_cookie` (line 902); tail `return resp`
/// and subsequent auth-required routes continue in slice 2.
pub fn auth_logout(request: &Request) -> RedirectResponse {
    let (_at, rt) = read_session_cookies(request);
    if !rt.is_empty() {
        // Best-effort revoke — mirrors lines 882-889
        for provider in list_providers() {
            // Mirrors `provider.revoke_session(refresh_token=rt)` with BLE001 swallow
            let _ = revoke_session_stub(&provider, &rt);
        }
    }

    let sess = request.state_session.clone();
    let provider_name = sess.as_ref().map(|s| s.provider.clone()).unwrap_or_else(|| "unknown".to_string());
    let user_id = sess.as_ref().map(|s| s.user_id.clone()).unwrap_or_default();
    audit_log(AuditEvent::Logout, &provider_name, &user_id, &client_ip(request));

    let prefix_val = prefix(request);
    let mut resp = RedirectResponse { url: format!("{prefix_val}/login"), status_code: 302, headers: HashMap::new(), cookies: Vec::new() };
    clear_session_cookies(&mut resp, &prefix_val);
    clear_pkce_cookie(&mut resp, &prefix_val);
    resp
}

fn revoke_session_stub(_provider: &Provider, _refresh_token: &str) -> Result<(), String> {
    // Mirrors `provider.revoke_session` — best-effort, never raises in caller
    Ok(())
}

// ---------------------------------------------------------------------------
// Slice boundary — line 900
// ---------------------------------------------------------------------------
// Python `routes.py` lines 901-1097 continue in `dashboard_routes_slice2.rs`:
//   - `auth_logout` tail `return resp` (903) already included above for
//     completeness — slice 2 re-exports it for continuity
//   - `GET /api/auth/me` (911-924), `POST /api/auth/ws-ticket` (932-961),
//     `POST /auth/native/token` (974-1019), `POST /auth/native/refresh` (1027-1097)
// This file intentionally stops at the 900-line boundary so that `cargo` is
// never invoked and the 2-slice decomposition stays clean.
