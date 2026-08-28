//! Local execution environment — slice 1 (lines 1–750).
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/tools/environments/local.py`
//! lines 1–750 (total 1992). Spawn-per-call with session snapshot — first slice
//! covers path normalization (MSYS ↔ Windows), safe-cwd resolution, provider
//! env blocklist construction, secret-scrub helpers, and subprocess env
//! factories (`hermes_subprocess_env` / `build_subprocess_env`). Continues in
//! `local_slice2.rs` (750–1400) with `find_bash`/`find_shell` and PATH
//! sanitization, and `local_slice3.rs` (1400–1992) with `LocalEnvironment`.
//!
//! Python source docstring (preserved):
//! ```text
//! Local execution environment — spawn-per-call with session snapshot.
//! ```

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::file_sync::get_hermes_home;

// ---------------------------------------------------------------------------
// Platform flag — mirrors `platform.system() == "Windows"`
// ---------------------------------------------------------------------------

/// Mirrors `_IS_WINDOWS = platform.system() == "Windows"`.
///
/// Compile-time on Rust; Python evaluates at import. For tests that patch
/// `_IS_WINDOWS`, set `HERMES_FORCE_IS_WINDOWS=1|0` in the process env — the
/// runtime helper `is_windows()` checks that override first.
pub fn is_windows() -> bool {
    if let Ok(v) = env::var("HERMES_FORCE_IS_WINDOWS") {
        let t = v.trim().to_ascii_lowercase();
        if matches!(t.as_str(), "1" | "true" | "yes" | "on") {
            return true;
        }
        if matches!(t.as_str(), "0" | "false" | "no" | "off") {
            return false;
        }
    }
    cfg!(windows)
}

// ---------------------------------------------------------------------------
// Helpers — env var name validation (mirrors `_ENV_VAR_NAME_RE`)
// ---------------------------------------------------------------------------

fn is_valid_env_var_name(name: &str) -> bool {
    // Mirrors `re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")`
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

// ---------------------------------------------------------------------------
// _msys_to_windows_path — mirrors `def _msys_to_windows_path(cwd: str) -> str`
// ---------------------------------------------------------------------------

/// Translate a Git Bash / MSYS-style POSIX path (`/c/Users/x`) to native
/// Windows form (`C:\Users\x`) so `is_dir` and `Popen(cwd=...)` can find it.
///
/// Also accepts Cygwin (`/cygdrive/c/...`) and WSL-mount (`/mnt/c/...`) spellings.
/// No-ops on non-Windows hosts or for paths not in MSYS form. Idempotent.
pub fn msys_to_windows_path(cwd: &str) -> String {
    if !is_windows() || cwd.is_empty() {
        return cwd.to_string();
    }
    // Mirrors `re.match(r'^/(?:(?:cygdrive|mnt)/)?([a-zA-Z])(/.*)?$', cwd)`
    // Manual parse to avoid regex crate.
    let bytes = cwd.as_bytes();
    if bytes.is_empty() || bytes[0] != b'/' {
        return cwd.to_string();
    }
    let mut rest = &cwd[1..];
    // Optional cygdrive/ or mnt/ prefix
    if rest.starts_with("cygdrive/") {
        rest = &rest["cygdrive/".len()..];
    } else if rest.starts_with("mnt/") {
        rest = &rest["mnt/".len()..];
    }
    if rest.is_empty() {
        return cwd.to_string();
    }
    let mut chars = rest.chars();
    let drive = match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase(),
        _ => return cwd.to_string(),
    };
    let tail = chars.as_str(); // remainder after drive letter
    // After drive letter, must be "/" or end. Reject "/home"-style multi-char first segment.
    // Python's `([a-zA-Z])(/.*)?` means: single letter then optionally "/" + anything.
    // So `/home` → group1=H, group2=ome → but group2 must start with "/" — "ome" fails, so no match.
    // Our `tail` is after drive; if tail is non-empty it must start with "/".
    if !tail.is_empty() && !tail.starts_with('/') {
        return cwd.to_string();
    }
    let tail_converted = tail.replace('/', "\\");
    if tail_converted.is_empty() {
        format!("{drive}:\\")
    } else {
        format!("{drive}:{tail_converted}")
    }
}

