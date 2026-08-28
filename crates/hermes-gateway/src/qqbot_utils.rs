//! QQBot shared utilities — User-Agent, HTTP helpers, config coercion.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/platforms/qqbot/utils.py` (71 LOC).
//!
//! Python source docstring (preserved):
//! ```text
//! QQBot shared utilities — User-Agent, HTTP helpers, config coercion.
//! ```
//!
//! Mapping:
//! - `def _get_hermes_version() -> str` → [`_get_hermes_version`] / [`get_hermes_version`] / [`hermes_version`]
//! - `def build_user_agent() -> str` → [`build_user_agent`]
//! - `def get_api_headers() -> Dict[str, str]` → [`get_api_headers`]
//! - `def coerce_list(value: Any) -> List[str]` → [`coerce_list`] / [`coerce_list_value`] / [`_coerce_list`]
//! - `QQBOT_VERSION` (from `.constants`) → [`crate::qqbot_constants::QQBOT_VERSION`] (re-exported via [`QQBOT_VERSION`])
//! - `platform.system().lower()` → [`os_name`]
//! - `sys.version_info` (py_version) → [`python_version`] (crate version analogue, ponytail)
//! - `importlib.metadata.version("hermes-agent")` → [`get_hermes_version`] (env + `CARGO_PKG_VERSION` fallback)

use std::collections::HashMap;

use crate::qqbot_constants::QQBOT_VERSION;

// ---------------------------------------------------------------------------
// QQBOT_VERSION re-export for discoverability
// ---------------------------------------------------------------------------

/// Re-export of `QQBOT_VERSION` for callers that import via `qqbot_utils`.
/// Mirrors `from .constants import QQBOT_VERSION` in `utils.py`.
pub use crate::qqbot_constants::QQBOT_VERSION as QQBOT_VERSION_REEXPORT;

// Local alias for doc-link convenience.
#[allow(unused_imports)]
use crate::qqbot_constants::QQBOT_VERSION as _QQBOT_VERSION;

// ---------------------------------------------------------------------------
// User-Agent helpers — mirrors `_get_hermes_version` + `build_user_agent`
// ---------------------------------------------------------------------------

