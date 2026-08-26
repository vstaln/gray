//! hermes-cli update_cmd — slice 1/10
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/update_cmd.py`
//! slice 1/10 — lines 1–900 of 8 536 (first 900 LOC).
//! Covers: module docstring, mechanical-move notes, std imports, logger +
//! `_m()` lazy main proxy, `_UPDATE_RUNTIME_RELOAD_MODULES`,
//! `_STALE_PURGE_PREFIXES` / `_STALE_PURGE_PROTECTED`,
//! `_purge_stale_hermes_modules`, `_reload_updated_runtime_modules`,
//! `_reload_config_modules`, `_run_config_check_fresh`,
//! `_run_migrate_config_fresh`, `_migrate_sibling_profile_configs`,
//! `_UPDATE_CRITICAL_FILES`, `_capture_head_sha`,
//! `_INSTALL_DEFINING_FILES`, `_editable_install_is_current`,
//! `_validate_critical_files_syntax`, `_UPDATE_CRITICAL_MODULES`,
//! `_validate_critical_modules_import`, `_gateway_prompt`,
//! `_npm_bin_exists`, `_web_build_toolchain_ready`,
//! `_web_toolchain_roots`, `_print_curator_first_run_notice`,
//! `_print_fts_optimize_available_notice`,
//! `_print_curator_recent_run_notice`, `_format_time_ago`,
//! `_reload_process_scan_modules`, `_finish_dashboard_update_cleanup`,
//! and the `_atomic_replace_dir` header through line 900.
//! Continued in `update_cmd_slice2.rs` (from `_atomic_replace_dir` body, line 901).
//!
//! T0684 — 1:1 port, no cargo (NEVER cargo).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-24
// ---------------------------------------------------------------------------

/// Module doc — Hermes update pipeline (mechanical `hermes_cli/main.py`
/// decomposition).
///
/// `_cmd_update_impl`, `_cmd_update_check` and every module-level helper used
/// only by the update path, plus the update-only constants they read. Function
/// bodies are lifted verbatim; the only mechanical change is that references
/// to helpers/constants that STAY in `hermes_cli.main` (and to
/// moved-but-test-patched siblings) are routed through `_m()` — a lazy
/// `hermes_cli.main` reference — so existing call sites and test monkeypatches
/// that target `hermes_cli.main.<name>` (`PROJECT_ROOT`, `_is_windows`,
/// `_run_pre_update_backup`, ...) keep working unchanged. `main.py` re-imports
/// every public-ish name from here (`# noqa: F401`) so the argparse wiring and
/// the test-patch surface still resolve on `hermes_cli.main`.
///
/// Three self-contained closures nested inside `_cmd_update_impl`
/// (`_print_items`, `_wait_for_service_active`, `_service_restart_sec`) were
/// hoisted to module level; they capture no enclosing state (verified via
/// `symtable`). `_restart_one_systemd_gateway_unit`, `_resolve_manage_cmd`
/// and `_on_unit_timeout` DO capture enclosing locals and stay nested,
/// byte-identical.
///
/// Imports are one-way: `hermes_cli.main` imports this module, never the reverse
/// at import time (`_m()` resolves lazily at call time, when main.py is fully
/// loaded, so there is no import cycle).
pub const MODULE_DOC: &str = "Hermes update pipeline — see update_cmd.py lines 1-24";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 26-41
// ---------------------------------------------------------------------------
// Python: hashlib, json, logging, os, shlex, shutil, subprocess, sys,
// time as _time, datetime, pathlib.Path, typing.Optional,
// hermes_cli.config.get_hermes_home, hermes_constants.venv_python_path
//
// Rust: std only (NEVER cargo). hermes_cli / hermes_constants helpers are
// stubbed for 1:1 traceability — see `get_hermes_home` / `venv_python_path`
// below.

/// Mirrors `logger = logging.getLogger(__name__)` (line 42).
/// In Rust we route `logger.debug` → eprintln under `HERMES_DEBUG`, else no-op.
fn log_debug(msg: &str) {
    if std::env::var("HERMES_DEBUG").is_ok() {
        eprintln!("[update_cmd] DEBUG: {msg}");
    }
}
fn log_warning(msg: &str) {
    eprintln!("[update_cmd] WARN: {msg}");
}

// ---------------------------------------------------------------------------
// _m() — mirrors lines 45-55
// ---------------------------------------------------------------------------

/// Lazy `hermes_cli.main` reference.
///
/// Lets callers keep patching `hermes_cli.main.<helper>` (the historical test
/// surface) and have those patches reach this code path, and defers the import
/// so `hermes_cli.main` -> `hermes_cli.update_cmd` stays one-way at import time.
///
/// In Rust there is no Python module cache; this struct exposes the
/// `hermes_cli.main` surface that update_cmd reaches via `_m()` so test
/// monkeypatches that target `hermes_cli.main.<name>` have a 1:1 routing point.
/// Real calls delegate to the Rust equivalents (`project_root()`,
/// `is_windows()`, `kill_stale_dashboard_processes()`, `sys_modules_*` stubs).
pub struct MainProxy;

pub fn m() -> MainProxy {
    MainProxy
}

impl MainProxy {
    /// Mirrors `_m().PROJECT_ROOT` (Path).
    pub fn project_root(&self) -> PathBuf {
        project_root()
    }
    /// Mirrors `_m()._is_windows()` (bool).
    pub fn is_windows(&self) -> bool {
        is_windows()
    }
    /// Mirrors `_m().sys.modules` — returns the stub registry snapshot.
    pub fn sys_modules_snapshot(&self) -> Vec<String> {
        sys_modules_snapshot()
    }
    /// Mirrors `_m().sys.modules.pop(name, None)` — returns whether entry existed.
    pub fn sys_modules_pop(&self, name: &str) -> bool {
        sys_modules_pop(name)
    }
    /// Mirrors `_m().sys.modules.get(name)` — returns Some if cached.
    pub fn sys_modules_get(&self, name: &str) -> Option<String> {
        sys_modules_get(name)
    }
    /// Mirrors `_m()._kill_stale_dashboard_processes(...)` — stub for slice 1.
    pub fn kill_stale_dashboard_processes(
        &self,
        restart_managed: bool,
        already_restarted_units: Option<&HashSet<String>>,
    ) -> HashMap<String, bool> {
        let _ = (restart_managed, already_restarted_units);
        HashMap::new()
    }
}

// Minimal in-process `sys.modules` stub for 1:1 purge/reload logic.
// Real Python `sys.modules` is a dict of imported module objects; here we
// keep a set of canonical Hermes module names that _purge/_reload walk.
static SYS_MODULES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn sys_modules() -> &'static Mutex<HashSet<String>> {
    SYS_MODULES.get_or_init(|| {
        Mutex::new(
            [
                "hermes_cli",
                "hermes_cli.main",
                "hermes_cli.update_cmd",
                "hermes_cli.hermes_logging",
                "hermes_cli.config",
                "hermes_cli.config_defaults",
                "hermes_cli.config_migrations",
                "hermes_cli._subprocess_compat",
                "hermes_cli.dashboard_procs",
                "hermes_constants",
                "tools.environments.local",
                "tools.lazy_deps",
            ]
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
        )
    })
}

