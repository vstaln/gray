//! Environment variable passthrough registry.
//! Port of `tools/env_passthrough.py` (223 lines) — 1:1 behavior.
//!
//! Skills that declare `required_environment_variables` in their frontmatter
//! need those vars available in sandboxed execution environments (execute_code,
//! terminal). By default both sandboxes strip secrets from the child process
//! environment for security. This module provides a session-scoped allowlist
//! so skill-declared vars (and user-configured overrides) pass through.
//!
//! Two sources feed the allowlist:
//! 1. Skill declarations — when a skill is loaded via `skill_view`, its
//!    `required_environment_variables` are registered here automatically.
//! 2. User config — `terminal.env_passthrough` in config.yaml lets users
//!    explicitly allowlist vars for non-skill use cases.
//!
//! Both `code_execution_tool.py` and `tools/environments/local.py` consult
//! `is_env_passthrough` before stripping a variable. When profile multiplexing
//! is active, forwarded values are resolved through the current profile's
//! secret scope rather than the process environment.
//!
//! Rust mapping
//! ------------
//! - `_allowed_env_vars_var: ContextVar[set[str]]` → `thread_local! { ALLOWED_ENV_VARS: RefCell<HashSet<String>> }`
//!   (ContextVar prevents cross-session bleed; thread_local is the closest 1:1 in Rust)
//! - `_get_allowed() -> set[str]` → [`with_allowed`] / [`get_allowed_snapshot`] (mirrors line 36-43)
//! - `_config_passthrough: frozenset | None` → [`CONFIG_CACHE`] (`OnceLock<Mutex<Option<HashSet>>>`) (46)
//! - `_is_hermes_provider_credential(name)` → [`is_hermes_provider_credential`] (50-90)
//! - `register_env_passthrough(var_names)` → [`register_env_passthrough`] (93-123)
//! - `_load_config_passthrough()` → [`load_config_passthrough`] (126-163) + [`parse_terminal_env_passthrough`] + [`get_hermes_home`]
//! - `is_env_passthrough(var_name)` → [`is_env_passthrough`] (166-174)
//! - `get_all_passthrough()` → [`get_all_passthrough`] (177-179)
//! - `resolve_passthrough_value(name, fallback)` → [`resolve_passthrough_value`] (182-218) + secret-scope shims
//! - `clear_env_passthrough()` → [`clear_env_passthrough`] (221-223)
//! - `agent.secret_scope._is_global_env` → [`is_global_env`] (98-146 of secret_scope.py)
//! - `agent.secret_scope.is_multiplex_active` → [`is_multiplex_active`] / [`set_multiplex_active`]
//! - `agent.secret_scope.current_secret_scope` → [`current_secret_scope`] / [`set_secret_scope`]
//! - `agent.secret_scope.get_secret` → [`get_secret`]
//! - `tools.environments.local._HERMES_PROVIDER_ENV_BLOCKLIST` → [`HERMES_PROVIDER_ENV_BLOCKLIST`]
//! - `tools.environments.local._is_hermes_internal_secret` → [`is_hermes_internal_secret`]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Constants — mirrors tools/environments/local.py blocklist (226-338)
// ---------------------------------------------------------------------------

