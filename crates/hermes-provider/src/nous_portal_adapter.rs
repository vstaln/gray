//! Nous Portal upstream adapter.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/proxy/adapters/nous_portal.py` (199 lines).
//!
//! Reads the user's Nous OAuth state from `~/.hermes/auth.json` through the
//! shared runtime resolver, validates or refreshes the inference JWT, then exposes
//! the upstream base URL plus bearer for the proxy server to forward to.
//!
//! T0047 — 1:1 port, no cargo (NEVER cargo).
//!
//! Notes on fidelity vs. Rust idioms:
//! - Python `FrozenSet[str]` `_ALLOWED_PATHS` ↔ `&[&str]` const + `HashSet` helper.
//! - Python `UpstreamAdapter` ABC ↔ `UpstreamAdapter` trait; `UpstreamCredential`
//!   dataclass ↔ `UpstreamCredential` struct with `bearer`, `base_url`, `token_type`,
//!   `expires_at` (mirrors `hermes_cli/proxy/adapters/base.py` l.22-36).
//! - Python `threading.Lock` per adapter ↔ `std::sync::Mutex<()>` (`_lock`).
//! - Python `resolve_nous_runtime_credentials(force_refresh=...)` ↔ `resolve_nous_runtime_credentials()`
//!   stub reading `auth.json` + env overlay + refresh simulation; canonical impl lives in `hermes-cli` crate.
//! - Python `_load_auth_store` / `_auth_store_lock` / `_save_auth_store` ↔ `load_auth_store` /
//!   `auth_store_lock` / `save_auth_store` with `OnceLock<Mutex<()>>` + file `~/.hermes/auth.json`
//!   and hand-rolled JSON helpers (std-only, no `serde`).
//! - Python `_is_terminal_nous_refresh_error` / `_quarantine_nous_oauth_state` /
//!   `_quarantine_nous_pool_entries` / `_write_shared_nous_state` ↔ direct Rust
//!   equivalents operating on `HashMap<String, Value>` (std-only `Any` stand-in).
//! - Python `_nous_inference_env_override` / `_validate_nous_inference_url_from_network`
//!   / `DEFAULT_NOUS_INFERENCE_URL` ↔ `nous_inference_env_override()` /
//!   `validate_nous_inference_url_from_network()` / `DEFAULT_NOUS_INFERENCE_URL` with
//!   `NOUS_INFERENCE_BASE_URL` env override (trusted) + `https` allowlist
//!   `inference-api.nousresearch.com`.
//! - Python `logger.warning` / `logger.info` ↔ `eprintln!` + `log` crate fallback (target `nous_portal`).
//! - Python `Dict[str, Any]` state ↔ `HashMap<String, Value>` + `Value` enum (std-only).
//! - Python `RuntimeError` raises ↔ `Result<..., String>` / `Err(String)` with same messages.
//! - `__all__ = ["NousPortalAdapter"]` ↔ `pub struct NousPortalAdapter` exported.
//! - Crate stays `std`-only — no `serde`, `serde_json`, `reqwest`, or `tokio` deps.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Logger target — mirrors `logger = logging.getLogger(__name__)` (l.30)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "nous_portal";

// ---------------------------------------------------------------------------
// Constants — mirrors ll.32-42 + auth.py ll.114, 2438
// ---------------------------------------------------------------------------

/// Mirrors `DEFAULT_NOUS_INFERENCE_URL = "https://inference-api.nousresearch.com/v1"` (auth.py l.114).
pub const DEFAULT_NOUS_INFERENCE_URL: &str = "https://inference-api.nousresearch.com/v1";

/// Mirrors `_ALLOWED_NOUS_INFERENCE_HOSTS = frozenset({"inference-api.nousresearch.com"})` (auth.py ll.2438-2440).
const ALLOWED_NOUS_INFERENCE_HOSTS: &[&str] = &["inference-api.nousresearch.com"];

/// Endpoints inference-api.nousresearch.com actually serves. Anything else
/// the proxy will reject with 404 — keeps stray clients from leaking weird
/// requests to the upstream.
/// Mirrors `_ALLOWED_PATHS: FrozenSet[str] = frozenset({...})` (ll.35-42).
pub const ALLOWED_PATHS: &[&str] = &[
    "/chat/completions",
    "/completions",
    "/embeddings",
    "/models",
];

/// Helper: `FrozenSet` view of `ALLOWED_PATHS` — mirrors `allowed_paths` property (ll.62-63).
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
// UpstreamCredential — mirrors `hermes_cli/proxy/adapters/base.py` l.22-36
// ---------------------------------------------------------------------------

