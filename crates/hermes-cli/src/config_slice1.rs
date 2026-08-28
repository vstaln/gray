//! hermes-cli config — slice 1/7
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/config.py`
//! slice 1/7 — lines 1–900 of 6 072 (first 900 LOC).
//! Covers: module docstring (config.yaml + .env layout, `hermes config` verbs),
//! bootstrap imports + logger, `_CONFIG_PARSE_WARNED` dedup set,
//! `_backup_corrupt_config` (timestamped .bak), `_warn_config_parse_failure`
//! (stderr + logger + last-known-good fallback), `_IS_WINDOWS` + `_ENV_VAR_NAME_RE`,
//! `_ENV_VAR_NAME_DENYLIST` (loader/Python/Node/PATH/git/HERMES_* denylist) +
//! `_reject_denylisted_env_var`, cache globals (`_LAST_EXPANDED_CONFIG_BY_PATH`,
//! `_LOAD_CONFIG_CACHE`, `_RAW_CONFIG_CACHE`, `_CONFIG_LOCK` reentrant lock,
//! `_EXTRA_ENV_KEYS`), yaml/colors/default_soul imports, managed-mode
//! constants (`_MANAGED_TRUE_VALUES`, `_NIX_MANAGED_SYSTEMS`, `_NIX_STORE`,
//! `_IGNORED_MANAGED_VALUES`) + `get_managed_system` / `is_managed` /
//! `_NIX_UPDATE_MSG` / `get_managed_update_command` / `_install_method_project_root` /
//! `detect_install_method` (code-scoped + home-scoped + managed + /nix/store + .git
//! resolution) / `_running_in_container` / `stamp_install_method` /
//! `is_nix_install_method` / `recommended_update_command_for_method` /
//! `recommended_update_command` / `_DOCKER_UPDATE_MESSAGE` /
//! `format_docker_update_message` / `format_managed_message` / `managed_error` /
//! `get_container_exec_info` (.container-mode), config paths
//! (`get_hermes_home` re-export + `get_config_path` / `get_env_path` / `get_project_root` /
//! `_resolve_hermes_uid_gid` / `_chown_to_hermes_uid` / `_secure_dir` / `_is_container` /
//! `_secure_file` / `_ensure_default_soul_md` / `_HERMES_HOME_ENSURED` /
//! `ensure_hermes_home` / `_ensure_hermes_home_managed`) through line 965
//! (slice boundary padded to 900; `ensure_hermes_home` header starts at 899).
//! Continued in `config_slice2.rs` (from `_ensure_hermes_home_managed` body tail
//! + `DEFAULT_CONFIG` import at line 971).
//!
//! T0687 — 1:1 port, no cargo (NEVER cargo).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, RwLock};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-15
// ---------------------------------------------------------------------------

/// Module doc — Configuration management for Hermes Agent.
///
/// Config files are stored in `~/.hermes/` for easy access:
/// - `~/.hermes/config.yaml`  — All settings (model, toolsets, terminal, etc.)
/// - `~/.hermes/.env`         — API keys and secrets
///
/// This module provides:
/// - `hermes config`          — Show current configuration
/// - `hermes config edit`     — Open config in editor
/// - `hermes config get`      — Print a resolved configuration value
/// - `hermes config set`      — Set a specific value
/// - `hermes config unset`    — Remove a user configuration value
/// - `hermes config wizard`   — Re-run setup wizard
/// Mirrors `hermes_cli/config.py` lines 1-15.
pub const MODULE_DOC: &str = "config: Configuration management for Hermes Agent — see lines 1-15";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 17-37
// ---------------------------------------------------------------------------
// Python: copy, hermes_cli.cli_output.line_input, json, logging, os, platform,
// re, shutil, stat, subprocess, sys, tempfile, threading, time, unicodedata,
// dataclasses, pathlib.Path, typing, hermes_cli.route_identity,
// hermes_cli.secret_prompt, yaml, hermes_cli.colors, hermes_cli.default_soul,
// hermes_constants, utils
//
// Rust: std only (NEVER cargo). All external/Python-specific imports are
// stubbed for 1:1 traceability; real wiring in later slices when those modules
// are ported.

/// Mirrors `from hermes_cli.cli_output import line_input` (line 18) — stub.
pub fn line_input_stub(_prompt: &str, _default: Option<&str>) -> String {
    String::new()
}

/// Mirrors `from hermes_cli.route_identity import normalize_route_base_url` (line 36) — stub.
pub fn normalize_route_base_url_stub(url: &str) -> String {
    url.trim().to_string()
}

/// Mirrors `from hermes_cli.secret_prompt import masked_secret_prompt` (line 37) — stub.
pub fn masked_secret_prompt_stub(_prompt: &str) -> String {
    String::new()
}

/// Mirrors `import yaml` (line 331) — stub safe_load.
pub fn yaml_safe_load_stub(text: &str) -> Option<HashMap<String, String>> {
    if text.trim().is_empty() {
        return Some(HashMap::new());
    }
    None
}

/// Mirrors `from hermes_cli.colors import Colors, color` (line 333) — stub.
pub fn color_stub(text: &str, _color: &str) -> String {
    text.to_string()
}

/// Mirrors `from hermes_cli.default_soul import DEFAULT_SOUL_MD, is_legacy_template_soul` (line 334) — stub.
pub const DEFAULT_SOUL_MD: &str = "# SOUL.md — default soul\n";
pub fn is_legacy_template_soul_stub(text: &str) -> bool {
    // Python checks for old comment-only scaffold; stub: true if empty or all comments.
    let trimmed = text.trim();
    trimmed.is_empty() || trimmed.lines().all(|l| l.trim().is_empty() || l.trim().starts_with('#'))
}

/// Mirrors `from hermes_constants import get_hermes_home, get_process_hermes_home` (line 723) + utils.
pub fn get_hermes_home() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_HOME") {
        if !v.trim().is_empty() {
            return PathBuf::from(v.trim());
        }
    }
    dirs_home().join(".hermes")
}
pub fn get_process_hermes_home() -> PathBuf {
    get_hermes_home()
}
fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}
/// Mirrors `from utils import atomic_replace, fast_safe_load` (line 724) — stubs.
pub fn atomic_replace_stub(_src: &Path, _dst: &Path) -> std::io::Result<()> {
    Ok(())
}
pub fn fast_safe_load_stub(_path: &Path) -> Option<HashMap<String, String>> {
    None
}

// ---------------------------------------------------------------------------
// Logger — mirrors line 39
// ---------------------------------------------------------------------------
// Python: logger = logging.getLogger(__name__)
// Rust: no logger crate in slice 1 (NEVER cargo); use eprintln!/log stub.

pub fn log_warning(msg: &str) {
    eprintln!("[hermes config WARN] {msg}");
}

// ---------------------------------------------------------------------------
// _CONFIG_PARSE_WARNED — mirrors lines 41-44
// ---------------------------------------------------------------------------

/// Track which (config_path, mtime_ns, size) tuples we've already warned about
/// so concurrent CLI/gateway loads of a broken config.yaml don't spam stderr
/// every time. Cleared automatically when the file changes (different mtime).
/// Mirrors `_CONFIG_PARSE_WARNED: set = set()` (lines 41-44).
static CONFIG_PARSE_WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn config_parse_warned() -> &'static Mutex<HashSet<String>> {
    CONFIG_PARSE_WARNED.get_or_init(|| Mutex::new(HashSet::new()))
}

fn parse_warned_key(path: &Path, mtime_ns: u128, size: u64) -> String {
    format!("{}:{}:{}", path.display(), mtime_ns, size)
}

// ---------------------------------------------------------------------------
// _backup_corrupt_config — mirrors lines 47-98
// ---------------------------------------------------------------------------

/// Preserve a corrupted `config.yaml` by copying it to a timestamped `.bak`.
///
/// Mirrors `_backup_corrupt_config(config_path: Path) -> Optional[Path]`
/// (lines 47-98). Best-effort: any failure (permissions, symlink, disk full)
/// is swallowed so config loading is never blocked. Symlinks are not followed
/// (mirrors Gemini #21541 lstat guard). Returns backup path on success else None.
pub fn backup_corrupt_config(config_path: &Path) -> Option<PathBuf> {
    // Symlink guard — mirrors `if config_path.is_symlink(): return None` (69)
    if let Ok(meta) = std::fs::symlink_metadata(config_path) {
        if meta.file_type().is_symlink() {
            return None;
        }
    } else {
        return None;
    }
    // Stat + empty guard (71-75)
    let st = std::fs::metadata(config_path).ok()?;
    let size = st.len();
    if size == 0 {
        return None;
    }
    // Timestamp — mirrors `time.strftime("%Y%m%d-%H%M%S")` (76)
    let ts = chrono_timestamp_fallback();
    let file_name = config_path.file_name()?.to_string_lossy().to_string();
    let backup_path = config_path.with_file_name(format!("{file_name}.corrupt.{ts}.bak"));
    // Dedup: same size as existing sibling bak -> skip (82-92)
    if let Some(parent) = config_path.parent() {
        if let Ok(entries) = std::fs::read_dir(parent) {
            let prefix = format!("{file_name}.corrupt.");
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&prefix) && name.ends_with(".bak") {
                    if let Ok(m) = entry.metadata() {
                        if m.len() == size {
                            return None;
                        }
                    }
                }
            }
        }
    }
    if backup_path.exists() {
        return None;
    }
    std::fs::copy(config_path, &backup_path).ok()?;
    Some(backup_path)
}

fn chrono_timestamp_fallback() -> String {
    // Mirrors `time.strftime("%Y%m%d-%H%M%S")` without chrono dep (NEVER cargo).
    // Use SystemTime -> approximate seconds since epoch formatted as raw secs.
    // For 1:1 traceability we produce YYYYMMDD-HHMMSS via time crate stub.
    // Fallback: unix secs as string if time calc fails; still timestamped.
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Simple deterministic fallback: secs mod day -> HHMMSS; not calendar-correct
    // but preserves the backup-path uniqueness contract. Full calendar would need chrono.
    // We keep the format length to satisfy the dedup contract (same second -> same stamp).
    // Callers only check existence, not parse.
    let day_secs = secs % 86400;
    let h = (day_secs / 3600) % 24;
    let m = (day_secs % 3600) / 60;
    let s = day_secs % 60;
    // Use secs /86400 as fake YYYYMMDD-ish day counter for uniqueness
    let fake_days = secs / 86400;
    format!("{fake_days:08}-{h:02}{m:02}{s:02}")
}

// ---------------------------------------------------------------------------
// _warn_config_parse_failure — mirrors lines 101-157
// ---------------------------------------------------------------------------

/// Surface a config.yaml parse failure to user, log, and stderr.
///
/// Mirrors `_warn_config_parse_failure(config_path, exc, *, fallback="defaults")`
/// (lines 101-157). Warns once per (path, mtime_ns, size) on stderr AND in
/// `agent.log` / `errors.log` at WARNING. Re-warns if file changes. On first
/// warning snapshots to timestamped `.bak` best-effort.
/// `fallback` is "defaults" or "last-known-good" (codex#31188 port).
pub fn warn_config_parse_failure(config_path: &Path, exc: &str, fallback: &str) {
    // Build dedup key — mirrors stat() branch (126-129)
    let (mtime_ns, size) = std::fs::metadata(config_path)
        .ok()
        .and_then(|m| {
            m.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| (d.as_nanos(), m.len()))
        })
        .unwrap_or((0, 0));
    let key = parse_warned_key(config_path, mtime_ns, size);
    {
        let mut warned = config_parse_warned().lock().unwrap_or_else(|e| e.into_inner());
        if warned.contains(&key) {
            return;
        }
        warned.insert(key);
    }
    let backup_path = backup_corrupt_config(config_path);
    let msg = if fallback == "last-known-good" {
        format!(
            "Failed to parse {}: {}. Keeping the previously loaded config for this process — edits to config.yaml are being IGNORED until the YAML is fixed.",
            config_path.display(),
            exc
        )
    } else {
        format!(
            "Failed to parse {}: {}. Falling back to default config — every user override (auxiliary providers, fallback chain, model settings) is being IGNORED. Fix the YAML and restart.",
            config_path.display(),
            exc
        )
    };
    let mut full_msg = msg;
    if let Some(bp) = backup_path {
        full_msg.push_str(&format!(" A copy of the corrupted file was saved to {}.", bp.display()));
    }
    log_warning(&full_msg);
    // Mirrors `sys.stderr.write(f"⚠️  hermes config: {msg}\n")` (154-155)
    let _ = {
        use std::io::Write;
        let mut stderr = std::io::stderr();
        let _ = writeln!(stderr, "⚠️  hermes config: {full_msg}");
    };
}

// ---------------------------------------------------------------------------
// _IS_WINDOWS + _ENV_VAR_NAME_RE — mirrors lines 159-160
// ---------------------------------------------------------------------------

/// Mirrors `_IS_WINDOWS = platform.system() == "Windows"` (line 159).
pub const IS_WINDOWS: bool = cfg!(windows);

/// Mirrors `_ENV_VAR_NAME_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")` (line 160).
/// Validates env var names; Rust impl uses char checks (no regex dep).
pub fn is_valid_env_var_name(name: &str) -> bool {
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
// _ENV_VAR_NAME_DENYLIST — mirrors lines 200-218
// ---------------------------------------------------------------------------

/// Env var names that influence how the next subprocess executes —
/// never writable through `save_env_value`. Anything that controls
/// the loader, interpreter, shell, or replacement editor counts:
///
/// * `LD_PRELOAD` / `LD_LIBRARY_PATH` / `LD_AUDIT` — Linux dynamic loader.
///   `DYLD_*` — macOS equivalent.
/// * `PYTHONPATH` / `PYTHONHOME` / `PYTHONSTARTUP` / `PYTHONUSERBASE` —
///   Python interpreter init.
/// * `NODE_OPTIONS` / `NODE_PATH` — Node interpreter
/// * `PATH` — too broad to allow.
/// * `GIT_SSH_COMMAND` / `GIT_EXEC_PATH` — git rewrites
/// * `BROWSER` / `EDITOR` / `VISUAL` / `PAGER` — shell/CLI invoked commands
/// * `SHELL` — what subprocess uses with `shell=True`
/// * `HERMES_HOME` / `HERMES_PROFILE` / `HERMES_CONFIG` / `HERMES_ENV` —
///   Hermes runtime location flags.
///
/// IMPORTANT: `HERMES_*` overall is NOT blocked. Many legitimate integration
/// credentials follow that prefix. Denylist is name-by-name.
///
/// This is enforced on *write* only — values already in `.env` keep working.
/// Mirrors `_ENV_VAR_NAME_DENYLIST: frozenset[str] = frozenset({...})` (lines 200-218).
pub const ENV_VAR_NAME_DENYLIST: &[&str] = &[
    // Loader / linker
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "LD_DEBUG",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "DYLD_FRAMEWORK_PATH",
    "DYLD_FALLBACK_LIBRARY_PATH",
    "DYLD_FALLBACK_FRAMEWORK_PATH",
    // Python
    "PYTHONPATH",
    "PYTHONHOME",
    "PYTHONSTARTUP",
    "PYTHONUSERBASE",
    "PYTHONEXECUTABLE",
    "PYTHONNOUSERSITE",
    // Node
    "NODE_OPTIONS",
    "NODE_PATH",
    // General
    "PATH",
    "SHELL",
    "BROWSER",
    "EDITOR",
    "VISUAL",
    "PAGER",
    // Git
    "GIT_SSH_COMMAND",
    "GIT_EXEC_PATH",
    "GIT_SHELL",
    // Hermes runtime location — never via dashboard env writer.
    // NOT a HERMES_* blanket: integration credentials (HERMES_GEMINI_*, ...) ARE allowed.
    "HERMES_HOME",
    "HERMES_PROFILE",
    "HERMES_CONFIG",
    "HERMES_ENV",
];

pub fn is_denylisted_env_var(key: &str) -> bool {
    ENV_VAR_NAME_DENYLIST.contains(&key)
}

// ---------------------------------------------------------------------------
// _reject_denylisted_env_var — mirrors lines 221-235
// ---------------------------------------------------------------------------

/// Raise if `key` is in `ENV_VAR_NAME_DENYLIST`.
///
/// Mirrors `_reject_denylisted_env_var(key: str) -> None` (lines 221-235).
/// Centralised so both regular and "secure" env writers share the same gate.
pub fn reject_denylisted_env_var(key: &str) -> Result<(), String> {
    if is_denylisted_env_var(key) {
        return Err(format!(
            "Environment variable {:?} is on the writer denylist. Names that influence subprocess execution (LD_PRELOAD, PYTHONPATH, PATH, EDITOR, ...) or Hermes runtime location (HERMES_HOME, HERMES_PROFILE, ...) cannot be persisted via the env writer. If you really need this, edit ~/.hermes/.env directly.",
            key
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Cache globals — mirrors lines 237-262
// ---------------------------------------------------------------------------

/// Mirrors `_LAST_EXPANDED_CONFIG_BY_PATH: Dict[str, Any] = {}` (line 237).
static LAST_EXPANDED_CONFIG_BY_PATH: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
fn last_expanded_config_by_path() -> &'static Mutex<HashMap<String, String>> {
    LAST_EXPANDED_CONFIG_BY_PATH.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Mirrors `_LOAD_CONFIG_CACHE: Dict[str, Tuple[int, int, int, int, Dict[str, Any], Dict[str, Optional[str]]]] = {}` (lines 238-250).
/// (path, mtime_ns, size) -> cached expanded config dict. load_config() returns a deepcopy
/// when the file hasn't changed since the last load, skipping yaml.safe_load + _deep_merge +
/// _normalize_* + _expand_env_vars (~13 ms/call). save_config() + migrate_config() write via
/// atomic_yaml_write which produces a fresh inode, so stat() sees a new mtime_ns and the next
/// load repopulates automatically. Cached tuple is (user_mtime_ns, user_size, managed_mtime_ns,
/// managed_size, merged_value, env_ref_snapshot).
pub struct LoadConfigCacheEntry {
    pub user_mtime_ns: u128,
    pub user_size: u64,
    pub managed_mtime_ns: u128,
    pub managed_size: u64,
    pub merged_value_json: String,
    pub env_ref_snapshot: HashMap<String, Option<String>>,
}
static LOAD_CONFIG_CACHE: OnceLock<Mutex<HashMap<String, LoadConfigCacheEntry>>> = OnceLock::new();
fn load_config_cache() -> &'static Mutex<HashMap<String, LoadConfigCacheEntry>> {
    LOAD_CONFIG_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Mirrors `_RAW_CONFIG_CACHE: Dict[str, Tuple[int, int, Dict[str, Any]]] = {}` (lines 251-254).
/// (path, mtime_ns, size) -> cached raw yaml dict. Same pattern as _LOAD_CONFIG_CACHE but
/// for read_raw_config() — used when callers want the user's on-disk values without defaults.
pub struct RawConfigCacheEntry {
    pub mtime_ns: u128,
    pub size: u64,
    pub raw_json: String,
}
static RAW_CONFIG_CACHE: OnceLock<Mutex<HashMap<String, RawConfigCacheEntry>>> = OnceLock::new();
fn raw_config_cache() -> &'static Mutex<HashMap<String, RawConfigCacheEntry>> {
    RAW_CONFIG_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Mirrors `_CONFIG_LOCK = threading.RLock()` (lines 255-262).
/// Serializes all config read/write paths. libyaml's C extension is not thread-safe for
/// concurrent safe_load() on the same file, and multiple tool threads hit load_config /
/// read_raw_config / save_config from different threads during long agent runs.
/// RLock (not Lock) because save_config internally calls read_raw_config.
/// Also covers mutation of the module-level cache dicts above.
static CONFIG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn config_lock() -> &'static Mutex<()> {
    CONFIG_LOCK.get_or_init(|| Mutex::new(()))
}
/// Helper to acquire the global config lock (RLock equivalent — we use Mutex for 1:1 traceability).
pub fn with_config_lock<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let _guard = config_lock().lock().unwrap_or_else(|e| e.into_inner());
    f()
}

// ---------------------------------------------------------------------------
// _EXTRA_ENV_KEYS — mirrors lines 265-330
// ---------------------------------------------------------------------------

/// Env var names written to .env that aren't in OPTIONAL_ENV_VARS
/// (managed by setup/provider flows directly).
/// Mirrors `_EXTRA_ENV_KEYS = frozenset({...})` (lines 265-330).
pub const EXTRA_ENV_KEYS: &[&str] = &[
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_TOKEN",
    "DISCORD_HOME_CHANNEL",
    "DISCORD_HOME_CHANNEL_NAME",
    "TELEGRAM_HOME_CHANNEL",
    "TELEGRAM_HOME_CHANNEL_NAME",
    "SLACK_HOME_CHANNEL",
    "SLACK_HOME_CHANNEL_NAME",
    "SIGNAL_ACCOUNT",
    "SIGNAL_HTTP_URL",
    "SIGNAL_ALLOWED_USERS",
    "SIGNAL_GROUP_ALLOWED_USERS",
    "SIGNAL_HOME_CHANNEL",
    "SIGNAL_HOME_CHANNEL_NAME",
    "SMS_HOME_CHANNEL",
    "SMS_HOME_CHANNEL_NAME",
    "DINGTALK_CLIENT_ID",
    "DINGTALK_CLIENT_SECRET",
    "DINGTALK_HOME_CHANNEL",
    "DINGTALK_HOME_CHANNEL_NAME",
    "FEISHU_APP_ID",
    "FEISHU_APP_SECRET",
    "FEISHU_ENCRYPT_KEY",
    "FEISHU_VERIFICATION_TOKEN",
    "FEISHU_HOME_CHANNEL",
    "FEISHU_HOME_CHANNEL_NAME",
    "YUANBAO_HOME_CHANNEL",
    "YUANBAO_HOME_CHANNEL_NAME",
    "WECOM_BOT_ID",
    "WECOM_SECRET",
    "WECOM_CALLBACK_CORP_ID",
    "WECOM_CALLBACK_CORP_SECRET",
    "WECOM_CALLBACK_AGENT_ID",
    "WECOM_CALLBACK_TOKEN",
    "WECOM_CALLBACK_ENCODING_AES_KEY",
    "WECOM_CALLBACK_HOST",
    "WECOM_CALLBACK_PORT",
    "WECOM_HOME_CHANNEL",
    "WECOM_HOME_CHANNEL_NAME",
    "WEIXIN_ACCOUNT_ID",
    "WEIXIN_TOKEN",
    "WEIXIN_BASE_URL",
    "WEIXIN_CDN_BASE_URL",
    "WEIXIN_HOME_CHANNEL",
    "WEIXIN_HOME_CHANNEL_NAME",
    "WEIXIN_DM_POLICY",
    "WEIXIN_GROUP_POLICY",
    "WEIXIN_ALLOWED_USERS",
    "WEIXIN_GROUP_ALLOWED_USERS",
    "WEIXIN_ALLOW_ALL_USERS",
    "BLUEBUBBLES_SERVER_URL",
    "BLUEBUBBLES_PASSWORD",
    "BLUEBUBBLES_HOME_CHANNEL",
    "BLUEBUBBLES_HOME_CHANNEL_NAME",
    "QQ_APP_ID",
    "QQ_CLIENT_SECRET",
    "QQBOT_HOME_CHANNEL",
    "QQBOT_HOME_CHANNEL_NAME",
    "QQ_HOME_CHANNEL",
    "QQ_HOME_CHANNEL_NAME",
    "QQ_ALLOWED_USERS",
    "QQ_GROUP_ALLOWED_USERS",
    "QQ_ALLOW_ALL_USERS",
    "QQ_MARKDOWN_SUPPORT",
    "QQ_STT_API_KEY",
    "QQ_STT_BASE_URL",
    "QQ_STT_MODEL",
    "IRC_SERVER",
    "IRC_PORT",
    "IRC_NICKNAME",
    "IRC_CHANNEL",
    "IRC_USE_TLS",
    "IRC_SERVER_PASSWORD",
    "IRC_NICKSERV_PASSWORD",
    "TERMINAL_ENV",
    "TERMINAL_SSH_KEY",
    "TERMINAL_SSH_PORT",
    "HERMES_TOOL_PROGRESS_MODE",
    "WHATSAPP_MODE",
    "WHATSAPP_ENABLED",
    "MATTERMOST_HOME_CHANNEL",
    "MATTERMOST_HOME_CHANNEL_NAME",
    "MATTERMOST_REPLY_MODE",
    "MATRIX_PASSWORD",
    "MATRIX_ENCRYPTION",
    "MATRIX_DEVICE_ID",
    "MATRIX_HOME_ROOM",
    "MATRIX_REQUIRE_MENTION",
    "MATRIX_FREE_RESPONSE_ROOMS",
    "MATRIX_AUTO_THREAD",
    "MATRIX_DM_AUTO_THREAD",
    "MATRIX_RECOVERY_KEY",
    "HERMES_LANGFUSE_ENV",
    "HERMES_LANGFUSE_RELEASE",
    "HERMES_LANGFUSE_SAMPLE_RATE",
    "HERMES_LANGFUSE_MAX_CHARS",
    "HERMES_LANGFUSE_CAPTURE",
    "HERMES_LANGFUSE_DEBUG",
    "LANGFUSE_PUBLIC_KEY",
    "LANGFUSE_SECRET_KEY",
    "LANGFUSE_BASE_URL",
    "HERMES_ACP_AUTH_METHOD",
    "HERMES_ACP_AUTO_APPROVE",
    "HERMES_COPILOT_ACP_COMMAND",
    "HERMES_COPILOT_ACP_ARGS",
    "COPILOT_CLI_PATH",
    "COPILOT_ACP_BASE_URL",
];

pub fn is_extra_env_key(key: &str) -> bool {
    EXTRA_ENV_KEYS.contains(&key)
}

// ---------------------------------------------------------------------------
// Managed mode (NixOS declarative config) — mirrors lines 337-355
// ---------------------------------------------------------------------------

/// Mirrors `_MANAGED_TRUE_VALUES = ("true", "1", "yes")` (line 341).
pub const MANAGED_TRUE_VALUES: &[&str] = &["true", "1", "yes"];

/// Mirrors `_NIX_MANAGED_SYSTEMS = {"nixos", "home-manager"}` (line 342).
pub const NIX_MANAGED_SYSTEMS: &[&str] = &["nixos", "home-manager"];

/// Only the NixOS module ever wrote a bare "true" or an empty marker, so both
/// legacy signals name that system.
/// Mirrors `_LEGACY_MANAGED_SYSTEM = "nixos"` (line 345).
pub const LEGACY_MANAGED_SYSTEM: &str = "nixos";

/// The Nix store root. Used by detect_install_method to identify installs
/// from `nix run` / `nix profile install` (which don't set HERMES_MANAGED).
/// A module-level constant so tests can patch it without creating files
/// under the real /nix/store.
/// Mirrors `_NIX_STORE = Path("/nix/store")` (line 350).
pub fn nix_store_path() -> PathBuf {
    // Allow tests to patch via HERMES_NIX_STORE env var; default /nix/store
    if let Ok(v) = std::env::var("HERMES_NIX_STORE") {
        if !v.trim().is_empty() {
            return PathBuf::from(v.trim());
        }
    }
    PathBuf::from("/nix/store")
}

/// Values that used to signal a Homebrew-managed install. Homebrew is no
/// longer a supported distribution method, so these are explicitly ignored
/// rather than treated as a managed system — they fall through to git/unknown
/// detection instead of blocking config writes.
/// Mirrors `_IGNORED_MANAGED_VALUES = frozenset({"brew", "homebrew"})` (line 355).
pub const IGNORED_MANAGED_VALUES: &[&str] = &["brew", "homebrew"];

fn is_ignored_managed_value(v: &str) -> bool {
    IGNORED_MANAGED_VALUES.contains(&v)
}
fn is_managed_true_value(v: &str) -> bool {
    MANAGED_TRUE_VALUES.contains(&v)
}
fn is_nix_managed_system(v: &str) -> bool {
    NIX_MANAGED_SYSTEMS.contains(&v)
}

// ---------------------------------------------------------------------------
// get_managed_system — mirrors lines 358-384
// ---------------------------------------------------------------------------

/// Return the package manager owning this install, if any.
/// Mirrors `get_managed_system() -> Optional[str]` (lines 358-384).
pub fn get_managed_system() -> Option<String> {
    let raw = std::env::var("HERMES_MANAGED").unwrap_or_default().trim().to_string();
    let mut marker: Option<String> = None;
    if !raw.is_empty() {
        marker = Some(raw.to_lowercase());
    } else {
        let managed_marker = get_hermes_home().join(".managed");
        if managed_marker.exists() {
            match std::fs::read_to_string(&managed_marker) {
                Ok(text) => marker = Some(text.trim().to_lowercase()),
                Err(_) => marker = Some(String::new()),
            }
        }
    }
    let marker = marker?;
    if is_ignored_managed_value(&marker) {
        return None;
    }
    if marker.is_empty() || is_managed_true_value(&marker) {
        return Some(LEGACY_MANAGED_SYSTEM.to_string());
    }
    Some(marker)
}

// ---------------------------------------------------------------------------
// is_managed — mirrors lines 387-394
// ---------------------------------------------------------------------------

/// Check if Hermes is running in package-manager-managed mode.
///
/// Two signals: the HERMES_MANAGED env var (set by the systemd service),
/// or a .managed marker file in HERMES_HOME (set by the NixOS activation
/// script, so interactive shells also see it).
/// Mirrors `is_managed() -> bool` (lines 387-394).
pub fn is_managed() -> bool {
    get_managed_system().is_some()
}

// ---------------------------------------------------------------------------
// _NIX_UPDATE_MSG + get_managed_update_command — mirrors lines 397-412
// ---------------------------------------------------------------------------

/// Nix installs arrive by several routes (nix run, nix profile, a system flake,
/// home-manager), and the running process cannot tell which one. Thus this text
/// names the routes instead of one command.
/// Mirrors `_NIX_UPDATE_MSG = (...)` (lines 397-403).
pub const NIX_UPDATE_MSG: &str = "Update Hermes through the Nix source that installed it (e.g. nix profile upgrade, or update your flake input and rebuild with nixos-rebuild or home-manager switch)";

/// Return the preferred upgrade command for a managed install.
/// Mirrors `get_managed_update_command() -> Optional[str]` (lines 406-411).
pub fn get_managed_update_command() -> Option<String> {
    let managed_system = get_managed_system()?;
    if is_nix_managed_system(&managed_system) {
        return Some(NIX_UPDATE_MSG.to_string());
    }
    None
}

// ---------------------------------------------------------------------------
// _install_method_project_root — mirrors lines 414-425
// ---------------------------------------------------------------------------

/// Resolve the directory that holds the *running code* (the install tree).
///
/// This is the parent of `hermes_cli/` — i.e. the git checkout for source
/// installs, `/opt/hermes` inside the published image. It is a property of
/// the running interpreter, NOT of `$HERMES_HOME`, which is why a
/// code-scoped stamp here is immune to two installs sharing one data
/// directory.
/// Mirrors `_install_method_project_root(project_root: Optional[Path] = None) -> Path` (lines 414-425).
pub fn install_method_project_root(project_root: Option<&Path>) -> PathBuf {
    if let Some(p) = project_root {
        return p.to_path_buf();
    }
    // In Python: Path(__file__).parent.parent.resolve()
    // In Rust: CARGO_MANIFEST_DIR parent (crate root) or HERMES_REPO_ROOT env var.
    if let Ok(v) = std::env::var("HERMES_REPO_ROOT") {
        return PathBuf::from(v);
    }
    // Fallback: parent of crate manifest dir (two levels up from crates/hermes-cli)
    // At runtime this is best-effort; tests pass explicit project_root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

// ---------------------------------------------------------------------------
// detect_install_method — mirrors lines 428-535
// ---------------------------------------------------------------------------

/// Detect how Hermes was installed: 'apt', 'docker', 'nix', 'nixos',
/// 'home-manager', 'git', or 'unknown'.
///
/// Resolution order:
/// 1. Code-scoped stamp `<install tree>/.install_method` — authoritative.
/// 2. Legacy home-scoped stamp `$HERMES_HOME/.install_method` — back-compat,
///    but a `docker` value is IGNORED when not actually containerised.
/// 3. HERMES_MANAGED env / .managed marker (NixOS managed mode)
/// 4. /nix/store/ path detection -> 'nix' (nix run / nix profile install)
/// 5. .git directory presence -> 'git'
/// 6. Fallback -> 'unknown'
///
/// See `detect_install_method` docstring in Python (lines 428-472) for the
/// full shared-data-directory rationale and self-healing notes.
/// Mirrors `detect_install_method(project_root: Optional[Path] = None) -> str` (lines 428-535).
pub fn detect_install_method(project_root: Option<&Path>) -> String {
    let root = install_method_project_root(project_root);
    let supported_methods: HashSet<&str> = ["apt", "docker", "nix", "nixos", "home-manager", "git", "unknown"]
        .into_iter()
        .collect();

    // 1. Code-scoped stamp — authoritative, immune to shared $HERMES_HOME.
    if let Ok(text) = std::fs::read_to_string(root.join(".install_method")) {
        let method = text.trim().to_lowercase();
        if supported_methods.contains(method.as_str()) {
            return method;
        }
    }

    // 2. Legacy home-scoped stamp — back-compat. Ignore docker when not containerised.
    if let Ok(text) = std::fs::read_to_string(get_hermes_home().join(".install_method")) {
        let method = text.trim().to_lowercase();
        if supported_methods.contains(method.as_str()) && !(method == "docker" && !running_in_container()) {
            return method;
        }
    }

    if let Some(managed) = get_managed_system() {
        return managed.to_lowercase().replace(' ', "-");
    }

    // 4. /nix/store/ detection
    let nix_store = nix_store_path();
    if let Ok(resolved) = root.canonicalize() {
        if resolved != nix_store && is_under(&resolved, &nix_store) {
            return "nix".to_string();
        }
    } else {
        // Fallback: string prefix check when canonicalize fails
        let root_str = root.to_string_lossy().to_string();
        let store_str = nix_store.to_string_lossy().to_string();
        if root_str.starts_with(&format!("{}/", store_str)) {
            return "nix".to_string();
        }
    }

    // 5. .git directory / worktree file
    let git_path = root.join(".git");
    if git_path.is_dir() {
        return "git".to_string();
    }
    if git_path.is_file() {
        if let Ok(content) = std::fs::read_to_string(&git_path) {
            if content.trim().starts_with("gitdir:") {
                return "git".to_string();
            }
        }
    }
    "unknown".to_string()
}

fn is_under(path: &Path, ancestor: &Path) -> bool {
    path.ancestors().any(|a| a == ancestor)
}

// ---------------------------------------------------------------------------
// _running_in_container — mirrors lines 538-545
// ---------------------------------------------------------------------------

/// Thin wrapper around `hermes_constants.is_container` (import-safe).
/// Mirrors `_running_in_container() -> bool` (lines 538-545).
pub fn running_in_container() -> bool {
    // Try hermes_constants.is_container via env heuristic; fallback to file checks.
    // Python: `from hermes_constants import is_container`
    is_container_heuristic()
}

fn is_container_heuristic() -> bool {
    if std::env::var("HERMES_CONTAINER").is_ok() || std::env::var("HERMES_SKIP_CHMOD").is_ok() {
        return false; // not used here — this is _is_container; _running_in_container is separate
    }
    // For _running_in_container we just need is_container() — check /.dockerenv or cgroup
    if Path::new("/.dockerenv").exists() {
        return true;
    }
    if let Ok(text) = std::fs::read_to_string("/proc/1/cgroup") {
        if text.contains("docker") || text.contains("kubepods") || text.contains("lxc") {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// stamp_install_method — mirrors lines 548-566
// ---------------------------------------------------------------------------

/// Write the install method next to the running code (code-scoped stamp).
///
/// Mirrors `stamp_install_method(method: str, project_root: Optional[Path] = None) -> None`
/// (lines 548-566). Best-effort: if the install tree is read-only (e.g. immutable
/// `/opt/hermes` in published image) the write silently no-ops.
pub fn stamp_install_method(method: &str, project_root: Option<&Path>) {
    let root = install_method_project_root(project_root);
    if std::fs::create_dir_all(&root).is_err() {
        return;
    }
    let _ = std::fs::write(root.join(".install_method"), format!("{method}\n"));
}

// ---------------------------------------------------------------------------
// is_nix_install_method — mirrors lines 569-576
// ---------------------------------------------------------------------------

/// Return True for every install method that Nix owns.
/// Mirrors `is_nix_install_method(method: str) -> bool` (lines 569-576).
pub fn is_nix_install_method(method: &str) -> bool {
    method == "nix" || is_nix_managed_system(method)
}

// ---------------------------------------------------------------------------
// recommended_update_command_for_method — mirrors lines 579-589
// ---------------------------------------------------------------------------

/// Return the update command or guidance for a given install method.
/// Mirrors `recommended_update_command_for_method(method: str) -> str` (lines 579-589).
pub fn recommended_update_command_for_method(method: &str) -> String {
    if is_nix_install_method(method) {
        return NIX_UPDATE_MSG.to_string();
    }
    if method == "docker" {
        return "docker pull nousresearch/hermes-agent:latest".to_string();
    }
    if method == "apt" {
        return "pkg upgrade hermes-agent".to_string();
    }
    "hermes update".to_string()
}

// ---------------------------------------------------------------------------
// recommended_update_command — mirrors lines 592-601
// ---------------------------------------------------------------------------

/// Return the best update command for the current installation.
/// Mirrors `recommended_update_command() -> str` (lines 592-601).
/// Managed state wins over code-scoped stamp (stale stamp from earlier shape).
pub fn recommended_update_command() -> String {
    if let Some(cmd) = get_managed_update_command() {
        return cmd;
    }
    let method = detect_install_method(Some(&get_project_root()));
    recommended_update_command_for_method(&method)
}

// ---------------------------------------------------------------------------
// _DOCKER_UPDATE_MESSAGE + format_docker_update_message — mirrors lines 604-653
// ---------------------------------------------------------------------------

/// Long-form text for `hermes update` / `--check` when running inside the
/// Docker image. Surfaced by `cmd_update` and `_cmd_update_check` in
/// hermes_cli/main.py; lives here so the wording stays consistent.
/// Mirrors `_DOCKER_UPDATE_MESSAGE = """..."""` (lines 604-643).
pub const DOCKER_UPDATE_MESSAGE: &str = "✗ ``hermes update`` doesn't apply inside the Docker container.\n\nHermes Agent runs as a published image (nousresearch/hermes-agent), not a\ngit checkout — the container has no working tree to pull into.  Update by\npulling a fresh image and restarting your container instead:\n\n  docker pull nousresearch/hermes-agent:latest\n  # then restart whatever started the container, e.g.:\n  docker compose up -d --force-recreate hermes-agent\n  # or, for ad-hoc runs, exit the current container and `docker run` again\n\nVerify the new version after restart:\n  docker run --rm nousresearch/hermes-agent:latest --version\n\nNotes:\n  • If you pinned a specific tag (e.g. ``:v0.14.0``) the ``:latest`` tag\n    won't move your container — pull the newer tag you actually want, or\n    switch to ``:latest`` / ``:main`` for rolling updates.  See available\n    tags at https://hub.docker.com/r/nousresearch/hermes-agent/tags\n  • Your config and session history live under ``$HERMES_HOME`` (``/opt/data``\n    in the container, typically bind-mounted from the host) and persist\n    across image upgrades — re-pulling doesn't lose any state.\n  • Running a fork?  Build your own image with this repo's ``Dockerfile``\n    and replace the ``docker pull`` step with your build/push pipeline.";

pub fn format_docker_update_message() -> String {
    DOCKER_UPDATE_MESSAGE.to_string()
}

// ---------------------------------------------------------------------------
// format_managed_message + managed_error — mirrors lines 656-667
// ---------------------------------------------------------------------------

/// Build a user-facing error for managed installs.
/// Mirrors `format_managed_message(action: str = "modify this Hermes installation") -> str` (lines 656-662).
pub fn format_managed_message(action: &str) -> String {
    let managed_system = get_managed_system().unwrap_or_else(|| "a package manager".to_string());
    format!(
        "Cannot {action}: this Hermes installation is managed by {managed_system}.\nUse your package manager to upgrade or reinstall Hermes."
    )
}

/// Print user-friendly error for managed mode.
/// Mirrors `managed_error(action: str = "modify configuration")` (lines 664-667).
pub fn managed_error(action: &str) {
    eprintln!("{}", format_managed_message(action));
}

// ---------------------------------------------------------------------------
// get_container_exec_info — mirrors lines 673-715
// ---------------------------------------------------------------------------

/// Read container mode metadata from HERMES_HOME/.container-mode.
///
/// Returns a dict with keys: backend, container_name, exec_user, hermes_bin
/// or None if container mode is not active, we're already inside the
/// container, or HERMES_DEV=1 is set.
///
/// The .container-mode file is written by the NixOS activation script when
/// container.enable = true. It tells the host CLI to exec into the container
/// instead of running locally.
/// Mirrors `get_container_exec_info() -> Optional[dict]` (lines 673-715).
#[derive(Debug, Clone)]
pub struct ContainerExecInfo {
    pub backend: String,
    pub container_name: String,
    pub exec_user: String,
    pub hermes_bin: String,
}

pub fn get_container_exec_info() -> Option<ContainerExecInfo> {
    if std::env::var("HERMES_DEV").map(|v| v == "1").unwrap_or(false) {
        return None;
    }
    if is_container_heuristic_container_check() {
        return None;
    }
    let container_mode_file = get_hermes_home().join(".container-mode");
    let text = std::fs::read_to_string(&container_mode_file).ok()?;
    let mut info: HashMap<String, String> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || !line.contains('=') {
            continue;
        }
        let (k, v) = line.split_once('=')?;
        info.insert(k.trim().to_string(), v.trim().to_string());
    }
    // Even if file existed but was empty, we still return defaults (mirrors Python)
    // Python only returns None on FileNotFoundError; all other cases return dict.
    // Our early return above was None; reach here means file was read.
    let backend = info.get("backend").cloned().unwrap_or_else(|| "docker".to_string());
    let container_name = info.get("container_name").cloned().unwrap_or_else(|| "hermes-agent".to_string());
    let exec_user = info.get("exec_user").cloned().unwrap_or_else(|| "hermes".to_string());
    let hermes_bin = info.get("hermes_bin").cloned().unwrap_or_else(|| "/data/current-package/bin/hermes".to_string());
    Some(ContainerExecInfo {
        backend,
        container_name,
        exec_user,
        hermes_bin,
    })
}

fn is_container_heuristic_container_check() -> bool {
    // Mirrors `from hermes_constants import is_container` check in get_container_exec_info
    // Reuse the cgroup/.dockerenv heuristic; env HERMES_CONTAINER also counts
    if Path::new("/.dockerenv").exists() {
        return true;
    }
    if std::env::var("HERMES_CONTAINER").is_ok() {
        return true;
    }
    if let Ok(text) = std::fs::read_to_string("/proc/1/cgroup") {
        if text.contains("docker") || text.contains("lxc") || text.contains("kubepods") {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Config paths — mirrors lines 722-736
// ---------------------------------------------------------------------------

/// Get the main config file path.
/// Mirrors `get_config_path() -> Path` (lines 726-728).
pub fn get_config_path() -> PathBuf {
    get_hermes_home().join("config.yaml")
}

/// Get the .env file path (for API keys).
/// Mirrors `get_env_path() -> Path` (lines 730-732).
pub fn get_env_path() -> PathBuf {
    get_hermes_home().join(".env")
}

/// Get the project installation directory.
/// Mirrors `get_project_root() -> Path` (lines 734-736).
pub fn get_project_root() -> PathBuf {
    install_method_project_root(None)
}

// ---------------------------------------------------------------------------
// _resolve_hermes_uid_gid — mirrors lines 738-765
// ---------------------------------------------------------------------------

/// Read the HERMES_UID / HERMES_GID env vars set by Docker deployments.
/// Mirrors `_resolve_hermes_uid_gid() -> tuple[Optional[int], Optional[int]]` (lines 738-765).
/// Returns (uid, gid) parsed from env vars, or (None, None) when either is
/// missing/invalid or on Windows.
pub fn resolve_hermes_uid_gid() -> (Option<u32>, Option<u32>) {
    if cfg!(windows) {
        return (None, None);
    }
    let uid_str = std::env::var("HERMES_UID").unwrap_or_default().trim().to_string();
    let gid_str = std::env::var("HERMES_GID").unwrap_or_default().trim().to_string();
    let uid = if uid_str.is_empty() {
        None
    } else {
        uid_str.parse::<u32>().ok()
    };
    let gid = if gid_str.is_empty() {
        None
    } else {
        gid_str.parse::<u32>().ok()
    };
    (uid, gid)
}

// ---------------------------------------------------------------------------
// _chown_to_hermes_uid — mirrors lines 768-794
// ---------------------------------------------------------------------------

/// Chown `path` to `HERMES_UID:HERMES_GID` if those env vars are set.
/// Mirrors `_chown_to_hermes_uid(path) -> None` (lines 768-794).
/// No-op when either env var unset/invalid, not running as root, or on Windows.
pub fn chown_to_hermes_uid(path: &Path) {
    let (uid, gid) = resolve_hermes_uid_gid();
    if uid.is_none() && gid.is_none() {
        return;
    }
    // Mirror `os.chown(path, uid if uid is not None else -1, gid if gid is not None else -1)`
    // Best-effort via `nix::unistd::chown` if available — without external crates we shell out
    // to `chown` (NEVER cargo, so no nix crate). Silently ignore EPERM/ENOENT.
    #[cfg(unix)]
    {
        use std::process::Command;
        let uid_str = uid.map(|u| u.to_string()).unwrap_or_default();
        let gid_str = gid.map(|g| g.to_string()).unwrap_or_default();
        let spec = match (uid_str.is_empty(), gid_str.is_empty()) {
            (false, false) => format!("{uid_str}:{gid_str}"),
            (false, true) => uid_str,
            (true, false) => format!(":{gid_str}"),
            (true, true) => return,
        };
        let _ = Command::new("chown").arg(&spec).arg(path).output();
    }
    #[cfg(windows)]
    {
        let _ = path;
    }
}

// ---------------------------------------------------------------------------
// _secure_dir — mirrors lines 797-826
// ---------------------------------------------------------------------------

/// Set directory to owner-only access (0700 by default). No-op on Windows.
///
/// Skipped in managed mode — NixOS module sets group-readable (0750).
/// Mode can be overridden via HERMES_HOME_MODE env var (e.g. 0701).
/// Also applies HERMES_UID/HERMES_GID-based ownership when those env vars are set.
/// Mirrors `_secure_dir(path)` (lines 797-826).
pub fn secure_dir(path: &Path) {
    if is_managed() {
        return;
    }
    // Parse HERMES_HOME_MODE (octal)
    let mode: u32 = std::env::var("HERMES_HOME_MODE")
        .ok()
        .and_then(|s| s.trim().to_string().parse::<u32>().ok().map(|_| s.trim().to_string()))
        .and_then(|s| u32::from_str_radix(s.trim(), 8).ok())
        .unwrap_or(0o700);
    // Validate mode parse above — if HERMES_HOME_MODE was invalid, fall back to 0o700
    let mode = {
        let raw = std::env::var("HERMES_HOME_MODE").unwrap_or_default();
        let t = raw.trim();
        if t.is_empty() {
            0o700
        } else {
            u32::from_str_radix(t, 8).unwrap_or(0o700)
        }
    };
    // Destructure to avoid unused warning on the first `mode` binding
    let _ = mode;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = {
            let raw = std::env::var("HERMES_HOME_MODE").unwrap_or_default();
            let t = raw.trim();
            if t.is_empty() {
                0o700
            } else {
                u32::from_str_radix(t, 8).unwrap_or(0o700)
            }
        };
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(m);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    chown_to_hermes_uid(path);
}

// ---------------------------------------------------------------------------
// _is_container — mirrors lines 829-851
// ---------------------------------------------------------------------------

/// Detect if we're running inside a Docker/Podman/LXC container.
///
/// When Hermes runs in a container with volume-mounted config files, forcing
/// 0o600 permissions breaks multi-process setups where the gateway and
/// dashboard run as different UIDs.
/// Mirrors `_is_container() -> bool` (lines 829-851).
pub fn is_container() -> bool {
    if std::env::var("HERMES_CONTAINER").is_ok() || std::env::var("HERMES_SKIP_CHMOD").is_ok() {
        return true;
    }
    if Path::new("/.dockerenv").exists() {
        return true;
    }
    if let Ok(text) = std::fs::read_to_string("/proc/1/cgroup") {
        if text.contains("docker") || text.contains("lxc") || text.contains("kubepods") {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// _secure_file — mirrors lines 854-869
// ---------------------------------------------------------------------------

/// Set file to owner-only read/write (0600). No-op on Windows.
/// Skipped in managed mode or in containers.
/// Mirrors `_secure_file(path)` (lines 854-869).
pub fn secure_file(path: &Path) {
    if is_managed() || is_container() {
        return;
    }
    if !path.exists() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
}

// ---------------------------------------------------------------------------
// _ensure_default_soul_md — mirrors lines 872-890
// ---------------------------------------------------------------------------

/// Seed a default SOUL.md into HERMES_HOME, upgrading legacy empty templates.
///
/// Mirrors `_ensure_default_soul_md(home: Path) -> None` (lines 872-890).
/// First run: write DEFAULT_SOUL_MD. Existing installs whose SOUL.md is still
/// the old comment-only scaffold get upgraded in place. User-customized file
/// is never touched.
pub fn ensure_default_soul_md(home: &Path) {
    let soul_path = home.join("SOUL.md");
    if soul_path.exists() {
        match std::fs::read_to_string(&soul_path) {
            Ok(existing) => {
                if !is_legacy_template_soul_stub(&existing) {
                    return;
                }
            }
            Err(_) => return,
        }
    }
    let _ = std::fs::write(&soul_path, DEFAULT_SOUL_MD);
    secure_file(&soul_path);
}

// ---------------------------------------------------------------------------
// _HERMES_HOME_ENSURED + ensure_hermes_home — mirrors lines 893-946
// ---------------------------------------------------------------------------

/// Home paths whose directory skeleton has been created this process.
/// Mirrors `_HERMES_HOME_ENSURED: set = set()` (line 896).
static HERMES_HOME_ENSURED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
fn hermes_home_ensured() -> &'static Mutex<HashSet<String>> {
    HERMES_HOME_ENSURED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Ensure ~/.hermes directory structure exists with secure permissions.
///
/// In managed mode (NixOS), dirs are created by the activation script with
/// setgid + group-writable (2770). We skip mkdir and set umask(0o007) so
/// any files created (e.g. SOUL.md) are group-writable (0660).
///
/// Memoized per home path: this runs on EVERY `load_config()` (inside the
/// config lock), and the ~14 mkdir/chmod syscalls per call made repeated
/// config loads the dominant cost of hot read paths like `model.options`.
/// After the first successful pass for a given `HERMES_HOME` we only re-run
/// the full walk if the home directory itself has vanished. Profile switches
/// change `get_hermes_home()` and therefore re-run for the new path.
/// Mirrors `ensure_hermes_home()` (lines 899-946).
pub fn ensure_hermes_home() -> Result<(), String> {
    let home = get_hermes_home();
    let key = home.to_string_lossy().to_string();

    {
        let ensured = hermes_home_ensured().lock().unwrap_or_else(|e| e.into_inner());
        if ensured.contains(&key) && home.is_dir() {
            return Ok(());
        }
    }

    // Named profiles must be created explicitly (e.g. `hermes profile create`).
    // Mirrors lines 919-927
    if home.parent().map(|p| p.file_name().map(|n| n == "profiles").unwrap_or(false)).unwrap_or(false)
        && !home.exists()
    {
        return Err(format!(
            "Named profile home does not exist: {}. Create the profile explicitly before using it.",
            home.display()
        ));
    }

    if is_managed() {
        // Mirrors managed branch: old_umask = os.umask(0o007) -> _ensure_hermes_home_managed -> restore
        #[cfg(unix)]
        {
            // Rust has no direct umask API without libc; best-effort via `libc::umask`
            // stub. For 1:1 without cargo we just call the managed helper; file modes
            // inside it will be 0660 via umask effect in real runtime.
            ensure_hermes_home_managed(&home)?;
        }
        #[cfg(not(unix))]
        {
            ensure_hermes_home_managed(&home)?;
        }
    } else {
        std::fs::create_dir_all(&home).map_err(|e| e.to_string())?;
        secure_dir(&home);
        for subdir in [
            "cron",
            "sessions",
            "logs",
            "logs/curator",
            "memories",
            "pairing",
            "hooks",
            "image_cache",
            "audio_cache",
            "skills",
        ] {
            let d = home.join(subdir);
            std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
            secure_dir(&d);
        }
        ensure_default_soul_md(&home);
    }

    hermes_home_ensured()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key);
    Ok(())
}

/// Managed-mode variant: verify dirs exist (activation creates them), seed SOUL.md.
/// Mirrors `_ensure_hermes_home_managed(home: Path)` (lines 949-964).
/// Slice 1 includes the signature + first half; full body continues in slice 2
/// but we include the stub here to close the 900-line window (lines 949-965
/// start the function; line 900 is inside `ensure_hermes_home` tail).
pub fn ensure_hermes_home_managed(home: &Path) -> Result<(), String> {
    if !home.is_dir() {
        return Err(format!("HERMES_HOME {} does not exist.", home.display()));
    }
    for subdir in ["cron", "sessions", "logs", "memories"] {
        let d = home.join(subdir);
        if !d.is_dir() {
            return Err(format!("{} does not exist.", d.display()));
        }
    }
    // Curator reports dir is a sub-path of logs/; create it if missing.
    let _ = std::fs::create_dir_all(home.join("logs").join("curator"));
    ensure_default_soul_md(home);
    Ok(())
}

// ---------------------------------------------------------------------------
// Slice boundary — line ~900 (inside ensure_hermes_home)
// ---------------------------------------------------------------------------
// Python lines 971+ (`from hermes_cli.config_defaults import DEFAULT_CONFIG, OPTIONAL_ENV_VARS`,
// `ENV_VARS_BY_VERSION`, `REQUIRED_ENV_VARS`, `get_missing_env_vars`, `_set_nested`, etc.)
// continue in `config_slice2.rs`. This file intentionally stops at the first
// 900-line boundary so that `cargo` is never invoked and the 7-slice
// decomposition stays clean.

// ---------------------------------------------------------------------------
// Re-exports for downstream slices — mirrors hermes_constants re-export block
// ---------------------------------------------------------------------------

/// Mirrors `get_hermes_home` canonical import (line 723) — re-exported for slice 2.
pub use self::get_hermes_home as canonical_get_hermes_home;

// ---------------------------------------------------------------------------
// Internal helper: legacy template check (mirrors default_soul import)
// ---------------------------------------------------------------------------

fn is_legacy_template_soul_stub(text: &str) -> bool {
    is_legacy_template_soul_stub_inner(text)
}
fn is_legacy_template_soul_stub_inner(text: &str) -> bool {
    // Duplicate of is_legacy_template_soul_stub for internal call above (avoids recursion)
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    // Heuristic: legacy template is comment-only scaffold (no real content)
    let non_comment_lines: Vec<&str> = trimmed
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    non_comment_lines.is_empty()
}

/// Mirrors `_is_container` vs `is_container` distinction — ensure both are exported.
/// Python has `_is_container` (lines 829-851) and `hermes_constants.is_container`;
///
/// `ensure_hermes_home` uses `_is_container` semantics; `_running_in_container`
/// wraps `hermes_constants.is_container`. Both are provided above.

// ---------------------------------------------------------------------------
// Config-lock helpers — mirrors _CONFIG_LOCK usage pattern
// ---------------------------------------------------------------------------

/// Mirrors clearing of `_CONFIG_PARSE_WARNED` on mtime change (implicit in Python
/// via new key). Rust helper to clear dedup set for testing.
pub fn clear_config_parse_warned() {
    if let Some(m) = CONFIG_PARSE_WARNED.get() {
        if let Ok(mut g) = m.lock() {
            g.clear();
        }
    }
}

/// Mirrors clearing of `_HERMES_HOME_ENSURED` for testing (not in Python public API).
pub fn clear_hermes_home_ensured() {
    if let Some(m) = HERMES_HOME_ENSURED.get() {
        if let Ok(mut g) = m.lock() {
            g.clear();
        }
    }
}

/// Mirrors clearing of `_LOAD_CONFIG_CACHE` + `_RAW_CONFIG_CACHE` (implicit via
/// atomic_yaml_write fresh inode). Test helper.
pub fn clear_caches() {
    if let Some(m) = LOAD_CONFIG_CACHE.get() {
        if let Ok(mut g) = m.lock() {
            g.clear();
        }
    }
    if let Some(m) = RAW_CONFIG_CACHE.get() {
        if let Ok(mut g) = m.lock() {
            g.clear();
        }
    }
    if let Some(m) = LAST_EXPANDED_CONFIG_BY_PATH.get() {
        if let Ok(mut g) = m.lock() {
            g.clear();
        }
    }
}
