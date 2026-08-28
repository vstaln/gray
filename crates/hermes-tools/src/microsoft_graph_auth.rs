//! Microsoft Graph app-only authentication helpers.
//! Port of `tools/microsoft_graph_auth.py` (245 lines) — 1:1 behavior.
//!
//! Provides [`GraphCredentials`] (tenant/client/secret + scope/authority),
//! [`CachedAccessToken`] (expiry-aware cache entry), and
//! [`MicrosoftGraphTokenProvider`] (acquire + cache app-only Graph tokens).
//!
//! Rust mapping:
//! - `DEFAULT_GRAPH_SCOPE` → [`DEFAULT_GRAPH_SCOPE`] (line 14)
//! - `DEFAULT_GRAPH_AUTHORITY_URL` → [`DEFAULT_GRAPH_AUTHORITY_URL`] (line 15)
//! - `DEFAULT_TOKEN_SKEW_SECONDS` → [`DEFAULT_TOKEN_SKEW_SECONDS`] (line 16)
//! - `MicrosoftGraphAuthError` → [`MicrosoftGraphAuthError`] (line 19)
//! - `MicrosoftGraphConfigError` → [`MicrosoftGraphConfigError`] (line 23)
//! - `MicrosoftGraphTokenError` → [`MicrosoftGraphTokenError`] (line 27)
//! - `GraphCredentials` + `token_url` + `from_env` → [`GraphCredentials`] (lines 31-85)
//! - `CachedAccessToken` + `is_expired` + `expires_in_seconds` → [`CachedAccessToken`] (lines 88-102)
//! - `MicrosoftGraphTokenProvider` + `from_env` + `clear_cache` + `inspect_token_health`
//!   + `get_access_token` + `_fetch_access_token` → [`MicrosoftGraphTokenProvider`] (lines 104-220)
//! - `_extract_error_detail` → [`extract_error_detail`] / [`extract_error_detail_from_json`]
//!   (lines 223-245)
//! - `httpx.AsyncBaseTransport` / `httpx.AsyncClient` → injectable `fetch` closure
//!   + [`TokenRequest`] / [`HttpResponse`] carriers (no SDK linked in this crate,
//!   same as `openrouter_client.rs` stubbing `resolve_provider_client`)
//! - `asyncio.Lock` → `std::sync::Mutex<Option<CachedAccessToken>>` with
//!   double-checked locking in [`MicrosoftGraphTokenProvider::get_access_token_with_fetch`]
//! - `time.time()` → [`now_secs`] / injected `now` for tests

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Constants — mirrors lines 14-16
// ---------------------------------------------------------------------------

/// Mirrors `DEFAULT_GRAPH_SCOPE = "https://graph.microsoft.com/.default"` (line 14).
pub const DEFAULT_GRAPH_SCOPE: &str = "https://graph.microsoft.com/.default";
/// Mirrors `DEFAULT_GRAPH_AUTHORITY_URL = "https://login.microsoftonline.com"` (line 15).
pub const DEFAULT_GRAPH_AUTHORITY_URL: &str = "https://login.microsoftonline.com";
/// Mirrors `DEFAULT_TOKEN_SKEW_SECONDS = 120` (line 16).
pub const DEFAULT_TOKEN_SKEW_SECONDS: i64 = 120;

// ---------------------------------------------------------------------------
// Errors — mirrors lines 19-28
// ---------------------------------------------------------------------------

/// Mirrors `class MicrosoftGraphAuthError(RuntimeError):` (line 19).
#[derive(Debug, Clone)]
pub struct MicrosoftGraphAuthError(pub String);

impl fmt::Display for MicrosoftGraphAuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for MicrosoftGraphAuthError {}

/// Mirrors `class MicrosoftGraphConfigError(MicrosoftGraphAuthError):` (line 23).
#[derive(Debug, Clone)]
pub struct MicrosoftGraphConfigError(pub String);

impl fmt::Display for MicrosoftGraphConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for MicrosoftGraphConfigError {}

/// Mirrors `class MicrosoftGraphTokenError(MicrosoftGraphAuthError):` (line 27).
#[derive(Debug, Clone)]
pub struct MicrosoftGraphTokenError(pub String);

impl fmt::Display for MicrosoftGraphTokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for MicrosoftGraphTokenError {}

impl From<MicrosoftGraphConfigError> for MicrosoftGraphAuthError {
    fn from(e: MicrosoftGraphConfigError) -> Self {
        Self(e.0)
    }
}
impl From<MicrosoftGraphTokenError> for MicrosoftGraphAuthError {
    fn from(e: MicrosoftGraphTokenError) -> Self {
        Self(e.0)
    }
}

// ---------------------------------------------------------------------------
// Helpers: time, trim, url building
// ---------------------------------------------------------------------------

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn trim_slashes(s: &str) -> &str {
    s.trim_matches('/')
}

// ---------------------------------------------------------------------------
// GraphCredentials — mirrors lines 31-85
// ---------------------------------------------------------------------------

/// Mirrors `@dataclass(frozen=True) class GraphCredentials:` (lines 31-85).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphCredentials {
    pub tenant_id: String,
    pub client_id: String,
    pub client_secret: String,
    pub scope: String,
    pub authority_url: String,
}

