//! Shared substrate for external secret-source backends.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/secret_sources/_cache.py` (215 lines).
//!
//! Every backend (Bitwarden, 1Password, …) needs the same handful of
//! security-sensitive primitives:
//! * a uniform result object (`FetchResult`),
//! * environment-variable name validation (`is_valid_env_name`),
//! * a two-layer fetch cache whose disk half writes atomically with `0600`
//!   permissions and honours a TTL (`DiskCache`, `CachedFetch`).
//!
//! These used to live inline inside `bitwarden.py`. Pulling them here means
//! the atomic-write / `0600` / TTL logic is audited and fixed in exactly one
//! place instead of drifting across copy-pasted per-backend modules — each
//! backend supplies only its own cache-key shape and a serializer for it.
//!
//! Nothing in this module ever raises out to the caller's hot path: the disk
//! layer is strictly best-effort (a miss just triggers a refetch), because a
//! cache problem must never block Hermes startup.
//!
//! T0046 — 1:1 port, no cargo (NEVER cargo).
//!
//! Notes on fidelity vs. Rust idioms:
//! - Python `FetchResult` / `is_valid_env_name` live canonically in `base.py` and are
//!   re-exported from `_cache.py` for compat; Rust mirrors via `pub use crate::secret_base::{...}`.
//! - Python `@dataclass class CachedFetch` ↔ `struct CachedFetch` with `is_fresh(ttl_seconds)`.
//! - Python `resolve_cache_home(home_path: Optional[Path] = None) -> Path` ↔
//!   `fn resolve_cache_home(home_path: Option<&Path>) -> PathBuf` via `HERMES_HOME`/HOME fallback.
//! - Python `DiskCache(Generic[K])` with `key_serializer: Callable[[K], str]` ↔
//!   `struct DiskCache<K>` holding `Arc<dyn Fn(&K) -> String + Send + Sync>`.
//!   `stem = basename.split(".", 1)[0]` → `tmp_prefix = format!(".{}_", stem)` preserved.
//! - Python `json.load`/`json.dump` ↔ std-only hand-rolled JSON helpers (no `serde` dep),
//!   mirroring `bitwarden.rs` / `onepassword.rs` helpers. `0600`/`0700` chmod preserved via
//!   `PermissionsExt` on unix; best-effort with `try`→`None`/`return` on any `OSError`.
//! - Python `tempfile.mkstemp → chmod 0600 → os.replace` ↔ `fs::write(tmp) → chmod 0600 → rename`.
//! - `crate stays std-only` — no `serde`, `regex`, `log` deps.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::HashMap;
use std::env;
use std::fs;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Re-exports — mirrors `from agent.secret_sources.base import FetchResult, is_valid_env_name`
// (lines 46-49). Canonical definitions live in `secret_base.rs`; re-exported
// here so backends that import from `_cache` keep working.
// ---------------------------------------------------------------------------

pub use crate::secret_base::is_valid_env_name;
pub use crate::secret_base::FetchResult;

// ---------------------------------------------------------------------------
// CachedFetch — mirrors `_cache.CachedFetch` (lines 58-67)
// ---------------------------------------------------------------------------

/// A set of fetched secret values plus when they were fetched.
/// Mirrors `CachedFetch` (lines 58-67).
#[derive(Debug, Clone)]
pub struct CachedFetch {
    pub secrets: HashMap<String, String>,
    pub fetched_at: f64,
}

impl CachedFetch {
    /// Mirrors `CachedFetch.is_fresh(self, ttl_seconds: float) -> bool` (lines 64-67).
    pub fn is_fresh(&self, ttl_seconds: f64) -> bool {
        if ttl_seconds <= 0.0 {
            return false;
        }
        let now = now_secs();
        (now - self.fetched_at) < ttl_seconds
    }
}

// ---------------------------------------------------------------------------
// resolve_cache_home — mirrors `_cache.resolve_cache_home` (lines 77-88)
// ---------------------------------------------------------------------------

/// Resolve the Hermes home used for cache paths.
///
/// `home_path` is whatever `load_hermes_dotenv()` already resolved;
/// falling back to `$HERMES_HOME` / `~/.hermes` keeps direct callers
/// (and tests that don't thread a home through) working.
/// Mirrors `resolve_cache_home` (lines 77-88).
pub fn resolve_cache_home(home_path: Option<&Path>) -> PathBuf {
    if let Some(p) = home_path {
        return p.to_path_buf();
    }
    get_hermes_home()
}

