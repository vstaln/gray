//! Shared HTTP client factory for long-lived platform adapters.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/platforms/_http_client_limits.py` (84 LOC).
//!
//! Python source docstring (preserved):
//! ```text
//! Shared HTTP client factory for long-lived platform adapters.
//!
//! Gateway messaging platforms (QQ Bot, Feishu, WeCom, DingTalk, Signal,
//! BlueBubbles, WeCom-callback) keep a persistent ``httpx.AsyncClient``
//! alive for the adapter's lifetime.  That amortises TLS/connection setup
//! across many API calls, but it also means the process's file-descriptor
//! pressure is sensitive to how aggressively the pool recycles idle keep-
//! alive connections.
//!
//! httpx's default ``keepalive_expiry`` is 5 seconds.  On macOS behind
//! Cloudflare Warp (and other transparent proxies), peer-initiated FIN can
//! sit in ``CLOSE_WAIT`` longer than that before the local socket actually
//! drains — which, multiplied across 7 long-lived adapters plus the LLM
//! client and MCP clients, walks straight into the default 256 fd limit.
//! See #18451.
//!
//! ``platform_httpx_limits()`` returns a tighter ``httpx.Limits`` the
//! adapter factories use instead of the httpx default.  The values chosen:
//!
//! * ``max_keepalive_connections=10`` — plenty for any single adapter;
//!   platform APIs rarely parallelise beyond this.
//! * ``keepalive_expiry=2.0`` — close idle sockets aggressively so a
//!   proxy's lingering CLOSE_WAIT window can't starve the process.
//!
//! Override via ``HERMES_GATEWAY_HTTPX_KEEPALIVE_EXPIRY`` /
//! ``HERMES_GATEWAY_HTTPX_MAX_KEEPALIVE`` env vars when tuning under load.
//! ```
//!
//! Mapping:
//! - `_DEFAULT_KEEPALIVE_EXPIRY_S = 2.0` → [`_DEFAULT_KEEPALIVE_EXPIRY_S`] / [`DEFAULT_KEEPALIVE_EXPIRY_S`]
//! - `_DEFAULT_MAX_KEEPALIVE = 10` → [`_DEFAULT_MAX_KEEPALIVE`] / [`DEFAULT_MAX_KEEPALIVE`]
//! - `HERMES_GATEWAY_HTTPX_KEEPALIVE_EXPIRY` → [`KEEPALIVE_EXPIRY_ENV`] / [`_KEEPALIVE_EXPIRY_ENV`]
//! - `HERMES_GATEWAY_HTTPX_MAX_KEEPALIVE` → [`MAX_KEEPALIVE_ENV`] / [`_MAX_KEEPALIVE_ENV`]
//! - `def _env_float(name, default)` → [`_env_float`]
//! - `def _env_int(name, default)` → [`_env_int`]
//! - `def platform_httpx_limits() -> httpx.Limits | None` → [`platform_httpx_limits`] / [`platform_httpx_limits_opt`]
//! - `httpx.Limits(max_keepalive_connections=..., keepalive_expiry=...)` → [`HttpClientLimits`] / [`HttpxLimits`]
//! - `httpx is None` guard → always `Some` in Rust (no optional `httpx` dep); [`platform_httpx_limits_opt`] preserves the `Option` shape
//! - `max_connections` left at httpx default (100) → [`DEFAULT_MAX_CONNECTIONS`] (not set on the wire, same headroom)

// ---------------------------------------------------------------------------
// Constants — mirrors Python module-level constants
// ---------------------------------------------------------------------------

/// Default keepalive expiry in seconds. Mirrors `_DEFAULT_KEEPALIVE_EXPIRY_S = 2.0`.
pub const _DEFAULT_KEEPALIVE_EXPIRY_S: f64 = 2.0;
/// Public alias.
pub const DEFAULT_KEEPALIVE_EXPIRY_S: f64 = _DEFAULT_KEEPALIVE_EXPIRY_S;

/// Default max keepalive connections. Mirrors `_DEFAULT_MAX_KEEPALIVE = 10`.
pub const _DEFAULT_MAX_KEEPALIVE: u32 = 10;
/// Public alias.
pub const DEFAULT_MAX_KEEPALIVE: u32 = _DEFAULT_MAX_KEEPALIVE;

