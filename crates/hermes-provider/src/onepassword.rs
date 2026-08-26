//! 1Password (`op` CLI) secret source.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/secret_sources/onepassword.py` (682 lines).
//!
//! Resolve provider credentials from 1Password `op://vault/item/field`
//! references at process startup so they don't have to live in plaintext in
//! `~/.hermes/.env`.
//!
//! Design summary (mirrors Python module docstring lines 1-38):
//! * Users map env-var names to `op://` references in `secrets.onepassword.env`
//!   (e.g. `OPENAI_API_KEY: "op://Private/OpenAI/api key"`).
//! * After `.env` loads, each reference is resolved with a single
//!   `op read -- <reference>` call and injected into `os.environ` (same point
//!   as the Bitwarden source).
//! * Authentication is whatever the user's `op` CLI already uses — a
//!   service-account token (`OP_SERVICE_ACCOUNT_TOKEN`) for headless boxes,
//!   or a desktop/interactive session (`OP_SESSION_*`). Hermes never
//!   authenticates on the user's behalf; it shells out to an already-trusted,
//!   already-authenticated CLI.
//! * Failures NEVER block startup. A missing `op` binary, expired auth, a
//!   bad reference, or a permission error each surface a one-line warning and
//!   Hermes continues.
//! * Successful, complete pulls are cached in-process and on disk under
//!   `<hermes_home>/cache/op_cache.json` so back-to-back short-lived `hermes`
//!   invocations don't re-shell `op` for every reference. The disk file holds
//!   only resolved secret *values*; auth material is fingerprinted, never stored.
//!
//! T0035 — 1:1 port, no cargo (NEVER cargo).
//!
//! Notes on fidelity vs. Rust idioms:
//! - Python `Optional[Path]` ↔ `Option<PathBuf>`; `Dict[str,str]` ↔ `HashMap<String,String>`.
//! - Python `os.environ` reads ↔ `std::env::var` via `get_source_environment()` shim.
//! - Python `subprocess.run` ↔ `std::process::Command` with polling timeout.
//! - Python `hashlib.sha256(...).hexdigest()[:16]` ↔ `sha256sum` probe + FNV fallback (preserves never-log-token property).
//! - Python `shutil.which` ↔ `PATH` scan.
//! - Python `tools.ansi_strip.strip_ansi` (full ECMA-48) ↔ hand-rolled `strip_ansi` without `regex` crate.
//! - `DiskCache` / `CachedFetch` / `is_valid_env_name` are re-implemented here for
//!   slice-local self-containment; canonical definitions live in `agent.secret_sources._cache`
//!   and `agent.secret_sources.base`. When the `hermes-secret` crate is assembled these
//!   collapse to the shared types.
//! - `SecretSource` trait is forward-declared here mirroring `base.SecretSource` ABC
//!   so this slice compiles standalone; merge step replaces with the canonical trait.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Configuration constants — mirrors lines 62-105
// ---------------------------------------------------------------------------

/// How long to wait for a single `op read`, in seconds — mirrors `_OP_RUN_TIMEOUT = 30` (line 68).
pub const OP_RUN_TIMEOUT_SECS: u64 = 30;

/// Default env var the official `op` CLI reads for service-account auth — mirrors `_DEFAULT_TOKEN_ENV` (line 74).
pub const DEFAULT_TOKEN_ENV: &str = "OP_SERVICE_ACCOUNT_TOKEN";

/// Env vars the `op` child actually needs — mirrors `_OP_ENV_ALLOWLIST` (lines 86-104).
/// We build a minimal allowlisted env rather than copying all of `os.environ`
/// (which, post-dotenv, holds every provider credential) into the child.
pub const OP_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "SystemRoot",
    "TMPDIR",
    "TMP",
    "TEMP",
    "XDG_CONFIG_HOME",
    "XDG_RUNTIME_DIR",
    "OP_ACCOUNT",
    "OP_CONNECT_HOST",
    "OP_CONNECT_TOKEN",
    "OP_LOAD_DESKTOP_APP_SETTINGS",
];

// ---------------------------------------------------------------------------
// Shared helpers — re-implemented for slice-local self-containment
// Mirrors `agent.secret_sources._cache` + `agent.secret_sources.base`
// ---------------------------------------------------------------------------

/// Machine-readable failure taxonomy — mirrors `base.ErrorKind` (lines 81-98).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    NotConfigured,
    BinaryMissing,
    AuthFailed,
    AuthExpired,
    RefInvalid,
    Network,
    EmptyValue,
    Timeout,
    Internal,
}

impl ErrorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorKind::NotConfigured => "not_configured",
            ErrorKind::BinaryMissing => "binary_missing",
            ErrorKind::AuthFailed => "auth_failed",
            ErrorKind::AuthExpired => "auth_expired",
            ErrorKind::RefInvalid => "ref_invalid",
            ErrorKind::Network => "network",
            ErrorKind::EmptyValue => "empty_value",
            ErrorKind::Timeout => "timeout",
            ErrorKind::Internal => "internal",
        }
    }
}

/// Outcome of one source's fetch — mirrors `base.FetchResult` (lines 101-124).
#[derive(Debug, Clone, Default)]
pub struct FetchResult {
    pub secrets: HashMap<String, String>,
    pub applied: Vec<String>,
    pub skipped: Vec<String>,
    pub warnings: Vec<String>,
    pub error: Option<String>,
    pub error_kind: Option<ErrorKind>,
    pub binary_path: Option<PathBuf>,
}

impl FetchResult {
    pub fn ok(&self) -> bool {
        self.error.is_none()
    }
}

/// Mirrors `_cache.CachedFetch` (lines 57-67).
#[derive(Debug, Clone)]
pub struct CachedFetch {
    pub secrets: HashMap<String, String>,
    pub fetched_at: f64,
}

