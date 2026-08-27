//! xAI Grok OAuth upstream adapter.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/proxy/adapters/xai.py` (145 lines).
//!
//! Proxies requests to `https://api.x.ai/v1` via Hermes-managed OAuth
//! credentials stored in the `xai-oauth` credential pool (`~/.hermes/auth.json`).
//!
//! T0049 — 1:1 port, no cargo (NEVER cargo).
//!
//! Notes on fidelity vs. Rust idioms:
//! - Python `FrozenSet[str]` `_ALLOWED_PATHS` ↔ `&[&str]` const + `HashSet` helper.
//! - Python `UpstreamAdapter` ABC ↔ `UpstreamAdapter` trait; `UpstreamCredential`
//!   dataclass ↔ `UpstreamCredential` struct (`bearer`, `base_url`, `token_type`, `expires_at`)
//!   mirrors `hermes_cli/proxy/adapters/base.py` l.22-36.
//! - Python `threading.Lock` per adapter ↔ `std::sync::Mutex<()>` (`lock`) + `Mutex<Option<CredentialPool>>` (`pool`).
//! - Python `CredentialPool` / `PooledCredential` / `load_pool` (`agent/credential_pool.py`)
//!   ↔ minimal `CredentialPool` / `PooledCredential` stubs reading `auth.json` `credential_pool.xai-oauth`
//!   with hand-rolled JSON (std-only, no `serde`). Real refresh/rotate logic would
//!   live behind `hermes-cli` crate; this slice preserves the method signatures and
//!   the retry-contract (`mark_exhausted_and_rotate`, `try_refresh_current`).
//! - Python `DEFAULT_XAI_OAUTH_BASE_URL` (`hermes_cli.auth` l.114, `https://api.x.ai/v1`)
//!   ↔ `pub const DEFAULT_XAI_OAUTH_BASE_URL: &str = "https://api.x.ai/v1"`.
//! - Python `logger.warning` / `logger.info` ↔ `eprintln!` with target `xai`.
//! - Python `RuntimeError` raises ↔ `Result<..., String>` / `Err(String)` with identical messages.
//! - Python `getattr(entry, "runtime_api_key", None) or getattr(entry, "access_token", "")`
//!   ↔ explicit `Option<String>` field priority (`runtime_api_key` → `access_token`).
//! - `__all__ = ["XAIGrokAdapter"]` ↔ `pub struct XAIGrokAdapter` exported.
//! - Crate stays `std`-only — no `serde`, `serde_json`, `reqwest`, or `tokio` deps.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Logger target — mirrors `logger = logging.getLogger(__name__)` (l.13)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "xai";

// ---------------------------------------------------------------------------
// Constants — mirrors ll.15-28 + hermes_cli.auth DEFAULT_XAI_OAUTH_BASE_URL
// ---------------------------------------------------------------------------

/// Mirrors `_POOL_PROVIDER = "xai-oauth"` (l.15).
pub const POOL_PROVIDER: &str = "xai-oauth";

/// Mirrors `DEFAULT_XAI_OAUTH_BASE_URL = "https://api.x.ai/v1"` (hermes_cli/auth.py l.113).
pub const DEFAULT_XAI_OAUTH_BASE_URL: &str = "https://api.x.ai/v1";

/// Mirrors `auth_hint = "hermes auth add xai-oauth --type oauth"` (l.34).
pub const AUTH_HINT: &str = "hermes auth add xai-oauth --type oauth";

/// Mirrors `_ALLOWED_PATHS: FrozenSet[str] = frozenset({...})` (ll.20-28).
/// xAI's public API is OpenAI-compatible for Hermes-common endpoints; `/responses`
/// is included because Hermes' native xAI runtime uses `codex_responses` mode (l.17-19).
pub const ALLOWED_PATHS: &[&str] = &[
    "/responses",
    "/chat/completions",
    "/completions",
    "/embeddings",
    "/models",
];

/// Helper: `FrozenSet` view of `ALLOWED_PATHS` — mirrors `allowed_paths` property (ll.49-50).
pub fn allowed_paths_set() -> HashSet<&'static str> {
    ALLOWED_PATHS.iter().copied().collect()
}

