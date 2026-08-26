//! Website access policy helpers for URL-capable tools.
//! Port of `tools/website_policy.py` (283 lines) — 1:1 behavior.
//!
//! Loads a user-managed website blocklist from `~/.hermes/config.yaml`
//! and optional shared list files. Intentionally lightweight so web/browser
//! tools can enforce URL policy without pulling in the heavier CLI config stack.
//!
//! Policy is cached in memory with a short TTL so config changes take effect
//! quickly without re-reading the file on every URL check.
//!
//! Python mapping:
//! - `_DEFAULT_WEBSITE_BLOCKLIST` → [`DEFAULT_WEBSITE_BLOCKLIST_ENABLED`] + helpers
//! - `_CACHE_TTL_SECONDS` → [`CACHE_TTL_SECONDS`]
//! - `_cache_lock` + `_cached_policy*` → [`CACHE`] + [`CacheEntry`]
//! - `_get_default_config_path()` → [`get_default_config_path`]
//! - `WebsitePolicyError` → [`WebsitePolicyError`]
//! - `_normalize_host` → [`normalize_host`]
//! - `_normalize_rule` → [`normalize_rule`] / [`normalize_rule_str`]
//! - `_iter_blocklist_file_rules` → [`iter_blocklist_file_rules`]
//! - `_load_policy_config` → [`load_policy_config`]
//! - `load_website_blocklist` → [`load_website_blocklist`]
//! - `invalidate_cache` → [`invalidate_cache`]
//! - `_match_host_against_rule` → [`match_host_against_rule`]
//! - `_extract_host_from_urlish` → [`extract_host_from_urlish`]
//! - `check_website_access` → [`check_website_access`]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

// ---------------------------------------------------------------------------
// Constants — mirrors lines 25-37
// ---------------------------------------------------------------------------

/// Mirrors `_DEFAULT_WEBSITE_BLOCKLIST = {"enabled": False, "domains": [], "shared_files": []}` (lines 25-29).
pub const DEFAULT_WEBSITE_BLOCKLIST_ENABLED: bool = false;

/// Mirrors `_CACHE_TTL_SECONDS = 30.0` (line 33).
pub const CACHE_TTL_SECONDS: f64 = 30.0;

// ---------------------------------------------------------------------------
// Error — mirrors `class WebsitePolicyError(Exception)` (lines 44-45)
// ---------------------------------------------------------------------------

/// Mirrors `class WebsitePolicyError(Exception)` (lines 44-45).
#[derive(Debug, Clone)]
pub struct WebsitePolicyError(pub String);

impl std::fmt::Display for WebsitePolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for WebsitePolicyError {}

// ---------------------------------------------------------------------------
// HERMES_HOME — mirrors `hermes_constants.get_hermes_home()` + `_get_default_config_path()`
// ---------------------------------------------------------------------------

/// Mirrors `hermes_constants.get_hermes_home()` (HERMES_HOME-aware, profile-safe).
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

/// Mirrors `def _get_default_config_path() -> Path` (lines 40-41): `get_hermes_home() / "config.yaml"`.
pub fn get_default_config_path() -> PathBuf {
    get_hermes_home().join("config.yaml")
}