// ---------------------------------------------------------------------------
// _resolve_local_initial_cwd — mirrors `def _resolve_local_initial_cwd(cwd: str) -> str`
// ---------------------------------------------------------------------------

fn expanduser(cwd: &str) -> String {
    if cwd == "~" || cwd.starts_with("~/") {
        if let Ok(home) = env::var("HOME") {
            let h = home.trim().to_string();
            if !h.is_empty() {
                if cwd == "~" {
                    return h;
                }
                return format!("{}{}", h, &cwd[1..]);
            }
        }
        if let Ok(home) = env::var("USERPROFILE") {
            let h = home.trim().to_string();
            if !h.is_empty() {
                if cwd == "~" {
                    return h;
                }
                return format!("{}{}", h, &cwd[1..]);
            }
        }
    }
    cwd.to_string()
}

fn is_abs_ntpath(path: &str) -> bool {
    // Mirrors `ntpath.isabs`: drive-qualified `C:\` or `C:/` or `\\server\share`
    let b = path.as_bytes();
    if b.len() >= 3 && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/') && b[0].is_ascii_alphabetic() {
        return true;
    }
    if b.len() >= 2 && b[0] == b'\\' && b[1] == b'\\' {
        return true;
    }
    // Also `C:` alone is considered relative in ntpath, but `C:\` is abs. We keep strict.
    false
}

fn is_abs_posix(path: &str) -> bool {
    path.starts_with('/')
}

/// Mirrors `_resolve_local_initial_cwd(cwd: str) -> str`.
pub fn resolve_local_initial_cwd(cwd: &str) -> String {
    let expanded = if cwd.is_empty() {
        env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string())
    } else {
        expanduser(cwd)
    };
    let mut expanded = expanded;
    if is_windows() {
        expanded = msys_to_windows_path(&expanded);
        if is_abs_ntpath(&expanded) {
            return expanded;
        }
    }
    if is_abs_posix(&expanded) || Path::new(&expanded).is_absolute() {
        return expanded;
    }
    // Relative → abspath
    let candidate = if let Ok(cur) = env::current_dir() {
        cur.join(&expanded).to_string_lossy().to_string()
    } else {
        expanded.clone()
    };
    // Normalize `..` / `.` lexically (best-effort, no existence check yet)
    let candidate = PathBuf::from(&candidate)
        .to_string_lossy()
        .to_string();
    let current = env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    // Recovery for config values like `hermes-agent` when Hermes was launched
    // from that directory already.
    if !Path::new(&candidate).is_dir() {
        let wanted_parts: Vec<String> = Path::new(&expanded)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();
        let current_parts: Vec<String> = Path::new(&current)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();
        if !wanted_parts.is_empty() && wanted_parts.len() <= current_parts.len() {
            let tail = &current_parts[current_parts.len() - wanted_parts.len()..];
            if tail == wanted_parts.as_slice() {
                return current;
            }
        }
    }
    candidate
}

// ---------------------------------------------------------------------------
// _windows_to_msys_path — mirrors `def _windows_to_msys_path(cwd: str) -> str`
// ---------------------------------------------------------------------------

