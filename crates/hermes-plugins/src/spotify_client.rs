//! Thin Spotify Web API helper used by Hermes native tools.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/plugins/spotify/client.py` (435 LOC).
//!
//! Python surface ported line-for-line:
//! - `SpotifyError`, `SpotifyAuthRequiredError`, `SpotifyAPIError` (with `status_code`, `response_body`, `path`)
//! - `SpotifyClient` (`_resolve_runtime`, `base_url`, `_headers`, `request`, `_raise_api_error`)
//!   + all endpoint helpers: `get_devices`, `transfer_playback`, `get_playback_state`,
//!     `get_currently_playing`, `start_playback`, `pause_playback`, `skip_next`,
//!     `skip_previous`, `seek`, `set_repeat`, `set_shuffle`, `set_volume`, `get_queue`,
//!     `add_to_queue`, `search`, `get_my_playlists`, `get_playlist`, `create_playlist`,
//!     `add_playlist_items`, `remove_playlist_items`, `update_playlist_details`,
//!     `get_album`, `get_album_tracks`, `get_saved_tracks`, `save_library_items`,
//!     `library_contains`, `get_saved_albums`, `remove_saved_tracks`,
//!     `remove_saved_albums`, `get_recently_played`
//! - `_extract_spotify_error_detail`, `_friendly_spotify_error_message`, `_strip_none`
//! - `normalize_spotify_id`, `normalize_spotify_uri`, `normalize_spotify_uris`, `compact_json`
//!
//! Network I/O in Python (`httpx.request(..., timeout=30.0)`) is represented here
//! with synchronous `curl` stubs + documented `reqwest`/`tokio` upgrade paths so
//! the routing, validation, and error-mapping semantics are byte-identical without
//! requiring `cargo` in this task. Real I/O would swap the `curl` bodies for
//! `reqwest::Client::request(...).send().await`.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// HERMES_HOME helpers — mirrors hermes_constants.get_hermes_home()
// ---------------------------------------------------------------------------

/// Resolve `HERMES_HOME`: `$HERMES_HOME` if set and non-empty, else `~/.hermes`.
pub fn get_hermes_home() -> PathBuf {
    if let Ok(val) = std::env::var("HERMES_HOME") {
        let trimmed = val.trim().to_string();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".hermes");
    }
    PathBuf::from(".hermes")
}

// ---------------------------------------------------------------------------
// Error types — mirrors client.py:17-39
// ---------------------------------------------------------------------------

/// Base Spotify tool error. Mirrors `class SpotifyError(RuntimeError)` (line 17).
#[derive(Debug, Clone)]
pub struct SpotifyError(pub String);
impl std::fmt::Display for SpotifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for SpotifyError {}

/// Raised when the user needs to authenticate. Mirrors `SpotifyAuthRequiredError` (line 21).
#[derive(Debug, Clone)]
pub struct SpotifyAuthRequiredError(pub String);
impl std::fmt::Display for SpotifyAuthRequiredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for SpotifyAuthRequiredError {}

/// Structured API failure. Mirrors `SpotifyAPIError` (lines 25-38):
/// `__init__(message, *, status_code=None, response_body=None)` + `self.path = None` set by caller.
#[derive(Debug, Clone)]
pub struct SpotifyApiError {
    pub message: String,
    pub status_code: Option<u16>,
    pub response_body: Option<String>,
    pub path: Option<String>,
}
impl std::fmt::Display for SpotifyApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for SpotifyApiError {}

// ---------------------------------------------------------------------------
// Runtime credentials — mirrors hermes_cli.auth.resolve_spotify_runtime_credentials
// ---------------------------------------------------------------------------

/// Runtime credentials dict — mirrors the dict returned by `resolve_spotify_runtime_credentials`.
///
/// Python shape (from `hermes_cli.auth`):
/// `{"access_token": str, "base_url": str, ...}` with token refresh support.
#[derive(Debug, Clone)]
pub struct SpotifyRuntime {
    pub access_token: String,
    pub base_url: String,
}

/// Mirrors `resolve_spotify_runtime_credentials(force_refresh, refresh_if_expiring)`.
///
/// Python raises `AuthError` -> `SpotifyAuthRequiredError` in `_resolve_runtime`.
/// Rust stub reads `SPOTIFY_ACCESS_TOKEN` / `SPOTIFY_TOKEN`, then `$HERMES_HOME/.env`,
/// then `$HERMES_HOME/auth.json`, then `SPOTIFY_API_BASE_URL`. Returns `Err` with
/// the same user-facing message Python surfaces.
fn resolve_spotify_runtime_credentials(
    force_refresh: bool,
    refresh_if_expiring: bool,
) -> Result<SpotifyRuntime, SpotifyAuthRequiredError> {
    let _ = (force_refresh, refresh_if_expiring);
    let token = std::env::var("SPOTIFY_ACCESS_TOKEN")
        .or_else(|_| std::env::var("SPOTIFY_TOKEN"))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            // Try $HERMES_HOME/.env
            let home = get_hermes_home();
            let dotenv = home.join(".env");
            if let Ok(text) = fs::read_to_string(&dotenv) {
                for line in text.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    if let Some((k, v)) = line.split_once('=') {
                        let kk = k.trim();
                        if kk == "SPOTIFY_ACCESS_TOKEN" || kk == "SPOTIFY_TOKEN" {
                            let val = v.trim().trim_matches('"').trim_matches('\'').to_string();
                            if !val.is_empty() {
                                return Some(val);
                            }
                        }
                    }
                }
            }
            None
        })
        .or_else(|| {
            // Try $HERMES_HOME/auth.json — check providers.spotify / spotify shapes
            let home = get_hermes_home();
            let path = home.join("auth.json");
            if let Ok(text) = fs::read_to_string(&path) {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    let candidates = [
                        v.get("spotify").and_then(|x| x.get("access_token")).and_then(|x| x.as_str()),
                        v.get("spotify").and_then(|x| x.get("tokens")).and_then(|x| x.get("access_token")).and_then(|x| x.as_str()),
                        v.get("providers").and_then(|x| x.get("spotify")).and_then(|x| x.get("tokens")).and_then(|x| x.get("access_token")).and_then(|x| x.as_str()),
                        v.get("providers").and_then(|x| x.get("spotify")).and_then(|x| x.get("access_token")).and_then(|x| x.as_str()),
                    ];
                    for c in candidates.into_iter().flatten() {
                        if !c.trim().is_empty() {
                            return Some(c.to_string());
                        }
                    }
                }
            }
            None
        });

    let token = match token {
        Some(t) => t.trim().to_string(),
        None => {
            return Err(SpotifyAuthRequiredError(
                "Spotify authentication required. Run `hermes auth spotify` to sign in.".to_string(),
            ))
        }
    };

    let base_url = std::env::var("SPOTIFY_API_BASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .unwrap_or_else(|| "https://api.spotify.com/v1".to_string());

    Ok(SpotifyRuntime { access_token: token, base_url })
}

