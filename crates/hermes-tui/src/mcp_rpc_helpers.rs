//! Shared helpers for the per-profile MCP lifecycle RPCs (mcp.servers.*).
//!
//! 1:1 port of `tui_gateway/mcp_rpc_helpers.py` (74 lines).
//!
//! These live in their own module (not methods_tools) because methods_tools
//! handlers are rebound onto ``tui_gateway.server``'s globals at install time
//! (see method_ctx.HandlerRegistry.install); a plain module-level def in
//! methods_tools would not be reachable from a rebound handler body. Handlers
//! import these at call time instead.
//!
//! ```python
//! # Python — tui_gateway/mcp_rpc_helpers.py
//! def resolve_profile(rid, params, err_fn) -> Tuple[Optional[Any], Optional[dict]]: ...
//! def reset_profile(token) -> None: ...
//! def summarize_server(name: str, cfg: dict) -> Dict[str, Any]: ...
//! ```
//!
//! # Rust mapping
//!
//! * `rid, params, err_fn` — `rid: &str`, `params: &HashMap<String, String>` (only
//!   `profile` is read; `str(params.get("profile") or "").strip()` is
//!   preserved), `err_fn: Fn(&str, i32, &str) -> String` (or `-> Value` as
//!   JSON string — callers choose). Returns `(Option<ProfileToken>, Option<String>)`
//!   where the second slot is the JSON-RPC error produced by `err_fn`.
//! * `get_profile_dir` / `set_hermes_home_override` — injected as closures so
//!   the helpers stay `std`-only and testable without touching `hermes_cli` or
//!   `hermes_constants`. The `is_dir()` check mirrors Python's `profile_dir.is_dir()`.
//! * `Token` from `hermes_constants.ContextVar[Token]` → [`ProfileToken`] opaque
//!   string wrapper (the real token is a `contextvars.Token`; the override path
//!   string is its observable payload — `reset` just needs identity).
//! * `reset_profile` — mirrors `if token is not None: try: reset(...) except Exception: pass`
//!   — the `try/except` is modelled as `catch_unwind` / `Result` ignore.
//! * `cfg: dict` → [`McpServerConfig`] typed struct; `cfg if isinstance(cfg, dict) else {}`
//!   is modelled as `Option<&McpServerConfig>` (`None` → empty). `headers`, `env`,
//!   `auth`, `url`/`command` truthiness, and `is not False` enabled check are
//!   preserved. `sorted(str(k) for k in env)` and `list(args or [])` are
//!   preserved via `BTreeMap`/`Vec` clones.
//! * `_oauth_tokens_present(name)` → injected `Fn(&str) -> bool` closure, only
//!   called when `auth == "oauth"` (mirrors the Python conditional).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Token — mirrors hermes_constants.Token + set_hermes_home_override return
// ---------------------------------------------------------------------------

/// Opaque reset token returned by `set_hermes_home_override`.
///
/// Python's `Token` is a `contextvars.Token`; its payload is the previous
/// `_HERMES_HOME_OVERRIDE` value. Here the token just carries the override
/// path string so `reset_profile` can thread it through — the real
/// `ContextVar.reset(token)` semantics are provided by the injected closure.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProfileToken(pub String);

impl ProfileToken {
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }
}

impl std::fmt::Display for ProfileToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ---------------------------------------------------------------------------
// resolve_profile / reset_profile
// ---------------------------------------------------------------------------