/// Return the hermes-agent package version, or `"dev"` if unavailable.
///
/// Mirrors:
/// ```python
/// def _get_hermes_version() -> str:
///     try:
///         from importlib.metadata import version
///         return version("hermes-agent")
///     except Exception:
///         return "dev"
/// ```
///
/// Rust analogue: probe `HERMES_VERSION` / `HERMES_AGENT_VERSION` env vars
/// (runtime overrides), then compile-time `CARGO_PKG_VERSION`, else `"dev"`.
/// `CARGO_PKG_VERSION` is always set (workspace `0.1.1`), so `"dev"` is the
/// fallback only when that env is empty / unset in exotic builds.
pub fn get_hermes_version() -> String {
    // ponytail: env probe first so tests / deploy overrides can pin UA without rebuild
    for key in ["HERMES_VERSION", "HERMES_AGENT_VERSION"] {
        if let Ok(raw) = std::env::var(key) {
            let trimmed = raw.trim().to_string();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
    }
    // Compile-time crate version (workspace 0.1.1). Mirrors importlib.metadata success path.
    let pkg = env!("CARGO_PKG_VERSION").trim();
    if !pkg.is_empty() {
        return pkg.to_string();
    }
    // Fallback mirrors `except Exception: return "dev"`.
    "dev".to_string()
}

/// Alias matching Python `hermes_version` lookup for grep-ability.
pub fn hermes_version() -> String {
    get_hermes_version()
}

/// Private alias mirroring Python `def _get_hermes_version`.
pub fn _get_hermes_version() -> String {
    get_hermes_version()
}

/// Python version analogue used inside [`build_user_agent`].
///
/// Python:
/// ```python
/// py_version = f"{sys.version_info.major}.{sys.version_info.minor}.{sys.version_info.micro}"
/// ```
///
/// Rust has no Python runtime; use the crate version as the closest stable
/// analogue (ponytail: avoids adding a `rustc_version` dep for a UA string).
/// Exposed for 1:1 traceability.
pub fn python_version() -> String {
    // Prefer CARGO_PKG_RUST_VERSION if the manifest sets `rust-version`, else crate version.
    if let Some(rv) = option_env!("CARGO_PKG_RUST_VERSION") {
        let t = rv.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    let pkg = env!("CARGO_PKG_VERSION").trim();
    if !pkg.is_empty() {
        return pkg.to_string();
    }
    "unknown".to_string()
}

/// Private alias for grep discoverability.
#[allow(dead_code)]
fn _python_version() -> String {
    python_version()
}

/// OS name analogue for `platform.system().lower()`.
///
/// Python: `platform.system().lower()` → `"linux"` / `"darwin"` / `"windows"`
/// Rust: `std::env::consts::OS` is already lowercased (`"linux"`, `"macos"`,
/// `"windows"`, …). Map `"macos"` → `"darwin"` to match Python's Darwin token.
pub fn os_name() -> String {
    match std::env::consts::OS {
        "macos" => "darwin".to_string(),
        other => other.to_lowercase(),
    }
}

/// Build a descriptive User-Agent string.
///
/// Mirrors:
/// ```python
/// def build_user_agent() -> str:
///     py_version = f"{sys.version_info.major}.{sys.version_info.minor}.{sys.version_info.micro}"
///     os_name = platform.system().lower()
///     hermes_version = _get_hermes_version()
///     return f"QQBotAdapter/{QQBOT_VERSION} (Python/{py_version}; {os_name}; Hermes/{hermes_version})"
/// ```
///
/// Format:
/// ```text
/// QQBotAdapter/<qqbot_version> (Python/<py_version>; <os>; Hermes/<hermes_version>)
/// ```
/// Example:
/// ```text
/// QQBotAdapter/1.1.0 (Python/0.1.1; linux; Hermes/0.1.1)
/// ```
pub fn build_user_agent() -> String {
    let py_version = python_version();
    let os = os_name();
    let hv = get_hermes_version();
    format!("QQBotAdapter/{QQBOT_VERSION} (Python/{py_version}; {os}; Hermes/{hv})")
}

/// Private alias for grep discoverability.
#[allow(dead_code)]
fn _build_user_agent() -> String {
    build_user_agent()
}

// ---------------------------------------------------------------------------
// HTTP helpers — mirrors `get_api_headers`
// ---------------------------------------------------------------------------

/// Return standard HTTP headers for QQBot API requests.
///
/// Mirrors:
/// ```python
/// def get_api_headers() -> Dict[str, str]:
///     return {
///         "Content-Type": "application/json",
///         "Accept": "application/json",
///         "User-Agent": build_user_agent(),
///     }
/// ```
///
/// `q.qq.com` requires `Accept: application/json` — without it, the server
/// returns a JavaScript anti-bot challenge page.
pub fn get_api_headers() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("Content-Type".to_string(), "application/json".to_string());
    m.insert("Accept".to_string(), "application/json".to_string());
    m.insert("User-Agent".to_string(), build_user_agent());
    m
}

/// Private alias for grep discoverability.
#[allow(dead_code)]
fn _get_api_headers() -> HashMap<String, String> {
    get_api_headers()
}

// ---------------------------------------------------------------------------
// Config helpers — mirrors `coerce_list`
// ---------------------------------------------------------------------------

/// Coerce config values into a trimmed string list.
///
/// Mirrors:
/// ```python
/// def coerce_list(value: Any) -> List[str]:
///     if value is None:
///         return []
///     if isinstance(value, str):
///         return [item.strip() for item in value.split(",") if item.strip()]
///     if isinstance(value, (list, tuple, set)):
///         return [str(item).strip() for item in value if str(item).strip()]
///     return [str(value).strip()] if str(value).strip() else []
/// ```
///
/// Rust input is `&serde_json::Value` (the closest `Any` for JSON-derived
/// gateway config). Callers with native Rust strings/vecs can use
/// [`coerce_list_from_str`] / [`coerce_list_from_vec_str`] or construct a
/// `serde_json::Value` via `serde_json::json!(...)`.
pub fn coerce_list(value: &serde_json::Value) -> Vec<String> {
    coerce_list_value(value)
}

/// Core implementation for `&serde_json::Value`.
pub fn coerce_list_value(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::Null => vec![],
        serde_json::Value::String(s) => s
            .split(',')
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty())
            .collect(),
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|v| {
                let s = value_to_string(v);
                let t = s.trim().to_string();
                if t.is_empty() {
                    None
                } else {
                    Some(t)
                }
            })
            .collect(),
        // Object, Bool, Number → single-value fallback: str(value).strip()
        _ => {
            let s = value_to_string(value);
            let t = s.trim().to_string();
            if t.is_empty() {
                vec![]
            } else {
                vec![t]
            }
        }
    }
}

/// Convert a `serde_json::Value` to its `str(value)` analogue for `coerce_list`.
///
/// - `String(s)` → `s.clone()` (no JSON quoting)
/// - `Number`, `Bool` → `to_string()`
/// - `Null` → `""` at top level is handled separately; inside arrays → `"null"` trimmed
/// - `Array`/`Object` → JSON string (`to_string()`) then trimmed; mirrors Python `str([...])`
fn value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "null".to_string(),
        _ => v.to_string().trim_matches('"').to_string(),
    }
}