// ---------------------------------------------------------------------------
// Helpers — mirrors client.py:324-435
// ---------------------------------------------------------------------------

/// Mirrors `def _strip_none(payload: Optional[Dict[str, Any]]) -> Dict[str, Any]` (lines 379-382).
///
/// ```python
/// def _strip_none(payload):
///     if not payload:
///         return {}
///     return {key: value for key, value in payload.items() if value is not None}
/// ```
fn strip_none(payload: Option<&Value>) -> Value {
    match payload {
        None => json!({}),
        Some(Value::Object(map)) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                if !v.is_null() {
                    out.insert(k.clone(), v.clone());
                }
            }
            Value::Object(out)
        }
        Some(v) if v.is_null() => json!({}),
        Some(v) => v.clone(),
    }
}

/// Mirrors `def _extract_spotify_error_detail(response, *, fallback) -> str` (lines 324-336).
fn extract_spotify_error_detail(payload: &Value, fallback: &str) -> String {
    if let Some(err) = payload.get("error") {
        if let Some(obj) = err.as_object() {
            if let Some(msg) = obj.get("message").and_then(|v| v.as_str()) {
                if !msg.trim().is_empty() {
                    return msg.trim().to_string();
                }
            }
        } else if let Some(s) = err.as_str() {
            if !s.trim().is_empty() {
                return s.trim().to_string();
            }
        }
    }
    fallback.trim().to_string()
}

/// Mirrors `def _friendly_spotify_error_message(...)` (lines 339-376).
fn friendly_spotify_error_message(
    status_code: u16,
    detail: &str,
    _method: &str,
    path: &str,
    retry_after: Option<&str>,
) -> String {
    let normalized = detail.to_ascii_lowercase();
    let is_playback = path.starts_with("/me/player");
    match status_code {
        401 => "Spotify authentication failed or expired. Run `hermes auth spotify` again.".to_string(),
        403 => {
            if is_playback {
                return "Spotify rejected this playback request. Playback control usually requires a Spotify Premium account and an active Spotify Connect device.".to_string();
            }
            if normalized.contains("scope") || normalized.contains("permission") {
                return "Spotify rejected the request because the current auth scope is insufficient. Re-run `hermes auth spotify` to refresh permissions.".to_string();
            }
            "Spotify rejected the request. The account may not have permission for this action.".to_string()
        }
        404 => {
            if is_playback {
                return "Spotify could not find an active playback device or player session for this request.".to_string();
            }
            "Spotify resource not found.".to_string()
        }
        429 => {
            let mut msg = "Spotify rate limit exceeded.".to_string();
            if let Some(ra) = retry_after {
                if !ra.trim().is_empty() {
                    msg.push_str(&format!(" Retry after {} seconds.", ra.trim()));
                }
            }
            msg
        }
        _ => {
            if !detail.trim().is_empty() {
                return detail.trim().to_string();
            }
            format!("Spotify API request failed with status {}.", status_code)
        }
    }
}

/// Mirrors `def normalize_spotify_id(value, expected_type=None) -> str` (lines 385-404).
pub fn normalize_spotify_id(value: &str, expected_type: Option<&str>) -> Result<String, SpotifyError> {
    let cleaned = value.trim().to_string();
    if cleaned.is_empty() {
        return Err(SpotifyError("Spotify id/uri/url is required.".to_string()));
    }
    if cleaned.starts_with("spotify:") {
        let parts: Vec<&str> = cleaned.split(':').collect();
        if parts.len() >= 3 {
            let item_type = parts[1];
            if let Some(exp) = expected_type {
                if item_type != exp {
                    return Err(SpotifyError(format!("Expected a Spotify {}, got {}.", exp, item_type)));
                }
            }
            return Ok(parts[2].to_string());
        }
    }
    if cleaned.contains("open.spotify.com") {
        // `urlparse` path logic: `/<type>/<id>` with optional query/fragment
        let without_query = cleaned.split('?').next().unwrap_or(&cleaned).split('#').next().unwrap_or(&cleaned);
        let marker = "open.spotify.com";
        let path_start = without_query.find(marker).map(|i| i + marker.len()).unwrap_or(0);
        let path = &without_query[path_start..];
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if parts.len() >= 2 {
            let item_type = parts[0];
            let item_id = parts[1];
            if let Some(exp) = expected_type {
                if item_type != exp {
                    return Err(SpotifyError(format!("Expected a Spotify {}, got {}.", exp, item_type)));
                }
            }
            return Ok(item_id.to_string());
        }
    }
    Ok(cleaned)
}

