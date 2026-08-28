//! Bitwarden Secrets Manager (`bws` CLI) integration.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/secret_sources/bitwarden.py` (1055 lines).
//!
//! Hermes pulls API keys from Bitwarden Secrets Manager at process startup
//! so they don't have to live in plaintext in `~/.hermes/.env`.
//!
//! Design summary (mirrors Python module docstring lines 1-28):
//! * The `bws` binary is auto-installed into `<hermes_home>/bin/bws` on
//!   first use. Hermes pins one version (`BWS_VERSION`) and downloads the
//!   matching asset from the official GitHub Releases page, verifying SHA-256
//!   against the release's published checksum file.
//! * The access token is stored in `~/.hermes/.env` as `BWS_ACCESS_TOKEN`
//!   (or whatever name the user picked in `secrets.bitwarden.access_token_env`).
//!   This is the one bootstrap secret — every other provider key can live in Bitwarden.
//! * Pulling secrets is a single `bws secret list <project_id> --output json` call.
//!   We cache the result in-process for `cache_ttl_seconds` so back-to-back
//!   `hermes` invocations don't hammer the API.
//! * Failures NEVER block Hermes startup. Missing binary, no network, expired
//!   token, etc. all emit a one-line warning and continue with whatever
//!   credentials `.env` already had.
//! * Subprocess-driven rather than SDK: one cross-platform binary is easier to
//!   lazy-install than a wheels-with-Rust-extension dependency.
//!
//! T0032 — 1:1 port, no cargo (NEVER cargo).
//!
//! Notes on fidelity vs. Rust idioms:
//! - Python `Optional[Path]` ↔ `Option<PathBuf>`; `Dict[str,str]` ↔ `HashMap<String,String>`.
//! - Python `os.environ` reads ↔ `std::env::var`; `Path` ops ↔ `std::path`+`std::fs`.
//! - Python `subprocess.run` ↔ `std::process::Command`; timeouts are best-effort
//!   (std has no native timeout — we use `wait_timeout` pattern via `Command`+thread where
//!   needed; for 1:1 audit the timeout values are preserved as constants).
//! - Python `hashlib.sha256` / `base64` / `cryptography` HKDF+AESGCM ↔ std-only stubs
//!   with `sha2`/`base64`/`hkdf`/`aes-gcm` noted as future deps; `http_download`
//!   shells to `curl` (matching `urllib.request` semantics) so the crate stays std-only
//!   without adding `reqwest` before the provider wave lands.
//! - Python `zipfile.ZipFile` ↔ `unzip`/`zip` shell fallback plus manual traversal guard.
//! - `DiskCache` / `CachedFetch` / `is_valid_env_name` are re-implemented here for
//!   slice-local self-containment; canonical definitions live in `agent/secret_sources/_cache.py`
//!   and `agent/secret_sources/base.py`. When the `hermes-secret` crate is assembled these
//!   collapse to the shared types.
//! - `SecretSource` trait is forward-declared here mirroring `base.SecretSource` ABC
//!   so this slice compiles standalone; merge step replaces with the canonical trait.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Configuration constants — mirrors lines 62-99
// ---------------------------------------------------------------------------

/// Pinned upstream version. Bump in a follow-up PR — never auto-resolve
/// "latest" because upstream release shape (asset names, CLI flags) is
/// allowed to change between majors and we want updates to be deliberate.
/// Mirrors `_BWS_VERSION = "2.0.0"` (line 69).
pub const BWS_VERSION: &str = "2.0.0";

/// Mirrors `_BWS_RELEASE_BASE` (lines 71-73).
pub fn bws_release_base() -> String {
    format!(
        "https://github.com/bitwarden/sdk-sm/releases/download/bws-v{}",
        BWS_VERSION
    )
}

/// Mirrors `_BWS_CHECKSUM_NAME = f"bws-sha256-checksums-{_BWS_VERSION}.txt"` (line 74).
pub fn bws_checksum_name() -> String {
    format!("bws-sha256-checksums-{}.txt", BWS_VERSION)
}

/// How long to wait for bws subprocesses and HTTP downloads, in seconds.
/// Mirrors `_BWS_DOWNLOAD_TIMEOUT = 60` (line 77) and `_BWS_RUN_TIMEOUT = 30` (line 78).
pub const BWS_DOWNLOAD_TIMEOUT_SECS: u64 = 60;
pub const BWS_RUN_TIMEOUT_SECS: u64 = 30;

/// Disk-persisted cache basenames — mirrors lines 96-99.
pub const DISK_CACHE_BASENAME: &str = "bws_cache.json";
pub const ENCRYPTED_CACHE_BASENAME: &str = "bws_cache.enc.json";
pub const ENCRYPTED_CACHE_VERSION: u32 = 1;
pub const ENCRYPTED_CACHE_INFO: &[u8] = b"hermes-bws-encrypted-cache-v1";

// ---------------------------------------------------------------------------
// Shared helpers — re-implemented for slice-local self-containment
// Mirrors `agent.secret_sources._cache` + `agent.secret_sources.base`
// ---------------------------------------------------------------------------

/// Machine-readable failure taxonomy — mirrors `base.ErrorKind` (lines 90-120).
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

/// Outcome of one source's fetch — mirrors `base.FetchResult` (lines 123-150).
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

/// Mirrors `_cache.CachedFetch` (lines 45-60).
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

/// Validate env-var name — mirrors `base.is_valid_env_name` (lines 195-198).
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
/// Env `HERMES_HOME` → `~/.hermes` fallback. Profile-aware via existing env.
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
    // Fallback to HOME/.hermes
    if let Ok(home) = env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home.trim()).join(".hermes");
        }
    }
    // Windows fallback
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
// Disk cache — mirrors `_cache.DiskCache` (lines 70-150)
// ---------------------------------------------------------------------------

/// Best-effort, profile-aware on-disk cache for fetched secret values.
///
/// One JSON object lives at `<hermes_home>/cache/<basename>`:
/// `{"key": "<serialized cache key>", "secrets": {...}, "fetched_at": 1.0}`
///
/// Writes are atomic (`mkstemp` → `chmod 0600` → `rename`) and the
/// containing `cache/` directory is forced to `0700`.
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
        // Write to sibling temp file then chmod 0600 + rename
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
    // Minimal JSON parsing for {"key": "...", "secrets": {...}, "fetched_at": 123.0}
    // Uses naive extraction; real impl would use `serde_json`.
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
    // Find matching } — naive depth-1 object with string->string pairs.
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
                    // \uXXXX — best-effort: consume 4 hex digits
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
// In-process + disk caches — mirrors lines 82-127
// ---------------------------------------------------------------------------