impl GraphCredentials {
    /// Create with explicit fields — mirrors `GraphCredentials(tenant_id=..., ...)` (lines 79-85).
    pub fn new(
        tenant_id: impl Into<String>,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        scope: impl Into<String>,
        authority_url: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            scope: scope.into(),
            authority_url: authority_url.into(),
        }
    }

    /// Mirrors `def token_url(self) -> str:` (lines 41-45):
    /// `base = self.authority_url.rstrip("/")` + `tenant = self.tenant_id.strip().strip("/")`
    /// + `return f"{base}/{tenant}/oauth2/v2.0/token"`
    pub fn token_url(&self) -> String {
        let base = self.authority_url.trim_end_matches('/');
        let tenant = self.tenant_id.trim().trim_matches('/');
        format!("{}/{}/oauth2/v2.0/token", base, tenant)
    }

    /// Testable variant with injected lookup — mirrors `GraphCredentials.from_env`
    /// (lines 47-85) with `environ: dict[str,str] | None` + `required: bool`.
    ///
    /// `lookup` mirrors `env.get(name)` where `None` = missing, `Some("")` = empty.
    /// Returns `Ok(None)` when `required=false` and any credential missing,
    /// `Err(MicrosoftGraphConfigError)` when `required=true` and missing,
    /// `Ok(Some(creds))` otherwise.
    pub fn from_env_with_lookup<F>(lookup: F, required: bool) -> Result<Option<Self>, MicrosoftGraphConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        // Mirrors `tenant_id = (env.get("MSGRAPH_TENANT_ID") or "").strip()` (line 55)
        let tenant_id = lookup("MSGRAPH_TENANT_ID").unwrap_or_default().trim().to_string();
        let client_id = lookup("MSGRAPH_CLIENT_ID").unwrap_or_default().trim().to_string();
        let client_secret = lookup("MSGRAPH_CLIENT_SECRET").unwrap_or_default().trim().to_string();

        // Mirrors `scope = (env.get("MSGRAPH_SCOPE") or DEFAULT_GRAPH_SCOPE).strip()` (line 58)
        // where `or` means fallback if None OR empty string. We treat None or empty/whitespace as fallback.
        let scope_raw = lookup("MSGRAPH_SCOPE");
        let scope = match scope_raw {
            Some(v) if !v.trim().is_empty() => v.trim().to_string(),
            _ => DEFAULT_GRAPH_SCOPE.to_string(),
        };
        let scope = scope.trim().to_string();
        let scope = if scope.is_empty() { DEFAULT_GRAPH_SCOPE.to_string() } else { scope };

        let authority_raw = lookup("MSGRAPH_AUTHORITY_URL");
        let authority_url = match authority_raw {
            Some(v) if !v.trim().is_empty() => v.trim().to_string(),
            _ => DEFAULT_GRAPH_AUTHORITY_URL.to_string(),
        };
        let authority_url = authority_url.trim().to_string();
        let authority_url = if authority_url.is_empty() {
            DEFAULT_GRAPH_AUTHORITY_URL.to_string()
        } else {
            authority_url
        };

        let mut missing: Vec<&str> = Vec::new();
        if tenant_id.is_empty() {
            missing.push("MSGRAPH_TENANT_ID");
        }
        if client_id.is_empty() {
            missing.push("MSGRAPH_CLIENT_ID");
        }
        if client_secret.is_empty() {
            missing.push("MSGRAPH_CLIENT_SECRET");
        }
        if !missing.is_empty() {
            if !required {
                return Ok(None);
            }
            return Err(MicrosoftGraphConfigError(format!(
                "Missing Microsoft Graph configuration: {}",
                missing.join(", ")
            )));
        }

        Ok(Some(Self {
            tenant_id,
            client_id,
            client_secret,
            scope,
            authority_url,
        }))
    }

    /// Mirrors `GraphCredentials.from_env(environ=None, required=True)` reading live env.
    pub fn from_env(required: bool) -> Result<Option<Self>, MicrosoftGraphConfigError> {
        Self::from_env_with_lookup(|k| std::env::var(k).ok(), required)
    }

    /// Convenience: required=true, returns Err if missing (mirrors default call site).
    pub fn from_env_required() -> Result<Self, MicrosoftGraphConfigError> {
        match Self::from_env(true)? {
            Some(c) => Ok(c),
            None => unreachable!("required=true never returns Ok(None)"),
        }
    }

    /// Variant with `HashMap` environ — mirrors `environ: dict[str,str] | None`.
    pub fn from_map(map: &HashMap<String, String>, required: bool) -> Result<Option<Self>, MicrosoftGraphConfigError> {
        Self::from_env_with_lookup(|k| map.get(k).cloned(), required)
    }
}

// ---------------------------------------------------------------------------
// CachedAccessToken — mirrors lines 88-102
// ---------------------------------------------------------------------------

/// Mirrors `@dataclass class CachedAccessToken:` (lines 88-102).
#[derive(Debug, Clone)]
pub struct CachedAccessToken {
    pub access_token: String,
    pub expires_at: f64,
    pub token_type: String,
}

impl CachedAccessToken {
    pub fn new(access_token: impl Into<String>, expires_at: f64, token_type: impl Into<String>) -> Self {
        Self {
            access_token: access_token.into(),
            expires_at,
            token_type: token_type.into(),
        }
    }

    /// Mirrors `def is_expired(self, *, skew_seconds: int = DEFAULT_TOKEN_SKEW_SECONDS) -> bool:`
    /// (lines 96-97): `return self.expires_at <= (time.time() + max(0, int(skew_seconds)))`
    pub fn is_expired(&self, skew_seconds: i64) -> bool {
        self.is_expired_with_now(skew_seconds, now_secs())
    }

    /// Testable variant with injected `now` — mirrors `time.time()` call.
    pub fn is_expired_with_now(&self, skew_seconds: i64, now: f64) -> bool {
        let skew = skew_seconds.max(0) as f64;
        self.expires_at <= (now + skew)
    }

    /// Mirrors `@property def expires_in_seconds(self) -> int:` (lines 99-101):
    /// `return max(0, int(self.expires_at - time.time()))`
    pub fn expires_in_seconds(&self) -> i64 {
        self.expires_in_seconds_with_now(now_secs())
    }