impl CachedFetch {
    pub fn is_fresh(&self, ttl_seconds: f64) -> bool {
        if ttl_seconds <= 0.0 {
            return false;
        }
        let now = now_secs();
        (now - self.fetched_at) < ttl_seconds
    }
}

/// Validate env-var name — mirrors `base.is_valid_env_name` (lines 257-270).
/// Regex `^[A-Za-z_][A-Za-z0-9_]*$` without `regex` crate.
pub fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {},
        _ => return false,
    }
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '_') {
            return false;
        }
    }
    true
}

/// Resolve Hermes home — mirrors `hermes_constants.get_hermes_home()` + `_cache.resolve_cache_home`.
pub fn resolve_cache_home(home_path: Option<&Path>) -> PathBuf {
    if let Some(p) = home_path {
        return p.to_path_buf();
    }
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

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Thin wrapper to allow tests to reason about env without touching `os.environ` directly.
/// Mirrors `base.get_source_environment()` — here we just collect `std::env`.
pub fn get_source_environment() -> HashMap<String, String> {
    env::vars().collect()
}

fn get_source_env_val(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.is_empty() || true)
}

// ---------------------------------------------------------------------------
// Disk cache — mirrors `_cache.DiskCache` (lines 94-215)
// ---------------------------------------------------------------------------

/// Best-effort, profile-aware on-disk cache for fetched secret values.
///
/// One JSON object lives at `<hermes_home>/cache/<basename>`:
/// `{"key": "<serialized cache key>", "secrets": {...}, "fetched_at": 1.0}`
#[derive(Debug, Clone)]
pub struct DiskCache {
    basename: String,
    tmp_prefix: String,
}

impl DiskCache {
    pub fn new(basename: &str) -> Self {
        let stem = basename.split('.').next().unwrap_or(basename);
        Self {
            basename: basename.to_string(),
            tmp_prefix: format!(".{}_", stem),
        }
    }

    pub fn path(&self, home_path: Option<&Path>) -> PathBuf {
        resolve_cache_home(home_path).join("cache").join(&self.basename)
    }

    /// Return a fresh cached entry for `key`, or None — mirrors `DiskCache.read`.
    pub fn read(&self, key: &str, ttl_seconds: f64, home_path: Option<&Path>) -> Option<CachedFetch> {
        if ttl_seconds <= 0.0 {
            return None;
        }
        let path = self.path(home_path);
        let text = fs::read_to_string(&path).ok()?;
        let payload = parse_disk_cache_payload(&text)?;
        if payload.key != key {
            return None;
        }
        let entry = CachedFetch {
            secrets: payload.secrets,
            fetched_at: payload.fetched_at,
        };
        if !entry.is_fresh(ttl_seconds) {
            return None;
        }
        Some(entry)
    }

    /// Persist `entry` for `key` atomically at mode `0600` — mirrors `DiskCache.write`.
    pub fn write(&self, key: &str, entry: &CachedFetch, ttl_seconds: f64, home_path: Option<&Path>) {
        if ttl_seconds <= 0.0 {
            return;
        }
        let path = self.path(home_path);
        let cache_dir = match path.parent() {
            Some(p) => p,
            None => return,
        };
        if fs::create_dir_all(cache_dir).is_err() {
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(cache_dir, fs::Permissions::from_mode(0o700));
        }
        let payload = format!(
            "{{\"key\":{},\"secrets\":{},\"fetched_at\":{}}}",
            json_escape_str(key),
            json_string_map(&entry.secrets),
            entry.fetched_at
        );
        let tmp = cache_dir.join(format!(
            "{}{}-{:x}.tmp",
            self.tmp_prefix,
            std::process::id(),
            now_secs().to_bits()
        ));
        if fs::write(&tmp, payload.as_bytes()).is_err() {
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
        }
        let _ = fs::rename(&tmp, &path);
        let _ = fs::remove_file(&tmp);
    }

    pub fn clear(&self, home_path: Option<&Path>) {
        let path = self.path(home_path);
        let _ = fs::remove_file(&path);
    }
}

struct DiskPayload {
    key: String,
    secrets: HashMap<String, String>,
    fetched_at: f64,
}

fn parse_disk_cache_payload(text: &str) -> Option<DiskPayload> {
    let key = extract_json_string_field(text, "key")?;
    let fetched_at = extract_json_number_field(text, "fetched_at")?;
    let secrets = extract_json_string_map_field(text, "secrets")?;
    Some(DiskPayload { key, secrets, fetched_at })
}