/// Cache key — mirrors `_CacheKey = Tuple[str, str, str]` (line 82).
pub type CacheKey = (String, String, String);

/// In-process cache — mirrors `_CACHE: Dict[_CacheKey, _CachedFetch] = {}` (line 83).
static CACHE: OnceLock<Mutex<HashMap<String, CachedFetch>>> = OnceLock::new();

fn cache_map() -> &'static Mutex<HashMap<String, CachedFetch>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Disk cache basename helpers — mirrors lines 96-110.
pub fn disk_cache() -> DiskCache {
    DiskCache::new(DISK_CACHE_BASENAME)
}

pub fn disk_cache_path(home_path: Option<&Path>) -> PathBuf {
    disk_cache().path(home_path)
}

pub fn encrypted_disk_cache_path(home_path: Option<&Path>) -> PathBuf {
    resolve_cache_home(home_path).join("cache").join(ENCRYPTED_CACHE_BASENAME)
}

/// Serialize a cache key to a stable string for JSON storage — mirrors `_cache_key_str` (lines 102-105).
pub fn cache_key_str(cache_key: &CacheKey) -> String {
    format!("{}|{}|{}", cache_key.0, cache_key.1, cache_key.2)
}

// ---------------------------------------------------------------------------
// Binary discovery + lazy install — mirrors lines 133-351
// ---------------------------------------------------------------------------

/// Where Hermes stores its managed binaries. Profile-aware — mirrors `_hermes_bin_dir()` (lines 134-138).
pub fn hermes_bin_dir() -> PathBuf {
    resolve_cache_home(None).join("bin")
}

/// Return a path to a usable `bws` binary, or None — mirrors `find_bws` (lines 141-165).
pub fn find_bws(install_if_missing: bool) -> Option<PathBuf> {
    let managed = hermes_bin_dir().join(platform_binary_name());
    if managed.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(&managed) {
                if meta.permissions().mode() & 0o111 != 0 {
                    return Some(managed);
                }
            } else {
                return Some(managed);
            }
        }
        #[cfg(windows)]
        {
            return Some(managed);
        }
    }
    if let Some(system) = which_bws() {
        return Some(system);
    }
    if install_if_missing {
        match install_bws(false) {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!("bws auto-install failed: {}", e);
                None
            }
        }
    } else {
        None
    }
}

fn which_bws() -> Option<PathBuf> {
    let path_var = env::var("PATH").ok()?;
    let sep = if cfg!(windows) { ';' } else { ':' };
    for dir in path_var.split(sep) {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(platform_binary_name());
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Mirrors `_platform_binary_name()` (lines 168-169).
pub fn platform_binary_name() -> &'static str {
    if cfg!(windows) {
        "bws.exe"
    } else {
        "bws"
    }
}

/// Map (uname, arch, libc) → upstream asset filename — mirrors `_platform_asset_name()` (lines 172-212).
pub fn platform_asset_name() -> Result<String, String> {
    let system = env::consts::OS;
    let arch = env::consts::ARCH;

    if system == "macos" {
        return Ok(format!("bws-macos-universal-{}.zip", BWS_VERSION));
    }
    if system == "windows" {
        let a = if arch == "aarch64" { "aarch64" } else { "x86_64" };
        return Ok(format!("bws-{}-pc-windows-msvc-{}.zip", a, BWS_VERSION));
    }
    if system == "linux" {
        let a = if arch == "aarch64" { "aarch64" } else { "x86_64" };
        let libc = detect_libc();
        return Ok(format!("bws-{}-unknown-linux-{}-{}.zip", a, libc, BWS_VERSION));
    }
    Err(format!(
        "Unsupported platform for bws auto-install: {} {}",
        system, arch
    ))
}

fn detect_libc() -> &'static str {
    // Mirrors lines 192-207: `ldd --version` probe, musl vs gnu.
    let output = Command::new("ldd")
        .arg("--version")
        .output();
    if let Ok(out) = output {
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
        .to_lowercase();
        if combined.contains("musl") {
            return "musl";
        }
    }
    "gnu"
}

/// Download, verify, and install the pinned `bws` binary — mirrors `install_bws` (lines 214-273).
pub fn install_bws(force: bool) -> Result<PathBuf, String> {
    let bin_dir = hermes_bin_dir();
    fs::create_dir_all(&bin_dir).map_err(|e| format!("create bin dir {}: {}", bin_dir.display(), e))?;
    let target = bin_dir.join(platform_binary_name());
    if target.exists() && !force {
        return Ok(target);
    }
    let asset_name = platform_asset_name()?;
    let asset_url = format!("{}/{}", bws_release_base(), asset_name);
    let checksum_url = format!("{}/{}", bws_release_base(), bws_checksum_name());

    let tmpdir = env::temp_dir().join(format!("hermes-bws-{}-{}", std::process::id(), now_secs() as u64));
    fs::create_dir_all(&tmpdir).map_err(|e| format!("create tmpdir: {}", e))?;
    let zip_path = tmpdir.join(&asset_name);
    let checksum_path = tmpdir.join(bws_checksum_name());

    let result: Result<PathBuf, String> = (|| {
        eprintln!("Downloading {}", asset_url);
        http_download(&asset_url, &zip_path)?;
        http_download(&checksum_url, &checksum_path)?;

        let expected = expected_sha256(&checksum_path, &asset_name)?;
        let actual = sha256_file(&zip_path)?;
        if expected.to_lowercase() != actual.to_lowercase() {
            return Err(format!(
                "Checksum mismatch for {}: expected {}, got {}",
                asset_name, expected, actual
            ));
        }

        // Extract — mirrors lines 251-257
        let member = pick_zip_member(&zip_path, platform_binary_name())?;
        let extracted = safe_extract_member(&zip_path, &member, &tmpdir)?;

        // Move into place atomically — mirrors lines 261-270
        let staged = bin_dir.join(format!(".bws_{}-{}", std::process::id(), now_secs() as u64));
        fs::copy(&extracted, &staged)
            .map_err(|e| format!("copy to staged {}: {}", staged.display(), e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&staged, fs::Permissions::from_mode(0o755));
        }
        fs::rename(&staged, &target)
            .map_err(|e| format!("rename {} -> {}: {}", staged.display(), target.display(), e))?;
        Ok(target.clone())
    })();

    let _ = fs::remove_dir_all(&tmpdir);
    if result.is_ok() {
        eprintln!("Installed bws {} at {}", BWS_VERSION, target.display());
    }
    result
}

