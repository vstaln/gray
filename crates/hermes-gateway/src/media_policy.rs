//! Shared config→env bridge for media-delivery policy.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/media_policy.py` (88 LOC).
//!
//! Python source docstring (preserved):
//! ```text
//! Shared config→env bridge for media-delivery policy.
//!
//! ``validate_media_delivery_path`` (gateway/platforms/base.py) reads its policy
//! from environment variables:
//!
//!   - ``HERMES_MEDIA_DELIVERY_STRICT``    <- gateway.strict
//!   - ``HERMES_MEDIA_ALLOW_DIRS``         <- gateway.media_delivery_allow_dirs
//!   - ``HERMES_MEDIA_TRUST_RECENT_FILES`` <- gateway.trust_recent_files
//!
//! Historically the config.yaml -> env translation ran ONLY in gateway startup
//! (gateway/run.py), so any process that delivers media without booting the
//! gateway — a manual ``hermes cron run`` in the CLI, ``hermes send``, a
//! standalone cron tick — filtered MEDIA paths under DIFFERENT policy than the
//! gateway's scheduled deliveries. In strict/allowlisted enterprise deployments
//! that divergence silently dropped attachments from manual cron runs while
//! scheduled runs delivered them (text is unaffected — only media goes through
//! path validation).
//!
//! ``apply_media_policy_env()`` is that same translation as a shared, idempotent
//! helper. Gateway startup calls it, and every standalone delivery entrypoint
//! calls it immediately before filtering media paths.
//!
//! Precedence: an explicitly-set environment variable WINS over config.yaml.
//! This preserves both the operator contract (env overrides are how deployments
//! pin behavior) and gateway/run.py's historical shape (it only wrote the env
//! var when the config key was present; we additionally refuse to overwrite a
//! pre-existing env value so a shell-exported override survives).
//! ```
//!
//! Mapping:
//! - `_STRICT_ENV = "HERMES_MEDIA_DELIVERY_STRICT"` → [`_STRICT_ENV`] / [`STRICT_ENV`]
//! - `_ALLOW_DIRS_ENV = "HERMES_MEDIA_ALLOW_DIRS"` → [`_ALLOW_DIRS_ENV`] / [`ALLOW_DIRS_ENV`]
//! - `_TRUST_RECENT_ENV = "HERMES_MEDIA_TRUST_RECENT_FILES"` → [`_TRUST_RECENT_ENV`] / [`TRUST_RECENT_ENV`]
//! - `def _load_gateway_cfg(config=None)` → [`_load_gateway_cfg`]
//! - `def apply_media_policy_env(config=None)` → [`apply_media_policy_env`]
//! - `config.get("gateway", {})` + `isinstance(gateway_cfg, dict)` → object check + clone
//! - `from hermes_cli.config import load_config` → [`get_hermes_home`] + minimal `config.yaml` parse (JSON first, YAML fallback)
//! - `os.environ.get(env)` falsy check (None or "") → [`env_is_empty`]
//! - `os.environ[env] = "1" if val else "0"` → [`is_truthy`] + `std::env::set_var`
//! - `os.pathsep.join(str(p) for p in allow_dirs if p)` → [`PATH_SEP`] join with [`is_truthy`] filter
//! - `try: ... except Exception: logger.debug(...)` → `catch_unwind` + `log::debug!` (never raises)
//! - `get_hermes_home()` → [`get_hermes_home`] (mirrors `hermes_constants.get_hermes_home`)

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

// ---------------------------------------------------------------------------
// Constants — mirrors Python module-level constants
// ---------------------------------------------------------------------------

/// Env var for `gateway.strict`. Mirrors `_STRICT_ENV = "HERMES_MEDIA_DELIVERY_STRICT"`.
pub const _STRICT_ENV: &str = "HERMES_MEDIA_DELIVERY_STRICT";
/// Public alias.
pub const STRICT_ENV: &str = _STRICT_ENV;

/// Env var for `gateway.media_delivery_allow_dirs`. Mirrors `_ALLOW_DIRS_ENV = "HERMES_MEDIA_ALLOW_DIRS"`.
pub const _ALLOW_DIRS_ENV: &str = "HERMES_MEDIA_ALLOW_DIRS";
/// Public alias.
pub const ALLOW_DIRS_ENV: &str = _ALLOW_DIRS_ENV;