/// Resolve the optional `profile` param to a `HERMES_HOME` override token.
///
/// Returns `(token, error)`: `token` is `None` for the launch profile (no
/// override) or `Some(token)` with an opaque reset token; `error` is `Some`
/// JSON-RPC error string (built via `err_fn`) when the named profile doesn't
/// exist. Callers must reset `token` in a finally via [`reset_profile`].
///
/// Mirrors `tui_gateway/mcp_rpc_helpers.py::resolve_profile`:
///
/// ```python
/// def resolve_profile(rid, params, err_fn) -> Tuple[Optional[Any], Optional[dict]]:
///     profile = str(params.get("profile") or "").strip()
///     if not profile:
///         return None, None
///     from hermes_cli.profiles import get_profile_dir
///     from hermes_constants import set_hermes_home_override
///     profile_dir = get_profile_dir(profile)
///     if not profile_dir or not profile_dir.is_dir():
///         return None, err_fn(rid, 4064, f"profile '{profile}' not found")
///     return set_hermes_home_override(str(profile_dir)), None
/// ```
pub fn resolve_profile<F, G, E>(
    rid: &str,
    params: &HashMap<String, String>,
    get_profile_dir: F,
    set_hermes_home_override: G,
    err_fn: E,
) -> (Option<ProfileToken>, Option<String>)
where
    F: Fn(&str) -> Option<PathBuf>,
    G: Fn(&str) -> ProfileToken,
    E: Fn(&str, i32, &str) -> String,
{
    // Python: str(params.get("profile") or "").strip()
    let raw = params.get("profile").map(|s| s.as_str()).unwrap_or("");
    let profile = raw.trim().to_string();
    if profile.is_empty() {
        return (None, None);
    }
    let profile_dir = get_profile_dir(&profile);
    let is_dir = profile_dir
        .as_deref()
        .is_some_and(|p| p.is_dir());
    if profile_dir.is_none() || !is_dir {
        let msg = format!("profile '{}' not found", profile);
        let err = err_fn(rid, 4064, &msg);
        return (None, Some(err));
    }
    let dir = profile_dir.unwrap();
    // Python: str(profile_dir) — PathBuf to string
    let dir_str = dir.to_string_lossy().to_string();
    let token = set_hermes_home_override(&dir_str);
    (Some(token), None)
}

/// Convenience wrapper when `params` may carry non-string values.
///
/// Accepts `HashMap<String, serde_json::Value>`-like stringified values;
/// provided for call sites that already stringify the profile value.
pub fn resolve_profile_str_map<F, G, E>(
    rid: &str,
    profile_value: Option<&str>,
    get_profile_dir: F,
    set_hermes_home_override: G,
    err_fn: E,
) -> (Option<ProfileToken>, Option<String>)
where
    F: Fn(&str) -> Option<PathBuf>,
    G: Fn(&str) -> ProfileToken,
    E: Fn(&str, i32, &str) -> String,
{
    let mut params = HashMap::new();
    if let Some(v) = profile_value {
        params.insert("profile".to_string(), v.to_string());
    }
    resolve_profile(rid, &params, get_profile_dir, set_hermes_home_override, err_fn)
}

/// Reset a `HERMES_HOME` override token.
///
/// Mirrors `tui_gateway/mcp_rpc_helpers.py::reset_profile`:
///
/// ```python
/// def reset_profile(token) -> None:
///     if token is not None:
///         try:
///             from hermes_constants import reset_hermes_home_override
///             reset_hermes_home_override(token)
///         except Exception:
///             pass
/// ```
///
/// The import + `reset` are injected as `reset_fn`; any panic/error is
/// swallowed (mirrors `except Exception: pass` + `catch_unwind`).
pub fn reset_profile<F>(token: Option<ProfileToken>, reset_fn: F)
where
    F: Fn(ProfileToken),
{
    if let Some(t) = token {
        // Swallow unwinds/panics like Python's `except Exception: pass`
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| reset_fn(t)));
        let _ = res;
    }
}

/// Variant of [`reset_profile`] that takes `&Option<ProfileToken>` and a
/// fallible closure, ignoring any `Err`.
pub fn reset_profile_fallible<F, E>(token: Option<ProfileToken>, reset_fn: F)
where
    F: Fn(ProfileToken) -> Result<(), E>,
{
    if let Some(t) = token {
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| reset_fn(t)));
        let _ = res;
    }
}

// ---------------------------------------------------------------------------
// summarize_server
// ---------------------------------------------------------------------------