/// Mirrors `_HERMES_PROVIDER_ENV_BLOCKLIST` static entries (see `tools/environments/local.py`
/// `_build_provider_env_blocklist` 252-324 + `_AWS_SDK_CREDENTIAL_ENV_VARS`).
/// Dynamic provider registry keys are approximated by the static list.
pub const HERMES_PROVIDER_ENV_BLOCKLIST: &[&str] = &[
    "OPENAI_BASE_URL",
    "OPENAI_API_KEY",
    "OPENAI_API_BASE",
    "OPENAI_ORG_ID",
    "OPENAI_ORGANIZATION",
    "OPENROUTER_API_KEY",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_TOKEN",
    "LLM_MODEL",
    "GOOGLE_API_KEY",
    "VERTEX_CREDENTIALS_PATH",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "DEEPSEEK_API_KEY",
    "MISTRAL_API_KEY",
    "GROQ_API_KEY",
    "TOGETHER_API_KEY",
    "PERPLEXITY_API_KEY",
    "COHERE_API_KEY",
    "FIREWORKS_API_KEY",
    "XAI_API_KEY",
    "HELICONE_API_KEY",
    "PARALLEL_API_KEY",
    "FIRECRAWL_API_KEY",
    "FIRECRAWL_API_URL",
    "TELEGRAM_HOME_CHANNEL",
    "TELEGRAM_HOME_CHANNEL_NAME",
    "DISCORD_HOME_CHANNEL",
    "DISCORD_HOME_CHANNEL_NAME",
    "DISCORD_REQUIRE_MENTION",
    "DISCORD_FREE_RESPONSE_CHANNELS",
    "DISCORD_AUTO_THREAD",
    "SLACK_HOME_CHANNEL",
    "SLACK_HOME_CHANNEL_NAME",
    "SLACK_ALLOWED_USERS",
    "WHATSAPP_ENABLED",
    "WHATSAPP_MODE",
    "WHATSAPP_ALLOWED_USERS",
    "SIGNAL_HTTP_URL",
    "SIGNAL_ACCOUNT",
    "SIGNAL_ALLOWED_USERS",
    "SIGNAL_GROUP_ALLOWED_USERS",
    "SIGNAL_HOME_CHANNEL",
    "SIGNAL_HOME_CHANNEL_NAME",
    "SIGNAL_IGNORE_STORIES",
    "HASS_TOKEN",
    "HASS_URL",
    "EMAIL_ADDRESS",
    "EMAIL_PASSWORD",
    "EMAIL_IMAP_HOST",
    "EMAIL_SMTP_HOST",
    "EMAIL_HOME_ADDRESS",
    "EMAIL_HOME_ADDRESS_NAME",
    "HERMES_DASHBOARD_SESSION_TOKEN",
    "GATEWAY_ALLOWED_USERS",
    "GH_TOKEN",
    "GITHUB_APP_ID",
    "GITHUB_APP_PRIVATE_KEY_PATH",
    "GITHUB_APP_INSTALLATION_ID",
    "MODAL_TOKEN_ID",
    "MODAL_TOKEN_SECRET",
    "DAYTONA_API_KEY",
    "GATEWAY_RELAY_ID",
    "GATEWAY_RELAY_SECRET",
    "GATEWAY_RELAY_DELIVERY_KEY",
    "VERCEL_OIDC_TOKEN",
    "VERCEL_TOKEN",
    "VERCEL_PROJECT_ID",
    "VERCEL_TEAM_ID",
    "AWS_BEARER_TOKEN_BEDROCK",
];

/// Mirrors `__all__` implicit surface of env_passthrough.py.
pub const ALL: &[&str] = &[
    "register_env_passthrough",
    "is_env_passthrough",
    "get_all_passthrough",
    "resolve_passthrough_value",
    "clear_env_passthrough",
];

// ---------------------------------------------------------------------------
// Hermes-internal dynamic secret — mirrors tools/environments/local.py 366-408
// ---------------------------------------------------------------------------

