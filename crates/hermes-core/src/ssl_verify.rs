//! TLS verify resolution for httpx/OpenAI provider clients.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/ssl_verify.py` (63 lines).
//!
//! Priority (mirrors Python docstring):
//! 1. `ssl_verify: false` — disable verification (local dev only)
//! 2. explicit `ca_bundle` (per-provider `ssl_ca_cert` config field)
//! 3. `HERMES_CA_BUNDLE`, `SSL_CERT_FILE`, `REQUESTS_CA_BUNDLE`, `CURL_CA_BUNDLE` env vars
//! 4. `True` (httpx/certifi default)
//!
//! `base_url` is used only for the insecure-mode warning message.
//!
//! Python `ssl.SSLContext` return is modeled as [`HttpxVerify::CaFile`] carrying the
//! validated bundle path. The Rust caller maps this to `reqwest`/`rustls` config;
//! the `.py` `ssl.create_default_context(cafile=)` validation (file exists) is
//! preserved as `Path::is_file`. The full `truststore`/`get_ca_certs` checks from
//! `ssl_guard.py` are out of scope — this file is `ssl_verify.py` only.
//!
//! Python source docstring (preserved):
//! ```text
//! TLS verify resolution for httpx/OpenAI provider clients.
//! ```

use std::env;
use std::path::{Path, PathBuf};

use serde_json::Value;

// ---------------------------------------------------------------------------
// Constants — mirrors lines 11-12 + 30-35 + 48-54
// ---------------------------------------------------------------------------

/// Env vars probed for a CA bundle, in priority order (lines 48-54).
/// Mirrors `os.getenv("HERMES_CA_BUNDLE")` etc. chain.
pub const CA_BUNDLE_ENV_VARS: &[&str] = &[
    "HERMES_CA_BUNDLE",
    "SSL_CERT_FILE",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
];

#[allow(dead_code)]
const _CA_BUNDLE_ENV_VARS: &[&str] = CA_BUNDLE_ENV_VARS;

/// String values that count as `ssl_verify: false` when `ssl_verify` is a str
/// (mirrors `{"false", "0", "no", "off"}` at line 17).
const INSECURE_STR_VALUES: &[&str] = &["false", "0", "no", "off"];

// ---------------------------------------------------------------------------
// HttpxVerify — mirrors `bool | ssl.SSLContext` return (line 27)
// ---------------------------------------------------------------------------

/// Resolved `httpx` verify value. Mirrors the Python return type
/// `bool | ssl.SSLContext` (line 27):
/// - `False` → [`HttpxVerify::Disabled`] (verification disabled)
/// - `True`  → [`HttpxVerify::Default`] (use httpx/certifi defaults)
/// - `ssl.SSLContext(cafile=path)` → [`HttpxVerify::CaFile(path)`]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpxVerify {
    /// Mirrors `return False` (lines 46) — verification disabled.
    Disabled,
    /// Mirrors `return True` (line 63) — default verification.
    Default,
    /// Mirrors `return ssl.create_default_context(cafile=ca_path)` (line 58).
    /// Carries the validated CA bundle path.
    CaFile(PathBuf),
}

impl HttpxVerify {
    /// Mirrors Python truthiness: `False` is falsy, `True`/`SSLContext` are truthy.
    pub fn is_disabled(&self) -> bool {
        matches!(self, Self::Disabled)
    }

    pub fn is_default(&self) -> bool {
        matches!(self, Self::Default)
    }

    pub fn ca_path(&self) -> Option<&Path> {
        match self {
            Self::CaFile(p) => Some(p.as_path()),
            _ => None,
        }
    }

    /// Mirrors Python `bool(verify)` / `verify is False` checks (e.g. `config.py:1551` `if tls.get("ssl_verify") is False`).
    /// Returns `false` only for `Disabled`, `true` otherwise.
    pub fn as_bool(&self) -> bool {
        !self.is_disabled()
    }
}

// ---------------------------------------------------------------------------
// _coerce_insecure — mirrors lines 14-19
// ---------------------------------------------------------------------------