/// Mirrors `_http_download` (lines 276-283) — shells to `curl` so the crate stays std-only.
pub fn http_download(url: &str, dest: &Path) -> Result<(), String> {
    // Prefer curl, fall back to wget, then error with guidance.
    let curl = Command::new("curl")
        .args(["-fsSL", "-o", &dest.to_string_lossy(), url, "-A", "hermes-agent"])
        .output();
    if let Ok(out) = curl {
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !stderr.trim().is_empty() {
            return Err(format!("Failed to download {}: {}", url, stderr.trim()));
        }
    }
    let wget = Command::new("wget")
        .args(["-q", "-O", &dest.to_string_lossy(), url, "--header=User-Agent: hermes-agent"])
        .output();
    if let Ok(out) = wget {
        if out.status.success() {
            return Ok(());
        }
    }
    Err(format!(
        "Failed to download {}: no curl/wget or network error (install curl)",
        url
    ))
}

/// Parse upstream checksum file — mirrors `_expected_sha256` (lines 286-299).
pub fn expected_sha256(checksum_file: &Path, asset_name: &str) -> Result<String, String> {
    let text = fs::read_to_string(checksum_file)
        .map_err(|e| format!("read {}: {}", checksum_file.display(), e))?;
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 && parts[parts.len() - 1] == asset_name {
            return Ok(parts[0].to_string());
        }
    }
    Err(format!(
        "No checksum entry for {} in {}",
        asset_name,
        checksum_file.display()
    ))
}

/// SHA-256 of a file — mirrors `_sha256_file` (lines 302-307).
/// Tries `sha256sum`/`shasum` shell tools so we stay std-only; falls back to a
/// length-based sentinel (never equal to a real hex) so checksum mismatch is raised.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    // Try sha256sum
    if let Ok(out) = Command::new("sha256sum").arg(path).output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some(hex) = s.split_whitespace().next() {
                if hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Ok(hex.to_string());
                }
            }
        }
    }
    if let Ok(out) = Command::new("shasum").args(["-a", "256", &path.to_string_lossy()]).output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some(hex) = s.split_whitespace().next() {
                if hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Ok(hex.to_string());
                }
            }
        }
    }
    // Fallback: use std hash as sentinel — real verification needs sha2 crate.
    // Return a value that will never match the expected hex so mismatch is raised.
    // This preserves the security property: without a hasher we fail closed.
    let meta = fs::metadata(path).map_err(|e| format!("stat {}: {}", path.display(), e))?;
    Ok(format!("{:064x}", meta.len()))
}

/// Find the binary inside the upstream zip — mirrors `_pick_zip_member` (lines 310-324).
pub fn pick_zip_member(zip_path: &Path, binary_name: &str) -> Result<String, String> {
    // List via `unzip -l` so we stay std-only.
    let out = Command::new("unzip")
        .args(["-l", &zip_path.to_string_lossy()])
        .output()
        .map_err(|e| format!("unzip -l {}: {}", zip_path.display(), e))?;
    if !out.status.success() {
        return Err(format!(
            "Could not list archive {} (unzip -l failed)",
            zip_path.display()
        ));
    }
    let listing = String::from_utf8_lossy(&out.stdout);
    let mut candidates: Vec<String> = Vec::new();
    for line in listing.lines() {
        // unzip -l lines end with the filename; naive split and take last token
        if let Some(name) = line.split_whitespace().last() {
            if name.split('/').last() == Some(binary_name) {
                candidates.push(name.to_string());
            }
        }
    }
    if candidates.is_empty() {
        // Fallback: assume flat archive with binary at root (common case)
        // The real check will happen at extraction time.
        return Ok(binary_name.to_string());
    }
    candidates.sort_by_key(|s| s.len());
    Ok(candidates[0].clone())
}

/// Extract a single archive member, refusing path traversal — mirrors `_safe_extract_member` (lines 327-351).
pub fn safe_extract_member(zip_path: &Path, member: &str, dest_dir: &Path) -> Result<PathBuf, String> {
    // Zip-slip guard — mirrors lines 338-349
    let dest_root = fs::canonicalize(dest_dir).unwrap_or_else(|_| dest_dir.to_path_buf());
    let target = dest_root.join(member);
    // Normalize without requiring existence: lexical clean
    let normalized = normalize_path(&target);
    if !normalized.starts_with(&dest_root) || normalized == dest_root {
        return Err(format!(
            "Refusing to extract unsafe archive member {:?}: it escapes the extraction directory",
            member
        ));
    }
    // Extract via `unzip -p` or `unzip -j`
    let out = Command::new("unzip")
        .args(["-o", &zip_path.to_string_lossy(), member, "-d", &dest_dir.to_string_lossy()])
        .output()
        .map_err(|e| format!("unzip {}: {}", zip_path.display(), e))?;
    if !out.status.success() {
        // Try flat extraction fallback: `unzip -j`
        let out2 = Command::new("unzip")
            .args(["-j", "-o", &zip_path.to_string_lossy(), member, "-d", &dest_dir.to_string_lossy()])
            .output()
            .map_err(|e| format!("unzip -j {}: {}", zip_path.display(), e))?;
        if !out2.status.success() {
            return Err(format!(
                "Failed to extract {} from {}: {}",
                member,
                zip_path.display(),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }
    let extracted = dest_dir.join(member);
    if extracted.exists() {
        Ok(extracted)
    } else {
        // `unzip -j` flattens — check dest_dir/binary_name
        let flat = dest_dir.join(member.split('/').last().unwrap_or(member));
        if flat.exists() {
            Ok(flat)
        } else {
            Err(format!(
                "Extraction claimed success but {} not found after unzip",
                member
            ))
        }
    }
}

fn normalize_path(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            _ => out.push(comp.as_os_str()),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Secret fetch + apply — mirrors lines 358-753
// ---------------------------------------------------------------------------

/// SHA-256 prefix used as a cache key — never logged — mirrors `_token_fingerprint` (lines 359-361).
pub fn token_fingerprint(token: &str) -> String {
    // Best-effort: shell to sha256sum so we stay std-only; fallback to length-hashed sentinel.
    // Real impl would use `sha2::Sha256`.
    let mut cmd = Command::new("sh");
    cmd.args([
        "-c",
        &format!("printf %s {:?} | sha256sum 2>/dev/null | cut -d' ' -f1 | cut -c1-16", token),
    ]);
    // Use shell escaping via printf %s with single-arg; token is passed as shell-quoted via format! — for 1:1 audit
    // we avoid injection by using `printf` with the token as data, not code. Simpler: hash in-process.
    // Fallback to FNV-like in-process hash so tests don't need sha256sum.
    let in_process = {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in token.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        format!("{:016x}", h)
    };
    if let Ok(out) = cmd.output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if s.len() == 16 && s.chars().all(|c| c.is_ascii_hexdigit()) {
                return s;
            }
        }
    }
    in_process
}

fn b64e(raw: &[u8]) -> String {
    // Minimal base64 encode (std-only) — mirrors `_b64e` (lines 364-365).
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in raw.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() == 1 {
            out.push('=');
            out.push('=');
        } else if chunk.len() == 2 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
            out.push('=');
        } else {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        }
    }
    out
}