fn extract_json_string_field(text: &str, field: &str) -> Option<String> {
    let needle = format!("\"{}\"", field);
    let idx = text.find(&needle)?;
    let after = &text[idx + needle.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    if !rest.starts_with('"') {
        return None;
    }
    parse_json_string(rest)
}

fn extract_json_number_field(text: &str, field: &str) -> Option<f64> {
    let needle = format!("\"{}\"", field);
    let idx = text.find(&needle)?;
    let after = &text[idx + needle.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    let end = rest
        .find(|c: char| c == ',' || c == '}' || c.is_whitespace())
        .unwrap_or(rest.len());
    rest[..end].trim().parse::<f64>().ok()
}

fn extract_json_string_map_field(text: &str, field: &str) -> Option<HashMap<String, String>> {
    let needle = format!("\"{}\"", field);
    let idx = text.find(&needle)?;
    let after = &text[idx + needle.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    if !rest.starts_with('{') {
        return None;
    }
    let end = find_matching_brace(rest)?;
    let inner = &rest[1..end];
    let mut map = HashMap::new();
    for pair in split_json_pairs(inner) {
        let p = pair.trim();
        if p.is_empty() {
            continue;
        }
        let colon_pos = p.find(':')?;
        let k_raw = p[..colon_pos].trim();
        let v_raw = p[colon_pos + 1..].trim();
        let k = parse_json_string(k_raw)?;
        let v = parse_json_string(v_raw)?;
        map.insert(k, v);
    }
    Some(map)
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

fn json_string_map(m: &HashMap<String, String>) -> String {
    let mut out = String::from("{");
    let mut first = true;
    for (k, v) in m {
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&json_escape_str(k));
        out.push(':');
        out.push_str(&json_escape_str(v));
    }
    out.push('}');
    out
}

// ---------------------------------------------------------------------------
// Cache — mirrors lines 106-139
// ---------------------------------------------------------------------------

/// In-process cache key — mirrors `_CacheKey = Tuple[str, str, str, str]` (line 115):
/// `(auth_fp, account, home, refs_fp)`.
pub type CacheKey = (String, String, String, String);

/// Disk cache basename — mirrors `_DISK_CACHE_BASENAME = "op_cache.json"` (line 118).
pub const DISK_CACHE_BASENAME: &str = "op_cache.json";

/// Serialize a cache key for on-disk storage, omitting home — mirrors `_disk_key_str` (lines 121-129).
pub fn disk_key_str(cache_key: &CacheKey) -> String {
    format!("{}|{}|{}", cache_key.0, cache_key.1, cache_key.3)
}

/// Full key including home for in-process L1 — mirrors `_CacheKey` tuple with home folded.
pub fn cache_key_str_full(cache_key: &CacheKey) -> String {
    format!("{}|{}|{}|{}", cache_key.0, cache_key.1, cache_key.2, cache_key.3)
}

/// In-process cache — mirrors `_CACHE: Dict[_CacheKey, CachedFetch] = {}` (line 116).
static CACHE: OnceLock<Mutex<HashMap<String, CachedFetch>>> = OnceLock::new();

fn cache_map() -> &'static Mutex<HashMap<String, CachedFetch>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Disk cache instance — mirrors `_DISK_CACHE: DiskCache = DiskCache(...)` (lines 132-134).
pub fn disk_cache() -> DiskCache {
    DiskCache::new(DISK_CACHE_BASENAME)
}

/// Path to the on-disk cache — mirrors `_disk_cache_path` (lines 137-139).
pub fn disk_cache_path(home_path: Option<&Path>) -> PathBuf {
    disk_cache().path(home_path)
}

// ---------------------------------------------------------------------------
// Reference validation + fingerprinting — mirrors lines 142-203
// ---------------------------------------------------------------------------

/// Return `(valid_refs, warnings)` from an `env` mapping — mirrors `_validate_references` (lines 147-172).
pub fn validate_references(
    references: Option<&HashMap<String, String>>,
) -> (HashMap<String, String>, Vec<String>) {
    let mut valid: HashMap<String, String> = HashMap::new();
    let mut warnings: Vec<String> = Vec::new();
    if let Some(map) = references {
        for (name, reference) in map {
            if !is_valid_env_name(name) {
                warnings.push(format!("Skipping {:?}: not a valid env-var name", name));
                continue;
            }
            // In Python, non-string values are also rejected; in Rust the map is typed
            // as String->String so this branch is vacuously true. We keep the shape
            // for audit parity: callers that pass a generic Value map should pre-filter.
            let cleaned = reference.trim();
            if !cleaned.starts_with("op://") {
                warnings.push(format!(
                    "Skipping {:?}: {:?} is not an op:// secret reference",
                    name, reference
                ));
                continue;
            }
            valid.insert(name.clone(), cleaned.to_string());
        }
    }
    (valid, warnings)
}

/// Generic validation for a loosely-typed config `env` value — mirrors the
/// `isinstance(ref, str)` + `startswith("op://")` checks. Used by the
/// `SecretSource` adapter where the `env` map originates from untyped config.
pub fn validate_references_generic(
    env_map: Option<&HashMap<String, serde_value::Value>>,
) -> (HashMap<String, String>, Vec<String>) {
    let mut valid: HashMap<String, String> = HashMap::new();
    let mut warnings: Vec<String> = Vec::new();
    let Some(map) = env_map else {
        return (valid, warnings);
    };
    for (name, value) in map {
        if !is_valid_env_name(name) {
            warnings.push(format!("Skipping {:?}: not a valid env-var name", name));
            continue;
        }
        let s = match value {
            serde_value::Value::String(st) => st.clone(),
            _ => {
                warnings.push(format!("Skipping {:?}: reference is not a string", name));
                continue;
            }
        };
        let cleaned = s.trim().to_string();
        if !cleaned.starts_with("op://") {
            warnings.push(format!(
                "Skipping {:?}: {:?} is not an op:// secret reference",
                name, s
            ));
            continue;
        }
        valid.insert(name.clone(), cleaned);
    }
    (valid, warnings)
}

/// SHA-256 hex prefix helper — tries `sha256sum` so the token never leaves the hash,
/// falls back to FNV-1a 64-bit hex (preserves never-log-token property).
fn sha256_prefix16(material: &str) -> String {
    // Try sha256sum via piped stdin (no shell injection surface).
    let attempted = (|| -> Option<String> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut child = Command::new("sha256sum")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(material.as_bytes());
        }
        let out = child.wait_with_output().ok()?;
        if !out.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let hex = stdout.split_whitespace().next()?;
        if hex.len() >= 16 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(hex[..16].to_string());
        }
        None
    })();
    if let Some(h) = attempted {
        return h;
    }
    // Fallback: FNV-1a 64-bit → 16 hex chars (deterministic, std-only).
    let mut h: u64 = 0xcbf29ce484222325;
    for b in material.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

/// SHA-256 prefix over the auth material `op` would use — mirrors `_auth_fingerprint` (lines 175-197).
pub fn auth_fingerprint(token_env: &str) -> String {
    let source_env = get_source_environment();
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!("token={}", source_env.get(token_env).map(|s| s.as_str()).unwrap_or("")));
    parts.push(format!("account={}", source_env.get("OP_ACCOUNT").map(|s| s.as_str()).unwrap_or("")));
    parts.push(format!("connect_host={}", source_env.get("OP_CONNECT_HOST").map(|s| s.as_str()).unwrap_or("")));
    parts.push(format!("connect_token={}", source_env.get("OP_CONNECT_TOKEN").map(|s| s.as_str()).unwrap_or("")));
    let mut session_keys: Vec<String> = source_env.keys().filter(|k| k.starts_with("OP_SESSION_")).cloned().collect();
    session_keys.sort();
    for key in session_keys {
        if let Some(val) = source_env.get(&key) {
            parts.push(format!("{}={}", key, val));
        }
    }
    let material = parts.join("\n");
    sha256_prefix16(&material)
}