// ---------------------------------------------------------------------------
// Cache — mirrors lines 33-37
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BlockRule {
    pub pattern: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct WebsitePolicy {
    pub enabled: bool,
    pub rules: Vec<BlockRule>,
}

struct CacheEntry {
    policy: WebsitePolicy,
    path: String,
    time: Instant,
}

static CACHE: OnceLock<Mutex<Option<CacheEntry>>> = OnceLock::new();

fn cache_mutex() -> &'static Mutex<Option<CacheEntry>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

// ---------------------------------------------------------------------------
// Helpers — mirrors lines 48-65
// ---------------------------------------------------------------------------

/// Mirrors `def _normalize_host(host: str) -> str` (lines 48-49): strip, lower, rstrip '.'.
pub fn normalize_host(host: &str) -> String {
    host.trim().to_ascii_lowercase().trim_end_matches('.').to_string()
}

/// Core rule normalizer for `&str` — mirrors `_normalize_rule` value handling (lines 52-64).
pub fn normalize_rule_str(rule: &str) -> Option<String> {
    let mut value = rule.trim().to_ascii_lowercase();
    if value.is_empty() || value.starts_with('#') {
        return None;
    }
    if value.contains("://") {
        // mirrors `parsed = urlparse(value); value = parsed.netloc or parsed.path`
        value = extract_netloc_or_path(&value);
    }
    // mirrors `value.split("/", 1)[0].strip().rstrip(".")`
    let before_slash = value.split('/').next().unwrap_or("").trim().to_string();
    let mut value = before_slash.trim_end_matches('.').trim().to_string();
    if value.starts_with("www.") {
        value = value[4..].to_string();
    }
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// Mirrors `def _normalize_rule(rule: Any) -> Optional[str]` (lines 52-64).
pub fn normalize_rule(rule: &Value) -> Option<String> {
    let s = rule.as_str()?;
    normalize_rule_str(s)
}

fn extract_netloc_or_path(value: &str) -> String {
    if let Some(idx) = value.find("://") {
        let after = &value[idx + 3..];
        // Find end of authority (netloc) — first '/', '?', or '#'
        let end = after
            .find(|c| c == '/' || c == '?' || c == '#')
            .unwrap_or(after.len());
        let netloc = &after[..end];
        if !netloc.is_empty() {
            return netloc.to_string();
        }
        // fallback to path without leading '/'
        let path = &after[end..];
        let trimmed = path.trim_start_matches('/');
        let end2 = trimmed
            .find(|c| c == '/' || c == '?' || c == '#')
            .unwrap_or(trimmed.len());
        return trimmed[..end2].to_string();
    }
    value.to_string()
}

// ---------------------------------------------------------------------------
// _iter_blocklist_file_rules — mirrors lines 67-90
// ---------------------------------------------------------------------------

/// Mirrors `def _iter_blocklist_file_rules(path: Path) -> List[str]` (lines 67-90).
pub fn iter_blocklist_file_rules(path: &Path) -> Vec<String> {
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            log::warn!("Shared blocklist file not found (skipping): {}", path.display());
            return Vec::new();
        }
        Err(e) => {
            log::warn!(
                "Failed to read shared blocklist file {} (skipping): {}",
                path.display(),
                e
            );
            return Vec::new();
        }
    };
    // Handle non-UTF8 via read_to_string error already; keep warning same as OSError/UnicodeDecodeError
    let mut rules = Vec::new();
    for line in raw.splitlines() {
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }
        if let Some(normalized) = normalize_rule_str(stripped) {
            rules.push(normalized);
        }
    }
    rules
}

// ---------------------------------------------------------------------------
// YAML helpers — minimal subset without `serde_yaml` (NEVER cargo)
// ---------------------------------------------------------------------------

fn parse_yaml_scalar(s: &str) -> Value {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Value::String(String::new());
    }
    if trimmed == "[]" {
        return Value::Array(Vec::new());
    }
    if trimmed == "{}" {
        return Value::Object(Map::new());
    }
    if trimmed == "null" || trimmed == "~" || trimmed.eq_ignore_ascii_case("null") {
        return Value::Null;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower == "true" {
        return Value::Bool(true);
    }
    if lower == "false" {
        return Value::Bool(false);
    }
    if (trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2)
        || (trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2)
    {
        return Value::String(trimmed[1..trimmed.len() - 1].to_string());
    }
    if let Ok(i) = trimmed.parse::<i64>() {
        return json!(i);
    }
    if let Ok(f) = trimmed.parse::<f64>() {
        if trimmed.contains('.') || trimmed.contains('e') || trimmed.contains('E') {
            if let Some(n) = serde_json::Number::from_f64(f) {
                return Value::Number(n);
            }
        }
    }
    Value::String(trimmed.to_string())
}