/// httpx default `max_connections` (100) — left untouched, same headroom.
/// Python comment: `# Leave max_connections at httpx default (100) — plenty of headroom.`
pub const DEFAULT_MAX_CONNECTIONS: u32 = 100;

/// Env var for keepalive expiry override. Mirrors `HERMES_GATEWAY_HTTPX_KEEPALIVE_EXPIRY`.
pub const _KEEPALIVE_EXPIRY_ENV: &str = "HERMES_GATEWAY_HTTPX_KEEPALIVE_EXPIRY";
/// Public alias.
pub const KEEPALIVE_EXPIRY_ENV: &str = _KEEPALIVE_EXPIRY_ENV;

/// Env var for max keepalive override. Mirrors `HERMES_GATEWAY_HTTPX_MAX_KEEPALIVE`.
pub const _MAX_KEEPALIVE_ENV: &str = "HERMES_GATEWAY_HTTPX_MAX_KEEPALIVE";
/// Public alias.
pub const MAX_KEEPALIVE_ENV: &str = _MAX_KEEPALIVE_ENV;

// ---------------------------------------------------------------------------
// HttpClientLimits — mirrors `httpx.Limits`
// ---------------------------------------------------------------------------

/// Tight `httpx.Limits` for persistent platform-adapter clients.
///
/// Mirrors `httpx.Limits(max_keepalive_connections=..., keepalive_expiry=...)`
/// where `max_connections` is left at the httpx default (100).
#[derive(Debug, Clone, PartialEq)]
pub struct HttpClientLimits {
    /// Mirrors `max_keepalive_connections`.
    pub max_keepalive_connections: u32,
    /// Mirrors `keepalive_expiry` (seconds).
    pub keepalive_expiry: f64,
    /// Mirrors `max_connections` (left at httpx default 100; stored for completeness).
    pub max_connections: u32,
}

/// Backwards-compatible alias — some callers may expect `HttpxLimits`.
pub type HttpxLimits = HttpClientLimits;

impl HttpClientLimits {
    /// Create with explicit values (mirrors `httpx.Limits(...)` constructor).
    pub fn new(max_keepalive_connections: u32, keepalive_expiry: f64) -> Self {
        Self {
            max_keepalive_connections,
            keepalive_expiry,
            max_connections: DEFAULT_MAX_CONNECTIONS,
        }
    }
}

impl Default for HttpClientLimits {
    fn default() -> Self {
        Self {
            max_keepalive_connections: DEFAULT_MAX_KEEPALIVE,
            keepalive_expiry: DEFAULT_KEEPALIVE_EXPIRY_S,
            max_connections: DEFAULT_MAX_CONNECTIONS,
        }
    }
}

// ---------------------------------------------------------------------------
// Env helpers — mirrors Python `def _env_float` / `def _env_int`
// ---------------------------------------------------------------------------

/// Parse `name` from env as `f64 > 0`, else `default`.
///
/// Mirrors:
/// ```python
/// def _env_float(name: str, default: float) -> float:
///     raw = os.environ.get(name, "").strip()
///     if not raw:
///         return default
///     try:
///         val = float(raw)
///     except (TypeError, ValueError):
///         return default
///     return val if val > 0 else default
/// ```
pub fn _env_float(name: &str, default: f64) -> f64 {
    let raw = std::env::var(name).unwrap_or_default();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return default;
    }
    let val: f64 = match trimmed.parse() {
        Ok(v) => v,
        Err(_) => return default,
    };
    if val > 0.0 && val.is_finite() {
        val
    } else {
        default
    }
}

/// Parse `name` from env as `int > 0`, else `default`.
///
/// Mirrors:
/// ```python
/// def _env_int(name: str, default: int) -> int:
///     raw = os.environ.get(name, "").strip()
///     if not raw:
///         return default
///     try:
///         val = int(raw)
///     except (TypeError, ValueError):
///         return default
///     return val if val > 0 else default
/// ```
pub fn _env_int(name: &str, default: i64) -> i64 {
    let raw = std::env::var(name).unwrap_or_default();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return default;
    }
    let val: i64 = match trimmed.parse() {
        Ok(v) => v,
        Err(_) => return default,
    };
    if val > 0 {
        val
    } else {
        default
    }
}