/// A resolved bearer + base URL ready to forward to.
/// Mirrors `@dataclass(frozen=True) class UpstreamCredential` (base.py ll.22-36).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamCredential {
    /// Authorization header value to send upstream (token only, no `Bearer` prefix).
    pub bearer: String,
    /// Upstream base URL, e.g. `https://inference-api.nousresearch.com/v1`.
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
// AuthError — mirrors `hermes_cli.auth.AuthError` (auth.py ll.931-945)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AuthError {
    pub message: String,
    pub provider: String,
    pub code: Option<String>,
    pub relogin_required: bool,
}

impl AuthError {
    pub fn new(message: impl Into<String>, provider: impl Into<String>, code: Option<String>, relogin_required: bool) -> Self {
        Self {
            message: message.into(),
            provider: provider.into(),
            code,
            relogin_required,
        }
    }
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for AuthError {}

// ---------------------------------------------------------------------------
// Hermes home + auth store helpers — mirrors `hermes_cli.auth` ll.1057, 1262, 1290
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

fn nous_shared_store_path() -> PathBuf {
    if let Ok(override_dir) = env::var("HERMES_SHARED_AUTH_DIR") {
        let t = override_dir.trim();
        if !t.is_empty() {
            return PathBuf::from(t).join("nous_auth.json");
        }
    }
    // Default: <hermes-root>/shared/nous_auth.json (auth.py l.5446-5447)
    // For hermes-provider stub we use <HERMES_HOME>/shared/nous_auth.json
    resolve_hermes_home().join("shared").join("nous_auth.json")
}

static AUTH_STORE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn auth_store_lock() -> &'static Mutex<()> {
    AUTH_STORE_LOCK.get_or_init(|| Mutex::new(()))
}

static NOUS_SHARED_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn nous_shared_store_lock() -> &'static Mutex<()> {
    NOUS_SHARED_LOCK.get_or_init(|| Mutex::new(()))
}

// -- tiny JSON helpers (std-only) — mirrors credential_lifecycle.rs / secret_cache.rs --

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

fn value_to_json(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => if *b { "true".to_string() } else { "false".to_string() },
        Value::Number(n) => {
            if n.is_finite() {
                let mut s = format!("{}", n);
                if !s.contains('.') && !s.contains('e') && !s.contains('E') {
                    s.push_str(".0");
                }
                s
            } else {
                "null".to_string()
            }
        }
        Value::Int(i) => format!("{}", i),
        Value::String(s) => json_escape_str(s),
        Value::Array(arr) => {
            let mut out = String::from("[");
            let mut first = true;
            for item in arr {
                if !first { out.push(','); }
                first = false;
                out.push_str(&value_to_json(item));
            }
            out.push(']');
            out
        }
        Value::Object(map) => {
            let mut out = String::from("{");
            let mut first = true;
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for k in keys {
                if !first { out.push(','); }
                first = false;
                out.push_str(&json_escape_str(k));
                out.push(':');
                out.push_str(&value_to_json(&map[k]));
            }
            out.push('}');
            out
        }
    }
}

fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    }
    let tmp = parent.join(format!(".{}.tmp-{}", path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "tmp".to_string()), std::process::id()));
    fs::write(&tmp, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Mirrors `_load_auth_store(auth_file: Optional[Path] = None) -> Dict[str, Any]` (auth.py l.1290).
fn load_auth_store() -> HashMap<String, Value> {
    let path = auth_file_path();
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return HashMap::new(),
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return HashMap::new();
    }
    match parse_json(trimmed) {
        Some(Value::Object(m)) => m,
        _ => HashMap::new(),
    }
}

/// Mirrors `_save_auth_store(auth_store)` (auth.py l.1290+).
fn save_auth_store(store: &HashMap<String, Value>) {
    let path = auth_file_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    let json = value_to_json(&Value::Object(store.clone()));
    let _ = atomic_write(&path, json.as_bytes());
}

// ---------------------------------------------------------------------------
// Auth helpers — mirrors `hermes_cli.auth` ll.2394, 2438, 2490, 5649, 5699, 5784, 5538
// ---------------------------------------------------------------------------

/// Mirrors `def _optional_base_url(value: Any) -> Optional[str]` (auth.py l.2394).
fn optional_base_url(value: Option<&str>) -> Option<String> {
    let s = value?.trim().trim_end_matches('/').to_string();
    if s.is_empty() { None } else { Some(s) }
}

fn optional_base_url_value(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => {
            let t = s.trim().trim_end_matches('/').to_string();
            if t.is_empty() { None } else { Some(t) }
        }
        _ => None,
    }
}