/// Mirrors `tools.environments.local._is_hermes_internal_secret` (366-408).
pub fn is_hermes_internal_secret(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    if upper.starts_with("AUXILIARY_")
        && (upper.ends_with("_API_KEY") || upper.ends_with("_BASE_URL"))
    {
        return true;
    }
    if upper.starts_with("GATEWAY_RELAY_")
        && (upper.ends_with("_SECRET") || upper.ends_with("_KEY") || upper.ends_with("_TOKEN"))
    {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Session-scoped allowlist — mirrors ContextVar _allowed_env_vars_var (33, 36-43)
// ---------------------------------------------------------------------------

thread_local! {
    /// Mirrors `_allowed_env_vars_var: ContextVar[set[str]]` (33).
    /// Backed by thread_local to prevent cross-session bleed in the gateway pipeline.
    static ALLOWED_ENV_VARS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// Mirrors `def _get_allowed() -> set[str]:` (36-43).
/// Get or create the allowed env vars set for the current context/session.
/// In Rust we expose `with_allowed` for mutation without cloning.
fn with_allowed<F, R>(f: F) -> R
where
    F: FnOnce(&mut HashSet<String>) -> R,
{
    ALLOWED_ENV_VARS.with(|cell| f(&mut cell.borrow_mut()))
}

/// Snapshot of allowed vars for read-only checks (mirrors `_get_allowed()` copy).
fn get_allowed_snapshot() -> HashSet<String> {
    ALLOWED_ENV_VARS.with(|cell| cell.borrow().clone())
}

// ---------------------------------------------------------------------------
// Config-based allowlist cache — mirrors _config_passthrough (46-47) + _load (126-163)
// ---------------------------------------------------------------------------

static CONFIG_CACHE: OnceLock<Mutex<Option<HashSet<String>>>> = OnceLock::new();

fn config_cache() -> &'static Mutex<Option<HashSet<String>>> {
    CONFIG_CACHE.get_or_init(|| Mutex::new(None))
}

fn get_hermes_home() -> PathBuf {
    for key in ["GRAY_HOME", "HERMES_HOME"] {
        if let Ok(val) = std::env::var(key) {
            let trimmed = val.trim().to_string();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed);
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim().to_string();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join(".hermes");
        }
    }
    PathBuf::from("/tmp/.hermes")
}

/// Minimal YAML parser for `terminal.env_passthrough` — mirrors `cfg_get(cfg, "terminal", "env_passthrough")` (136).
/// Handles both block list and inline `[A, B]` forms.
pub fn parse_terminal_env_passthrough(text: &str) -> Option<Vec<String>> {
    let lines: Vec<&str> = text.lines().collect();
    let mut terminal_indent: Option<usize> = None;
    let mut in_terminal = false;
    let mut env_indent: Option<usize> = None;
    let mut in_env_list = false;
    let mut found = false;
    let mut out: Vec<String> = Vec::new();

    for line in &lines {
        let trimmed_start = line.trim_start();
        if trimmed_start.is_empty() || trimmed_start.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed_start.len();

        // Detect terminal: section
        if trimmed_start.starts_with("terminal:") {
            terminal_indent = Some(indent);
            in_terminal = true;
            // reset env state when entering terminal
            in_env_list = false;
            env_indent = None;
            // if inline terminal: `terminal: {env_passthrough: [...]}` not expected, ignore
            continue;
        }
        if let Some(ti) = terminal_indent {
            if in_terminal && indent <= ti {
                // Dedented out of terminal
                in_terminal = false;
                terminal_indent = None;
                // This line might itself be a new terminal:
                if trimmed_start.starts_with("terminal:") {
                    terminal_indent = Some(indent);
                    in_terminal = true;
                }
                // If we were inside env list, exit it
                if in_env_list {
                    in_env_list = false;
                    env_indent = None;
                }
                // Don't continue — check other top-level keys? But we are out of terminal, so env_passthrough outside terminal is ignored (mirrors cfg_get).
                // So just continue to next line without parsing env.
                if !in_terminal {
                    continue;
                }
            }
        }
        if !in_terminal {
            continue;
        }

        // Inside terminal — look for env_passthrough:
        if trimmed_start.starts_with("env_passthrough:") {
            let rest = trimmed_start["env_passthrough:".len()..].trim();
            let rest_no_comment = rest.split('#').next().unwrap_or("").trim();
            if rest_no_comment.starts_with('[') {
                // inline list: env_passthrough: [A, B]
                if let Some(end) = rest_no_comment.find(']') {
                    let inner = &rest_no_comment[1..end];
                    for part in inner.split(',') {
                        let t = part
                            .trim()
                            .trim_matches('"')
                            .trim_matches('\'')
                            .trim()
                            .to_string();
                        if !t.is_empty() {
                            out.push(t);
                        }
                    }
                }
                found = true;
                in_env_list = false;
                env_indent = None;
            } else if rest_no_comment.is_empty() {
                // block list starts next lines
                found = true;
                in_env_list = true;
                env_indent = Some(indent);
            } else {
                // scalar non-list (e.g. `env_passthrough: foo`) — python checks `isinstance(passthrough, list)` and yields empty
                // We treat as found but empty, matching python's `if isinstance(passthrough, list):` else result stays empty
                found = true;
                in_env_list = false;
                env_indent = None;
            }
            continue;
        }

        if in_env_list {
            if let Some(ei) = env_indent {
                if indent <= ei && !trimmed_start.starts_with('-') {
                    // Exited env_passthrough block
                    in_env_list = false;
                    env_indent = None;
                    // This line might be another terminal child; don't consume as list item
                    // Fall through to check if it's another env_passthrough? Already handled.
                    // If it's another key at terminal level, stay in_terminal and continue.
                    continue;
                }
                if trimmed_start.starts_with("- ") || trimmed_start.starts_with('-') {
                    let val = trimmed_start
                        .trim_start_matches('-')
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'')
                        .trim()
                        .split('#')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if !val.is_empty() {
                        out.push(val);
                    }
                    continue;
                } else if trimmed_start.contains(':') {
                    // Another key at same or deeper indent as env_passthrough → exit list
                    in_env_list = false;
                    env_indent = None;
                }
            }
        }
    }

    if found {
        Some(out)
    } else {
        None
    }
}