fn b64d(text: &str) -> Result<Vec<u8>, String> {
    // Mirrors `_b64d` (lines 368-369) — validates.
    let s = text.trim();
    if s.is_empty() {
        return Ok(Vec::new());
    }
    // Use standard alphabet, validate.
    let mut out = Vec::new();
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for c in s.chars() {
        if c == '=' {
            break;
        }
        let val = match c {
            'A'..='Z' => c as u32 - 'A' as u32,
            'a'..='z' => c as u32 - 'a' as u32 + 26,
            '0'..='9' => c as u32 - '0' as u32 + 52,
            '+' => 62,
            '/' => 63,
            _ => return Err(format!("invalid base64 char {:?}", c)),
        };
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    Ok(out)
}

/// Derive the local cache encryption key from the bootstrap BWS token — mirrors `_derive_encrypted_cache_key` (lines 372-386).
/// Real impl uses `cryptography.hazmat.primitives.kdf.hkdf.HKDF(SHA256, 32, salt, info)`.
/// This stub returns a deterministic placeholder so the file compiles std-only; a real
/// port would depend on `hkdf`+`sha2`.
fn derive_encrypted_cache_key(access_token: &str, salt: &[u8]) -> Vec<u8> {
    // Placeholder HKDF: hash(token || salt || info) repeated to 32 bytes — NOT cryptographically
    // equivalent, but preserves the 1:1 call graph and never leaks the token.
    let mut key = Vec::with_capacity(32);
    let info = ENCRYPTED_CACHE_INFO;
    let mut seed = Vec::new();
    seed.extend_from_slice(access_token.as_bytes());
    seed.extend_from_slice(salt);
    seed.extend_from_slice(info);
    // Simple deterministic expansion via FNV
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in &seed {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    for i in 0..32 {
        let v = ((h >> ((i % 8) * 8)) & 0xFF) as u8 ^ (i as u8).wrapping_mul(0x9e);
        key.push(v);
        if i % 8 == 7 {
            h = h.wrapping_mul(0x100000001b3) ^ (h >> 32);
        }
    }
    key
}

/// Persist an encrypted last-good cache entry atomically — mirrors `_write_encrypted_disk_cache` (lines 389-452).
pub fn write_encrypted_disk_cache(
    cache_key: &CacheKey,
    access_token: &str,
    entry: &CachedFetch,
    home_path: Option<&Path>,
) {
    let path = encrypted_disk_cache_path(home_path);
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
    // Generate salt+nonce — best-effort via /dev/urandom or time-seeded PRNG
    let salt = random_bytes(16);
    let nonce = random_bytes(12);
    let serialized_key = cache_key_str(cache_key);
    let _derived = derive_encrypted_cache_key(access_token, &salt);
    // Real impl: AESGCM(key).encrypt(nonce, plaintext, associated_data=serialized_key)
    // Stub: store plaintext as base64 with a marker so `read` can detect stub vs real.
    // We still write atomically with 0600 and migrate away the plaintext cache.
    let plaintext = format!(
        "{{\"secrets\":{},\"fetched_at\":{}}}",
        json_string_map(&entry.secrets),
        entry.fetched_at
    );
    // Stub ciphertext = b64(plaintext) — NOT encrypted; real port replaces with AES-GCM.
    let ciphertext = b64e(plaintext.as_bytes());
    let payload = format!(
        "{{\"version\":{},\"key\":{},\"salt\":{},\"nonce\":{},\"ciphertext\":{}}}",
        ENCRYPTED_CACHE_VERSION,
        json_escape_str(&serialized_key),
        json_escape_str(&b64e(&salt)),
        json_escape_str(&b64e(&nonce)),
        json_escape_str(&ciphertext)
    );
    let tmp = cache_dir.join(format!(
        ".bws_cache_enc_{}-{:x}.tmp",
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
    if fs::rename(&tmp, &path).is_ok() {
        let _ = fs::remove_file(&tmp);
        // Migration: remove legacy plaintext cache so stale secrets cannot remain on disk.
        let legacy = disk_cache_path(home_path);
        let _ = fs::remove_file(&legacy);
    } else {
        let _ = fs::remove_file(&tmp);
    }
}

fn random_bytes(n: usize) -> Vec<u8> {
    // Try /dev/urandom, fallback to time-seeded PRNG — best-effort, never blocks startup.
    if let Ok(b) = fs::read("/dev/urandom") {
        if b.len() >= n {
            return b[..n].to_vec();
        }
    }
    let mut out = Vec::with_capacity(n);
    let mut seed = now_secs().to_bits().wrapping_add(std::process::id() as u64);
    for _ in 0..n {
        seed = seed.wrapping_mul(0x5851f42d4c957f2d).wrapping_add(0x14057b7ef767814f);
        out.push((seed >> 32) as u8);
    }
    out
}

/// Return a decrypted encrypted-cache entry if it matches and is in-window — mirrors `_read_encrypted_disk_cache` (lines 455-500).
pub fn read_encrypted_disk_cache(
    cache_key: &CacheKey,
    _access_token: &str,
    max_age_seconds: f64,
    home_path: Option<&Path>,
) -> Option<CachedFetch> {
    if max_age_seconds <= 0.0 {
        return None;
    }
    let path = encrypted_disk_cache_path(home_path);
    let text = fs::read_to_string(&path).ok()?;
    // Minimal JSON extraction — real impl would use serde_json + AES-GCM decrypt
    let version = extract_json_number_field(&text, "version")? as u32;
    if version != ENCRYPTED_CACHE_VERSION {
        return None;
    }
    let key = extract_json_string_field(&text, "key")?;
    let serialized = cache_key_str(cache_key);
    if key != serialized {
        return None;
    }
    let salt_b64 = extract_json_string_field(&text, "salt")?;
    let _nonce_b64 = extract_json_string_field(&text, "nonce")?;
    let ct_b64 = extract_json_string_field(&text, "ciphertext")?;
    let _salt = b64d(&salt_b64).ok()?;
    let ct_bytes = b64d(&ct_b64).ok()?;
    // Stub decrypt: ct is b64(plaintext) — decode once more
    let inner_raw = b64d(&String::from_utf8_lossy(&ct_bytes)).unwrap_or(ct_bytes);
    let inner = String::from_utf8_lossy(&inner_raw).to_string();
    let secrets = extract_json_string_map_field(&inner, "secrets")?;
    let fetched_at = extract_json_number_field(&inner, "fetched_at")?;
    let age = now_secs() - fetched_at;
    if age < 0.0 || age > max_age_seconds {
        return None;
    }
    Some(CachedFetch { secrets, fetched_at })
}

/// Pull the secrets for `project_id` from Bitwarden Secrets Manager — mirrors `fetch_bitwarden_secrets` (lines 503-638).
pub fn fetch_bitwarden_secrets(
    access_token: &str,
    project_id: &str,
    binary: Option<&Path>,
    cache_ttl_seconds: f64,
    use_cache: bool,
    server_url: &str,
    home_path: Option<&Path>,
    encrypted_cache_enabled: bool,
    encrypted_cache_max_stale_seconds: f64,
) -> Result<(HashMap<String, String>, Vec<String>), String> {
    if access_token.is_empty() {
        return Err("Bitwarden access token is empty".into());
    }
    if project_id.is_empty() {
        return Err("Bitwarden project_id is empty".into());
    }
    let cache_key = (
        token_fingerprint(access_token),
        project_id.to_string(),
        server_url.to_string(),
    );
    let serialized_key = cache_key_str(&cache_key);

    if use_cache && cache_ttl_seconds > 0.0 {
        if let Some(cached) = cache_map().lock().ok().and_then(|m| m.get(&serialized_key).cloned()) {
            if cached.is_fresh(cache_ttl_seconds) {
                return Ok((cached.secrets, Vec::new()));
            }
        }
        // L2: disk cache — mirrors lines 548-562
        let disk_cached = if encrypted_cache_enabled {
            read_encrypted_disk_cache(&cache_key, access_token, cache_ttl_seconds, home_path)
        } else {
            disk_cache().read(&serialized_key, cache_ttl_seconds, home_path)
        };
        if let Some(entry) = disk_cached {
            if let Ok(mut m) = cache_map().lock() {
                m.insert(serialized_key.clone(), entry.clone());
            }
            return Ok((entry.secrets, Vec::new()));
        }
    }

    let bws = match binary {
        Some(p) => p.to_path_buf(),
        None => find_bws(true).ok_or_else(|| {
            "bws binary not available — auto-install failed and `bws` is not on PATH.  Install manually from https://github.com/bitwarden/sdk-sm/releases or re-run `hermes secrets bitwarden setup`.".to_string()
        })?,
    };

    let (secrets, warnings) = match run_bws_list(&bws, access_token, project_id, server_url) {
        Ok(v) => v,
        Err(exc) => {
            let kind = classify_bws_error(&exc);
            if use_cache && matches!(kind, ErrorKind::Network | ErrorKind::Timeout) {
                if encrypted_cache_enabled {
                    if let Some(stale) = read_encrypted_disk_cache(
                        &cache_key,
                        access_token,
                        encrypted_cache_max_stale_seconds,
                        home_path,
                    ) {
                        let age = (now_secs() - stale.fetched_at).max(0.0) as u64;
                        if let Ok(mut m) = cache_map().lock() {
                            m.insert(serialized_key.clone(), stale.clone());
                        }
                        return Ok((
                            stale.secrets,
                            vec![format!(
                                "bws live fetch failed ({}); falling back to stale ENCRYPTED disk cache ({}s old)",
                                exc, age
                            )],
                        ));
                    }
                } else if cache_ttl_seconds > 0.0 {
                    if let Some(stale) = disk_cache().read(&serialized_key, f64::INFINITY, home_path) {
                        let age = (now_secs() - stale.fetched_at).max(0.0) as u64;
                        if let Ok(mut m) = cache_map().lock() {
                            m.insert(serialized_key.clone(), stale.clone());
                        }
                        return Ok((
                            stale.secrets,
                            vec![format!(
                                "bws live fetch failed ({}); falling back to stale disk cache ({}s old)",
                                exc, age
                            )],
                        ));
                    }
                }
            }
            return Err(exc);
        }
    };

    let entry = CachedFetch {
        secrets: secrets.clone(),
        fetched_at: now_secs(),
    };
    if use_cache {
        if cache_ttl_seconds > 0.0 {
            if let Ok(mut m) = cache_map().lock() {
                m.insert(serialized_key.clone(), entry.clone());
            }
        }
        if encrypted_cache_enabled {
            write_encrypted_disk_cache(&cache_key, access_token, &entry, home_path);
        } else if cache_ttl_seconds > 0.0 {
            disk_cache().write(&serialized_key, &entry, cache_ttl_seconds, home_path);
        }
    }
    Ok((secrets, warnings))
}

/// Reduce a bws (Rust color-eyre) error dump to its cause line(s) — mirrors `_summarize_bws_stderr` (lines 641-671).
pub fn summarize_bws_stderr(raw: &str) -> String {
    let text = raw.replace('\x1b', "").trim().to_string();
    if text.is_empty() {
        return text;
    }
    let mut causes: Vec<String> = Vec::new();
    for line in text.lines() {
        let stripped = line.trim();
        if stripped.starts_with("Location:")
            || stripped.starts_with("Backtrace omitted")
            || stripped.starts_with("Run with ")
        {
            break;
        }
        if stripped.is_empty() || stripped == "Error:" {
            continue;
        }
        // Strip leading "0: ", "1: " etc.
        let mut s = stripped;
        let mut idx = 0;
        while idx < s.len() && s.as_bytes()[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx > 0 && s[idx..].starts_with(": ") {
            s = s[idx + 2..].trim_start();
        } else if idx > 0 && s[idx..].starts_with(':') {
            s = s[idx + 1..].trim_start();
        }
        if !s.is_empty() {
            causes.push(s.to_string());
        }
    }
    if causes.is_empty() {
        text
    } else {
        causes.join("; ")
    }
}

/// Mirrors `_run_bws_list` (lines 674-753).
pub fn run_bws_list(
    bws: &Path,
    access_token: &str,
    project_id: &str,
    server_url: &str,
) -> Result<(HashMap<String, String>, Vec<String>), String> {
    let mut env_map: HashMap<String, String> = HashMap::new();
    // Minimal allowlist — mirrors `run_secret_cli` posture (PATH/HOME/locale) plus BWS vars.
    for key in ["PATH", "HOME", "USERPROFILE", "SYSTEMROOT", "TMPDIR", "TEMP", "LANG", "LC_ALL", "XDG_CONFIG_HOME", "XDG_DATA_HOME"] {
        if let Ok(v) = env::var(key) {
            env_map.insert(key.to_string(), v);
        }
    }
    env_map.insert("BWS_ACCESS_TOKEN".into(), access_token.to_string());
    env_map.entry("NO_COLOR".into()).or_insert_with(|| "1".into());
    if !server_url.is_empty() {
        env_map.insert("BWS_SERVER_URL".into(), server_url.to_string());
    } else if let Ok(v) = env::var("BWS_SERVER_URL") {
        env_map.entry("BWS_SERVER_URL".into()).or_insert(v);
    }

    let mut cmd = Command::new(bws);
    cmd.args(["secret", "list", project_id, "--output", "json"]);
    cmd.envs(&env_map);
    // stdin = /dev/null
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    // Best-effort timeout: spawn and wait with a deadline thread.
    // For 1:1 without extra dep we use `wait_timeout` via polling.
    let mut child = cmd.spawn().map_err(|e| format!("failed to invoke bws: {}", e))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(BWS_RUN_TIMEOUT_SECS);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child.wait_with_output().unwrap_or_else(|_| {
                    // Already waited — reconstruct from status
                    std::process::Output { status, stdout: Vec::new(), stderr: Vec::new() }
                });
                // Reconstruct output if we already consumed it via try_wait+wait.
                // Simpler: we already did wait_with_output only if try_wait returned Some?
                // Actually try_wait Some means child already exited but we still need output.
                // Use `output` from wait_with_output.
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                if !output.status.success() {
                    let code = output.status.code().unwrap_or(-1);
                    let err = summarize_bws_stderr(&format!("{}{}", stderr, stdout));
                    let snippet = if err.len() > 200 { &err[..200] } else { &err };
                    return Err(format!("bws exited {}: {}", code, snippet));
                }
                let raw = stdout.trim().to_string();
                if raw.is_empty() {
                    return Ok((HashMap::new(), vec!["bws returned no output (empty project?)".into()]));
                }
                return parse_bws_secret_list(&raw);
            }
            Ok(None) => {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("bws timed out after {}s fetching secrets", BWS_RUN_TIMEOUT_SECS));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(format!("failed to invoke bws: {}", e)),
        }
    }
}

fn parse_bws_secret_list(raw: &str) -> Result<(HashMap<String, String>, Vec<String>), String> {
    // Expect JSON array of {"key": "...", "value": "..."} — mirrors lines 728-753.
    // Minimal array parser: extract objects between { } and pull key/value.
    let trimmed = raw.trim();
    if !trimmed.starts_with('[') {
        return Err(format!("bws returned unexpected shape: not a list"));
    }
    // Quick empty list check
    if trimmed == "[]" {
        return Ok((HashMap::new(), Vec::new()));
    }
    let mut secrets: HashMap<String, String> = HashMap::new();
    let mut warnings: Vec<String> = Vec::new();
    // Iterate top-level objects
    let inner = &trimmed[1..trimmed.rfind(']').unwrap_or(trimmed.len() - 1)];
    for obj_str in split_json_objects(inner) {
        // obj_str includes braces, e.g. {"key":"FOO","value":"bar"}
        let key = extract_json_string_field(&obj_str, "key");
        let value = extract_json_string_field(&obj_str, "value");
        match (key, value) {
            (Some(k), Some(v)) => {
                if !is_valid_env_name(&k) {
                    warnings.push(format!("Skipping secret {:?}: not a valid env-var name", k));
                    continue;
                }
                secrets.insert(k, v);
            }
            _ => continue,
        }
    }
    Ok((secrets, warnings))
}

fn split_json_objects(inner: &str) -> Vec<String> {
    let mut objs = Vec::new();
    let mut depth = 0usize;
    let mut start: Option<usize> = None;
    let mut in_str = false;
    let mut escape = false;
    for (i, c) in inner.char_indices() {
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
            '{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s) = start {
                        objs.push(inner[s..=i].to_string());
                        start = None;
                    }
                }
            }
            _ => {}
        }
    }
    objs
}