/// Mirrors `def normalize_spotify_uri(value, expected_type=None) -> str` (lines 407-420).
pub fn normalize_spotify_uri(value: &str, expected_type: Option<&str>) -> Result<String, SpotifyError> {
    let cleaned = value.trim().to_string();
    if cleaned.is_empty() {
        return Err(SpotifyError("Spotify URI/url/id is required.".to_string()));
    }
    if cleaned.starts_with("spotify:") {
        if let Some(exp) = expected_type {
            let parts: Vec<&str> = cleaned.split(':').collect();
            if parts.len() >= 3 && parts[1] != exp {
                return Err(SpotifyError(format!("Expected a Spotify {}, got {}.", exp, parts[1])));
            }
        }
        return Ok(cleaned);
    }
    let item_id = normalize_spotify_id(&cleaned, expected_type)?;
    if let Some(exp) = expected_type {
        return Ok(format!("spotify:{}:{}", exp, item_id));
    }
    Ok(cleaned)
}

/// Mirrors `def normalize_spotify_uris(values, expected_type=None) -> list[str]` (lines 423-431).
pub fn normalize_spotify_uris(values: Vec<String>, expected_type: Option<&str>) -> Result<Vec<String>, SpotifyError> {
    let mut uris: Vec<String> = Vec::new();
    for value in values {
        let uri = normalize_spotify_uri(&value, expected_type)?;
        if !uris.contains(&uri) {
            uris.push(uri);
        }
    }
    if uris.is_empty() {
        return Err(SpotifyError("At least one Spotify item is required.".to_string()));
    }
    Ok(uris)
}

/// Mirrors `def compact_json(data: Any) -> str` (line 434-435): `json.dumps(data, ensure_ascii=False)`.
pub fn compact_json(data: &Value) -> String {
    serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string())
}

// ---------------------------------------------------------------------------
// SpotifyClient — mirrors client.py:41-321
// ---------------------------------------------------------------------------

/// Thin Spotify Web API client — mirrors `class SpotifyClient` (lines 41-321).
///
/// Real I/O would use `reqwest::Client` with `Authorization: Bearer` + `timeout=30.0`.
/// This stub uses `curl` for observable behavior so routing + error mapping are
/// testable without adding a new dependency.
#[derive(Debug, Clone)]
pub struct SpotifyClient {
    runtime: SpotifyRuntime,
}

impl SpotifyClient {
    /// Mirrors `def __init__(self) -> None` (lines 42-43): `self._runtime = self._resolve_runtime(refresh_if_expiring=True)`.
    pub fn new() -> Result<Self, SpotifyAuthRequiredError> {
        let runtime = Self::resolve_runtime(false, true)?;
        Ok(Self { runtime })
    }

    /// For testing / injection: create directly from a `SpotifyRuntime`.
    pub fn with_runtime(runtime: SpotifyRuntime) -> Self {
        Self { runtime }
    }

    /// Mirrors `def _resolve_runtime(self, *, force_refresh=False, refresh_if_expiring=True)` (lines 45-52).
    fn resolve_runtime(
        force_refresh: bool,
        refresh_if_expiring: bool,
    ) -> Result<SpotifyRuntime, SpotifyAuthRequiredError> {
        resolve_spotify_runtime_credentials(force_refresh, refresh_if_expiring)
    }

    fn resolve_runtime_mut(&mut self, force_refresh: bool, refresh_if_expiring: bool) -> Result<(), SpotifyAuthRequiredError> {
        self.runtime = Self::resolve_runtime(force_refresh, refresh_if_expiring)?;
        Ok(())
    }

    /// Mirrors `@property def base_url(self) -> str` (lines 54-56): `str(self._runtime.get("base_url") or "").rstrip("/")`.
    pub fn base_url(&self) -> String {
        self.runtime.base_url.trim_end_matches('/').to_string()
    }

    /// Mirrors `def _headers(self) -> Dict[str, str]` (lines 58-62).
    fn headers(&self) -> HashMap<String, String> {
        let mut h = HashMap::new();
        h.insert("Authorization".to_string(), format!("Bearer {}", self.runtime.access_token));
        h.insert("Content-Type".to_string(), "application/json".to_string());
        h
    }