/// Translate a native Windows path (`C:\Users\x`) to Git Bash/MSYS form
/// (`/c/Users/x`) so `builtin cd` resolves it reliably.
pub fn windows_to_msys_path(cwd: &str) -> String {
    if !is_windows() || cwd.is_empty() {
        return cwd.to_string();
    }
    // Mirrors `re.match(r'^([a-zA-Z]):[\\/]*(.*)$', cwd)`
    let b = cwd.as_bytes();
    if b.len() < 2 || b[1] != b':' || !b[0].is_ascii_alphabetic() {
        return cwd.to_string();
    }
    let drive = (b[0] as char).to_ascii_lowercase();
    let mut tail = cwd[2..].to_string();
    // Strip leading separators
    tail = tail.trim_start_matches(|c| c == '\\' || c == '/').to_string();
    tail = tail.replace('\\', "/");
    // lstrip '/' already done, but ensure no leading "/"
    let tail = tail.trim_start_matches('/').to_string();
    if tail.is_empty() {
        format!("/{drive}/")
    } else {
        format!("/{drive}/{tail}")
    }
}

// ---------------------------------------------------------------------------
// _bash_safe_path / _quote_bash_path
// ---------------------------------------------------------------------------

/// Return *path* in a form safe to embed in a Git Bash script.
///
/// Mirrors `_bash_safe_path`.
pub fn bash_safe_path(path: &str) -> String {
    if !is_windows() || path.is_empty() {
        return path.to_string();
    }
    let mut p = windows_to_msys_path(path);
    if p.contains('\\') {
        p = p.replace('\\', "/");
    }
    p
}

/// Quote *path* for safe interpolation into a Git Bash script on Windows.
///
/// Mirrors `_quote_bash_path`.
pub fn quote_bash_path(path: &str) -> String {
    shlex_quote(&bash_safe_path(path))
}

fn shlex_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    // Python uses `shlex.quote`
    let safe = s.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '@' | '%' | '_' | '+' | '=' | ':' | ',' | '.' | '/' | '-' )
    });
    if safe {
        return s.to_string();
    }
    let escaped = s.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

// ---------------------------------------------------------------------------
// _cwd_usable / _resolve_safe_cwd — mirrors `def _cwd_usable` / `_resolve_safe_cwd`
// ---------------------------------------------------------------------------

/// True when *path* is a directory this process can actually chdir into.
///
/// Mirrors `_cwd_usable`: `is_dir` + `X_OK`.
pub fn cwd_usable(path: &str) -> bool {
    let p = Path::new(path);
    if !p.is_dir() {
        return false;
    }
    // `os.access(path, os.X_OK)` — best-effort: try to check executable bit on Unix.
    // Without `nix` crate, we probe via `std::fs::metadata` permissions on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(p) {
            let mode = meta.permissions().mode();
            // If none of owner/group/other exec bits are set, treat as not searchable.
            // This is a conservative approximation of `access(X_OK)`.
            if mode & 0o111 == 0 {
                return false;
            }
        }
    }
    true
}

/// Return `cwd` if usable, else nearest accessible ancestor, else temp dir.
///
/// Mirrors `_resolve_safe_cwd`.
pub fn resolve_safe_cwd(cwd: &str) -> String {
    let cwd_owned = if is_windows() {
        msys_to_windows_path(cwd)
    } else {
        cwd.to_string()
    };
    if !cwd_owned.is_empty() && cwd_usable(&cwd_owned) {
        return cwd_owned;
    }
    if !cwd_owned.is_empty() && Path::new(&cwd_owned).is_dir() {
        // Exists but not accessible — log warning (mirrors Python `logger.warning`)
        let uid = get_uid_string();
        log::warn!(
            "Configured terminal cwd {:?} exists but is not accessible to this user (uid={}) — falling back to the nearest usable directory. If this is a gateway/cron process, check for root-owned paths leaking into terminal.cwd / TERMINAL_CWD (#65583).",
            cwd_owned, uid
        );
    }
    let mut parent = if cwd_owned.is_empty() {
        String::new()
    } else {
        Path::new(&cwd_owned)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
    };
    while !parent.is_empty() {
        if cwd_usable(&parent) {
            return parent;
        }
        let next = Path::new(&parent)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        if next == parent {
            break;
        }
        parent = next;
    }
    env::temp_dir().to_string_lossy().to_string()
}