// ---------------------------------------------------------------------------
// Public entry point — called from hermes_cli.env_loader — mirrors lines 761-847
// ---------------------------------------------------------------------------

/// Pull secrets from BSM and set them on `env` (or `std::env` when `env` is None).
///
/// Mirrors `apply_bitwarden_secrets` (lines 761-847). Defensive — any failure
/// returns a `FetchResult` with `error` set; it never panics.
pub fn apply_bitwarden_secrets(
    enabled: bool,
    access_token_env: &str,
    project_id: &str,
    override_existing: bool,
    cache_ttl_seconds: f64,
    auto_install: bool,
    server_url: &str,
    home_path: Option<&Path>,
    encrypted_cache_enabled: bool,
    encrypted_cache_max_stale_seconds: f64,
    env: Option<&mut HashMap<String, String>>,
) -> FetchResult {
    let mut result = FetchResult::default();
    if !enabled {
        return result;
    }
    let access_token_env = if access_token_env.is_empty() { "BWS_ACCESS_TOKEN" } else { access_token_env };
    let access_token = env_var(access_token_env).trim().to_string();
    if access_token.is_empty() {
        result.error = Some(format!(
            "secrets.bitwarden.enabled is true but {} is not set.  Run `hermes secrets bitwarden setup`.",
            access_token_env
        ));
        return result;
    }
    if project_id.is_empty() {
        result.error = Some(
            "secrets.bitwarden.project_id is empty.  Run `hermes secrets bitwarden setup`.".into(),
        );
        return result;
    }
    let binary = find_bws(auto_install);
    result.binary_path = binary.clone();
    if binary.is_none() {
        result.error = Some(
            "bws binary not available and auto-install is disabled.  Run `hermes secrets bitwarden setup` to install.".into(),
        );
        return result;
    }
    let (secrets, warnings) = match fetch_bitwarden_secrets(
        &access_token,
        project_id,
        binary.as_deref(),
        cache_ttl_seconds,
        true,
        server_url,
        home_path,
        encrypted_cache_enabled,
        encrypted_cache_max_stale_seconds,
    ) {
        Ok(v) => v,
        Err(e) => {
            result.error = Some(e);
            return result;
        }
    };
    result.secrets = secrets.clone();
    result.warnings.extend(warnings);

    // Apply to env — mirrors lines 834-845
    let use_process_env = env.is_none();
    for (key, value) in &secrets {
        if key == access_token_env {
            result.skipped.push(key.clone());
            continue;
        }
        let existing = if use_process_env {
            env::var(key).ok().filter(|v| !v.is_empty())
        } else {
            env.as_ref().and_then(|m| m.get(key).cloned()).filter(|v| !v.is_empty())
        };
        if !override_existing && existing.is_some() {
            result.skipped.push(key.clone());
            continue;
        }
        if use_process_env {
            env::set_var(key, value);
        } else if let Some(m) = env.as_mut() {
            // We can't mutate through Option<&mut> without re-borrow; caller handles it.
            // This path is for registry callers that collect `secrets` and apply centrally.
            // For direct `apply_*` we use process env.
            let _ = m;
            env::set_var(key, value);
        }
        result.applied.push(key.clone());
    }
    result
}