/// Mirrors `_coerce_insecure(ssl_verify: Any) -> bool` (lines 14-19):
/// ```python
/// def _coerce_insecure(ssl_verify: Any) -> bool:
///     if ssl_verify is False:
///         return True
///     if isinstance(ssl_verify, str) and ssl_verify.strip().lower() in {"false", "0", "no", "off"}:
///         return True
///     return False
/// ```
pub fn coerce_insecure(ssl_verify: Option<&Value>) -> bool {
    match ssl_verify {
        Some(Value::Bool(b)) => !*b, // `is False` → true, `is True` → false (identity check, not truthiness)
        Some(Value::String(s)) => INSECURE_STR_VALUES.contains(&s.trim().to_lowercase().as_str()),
        _ => false,
    }
}

/// Convenience overload for callers that already have a bool.
pub fn coerce_insecure_bool(ssl_verify: Option<bool>) -> bool {
    matches!(ssl_verify, Some(false))
}

/// Convenience overload for callers that already have a string slice.
pub fn coerce_insecure_str(ssl_verify: Option<&str>) -> bool {
    match ssl_verify {
        Some(s) => INSECURE_STR_VALUES.contains(&s.trim().to_lowercase().as_str()),
        None => false,
    }
}

#[allow(dead_code)]
fn _coerce_insecure(ssl_verify: Option<&Value>) -> bool {
    coerce_insecure(ssl_verify)
}

// ---------------------------------------------------------------------------
// helpers — mirrors lines 39-62
// ---------------------------------------------------------------------------

/// Expand leading `~` to `$HOME` / `$USERPROFILE` (mirrors `Path(...).expanduser()` line 56).
fn expand_user(path: &str) -> String {
    if path == "~" {
        if let Ok(home) = env::var("HOME").or_else(|_| env::var("USERPROFILE")) {
            if !home.is_empty() {
                return home;
            }
        }
        return path.to_string();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = env::var("HOME").or_else(|_| env::var("USERPROFILE")) {
            if !home.is_empty() {
                return format!("{}/{}", home.trim_end_matches('/'), rest);
            }
        }
    }
    // Windows `~\` variant (rare, but cheap to handle)
    if let Some(rest) = path.strip_prefix("~\\") {
        if let Ok(home) = env::var("HOME").or_else(|_| env::var("USERPROFILE")) {
            if !home.is_empty() {
                return format!("{}\\{}", home.trim_end_matches(['/', '\\']), rest);
            }
        }
    }
    path.to_string()
}

#[allow(dead_code)]
fn _expand_user(path: &str) -> String {
    expand_user(path)
}