/// Typed mirror of an MCP server config dict.
///
/// Each field mirrors one `cfg.get(...)` site in `summarize_server`. Missing
/// keys become `None`; empty strings are treated as falsy for `url`/`command`
/// transport detection (Python's `if cfg.get("url")` branch).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpServerConfig {
    /// `cfg.get("url")`
    pub url: Option<String>,
    /// `cfg.get("command")`
    pub command: Option<String>,
    /// `cfg.get("args")` — `list(cfg.get("args") or [])`
    pub args: Option<Vec<String>>,
    /// `cfg.get("env")` — `sorted(str(k) for k in (cfg.get("env") or {}))`
    pub env: Option<BTreeMap<String, String>>,
    /// `cfg.get("auth")`
    pub auth: Option<String>,
    /// `cfg.get("headers")`
    pub headers: Option<BTreeMap<String, String>>,
    /// `cfg.get("enabled", True) is not False`
    pub enabled: Option<bool>,
    /// `cfg.get("tools")` — opaque; preserved verbatim.
    pub tools: Option<String>,
}

impl McpServerConfig {
    pub fn new() -> Self {
        Self::default()
    }
}

/// UI-facing summary of one MCP server — no secret values.
///
/// Mirrors `web_server._mcp_server_summary` plus `oauth_tokens_present`.
///
/// ```python
/// def summarize_server(name: str, cfg: dict) -> Dict[str, Any]:
///     cfg = cfg if isinstance(cfg, dict) else {}
///     transport = "http" if cfg.get("url") else ("stdio" if cfg.get("command") else "unknown")
///     auth = cfg.get("auth")
///     headers = cfg.get("headers") or {}
///     if not auth and isinstance(headers, dict) and any(str(key).lower() == "authorization" for key in headers):
///         auth = "header"
///     tokens_present = _oauth_tokens_present(name) if auth == "oauth" else None
///     return {"name": name, "transport": transport, "url": cfg.get("url"), "command": cfg.get("command"),
///             "args": list(cfg.get("args") or []), "env": sorted(str(k) for k in (cfg.get("env") or {})),
///             "auth": auth, "oauth_tokens_present": tokens_present,
///             "enabled": cfg.get("enabled", True) is not False, "tools": cfg.get("tools")}
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSummary {
    pub name: String,
    pub transport: String,
    pub url: Option<String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    /// Sorted env key names.
    pub env: Vec<String>,
    pub auth: Option<String>,
    pub oauth_tokens_present: Option<bool>,
    /// `cfg.get("enabled", True) is not False` — only explicit `Some(false)` disables.
    pub enabled: bool,
    pub tools: Option<String>,
}