// ---------------------------------------------------------------------------
// Minimal Value — mirrors `Any` / `Dict[str, Any]` payloads (std-only)
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
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
    pub fn as_object(&self) -> Option<&HashMap<String, Value>> {
        match self {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// UpstreamCredential — mirrors `hermes_cli/proxy/adapters/base.py` ll.22-36
// ---------------------------------------------------------------------------

/// A resolved bearer + base URL ready to forward to.
/// Mirrors `@dataclass(frozen=True) class UpstreamCredential` (base.py ll.22-36).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamCredential {
    /// Authorization header value to send upstream (token only, no `Bearer` prefix).
    pub bearer: String,
    /// Upstream base URL, e.g. `https://api.x.ai/v1`.
    pub base_url: String,
    /// Auth scheme — currently always `Bearer`.
    pub token_type: String,
    /// ISO-8601 expiry timestamp for the bearer, when known. Informational.
    pub expires_at: Option<String>,
}

impl UpstreamCredential {
    pub fn new(bearer: String, base_url: String, expires_at: Option<String>) -> Self {
        Self {
            bearer,
            base_url,
            token_type: "Bearer".to_string(),
            expires_at,
        }
    }
}

// ---------------------------------------------------------------------------
// UpstreamAdapter trait — mirrors `class UpstreamAdapter(ABC)` (base.py ll.38-106)
// ---------------------------------------------------------------------------

pub trait UpstreamAdapter: Send + Sync {
    fn name(&self) -> &str;
    fn display_name(&self) -> &str;
    fn allowed_paths(&self) -> &[&str];
    fn is_authenticated(&self) -> bool;
    fn get_credential(&self) -> Result<UpstreamCredential, String>;
    fn get_retry_credential(
        &self,
        failed_credential: &UpstreamCredential,
        status_code: u16,
    ) -> Option<UpstreamCredential>;
    fn describe(&self) -> String {
        match self.get_credential() {
            Ok(cred) => {
                let ttl = cred
                    .expires_at
                    .as_deref()
                    .map(|e| format!(" (expires {})", e))
                    .unwrap_or_default();
                format!("{}: {}{}", self.display_name(), cred.base_url, ttl)
            }
            Err(e) => format!("{}: not ready ({})", self.display_name(), e),
        }
    }
}

// ---------------------------------------------------------------------------
// PooledCredential / CredentialPool — mirrors `agent/credential_pool.py`
// ---------------------------------------------------------------------------

/// Minimal pool entry — mirrors `PooledCredential` dataclass (credential_pool.py ll.193-298).
/// Only the fields the xAI adapter reads are materialized; extra keys round-trip via `extra`.
#[derive(Debug, Clone, PartialEq)]
pub struct PooledCredential {
    pub id: String,
    pub provider: String,
    pub access_token: String,
    pub runtime_api_key: Option<String>,
    pub refresh_token: Option<String>,
    pub base_url: Option<String>,
    pub runtime_base_url: Option<String>,
    pub inference_base_url: Option<String>,
    pub expires_at: Option<String>,
    pub extra: HashMap<String, Value>,
}

impl PooledCredential {
    pub fn from_map(provider: &str, map: &HashMap<String, Value>) -> Self {
        let id = map
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let access_token = map
            .get("access_token")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default();
        let runtime_api_key = map.get("runtime_api_key").and_then(|v| v.as_str()).map(|s| s.to_string())
            .or_else(|| map.get("agent_key").and_then(|v| v.as_str()).map(|s| s.to_string()));
        // `runtime_api_key` in the provider runtime is coalesced from agent_key/access_token;
        // here we preserve explicit `runtime_api_key` if present for 1:1 line fidelity.
        let refresh_token = map.get("refresh_token").and_then(|v| v.as_str()).map(|s| s.to_string());
        let base_url = map.get("base_url").and_then(|v| v.as_str()).map(|s| s.to_string());
        let runtime_base_url = map
            .get("runtime_base_url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| map.get("inference_base_url").and_then(|v| v.as_str()).map(|s| s.to_string()));
        let expires_at = map.get("expires_at").and_then(|v| v.as_str()).map(|s| s.to_string());
        let mut extra = HashMap::new();
        for (k, v) in map {
            // mirror _EXTRA_KEYS round-trip: keep anything not a first-class field
            if !matches!(k.as_str(), "id" | "access_token" | "runtime_api_key" | "refresh_token" | "base_url" | "runtime_base_url" | "inference_base_url" | "expires_at" | "provider") {
                extra.insert(k.clone(), v.clone());
            }
        }
        Self {
            id,
            provider: provider.to_string(),
            access_token,
            runtime_api_key,
            refresh_token,
            base_url,
            runtime_base_url,
            inference_base_url: runtime_base_url.clone(),
            expires_at,
            extra,
        }
    }
}

/// Credential pool for a single provider — mirrors `CredentialPool` (credential_pool.py ll.713-...).
/// Thread-safe via outer adapter lock; `CredentialPool` itself is not internally locked
/// beyond what `XAIGrokAdapter` provides (mirrors Python `threading.RLock` inside the pool).
#[derive(Debug, Clone)]
pub struct CredentialPool {
    pub provider: String,
    entries: Vec<PooledCredential>,
    current_idx: Option<usize>,
}

impl CredentialPool {
    pub fn new(provider: impl Into<String>, entries: Vec<PooledCredential>) -> Self {
        Self {
            provider: provider.into(),
            entries,
            current_idx: if entries.is_empty() { None } else { Some(0) },
        }
    }

    /// Mirrors `def has_credentials(self) -> bool` (l.742): `return bool(self._entries)`.
    pub fn has_credentials(&self) -> bool {
        !self.entries.is_empty()
    }

    /// Mirrors `def has_available(self) -> bool` (l.746): at least one entry not in exhaustion cooldown.
    /// Stub: entries with `last_status == "exhausted"` or `"dead"` are unavailable; others are available.
    pub fn has_available(&self) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        // Check `extra.last_status` when present; absent → available
        for e in &self.entries {
            let status = e.extra.get("last_status").and_then(|v| v.as_str()).unwrap_or("");
            if status != "exhausted" && status != "dead" {
                return true;
            }
        }
        false
    }

    /// Mirrors `def select(self) -> Optional[PooledCredential]` / pool rotation helpers:
    /// return the current preferred entry (priority-sorted, first available).
    pub fn select(&self) -> Option<PooledCredential> {
        // Prefer current_idx if available and not dead; else first available
        if let Some(idx) = self.current_idx {
            if let Some(entry) = self.entries.get(idx) {
                let status = entry.extra.get("last_status").and_then(|v| v.as_str()).unwrap_or("");
                if status != "dead" {
                    return Some(entry.clone());
                }
            }
        }
        // Fall back to first available
        for entry in &self.entries {
            let status = entry.extra.get("last_status").and_then(|v| v.as_str()).unwrap_or("");
            if status != "dead" {
                return Some(entry.clone());
            }
        }
        // If all dead, return None (mirrors pool returning None when no available)
        None
    }

    /// Mirrors `def mark_exhausted_and_rotate(self, status_code: int) -> Optional[PooledCredential]`:
    /// mark the current entry exhausted (with 1h cooldown for 429) and rotate to next available.
    /// Returns `None` when pool has no other key to offer — the 429 will flow back to client (l.94).
    pub fn mark_exhausted_and_rotate(&mut self, status_code: u16) -> Option<PooledCredential> {
        if self.entries.is_empty() {
            return None;
        }
        let cur_idx = self.current_idx.unwrap_or(0);
        // Mark current exhausted (best-effort; persist omitted in stub — real impl calls `write_credential_pool`)
        if let Some(cur) = self.entries.get_mut(cur_idx) {
            let mut extra = cur.extra.clone();
            extra.insert("last_status".to_string(), Value::String("exhausted".to_string()));
            extra.insert("last_error_code".to_string(), Value::Int(status_code as i64));
            extra.insert(
                "last_status_at".to_string(),
                Value::String(format!("{}", SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0))),
            );
            cur.extra = extra;
        }
        // Rotate to next non-exhausted entry (circular). For 429 cooldown path, skip exhausted entries.
        if self.entries.len() == 1 {
            // Sole credential: 429 cooldown is transient; caller will still get None here
            // and the 429 flows back (l.94). Real pool with sole credential would still bench
            // it and return None — matching Python's `if refreshed is None: return None` (l.99-100).
            return None;
        }
        for offset in 1..=self.entries.len() {
            let idx = (cur_idx + offset) % self.entries.len();
            let status = self.entries[idx]
                .extra
                .get("last_status")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if status != "exhausted" && status != "dead" {
                self.current_idx = Some(idx);
                return Some(self.entries[idx].clone());
            }
        }
        // All others exhausted/dead → no rotation target
        None
    }

    /// Mirrors `def try_refresh_current(self) -> Optional[PooledCredential]`:
    /// attempt to refresh the current entry's OAuth token without marking exhausted.
    /// Stub: tries to re-read the auth store for a newer token (mirrors
    /// `_sync_xai_oauth_entry_from_auth_store` in credential_pool.py ll.1058-1114);
    /// if no refreshed token found, returns None so caller falls through to
    /// `mark_exhausted_and_rotate` (l.97-98).
    pub fn try_refresh_current(&mut self) -> Option<PooledCredential> {
        // Best-effort: re-load from disk to see if another process refreshed the token.
        // If the on-disk pool entry has a different access_token, adopt it.
        let fresh = load_pool(&self.provider);
        if fresh.entries.is_empty() {
            return None;
        }
        // Find entry with same id but different token
        if let Some(cur_idx) = self.current_idx {
            if let Some(cur) = self.entries.get(cur_idx) {
                for fresh_entry in &fresh.entries {
                    if fresh_entry.id == cur.id && fresh_entry.access_token != cur.access_token && !fresh_entry.access_token.is_empty() {
                        // Adopt refreshed token into current pool
                        self.entries[cur_idx] = fresh_entry.clone();
                        return Some(fresh_entry.clone());
                    }
                }
            }
        }
        // No refresh available → None (mirrors Python returning None when refresh not applicable)
        None
    }

    pub fn entries(&self) -> &[PooledCredential] {
        &self.entries
    }
}