    pub fn expires_in_seconds_with_now(&self, now: f64) -> i64 {
        let diff = self.expires_at - now;
        (diff as i64).max(0)
    }
}

// ---------------------------------------------------------------------------
// Token request / response carriers — mirrors _fetch_access_token (lines 167-220)
// ---------------------------------------------------------------------------

/// Carrier for the token endpoint request — mirrors `data` + `headers` + `token_url` in
/// `_fetch_access_token` (lines 168-184).
#[derive(Debug, Clone)]
pub struct TokenRequest {
    pub url: String,
    pub data: HashMap<String, String>,
    pub headers: HashMap<String, String>,
}

/// Minimal HTTP response carrier — mirrors `httpx.Response` surface used in
/// `_fetch_access_token` and `_extract_error_detail`.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status_code: u16,
    pub body: String,
    pub headers: HashMap<String, String>,
}

impl HttpResponse {
    pub fn new(status_code: u16, body: impl Into<String>) -> Self {
        Self {
            status_code,
            body: body.into(),
            headers: HashMap::new(),
        }
    }

    pub fn is_success(&self) -> bool {
        self.status_code < 400
    }

    pub fn text(&self) -> &str {
        &self.body
    }

    /// Try to parse body as JSON — mirrors `response.json()` (lines 194, 225).
    pub fn json(&self) -> Result<Value, String> {
        serde_json::from_str(&self.body).map_err(|e| e.to_string())
    }
}