/// Typed variant returning `u32` (clamped to `u32::MAX` on overflow, `>0` guard same).
pub fn _env_int_u32(name: &str, default: u32) -> u32 {
    let v = _env_int(name, default as i64);
    if v < 0 {
        return default;
    }
    // Python int is unbounded; clamp to u32 range for Rust.
    if v > u32::MAX as i64 {
        default
    } else {
        v as u32
    }
}

// ---------------------------------------------------------------------------
// platform_httpx_limits — mirrors Python
// ---------------------------------------------------------------------------

/// Return `HttpClientLimits` tuned for persistent platform-adapter clients.
///
/// Mirrors:
/// ```python
/// def platform_httpx_limits() -> "httpx.Limits | None":
///     if httpx is None:
///         return None
///     keepalive_expiry = _env_float("HERMES_GATEWAY_HTTPX_KEEPALIVE_EXPIRY", _DEFAULT_KEEPALIVE_EXPIRY_S)
///     max_keepalive = _env_int("HERMES_GATEWAY_HTTPX_MAX_KEEPALIVE", _DEFAULT_MAX_KEEPALIVE)
///     return httpx.Limits(
///         max_keepalive_connections=max_keepalive,
///         keepalive_expiry=keepalive_expiry,
///     )
/// ```
///
/// In Rust `httpx` is always available (no optional import), so this never
/// returns `None`. Use [`platform_httpx_limits_opt`] if you need the `Option`
/// shape for 1:1 grep-ability.
pub fn platform_httpx_limits() -> HttpClientLimits {
    let keepalive_expiry = _env_float(
        _KEEPALIVE_EXPIRY_ENV,
        _DEFAULT_KEEPALIVE_EXPIRY_S,
    );
    let max_keepalive = _env_int_u32(
        _MAX_KEEPALIVE_ENV,
        _DEFAULT_MAX_KEEPALIVE,
    );
    HttpClientLimits::new(max_keepalive, keepalive_expiry)
}

/// `Option`-returning wrapper preserving Python's `httpx.Limits | None` shape.
///
/// Always `Some` in Rust (no missing `httpx` dep); `None` would only occur
/// if the Python helper hit `httpx is None`.
pub fn platform_httpx_limits_opt() -> Option<HttpClientLimits> {
    Some(platform_httpx_limits())
}