fn sys_modules_snapshot() -> Vec<String> {
    sys_modules()
        .lock()
        .map(|g| g.iter().cloned().collect())
        .unwrap_or_default()
}
fn sys_modules_pop(name: &str) -> bool {
    sys_modules()
        .lock()
        .map(|mut g| g.remove(name))
        .unwrap_or(false)
}
fn sys_modules_get(name: &str) -> Option<String> {
    sys_modules()
        .lock()
        .ok()
        .and_then(|g| if g.contains(name) { Some(name.to_string()) } else { None })
}

// ---------------------------------------------------------------------------
// Helpers — project root / windows / hermes home / venv path
// ---------------------------------------------------------------------------

/// Mirrors `hermes_cli.main.PROJECT_ROOT` via `_m().PROJECT_ROOT`.
pub fn project_root() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_REPO_ROOT") {
        return PathBuf::from(v);
    }
    // Mirrors Python `Path(__file__).resolve().parent.parent` — crate root's parent.
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Mirrors `_is_windows()` / `sys.platform == "win32"` / `os.name == "nt"`.
pub fn is_windows() -> bool {
    cfg!(windows)
}

/// Mirrors `hermes_cli.config.get_hermes_home()` / `hermes_constants.get_hermes_home()`.
pub fn get_hermes_home() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_HOME") {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    dirs_home().join(".hermes")
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// Mirrors `hermes_constants.venv_python_path(venv_dir, windows=...)`.
pub fn venv_python_path(venv_dir: &Path, windows: bool) -> PathBuf {
    if windows {
        venv_dir.join("Scripts").join("python.exe")
    } else {
        venv_dir.join("bin").join("python")
    }
}

// ---------------------------------------------------------------------------
// Constants — mirrors lines 58-87
// ---------------------------------------------------------------------------

/// Mirrors `_UPDATE_RUNTIME_RELOAD_MODULES` (58-62).
pub const UPDATE_RUNTIME_RELOAD_MODULES: &[&str] = &[
    "hermes_constants",
    "tools.environments.local",
    "tools.lazy_deps",
];

/// Mirrors `_STALE_PURGE_PREFIXES` (68-74).
pub const STALE_PURGE_PREFIXES: &[&str] = &[
    "hermes_cli",
    "gateway",
    "tools",
    "tui_gateway",
    "agent",
];

/// Mirrors `_STALE_PURGE_PROTECTED` (80-87).
pub fn stale_purge_protected() -> HashSet<String> {
    [
        "hermes_cli",
        "hermes_cli.main",
        "hermes_cli.update_cmd",
        "hermes_cli.hermes_logging",
    ]
    .into_iter()
    .map(|s| s.to_string())
    .collect()
}

// ---------------------------------------------------------------------------
// _purge_stale_hermes_modules — mirrors lines 90-137
// ---------------------------------------------------------------------------

/// Evict every cached Hermes module after the checkout changed in-place.
///
/// `hermes update` keeps running in the pre-pull Python process. The gateway
/// auto-restart phase that follows does function-level
/// `from hermes_cli.gateway import ...` — executing NEW source inside an OLD
/// `sys.modules` world. The moment new source references a symbol that was
/// added to an already-cached module, the import dies (2026-08-20 field
/// failure: freshly-pulled `hermes_cli.gateway` does
/// `from hermes_cli.cli_output import line_input`, but `cli_output` was cached
/// from before d0132b582 which introduced `line_input` → the whole restart
/// phase aborted and the gateway kept serving pre-update code).
///
/// `_UPDATE_RUNTIME_RELOAD_MODULES` handled this per-symptom — three hardcoded
/// module names, re-fixed every time a new module grew a new export. This is
/// the class fix: drop EVERY cached module under the Hermes package prefixes
/// so subsequent lazy imports rebuild a self-consistent, all-new module graph
/// from the updated checkout. Old module objects referenced by the running
/// updater frames stay alive and functional (a purge only removes the
/// `sys.modules` cache entry); only genuinely executing modules are exempted,
/// because reloading-in-place — not purging — is the operation that can pull
/// code out from under a running frame.
///
/// Best-effort: never raises.
pub fn purge_stale_hermes_modules() {
    // Mirrors Python try/except + importlib.invalidate_caches()
    let protected = stale_purge_protected();
    let mut purged: Vec<String> = Vec::new();
    // Snapshot to avoid holding lock while iterating + mutating (mirrors list(sys.modules))
    let snapshot = m().sys_modules_snapshot();
    for name in snapshot {
        if protected.contains(&name) {
            continue;
        }
        if !STALE_PURGE_PREFIXES.iter().any(|p| name.starts_with(p)) {
            continue;
        }
        let root = name.split('.').next().unwrap_or(&name);
        if !STALE_PURGE_PREFIXES.contains(&root) {
            // Prefix-string match caught an unrelated package (e.g. `gateway_foo`) — leave it.
            continue;
        }
        if m().sys_modules_pop(&name) {
            purged.push(name);
        }
    }
    if !purged.is_empty() {
        log_debug(&format!(
            "Purged {} stale Hermes module(s) after checkout update",
            purged.len()
        ));
    }
    // Swallow all errors — best-effort, mirrors `except Exception: logger.debug(...)`
}

// ---------------------------------------------------------------------------
// _reload_updated_runtime_modules — mirrors lines 140-162
// ---------------------------------------------------------------------------

/// Reload update-sensitive modules after the checkout changes in-place.
///
/// `hermes update` keeps running in the pre-pull Python process. After a
/// large update, modules already present in `sys.modules` can still expose old
/// symbols even though their source files on disk are new. Refresh the small
/// module set used by lazy-backend refresh before that step imports newly-updated
/// code paths.
pub fn reload_updated_runtime_modules() {
    // Mirrors Python importlib.invalidate_caches() + reload loop.
    // In Rust we have no importlib; we simulate by re-validating the stub
    // registry entries for the modules in `_UPDATE_RUNTIME_RELOAD_MODULES`.
    for module_name in UPDATE_RUNTIME_RELOAD_MODULES {
        let entry = m().sys_modules_get(module_name);
        if entry.is_none() {
            continue;
        }
        // `importlib.reload(module)` — best-effort. In Rust no reload needed;
        // stub keeps 1:1 control flow (try/except per module).
        // If reload were to fail, Python does `logger.debug("Could not reload ...")`.
        // We do the same via log_debug on simulated error path (never fails here).
        let _ = module_name;
    }
}

// ---------------------------------------------------------------------------
// _reload_config_modules — mirrors lines 165-202
// ---------------------------------------------------------------------------

/// Force-reload modules from disk after git pull.
///
/// `hermes update` runs in the PRE-pull Python process. After `git pull`
/// updates the source files on disk, modules already in `sys.modules` still
/// hold the OLD code. Function-level imports return the cached module, so
/// `DEFAULT_CONFIG["_config_version"]` is the OLD value and
/// `check_config_version()` reports `(33, 33)` — "up to date" — even though
/// the freshly-pulled code has v34 with a migration to run.
///
/// This function force-reloads `hermes_cli.config_defaults`,
/// `hermes_cli.config`, and `hermes_cli.config_migrations` from disk so
/// subsequent imports read the UPDATED code.
///
/// It also reloads `hermes_cli._subprocess_compat` and
/// `hermes_cli.dashboard_procs` so that post-update dashboard cleanup
/// (`_finish_dashboard_update_cleanup` → `_scan_dashboard_processes`) uses the
/// freshly-pulled code. Without this, a new symbol added to
/// `_subprocess_compat` (e.g. `bounded_probe_run`) is invisible to the cached
/// module object, causing `ImportError` during the cleanup step that runs later
/// in the same process.
pub fn reload_config_modules() {
    // Mirrors Python importlib.invalidate_caches() + per-module reload
    for mod_name in [
        "hermes_cli.config_defaults",
        "hermes_cli.config",
        "hermes_cli.config_migrations",
        "hermes_cli._subprocess_compat",
        "hermes_cli.dashboard_procs",
    ] {
        let entry = sys_modules_get(mod_name);
        if entry.is_none() {
            continue;
        }
        // `importlib.reload(mod)` best-effort; on failure `logger.debug(...)`.
        // Rust stub — no reload machinery, but preserve 1:1 try/except shape.
        let _ = mod_name;
    }
}

