//! hermes-cli profiles — slice 1/3
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/profiles.py`
//! slice 1/3 — lines 1–900 of 2 543 (first 900 LOC).
//! Covers: module docstring + imports, constants (`_PROFILE_ID_RE`,
//! `_WARNED_MISSING_ALLOWLIST_ENTRIES`, `_PROFILE_DIRS`, `_CLONE_CONFIG_FILES`,
//! `_CLONE_SUBDIR_FILES`, `_CLONE_ALL_STRIP`, `_CLONE_ALL_DEFAULT_EXCLUDE_ROOT`,
//! `_CLONE_ALL_HISTORY_EXCLUDE_ROOT`, `NO_BUNDLED_SKILLS_MARKER`,
//! `_DEFAULT_EXPORT_EXCLUDE_ROOT`, `_DEFAULT_EXPORT_INCLUDE_ROOT`,
//! `_RESERVED_NAMES`, `_HERMES_SUBCOMMANDS`), path helpers
//! (`_get_profiles_root`, `_get_default_hermes_home`, `_get_active_profile_path`,
//! `_get_wrapper_dir`), validation (`normalize_profile_name`, `validate_profile_name`,
//! `validate_alias_name`, `get_profile_dir`, `profile_exists`, `profile_matches_home`,
//! `list_profile_names`), alias/wrapper management (`check_alias_collision`,
//! `_is_wrapper_dir_in_path`, `create_wrapper_script`, `remove_wrapper_script`,
//! `_migrate_profile_config_if_outdated`, `find_alias_for_profile`,
//! `_WRAPPER_READ_LIMIT`, `build_alias_map`), `ProfileInfo` dataclass,
//! distribution/config/gateway helpers (`_read_distribution_meta`,
//! `_read_config_model`, `_check_gateway_running`), skill-count cache
//! (`_SKILL_COUNT_CACHE`, `_skills_dir_signature`, `_count_skills`), and
//! `profile.yaml` meta (`_profile_yaml_path`, `read_profile_meta` through line 901).
//! Continued in `profiles_slice2.rs` (from `write_profile_meta`, line 904).
//!
//! T0696 — 1:1 port, no cargo (NEVER cargo).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-20
// ---------------------------------------------------------------------------

/// Profile management for multiple isolated Hermes instances.
///
/// Each profile is a fully independent HERMES_HOME directory with its own
/// config.yaml, .env, memory, sessions, skills, gateway, cron, and logs.
/// Profiles live under `~/.hermes/profiles/<name>/` by default.
///
/// The "default" profile is `~/.hermes` itself — backward compatible,
/// zero migration needed.
///
/// Mirrors `hermes_cli/profiles.py` lines 1-20.
pub const MODULE_DOC: &str = "profiles: multiple isolated Hermes instances — see lines 1-20";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 22-37
// ---------------------------------------------------------------------------
// Python: json, logging, os, re, shlex, shutil, stat, subprocess, sys, time,
// dataclasses, pathlib (Path, PurePosixPath, PureWindowsPath), typing
// (Dict, List, Optional, Tuple), agent.skill_utils.is_excluded_skill_path
//
// Rust: std only (NEVER cargo). External crates and hermes-internal modules
// are stubbed for 1:1 traceability; real wiring in later slices.

/// Mirrors `agent.skill_utils.is_excluded_skill_path` (line 36).
/// Returns true for skill paths that should be excluded from counting/listing.
pub fn is_excluded_skill_path_stub(path: &Path) -> bool {
    let s = path.to_string_lossy();
    // Minimal stub: exclude hidden or backup-like paths; real logic in skill_utils.
    s.contains("/.archive/") || s.contains("/.backup/")
}

// Logger — mirrors line 38: logger = logging.getLogger(__name__)
pub fn log_warning(msg: &str) {
    eprintln!("[hermes profiles WARN] {msg}");
}
pub fn log_debug(msg: &str) {
    eprintln!("[hermes profiles DEBUG] {msg}");
}

// ---------------------------------------------------------------------------
// _PROFILE_ID_RE — mirrors line 40
// ---------------------------------------------------------------------------

/// Mirrors `_PROFILE_ID_RE = re.compile(r"^[a-z0-9][a-z0-9_-]{0,63}$")` (line 40).
/// Valid profile/alias identifier: lowercase alnum + _-, 1-64 chars, starts alnum.
pub const PROFILE_ID_RE_STR: &str = r"^[a-z0-9][a-z0-9_-]{0,63}$";

pub fn is_valid_profile_id(name: &str) -> bool {
    // std only — no regex crate (NEVER cargo). Mirrors the compiled regex exactly.
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {},
        _ => return false,
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// _WARNED_MISSING_ALLOWLIST_ENTRIES — mirrors line 41
// ---------------------------------------------------------------------------

/// Dedup set for missing allowlist warnings (line 41).
/// Python: set[tuple[str, ...]] — Rust uses HashSet<Vec<String>> + Mutex.
static WARNED_MISSING_ALLOWLIST_ENTRIES: OnceLock<Mutex<HashSet<Vec<String>>>> = OnceLock::new();

fn warned_missing_allowlist_entries() -> &'static Mutex<HashSet<Vec<String>>> {
    WARNED_MISSING_ALLOWLIST_ENTRIES.get_or_init(|| Mutex::new(HashSet::new()))
}

// ---------------------------------------------------------------------------
// _PROFILE_DIRS — mirrors lines 44-58
// ---------------------------------------------------------------------------

/// Directories bootstrapped inside every new profile (lines 44-58).
pub const PROFILE_DIRS: &[&str] = &[
    "memories",
    "sessions",
    "skills",
    "skins",
    "logs",
    "plans",
    "workspace",
    "cron",
    // Back-compat/Docker HOME for tool subprocesses. Host subprocesses keep
    // the user's real HOME by default so normal CLI credentials remain visible;
    // containers still use this directory for persistent HOME state.
    // See hermes_constants.get_subprocess_home().
    "home",
];

// ---------------------------------------------------------------------------
// _CLONE_CONFIG_FILES — mirrors lines 61-65
// ---------------------------------------------------------------------------

/// Files copied during --clone (if they exist in the source) (lines 61-65).
pub const CLONE_CONFIG_FILES: &[&str] = &["config.yaml", ".env", "SOUL.md"];

// ---------------------------------------------------------------------------
// _CLONE_SUBDIR_FILES — mirrors lines 70-73
// ---------------------------------------------------------------------------

/// Subdirectory files copied during --clone (path relative to profile root).
/// Memory files are part of the agent's curated identity — just as important
/// as SOUL.md for continuity when cloning a profile (lines 70-73).
pub const CLONE_SUBDIR_FILES: &[&str] = &["memories/MEMORY.md", "memories/USER.md"];

// ---------------------------------------------------------------------------
// _CLONE_ALL_STRIP — mirrors lines 78-82
// ---------------------------------------------------------------------------

/// Runtime files stripped after --clone-all (shouldn't carry over) (lines 78-82).
pub const CLONE_ALL_STRIP: &[&str] = &["gateway.pid", "gateway_state.json", "processes.json"];

// ---------------------------------------------------------------------------
// _CLONE_ALL_DEFAULT_EXCLUDE_ROOT — mirrors lines 100-106
// ---------------------------------------------------------------------------

/// Infrastructure artifacts excluded from --clone-all when the source is the
/// default profile (`~/.hermes`). Named profiles never contain these
/// directories at root, so the exclusion is gated to avoid silently dropping
/// user data from a named-profile source (lines 100-106).
pub const CLONE_ALL_DEFAULT_EXCLUDE_ROOT: &[&str] = &[
    "hermes-agent",
    ".worktrees",
    "profiles",
    "bin",
    "node_modules",
];

fn is_clone_all_default_excluded(name: &str) -> bool {
    CLONE_ALL_DEFAULT_EXCLUDE_ROOT.contains(&name)
}

// ---------------------------------------------------------------------------
// _CLONE_ALL_HISTORY_EXCLUDE_ROOT — mirrors lines 122-130
// ---------------------------------------------------------------------------

/// Per-profile history artifacts excluded from --clone-all regardless of the
/// source profile. A new profile is a fresh workspace — inheriting the source
/// profile's session history, backup archives, or quick-backup snapshots is
/// never useful and can balloon the copy by tens of GB (lines 122-130).
pub const CLONE_ALL_HISTORY_EXCLUDE_ROOT: &[&str] = &[
    "state.db",
    "state.db-wal",
    "state.db-shm",
    "sessions",
    "backups",
    "state-snapshots",
    "checkpoints",
];

fn is_clone_all_history_excluded(name: &str) -> bool {
    CLONE_ALL_HISTORY_EXCLUDE_ROOT.contains(&name)
}

// ---------------------------------------------------------------------------
// NO_BUNDLED_SKILLS_MARKER — mirrors line 138
// ---------------------------------------------------------------------------

/// Marker file written by `hermes profile create --no-skills` (lines 132-138).
pub const NO_BUNDLED_SKILLS_MARKER: &str = ".no-bundled-skills";

// ---------------------------------------------------------------------------
// has_bundled_skills_opt_out — mirrors lines 141-146
// ---------------------------------------------------------------------------

/// Mirrors `has_bundled_skills_opt_out(profile_dir: Path) -> bool` (lines 141-146).
pub fn has_bundled_skills_opt_out(profile_dir: &Path) -> bool {
    // Python: try (profile_dir / NO_BUNDLED_SKILLS_MARKER).exists() except OSError: return False
    match std::fs::metadata(profile_dir.join(NO_BUNDLED_SKILLS_MARKER)) {
        Ok(_) => profile_dir.join(NO_BUNDLED_SKILLS_MARKER).exists(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// _clone_all_copytree_ignore — mirrors lines 149-200
// ---------------------------------------------------------------------------

/// Mirrors `_clone_all_copytree_ignore(source_dir: Path)` (lines 149-200).
///
/// Returns a closure suitable for `shutil.copytree(ignore=...)` that excludes:
///   1. Root-level entries in `_CLONE_ALL_HISTORY_EXCLUDE_ROOT` (any source).
///   2. Root-level entries in `_CLONE_ALL_DEFAULT_EXCLUDE_ROOT` (default profile only).
///   3. Universal exclusions at any depth: `__pycache__`, `*.pyc`, `*.pyo`, `*.sock`, `*.tmp`.
///
/// In Rust we expose `clone_all_should_ignore(source_resolved, is_default, directory, entry)` for
/// testability, plus `clone_all_copytree_ignore(source_dir)` that returns the callable.
pub fn clone_all_copytree_ignore(
    source_dir: &Path,
) -> impl Fn(&str, &[String]) -> Vec<String> + '_ {
    let source_resolved = source_dir
        .canonicalize()
        .unwrap_or_else(|_| source_dir.to_path_buf());
    let is_default_source = source_resolved == get_default_hermes_home().canonicalize().unwrap_or_else(|_| get_default_hermes_home());
    move |directory: &str, names: &[String]| -> Vec<String> {
        let mut ignored: Vec<String> = Vec::new();
        for entry in names {
            // Universal exclusions at any depth.
            if entry == "__pycache__"
                || entry.ends_with(".pyc")
                || entry.ends_with(".pyo")
                || entry.ends_with(".sock")
                || entry.ends_with(".tmp")
            {
                ignored.push(entry.clone());
                continue;
            }
            let at_root = Path::new(directory)
                .canonicalize()
                .map(|p| p == source_resolved)
                .unwrap_or(false);
            if at_root {
                if is_clone_all_history_excluded(entry) {
                    ignored.push(entry.clone());
                    continue;
                }
                if is_default_source && is_clone_all_default_excluded(entry) {
                    ignored.push(entry.clone());
                }
            }
        }
        ignored
    }
}

/// Testable predicate form of the ignore logic (mirrors inner `_ignore`).
pub fn clone_all_should_ignore(
    source_resolved: &Path,
    is_default_source: bool,
    directory: &Path,
    entry: &str,
) -> bool {
    if entry == "__pycache__"
        || entry.ends_with(".pyc")
        || entry.ends_with(".pyo")
        || entry.ends_with(".sock")
        || entry.ends_with(".tmp")
    {
        return true;
    }
    let at_root = directory.canonicalize().map(|p| p == source_resolved).unwrap_or(false);
    if at_root {
        if is_clone_all_history_excluded(entry) {
            return true;
        }
        if is_default_source && is_clone_all_default_excluded(entry) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// _DEFAULT_EXPORT_EXCLUDE_ROOT — mirrors lines 207-229
// ---------------------------------------------------------------------------

/// Directories/files to exclude when exporting the default (~/.hermes) profile (lines 207-229).
pub const DEFAULT_EXPORT_EXCLUDE_ROOT: &[&str] = &[
    // Infrastructure
    "hermes-agent",
    ".worktrees",
    "profiles",
    "bin",
    "node_modules",
    // Databases & runtime state
    "state.db",
    "state.db-shm",
    "state.db-wal",
    "hermes_state.db",
    "response_store.db",
    "response_store.db-shm",
    "response_store.db-wal",
    "gateway.pid",
    "gateway_state.json",
    "processes.json",
    "auth.json",
    ".env",
    "auth.lock",
    "active_profile",
    ".update_check",
    "errors.log",
    ".hermes_history",
    // Caches (regenerated on use)
    "image_cache",
    "audio_cache",
    "document_cache",
    "browser_screenshots",
    "checkpoints",
    "sandboxes",
    "logs",
];

// ---------------------------------------------------------------------------
// _DEFAULT_EXPORT_INCLUDE_ROOT — mirrors lines 240-251
// ---------------------------------------------------------------------------

/// Allow-list for `export_profile("default")`: when HERMES_HOME equals the cwd
/// (Docker/custom deployments), only these known artifacts are bundled (lines 240-251).
pub const DEFAULT_EXPORT_INCLUDE_ROOT: &[&str] = &[
    // Configuration / persona
    "config.yaml",
    "SOUL.md",
    "MEMORY.md",
    "USER.md",
    "todo.json",
    "system_prompt.md",
    "AGENTS.md",
    "CLAUDE.md",
    ".cursorrules",
    // Desktop appearance/interface overlay
    "desktop.json",
    // User-facing skill, cron, and session artifacts
    "skills",
    "cron",
    "scripts",
    "sessions",
    // Plugin / memory surfaces (per-profile overrides live here)
    "plugins",
    "memories",
    "knowledge",
    "preferences",
];

// ---------------------------------------------------------------------------
// _RESERVED_NAMES — mirrors lines 254-256
// ---------------------------------------------------------------------------

/// Names that cannot be used as profile aliases (lines 254-256).
pub const RESERVED_NAMES: &[&str] = &["hermes", "default", "test", "tmp", "root", "sudo"];

pub fn is_reserved_name(name: &str) -> bool {
    RESERVED_NAMES.contains(&name)
}

// ---------------------------------------------------------------------------
// _HERMES_SUBCOMMANDS — mirrors lines 259-264
// ---------------------------------------------------------------------------

/// Hermes subcommands that cannot be used as profile names/aliases (lines 259-264).
pub const HERMES_SUBCOMMANDS: &[&str] = &[
    "chat",
    "model",
    "gateway",
    "setup",
    "whatsapp",
    "login",
    "logout",
    "status",
    "cron",
    "doctor",
    "dump",
    "config",
    "pairing",
    "skills",
    "tools",
    "mcp",
    "sessions",
    "insights",
    "version",
    "update",
    "uninstall",
    "profile",
    "plugins",
    "honcho",
    "acp",
];

pub fn is_hermes_subcommand(name: &str) -> bool {
    HERMES_SUBCOMMANDS.contains(&name)
}

// ---------------------------------------------------------------------------
// Path helpers — mirrors lines 271-303
// ---------------------------------------------------------------------------

/// Mirrors `_get_profiles_root() -> Path` (lines 271-282).
/// Anchored to the hermes root, NOT to the current HERMES_HOME.
pub fn get_profiles_root() -> PathBuf {
    get_default_hermes_home().join("profiles")
}

/// Mirrors `_get_default_hermes_home() -> Path` (lines 285-293).
/// In standard deployments this is `~/.hermes`.
/// In Docker/custom deployments where HERMES_HOME is outside `~/.hermes`, returns HERMES_HOME directly.
pub fn get_default_hermes_home() -> PathBuf {
    // Python: from hermes_constants import get_default_hermes_root; return get_default_hermes_root()
    // Rust stub: check HERMES_HOME env; if set and not under ~/.hermes, use it; else ~/.hermes
    // Mirrors hermes_constants.get_default_hermes_root heuristic: prefer HERMES_HOME when it is outside standard location.
    if let Ok(v) = std::env::var("HERMES_HOME") {
        let p = PathBuf::from(v.trim());
        if !p.as_os_str().is_empty() {
            // Heuristic: if HERMES_HOME is not ~/.hermes, treat as custom deployment root.
            let home = dirs_home();
            let standard = home.join(".hermes");
            if p != standard {
                // If HERMES_HOME points outside standard (e.g. /opt/data), return it directly per docstring.
                // Python's get_default_hermes_root does similar; we approximate.
                // For safety, check if HERMES_HOME does not end with ".hermes" or is custom.
                // Keep simple: if HERMES_HOME set, use it as default when it looks custom.
                // But original get_default_hermes_home always returns ~/.hermes unless HERMES_HOME outside ~/.hermes.
                // We replicate: if HERMES_HOME is set and its parent is not HOME/.hermes, return it.
                // Simpler: always return ~/.hermes when HERMES_HOME == ~/.hermes/profiles/* -> still return ~/.hermes
                // So detect if HERMES_HOME is under profiles: strip and return default.
                let s = p.to_string_lossy();
                if s.contains("/profiles/") {
                    return standard;
                }
                // If HERMES_HOME is exactly standard, return standard.
                if p == standard {
                    return standard;
                }
                // Otherwise custom deployment: return HERMES_HOME
                // However original logic: when HERMES_HOME == /opt/data, return /opt/data
                // We'll return p when p != standard.
                return p;
            }
            return standard;
        }
    }
    dirs_home().join(".hermes")
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// Mirrors `_get_active_profile_path() -> Path` (lines 296-298).
pub fn get_active_profile_path() -> PathBuf {
    get_default_hermes_home().join("active_profile")
}

/// Mirrors `_get_wrapper_dir() -> Path` (lines 301-303).
pub fn get_wrapper_dir() -> PathBuf {
    dirs_home().join(".local").join("bin")
}

// ---------------------------------------------------------------------------
// Validation — mirrors lines 310-438
// ---------------------------------------------------------------------------

/// Mirrors `normalize_profile_name(name: str) -> str` (lines 310-325).
/// Return the canonical profile id used on disk and in CLI `-p` argv.
pub fn normalize_profile_name(name: &str) -> Result<String, String> {
    let stripped = name.trim();
    if stripped.is_empty() {
        return Err("profile name cannot be empty".to_string());
    }
    if stripped.to_lowercase() == "default" {
        return Ok("default".to_string());
    }
    Ok(stripped.to_lowercase())
}

/// Fallback that coerces non-string-like input via ToString (mirrors Python's `if not isinstance(name, str): name = str(name)`).
pub fn normalize_profile_name_any<S: ToString>(name: S) -> Result<String, String> {
    normalize_profile_name(&name.to_string())
}

/// Mirrors `validate_profile_name(name: str) -> None` (lines 328-355).
/// Raise ValueError if name is not a valid profile identifier.
pub fn validate_profile_name(name: &str) -> Result<(), String> {
    if name == "default" {
        return Ok(());
    }
    if !is_valid_profile_id(name) {
        return Err(format!(
            "Invalid profile name {name:?}. Must match [a-z0-9][a-z0-9_-]{{0,63}}"
        ));
    }
    if is_reserved_name(name) {
        return Err(format!(
            "Profile name {name:?} is reserved — it collides with either the Hermes installation itself or a common system binary.  Pick a different name."
        ));
    }
    Ok(())
}

/// Mirrors `validate_alias_name(name: str) -> None` (lines 358-371).
pub fn validate_alias_name(name: &str) -> Result<(), String> {
    if !is_valid_profile_id(name) {
        return Err(format!(
            "Invalid alias name {name:?}. Must match [a-z0-9][a-z0-9_-]{{0,63}}"
        ));
    }
    Ok(())
}

/// Mirrors `get_profile_dir(name: str) -> Path` (lines 374-379).
pub fn get_profile_dir(name: &str) -> Result<PathBuf, String> {
    let canon = normalize_profile_name(name)?;
    if canon == "default" {
        return Ok(get_default_hermes_home());
    }
    Ok(get_profiles_root().join(canon))
}

/// Mirrors `profile_exists(name: str) -> bool` (lines 382-388).
pub fn profile_exists(name: &str) -> bool {
    let canon = match normalize_profile_name(name) {
        Ok(c) => c,
        Err(_) => return false,
    };
    if canon == "default" {
        return true;
    }
    match get_profile_dir(&canon) {
        Ok(p) => p.is_dir(),
        Err(_) => false,
    }
}

/// Mirrors `profile_matches_home(name: str, home: Path | None = None) -> bool` (lines 391-419).
pub fn profile_matches_home(name: &str, home: Option<&Path>) -> bool {
    let target = match get_profile_dir(name) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let home_path: PathBuf = match home {
        Some(h) => h.to_path_buf(),
        None => match get_hermes_home() {
            Ok(h) => h,
            Err(_) => return false,
        },
    };
    // Python: Path(target).expanduser().resolve(strict=False) == Path(home).expanduser().resolve(strict=False)
    // Rust: best-effort canonicalize with fallback to absolute.
    let target_resolved = target.canonicalize().unwrap_or_else(|_| {
        if target.is_absolute() {
            target.clone()
        } else {
            dirs_home().join(&target)
        }
    });
    let home_resolved = home_path.canonicalize().unwrap_or_else(|_| {
        if home_path.is_absolute() {
            home_path.clone()
        } else {
            dirs_home().join(&home_path)
        }
    });
    target_resolved == home_resolved
}

/// Mirrors `hermes_constants.get_hermes_home()` stub for profile_matches_home.
fn get_hermes_home() -> Result<PathBuf, String> {
    if let Ok(v) = std::env::var("HERMES_HOME") {
        if !v.trim().is_empty() {
            return Ok(PathBuf::from(v.trim()));
        }
    }
    Ok(dirs_home().join(".hermes"))
}

/// Mirrors `list_profile_names() -> List[str]` (lines 422-438).
/// Cheap name-only listing: `default` plus profile dirs.
pub fn list_profile_names() -> Vec<String> {
    let mut names = vec!["default".to_string()];
    let profiles_root = get_profiles_root();
    if let Ok(entries) = std::fs::read_dir(&profiles_root) {
        let mut extra: Vec<String> = Vec::new();
        for entry in entries.flatten() {
            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name != "default" && is_valid_profile_id(&name) {
                        extra.push(name);
                    }
                }
            }
        }
        extra.sort();
        names.extend(extra);
    }
    names
}

// ---------------------------------------------------------------------------
// Alias / wrapper script management — mirrors lines 445-673
// ---------------------------------------------------------------------------

/// Mirrors `check_alias_collision(name: str) -> Optional[str]` (lines 445-484).
pub fn check_alias_collision(name: &str) -> Option<String> {
    let canon = normalize_profile_name(name).unwrap_or_else(|_| name.to_lowercase());
    if let Err(e) = validate_alias_name(&canon) {
        return Some(e);
    }
    if is_reserved_name(&canon) {
        return Some(format!("'{canon}' is a reserved name"));
    }
    if is_hermes_subcommand(&canon) {
        return Some(format!("'{canon}' conflicts with a hermes subcommand"));
    }
    // Check existing commands in PATH — mirrors subprocess.run(["which"/"where", canon])
    let wrapper_dir = get_wrapper_dir();
    let is_windows = cfg!(windows);
    let which_cmd = if is_windows { "where" } else { "which" };
    if let Ok(output) = std::process::Command::new(which_cmd).arg(&canon).output() {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(existing_path) = stdout.lines().next().map(|l| l.trim().to_string()) {
                if !existing_path.is_empty() {
                    let expected = if is_windows {
                        wrapper_dir.join(format!("{canon}.bat"))
                    } else {
                        wrapper_dir.join(&canon)
                    };
                    if existing_path == expected.to_string_lossy().to_string() {
                        if let Ok(content) = std::fs::read_to_string(&expected) {
                            if content.contains("hermes -p") {
                                return None;
                            }
                        }
                    }
                    return Some(format!(
                        "'{canon}' conflicts with an existing command ({existing_path})"
                    ));
                }
            }
        }
    }
    None
}

/// Mirrors `_is_wrapper_dir_in_path() -> bool` (lines 487-490).
pub fn is_wrapper_dir_in_path() -> bool {
    let wrapper_dir = get_wrapper_dir().to_string_lossy().to_string();
    if let Ok(path_var) = std::env::var("PATH") {
        return path_var.split(':').any(|p| p == wrapper_dir)
            || path_var.split(';').any(|p| p == wrapper_dir);
    }
    false
}

/// Mirrors `create_wrapper_script(name: str, target: Optional[str] = None) -> Optional[Path]` (lines 493-533).
pub fn create_wrapper_script(name: &str, target: Option<&str>) -> Option<PathBuf> {
    let canon = normalize_profile_name(name).ok()?;
    let profile = if let Some(t) = target {
        normalize_profile_name(t).ok()?
    } else {
        canon.clone()
    };
    if validate_alias_name(&canon).is_err() {
        return None;
    }
    let wrapper_dir = get_wrapper_dir();
    if std::fs::create_dir_all(&wrapper_dir).is_err() {
        eprintln!("⚠ Could not create {}", wrapper_dir.display());
        return None;
    }
    let is_windows = cfg!(windows);
    if is_windows {
        let wrapper_path = wrapper_dir.join(format!("{canon}.bat"));
        let content = format!("@echo off\r\nhermes -p {profile} %*\r\n");
        if std::fs::write(&wrapper_path, content).is_err() {
            eprintln!("⚠ Could not create wrapper at {}", wrapper_path.display());
            return None;
        }
        Some(wrapper_path)
    } else {
        let wrapper_path = wrapper_dir.join(&canon);
        let hermes_exe = which_hermes().unwrap_or_else(|| "hermes".to_string());
        // Python: shlex.quote(hermes_exe)
        let quoted = shlex_quote(&hermes_exe);
        let content = format!("#!/bin/sh\nexec {quoted} -p {profile} \"$@\"\n");
        if std::fs::write(&wrapper_path, &content).is_err() {
            eprintln!("⚠ Could not create wrapper at {}", wrapper_path.display());
            return None;
        }
        // chmod +x — mirrors wrapper_path.chmod(... | S_IEXEC | S_IXGRP | S_IXOTH)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(&wrapper_path) {
                let mut perms = meta.permissions();
                let mode = perms.mode();
                perms.set_mode(mode | 0o111);
                let _ = std::fs::set_permissions(&wrapper_path, perms);
            }
        }
        Some(wrapper_path)
    }
}

fn which_hermes() -> Option<String> {
    // Mirrors shutil.which("hermes")
    if let Ok(output) = std::process::Command::new("which").arg("hermes").output() {
        if output.status.success() {
            let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    // Fallback: check PATH manually
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            let cand = Path::new(dir).join("hermes");
            if cand.is_file() {
                return Some(cand.to_string_lossy().to_string());
            }
        }
    }
    None
}

fn shlex_quote(s: &str) -> String {
    // Minimal shlex.quote — mirrors Python's shlex.quote for hermes_exe.
    if s.is_empty() {
        return "''".to_string();
    }
    let safe = s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '%' | '_' | '+' | '=' | ':' | ',' | '.' | '/' | '-'));
    if safe {
        return s.to_string();
    }
    let escaped = s.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

/// Mirrors `remove_wrapper_script(name: str) -> bool` (lines 536-563).
pub fn remove_wrapper_script(name: &str) -> bool {
    let wrapper_dir = get_wrapper_dir();
    let canon = match normalize_profile_name(name) {
        Ok(c) => c,
        Err(_) => return false,
    };
    if validate_alias_name(&canon).is_err() {
        return false;
    }
    let is_windows = cfg!(windows);
    let mut candidates = vec![wrapper_dir.join(&canon)];
    if is_windows {
        candidates.insert(0, wrapper_dir.join(format!("{canon}.bat")));
    }
    for wrapper_path in candidates {
        if wrapper_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&wrapper_path) {
                if content.contains("hermes -p") {
                    if std::fs::remove_file(&wrapper_path).is_ok() {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Mirrors `_migrate_profile_config_if_outdated(profile_dir: Path) -> None` (lines 566-594).
pub fn migrate_profile_config_if_outdated(profile_dir: &Path) {
    let config_path = profile_dir.join("config.yaml");
    if !config_path.exists() {
        return;
    }
    // Python:
    //   from hermes_constants import reset_hermes_home_override, set_hermes_home_override
    //   from hermes_cli.config import check_config_version, migrate_config
    //   token = set_hermes_home_override(str(profile_dir))
    //   try: current_ver, latest_ver = check_config_version(); if current_ver < latest_ver: migrate_config(interactive=False, quiet=True)
    //   finally: reset_hermes_home_override(token)
    // Rust stub: best-effort; profile creation should not fail because migration failed.
    let _ = config_path;
    // No-op in slice 1 — full migration wiring in later slice with config module ported.
}

/// Mirrors `find_alias_for_profile(profile_name: str) -> Optional[str]` (lines 597-615).
pub fn find_alias_for_profile(profile_name: &str) -> Option<String> {
    build_alias_map().get(&normalize_profile_name(profile_name).ok()?).cloned()
}

// Cap how much of a wrapper file we read when reverse-looking-up its profile (lines 618-624).
pub const WRAPPER_READ_LIMIT: usize = 8192;

/// Mirrors `build_alias_map() -> dict[str, str]` (lines 627-673).
/// Single-pass reverse map `{canonical_profile -> alias_name}`.
pub fn build_alias_map() -> HashMap<String, String> {
    let wrapper_dir = get_wrapper_dir();
    let mut result: HashMap<String, String> = HashMap::new();
    if !wrapper_dir.is_dir() {
        return result;
    }
    let is_windows = cfg!(windows);
    let prefix = "hermes -p ";
    let entries = match std::fs::read_dir(&wrapper_dir) {
        Ok(it) => it,
        Err(_) => return result,
    };
    let mut sorted: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        if let Ok(ft) = entry.file_type() {
            if ft.is_file() {
                sorted.push(entry.path());
            }
        }
    }
    sorted.sort();
    for entry_path in sorted {
        let file_name = entry_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if is_windows && entry_path.extension().and_then(|e| e.to_str()) != Some("bat") {
            continue;
        }
        if !is_windows && entry_path.extension().is_some() {
            continue;
        }
        let content = match read_wrapper_head(&entry_path, WRAPPER_READ_LIMIT) {
            Some(c) => c,
            None => continue,
        };
        let idx = match content.find(prefix) {
            Some(i) => i,
            None => continue,
        };
        let rest = &content[idx + prefix.len()..];
        let canon_raw = rest.split_whitespace().next().unwrap_or("").trim().to_string();
        if canon_raw.is_empty() {
            continue;
        }
        let canon = match normalize_profile_name(&canon_raw) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let alias = if is_windows {
            entry_path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or(file_name)
        } else {
            file_name
        };
        if alias == canon {
            result.entry(canon).or_insert(alias);
        } else {
            result.insert(canon, alias);
        }
    }
    result
}

fn read_wrapper_head(path: &Path, limit: usize) -> Option<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; limit];
    let n = file.read(&mut buf).ok()?;
    buf.truncate(n);
    // UnicodeDecodeError = binary on PATH (ffmpeg etc.) — not a wrapper.
    String::from_utf8(buf).ok()
}

// ---------------------------------------------------------------------------
// ProfileInfo — mirrors lines 681-714
// ---------------------------------------------------------------------------

/// Summary information about a profile (lines 681-714).
#[derive(Debug, Clone)]
pub struct ProfileInfo {
    pub name: String,
    pub path: PathBuf,
    pub is_default: bool,
    pub gateway_running: bool,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub has_env: bool,
    pub skill_count: usize,
    pub alias_path: Option<PathBuf>,
    /// Custom alias name (the wrapper file name) when it differs from `name`.
    pub alias_name: Option<String>,
    pub distribution_name: Option<String>,
    pub distribution_version: Option<String>,
    pub distribution_source: Option<String>,
    /// Free-form description (1-2 sentences) of what this profile is good at.
    pub description: String,
    /// When True, `description` was auto-generated by the LLM describer.
    pub description_auto: bool,
    /// Optional user-facing display name from profile.yaml.
    pub display_name: String,
}

// ---------------------------------------------------------------------------
// _read_distribution_meta — mirrors lines 716-738
// ---------------------------------------------------------------------------

/// Mirrors `_read_distribution_meta(profile_dir: Path) -> tuple` (lines 716-738).
pub fn read_distribution_meta(profile_dir: &Path) -> (Option<String>, Option<String>, Option<String>) {
    let mf_path = profile_dir.join("distribution.yaml");
    if !mf_path.is_file() {
        return (None, None, None);
    }
    // Python: yaml.safe_load + data.get("name"/"version"/"source")
    // Rust stub without yaml crate: minimal key extraction.
    let text = match std::fs::read_to_string(&mf_path) {
        Ok(t) => t,
        Err(_) => return (None, None, None),
    };
    // Very small yaml-ish parse: look for `name:`, `version:`, `source:` at any indent.
    let mut name: Option<String> = None;
    let mut version: Option<String> = None;
    let mut source: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("name:") {
            name = Some(trimmed["name:".len()..].trim().trim_matches(|c| c == '"' || c == '\'').to_string());
        } else if trimmed.starts_with("version:") {
            version = Some(trimmed["version:".len()..].trim().trim_matches(|c| c == '"' || c == '\'').to_string());
        } else if trimmed.starts_with("source:") {
            source = Some(trimmed["source:".len()..].trim().trim_matches(|c| c == '"' || c == '\'').to_string());
        }
    }
    // If file was present but not a dict-like, Python returns (None,None,None) — our heuristic mimics that
    // by returning None for missing keys; empty strings become None to preserve Python's .get behavior.
    let name = name.filter(|v| !v.is_empty());
    let version = version.filter(|v| !v.is_empty());
    let source = source.filter(|v| !v.is_empty());
    (name, version, source)
}

// ---------------------------------------------------------------------------
// _read_config_model — mirrors lines 741-758
// ---------------------------------------------------------------------------

/// Mirrors `_read_config_model(profile_dir: Path) -> tuple` (lines 741-758).
/// Read model/provider from a profile's config.yaml. Returns (model, provider).
pub fn read_config_model(profile_dir: &Path) -> (Option<String>, Option<String>) {
    let config_path = profile_dir.join("config.yaml");
    if !config_path.exists() {
        return (None, None);
    }
    // Python: from hermes_cli.config import read_user_config_raw; cfg = read_user_config_raw(config_path); model_cfg = cfg.get("model", {})
    // Rust stub: minimal yaml parse for model/provider.
    let text = match std::fs::read_to_string(&config_path) {
        Ok(t) => t,
        Err(_) => return (None, None),
    };
    // Handle `model: str` vs `model: { default, provider, model }`
    // We look for `model:` at top-level and try to parse its value.
    let mut model: Option<String> = None;
    let mut provider: Option<String> = None;
    let mut in_model_block = false;
    let mut model_indent: Option<usize> = None;
    for line in text.lines() {
        if line.trim().is_empty() || line.trim().starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        let trimmed = line.trim();
        if trimmed.starts_with("model:") {
            let rest = trimmed["model:".len()..].trim();
            if !rest.is_empty() && rest != "{}" {
                // model: "string"
                let val = rest.trim_matches(|c| c == '"' || c == '\'').to_string();
                if !val.is_empty() && val != "{}" {
                    // Could be inline dict — ignore; real dict case handled below.
                    if !val.starts_with('{') {
                        model = Some(val);
                        in_model_block = false;
                        continue;
                    }
                }
            }
            in_model_block = true;
            model_indent = Some(indent);
            continue;
        }
        if in_model_block {
            if indent <= model_indent.unwrap_or(0) && !trimmed.is_empty() {
                in_model_block = false;
                continue;
            }
            if trimmed.starts_with("default:") || trimmed.starts_with("model:") {
                // `model:` inside model block is the actual model name
                let key = if trimmed.starts_with("default:") { "default:" } else { "model:" };
                let val = trimmed[key.len()..].trim().trim_matches(|c| c == '"' || c == '\'').to_string();
                if !val.is_empty() {
                    model = Some(val);
                }
            } else if trimmed.starts_with("provider:") {
                let val = trimmed["provider:".len()..].trim().trim_matches(|c| c == '"' || c == '\'').to_string();
                if !val.is_empty() {
                    provider = Some(val);
                }
            }
        }
    }
    (model, provider)
}

// ---------------------------------------------------------------------------
// _check_gateway_running — mirrors lines 761-791
// ---------------------------------------------------------------------------

/// Mirrors `_check_gateway_running(profile_dir: Path) -> bool` (lines 761-791).
/// Checks `gateway.pid` via `gateway.status.get_running_pid` then falls back to
/// validating `gateway_state.json` against the live process table.
pub fn check_gateway_running(profile_dir: &Path) -> bool {
    // Primary signal: profile's `gateway.pid` verified against runtime lock.
    if let Some(pid) = get_running_pid_stub(&profile_dir.join("gateway.pid"), false) {
        let _ = pid;
        return true;
    }
    // Fallback: validate PID in `gateway_state.json` against live process table.
    if let Some(runtime) = read_runtime_status_stub(&profile_dir.join("gateway_state.json")) {
        if get_runtime_status_running_pid_stub(&runtime, profile_dir).is_some() {
            return true;
        }
    }
    false
}

// Stubs for gateway.status — mirrors imports inside _check_gateway_running.

fn get_running_pid_stub(pid_path: &Path, _cleanup_stale: bool) -> Option<u32> {
    // Mirrors gateway.status.get_running_pid(profile_dir / "gateway.pid", cleanup_stale=False)
    // Real impl checks pid file + lock; stub reads pid file and checks if process exists via kill -0 heuristic.
    let text = std::fs::read_to_string(pid_path).ok()?;
    let pid: u32 = text.trim().parse().ok()?;
    // Check liveness: try `kill -0` or /proc/<pid> exists.
    #[cfg(unix)]
    {
        if Path::new(&format!("/proc/{pid}")).exists() {
            return Some(pid);
        }
        // Fallback: try kill -0
        let out = std::process::Command::new("kill").args(["-0", &pid.to_string()]).output().ok()?;
        if out.status.success() {
            return Some(pid);
        }
        return None;
    }
    #[cfg(not(unix))]
    {
        // On non-unix, assume pid file presence means running (best-effort stub).
        Some(pid)
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeStatusStub {
    pub pid: Option<u32>,
    pub home: Option<PathBuf>,
}

fn read_runtime_status_stub(path: &Path) -> Option<RuntimeStatusStub> {
    // Mirrors gateway.status.read_runtime_status(profile_dir / "gateway_state.json")
    let text = std::fs::read_to_string(path).ok()?;
    // Minimal JSON extraction without serde (NEVER cargo): look for "pid"
    let mut pid: Option<u32> = None;
    let mut home: Option<PathBuf> = None;
    for line in text.lines() {
        let t = line.trim();
        if t.contains("\"pid\"") {
            if let Some(colon) = t.find(':') {
                let after = t[colon + 1..].trim().trim_matches(|c| c == ',' || c == '"' || c == '\'' || c == ' ');
                let num_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(n) = num_str.parse::<u32>() {
                    pid = Some(n);
                }
            }
        }
        if t.contains("\"hermes_home\"") || t.contains("\"home\"") {
            if let Some(colon) = t.find(':') {
                let after = t[colon + 1..].trim();
                let val = after.trim_matches(|c| c == ',' || c == '"' || c == '\'' || c == ' ').to_string();
                if !val.is_empty() && val != "null" {
                    home = Some(PathBuf::from(val));
                }
            }
        }
    }
    Some(RuntimeStatusStub { pid, home })
}

fn get_runtime_status_running_pid_stub(runtime: &RuntimeStatusStub, expected_home: &Path) -> Option<u32> {
    // Mirrors gateway.status.get_runtime_status_running_pid(runtime, expected_home=profile_dir)
    let pid = runtime.pid?;
    if let Some(home) = &runtime.home {
        if home != expected_home {
            // Home mismatch — not our profile's gateway
            // Real impl checks expected_home equality; stub respects it.
            // Allow when home is None or matches.
            // For stub, require exact match.
            if home.canonicalize().ok() != expected_home.canonicalize().ok() {
                // If homes differ, still check pid liveness but don't claim running for this profile?
                // Python returns None if expected_home mismatch. We mimic.
                return None;
            }
        }
    }
    // Verify pid liveness
    #[cfg(unix)]
    {
        if Path::new(&format!("/proc/{pid}")).exists() {
            return Some(pid);
        }
        let out = std::process::Command::new("kill").args(["-0", &pid.to_string()]).output().ok()?;
        if out.status.success() {
            return Some(pid);
        }
        return None;
    }
    #[cfg(not(unix))]
    {
        Some(pid)
    }
}

// ---------------------------------------------------------------------------
// Skill count cache — mirrors lines 802-856
// ---------------------------------------------------------------------------

/// In-process cache for skill counts (lines 802-803).
/// Keyed by skills dir string -> (signature, cached_at_secs, count).
static SKILL_COUNT_CACHE: OnceLock<Mutex<HashMap<String, (f64, f64, usize)>>> = OnceLock::new();

fn skill_count_cache() -> &'static Mutex<HashMap<String, (f64, f64, usize)>> {
    SKILL_COUNT_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub const SKILL_COUNT_TTL_SECONDS: f64 = 30.0;

/// Mirrors `_skills_dir_signature(skills_dir: Path) -> float` (lines 806-830).
pub fn skills_dir_signature(skills_dir: &Path) -> f64 {
    let sig = match std::fs::metadata(skills_dir) {
        Ok(m) => m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0),
        Err(_) => return 0.0,
    };
    let mut max_sig = sig;
    if let Ok(entries) = std::fs::read_dir(skills_dir) {
        for entry in entries.flatten() {
            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() {
                    if let Ok(meta) = entry.metadata() {
                        if let Ok(mtime) = meta.modified() {
                            if let Ok(d) = mtime.duration_since(UNIX_EPOCH) {
                                let m = d.as_secs_f64();
                                if m > max_sig {
                                    max_sig = m;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    max_sig
}

/// Mirrors `_count_skills(profile_dir: Path) -> int` (lines 833-856).
pub fn count_skills(profile_dir: &Path) -> usize {
    let skills_dir = profile_dir.join("skills");
    if !skills_dir.is_dir() {
        return 0;
    }
    let key = skills_dir.to_string_lossy().to_string();
    let signature = skills_dir_signature(&skills_dir);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    {
        let cache = skill_count_cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some((cached_sig, cached_at, cached_count)) = cache.get(&key) {
            if (*cached_sig - signature).abs() < f64::EPSILON && (now - *cached_at) < SKILL_COUNT_TTL_SECONDS {
                return *cached_count;
            }
        }
    }
    let mut count = 0usize;
    // Walk skills_dir.rglob("SKILL.md") — std only recursive walk.
    let mut stack = vec![skills_dir.clone()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(it) => it,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Skip symlinked dirs? Python rglob follows? Use symlink check.
                if let Ok(ft) = entry.file_type() {
                    if ft.is_symlink() {
                        // Python shutil.copytree symlinks=True but rglob follows? We follow for counting.
                        // Keep simple: push dir.
                    }
                }
                stack.push(path);
            } else if path.file_name().map(|n| n == "SKILL.md").unwrap_or(false) {
                if is_excluded_skill_path_stub(&path) {
                    continue;
                }
                count += 1;
            }
        }
    }
    if let Ok(mut cache) = skill_count_cache().lock() {
        cache.insert(key, (signature, now, count));
    }
    count
}

// ---------------------------------------------------------------------------
// profile.yaml — per-profile metadata — mirrors lines 862-901
// ---------------------------------------------------------------------------

/// Mirrors `_profile_yaml_path(profile_dir: Path) -> Path` (lines 873-874).
pub fn profile_yaml_path(profile_dir: &Path) -> PathBuf {
    profile_dir.join("profile.yaml")
}

/// Mirrors `read_profile_meta(profile_dir: Path) -> dict` (lines 877-901).
/// Returns `{"description": "", "description_auto": false, "display_name": ""}` when missing/unreadable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileMeta {
    pub description: String,
    pub description_auto: bool,
    pub display_name: String,
}

impl Default for ProfileMeta {
    fn default() -> Self {
        Self {
            description: String::new(),
            description_auto: false,
            display_name: String::new(),
        }
    }
}

pub fn read_profile_meta(profile_dir: &Path) -> ProfileMeta {
    let path = profile_yaml_path(profile_dir);
    if !path.is_file() {
        return ProfileMeta::default();
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return ProfileMeta::default(),
    };
    // Minimal yaml-ish parse without yaml crate (NEVER cargo).
    // Python: yaml.safe_load(f) or {} -> dict check -> return {description: str(...).strip(), ...}
    // We parse top-level keys `description`, `description_auto`, `display_name`.
    let mut description = String::new();
    let mut description_auto = false;
    let mut display_name = String::new();
    let mut is_dict_like = false;
    let mut has_content = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        has_content = true;
        // Detect non-dict yaml (e.g. list): starts with `- ` or not containing `:`
        if trimmed.starts_with("- ") || trimmed.starts_with("-	") {
            return ProfileMeta::default();
        }
        if !trimmed.contains(':') {
            continue;
        }
        is_dict_like = true;
        if trimmed.starts_with("description:") {
            let val = trimmed["description:".len()..].trim().trim_matches(|c| c == '"' || c == '\'').to_string();
            description = val.trim().to_string();
        } else if trimmed.starts_with("description_auto:") {
            let val = trimmed["description_auto:".len()..].trim().to_lowercase();
            description_auto = matches!(val.as_str(), "true" | "yes" | "1" | "on");
        } else if trimmed.starts_with("display_name:") {
            let val = trimmed["display_name:".len()..].trim().trim_matches(|c| c == '"' || c == '\'').to_string();
            display_name = val.trim().to_string();
        }
    }
    if has_content && !is_dict_like {
        return ProfileMeta::default();
    }
    ProfileMeta {
        description,
        description_auto,
        display_name,
    }
}

// ---------------------------------------------------------------------------
// Slice boundary — line 900
// ---------------------------------------------------------------------------
// Python `profiles.py` lines 904-2543 ( `write_profile_meta`,
// `format_profile_label`, `set_profile_display_name`, `list_profiles`,
// `profiles_to_serve`, `create_profile`, ... through export/import/remove
// helpers) continue in `profiles_slice2.rs` (from line 904).
// This file intentionally stops at the 900-line boundary (mid-`write_profile_meta`
// header at `return ProfileMeta` of `read_profile_meta`) so that `cargo` is never
// invoked and the 3-slice decomposition stays clean.