/// Serialize one server's config for a UI (no secret values).
///
/// `cfg` is `None` when the Python caller passes a non-dict (mirrors
/// `cfg = cfg if isinstance(cfg, dict) else {}`). `oauth_tokens_present`
/// is only invoked when `auth == "oauth"`.
///
/// # Example
///
/// ```rust
/// use std::collections::BTreeMap;
/// use hermes_tui::mcp_rpc_helpers::{McpServerConfig, summarize_server};
/// let cfg = McpServerConfig { url: Some("https://mcp.example.com".into()), ..Default::default() };
/// let s = summarize_server("my-server", Some(&cfg), |_| true);
/// assert_eq!(s.transport, "http");
/// ```
pub fn summarize_server<F>(name: &str, cfg: Option<&McpServerConfig>, oauth_tokens_present: F) -> ServerSummary
where
    F: Fn(&str) -> bool,
{
    let cfg = cfg.cloned().unwrap_or_default();

    // transport = "http" if cfg.get("url") else ("stdio" if cfg.get("command") else "unknown")
    // Python truthiness: empty string is falsy.
    let has_url = cfg
        .url
        .as_deref()
        .is_some_and(|s| !s.is_empty());
    let has_command = cfg
        .command
        .as_deref()
        .is_some_and(|s| !s.is_empty());
    let transport = if has_url {
        "http"
    } else if has_command {
        "stdio"
    } else {
        "unknown"
    }
    .to_string();

    // auth = cfg.get("auth"); headers = cfg.get("headers") or {}
    let mut auth = cfg.auth.clone();
    let headers = cfg.headers.clone().unwrap_or_default();
    // if not auth and isinstance(headers, dict) and any(str(key).lower() == "authorization" ...)
    if auth.is_none() {
        // headers is always a dict in Rust (BTreeMap); check any key case-insensitive
        let has_auth_header = headers
            .keys()
            .any(|k| k.to_ascii_lowercase() == "authorization");
        if has_auth_header {
            auth = Some("header".to_string());
        }
    }

    // tokens_present = _oauth_tokens_present(name) if auth == "oauth" else None
    let oauth_tokens_present_val = if auth.as_deref() == Some("oauth") {
        Some(oauth_tokens_present(name))
    } else {
        None
    };

    // args = list(cfg.get("args") or [])
    let args = cfg.args.clone().unwrap_or_default();

    // env = sorted(str(k) for k in (cfg.get("env") or {}))
    let env: Vec<String> = cfg
        .env
        .as_ref()
        .map(|m| {
            let mut keys: Vec<String> = m.keys().cloned().collect();
            keys.sort();
            keys
        })
        .unwrap_or_default();

    // enabled = cfg.get("enabled", True) is not False  → only Some(false) disables
    let enabled = !matches!(cfg.enabled, Some(false));

    ServerSummary {
        name: name.to_string(),
        transport,
        url: cfg.url.clone(),
        command: cfg.command.clone(),
        args,
        env,
        auth,
        oauth_tokens_present: oauth_tokens_present_val,
        enabled,
        tools: cfg.tools.clone(),
    }
}

/// Simplified `summarize_server` without OAuth callback — always `None` for
/// `oauth_tokens_present` unless `auth == "oauth"` where it returns `Some(false)`.
/// Useful when the caller doesn't have a token store.
pub fn summarize_server_simple(name: &str, cfg: Option<&McpServerConfig>) -> ServerSummary {
    summarize_server(name, cfg, |_| false)
}

// ---------------------------------------------------------------------------
// Helpers mirroring hermes_constants / hermes_cli.profiles for call sites that
// want concrete (non-injected) behaviour. Kept std-only and filesystem-backed.
// ---------------------------------------------------------------------------

/// Resolve a profile name to its `HERMES_HOME` directory using the filesystem
/// layout `~/.hermes/profiles/<name>` (or `HERMES_HOME` env fallback).
///
/// Mirrors `hermes_cli.profiles.get_profile_dir(name)` simplified to the
/// standard layout. For full `normalize_profile_name` / `_get_profiles_root`
/// semantics inject a custom `get_profile_dir` into [`resolve_profile`].
pub fn get_profile_dir_std(name: &str) -> Option<PathBuf> {
    let home = get_hermes_home();
    if name == "default" {
        return Some(home);
    }
    Some(home.join("profiles").join(name))
}