/// SHA-256 prefix over the configured name→reference mapping — mirrors `_refs_fingerprint` (lines 200-203).
pub fn refs_fingerprint(references: &HashMap<String, String>) -> String {
    let mut keys: Vec<&String> = references.keys().collect();
    keys.sort();
    let mut material = String::new();
    for name in keys {
        if let Some(r) = references.get(name) {
            material.push_str(&format!("{}={}\n", name, r));
        }
    }
    // Strip trailing newline to match Python's `"\n".join(...)` without trailing newline
    if material.ends_with('\n') {
        material.pop();
    }
    sha256_prefix16(&material)
}

// ---------------------------------------------------------------------------
// Binary discovery — mirrors lines 206-225
// ---------------------------------------------------------------------------

/// Resolve a usable `op` binary, or None — mirrors `find_op` (lines 211-225).
///
/// When `binary_path` is set it is used verbatim and `PATH` is NOT consulted.
/// A pinned-but-missing path returns `None`; the caller surfaces a clear error.
pub fn find_op(binary_path: &str) -> Option<PathBuf> {
    if !binary_path.is_empty() {
        let pinned = PathBuf::from(binary_path);
        if pinned.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = fs::metadata(&pinned) {
                    if meta.permissions().mode() & 0o111 != 0 {
                        return Some(pinned);
                    }
                }
                // Also allow file that exists but permission check fails on some FS
                // — mirror Python's `os.access(pinned, os.X_OK)` strictness: require exec bit.
                return None;
            }
            #[cfg(windows)]
            {
                return Some(pinned);
            }
            #[cfg(not(any(unix, windows)))]
            {
                return Some(pinned);
            }
        }
        return None;
    }
    which_op().map(PathBuf::from)
}

fn which_op() -> Option<String> {
    let path_var = env::var("PATH").ok()?;
    let sep = if cfg!(windows) { ';' } else { ':' };
    for dir in path_var.split(sep) {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(if cfg!(windows) { "op.exe" } else { "op" });
        if candidate.is_file() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = fs::metadata(&candidate) {
                    if meta.permissions().mode() & 0o111 != 0 {
                        return Some(candidate.to_string_lossy().to_string());
                    }
                    continue;
                }
            }
            #[cfg(windows)]
            {
                return Some(candidate.to_string_lossy().to_string());
            }
            #[cfg(not(any(unix, windows)))]
            {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// `op read` invocation — mirrors lines 228-313
// ---------------------------------------------------------------------------

/// Remove ANSI control sequences and trim, for safe message surfacing — mirrors `_scrub` (lines 233-238).
pub fn scrub(text: &str) -> String {
    strip_ansi(text).replace('\x1b', "").trim().to_string()
}

/// Full ECMA-48 strip — mirrors `tools.ansi_strip.strip_ansi` (lines 14-74).
/// Covers CSI, OSC (BEL/ST), DCS/SOS/PM/APC, nF, Fp/Fe/Fs, 8-bit C1.
pub fn strip_ansi(text: &str) -> String {
    if text.is_empty() || !text.chars().any(|c| c == '\x1b' || ('\x80'..='\x9f').contains(&c)) {
        return text.to_string();
    }
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b == 0x1b {
            // ESC — try to consume a full sequence
            if i + 1 >= bytes.len() {
                // lone ESC — strip it (mirrors `replace("\x1b","")` follow-up)
                i += 1;
                continue;
            }
            let n1 = bytes[i + 1];
            match n1 {
                b'[' => {
                    // CSI: ESC [ ... @-~
                    let mut j = i + 2;
                    while j < bytes.len() {
                        let c = bytes[j];
                        if (0x40..=0x7e).contains(&c) {
                            j += 1;
                            break;
                        }
                        j += 1;
                    }
                    i = j;
                }
                b']' => {
                    // OSC: ESC ] ... BEL or ESC\
                    let mut j = i + 2;
                    while j < bytes.len() {
                        if bytes[j] == 0x07 {
                            j += 1;
                            break;
                        }
                        if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                            j += 2;
                            break;
                        }
                        j += 1;
                    }
                    i = j;
                }
                b'P' | b'X' | b'^' | b'_' => {
                    // DCS/SOS/PM/APC: ESC P/X/^/_ ... ESC\
                    let mut j = i + 2;
                    while j + 1 < bytes.len() {
                        if bytes[j] == 0x1b && bytes[j + 1] == b'\\' {
                            j += 2;
                            break;
                        }
                        j += 1;
                    }
                    i = j;
                }
                0x20..=0x2f => {
                    // nF: ESC 20-2F * 30-7E
                    let mut j = i + 1;
                    while j < bytes.len() && (0x20..=0x2f).contains(&bytes[j]) {
                        j += 1;
                    }
                    if j < bytes.len() && (0x30..=0x7e).contains(&bytes[j]) {
                        j += 1;
                    }
                    i = j;
                }
                0x30..=0x7e => {
                    // Fp/Fe/Fs single-byte
                    i += 2;
                }
                _ => {
                    i += 2;
                }
            }
        } else if (0x80..=0x9f).contains(&b) {
            if b == 0x9b {
                // 8-bit CSI: 9B ... @-~
                let mut j = i + 1;
                while j < bytes.len() {
                    let c = bytes[j];
                    if (0x40..=0x7e).contains(&c) {
                        j += 1;
                        break;
                    }
                    j += 1;
                }
                i = j;
            } else if b == 0x9d {
                // 8-bit OSC: 9D ... BEL/9C
                let mut j = i + 1;
                while j < bytes.len() {
                    if bytes[j] == 0x07 || bytes[j] == 0x9c {
                        j += 1;
                        break;
                    }
                    j += 1;
                }
                i = j;
            } else {
                // Other C1
                i += 1;
            }
        } else {
            // Regular character — copy as UTF-8 char
            // `bytes` may contain non-UTF8; we iterate via string slices to keep Unicode.
            // Fallback: push the byte as char if valid, else skip.
            // Use `text[i..]` to get correct char boundary.
            let remaining = &text[i..];
            if let Some(ch) = remaining.chars().next() {
                out.push(ch);
                i += ch.len_utf8();
            } else {
                i += 1;
            }
        }
    }
    out
}