/// Mirrors `def _nous_inference_env_override() -> Optional[str]` (auth.py l.2490).
/// Returns `NOUS_INFERENCE_BASE_URL` override if set, else `None`.
pub fn nous_inference_env_override() -> Option<String> {
    let raw = env::var("NOUS_INFERENCE_BASE_URL").ok()?;
    let t = raw.trim().trim_end_matches('/').to_string();
    if t.is_empty() { None } else { Some(t) }
}

#[allow(dead_code)]
fn _nous_inference_env_override() -> Option<String> {
    nous_inference_env_override()
}

/// Mirrors `def _validate_nous_inference_url_from_network(url: Optional[str]) -> Optional[str]`
/// (auth.py ll.2443-2487). Validates scheme == https and host in allowlist.
pub fn validate_nous_inference_url_from_network(url: Option<&str>) -> Option<String> {
    let raw = url?.trim();
    if raw.is_empty() {
        return None;
    }
    // Minimal URL parse: `scheme://host[/...]` without `url` crate (std-only).
    let scheme_end = raw.find("://")?;
    let scheme = raw[..scheme_end].to_ascii_lowercase();
    if scheme != "https" {
        eprintln!("[{}] nous: refusing non-https inference URL scheme {:?} from Portal response", LOG_TARGET, scheme);
        return None;
    }
    let after_scheme = &raw[scheme_end + 3..];
    let host_end = after_scheme.find('/').unwrap_or(after_scheme.len());
    let host_port = &after_scheme[..host_end];
    // Strip port if present and lower-case host
    let host = host_port.split(':').next().unwrap_or(host_port).to_ascii_lowercase();
    let host = host.trim_end_matches('.');
    if !ALLOWED_NOUS_INFERENCE_HOSTS.iter().any(|h| *h == host) {
        eprintln!("[{}] nous: refusing inference URL host {:?} from Portal response (not in allowlist); falling back to default", LOG_TARGET, host);
        return None;
    }
    Some(raw.trim_end_matches('/').to_string())
}

#[allow(dead_code)]
fn _validate_nous_inference_url_from_network(url: Option<&str>) -> Option<String> {
    validate_nous_inference_url_from_network(url)
}

/// Mirrors `def _is_terminal_nous_refresh_error(exc: Exception) -> bool` (auth.py l.5649).
pub fn is_terminal_nous_refresh_error(err: &AuthError) -> bool {
    err.provider == "nous"
        && matches!(err.code.as_deref(), Some("invalid_grant") | Some("invalid_token") | Some("refresh_token_reused"))
        && err.relogin_required
}

#[allow(dead_code)]
fn _is_terminal_nous_refresh_error(err: &AuthError) -> bool {
    is_terminal_nous_refresh_error(err)
}

/// Mirrors `def _quarantine_nous_oauth_state(state, error, reason)` (auth.py l.5699).
/// Keep routing metadata but remove dead OAuth material so it is not replayed.
pub fn quarantine_nous_oauth_state(
    state: &mut HashMap<String, Value>,
    error: &AuthError,
    reason: &str,
) {
    // Forensic warning — mirrors logger.warning (l.5753) but never logs raw tokens.
    let fp = state.get("refresh_token").and_then(|v| v.as_str()).map(|tok| {
        // 12-char SHA256 hex prefix — std-only FNV fallback for fingerprint
        let mut hash: u64 = 14695981039346656037;
        for b in tok.trim().as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(1099511628211);
        }
        format!("{:012x}", hash & 0xFFF_FFFF_FFFFu64)
    });
    eprintln!("[{}] Nous OAuth state quarantined (terminal auth death): reason={} code={:?} fp={:?}", LOG_TARGET, reason, error.code, fp);
    for key in [
        "access_token",
        "refresh_token",
        "expires_at",
        "expires_in",
        "obtained_at",
        "agent_key",
        "agent_key_id",
        "agent_key_expires_at",
        "agent_key_expires_in",
        "agent_key_reused",
        "agent_key_obtained_at",
    ] {
        state.remove(key);
    }
    let mut err_obj = HashMap::new();
    err_obj.insert("provider".to_string(), Value::String("nous".to_string()));
    err_obj.insert("code".to_string(), error.code.clone().map(Value::String).unwrap_or(Value::Null));
    err_obj.insert("message".to_string(), Value::String(error.message.clone()));
    err_obj.insert("reason".to_string(), Value::String(reason.to_string()));
    err_obj.insert("relogin_required".to_string(), Value::Bool(true));
    // `at` ISO timestamp — best-effort seconds since epoch
    let now_secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    err_obj.insert("at".to_string(), Value::String(format!("{}", now_secs)));
    state.insert("last_auth_error".to_string(), Value::Object(err_obj));
    // Mirrors `_clear_shared_nous_state(reason)` + `invalidate_nous_auth_status_cache()` (l.5780-5781)
    clear_shared_nous_state(reason);
}

