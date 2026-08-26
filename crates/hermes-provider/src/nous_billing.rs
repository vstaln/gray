//! Nous Portal Remote Spending HTTP client (Phase 2b).
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/nous_billing.py` (675 lines).
//!
//! Thin, fail-loud client for the four `/api/billing/*` endpoints the terminal
//! billing screens drive. Companion to `nous_account.rs` (which owns read-only
//! entitlement/balance) — this module owns the *write* side: buy credits, poll
//! a charge, configure auto-reload.
//!
//! Design rules (mirrors Python module docstring ll.8-24):
//! - **Money is decimal, never float.** Server emits decimal STRINGS (`"142.5"`)
//!   — we parse with decimal (here `f64` for transport; caller keeps string when
//!   precision matters) and never round-trip through lossy paths silently.
//! - **This client raises typed errors; it does NOT fail open.** Fail-open is the
//!   caller's job (the `agent/billing_view.py` builders) so each surface can
//!   decide how to degrade.
//! - **Auth** = the OAuth bearer JWT Hermes already holds for inference
//!   (`get_provider_auth_state("nous")["access_token"]`). No API-key auth.
//! - **Portal base URL** resolves with same precedence as device-flow login
//!   (`auth.py`): `HERMES_PORTAL_BASE_URL` → `NOUS_PORTAL_BASE_URL` → stored
//!   auth-state `portal_base_url` → registry default.
//!
//! T0036 — 1:1 port, no cargo (NEVER cargo).
//!
//! Notes on fidelity vs. Rust idioms:
//! - Python `decimal.Decimal` money strings ↔ `f64` for transport + `String` raw
//!   preservation; `float(x)` coercion ↔ `f64` parse with `is_finite` guard.
//! - Python `urllib.request` + `urllib.error` ↔ `curl`-backed stub (std-only) +
//!   hand-rolled JSON helpers; status + `Retry-After` header captured via `curl -D`.
//! - Python `class BillingError(Exception)` hierarchy ↔ `BillingError` struct +
//!   `BillingErrorKind` enum; `isinstance` checks ↔ `kind ==` / helper predicates.
//!   Inheritance (`BillingSessionRevoked(BillingAuthError)`, siblings under
//!   `BillingTransient`) is preserved via `kind` discriminant.
//! - Python `Optional[dict[str, Any]]` payloads ↔ `HashMap<String, Value>` with
//!   `Value` enum (std-only `serde_json::Value` stand-in).
//! - Python `urllib.parse.urljoin` / `quote` ↔ minimal `absolutize` + `percent_encode`
//!   (unreserved `A-Za-z0-9-._~`); `json.loads`/`dumps` ↔ hand-rolled extractors +
//!   `format!`-built JSON for the fixed endpoint bodies.
//! - Python `threading` + global tuple token cache ↔ `OnceLock<Mutex<Option<...>>>` + `Instant`.
//! - Python `agent.retry_utils.parse_retry_after_seconds` ↔ `retry_after_seconds`
//!   (integer + HTTP-date + clamp) std-only re-impl.
//! - Python `get_provider_auth_state` / `resolve_nous_access_token` / `AuthError`
//!   ↔ stubs reading env/pool files; canonical impls live in `hermes-cli` crate.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Constants — mirrors ll.35-44
// ---------------------------------------------------------------------------

/// Mirrors `DEFAULT_PORTAL_BASE_URL = "https://portal.nousresearch.com"` (l.35).
pub const DEFAULT_PORTAL_BASE_URL: &str = "https://portal.nousresearch.com";

/// Mirrors `DEFAULT_TIMEOUT = 15.0` (l.39).
pub const DEFAULT_TIMEOUT_SECS: f64 = 15.0;

/// Mirrors `BILLING_MANAGE_SCOPE = "billing:manage"` (l.44).
pub const BILLING_MANAGE_SCOPE: &str = "billing:manage";

// ---------------------------------------------------------------------------
// Minimal Value — mirrors `Any` dict payloads for 1:1 coercion (std-only)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    Int(i64),
    String(String),
    Array(Vec<Value>),
    Object(HashMap<String, Value>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Number(n) => Some(*n as i64),
            _ => None,
        }
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
}

// ---------------------------------------------------------------------------
// Typed errors — mirrors ll.52-165
// ---------------------------------------------------------------------------

/// Discriminant for the Python exception hierarchy (ll.52-165).
/// Python inheritance is preserved as kind values:
///
/// - `BillingError` → `Generic`
/// - `BillingScopeRequired(BillingError)` → `ScopeRequired`
/// - `BillingAuthError(BillingError)` → `AuthError`
/// - `BillingRemoteSpendingRevoked(BillingError)` → `RemoteSpendingRevoked`
/// - `BillingSessionRevoked(BillingAuthError)` → `SessionRevoked`
/// - `BillingTransient(BillingError)` → `Transient`
/// - `BillingRateLimited(BillingTransient)` → `RateLimited`
/// - `BillingStripeUnavailable(BillingTransient)` → `StripeUnavailable`
/// - `BillingUpgradeCapExceeded(BillingTransient)` → `UpgradeCapExceeded`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BillingErrorKind {
    Generic,
    ScopeRequired,
    AuthError,
    RemoteSpendingRevoked,
    SessionRevoked,
    Transient,
    RateLimited,
    StripeUnavailable,
    UpgradeCapExceeded,
}

impl BillingErrorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            BillingErrorKind::Generic => "BillingError",
            BillingErrorKind::ScopeRequired => "BillingScopeRequired",
            BillingErrorKind::AuthError => "BillingAuthError",
            BillingErrorKind::RemoteSpendingRevoked => "BillingRemoteSpendingRevoked",
            BillingErrorKind::SessionRevoked => "BillingSessionRevoked",
            BillingErrorKind::Transient => "BillingTransient",
            BillingErrorKind::RateLimited => "BillingRateLimited",
            BillingErrorKind::StripeUnavailable => "BillingStripeUnavailable",
            BillingErrorKind::UpgradeCapExceeded => "BillingUpgradeCapExceeded",
        }
    }
}

/// Mirrors `class BillingError(Exception)` (ll.52-86).
///
/// Carries everything a surface needs to render the right message + affordance:
/// the server `error` code, HTTP `status`, optional human `message`, the
/// `portalUrl` deep-link, `retry_after` seconds (429/503), `payload` full
/// parsed JSON body when available, plus Remote-Spending contract extras
/// (`actor`, `code`, `recovery`) — additive, absent on older NAS.
#[derive(Debug, Clone)]
pub struct BillingError {
    pub message: String,
    pub status: Option<i32>,
    pub error: Option<String>,
    pub portal_url: Option<String>,
    pub retry_after: Option<i64>,
    pub payload: HashMap<String, Value>,
    pub actor: Option<String>,
    pub code: Option<String>,
    pub recovery: Option<String>,
    pub kind: BillingErrorKind,
}

impl BillingError {
    pub fn new(
        message: impl Into<String>,
        status: Option<i32>,
        error: Option<String>,
        portal_url: Option<String>,
        retry_after: Option<i64>,
        payload: Option<HashMap<String, Value>>,
        actor: Option<String>,
        code: Option<String>,
        recovery: Option<String>,
        kind: BillingErrorKind,
    ) -> Self {
        Self {
            message: message.into(),
            status,
            error,
            portal_url,
            retry_after,
            payload: payload.unwrap_or_default(),
            actor,
            code,
            recovery,
            kind,
        }
    }

    // ---- kind predicates — mirrors `isinstance(exc, BillingXxx)` ----

    pub fn is_scope_required(&self) -> bool {
        self.kind == BillingErrorKind::ScopeRequired
    }
    pub fn is_auth_error(&self) -> bool {
        matches!(
            self.kind,
            BillingErrorKind::AuthError | BillingErrorKind::SessionRevoked
        )
    }
    pub fn is_session_revoked(&self) -> bool {
        self.kind == BillingErrorKind::SessionRevoked
    }
    pub fn is_remote_spending_revoked(&self) -> bool {
        self.kind == BillingErrorKind::RemoteSpendingRevoked
    }
    pub fn is_transient(&self) -> bool {
        matches!(
            self.kind,
            BillingErrorKind::Transient
                | BillingErrorKind::RateLimited
                | BillingErrorKind::StripeUnavailable
                | BillingErrorKind::UpgradeCapExceeded
        )
    }
    pub fn is_rate_limited(&self) -> bool {
        self.kind == BillingErrorKind::RateLimited
    }
    pub fn is_stripe_unavailable(&self) -> bool {
        self.kind == BillingErrorKind::StripeUnavailable
    }
    pub fn is_upgrade_cap_exceeded(&self) -> bool {
        self.kind == BillingErrorKind::UpgradeCapExceeded
    }
}

impl std::fmt::Display for BillingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for BillingError {}

// Type aliases preserving Python class names for 1:1 audit (all are BillingError).
pub type BillingScopeRequired = BillingError;
pub type BillingAuthError = BillingError;
pub type BillingRemoteSpendingRevoked = BillingError;
pub type BillingSessionRevoked = BillingError;
pub type BillingTransient = BillingError;
pub type BillingRateLimited = BillingError;
pub type BillingStripeUnavailable = BillingError;
pub type BillingUpgradeCapExceeded = BillingError;

// ---------------------------------------------------------------------------
// Base-URL + auth resolution — mirrors ll.173-204
// ---------------------------------------------------------------------------

/// Mirrors `resolve_portal_base_url(state=None)` (ll.173-186).
pub fn resolve_portal_base_url(state: Option<&HashMap<String, Value>>) -> String {
    let env_val = env::var("HERMES_PORTAL_BASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| env::var("NOUS_PORTAL_BASE_URL").ok().filter(|s| !s.trim().is_empty()));
    if let Some(v) = env_val {
        let t = v.trim().to_string();
        if !t.is_empty() {
            return t.trim_end_matches('/').to_string();
        }
    }
    if let Some(s) = state {
        if let Some(Value::String(stored)) = s.get("portal_base_url") {
            let t = stored.trim();
            if !t.is_empty() {
                return t.trim_end_matches('/').to_string();
            }
        }
    }
    DEFAULT_PORTAL_BASE_URL.to_string()
}

#[allow(dead_code)]
fn _resolve_portal_base_url(state: Option<&HashMap<String, Value>>) -> String {
    resolve_portal_base_url(state)
}

/// Mirrors `_absolutize_portal_url(portal_url)` (ll.189-203).
pub fn absolutize_portal_url(portal_url: Option<&str>) -> Option<String> {
    let url = portal_url?;
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return portal_url.map(|s| s.to_string());
    }
    // Already absolute (has scheme) → return unchanged (urljoin keeps it).
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Some(trimmed.to_string());
    }
    let base = resolve_portal_base_url(None);
    let base = base.trim_end_matches('/').to_string() + "/";
    // Python: urllib.parse.urljoin(base + "/", portal_url)
    // If portal_url is absolute path like "/billing?topup=open", urljoin yields base host + path.
    // Emulate: base host + portal_url (which already starts with "/") or base + "/" + relative.
    if trimmed.starts_with('/') {
        // Extract scheme+host from base
        if let Some(scheme_end) = base.find("://") {
            let after_scheme = &base[scheme_end + 3..];
            if let Some(slash) = after_scheme.find('/') {
                let host_part = &base[..scheme_end + 3 + slash];
                return Some(format!("{}{}", host_part.trim_end_matches('/'), trimmed));
            }
        }
        return Some(format!("{}{}", base.trim_end_matches('/'), trimmed));
    }
    Some(format!("{}{}", base, trimmed))
}

#[allow(dead_code)]
fn _absolutize_portal_url(portal_url: Option<&str>) -> Option<String> {
    absolutize_portal_url(portal_url)
}

// ---------------------------------------------------------------------------
// Token cache — mirrors ll.213-229
// ---------------------------------------------------------------------------

/// Mirrors `_TOKEN_CACHE_TTL_SECONDS = 30.0` (l.213).
pub const TOKEN_CACHE_TTL_SECS: f64 = 30.0;
pub const TOKEN_CACHE_TTL: Duration = Duration::from_secs(30);

static TOKEN_CACHE: OnceLock<Mutex<Option<(Instant, String, String)>>> = OnceLock::new();

fn token_cache() -> &'static Mutex<Option<(Instant, String, String)>> {
    TOKEN_CACHE.get_or_init(|| Mutex::new(None))
}

/// Mirrors `invalidate_cached_token()` (ll.217-228).
pub fn invalidate_cached_token() {
    if let Ok(mut g) = token_cache().lock() {
        *g = None;
    }
}

#[allow(dead_code)]
fn _invalidate_cached_token() {
    invalidate_cached_token()
}

/// Mirrors `_billing_not_logged_in(exc=None)` (ll.231-240).
pub fn billing_not_logged_in() -> BillingError {
    BillingError::new(
        "Not logged into Nous Portal \u{2014} run `hermes portal` to log in.",
        Some(401),
        Some("invalid_token".to_string()),
        None,
        None,
        None,
        None,
        None,
        None,
        BillingErrorKind::AuthError,
    )
}

fn billing_not_logged_in_with_cause(cause: Option<String>) -> BillingError {
    let mut err = billing_not_logged_in();
    // Preserve cause via message suffix for std-only (Python sets __cause__).
    if let Some(c) = cause {
        if !c.trim().is_empty() {
            err.message = format!("{} ({})", err.message, c.trim());
        }
    }
    err
}

#[allow(dead_code)]
fn _billing_not_logged_in() -> BillingError {
    billing_not_logged_in()
}

// ---------------------------------------------------------------------------
// Auth state helpers — mirrors `hermes_cli.auth` stubs (also in nous_account.rs)
// ---------------------------------------------------------------------------

pub type AuthState = HashMap<String, Value>;

fn auth_state_get_str(state: &AuthState, key: &str) -> Option<String> {
    state.get(key).and_then(|v| match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    })
}

/// Stub: mirrors `hermes_cli.auth.get_provider_auth_state("nous")`.
/// Real impl lives in `hermes-cli` crate; this stub reads env / `HERMES_HOME/auth.json`.
pub fn get_provider_auth_state(provider: &str) -> Option<AuthState> {
    if provider != "nous" {
        return None;
    }
    let mut state: AuthState = HashMap::new();
    let mut has_any = false;
    for (env_key, state_key) in [
        ("NOUS_ACCESS_TOKEN", "access_token"),
        ("NOUS_PORTAL_BASE_URL", "portal_base_url"),
        ("NOUS_CLIENT_ID", "client_id"),
        ("NOUS_INFERENCE_BASE_URL", "inference_base_url"),
        ("NOUS_AGENT_KEY", "agent_key"),
        ("NOUS_CREDENTIAL_SOURCE", "credential_source"),
    ] {
        if let Ok(v) = env::var(env_key) {
            let t = v.trim().to_string();
            if !t.is_empty() {
                state.insert(state_key.to_string(), Value::String(t));
                has_any = true;
            }
        }
    }
    if !has_any {
        if let Ok(home) = env::var("HERMES_HOME") {
            let p = Path::new(&home).join("auth.json");
            if let Ok(text) = fs::read_to_string(&p) {
                if let Some(tok) = extract_json_string(&text, "access_token") {
                    if !tok.trim().is_empty() {
                        state.insert("access_token".to_string(), Value::String(tok));
                        has_any = true;
                    }
                }
                if let Some(url) = extract_json_string(&text, "portal_base_url") {
                    if !url.trim().is_empty() {
                        state.insert("portal_base_url".to_string(), Value::String(url));
                    }
                }
            }
        }
    }
    if has_any { Some(state) } else { None }
}

/// Stub: mirrors `hermes_cli.auth.resolve_nous_access_token()`.
pub fn resolve_nous_access_token() -> Result<String, String> {
    if let Some(state) = get_provider_auth_state("nous") {
        if let Some(tok) = auth_state_get_str(&state, "access_token") {
            if !tok.trim().is_empty() {
                return Ok(tok);
            }
        }
    }
    if let Ok(v) = env::var("NOUS_ACCESS_TOKEN") {
        let t = v.trim().to_string();
        if !t.is_empty() {
            return Ok(t);
        }
    }
    Err("no nous access token".to_string())
}

/// Mirrors `_resolve_token_and_base(*, use_cache=True)` (ll.243-290).
pub fn resolve_token_and_base(use_cache: bool) -> Result<(String, String), BillingError> {
    if use_cache {
        if let Ok(g) = token_cache().lock() {
            if let Some((cached_at, token, base)) = g.as_ref() {
                if cached_at.elapsed() < TOKEN_CACHE_TTL {
                    return Ok((token.clone(), base.clone()));
                }
            }
        }
    }

    let state = match get_provider_auth_state("nous") {
        Some(s) => s,
        None => HashMap::new(),
    };
    let base = resolve_portal_base_url(if state.is_empty() { None } else { Some(&state) });

    // Try refresh-aware resolver; fall back to raw stored token.
    let token = match resolve_nous_access_token() {
        Ok(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => {
            if let Some(tok) = state.get("access_token").and_then(|v| match v {
                Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
                _ => None,
            }) {
                tok
            } else {
                return Err(billing_not_logged_in());
            }
        }
    };

    let resolved = (token.clone(), base.clone());
    if let Ok(mut g) = token_cache().lock() {
        *g = Some((Instant::now(), token.clone(), base.clone()));
    }
    Ok((token, base))
}

#[allow(dead_code)]
fn _resolve_token_and_base(use_cache: bool) -> Result<(String, String), BillingError> {
    resolve_token_and_base(use_cache)
}

// ---------------------------------------------------------------------------
// HTTP plumbing — mirrors ll.298-468
// ---------------------------------------------------------------------------

/// Mirrors `_retry_after_seconds(headers)` (ll.298-307).
///
/// Thin wrapper around `agent.retry_utils.parse_retry_after_seconds`
/// (shared parser also handles HTTP-date forms and clamps negatives).
pub fn retry_after_seconds(headers: &HashMap<String, String>) -> Option<i64> {
    parse_retry_after_seconds(headers)
}

fn parse_retry_after_seconds(headers: &HashMap<String, String>) -> Option<i64> {
    // Lookup case-insensitively
    let mut raw: Option<&str> = None;
    for (k, v) in headers {
        if k.to_ascii_lowercase() == "retry-after" {
            raw = Some(v.as_str());
            break;
        }
    }
    let s = raw?.trim();
    if s.is_empty() {
        return None;
    }
    // Integer seconds form — clamp negative to 0 downstream via max(0)
    if let Ok(n) = s.parse::<i64>() {
        let clamped = n.max(0);
        return Some(clamped);
    }
    if let Ok(f) = s.parse::<f64>() {
        if f.is_finite() {
            return Some((f as i64).max(0));
        }
    }
    // HTTP-date form: try parse as IMF-fixdate / RFC1123, then delta from now.
    // Mirrors `retry_utils` which handles date → seconds. We compute max(0, date - now).
    if let Some(ts) = parse_http_date(s) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let delta = ts - now;
        return Some(delta.max(0));
    }
    None
}

fn parse_http_date(s: &str) -> Option<i64> {
    // Handles `Sun, 06 Nov 1994 08:49:37 GMT` (RFC1123) and variants.
    // Minimal: strip weekday, parse `DD Mon YYYY HH:MM:SS GMT`.
    let s = s.trim();
    // Find comma → after it is date part
    let date_part = if let Some(comma) = s.find(',') {
        s[comma + 1..].trim()
    } else {
        s
    };
    // Expect `06 Nov 1994 08:49:37 GMT`
    let mut parts = date_part.split_whitespace();
    let day: u32 = parts.next()?.parse().ok()?;
    let mon_str = parts.next()?;
    let year: i32 = parts.next()?.parse().ok()?;
    let time_str = parts.next()?;
    // parts.next() is GMT — ignore
    let mon = match mon_str.to_ascii_lowercase().as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    };
    let tparts: Vec<&str> = time_str.split(':').collect();
    if tparts.len() != 3 {
        return None;
    }
    let hh: u32 = tparts[0].parse().ok()?;
    let mm: u32 = tparts[1].parse().ok()?;
    let ss: u32 = tparts[2].parse().ok()?;
    let days = days_since_epoch(year, mon, day)?;
    let secs = days * 86400 + (hh as i64) * 3600 + (mm as i64) * 60 + (ss as i64);
    Some(secs)
}

fn days_since_epoch(year: i32, month: u32, day: u32) -> Option<i64> {
    let mut y = year as i64;
    let mut m = month as i64;
    let d = day as i64;
    if m <= 2 {
        y -= 1;
        m += 12;
    }
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m - 3) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146097 + doe - 719468)
}

#[allow(dead_code)]
fn _retry_after_seconds(headers: &HashMap<String, String>) -> Option<i64> {
    retry_after_seconds(headers)
}

/// Mirrors `_raise_for_error(status, payload, headers=None)` (ll.310-379).
pub fn raise_for_error(
    status: i32,
    payload: &HashMap<String, Value>,
    headers: Option<&HashMap<String, String>>,
) -> Result<(), BillingError> {
    let error = payload.get("error").and_then(|v| v.as_str()).map(|s| s.to_string());
    let message = payload.get("message").and_then(|v| v.as_str()).map(|s| s.to_string());
    let code = payload.get("code").and_then(|v| v.as_str()).map(|s| s.to_string());
    let actor = payload.get("actor").and_then(|v| v.as_str()).map(|s| s.to_string());
    let recovery = payload.get("recovery").and_then(|v| v.as_str()).map(|s| s.to_string());
    let portal_url = absolutize_portal_url(
        payload.get("portalUrl").and_then(|v| v.as_str()),
    );
    let retry_after = headers.and_then(|h| retry_after_seconds(h));

    let common_payload = payload.clone();

    // Helper to build BillingError with shared fields
    let mk = |msg: String, kind: BillingErrorKind| BillingError::new(
        msg,
        Some(status),
        error.clone(),
        portal_url.clone(),
        retry_after,
        Some(common_payload.clone()),
        actor.clone(),
        code.clone(),
        recovery.clone(),
        kind,
    );

    if error.as_deref() == Some("stripe_unavailable") {
        return Err(mk(
            message.unwrap_or_else(|| "Stripe is temporarily unavailable \u{2014} try again shortly.".to_string()),
            BillingErrorKind::StripeUnavailable,
        ));
    }
    if error.as_deref() == Some("upgrade_cap_exceeded") {
        return Err(mk(
            message.unwrap_or_else(|| "Daily plan-change limit reached \u{2014} try again tomorrow.".to_string()),
            BillingErrorKind::UpgradeCapExceeded,
        ));
    }

    if status == 401 {
        if error.as_deref() == Some("session_revoked") {
            return Err(mk(
                message.unwrap_or_else(|| "Your session was logged out \u{2014} log in again.".to_string()),
                BillingErrorKind::SessionRevoked,
            ));
        }
        return Err(mk(
            message.unwrap_or_else(|| "Authentication required.".to_string()),
            BillingErrorKind::AuthError,
        ));
    }
    if status == 403 {
        if error.as_deref() == Some("remote_spending_revoked") {
            return Err(mk(
                message.unwrap_or_else(|| "Remote spending was stopped for this terminal.".to_string()),
                BillingErrorKind::RemoteSpendingRevoked,
            ));
        }
        if error.as_deref() == Some("insufficient_scope") {
            return Err(mk(
                message.unwrap_or_else(|| "This action needs the billing:manage scope.".to_string()),
                BillingErrorKind::ScopeRequired,
            ));
        }
        return Err(mk(
            message.clone().or_else(|| error.clone()).unwrap_or_else(|| "Billing request denied.".to_string()),
            BillingErrorKind::Generic,
        ));
    }
    if status == 429 || status == 503 {
        return Err(mk(
            message.unwrap_or_else(|| "Rate limited \u{2014} try again shortly.".to_string()),
            BillingErrorKind::RateLimited,
        ));
    }
    Err(mk(
        message.or_else(|| error.clone()).unwrap_or_else(|| format!("Billing request failed ({}).", status)),
        BillingErrorKind::Generic,
    ))
}

#[allow(dead_code)]
fn _raise_for_error(status: i32, payload: &HashMap<String, Value>, headers: Option<&HashMap<String, String>>) -> Result<(), BillingError> {
    raise_for_error(status, payload, headers)
}

// ---------------------------------------------------------------------------
// _request — mirrors ll.382-468 (curl-backed, std-only)
// ---------------------------------------------------------------------------

/// Mirrors `_request(method, path, *, body=None, extra_headers=None, timeout=DEFAULT_TIMEOUT, _retried_auth=False)` (ll.382-468).
pub fn request(
    method: &str,
    path: &str,
    body: Option<&str>,
    extra_headers: Option<&HashMap<String, String>>,
    timeout: f64,
    retried_auth: bool,
) -> Result<HashMap<String, Value>, BillingError> {
    let (token, base) = resolve_token_and_base(!retried_auth).map_err(|e| e)?;

    let url = format!("{}{}", base.trim_end_matches('/'), path);
    let timeout_secs = if timeout.is_finite() && timeout > 0.0 { timeout } else { DEFAULT_TIMEOUT_SECS };

    // Build header map for curl -H flags + for retry_after capture
    let mut headers: HashMap<String, String> = HashMap::new();
    headers.insert("Authorization".to_string(), format!("Bearer {}", token));
    headers.insert("Accept".to_string(), "application/json".to_string());
    if body.is_some() {
        headers.insert("Content-Type".to_string(), "application/json".to_string());
    }
    if let Some(extra) = extra_headers {
        for (k, v) in extra {
            headers.insert(k.clone(), v.clone());
        }
    }

    match curl_request(method, &url, &headers, body, timeout_secs) {
        Ok((status, resp_headers, body_text)) => {
            if (200..300).contains(&status) {
                let trimmed = body_text.trim();
                if trimmed.is_empty() {
                    return Ok(HashMap::new());
                }
                match parse_json_object(trimmed) {
                    Some(map) => return Ok(map),
                    None => {
                        // Try to detect non-JSON 2xx (reverse-proxy / SPA fallback HTML)
                        // Mirrors ll.418-431
                        if looks_like_json(trimmed) {
                            // JSON parse failed but looks like JSON — surface as endpoint_unavailable
                            return Err(BillingError::new(
                                "Billing endpoint returned a non-JSON response (it may not be available on this deployment).",
                                Some("endpoint_unavailable".to_string()).is_some().then_some(status),
                                Some("endpoint_unavailable".to_string()),
                                None,
                                None,
                                None,
                                None,
                                None,
                                None,
                                BillingErrorKind::Generic,
                            ));
                        } else {
                            // Not JSON at all — same error but with status from resp
                            return Err(BillingError::new(
                                "Billing endpoint returned a non-JSON response (it may not be available on this deployment).",
                                Some(status),
                                Some("endpoint_unavailable".to_string()),
                                None,
                                None,
                                None,
                                None,
                                None,
                                None,
                                BillingErrorKind::Generic,
                            ));
                        }
                    }
                }
            } else {
                // 401 on cached token → drop cache and retry once (ll.435-445)
                if status == 401 && !retried_auth {
                    invalidate_cached_token();
                    return request(method, path, body, extra_headers, timeout, true);
                }
                // Non-2xx: parse payload + raise typed error (ll.446-455)
                let payload = if body_text.trim().is_empty() {
                    HashMap::new()
                } else {
                    parse_json_object(body_text.trim()).unwrap_or_default()
                };
                // _raise_for_error always returns Err; propagate
                return match raise_for_error(status, &payload, Some(&resp_headers)) {
                    Ok(()) => Err(BillingError::new(
                        format!("Billing request failed ({}).", status),
                        Some(status),
                        None,
                        None,
                        None,
                        Some(payload),
                        None,
                        None,
                        None,
                        BillingErrorKind::Generic,
                    )),
                    Err(e) => Err(e),
                };
            }
        }
        Err(e) if e.kind == BillingErrorKind::Generic && e.error.as_deref() == Some("network_error") => {
            // Already a BillingError from curl transport — pass through
            // Check 401 retry already handled via status path; here it's URLError/timeout case
            return Err(e);
        }
        Err(e) => return Err(e),
    }

    // Unreachable — _raise_for_error always raises
    #[allow(unreachable_code)]
    Ok(HashMap::new())
}

#[allow(dead_code)]
fn _request(
    method: &str,
    path: &str,
    body: Option<&str>,
    extra_headers: Option<&HashMap<String, String>>,
    timeout: f64,
    retried_auth: bool,
) -> Result<HashMap<String, Value>, BillingError> {
    request(method, path, body, extra_headers, timeout, retried_auth)
}

/// Execute an HTTP request via `curl` (std-only, no reqwest).
/// Returns `(status_code, response_headers, body_text)` on success,
/// or `BillingError` with `error="network_error"` on transport failure.
fn curl_request(
    method: &str,
    url: &str,
    headers: &HashMap<String, String>,
    body: Option<&str>,
    timeout: f64,
) -> Result<(i32, HashMap<String, String>, String), BillingError> {
    let mut tmp_dir = env::temp_dir();
    tmp_dir.push(format!("hermes-billing-{}-{}", std::process::id(), Instant::now().elapsed().as_millis()));
    let _ = fs::create_dir_all(&tmp_dir);
    let hdr_path = tmp_dir.join("headers.txt");
    let body_path = tmp_dir.join("body.txt");

    let timeout_arg = format!("{}", timeout.max(1.0).ceil() as u64);
    let mut args: Vec<String> = Vec::new();
    args.push("-s".to_string());
    args.push("-S".to_string());
    args.push("--max-time".to_string());
    args.push(timeout_arg);
    args.push("-X".to_string());
    args.push(method.to_string());
    args.push("-D".to_string());
    args.push(hdr_path.to_string_lossy().to_string());
    args.push("-o".to_string());
    args.push(body_path.to_string_lossy().to_string());
    args.push("-w".to_string());
    args.push("%{http_code}".to_string());
    for (k, v) in headers {
        args.push("-H".to_string());
        args.push(format!("{}: {}", k, v));
    }
    if let Some(b) = body {
        args.push("-d".to_string());
        args.push(b.to_string());
    }
    args.push(url.to_string());

    let output = Command::new("curl").args(&args).output();

    // Cleanup helper
    let cleanup = |tmp: &PathBuf| {
        let _ = fs::remove_file(tmp.join("headers.txt"));
        let _ = fs::remove_file(tmp.join("body.txt"));
        let _ = fs::remove_dir(tmp);
    };

    match output {
        Ok(out) => {
            // Curl writes http_code to stdout; body to file.
            let http_code_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let status: i32 = http_code_str.parse().unwrap_or_else(|_| {
                // If curl failed to write http_code, treat as transport error if stderr non-empty
                if !out.status.success() { 0 } else { 200 }
            });

            // If curl itself failed (non-zero exit and no http_code), surface as network_error
            if !out.status.success() && (http_code_str.is_empty() || status == 0) {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                let reason = if !stderr.is_empty() { stderr } else { format!("curl exit {}", out.status) };
                cleanup(&tmp_dir);
                // Mirrors `except TimeoutError` / `URLError` branches (ll.457-467)
                // urlopen wraps timeouts in URLError; resp.read() bare TimeoutError → BillingError network_error
                return Err(BillingError::new(
                    format!("Could not reach Nous Portal: {}", reason),
                    None,
                    Some("network_error".to_string()),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    BillingErrorKind::Generic,
                ));
            }

            let body_text = fs::read_to_string(&body_path).unwrap_or_default();
            let hdr_text = fs::read_to_string(&hdr_path).unwrap_or_default();
            let mut resp_headers: HashMap<String, String> = HashMap::new();
            for line in hdr_text.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with("HTTP/") {
                    continue;
                }
                if let Some(colon) = trimmed.find(':') {
                    let k = trimmed[..colon].trim().to_string();
                    let v = trimmed[colon + 1..].trim().to_string();
                    resp_headers.insert(k, v);
                }
            }

            // Edge: curl --max-time timeout during body read surfaces as exit 28 with partial body;
            // if stderr mentions timeout, normalize to network_error (mirrors Python TimeoutError branch ll.461-467)
            if !out.status.success() && (status == 0 || status == 28) {
                let stderr = String::from_utf8_lossy(&out.stderr).to_ascii_lowercase();
                if stderr.contains("timed out") || stderr.contains("timeout") || stderr.contains("operation timed out") {
                    cleanup(&tmp_dir);
                    return Err(BillingError::new(
                        "Could not reach Nous Portal: timed out",
                        None,
                        Some("network_error".to_string()),
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        BillingErrorKind::Generic,
                    ));
                }
            }

            cleanup(&tmp_dir);
            Ok((status, resp_headers, body_text))
        }
        Err(e) => {
            cleanup(&tmp_dir);
            Err(BillingError::new(
                format!("Could not reach Nous Portal: {}", e),
                None,
                Some("network_error".to_string()),
                None,
                None,
                None,
                None,
                None,
                None,
                BillingErrorKind::Generic,
            ))
        }
    }
}

fn looks_like_json(s: &str) -> bool {
    let t = s.trim_start();
    t.starts_with('{') || t.starts_with('[') || t.starts_with('"')
}

// ---------------------------------------------------------------------------
// The four endpoints + subscription — mirrors ll.475-675
// ---------------------------------------------------------------------------

/// Mirrors `get_billing_state(*, timeout=DEFAULT_TIMEOUT)` (ll.475-477).
pub fn get_billing_state(timeout: f64) -> Result<HashMap<String, Value>, BillingError> {
    request("GET", "/api/billing/state", None, None, timeout, false)
}

/// Mirrors `patch_auto_top_up(*, enabled, threshold, top_up_amount, timeout=DEFAULT_TIMEOUT)` (ll.480-501).
pub fn patch_auto_top_up(
    enabled: bool,
    threshold: f64,
    top_up_amount: f64,
    timeout: f64,
) -> Result<HashMap<String, Value>, BillingError> {
    let body = format!(
        "{{\"enabled\":{},\"threshold\":{},\"topUpAmount\":{}}}",
        if enabled { "true" } else { "false" },
        format_f64(threshold),
        format_f64(top_up_amount)
    );
    request("PATCH", "/api/billing/auto-top-up", Some(&body), None, timeout, false)
}

/// String-coercing variant matching Python `float(threshold)` + `float(top_up_amount)` (ll.496-498).
pub fn patch_auto_top_up_str(
    enabled: bool,
    threshold: &str,
    top_up_amount: &str,
    timeout: f64,
) -> Result<HashMap<String, Value>, BillingError> {
    let t: f64 = threshold.trim().parse().unwrap_or(f64::NAN);
    let a: f64 = top_up_amount.trim().parse().unwrap_or(f64::NAN);
    patch_auto_top_up(enabled, t, a, timeout)
}

/// Mirrors `post_charge(*, amount_usd, idempotency_key, timeout=DEFAULT_TIMEOUT)` (ll.504-528).
pub fn post_charge(
    amount_usd: f64,
    idempotency_key: &str,
    timeout: f64,
) -> Result<HashMap<String, Value>, BillingError> {
    if idempotency_key.trim().is_empty() {
        return Err(BillingError::new(
            "Idempotency-Key is required for a charge.",
            None,
            Some("idempotency_key_required".to_string()),
            None,
            None,
            None,
            None,
            None,
            None,
            BillingErrorKind::Generic,
        ));
    }
    let body = format!("{{\"amountUsd\":{}}}", format_f64(amount_usd));
    let mut extra: HashMap<String, String> = HashMap::new();
    extra.insert("Idempotency-Key".to_string(), idempotency_key.trim().to_string());
    request("POST", "/api/billing/charge", Some(&body), Some(&extra), timeout, false)
}

/// String-coercing variant matching Python `float(amount_usd)` (l.525).
pub fn post_charge_str(
    amount_usd: &str,
    idempotency_key: &str,
    timeout: f64,
) -> Result<HashMap<String, Value>, BillingError> {
    let a: f64 = amount_usd.trim().parse().unwrap_or(f64::NAN);
    post_charge(a, idempotency_key, timeout)
}

/// Mirrors `get_charge_status(charge_id, *, timeout=DEFAULT_TIMEOUT)` (ll.531-545).
pub fn get_charge_status(
    charge_id: &str,
    timeout: f64,
) -> Result<HashMap<String, Value>, BillingError> {
    if charge_id.trim().is_empty() {
        return Err(BillingError::new(
            "A charge id is required.",
            None,
            Some("invalid_charge_id".to_string()),
            None,
            None,
            None,
            None,
            None,
            None,
            BillingErrorKind::Generic,
        ));
    }
    let safe_id = percent_encode(charge_id.trim());
    let path = format!("/api/billing/charge/{}", safe_id);
    request("GET", &path, None, None, timeout, false)
}

/// Mirrors `get_subscription_state(*, timeout=DEFAULT_TIMEOUT)` (ll.548-555).
pub fn get_subscription_state(timeout: f64) -> Result<HashMap<String, Value>, BillingError> {
    request("GET", "/api/billing/subscription", None, None, timeout, false)
}

/// Mirrors `post_subscription_preview(*, subscription_type_id, timeout=DEFAULT_TIMEOUT)` (ll.573-591).
pub fn post_subscription_preview(
    subscription_type_id: &str,
    timeout: f64,
) -> Result<HashMap<String, Value>, BillingError> {
    let body = format!(
        "{{\"subscriptionTypeId\":{}}}",
        json_escape_str(subscription_type_id)
    );
    request(
        "POST",
        "/api/billing/subscription/preview",
        Some(&body),
        None,
        timeout,
        false,
    )
}

/// Mirrors `put_subscription_pending_change(*, subscription_type_id=None, cancel=False, timeout=DEFAULT_TIMEOUT)` (ll.594-628).
pub fn put_subscription_pending_change(
    subscription_type_id: Option<&str>,
    cancel: bool,
    timeout: f64,
) -> Result<HashMap<String, Value>, BillingError> {
    let body = if cancel {
        r#"{"type":"cancellation"}"#.to_string()
    } else {
        let tid = subscription_type_id.unwrap_or("").trim();
        if tid.is_empty() {
            return Err(BillingError::new(
                "A subscription tier is required to schedule a plan change.",
                None,
                Some("invalid_subscription_type".to_string()),
                None,
                None,
                None,
                None,
                None,
                None,
                BillingErrorKind::Generic,
            ));
        }
        format!(
            "{{\"type\":\"tier_change\",\"subscriptionTypeId\":{}}}",
            json_escape_str(tid)
        )
    };
    request(
        "PUT",
        "/api/billing/subscription/pending-change",
        Some(&body),
        None,
        timeout,
        false,
    )
}

/// Mirrors `delete_subscription_pending_change(*, timeout=DEFAULT_TIMEOUT)` (ll.631-645).
pub fn delete_subscription_pending_change(
    timeout: f64,
) -> Result<HashMap<String, Value>, BillingError> {
    request(
        "DELETE",
        "/api/billing/subscription/pending-change",
        None,
        None,
        timeout,
        false,
    )
}

/// Mirrors `post_subscription_upgrade(*, subscription_type_id, idempotency_key, timeout=DEFAULT_TIMEOUT)` (ll.648-674).
pub fn post_subscription_upgrade(
    subscription_type_id: &str,
    idempotency_key: &str,
    timeout: f64,
) -> Result<HashMap<String, Value>, BillingError> {
    if idempotency_key.trim().is_empty() {
        return Err(BillingError::new(
            "Idempotency-Key is required for an upgrade.",
            None,
            Some("idempotency_key_required".to_string()),
            None,
            None,
            None,
            None,
            None,
            None,
            BillingErrorKind::Generic,
        ));
    }
    let body = format!(
        "{{\"subscriptionTypeId\":{}}}",
        json_escape_str(subscription_type_id)
    );
    let mut extra: HashMap<String, String> = HashMap::new();
    extra.insert("Idempotency-Key".to_string(), idempotency_key.trim().to_string());
    request(
        "POST",
        "/api/billing/subscription/upgrade",
        Some(&body),
        Some(&extra),
        timeout,
        false,
    )
}

// ---------------------------------------------------------------------------
// Small helpers — percent_encode, format_f64, json_escape, JSON parse
// ---------------------------------------------------------------------------

fn percent_encode(input: &str) -> String {
    // Mirrors `urllib.parse.quote(charge_id.strip(), safe="")` (l.544) — encode all except unreserved.
    let mut out = String::new();
    for b in input.bytes() {
        let c = b as char;
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' {
            out.push(c);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn format_f64(v: f64) -> String {
    if !v.is_finite() {
        // Mirrors Python `float("nan")` → json dumps as NaN is invalid JSON; we emit "null" to fail server-side like Python would via JSON? Keep raw.
        return "null".to_string();
    }
    // Use minimal representation like Python's json dumps for numbers.
    let s = format!("{}", v);
    s
}

fn json_escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// Minimal JSON extraction helpers — mirrors `response.json()` / `json.loads`
// Std-only, hand-rolled (maps to real `serde_json` at merge)
// ---------------------------------------------------------------------------

fn extract_json_string(json: &str, field: &str) -> Option<String> {
    let needle = format!("\"{}\"", field);
    let idx = json.find(&needle)?;
    let after = &json[idx + needle.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    if !rest.starts_with('"') {
        return None;
    }
    parse_json_string(rest)
}

fn parse_json_string(s: &str) -> Option<String> {
    let s = s.trim();
    if !s.starts_with('"') {
        return None;
    }
    let mut out = String::new();
    let mut chars = s[1..].chars();
    let mut escape = false;
    while let Some(c) = chars.next() {
        if escape {
            match c {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Ok(code) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(code) {
                            out.push(ch);
                        }
                    }
                }
                _ => out.push(c),
            }
            escape = false;
        } else if c == '\\' {
            escape = true;
        } else if c == '"' {
            return Some(out);
        } else {
            out.push(c);
        }
    }
    None
}

/// Parse a flat JSON object `{"k": v, ...}` into `HashMap<String, Value>` for payloads.
/// Handles string, bool, null, number, nested object/array as `Value::String` sentinel for deeper parse.
fn parse_json_object(text: &str) -> Option<HashMap<String, Value>> {
    let trimmed = text.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    // Find matching brace at end
    if !trimmed.ends_with('}') {
        // Try to extract via brace matching (handles trailing whitespace)
        // Fallback: if not well-formed, hand-parse shallow object
    }
    // Use parse_shallow_object for top-level
    let inner_start = trimmed.find('{')?;
    let inner_end = find_matching_brace(&trimmed[inner_start..])?;
    let obj_text = &trimmed[inner_start..inner_start + inner_end + 1];
    Some(parse_shallow_object(obj_text))
}

fn parse_shallow_object(obj_text: &str) -> HashMap<String, Value> {
    let mut out: HashMap<String, Value> = HashMap::new();
    let inner = obj_text.trim();
    let inner = inner.strip_prefix('{').unwrap_or(inner).strip_suffix('}').unwrap_or(inner);
    for pair in split_json_pairs(inner) {
        let p = pair.trim();
        if p.is_empty() {
            continue;
        }
        let colon = match p.find(':') {
            Some(c) => c,
            None => continue,
        };
        let k_raw = p[..colon].trim();
        let v_raw = p[colon + 1..].trim();
        let key = match parse_json_string(k_raw) {
            Some(k) => k,
            None => continue,
        };
        let value = if v_raw.starts_with('"') {
            match parse_json_string(v_raw) {
                Some(s) => Value::String(s),
                None => Value::Null,
            }
        } else if v_raw == "null" {
            Value::Null
        } else if v_raw == "true" {
            Value::Bool(true)
        } else if v_raw == "false" {
            Value::Bool(false)
        } else if v_raw.starts_with('{') {
            // Nested object — parse shallow recursively for one level, else keep string
            if let Some(end) = find_matching_brace(v_raw) {
                let nested_text = &v_raw[..=end];
                let inner_map = parse_shallow_object(nested_text);
                Value::Object(inner_map)
            } else {
                Value::String(v_raw.to_string())
            }
        } else if v_raw.starts_with('[') {
            // Array — keep as string for now; caller can parse via helpers if needed
            Value::String(v_raw.to_string())
        } else {
            // Number
            if let Ok(i) = v_raw.parse::<i64>() {
                if v_raw.contains('.') || v_raw.contains('e') || v_raw.contains('E') {
                    if let Ok(f) = v_raw.parse::<f64>() {
                        Value::Number(f)
                    } else {
                        Value::Int(i)
                    }
                } else {
                    Value::Int(i)
                }
            } else if let Ok(f) = v_raw.parse::<f64>() {
                Value::Number(f)
            } else {
                Value::String(v_raw.to_string())
            }
        };
        out.insert(key, value);
    }
    out
}

fn find_matching_brace(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escape = false;
    for (i, c) in s.char_indices() {
        if in_str {
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_json_pairs(inner: &str) -> Vec<String> {
    let mut pairs = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escape = false;
    for c in inner.chars() {
        if in_str {
            cur.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                cur.push(c);
            }
            '{' | '[' => {
                depth += 1;
                cur.push(c);
            }
            '}' | ']' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                pairs.push(cur.clone());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        pairs.push(cur);
    }
    pairs
}