fn get_hermes_home() -> PathBuf {
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

// ---------------------------------------------------------------------------
// DiskCache — mirrors `_cache.DiskCache` (lines 94-215)
// ---------------------------------------------------------------------------

/// Best-effort, profile-aware on-disk cache for fetched secret values.
///
/// One JSON object per backend lives at `<hermes_home>/cache/<basename>`:
/// `{"key": "<serialized cache key>", "secrets": {...}, "fetched_at": 1.0}`
///
/// The file holds only secret *values* keyed by the serialized cache key —
/// never raw auth material. Backends are responsible for fingerprinting
/// tokens/sessions *before* they reach `key_serializer` so the token can't
/// land in the key.
///
/// Writes are atomic (`mkstemp` → `chmod 0600` → `os.replace`) and the
/// containing `cache/` directory is forced to `0700` — `mkdir`'s mode is
/// umask-subject, so the chmod is the reliable form. Both `read` and
/// `write` short-circuit when `ttl_seconds <= 0`, so setting the TTL to
/// zero disables *both* cache layers symmetrically: a user opting out never
/// gets secret values written to disk at all.
/// Mirrors `DiskCache` (lines 94-215).
pub struct DiskCache<K> {
    basename: String,
    key_serializer: Arc<dyn Fn(&K) -> String + Send + Sync>,
    tmp_prefix: String,
    _phantom: PhantomData<K>,
}

impl<K> Clone for DiskCache<K> {
    fn clone(&self) -> Self {
        Self {
            basename: self.basename.clone(),
            key_serializer: Arc::clone(&self.key_serializer),
            tmp_prefix: self.tmp_prefix.clone(),
            _phantom: PhantomData,
        }
    }
}

impl<K> DiskCache<K> {
    /// Mirrors `DiskCache.__init__(self, basename: str, *, key_serializer: Callable[[K], str])`
    /// (lines 114-120). `stem = basename.split(".", 1)[0]` → `tmp_prefix = f".{stem}_"`.
    pub fn new<F>(basename: &str, key_serializer: F) -> Self
    where
        F: Fn(&K) -> String + Send + Sync + 'static,
    {
        let stem = basename.split('.').next().unwrap_or(basename);
        Self {
            basename: basename.to_string(),
            key_serializer: Arc::new(key_serializer),
            tmp_prefix: format!(".{}_", stem),
            _phantom: PhantomData,
        }
    }

    /// Mirrors `DiskCache.path(self, home_path: Optional[Path] = None) -> Path` (lines 122-123).
    pub fn path(&self, home_path: Option<&Path>) -> PathBuf {
        resolve_cache_home(home_path).join("cache").join(&self.basename)
    }

    /// Return a fresh cached entry for `key`, or None.
    ///
    /// Best-effort: any I/O or parse error, a key mismatch, or a stale entry
    /// all return None so the caller re-fetches.
    /// Mirrors `DiskCache.read` (lines 125-160).
    pub fn read(&self, key: &K, ttl_seconds: f64, home_path: Option<&Path>) -> Option<CachedFetch> {
        if ttl_seconds <= 0.0 {
            return None;
        }
        let path = self.path(home_path);
        let text = fs::read_to_string(&path).ok()?;
        let payload = parse_disk_cache_payload(&text)?;
        let serialized = (self.key_serializer)(key);
        if payload.key != serialized {
            return None;
        }
        // JSON permits non-string values; env vars need strings, so the Python
        // drops anything that isn't a str→str pair. Our `parse_disk_cache_payload`
        // already only captures string→string pairs via `extract_json_string_map_field`,
        // so filtering is implicit.
        let entry = CachedFetch {
            secrets: payload.secrets,
            fetched_at: payload.fetched_at,
        };
        if !entry.is_fresh(ttl_seconds) {
            return None;
        }
        Some(entry)
    }

    /// Persist `entry` for `key` atomically at mode `0600`.
    ///
    /// No-op when `ttl_seconds <= 0` (so caching is genuinely off) or on any
    /// I/O error — the next invocation just re-fetches.
    /// Mirrors `DiskCache.write` (lines 162-208).
    pub fn write(&self, key: &K, entry: &CachedFetch, ttl_seconds: f64, home_path: Option<&Path>) {
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
            json_escape_str(&(self.key_serializer)(key)),
            json_string_map(&entry.secrets),
            entry.fetched_at
        );
        // Write to a sibling temp file and atomic-rename. `tempfile` honours
        // `os.umask`, so we explicitly chmod 0600 before the rename.
        // Mirrors `fd, tmp = tempfile.mkstemp(prefix=self._tmp_prefix, suffix=".tmp", dir=str(cache_dir))`
        let tmp = cache_dir.join(format!(
            "{}{}-{:x}.tmp",
            self.tmp_prefix,
            std::process::id(),
            now_secs().to_bits()
        ));
        let write_res = fs::write(&tmp, payload.as_bytes());
        if write_res.is_err() {
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
        }
        // Mirrors `os.replace(tmp, path)` with BaseException cleanup: unlink tmp on failure.
        if fs::rename(&tmp, &path).is_err() {
            let _ = fs::remove_file(&tmp);
            return;
        }
        // `os.replace` leaves `tmp` gone on success; our `rename` already does, but
        // the `remove_file` fallback is harmless and mirrors the Python `except BaseException: unlink(tmp)`.
        let _ = fs::remove_file(&tmp);
    }

    /// Delete the on-disk cache file if present (idempotent).
    /// Mirrors `DiskCache.clear` (lines 210-215).
    pub fn clear(&self, home_path: Option<&Path>) {
        let path = self.path(home_path);
        let _ = fs::remove_file(&path);
    }
}

// Convenience alias for string-keyed caches where serializer is identity.
// Mirrors the common case `DiskCache(basename, key_serializer=lambda k: k)` or `str(key)`.
impl DiskCache<String> {
    /// Create a `DiskCache<String>` with identity serializer (`|k| k.clone()`).
    /// Convenience for backends whose serialized key is already a `String`.
    pub fn new_string(basename: &str) -> Self {
        Self::new(basename, |k: &String| k.clone())
    }
}

// Convenience for `DiskCache<&str>`-like usage via `String` storage.
impl DiskCache<str> {
    /// Create a `DiskCache<str>` (unsized) is not directly constructible; use `DiskCache<String>::new_string`.
    #[allow(dead_code)]
    fn _phantom_str() {}
}

struct DiskPayload {
    key: String,
    secrets: HashMap<String, String>,
    fetched_at: f64,
}

fn parse_disk_cache_payload(text: &str) -> Option<DiskPayload> {
    // Minimal JSON parsing for {"key": "...", "secrets": {...}, "fetched_at": 123.0}
    // Mirrors Python: `payload = json.load(f)` then isinstance checks + key mismatch + type checks.
    let key = extract_json_string_field(text, "key")?;
    let secrets = extract_json_string_map_field(text, "secrets")?;
    let fetched_at = extract_json_number_field(text, "fetched_at")?;
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