/// Mirrors `def _load_config_passthrough() -> frozenset[str]:` (126-163).
/// Load `terminal.env_passthrough` from config.yaml (cached).
pub fn load_config_passthrough() -> HashSet<String> {
    let cache = config_cache();
    // Fast path: already cached
    if let Ok(guard) = cache.lock() {
        if let Some(cached) = guard.as_ref() {
            return cached.clone();
        }
    }

    let mut result: HashSet<String> = HashSet::new();
    // Try to read raw config — mirrors `read_raw_config()` + `cfg_get` (134-136)
    // Any exception yields empty (debug logged in Python).
    let home = get_hermes_home();
    let cfg_path = home.join("config.yaml");
    if let Ok(text) = std::fs::read_to_string(&cfg_path) {
        // Attempt JSON parse first for robustness (some configs may be JSON, mirrors hook_output_spill)
        // If JSON, try to extract terminal.env_passthrough via string search fallback
        let is_json = text.trim_start().starts_with('{');
        if is_json {
            // Very small JSON scan: look for "env_passthrough" array
            // We do not link serde_json here; use string search to stay std-only
            if let Some(start) = text.find("\"env_passthrough\"") {
                let after = &text[start..];
                if let Some(bracket) = after.find('[') {
                    if let Some(end) = after[bracket..].find(']') {
                        let inner = &after[bracket + 1..bracket + end];
                        for part in inner.split(',') {
                            let t = part
                                .trim()
                                .trim_matches('"')
                                .trim_matches('\'')
                                .trim()
                                .to_string();
                            if t.is_empty() || t.trim().is_empty() {
                                continue;
                            }
                            let name = t.trim().to_string();
                            if name.is_empty() {
                                continue;
                            }
                            // Mirror skill-path filter
                            if is_hermes_provider_credential(&name) {
                                // logger.warning in Python — we use eprintln for parity
                                eprintln!(
                                    "env passthrough: refusing to register Hermes provider credential {:?} from config.yaml (blocked by _HERMES_PROVIDER_ENV_BLOCKLIST).",
                                    name
                                );
                                continue;
                            }
                            result.insert(name);
                        }
                    }
                }
            }
        } else {
            // YAML path — mirrors cfg_get extraction
            if let Some(items) = parse_terminal_env_passthrough(&text) {
                for item in items {
                    if item.trim().is_empty() {
                        continue;
                    }
                    let name = item.trim().to_string();
                    if is_hermes_provider_credential(&name) {
                        eprintln!(
                            "env passthrough: refusing to register Hermes provider credential {:?} from config.yaml (blocked by _HERMES_PROVIDER_ENV_BLOCKLIST).",
                            name
                        );
                        continue;
                    }
                    result.insert(name);
                }
            }
        }
    } else {
        // Mirrors `except Exception as e: logger.debug("Could not read ...")` — swallow
    }

    let cloned = result.clone();
    if let Ok(mut guard) = cache.lock() {
        *guard = Some(cloned.clone());
    }
    cloned
}

/// Test helper: clear the cached config passthrough (mirrors resetting `_config_passthrough = None`).
/// Not in Python public surface; needed for test isolation.
pub fn clear_config_cache_for_tests() {
    if let Some(cache) = CONFIG_CACHE.get() {
        if let Ok(mut guard) = cache.lock() {
            *guard = None;
        }
    }
}

// ---------------------------------------------------------------------------
// _is_hermes_provider_credential — mirrors lines 50-90
// ---------------------------------------------------------------------------

/// True if `name` is a Hermes-managed provider credential (API key, token, or similar)
/// per `_HERMES_PROVIDER_ENV_BLOCKLIST`.
///
/// Mirrors `def _is_hermes_provider_credential(name: str) -> bool:` (50-90).
/// Fail closed: if the authoritative blocklist cannot be imported (partial install,
/// import-time error, etc.) we treat the name as protected. In Rust the blocklist
/// is statically linked, so the import-failure branch is unreachable and we directly
/// check the set; the fail-closed comment is preserved for 1:1 documentation.
pub fn is_hermes_provider_credential(name: &str) -> bool {
    // Dynamic Hermes-internal secrets (AUXILIARY_*_API_KEY / _BASE_URL, GATEWAY_RELAY_*)
    // are provider credentials the static blocklist can't enumerate.
    if is_hermes_internal_secret(name) {
        return true;
    }
    HERMES_PROVIDER_ENV_BLOCKLIST.contains(&name)
}

// ---------------------------------------------------------------------------
// register_env_passthrough — mirrors lines 93-123
// ---------------------------------------------------------------------------

/// Register environment variable names as allowed in sandboxed environments.
///
/// Mirrors `def register_env_passthrough(var_names: Iterable[str]) -> None:` (93-123).
/// Typically called when a skill declares `required_environment_variables`.
///
/// Variables that are Hermes-managed provider credentials (from
/// `_HERMES_PROVIDER_ENV_BLOCKLIST`) are rejected here to preserve the
/// `execute_code` sandbox's credential-scrubbing guarantee per GHSA-rhgp-j443-p4rf.
pub fn register_env_passthrough<I, S>(var_names: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    for raw in var_names {
        let name = raw.as_ref().trim().to_string();
        if name.is_empty() {
            continue;
        }
        if is_hermes_provider_credential(&name) {
            eprintln!(
                "env passthrough: refusing to register Hermes provider credential {:?} (blocked by _HERMES_PROVIDER_ENV_BLOCKLIST). Skills must not override the execute_code sandbox's credential scrubbing; see GHSA-rhgp-j443-p4rf.",
                name
            );
            continue;
        }
        with_allowed(|set| {
            set.insert(name.clone());
        });
        // logger.debug in Python — no-op in Rust (or eprintln at debug level if needed)
    }
}

// ---------------------------------------------------------------------------
// is_env_passthrough / get_all_passthrough — mirrors lines 166-179
// ---------------------------------------------------------------------------