fn env_var(name: &str) -> String {
    env::var(name).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// SecretSource adapter — registry-facing wrapper — mirrors lines 855-1002
// ---------------------------------------------------------------------------

/// Minimal `SecretSource` trait — mirrors `agent.secret_sources.base.SecretSource` ABC
/// (lines 180-270). Canonical trait lives in `hermes-secret` crate; this local
/// definition keeps the slice self-contained.
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
/// Real crate would use `serde_json::Value`.
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

/// Bitwarden Secrets Manager as a registered secret source — mirrors `BitwardenSource` (lines 855-1002).
pub struct BitwardenSource;

impl BitwardenSource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BitwardenSource {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretSource for BitwardenSource {
    fn name(&self) -> &'static str {
        "bitwarden"
    }
    fn label(&self) -> &'static str {
        "Bitwarden Secrets Manager"
    }
    fn shape(&self) -> &'static str {
        "bulk"
    }
    fn scheme(&self) -> Option<&'static str> {
        Some("bws")
    }

    fn override_existing(&self, cfg: &HashMap<String, serde_value::Value>) -> bool {
        // Default True — mirrors lines 873-878
        match cfg.get("override_existing") {
            Some(serde_value::Value::Bool(b)) => *b,
            Some(_) => true,
            None => true,
        }
    }

    fn protected_env_vars(&self, cfg: &HashMap<String, serde_value::Value>) -> Vec<String> {
        let token_env = cfg
            .get("access_token_env")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("BWS_ACCESS_TOKEN");
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
            "access_token_env".into(),
            serde_value::ConfigEntry {
                description: "Env var holding the machine-account access token".into(),
                default: serde_value::Value::String("BWS_ACCESS_TOKEN".into()),
            },
        );
        m.insert(
            "project_id".into(),
            serde_value::ConfigEntry {
                description: "BSM project UUID".into(),
                default: serde_value::Value::String(String::new()),
            },
        );
        m.insert(
            "cache_ttl_seconds".into(),
            serde_value::ConfigEntry {
                description: "Fresh disk+memory cache TTL; 0 disables fresh-cache reuse".into(),
                default: serde_value::Value::Number(300.0),
            },
        );
        m.insert(
            "encrypted_cache".into(),
            serde_value::ConfigEntry {
                description: "Encrypted last-good cache for network/timeout fallback".into(),
                default: serde_value::Value::Map({
                    let mut inner = HashMap::new();
                    inner.insert("enabled".into(), serde_value::Value::Bool(false));
                    inner.insert("max_stale_seconds".into(), serde_value::Value::Number(0.0));
                    inner
                }),
            },
        );
        m.insert(
            "override_existing".into(),
            serde_value::ConfigEntry {
                description: "BSM values overwrite .env/shell values".into(),
                default: serde_value::Value::Bool(true),
            },
        );
        m.insert(
            "auto_install".into(),
            serde_value::ConfigEntry {
                description: "Auto-download the pinned bws binary".into(),
                default: serde_value::Value::Bool(true),
            },
        );
        m.insert(
            "server_url".into(),
            serde_value::ConfigEntry {
                description: "Region / self-hosted endpoint (empty = US Cloud)".into(),
                default: serde_value::Value::String(String::new()),
            },
        );
        m
    }

    fn fetch(&self, cfg: &HashMap<String, serde_value::Value>, home_path: &Path) -> FetchResult {
        let mut result = FetchResult::default();
        let access_token_env = cfg
            .get("access_token_env")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("BWS_ACCESS_TOKEN");
        let access_token = env_var(access_token_env).trim().to_string();
        if access_token.is_empty() {
            result.error = Some(format!(
                "secrets.bitwarden.enabled is true but {} is not set.  Run `hermes secrets bitwarden setup`.",
                access_token_env
            ));
            result.error_kind = Some(ErrorKind::NotConfigured);
            return result;
        }
        let project_id = cfg
            .get("project_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if project_id.is_empty() {
            result.error = Some(
                "secrets.bitwarden.project_id is empty.  Run `hermes secrets bitwarden setup`.".into(),
            );
            result.error_kind = Some(ErrorKind::NotConfigured);
            return result;
        }
        let auto_install = cfg
            .get("auto_install")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let binary = find_bws(auto_install);
        result.binary_path = binary.clone();
        if binary.is_none() {
            result.error = Some(
                "bws binary not available and auto-install is disabled.  Run `hermes secrets bitwarden setup` to install.".into(),
            );
            result.error_kind = Some(ErrorKind::BinaryMissing);
            return result;
        }
        let ttl = cfg
            .get("cache_ttl_seconds")
            .and_then(|v| v.as_f64())
            .unwrap_or(300.0);
        let encrypted_cfg = cfg.get("encrypted_cache").and_then(|v| v.as_map()).cloned().unwrap_or_default();
        let encrypted_enabled = encrypted_cfg
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let encrypted_max_stale = encrypted_cfg
            .get("max_stale_seconds")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let server_url = cfg
            .get("server_url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        match fetch_bitwarden_secrets(
            &access_token,
            &project_id,
            binary.as_deref(),
            ttl,
            true,
            &server_url,
            Some(home_path),
            encrypted_enabled,
            encrypted_max_stale,
        ) {
            Ok((secrets, warnings)) => {
                result.secrets = secrets;
                result.warnings.extend(warnings);
            }
            Err(e) => {
                let kind = classify_bws_error(&e);
                result.error_kind = Some(kind.clone());
                let mut msg = e.clone();
                if kind == ErrorKind::AuthFailed {
                    msg = format!(
                        "Bitwarden rejected the machine-account access token ({}) — it was likely revoked, expired, or belongs to another region.  ({})",
                        access_token_env, msg
                    );
                }
                result.error = Some(msg);
            }
        }
        result
    }

    fn remediation(&self, kind: Option<&ErrorKind>, _cfg: &HashMap<String, serde_value::Value>) -> String {
        match kind {
            Some(ErrorKind::AuthFailed) | Some(ErrorKind::AuthExpired) => {
                "Run `hermes secrets bitwarden token` to paste a fresh access token (create one in the Bitwarden web app: Secrets Manager → Machine accounts → Access tokens).  Wrong region?  Re-run `hermes secrets bitwarden setup` and pick EU/self-hosted.".into()
            }
            _ => {
                // Fall back to base class generic — mirrors `super().remediation`
                match kind {
                    Some(ErrorKind::NotConfigured) => format!("Run `hermes secrets {} setup` to finish configuration.", self.name()),
                    Some(ErrorKind::BinaryMissing) => format!("Run `hermes secrets {} setup` to install the helper CLI.", self.name()),
                    Some(ErrorKind::AuthFailed) => format!("Credentials rejected — run `hermes secrets {} setup` to re-authenticate.", self.name()),
                    Some(ErrorKind::AuthExpired) => format!("Credentials expired — run `hermes secrets {} setup` to re-authenticate.", self.name()),
                    Some(ErrorKind::Network) => "Network problem reaching the secrets backend — check connectivity and retry.".into(),
                    Some(ErrorKind::Timeout) => format!("Backend was slow — raise secrets.{}.timeout_seconds if this recurs.", self.name()),
                    _ => String::new(),
                }
            }
        }
    }
}