/// Env var for `gateway.trust_recent_files`. Mirrors `_TRUST_RECENT_ENV = "HERMES_MEDIA_TRUST_RECENT_FILES"`.
pub const _TRUST_RECENT_ENV: &str = "HERMES_MEDIA_TRUST_RECENT_FILES";
/// Public alias.
pub const TRUST_RECENT_ENV: &str = _TRUST_RECENT_ENV;

/// OS path separator for `media_delivery_allow_dirs` join.
/// Mirrors `os.pathsep` (":" on POSIX, ";" on Windows).
#[cfg(windows)]
const PATH_SEP: &str = ";";
#[cfg(not(windows))]
const PATH_SEP: &str = ":";

// ---------------------------------------------------------------------------
// HERMES_HOME — mirrors `hermes_constants.get_hermes_home()`
// ---------------------------------------------------------------------------

/// Resolve `HERMES_HOME`: `$HERMES_HOME` if set and non-empty, else `~/.hermes`.
///
/// Mirrors `hermes_constants.get_hermes_home()` / `hermes_cli.config.get_hermes_home`.
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

// Provide private alias mirroring Python's import indirection for grep-ability.
#[allow(dead_code)]
fn _get_hermes_home() -> PathBuf {
    get_hermes_home()
}

// ---------------------------------------------------------------------------
// Helpers — mirrors Python underscore-prefixed helpers
// ---------------------------------------------------------------------------

/// Returns true when `env` is absent or empty (`os.environ.get(env)` falsy).
///
/// Mirrors `not os.environ.get(_STRICT_ENV)` where `get` returns `None` on
/// missing and `""` is falsy. Whitespace-only `" "` is truthy in Python
/// and here.
fn env_is_empty(key: &str) -> bool {
    match std::env::var(key) {
        Ok(v) => v.is_empty(),
        Err(_) => true,
    }
}

/// Python truthiness for `serde_json::Value`.
///
/// Mirrors `bool(value)` / `if value` / `if strict / trust_recent else`:
/// - `None`/`Null` → false
/// - `bool` → itself
/// - `number` 0 / 0.0 → false, else true
/// - `string` "" → false, else true
/// - `array` [] → false, else true
/// - `object` {} → false, else true
fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i != 0
            } else if let Some(u) = n.as_u64() {
                u != 0
            } else if let Some(f) = n.as_f64() {
                f != 0.0
            } else {
                true
            }
        }
        Value::String(s) => !s.is_empty(),
        Value::Array(arr) => !arr.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