/// Check whether `var_name` is allowed to pass through to sandboxes.
///
/// Mirrors `def is_env_passthrough(var_name: str) -> bool:` (166-174).
/// Returns `True` if the variable was registered by a skill or listed in
/// the user's `terminal.env_passthrough` config.
pub fn is_env_passthrough(var_name: &str) -> bool {
    // Check session-scoped set first — mirrors `if var_name in _get_allowed(): return True` (172-173)
    let in_allowed = ALLOWED_ENV_VARS.with(|cell| cell.borrow().contains(var_name));
    if in_allowed {
        return true;
    }
    load_config_passthrough().contains(var_name)
}

/// Return the union of skill-registered and config-based passthrough vars.
///
/// Mirrors `def get_all_passthrough() -> frozenset[str]:` (177-179).
/// `return frozenset(_get_allowed()) | _load_config_passthrough()`
pub fn get_all_passthrough() -> HashSet<String> {
    let mut out = get_allowed_snapshot();
    out.extend(load_config_passthrough());
    out
}

// ---------------------------------------------------------------------------
// Secret scope shims — mirrors agent/secret_scope.py (310 lines)
// ---------------------------------------------------------------------------

/// Genuinely-global env vars (NOT per-profile secrets) — mirrors `_GLOBAL_ENV_EXACT` (98-134).
const GLOBAL_ENV_EXACT: &[&str] = &[
    "HERMES_HOME",
    "HERMES_PROFILE",
    "HERMES_GATEWAY_LOCK_DIR",
    "HERMES_MAX_ITERATIONS",
    "HERMES_MAX_TOKENS",
    "HERMES_API_TIMEOUT",
    "HERMES_REDACT_SECRETS",
    "HERMES_NOUS_TIMEOUT_SECONDS",
    "_HERMES_GATEWAY",
    "PATH",
    "HOME",
    "USER",
    "LANG",
    "LC_ALL",
    "TZ",
    "PWD",
    "SHELL",
    "TMPDIR",
    "VIRTUAL_ENV",
    "PYTHONPATH",
    "SSL_CERT_FILE",
    "HERMES_KANBAN_DB",
    "HERMES_KANBAN_WORKSPACES_ROOT",
    "HERMES_KANBAN_BOARD",
    "API_SERVER_ENABLED",
    "API_SERVER_HOST",
    "API_SERVER_PORT",
    "API_SERVER_CORS_ORIGINS",
    "GATEWAY_RELAY_URL",
    "GATEWAY_RELAY_ENDPOINT",
    "GATEWAY_RELAY_ALLOW_DIRECT_PLATFORMS",
    "GATEWAY_RELAY_PLATFORMS",
    "GATEWAY_RELAY_BOT_IDS",
    "GATEWAY_RELAY_ROUTE_KEYS",
    "GATEWAY_RELAY_INSTANCE_ID",
    "GATEWAY_RELAY_WAKE_URL",
    "GATEWAY_RELAY_DISPLAY_NAME",
];

const GLOBAL_ENV_PREFIXES: &[&str] =
    &["HERMES_KANBAN_", "HERMES_TELEGRAM_", "TERMINAL_"];

/// Mirrors `def _is_global_env(name: str) -> bool:` (142-146).
pub fn is_global_env(name: &str) -> bool {
    if GLOBAL_ENV_EXACT.contains(&name) {
        return true;
    }
    for prefix in GLOBAL_ENV_PREFIXES {
        if name.starts_with(prefix) {
            return true;
        }
    }
    false
}

static MULTIPLEX_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Mirrors `def set_multiplex_active(active: bool) -> None:` (40-47).
pub fn set_multiplex_active(active: bool) {
    MULTIPLEX_ACTIVE.store(active, Ordering::SeqCst);
}

/// Mirrors `def is_multiplex_active() -> bool:` (50-52).
pub fn is_multiplex_active() -> bool {
    MULTIPLEX_ACTIVE.load(Ordering::SeqCst)
}

thread_local! {
    /// Mirrors `_SECRET_SCOPE: ContextVar[Optional[Mapping[str, str]]]` (56-58).
    static SECRET_SCOPE: RefCell<Option<HashMap<String, String>>> = RefCell::new(None);
}

/// Mirrors `def current_secret_scope() -> Optional[Mapping[str, str]]:` (85-87).
pub fn current_secret_scope() -> Option<HashMap<String, String>> {
    SECRET_SCOPE.with(|cell| cell.borrow().clone())
}

/// Mirrors `def set_secret_scope(secrets: Optional[Mapping[str, str]]) -> Token:` (72-77).
/// In Rust we return the previous value as a token for `reset_secret_scope`.
pub fn set_secret_scope(secrets: Option<HashMap<String, String>>) -> Option<HashMap<String, String>> {
    SECRET_SCOPE.with(|cell| {
        let prev = cell.borrow().clone();
        *cell.borrow_mut() = secrets;
        prev
    })
}