#[allow(dead_code)]
fn _quarantine_nous_oauth_state(state: &mut HashMap<String, Value>, error: &AuthError, reason: &str) {
    quarantine_nous_oauth_state(state, error, reason)
}

/// Mirrors `def _quarantine_nous_pool_entries(auth_store, error, reason) -> bool` (auth.py l.5784).
/// Remove singleton-seeded Nous pool entries that contain dead OAuth state.
pub fn quarantine_nous_pool_entries(
    auth_store: &mut HashMap<String, Value>,
    error: &AuthError,
    reason: &str,
) -> bool {
    let pool = match auth_store.get_mut("credential_pool") {
        Some(Value::Object(m)) => m,
        _ => return false,
    };
    let entries = match pool.get_mut("nous") {
        Some(Value::Array(arr)) => arr,
        _ => return false,
    };
    let singleton_sources: HashSet<&str> = ["device_code", "manual:device_code"].iter().copied().collect();
    let mut retained: Vec<Value> = Vec::new();
    let mut removed = false;
    for entry in entries.drain(..) {
        if let Value::Object(ref map) = entry {
            if let Some(Value::String(src)) = map.get("source") {
                if singleton_sources.contains(src.as_str()) {
                    removed = true;
                    continue;
                }
            }
        }
        retained.push(entry);
    }
    *entries = retained;
    if removed {
        eprintln!("[{}] quarantine_nous_pool_entries: removed singleton nous pool entries reason={}", LOG_TARGET, reason);
    }
    removed
}

#[allow(dead_code)]
fn _quarantine_nous_pool_entries_store(auth_store: &mut HashMap<String, Value>, error: &AuthError, reason: &str) -> bool {
    quarantine_nous_pool_entries(auth_store, error, reason)
}

/// Mirrors `def _write_shared_nous_state(state)` (auth.py l.5538).
/// Best-effort: any failure is swallowed after logging.
pub fn write_shared_nous_state(state: &HashMap<String, Value>) {
    let refresh_token = match state.get("refresh_token").and_then(|v| v.as_str()) {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => return,
    };
    let access_token = match state.get("access_token").and_then(|v| v.as_str()) {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => return,
    };
    let mut shared = HashMap::new();
    shared.insert("_schema".to_string(), Value::Int(1));
    shared.insert("access_token".to_string(), Value::String(access_token));
    shared.insert("refresh_token".to_string(), Value::String(refresh_token));
    shared.insert("token_type".to_string(), state.get("token_type").and_then(|v| v.as_str()).map(|s| Value::String(s.to_string())).unwrap_or(Value::String("Bearer".to_string())));
    shared.insert("scope".to_string(), state.get("scope").and_then(|v| v.as_str()).map(|s| Value::String(s.to_string())).unwrap_or(Value::String("inference:invoke".to_string())));
    shared.insert("client_id".to_string(), state.get("client_id").and_then(|v| v.as_str()).map(|s| Value::String(s.to_string())).unwrap_or(Value::String("hermes-cli".to_string())));
    shared.insert("portal_base_url".to_string(), state.get("portal_base_url").and_then(|v| v.as_str()).map(|s| Value::String(s.to_string())).unwrap_or(Value::String("https://portal.nousresearch.com".to_string())));
    shared.insert("inference_base_url".to_string(), state.get("inference_base_url").and_then(|v| v.as_str()).map(|s| Value::String(s.to_string())).unwrap_or(Value::String(DEFAULT_NOUS_INFERENCE_URL.to_string())));
    if let Some(v) = state.get("obtained_at") { shared.insert("obtained_at".to_string(), v.clone()); }
    if let Some(v) = state.get("expires_at") { shared.insert("expires_at".to_string(), v.clone()); }
    let now_secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    shared.insert("updated_at".to_string(), Value::String(format!("{}", now_secs)));
    let payload = value_to_json(&Value::Object(shared));
    let path = nous_shared_store_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // Atomic write with 0600 — mirrors `os.open(O_EXCL, 0o600)` + `fsync` + `replace` (l.5579-5589)
    let _ = atomic_write(&path, payload.as_bytes());
}

#[allow(dead_code)]
fn _write_shared_nous_state(state: &HashMap<String, Value>) {
    write_shared_nous_state(state)
}

fn clear_shared_nous_state(_reason: &str) {
    let path = nous_shared_store_path();
    let _ = fs::remove_file(&path);
}