/// Build the token request — mirrors lines 168-174.
pub fn build_token_request(credentials: &GraphCredentials) -> TokenRequest {
    let mut data = HashMap::new();
    data.insert("grant_type".to_string(), "client_credentials".to_string());
    data.insert("client_id".to_string(), credentials.client_id.clone());
    data.insert("client_secret".to_string(), credentials.client_secret.clone());
    data.insert("scope".to_string(), credentials.scope.clone());
    let mut headers = HashMap::new();
    headers.insert("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string());
    TokenRequest {
        url: credentials.token_url(),
        data,
        headers,
    }
}

/// Parse a successful token JSON payload — mirrors lines 193-220.
///
/// `payload` is the parsed `response.json()` dict. `now` mirrors `time.time()`.
pub fn parse_token_payload(payload: &Value, now: f64) -> Result<CachedAccessToken, MicrosoftGraphTokenError> {
    // Mirrors `payload = response.json()` try/except (193-198) — caller handles JSON parse error.
    let access_token = payload
        .get("access_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let token_type_raw = payload
        .get("token_type")
        .and_then(|v| v.as_str())
        .unwrap_or("Bearer")
        .trim()
        .to_string();
    let token_type = if token_type_raw.is_empty() {
        "Bearer".to_string()
    } else {
        token_type_raw
    };

    if access_token.is_empty() {
        return Err(MicrosoftGraphTokenError(
            "Microsoft Graph token response did not include access_token.".to_string(),
        ));
    }

    let expires_in = payload.get("expires_in");
    let expires_in_seconds: i64 = match expires_in {
        Some(Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                i
            } else if let Some(f) = n.as_f64() {
                f as i64
            } else {
                return Err(MicrosoftGraphTokenError(
                    "Microsoft Graph token response did not include a valid expires_in.".to_string(),
                ));
            }
        }
        Some(Value::String(s)) => s.trim().parse::<i64>().map_err(|_| {
            MicrosoftGraphTokenError(
                "Microsoft Graph token response did not include a valid expires_in.".to_string(),
            )
        })?,
        _ => {
            return Err(MicrosoftGraphTokenError(
                "Microsoft Graph token response did not include a valid expires_in.".to_string(),
            ));
        }
    };

    let expires_at = now + (expires_in_seconds.max(0) as f64);
    Ok(CachedAccessToken {
        access_token,
        expires_at,
        token_type,
    })
}

/// Parse token response body string — convenience wrapper that handles JSON parse error
/// mirroring lines 193-198.
pub fn parse_token_response_body(body: &str, now: f64) -> Result<CachedAccessToken, MicrosoftGraphTokenError> {
    let payload: Value = serde_json::from_str(body).map_err(|_| {
        MicrosoftGraphTokenError("Microsoft Graph token response was not valid JSON.".to_string())
    })?;
    parse_token_payload(&payload, now)
}

// ---------------------------------------------------------------------------
// _extract_error_detail — mirrors lines 223-245
// ---------------------------------------------------------------------------

/// Mirrors `def _extract_error_detail(response: httpx.Response) -> str:` (lines 223-245).
///
/// Tries `response.json()`: if `ValueError` returns `response.text.strip() or "unknown error"`.
/// If JSON dict, checks `error_description` str, then `error` dict with `message`/`code`,
/// then `error` str, else `str(payload)`.
pub fn extract_error_detail(response: &HttpResponse) -> String {
    extract_error_detail_from_body(&response.body)
}

/// Testable variant that operates on raw body string.
pub fn extract_error_detail_from_body(body: &str) -> String {
    let payload: Result<Value, _> = serde_json::from_str(body);
    match payload {
        Err(_) => {
            let text = body.trim();
            if text.is_empty() {
                "unknown error".to_string()
            } else {
                text.to_string()
            }
        }
        Ok(value) => extract_error_detail_from_json(&value),
    }
}

/// Mirrors the JSON branch of `_extract_error_detail` (lines 230-245).
pub fn extract_error_detail_from_json(payload: &Value) -> String {
    if let Value::Object(map) = payload {
        if let Some(Value::String(s)) = map.get("error_description") {
            return s.clone();
        }
        if let Some(error) = map.get("error") {
            match error {
                Value::Object(err_map) => {
                    let message = err_map.get("message").and_then(|v| v.as_str());
                    let code = err_map.get("code").and_then(|v| v.as_str());
                    match (code, message) {
                        (Some(c), Some(m)) if !c.is_empty() && !m.is_empty() => {
                            return format!("{c}: {m}");
                        }
                        (_, Some(m)) if !m.is_empty() => return m.to_string(),
                        (Some(c), _) if !c.is_empty() => return c.to_string(),
                        _ => {}
                    }
                }
                Value::String(s) => return s.clone(),
                _ => {}
            }
        }
    }
    // Mirrors `return str(payload)` — serialize back to JSON string for non-dict / unmatched
    // For Value::String, str(payload) in Python would include quotes? But Python's str(dict) vs json.
    // We use `to_string` which for Value serializes JSON; matching "str(payload)" for dict.
    // For raw string payload already handled above as JSON parse failure path.
    payload.to_string()
}

// ---------------------------------------------------------------------------
// MicrosoftGraphTokenProvider — mirrors lines 104-220
// ---------------------------------------------------------------------------

/// Mirrors `class MicrosoftGraphTokenProvider:` (lines 104-220).
pub struct MicrosoftGraphTokenProvider {
    pub credentials: GraphCredentials,
    pub timeout_secs: f64,
    pub skew_seconds: i64,
    cached_token: Mutex<Option<CachedAccessToken>>,
}

impl MicrosoftGraphTokenProvider {
    /// Mirrors `def __init__(self, credentials, *, timeout=20.0, skew_seconds=120, transport=None)` (lines 107-120).
    pub fn new(credentials: GraphCredentials, timeout_secs: f64, skew_seconds: i64) -> Self {
        Self {
            credentials,
            timeout_secs,
            skew_seconds: skew_seconds.max(0),
            cached_token: Mutex::new(None),
        }
    }

    /// Mirrors default `__init__` with defaults `timeout=20.0`, `skew_seconds=DEFAULT_TOKEN_SKEW_SECONDS`.
    pub fn with_defaults(credentials: GraphCredentials) -> Self {
        Self::new(credentials, 20.0, DEFAULT_TOKEN_SKEW_SECONDS)
    }

    /// Mirrors `@classmethod def from_env(cls, environ=None, **kwargs)` (lines 122-129).
    pub fn from_env_with_lookup<F>(lookup: F, timeout_secs: f64, skew_seconds: i64) -> Result<Self, MicrosoftGraphConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let credentials = match GraphCredentials::from_env_with_lookup(lookup, true)? {
            Some(c) => c,
            None => unreachable!("required=true"),
        };
        Ok(Self::new(credentials, timeout_secs, skew_seconds))
    }

    pub fn from_env() -> Result<Self, MicrosoftGraphConfigError> {
        Self::from_env_with_lookup(|k| std::env::var(k).ok(), 20.0, DEFAULT_TOKEN_SKEW_SECONDS)
    }

    pub fn from_map(map: &HashMap<String, String>) -> Result<Self, MicrosoftGraphConfigError> {
        Self::from_env_with_lookup(|k| map.get(k).cloned(), 20.0, DEFAULT_TOKEN_SKEW_SECONDS)
    }

    /// Mirrors `def clear_cache(self) -> None:` (lines 131-132).
    pub fn clear_cache(&self) {
        if let Ok(mut guard) = self.cached_token.lock() {
            *guard = None;
        }
    }

    /// Mirrors `def inspect_token_health(self) -> dict[str, Any]:` (lines 134-147).
    pub fn inspect_token_health(&self) -> Value {
        self.inspect_token_health_with_now(now_secs())
    }

    pub fn inspect_token_health_with_now(&self, now: f64) -> Value {
        let cached = self.cached_token.lock().ok().and_then(|g| g.clone());
        let (cached_bool, expires_in, is_expired) = match &cached {
            Some(tok) => {
                let exp = tok.expires_in_seconds_with_now(now);
                let expired = tok.is_expired_with_now(0, now);
                (true, Some(exp), Some(expired))
            }
            None => (false, None, None),
        };
        json!({
            "configured": true,
            "tenant_id": self.credentials.tenant_id,
            "client_id": self.credentials.client_id,
            "scope": self.credentials.scope,
            "authority_url": self.credentials.authority_url,
            "token_url": self.credentials.token_url(),
            "cached": cached_bool,
            "expires_in_seconds": expires_in,
            "is_expired": is_expired,
            "refresh_skew_seconds": self.skew_seconds,
        })
    }

    /// Synchronous token acquisition with double-checked locking — mirrors
    /// `async def get_access_token(self, *, force_refresh=False) -> str:` (lines 149-165).
    ///
    /// The `fetch` closure mirrors `await self._fetch_access_token()` (line 163).
    /// It is called only when cache miss or `force_refresh`, and its result is
    /// cached before returning.
    pub fn get_access_token_with_fetch<F>(&self, force_refresh: bool, fetch: F) -> Result<String, MicrosoftGraphTokenError>
    where
        F: FnOnce() -> Result<CachedAccessToken, MicrosoftGraphTokenError>,
    {
        self.get_access_token_with_fetch_and_now(force_refresh, now_secs(), fetch)
    }

    pub fn get_access_token_with_fetch_and_now<F>(&self, force_refresh: bool, now: f64, fetch: F) -> Result<String, MicrosoftGraphTokenError>
    where
        F: FnOnce() -> Result<CachedAccessToken, MicrosoftGraphTokenError>,
    {
        // Fast path without lock — mirrors first `cached = self._cached_token` check (lines 150-154)
        {
            if let Ok(guard) = self.cached_token.lock() {
                if let Some(cached) = guard.as_ref() {
                    if !force_refresh && !cached.is_expired_with_now(self.skew_seconds, now) {
                        return Ok(cached.access_token.clone());
                    }
                }
            }
        }
        // Acquire lock — mirrors `async with self._lock:` (line 156)
        let mut guard = self.cached_token.lock().map_err(|_| {
            MicrosoftGraphTokenError("Microsoft Graph token cache lock poisoned".to_string())
        })?;
        // Double-check after acquiring lock (lines 157-161)
        if let Some(cached) = guard.as_ref() {
            if !force_refresh && !cached.is_expired_with_now(self.skew_seconds, now) {
                return Ok(cached.access_token.clone());
            }
        }
        // Fetch and cache — mirrors lines 163-165
        let token = fetch()?;
        let access = token.access_token.clone();
        *guard = Some(token);
        Ok(access)
    }

    /// Build the token request — mirrors `_fetch_access_token` data/headers prep (lines 168-174).
    pub fn token_request(&self) -> TokenRequest {
        build_token_request(&self.credentials)
    }

    /// Handle an HTTP response for token acquisition — mirrors lines 186-220.
    ///
    /// Returns `CachedAccessToken` on success, `MicrosoftGraphTokenError` on failure,
    /// with the same error messages as Python.
    pub fn handle_token_response(&self, response: &HttpResponse, now: f64) -> Result<CachedAccessToken, MicrosoftGraphTokenError> {
        if response.status_code >= 400 {
            let detail = extract_error_detail(response);
            return Err(MicrosoftGraphTokenError(format!(
                "Microsoft Graph token request failed with HTTP {}: {}",
                response.status_code, detail
            )));
        }
        // Mirrors try: payload = response.json() except ValueError (193-198)
        let payload: Value = serde_json::from_str(&response.body).map_err(|_| {
            MicrosoftGraphTokenError("Microsoft Graph token response was not valid JSON.".to_string())
        })?;
        parse_token_payload(&payload, now)
    }

    /// Convenience: fetch via a synchronous transport closure
    /// `transport: Fn(&TokenRequest) -> Result<HttpResponse, String>`.
    ///
    /// Mirrors `async with httpx.AsyncClient(..., transport=self._transport) as client: response = await client.post(...)`
    /// (lines 176-184) plus the error handling above.
    pub fn fetch_access_token_via<F>(&self, transport: F) -> Result<CachedAccessToken, MicrosoftGraphTokenError>
    where
        F: Fn(&TokenRequest) -> Result<HttpResponse, String>,
    {
        self.fetch_access_token_via_with_now(transport, now_secs())
    }

    pub fn fetch_access_token_via_with_now<F>(&self, transport: F, now: f64) -> Result<CachedAccessToken, MicrosoftGraphTokenError>
    where
        F: Fn(&TokenRequest) -> Result<HttpResponse, String>,
    {
        let req = self.token_request();
        let resp = transport(&req).map_err(|e| {
            MicrosoftGraphTokenError(format!(
                "Microsoft Graph token request failed with HTTP error: {e}"
            ))
        })?;
        self.handle_token_response(&resp, now)
    }

    /// Inspect cached token directly (test helper) — mirrors `self._cached_token`.
    pub fn cached_token(&self) -> Option<CachedAccessToken> {
        self.cached_token.lock().ok().and_then(|g| g.clone())
    }

    /// Returns true if cached token exists and is not expired with skew.
    pub fn has_valid_cached_token_with_now(&self, now: f64) -> bool {
        if let Ok(g) = self.cached_token.lock() {
            if let Some(tok) = g.as_ref() {
                return !tok.is_expired_with_now(self.skew_seconds, now);
            }
        }
        false
    }
}

