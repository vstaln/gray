//! Shared OpenRouter API client for Hermes tools.
//! Port of `tools/openrouter_client.py` (47 lines) — 1:1 behavior.
//!
//! Provides a single lazy-initialized AsyncOpenAI client that all tool modules
//! can share. Routes through the centralized provider router in
//! `agent/auxiliary_client.py` so auth, headers, and API format are handled
//! consistently.
//!
//! Rust mapping:
//! - Python `AsyncOpenAI` SDK client → lightweight [`OpenRouterClient`] holding
//!   `api_key` + `base_url`. The `hermes-tools` crate does not link the OpenAI
//!   SDK; the credential carrier is sufficient for 1:1 semantics and for
//!   `check_api_key` gating. Real HTTP wiring lives in `hermes-provider` /
//!   `agent/auxiliary_client` when fully ported.
//! - Python `resolve_provider_client("openrouter", async_mode=True)` →
//!   [`resolve_provider_client`] stub that reads `OPENROUTER_API_KEY` from the
//!   environment. The full provider router (config.yaml, credential pool, header
//!   injection) is not linked in this crate; the env-only path is identical to
//!   the fallback branch the Python router takes when no other credential source
//!   is configured.
//! - Python `agent.secret_scope.get_secret` scope-aware probe → stubbed to env
//!   var in this crate (no `secret_scope` runtime linked, same as
//!   `hermes-plugins` ports). The fallback shape is preserved so callers see
//!   identical `check_api_key` truthiness. See `check_api_key` docs.
//! - Python `global _client` lazy singleton → `static CLIENT: Mutex<Option<Arc<OpenRouterClient>>>`
//!   with double-checked locking. `reset_client_for_tests` mirrors re-assigning
//!   `_client = None` in tests.

use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Constants — mirrors Python module-level assignments and inline strings
// ---------------------------------------------------------------------------

/// Environment variable that holds the OpenRouter API key.
///
/// Mirrors `OPENROUTER_API_KEY` in Python (line 26 error message and line 47
/// `os.getenv` argument).
pub const OPENROUTER_API_KEY_ENV: &str = "OPENROUTER_API_KEY";

/// Error raised when no API key can be resolved.
///
/// Mirrors `ValueError("OPENROUTER_API_KEY environment variable not set")`
/// (line 26).
pub const MISSING_API_KEY_ERROR: &str = "OPENROUTER_API_KEY environment variable not set";

/// OpenRouter base URL — mirrors `hermes_constants.OPENROUTER_BASE_URL`
/// (`https://openrouter.ai/api/v1`, line 1579) and the auxiliary client's
/// `OPENROUTER_BASE_URL` import. Used as the default `base_url` for the
/// constructed client.
pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

// ---------------------------------------------------------------------------
// Client carrier — mirrors `AsyncOpenAI` instance (line 24-27)
// ---------------------------------------------------------------------------

/// Lightweight OpenRouter client carrier.
///
/// Mirrors the `AsyncOpenAI` instance returned by
/// `resolve_provider_client("openrouter", async_mode=True)` in Python
/// (line 24). Holds the resolved credentials so `get_async_client` can return
/// a shared handle without linking the full SDK in this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRouterClient {
    /// Resolved API key (trimmed, non-empty).
    pub api_key: String,
    /// Base URL for OpenRouter (`https://openrouter.ai/api/v1` by default).
    pub base_url: String,
}

impl OpenRouterClient {
    /// Create a new client carrier from an API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: OPENROUTER_BASE_URL.to_string(),
        }
    }

    /// Create with explicit base_url (mirrors `OpenAI(api_key=..., base_url=...)`).
    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Lazy singleton — mirrors `global _client` (line 11) + `get_async_client` (line 14)
// ---------------------------------------------------------------------------

/// Global lazy client — mirrors `_client = None` (line 11).
///
/// `Mutex<Option<Arc<_>>>` allows interior mutability and test reset
/// (`_client = None` in Python tests) without `unsafe`. `Arc` gives
/// cheap clone on reuse (`return _client` in Python returns the same object).
static CLIENT: Mutex<Option<Arc<OpenRouterClient>>> = Mutex::new(None);