/// Resolve the effective CA bundle path from explicit `ca_bundle` and env fallback.
///
/// Mirrors (lines 48-54):
/// ```python
/// effective_ca = (
///     (ca_bundle or "").strip()
///     or os.getenv("HERMES_CA_BUNDLE", "").strip()
///     or os.getenv("SSL_CERT_FILE", "").strip()
///     or os.getenv("REQUESTS_CA_BUNDLE", "").strip()
///     or os.getenv("CURL_CA_BUNDLE", "").strip()
/// )
/// ```
fn effective_ca_bundle(ca_bundle: Option<&str>) -> Option<String> {
    if let Some(v) = ca_bundle {
        let t = v.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    for var in CA_BUNDLE_ENV_VARS {
        if let Ok(val) = env::var(var) {
            let t = val.trim().to_string();
            if !t.is_empty() {
                return Some(t);
            }
        }
    }
    None
}

#[allow(dead_code)]
fn _effective_ca_bundle(ca_bundle: Option<&str>) -> Option<String> {
    effective_ca_bundle(ca_bundle)
}

/// Testable variant that uses an explicit env map instead of `std::env::var`.
/// Mirrors same priority but injects env for hermetic tests.
pub fn effective_ca_bundle_with_env(
    ca_bundle: Option<&str>,
    env_map: &std::collections::HashMap<String, String>,
) -> Option<String> {
    if let Some(v) = ca_bundle {
        let t = v.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    for var in CA_BUNDLE_ENV_VARS {
        if let Some(val) = env_map.get(*var) {
            let t = val.trim().to_string();
            if !t.is_empty() {
                return Some(t);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// resolve_httpx_verify — mirrors lines 22-63
// ---------------------------------------------------------------------------

/// Resolve `httpx` `verify` for provider HTTP clients.
///
/// Mirrors `resolve_httpx_verify(*, ca_bundle, ssl_verify, base_url) -> bool | ssl.SSLContext`
/// (lines 22-63):
/// ```python
/// def resolve_httpx_verify(*, ca_bundle=None, ssl_verify=None, base_url=""):
///     if _coerce_insecure(ssl_verify):
///         logger.warning("TLS certificate verification DISABLED ... for %s", base_url or "a custom provider endpoint")
///         return False
///     effective_ca = ((ca_bundle or "").strip() or os.getenv(...) ...)
///     if effective_ca:
///         ca_path = str(Path(effective_ca).expanduser())
///         if os.path.isfile(ca_path):
///             return ssl.create_default_context(cafile=ca_path)
///         logger.warning("CA bundle path does not exist: %s — falling back to default certificates", effective_ca)
///     return True
/// ```
pub fn resolve_httpx_verify(
    ca_bundle: Option<&str>,
    ssl_verify: Option<&Value>,
    base_url: &str,
) -> HttpxVerify {
    // Mirrors `if _coerce_insecure(ssl_verify): logger.warning(...); return False` (lines 39-46)
    if coerce_insecure(ssl_verify) {
        log::warn!(
            "TLS certificate verification DISABLED (ssl_verify: false) for {} — this is intended for local development only and is unsafe on any network you do not fully control.",
            if base_url.is_empty() { "a custom provider endpoint" } else { base_url }
        );
        return HttpxVerify::Disabled;
    }

    // Mirrors `effective_ca = (...)` chain (lines 48-54)
    let effective = effective_ca_bundle(ca_bundle);

    // Mirrors `if effective_ca: ca_path = str(Path(effective_ca).expanduser()); if os.path.isfile(ca_path): return SSLContext; else warn` (lines 55-62)
    if let Some(effective_ca) = effective {
        let ca_path_str = expand_user(&effective_ca);
        let ca_path = Path::new(&ca_path_str);
        if ca_path.is_file() {
            // Python validates by `ssl.create_default_context(cafile=)` which would also
            // raise on unreadable/corrupt bundles; here we preserve the existence check
            // (the `get_ca_certs` / file-size checks live in `ssl_guard.py`, not here).
            return HttpxVerify::CaFile(PathBuf::from(ca_path_str));
        }
        log::warn!(
            "CA bundle path does not exist: {} — falling back to default certificates",
            effective_ca
        );
    }

    // Mirrors `return True` (line 63)
    HttpxVerify::Default
}

/// Testable variant with explicit env map (for hermetic tests that monkeypatch env).
pub fn resolve_httpx_verify_with_env(
    ca_bundle: Option<&str>,
    ssl_verify: Option<&Value>,
    base_url: &str,
    env_map: &std::collections::HashMap<String, String>,
) -> HttpxVerify {
    if coerce_insecure(ssl_verify) {
        log::warn!(
            "TLS certificate verification DISABLED (ssl_verify: false) for {} — this is intended for local development only and is unsafe on any network you do not fully control.",
            if base_url.is_empty() { "a custom provider endpoint" } else { base_url }
        );
        return HttpxVerify::Disabled;
    }
    let effective = effective_ca_bundle_with_env(ca_bundle, env_map);
    if let Some(effective_ca) = effective {
        let ca_path_str = expand_user(&effective_ca);
        let ca_path = Path::new(&ca_path_str);
        if ca_path.is_file() {
            return HttpxVerify::CaFile(PathBuf::from(ca_path_str));
        }
        log::warn!(
            "CA bundle path does not exist: {} — falling back to default certificates",
            effective_ca
        );
    }
    HttpxVerify::Default
}

#[allow(dead_code)]
fn _resolve_httpx_verify(
    ca_bundle: Option<&str>,
    ssl_verify: Option<&Value>,
    base_url: &str,
) -> HttpxVerify {
    resolve_httpx_verify(ca_bundle, ssl_verify, base_url)
}

#[allow(dead_code)]
fn _resolve_httpx_verify_with_env(
    ca_bundle: Option<&str>,
    ssl_verify: Option<&Value>,
    base_url: &str,
    env_map: &std::collections::HashMap<String, String>,
) -> HttpxVerify {
    resolve_httpx_verify_with_env(ca_bundle, ssl_verify, base_url, env_map)
}