fn strip_quotes(s: &str) -> &str {
    let t = s.trim();
    if t.len() >= 2 {
        let bytes = t.as_bytes();
        if (bytes[0] == b'"' && bytes[t.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[t.len() - 1] == b'\'')
        {
            return &t[1..t.len() - 1];
        }
    }
    t
}

/// Minimal YAML `gateway:` block parser for `config.yaml` fallback.
///
/// Tries to extract `strict`, `media_delivery_allow_dirs`, `trust_recent_files`
/// from a `gateway:` mapping without adding `serde_yaml`. Handles:
/// - inline scalars: `strict: true`, `trust_recent_files: false`
/// - inline string: `media_delivery_allow_dirs: "/a:/b"`
/// - inline list: `media_delivery_allow_dirs: ["/a", "/b"]`
/// - block list:
///   ```yaml
///   media_delivery_allow_dirs:
///     - /a
///     - /b
///   ```
/// Returns the gateway map (may be empty → caller treats as {}).
fn parse_gateway_yaml_block(text: &str) -> Map<String, Value> {
    let mut gateway_map = Map::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut gateway_start: Option<usize> = None;
    let mut gateway_indent: usize = 0;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
        if indent == 0 && (trimmed == "gateway:" || trimmed.starts_with("gateway:")) {
            let after = trimmed.strip_prefix("gateway:").unwrap_or("").trim();
            if after.is_empty() {
                gateway_start = Some(idx);
                gateway_indent = indent;
                break;
            } else if after == "{}" || after == "null" || after == "~" {
                // `gateway: {}` or `gateway: null` → empty
                return gateway_map;
            }
        }
    }
    let Some(start) = gateway_start else {
        return gateway_map;
    };
    let mut i = start + 1;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }
        let indent = line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
        if indent <= gateway_indent {
            break;
        }
        if trimmed.starts_with("- ") {
            i += 1;
            continue;
        }
        let Some(colon_pos) = trimmed.find(':') else {
            i += 1;
            continue;
        };
        let key = trimmed[..colon_pos].trim().to_string();
        if key != "strict" && key != "media_delivery_allow_dirs" && key != "trust_recent_files" {
            i += 1;
            continue;
        }
        let val_part = trimmed[colon_pos + 1..].trim().to_string();
        if val_part.is_empty() {
            // Block list or empty value — peek ahead for "- " items
            let mut list_vals: Vec<Value> = Vec::new();
            let mut found_list = false;
            let mut j = i + 1;
            while j < lines.len() {
                let next_line = lines[j];
                let next_trimmed = next_line.trim();
                if next_trimmed.is_empty() || next_trimmed.starts_with('#') {
                    j += 1;
                    continue;
                }
                let next_indent = next_line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
                if next_indent <= indent {
                    break;
                }
                if next_trimmed.starts_with("- ") {
                    found_list = true;
                    let item = next_trimmed[2..].trim();
                    let unquoted = strip_quotes(item);
                    if !unquoted.is_empty() {
                        list_vals.push(Value::String(unquoted.to_string()));
                    }
                    j += 1;
                } else {
                    break;
                }
            }
            if found_list {
                gateway_map.insert(key, Value::Array(list_vals));
                i = j;
                continue;
            } else {
                // Empty scalar → Null (treated as missing/None by caller)
                gateway_map.insert(key, Value::Null);
            }
        } else if val_part.starts_with('[') {
            // Inline list
            let inner = val_part
                .trim()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim();
            if inner.is_empty() {
                gateway_map.insert(key, Value::Array(vec![]));
            } else {
                let mut arr: Vec<Value> = Vec::new();
                for part in inner.split(',') {
                    let p = part.trim();
                    let unquoted = strip_quotes(p);
                    if !unquoted.is_empty() {
                        arr.push(Value::String(unquoted.to_string()));
                    }
                }
                gateway_map.insert(key, Value::Array(arr));
            }
        } else {
            let unquoted = strip_quotes(&val_part).to_string();
            let lower = unquoted.to_lowercase();
            if key == "strict" || key == "trust_recent_files" {
                let bool_val = match lower.as_str() {
                    "true" | "yes" | "on" | "1" => Some(true),
                    "false" | "no" | "off" | "0" => Some(false),
                    _ => None,
                };
                if let Some(b) = bool_val {
                    gateway_map.insert(key, Value::Bool(b));
                } else {
                    // Keep raw string for truthiness fallback
                    gateway_map.insert(key, Value::String(unquoted));
                }
            } else {
                // media_delivery_allow_dirs inline scalar string
                gateway_map.insert(key, Value::String(unquoted));
            }
        }
        i += 1;
    }
    gateway_map
}

fn load_gateway_cfg_from_file() -> Option<Value> {
    let home = get_hermes_home();
    let path = home.join("config.yaml");
    let text = std::fs::read_to_string(&path).ok()?;
    // Try JSON first (some deployments write JSON)
    if let Ok(v) = serde_json::from_str::<Value>(&text) {
        if let Some(obj) = v.as_object() {
            if let Some(gw) = obj.get("gateway") {
                if gw.is_object() {
                    return Some(gw.clone());
                }
            }
        }
        // JSON parsed but no gateway object → treat as empty (Python would return {})
        return None;
    }
    let gateway_map = parse_gateway_yaml_block(&text);
    if gateway_map.is_empty() {
        None
    } else {
        Some(Value::Object(gateway_map))
    }
}

// ---------------------------------------------------------------------------
// _load_gateway_cfg — mirrors Python `def _load_gateway_cfg(...)`
// ---------------------------------------------------------------------------

/// Load `gateway` sub-config.
///
/// Mirrors:
/// ```python
/// def _load_gateway_cfg(config: Optional[Dict[str, Any]] = None) -> Dict[str, Any]:
///     if config is None:
///         try:
///             from hermes_cli.config import load_config
///             config = load_config() or {}
///         except Exception:
///             return {}
///     gateway_cfg = config.get("gateway", {})
///     return gateway_cfg if isinstance(gateway_cfg, dict) else {}
/// ```
pub fn _load_gateway_cfg(config: Option<&Value>) -> Value {
    let gateway_val: Option<Value> = match config {
        Some(cfg) => cfg.get("gateway").cloned(),
        None => load_gateway_cfg_from_file(),
    };
    match gateway_val {
        Some(v) if v.is_object() => v,
        _ => Value::Object(Map::new()),
    }
}