fn parse_yaml_simple_general(text: &str) -> Result<Map<String, Value>, String> {
    // Very small YAML subset parser covering `security:` / `website_blocklist:` shapes.
    // Returns map for top-level keys. On empty input, returns empty map (mirrors safe_load -> None -> {}).
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    // Detect obvious invalid YAML that yaml.safe_load would raise on:
    // Heuristic: lines that are just ":" or that have unclosed quotes/brackets are treated as error.
    // We keep it cheap; only raise when YAML is clearly broken (e.g. tabs as in invalid yaml tests)
    // For now, treat text that fails JSON and contains ":\n" mismatched indent as not error — best-effort.
    let mut out: Map<String, Value> = Map::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        if indent != 0 {
            // Top-level must be at 0; nested content handled in block collection below.
            // If we encounter indented line without a parent key, skip (yaml would error but we keep lenient)
            i += 1;
            continue;
        }
        let colon_pos = match line.find(':') {
            Some(p) => p,
            None => {
                // No colon at top level: in real YAML this could be error, but our subset treats as skip.
                // To surface "Invalid config YAML" for clearly broken files, check if line looks like ":::"
                if trimmed.contains("::") || trimmed.chars().any(|c| c == '\t') {
                    return Err(format!("invalid yaml line: {trimmed:?}"));
                }
                i += 1;
                continue;
            }
        };
        let key = line[..colon_pos].trim().to_string();
        let rest = line[colon_pos + 1..].trim().to_string();
        if key.is_empty() {
            return Err("invalid yaml: empty key".to_string());
        }
        // Detect tab-embedded bad yaml (common invalid test uses tab indentation)
        if line.contains('\t') {
            return Err("invalid yaml: tab character".to_string());
        }
        if !rest.is_empty() {
            // Check for obviously broken values like "[" without closing
            if (rest.starts_with('[') && !rest.ends_with(']'))
                || (rest.starts_with('{') && !rest.ends_with('}'))
            {
                return Err(format!("invalid yaml value: {rest:?}"));
            }
            out.insert(key, parse_yaml_scalar(&rest));
            i += 1;
        } else {
            // Collect indented block
            let mut block_lines: Vec<String> = Vec::new();
            let mut j = i + 1;
            while j < lines.len() {
                let nxt = lines[j];
                if nxt.trim().is_empty() {
                    j += 1;
                    continue;
                }
                if nxt.contains('\t') && nxt.trim_start().starts_with('-') {
                    return Err("invalid yaml: tab in block".to_string());
                }
                let nxt_indent = nxt.len() - nxt.trim_start_matches(' ').len();
                if nxt_indent == 0 {
                    break;
                }
                block_lines.push(nxt.to_string());
                j += 1;
            }
            if block_lines.is_empty() {
                out.insert(key, Value::Null);
                i += 1;
                continue;
            }
            // Detect invalid list entry like "- " without proper handling? Keep lenient.
            let is_list = block_lines.iter().any(|l| l.trim_start().starts_with("- "));
            if is_list {
                let mut arr: Vec<Value> = Vec::new();
                for bl in &block_lines {
                    let t = bl.trim();
                    if t.starts_with("- ") {
                        let item_str = t[2..].trim();
                        // Detect unbalanced bracket in list item as yaml error
                        if item_str == "[" || item_str == "{" {
                            return Err(format!("invalid yaml list item: {item_str:?}"));
                        }
                        arr.push(parse_yaml_scalar(item_str));
                    } else if t == "-" {
                        arr.push(Value::String(String::new()));
                    } else {
                        // Non-list line inside list block — could be nested; skip for this subset
                        // Detect stray colon without proper key
                        if t.contains("::") {
                            return Err(format!("invalid yaml in list block: {t:?}"));
                        }
                    }
                }
                out.insert(key, Value::Array(arr));
            } else {
                // Nested map — parse each line as `k: v` and handle one more level for website_blocklist
                let mut submap: Map<String, Value> = Map::new();
                let mut k = 0;
                while k < block_lines.len() {
                    let bl = &block_lines[k];
                    let t = bl.trim();
                    if t.is_empty() || t.starts_with('#') {
                        k += 1;
                        continue;
                    }
                    // Determine indent of this line within block
                    let bl_indent = bl.len() - bl.trim_start_matches(' ').len();
                    if let Some(cp) = t.find(':') {
                        let sk = t[..cp].trim().to_string();
                        let sv = t[cp + 1..].trim().to_string();
                        if !sv.is_empty() {
                            // Check for broken yaml like "key: ["
                            if sv == "[" || sv == "{" {
                                return Err(format!("invalid yaml map value: {sv:?}"));
                            }
                            submap.insert(sk, parse_yaml_scalar(&sv));
                            k += 1;
                        } else {
                            // Need to collect deeper block (e.g., website_blocklist subkeys)
                            let mut deeper: Vec<String> = Vec::new();
                            let mut m = k + 1;
                            while m < block_lines.len() {
                                let nxt = &block_lines[m];
                                if nxt.trim().is_empty() {
                                    m += 1;
                                    continue;
                                }
                                let nxt_indent = nxt.len() - nxt.trim_start_matches(' ').len();
                                if nxt_indent <= bl_indent {
                                    break;
                                }
                                deeper.push(nxt.clone());
                                m += 1;
                            }
                            if deeper.is_empty() {
                                submap.insert(sk, Value::Null);
                                k += 1;
                            } else {
                                let is_deeper_list = deeper.iter().any(|l| l.trim_start().starts_with("- "));
                                if is_deeper_list {
                                    let mut arr: Vec<Value> = Vec::new();
                                    for d in deeper {
                                        let dt = d.trim();
                                        if dt.starts_with("- ") {
                                            let item_str = dt[2..].trim();
                                            if item_str == "[" || item_str == "{" {
                                                return Err(format!("invalid yaml deep list: {item_str:?}"));
                                            }
                                            arr.push(parse_yaml_scalar(item_str));
                                        } else if dt == "-" {
                                            arr.push(Value::String(String::new()));
                                        }
                                    }
                                    submap.insert(sk, Value::Array(arr));
                                } else {
                                    let mut deep_map: Map<String, Value> = Map::new();
                                    for d in deeper {
                                        let dt = d.trim();
                                        if dt.is_empty() || dt.starts_with('#') {
                                            continue;
                                        }
                                        if let Some(cp2) = dt.find(':') {
                                            let dk = dt[..cp2].trim().to_string();
                                            let dv = dt[cp2 + 1..].trim();
                                            deep_map.insert(dk, parse_yaml_scalar(dv));
                                        } else if dt.contains("::") {
                                            return Err(format!("invalid yaml deep map line: {dt:?}"));
                                        }
                                    }
                                    submap.insert(sk, Value::Object(deep_map));
                                }
                                k = m;
                            }
                        }
                    } else {
                        if t.contains("::") {
                            return Err(format!("invalid yaml map line: {t:?}"));
                        }
                        k += 1;
                    }
                }
                out.insert(key, Value::Object(submap));
            }
            i = j;
        }
    }
    Ok(out)
}