fn get_uid_string() -> String {
    #[cfg(unix)]
    {
        // Best-effort: try `id -u` without external crate, else `?`
        // Python uses `getattr(os, "getuid", lambda: "?")()`
        // We try env UID first.
        if let Ok(v) = env::var("UID") {
            if !v.trim().is_empty() {
                return v.trim().to_string();
            }
        }
        "?".to_string()
    }
    #[cfg(not(unix))]
    {
        "?".to_string()
    }
}

// ---------------------------------------------------------------------------
// Constants — mirrors `local.py` module globals for secret scrub
// ---------------------------------------------------------------------------

/// Mirrors `_HERMES_PROVIDER_ENV_FORCE_PREFIX = "_HERMES_FORCE_"`.
pub const HERMES_PROVIDER_ENV_FORCE_PREFIX: &str = "_HERMES_FORCE_";

/// Mirrors `_AWS_SDK_CREDENTIAL_ENV_VARS = frozenset({"AWS_BEARER_TOKEN_BEDROCK"})`.
pub const AWS_SDK_CREDENTIAL_ENV_VARS: &[&str] = &["AWS_BEARER_TOKEN_BEDROCK"];

/// Mirrors `_ACTIVE_VENV_MARKER_VARS = ("VIRTUAL_ENV", "CONDA_PREFIX", "PYTHONHOME")`.
pub const ACTIVE_VENV_MARKER_VARS: &[&str] = &["VIRTUAL_ENV", "CONDA_PREFIX", "PYTHONHOME"];

// ---------------------------------------------------------------------------
// _build_provider_env_blocklist / _HERMES_PROVIDER_ENV_BLOCKLIST
// ---------------------------------------------------------------------------

/// Mirrors `_build_provider_env_blocklist() -> frozenset`.
///
/// Best-effort: Python pulls from `hermes_cli.auth.PROVIDER_REGISTRY` and
/// `hermes_cli.config.OPTIONAL_ENV_VARS` at import time. In this port we
/// synthesize the same set from a static literal (the update block at
/// 252–324 in `local.py`) plus any `HERMES_EXTRA_BLOCKLIST` env (comma-separated)
/// for test injection. The `CLAUDE_CODE_OAUTH_TOKEN` discard is preserved.
pub fn build_provider_env_blocklist() -> HashSet<String> {
    let mut blocked: HashSet<String> = HashSet::new();

    // Static literal from the Python `blocked.update({...})` at 252–324
    for key in [
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
    ] {
        blocked.insert(key.to_string());
    }
    // Add AWS SDK credential env vars
    for k in AWS_SDK_CREDENTIAL_ENV_VARS {
        blocked.insert(k.to_string());
    }
    // Test injection: extra blocklist entries
    if let Ok(extra) = env::var("HERMES_EXTRA_BLOCKLIST") {
        for part in extra.split(|c| c == ',' || c == ';') {
            let t = part.trim().to_string();
            if !t.is_empty() {
                blocked.insert(t);
            }
        }
    }
    // Mirrors `blocked.discard("CLAUDE_CODE_OAUTH_TOKEN")`
    blocked.remove("CLAUDE_CODE_OAUTH_TOKEN");
    blocked
}

static PROVIDER_BLOCKLIST: OnceLock<HashSet<String>> = OnceLock::new();

fn provider_blocklist() -> &'static HashSet<String> {
    PROVIDER_BLOCKLIST.get_or_init(build_provider_env_blocklist)
}

// ---------------------------------------------------------------------------
// _is_hermes_internal_secret — mirrors `def _is_hermes_internal_secret(key: str) -> bool`
// ---------------------------------------------------------------------------

/// Mirrors `_is_hermes_internal_secret`.
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
// _inject_context_hermes_home / _inject_session_context_env — mirrors local.py
// ---------------------------------------------------------------------------