// ---------------------------------------------------------------------------
// apply_media_policy_env — mirrors Python `def apply_media_policy_env(...)`
// ---------------------------------------------------------------------------

/// Bridge gateway media-policy settings from `config.yaml` into the env.
///
/// Idempotent and env-wins: a variable already present in the environment is
/// never overwritten, so gateway startup (which runs this same helper) and
/// operator shell exports keep precedence. Never raises — a policy-bridge
/// failure must not break delivery; the validator falls back to its
/// defaults exactly as before.
///
/// Mirrors:
/// ```python
/// def apply_media_policy_env(config: Optional[Dict[str, Any]] = None) -> None:
///     try:
///         gateway_cfg = _load_gateway_cfg(config)
///         if not gateway_cfg:
///             return
///         strict = gateway_cfg.get("strict")
///         if strict is not None and not os.environ.get(_STRICT_ENV):
///             os.environ[_STRICT_ENV] = "1" if strict else "0"
///         allow_dirs = gateway_cfg.get("media_delivery_allow_dirs")
///         if allow_dirs and not os.environ.get(_ALLOW_DIRS_ENV):
///             if isinstance(allow_dirs, str):
///                 allow_dirs_str = allow_dirs
///             elif isinstance(allow_dirs, (list, tuple)):
///                 allow_dirs_str = os.pathsep.join(str(p) for p in allow_dirs if p)
///             else:
///                 allow_dirs_str = ""
///             if allow_dirs_str:
///                 os.environ[_ALLOW_DIRS_ENV] = allow_dirs_str
///         trust_recent = gateway_cfg.get("trust_recent_files")
///         if trust_recent is not None and not os.environ.get(_TRUST_RECENT_ENV):
///             os.environ[_TRUST_RECENT_ENV] = "1" if trust_recent else "0"
///     except Exception:
///         logger.debug("apply_media_policy_env failed", exc_info=True)
/// ```
pub fn apply_media_policy_env(config: Option<&Value>) {
    // Never raises — mirror Python's `except Exception: logger.debug(...)`
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        apply_media_policy_env_inner(config);
    }));
    if result.is_err() {
        // Mirrors `logger.debug("apply_media_policy_env failed", exc_info=True)`
        // Use log::debug when available; fall back to no-op if log not linked.
        #[allow(unused)]
        {
            log::debug!("apply_media_policy_env failed");
        }
    }
}

fn apply_media_policy_env_inner(config: Option<&Value>) {
    let gateway_cfg = _load_gateway_cfg(config);
    let Some(map) = gateway_cfg.as_object() else {
        return;
    };
    if map.is_empty() {
        return;
    }

    // strict — `is not None` + env-wins + truthiness → "1"/"0"
    if let Some(strict) = map.get("strict") {
        if !strict.is_null() && env_is_empty(_STRICT_ENV) {
            let val = if is_truthy(strict) { "1" } else { "0" };
            std::env::set_var(_STRICT_ENV, val);
        }
    }

    // media_delivery_allow_dirs — truthy guard + env-wins + str/list/tuple branches
    if let Some(allow_dirs) = map.get("media_delivery_allow_dirs") {
        if is_truthy(allow_dirs) && env_is_empty(_ALLOW_DIRS_ENV) {
            let allow_dirs_str = match allow_dirs {
                Value::String(s) => s.clone(),
                Value::Array(arr) => {
                    let parts: Vec<String> = arr
                        .iter()
                        .filter(|v| is_truthy(v))
                        .map(|v| match v {
                            Value::String(s) => s.clone(),
                            Value::Number(n) => n.to_string(),
                            Value::Bool(b) => b.to_string(),
                            _ => {
                                let s = v.to_string();
                                s.trim_matches('"').to_string()
                            }
                        })
                        .filter(|s| !s.is_empty())
                        .collect();
                    parts.join(PATH_SEP)
                }
                _ => String::new(),
            };
            if !allow_dirs_str.is_empty() {
                std::env::set_var(_ALLOW_DIRS_ENV, allow_dirs_str);
            }
        }
    }

    // trust_recent_files — `is not None` + env-wins + truthiness → "1"/"0"
    if let Some(trust_recent) = map.get("trust_recent_files") {
        if !trust_recent.is_null() && env_is_empty(_TRUST_RECENT_ENV) {
            let val = if is_truthy(trust_recent) { "1" } else { "0" };
            std::env::set_var(_TRUST_RECENT_ENV, val);
        }
    }
}