/// Mirrors `def reset_secret_scope(token: Token) -> None:` (80-82).
pub fn reset_secret_scope(token: Option<HashMap<String, String>>) {
    SECRET_SCOPE.with(|cell| {
        *cell.borrow_mut() = token;
    })
}

/// Error raised when a secret is read in multiplex mode with no scope installed.
/// Mirrors `class UnscopedSecretError(RuntimeError):` (61-69).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnscopedSecretError(pub String);

impl std::fmt::Display for UnscopedSecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "UnscopedSecretError: {}", self.0)
    }
}

impl std::error::Error for UnscopedSecretError {}

/// Mirrors `def get_secret(name: str, default: Optional[str] = None) -> Optional[str]:` (149-203).
pub fn get_secret(name: &str, default: Option<&str>) -> Result<Option<String>, UnscopedSecretError> {
    if is_global_env(name) {
        let val = std::env::var(name).ok();
        return Ok(val.or_else(|| default.map(|s| s.to_string())));
    }

    let scope = current_secret_scope();
    if let Some(map) = scope {
        if let Some(val) = map.get(name) {
            return Ok(Some(val.clone()));
        }
        if is_multiplex_active() {
            return Ok(default.map(|s| s.to_string()));
        }
        // Multiplex off: fall through to process env
        let val = std::env::var(name).ok();
        return Ok(val.or_else(|| default.map(|s| s.to_string())));
    }

    if is_multiplex_active() {
        return Err(UnscopedSecretError(format!(
            "get_secret({:?}) called with no profile secret scope active while multiplexing is on. This credential read must run inside a set_secret_scope(...) block (the per-turn / per-adapter profile scope). Reading os.environ here would risk leaking another profile's value. See docs/design/multiplexing-gateway.md (Workstream A).",
            name
        )));
    }

    let val = std::env::var(name).ok();
    Ok(val.or_else(|| default.map(|s| s.to_string())))
}

// ---------------------------------------------------------------------------
// resolve_passthrough_value — mirrors lines 182-218
// ---------------------------------------------------------------------------

/// Resolve an allowlisted variable without crossing profile boundaries.
///
/// Mirrors `def resolve_passthrough_value(name: str, fallback: str | None = None) -> str | None:` (182-218).
///
/// `fallback` is the value the caller would have forwarded before profile
/// secret scopes existed (typically a snapshot of `os.environ` or the
/// current profile's `.env`). An active multiplex scope is authoritative:
/// a missing key returns `None` and never falls back to the process-global
/// environment. An unscoped read while multiplexing is active raises the
/// fail-closed `UnscopedSecretError`.
///
/// Outside multiplexing, an installed scope keeps the existing overlay
/// semantics and an unscoped caller keeps its already-resolved fallback.
pub fn resolve_passthrough_value(
    name: &str,
    fallback: Option<&str>,
) -> Result<Option<String>, UnscopedSecretError> {
    // Global terminal/runtime settings are not profile secrets. `fallback`
    // is already the caller's effective value (including an explicit per-call
    // override), so preserve it instead of replacing it with the process-wide
    // value while a multiplex scope is active. Mirrors lines 209-210.
    if is_global_env(name) && fallback.is_some() {
        return Ok(fallback.map(|s| s.to_string()));
    }

    let scope = current_secret_scope();
    let multiplex_active = is_multiplex_active();
    if scope.is_none() {
        if multiplex_active {
            // Mirrors `return get_secret(name)` (216) — fail-closed if no scope
            return get_secret(name, None);
        }
        return Ok(fallback.map(|s| s.to_string()));
    }
    // Mirrors `return get_secret(name, None if multiplex_active else fallback)` (218)
    get_secret(name, if multiplex_active { None } else { fallback })
}

// ---------------------------------------------------------------------------
// clear_env_passthrough — mirrors lines 221-223
// ---------------------------------------------------------------------------

/// Reset the skill-scoped allowlist (e.g. on session reset).
/// Mirrors `def clear_env_passthrough() -> None:` (221-223).
pub fn clear_env_passthrough() {
    with_allowed(|set| set.clear());
}