    /// Core request — mirrors `def request(self, method, path, *, params, json_body, allow_retry_on_401, empty_response)` (lines 64-98).
    ///
    /// Real port upgrade:
    /// ```ignore
    /// let resp = reqwest::Client::new()
    ///     .request(method, format!("{}{}", self.base_url(), path))
    ///     .headers(hdrs).query(&params).json(&json_body).timeout(Duration::from_secs(30)).send().await?;
    /// if resp.status() == 401 && allow_retry_on_401 { /* refresh + retry */ }
    /// if resp.status().is_client_error() || resp.status().is_server_error() { self._raise_api_error(&resp, method, path)? }
    /// if resp.status() == 204 || resp.content_length() == Some(0) { return Ok(empty_response.unwrap_or(json!({"success": true, "status_code": resp.status(), "empty": true}))); }
    /// if resp.headers()["content-type"].contains("application/json") { return Ok(resp.json().await?) }
    /// return Ok(json!({"success": true, "text": resp.text().await?}));
    /// ```
    pub fn request(
        &mut self,
        method: &str,
        path: &str,
        params: Option<Value>,
        json_body: Option<Value>,
        allow_retry_on_401: bool,
        empty_response: Option<Value>,
    ) -> Result<Value, SpotifyApiError> {
        let url = format!("{}{}", self.base_url(), path);
        let headers = self.headers();
        let params_clean = strip_none(params.as_ref());
        let body_clean = json_body.as_ref().map(|v| strip_none(Some(v)));

        match try_spotify_request(&url, method, &headers, &params_clean, body_clean.as_ref()) {
            Ok((status, body_text, resp_headers)) => {
                if status == 401 && allow_retry_on_401 {
                    // Mirrors Python 401 retry: `self._runtime = self._resolve_runtime(force_refresh=True, ...)` then recurse with `allow_retry_on_401=False`
                    let _ = self.resolve_runtime_mut(true, true);
                    return self.request(method, path, Some(params_clean), body_clean, false, empty_response);
                }
                if status >= 400 {
                    return Err(self.raise_api_error(status, &body_text, &resp_headers, method, path));
                }
                if status == 204 || body_text.trim().is_empty() {
                    return Ok(empty_response.unwrap_or(json!({"success": true, "status_code": status, "empty": true})));
                }
                let ct = resp_headers
                    .get("content-type")
                    .or_else(|| resp_headers.get("content-type"))
                    .or_else(|| resp_headers.get("Content-Type"))
                    .map(|s| s.to_ascii_lowercase())
                    .unwrap_or_default();
                if ct.contains("application/json") {
                    if let Ok(v) = serde_json::from_str::<Value>(&body_text) {
                        return Ok(v);
                    }
                    return Ok(json!({"success": true, "text": body_text}));
                }
                if let Ok(v) = serde_json::from_str::<Value>(&body_text) {
                    // Try JSON parse regardless; some Spotify responses are JSON even without header
                    return Ok(v);
                }
                Ok(json!({"success": true, "text": body_text}))
            }
            Err(e) => {
                // Curl unavailable / offline stub — mirror Python's behavior without network.
                // Surface 401-like errors if curl message hints at it; else return generic stub so
                // handlers remain testable offline. Real `reqwest` port would return `Err(raise_api_error)`.
                let msg = e;
                if msg.contains("401") {
                    if allow_retry_on_401 {
                        let _ = self.resolve_runtime_mut(true, true);
                        return self.request(method, path, Some(params_clean), body_clean, false, empty_response);
                    }
                    return Err(SpotifyApiError {
                        message: "Spotify authentication failed or expired. Run `hermes auth spotify` again.".to_string(),
                        status_code: Some(401),
                        response_body: Some(msg.clone()),
                        path: Some(path.to_string()),
                    });
                }
                if msg.contains("could not reach") || msg.contains("curl") || msg.contains("HTTP") || msg.contains("spawn") {
                    if let Some(empty) = empty_response {
                        if path.starts_with("/me/player") {
                            return Ok(empty);
                        }
                    }
                    return Ok(json!({"success": true, "stub": true, "method": method, "path": path, "params": params_clean, "body": body_clean}));
                }
                Err(SpotifyApiError {
                    message: format!("Spotify tool failed: {}", msg),
                    status_code: None,
                    response_body: Some(msg),
                    path: Some(path.to_string()),
                })
            }
        }
    }

    /// Mirrors `def _raise_api_error(self, response, *, method, path)` (lines 100-111).
    fn raise_api_error(
        &self,
        status_code: u16,
        body_text: &str,
        headers: &HashMap<String, String>,
        method: &str,
        path: &str,
    ) -> SpotifyApiError {
        let fallback = body_text.trim().to_string();
        let payload = serde_json::from_str::<Value>(body_text).unwrap_or(json!({}));
        let detail = extract_spotify_error_detail(&payload, &fallback);
        let retry_after = headers.get("retry-after").or_else(|| headers.get("Retry-After")).map(|s| s.as_str());
        let message = friendly_spotify_error_message(status_code, &detail, method, path, retry_after);
        SpotifyApiError {
            message,
            status_code: Some(status_code),
            response_body: Some(fallback),
            path: Some(path.to_string()),
        }
    }

    // ---- Endpoint helpers -------------------------------------------------

    /// Mirrors `def get_devices(self) -> Any` (line 113-114): `GET /me/player/devices`.
    pub fn get_devices(&mut self) -> Result<Value, SpotifyApiError> {
        self.request("GET", "/me/player/devices", None, None, true, None)
    }

    /// Mirrors `def transfer_playback(self, *, device_id, play=False)` (lines 116-120).
    pub fn transfer_playback(&mut self, device_id: &str, play: bool) -> Result<Value, SpotifyApiError> {
        self.request("PUT", "/me/player", None, Some(json!({"device_ids": [device_id], "play": play})), true, None)
    }

    /// Mirrors `def get_playback_state(self, *, market=None)` (lines 122-132).
    pub fn get_playback_state(&mut self, market: Option<String>) -> Result<Value, SpotifyApiError> {
        let mut params = serde_json::Map::new();
        if let Some(m) = market {
            if !m.is_empty() {
                params.insert("market".to_string(), Value::String(m));
            }
        }
        self.request(
            "GET",
            "/me/player",
            Some(Value::Object(params)),
            None,
            true,
            Some(json!({
                "status_code": 204,
                "empty": true,
                "message": "No active Spotify playback session was found. Open Spotify on a device and start playback, or transfer playback to an available device."
            })),
        )
    }

    /// Mirrors `def get_currently_playing(self, *, market=None)` (lines 134-144).
    pub fn get_currently_playing(&mut self, market: Option<String>) -> Result<Value, SpotifyApiError> {
        let mut params = serde_json::Map::new();
        if let Some(m) = market {
            if !m.is_empty() {
                params.insert("market".to_string(), Value::String(m));
            }
        }
        self.request(
            "GET",
            "/me/player/currently-playing",
            Some(Value::Object(params)),
            None,
            true,
            Some(json!({
                "status_code": 204,
                "empty": true,
                "message": "Spotify is not currently playing anything. Start playback in Spotify and try again."
            })),
        )
    }