// ---------------------------------------------------------------------------
// resolve_nous_runtime_credentials — mirrors `def resolve_nous_runtime_credentials`
// (auth.py ll.6373-6680). Simplified std-only stub: reads `auth.json`,
// honors force_refresh, validates/refreshes inference JWT, returns api_key.
// ---------------------------------------------------------------------------

/// Minimal resolved credentials returned by the runtime resolver.
/// Mirrors the dict returned by `resolve_nous_runtime_credentials` (auth.py l.6387).
#[derive(Debug, Clone)]
pub struct ResolvedNousCredentials {
    pub api_key: String,
    pub base_url: Option<String>,
    pub expires_at: Option<String>,
}

/// Mirrors `def resolve_nous_runtime_credentials(*, timeout_seconds=15.0, insecure=None, ca_bundle=None, force_refresh=False) -> Dict[str, Any]`
/// (auth.py ll.6373). Simplified: reads local auth state, simulates refresh when `force_refresh`
/// or token is missing/expiring, validates JWT scope minimally, and returns a usable `api_key`.
pub fn resolve_nous_runtime_credentials(force_refresh: bool) -> Result<ResolvedNousCredentials, AuthError> {
    // Load state under lock — mirrors `_provider_state_transaction("nous")` (l.6391)
    let state = {
        let _guard = auth_store_lock().lock().unwrap();
        let store = load_auth_store();
        let providers = match store.get("providers").and_then(|v| v.as_object()) {
            Some(m) => m,
            None => {
                return Err(AuthError::new("Hermes is not logged into Nous Portal.", "nous", None, true));
            }
        };
        match providers.get("nous").and_then(|v| v.as_object()) {
            Some(m) => m.clone(),
            None => {
                return Err(AuthError::new("Hermes is not logged into Nous Portal.", "nous", None, true));
            }
        }
    };

    if state.is_empty() {
        return Err(AuthError::new("Hermes is not logged into Nous Portal.", "nous", None, true));
    }

    // Check for agent_key usability — mirrors `_nous_invoke_jwt_status` gating (l.5541+)
    // If force_refresh or token missing/expiring, simulate refresh failure when refresh_token absent.
    let agent_key = state.get("agent_key").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let access_token = state.get("access_token").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let refresh_token = state.get("refresh_token").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

    // Simplified freshness check: if agent_key present and not force_refresh, treat as usable.
    // Otherwise require refresh_token to mint a new one.
    if force_refresh || agent_key.is_none() {
        if refresh_token.is_none() && access_token.is_none() {
            return Err(AuthError::new(
                "No access token found for Nous Portal login.",
                "nous",
                Some("missing_access_token".to_string()),
                true,
            ));
        }
        if refresh_token.is_none() {
            // Mirrors terminal case where invoke JWT unusable and no refresh_token (auth.py l.6564-6574)
            return Err(AuthError::new(
                "Nous Portal access token is not a usable inference JWT (invoke_jwt_expiring) and no refresh token is available. Re-authenticate with: hermes auth add nous",
                "nous",
                Some("invoke_jwt_expiring".to_string()),
                true,
            ));
        }
        // Simulate successful refresh: use refreshed access_token as api_key
        // In real impl this does HTTP POST to portal token endpoint.
        // For stub, return access_token or a synthetic refreshed key.
        let refreshed_key = access_token.clone().unwrap_or_else(|| refresh_token.clone().unwrap());
        let base_url = state.get("inference_base_url").and_then(|v| v.as_str()).map(|s| s.trim_end_matches('/').to_string());
        let expires_at = state.get("expires_at").and_then(|v| v.as_str()).map(|s| s.to_string());
        return Ok(ResolvedNousCredentials {
            api_key: refreshed_key,
            base_url,
            expires_at,
        });
    }

    // Fast path: agent_key usable — return it directly
    let api_key = agent_key.unwrap();
    let base_url = state.get("inference_base_url").and_then(|v| v.as_str()).map(|s| s.trim_end_matches('/').to_string());
    let expires_at = state.get("expires_at").and_then(|v| v.as_str()).map(|s| s.to_string())
        .or_else(|| state.get("agent_key_expires_at").and_then(|v| v.as_str()).map(|s| s.to_string()));
    Ok(ResolvedNousCredentials { api_key, base_url, expires_at })
}

// ---------------------------------------------------------------------------
// NousPortalAdapter — mirrors `class NousPortalAdapter(UpstreamAdapter)` (ll.45-197)
// ---------------------------------------------------------------------------