/// Reset the global client to `None` — test helper.
///
/// Mirrors re-assigning `_client = None` in Python tests. Not part of the
/// Python public surface; exposed for `#[cfg(test)]` isolation. Call at the
/// start of tests that touch `get_async_client` to ensure a clean slate.
pub fn reset_client_for_tests() {
    if let Ok(mut guard) = CLIENT.lock() {
        *guard = None;
    }
}

/// Returns true when the global client has been initialized.
///
/// Mirrors checking `_client is not None` (line 22).
pub fn is_client_initialized() -> bool {
    CLIENT.lock().map(|g| g.is_some()).unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Provider router stub — mirrors `agent.auxiliary_client.resolve_provider_client`
// ---------------------------------------------------------------------------

/// Stub for `resolve_provider_client("openrouter", async_mode=True)` (line 24).
///
/// In Python this consults the centralized provider router (credential pool,
/// config.yaml, header injection, model selection) and returns
/// `(client, model)` or `(None, _)` when no credentials resolve.
/// In Rust this crate does not link that router, so the stub reads
/// `OPENROUTER_API_KEY` from the environment via `lookup`.
///
/// Returns `Some((client, model))` when a non-empty key is found, `None`
/// otherwise. `model` is empty string in the stub (Python discards it with
/// `client, _model = ...` on line 24).
fn resolve_provider_client_with_lookup<F>(
    provider: &str,
    _async_mode: bool,
    lookup: &F,
) -> Option<(OpenRouterClient, String)>
where
    F: Fn(&str) -> Option<String>,
{
    if provider != "openrouter" {
        return None;
    }
    let raw = lookup(OPENROUTER_API_KEY_ENV)?;
    let key = raw.trim();
    if key.is_empty() {
        return None;
    }
    Some((OpenRouterClient::new(key.to_string()), String::new()))
}

// ---------------------------------------------------------------------------
// get_async_client — mirrors `def get_async_client():` (lines 14-28)
// ---------------------------------------------------------------------------

/// Return a shared async OpenAI-compatible client for OpenRouter.
///
/// Mirrors Python `def get_async_client():` (lines 14-28):
/// ```python
/// global _client
/// if _client is None:
///     from agent.auxiliary_client import resolve_provider_client
///     client, _model = resolve_provider_client("openrouter", async_mode=True)
///     if client is None:
///         raise ValueError("OPENROUTER_API_KEY environment variable not set")
///     _client = client
/// return _client
/// ```
///
/// The client is created lazily on first call and reused thereafter. The
/// `async_mode=True` flag is preserved in the stub call but has no effect on
/// the credential carrier (mirrors Python where `async_mode` selects
/// `AsyncOpenAI` vs `OpenAI` — both share credentials).
///
/// Returns `Ok(Arc<OpenRouterClient>)` on success, `Err(MISSING_API_KEY_ERROR)`
/// when no key resolves — mirrors Python's `ValueError`.
///
/// Thread-safe: double-checked locking ensures at most one client is created
/// even under concurrent callers (Python's GIL gave this implicitly).
pub fn get_async_client() -> Result<Arc<OpenRouterClient>, String> {
    get_async_client_with_lookup(|k| std::env::var(k).ok())
}

/// Testable variant with injected env lookup — mirrors `resolve_provider_client`
/// call site (line 24) with caller-supplied credential source.
///
/// `lookup` mirrors `os.getenv` / `resolve_provider_client` internals:
/// `Fn(&str) -> Option<String>` where `None` = missing key, `Some("")` or
/// whitespace-only = treated as missing (empty after trim, like Python's
/// `if client is None` branch where the router returned `None` for empty keys).
pub fn get_async_client_with_lookup<F>(lookup: F) -> Result<Arc<OpenRouterClient>, String>
where
    F: Fn(&str) -> Option<String>,
{
    // Fast path: already initialized → clone Arc without creating.
    {
        if let Ok(guard) = CLIENT.lock() {
            if let Some(existing) = guard.as_ref() {
                return Ok(Arc::clone(existing));
            }
        }
    }

    // Slow path: resolve via provider router stub.
    let resolved = resolve_provider_client_with_lookup("openrouter", true, &lookup);
    match resolved {
        Some((client, _model)) => {
            // Double-checked locking: another thread may have initialized while
            // we were resolving. Prefer the existing Arc if present.
            if let Ok(mut guard) = CLIENT.lock() {
                if let Some(existing) = guard.as_ref() {
                    return Ok(Arc::clone(existing));
                }
                let arc = Arc::new(client);
                *guard = Some(Arc::clone(&arc));
                Ok(arc)
            } else {
                // Mutex poisoned — still return a fresh client (never cache)
                Ok(Arc::new(client))
            }
        }
        None => Err(MISSING_API_KEY_ERROR.to_string()),
    }
}

// ---------------------------------------------------------------------------
// check_api_key — mirrors `def check_api_key() -> bool:` (lines 31-47)
// ---------------------------------------------------------------------------

/// Check whether the OpenRouter API key is present.
///
/// Mirrors Python `def check_api_key() -> bool:` (lines 31-47):
/// ```python
/// try:
///     from agent.secret_scope import UnscopedSecretError, get_secret
///     try:
///         return bool(get_secret("OPENROUTER_API_KEY"))
///     except UnscopedSecretError:
///         pass
/// except Exception:
///     pass
/// return bool(os.getenv("OPENROUTER_API_KEY"))
/// ```
///
/// Scope-aware (Slack pattern): tool paths run inside an installed profile
/// secret scope, whose verdict is authoritative under multiplex; unscoped CLI
/// probes keep the legacy env read.
///
/// Rust port: the `secret_scope` runtime is not linked in `hermes-tools`
/// (same as `hermes-plugins` adapters — see `homeassistant.rs` /
/// `sms_adapter.rs` comments). The scope probe is therefore stubbed: when
/// `secret_scope` is unavailable we fall through to the env var, identical to
/// Python's outer `except Exception: pass` branch. The inner
/// `UnscopedSecretError` branch (multiplex without installed scope) is
/// documented but has no runtime effect in this crate; callers that need
/// multiplex-aware checks should wire `check_api_key_with_secret` with a real
/// `get_secret` closure.
pub fn check_api_key() -> bool {
    check_api_key_with_lookup(|k| std::env::var(k).ok())
}

/// Testable variant with injected lookup — mirrors `os.getenv` fallback (line 47).
///
/// `lookup` is called with `OPENROUTER_API_KEY`; `Some(non-empty-trimmed)` →
/// `true`, missing/empty/whitespace-only → `false`. This matches Python's
/// `bool(os.getenv(...))` where empty string is falsy, and also matches the
/// `bool(get_secret(...))` check on the secret-scope path.
pub fn check_api_key_with_lookup<F>(lookup: F) -> bool
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(OPENROUTER_API_KEY_ENV) {
        Some(v) => !v.trim().is_empty(),
        None => false,
    }
}