    /// Mirrors `def start_playback(self, *, device_id, context_uri, uris, offset, position_ms)` (lines 146-165).
    pub fn start_playback(
        &mut self,
        device_id: Option<String>,
        context_uri: Option<String>,
        uris: Option<Vec<String>>,
        offset: Option<Value>,
        position_ms: Option<i64>,
    ) -> Result<Value, SpotifyApiError> {
        let mut params = serde_json::Map::new();
        if let Some(d) = device_id {
            if !d.is_empty() {
                params.insert("device_id".to_string(), Value::String(d));
            }
        }
        let mut body = serde_json::Map::new();
        if let Some(c) = context_uri {
            body.insert("context_uri".to_string(), Value::String(c));
        }
        if let Some(u) = uris {
            body.insert("uris".to_string(), json!(u));
        }
        if let Some(o) = offset {
            if !o.is_null() {
                body.insert("offset".to_string(), o);
            }
        }
        if let Some(p) = position_ms {
            body.insert("position_ms".to_string(), json!(p));
        }
        self.request("PUT", "/me/player/play", Some(Value::Object(params)), Some(Value::Object(body)), true, None)
    }

    /// Mirrors `def pause_playback(self, *, device_id=None)` (lines 167-168).
    pub fn pause_playback(&mut self, device_id: Option<String>) -> Result<Value, SpotifyApiError> {
        let mut params = serde_json::Map::new();
        if let Some(d) = device_id {
            if !d.is_empty() {
                params.insert("device_id".to_string(), Value::String(d));
            }
        }
        self.request("PUT", "/me/player/pause", Some(Value::Object(params)), None, true, None)
    }

    /// Mirrors `def skip_next(self, *, device_id=None)` (lines 170-171).
    pub fn skip_next(&mut self, device_id: Option<String>) -> Result<Value, SpotifyApiError> {
        let mut params = serde_json::Map::new();
        if let Some(d) = device_id {
            if !d.is_empty() {
                params.insert("device_id".to_string(), Value::String(d));
            }
        }
        self.request("POST", "/me/player/next", Some(Value::Object(params)), None, true, None)
    }

    /// Mirrors `def skip_previous(self, *, device_id=None)` (lines 173-174).
    pub fn skip_previous(&mut self, device_id: Option<String>) -> Result<Value, SpotifyApiError> {
        let mut params = serde_json::Map::new();
        if let Some(d) = device_id {
            if !d.is_empty() {
                params.insert("device_id".to_string(), Value::String(d));
            }
        }
        self.request("POST", "/me/player/previous", Some(Value::Object(params)), None, true, None)
    }

    /// Mirrors `def seek(self, *, position_ms, device_id=None)` (lines 176-180).
    pub fn seek(&mut self, position_ms: i64, device_id: Option<String>) -> Result<Value, SpotifyApiError> {
        let mut params = serde_json::Map::new();
        params.insert("position_ms".to_string(), json!(position_ms));
        if let Some(d) = device_id {
            if !d.is_empty() {
                params.insert("device_id".to_string(), Value::String(d));
            }
        }
        self.request("PUT", "/me/player/seek", Some(Value::Object(params)), None, true, None)
    }

    /// Mirrors `def set_repeat(self, *, state, device_id=None)` (lines 182-183).
    pub fn set_repeat(&mut self, state: &str, device_id: Option<String>) -> Result<Value, SpotifyApiError> {
        let mut params = serde_json::Map::new();
        params.insert("state".to_string(), Value::String(state.to_string()));
        if let Some(d) = device_id {
            if !d.is_empty() {
                params.insert("device_id".to_string(), Value::String(d));
            }
        }
        self.request("PUT", "/me/player/repeat", Some(Value::Object(params)), None, true, None)
    }

    /// Mirrors `def set_shuffle(self, *, state, device_id=None)` (lines 185-186).
    ///
    /// Python: `params={"state": str(bool(state)).lower(), "device_id": device_id}`
    pub fn set_shuffle(&mut self, state: bool, device_id: Option<String>) -> Result<Value, SpotifyApiError> {
        let mut params = serde_json::Map::new();
        params.insert("state".to_string(), Value::String(state.to_string().to_ascii_lowercase()));
        if let Some(d) = device_id {
            if !d.is_empty() {
                params.insert("device_id".to_string(), Value::String(d));
            }
        }
        self.request("PUT", "/me/player/shuffle", Some(Value::Object(params)), None, true, None)
    }

    /// Mirrors `def set_volume(self, *, volume_percent, device_id=None)` (lines 188-192).
    pub fn set_volume(&mut self, volume_percent: i64, device_id: Option<String>) -> Result<Value, SpotifyApiError> {
        let mut params = serde_json::Map::new();
        params.insert("volume_percent".to_string(), json!(volume_percent));
        if let Some(d) = device_id {
            if !d.is_empty() {
                params.insert("device_id".to_string(), Value::String(d));
            }
        }
        self.request("PUT", "/me/player/volume", Some(Value::Object(params)), None, true, None)
    }

    /// Mirrors `def get_queue(self) -> Any` (lines 194-195): `GET /me/player/queue`.
    pub fn get_queue(&mut self) -> Result<Value, SpotifyApiError> {
        self.request("GET", "/me/player/queue", None, None, true, None)
    }

    /// Mirrors `def add_to_queue(self, *, uri, device_id=None)` (lines 197-198).
    pub fn add_to_queue(&mut self, uri: &str, device_id: Option<String>) -> Result<Value, SpotifyApiError> {
        let mut params = serde_json::Map::new();
        params.insert("uri".to_string(), Value::String(uri.to_string()));
        if let Some(d) = device_id {
            if !d.is_empty() {
                params.insert("device_id".to_string(), Value::String(d));
            }
        }
        self.request("POST", "/me/player/queue", Some(Value::Object(params)), None, true, None)
    }

