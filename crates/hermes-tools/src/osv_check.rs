//! OSV malware check for MCP extension packages.
//! Port of `tools/osv_check.py` (218 lines) — 1:1 behavior.
//!
//! Before launching an MCP server via npx/uvx, queries the OSV (Open Source
//! Vulnerabilities) API to check if the package has any known malware advisories
//! (MAL-* IDs). Regular CVEs are ignored — only confirmed malware is blocked.
//!
//! The API is free, public, and maintained by Google. Typical latency is ~300ms.
//! Fail-open: network errors allow the package to proceed.
//!
//! Inspired by Block/goose's extension malware check.
//!
//! Rust mapping
//! ------------
//! - `_OSV_ENDPOINT = os.getenv("OSV_ENDPOINT", "https://api.osv.dev/v1/query")` → [`OSV_ENDPOINT_DEFAULT`] + [`osv_endpoint`]
//! - `_TIMEOUT = 10` → [`TIMEOUT_SECS`] + [`TIMEOUT_DURATION`]
//! - `_CACHE_TTL_S = float(os.getenv("OSV_CHECK_CACHE_TTL", "3600"))` → [`CACHE_TTL_S_DEFAULT`] + [`osv_cache_ttl`] / [`osv_cache_ttl_duration`]
//! - `_CACHE_MAX_ENTRIES = 256` → [`CACHE_MAX_ENTRIES`]
//! - `_cache: dict` + `_cache_lock = threading.Lock()` → `static CACHE: OnceLock<Mutex<HashMap<CacheKey, CacheEntry>>>`
//! - `def _cache_get(key)` → [`cache_get`] (monotonic expiry via `Instant`)
//! - `def _cache_put(key, result)` → [`cache_put`] (evict expired, clear on overflow)
//! - `def check_package_for_malware(command, args)` → [`check_package_for_malware`] + [`check_package_for_malware_with_fetch`] (fail-open, caches success only)
//! - `def _infer_ecosystem(command)` → [`infer_ecosystem`]
//! - `def _parse_package_from_args(args, ecosystem)` → [`parse_package_from_args`]
//! - `def _parse_npm_package(token)` → [`parse_npm_package`]
//! - `def _parse_pypi_package(token)` → [`parse_pypi_package`]
//! - `def _query_osv(package, ecosystem, version)` → [`query_osv`] + [`query_osv_with_fetch`] + [`build_osv_payload`] + [`parse_osv_response`] + [`filter_malware_vulns`]
//! - `urllib.request.Request` + `urlopen` + `json.dumps` → injectable `fetch` closure (`Fn(&str, &str) -> Result<String, String>`) + [`default_fetch`] stub (fail-open; real HTTP wiring lives in `gray-provider` when linked, same as `microsoft_graph_auth` transport pattern)
//! - `v.get("id","").startswith("MAL-")` → [`is_malware_id`] + [`filter_malware_vulns`]
//! - BLOCKED message formatting (ids/summaries, [:100], first 3) → [`format_blocked_message`]

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Constants — mirrors lines 24-37
// ---------------------------------------------------------------------------

/// Mirrors `_OSV_ENDPOINT = os.getenv("OSV_ENDPOINT", "https://api.osv.dev/v1/query")` (24).
pub const OSV_ENDPOINT_DEFAULT: &str = "https://api.osv.dev/v1/query";

/// Environment variable for OSV endpoint override — mirrors `OSV_ENDPOINT` (24).
pub const OSV_ENDPOINT_ENV: &str = "OSV_ENDPOINT";

/// Mirrors `_TIMEOUT = 10` seconds (25).
pub const TIMEOUT_SECS: u64 = 10;

/// Mirrors `User-Agent: hermes-agent-osv-check/1.0` (207).
pub const USER_AGENT: &str = "hermes-agent-osv-check/1.0";

/// Mirrors `_CACHE_TTL_S = float(os.getenv("OSV_CHECK_CACHE_TTL", "3600"))` (36).
pub const CACHE_TTL_S_DEFAULT: f64 = 3600.0;

/// Environment variable for cache TTL — mirrors `OSV_CHECK_CACHE_TTL` (36).
pub const CACHE_TTL_ENV: &str = "OSV_CHECK_CACHE_TTL";

/// Mirrors `_CACHE_MAX_ENTRIES = 256` (37).
pub const CACHE_MAX_ENTRIES: usize = 256;

/// Mirrors `__all__` implicit surface of osv_check.py.
pub const ALL: &[&str] = &[
    "check_package_for_malware",
    "infer_ecosystem",
    "parse_package_from_args",
    "parse_npm_package",
    "parse_pypi_package",
    "query_osv",
];

// ---------------------------------------------------------------------------
// Env helpers — mirrors os.getenv usage (24, 36)
// ---------------------------------------------------------------------------

/// Mirrors `os.getenv("OSV_ENDPOINT", "https://api.osv.dev/v1/query")` (24).
pub fn osv_endpoint() -> String {
    match std::env::var(OSV_ENDPOINT_ENV) {
        Ok(v) if !v.is_empty() => v,
        Ok(_) => OSV_ENDPOINT_DEFAULT.to_string(),
        Err(_) => OSV_ENDPOINT_DEFAULT.to_string(),
    }
}