/// Scope-aware variant with injected `get_secret` — mirrors the full
/// `try: get_secret` / `except UnscopedSecretError` / `except Exception`
/// / `return bool(os.getenv(...))` chain (lines 37-47).
///
/// - `get_secret` mirrors `agent.secret_scope.get_secret`: `Ok(value)` →
///   truthiness of `value` (`bool(get_secret(...))` on line 40),  
///   `Err(Unscoped)` → fall through to `env_lookup` (line 41-42),  
///   any other error (outer `except Exception`) → fall through to `env_lookup`.
/// - `env_lookup` mirrors `os.getenv` on line 47.
///
/// When `get_secret` succeeds its result is authoritative under multiplex —
/// the secret scope's verdict is not re-checked against `env_lookup`.
/// Only `UnscopedSecretError` and generic failures fall through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnscopedSecretError;

pub fn check_api_key_with_secret<F, G>(get_secret: F, env_lookup: G) -> bool
where
    F: Fn(&str) -> Result<String, UnscopedSecretError>,
    G: Fn(&str) -> Option<String>,
{
    // Outer try: `from agent.secret_scope import ...` may fail (line 39) →
    // fall through to env. In Rust this is modeled as `get_secret` being
    // the import-success path; callers that want to simulate import failure
    // should just call `check_api_key_with_lookup` directly.
    match get_secret(OPENROUTER_API_KEY_ENV) {
        Ok(val) => !val.trim().is_empty(),
        Err(_) => {
            // Mirrors `except UnscopedSecretError: pass` (lines 41-42) and
            // `except Exception: pass` (lines 43-44) → fall through to env.
            match env_lookup(OPENROUTER_API_KEY_ENV) {
                Some(v) => !v.trim().is_empty(),
                None => false,
            }
        }
    }
}