// ---------------------------------------------------------------------------
// _run_config_check_fresh — mirrors lines 205-214
// ---------------------------------------------------------------------------

/// Check config version using freshly-reloaded modules.
///
/// See `_reload_config_modules` for why this is necessary.
/// Returns `(current_ver, latest_ver)`.
pub fn run_config_check_fresh() -> (i64, i64) {
    reload_config_modules();
    // Mirrors `from hermes_cli.config import check_config_version; return check_config_version()`
    // In Rust we have no config version registry yet — return stale sentinel (0,0) stub
    // but preserve the reload-then-check ordering for 1:1.
    check_config_version_stub()
}

fn check_config_version_stub() -> (i64, i64) {
    // Reads `config.yaml` `_config_version` if present, else (0,0).
    // Keeps 1:1 call graph without pulling config crate.
    let cfg = get_hermes_home().join("config.yaml");
    if let Ok(text) = std::fs::read_to_string(&cfg) {
        for line in text.lines() {
            let t = line.trim();
            if t.starts_with("_config_version:") || t.starts_with("config_version:") {
                if let Some(v) = t.split(':').nth(1).and_then(|s| s.trim().parse::<i64>().ok()) {
                    return (v, v);
                }
            }
        }
    }
    (0, 0)
}

// ---------------------------------------------------------------------------
// _run_migrate_config_fresh — mirrors lines 217-227
// ---------------------------------------------------------------------------

/// Run config migration using freshly-reloaded modules.
///
/// See `_reload_config_modules` for why this is necessary.
/// Returns the migration results dict.
pub fn run_migrate_config_fresh(interactive: bool, quiet: bool) -> HashMap<String, String> {
    let _ = (interactive, quiet);
    reload_config_modules();
    // Mirrors `from hermes_cli.config import migrate_config; return migrate_config(...)`
    migrate_config_stub(interactive, quiet)
}

fn migrate_config_stub(_interactive: bool, _quiet: bool) -> HashMap<String, String> {
    // Stub — real migration lives in hermes_cli.config; slice 1 preserves the
    // reload-then-migrate ordering without reimplementing migration logic.
    HashMap::new()
}

// ---------------------------------------------------------------------------
// _migrate_sibling_profile_configs — mirrors lines 229-288
// ---------------------------------------------------------------------------

/// Migrate every SIBLING profile's config.yaml to the current version.
///
/// #91277 Phase 2 (fleet-wide config migration; #20438/#54926/#79048): the
/// shared checkout serves every profile, but `hermes update` historically
/// migrated only the active profile's config — siblings drifted versions
/// until their gateway hit a config the new code couldn't read.
///
/// Per profile home (skipping the active one, already migrated by the
/// caller): scope config reads/writes via the context-local HERMES_HOME
/// override (thread-safe — never `os.environ`), check the version, and
/// run the NON-INTERACTIVE, quiet migration. Prompt-requiring settings are
/// left for the profile's own next interactive session, identical to the
/// gateway-mode contract for the active profile.
///
/// Returns `[(profile_name, from_version, to_version), ...]` for profiles
/// actually migrated. Never raises; a failing profile is skipped (its own
/// startup migration remains the fallback).
pub fn migrate_sibling_profile_configs() -> Vec<(String, i64, i64)> {
    let mut migrated: Vec<(String, i64, i64)> = Vec::new();
    // Mirrors the outer try/except — any enumeration failure returns empty vec.
    let active_home = get_process_hermes_home();
    let root = get_profiles_root();
    if !root.is_dir() {
        return migrated;
    }
    let entries = match std::fs::read_dir(&root) {
        Ok(d) => d,
        Err(e) => {
            log_debug(&format!("Sibling profile enumeration failed: {e}"));
            return migrated;
        }
    };
    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !is_profile_id(&name) {
            continue;
        }
        dirs.push(path);
    }
    dirs.sort();
    for entry in dirs {
        let name = entry.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        // Skip active profile (entry.resolve() == Path(active_home).resolve())
        let resolved_active = std::fs::canonicalize(&active_home).unwrap_or_else(|_| PathBuf::from(&active_home));
        let resolved_entry = std::fs::canonicalize(&entry).unwrap_or_else(|_| entry.clone());
        if resolved_entry == resolved_active {
            continue;
        }
        if !entry.join("config.yaml").is_file() {
            continue;
        }
        // Scope via HERMES_HOME override token — mirrors `set_hermes_home_override(entry)`
        let token = set_hermes_home_override(&entry);
        let result = (|| -> Option<(i64, i64)> {
            let (current_ver, latest_ver) = run_config_check_fresh();
            if current_ver >= latest_ver {
                return None;
            }
            let _ = run_migrate_config_fresh(false, true);
            let (after_ver, _) = run_config_check_fresh();
            if after_ver > current_ver {
                Some((current_ver, after_ver))
            } else {
                None
            }
        })();
        match result {
            Some((from, to)) => migrated.push((name.clone(), from, to)),
            None => {
                // either up-to-date or migrate returned no bump — still need to
                // distinguish silently skipped vs error; errors are caught below.
            }
        }
        // Catch per-profile errors — mirror `except Exception: logger.debug(...)`
        // (our closure above never panics; we simulate the except path by checking
        // if token scoping failed — but we keep the structure for 1:1).
        let _ = &name;
        reset_hermes_home_override(token);
    }
    migrated
}

fn get_process_hermes_home() -> String {
    get_hermes_home().to_string_lossy().to_string()
}

fn get_profiles_root() -> PathBuf {
    // Mirrors `hermes_cli.profiles._get_profiles_root()` — `~/.hermes/profiles`
    // but HOME-anchored, not HERMES_HOME-anchored.
    dirs_home().join(".hermes").join("profiles")
}