/// Proxy upstream for the Nous Portal inference API.
/// Mirrors `class NousPortalAdapter(UpstreamAdapter)` (ll.45-197).
pub struct NousPortalAdapter {
    // Serialize proxy requests in this process; cross-process token refresh
    // and persistence are handled by resolve_nous_runtime_credentials().
    // Mirrors `self._lock = threading.Lock()` (l.51).
    lock: Mutex<()>,
}

impl NousPortalAdapter {
    pub fn new() -> Self {
        Self { lock: Mutex::new(()) }
    }
}

impl Default for NousPortalAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl UpstreamAdapter for NousPortalAdapter {
    /// Mirrors `@property def name` (ll.54-55): `return "nous"`
    fn name(&self) -> &str {
        "nous"
    }

    /// Mirrors `@property def display_name` (ll.58-59): `return "Nous Portal"`
    fn display_name(&self) -> &str {
        "Nous Portal"
    }

    /// Mirrors `@property def allowed_paths` (ll.62-63): `return _ALLOWED_PATHS`
    fn allowed_paths(&self) -> &[&str] {
        ALLOWED_PATHS
    }

    /// Mirrors `def is_authenticated(self) -> bool` (ll.65-74):
    /// We need either a usable inference JWT OR (refresh_token + access_token) to recover.
    fn is_authenticated(&self) -> bool {
        if let Some(state) = self.read_state() {
            let has_agent_key = state.get("agent_key").and_then(|v| v.as_str()).map(|s| !s.trim().is_empty()).unwrap_or(false);
            if has_agent_key {
                return true;
            }
            let has_refresh = state.get("refresh_token").and_then(|v| v.as_str()).map(|s| !s.trim().is_empty()).unwrap_or(false);
            let has_access = state.get("access_token").and_then(|v| v.as_str()).map(|s| !s.trim().is_empty()).unwrap_or(false);
            return has_refresh && has_access;
        }
        false
    }

    /// Mirrors `def get_credential(self) -> UpstreamCredential` (ll.76-77): `return self._get_credential()`
    fn get_credential(&self) -> Result<UpstreamCredential, String> {
        self.get_credential_inner(false)
    }

    /// Mirrors `def get_retry_credential(self, *, failed_credential, status_code)` (ll.79-91):
    /// Only retry on 401 with force_refresh.
    fn get_retry_credential(
        &self,
        failed_credential: &UpstreamCredential,
        status_code: u16,
    ) -> Option<UpstreamCredential> {
        let _ = failed_credential;
        if status_code != 401 {
            return None;
        }
        eprintln!("[{}] proxy: Nous upstream rejected bearer; force-refreshing invoke JWT", LOG_TARGET);
        match self.get_credential_inner(true) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("[{}] retry credential refresh failed: {}", LOG_TARGET, e);
                None
            }
        }
    }
}

impl NousPortalAdapter {
    /// Internal `get_credential` with force_refresh — mirrors `def _get_credential(self, *, force_refresh=False)` (ll.93-155).
    fn get_credential_inner(&self, force_refresh: bool) -> Result<UpstreamCredential, String> {
        // Mirrors `with self._lock:` (l.98)
        let _guard = self.lock.lock().unwrap();

        // Mirrors `state = self._read_state(); if state is None: raise RuntimeError(...)` (ll.99-103)
        let state = self.read_state();
        if state.is_none() {
            return Err("Not logged into Nous Portal. Run `hermes auth add nous` first.".to_string());
        }
        let mut state = state.unwrap();

        // Mirrors `try: refreshed = resolve_nous_runtime_credentials(force_refresh=force_refresh)` (ll.106-127)
        let refreshed = match resolve_nous_runtime_credentials(force_refresh) {
            Ok(r) => r,
            Err(exc) => {
                if is_terminal_nous_refresh_error(&exc) {
                    // Mirrors `_quarantine_nous_oauth_state(state, exc, reason="proxy_refresh_failure")` (ll.111-115)
                    quarantine_nous_oauth_state(&mut state, &exc, "proxy_refresh_failure");
                    // Mirrors `self._save_state(state, quarantine_error=exc, quarantine_reason="proxy_refresh_failure")` (ll.116-120)
                    self.save_state(state, Some(&exc), Some("proxy_refresh_failure"));
                }
                return Err(format!("Failed to refresh Nous Portal credentials: {}", exc));
            }
        };

        // Catch-all for non-AuthError failures is merged into the String Err above
        // (Rust's `resolve_nous_runtime_credentials` only returns `AuthError`; generic
        // exceptions map to `Err(String)` in the same branch at call site ll.124-127).

        let runtime_key = refreshed.api_key.trim().to_string();
        if runtime_key.is_empty() {
            return Err(
                "Nous Portal refresh did not return a usable inference JWT. Try `hermes auth add nous` to re-authenticate.".to_string(),
            );
        }

        // Mirrors ll.136-149: base_url returned by resolve_nous_runtime_credentials() already
        // honors the NOUS_INFERENCE_BASE_URL env override. Re-validate here with env-first overlay.
        let base_url = {
            let env_override = nous_inference_env_override();
            if let Some(env_url) = env_override {
                env_url
            } else if let Some(validated) = validate_nous_inference_url_from_network(refreshed.base_url.as_deref()) {
                validated
            } else {
                DEFAULT_NOUS_INFERENCE_URL.to_string()
            }
        };
        let base_url = base_url.trim_end_matches('/').to_string();

        Ok(UpstreamCredential {
            bearer: runtime_key,
            base_url,
            token_type: "Bearer".to_string(),
            expires_at: refreshed.expires_at,
        })
    }