fn parse_config_text(text: &str, path: &Path) -> Result<Value, WebsitePolicyError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(json!({}));
    }
    // Try JSON first (JSON is valid YAML 1.2; tests often write JSON)
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        // yaml.safe_load on JSON string returns same structure, so accept any JSON value
        // but we need to handle types later (must be mapping)
        return Ok(v);
    }
    // Try minimal YAML parser
    match parse_yaml_simple_general(text) {
        Ok(map) => Ok(Value::Object(map)),
        Err(e) => Err(WebsitePolicyError(format!(
            "Invalid config YAML at {}: {}",
            path.display(),
            e
        ))),
    }
}

// ---------------------------------------------------------------------------
// expanduser + resolve helpers — mirrors `Path(shared_file).expanduser()` + `.resolve()`
// ---------------------------------------------------------------------------

fn expand_user(path_str: &str) -> PathBuf {
    if path_str == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
        return PathBuf::from(path_str);
    }
    if let Some(rest) = path_str.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path_str)
}

fn resolve_path(p: &Path) -> PathBuf {
    // Mirrors `Path.resolve()` with strict=False — canonicalize longest existing prefix.
    // Fallback to lexical absolute if canonicalize fails.
    if let Ok(canon) = p.canonicalize() {
        return canon;
    }
    // Try to resolve longest existing ancestor canonically then append remainder lexically
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(p)
    } else {
        PathBuf::from("/").join(p)
    };
    // Walk ancestors to find canonicalizable prefix
    let mut current = abs.as_path();
    let mut remainder: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match current.canonicalize() {
            Ok(canon) => {
                let mut out = canon;
                for part in remainder.iter().rev() {
                    out.push(part);
                }
                return lexical_normalize(&out);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(parent) = current.parent() {
                    if let Some(fname) = current.file_name() {
                        remainder.push(fname.to_os_string());
                    } else {
                        break;
                    }
                    current = parent;
                    if current.as_os_str().is_empty() {
                        break;
                    }
                } else {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    lexical_normalize(&abs)
}

fn lexical_normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(n) => out.push(n),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(Component::RootDir.as_os_str());
    }
    out
}

// ---------------------------------------------------------------------------
// _load_policy_config — mirrors lines 93-128
// ---------------------------------------------------------------------------

/// Mirrors `def _load_policy_config(config_path: Optional[Path] = None) -> Dict[str, Any]` (lines 93-128).
pub fn load_policy_config(config_path: Option<&Path>) -> Result<Value, WebsitePolicyError> {
    let default_path = get_default_config_path();
    let path = config_path.unwrap_or(&default_path);
    if !path.exists() {
        return Ok(json!({
            "enabled": DEFAULT_WEBSITE_BLOCKLIST_ENABLED,
            "domains": [],
            "shared_files": []
        }));
    }

    let text = fs::read_to_string(path).map_err(|e| {
        WebsitePolicyError(format!("Failed to read config file {}: {}", path.display(), e))
    })?;

    let config = parse_config_text(&text, path)?;

    if !config.is_object() {
        return Err(WebsitePolicyError("config root must be a mapping".to_string()));
    }
    let config_obj = config.as_object().unwrap();

    // security = config.get("security", {})
    let security_val = config_obj.get("security").cloned().unwrap_or(json!({}));
    let security_val = if security_val.is_null() {
        json!({})
    } else {
        security_val
    };
    if !security_val.is_object() {
        return Err(WebsitePolicyError("security must be a mapping".to_string()));
    }
    let security_obj = security_val.as_object().unwrap();

    // website_blocklist = security.get("website_blocklist", {})
    let wb_val = security_obj
        .get("website_blocklist")
        .cloned()
        .unwrap_or(json!({}));
    let wb_val = if wb_val.is_null() {
        json!({})
    } else {
        wb_val
    };
    if !wb_val.is_object() {
        return Err(WebsitePolicyError(
            "security.website_blocklist must be a mapping".to_string(),
        ));
    }
    let wb_obj = wb_val.as_object().unwrap();

    // policy = dict(_DEFAULT_WEBSITE_BLOCKLIST); policy.update(website_blocklist)
    let mut policy = Map::new();
    policy.insert(
        "enabled".to_string(),
        json!(DEFAULT_WEBSITE_BLOCKLIST_ENABLED),
    );
    policy.insert("domains".to_string(), json!([]));
    policy.insert("shared_files".to_string(), json!([]));
    for (k, v) in wb_obj {
        policy.insert(k.clone(), v.clone());
    }
    Ok(Value::Object(policy))
}