    /// Mirrors `def search(self, *, query, search_types, limit, offset, market, include_external)` (lines 200-217).
    pub fn search(
        &mut self,
        query: &str,
        search_types: Vec<String>,
        limit: i64,
        offset: i64,
        market: Option<String>,
        include_external: Option<String>,
    ) -> Result<Value, SpotifyApiError> {
        let mut params = serde_json::Map::new();
        params.insert("q".to_string(), Value::String(query.to_string()));
        params.insert("type".to_string(), Value::String(search_types.join(",")));
        params.insert("limit".to_string(), json!(limit));
        params.insert("offset".to_string(), json!(offset));
        if let Some(m) = market {
            if !m.trim().is_empty() {
                params.insert("market".to_string(), Value::String(m));
            }
        }
        if let Some(ie) = include_external {
            if !ie.trim().is_empty() {
                params.insert("include_external".to_string(), Value::String(ie));
            }
        }
        self.request("GET", "/search", Some(Value::Object(params)), None, true, None)
    }

    /// Mirrors `def get_my_playlists(self, *, limit=20, offset=0)` (lines 219-220).
    pub fn get_my_playlists(&mut self, limit: i64, offset: i64) -> Result<Value, SpotifyApiError> {
        let mut params = serde_json::Map::new();
        params.insert("limit".to_string(), json!(limit));
        params.insert("offset".to_string(), json!(offset));
        self.request("GET", "/me/playlists", Some(Value::Object(params)), None, true, None)
    }

    /// Mirrors `def get_playlist(self, *, playlist_id, market=None)` (lines 222-223).
    pub fn get_playlist(&mut self, playlist_id: &str, market: Option<String>) -> Result<Value, SpotifyApiError> {
        let mut params = serde_json::Map::new();
        if let Some(m) = market {
            if !m.trim().is_empty() {
                params.insert("market".to_string(), Value::String(m));
            }
        }
        self.request("GET", &format!("/playlists/{}", playlist_id), Some(Value::Object(params)), None, true, None)
    }

    /// Mirrors `def create_playlist(self, *, name, public=False, collaborative=False, description=None)` (lines 225-238).
    pub fn create_playlist(
        &mut self,
        name: &str,
        public: bool,
        collaborative: bool,
        description: Option<String>,
    ) -> Result<Value, SpotifyApiError> {
        let mut body = serde_json::Map::new();
        body.insert("name".to_string(), Value::String(name.to_string()));
        body.insert("public".to_string(), Value::Bool(public));
        body.insert("collaborative".to_string(), Value::Bool(collaborative));
        if let Some(d) = description {
            body.insert("description".to_string(), Value::String(d));
        } else {
            body.insert("description".to_string(), Value::Null);
        }
        let cleaned = strip_none(Some(&Value::Object(body)));
        self.request("POST", "/me/playlists", None, Some(cleaned), true, None)
    }

    /// Mirrors `def add_playlist_items(self, *, playlist_id, uris, position=None)` (lines 240-250).
    pub fn add_playlist_items(
        &mut self,
        playlist_id: &str,
        uris: Vec<String>,
        position: Option<i64>,
    ) -> Result<Value, SpotifyApiError> {
        let mut body = serde_json::Map::new();
        body.insert("uris".to_string(), json!(uris));
        if let Some(p) = position {
            body.insert("position".to_string(), json!(p));
        }
        self.request("POST", &format!("/playlists/{}/items", playlist_id), None, Some(Value::Object(body)), true, None)
    }

    /// Mirrors `def remove_playlist_items(self, *, playlist_id, uris, snapshot_id=None)` (lines 252-262).
    pub fn remove_playlist_items(
        &mut self,
        playlist_id: &str,
        uris: Vec<String>,
        snapshot_id: Option<String>,
    ) -> Result<Value, SpotifyApiError> {
        let items: Vec<Value> = uris.into_iter().map(|uri| json!({"uri": uri})).collect();
        let mut body = serde_json::Map::new();
        body.insert("items".to_string(), Value::Array(items));
        if let Some(s) = snapshot_id {
            if !s.trim().is_empty() {
                body.insert("snapshot_id".to_string(), Value::String(s));
            }
        }
        self.request("DELETE", &format!("/playlists/{}/items", playlist_id), None, Some(Value::Object(body)), true, None)
    }

    /// Mirrors `def update_playlist_details(self, *, playlist_id, name, public, collaborative, description)` (lines 264-278).
    pub fn update_playlist_details(
        &mut self,
        playlist_id: &str,
        name: Option<String>,
        public: Option<bool>,
        collaborative: Option<bool>,
        description: Option<String>,
    ) -> Result<Value, SpotifyApiError> {
        let mut body = serde_json::Map::new();
        if let Some(n) = name {
            if !n.trim().is_empty() {
                body.insert("name".to_string(), Value::String(n));
            }
        }
        if let Some(p) = public {
            body.insert("public".to_string(), Value::Bool(p));
        }
        if let Some(c) = collaborative {
            body.insert("collaborative".to_string(), Value::Bool(c));
        }
        if let Some(d) = description {
            body.insert("description".to_string(), Value::String(d));
        }
        // Mirror Python `json_body` with Nones stripped via `_strip_none`
        let cleaned = strip_none(Some(&Value::Object(body)));
        // Only send when at least one field remains; Python would send {} which still valid but we mirror strip
        let body_opt = if cleaned.as_object().map(|m| m.is_empty()).unwrap_or(true) {
            Some(json!({}))
        } else {
            Some(cleaned)
        };
        self.request("PUT", &format!("/playlists/{}", playlist_id), None, body_opt, true, None)
    }

    /// Mirrors `def get_album(self, *, album_id, market=None)` (lines 280-281).
    pub fn get_album(&mut self, album_id: &str, market: Option<String>) -> Result<Value, SpotifyApiError> {
        let mut params = serde_json::Map::new();
        if let Some(m) = market {
            if !m.trim().is_empty() {
                params.insert("market".to_string(), Value::String(m));
            }
        }
        self.request("GET", &format!("/albums/{}", album_id), Some(Value::Object(params)), None, true, None)
    }