/// Mirrors `float(os.getenv("OSV_CHECK_CACHE_TTL", "3600"))` (36).
pub fn osv_cache_ttl() -> f64 {
    match std::env::var(CACHE_TTL_ENV) {
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                CACHE_TTL_S_DEFAULT
            } else {
                trimmed.parse::<f64>().unwrap_or(CACHE_TTL_S_DEFAULT)
            }
        }
        Err(_) => CACHE_TTL_S_DEFAULT,
    }
}

/// Duration form of [`osv_cache_ttl()`] — mirrors `time.monotonic() + _CACHE_TTL_S` (63).
pub fn osv_cache_ttl_duration() -> Duration {
    let secs = osv_cache_ttl();
    if secs.is_nan() || secs.is_infinite() || secs <= 0.0 {
        Duration::from_secs_f64(CACHE_TTL_S_DEFAULT)
    } else {
        Duration::from_secs_f64(secs)
    }
}

/// Mirrors `_TIMEOUT` as `Duration`.
pub fn timeout_duration() -> Duration {
    Duration::from_secs(TIMEOUT_SECS)
}

// ---------------------------------------------------------------------------
// Cache — mirrors lines 27-63
// ---------------------------------------------------------------------------

/// Cache key: (ecosystem, package, version) — mirrors `cache_key = (ecosystem, package, version)` (86).
pub type CacheKey = (String, String, Option<String>);

#[derive(Debug, Clone)]
struct CacheEntry {
    expiry: Instant,
    result: Option<String>,
}

static CACHE: OnceLock<Mutex<HashMap<CacheKey, CacheEntry>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<CacheKey, CacheEntry>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Mirrors `def _cache_get(key) -> Tuple[bool, Optional[str]]:` (42-52).
///
/// Returns `Some(result)` on hit where `result` is `Option<String>` (`None` = clean, `Some` = blocked message),
/// `None` on miss or expiry (expired entry is removed).
pub fn cache_get(key: &CacheKey) -> Option<Option<String>> {
    let mut map = cache().lock().ok()?;
    let entry = map.get(key)?;
    if Instant::now() >= entry.expiry {
        map.remove(key);
        return None;
    }
    Some(entry.result.clone())
}

/// Mirrors `def _cache_put(key, result: Optional[str]) -> None:` (55-63).
pub fn cache_put(key: CacheKey, result: Option<String>) {
    let ttl = osv_cache_ttl_duration();
    let expiry = Instant::now() + ttl;
    let mut map = match cache().lock() {
        Ok(m) => m,
        Err(_) => return,
    };
    if map.len() >= CACHE_MAX_ENTRIES {
        // Evict expired first — mirrors `for k in [k for k, (exp, _) in _cache.items() if exp <= now]: del _cache[k]` (59-60)
        let now = Instant::now();
        let expired: Vec<CacheKey> = map
            .iter()
            .filter_map(|(k, v)| if v.expiry <= now { Some(k.clone()) } else { None })
            .collect();
        for k in expired {
            map.remove(&k);
        }
        if map.len() >= CACHE_MAX_ENTRIES {
            // tiny working set in practice; safe reset — mirrors `_cache.clear()` (62)
            map.clear();
        }
    }
    map.insert(key, CacheEntry { expiry, result });
}

/// Test helper: clear the cache (mirrors resetting `_cache = {}` in tests).
pub fn clear_cache() {
    if let Some(m) = CACHE.get() {
        if let Ok(mut map) = m.lock() {
            map.clear();
        }
    }
}

/// Test helper: number of entries.
pub fn cache_len() -> usize {
    CACHE.get().and_then(|m| m.lock().ok()).map(|map| map.len()).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Vuln types — mirrors _query_osv return (216-218)
// ---------------------------------------------------------------------------

/// Mirrors a single `vulns[]` entry with `id` + optional `summary` — mirrors `v.get("id","")` + `v.get("summary",...)` (99-103, 218).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsvVuln {
    pub id: String,
    pub summary: Option<String>,
}

impl OsvVuln {
    pub fn new(id: impl Into<String>, summary: Option<impl Into<String>>) -> Self {
        Self {
            id: id.into(),
            summary: summary.map(|s| s.into()),
        }
    }
}

/// Mirrors `v.get("id","").startswith("MAL-")` (218).
pub fn is_malware_id(id: &str) -> bool {
    id.starts_with("MAL-")
}

// ---------------------------------------------------------------------------
// _infer_ecosystem — mirrors lines 114-121
// ---------------------------------------------------------------------------