/// Build a minimal allowlisted environment for the `op` child process — mirrors `_op_child_env` (lines 241-258).
pub fn op_child_env(token_value: &str) -> HashMap<String, String> {
    let source_env = get_source_environment();
    let mut env_map: HashMap<String, String> = HashMap::new();
    for key in OP_ENV_ALLOWLIST {
        if let Some(val) = source_env.get(*key) {
            env_map.insert(key.to_string(), val.clone());
        }
    }
    // Desktop / interactive session credentials — mirrors loop over OP_SESSION_*
    for (key, val) in &source_env {
        if key.starts_with("OP_SESSION_") {
            env_map.insert(key.clone(), val.clone());
        }
    }
    if !token_value.is_empty() {
        env_map.insert("OP_SERVICE_ACCOUNT_TOKEN".to_string(), token_value.to_string());
    }
    env_map.insert("NO_COLOR".to_string(), "1".to_string());
    env_map
}

/// Resolve a single `op://` reference to its value — mirrors `_run_op_read` (lines 261-313).
pub fn run_op_read(
    op: &Path,
    reference: &str,
    account: &str,
    token_value: &str,
) -> Result<String, String> {
    let mut cmd_argv: Vec<String> = Vec::new();
    cmd_argv.push(op.to_string_lossy().to_string());
    cmd_argv.push("read".to_string());
    if !account.is_empty() {
        cmd_argv.push("--account".to_string());
        cmd_argv.push(account.to_string());
    }
    cmd_argv.push("--".to_string());
    cmd_argv.push(reference.to_string());

    let child_env = op_child_env(token_value);

    // Spawn with piped stdout/stderr, stdin = null
    let mut cmd = Command::new(op);
    if !account.is_empty() {
        cmd.args(["--account", account]);
    }
    cmd.args(["--", reference]);
    cmd.env_clear();
    cmd.envs(&child_env);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| format!("failed to invoke op: {}", e))?;

    // Polling timeout — mirrors `subprocess.run(..., timeout=_OP_RUN_TIMEOUT)`
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(OP_RUN_TIMEOUT_SECS);
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "op read timed out after {}s for {:?}",
                        OP_RUN_TIMEOUT_SECS, reference
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            Err(e) => return Err(format!("failed to invoke op: {}", e)),
        }
    }

    let output = child.wait_with_output().map_err(|e| format!("failed to invoke op: {}", e))?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let err = scrub(&stderr);
        let snippet = if err.len() > 200 { &err[..200] } else { &err };
        if !snippet.is_empty() {
            return Err(format!("op read failed for {:?}: {}", reference, snippet));
        }
        return Err(format!("op read exited {} for {:?}", code, reference));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    // `op` appends a trailing newline; strip only that so intentional internal/edge spaces survive.
    // But a value that is empty or whitespace-only is treated as empty — mirrors lines 307-312.
    let value = stdout.trim_end_matches(|c| c == '\r' || c == '\n').to_string();
    if value.trim().is_empty() {
        return Err(format!("op read returned an empty value for {:?}", reference));
    }
    Ok(value)
}

// ---------------------------------------------------------------------------
// Fetch — mirrors lines 316-388
// ---------------------------------------------------------------------------