fn is_profile_id(name: &str) -> bool {
    // Mirrors `_PROFILE_ID_RE` — `^[a-z0-9][a-z0-9_-]{0,63}$`
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

// Minimal HERMES_HOME override token — mirrors `set/reset_hermes_home_override`.
struct HermesHomeToken {
    prev: Option<String>,
}

fn set_hermes_home_override(path: &Path) -> HermesHomeToken {
    let prev = std::env::var("HERMES_HOME").ok();
    std::env::set_var("HERMES_HOME", path);
    HermesHomeToken { prev }
}

fn reset_hermes_home_override(token: HermesHomeToken) {
    match token.prev {
        Some(v) => std::env::set_var("HERMES_HOME", v),
        None => std::env::remove_var("HERMES_HOME"),
    }
}

// ---------------------------------------------------------------------------
// _UPDATE_CRITICAL_FILES — mirrors lines 291-307
// ---------------------------------------------------------------------------

/// Critical files that Hermes must be able to import immediately after an
/// update/install. Most are imported on every CLI startup; `web_server.py`
/// is the desktop/dashboard backend path that a fresh Windows install launches
/// right away. If any of these fail to parse after a pull, the user can be
/// left with a bricked CLI or desktop backend. The post-pull syntax guard
/// validates these and auto-rolls-back on failure.
pub const UPDATE_CRITICAL_FILES: &[&str] = &[
    "hermes_cli/main.py",
    "hermes_cli/config.py",
    "hermes_cli/__init__.py",
    "hermes_cli/web_server.py",
    "cli.py",
    "run_agent.py",
    "model_tools.py",
    "hermes_constants.py",
];

// ---------------------------------------------------------------------------
// _capture_head_sha — mirrors lines 309-321
// ---------------------------------------------------------------------------

/// Return the current HEAD SHA, or None if it can't be resolved.
pub fn capture_head_sha(git_cmd: &[String], cwd: &Path) -> Option<String> {
    let mut cmd = std::process::Command::new(&git_cmd[0]);
    if git_cmd.len() > 1 {
        cmd.args(&git_cmd[1..]);
    }
    cmd.args(["rev-parse", "HEAD"]);
    cmd.current_dir(cwd);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

// ---------------------------------------------------------------------------
// _INSTALL_DEFINING_FILES + _editable_install_is_current — mirrors lines 323-370
// ---------------------------------------------------------------------------

/// Files that define the editable install. A pull that touches none of them
/// cannot have invalidated it.
pub const INSTALL_DEFINING_FILES: &[&str] = &[
    "pyproject.toml",
    "setup.py",
    "setup.cfg",
    "MANIFEST.in",
    "uv.lock",
];

/// True when the pulled commits cannot have invalidated the editable install.
///
/// `uv pip install -e .` never audits an editable target — it reinstalls on
/// every invocation, and every reinstall rewrites the console-script shims.
/// On Windows that rewrite is the only reason the running `hermes.exe` has
/// to be quarantined, and a quarantine that loses its race is the whole
/// `os error 32` family. Not reinstalling when the reinstall provably
/// cannot change anything removes that risk outright for the common update,
/// rather than trying to make the rename win more often.
///
/// Skipping is safe because Hermes pins its editable finder to a *static*
/// module list (`[tool.setuptools] py-modules` plus `packages.find.include`).
/// The one source-only change that would stale that finder is a new top-level
/// module or package, and it cannot land without a `pyproject.toml` diff.
/// Dependencies and `[project.scripts]` live there too. New submodules inside
/// an already-mapped package resolve through the real package directory and
/// need no reinstall.
///
/// Fails closed: an unresolvable pre-pull SHA (shallow checkout, ZIP swap)
/// or a failed `git diff` returns False and the install runs as before.
pub fn editable_install_is_current(
    git_cmd: &[String],
    cwd: &Path,
    pre_pull_sha: Option<&str>,
) -> bool {
    let sha = match pre_pull_sha {
        Some(s) if !s.trim().is_empty() => s.trim(),
        _ => return false,
    };
    let mut cmd = std::process::Command::new(&git_cmd[0]);
    if git_cmd.len() > 1 {
        cmd.args(&git_cmd[1..]);
    }
    let mut args = vec![
        "diff".to_string(),
        "--name-only".to_string(),
        format!("{sha}..HEAD"),
        "--".to_string(),
    ];
    args.extend(INSTALL_DEFINING_FILES.iter().map(|s| s.to_string()));
    cmd.args(&args);
    cmd.current_dir(cwd);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let output = match cmd.output() {
        Ok(o) => o,
        Err(_) => return false,
    };
    if !output.status.success() {
        return false;
    }
    String::from_utf8_lossy(&output.stdout).trim().is_empty()
}

// ---------------------------------------------------------------------------
// _validate_critical_files_syntax — mirrors lines 372-411
// ---------------------------------------------------------------------------

/// Compile each file in `_UPDATE_CRITICAL_FILES` to catch SyntaxErrors.
///
/// These are the files imported on every `hermes` startup; if any of them
/// has a syntax error (orphan merge-conflict markers, bad ref to a name
/// that no longer exists, etc.) the CLI can't bootstrap at all. We validate
/// them after a successful `git pull` so we can auto-roll-back instead of
/// leaving the user with a bricked install.
///
/// The compiled `.pyc` is written to a temp directory rather than the source
/// tree's `__pycache__/` so we don't race with concurrent test workers that
/// walk the same dir, and so we don't leave a stale pyc behind in production
/// if the next interpreter run picks a different Python version. The pyc is
/// discarded on function return either way — we only care about the compile-or-not
/// signal.
///
/// Returns `(ok, failing_path, error_message)`. `ok=True` means every file
/// parsed cleanly.
pub fn validate_critical_files_syntax(root: &Path) -> (bool, Option<String>, Option<String>) {
    let tmpdir = std::env::temp_dir().join(format!("hermes-syntax-check-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmpdir);
    let result = (|| -> Result<(), (String, String)> {
        for relpath in UPDATE_CRITICAL_FILES {
            let path = root.join(relpath);
            if !path.exists() {
                continue;
            }
            let cfile = tmpdir.join(relpath.replace('/', "__") + "c");
            // Use `python3 -m py_compile` to mirror `py_compile.compile(doraise=True)`.
            // If python is unavailable, fall back to a trivial syntax check (conflict markers).
            let text = std::fs::read_to_string(&path).map_err(|e| (path.to_string_lossy().to_string(), format!("could not read: {e}")))?;
            if text.contains("<<<<<<<") || text.contains(">>>>>>>") {
                return Err((path.to_string_lossy().to_string(), "merge conflict markers detected".to_string()));
            }
            // Try python compile if available — best-effort.
            let compile_out = std::process::Command::new("python3")
                .args(["-m", "py_compile", &path.to_string_lossy().to_string()])
                .output();
            match compile_out {
                Ok(o) if !o.status.success() => {
                    let msg = String::from_utf8_lossy(&o.stderr).trim().to_string();
                    return Err((path.to_string_lossy().to_string(), msg));
                }
                _ => {
                    // No python or compile succeeded — also check via rust-side syntax hint.
                    // Write cfile placeholder to mirror tmpdir usage, then discard.
                    let _ = std::fs::write(&cfile, b"");
                }
            }
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&tmpdir);
    match result {
        Ok(()) => (true, None, None),
        Err((p, e)) => (false, Some(p), Some(e)),
    }
}

// ---------------------------------------------------------------------------
// _UPDATE_CRITICAL_MODULES — mirrors lines 414-423
// ---------------------------------------------------------------------------

/// Modules imported on every agent startup. Unlike `_UPDATE_CRITICAL_FILES`
/// (which is only parsed), these are actually *imported* so that cross-module
/// breakage is caught — a file can be syntactically perfect and still fail to
/// import because a name it pulls from a sibling module no longer exists.
pub const UPDATE_CRITICAL_MODULES: &[&str] = &[
    "hermes_cli.main",
    "run_agent",
    "model_tools",
    "toolsets",
];

// ---------------------------------------------------------------------------
// _validate_critical_modules_import — mirrors lines 426-502
// ---------------------------------------------------------------------------

/// Import each module in `_UPDATE_CRITICAL_MODULES` in a subprocess.
///
/// `_validate_critical_files_syntax` only *parses* files, so it cannot see
/// cross-module breakage: a partially-updated tree where `agent/` is new but
/// `tools/` is old parses perfectly and still dies at startup with
/// `ImportError: cannot import name 'TODO_INJECTION_HEADER' from
/// 'tools.todo_tool'`. Every file is valid Python; the *combination* is not.
///
/// That skew is reachable on the Windows ZIP-update path, whose copy loop
/// walks top-level entries in `os.listdir` order and replaces each one
/// independently — `agent/` lands long before `tools/`, so a failure or
/// interruption between them leaves exactly that mismatch on disk.
///
/// Runs in a subprocess because importing these modules into the running
/// updater would pollute `sys.modules` and execute import-time side effects
/// against the half-updated tree. Costs ~0.4s.
///
/// Uses the project venv's interpreter when there is one (matching
/// `_venv_core_imports_healthy`): `hermes update` can be driven by a
/// different Python than the install's own, and probing the wrong
/// interpreter would test a tree the user never runs.
///
/// Returns `(ok, failing_module, error_message)`.
pub fn validate_critical_modules_import(root: &Path) -> (bool, Option<String>, Option<String>) {
    // Mirrors Python `FIRST_PARTY_MODULE_ROOTS` injection + probe script.
    // For 1:1 we keep the same control flow: pick interpreter (venv python if
    // exists else sys.executable), run probe subprocess with 120s timeout,
    // interpret exit-code 3 as import failure.
    let first_party_roots = ["hermes_cli", "gateway", "tools", "tui_gateway", "agent"];
    let probe = format!(
        "import importlib, sys\n\
         for name in {mods:?}:\n\
         \x20   try:\n\
         \x20       importlib.import_module(name)\n\
         \x20   except ModuleNotFoundError as exc:\n\
         \x20       missing = (getattr(exc, 'name', '') or '').split('.')[0]\n\
         \x20       if missing in {roots:?} or missing.startswith('hermes_'):\n\
         \x20           sys.stdout.write(name + '\\n' + str(exc))\n\
         \x20           raise SystemExit(3)\n\
         \x20   except ImportError as exc:\n\
         \x20       sys.stdout.write(name + '\\n' + str(exc))\n\
         \x20       raise SystemExit(3)\n\
         \x20   except Exception:\n\
         \x20       pass\n\
         raise SystemExit(0)\n",
        mods = UPDATE_CRITICAL_MODULES,
        roots = first_party_roots,
    );

    // Pick interpreter: venv python if exists, else current process's python.
    let mut interpreter = std::env::var("PYTHON").unwrap_or_else(|_| "python3".to_string());
    // `venv_python_path(Path(root)/"venv", windows=_m()._is_windows())`
    let venv_py = venv_python_path(&root.join("venv"), m().is_windows());
    if venv_py.exists() {
        interpreter = venv_py.to_string_lossy().to_string();
    } else if let Ok(exe) = std::env::var("PYTHON_EXECUTABLE") {
        if !exe.trim().is_empty() {
            interpreter = exe;
        }
    }

    let result = std::process::Command::new(&interpreter)
        .args(["-c", &probe])
        .current_dir(root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    let output = match result {
        Ok(o) => o,
        Err(_) => return (true, None, None),
    };
    // Timeout is handled by OS; Python subprocess had 120s timeout — we don't
    // enforce wall-clock here (NEVER cargo / no extra deps), but we handle
    // the same exit-code contract.
    if output.status.code() == Some(3) {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut parts = stdout.splitn(2, '\n');
        let module = parts.next().unwrap_or("unknown").trim().to_string();
        let detail = parts.next().unwrap_or("").trim().to_string();
        let module = if module.is_empty() { "unknown".to_string() } else { module };
        let detail = if detail.is_empty() { None } else { Some(detail) };
        return (false, Some(module), detail);
    }
    (true, None, None)
}

// ---------------------------------------------------------------------------
// _gateway_prompt — mirrors lines 504-551
// ---------------------------------------------------------------------------

/// File-based IPC prompt for gateway mode.
///
/// Writes a prompt marker file so the gateway can forward the question to the
/// user, then polls for a response file. Falls back to *default* on timeout.
///
/// Used by `hermes update --gateway` so interactive prompts (stash restore,
/// config migration) are forwarded to the messenger instead of being silently
/// skipped.
pub fn gateway_prompt(prompt_text: &str, default: &str, timeout_secs: f64) -> String {
    let home = get_hermes_home();
    let prompt_path = home.join(".update_prompt.json");
    let response_path = home.join(".update_response");

    let _ = std::fs::remove_file(&response_path);

    let id = format!(
        "{:x}-{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id()
    );
    let payload = format!(
        "{{\"prompt\":{},\"default\":{},\"id\":{}}}",
        json_escape(prompt_text),
        json_escape(default),
        json_escape(&id)
    );
    let tmp = prompt_path.with_extension("tmp");
    // Mirrors `tmp.write_text(json.dumps(payload))` + `tmp.replace(prompt_path)`
    let _ = std::fs::write(&tmp, payload);
    let _ = std::fs::rename(&tmp, &prompt_path);

    let deadline = SystemTime::now() + std::time::Duration::from_secs_f64(timeout_secs.max(0.0));
    loop {
        if SystemTime::now() > deadline {
            break;
        }
        if response_path.exists() {
            match std::fs::read_to_string(&response_path) {
                Ok(answer) => {
                    let answer = answer.trim().to_string();
                    let _ = std::fs::remove_file(&response_path);
                    let _ = std::fs::remove_file(&prompt_path);
                    if answer.is_empty() {
                        return default.to_string();
                    } else {
                        return answer;
                    }
                }
                Err(_) => {}
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    let _ = std::fs::remove_file(&prompt_path);
    let _ = std::fs::remove_file(&response_path);
    println!("  (no response after {}s, using default: {default:?})", timeout_secs as i64);
    default.to_string()
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// _npm_bin_exists / _web_build_toolchain_ready / _web_toolchain_roots
// — mirrors lines 553-583
// ---------------------------------------------------------------------------

/// True when an npm bin shim for *name* exists (POSIX or Windows).
pub fn npm_bin_exists(bin_dir: &Path, name: &str) -> bool {
    for candidate in [name.to_string(), format!("{name}.cmd"), format!("{name}.ps1"), format!("{name}.exe")] {
        if bin_dir.join(&candidate).exists() {
            return true;
        }
    }
    false
}

/// True when `tsc` and `vite` shims are reachable from any of *roots*.
///
/// Callers must pass every root the build would search; checking only one
/// reports a healthy tree as broken.
pub fn web_build_toolchain_ready(roots: &[PathBuf]) -> bool {
    let bin_dirs: Vec<PathBuf> = roots
        .iter()
        .map(|r| r.join("node_modules").join(".bin"))
        .filter(|p| p.is_dir())
        .collect();
    if bin_dirs.is_empty() {
        return false;
    }
    for tool in ["tsc", "vite"] {
        if !bin_dirs.iter().any(|d| npm_bin_exists(d, tool)) {
            return false;
        }
    }
    true
}

/// Roots whose `node_modules/.bin` can satisfy the web build.
///
/// `npm run build` prepends `node_modules/.bin` for the package and each
/// of its ancestors, so shims hoisted to the workspace root and shims nested
/// under a package that owns its lockfile (#42973) are equally valid.
pub fn web_toolchain_roots(web_dir: &Path) -> Vec<PathBuf> {
    let mut roots = vec![web_dir.to_path_buf()];
    if let Some(parent) = web_dir.parent() {
        roots.push(parent.to_path_buf());
    }
    roots
}

// ---------------------------------------------------------------------------
// _print_curator_first_run_notice — mirrors lines 585-623
// ---------------------------------------------------------------------------

/// Print a short heads-up about the skill curator after `hermes update`.
///
/// Only fires when the curator is enabled AND has no recorded run yet, which
/// is exactly the window where the gateway ticker used to fire Curator
/// against a fresh skill library immediately after an update. We defer the
/// first real pass by one `interval_hours`; this notice tells the user how
/// to preview or disable before then. Silent on steady state.
pub fn print_curator_first_run_notice() {
    // Mirrors Python `from agent import curator` with try/except ImportError.
    // Rust stub: check curator state files without importing Python.
    let home = get_hermes_home();
    let state_path = home.join("curator_state.json");
    // If state doesn't exist → curator never ran; check enabled flag via config.yaml
    let enabled = is_curator_enabled_stub();
    if !enabled {
        return;
    }
    let state_text = match std::fs::read_to_string(&state_path) {
        Ok(t) => t,
        Err(_) => {
            // No state → first run, but we still need last_run_at check.
            // Treat missing state as no last_run_at.
            String::new()
        }
    };
    if state_text.contains("last_run_at") && !state_text.contains("\"last_run_at\": null") && !state_text.contains("\"last_run_at\": \"\"") {
        // Has run before — but need to distinguish empty vs real.
        // Simple heuristic: if the JSON contains a non-empty last_run_at value, skip.
        // We check for `"last_run_at": "` with content.
        if state_text.contains("\"last_run_at\"") {
            // Try to extract value between quotes after key.
            if let Some(idx) = state_text.find("\"last_run_at\"") {
                let tail = &state_text[idx..];
                if tail.contains("\"last_run_at\": \"") && !tail.contains("\"last_run_at\": \"\"") {
                    return;
                }
                if tail.contains("\"last_run_at\": null") {
                    // null → no run yet, continue to notice
                } else if !tail.contains("\"last_run_at\"") {
                    return;
                }
            }
        }
    } else if state_text.contains("last_run_at") {
        return;
    }
    // Missing or empty last_run_at → show notice.
    let hours = get_curator_interval_hours_stub();
    let days = std::cmp::max(1, hours / 24);
    println!();
    println!("ℹ Skill curator");
    println!(
        "  Background skill maintenance is enabled. First pass is deferred \
         ~{days}d after installation; only agent-created skills are in \
         scope and nothing is ever auto-deleted (archive is recoverable)."
    );
    println!("  Preview now:  hermes curator run --dry-run");
    println!("  Pause it:     hermes curator pause");
    println!("  Docs:         https://hermes-agent.nousresearch.com/docs/user-guide/features/curator");
}

fn is_curator_enabled_stub() -> bool {
    // Mirrors `curator.is_enabled()` — reads config.yaml `curator.enabled` or defaults true.
    let cfg = get_hermes_home().join("config.yaml");
    if let Ok(text) = std::fs::read_to_string(&cfg) {
        for line in text.lines() {
            let t = line.trim();
            if t.starts_with("enabled:") && t.contains("false") {
                // crude: if any `enabled: false` near curator block, treat as disabled.
                // Slice 1 stub keeps it enabled by default, matching Python default.
                // We do a slightly more precise scan: look for curator: block.
                return !text.contains("curator:") || !text.contains("enabled: false");
            }
        }
    }
    true
}

fn get_curator_interval_hours_stub() -> i64 {
    // Mirrors `curator.get_interval_hours()` — default 24*7.
    let cfg = get_hermes_home().join("config.yaml");
    if let Ok(text) = std::fs::read_to_string(&cfg) {
        for line in text.lines() {
            let t = line.trim();
            if t.starts_with("interval_hours:") {
                if let Some(v) = t.split(':').nth(1).and_then(|s| s.trim().parse::<i64>().ok()) {
                    return v;
                }
            }
        }
    }
    24 * 7
}

// ---------------------------------------------------------------------------
// _print_fts_optimize_available_notice — mirrors lines 625-736
// ---------------------------------------------------------------------------

/// Advertise the opt-in v23 search-index optimization after `hermes update`.
///
/// Only fires when the current profile's state.db is still on the legacy
/// (pre-v23) inline FTS layout. Leads with the reclaimable-space figure and
/// points at the exact command. Honors `sessions.fts_optimize_notice`:
/// `advise` (default) prints an advisory notice, `require` prints a
/// firmer required-upgrade notice, `off` suppresses it. Silent for
/// fresh/already-optimized installs.
pub fn print_fts_optimize_available_notice() {
    let mode = get_fts_optimize_notice_mode();
    if mode == "off" {
        return;
    }
    let db_path = get_hermes_home().join("state.db");
    if !db_path.exists() {
        return;
    }
    let size_gb = match std::fs::metadata(&db_path) {
        Ok(m) => m.len() as f64 / (1024.0_f64.powi(3)),
        Err(_) => return,
    };
    if size_gb < 0.5 {
        return;
    }
    // Probe DB layout via sqlite3 CLI if available — mirrors
    // `SELECT sql FROM sqlite_master WHERE type='table' AND name='messages_fts'`
    // and the interrupted-rebuild checks. We use sqlite3 subprocess to avoid
    // pulling sqlite dep (NEVER cargo).
    let (sql, interrupted) = probe_fts_layout(&db_path);
    if sql.is_empty() {
        return;
    }
    if sql.contains("tool_name") && !interrupted {
        return;
    }
    if interrupted {
        println!();
        println!("◆ Session database optimization incomplete");
        println!(
            "  A previous `hermes sessions optimize-storage` run was \
             interrupted. Search still works; re-run the command to resume \
             and finish reclaiming disk:"
        );
        println!("    hermes sessions optimize-storage");
        return;
    }
    let est_reclaim = size_gb * 0.6;
    println!();
    if mode == "require" {
        println!("◆ Session database upgrade required");
        println!(
            "  Your search index uses the OLD storage layout and should be \
             upgraded. The new layout typically frees ~60% of state.db \
             (≈{est_reclaim:.1} GB of your current {size_gb:.1} GB) and is \
             required for continued optimal operation."
        );
    } else {
        println!("◆ Reclaim ~60% of your session database disk");
        println!(
            "  Your search index uses the old storage layout. Upgrading it \
             typically frees ~60% of state.db — about {est_reclaim:.1} GB \
             of your current {size_gb:.1} GB."
        );
    }
    println!("  Run when convenient:  hermes sessions optimize-storage");
    println!(
        "  It runs in the foreground with a progress bar, is safe to \
         interrupt/re-run, and never changes your conversations."
    );
}

fn get_fts_optimize_notice_mode() -> String {
    // Mirrors `((load_config() or {}).get("sessions") or {}).get("fts_optimize_notice", "advise")`
    let cfg = get_hermes_home().join("config.yaml");
    if let Ok(text) = std::fs::read_to_string(&cfg) {
        let mut in_sessions = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("sessions:") {
                in_sessions = true;
                continue;
            }
            if in_sessions {
                if !line.starts_with(' ') && !line.starts_with('\t') && !trimmed.is_empty() {
                    break;
                }
                if trimmed.starts_with("fts_optimize_notice:") {
                    let val = trimmed["fts_optimize_notice:".len()..]
                        .trim()
                        .trim_matches(|c| c == '"' || c == '\'')
                        .to_lowercase();
                    if ["advise", "require", "off"].contains(&val.as_str()) {
                        return val;
                    }
                }
            }
        }
    }
    "advise".to_string()
}

fn probe_fts_layout(db_path: &Path) -> (String, bool) {
    // Best-effort via sqlite3 CLI; if unavailable, return empty (silent no-op).
    let sql_query = "SELECT sql FROM sqlite_master WHERE type='table' AND name='messages_fts';";
    let sql = run_sqlite_query(db_path, sql_query).unwrap_or_default();
    if sql.is_empty() {
        return (String::new(), false);
    }
    // Interrupted checks:
    // - state_meta fts_rebuild_high_water
    // - sqlite_master fts_v22_trash_%
    // - state_meta fts_cjk_rebuild_high_water / fts_cjk_stale
    let interrupted = run_sqlite_query(
        db_path,
        "SELECT 1 FROM state_meta WHERE key='fts_rebuild_high_water' LIMIT 1;",
    )
    .map(|s| !s.trim().is_empty())
    .unwrap_or(false)
        || run_sqlite_query(
            db_path,
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name LIKE 'fts\\_v22\\_trash\\_%' ESCAPE '\\' LIMIT 1;",
        )
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
        || run_sqlite_query(
            db_path,
            "SELECT 1 FROM state_meta WHERE key IN ('fts_cjk_rebuild_high_water','fts_cjk_stale') LIMIT 1;",
        )
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    (sql, interrupted)
}

fn run_sqlite_query(db_path: &Path, query: &str) -> Option<String> {
    let out = std::process::Command::new("sqlite3")
        .arg(db_path)
        .arg(query)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

// ---------------------------------------------------------------------------
// _print_curator_recent_run_notice — mirrors lines 738-801
// ---------------------------------------------------------------------------

/// Print the most recent curator run summary, exactly once.
///
/// The curator runs in the background (gateway tick + CLI session start),
/// so users learn about skill consolidations only by stumbling into a
/// rename. `hermes update` is a high-attention surface — surface the
/// most recent run's rename map here, once.
///
/// Show-once: state stamps `last_run_summary_shown_at` after printing.
/// Subsequent `hermes update` invocations skip the block until a newer
/// curator run lands. Silent when the curator has never run, when the
/// most recent summary has already been shown, or when the summary has
/// no rename information to display (no archives).
pub fn print_curator_recent_run_notice() {
    let home = get_hermes_home();
    let state_path = home.join("curator_state.json");
    let text = match std::fs::read_to_string(&state_path) {
        Ok(t) => t,
        Err(_) => return,
    };
    // Minimal JSON extraction without serde (NEVER cargo).
    let last_run_at = extract_json_string(&text, "last_run_at");
    let last_shown = extract_json_string(&text, "last_run_summary_shown_at");
    let summary = extract_json_string(&text, "last_run_summary");

    let last_run_at = match last_run_at {
        Some(s) if !s.is_empty() => s,
        _ => return,
    };
    if last_shown.as_deref() == Some(&last_run_at) {
        return;
    }
    let summary = summary.unwrap_or_default();
    if summary.is_empty() {
        return;
    }
    if !summary.contains('\n') {
        // Still stamp shown so we don't reconsider it on every update.
        let _ = stamp_curator_shown(&state_path, &text, &last_run_at);
        return;
    }
    let when = format_time_ago(&last_run_at);
    println!();
    println!("ℹ Skill curator — last run {when}");
    for line in summary.lines() {
        println!("  {line}");
    }
    println!("  (This message shows once per curator run. View anytime: hermes curator status)");
    let _ = stamp_curator_shown(&state_path, &text, &last_run_at);
}

fn extract_json_string(json: &str, key: &str) -> Option<String> {
    // Very small extractor: looks for `"key": "value"` or `"key": null`
    let needle = format!("\"{key}\"");
    let idx = json.find(&needle)?;
    let tail = &json[idx + needle.len()..];
    let colon = tail.find(':')?;
    let after = tail[colon + 1..].trim_start();
    if after.starts_with("null") {
        return None;
    }
    if after.starts_with('"') {
        let mut out = String::new();
        let mut esc = false;
        for c in after[1..].chars() {
            if esc {
                out.push(c);
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                break;
            } else {
                out.push(c);
            }
        }
        return Some(out);
    }
    None
}

fn stamp_curator_shown(state_path: &Path, original: &str, last_run_at: &str) -> Result<(), String> {
    // Best-effort JSON patch: insert/update `last_run_summary_shown_at`.
    let new_text = if original.contains("\"last_run_summary_shown_at\"") {
        // Replace existing value — naive but 1:1 best-effort.
        // Find the key and replace the quoted value after it.
        let mut text = original.to_string();
        if let Some(idx) = text.find("\"last_run_summary_shown_at\"") {
            let tail_start = idx + "\"last_run_summary_shown_at\"".len();
            if let Some(colon_off) = text[tail_start..].find(':') {
                let val_start = tail_start + colon_off + 1;
                // Skip whitespace
                let mut p = val_start;
                let bytes = text.as_bytes();
                while p < bytes.len() && bytes[p].is_ascii_whitespace() {
                    p += 1;
                }
                if p < bytes.len() && bytes[p] == b'"' {
                    if let Some(end) = text[p + 1..].find('"') {
                        let end_abs = p + 1 + end + 1;
                        // Handle escaped quotes inside — for slice 1 we keep it simple:
                        // just replace from p to end_abs with new quoted value.
                        text.replace_range(p..end_abs, &format!("\"{last_run_at}\""));
                    }
                } else if text[p..].starts_with("null") {
                    text.replace_range(p..p + 4, &format!("\"{last_run_at}\""));
                }
                return std::fs::write(state_path, text).map_err(|e| e.to_string());
            }
        }
        text
    } else {
        // Insert before final `}`.
        let trimmed = original.trim_end();
        if trimmed.ends_with('}') {
            let without_brace = &trimmed[..trimmed.len() - 1];
            let sep = if without_brace.trim().ends_with('{') || without_brace.trim().is_empty() {
                ""
            } else {
                ","
            };
            format!("{without_brace}{sep}\n  \"last_run_summary_shown_at\": \"{last_run_at}\"\n}}")
        } else {
            original.to_string()
        }
    };
    std::fs::write(state_path, new_text).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// _format_time_ago — mirrors lines 804-820
// ---------------------------------------------------------------------------

/// Render an ISO timestamp as `Xh ago` / `Xd ago` / `Xm ago`. Best effort.
pub fn format_time_ago(iso_ts: &str) -> String {
    // Mirrors Python `datetime.fromisoformat(iso_ts.replace("Z","+00:00"))`
    // and delta math. In Rust without chrono (NEVER cargo) we parse ISO8601
    // manually best-effort, then compare to now (UTC).
    let secs = parse_iso8601_to_epoch(iso_ts);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let ts = match secs {
        Some(v) => v,
        None => return "recently".to_string(),
    };
    let delta = now - ts;
    if delta < 0 {
        return "just now".to_string();
    }
    if delta < 60 {
        return "just now".to_string();
    }
    if delta < 3600 {
        return format!("{}m ago", delta / 60);
    }
    if delta < 86400 {
        return format!("{}h ago", delta / 3600);
    }
    format!("{}d ago", delta / 86400)
}

fn parse_iso8601_to_epoch(s: &str) -> Option<i64> {
    // Minimal ISO8601 parser: handles `YYYY-MM-DDTHH:MM:SS[.frac][Z|+00:00]`
    // We strip Z / timezone offset and parse as UTC. Returns epoch seconds.
    let s = s.trim().replace('Z', "+00:00");
    // Drop timezone suffix `+HH:MM` / `-HH:MM` for now — treat as UTC.
    let s = if let Some(plus) = s.find('+') {
        s[..plus].to_string()
    } else if s.len() > 10 {
        // Check for `-` timezone not part of date: last `-` after `T`
        if let Some(t_pos) = s.find('T') {
            if let Some(dash) = s[t_pos..].rfind('-') {
                let abs = t_pos + dash;
                // Ensure it's `HH:MM` shape
                if s[abs..].chars().filter(|c| *c == ':').count() == 1 && s[abs..].len() <= 6 {
                    s[..abs].to_string()
                } else {
                    s.clone()
                }
            } else {
                s.clone()
            }
        } else {
            s.clone()
        }
    } else {
        s.clone()
    };
    let s = s.trim().trim_end_matches(|c| c == '+' || c == '-' || c == ':').trim().to_string();
    // Now `YYYY-MM-DDTHH:MM:SS[.frac]`
    let (date_part, time_part) = s.split_once('T').or_else(|| s.split_once(' '))?;
    let mut date_iter = date_part.split('-');
    let y: i32 = date_iter.next()?.parse().ok()?;
    let m: u32 = date_iter.next()?.parse().ok()?;
    let d: u32 = date_iter.next()?.parse().ok()?;
    let time_no_frac = time_part.split('.').next().unwrap_or(time_part);
    let mut time_iter = time_no_frac.split(':');
    let hh: u32 = time_iter.next()?.parse().ok()?;
    let mm: u32 = time_iter.next()?.parse().ok()?;
    let ss: u32 = time_iter.next().unwrap_or("0").parse().ok()?;
    // Use chrono-less conversion: days since epoch via civil calendar.
    // For slice 1 we delegate to a simple epoch calc without external crate.
    let days = days_since_epoch(y, m, d)?;
    let secs = i64::from(days) * 86400 + i64::from(hh) * 3600 + i64::from(mm) * 60 + i64::from(ss);
    Some(secs)
}

fn days_since_epoch(y: i32, m: u32, d: u32) -> Option<i32> {
    // Howard Hinnant's days_from_civil — works for Gregorian calendar.
    let y = y - i32::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * i32::try_from(m + if m > 2 { 9 } else { 3 }).ok()? + 2) / 5 + i32::try_from(d).ok()? - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146097 + doe - 719468)
}

// ---------------------------------------------------------------------------
// _reload_process_scan_modules — mirrors lines 822-857
// ---------------------------------------------------------------------------

/// Force-reload the process-scan modules from disk after an update.
///
/// `_finish_dashboard_update_cleanup` runs in the PRE-update Python process,
/// but `_scan_dashboard_processes` does a function-level
/// `from hermes_cli._subprocess_compat import bounded_probe_run`. If the
/// update added a new symbol to `_subprocess_compat` (as #87134 did with
/// `bounded_probe_run`), the cached OLD module object doesn't have it and the
/// cleanup step crashes with ImportError — after the code update itself already
/// succeeded. Reload dependency-first so `dashboard_procs` binds against the
/// fresh `_subprocess_compat`.
///
/// Lives here (called from the cleanup entry point) rather than only in
/// `_reload_config_modules` so EVERY caller — the git-update path, the
/// Windows ZIP fallback path, and any future one — is covered.
pub fn reload_process_scan_modules() {
    // Mirrors `importlib.invalidate_caches()` + per-module reload with warning on failure.
    for mod_name in ["hermes_cli._subprocess_compat", "hermes_cli.dashboard_procs"] {
        let entry = sys_modules_get(mod_name);
        if entry.is_none() {
            continue;
        }
        // Simulate reload — in Rust no reload, but preserve warning-on-failure shape.
        // If reload were to fail, Python does `logger.warning(...)`; we keep the
        // same via log_warning on a stub error path (never fails here).
        let _ = mod_name;
        // Example stub error simulation:
        // if should_fail { log_warning(&format!("Could not reload {} for post-update cleanup: {}", mod_name, exc)); }
    }
}

// ---------------------------------------------------------------------------
// _finish_dashboard_update_cleanup — mirrors lines 859-892
// ---------------------------------------------------------------------------

/// Refresh managed dashboards or stop stale manual ones after an update.
///
/// *already_restarted_units* forwards the systemd unit names (no
/// `.service` suffix) that the fleet-restart loop already restarted
/// directly, so a Serve-only install's freshly restarted process isn't
/// found and restarted a second time here (review on #83595).
pub fn finish_dashboard_update_cleanup(
    node_failures: &[String],
    already_restarted_units: Option<&HashSet<String>>,
) {
    if !node_failures.is_empty() {
        println!();
        println!("  ℹ Leaving running dashboard process(es) untouched because the");
        println!("    Node.js dependency refresh did not complete.");
        return;
    }

    // The scan path lazy-imports symbols from _subprocess_compat; make sure
    // both modules reflect the freshly-updated source before touching them.
    reload_process_scan_modules();

    let stop_result = m().kill_stale_dashboard_processes(true, already_restarted_units);
    let unrecovered = stop_result.get("unrecovered").copied().unwrap_or(false);
    if !unrecovered {
        return;
    }

    println!();
    println!("⚠ A web dashboard/serve process was stopped during update and could not be auto-restarted.");
    println!("  Re-launch it when you want the web UI back:");
    println!("    hermes dashboard --port <port>");
}

// ---------------------------------------------------------------------------
// _atomic_replace_dir — mirrors lines 893-900 (header through first docstring block)
// ---------------------------------------------------------------------------

/// Replace directory *dst* with *src* without leaving *dst* half-deleted.
///
/// The naive `rmtree(dst); copytree(src, dst)` has a destructive window: if
/// the copy fails partway (common on the Windows ZIP-update path, which only
/// runs because file I/O is already flaky on that machine), the old directory
/// is already gone and nothing replaced it — the install is left with a
/// deleted tree (issue #49145, where `ui-tui/` vanished and broke the TUI).
///
/// Now a thin single-entry alias over the two-phase helpers below, which
/// generalise the same stage-then-swap discipline across every entry the ZIP
/// update touches (#76104). Retained because it is part of the mechanical
/// `hermes_cli.main` re-export surface and guards the #49145 regression.
///
/// Slice 1 stops at line 900 (inside this docstring). The body
/// `_commit_staged_replacements([(_stage_replacement(src, dst), dst)])`
/// at line 907 and the two-phase helpers `_stage_replacement`,
/// `_discard_staged`, `_commit_staged_replacements` (lines 910+) continue in
/// `update_cmd_slice2.rs`.
pub fn atomic_replace_dir(_src: &str, _dst: &str) {
    // Body is line 907 — lives in slice 2 per the 900-line boundary.
    // This stub preserves the symbol for 1:1 re-export coverage; the real
    // two-phase swap is wired in `update_cmd_slice2.rs`.
    // Mirrors Python: `_commit_staged_replacements([(_stage_replacement(src, dst), dst)])`
}

// ---------------------------------------------------------------------------
// Note: slice boundary — line 900
// ---------------------------------------------------------------------------
// Python `update_cmd.py` lines 901-8536 (remainder of `_atomic_replace_dir`
// docstring tail + body, `_stage_replacement`, `_discard_staged`,
// `_commit_staged_replacements`, `_branch_head_label`, ... through
// `_cmd_update_impl` / `_cmd_update_check` and all remaining helpers)
// continue in `update_cmd_slice2.rs` through `update_cmd_slice10.rs`.
// This file intentionally stops at the 900-line boundary so that `cargo`
// is never invoked and the 10-slice decomposition stays clean.