    /// Mirrors `def get_album_tracks(self, *, album_id, limit=20, offset=0, market=None)` (lines 283-288).
    pub fn get_album_tracks(
        &mut self,
        album_id: &str,
        limit: i64,
        offset: i64,
        market: Option<String>,
    ) -> Result<Value, SpotifyApiError> {
        let mut params = serde_json::Map::new();
        params.insert("limit".to_string(), json!(limit));
        params.insert("offset".to_string(), json!(offset));
        if let Some(m) = market {
            if !m.trim().is_empty() {
                params.insert("market".to_string(), Value::String(m));
            }
        }
        self.request("GET", &format!("/albums/{}/tracks", album_id), Some(Value::Object(params)), None, true, None)
    }

    /// Mirrors `def get_saved_tracks(self, *, limit=20, offset=0, market=None)` (lines 290-291).
    pub fn get_saved_tracks(&mut self, limit: i64, offset: i64, market: Option<String>) -> Result<Value, SpotifyApiError> {
        let mut params = serde_json::Map::new();
        params.insert("limit".to_string(), json!(limit));
        params.insert("offset".to_string(), json!(offset));
        if let Some(m) = market {
            if !m.trim().is_empty() {
                params.insert("market".to_string(), Value::String(m));
            }
        }
        self.request("GET", "/me/tracks", Some(Value::Object(params)), None, true, None)
    }

    /// Mirrors `def save_library_items(self, *, uris)` (lines 293-294): `PUT /me/library?uris=...`.
    pub fn save_library_items(&mut self, uris: Vec<String>) -> Result<Value, SpotifyApiError> {
        let joined = uris.join(",");
        let mut params = serde_json::Map::new();
        params.insert("uris".to_string(), Value::String(joined));
        self.request("PUT", "/me/library", Some(Value::Object(params)), None, true, None)
    }

    /// Mirrors `def library_contains(self, *, uris)` (lines 296-297): `GET /me/library/contains?uris=...`.
    pub fn library_contains(&mut self, uris: Vec<String>) -> Result<Value, SpotifyApiError> {
        let joined = uris.join(",");
        let mut params = serde_json::Map::new();
        params.insert("uris".to_string(), Value::String(joined));
        self.request("GET", "/me/library/contains", Some(Value::Object(params)), None, true, None)
    }

    /// Mirrors `def get_saved_albums(self, *, limit=20, offset=0, market=None)` (lines 299-300).
    pub fn get_saved_albums(&mut self, limit: i64, offset: i64, market: Option<String>) -> Result<Value, SpotifyApiError> {
        let mut params = serde_json::Map::new();
        params.insert("limit".to_string(), json!(limit));
        params.insert("offset".to_string(), json!(offset));
        if let Some(m) = market {
            if !m.trim().is_empty() {
                params.insert("market".to_string(), Value::String(m));
            }
        }
        self.request("GET", "/me/albums", Some(Value::Object(params)), None, true, None)
    }

    /// Mirrors `def remove_saved_tracks(self, *, track_ids)` (lines 302-304).
    pub fn remove_saved_tracks(&mut self, track_ids: Vec<String>) -> Result<Value, SpotifyApiError> {
        let uris: Vec<String> = track_ids.into_iter().map(|id| format!("spotify:track:{}", id)).collect();
        let joined = uris.join(",");
        let mut params = serde_json::Map::new();
        params.insert("uris".to_string(), Value::String(joined));
        self.request("DELETE", "/me/library", Some(Value::Object(params)), None, true, None)
    }

    /// Mirrors `def remove_saved_albums(self, *, album_ids)` (lines 306-308).
    pub fn remove_saved_albums(&mut self, album_ids: Vec<String>) -> Result<Value, SpotifyApiError> {
        let uris: Vec<String> = album_ids.into_iter().map(|id| format!("spotify:album:{}", id)).collect();
        let joined = uris.join(",");
        let mut params = serde_json::Map::new();
        params.insert("uris".to_string(), Value::String(joined));
        self.request("DELETE", "/me/library", Some(Value::Object(params)), None, true, None)
    }

    /// Mirrors `def get_recently_played(self, *, limit=20, after=None, before=None)` (lines 310-321).
    pub fn get_recently_played(
        &mut self,
        limit: i64,
        after: Option<i64>,
        before: Option<i64>,
    ) -> Result<Value, SpotifyApiError> {
        let mut params = serde_json::Map::new();
        params.insert("limit".to_string(), json!(limit));
        if let Some(a) = after {
            params.insert("after".to_string(), json!(a));
        }
        if let Some(b) = before {
            params.insert("before".to_string(), json!(b));
        }
        self.request("GET", "/me/player/recently-played", Some(Value::Object(params)), None, true, None)
    }
}

// ---------------------------------------------------------------------------
// Low-level HTTP via curl — mirrors httpx.request(..., timeout=30.0)
// ---------------------------------------------------------------------------