/// Resolve `references` (name → `op://…`) to `(secrets, warnings)` — mirrors `fetch_onepassword_secrets` (lines 321-388).
///
/// Raises `Err` only when no `op` binary is available — a fatal "can't fetch anything" condition.
/// Per-reference failures are collected as warnings and the reference is dropped.
pub fn fetch_onepassword_secrets(
    references: &HashMap<String, String>,
    account: &str,
    token_env: &str,
    binary: Option<&Path>,
    binary_path: &str,
    use_cache: bool,
    cache_ttl_seconds: f64,
    home_path: Option<&Path>,
) -> Result<(HashMap<String, String>, Vec<String>), String> {
    let (valid, mut warnings) = validate_references(Some(references));
    if valid.is_empty() {
        return Ok((HashMap::new(), warnings));
    }

    let token_value = env::var(token_env).unwrap_or_default().trim().to_string();
    let auth_fp = auth_fingerprint(token_env);
    let refs_fp = refs_fingerprint(&valid);
    let home_str = home_path.map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let cache_key: CacheKey = (auth_fp.clone(), account.to_string(), home_str.clone(), refs_fp.clone());

    if use_cache {
        let full_key = cache_key_str_full(&cache_key);
        if let Ok(guard) = cache_map().lock() {
            if let Some(entry) = guard.get(&full_key) {
                if entry.is_fresh(cache_ttl_seconds) {
                    return Ok((entry.secrets.clone(), warnings));
                }
            }
        }
        let disk_key = disk_key_str(&cache_key);
        if let Some(entry) = disk_cache().read(&disk_key, cache_ttl_seconds, home_path) {
            if let Ok(mut guard) = cache_map().lock() {
                guard.insert(full_key.clone(), entry.clone());
            }
            return Ok((entry.secrets.clone(), warnings));
        }
    }

    let op: PathBuf = match binary {
        Some(p) => p.to_path_buf(),
        None => find_op(binary_path).ok_or_else(|| {
            "op CLI not found.  Install the 1Password CLI (https://developer.1password.com/docs/cli/get-started/) or set secrets.onepassword.binary_path to its absolute location.".to_string()
        })?,
    };

    let mut secrets: HashMap<String, String> = HashMap::new();
    let mut read_errors: usize = 0;
    let mut sorted_names: Vec<&String> = valid.keys().collect();
    sorted_names.sort();
    for name in sorted_names {
        let reference = valid.get(name).unwrap();
        match run_op_read(&op, reference, account, &token_value) {
            Ok(v) => {
                secrets.insert(name.clone(), v);
            }
            Err(exc) => {
                warnings.push(exc);
                read_errors += 1;
            }
        }
    }

    if use_cache && read_errors == 0 && !secrets.is_empty() {
        let entry = CachedFetch {
            secrets: secrets.clone(),
            fetched_at: now_secs(),
        };
        let full_key = cache_key_str_full(&cache_key);
        if let Ok(mut guard) = cache_map().lock() {
            guard.insert(full_key.clone(), entry.clone());
        }
        let disk_key = disk_key_str(&cache_key);
        disk_cache().write(&disk_key, &entry, cache_ttl_seconds, home_path);
    }

    Ok((secrets, warnings))
}

// ---------------------------------------------------------------------------
// Public entry point — mirrors lines 391-489
// ---------------------------------------------------------------------------

/// Resolve configured `op://` references and set them on `env::set_var` — mirrors `apply_onepassword_secrets` (lines 396-489).
pub fn apply_onepassword_secrets(
    enabled: bool,
    env_map: Option<&HashMap<String, String>>,
    account: &str,
    service_account_token_env: &str,
    binary_path: &str,
    override_existing: bool,
    cache_ttl_seconds: f64,
    home_path: Option<&Path>,
) -> FetchResult {
    let mut result = FetchResult::default();

    if !enabled {
        return result;
    }

    let (valid, warnings) = validate_references(env_map);
    result.warnings.extend(warnings);

    let mut refs_to_fetch: HashMap<String, String> = HashMap::new();
    for (name, reference) in &valid {
        if name == service_account_token_env {
            result.skipped.push(name.clone());
            continue;
        }
        if !override_existing {
            if let Ok(existing) = env::var(name) {
                if !existing.is_empty() {
                    result.skipped.push(name.clone());
                    continue;
                }
            }
        }
        refs_to_fetch.insert(name.clone(), reference.clone());
    }

    if refs_to_fetch.is_empty() {
        return result;
    }

    let binary = find_op(binary_path);
    result.binary_path = binary.clone();
    if binary.is_none() {
        if !binary_path.is_empty() {
            result.error = Some(format!(
                "secrets.onepassword.binary_path ({:?}) is not an executable op binary.",
                binary_path
            ));
        } else {
            result.error = Some(
                "secrets.onepassword.enabled is true but the op CLI was not found on PATH.  Install it (https://developer.1password.com/docs/cli/get-started/) or set secrets.onepassword.binary_path.".to_string(),
            );
        }
        return result;
    }

    let fetched = fetch_onepassword_secrets(
        &refs_to_fetch,
        account,
        service_account_token_env,
        binary.as_deref(),
        "",
        true,
        cache_ttl_seconds,
        home_path,
    );

    match fetched {
        Ok((secrets, fetch_warnings)) => {
            // Capture secrets before moving into result so we can iterate
            let secrets_clone = secrets.clone();
            result.secrets = secrets;
            result.warnings.extend(fetch_warnings);
            for (name, value) in secrets_clone {
                if name == service_account_token_env {
                    if !result.skipped.contains(&name) {
                        result.skipped.push(name.clone());
                    }
                    continue;
                }
                if !override_existing {
                    if let Ok(existing) = env::var(&name) {
                        if !existing.is_empty() {
                            if !result.skipped.contains(&name) {
                                result.skipped.push(name.clone());
                            }
                            continue;
                        }
                    }
                }
                env::set_var(&name, &value);
                result.applied.push(name);
            }
        }
        Err(exc) => {
            result.error = Some(exc);
        }
    }

    result
}

// ---------------------------------------------------------------------------
// SecretSource adapter — mirrors lines 492-637
// ---------------------------------------------------------------------------

/// Minimal `SecretSource` trait — mirrors `agent.secret_sources.base.SecretSource` ABC.
/// Canonical trait lives in `hermes-secret` crate; this local definition keeps
/// the slice self-contained.
pub trait SecretSource {
    fn name(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn shape(&self) -> &'static str;
    fn scheme(&self) -> Option<&'static str>;
    fn override_existing(&self, cfg: &HashMap<String, serde_value::Value>) -> bool;
    fn protected_env_vars(&self, cfg: &HashMap<String, serde_value::Value>) -> Vec<String>;
    fn config_schema(&self) -> HashMap<String, serde_value::ConfigEntry>;
    fn fetch(&self, cfg: &HashMap<String, serde_value::Value>, home_path: &Path) -> FetchResult;
    fn remediation(&self, kind: Option<&ErrorKind>, cfg: &HashMap<String, serde_value::Value>) -> String;
}

/// Lightweight `serde_json::Value`-like enum so the trait stays std-only.
pub mod serde_value {
    use std::collections::HashMap;