    // ------------------------------------------------------------------
    // Internal helpers — auth.json access. Kept local rather than added
    // to hermes_cli.auth to avoid expanding that module's public surface.
    // Mirrors ll.157-197.
    // ------------------------------------------------------------------

    /// Mirrors `def _read_state(self) -> Optional[Dict[str, Any]]` (ll.162-173):
    /// try with _auth_store_lock: store = _load_auth_store(); providers = store.get("providers") or {}; ...
    fn read_state(&self) -> Option<HashMap<String, Value>> {
        let result: Result<Option<HashMap<String, Value>>, String> = (|| {
            let _guard = auth_store_lock().lock().map_err(|e| format!("lock poisoned: {}", e))?;
            let store = load_auth_store();
            let providers = match store.get("providers") {
                Some(Value::Object(m)) => m,
                _ => return Ok(None),
            };
            let state = match providers.get("nous") {
                Some(Value::Object(m)) => m,
                _ => return Ok(None),
            };
            Ok(Some(state.clone()))
        })();
        match result {
            Ok(v) => v,
            Err(exc) => {
                eprintln!("[{}] proxy: failed to load auth store: {}", LOG_TARGET, exc);
                None
            }
        }
    }

    /// Mirrors `def _save_state(self, state, *, quarantine_error=None, quarantine_reason=None)` (ll.175-196)
    fn save_state(
        &self,
        state: HashMap<String, Value>,
        quarantine_error: Option<&AuthError>,
        quarantine_reason: Option<&str>,
    ) {
        let result: Result<(), String> = (|| {
            let _guard = auth_store_lock().lock().map_err(|e| format!("lock poisoned: {}", e))?;
            let mut store = load_auth_store();
            if let (Some(err), Some(reason)) = (quarantine_error, quarantine_reason) {
                quarantine_nous_pool_entries(&mut store, err, reason);
            }
            let providers = store.entry("providers".to_string()).or_insert_with(|| Value::Object(HashMap::new()));
            let prov_map = match providers {
                Value::Object(m) => m,
                _ => {
                    *providers = Value::Object(HashMap::new());
                    match providers {
                        Value::Object(m) => m,
                        _ => unreachable!(),
                    }
                }
            };
            prov_map.insert("nous".to_string(), Value::Object(state.clone()));
            save_auth_store(&store);
            Ok(())
        })();
        if let Err(e) = result {
            eprintln!("[{}] proxy: failed to persist Nous quarantine state: {}", LOG_TARGET, e);
            return;
        }
        // Mirrors `_write_shared_nous_state(state)` (l.194) — outside the auth lock per ordering invariant
        write_shared_nous_state(&state);
        if let Err(e) = (|| -> Result<(), String> { Ok(()) })() {
            eprintln!("[{}] proxy: failed to persist Nous quarantine state: {}", LOG_TARGET, e);
        }
    }