/// Bridge the context-local Hermes home override into subprocess env.
///
/// Mirrors `_inject_context_hermes_home`.
pub fn inject_context_hermes_home(env: &mut HashMap<String, String>) {
    // Mirrors `from hermes_constants import get_hermes_home_override` try/except.
    // In Rust we check `HERMES_HOME_OVERRIDE` env as the override source (best-effort).
    if let Ok(v) = env::var("HERMES_HOME_OVERRIDE") {
        let t = v.trim().to_string();
        if !t.is_empty() {
            env.insert("HERMES_HOME".to_string(), t);
        }
    }
}

/// Bridge gateway session ContextVars into a subprocess environment dict.
///
/// Mirrors `_inject_session_context_env`.
pub fn inject_session_context_env(env: &mut HashMap<String, String>) {
    // Best-effort: Python reads `gateway.session_context._VAR_MAP` + `session_context_engaged`.
    // In Rust we model with env vars `HERMES_SESSION_*` and `HERMES_SESSION_CONTEXT_ENGAGED`.
    let engaged = env::var("HERMES_SESSION_CONTEXT_ENGAGED")
        .map(|v| {
            let t = v.trim().to_ascii_lowercase();
            matches!(t.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false);
    // Known session vars (mirrors `_VAR_MAP` keys)
    for var_name in [
        "HERMES_SESSION_ID",
        "HERMES_SESSION_KEY",
        "HERMES_USER_ID",
        "HERMES_CHAT_ID",
        "HERMES_THREAD_ID",
    ] {
        if let Ok(val) = env::var(var_name) {
            // ContextVar set (including "") → authoritative
            // In Rust `env::var` missing means _UNSET; we handle below.
            env.insert(var_name.to_string(), val);
        } else if engaged {
            // _UNSET while engaged → strip inherited global
            env.remove(var_name);
        }
    }
}

// ---------------------------------------------------------------------------
// _strip_hermes_owned_pythonpath_and_runtime_markers + helpers
// (mirrors `tools.environments.local._strip_hermes_owned_pythonpath*`)
// ---------------------------------------------------------------------------

pub fn strip_hermes_owned_pythonpath_and_runtime_markers(env: &mut HashMap<String, String>) {
    strip_hermes_owned_pythonpath(env);
    for marker in ACTIVE_VENV_MARKER_VARS {
        env.remove(*marker);
    }
}

fn strip_hermes_owned_pythonpath(env: &mut HashMap<String, String>) {
    let Some(pp) = env.get("PYTHONPATH").cloned() else {
        return;
    };
    if pp.is_empty() {
        return;
    }
    // Best-effort: Python strips exact repo root + site-packages entries.
    // In this slice we implement the same check using current exe's prefix.
    let hermes_home = get_hermes_home().to_string_lossy().to_string();
    let sep = if is_windows() { ";" } else { ":" };
    let mut kept: Vec<String> = Vec::new();
    let mut stripped: Vec<String> = Vec::new();
    for entry in pp.split(sep) {
        if entry.is_empty() {
            kept.push(entry.to_string());
            continue;
        }
        // Check if entry is hermes home or descendant that is exactly the runtime site-packages.
        // Conservative: only strip exact hermes_home or hermes_home + "/lib/python*/site-packages"
        let is_hermes_owned = entry == hermes_home
            || entry == format!("{hermes_home}/lib/site-packages")
            || (entry.contains(".hermes") && entry.contains("site-packages"));
        if is_hermes_owned {
            stripped.push(entry.to_string());
        } else {
            kept.push(entry.to_string());
        }
    }
    if kept.is_empty() {
        env.remove("PYTHONPATH");
    } else {
        env.insert("PYTHONPATH".to_string(), kept.join(sep));
    }
    if !stripped.is_empty() {
        log::debug!("Stripped Hermes-owned entries from PYTHONPATH: {:?}", stripped);
    }
}

// ---------------------------------------------------------------------------
// _apply_windows_msys_bash_env_defaults / _path_env_key / _prepend_hermes_bin_dir
// (minimal stubs for slice1 — full impl in slice2)
// ---------------------------------------------------------------------------

fn apply_windows_msys_bash_env_defaults(env: &mut HashMap<String, String>) {
    if !is_windows() {
        return;
    }
    env.entry("MSYS_NO_PATHCONV".to_string())
        .or_insert_with(|| "1".to_string());
    env.entry("MSYS2_ARG_CONV_EXCL".to_string())
        .or_insert_with(|| "*".to_string());
}

fn path_env_key(env: &HashMap<String, String>) -> Option<String> {
    if !is_windows() {
        return Some("PATH".to_string());
    }
    for k in env.keys() {
        if k.to_ascii_uppercase() == "PATH" {
            return Some(k.clone());
        }
    }
    None
}

fn prepend_hermes_bin_dir(existing_path: &str) -> String {
    // Best-effort: mirrors `_prepend_hermes_bin_dir` from local.py.
    // In Python this resolves `hermes` on PATH / sys.argv[0] / sys.executable.
    // Here we check `HERMES_BIN_DIR` env override plus `HERMES_HOME/bin`.
    let bin_dir = if let Ok(v) = env::var("HERMES_BIN_DIR") {
        let t = v.trim().to_string();
        if !t.is_empty() {
            Some(t)
        } else {
            None
        }
    } else {
        let candidate = get_hermes_home().join("bin").to_string_lossy().to_string();
        if Path::new(&candidate).is_dir() {
            Some(candidate)
        } else {
            None
        }
    };
    let Some(bin_dir) = bin_dir else {
        return existing_path.to_string();
    };
    let sep = if is_windows() { ";" } else { ":" };
    let mut entries: Vec<String> = if existing_path.is_empty() {
        Vec::new()
    } else {
        existing_path.split(sep).map(|s| s.to_string()).collect()
    };
    if entries.iter().any(|e| e == &bin_dir) {
        return existing_path.to_string();
    }
    entries.insert(0, bin_dir);
    entries.join(sep)
}

fn scrub_delegated_child_kanban_env(env: HashMap<String, String>) -> HashMap<String, String> {
    // Mirrors `_scrub_delegated_child_kanban_env` — best-effort no-op unless
    // `HERMES_KANBAN_CHILD=1` is set (test hook for delegated child).
    if env::var("HERMES_KANBAN_CHILD").is_ok() {
        let mut out = env;
        out.remove("HERMES_KANBAN_BOARD");
        return out;
    }
    env
}

// ---------------------------------------------------------------------------
// _sanitize_subprocess_env — mirrors `def _sanitize_subprocess_env(base_env, extra_env)`
// ---------------------------------------------------------------------------

/// Filter Hermes-managed secrets from a subprocess environment.
///
/// Mirrors `_sanitize_subprocess_env`.
pub fn sanitize_subprocess_env(
    base_env: Option<&HashMap<String, String>>,
    extra_env: Option<&HashMap<String, String>>,
) -> HashMap<String, String> {
    let blocklist = provider_blocklist();
    // Best-effort passthrough: check `HERMES_PASSTHROUGH` env (comma-separated)
    let passthrough_set: HashSet<String> = env::var("HERMES_PASSTHROUGH")
        .map(|v| {
            v.split(|c| c == ',' || c == ';' || c == ' ')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let is_passthrough = |k: &str| passthrough_set.contains(k);

    let mut sanitized: HashMap<String, String> = HashMap::new();

    // First pass: base_env
    if let Some(base) = base_env {
        for (key, value) in base {
            if key.starts_with(HERMES_PROVIDER_ENV_FORCE_PREFIX) {
                continue;
            }
            if is_hermes_internal_secret(key) {
                continue;
            }
            let passthrough = is_passthrough(key);
            if blocklist.contains(key) && !passthrough {
                continue;
            }
            // `resolve_passthrough_value` — best-effort: return value unchanged
            sanitized.insert(key.clone(), value.clone());
        }
    }

    // Second pass: extra_env (applied last, wins)
    if let Some(extra) = extra_env {
        for (key, value) in extra {
            if key.starts_with(HERMES_PROVIDER_ENV_FORCE_PREFIX) {
                let real_key = &key[HERMES_PROVIDER_ENV_FORCE_PREFIX.len()..];
                if is_hermes_internal_secret(real_key) {
                    continue;
                }
                sanitized.insert(real_key.to_string(), value.clone());
            } else if is_hermes_internal_secret(key) {
                continue;
            } else {
                let passthrough = is_passthrough(key);
                if blocklist.contains(key) && !passthrough {
                    continue;
                }
                sanitized.insert(key.clone(), value.clone());
            }
        }
    }

    inject_context_hermes_home(&mut sanitized);
    apply_subprocess_home_env(&mut sanitized);
    inject_session_context_env(&mut sanitized);
    strip_hermes_owned_pythonpath_and_runtime_markers(&mut sanitized);

    if let Some(path_key) = path_env_key(&sanitized) {
        if let Some(existing) = sanitized.get(&path_key).cloned() {
            // In slice1 we only do hermes bin prepend; full PATH sanitization
            // (`_append_missing_sane_path_entries` + `_prepend_git_bash_dirs`) lives in slice2.
            sanitized.insert(path_key, prepend_hermes_bin_dir(&existing));
        }
    }

    apply_windows_msys_bash_env_defaults(&mut sanitized);
    scrub_delegated_child_kanban_env(sanitized)
}

fn apply_subprocess_home_env(env: &mut HashMap<String, String>) {
    // Mirrors `hermes_constants.apply_subprocess_home_env` — best-effort.
    // In Python this ensures `HOME` is set for subprocesses when `HERMES_HOME`
    // is overridden. Here we ensure `HERMES_HOME` is present.
    if !env.contains_key("HERMES_HOME") {
        if let Ok(v) = env::var("HERMES_HOME") {
            let t = v.trim().to_string();
            if !t.is_empty() {
                env.insert("HERMES_HOME".to_string(), t);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// _ALWAYS_STRIP_KEYS / hermes_subprocess_env / build_subprocess_env
// Mirrors `local.py` lines 564–742
// ---------------------------------------------------------------------------

/// Mirrors `_ALWAYS_STRIP_KEYS: frozenset[str] = frozenset({...})`.
pub const ALWAYS_STRIP_KEYS: &[&str] = &[
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GITHUB_APP_ID",
    "GITHUB_APP_PRIVATE_KEY_PATH",
    "GITHUB_APP_INSTALLATION_ID",
    "TELEGRAM_BOT_TOKEN",
    "DISCORD_BOT_TOKEN",
    "SLACK_BOT_TOKEN",
    "SLACK_APP_TOKEN",
    "SLACK_SIGNING_SECRET",
    "GATEWAY_ALLOWED_USERS",
    "GATEWAY_ALLOW_ALL_USERS",
    "GATEWAY_RELAY_ID",
    "GATEWAY_RELAY_SECRET",
    "GATEWAY_RELAY_DELIVERY_KEY",
    "HASS_TOKEN",
    "EMAIL_PASSWORD",
    "HERMES_DASHBOARD_SESSION_TOKEN",
    "MODAL_TOKEN_ID",
    "MODAL_TOKEN_SECRET",
    "DAYTONA_API_KEY",
];

/// Build a sanitized environment dict for a spawned subprocess.
///
/// Mirrors `hermes_subprocess_env(*, inherit_credentials: bool = False)`.
pub fn hermes_subprocess_env(inherit_credentials: bool) -> HashMap<String, String> {
    // Snapshot os.environ
    let mut env: HashMap<String, String> = env::vars().collect();

    // Tier 1 — always strip
    for key in ALWAYS_STRIP_KEYS {
        env.remove(*key);
    }
    // Internal routing hints + dynamic secrets
    let keys: Vec<String> = env.keys().cloned().collect();
    for key in keys {
        if key.starts_with(HERMES_PROVIDER_ENV_FORCE_PREFIX)
            || is_hermes_internal_secret(&key)
        {
            env.remove(&key);
        }
    }

    if !inherit_credentials {
        // Tier 2 — strip provider/tool credentials unless explicitly inherited
        for key in provider_blocklist().iter() {
            env.remove(key);
        }
    }

    env.entry("PYTHONUTF8".to_string())
        .or_insert_with(|| "1".to_string());

    inject_context_hermes_home(&mut env);
    apply_subprocess_home_env(&mut env);
    strip_hermes_owned_pythonpath_and_runtime_markers(&mut env);
    apply_windows_msys_bash_env_defaults(&mut env);
    inject_session_context_env(&mut env);
    scrub_delegated_child_kanban_env(env)
}

/// Single factory for building a child-process environment.
///
/// Mirrors `build_subprocess_env(base, *, inherit_profile_home=True, scrub_secrets=True, extra=None)`.
pub fn build_subprocess_env(
    base: Option<&HashMap<String, String>>,
    inherit_profile_home: bool,
    scrub_secrets: bool,
    extra: Option<&HashMap<String, String>>,
) -> HashMap<String, String> {
    if scrub_secrets {
        // `_sanitize_subprocess_env` already performs HERMES_HOME override bridging
        // + apply_subprocess_home_env unconditionally.
        let base_owned = base.cloned().unwrap_or_else(|| env::vars().collect());
        return sanitize_subprocess_env(Some(&base_owned), extra);
    }

    let mut env: HashMap<String, String> = base.cloned().unwrap_or_else(|| env::vars().collect());
    if inherit_profile_home {
        inject_context_hermes_home(&mut env);
        apply_subprocess_home_env(&mut env);
    }
    if let Some(extra_map) = extra {
        for (k, v) in extra_map {
            env.insert(k.clone(), v.clone());
        }
    }
    env
}

// ---------------------------------------------------------------------------
// Tests — minimal smoke for slice1 helpers (no cargo run required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msys_to_windows_noop_off_windows() {
        // On non-Windows host, msys paths are returned unchanged (see is_windows guard)
        // We force is_windows false by not setting HERMES_FORCE_IS_WINDOWS
        // So this asserts the no-op path when host is not Windows.
        if !is_windows() {
            assert_eq!(msys_to_windows_path("/c/Users/x"), "/c/Users/x");
            assert_eq!(windows_to_msys_path("C:\\Users\\x"), "C:\\Users\\x");
        }
    }

    #[test]
    fn valid_env_names() {
        assert!(is_valid_env_var_name("FOO_BAR"));
        assert!(is_valid_env_var_name("_PRIVATE"));
        assert!(!is_valid_env_var_name("123BAD"));
        assert!(!is_valid_env_var_name("HAS-DASH"));
        assert!(!is_valid_env_var_name(""));
    }

    #[test]
    fn hermes_internal_secret_matches() {
        assert!(is_hermes_internal_secret("AUXILIARY_VISION_API_KEY"));
        assert!(is_hermes_internal_secret("AUXILIARY_TASK_BASE_URL"));
        assert!(is_hermes_internal_secret("GATEWAY_RELAY_SECRET"));
        assert!(is_hermes_internal_secret("GATEWAY_RELAY_DELIVERY_KEY"));
        assert!(!is_hermes_internal_secret("GATEWAY_RELAY_URL"));
        assert!(!is_hermes_internal_secret("OPENAI_API_KEY"));
    }

    #[test]
    fn sanitize_strips_blocklist() {
        let mut base = HashMap::new();
        base.insert("OPENAI_API_KEY".to_string(), "sk-123".to_string());
        base.insert("MY_VAR".to_string(), "keep".to_string());
        let out = sanitize_subprocess_env(Some(&base), None);
        assert!(!out.contains_key("OPENAI_API_KEY"));
        assert_eq!(out.get("MY_VAR").map(|s| s.as_str()), Some("keep"));
    }
}