    #[derive(Debug, Clone)]
    pub enum Value {
        Null,
        Bool(bool),
        Number(f64),
        String(String),
        Map(HashMap<String, Value>),
        Array(Vec<Value>),
    }

    #[derive(Debug, Clone)]
    pub struct ConfigEntry {
        pub description: String,
        pub default: Value,
    }

    impl Value {
        pub fn as_str(&self) -> Option<&str> {
            match self { Value::String(s) => Some(s), _ => None }
        }
        pub fn as_f64(&self) -> Option<f64> {
            match self { Value::Number(n) => Some(*n), _ => None }
        }
        pub fn as_bool(&self) -> Option<bool> {
            match self { Value::Bool(b) => Some(*b), _ => None }
        }
        pub fn as_map(&self) -> Option<&HashMap<String, Value>> {
            match self { Value::Map(m) => Some(m), _ => None }
        }
    }
}

/// 1Password as a registered secret source — mirrors `OnePasswordSource` (lines 497-637).
pub struct OnePasswordSource;

impl OnePasswordSource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OnePasswordSource {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretSource for OnePasswordSource {
    fn name(&self) -> &'static str {
        "onepassword"
    }

    fn label(&self) -> &'static str {
        "1Password"
    }

    fn shape(&self) -> &'static str {
        "mapped"
    }

    fn scheme(&self) -> Option<&'static str> {
        Some("op")
    }

    fn override_existing(&self, cfg: &HashMap<String, serde_value::Value>) -> bool {
        match cfg.get("override_existing") {
            Some(serde_value::Value::Bool(b)) => *b,
            Some(_) => true,
            None => true,
        }
    }

    fn protected_env_vars(&self, cfg: &HashMap<String, serde_value::Value>) -> Vec<String> {
        let token_env = cfg
            .get("service_account_token_env")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_TOKEN_ENV);
        vec![token_env.to_string()]
    }

    fn config_schema(&self) -> HashMap<String, serde_value::ConfigEntry> {
        let mut m = HashMap::new();
        m.insert(
            "enabled".into(),
            serde_value::ConfigEntry {
                description: "Master switch".into(),
                default: serde_value::Value::Bool(false),
            },
        );
        m.insert(
            "env".into(),
            serde_value::ConfigEntry {
                description: "Map of ENV_VAR -> op://vault/item/field reference".into(),
                default: serde_value::Value::Map(HashMap::new()),
            },
        );
        m.insert(
            "account".into(),
            serde_value::ConfigEntry {
                description: "op --account shorthand (empty = default account)".into(),
                default: serde_value::Value::String(String::new()),
            },
        );
        m.insert(
            "service_account_token_env".into(),
            serde_value::ConfigEntry {
                description: "Env var holding the service-account token (unset = desktop/interactive session)".into(),
                default: serde_value::Value::String(DEFAULT_TOKEN_ENV.to_string()),
            },
        );
        m.insert(
            "binary_path".into(),
            serde_value::ConfigEntry {
                description: "Pin the op binary (empty = resolve via PATH)".into(),
                default: serde_value::Value::String(String::new()),
            },
        );
        m.insert(
            "cache_ttl_seconds".into(),
            serde_value::ConfigEntry {
                description: "Disk+memory cache TTL; 0 disables".into(),
                default: serde_value::Value::Number(300.0),
            },
        );
        m.insert(
            "override_existing".into(),
            serde_value::ConfigEntry {
                description: "Resolved values overwrite .env/shell values".into(),
                default: serde_value::Value::Bool(true),
            },
        );
        m
    }

    fn fetch(&self, cfg: &HashMap<String, serde_value::Value>, home_path: &Path) -> FetchResult {
        let mut result = FetchResult::default();

        let env_map = cfg.get("env").and_then(|v| v.as_map());
        let (valid, warnings) = validate_references_generic(env_map);
        result.warnings.extend(warnings);
        if valid.is_empty() {
            if result.warnings.is_empty() {
                result.error = Some(
                    "secrets.onepassword.enabled is true but the env: map is empty.  Add ENV_VAR: op://vault/item/field entries.".to_string(),
                );
                result.error_kind = Some(ErrorKind::NotConfigured);
            }
            return result;
        }

        let binary_path = cfg
            .get("binary_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let binary = find_op(&binary_path);
        result.binary_path = binary.clone();
        if binary.is_none() {
            if !binary_path.is_empty() {
                result.error = Some(format!(
                    "secrets.onepassword.binary_path ({:?}) is not an executable op binary.",
                    binary_path
                ));
            } else {
                result.error = Some(
                    "secrets.onepassword.enabled is true but the op CLI was not found on PATH.  Install it (https://developer.1password.com/docs/cli/get-started/) or set secrets.onepassword.binary_path.".to_string(),
                );
            }
            result.error_kind = Some(ErrorKind::BinaryMissing);
            return result;
        }

        let ttl: f64 = cfg
            .get("cache_ttl_seconds")
            .and_then(|v| {
                if let Some(n) = v.as_f64() {
                    Some(n)
                } else if let Some(s) = v.as_str() {
                    s.parse::<f64>().ok()
                } else {
                    None
                }
            })
            .unwrap_or(300.0);

        let account = cfg
            .get("account")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let token_env = cfg
            .get("service_account_token_env")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_TOKEN_ENV)
            .to_string();

        match fetch_onepassword_secrets(
            &valid,
            &account,
            &token_env,
            binary.as_deref(),
            "",
            true,
            ttl,
            Some(home_path),
        ) {
            Ok((secrets, fetch_warnings)) => {
                result.secrets = secrets;
                result.warnings.extend(fetch_warnings);
            }
            Err(exc) => {
                result.error = Some(exc.clone());
                result.error_kind = Some(classify_op_error(&exc));
            }
        }

        result
    }

    fn remediation(&self, kind: Option<&ErrorKind>, cfg: &HashMap<String, serde_value::Value>) -> String {
        match kind {
            Some(ErrorKind::AuthFailed) | Some(ErrorKind::AuthExpired) => {
                let token_env = cfg
                    .get("service_account_token_env")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or(DEFAULT_TOKEN_ENV);
                format!(
                    "Run `hermes secrets onepassword token` to paste a fresh service-account token ({}), or `op signin` for an interactive session.",
                    token_env
                )
            }
            Some(ErrorKind::BinaryMissing) => {
                "Install the 1Password CLI (https://developer.1password.com/docs/cli/get-started/) or set secrets.onepassword.binary_path.".to_string()
            }
            _ => {
                match kind {
                    Some(ErrorKind::NotConfigured) => format!("Run `hermes secrets {} setup` to finish configuration.", self.name()),
                    Some(ErrorKind::BinaryMissing) => format!("Run `hermes secrets {} setup` to install the helper CLI.", self.name()),
                    Some(ErrorKind::AuthFailed) => format!("Credentials rejected — run `hermes secrets {} setup` to re-authenticate.", self.name()),
                    Some(ErrorKind::AuthExpired) => format!("Credentials expired — run `hermes secrets {} setup` to re-authenticate.", self.name()),
                    Some(ErrorKind::Network) => "Network problem reaching the secrets backend — check connectivity and retry.".to_string(),
                    Some(ErrorKind::Timeout) => format!("Backend was slow — raise secrets.{}.timeout_seconds if this recurs.", self.name()),
                    _ => String::new(),
                }
            }
        }
    }
}