// Testable variant with explicit home (mirrors `get_hermes_home()` indirection).
#[allow(dead_code)]
fn _load_gateway_cfg_with_home(config: Option<&Value>, _home: &Path) -> Value {
    // For test isolation, when config is Some we ignore _home, same as production.
    // When config is None we load from _home/config.yaml instead of global HERMES_HOME.
    match config {
        Some(cfg) => {
            let gw = cfg.get("gateway").cloned();
            match gw {
                Some(v) if v.is_object() => v,
                _ => Value::Object(Map::new()),
            }
        }
        None => {
            let path = _home.join("config.yaml");
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(_) => return Value::Object(Map::new()),
            };
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                if let Some(obj) = v.as_object() {
                    if let Some(gw) = obj.get("gateway") {
                        if gw.is_object() {
                            return gw.clone();
                        }
                    }
                }
                return Value::Object(Map::new());
            }
            let gateway_map = parse_gateway_yaml_block(&text);
            if gateway_map.is_empty() {
                Value::Object(Map::new())
            } else {
                Value::Object(gateway_map)
            }
        }
    }
}

// Provide private aliases mirroring Python's underscore-prefixed helpers for traceability.
#[allow(dead_code)]
fn _is_truthy(v: &Value) -> bool {
    is_truthy(v)
}

#[allow(dead_code)]
fn _env_is_empty(key: &str) -> bool {
    env_is_empty(key)
}