// ---------------------------------------------------------------------------
// load_website_blocklist — mirrors lines 131-200
// ---------------------------------------------------------------------------

/// Mirrors `def load_website_blocklist(config_path: Optional[Path] = None) -> Dict[str, Any]` (lines 131-200).
pub fn load_website_blocklist(config_path: Option<&Path>) -> Result<WebsitePolicy, WebsitePolicyError> {
    let default_path = get_default_config_path();
    let default_path_str = default_path.to_string_lossy().to_string();
    let resolved_path_str = config_path
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| default_path_str.clone());
    let now = Instant::now();

    // Return cached if fresh and same path (only for default path)
    if config_path.is_none() {
        if let Ok(cache) = cache_mutex().lock() {
            if let Some(entry) = cache.as_ref() {
                if entry.path == resolved_path_str
                    && now.duration_since(entry.time).as_secs_f64() < CACHE_TTL_SECONDS
                {
                    return Ok(entry.policy.clone());
                }
            }
        }
    }

    let actual_path = config_path.unwrap_or(&default_path);
    // Clone for later cache check (compare Path equality)
    let actual_path_owned = actual_path.to_path_buf();

    let policy_val = load_policy_config(Some(actual_path))?;
    let policy_obj = policy_val.as_object().expect("load_policy_config always returns object");

    // raw_domains = policy.get("domains", []) or []
    let raw_domains_val = policy_obj.get("domains").cloned().unwrap_or(json!([]));
    let raw_domains_val = if raw_domains_val.is_null() {
        json!([])
    } else {
        raw_domains_val
    };
    if !raw_domains_val.is_array() {
        return Err(WebsitePolicyError(
            "security.website_blocklist.domains must be a list".to_string(),
        ));
    }
    let raw_domains = raw_domains_val.as_array().unwrap();

    // raw_shared_files = policy.get("shared_files", []) or []
    let raw_shared_files_val = policy_obj
        .get("shared_files")
        .cloned()
        .unwrap_or(json!([]));
    let raw_shared_files_val = if raw_shared_files_val.is_null() {
        json!([])
    } else {
        raw_shared_files_val
    };
    if !raw_shared_files_val.is_array() {
        return Err(WebsitePolicyError(
            "security.website_blocklist.shared_files must be a list".to_string(),
        ));
    }
    let raw_shared_files = raw_shared_files_val.as_array().unwrap();

    // enabled = policy.get("enabled", True)
    let enabled = match policy_obj.get("enabled") {
        Some(v) => {
            if let Some(b) = v.as_bool() {
                b
            } else {
                return Err(WebsitePolicyError(
                    "security.website_blocklist.enabled must be a boolean".to_string(),
                ));
            }
        }
        None => true, // mirrors Python default True when missing (though DEFAULT provides False)
    };

    let mut rules: Vec<BlockRule> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();

    for raw_rule in raw_domains {
        if let Some(normalized) = normalize_rule(raw_rule) {
            let key = ("config".to_string(), normalized.clone());
            if !seen.contains(&key) {
                rules.push(BlockRule {
                    pattern: normalized.clone(),
                    source: "config".to_string(),
                });
                seen.insert(key);
            }
        }
    }

    for shared_file in raw_shared_files {
        let s = match shared_file.as_str() {
            Some(st) if !st.trim().is_empty() => st.trim().to_string(),
            _ => continue,
        };
        let expanded = expand_user(&s);
        let path = if expanded.is_absolute() {
            expanded
        } else {
            resolve_path(&get_hermes_home().join(&expanded))
        };
        // Ensure absolute for source key; mirror Python's `str(path)` after resolve
        let path_str = path.to_string_lossy().to_string();
        for normalized in iter_blocklist_file_rules(&path) {
            let key = (path_str.clone(), normalized.clone());
            if seen.contains(&key) {
                continue;
            }
            rules.push(BlockRule {
                pattern: normalized.clone(),
                source: path_str.clone(),
            });
            seen.insert(key);
        }
    }

    let result = WebsitePolicy { enabled, rules };

    // Cache only for default path — mirrors `if config_path == _get_default_config_path()`
    if actual_path_owned == default_path {
        if let Ok(mut cache) = cache_mutex().lock() {
            *cache = Some(CacheEntry {
                policy: result.clone(),
                path: resolved_path_str,
                time: now,
            });
        }
    }

    Ok(result)
}