// ---------------------------------------------------------------------------
// Disk helpers — mirrors `agent/credential_pool.py` + `hermes_cli.auth`
// ---------------------------------------------------------------------------

fn resolve_hermes_home() -> PathBuf {
    if let Ok(v) = env::var("HERMES_HOME") {
        let t = v.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    if let Ok(home) = env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home.trim()).join(".hermes");
        }
    }
    if let Ok(home) = env::var("USERPROFILE") {
        if !home.trim().is_empty() {
            return PathBuf::from(home.trim()).join(".hermes");
        }
    }
    PathBuf::from(".hermes")
}

fn auth_file_path() -> PathBuf {
    resolve_hermes_home().join("auth.json")
}

// -- tiny JSON helpers (std-only) — copied from nous_portal_adapter.rs pattern --

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

fn parse_json_string(s: &str) -> Option<(String, usize)> {
    let s = s.trim_start();
    if !s.starts_with('"') {
        return None;
    }
    let mut out = String::new();
    let mut chars = s[1..].chars().peekable();
    let mut consumed = 1usize;
    let mut escape = false;
    while let Some(c) = chars.next() {
        consumed += c.len_utf8();
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
                    consumed += 4;
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
            return Some((out, consumed));
        } else {
            out.push(c);
        }
    }
    None
}