fn get_hermes_home() -> PathBuf {
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

/// Direct `HERMES_HOME` override token store — `thread_local` + `ContextVar`
/// analogue for single-threaded call sites.
///
/// This is a minimal in-process override used by [`set_hermes_home_override_std`]
/// / [`reset_hermes_home_override_std`]. Real `hermes_constants` uses a
/// `ContextVar`; this thread-local keeps the same call shape for wiring without
/// pulling `hermes_constants` as a dep.
use std::cell::RefCell;
thread_local! {
    static HERMES_HOME_OVERRIDE: RefCell<Option<String>> = const { RefCell::new(None) };
    static HERMES_HOME_OVERRIDE_STACK: RefCell<Vec<Option<String>>> = const { RefCell::new(Vec::new()) };
}

/// Set a thread-local `HERMES_HOME` override and return a reset token.
///
/// Mirrors `hermes_constants.set_hermes_home_override(str(profile_dir))`.
pub fn set_hermes_home_override_std(path: &str) -> ProfileToken {
    let prev = HERMES_HOME_OVERRIDE.with(|c| c.borrow().clone());
    HERMES_HOME_OVERRIDE_STACK.with(|s| s.borrow_mut().push(prev));
    HERMES_HOME_OVERRIDE.with(|c| *c.borrow_mut() = Some(path.to_string()));
    ProfileToken(path.to_string())
}

/// Restore the previous `HERMES_HOME` override.
///
/// Mirrors `hermes_constants.reset_hermes_home_override(token)`.
pub fn reset_hermes_home_override_std(_token: ProfileToken) {
    let prev = HERMES_HOME_OVERRIDE_STACK.with(|s| s.borrow_mut().pop().unwrap_or(None));
    HERMES_HOME_OVERRIDE.with(|c| *c.borrow_mut() = prev);
}

/// Read the current thread-local override (for tests / diagnostics).
pub fn get_hermes_home_override_std() -> Option<String> {
    HERMES_HOME_OVERRIDE.with(|c| c.borrow().clone())
}

// ---------------------------------------------------------------------------
// Tests — mirror Python invariants
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn err_fn(rid: &str, code: i32, msg: &str) -> String {
        format!(r#"{{"rid":"{}","code":{},"message":"{}"}}"#, rid, code, msg)
    }

    // --- resolve_profile ---

    #[test]
    fn resolve_profile_no_profile_is_noop() {
        let params: HashMap<String, String> = HashMap::new();
        let (tok, err) = resolve_profile(
            "1",
            &params,
            |_| None,
            |p| ProfileToken(p.to_string()),
            err_fn,
        );
        assert!(tok.is_none());
        assert!(err.is_none());

        let mut p2 = HashMap::new();
        p2.insert("profile".to_string(), "".to_string());
        let (tok2, err2) = resolve_profile("1", &p2, |_| None, |p| ProfileToken(p.to_string()), err_fn);
        assert!(tok2.is_none());
        assert!(err2.is_none());

        let mut p3 = HashMap::new();
        p3.insert("profile".to_string(), "   ".to_string());
        let (tok3, err3) = resolve_profile("1", &p3, |_| None, |p| ProfileToken(p.to_string()), err_fn);
        assert!(tok3.is_none());
        assert!(err3.is_none());
    }

    #[test]
    fn resolve_profile_trims_and_calls_override() {
        let mut params = HashMap::new();
        params.insert("profile".to_string(), "  coder  ".to_string());
        let tmp = tempfile_dir();
        let profile_path = tmp.join("profiles").join("coder");
        fs::create_dir_all(&profile_path).unwrap();
        let (tok, err) = resolve_profile(
            "42",
            &params,
            |name| {
                assert_eq!(name, "coder");
                Some(profile_path.clone())
            },
            |p| ProfileToken(format!("token:{}", p)),
            err_fn,
        );
        assert!(err.is_none());
        assert_eq!(tok.unwrap().0, format!("token:{}", profile_path.display()));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_profile_missing_dir_returns_4064() {
        let mut params = HashMap::new();
        params.insert("profile".to_string(), "ghost".to_string());
        let (tok, err) = resolve_profile(
            "99",
            &params,
            |_| Some(PathBuf::from("/tmp/__hermes_no_such_profile_xyz__")),
            |p| ProfileToken(p.to_string()),
            err_fn,
        );
        assert!(tok.is_none());
        let e = err.unwrap();
        assert!(e.contains("4064"));
        assert!(e.contains("profile 'ghost' not found"));
        assert!(e.contains("\"rid\":\"99\""));
    }

    #[test]
    fn resolve_profile_none_dir_returns_4064() {
        let mut params = HashMap::new();
        params.insert("profile".to_string(), "ghost".to_string());
        let (tok, err) = resolve_profile("1", &params, |_| None, |p| ProfileToken(p.to_string()), err_fn);
        assert!(tok.is_none());
        assert!(err.unwrap().contains("4064"));
    }

    #[test]
    fn resolve_profile_none_dir_with_no_is_dir() {
        // get_profile_dir returns Some(path) that is not a dir
        let tmp = tempfile_dir();
        let file = tmp.join("not_a_dir");
        fs::write(&file, b"hi").unwrap();
        let mut params = HashMap::new();
        params.insert("profile".to_string(), "x".to_string());
        let (tok, err) = resolve_profile("1", &params, |_| Some(file.clone()), |p| ProfileToken(p.to_string()), err_fn);
        assert!(tok.is_none());
        assert!(err.unwrap().contains("not found"));
        let _ = fs::remove_dir_all(&tmp);
    }

    // --- reset_profile ---

    #[test]
    fn reset_profile_none_is_noop() {
        let called = std::cell::Cell::new(false);
        reset_profile(None, |_| called.set(true));
        assert!(!called.get());
    }

    #[test]
    fn reset_profile_calls_with_token() {
        let got = std::cell::RefCell::new(String::new());
        reset_profile(Some(ProfileToken("tok123".into())), |t| {
            *got.borrow_mut() = t.0;
        });
        assert_eq!(*got.borrow(), "tok123");
    }

    #[test]
    fn reset_profile_swallows_panic() {
        // Mirrors except Exception: pass — panic in reset is swallowed
        reset_profile(Some(ProfileToken("x".into())), |_| panic!("boom"));
        // no panic propagated
    }

    #[test]
    fn reset_profile_fallible_ignores_err() {
        reset_profile_fallible(Some(ProfileToken("x".into())), |_| Err::<(), &str>("err"));
        reset_profile_fallible(None, |_| Ok::<(), &str>(()));
    }

    // --- summarize_server ---

    #[test]
    fn summarize_transport() {
        let http = McpServerConfig {
            url: Some("https://mcp.example.com".into()),
            ..Default::default()
        };
        assert_eq!(summarize_server("a", Some(&http), |_| false).transport, "http");

        let stdio = McpServerConfig {
            command: Some("npx".into()),
            ..Default::default()
        };
        assert_eq!(summarize_server("a", Some(&stdio), |_| false).transport, "stdio");

        let unknown = McpServerConfig::default();
        assert_eq!(summarize_server("a", Some(&unknown), |_| false).transport, "unknown");

        // Empty string is falsy — unknown even though Some("")
        let empty = McpServerConfig {
            url: Some("".into()),
            command: Some("".into()),
            ..Default::default()
        };
        assert_eq!(summarize_server("a", Some(&empty), |_| false).transport, "unknown");

        // url wins over command
        let both = McpServerConfig {
            url: Some("https://x".into()),
            command: Some("cmd".into()),
            ..Default::default()
        };
        assert_eq!(summarize_server("a", Some(&both), |_| false).transport, "http");
    }

    #[test]
    fn summarize_auth_header_inference() {
        let mut headers = BTreeMap::new();
        headers.insert("Authorization".into(), "Bearer tok".into());
        let cfg = McpServerConfig {
            headers: Some(headers),
            ..Default::default()
        };
        let s = summarize_server("a", Some(&cfg), |_| false);
        assert_eq!(s.auth.as_deref(), Some("header"));

        // case-insensitive
        let mut h2 = BTreeMap::new();
        h2.insert("authorization".into(), "Bearer x".into());
        let cfg2 = McpServerConfig {
            headers: Some(h2),
            ..Default::default()
        };
        assert_eq!(summarize_server("a", Some(&cfg2), |_| false).auth.as_deref(), Some("header"));

        // explicit auth wins over header inference
        let mut h3 = BTreeMap::new();
        h3.insert("Authorization".into(), "Bearer x".into());
        let cfg3 = McpServerConfig {
            auth: Some("oauth".into()),
            headers: Some(h3),
            ..Default::default()
        };
        assert_eq!(summarize_server("a", Some(&cfg3), |_| false).auth.as_deref(), Some("oauth"));

        // non-authorization header → no inference
        let mut h4 = BTreeMap::new();
        h4.insert("X-Custom".into(), "1".into());
        let cfg4 = McpServerConfig {
            headers: Some(h4),
            ..Default::default()
        };
        assert!(summarize_server("a", Some(&cfg4), |_| false).auth.is_none());
    }

    #[test]
    fn summarize_oauth_tokens_flag() {
        let oauth = McpServerConfig {
            auth: Some("oauth".into()),
            ..Default::default()
        };
        assert_eq!(summarize_server("a", Some(&oauth), |_| true).oauth_tokens_present, Some(true));
        assert_eq!(summarize_server("a", Some(&oauth), |_| false).oauth_tokens_present, Some(false));
        // only called when oauth
        let header = McpServerConfig {
            auth: Some("header".into()),
            ..Default::default()
        };
        let mut called = false;
        let s = summarize_server("a", Some(&header), |_| {
            called = true;
            true
        });
        assert!(!called);
        assert_eq!(s.oauth_tokens_present, None);

        let none = McpServerConfig::default();
        assert_eq!(summarize_server("a", Some(&none), |_| true).oauth_tokens_present, None);
    }

    #[test]
    fn summarize_env_sorted_and_args() {
        let mut env = BTreeMap::new();
        env.insert("Z_KEY".into(), "1".into());
        env.insert("A_KEY".into(), "2".into());
        let cfg = McpServerConfig {
            env: Some(env),
            args: Some(vec!["--foo".into(), "bar".into()]),
            url: Some("https://x".into()),
            command: Some("cmd".into()),
            ..Default::default()
        };
        let s = summarize_server("srv", Some(&cfg), |_| false);
        assert_eq!(s.env, vec!["A_KEY", "Z_KEY"]);
        assert_eq!(s.args, vec!["--foo", "bar"]);
        assert_eq!(s.url.as_deref(), Some("https://x"));
        assert_eq!(s.command.as_deref(), Some("cmd"));
        assert_eq!(s.name, "srv");
    }

    #[test]
    fn summarize_enabled_logic() {
        // enabled = cfg.get("enabled", True) is not False → only Some(false) disables
        assert!(summarize_server("a", Some(&McpServerConfig { enabled: None, ..Default::default() }), |_| false).enabled);
        assert!(summarize_server("a", Some(&McpServerConfig { enabled: Some(true), ..Default::default() }), |_| false).enabled);
        assert!(!summarize_server("a", Some(&McpServerConfig { enabled: Some(false), ..Default::default() }), |_| false).enabled);
        // None cfg → enabled true (Python: {}.get("enabled", True) is not False → True)
        assert!(summarize_server("a", None, |_| false).enabled);
    }

    #[test]
    fn summarize_none_cfg() {
        let s = summarize_server("x", None, |_| false);
        assert_eq!(s.transport, "unknown");
        assert!(s.url.is_none());
        assert!(s.command.is_none());
        assert!(s.args.is_empty());
        assert!(s.env.is_empty());
        assert!(s.auth.is_none());
        assert_eq!(s.oauth_tokens_present, None);
        assert!(s.enabled);
        assert!(s.tools.is_none());
    }

    #[test]
    fn summarize_tools_passthrough() {
        let cfg = McpServerConfig {
            tools: Some(r#"{"a":1}"#.into()),
            ..Default::default()
        };
        assert_eq!(summarize_server("a", Some(&cfg), |_| false).tools.as_deref(), Some(r#"{"a":1}"#));
    }

    // helpers

    fn tempfile_dir() -> PathBuf {
        let base = std::env::temp_dir().join(format!("hermes-tui-mcp-helpers-{}", std::process::id()));
        let uniq = format!("{}-{}", base.display(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let p = PathBuf::from(uniq);
        let _ = fs::create_dir_all(&p);
        p
    }

    #[test]
    fn std_override_roundtrip() {
        let tok = set_hermes_home_override_std("/tmp/profile-a");
        assert_eq!(get_hermes_home_override_std().as_deref(), Some("/tmp/profile-a"));
        reset_hermes_home_override_std(tok);
        assert_eq!(get_hermes_home_override_std(), None);
    }
}