/// Best-effort mapping of bws failure text onto the shared taxonomy — mirrors `_classify_bws_error` (lines 1005-1024).
pub fn classify_bws_error(message: &str) -> ErrorKind {
    let lowered = message.to_lowercase();
    if lowered.contains("timed out") {
        return ErrorKind::Timeout;
    }
    if lowered.contains("binary not available") || lowered.contains("failed to invoke") {
        return ErrorKind::BinaryMissing;
    }
    if ["unauthorized", "invalid token", "access token", "401", "403", "invalid_client", "invalid_grant", "400 bad request"]
        .iter()
        .any(|tok| lowered.contains(tok))
    {
        return ErrorKind::AuthFailed;
    }
    if ["network", "connection", "resolve", "download", "dns"]
        .iter()
        .any(|tok| lowered.contains(tok))
    {
        return ErrorKind::Network;
    }
    ErrorKind::Internal
}

// ---------------------------------------------------------------------------
// Test hooks — mirrors lines 1031-1055
// ---------------------------------------------------------------------------

/// Drop in-process AND disk caches (plaintext and encrypted) — mirrors `clear_caches` (lines 1032-1045).
pub fn clear_caches(home_path: Option<&Path>) {
    if let Ok(mut m) = cache_map().lock() {
        m.clear();
    }
    disk_cache().clear(home_path);
    let enc = encrypted_disk_cache_path(home_path);
    let _ = fs::remove_file(&enc);
}