impl fmt::Debug for MicrosoftGraphTokenProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MicrosoftGraphTokenProvider")
            .field("credentials", &self.credentials)
            .field("timeout_secs", &self.timeout_secs)
            .field("skew_seconds", &self.skew_seconds)
            .field("cached", &self.cached_token.lock().ok().and_then(|g| g.is_some()))
            .finish()
    }
}

// ---------------------------------------------------------------------------
// __all__ equivalent
// ---------------------------------------------------------------------------

pub const ALL: &[&str] = &[
    "DEFAULT_GRAPH_SCOPE",
    "DEFAULT_GRAPH_AUTHORITY_URL",
    "DEFAULT_TOKEN_SKEW_SECONDS",
    "MicrosoftGraphAuthError",
    "MicrosoftGraphConfigError",
    "MicrosoftGraphTokenError",
    "GraphCredentials",
    "CachedAccessToken",
    "MicrosoftGraphTokenProvider",
    "extract_error_detail",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn creds() -> GraphCredentials {
        GraphCredentials::new("tenant123", "client123", "secret123", DEFAULT_GRAPH_SCOPE, DEFAULT_GRAPH_AUTHORITY_URL)
    }

    #[test]
    fn constants_match_python() {
        assert_eq!(DEFAULT_GRAPH_SCOPE, "https://graph.microsoft.com/.default");
        assert_eq!(DEFAULT_GRAPH_AUTHORITY_URL, "https://login.microsoftonline.com");
        assert_eq!(DEFAULT_TOKEN_SKEW_SECONDS, 120);
    }

    #[test]
    fn token_url_strips_slashes() {
        let c = GraphCredentials::new(" tenant/ ", "c", "s", DEFAULT_GRAPH_SCOPE, "https://login.example.com/");
        assert_eq!(c.token_url(), "https://login.example.com/tenant/oauth2/v2.0/token");
        let c2 = GraphCredentials::new("tenant", "c", "s", DEFAULT_GRAPH_SCOPE, "https://login.example.com///");
        assert_eq!(c2.token_url(), "https://login.example.com/tenant/oauth2/v2.0/token");
        let c3 = creds();
        assert_eq!(c3.token_url(), "https://login.microsoftonline.com/tenant123/oauth2/v2.0/token");
    }

    #[test]
    fn trim_slashes_helper() {
        assert_eq!(trim_slashes("///a/b///"), "a/b");
        assert_eq!(trim_slashes("a"), "a");
    }

    #[test]
    fn graph_credentials_from_env_success() {
        let mut m = HashMap::new();
        m.insert("MSGRAPH_TENANT_ID".to_string(), " t1 ".to_string());
        m.insert("MSGRAPH_CLIENT_ID".to_string(), "c1".to_string());
        m.insert("MSGRAPH_CLIENT_SECRET".to_string(), "s1".to_string());
        let c = GraphCredentials::from_map(&m, true).unwrap().unwrap();
        assert_eq!(c.tenant_id, "t1");
        assert_eq!(c.client_id, "c1");
        assert_eq!(c.client_secret, "s1");
        assert_eq!(c.scope, DEFAULT_GRAPH_SCOPE);
        assert_eq!(c.authority_url, DEFAULT_GRAPH_AUTHORITY_URL);

        // custom scope/authority
        let mut m2 = m.clone();
        m2.insert("MSGRAPH_SCOPE".to_string(), " https://custom.scope ".to_string());
        m2.insert("MSGRAPH_AUTHORITY_URL".to_string(), " https://custom.authority ".to_string());
        let c2 = GraphCredentials::from_map(&m2, true).unwrap().unwrap();
        assert_eq!(c2.scope, "https://custom.scope");
        assert_eq!(c2.authority_url, "https://custom.authority");

        // empty scope fallback to default (or check)
        let mut m3 = m.clone();
        m3.insert("MSGRAPH_SCOPE".to_string(), "   ".to_string());
        let c3 = GraphCredentials::from_map(&m3, true).unwrap().unwrap();
        assert_eq!(c3.scope, DEFAULT_GRAPH_SCOPE);
    }

    #[test]
    fn graph_credentials_from_env_missing_required() {
        let m = HashMap::new();
        let err = GraphCredentials::from_map(&m, true).unwrap_err();
        assert!(err.0.contains("MSGRAPH_TENANT_ID"));
        assert!(err.0.contains("MSGRAPH_CLIENT_ID"));
        assert!(err.0.contains("MSGRAPH_CLIENT_SECRET"));
        assert!(err.0.contains("Missing Microsoft Graph configuration:"));

        let mut partial = HashMap::new();
        partial.insert("MSGRAPH_TENANT_ID".to_string(), "t".to_string());
        partial.insert("MSGRAPH_CLIENT_ID".to_string(), "c".to_string());
        let err2 = GraphCredentials::from_map(&partial, true).unwrap_err();
        assert!(err2.0.contains("MSGRAPH_CLIENT_SECRET"));
        assert!(!err2.0.contains("MSGRAPH_TENANT_ID"));

        // required=false returns None
        let none = GraphCredentials::from_map(&partial, false).unwrap();
        assert!(none.is_none());
        let none2 = GraphCredentials::from_map(&HashMap::new(), false).unwrap();
        assert!(none2.is_none());

        // whitespace treated as missing
        let mut ws = HashMap::new();
        ws.insert("MSGRAPH_TENANT_ID".to_string(), "   ".to_string());
        ws.insert("MSGRAPH_CLIENT_ID".to_string(), "c".to_string());
        ws.insert("MSGRAPH_CLIENT_SECRET".to_string(), "s".to_string());
        let err_ws = GraphCredentials::from_map(&ws, true).unwrap_err();
        assert!(err_ws.0.contains("MSGRAPH_TENANT_ID"));
    }

    #[test]
    fn graph_credentials_from_env_with_lookup_none() {
        let c = GraphCredentials::from_env_with_lookup(|_| None, false).unwrap();
        assert!(c.is_none());
        let err = GraphCredentials::from_env_with_lookup(|_| None, true).unwrap_err();
        assert!(err.0.contains("Missing"));
    }

    #[test]
    fn cached_access_token_is_expired() {
        let now = 1000.0;
        let tok = CachedAccessToken::new("tok", 1100.0, "Bearer");
        // expires_at 1100, now 1000, skew 120 => 1100 <= 1120 => true (expired)
        assert!(tok.is_expired_with_now(120, now));
        // skew 0 => 1100 <= 1000 => false
        assert!(!tok.is_expired_with_now(0, now));
        // expires_at exactly at now+skew => expired (<=)
        assert!(tok.is_expired_with_now(100, 1000.0)); // 1100 <= 1100 => true
        // negative skew clamped to 0
        assert!(!tok.is_expired_with_now(-5, now));
        assert!(!tok.is_expired_with_now(50, now)); // 1100 <=1050 => false
        assert!(tok.is_expired_with_now(100, now));
    }

    #[test]
    fn cached_access_token_expires_in_seconds() {
        let now = 1000.0;
        let tok = CachedAccessToken::new("tok", 1500.0, "Bearer");
        assert_eq!(tok.expires_in_seconds_with_now(now), 500);
        let expired = CachedAccessToken::new("tok", 900.0, "Bearer");
        assert_eq!(expired.expires_in_seconds_with_now(now), 0);
        // fractional truncated via cast
        let tok2 = CachedAccessToken::new("tok", 1000.9, "Bearer");
        assert_eq!(tok2.expires_in_seconds_with_now(1000.0), 0); // 0.9 as i64 =0
        let tok3 = CachedAccessToken::new("tok", 1001.9, "Bearer");
        assert_eq!(tok3.expires_in_seconds_with_now(1000.0), 1);
    }

    #[test]
    fn provider_new_clamps_skew() {
        let p = MicrosoftGraphTokenProvider::new(creds(), 20.0, -10);
        assert_eq!(p.skew_seconds, 0);
        let p2 = MicrosoftGraphTokenProvider::new(creds(), 20.0, 50);
        assert_eq!(p2.skew_seconds, 50);
        assert_eq!(p2.timeout_secs, 20.0);
    }

    #[test]
    fn provider_clear_cache() {
        let p = MicrosoftGraphTokenProvider::with_defaults(creds());
        // inject a token via fetch
        let tok = CachedAccessToken::new("abc", now_secs() + 3600.0, "Bearer");
        let fetched = tok.clone();
        let _ = p.get_access_token_with_fetch(false, move || Ok(fetched.clone()));
        assert!(p.cached_token().is_some());
        p.clear_cache();
        assert!(p.cached_token().is_none());
    }

    #[test]
    fn provider_inspect_token_health_no_cache() {
        let p = MicrosoftGraphTokenProvider::new(creds(), 20.0, 120);
        let v = p.inspect_token_health_with_now(1000.0);
        assert_eq!(v["configured"], json!(true));
        assert_eq!(v["tenant_id"], json!("tenant123"));
        assert_eq!(v["client_id"], json!("client123"));
        assert_eq!(v["scope"], json!(DEFAULT_GRAPH_SCOPE));
        assert_eq!(v["authority_url"], json!(DEFAULT_GRAPH_AUTHORITY_URL));
        assert_eq!(v["cached"], json!(false));
        assert_eq!(v["expires_in_seconds"], Value::Null);
        assert_eq!(v["is_expired"], Value::Null);
        assert_eq!(v["refresh_skew_seconds"], json!(120));
        assert_eq!(v["token_url"], json!("https://login.microsoftonline.com/tenant123/oauth2/v2.0/token"));
    }

    #[test]
    fn provider_inspect_token_health_with_cache() {
        let p = MicrosoftGraphTokenProvider::with_defaults(creds());
        let now = 1000.0;
        let tok = CachedAccessToken::new("tok123", 2000.0, "Bearer");
        // prime cache
        let tok_clone = tok.clone();
        let _ = p.get_access_token_with_fetch_and_now(false, now, move || Ok(tok_clone.clone()));
        let v = p.inspect_token_health_with_now(now);
        assert_eq!(v["cached"], json!(true));
        assert_eq!(v["expires_in_seconds"], json!(1000));
        assert_eq!(v["is_expired"], json!(false));
        // after expiry
        let v2 = p.inspect_token_health_with_now(2000.0);
        assert_eq!(v2["is_expired"], json!(true));
        assert_eq!(v2["expires_in_seconds"], json!(0));
    }

    #[test]
    fn provider_get_access_token_caches_and_double_checked() {
        let p = MicrosoftGraphTokenProvider::new(creds(), 20.0, 120);
        let now = 1000.0;
        let mut fetch_count = 0;
        let tok = CachedAccessToken::new("first", 5000.0, "Bearer");
        let tok2 = tok.clone();
        let res = p.get_access_token_with_fetch_and_now(false, now, move || {
            fetch_count += 1;
            Ok(tok2.clone())
        });
        assert_eq!(res.unwrap(), "first");
        // second call should hit cache without fetch
        let res2 = p.get_access_token_with_fetch_and_now(false, now, || {
            panic!("should not fetch when cached valid");
        });
        assert_eq!(res2.unwrap(), "first");

        // force_refresh bypasses cache
        let tok_new = CachedAccessToken::new("second", 5000.0, "Bearer");
        let res3 = p.get_access_token_with_fetch_and_now(true, now, move || Ok(tok_new.clone()));
        assert_eq!(res3.unwrap(), "second");

        // expired cache triggers fetch
        let late_now = 4900.0; // cached "second" expires_at 5000, skew 120 => 5000 <= 5020 => true => expired
        let tok3 = CachedAccessToken::new("third", 9000.0, "Bearer");
        let res4 = p.get_access_token_with_fetch_and_now(false, late_now, move || Ok(tok3.clone()));
        assert_eq!(res4.unwrap(), "third");
    }

    #[test]
    fn provider_fetch_via_transport_success() {
        let p = MicrosoftGraphTokenProvider::with_defaults(creds());
        let now = 1000.0;
        let body = r#"{"access_token":"abc123","token_type":"Bearer","expires_in":3600}"#;
        let resp = HttpResponse::new(200, body);
        let tok = p.handle_token_response(&resp, now).unwrap();
        assert_eq!(tok.access_token, "abc123");
        assert_eq!(tok.token_type, "Bearer");
        assert_eq!(tok.expires_at, 4600.0);
    }

    #[test]
    fn provider_fetch_via_transport_http_error() {
        let p = MicrosoftGraphTokenProvider::with_defaults(creds());
        let body = r#"{"error":"invalid_client","error_description":"bad secret"}"#;
        let resp = HttpResponse::new(401, body);
        let err = p.handle_token_response(&resp, 1000.0).unwrap_err();
        assert!(err.0.contains("HTTP 401"));
        assert!(err.0.contains("bad secret"));
    }

    #[test]
    fn parse_token_payload_missing_access_token() {
        let payload = json!({"token_type":"Bearer","expires_in":3600});
        let err = parse_token_payload(&payload, 1000.0).unwrap_err();
        assert!(err.0.contains("access_token"));
    }

    #[test]
    fn parse_token_payload_token_type_default() {
        let payload = json!({"access_token":"tok","expires_in":100});
        let tok = parse_token_payload(&payload, 1000.0).unwrap();
        assert_eq!(tok.token_type, "Bearer");
        let payload2 = json!({"access_token":"tok","token_type":"","expires_in":100});
        let tok2 = parse_token_payload(&payload2, 1000.0).unwrap();
        assert_eq!(tok2.token_type, "Bearer");
        let payload3 = json!({"access_token":"tok","token_type":"bearer","expires_in":100});
        let tok3 = parse_token_payload(&payload3, 1000.0).unwrap();
        assert_eq!(tok3.token_type, "bearer");
    }

    #[test]
    fn parse_token_payload_expires_in_string_and_invalid() {
        let payload = json!({"access_token":"tok","expires_in":"3600"});
        let tok = parse_token_payload(&payload, 1000.0).unwrap();
        assert_eq!(tok.expires_at, 4600.0);
        // invalid expires_in
        let payload2 = json!({"access_token":"tok","expires_in":"notanint"});
        assert!(parse_token_payload(&payload2, 1000.0).is_err());
        let payload3 = json!({"access_token":"tok"});
        assert!(parse_token_payload(&payload3, 1000.0).is_err());
        let payload4 = json!({"access_token":"tok","expires_in":null});
        assert!(parse_token_payload(&payload4, 1000.0).is_err());
        // negative expires_in clamped to 0
        let payload5 = json!({"access_token":"tok","expires_in":-100});
        let tok5 = parse_token_payload(&payload5, 1000.0).unwrap();
        assert_eq!(tok5.expires_at, 1000.0);
    }

    #[test]
    fn parse_token_response_body_invalid_json() {
        let err = parse_token_response_body("not json", 1000.0).unwrap_err();
        assert!(err.0.contains("not valid JSON"));
    }

    #[test]
    fn build_token_request_shape() {
        let c = creds();
        let req = build_token_request(&c);
        assert_eq!(req.url, c.token_url());
        assert_eq!(req.data.get("grant_type").unwrap(), "client_credentials");
        assert_eq!(req.data.get("client_id").unwrap(), "client123");
        assert_eq!(req.data.get("client_secret").unwrap(), "secret123");
        assert_eq!(req.data.get("scope").unwrap(), DEFAULT_GRAPH_SCOPE);
        assert_eq!(req.headers.get("Content-Type").unwrap(), "application/x-www-form-urlencoded");
    }

    #[test]
    fn extract_error_detail_non_json() {
        let body = "  plain text error  ";
        assert_eq!(extract_error_detail_from_body(body), "plain text error");
        assert_eq!(extract_error_detail_from_body("   "), "unknown error");
        assert_eq!(extract_error_detail_from_body(""), "unknown error");
    }

    #[test]
    fn extract_error_detail_error_description() {
        let body = r#"{"error_description":"AADSTS7000222: bad"}"#;
        assert_eq!(extract_error_detail_from_body(body), "AADSTS7000222: bad");
    }

    #[test]
    fn extract_error_detail_error_dict() {
        let body = r#"{"error":{"code":"Invalid","message":"oops"}}"#;
        assert_eq!(extract_error_detail_from_body(body), "Invalid: oops");
        let body2 = r#"{"error":{"message":"only msg"}}"#;
        assert_eq!(extract_error_detail_from_body(body2), "only msg");
        let body3 = r#"{"error":{"code":"OnlyCode"}}"#;
        assert_eq!(extract_error_detail_from_body(body3), "OnlyCode");
        let body4 = r#"{"error":"invalid_grant"}"#;
        assert_eq!(extract_error_detail_from_body(body4), "invalid_grant");
    }

    #[test]
    fn extract_error_detail_fallback_to_payload_string() {
        let body = r#"{"unexpected":123}"#;
        let detail = extract_error_detail_from_body(body);
        // should be JSON stringified payload
        assert!(detail.contains("unexpected"));
        let body2 = r#"[1,2,3]"#;
        let detail2 = extract_error_detail_from_body(body2);
        assert!(detail2.contains("1"));
    }

    #[test]
    fn http_response_helpers() {
        let r = HttpResponse::new(200, "ok");
        assert!(r.is_success());
        assert_eq!(r.text(), "ok");
        let r2 = HttpResponse::new(400, "bad");
        assert!(!r2.is_success());
        let r3 = HttpResponse::new(200, r#"{"a":1}"#);
        assert!(r3.json().is_ok());
        let r4 = HttpResponse::new(200, "not json");
        assert!(r4.json().is_err());
    }

    #[test]
    fn provider_from_env_missing() {
        let mut m = HashMap::new();
        m.insert("MSGRAPH_TENANT_ID".to_string(), "t".to_string());
        // missing client_id/secret
        let err = MicrosoftGraphTokenProvider::from_env_with_lookup(|k| m.get(k).cloned(), 20.0, 120).unwrap_err();
        assert!(err.0.contains("MSGRAPH_CLIENT_ID"));
    }

    #[test]
    fn provider_fetch_via_closure() {
        let p = MicrosoftGraphTokenProvider::with_defaults(creds());
        let now = 1000.0;
        let tok = p.fetch_access_token_via_with_now(|req| {
            assert_eq!(req.url, creds().token_url());
            assert_eq!(req.data.get("grant_type").unwrap(), "client_credentials");
            Ok(HttpResponse::new(200, r#"{"access_token":"via","token_type":"Bearer","expires_in":100}"#))
        }, now).unwrap();
        assert_eq!(tok.access_token, "via");
        assert_eq!(tok.expires_at, 1100.0);
    }

    #[test]
    fn provider_fetch_via_transport_error_propagation() {
        let p = MicrosoftGraphTokenProvider::with_defaults(creds());
        let err = p.fetch_access_token_via(|_| Err("network down".to_string())).unwrap_err();
        assert!(err.0.contains("network down"));
    }
}