fn skip_ws(s: &str, mut idx: usize) -> usize {
    let bytes = s.as_bytes();
    while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
        idx += 1;
    }
    idx
}

fn parse_value(s: &str, start: usize) -> Option<(Value, usize)> {
    let idx = skip_ws(s, start);
    let bytes = s.as_bytes();
    if idx >= bytes.len() {
        return None;
    }
    match bytes[idx] {
        b'"' => {
            let (st, consumed) = parse_json_string(&s[idx..])?;
            Some((Value::String(st), idx + consumed))
        }
        b'{' => parse_object(s, idx),
        b'[' => parse_array(s, idx),
        b't' if s[idx..].starts_with("true") => Some((Value::Bool(true), idx + 4)),
        b'f' if s[idx..].starts_with("false") => Some((Value::Bool(false), idx + 5)),
        b'n' if s[idx..].starts_with("null") => Some((Value::Null, idx + 4)),
        b'-' | b'0'..=b'9' => {
            let mut end = idx;
            while end < bytes.len()
                && (bytes[end].is_ascii_digit()
                    || bytes[end] == b'.'
                    || bytes[end] == b'-'
                    || bytes[end] == b'+'
                    || bytes[end] == b'e'
                    || bytes[end] == b'E')
            {
                end += 1;
            }
            let num_str = &s[idx..end];
            if num_str.contains('.') || num_str.contains('e') || num_str.contains('E') {
                if let Ok(n) = num_str.parse::<f64>() {
                    Some((Value::Number(n), end))
                } else {
                    None
                }
            } else if let Ok(i) = num_str.parse::<i64>() {
                Some((Value::Int(i), end))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn parse_object(s: &str, start: usize) -> Option<(Value, usize)> {
    let mut idx = skip_ws(s, start + 1);
    let mut map = HashMap::new();
    let bytes = s.as_bytes();
    if idx < bytes.len() && bytes[idx] == b'}' {
        return Some((Value::Object(map), idx + 1));
    }
    loop {
        idx = skip_ws(s, idx);
        let (key, consumed) = parse_json_string(&s[idx..])?;
        idx += consumed;
        idx = skip_ws(s, idx);
        if idx >= bytes.len() || bytes[idx] != b':' {
            return None;
        }
        idx += 1;
        let (val, next) = parse_value(s, idx)?;
        map.insert(key, val);
        idx = skip_ws(s, next);
        if idx >= bytes.len() {
            return None;
        }
        if bytes[idx] == b'}' {
            return Some((Value::Object(map), idx + 1));
        }
        if bytes[idx] != b',' {
            return None;
        }
        idx += 1;
    }
}

fn parse_array(s: &str, start: usize) -> Option<(Value, usize)> {
    let mut idx = skip_ws(s, start + 1);
    let mut arr = Vec::new();
    let bytes = s.as_bytes();
    if idx < bytes.len() && bytes[idx] == b']' {
        return Some((Value::Array(arr), idx + 1));
    }
    loop {
        let (val, next) = parse_value(s, idx)?;
        arr.push(val);
        idx = skip_ws(s, next);
        if idx >= bytes.len() {
            return None;
        }
        if bytes[idx] == b']' {
            return Some((Value::Array(arr), idx + 1));
        }
        if bytes[idx] != b',' {
            return None;
        }
        idx = skip_ws(s, idx + 1);
    }
}

fn parse_json(s: &str) -> Option<Value> {
    let (v, next) = parse_value(s, 0)?;
    let end = skip_ws(s, next);
    if s[end..].trim().is_empty() {
        Some(v)
    } else {
        Some(v)
    }
}

/// Mirrors `agent.credential_pool.load_pool(pool_provider)` / `hermes_cli.auth.read_credential_pool`.
/// Reads `auth.json` `credential_pool.<provider>` array; also falls back to
/// `providers.xai-oauth.tokens` singleton for backwards-compat seeding.
pub fn load_pool(pool_provider: &str) -> CredentialPool {
    let path = auth_file_path();
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return CredentialPool::new(pool_provider, Vec::new()),
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return CredentialPool::new(pool_provider, Vec::new());
    }
    let root = match parse_json(trimmed) {
        Some(Value::Object(m)) => m,
        _ => return CredentialPool::new(pool_provider, Vec::new()),
    };

    // Primary: `credential_pool.<provider>` array (credential_pool.py persistent store)
    if let Some(Value::Object(pool_obj)) = root.get("credential_pool") {
        if let Some(Value::Array(arr)) = pool_obj.get(pool_provider) {
            let mut entries = Vec::new();
            for item in arr {
                if let Value::Object(map) = item {
                    entries.push(PooledCredential::from_map(pool_provider, map));
                }
            }
            if !entries.is_empty() {
                return CredentialPool::new(pool_provider, entries);
            }
        }
    }
    // Also check top-level `credential_pool` as list keyed by provider id
    // Fallback: legacy `providers["xai-oauth"]` singleton (mirrors _seed_from_singletons)
    if let Some(Value::Object(providers)) = root.get("providers") {
        if let Some(Value::Object(state)) = providers.get(pool_provider).or_else(|| providers.get("xai-oauth")) {
            // `tokens` sub-object holds access/refresh for singleton-seeded entries
            let tokens = state.get("tokens").and_then(|v| v.as_object());
            let access_opt = tokens
                .and_then(|t| t.get("access_token").and_then(|v| v.as_str()))
                .or_else(|| state.get("access_token").and_then(|v| v.as_str()))
                .map(|s| s.to_string());
            if let Some(access) = access_opt {
                if !access.trim().is_empty() {
                    let mut map = HashMap::new();
                    map.insert("id".to_string(), Value::String("singleton".to_string()));
                    map.insert("access_token".to_string(), Value::String(access));
                    if let Some(rt) = tokens.and_then(|t| t.get("refresh_token").and_then(|v| v.as_str())).or_else(|| state.get("refresh_token").and_then(|v| v.as_str())) {
                        map.insert("refresh_token".to_string(), Value::String(rt.to_string()));
                    }
                    if let Some(bu) = tokens.and_then(|t| t.get("base_url").and_then(|v| v.as_str())).or_else(|| state.get("base_url").and_then(|v| v.as_str())) {
                        map.insert("base_url".to_string(), Value::String(bu.to_string()));
                    }
                    if let Some(bu) = state.get("inference_base_url").and_then(|v| v.as_str()) {
                        map.insert("runtime_base_url".to_string(), Value::String(bu.to_string()));
                    }
                    if let Some(ea) = tokens.and_then(|t| t.get("expires_at").and_then(|v| v.as_str())).or_else(|| state.get("expires_at").and_then(|v| v.as_str())) {
                        map.insert("expires_at".to_string(), Value::String(ea.to_string()));
                    }
                    if let Some(rt) = state.get("source").and_then(|v| v.as_str()) {
                        map.insert("source".to_string(), Value::String(rt.to_string()));
                    } else {
                        map.insert("source".to_string(), Value::String("device_code".to_string()));
                    }
                    let entry = PooledCredential::from_map(pool_provider, &map);
                    return CredentialPool::new(pool_provider, vec![entry]);
                }
            }
        }
    }

    CredentialPool::new(pool_provider, Vec::new())
}

// Back-compat alias mirroring private Python name
#[allow(dead_code)]
fn _load_pool() -> Option<CredentialPool> {
    let p = load_pool(POOL_PROVIDER);
    if p.has_credentials() { Some(p) } else { None }
}

// ---------------------------------------------------------------------------
// XAIGrokAdapter — mirrors `class XAIGrokAdapter(UpstreamAdapter)` (ll.31-143)
// ---------------------------------------------------------------------------

/// Proxy upstream for xAI Grok via Hermes-managed OAuth credentials.
/// Mirrors `class XAIGrokAdapter(UpstreamAdapter)` (ll.31-143).
pub struct XAIGrokAdapter {
    /// Mirrors `self._lock = threading.Lock()` (l.37).
    lock: Mutex<()>,
    /// Mirrors `self._pool: Optional[CredentialPool] = None` (l.38).
    /// Stores the last successfully loaded pool so `get_retry_credential` can
    /// reuse it without re-reading disk when already held (l.86).
    pool: Mutex<Option<CredentialPool>>,
}

impl XAIGrokAdapter {
    pub fn new() -> Self {
        Self {
            lock: Mutex::new(()),
            pool: Mutex::new(None),
        }
    }
}

impl Default for XAIGrokAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl UpstreamAdapter for XAIGrokAdapter {
    /// Mirrors `@property def name` (ll.41-42): `return "xai"`
    fn name(&self) -> &str {
        "xai"
    }

    /// Mirrors `@property def display_name` (ll.45-46): `return "xAI Grok OAuth"`
    fn display_name(&self) -> &str {
        "xAI Grok OAuth"
    }

    /// Mirrors `@property def allowed_paths` (ll.49-50): `return _ALLOWED_PATHS`
    fn allowed_paths(&self) -> &[&str] {
        ALLOWED_PATHS
    }

    /// Mirrors `def is_authenticated(self) -> bool` (ll.52-54):
    /// `pool = self._load_pool(); return bool(pool and pool.has_available())`
    fn is_authenticated(&self) -> bool {
        let pool = self.load_pool();
        pool.as_ref().map(|p| p.has_available()).unwrap_or(false)
    }

    /// Mirrors `def get_credential(self) -> UpstreamCredential` (ll.56-74).
    fn get_credential(&self) -> Result<UpstreamCredential, String> {
        // Mirrors `with self._lock:` (l.57)
        let _guard = self.lock.lock().unwrap();
        let pool = self.load_pool();
        let pool = match pool {
            Some(p) if p.has_credentials() => p,
            _ => {
                return Err(
                    "No xAI OAuth credentials found. Run `hermes auth add xai-oauth --type oauth` first.".to_string(),
                )
            }
        };
        let entry = match pool.select() {
            Some(e) => e,
            None => {
                return Err(
                    "No available xAI OAuth credentials found. Run `hermes auth reset xai-oauth` or re-authenticate with `hermes auth add xai-oauth --type oauth`."
                        .to_string(),
                )
            }
        };
        // Mirrors `self._pool = pool` (l.73)
        {
            let mut g = self.pool.lock().unwrap();
            *g = Some(pool.clone());
        }
        self.credential_from_entry(&entry)
    }

    /// Mirrors `def get_retry_credential(self, *, failed_credential, status_code)` (ll.76-109).
    fn get_retry_credential(
        &self,
        failed_credential: &UpstreamCredential,
        status_code: u16,
    ) -> Option<UpstreamCredential> {
        if status_code != 401 && status_code != 429 {
            return None;
        }
        // Mirrors `with self._lock:` (l.85)
        let _guard = self.lock.lock().unwrap();
        // Mirrors `pool = self._pool or self._load_pool()` (l.86)
        let mut pool = {
            let g = self.pool.lock().unwrap();
            g.clone()
        };
        if pool.is_none() {
            pool = self.load_pool();
        }
        let mut pool = pool?;
        let refreshed = if status_code == 429 {
            // Mirrors `refreshed = pool.mark_exhausted_and_rotate(status_code=status_code)` (l.94)
            pool.mark_exhausted_and_rotate(status_code)
        } else {
            // Mirrors `refreshed = pool.try_refresh_current(); if refreshed is None: refreshed = pool.mark_exhausted_and_rotate` (ll.96-98)
            let r = pool.try_refresh_current();
            if r.is_none() {
                pool.mark_exhausted_and_rotate(status_code)
            } else {
                r
            }
        };
        let refreshed = refreshed?;
        let retry_cred = self.credential_from_entry(&refreshed).ok()?;
        if retry_cred.bearer == failed_credential.bearer {
            return None;
        }
        eprintln!(
            "[{}] proxy: xAI upstream returned {}; retrying with rotated pool credential",
            LOG_TARGET, status_code
        );
        // Persist rotated pool for next call (mirrors Python pool mutation persisting via write_credential_pool)
        {
            let mut g = self.pool.lock().unwrap();
            *g = Some(pool);
        }
        Some(retry_cred)
    }
}

impl XAIGrokAdapter {
    /// Mirrors `def _load_pool(self) -> Optional[CredentialPool]` (ll.111-116):
    /// `try: return load_pool(_POOL_PROVIDER) except Exception: logger.warning(...); return None`
    fn load_pool(&self) -> Option<CredentialPool> {
        // `load_pool` in Rust never panics (returns empty pool on I/O error),
        // so the try/except is modeled as empty-check + warning on parse failure.
        // We still emit a warning when the pool file is unreadable but present.
        let pool = load_pool(POOL_PROVIDER);
        // If auth.json exists but failed to parse, log a warning (mirrors l.115)
        let path = auth_file_path();
        if path.exists() {
            if let Ok(text) = fs::read_to_string(&path) {
                if !text.trim().is_empty() && parse_json(text.trim()).is_none() {
                    eprintln!("[{}] proxy: failed to load xAI OAuth credential pool: auth.json parse error", LOG_TARGET);
                    return None;
                }
            }
        }
        if pool.has_credentials() {
            Some(pool)
        } else {
            // Distinguish "no file" (pool empty) from "parse error" already handled.
            // Empty pool is not an error — `is_authenticated` returns false, `get_credential` raises.
            if pool.entries().is_empty() && !path.exists() {
                // No file at all → None without warning (mirrors load_pool returning None when no creds)
                // But `load_pool` returning empty is still "no credentials" — treat as None for is_authenticated parity.
                // Return empty pool as None so callers see no credentials.
                // However we previously returned Some(empty)? Handle: empty → None
                return None;
            }
            // Check if we actually had an empty pool (no entries): treat as None with no warning
            // unless file was malformed (handled above).
            if pool.entries().is_empty() {
                return None;
            }
            Some(pool)
        }
    }

    /// Mirrors `def _credential_from_entry(self, entry: PooledCredential) -> UpstreamCredential` (ll.118-142).
    fn credential_from_entry(&self, entry: &PooledCredential) -> Result<UpstreamCredential, String> {
        // Mirrors `bearer = getattr(entry, "runtime_api_key", None) or getattr(entry, "access_token", "") or ""` (ll.119-123)
        let bearer_raw = entry
            .runtime_api_key
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string())
            .or_else(|| {
                let t = entry.access_token.trim().to_string();
                if t.is_empty() { None } else { Some(t) }
            })
            .unwrap_or_default();
        let bearer = bearer_raw.trim().to_string();
        if bearer.is_empty() {
            return Err(
                "xAI OAuth credential pool entry did not contain an access token. Re-authenticate with `hermes auth add xai-oauth --type oauth`."
                    .to_string(),
            );
        }
        // Mirrors `base_url = getattr(entry, "runtime_base_url", None) or getattr(entry, "base_url", None) or DEFAULT_XAI_OAUTH_BASE_URL` (ll.131-135)
        let base_url_raw = entry
            .runtime_base_url
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string())
            .or_else(|| {
                entry
                    .base_url
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| DEFAULT_XAI_OAUTH_BASE_URL.to_string());
        let base_url = base_url_raw.trim().trim_end_matches('/').to_string();
        let base_url = if base_url.is_empty() {
            DEFAULT_XAI_OAUTH_BASE_URL.to_string()
        } else {
            base_url
        };
        Ok(UpstreamCredential {
            bearer,
            base_url: base_url.clone(),
            token_type: "Bearer".to_string(),
            expires_at: entry.expires_at.clone(),
        })
    }

    // Back-compat aliases mirroring private Python names (for line-level audit: `_load_pool`, `_credential_from_entry`)

    #[allow(dead_code)]
    fn _load_pool(&self) -> Option<CredentialPool> {
        self.load_pool()
    }

    #[allow(dead_code)]
    fn _credential_from_entry(&self, entry: &PooledCredential) -> Result<UpstreamCredential, String> {
        self.credential_from_entry(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn with_temp_hermes_home<F: FnOnce(&Path)>(f: F) {
        let tmp = std::env::temp_dir().join(format!("hermes-test-xai-{}-{}", std::process::id(), {
            use std::time::{SystemTime, UNIX_EPOCH};
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        }));
        let _ = fs::create_dir_all(&tmp);
        let prev = env::var("HERMES_HOME").ok();
        unsafe { env::set_var("HERMES_HOME", tmp.to_string_lossy().to_string()); }
        f(&tmp);
        if let Some(v) = prev { unsafe { env::set_var("HERMES_HOME", v); } } else { unsafe { env::remove_var("HERMES_HOME"); } }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn allowed_paths_contains_expected() {
        let set = allowed_paths_set();
        assert!(set.contains("/responses"));
        assert!(set.contains("/chat/completions"));
        assert!(set.contains("/completions"));
        assert!(set.contains("/embeddings"));
        assert!(set.contains("/models"));
        assert_eq!(set.len(), 5);
    }

    #[test]
    fn adapter_name_and_display() {
        let a = XAIGrokAdapter::new();
        assert_eq!(a.name(), "xai");
        assert_eq!(a.display_name(), "xAI Grok OAuth");
        assert_eq!(a.allowed_paths(), ALLOWED_PATHS);
        assert_eq!(AUTH_HINT, "hermes auth add xai-oauth --type oauth");
        assert_eq!(POOL_PROVIDER, "xai-oauth");
    }

    #[test]
    fn default_base_url() {
        assert_eq!(DEFAULT_XAI_OAUTH_BASE_URL, "https://api.x.ai/v1");
    }

    #[test]
    fn is_authenticated_false_when_no_store() {
        with_temp_hermes_home(|_tmp| {
            let a = XAIGrokAdapter::new();
            assert!(!a.is_authenticated());
        });
    }

    #[test]
    fn get_credential_fails_when_no_credentials() {
        with_temp_hermes_home(|_tmp| {
            let a = XAIGrokAdapter::new();
            let err = a.get_credential().unwrap_err();
            assert!(err.contains("No xAI OAuth credentials found"), "err: {}", err);
            assert!(err.contains("hermes auth add xai-oauth"));
        });
    }

    #[test]
    fn get_credential_succeeds_with_pool_entry() {
        with_temp_hermes_home(|tmp| {
            // Write minimal auth.json credential_pool.xai-oauth entry
            let auth_json = r#"{"credential_pool":{"xai-oauth":[{"id":"abc123","access_token":"sk-xai-test-token","base_url":"https://api.x.ai/v1","expires_at":"2026-12-31T00:00:00Z"}]}}"#;
            let _ = fs::write(tmp.join("auth.json"), auth_json);
            let a = XAIGrokAdapter::new();
            assert!(a.is_authenticated());
            let cred = a.get_credential().expect("should succeed");
            assert_eq!(cred.bearer, "sk-xai-test-token");
            assert_eq!(cred.base_url, "https://api.x.ai/v1");
            assert_eq!(cred.expires_at.as_deref(), Some("2026-12-31T00:00:00Z"));
            assert_eq!(cred.token_type, "Bearer");
        });
    }

    #[test]
    fn credential_from_entry_prefers_runtime_api_key_and_strips() {
        let a = XAIGrokAdapter::new();
        let mut map = HashMap::new();
        map.insert("id".to_string(), Value::String("1".to_string()));
        map.insert("access_token".to_string(), Value::String("  fallback  ".to_string()));
        map.insert("runtime_api_key".to_string(), Value::String("  preferred  ".to_string()));
        map.insert("base_url".to_string(), Value::String("https://api.x.ai/v1/".to_string()));
        let entry = PooledCredential::from_map("xai-oauth", &map);
        let cred = a.credential_from_entry(&entry).unwrap();
        assert_eq!(cred.bearer, "preferred");
        assert_eq!(cred.base_url, "https://api.x.ai/v1");
    }

    #[test]
    fn credential_from_entry_falls_back_to_default_base_url() {
        let a = XAIGrokAdapter::new();
        let mut map = HashMap::new();
        map.insert("id".to_string(), Value::String("1".to_string()));
        map.insert("access_token".to_string(), Value::String("tok123".to_string()));
        // no base_url / runtime_base_url
        let entry = PooledCredential::from_map("xai-oauth", &map);
        let cred = a.credential_from_entry(&entry).unwrap();
        assert_eq!(cred.base_url, DEFAULT_XAI_OAUTH_BASE_URL);
    }

    #[test]
    fn credential_from_entry_runtime_base_url_wins() {
        let a = XAIGrokAdapter::new();
        let mut map = HashMap::new();
        map.insert("id".to_string(), Value::String("1".to_string()));
        map.insert("access_token".to_string(), Value::String("tok".to_string()));
        map.insert("base_url".to_string(), Value::String("https://old.example.com".to_string()));
        map.insert("runtime_base_url".to_string(), Value::String("https://api.x.ai/v1/".to_string()));
        let entry = PooledCredential::from_map("xai-oauth", &map);
        let cred = a.credential_from_entry(&entry).unwrap();
        assert_eq!(cred.base_url, "https://api.x.ai/v1");
    }

    #[test]
    fn credential_from_entry_errors_when_no_bearer() {
        let a = XAIGrokAdapter::new();
        let mut map = HashMap::new();
        map.insert("id".to_string(), Value::String("1".to_string()));
        map.insert("access_token".to_string(), Value::String("   ".to_string()));
        let entry = PooledCredential::from_map("xai-oauth", &map);
        let err = a.credential_from_entry(&entry).unwrap_err();
        assert!(err.contains("did not contain an access token"), "err: {}", err);
    }

    #[test]
    fn get_retry_credential_ignores_non_retry_codes() {
        with_temp_hermes_home(|tmp| {
            let auth_json = r#"{"credential_pool":{"xai-oauth":[{"id":"1","access_token":"tok1"}]}}"#;
            let _ = fs::write(tmp.join("auth.json"), auth_json);
            let a = XAIGrokAdapter::new();
            let _ = a.get_credential().unwrap(); // seed pool
            let cred = UpstreamCredential::new("tok1".to_string(), DEFAULT_XAI_OAUTH_BASE_URL.to_string(), None);
            assert!(a.get_retry_credential(&cred, 500).is_none());
            assert!(a.get_retry_credential(&cred, 400).is_none());
            assert!(a.get_retry_credential(&cred, 200).is_none());
        });
    }

    #[test]
    fn get_retry_credential_429_rotates_when_multiple_entries() {
        with_temp_hermes_home(|tmp| {
            let auth_json = r#"{"credential_pool":{"xai-oauth":[{"id":"1","access_token":"tok1"},{"id":"2","access_token":"tok2"}]}}"#;
            let _ = fs::write(tmp.join("auth.json"), auth_json);
            let a = XAIGrokAdapter::new();
            let first = a.get_credential().unwrap();
            assert_eq!(first.bearer, "tok1");
            let retry = a.get_retry_credential(&first, 429).expect("should rotate on 429");
            assert_eq!(retry.bearer, "tok2");
            assert_ne!(retry.bearer, first.bearer);
        });
    }

    #[test]
    fn get_retry_credential_returns_none_when_same_bearer() {
        with_temp_hermes_home(|tmp| {
            // Sole credential entry — 429 mark_exhausted returns None (no other key)
            let auth_json = r#"{"credential_pool":{"xai-oauth":[{"id":"1","access_token":"only-token"}]}}"#;
            let _ = fs::write(tmp.join("auth.json"), auth_json);
            let a = XAIGrokAdapter::new();
            let cred = a.get_credential().unwrap();
            // 429 with sole credential → mark_exhausted_and_rotate returns None → retry is None
            assert!(a.get_retry_credential(&cred, 429).is_none());
        });
    }

    #[test]
    fn load_pool_reads_singleton_fallback() {
        with_temp_hermes_home(|tmp| {
            // Legacy providers.xai-oauth.tokens path
            let auth_json = r#"{"providers":{"xai-oauth":{"tokens":{"access_token":"singleton-tok","refresh_token":"rt","base_url":"https://api.x.ai/v1"}}}}"#;
            let _ = fs::write(tmp.join("auth.json"), auth_json);
            let pool = load_pool("xai-oauth");
            assert!(pool.has_credentials());
            assert_eq!(pool.select().unwrap().access_token, "singleton-tok");
        });
    }

    #[test]
    fn has_available_reflects_dead_entries() {
        let mut m1 = HashMap::new();
        m1.insert("id".to_string(), Value::String("1".to_string()));
        m1.insert("access_token".to_string(), Value::String("tok1".to_string()));
        let mut m2 = HashMap::new();
        m2.insert("id".to_string(), Value::String("2".to_string()));
        m2.insert("access_token".to_string(), Value::String("tok2".to_string()));
        m2.insert("last_status".to_string(), Value::String("dead".to_string()));
        // Manually construct pool entries and verify via credential_pool logic:
        // Use PooledCredential::from_map then inject extra status
        let e1 = PooledCredential::from_map("xai-oauth", &m1);
        let mut e2 = PooledCredential::from_map("xai-oauth", &m2);
        // from_map round-trips "last_status" into extra
        assert!(e1.extra.get("last_status").is_none());
        assert_eq!(e2.extra.get("last_status").and_then(|v| v.as_str()), Some("dead"));
        let pool = CredentialPool::new("xai-oauth", vec![e1, e2]);
        assert!(pool.has_available()); // e1 still available
        let pool2 = CredentialPool::new("xai-oauth", vec![e2.clone()]);
        assert!(!pool2.has_available());
    }
}