#[allow(dead_code)]
fn _parse_gateway_yaml_block(text: &str) -> Map<String, Value> {
    parse_gateway_yaml_block(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::env;

    fn with_env_lock<F: FnOnce()>(f: F) {
        // Simple serialisation: env vars are process-global.
        // Tests that touch env should be run single-threaded.
        f()
    }

    #[test]
    fn load_gateway_cfg_returns_object() {
        let cfg = json!({"gateway": {"strict": true}});
        let gw = _load_gateway_cfg(Some(&cfg));
        assert_eq!(gw.get("strict").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn load_gateway_cfg_non_dict_returns_empty() {
        let cfg = json!({"gateway": "not a dict"});
        let gw = _load_gateway_cfg(Some(&cfg));
        assert!(gw.as_object().unwrap().is_empty());
    }

    #[test]
    fn load_gateway_cfg_missing_returns_empty() {
        let cfg = json!({"other": 1});
        let gw = _load_gateway_cfg(Some(&cfg));
        assert!(gw.as_object().unwrap().is_empty());
    }

    #[test]
    fn apply_media_policy_env_strict_sets_env() {
        with_env_lock(|| {
            let _ = env::remove_var(_STRICT_ENV);
            let cfg = json!({"gateway": {"strict": true}});
            apply_media_policy_env(Some(&cfg));
            assert_eq!(env::var(_STRICT_ENV).unwrap(), "1");
            let _ = env::remove_var(_STRICT_ENV);
        });
    }

    #[test]
    fn apply_media_policy_env_strict_false_sets_zero() {
        with_env_lock(|| {
            let _ = env::remove_var(_STRICT_ENV);
            let cfg = json!({"gateway": {"strict": false}});
            apply_media_policy_env(Some(&cfg));
            assert_eq!(env::var(_STRICT_ENV).unwrap(), "0");
            let _ = env::remove_var(_STRICT_ENV);
        });
    }

    #[test]
    fn apply_media_policy_env_env_wins() {
        with_env_lock(|| {
            env::set_var(_STRICT_ENV, "0");
            let cfg = json!({"gateway": {"strict": true}});
            apply_media_policy_env(Some(&cfg));
            assert_eq!(env::var(_STRICT_ENV).unwrap(), "0");
            let _ = env::remove_var(_STRICT_ENV);
        });
    }

    #[test]
    fn apply_media_policy_env_allow_dirs_string() {
        with_env_lock(|| {
            let _ = env::remove_var(_ALLOW_DIRS_ENV);
            let cfg = json!({"gateway": {"media_delivery_allow_dirs": "/tmp/a:/tmp/b"}});
            apply_media_policy_env(Some(&cfg));
            assert_eq!(env::var(_ALLOW_DIRS_ENV).unwrap(), "/tmp/a:/tmp/b");
            let _ = env::remove_var(_ALLOW_DIRS_ENV);
        });
    }

    #[test]
    fn apply_media_policy_env_allow_dirs_list() {
        with_env_lock(|| {
            let _ = env::remove_var(_ALLOW_DIRS_ENV);
            let cfg = json!({"gateway": {"media_delivery_allow_dirs": ["/tmp/a", "/tmp/b"]}});
            apply_media_policy_env(Some(&cfg));
            let sep = PATH_SEP;
            assert_eq!(
                env::var(_ALLOW_DIRS_ENV).unwrap(),
                format!("/tmp/a{}/tmp/b", sep)
            );
            let _ = env::remove_var(_ALLOW_DIRS_ENV);
        });
    }

    #[test]
    fn apply_media_policy_env_allow_dirs_empty_skips() {
        with_env_lock(|| {
            let _ = env::remove_var(_ALLOW_DIRS_ENV);
            let cfg = json!({"gateway": {"media_delivery_allow_dirs": ""}});
            apply_media_policy_env(Some(&cfg));
            assert!(env::var(_ALLOW_DIRS_ENV).is_err());
            let cfg2 = json!({"gateway": {"media_delivery_allow_dirs": []}});
            apply_media_policy_env(Some(&cfg2));
            assert!(env::var(_ALLOW_DIRS_ENV).is_err());
        });
    }

    #[test]
    fn apply_media_policy_env_trust_recent() {
        with_env_lock(|| {
            let _ = env::remove_var(_TRUST_RECENT_ENV);
            let cfg = json!({"gateway": {"trust_recent_files": true}});
            apply_media_policy_env(Some(&cfg));
            assert_eq!(env::var(_TRUST_RECENT_ENV).unwrap(), "1");
            let _ = env::remove_var(_TRUST_RECENT_ENV);
            let cfg2 = json!({"gateway": {"trust_recent_files": false}});
            apply_media_policy_env(Some(&cfg2));
            assert_eq!(env::var(_TRUST_RECENT_ENV).unwrap(), "0");
            let _ = env::remove_var(_TRUST_RECENT_ENV);
        });
    }

    #[test]
    fn apply_media_policy_env_idempotent() {
        with_env_lock(|| {
            let _ = env::remove_var(_STRICT_ENV);
            let cfg = json!({"gateway": {"strict": true}});
            apply_media_policy_env(Some(&cfg));
            assert_eq!(env::var(_STRICT_ENV).unwrap(), "1");
            // second call with different config must not overwrite (env-wins)
            let cfg2 = json!({"gateway": {"strict": false}});
            apply_media_policy_env(Some(&cfg2));
            assert_eq!(env::var(_STRICT_ENV).unwrap(), "1");
            let _ = env::remove_var(_STRICT_ENV);
        });
    }

    #[test]
    fn apply_media_policy_env_missing_gateway_noop() {
        with_env_lock(|| {
            let _ = env::remove_var(_STRICT_ENV);
            let _ = env::remove_var(_ALLOW_DIRS_ENV);
            let _ = env::remove_var(_TRUST_RECENT_ENV);
            let cfg = json!({"other": 123});
            apply_media_policy_env(Some(&cfg));
            assert!(env::var(_STRICT_ENV).is_err());
            assert!(env::var(_ALLOW_DIRS_ENV).is_err());
            assert!(env::var(_TRUST_RECENT_ENV).is_err());
        });
    }

    #[test]
    fn parse_yaml_block_strict() {
        let yaml = "gateway:\n  strict: true\n  trust_recent_files: false\n";
        let map = parse_gateway_yaml_block(yaml);
        assert_eq!(map.get("strict").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            map.get("trust_recent_files").and_then(|v| v.as_bool()),
            Some(false)
        );
    }

    #[test]
    fn parse_yaml_block_allow_dirs_block_list() {
        let yaml = "gateway:\n  media_delivery_allow_dirs:\n    - /tmp/a\n    - /tmp/b\n";
        let map = parse_gateway_yaml_block(yaml);
        let arr = map.get("media_delivery_allow_dirs").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].as_str().unwrap(), "/tmp/a");
    }
}