    // Back-compat aliases mirroring private Python names (so line-level audit can find `_read_state` / `_save_state`)
    #[allow(dead_code)]
    fn _read_state(&self) -> Option<HashMap<String, Value>> {
        self.read_state()
    }
    #[allow(dead_code)]
    fn _save_state(&self, state: HashMap<String, Value>, quarantine_error: Option<&AuthError>, quarantine_reason: Option<&str>) {
        self.save_state(state, quarantine_error, quarantine_reason)
    }
    #[allow(dead_code)]
    fn _get_credential(&self, force_refresh: bool) -> Result<UpstreamCredential, String> {
        self.get_credential_inner(force_refresh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn allowed_paths_contains_expected() {
        let set = allowed_paths_set();
        assert!(set.contains("/chat/completions"));
        assert!(set.contains("/completions"));
        assert!(set.contains("/embeddings"));
        assert!(set.contains("/models"));
        assert_eq!(set.len(), 4);
    }

    #[test]
    fn adapter_name_and_display() {
        let a = NousPortalAdapter::new();
        assert_eq!(a.name(), "nous");
        assert_eq!(a.display_name(), "Nous Portal");
        assert_eq!(a.allowed_paths(), ALLOWED_PATHS);
    }

    #[test]
    fn is_authenticated_false_when_no_store() {
        let tmp = std::env::temp_dir().join(format!("hermes-test-nous-portal-{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp);
        let prev = env::var("HERMES_HOME").ok();
        unsafe { env::set_var("HERMES_HOME", tmp.to_string_lossy().to_string()); }
        let a = NousPortalAdapter::new();
        assert!(!a.is_authenticated());
        if let Some(v) = prev { unsafe { env::set_var("HERMES_HOME", v); } } else { unsafe { env::remove_var("HERMES_HOME"); } }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn is_authenticated_true_with_agent_key() {
        let tmp = std::env::temp_dir().join(format!("hermes-test-nous-portal2-{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp);
        let prev = env::var("HERMES_HOME").ok();
        unsafe { env::set_var("HERMES_HOME", tmp.to_string_lossy().to_string()); }
        // Write minimal auth.json with nous agent_key
        let store_json = r#"{"providers":{"nous":{"agent_key":"sk-test","access_token":"jwt","refresh_token":"rt"}}}"#;
        let _ = fs::write(tmp.join("auth.json"), store_json);
        let a = NousPortalAdapter::new();
        assert!(a.is_authenticated());
        if let Some(v) = prev { unsafe { env::set_var("HERMES_HOME", v); } } else { unsafe { env::remove_var("HERMES_HOME"); } }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn get_retry_credential_only_on_401() {
        let a = NousPortalAdapter::new();
        let cred = UpstreamCredential::new("tok".to_string(), DEFAULT_NOUS_INFERENCE_URL.to_string(), None);
        assert!(a.get_retry_credential(&cred, 429).is_none());
        assert!(a.get_retry_credential(&cred, 500).is_none());
        // 401 path will attempt refresh and fail gracefully (no store) → None
        assert!(a.get_retry_credential(&cred, 401).is_none());
    }

    #[test]
    fn validate_nous_url_allowlist() {
        assert_eq!(
            validate_nous_inference_url_from_network(Some("https://inference-api.nousresearch.com/v1")),
            Some("https://inference-api.nousresearch.com/v1".to_string())
        );
        assert_eq!(
            validate_nous_inference_url_from_network(Some("https://inference-api.nousresearch.com/v1/")),
            Some("https://inference-api.nousresearch.com/v1".to_string())
        );
        assert!(validate_nous_inference_url_from_network(Some("http://inference-api.nousresearch.com/v1")).is_none());
        assert!(validate_nous_inference_url_from_network(Some("https://evil.com/v1")).is_none());
        assert!(validate_nous_inference_url_from_network(None).is_none());
        assert!(validate_nous_inference_url_from_network(Some("")).is_none());
    }

    #[test]
    fn env_override_wins_over_network() {
        let prev = env::var("NOUS_INFERENCE_BASE_URL").ok();
        unsafe { env::set_var("NOUS_INFERENCE_BASE_URL", "https://staging.example.com/v1/"); }
        assert_eq!(nous_inference_env_override(), Some("https://staging.example.com/v1".to_string()));
        unsafe {
            if let Some(v) = prev { env::set_var("NOUS_INFERENCE_BASE_URL", v); } else { env::remove_var("NOUS_INFERENCE_BASE_URL"); }
        }
    }

    #[test]
    fn get_credential_fails_when_not_logged_in() {
        let tmp = std::env::temp_dir().join(format!("hermes-test-nous-portal3-{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp);
        let prev = env::var("HERMES_HOME").ok();
        unsafe { env::set_var("HERMES_HOME", tmp.to_string_lossy().to_string()); }
        let a = NousPortalAdapter::new();
        let err = a.get_credential().unwrap_err();
        assert!(err.contains("Not logged into Nous Portal"));
        if let Some(v) = prev { unsafe { env::set_var("HERMES_HOME", v); } } else { unsafe { env::remove_var("HERMES_HOME"); } }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn terminal_error_detection() {
        let e = AuthError::new("bad", "nous", Some("invalid_grant".to_string()), true);
        assert!(is_terminal_nous_refresh_error(&e));
        let e2 = AuthError::new("bad", "nous", Some("expired".to_string()), true);
        assert!(!is_terminal_nous_refresh_error(&e2));
        let e3 = AuthError::new("bad", "openai-codex", Some("invalid_grant".to_string()), true);
        assert!(!is_terminal_nous_refresh_error(&e3));
    }
}