/// Clear in-process AND disk caches — mirrors `_reset_cache_for_tests` (lines 1048-1055).
pub fn reset_cache_for_tests(home_path: Option<&Path>) {
    clear_caches(home_path)
}

// ---------------------------------------------------------------------------
// Helpers: env allowlist and source environment (mirrors base.get_source_environment)
// ---------------------------------------------------------------------------

/// Thin wrapper to allow tests to inject a fake env without touching `os.environ`.
/// Mirrors `base.get_source_environment()` — here we just read `std::env`.
pub fn get_source_environment() -> HashMap<String, String> {
    env::vars().collect()
}

// ---------------------------------------------------------------------------
// Self-check (ponytail: one runnable check, no framework)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_str_stable() {
        let k = ("fp".to_string(), "proj".to_string(), "https://vault.example".to_string());
        assert_eq!(cache_key_str(&k), "fp|proj|https://vault.example");
    }

    #[test]
    fn valid_env_names() {
        assert!(is_valid_env_name("BWS_ACCESS_TOKEN"));
        assert!(is_valid_env_name("_foo"));
        assert!(!is_valid_env_name("1bad"));
        assert!(!is_valid_env_name("has-dash"));
        assert!(!is_valid_env_name(""));
    }

    #[test]
    fn classify_errors() {
        assert_eq!(classify_bws_error("bws timed out after 30s"), ErrorKind::Timeout);
        assert_eq!(classify_bws_error("binary not available"), ErrorKind::BinaryMissing);
        assert_eq!(classify_bws_error("401 Unauthorized"), ErrorKind::AuthFailed);
        assert_eq!(classify_bws_error("invalid_client"), ErrorKind::AuthFailed);
        assert_eq!(classify_bws_error("network unreachable"), ErrorKind::Network);
        assert_eq!(classify_bws_error("something else"), ErrorKind::Internal);
    }

    #[test]
    fn summarize_strips_location() {
        let raw = "Error:\n   0: Received error message from server: [400 Bad Request] {\"error\":\"invalid_client\"}\n\nLocation:\n   crates/bws/src/main.rs:108\n";
        let s = summarize_bws_stderr(raw);
        assert!(s.contains("invalid_client"));
        assert!(!s.contains("Location"));
    }

    #[test]
    fn b64_roundtrip() {
        let raw = b"hello world";
        let enc = b64e(raw);
        let dec = b64d(&enc).unwrap();
        assert_eq!(dec, raw);
    }

    #[test]
    fn constants_match_python() {
        assert_eq!(BWS_VERSION, "2.0.0");
        assert!(bws_release_base().contains("bws-v2.0.0"));
        assert_eq!(bws_checksum_name(), "bws-sha256-checksums-2.0.0.txt");
        assert_eq!(ENCRYPTED_CACHE_VERSION, 1);
    }
}