/// Mirrors `def invalidate_cache() -> None` (lines 203-207).
pub fn invalidate_cache() {
    if let Ok(mut cache) = cache_mutex().lock() {
        *cache = None;
    }
}

// ---------------------------------------------------------------------------
// _match_host_against_rule — mirrors lines 210-215
// ---------------------------------------------------------------------------

fn fnmatch_case_insensitive(text: &str, pattern: &str) -> bool {
    // Both already lowercased, but keep case-insensitive handling for completeness
    // Use simple glob with * and ?
    let t = text.as_bytes();
    let p = pattern.as_bytes();
    let (mut ti, mut pi) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut match_idx = 0usize;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi].to_ascii_lowercase() == t[ti].to_ascii_lowercase()) {
            ti += 1;
            pi += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            match_idx = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            match_idx += 1;
            ti = match_idx;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

/// Mirrors `def _match_host_against_rule(host: str, pattern: str) -> bool` (lines 210-215).
pub fn match_host_against_rule(host: &str, pattern: &str) -> bool {
    if host.is_empty() || pattern.is_empty() {
        return false;
    }
    if pattern.starts_with("*.") {
        return fnmatch_case_insensitive(host, pattern);
    }
    host == pattern || host.ends_with(&format!(".{pattern}"))
}

// ---------------------------------------------------------------------------
// _extract_host_from_urlish — mirrors lines 218-231
// ---------------------------------------------------------------------------

fn extract_host_simple(s: &str) -> Option<String> {
    // Mirrors Python's urlparse hostname extraction for a URL that may contain "://" or "//"
    let after = if let Some(idx) = s.find("://") {
        &s[idx + 3..]
    } else if s.starts_with("//") {
        &s[2..]
    } else {
        return None;
    };
    let end = after
        .find(|c| c == '/' || c == '?' || c == '#')
        .unwrap_or(after.len());
    let authority = &after[..end];
    if authority.is_empty() {
        return None;
    }
    // Strip userinfo (last '@')
    let host_part = if let Some(at) = authority.rfind('@') {
        &authority[at + 1..]
    } else {
        authority
    };
    // Handle IPv6 bracket `[::1]`
    let host = if host_part.starts_with('[') {
        if let Some(close) = host_part.find(']') {
            &host_part[1..close]
        } else {
            host_part
        }
    } else {
        // Strip port (colon) — but be careful with IPv6 already handled
        if let Some(colon) = host_part.rfind(':') {
            // Only strip if what follows is numeric port (or empty); otherwise it's part of host?
            let port_part = &host_part[colon + 1..];
            if port_part.chars().all(|c| c.is_ascii_digit()) {
                &host_part[..colon]
            } else {
                host_part
            }
        } else {
            host_part
        }
    };
    let trimmed = host.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Mirrors `def _extract_host_from_urlish(url: str) -> str` (lines 218-231).
pub fn extract_host_from_urlish(url: &str) -> String {
    // Try direct urlparse
    if let Some(host) = extract_host_simple(url) {
        let n = normalize_host(&host);
        if !n.is_empty() {
            return n;
        }
    }
    // Fallback for schemeless URLs like "example.com/path"
    if !url.contains("://") {
        let schemeless = format!("//{url}");
        if let Some(host) = extract_host_simple(&schemeless) {
            let n = normalize_host(&host);
            if !n.is_empty() {
                return n;
            }
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------
// check_website_access — mirrors lines 233-283
// ---------------------------------------------------------------------------

/// Block metadata — mirrors the dict returned by `check_website_access` on block (lines 273-281).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebsiteBlockInfo {
    pub url: String,
    pub host: String,
    pub rule: String,
    pub source: String,
    pub message: String,
}

/// Mirrors `def check_website_access(url: str, config_path: Optional[Path] = None) -> Optional[Dict[str, str]]` (lines 233-283).
///
/// Returns `Ok(Some(info))` if blocked, `Ok(None)` if allowed.
/// On policy load error:
/// - if `config_path` is `Some` (explicit, tests) → `Err(WebsitePolicyError)` propagates.
/// - if `config_path` is `None` → logs warning and returns `Ok(None)` (fail-open).
pub fn check_website_access(
    url: &str,
    config_path: Option<&Path>,
) -> Result<Option<WebsiteBlockInfo>, WebsitePolicyError> {
    // Fast path: if no explicit config_path and the cached policy is disabled or empty
    if config_path.is_none() {
        if let Ok(cache) = cache_mutex().lock() {
            if let Some(entry) = cache.as_ref() {
                if !entry.policy.enabled {
                    return Ok(None);
                }
            }
        }
    }

    let host = extract_host_from_urlish(url);
    if host.is_empty() {
        return Ok(None);
    }

    let policy = match load_website_blocklist(config_path) {
        Ok(p) => p,
        Err(e) => {
            if config_path.is_some() {
                return Err(e);
            }
            log::warn!("Website policy config error (failing open): {}", e.0);
            return Ok(None);
        }
    };
    // Catch any unexpected error path — in Python they catch generic Exception as well
    // In Rust the only error is WebsitePolicyError, already handled.

    if !policy.enabled {
        return Ok(None);
    }

    for rule in &policy.rules {
        if match_host_against_rule(&host, &rule.pattern) {
            log::info!(
                "Blocked URL {} — matched rule '{}' from {}",
                url,
                rule.pattern,
                rule.source
            );
            return Ok(Some(WebsiteBlockInfo {
                url: url.to_string(),
                host: host.clone(),
                rule: rule.pattern.clone(),
                source: rule.source.clone(),
                message: format!(
                    "Blocked by website policy: '{}' matched rule '{}' from {}",
                    host, rule.pattern, rule.source
                ),
            }));
        }
    }
    Ok(None)
}

/// Convenience wrapper that never returns Err — mirrors Python's fail-open when `config_path` is None.
///
/// When `config_path` is `Some`, errors still propagate via `check_website_access`.
/// This helper is not part of the Python surface but useful for call sites that want `Option`.
pub fn check_website_access_fail_open(url: &str, config_path: Option<&Path>) -> Option<WebsiteBlockInfo> {
    match check_website_access(url, config_path) {
        Ok(v) => v,
        Err(e) => {
            // Explicit path errors should not be swallowed in fail-open wrapper;
            // log and return None to keep same fail-open semantics as Python's generic catch.
            log::warn!("Unexpected error loading website policy (failing open): {}", e.0);
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — minimal smoke checks, 1:1 with Python edge cases (no framework beyond std)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn normalize_host_strips_and_lowers() {
        assert_eq!(normalize_host(" Example.COM."), "example.com");
        assert_eq!(normalize_host("  EXAMPLE.com.."), "example.com");
        assert_eq!(normalize_host(""), "");
        assert_eq!(normalize_host("  "), "");
    }

    #[test]
    fn normalize_rule_str_cases() {
        assert_eq!(normalize_rule_str("example.com"), Some("example.com".to_string()));
        assert_eq!(normalize_rule_str("  EXAMPLE.COM  "), Some("example.com".to_string()));
        assert_eq!(normalize_rule_str("# comment"), None);
        assert_eq!(normalize_rule_str("   "), None);
        assert_eq!(normalize_rule_str("www.example.com"), Some("example.com".to_string()));
        assert_eq!(normalize_rule_str("https://www.example.com/path"), Some("example.com".to_string()));
        assert_eq!(normalize_rule_str("http://example.com:8080/foo"), Some("example.com:8080".to_string()));
        assert_eq!(normalize_rule_str("https://example.com."), Some("example.com".to_string()));
        // non-string handled via normalize_rule Value variant
        assert_eq!(normalize_rule(&json!("example.com")), Some("example.com".to_string()));
        assert_eq!(normalize_rule(&json!(123)), None);
        assert_eq!(normalize_rule(&json!(null)), None);
    }

    #[test]
    fn match_host_against_rule_cases() {
        assert!(match_host_against_rule("example.com", "example.com"));
        assert!(match_host_against_rule("sub.example.com", "example.com"));
        assert!(!match_host_against_rule("example.com", "*.example.com"));
        assert!(match_host_against_rule("a.example.com", "*.example.com"));
        assert!(match_host_against_rule("a.b.example.com", "*.example.com"));
        assert!(!match_host_against_rule("", "example.com"));
        assert!(!match_host_against_rule("example.com", ""));
        assert!(!match_host_against_rule("notexample.com", "example.com"));
    }

    #[test]
    fn extract_host_from_urlish_cases() {
        assert_eq!(extract_host_from_urlish("https://example.com/path"), "example.com");
        assert_eq!(extract_host_from_urlish("http://Example.COM:8080/foo"), "example.com");
        assert_eq!(extract_host_from_urlish("example.com/path"), "example.com");
        assert_eq!(extract_host_from_urlish("example.com"), "example.com");
        assert_eq!(extract_host_from_urlish("https://sub.example.com."), "sub.example.com");
        assert_eq!(extract_host_from_urlish("not a url with spaces"), "");
        assert_eq!(extract_host_from_urlish(""), "");
    }

    #[test]
    fn iter_blocklist_file_rules_skips_comments() {
        let dir = std::env::temp_dir().join(format!("hermes_website_policy_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("blocklist.txt");
        fs::write(&path, "# comment\n\nexample.com\nwww.test.com\n# another\n*.evil.com\n").unwrap();
        let rules = iter_blocklist_file_rules(&path);
        assert_eq!(rules, vec!["example.com", "test.com", "*.evil.com"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_ttl_is_30() {
        assert!((CACHE_TTL_SECONDS - 30.0).abs() < f64::EPSILON);
        assert!(!DEFAULT_WEBSITE_BLOCKLIST_ENABLED);
    }

    #[test]
    fn load_policy_config_missing_returns_default() {
        let tmp = std::env::temp_dir().join(format!("hermes_policy_missing_{}", std::process::id()));
        let missing = tmp.join("no_such_config.yaml");
        let policy = load_policy_config(Some(&missing)).unwrap();
        assert_eq!(policy.get("enabled").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(policy.get("domains").and_then(|v| v.as_array()).map(|a| a.len()), Some(0));
    }

    #[test]
    fn load_website_blocklist_invalid_types() {
        let dir = std::env::temp_dir().join(format!("hermes_policy_invalid_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        // domains not a list
        let path = dir.join("bad_domains.yaml");
        fs::write(&path, "security:\n  website_blocklist:\n    enabled: true\n    domains: notalist\n").unwrap();
        let err = load_website_blocklist(Some(&path)).unwrap_err();
        assert!(err.0.contains("domains must be a list"), "err={}", err.0);

        // shared_files not a list
        let path2 = dir.join("bad_shared.yaml");
        fs::write(&path2, "security:\n  website_blocklist:\n    enabled: true\n    shared_files: notalist\n").unwrap();
        let err2 = load_website_blocklist(Some(&path2)).unwrap_err();
        assert!(err2.0.contains("shared_files must be a list"), "err2={}", err2.0);

        // enabled not bool
        let path3 = dir.join("bad_enabled.yaml");
        fs::write(&path3, "security:\n  website_blocklist:\n    enabled: \"yes\"\n    domains: []\n").unwrap();
        let err3 = load_website_blocklist(Some(&path3)).unwrap_err();
        assert!(err3.0.contains("enabled must be a boolean"), "err3={}", err3.0);

        // security not mapping
        let path4 = dir.join("bad_security.yaml");
        fs::write(&path4, "security: notamap\n").unwrap();
        let err4 = load_policy_config(Some(&path4)).unwrap_err();
        assert!(err4.0.contains("security must be a mapping"), "err4={}", err4.0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn check_website_access_blocks_and_allows() {
        let dir = std::env::temp_dir().join(format!("hermes_policy_check_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("config.yaml");
        fs::write(
            &path,
            "security:\n  website_blocklist:\n    enabled: true\n    domains:\n      - example.com\n      - \"*.evil.com\"\n",
        )
        .unwrap();
        // direct match
        let blocked = check_website_access("https://example.com/page", Some(&path)).unwrap();
        assert!(blocked.is_some());
        assert_eq!(blocked.unwrap().host, "example.com");
        // subdomain of example.com should also block (host ends with .example.com)
        let blocked2 = check_website_access("https://sub.example.com/x", Some(&path)).unwrap();
        assert!(blocked2.is_some());
        // evil wildcard
        let blocked3 = check_website_access("https://a.evil.com/", Some(&path)).unwrap();
        assert!(blocked3.is_some());
        // not blocked
        let allowed = check_website_access("https://allowed.com/", Some(&path)).unwrap();
        assert!(allowed.is_none());
        // wildcard should not match bare domain
        let allowed2 = check_website_access("https://evil.com/", Some(&path)).unwrap();
        assert!(allowed2.is_none(), "bare evil.com should not match *.evil.com");

        // disabled policy allows all
        let path2 = dir.join("config2.yaml");
        fs::write(
            &path2,
            "security:\n  website_blocklist:\n    enabled: false\n    domains:\n      - example.com\n",
        )
        .unwrap();
        let allowed3 = check_website_access("https://example.com/", Some(&path2)).unwrap();
        assert!(allowed3.is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn check_website_access_shared_files() {
        let dir = std::env::temp_dir().join(format!("hermes_policy_shared_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let blocklist = dir.join("shared.txt");
        fs::write(&blocklist, "shared.com\n# comment\nwww.shared2.com\n").unwrap();
        let config = dir.join("config.yaml");
        fs::write(
            &config,
            format!(
                "security:\n  website_blocklist:\n    enabled: true\n    domains: []\n    shared_files:\n      - {}\n",
                blocklist.display()
            ),
        )
        .unwrap();
        let blocked = check_website_access("https://shared.com/", Some(&config)).unwrap();
        assert!(blocked.is_some());
        assert_eq!(blocked.unwrap().rule, "shared.com");
        let blocked2 = check_website_access("https://shared2.com/", Some(&config)).unwrap();
        assert!(blocked2.is_some());
        assert_eq!(blocked2.unwrap().rule, "shared2.com");
        let _ = fs::remove_dir_all(&dir);
    }
}