// Private aliases for grep-ability (Python underscore names).
#[allow(dead_code)]
fn _platform_httpx_limits() -> HttpClientLimits {
    platform_httpx_limits()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn with_env_lock<F: FnOnce()>(f: F) {
        f()
    }

    #[test]
    fn defaults_when_env_missing() {
        with_env_lock(|| {
            let _ = env::remove_var(_KEEPALIVE_EXPIRY_ENV);
            let _ = env::remove_var(_MAX_KEEPALIVE_ENV);
            let lim = platform_httpx_limits();
            assert_eq!(lim.max_keepalive_connections, _DEFAULT_MAX_KEEPALIVE);
            assert!((lim.keepalive_expiry - _DEFAULT_KEEPALIVE_EXPIRY_S).abs() < f64::EPSILON);
            assert_eq!(lim.max_connections, DEFAULT_MAX_CONNECTIONS);
        });
    }

    #[test]
    fn env_override_float_and_int() {
        with_env_lock(|| {
            env::set_var(_KEEPALIVE_EXPIRY_ENV, "5.5");
            env::set_var(_MAX_KEEPALIVE_ENV, "20");
            let lim = platform_httpx_limits();
            assert_eq!(lim.max_keepalive_connections, 20);
            assert!((lim.keepalive_expiry - 5.5).abs() < f64::EPSILON);
            let _ = env::remove_var(_KEEPALIVE_EXPIRY_ENV);
            let _ = env::remove_var(_MAX_KEEPALIVE_ENV);
        });
    }

    #[test]
    fn env_trims_whitespace() {
        with_env_lock(|| {
            env::set_var(_KEEPALIVE_EXPIRY_ENV, "  3.0  ");
            env::set_var(_MAX_KEEPALIVE_ENV, "  15  ");
            let lim = platform_httpx_limits();
            assert!((lim.keepalive_expiry - 3.0).abs() < f64::EPSILON);
            assert_eq!(lim.max_keepalive_connections, 15);
            let _ = env::remove_var(_KEEPALIVE_EXPIRY_ENV);
            let _ = env::remove_var(_MAX_KEEPALIVE_ENV);
        });
    }

    #[test]
    fn env_zero_or_negative_falls_back() {
        with_env_lock(|| {
            env::set_var(_KEEPALIVE_EXPIRY_ENV, "0");
            env::set_var(_MAX_KEEPALIVE_ENV, "-5");
            let lim = platform_httpx_limits();
            assert_eq!(lim.max_keepalive_connections, _DEFAULT_MAX_KEEPALIVE);
            assert!((lim.keepalive_expiry - _DEFAULT_KEEPALIVE_EXPIRY_S).abs() < f64::EPSILON);
            env::set_var(_KEEPALIVE_EXPIRY_ENV, "-1.0");
            env::set_var(_MAX_KEEPALIVE_ENV, "0");
            let lim2 = platform_httpx_limits();
            assert_eq!(lim2.max_keepalive_connections, _DEFAULT_MAX_KEEPALIVE);
            assert!((lim2.keepalive_expiry - _DEFAULT_KEEPALIVE_EXPIRY_S).abs() < f64::EPSILON);
            let _ = env::remove_var(_KEEPALIVE_EXPIRY_ENV);
            let _ = env::remove_var(_MAX_KEEPALIVE_ENV);
        });
    }

    #[test]
    fn env_invalid_falls_back() {
        with_env_lock(|| {
            env::set_var(_KEEPALIVE_EXPIRY_ENV, "not-a-float");
            env::set_var(_MAX_KEEPALIVE_ENV, "not-an-int");
            let lim = platform_httpx_limits();
            assert_eq!(lim.max_keepalive_connections, _DEFAULT_MAX_KEEPALIVE);
            assert!((lim.keepalive_expiry - _DEFAULT_KEEPALIVE_EXPIRY_S).abs() < f64::EPSILON);
            let _ = env::remove_var(_KEEPALIVE_EXPIRY_ENV);
            let _ = env::remove_var(_MAX_KEEPALIVE_ENV);
        });
    }

    #[test]
    fn env_empty_falls_back() {
        with_env_lock(|| {
            env::set_var(_KEEPALIVE_EXPIRY_ENV, "");
            env::set_var(_MAX_KEEPALIVE_ENV, "   ");
            let lim = platform_httpx_limits();
            assert_eq!(lim.max_keepalive_connections, _DEFAULT_MAX_KEEPALIVE);
            assert!((lim.keepalive_expiry - _DEFAULT_KEEPALIVE_EXPIRY_S).abs() < f64::EPSILON);
            let _ = env::remove_var(_KEEPALIVE_EXPIRY_ENV);
            let _ = env::remove_var(_MAX_KEEPALIVE_ENV);
        });
    }

    #[test]
    fn opt_always_some() {
        with_env_lock(|| {
            let _ = env::remove_var(_KEEPALIVE_EXPIRY_ENV);
            let _ = env::remove_var(_MAX_KEEPALIVE_ENV);
            assert!(platform_httpx_limits_opt().is_some());
        });
    }

    #[test]
    fn helpers_directly() {
        with_env_lock(|| {
            env::set_var("TEST_FLOAT_X", "2.5");
            assert!((_env_float("TEST_FLOAT_X", 1.0) - 2.5).abs() < f64::EPSILON);
            env::set_var("TEST_FLOAT_X", "0");
            assert!((_env_float("TEST_FLOAT_X", 1.0) - 1.0).abs() < f64::EPSILON);
            env::remove_var("TEST_FLOAT_X");
            assert!((_env_float("TEST_FLOAT_X", 1.0) - 1.0).abs() < f64::EPSILON);

            env::set_var("TEST_INT_X", "7");
            assert_eq!(_env_int("TEST_INT_X", 10), 7);
            env::set_var("TEST_INT_X", "0");
            assert_eq!(_env_int("TEST_INT_X", 10), 10);
            env::remove_var("TEST_INT_X");
            assert_eq!(_env_int("TEST_INT_X", 10), 10);
        });
    }
}