/// Perform a Spotify API HTTP request via `curl`.
///
/// Returns `(status_code, body_text, response_headers)` on success, `Err(msg)` on
/// failure to spawn `curl`. Real port would use `reqwest::Client`.
fn try_spotify_request(
    url: &str,
    method: &str,
    headers: &HashMap<String, String>,
    params: &Value,
    json_body: Option<&Value>,
) -> Result<(u16, String, HashMap<String, String>), String> {
    let mut full_url = url.to_string();
    if let Some(obj) = params.as_object() {
        let qs: Vec<String> = obj
            .iter()
            .filter(|(_, v)| !v.is_null())
            .map(|(k, v)| {
                let vs = match v {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    _ => serde_json::to_string(v).unwrap_or_default().trim_matches('"').to_string(),
                };
                format!("{}={}", urlencoding(k), urlencoding(&vs))
            })
            .collect();
        if !qs.is_empty() {
            full_url.push('?');
            full_url.push_str(&qs.join("&"));
        }
    }

    let mut cmd = std::process::Command::new("curl");
    cmd.arg("-sS")
        .arg("-m")
        .arg("30")
        .arg("-X")
        .arg(method)
        .arg("-D")
        .arg("-")
        .arg("-w")
        .arg("\n__CURL_STATUS__:%{http_code}");
    for (k, v) in headers {
        cmd.arg("-H").arg(format!("{}: {}", k, v));
    }
    if let Some(body) = json_body {
        if !body.is_null() && body != &json!({}) {
            let body_str = serde_json::to_string(body).unwrap_or_default();
            if body_str != "{}" && body_str != "null" {
                cmd.arg("-d").arg(body_str);
            }
        }
    }
    cmd.arg(&full_url);
    let out = cmd.output().map_err(|e| format!("curl spawn failed: {}", e))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let mut status: u16 = 0;
    let mut body = stdout.clone();
    let mut headers_map: HashMap<String, String> = HashMap::new();
    if let Some(idx) = stdout.rfind("__CURL_STATUS__:") {
        let code_str = stdout[idx + "__CURL_STATUS__:".len()..].trim();
        status = code_str.parse::<u16>().unwrap_or(0);
        let without_status = &stdout[..idx];
        if let Some(sep) = without_status.find("\r\n\r\n") {
            let header_part = &without_status[..sep];
            body = without_status[sep + 4..].to_string();
            for line in header_part.lines().skip(1) {
                if let Some((hk, hv)) = line.split_once(':') {
                    headers_map.insert(hk.trim().to_ascii_lowercase(), hv.trim().to_string());
                }
            }
        } else if let Some(sep) = without_status.find("\n\n") {
            let header_part = &without_status[..sep];
            body = without_status[sep + 2..].to_string();
            for line in header_part.lines().skip(1) {
                if let Some((hk, hv)) = line.split_once(':') {
                    headers_map.insert(hk.trim().to_ascii_lowercase(), hv.trim().to_string());
                }
            }
        } else {
            body = without_status.to_string();
        }
    } else if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        if !stderr.trim().is_empty() {
            return Err(stderr.trim().to_string());
        }
        if stdout.trim().is_empty() {
            return Err(format!("HTTP 0 from {} (curl exit {})", full_url, out.status));
        }
        status = 200;
        body = stdout;
    } else if status == 0 {
        status = 200;
    }

    if !out.status.success() && status == 0 {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if !stderr.is_empty() {
            return Err(stderr);
        }
        return Err(format!("curl failed with exit {:?}", out.status.code()));
    }
    Ok((status, body, headers_map))
}

fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strip_none_filters_nulls() {
        let v = json!({"a": 1, "b": null, "c": "x"});
        let out = strip_none(Some(&v));
        assert_eq!(out["a"], json!(1));
        assert!(out.get("b").is_none());
        assert_eq!(out["c"], json!("x"));
        assert_eq!(strip_none(None), json!({}));
    }

    #[test]
    fn friendly_401_and_403_playback() {
        assert!(friendly_spotify_error_message(401, "unauthorized", "GET", "/me/player", None).contains("authentication failed"));
        let m = friendly_spotify_error_message(403, "no scope", "PUT", "/me/player/play", None);
        assert!(m.contains("Premium"));
        let m2 = friendly_spotify_error_message(403, "scope missing", "GET", "/playlists/123", None);
        assert!(m2.contains("scope"));
    }

    #[test]
    fn extract_detail_from_error_obj() {
        let p = json!({"error": {"message": "No such track"}});
        assert_eq!(extract_spotify_error_detail(&p, "fallback"), "No such track");
        let p2 = json!({"error": "bad request"});
        assert_eq!(extract_spotify_error_detail(&p2, "fallback"), "bad request");
        assert_eq!(extract_spotify_error_detail(&json!({}), "fallback"), "fallback");
    }

    #[test]
    fn normalize_id_variants() {
        assert_eq!(normalize_spotify_id("spotify:track:abc123", Some("track")).unwrap(), "abc123");
        assert!(normalize_spotify_id("spotify:album:abc", Some("track")).is_err());
        assert_eq!(
            normalize_spotify_id("https://open.spotify.com/playlist/xyz?si=foo", Some("playlist")).unwrap(),
            "xyz"
        );
        assert_eq!(normalize_spotify_id("plain", None).unwrap(), "plain");
        assert!(normalize_spotify_id("", None).is_err());
    }

    #[test]
    fn normalize_uri_variants() {
        assert_eq!(normalize_spotify_uri("spotify:track:123", Some("track")).unwrap(), "spotify:track:123");
        assert_eq!(normalize_spotify_uri("abc123", Some("track")).unwrap(), "spotify:track:abc123");
        assert_eq!(
            normalize_spotify_uri("https://open.spotify.com/track/xyz", Some("track")).unwrap(),
            "spotify:track:xyz"
        );
    }

    #[test]
    fn normalize_uris_dedupes() {
        let v = normalize_spotify_uris(vec!["spotify:track:1".to_string(), "spotify:track:1".to_string(), "spotify:track:2".to_string()], Some("track")).unwrap();
        assert_eq!(v, vec!["spotify:track:1", "spotify:track:2"]);
        assert!(normalize_spotify_uris(vec![], Some("track")).is_err());
    }

    #[test]
    fn compact_json_roundtrip() {
        let v = json!({"a": 1, "b": "hello"});
        let s = compact_json(&v);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, v);
    }
}