/// Best-effort mapping of op failure text onto the shared taxonomy — mirrors `_classify_op_error` (lines 640-657).
pub fn classify_op_error(message: &str) -> ErrorKind {
    let lowered = message.to_lowercase();
    if lowered.contains("timed out") {
        return ErrorKind::Timeout;
    }
    if lowered.contains("not found on path")
        || lowered.contains("not an executable")
        || lowered.contains("failed to invoke")
    {
        return ErrorKind::BinaryMissing;
    }
    if ["unauthorized", "not signed in", "session expired", "authentication", "401", "403"]
        .iter()
        .any(|tok| lowered.contains(tok))
    {
        return ErrorKind::AuthFailed;
    }
    if lowered.contains("empty value") {
        return ErrorKind::EmptyValue;
    }
    if ["network", "connection", "resolve host", "dns"]
        .iter()
        .any(|tok| lowered.contains(tok))
    {
        return ErrorKind::Network;
    }
    ErrorKind::Internal
}

// ---------------------------------------------------------------------------
// Test hook — mirrors lines 660-682
// ---------------------------------------------------------------------------

/// Drop in-process AND disk caches — mirrors `clear_caches` (lines 665-673).
pub fn clear_caches(home_path: Option<&Path>) {
    if let Ok(mut m) = cache_map().lock() {
        m.clear();
    }
    disk_cache().clear(home_path);
}

/// Clear in-process AND disk caches (test-scoped) — mirrors `_reset_cache_for_tests` (lines 676-682).
pub fn reset_cache_for_tests(home_path: Option<&Path>) {
    clear_caches(home_path)
}

// ---------------------------------------------------------------------------
// Self-check (ponytail: one runnable check, no framework)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_references_filters_invalid() {
        let mut m = HashMap::new();
        m.insert("GOOD".to_string(), "op://vault/item/field".to_string());
        m.insert("bad-name".to_string(), "op://vault/item/field".to_string());
        m.insert("BADREF".to_string(), "not-op://".to_string());
        let (valid, warnings) = validate_references(Some(&m));
        assert_eq!(valid.len(), 1);
        assert!(valid.contains_key("GOOD"));
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn disk_key_omits_home() {
        let k = (
            "authfp".to_string(),
            "acc".to_string(),
            "/home/test/.hermes".to_string(),
            "refsfp".to_string(),
        );
        assert_eq!(disk_key_str(&k), "authfp|acc|refsfp");
        assert_eq!(cache_key_str_full(&k), "authfp|acc|/home/test/.hermes|refsfp");
    }

    #[test]
    fn valid_env_names() {
        assert!(is_valid_env_name("OPENAI_API_KEY"));
        assert!(is_valid_env_name("_foo"));
        assert!(!is_valid_env_name("1bad"));
        assert!(!is_valid_env_name("has-dash"));
        assert!(!is_valid_env_name(""));
    }

    #[test]
    fn classify_errors() {
        assert_eq!(classify_op_error("op read timed out after 30s"), ErrorKind::Timeout);
        assert_eq!(classify_op_error("op CLI not found on PATH"), ErrorKind::BinaryMissing);
        assert_eq!(classify_op_error("not signed in"), ErrorKind::AuthFailed);
        assert_eq!(classify_op_error("op read returned an empty value"), ErrorKind::EmptyValue);
        assert_eq!(classify_op_error("network unreachable"), ErrorKind::Network);
        assert_eq!(classify_op_error("something else"), ErrorKind::Internal);
    }

    #[test]
    fn scrub_strips_ansi() {
        let raw = "\x1b[31mred\x1b[0m plain";
        assert_eq!(scrub(raw), "red plain");
        let lone = "hello\x1bworld";
        assert_eq!(scrub(lone), "helloworld");
    }

    #[test]
    fn constants_match_python() {
        assert_eq!(OP_RUN_TIMEOUT_SECS, 30);
        assert_eq!(DEFAULT_TOKEN_ENV, "OP_SERVICE_ACCOUNT_TOKEN");
        assert_eq!(DISK_CACHE_BASENAME, "op_cache.json");
    }
}