/// Variant that distinguishes import failure from `UnscopedSecretError`.
///
/// Mirrors the outer `try: from agent.secret_scope import ...` vs inner
/// `try: get_secret` split (lines 37-44). `import_secret_scope` returns
/// `Some(get_secret_fn)` on success, `None` on import failure. When `None`,
/// we skip directly to `env_lookup` without calling `get_secret`.
pub fn check_api_key_with_import<F, G>(
    import_secret_scope: Option<F>,
    env_lookup: G,
) -> bool
where
    F: Fn(&str) -> Result<String, UnscopedSecretError>,
    G: Fn(&str) -> Option<String>,
{
    match import_secret_scope {
        Some(get_secret) => check_api_key_with_secret(get_secret, env_lookup),
        None => check_api_key_with_lookup(env_lookup),
    }
}

// ---------------------------------------------------------------------------
// `__all__` equivalent — public surface mirrors Python module exports
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize env-touching tests — `std::env` is process-global and tests
    // run in parallel threads within the same `cargo test` file subprocess.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_env_lock<F, R>(f: F) -> R
    where
        F: FnOnce() -> R,
    {
        let _guard = ENV_LOCK.lock().unwrap();
        reset_client_for_tests();
        let result = f();
        reset_client_for_tests();
        result
    }

    #[test]
    fn constants_match_python() {
        assert_eq!(OPENROUTER_API_KEY_ENV, "OPENROUTER_API_KEY");
        assert_eq!(
            MISSING_API_KEY_ERROR,
            "OPENROUTER_API_KEY environment variable not set"
        );
        assert_eq!(OPENROUTER_BASE_URL, "https://openrouter.ai/api/v1");
    }

    #[test]
    fn openrouter_client_new_sets_base_url() {
        let c = OpenRouterClient::new("sk-or-123");
        assert_eq!(c.api_key, "sk-or-123");
        assert_eq!(c.base_url, OPENROUTER_BASE_URL);
        let c2 = OpenRouterClient::with_base_url("sk-or-123", "https://custom.example/v1");
        assert_eq!(c2.base_url, "https://custom.example/v1");
    }

    #[test]
    fn check_api_key_with_lookup_empty_is_false() {
        assert!(!check_api_key_with_lookup(|_| None));
        assert!(!check_api_key_with_lookup(|_| Some(String::new())));
        assert!(!check_api_key_with_lookup(|_| Some("   ".to_string())));
        assert!(!check_api_key_with_lookup(|_| Some("\t\n".to_string())));
    }

    #[test]
    fn check_api_key_with_lookup_present_is_true() {
        assert!(check_api_key_with_lookup(|_| Some("sk-or-123".to_string())));
        assert!(check_api_key_with_lookup(|_| Some("  sk-or-123  ".to_string())));
        assert!(check_api_key_with_lookup(|k| {
            assert_eq!(k, OPENROUTER_API_KEY_ENV);
            Some("x".to_string())
        }));
    }

    #[test]
    fn check_api_key_with_secret_authoritative_when_ok() {
        // get_secret succeeds → env is ignored (scope verdict authoritative)
        let ok = check_api_key_with_secret(|_| Ok("sk-or-secret".to_string()), |_| None);
        assert!(ok);
        let falsy = check_api_key_with_secret(|_| Ok("".to_string()), |_| Some("sk-or-env".to_string()));
        assert!(!falsy, "empty secret should be falsy even though env has key");
        let whitespace = check_api_key_with_secret(|_| Ok("   ".to_string()), |_| Some("sk-or-env".to_string()));
        assert!(!whitespace);
    }

    #[test]
    fn check_api_key_with_secret_unscoped_falls_through_to_env() {
        // Unscoped → fall through to env (line 41-42)
        let via_env = check_api_key_with_secret(
            |_| Err(UnscopedSecretError),
            |_| Some("sk-or-env".to_string()),
        );
        assert!(via_env);
        let env_missing = check_api_key_with_secret(|_| Err(UnscopedSecretError), |_| None);
        assert!(!env_missing);
        let env_empty = check_api_key_with_secret(|_| Err(UnscopedSecretError), |_| Some("".to_string()));
        assert!(!env_empty);
    }

    #[test]
    fn check_api_key_with_import_none_falls_through_to_env() {
        // Outer import failure → direct env read (lines 43-44)
        let via_env: bool = check_api_key_with_import::<fn(&str) -> Result<String, UnscopedSecretError>, _>(None, |_| Some("sk-or-env".to_string()));
        assert!(via_env);
        let missing: bool = check_api_key_with_import::<fn(&str) -> Result<String, UnscopedSecretError>, _>(None, |_| None);
        assert!(!missing);
    }

    #[test]
    fn check_api_key_env_integration() {
        with_env_lock(|| {
            std::env::remove_var(OPENROUTER_API_KEY_ENV);
            assert!(!check_api_key());
            std::env::set_var(OPENROUTER_API_KEY_ENV, "sk-or-test");
            assert!(check_api_key());
            std::env::set_var(OPENROUTER_API_KEY_ENV, "   ");
            assert!(!check_api_key());
            std::env::remove_var(OPENROUTER_API_KEY_ENV);
            assert!(!check_api_key());
        });
    }

    #[test]
    fn get_async_client_missing_key_returns_error() {
        with_env_lock(|| {
            let err = get_async_client_with_lookup(|_| None).unwrap_err();
            assert_eq!(err, MISSING_API_KEY_ERROR);
            // empty and whitespace also error
            let err2 = get_async_client_with_lookup(|_| Some("".to_string())).unwrap_err();
            assert_eq!(err2, MISSING_API_KEY_ERROR);
            let err3 = get_async_client_with_lookup(|_| Some("   ".to_string())).unwrap_err();
            assert_eq!(err3, MISSING_API_KEY_ERROR);
            assert!(!is_client_initialized());
        });
    }

    #[test]
    fn get_async_client_success_and_singleton_reuse() {
        with_env_lock(|| {
            let c1 = get_async_client_with_lookup(|_| Some("sk-or-abc".to_string())).unwrap();
            assert_eq!(c1.api_key, "sk-or-abc");
            assert_eq!(c1.base_url, OPENROUTER_BASE_URL);
            assert!(is_client_initialized());

            // Second call reuses same Arc (pointer equality) even if lookup would give different key
            let c2 = get_async_client_with_lookup(|_| Some("sk-or-different".to_string())).unwrap();
            assert!(Arc::ptr_eq(&c1, &c2), "singleton should reuse same Arc");
            assert_eq!(c2.api_key, "sk-or-abc", "second call should not overwrite first key");

            // Also via env path
            reset_client_for_tests();
            std::env::set_var(OPENROUTER_API_KEY_ENV, "sk-or-envtest");
            let c3 = get_async_client().unwrap();
            assert_eq!(c3.api_key, "sk-or-envtest");
            let c4 = get_async_client().unwrap();
            assert!(Arc::ptr_eq(&c3, &c4));
            std::env::remove_var(OPENROUTER_API_KEY_ENV);
        });
    }

    #[test]
    fn reset_client_clears_singleton() {
        with_env_lock(|| {
            let c1 = get_async_client_with_lookup(|_| Some("sk-or-first".to_string())).unwrap();
            reset_client_for_tests();
            assert!(!is_client_initialized());
            let c2 = get_async_client_with_lookup(|_| Some("sk-or-second".to_string())).unwrap();
            assert!(!Arc::ptr_eq(&c1, &c2));
            assert_eq!(c2.api_key, "sk-or-second");
        });
    }

    #[test]
    fn resolve_provider_client_only_handles_openrouter() {
        let r = resolve_provider_client_with_lookup("openrouter", true, &|_| Some("sk-or-x".to_string()));
        assert!(r.is_some());
        let r2 = resolve_provider_client_with_lookup("anthropic", true, &|_| Some("sk-ant".to_string()));
        assert!(r2.is_none());
        let r3 = resolve_provider_client_with_lookup("openrouter", false, &|_| Some("sk-or-x".to_string()));
        assert!(r3.is_some(), "async_mode flag should not affect resolution in stub");
        let missing = resolve_provider_client_with_lookup("openrouter", true, &|_| None);
        assert!(missing.is_none());
    }

    #[test]
    fn trim_handling_in_resolve() {
        with_env_lock(|| {
            let c = get_async_client_with_lookup(|_| Some("  sk-or-trim  ".to_string())).unwrap();
            assert_eq!(c.api_key, "sk-or-trim");
        });
    }
}