/// Convenience: coerce a comma-separated string (mirrors `isinstance(value, str)` branch).
pub fn coerce_list_from_str(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

/// Convenience: coerce an optional comma-separated string (`None` → `[]`).
pub fn coerce_list_from_opt_str(value: Option<&str>) -> Vec<String> {
    match value {
        None => vec![],
        Some(s) => coerce_list_from_str(s),
    }
}

/// Convenience: coerce a slice of string-like items (mirrors `list/tuple/set` branch).
pub fn coerce_list_from_vec_str<I, S>(items: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    items
        .into_iter()
        .map(|s| s.as_ref().trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Private alias matching Python's `def coerce_list` name for grep traceability
/// (some callers may search `_coerce_list` as in `qqbot_init` adapter re-export).
#[allow(dead_code)]
fn _coerce_list(value: &serde_json::Value) -> Vec<String> {
    coerce_list(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn hermes_version_not_empty() {
        let v = get_hermes_version();
        assert!(!v.is_empty());
        assert_eq!(_get_hermes_version(), v);
        assert_eq!(hermes_version(), v);
    }

    #[test]
    fn hermes_version_env_override() {
        let key = "HERMES_VERSION";
        let prev = env::var(key).ok();
        env::set_var(key, "9.9.9-test");
        assert_eq!(get_hermes_version(), "9.9.9-test");
        match prev {
            Some(val) => env::set_var(key, val),
            None => env::remove_var(key),
        }
    }

    #[test]
    fn build_user_agent_format() {
        let ua = build_user_agent();
        assert!(ua.starts_with(&format!("QQBotAdapter/{QQBOT_VERSION} ")));
        assert!(ua.contains("Python/"));
        assert!(ua.contains("Hermes/"));
        assert!(ua.contains(";"));
        // os token present
        assert!(ua.contains(&os_name()));
    }

    #[test]
    fn get_api_headers_shape() {
        let h = get_api_headers();
        assert_eq!(h.get("Content-Type").map(|s| s.as_str()), Some("application/json"));
        assert_eq!(h.get("Accept").map(|s| s.as_str()), Some("application/json"));
        let ua = h.get("User-Agent").expect("User-Agent header");
        assert!(ua.contains("QQBotAdapter"));
        assert_eq!(ua, &build_user_agent());
    }

    #[test]
    fn coerce_list_none() {
        assert_eq!(coerce_list(&serde_json::Value::Null), Vec::<String>::new());
        assert_eq!(coerce_list_from_opt_str(None), Vec::<String>::new());
    }

    #[test]
    fn coerce_list_str_comma() {
        assert_eq!(
            coerce_list(&serde_json::json!("a, b ,c")),
            vec!["a", "b", "c"]
        );
        assert_eq!(coerce_list_from_str("a, b ,c"), vec!["a", "b", "c"]);
        assert_eq!(coerce_list(&serde_json::json!("  , , ")), Vec::<String>::new());
        assert_eq!(coerce_list(&serde_json::json!("single")), vec!["single"]);
    }

    #[test]
    fn coerce_list_array() {
        assert_eq!(
            coerce_list(&serde_json::json!(["x", " y ", "", "z"])),
            vec!["x", "y", "z"]
        );
        assert_eq!(
            coerce_list(&serde_json::json!([1, 2, " 3 "])),
            vec!["1", "2", "3"]
        );
        assert_eq!(
            coerce_list_from_vec_str(vec!["a", " b ", ""]),
            vec!["a", "b"]
        );
    }

    #[test]
    fn coerce_list_single_value_fallback() {
        // Number
        assert_eq!(coerce_list(&serde_json::json!(123)), vec!["123"]);
        // Bool
        assert_eq!(coerce_list(&serde_json::json!(true)), vec!["true"]);
        // Empty string fallback → []
        assert_eq!(coerce_list(&serde_json::json!("   ")), Vec::<String>::new());
    }

    #[test]
    fn coerce_list_trims_and_filters() {
        // mirrors Python: [str(item).strip() for item in value if str(item).strip()]
        assert_eq!(
            coerce_list(&serde_json::json!(["  ", "a", "", " b "])),
            vec!["a", "b"]
        );
    }

    #[test]
    fn os_name_not_empty() {
        assert!(!os_name().is_empty());
    }

    #[test]
    fn python_version_not_empty() {
        assert!(!python_version().is_empty());
    }
}