/// Mirrors `def _infer_ecosystem(command: str) -> Optional[str]:` (114-121).
///
/// Uses `os.path.basename(command).lower()` semantics — handles both `/` and `\`.
pub fn infer_ecosystem(command: &str) -> Option<String> {
    // Mirror `os.path.basename` — on POSIX it only splits on '/', but for
    // 1:1 we handle both separators so Windows-style paths are covered.
    let base = command
        .rsplit('/')
        .next()
        .unwrap_or(command)
        .rsplit('\\')
        .next()
        .unwrap_or(command);
    let lower = base.to_ascii_lowercase();
    match lower.as_str() {
        "npx" | "npx.cmd" => Some("npm".to_string()),
        "uvx" | "uvx.cmd" | "pipx" => Some("PyPI".to_string()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// _parse_npm_package — mirrors lines 168-182
// ---------------------------------------------------------------------------

/// Mirrors `def _parse_npm_package(token: str) -> Tuple[Optional[str], Optional[str]]:` (168-182).
///
/// - Scoped: `@scope/name@version` via `^(@[^/]+/[^@]+)(?:@(.+))?$`
/// - Unscoped: `name@version` via `rsplit("@",1)` with `"latest"` special-cased.
pub fn parse_npm_package(token: &str) -> (Option<String>, Option<String>) {
    if token.is_empty() {
        return (None, None);
    }
    if token.starts_with('@') {
        // Scoped: @scope/name@version
        // Regex: ^(@[^/]+/[^@]+)(?:@(.+))?$
        // Find first '/' after '@' (scope delimiter)
        let scope_slice = &token[1..];
        let slash_pos = match scope_slice.find('/') {
            Some(pos) => pos + 1, // index in original token
            None => return (Some(token.to_string()), None),
        };
        // Scope must be non-empty: token[1..slash_pos] cannot be empty (already ensures slash_pos >1)
        if slash_pos <= 1 {
            return (Some(token.to_string()), None);
        }
        let after_slash = &token[slash_pos + 1..];
        if after_slash.is_empty() {
            return (Some(token.to_string()), None);
        }
        // Look for '@' after slash (version delimiter)
        if let Some(at_idx) = after_slash.find('@') {
            let name = &token[..slash_pos + 1 + at_idx];
            let version = &token[slash_pos + 1 + at_idx + 1..];
            // .+ requires non-empty version; empty fails match -> fallback
            if version.is_empty() {
                return (Some(token.to_string()), None);
            }
            // [^@]+ for name part after slash prefix already satisfied (at_idx >0)
            // But need to ensure name part after slash prefix non-empty and no extra check beyond regex
            // If version present, name is before '@', already validated non-empty prefix
            if at_idx == 0 {
                return (Some(token.to_string()), None);
            }
            return (Some(name.to_string()), Some(version.to_string()));
        } else {
            // No version delimiter — entire token is name if it matches @[^/]+/[^@]+
            // Need to check that after_slash has no '@' (already) and is non-empty
            // Also ensure token doesn't have unexpected structure: regex allows "/" in [^@]+, so any remaining slashes are allowed (e.g., "@scope/a/b")
            // So we just return token as name.
            return (Some(token.to_string()), None);
        }
    }
    // Unscoped: name@version
    if token.contains('@') {
        // rsplit("@",1)
        if let Some(pos) = token.rfind('@') {
            let name = &token[..pos];
            let ver = &token[pos + 1..];
            if name.is_empty() {
                return (Some(token.to_string()), None);
            }
            // Python: `version = parts[1] if len(parts) > 1 and parts[1] != "latest" else None`
            // This includes empty string as valid version (since "" != "latest"), but _query_osv will treat empty as falsy and not send it.
            // We mirror exactly: "latest" -> None, otherwise Some(ver) even if ver == ""
            let version = if ver == "latest" { None } else { Some(ver.to_string()) };
            return (Some(name.to_string()), version);
        }
    }
    (Some(token.to_string()), None)
}

// ---------------------------------------------------------------------------
// _parse_pypi_package — mirrors lines 185-191
// ---------------------------------------------------------------------------

/// Mirrors `def _parse_pypi_package(token: str) -> Tuple[Optional[str], Optional[str]]:` (185-191).
///
/// Regex: `^([a-zA-Z0-9._-]+)(?:\[[^\]]*\])?(?:==(.+))?$`
pub fn parse_pypi_package(token: &str) -> (Option<String>, Option<String>) {
    if token.is_empty() {
        return (None, None);
    }
    let bytes = token.as_bytes();
    let mut idx = 0usize;
    // 1. Parse name: [a-zA-Z0-9._-]+
    let name_start = idx;
    while idx < bytes.len() {
        let c = bytes[idx] as char;
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
            idx += 1;
        } else {
            break;
        }
    }
    if idx == name_start {
        return (Some(token.to_string()), None);
    }
    let name = &token[name_start..idx];

    // 2. Optional [extras]: \[[^\]]*\]
    if idx < bytes.len() && bytes[idx] == b'[' {
        // Find closing ']'
        let closing = token[idx..].find(']');
        match closing {
            Some(rel) => {
                // Inside [^\]]* allows any chars except ']' — we already found next ']'
                // No validation on inside content beyond that.
                idx += rel + 1; // move past ']'
            }
            None => {
                // No closing ']' -> no match
                return (Some(token.to_string()), None);
            }
        }
    }

    // 3. Optional ==version: (?:==(.+))?
    if idx + 1 < bytes.len() && bytes[idx] == b'=' && bytes[idx + 1] == b'=' {
        let ver_start = idx + 2;
        if ver_start >= bytes.len() {
            // == with empty version -> .+ fails -> no match
            return (Some(token.to_string()), None);
        }
        let version = &token[ver_start..];
        // .+ requires at least one char, which we have (ver_start < len)
        // Need to ensure we consumed entire token (anchor $) — we do, version takes rest
        return (Some(name.to_string()), Some(version.to_string()));
    }

    // Must be at end
    if idx != bytes.len() {
        return (Some(token.to_string()), None);
    }
    (Some(name.to_string()), None)
}

// ---------------------------------------------------------------------------
// _parse_package_from_args — mirrors lines 124-165
// ---------------------------------------------------------------------------

/// Mirrors `def _parse_package_from_args(args: list, ecosystem: str) -> Tuple[Optional[str], Optional[str]]:` (124-165).
pub fn parse_package_from_args(args: &[String], ecosystem: &str) -> (Option<String>, Option<String>) {
    if args.is_empty() {
        return (None, None);
    }
    let mut package_token: Option<String> = None;
    let mut take_next = false;
    for arg in args {
        if take_next {
            package_token = Some(arg.clone());
            break;
        }
        if arg == "--package" || arg == "-p" {
            take_next = true;
            continue;
        }
        if arg.starts_with("--package=") {
            let v = arg["--package=".len()..].to_string();
            package_token = Some(v);
            break;
        }
        if arg.starts_with('-') {
            continue;
        }
        package_token = Some(arg.clone());
        break;
    }
    let token = match package_token {
        Some(t) if !t.is_empty() => t,
        _ => return (None, None),
    };
    if ecosystem == "npm" {
        parse_npm_package(&token)
    } else if ecosystem == "PyPI" {
        parse_pypi_package(&token)
    } else {
        (Some(token), None)
    }
}

// ---------------------------------------------------------------------------
// OSV payload + response helpers — mirrors lines 194-218
// ---------------------------------------------------------------------------

/// Build the JSON payload for the OSV query — mirrors `payload = {"package": {"name": package, "ecosystem": ecosystem}}` + optional version (198-200).
pub fn build_osv_payload(package: &str, ecosystem: &str, version: Option<&str>) -> Value {
    let mut payload = json!({
        "package": {
            "name": package,
            "ecosystem": ecosystem
        }
    });
    if let Some(v) = version {
        if !v.is_empty() {
            payload["version"] = Value::String(v.to_string());
        }
    }
    payload
}

/// Parse the OSV JSON response body and extract `vulns` — mirrors `result.get("vulns", [])` (216).
pub fn parse_osv_response(body: &str) -> Result<Vec<OsvVuln>, String> {
    let value: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    parse_osv_response_value(&value)
}

/// Value-based variant for injectable fetch that returns `Value`.
pub fn parse_osv_response_value(value: &Value) -> Result<Vec<OsvVuln>, String> {
    let vulns = match value.get("vulns") {
        Some(Value::Array(arr)) => arr,
        Some(_) => return Err("vulns is not an array".to_string()),
        None => return Ok(Vec::new()),
    };
    let mut out = Vec::with_capacity(vulns.len());
    for v in vulns {
        let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let summary = v.get("summary").and_then(|x| x.as_str()).map(|s| s.to_string());
        out.push(OsvVuln { id, summary });
    }
    Ok(out)
}

/// Mirrors `return [v for v in vulns if v.get("id","").startswith("MAL-")]` (218).
pub fn filter_malware_vulns(vulns: Vec<OsvVuln>) -> Vec<OsvVuln> {
    vulns.into_iter().filter(|v| is_malware_id(&v.id)).collect()
}

/// Truncate to first 100 chars like Python `[:100]` (102).
fn truncate_100(s: &str) -> String {
    if s.chars().count() <= 100 {
        s.to_string()
    } else {
        s.chars().take(100).collect()
    }
}

/// Mirrors BLOCKED message formatting in `check_package_for_malware` (99-107).
pub fn format_blocked_message(package: &str, ecosystem: &str, malware: &[OsvVuln]) -> String {
    let ids: String = malware.iter().take(3).map(|m| m.id.as_str()).collect::<Vec<_>>().join(", ");
    let summaries: String = malware
        .iter()
        .take(3)
        .map(|m| {
            let s = m.summary.as_deref().unwrap_or(&m.id);
            truncate_100(s)
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "BLOCKED: Package '{}' ({}) has known malware advisories: {}. Details: {}",
        package, ecosystem, ids, summaries
    )
}

// ---------------------------------------------------------------------------
// _query_osv — mirrors lines 194-218 (with injectable fetch)
// ---------------------------------------------------------------------------

/// Injectable fetch type: `Fn(endpoint, payload_json_str) -> Result<response_body_str, error_str>`.
/// Mirrors `urllib.request.urlopen(req, timeout=_TIMEOUT)` + `json.loads(resp.read())` (213-214).
/// The closure should perform `POST` with headers `Content-Type: application/json` + `User-Agent: hermes-agent-osv-check/1.0`.

/// Default fetch stub — always fails open (no HTTP transport linked in `hermes-tools` crate).
/// Mirrors `except Exception as exc: logger.debug(...) return None` (93-97) — without wiring, every query fails open to `None`.
/// Real wiring lives in `gray-provider` / `hermes-core` when `reqwest` is linked, same as `microsoft_graph_auth` injectable transport.
pub fn default_fetch(_endpoint: &str, _payload: &str) -> Result<String, String> {
    Err("no OSV HTTP transport configured in hermes-tools (inject a fetch closure)".to_string())
}

/// Mirrors `def _query_osv(package, ecosystem, version) -> list:` (194-218) with injectable fetch.
pub fn query_osv_with_fetch<F>(package: &str, ecosystem: &str, version: Option<&str>, fetch: F) -> Result<Vec<OsvVuln>, String>
where
    F: Fn(&str, &str) -> Result<String, String>,
{
    let payload = build_osv_payload(package, ecosystem, version);
    let endpoint = osv_endpoint();
    let data = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    let body = fetch(&endpoint, &data)?;
    let vulns = parse_osv_response(&body)?;
    Ok(filter_malware_vulns(vulns))
}

/// Convenience that uses [`default_fetch`] (always fails) — mirrors fail-open when no transport.
pub fn query_osv(package: &str, ecosystem: &str, version: Option<&str>) -> Result<Vec<OsvVuln>, String> {
    query_osv_with_fetch(package, ecosystem, version, default_fetch)
}

// ---------------------------------------------------------------------------
// check_package_for_malware — mirrors lines 66-111
// ---------------------------------------------------------------------------

/// Core implementation with injectable fetch — mirrors `def check_package_for_malware(command, args)` (66-111).
///
/// - Infers ecosystem via [`infer_ecosystem`] (78)
/// - Parses package via [`parse_package_from_args`] (82)
/// - Checks cache via [`cache_get`] (87-89)
/// - Queries OSV via `fetch` (92) with fail-open on any error (93-97, not cached)
/// - Formats BLOCKED message or None and caches (99-110)
pub fn check_package_for_malware_with_fetch<F>(command: &str, args: &[String], fetch: F) -> Option<String>
where
    F: Fn(&str, &str) -> Result<String, String>,
{
    let ecosystem = infer_ecosystem(command)?;
    let (package_opt, version_opt) = parse_package_from_args(args, &ecosystem);
    let package = package_opt?;
    let version = version_opt;

    let cache_key: CacheKey = (ecosystem.clone(), package.clone(), version.clone());
    if let Some(cached) = cache_get(&cache_key) {
        return cached;
    }

    let malware = match query_osv_with_fetch(&package, &ecosystem, version.as_deref(), &fetch) {
        Ok(m) => m,
        Err(exc) => {
            // Mirrors `logger.debug("OSV check failed for %s/%s (allowing): %s", ecosystem, package, exc)`
            log::debug!("OSV check failed for {}/{} (allowing): {}", ecosystem, package, exc);
            return None;
        }
    };

    let result = if malware.is_empty() {
        None
    } else {
        Some(format_blocked_message(&package, &ecosystem, &malware))
    };
    cache_put(cache_key, result.clone());
    result
}

/// Mirrors `def check_package_for_malware(command: str, args: list) -> Optional[str]:` (66-111).
/// Uses [`default_fetch`] which always fails open; wire a real fetch via [`check_package_for_malware_with_fetch`] for live checks.
pub fn check_package_for_malware(command: &str, args: &[String]) -> Option<String> {
    check_package_for_malware_with_fetch(command, args, default_fetch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    fn s(v: &str) -> String {
        v.to_string()
    }

    // -----------------------------------------------------------------------
    // infer_ecosystem — mirrors lines 114-121
    // -----------------------------------------------------------------------
    #[test]
    fn infer_ecosystem_npm() {
        let _g = test_lock();
        assert_eq!(infer_ecosystem("npx"), Some("npm".to_string()));
        assert_eq!(infer_ecosystem("npx.cmd"), Some("npm".to_string()));
        assert_eq!(infer_ecosystem("/usr/bin/npx"), Some("npm".to_string()));
        assert_eq!(infer_ecosystem("C:\\tools\\npx.cmd"), Some("npm".to_string()));
        assert_eq!(infer_ecosystem("Npx"), Some("npm".to_string()));
        assert_eq!(infer_ecosystem("NPX.CMD"), Some("npm".to_string()));
    }

    #[test]
    fn infer_ecosystem_pypi() {
        let _g = test_lock();
        assert_eq!(infer_ecosystem("uvx"), Some("PyPI".to_string()));
        assert_eq!(infer_ecosystem("uvx.cmd"), Some("PyPI".to_string()));
        assert_eq!(infer_ecosystem("pipx"), Some("PyPI".to_string()));
        assert_eq!(infer_ecosystem("/home/user/.local/bin/uvx"), Some("PyPI".to_string()));
        assert_eq!(infer_ecosystem("UVX"), Some("PyPI".to_string()));
        assert_eq!(infer_ecosystem("Pipx"), Some("PyPI".to_string()));
    }

    #[test]
    fn infer_ecosystem_unknown() {
        let _g = test_lock();
        assert_eq!(infer_ecosystem("node"), None);
        assert_eq!(infer_ecosystem("python"), None);
        assert_eq!(infer_ecosystem(""), None);
        assert_eq!(infer_ecosystem("npxx"), None);
    }

    // -----------------------------------------------------------------------
    // parse_npm_package — mirrors lines 168-182
    // -----------------------------------------------------------------------
    #[test]
    fn parse_npm_scoped() {
        let _g = test_lock();
        assert_eq!(parse_npm_package("@scope/name"), (Some(s("@scope/name")), None));
        assert_eq!(
            parse_npm_package("@scope/name@1.2.3"),
            (Some(s("@scope/name")), Some(s("1.2.3")))
        );
        assert_eq!(
            parse_npm_package("@scope/name@latest"),
            (Some(s("@scope/name")), Some(s("latest")))
        );
        // With extra @ in version (greedy .+)
        assert_eq!(
            parse_npm_package("@scope/name@c@d"),
            (Some(s("@scope/name")), Some(s("c@d")))
        );
    }

    #[test]
    fn parse_npm_scoped_edge() {
        let _g = test_lock();
        // Empty version after @ should be no match -> fallback
        assert_eq!(parse_npm_package("@scope/name@"), (Some(s("@scope/name@")), None));
        // No slash after @ -> no match
        assert_eq!(parse_npm_package("@scope"), (Some(s("@scope")), None));
        assert_eq!(parse_npm_package("@/name"), (Some(s("@/name")), None));
        // Single slash but empty after
        assert_eq!(parse_npm_package("@scope/"), (Some(s("@scope/")), None));
    }

    #[test]
    fn parse_npm_unscoped() {
        let _g = test_lock();
        assert_eq!(parse_npm_package("lodash"), (Some(s("lodash")), None));
        assert_eq!(parse_npm_package("lodash@4.17.21"), (Some(s("lodash")), Some(s("4.17.21"))));
        assert_eq!(parse_npm_package("lodash@latest"), (Some(s("lodash")), None));
        // rsplit on last @
        assert_eq!(parse_npm_package("a@b@c"), (Some(s("a@b")), Some(s("c"))));
        assert_eq!(parse_npm_package("foo@"), (Some(s("foo")), Some(s(""))));
    }

    // -----------------------------------------------------------------------
    // parse_pypi_package — mirrors lines 185-191
    // -----------------------------------------------------------------------
    #[test]
    fn parse_pypi_basic() {
        let _g = test_lock();
        assert_eq!(parse_pypi_package("requests"), (Some(s("requests")), None));
        assert_eq!(
            parse_pypi_package("requests==2.28.0"),
            (Some(s("requests")), Some(s("2.28.0")))
        );
        assert_eq!(parse_pypi_package("my-pkg_1"), (Some(s("my-pkg_1")), None));
        assert_eq!(parse_pypi_package("a.b-c_d"), (Some(s("a.b-c_d")), None));
    }

    #[test]
    fn parse_pypi_extras() {
        let _g = test_lock();
        assert_eq!(
            parse_pypi_package("requests[security]==2.28.0"),
            (Some(s("requests")), Some(s("2.28.0")))
        );
        assert_eq!(parse_pypi_package("name[extra1,extra2]"), (Some(s("name")), None));
        assert_eq!(
            parse_pypi_package("name[extra]==1.0"),
            (Some(s("name")), Some(s("1.0")))
        );
        // Empty version after == -> no match -> fallback
        assert_eq!(parse_pypi_package("name=="), (Some(s("name==")), None));
        // No closing bracket -> fallback
        assert_eq!(parse_pypi_package("name[extra"), (Some(s("name[extra")), None));
        // Trailing chars after extras without == -> fallback
        assert_eq!(parse_pypi_package("name[extra]extra"), (Some(s("name[extra]extra")), None));
        // Invalid name char
        assert_eq!(parse_pypi_package("[extra]"), (Some(s("[extra]")), None));
    }

    // -----------------------------------------------------------------------
    // parse_package_from_args — mirrors lines 124-165
    // -----------------------------------------------------------------------
    #[test]
    fn parse_package_from_args_basic() {
        let _g = test_lock();
        assert_eq!(
            parse_package_from_args(&[s("lodash")], "npm"),
            (Some(s("lodash")), None)
        );
        assert_eq!(
            parse_package_from_args(&[s("requests==1.0")], "PyPI"),
            (Some(s("requests")), Some(s("1.0")))
        );
        assert_eq!(parse_package_from_args(&[], "npm"), (None, None));
    }

    #[test]
    fn parse_package_from_args_flags() {
        let _g = test_lock();
        // Skip flags
        assert_eq!(
            parse_package_from_args(&[s("-y"), s("lodash")], "npm"),
            (Some(s("lodash")), None)
        );
        assert_eq!(
            parse_package_from_args(&[s("--yes"), s("lodash")], "npm"),
            (Some(s("lodash")), None)
        );
        // --package / -p
        assert_eq!(
            parse_package_from_args(&[s("--package"), s("my-pkg"), s("extra")], "npm"),
            (Some(s("my-pkg")), None)
        );
        assert_eq!(
            parse_package_from_args(&[s("-p"), s("my-pkg")], "npm"),
            (Some(s("my-pkg")), None)
        );
        assert_eq!(
            parse_package_from_args(&[s("--package=my-pkg")], "npm"),
            (Some(s("my-pkg")), None)
        );
        assert_eq!(
            parse_package_from_args(&[s("--package=@scope/name@1.0")], "npm"),
            (Some(s("@scope/name")), Some(s("1.0")))
        );
        // --package flag takes next arg, even if it looks like flag value? No, just next arg
        assert_eq!(
            parse_package_from_args(&[s("--package"), s("--not-a-flag")], "npm"),
            (Some(s("--not-a-flag")), None)
        );
        // First bare positional wins if no --package
        assert_eq!(
            parse_package_from_args(&[s("--flag"), s("pkg1"), s("pkg2")], "npm"),
            (Some(s("pkg1")), None)
        );
    }

    #[test]
    fn parse_package_from_args_empty_token() {
        let _g = test_lock();
        assert_eq!(parse_package_from_args(&[s("")], "npm"), (None, None));
        assert_eq!(parse_package_from_args(&[s("--package=")], "npm"), (None, None));
        assert_eq!(parse_package_from_args(&[s("--package"), s("")], "npm"), (None, None));
    }

    // -----------------------------------------------------------------------
    // build payload + filter + format — mirrors lines 194-107
    // -----------------------------------------------------------------------
    #[test]
    fn build_payload() {
        let _g = test_lock();
        let v = build_osv_payload("lodash", "npm", None);
        assert_eq!(v["package"]["name"], json!("lodash"));
        assert_eq!(v["package"]["ecosystem"], json!("npm"));
        assert!(v.get("version").is_none());

        let v2 = build_osv_payload("requests", "PyPI", Some("2.0"));
        assert_eq!(v2["version"], json!("2.0"));

        let v3 = build_osv_payload("pkg", "npm", Some(""));
        assert!(v3.get("version").is_none());

        let v4 = build_osv_payload("pkg", "npm", Some("latest"));
        assert_eq!(v4["version"], json!("latest"));
    }

    #[test]
    fn parse_and_filter() {
        let _g = test_lock();
        let body = r#"{"vulns":[{"id":"MAL-123","summary":"evil"},{"id":"CVE-123","summary":"not evil"},{"id":"MAL-456"}]}"#;
        let vulns = parse_osv_response(body).unwrap();
        assert_eq!(vulns.len(), 3);
        let malware = filter_malware_vulns(vulns);
        assert_eq!(malware.len(), 2);
        assert_eq!(malware[0].id, "MAL-123");
        assert_eq!(malware[1].id, "MAL-456");
        assert_eq!(malware[1].summary, None);
    }

    #[test]
    fn parse_empty_vulns() {
        let _g = test_lock();
        assert_eq!(parse_osv_response(r#"{}"#).unwrap().len(), 0);
        assert_eq!(parse_osv_response(r#"{"vulns":[]}"#).unwrap().len(), 0);
    }

    #[test]
    fn format_blocked_truncates() {
        let _g = test_lock();
        let vulns = vec![
            OsvVuln { id: "MAL-1".to_string(), summary: Some("a".repeat(200)) },
            OsvVuln { id: "MAL-2".to_string(), summary: None },
            OsvVuln { id: "MAL-3".to_string(), summary: Some("short".to_string()) },
            OsvVuln { id: "MAL-4".to_string(), summary: Some("should be ignored".to_string()) },
        ];
        let msg = format_blocked_message("pkg", "npm", &vulns);
        assert!(msg.contains("MAL-1, MAL-2, MAL-3"));
        assert!(!msg.contains("MAL-4"));
        // First summary truncated to 100
        assert!(msg.contains(&"a".repeat(100)));
        assert!(!msg.contains(&"a".repeat(101)));
        // Second uses id as fallback
        assert!(msg.contains("MAL-2"));
        assert!(msg.starts_with("BLOCKED: Package 'pkg' (npm) has known malware"));
    }

    // -----------------------------------------------------------------------
    // cache — mirrors lines 42-63
    // -----------------------------------------------------------------------
    #[test]
    fn cache_get_put() {
        let _g = test_lock();
        clear_cache();
        let key = (s("npm"), s("lodash"), None);
        assert_eq!(cache_get(&key), None);
        cache_put(key.clone(), None);
        assert_eq!(cache_get(&key), Some(None));
        cache_put(key.clone(), Some(s("blocked")));
        assert_eq!(cache_get(&key), Some(Some(s("blocked"))));
        assert_eq!(cache_len(), 1);
        clear_cache();
        assert_eq!(cache_len(), 0);
    }

    #[test]
    fn cache_eviction() {
        let _g = test_lock();
        clear_cache();
        // Fill to max
        for i in 0..CACHE_MAX_ENTRIES {
            let k = (s("npm"), format!("pkg{i}"), None);
            cache_put(k, None);
        }
        assert_eq!(cache_len(), CACHE_MAX_ENTRIES);
        // One more should evict (clear)
        let k = (s("npm"), s("one-more"), None);
        cache_put(k.clone(), None);
        // After overflow, cache was cleared then one inserted
        assert_eq!(cache_len(), 1);
        assert_eq!(cache_get(&k), Some(None));
        clear_cache();
    }

    // -----------------------------------------------------------------------
    // check_package_for_malware — mirrors lines 66-111
    // -----------------------------------------------------------------------
    #[test]
    fn check_unknown_ecosystem_skips() {
        let _g = test_lock();
        clear_cache();
        let res = check_package_for_malware_with_fetch("node", &[s("lodash")], |_, _| {
            panic!("should not query for unknown ecosystem")
        });
        assert_eq!(res, None);
    }

    #[test]
    fn check_unparseable_package_skips() {
        let _g = test_lock();
        clear_cache();
        let res = check_package_for_malware_with_fetch("npx", &[], |_, _| {
            panic!("should not query when no package")
        });
        assert_eq!(res, None);
    }

    #[test]
    fn check_clean_caches_none() {
        let _g = test_lock();
        clear_cache();
        let args = vec![s("clean-pkg")];
        // First call: fetch returns no vulns
        let res = check_package_for_malware_with_fetch("npx", &args, |_, _| {
            Ok(r#"{"vulns":[]}"#.to_string())
        });
        assert_eq!(res, None);
        // Second call: should hit cache and not call fetch
        let res2 = check_package_for_malware_with_fetch("npx", &args, |_, _| {
            panic!("should be cached")
        });
        assert_eq!(res2, None);
        clear_cache();
    }

    #[test]
    fn check_malware_blocked_and_cached() {
        let _g = test_lock();
        clear_cache();
        let args = vec![s("evil-pkg")];
        let res = check_package_for_malware_with_fetch("npx", &args, |_, _| {
            Ok(r#"{"vulns":[{"id":"MAL-123","summary":"this is malware"},{"id":"MAL-456","summary":"also malware"},{"id":"CVE-999","summary":"ignore me"},{"id":"MAL-789","summary":"fourth"}]}"#.to_string())
        });
        assert!(res.is_some());
        let msg = res.unwrap();
        assert!(msg.contains("evil-pkg"));
        assert!(msg.contains("npm"));
        assert!(msg.contains("MAL-123"));
        assert!(msg.contains("MAL-456"));
        assert!(msg.contains("MAL-789"));
        assert!(!msg.contains("CVE-999"));
        // Cached
        let res2 = check_package_for_malware_with_fetch("npx", &args, |_, _| {
            panic!("should be cached")
        });
        assert_eq!(res2, Some(msg));
        clear_cache();
    }

    #[test]
    fn check_fail_open_not_cached() {
        let _g = test_lock();
        clear_cache();
        let args = vec![s("pkg")];
        let res = check_package_for_malware_with_fetch("npx", &args, |_, _| Err("network down".to_string()));
        assert_eq!(res, None);
        // Should not be cached — next call retries
        let mut called = false;
        let res2 = check_package_for_malware_with_fetch("npx", &args, |_, _| {
            called = true;
            Ok(r#"{"vulns":[]}"#.to_string())
        });
        assert!(called);
        assert_eq!(res2, None);
        clear_cache();
    }

    #[test]
    fn check_filters_non_malware() {
        let _g = test_lock();
        clear_cache();
        let args = vec![s("pkg")];
        let res = check_package_for_malware_with_fetch("npx", &args, |_, _| {
            Ok(r#"{"vulns":[{"id":"CVE-123","summary":"not malware"}]}"#.to_string())
        });
        assert_eq!(res, None);
        clear_cache();
    }

    #[test]
    fn check_pypi_with_version() {
        let _g = test_lock();
        clear_cache();
        let args = vec![s("requests==2.28.0")];
        let res = check_package_for_malware_with_fetch("uvx", &args, |_, payload| {
            // Verify payload contains version
            assert!(payload.contains("2.28.0"));
            assert!(payload.contains("requests"));
            assert!(payload.contains("PyPI"));
            Ok(r#"{"vulns":[]}"#.to_string())
        });
        assert_eq!(res, None);
        clear_cache();
    }

    #[test]
    fn check_npx_package_flag() {
        let _g = test_lock();
        clear_cache();
        let args = vec![s("--package"), s("real-pkg"), s("fake-bin")];
        let res = check_package_for_malware_with_fetch("npx", &args, |_, payload| {
            assert!(payload.contains("real-pkg"));
            assert!(!payload.contains("fake-bin"));
            Ok(r#"{"vulns":[]}"#.to_string())
        });
        assert_eq!(res, None);
        clear_cache();
    }

    #[test]
    fn osv_endpoint_and_ttl_env() {
        let _g = test_lock();
        // Default
        std::env::remove_var(OSV_ENDPOINT_ENV);
        assert_eq!(osv_endpoint(), OSV_ENDPOINT_DEFAULT);
        std::env::set_var(OSV_ENDPOINT_ENV, "https://example.test/v1/query");
        assert_eq!(osv_endpoint(), "https://example.test/v1/query");
        std::env::remove_var(OSV_ENDPOINT_ENV);

        std::env::remove_var(CACHE_TTL_ENV);
        assert_eq!(osv_cache_ttl(), CACHE_TTL_S_DEFAULT);
        std::env::set_var(CACHE_TTL_ENV, "100");
        assert_eq!(osv_cache_ttl(), 100.0);
        std::env::set_var(CACHE_TTL_ENV, "invalid");
        assert_eq!(osv_cache_ttl(), CACHE_TTL_S_DEFAULT);
        std::env::remove_var(CACHE_TTL_ENV);
    }

    #[test]
    fn is_malware_id_helper() {
        assert!(is_malware_id("MAL-123"));
        assert!(is_malware_id("MAL-"));
        assert!(!is_malware_id("CVE-123"));
        assert!(!is_malware_id("mal-123"));
        assert!(!is_malware_id(""));
    }

    #[test]
    fn cache_ttl_duration_nan_infinite() {
        let _g = test_lock();
        std::env::set_var(CACHE_TTL_ENV, "NaN");
        let d = osv_cache_ttl_duration();
        assert_eq!(d, Duration::from_secs_f64(CACHE_TTL_S_DEFAULT));
        std::env::set_var(CACHE_TTL_ENV, "inf");
        let d2 = osv_cache_ttl_duration();
        assert_eq!(d2, Duration::from_secs_f64(CACHE_TTL_S_DEFAULT));
        std::env::set_var(CACHE_TTL_ENV, "-10");
        let d3 = osv_cache_ttl_duration();
        assert_eq!(d3, Duration::from_secs_f64(CACHE_TTL_S_DEFAULT));
        std::env::remove_var(CACHE_TTL_ENV);
    }
}