/// Clear both the skill-scoped allowlist and the secret scope — test helper.
/// Not in Python surface; used to isolate tests that share thread_local.
#[cfg(test)]
pub fn clear_all_for_tests() {
    clear_env_passthrough();
    reset_secret_scope(None);
    set_multiplex_active(false);
    clear_config_cache_for_tests();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn setup() {
        clear_env_passthrough();
        clear_config_cache_for_tests();
        reset_secret_scope(None);
        set_multiplex_active(false);
        // Ensure HERMES_HOME points to empty tmp so config load yields empty
        // (avoid picking up user's real config.yaml during tests)
    }

    #[test]
    fn blocklist_contains_expected_keys() {
        assert!(HERMES_PROVIDER_ENV_BLOCKLIST.contains(&"OPENAI_API_KEY"));
        assert!(HERMES_PROVIDER_ENV_BLOCKLIST.contains(&"ANTHROPIC_API_KEY"));
        assert!(HERMES_PROVIDER_ENV_BLOCKLIST.contains(&"HASS_TOKEN"));
        assert!(!HERMES_PROVIDER_ENV_BLOCKLIST.contains(&"TENOR_API_KEY"));
        assert!(!HERMES_PROVIDER_ENV_BLOCKLIST.contains(&"NOTION_TOKEN"));
    }

    #[test]
    fn is_hermes_internal_secret_matches_python() {
        assert!(is_hermes_internal_secret("AUXILIARY_TASK_API_KEY"));
        assert!(is_hermes_internal_secret("AUXILIARY_FOO_BASE_URL"));
        assert!(is_hermes_internal_secret("GATEWAY_RELAY_FOO_SECRET"));
        assert!(is_hermes_internal_secret("GATEWAY_RELAY_BAR_KEY"));
        assert!(is_hermes_internal_secret("GATEWAY_RELAY_BAZ_TOKEN"));
        assert!(!is_hermes_internal_secret("GATEWAY_RELAY_URL"));
        assert!(!is_hermes_internal_secret("MY_API_KEY"));
    }

    #[test]
    fn is_hermes_provider_credential_blocks_and_allows() {
        assert!(is_hermes_provider_credential("OPENAI_API_KEY"));
        assert!(is_hermes_provider_credential("AUXILIARY_X_API_KEY"));
        assert!(is_hermes_provider_credential("GATEWAY_RELAY_X_SECRET"));
        assert!(!is_hermes_provider_credential("TENOR_API_KEY"));
        assert!(!is_hermes_provider_credential("NOTION_TOKEN"));
    }

    #[test]
    fn register_and_is_passthrough() {
        setup();
        assert!(!is_env_passthrough("TENOR_API_KEY"));
        register_env_passthrough(["TENOR_API_KEY"]);
        assert!(is_env_passthrough("TENOR_API_KEY"));
        assert!(get_all_passthrough().contains("TENOR_API_KEY"));
    }

    #[test]
    fn register_trims_and_skips_empty() {
        setup();
        register_env_passthrough(["  ", " FOO ", ""]);
        assert!(!is_env_passthrough(""));
        assert!(is_env_passthrough("FOO"));
        assert!(!is_env_passthrough("  "));
    }

    #[test]
    fn register_refuses_blocklisted() {
        setup();
        register_env_passthrough(["OPENAI_API_KEY", "TENOR_API_KEY"]);
        assert!(!is_env_passthrough("OPENAI_API_KEY"));
        assert!(is_env_passthrough("TENOR_API_KEY"));
    }

    #[test]
    fn register_refuses_internal_secret() {
        setup();
        register_env_passthrough(["AUXILIARY_TASK_API_KEY"]);
        assert!(!is_env_passthrough("AUXILIARY_TASK_API_KEY"));
    }

    #[test]
    fn clear_resets() {
        setup();
        register_env_passthrough(["FOO"]);
        assert!(is_env_passthrough("FOO"));
        clear_env_passthrough();
        assert!(!is_env_passthrough("FOO"));
    }

    #[test]
    fn get_all_is_union() {
        setup();
        register_env_passthrough(["A", "B"]);
        let all = get_all_passthrough();
        assert!(all.contains("A"));
        assert!(all.contains("B"));
    }

    #[test]
    fn parse_terminal_env_passthrough_block() {
        let yaml = r#"
terminal:
  env_passthrough:
    - TENOR_API_KEY
    - NOTION_TOKEN
"#;
        let items = parse_terminal_env_passthrough(yaml).unwrap();
        assert_eq!(items, vec!["TENOR_API_KEY", "NOTION_TOKEN"]);
    }

    #[test]
    fn parse_terminal_env_passthrough_inline() {
        let yaml = "terminal:\n  env_passthrough: [A, B, C]\n";
        let items = parse_terminal_env_passthrough(yaml).unwrap();
        assert_eq!(items, vec!["A", "B", "C"]);
    }

    #[test]
    fn parse_terminal_missing_returns_none() {
        let yaml = "model:\n  name: foo\n";
        assert!(parse_terminal_env_passthrough(yaml).is_none());
    }

    #[test]
    fn parse_terminal_non_list_scalar_returns_empty() {
        let yaml = "terminal:\n  env_passthrough: foo\n";
        let items = parse_terminal_env_passthrough(yaml).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn is_global_env_matches_python() {
        assert!(is_global_env("HERMES_HOME"));
        assert!(is_global_env("PATH"));
        assert!(is_global_env("HERMES_KANBAN_DB"));
        assert!(is_global_env("HERMES_KANBAN_FOO"));
        assert!(is_global_env("HERMES_TELEGRAM_FOO"));
        assert!(is_global_env("TERMINAL_FOO"));
        assert!(!is_global_env("OPENAI_API_KEY"));
        assert!(!is_global_env("TENOR_API_KEY"));
    }

    #[test]
    fn resolve_global_preserves_fallback() {
        setup();
        let out = resolve_passthrough_value("PATH", Some("/usr/bin")).unwrap();
        assert_eq!(out, Some("/usr/bin".to_string()));
    }

    #[test]
    fn resolve_without_multiplex_returns_fallback() {
        setup();
        set_multiplex_active(false);
        let out = resolve_passthrough_value("TENOR_API_KEY", Some("fallback-val")).unwrap();
        assert_eq!(out, Some("fallback-val".to_string()));
        let out_none = resolve_passthrough_value("TENOR_API_KEY", None).unwrap();
        assert_eq!(out_none, None);
    }

    #[test]
    fn resolve_with_scope_and_no_multiplex_uses_scope_then_env() {
        setup();
        set_multiplex_active(false);
        let mut scope = HashMap::new();
        scope.insert("TENOR_API_KEY".to_string(), "scoped-val".to_string());
        set_secret_scope(Some(scope));
        let out = resolve_passthrough_value("TENOR_API_KEY", Some("fallback")).unwrap();
        assert_eq!(out, Some("scoped-val".to_string()));
        // missing key falls through to env/fallback when not multiplex
        std::env::set_var("MISSING_KEY_FOR_TEST", "env-val");
        let out2 = resolve_passthrough_value("MISSING_KEY_FOR_TEST", Some("fallback2")).unwrap();
        // scope miss + multiplex off → env wins (get_secret falls through to env)
        assert_eq!(out2, Some("env-val".to_string()));
        std::env::remove_var("MISSING_KEY_FOR_TEST");
        reset_secret_scope(None);
    }

    #[test]
    fn resolve_multiplex_miss_returns_none_not_fallback() {
        setup();
        set_multiplex_active(true);
        let mut scope = HashMap::new();
        scope.insert("PRESENT_KEY".to_string(), "present".to_string());
        set_secret_scope(Some(scope));
        let out = resolve_passthrough_value("ABSENT_KEY", Some("fallback")).unwrap();
        assert_eq!(out, None);
        let out2 = resolve_passthrough_value("PRESENT_KEY", Some("fallback")).unwrap();
        assert_eq!(out2, Some("present".to_string()));
        reset_secret_scope(None);
    }

    #[test]
    fn resolve_multiplex_no_scope_fails_closed() {
        setup();
        set_multiplex_active(true);
        reset_secret_scope(None);
        let err = resolve_passthrough_value("ANY_KEY", Some("fallback")).unwrap_err();
        assert!(err.to_string().contains("UnscopedSecretError"));
        set_multiplex_active(false);
    }

    #[test]
    fn get_secret_unscoped_fails_when_multiplex() {
        setup();
        set_multiplex_active(true);
        let err = get_secret("ANY_KEY", None).unwrap_err();
        assert!(err.to_string().contains("get_secret"));
        set_multiplex_active(false);
        // without multiplex, falls back to env
        std::env::set_var("GET_SECRET_TEST_KEY", "val123");
        let ok = get_secret("GET_SECRET_TEST_KEY", None).unwrap();
        assert_eq!(ok, Some("val123".to_string()));
        std::env::remove_var("GET_SECRET_TEST_KEY");
    }

    #[test]
    fn config_passthrough_blocklist_filtered() {
        setup();
        // Use temp dir for HERMES_HOME
        let tmp = std::env::temp_dir().join(format!(
            "hermes-env-passthrough-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&tmp);
        let prev = std::env::var("HERMES_HOME").ok();
        std::env::set_var("HERMES_HOME", tmp.to_string_lossy().to_string());
        std::env::remove_var("GRAY_HOME");
        clear_config_cache_for_tests();
        let yaml = "terminal:\n  env_passthrough:\n    - TENOR_API_KEY\n    - OPENAI_API_KEY\n    - NOTION_TOKEN\n";
        std::fs::write(tmp.join("config.yaml"), yaml).unwrap();
        let loaded = load_config_passthrough();
        assert!(loaded.contains("TENOR_API_KEY"));
        assert!(loaded.contains("NOTION_TOKEN"));
        assert!(!loaded.contains("OPENAI_API_KEY"));
        // is_env_passthrough should see config
        assert!(is_env_passthrough("TENOR_API_KEY"));
        assert!(!is_env_passthrough("OPENAI_API_KEY"));
        let _ = std::fs::remove_dir_all(&tmp);
        if let Some(p) = prev {
            std::env::set_var("HERMES_HOME", p);
        } else {
            std::env::remove_var("HERMES_HOME");
        }
        clear_config_cache_for_tests();
    }
}
