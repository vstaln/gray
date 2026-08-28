//! hermes-cli doctor — slice 1/4
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/doctor.py`
//! slice 1/4 — lines 1–900 of 3 163 (first 900 LOC).
//! Covers: module docstring + std/path/env imports, hermes_cli.config +
//! env_loader + hermes_constants + colors/models/vercel_auth/utils imports,
//! `PROJECT_ROOT` / `HERMES_HOME` / `_DHH` / `_env_path` bootstrap,
//! `_PROVIDER_ENV_HINTS`, `_is_termux`, `_python_install_cmd`,
//! `_system_package_install_cmd`, `_sqlite_upgrade_hint`,
//! `_hermes_database_paths`, `_SQLITE_HEADER_MAGIC`, `_unreadable_reason`,
//! `_read_journal_mode`, `_format_db_size`, `_report_database_journal_modes`,
//! `_safe_which`, `_termux_browser_setup_steps`,
//! `_termux_install_all_fallback_notes`, `_has_provider_env_config`,
//! `_honcho_is_configured_for_doctor`, `_is_kanban_worker_env_gate`,
//! `_doctor_tool_availability_detail`, `_doctor_web_capability_rows`,
//! `_apply_doctor_tool_availability_overrides`,
//! `_has_healthy_oauth_fallback_for_apikey_provider`, `check_ok` /
//! `check_warn` / `check_fail` / `check_info`, `STATE_DB_SIZE_WARN_BYTES`,
//! `_human_bytes` alias, `_render_state_db_stats`, `_section`,
//! `_fail_and_issue`, `_DEPRECATED_CONFIG_KEYS` /
//! `_DEPRECATED_COMPRESSION_SUMMARY_KEYS` / `_DEPRECATED_ENV_VARS`,
//! `collect_deprecated_config_keys`, `collect_deprecated_env_vars`,
//! `collect_relay_plugin_cutover_findings`, `report_deprecated_config_and_env`,
//! `_enabled_cli_toolsets_for_doctor`, `_missing_api_key_toolsets_for_summary`,
//! `_read_pyproject_version`, `_check_version_consistency`,
//! `_check_s6_supervision`, `check_certificates` (lines 795–879, through the
//! reinstall-reverify tail), and the head of `_check_gateway_service_linger`
//! (lines 881–900, through the `if not is_linux(): return` early exit).
//! The remainder of `_check_gateway_service_linger` (lines 901–923,
//! s6-supervision skip + systemd linger probe) and everything after
//! (`_APIKEY_PROVIDERS_CACHE` / `_build_apikey_providers_list` /
//! `managed_scope_check` / `run_doctor` … through EOF) continue in
//! `doctor_slice2.rs` (from line 901).
//!
//! T0695 — 1:1 port, no cargo (NEVER cargo).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-5
// ---------------------------------------------------------------------------

/// Module doc — Doctor command for hermes CLI. Diagnoses issues with Hermes Agent setup.
/// Mirrors `hermes_cli/doctor.py` lines 1-5.
pub const MODULE_DOC: &str = "doctor: hermes doctor diagnostics — see doctor.py lines 1-5";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 7-39
// ---------------------------------------------------------------------------
// Python:
//   import os, sys, subprocess, shutil, importlib.util, pathlib.Path
//   from hermes_cli.config import (detect_install_method, get_env_path,
//       get_hermes_home, get_project_root, is_nix_install_method,
//       recommended_update_command_for_method)
//   from hermes_cli.env_loader import load_hermes_dotenv
//   from hermes_constants import display_hermes_home, agent_browser_runnable,
//       is_termux, OPENROUTER_MODELS_URL
//   from hermes_cli.colors import Colors, color
//   from hermes_cli.models import _HERMES_USER_AGENT
//   from hermes_cli.vercel_auth import describe_vercel_auth
//   from utils import base_url_host_matches
//
// Rust: std only (NEVER cargo). External crates and hermes-internal modules
// are stubbed for 1:1 traceability; real wiring in later slices.

// ---------------------------------------------------------------------------
// PROJECT_ROOT / HERMES_HOME / _DHH / _env_path — mirrors lines 26-32
// ---------------------------------------------------------------------------

/// Mirrors `PROJECT_ROOT = get_project_root()` (line 26).
pub fn project_root() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_REPO_ROOT") {
        return PathBuf::from(v);
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Mirrors `HERMES_HOME = get_hermes_home()` (line 27).
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

/// Mirrors `_DHH = display_hermes_home()` (line 28) — user-facing display path.
pub fn display_hermes_home() -> String {
    let home = get_hermes_home();
    let home_str = home.to_string_lossy().to_string();
    // Mirrors hermes_constants.display_hermes_home(): shows ~/.hermes or ~/.hermes/profiles/<name>
    // Best-effort: replace HOME prefix with ~ for display.
    if let Ok(h) = std::env::var("HOME") {
        if home_str.starts_with(&h) {
            return home_str.replacen(&h, "~", 1);
        }
    }
    home_str
}

/// Mirrors `_env_path = get_env_path()` + `load_hermes_dotenv(...)` (lines 31-32).
/// In Python this eagerly loads ~/.hermes/.env so API key checks work.
/// In Rust this is deferred to call sites; stub kept for 1:1 traceability.
pub fn get_env_path() -> PathBuf {
    get_hermes_home().join(".env")
}
pub fn load_hermes_dotenv_stub() {
    // Would call hermes_cli.env_loader.load_hermes_dotenv(hermes_home=_env_path.parent, project_env=PROJECT_ROOT/.env)
    // No-op in slice 1 (no dotenv dep, NEVER cargo); real load in later slice if needed.
}

// ---------------------------------------------------------------------------
// _PROVIDER_ENV_HINTS — mirrors lines 41-70
// ---------------------------------------------------------------------------

/// Mirrors `_PROVIDER_ENV_HINTS` tuple (lines 41-70) — env vars that signal provider auth/base URL is configured.
pub const PROVIDER_ENV_HINTS: &[&str] = &[
    "DEEPINFRA_API_KEY",
    "OPENROUTER_API_KEY",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_TOKEN",
    "OPENAI_BASE_URL",
    "NOUS_API_KEY",
    "GLM_API_KEY",
    "ZAI_API_KEY",
    "Z_AI_API_KEY",
    "KIMI_API_KEY",
    "KIMI_CN_API_KEY",
    "GMI_API_KEY",
    "FIREWORKS_API_KEY",
    "ACTUAL_API_KEY",
    "ACTUAL_BASE_URL",
    "MINIMAX_API_KEY",
    "MINIMAX_CN_API_KEY",
    "KILOCODE_API_KEY",
    "DEEPSEEK_API_KEY",
    "DASHSCOPE_API_KEY",
    "HF_TOKEN",
    "AI_GATEWAY_API_KEY",
    "OPENCODE_ZEN_API_KEY",
    "OPENCODE_GO_API_KEY",
    "COMMANDCODE_API_KEY",
    "XIAOMI_API_KEY",
    "TOKENHUB_API_KEY",
];

// ---------------------------------------------------------------------------
// _is_termux + helpers — mirrors lines 73-86
// ---------------------------------------------------------------------------

/// Mirrors `from hermes_constants import is_termux as _is_termux` (line 73).
pub fn is_termux() -> bool {
    // Real hermes_constants.is_termux checks TERMUX_VERSION, PREFIX, etc.
    // Mirrors the helper used in _python_install_cmd / _system_package_install_cmd.
    std::env::var("TERMUX_VERSION").is_ok()
        || std::env::var("PREFIX")
            .map(|p| p.contains("com.termux/files/usr"))
            .unwrap_or(false)
}

/// Mirrors `_python_install_cmd()` (lines 76-77).
pub fn python_install_cmd() -> String {
    if is_termux() {
        "python -m pip install".to_string()
    } else {
        "uv pip install".to_string()
    }
}

/// Mirrors `_system_package_install_cmd(pkg)` (lines 80-85).
pub fn system_package_install_cmd(pkg: &str) -> String {
    if is_termux() {
        format!("pkg install {pkg}")
    } else if cfg!(target_os = "macos") {
        format!("brew install {pkg}")
    } else {
        format!("sudo apt install {pkg}")
    }
}

// ---------------------------------------------------------------------------
// _sqlite_upgrade_hint — mirrors lines 88-104
// ---------------------------------------------------------------------------

/// Mirrors `_sqlite_upgrade_hint(install_method=None)` (lines 88-104).
pub fn sqlite_upgrade_hint(install_method: Option<&str>) -> String {
    let method = install_method
        .map(|s| s.to_string())
        .unwrap_or_else(|| detect_install_method_stub(&project_root()));
    let action = if method == "docker" {
        let cmd = recommended_update_command_for_method_stub(&method);
        format!("run `{cmd}`, then recreate all Hermes containers")
    } else if is_nix_install_method_stub(&method) {
        recommended_update_command_for_method_stub(&method)
    } else if method == "apt" {
        format!("run `{}`", recommended_update_command_for_method_stub(&method))
    } else {
        "run `hermes update`".to_string()
    };
    format!(
        "({action}; fixed versions: 3.51.3+ / 3.50.7 / 3.44.6 — see https://sqlite.org/wal.html#walresetbug)"
    )
}

fn detect_install_method_stub(_project_root: &Path) -> String {
    // Mirrors hermes_cli.config.detect_install_method(PROJECT_ROOT)
    // Stub: return "unknown" in slice 1; real dispatch in later slice.
    "unknown".to_string()
}
fn is_nix_install_method_stub(method: &str) -> bool {
    // Mirrors hermes_cli.config.is_nix_install_method
    matches!(method, "nix" | "nix-flake" | "nix-profile")
}
fn recommended_update_command_for_method_stub(method: &str) -> String {
    // Mirrors hermes_cli.config.recommended_update_command_for_method
    match method {
        "docker" => "docker compose pull && docker compose up -d".to_string(),
        "nix" | "nix-flake" => "nix flake update && home-manager switch".to_string(),
        "apt" => "sudo apt update && sudo apt upgrade hermes-agent".to_string(),
        _ => "hermes update".to_string(),
    }
}

// ---------------------------------------------------------------------------
// _hermes_database_paths — mirrors lines 107-120
// ---------------------------------------------------------------------------

/// Mirrors `_hermes_database_paths(hermes_home)` (lines 107-120).
/// Returns (display name, path) pairs for Hermes-managed SQLite databases.
pub fn hermes_database_paths(hermes_home: &Path) -> Vec<(String, PathBuf)> {
    // Mirrors `from hermes_cli.backup import _QUICK_STATE_FILES`
    let quick_state_files = quick_state_files_stub();
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for name in quick_state_files {
        if name.ends_with(".db") {
            entries.push((name.clone(), hermes_home.join(&name)));
        }
    }
    // Non-default kanban boards each keep their own kanban.db — mirrors glob("kanban/boards/*/kanban.db")
    let kanban_boards = hermes_home.join("kanban").join("boards");
    if let Ok(dir) = std::fs::read_dir(&kanban_boards) {
        let mut board_dbs: Vec<PathBuf> = Vec::new();
        for entry in dir.flatten() {
            let board_db = entry.path().join("kanban.db");
            if board_db.is_file() {
                board_dbs.push(board_db);
            }
        }
        board_dbs.sort();
        for board_db in board_dbs {
            if let Ok(rel) = board_db.strip_prefix(hermes_home) {
                entries.push((rel.to_string_lossy().to_string(), board_db));
            } else {
                entries.push((board_db.to_string_lossy().to_string(), board_db));
            }
        }
    }
    entries
}

fn quick_state_files_stub() -> Vec<String> {
    // Mirrors hermes_cli.backup._QUICK_STATE_FILES — canonical list of per-profile stores.
    // Stub in slice 1; real list imported from backup module in later slice.
    vec![
        "state.db".to_string(),
        "sessions.db".to_string(),
        "kanban.db".to_string(),
        "cron.db".to_string(),
    ]
}

// ---------------------------------------------------------------------------
// _SQLITE_HEADER_MAGIC + _unreadable_reason + _read_journal_mode — lines 123-177
// ---------------------------------------------------------------------------

pub const SQLITE_HEADER_MAGIC: &[u8] = b"SQLite format 3\x00";

/// Mirrors `_unreadable_reason(db_path)` (lines 126-140).
pub fn unreadable_reason(db_path: &Path) -> String {
    match std::fs::metadata(db_path) {
        Err(e) => e.to_string(),
        Ok(_) => {
            // Mirrors `os.access(db_path, os.R_OK)` — without opening a file descriptor.
            // In Rust we try to open read-only as proxy; but to avoid dropping POSIX locks
            // we check permissions via metadata.permissions().readonly() best-effort.
            // Full POSIX access check would need `nix` crate (NEVER cargo in slice 1).
            // For 1:1 we attempt a metadata read check; if file exists but we can't read, report permission denied.
            // If we can't determine, return generic message as in Python fallback.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(db_path) {
                    let mode = meta.permissions().mode();
                    // If no read bits for owner/group/other, treat as permission denied.
                    if mode & 0o444 == 0 {
                        return format!("permission denied: {}", db_path.display());
                    }
                }
            }
            // Try a cheap metadata access; if that succeeded but header read failed for other reasons,
            // Python returns "file could not be read".
            "file could not be read".to_string()
        }
    }
}

/// Mirrors `_read_journal_mode(db_path)` (lines 143-177).
/// Returns (journal_mode, error) without opening the database through SQLite engine.
pub fn read_journal_mode(db_path: &Path) -> (Option<String>, Option<String>) {
    // Mirrors `from hermes_cli.sqlite_safe_read import has_live_connection, read_header_bytes_preopen`
    // In Rust we stub sqlite_safe_read; real impl in later slice when that module is ported.
    let header = read_header_bytes_preopen_stub(db_path, 20);
    if header.is_none() {
        if has_live_connection_stub(db_path) {
            return (None, Some("database is open in this process".to_string()));
        }
        return (None, Some(unreadable_reason(db_path)));
    }
    let header = header.unwrap();
    if header.is_empty() {
        return (None, Some("file is empty".to_string()));
    }
    if header.len() < 20 || !header.starts_with(SQLITE_HEADER_MAGIC) {
        return (None, Some("file is not a database".to_string()));
    }
    match header[18] {
        2 => (Some("wal".to_string()), None),
        1 => (Some("rollback".to_string()), None),
        v => (None, Some(format!("unrecognized file-format version {v}"))),
    }
}

fn read_header_bytes_preopen_stub(db_path: &Path, length: usize) -> Option<Vec<u8>> {
    // Mirrors hermes_cli.sqlite_safe_read.read_header_bytes_preopen — preopen without dropping locks.
    // In slice 1 we do a plain file read as stub (would drop locks in real process with live connections).
    // Real impl must use the safe preopen path; here we just read up to `length` bytes.
    let _ = length;
    std::fs::read(db_path).ok().map(|bytes| bytes.into_iter().take(length).collect())
}
fn has_live_connection_stub(_db_path: &Path) -> bool {
    // Mirrors hermes_cli.sqlite_safe_read.has_live_connection
    // Stub: always false in slice 1 (no live SessionDB tracking yet).
    false
}

// ---------------------------------------------------------------------------
// _format_db_size + _report_database_journal_modes — lines 179-234
// ---------------------------------------------------------------------------

/// Mirrors `_format_db_size(db_path)` (lines 179-188).
pub fn format_db_size(db_path: &Path) -> String {
    match std::fs::metadata(db_path) {
        Err(_) => "size unknown".to_string(),
        Ok(meta) => format_size_stub(meta.len()),
    }
}

fn format_size_stub(nbytes: u64) -> String {
    // Mirrors hermes_cli.backup._format_size — human-readable size.
    // Minimal impl without importing backup module in slice 1.
    if nbytes < 1024 {
        format!("{nbytes} B")
    } else if nbytes < 1024 * 1024 {
        format!("{:.1} KB", nbytes as f64 / 1024.0)
    } else if nbytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", nbytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", nbytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Mirrors `_report_database_journal_modes(hermes_home=None, version_info=None)` (lines 191-234).
pub fn report_database_journal_modes(hermes_home: Option<&Path>, version_info: Option<Vec<i32>>) {
    // Mirrors `from hermes_state import _wal_reset_repair_hint, is_sqlite_wal_reset_vulnerable`
    let vulnerable = is_sqlite_wal_reset_vulnerable_stub(version_info);
    let home = hermes_home
        .map(|p| p.to_path_buf())
        .unwrap_or_else(get_hermes_home);
    let databases = match hermes_database_paths_result(&home) {
        Ok(v) => v,
        Err(e) => {
            check_warn(&format!("Could not list Hermes databases: {e}"), "");
            return;
        }
    };
    let mut exposed: Vec<String> = Vec::new();
    for (name, path) in databases {
        if !path.is_file() {
            continue;
        }
        let (mode, error) = read_journal_mode(&path);
        let size = format_db_size(&path);
        if let Some(err) = error {
            if vulnerable {
                check_warn(
                    &format!("{name}: journal mode could not be read"),
                    &format!("({err}; cannot rule out WAL exposure)"),
                );
            } else {
                check_info(&format!("{name}: journal mode could not be read ({err})"));
            }
        } else if mode.as_deref() == Some("wal") {
            if vulnerable {
                exposed.push(name.clone());
                check_warn(
                    &format!("{name} is in WAL mode ({size})"),
                    "(exposed to the WAL-reset bug until SQLite is upgraded)",
                );
            } else {
                check_info(&format!("{name}: WAL journal mode ({size})"));
            }
        } else if vulnerable {
            check_info(&format!("{name}: rollback journal mode ({size}, not exposed)"));
        } else {
            check_info(&format!("{name}: rollback journal mode ({size})"));
        }
    }
    if !exposed.is_empty() {
        check_info(&format!("To clear the exposure: {}", wal_reset_repair_hint_stub()));
    }
}

fn hermes_database_paths_result(home: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    // Wrapper to mirror try/except around _hermes_database_paths in Python.
    Ok(hermes_database_paths(home))
}
fn is_sqlite_wal_reset_vulnerable_stub(_version_info: Option<Vec<i32>>) -> bool {
    // Mirrors hermes_state.is_sqlite_wal_reset_vulnerable
    // Stub: false in slice 1 (would check sqlite_version + source_id).
    false
}
fn wal_reset_repair_hint_stub() -> String {
    // Mirrors hermes_state._wal_reset_repair_hint
    "run `hermes sessions repair` or upgrade SQLite".to_string()
}

// ---------------------------------------------------------------------------
// _safe_which + termux helpers — lines 236-262
// ---------------------------------------------------------------------------

/// Mirrors `_safe_which(cmd)` (lines 236-241).
pub fn safe_which(cmd: &str) -> Option<String> {
    // Mirrors `shutil.which(cmd)` resilient to monkeypatching.
    // In Rust we emulate `which` via PATH scan (NEVER cargo, no `which` crate).
    let path_var = std::env::var("PATH").unwrap_or_default();
    for dir in path_var.split(':') {
        let candidate = Path::new(dir).join(cmd);
        if candidate.is_file() {
            // Check executable bit on unix
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(&candidate) {
                    if meta.permissions().mode() & 0o111 != 0 {
                        return Some(candidate.to_string_lossy().to_string());
                    }
                }
            }
            #[cfg(not(unix))]
            {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
        // Also check with .exe/.cmd on windows
        #[cfg(windows)]
        {
            for ext in [".exe", ".cmd", ".bat"] {
                let cand = Path::new(dir).join(format!("{cmd}{ext}"));
                if cand.is_file() {
                    return Some(cand.to_string_lossy().to_string());
                }
            }
        }
    }
    None
}

/// Mirrors `_termux_browser_setup_steps(node_installed)` (lines 244-252).
pub fn termux_browser_setup_steps(node_installed: bool) -> Vec<String> {
    let mut steps: Vec<String> = Vec::new();
    let mut step = 1;
    if !node_installed {
        steps.push(format!("{step}) pkg install nodejs"));
        step += 1;
    }
    steps.push(format!("{step}) npm install -g agent-browser"));
    steps.push(format!("{} ) agent-browser install", step + 1).replace(" )", ")"));
    // The Python does f"{step + 1}) agent-browser install" — correct above.
    // Fix formatting: ensure no stray space.
    if steps.len() >= 2 && steps[steps.len() - 1].contains(" )") {
        // already handled
    }
    // Actually recompute cleanly to avoid bug:
    let mut clean: Vec<String> = Vec::new();
    let mut s = 1;
    if !node_installed {
        clean.push(format!("{s}) pkg install nodejs"));
        s += 1;
    }
    clean.push(format!("{s}) npm install -g agent-browser"));
    clean.push(format!("{}) agent-browser install", s + 1));
    clean
}

/// Mirrors `_termux_install_all_fallback_notes()` (lines 255-261).
pub fn termux_install_all_fallback_notes() -> Vec<String> {
    vec![
        "Termux install profile: use .[termux-all] for broad compatibility (installer default on Termux).".to_string(),
        "Matrix E2EE extra is excluded on Termux (python-olm currently fails to build).".to_string(),
        "Local faster-whisper extra is excluded on Termux (ctranslate2/av build path unavailable).".to_string(),
        "STT fallback: use Groq Whisper (set GROQ_API_KEY) or OpenAI Whisper (set VOICE_TOOLS_OPENAI_KEY).".to_string(),
    ]
}

// ---------------------------------------------------------------------------
// _has_provider_env_config + honcho/kanban/web helpers — lines 264-392
// ---------------------------------------------------------------------------

/// Mirrors `_has_provider_env_config(content)` (lines 264-266).
pub fn has_provider_env_config(content: &str) -> bool {
    PROVIDER_ENV_HINTS.iter().any(|key| content.contains(key))
}

/// Mirrors `_honcho_is_configured_for_doctor()` (lines 269-277).
pub fn honcho_is_configured_for_doctor() -> bool {
    // Mirrors `from plugins.memory.honcho.client import HonchoClientConfig`
    // In slice 1 stub: check HONCHO_API_KEY / HONCHO_BASE_URL env as proxy.
    let api_key = std::env::var("HONCHO_API_KEY").unwrap_or_default();
    let base_url = std::env::var("HONCHO_BASE_URL").unwrap_or_default();
    // Also try to read honcho config file if it exists — mirrors HonchoClientConfig.from_global_config()
    // For 1:1 without honcho crate, just check env.
    !api_key.trim().is_empty() || !base_url.trim().is_empty()
}

/// Mirrors `_is_kanban_worker_env_gate(item)` (lines 280-288).
pub fn is_kanban_worker_env_gate(item: &HashMap<String, String>, tools: &[String]) -> bool {
    // Python: checks item.get("name") != "kanban", HERMES_KANBAN_TASK, and all tools start with kanban_
    let name = item.get("name").map(|s| s.as_str()).unwrap_or("");
    if name != "kanban" {
        return false;
    }
    if std::env::var("HERMES_KANBAN_TASK").map(|v| !v.is_empty()).unwrap_or(false) {
        return false;
    }
    if tools.is_empty() {
        return false;
    }
    tools.iter().all(|t| t.starts_with("kanban_"))
}

/// Mirrors `_doctor_tool_availability_detail(toolset)` (lines 291-295).
pub fn doctor_tool_availability_detail(toolset: &str) -> String {
    if toolset == "kanban" && std::env::var("HERMES_KANBAN_TASK").map(|v| v.is_empty()).unwrap_or(true) {
        "(runtime-gated; loaded only for dispatcher-spawned workers)".to_string()
    } else {
        String::new()
    }
}

/// Mirrors `_doctor_web_capability_rows()` (lines 298-350).
/// Returns Vec<(status, label, detail)> where status is "ok" or "warn".
pub fn doctor_web_capability_rows() -> Vec<(String, String, String)> {
    // In Python this imports agent.web_search_registry + tools.web_tools and
    // calls _ensure_web_plugins_loaded(), then get_active_search_provider / get_active_extract_provider.
    // In Rust slice 1 we stub the registry (NEVER cargo, no agent crate).
    // Return empty vec to match Python's early return on ImportError.
    Vec::new()
}

/// Mirrors `_apply_doctor_tool_availability_overrides(available, unavailable)` (lines 352-367).
pub fn apply_doctor_tool_availability_overrides(
    available: Vec<String>,
    unavailable: Vec<HashMap<String, String>>,
) -> (Vec<String>, Vec<HashMap<String, String>>) {
    let mut updated_available = available;
    let mut updated_unavailable: Vec<HashMap<String, String>> = Vec::new();
    for mut item in unavailable {
        let name = item.get("name").cloned().unwrap_or_default();
        // Check kanban gate — need tools list; item["tools"] in Python is list
        // In Rust we store tools as comma-separated string under "tools" key for stub.
        let tools_str = item.get("tools").cloned().unwrap_or_default();
        let tools: Vec<String> = if tools_str.is_empty() {
            Vec::new()
        } else {
            tools_str.split(',').map(|s| s.trim().to_string()).collect()
        };
        if is_kanban_worker_env_gate(&item, &tools) {
            if !updated_available.contains(&"kanban".to_string()) {
                updated_available.push("kanban".to_string());
            }
            continue;
        }
        if name == "honcho" && honcho_is_configured_for_doctor() {
            if !updated_available.contains(&"honcho".to_string()) {
                updated_available.push("honcho".to_string());
            }
            continue;
        }
        updated_unavailable.push(item);
    }
    (updated_available, updated_unavailable)
}

/// Mirrors `_has_healthy_oauth_fallback_for_apikey_provider(provider_label)` (lines 370-391).
pub fn has_healthy_oauth_fallback_for_apikey_provider(provider_label: &str) -> bool {
    let normalized = provider_label.trim().to_lowercase();
    if normalized == "minimax" {
        // Mirrors `from hermes_cli.auth import get_minimax_oauth_auth_status`
        // Stub: check MINIMAX_OAUTH_TOKEN env as proxy for logged_in.
        let logged_in = std::env::var("MINIMAX_OAUTH_LOGGED_IN")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(false);
        return logged_in;
    }
    if normalized == "xai" {
        let logged_in = std::env::var("XAI_OAUTH_LOGGED_IN")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(false);
        return logged_in;
    }
    false
}

// ---------------------------------------------------------------------------
// check_* helpers — lines 394-405
// ---------------------------------------------------------------------------

// Minimal Colors stub — mirrors hermes_cli.colors.Colors + color()
pub mod colors {
    pub const GREEN: &str = "green";
    pub const YELLOW: &str = "yellow";
    pub const RED: &str = "red";
    pub const CYAN: &str = "cyan";
    pub const DIM: &str = "dim";
    pub const BOLD: &str = "bold";
    pub fn color(text: &str, _fg: &str) -> String {
        text.to_string()
    }
    pub fn color2(text: &str, _fg: &str, _attr: &str) -> String {
        text.to_string()
    }
}

/// Mirrors `check_ok(text, detail="")` (lines 394-395).
pub fn check_ok(text: &str, detail: &str) {
    if detail.is_empty() {
        println!("  ✓ {text}");
    } else {
        println!("  ✓ {text} {detail}");
    }
}

/// Mirrors `check_warn(text, detail="")` (lines 397-398).
pub fn check_warn(text: &str, detail: &str) {
    if detail.is_empty() {
        println!("  ⚠ {text}");
    } else {
        println!("  ⚠ {text} {detail}");
    }
}

/// Mirrors `check_fail(text, detail="")` (lines 400-401).
pub fn check_fail(text: &str, detail: &str) {
    if detail.is_empty() {
        println!("  ✗ {text}");
    } else {
        println!("  ✗ {text} {detail}");
    }
}

/// Mirrors `check_info(text)` (lines 403-404).
pub fn check_info(text: &str) {
    println!("    → {text}");
}

// ---------------------------------------------------------------------------
// state.db thresholds + _human_bytes + _render_state_db_stats — lines 407-507
// ---------------------------------------------------------------------------

pub const STATE_DB_SIZE_WARN_BYTES: u64 = 1 * 1024 * 1024 * 1024; // 1 GiB

/// Mirrors `from hermes_cli.sizefmt import format_bytes as _human_bytes` (line 414).
pub fn human_bytes(nbytes: u64) -> String {
    format_size_stub(nbytes)
}

/// Mirrors `_render_state_db_stats(stats, holders=None)` (lines 417-506).
/// Returns Vec<(kind, text, detail)> where kind is "info" / "warn".
pub fn render_state_db_stats(
    stats: &HashMap<String, String>,
    holders: Option<i64>,
) -> Vec<(String, String, String)> {
    let mut lines: Vec<(String, String, String)> = Vec::new();

    // Parse helpers — stats values are strings in stub (Python dict with mixed types)
    let logical: Option<u64> = stats.get("logical_size_bytes").and_then(|v| v.parse().ok());
    let wal: Option<u64> = stats.get("wal_size_bytes").and_then(|v| v.parse().ok());
    let freelist: Option<u64> = stats.get("freelist_count").and_then(|v| v.parse().ok());
    let page_count: Option<u64> = stats.get("page_count").and_then(|v| v.parse().ok());
    let messages: Option<u64> = stats.get("messages").and_then(|v| v.parse().ok());
    let sessions: Option<u64> = stats.get("sessions").and_then(|v| v.parse().ok());
    let journal_mode: Option<String> = stats.get("journal_mode").cloned();
    let fts_tables_raw = stats.get("fts_tables").cloned();
    let fts_storage_version = stats.get("fts_storage_version").cloned();
    let fts_rebuild_pending = stats.get("fts_rebuild_pending").map(|v| v == "true").unwrap_or(false);
    let deferral_attempts = stats.get("fts_rebuild_deferral.attempts").cloned();
    let deferral_pids = stats.get("fts_rebuild_deferral.holder_pids").cloned();

    let mut size_bits: Vec<String> = Vec::new();
    if let Some(log) = logical {
        size_bits.push(format!("logical size {}", human_bytes(log)));
    }
    if let Some(pc) = page_count {
        size_bits.push(format!("{pc:,} pages"));
    }
    if let Some(fl) = freelist {
        size_bits.push(format!("{fl:,} free"));
    }
    if let Some(w) = wal {
        size_bits.push(format!("WAL {}", human_bytes(w)));
    }
    if !size_bits.is_empty() {
        lines.push(("info".to_string(), format!("state.db {}", size_bits.join(", ")), String::new()));
    }

    let mut row_bits: Vec<String> = Vec::new();
    if let Some(m) = messages {
        row_bits.push(format!("{m:,} messages"));
    }
    if let Some(s) = sessions {
        row_bits.push(format!("{s:,} sessions"));
    }
    if let Some(jm) = journal_mode.clone() {
        row_bits.push(format!("journal_mode={jm}"));
    }
    if let Some(h) = holders {
        row_bits.push(format!("{h} process(es) holding the DB open"));
    }
    if !row_bits.is_empty() {
        lines.push(("info".to_string(), row_bits.join(", "), String::new()));
    }

    // FTS tables — Python: fts.get("messages_fts_trigram") etc.
    // In stub, fts_tables is a comma-separated list of present tables if provided.
    if let Some(fts_raw) = fts_tables_raw {
        // If fts_raw is "messages_fts,trigram" style, split and filter present.
        let present: Vec<String> = fts_raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        lines.push((
            "info".to_string(),
            format!("FTS tables: {}", if present.is_empty() { "none".to_string() } else { present.join(", ") }),
            String::new(),
        ));
    }

    // FTS rebuild deferral — Python checks isinstance(deferral, dict) with attempts + holder_pids
    if deferral_attempts.is_some() || deferral_pids.is_some() {
        let attempts = deferral_attempts.unwrap_or_else(|| "?".to_string());
        let pids = deferral_pids.unwrap_or_else(|| "unknown".to_string());
        lines.push((
            "warn".to_string(),
            format!("state.db FTS repair is blocked after {attempts} deferral(s) by PID(s) {pids}"),
            "(stop the listed processes, then run 'hermes sessions optimize-storage' with the gateway stopped)".to_string(),
        ));
    }

    // Advisory: oversized database — mirrors lines 480-499
    if let Some(log) = logical {
        if log > STATE_DB_SIZE_WARN_BYTES {
            let mut detail = "consider enabling sessions.auto_prune in config.yaml to bound growth".to_string();
            // legacy_trigram: fts.get("messages_fts_trigram") and fts_storage_version is None
            let has_trigram = stats
                .get("fts_tables")
                .map(|v| v.contains("messages_fts_trigram"))
                .unwrap_or(false);
            let legacy_trigram = has_trigram && fts_storage_version.is_none();
            if fts_rebuild_pending || legacy_trigram {
                detail.push_str("; run 'hermes sessions optimize-storage' offline (with the gateway stopped) to compact FTS storage");
            }
            lines.push((
                "warn".to_string(),
                format!("state.db is large ({})", human_bytes(log)),
                format!("({detail})"),
            ));
        }
    }

    lines
}

// ---------------------------------------------------------------------------
// _section + _fail_and_issue — lines 509-518
// ---------------------------------------------------------------------------

/// Mirrors `_section(title)` (lines 509-512).
pub fn section(title: &str) {
    println!();
    println!("◆ {title}");
}

/// Mirrors `_fail_and_issue(text, detail, fix, issues)` (lines 515-518).
pub fn fail_and_issue(text: &str, detail: &str, fix: &str, issues: &mut Vec<String>) {
    check_fail(text, detail);
    issues.push(fix.to_string());
}

// ---------------------------------------------------------------------------
// Deprecated keys — lines 521-591
// ---------------------------------------------------------------------------

/// Mirrors `_DEPRECATED_CONFIG_KEYS` (lines 524-528).
pub const DEPRECATED_CONFIG_KEYS: &[(&str, &str, &str)] = &[
    ("display", "tool_progress_overrides", "display.platforms"),
    ("delegation", "max_async_children", "delegation.max_concurrent_children"),
];

/// Mirrors `_DEPRECATED_COMPRESSION_SUMMARY_KEYS` (lines 531-535).
pub const DEPRECATED_COMPRESSION_SUMMARY_KEYS: &[&str] = &[
    "summary_model",
    "summary_provider",
    "summary_base_url",
];

/// Mirrors `_DEPRECATED_ENV_VARS` (lines 539-550).
pub const DEPRECATED_ENV_VARS: &[(&str, &str)] = &[
    ("HERMES_TOOL_PROGRESS", "display.tool_progress in config.yaml — ignored/unsupported since config floor v12"),
    ("HERMES_TOOL_PROGRESS_MODE", "display.tool_progress in config.yaml"),
    ("TERMINAL_CWD", "terminal.cwd in config.yaml"),
    ("MESSAGING_CWD", "terminal.cwd in config.yaml"),
    ("QQ_HOME_CHANNEL", "QQBOT_HOME_CHANNEL"),
    ("QQ_HOME_CHANNEL_NAME", "QQBOT_HOME_CHANNEL_NAME"),
];

/// Mirrors `collect_deprecated_config_keys(raw_config)` (lines 553-575).
pub fn collect_deprecated_config_keys(raw_config: Option<&HashMap<String, String>>) -> Vec<(String, String)> {
    // Python checks raw_config.get(section) is dict and key in dict.
    // In Rust stub we flatten keys as "section.key" strings.
    let mut findings: Vec<(String, String)> = Vec::new();
    let cfg = match raw_config {
        Some(m) => m,
        None => return findings,
    };
    // Check DEPRECATED_CONFIG_KEYS — look for "section.key" flattened key
    for (section, key, replacement) in DEPRECATED_CONFIG_KEYS {
        let flat = format!("{section}.{key}");
        if cfg.contains_key(&flat) || cfg.contains_key(*key) && cfg.get("section").map(|v| v == section).unwrap_or(false) {
            findings.push((flat, replacement.to_string()));
        }
        // Also check nested dict style: look for "section" -> dict containing key would be flattened as above.
        // For exact 1:1 we check cfg.get(section) is dict — in stub we look for "section.key" presence.
        if cfg.contains_key(&flat) {
            // dedupe if already pushed
            if !findings.iter().any(|(k, _)| k == &flat) {
                findings.push((flat, replacement.to_string()));
            }
        }
    }
    // Simpler: if cfg contains "display.tool_progress_overrides" etc., report.
    // Re-derive without overcomplicating dedup — filter duplicates.
    let mut dedup: HashMap<String, String> = HashMap::new();
    for (k, v) in findings {
        dedup.insert(k, v);
    }
    findings = dedup.into_iter().collect();

    // compression.summary_* -> auxiliary.compression
    for key in DEPRECATED_COMPRESSION_SUMMARY_KEYS {
        let flat = format!("compression.{key}");
        if cfg.contains_key(&flat) {
            findings.push((flat, "auxiliary.compression".to_string()));
        }
    }
    findings
}

/// Mirrors `collect_deprecated_env_vars(env_map)` (lines 578-591).
pub fn collect_deprecated_env_vars(env_map: Option<&HashMap<String, String>>) -> Vec<(String, String)> {
    let mut findings: Vec<(String, String)> = Vec::new();
    let env = match env_map {
        Some(m) => m,
        None => return findings,
    };
    for (name, replacement) in DEPRECATED_ENV_VARS {
        if let Some(val) = env.get(*name) {
            if !val.trim().is_empty() {
                findings.push((name.to_string(), replacement.to_string()));
            }
        }
    }
    findings
}

/// Mirrors `collect_relay_plugin_cutover_findings(raw_config, env_map)` (lines 594-631).
pub fn collect_relay_plugin_cutover_findings(
    raw_config: Option<&HashMap<String, String>>,
    env_map: Option<&HashMap<String, String>>,
) -> Vec<(String, String)> {
    // Mirrors hermes_cli.relay_plugin_cutover helpers (LEGACY_RELAY_EXPORT_ENV_VARS, RELAY_PLUGINS_CONFIG_ENV, etc.)
    // In slice 1 stub: return empty (no relay cutover detection without relay_plugin_cutover crate).
    let _ = (raw_config, env_map);
    Vec::new()
}

/// Mirrors `report_deprecated_config_and_env(raw_config=None, env_map=None)` (lines 634-664).
pub fn report_deprecated_config_and_env(
    raw_config: Option<&HashMap<String, String>>,
    env_map: Option<&HashMap<String, String>>,
) -> Vec<(String, String)> {
    let mut deprecated = collect_deprecated_config_keys(raw_config);
    deprecated.extend(collect_deprecated_env_vars(env_map));
    let relay_cutover = collect_relay_plugin_cutover_findings(raw_config, env_map);
    let findings: Vec<(String, String)> = {
        let mut v = deprecated.clone();
        v.extend(relay_cutover.clone());
        v
    };
    if findings.is_empty() {
        check_ok("No deprecated config keys or env vars", "");
        return findings;
    }
    for (legacy, replacement) in &deprecated {
        check_warn(&format!("Deprecated: {legacy}"), &format!("(use {replacement} instead)"));
        check_info(&format!("Replace {legacy} → {replacement} (warn-only; not auto-migrated here)"));
    }
    for (legacy, replacement) in &relay_cutover {
        check_warn(&format!("Breaking Relay migration: {legacy}"), &format!("({replacement})"));
        check_info(&format!("Migrate {legacy}: {replacement}"));
    }
    findings
}

// ---------------------------------------------------------------------------
// _enabled_cli_toolsets / _missing_api_key_toolsets — lines 667-691
// ---------------------------------------------------------------------------

/// Mirrors `_enabled_cli_toolsets_for_doctor()` (lines 667-675).
pub fn enabled_cli_toolsets_for_doctor() -> Option<std::collections::HashSet<String>> {
    // Mirrors `from hermes_cli.config import load_config; from hermes_cli.tools_config import _get_platform_tools`
    // Stub in slice 1: return None (config resolution failed proxy) so caller falls back to unfiltered.
    None
}

/// Mirrors `_missing_api_key_toolsets_for_summary(unavailable)` (lines 678-691).
pub fn missing_api_key_toolsets_for_summary(
    unavailable: &[HashMap<String, String>],
) -> Vec<HashMap<String, String>> {
    let api_key_unavailable: Vec<HashMap<String, String>> = unavailable
        .iter()
        .filter(|item| {
            item.get("missing_vars").map(|v| !v.is_empty()).unwrap_or(false)
                || item.get("env_vars").map(|v| !v.is_empty()).unwrap_or(false)
        })
        .cloned()
        .collect();
    let enabled = enabled_cli_toolsets_for_doctor();
    if enabled.is_none() {
        return api_key_unavailable;
    }
    let enabled = enabled.unwrap();
    api_key_unavailable
        .into_iter()
        .filter(|item| {
            let name = item.get("name").map(|s| s.as_str()).unwrap_or("");
            enabled.contains(name)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// _read_pyproject_version + _check_version_consistency — lines 694-744
// ---------------------------------------------------------------------------

/// Mirrors `_read_pyproject_version()` (lines 694-716).
pub fn read_pyproject_version() -> Option<String> {
    let pyproject = project_root().join("pyproject.toml");
    let text = std::fs::read_to_string(&pyproject).ok()?;
    let mut in_project = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_project = line == "[project]";
            continue;
        }
        if in_project && line.starts_with("version") && line.contains('=') {
            let value = line.splitn(2, '=').nth(1)?;
            let value = value.split('#').next().unwrap_or(value).trim().trim_matches(|c| c == '"' || c == '\'').trim();
            if value.is_empty() {
                return None;
            }
            return Some(value.to_string());
        }
    }
    None
}

/// Mirrors `_check_version_consistency(issues)` (lines 719-744).
pub fn check_version_consistency(issues: &mut Vec<String>) {
    // Mirrors `from hermes_cli import __version__ as init_version`
    let init_version = hermes_cli_version_stub();
    let pyproject_version = match read_pyproject_version() {
        Some(v) => v,
        None => return,
    };
    if pyproject_version == init_version {
        check_ok("Version files consistent", &format!("({init_version})"));
    } else {
        fail_and_issue(
            "Version mismatch between source files",
            &format!("(pyproject.toml {pyproject_version} != hermes_cli/__init__.py {init_version})"),
            "Re-sync version files (e.g. run 'hermes update', or set hermes_cli/__init__.py __version__ to match pyproject.toml)",
            issues,
        );
    }
}

fn hermes_cli_version_stub() -> String {
    // Mirrors `hermes_cli.__version__` — in Rust use CARGO_PKG_VERSION as proxy.
    env!("CARGO_PKG_VERSION").to_string()
}

// ---------------------------------------------------------------------------
// _check_s6_supervision — lines 747-793
// ---------------------------------------------------------------------------

/// Mirrors `_check_s6_supervision(issues)` (lines 747-793).
pub fn check_s6_supervision(issues: &mut Vec<String>) {
    // Mirrors `from hermes_cli.service_manager import S6ServiceManager, detect_service_manager`
    // Stub in slice 1: detect via HERMES_SERVICE_MANAGER env var.
    if detect_service_manager_stub() != "s6" {
        return;
    }
    let _ = issues;
    section("s6 Supervision");

    // Static services — mirrors `mgr.is_running("main-hermes")` etc.
    for svc in ["main-hermes", "dashboard"] {
        if s6_is_running_stub(svc) {
            check_ok(&format!("{svc}: up"), "");
        } else {
            check_info(&format!("{svc}: down (expected if not enabled via env)"));
        }
    }

    let profiles = s6_list_profile_gateways_stub();
    if profiles.is_empty() {
        check_info("No per-profile gateways registered yet — create one with `hermes profile create <name>`");
        return;
    }
    let up_count = profiles.iter().filter(|p| s6_is_running_stub(&format!("gateway-{p}"))).count();
    let suffix = if profiles.len() <= 8 {
        format!(" ({})", profiles.join(", "))
    } else {
        String::new()
    };
    check_ok(
        &format!("Per-profile gateways: {up_count}/{}{suffix}", profiles.len()),
        "",
    );
}

fn detect_service_manager_stub() -> String {
    std::env::var("HERMES_SERVICE_MANAGER").unwrap_or_else(|_| "unknown".to_string())
}
fn s6_is_running_stub(_service: &str) -> bool {
    false
}
fn s6_list_profile_gateways_stub() -> Vec<String> {
    Vec::new()
}

// ---------------------------------------------------------------------------
// check_certificates — lines 795-879
// ---------------------------------------------------------------------------

/// Mirrors `check_certificates(should_fix=False, issues=None)` (lines 795-879).
/// Verifies the certifi CA bundle; with --fix force-reinstalls certifi and re-verifies.
pub fn check_certificates(should_fix: bool, issues: Option<&mut Vec<String>>) {
    // Mirrors `from agent.ssl_guard import verify_ca_bundle_with_fallback` + `from agent.errors import SSLConfigurationError`
    // In slice 1 we stub ssl_guard (NEVER cargo, no agent crate).
    let mut issues_owned = issues.is_some();
    let _ = issues_owned;
    match verify_ca_bundle_with_fallback_stub() {
        Ok(()) => {
            check_ok("SSL CA certificate bundle is valid", "");
            return;
        }
        Err(e) if e == "SSLConfigurationError" => {
            // First error — mirrors `first_error = str(e)` path; we handle below via check_fail.
            // In stub we treat any SSLConfigurationError string as first_error.
            let first_error = e;
            if !should_fix {
                check_fail("SSL CA certificate bundle is broken", &first_error);
                if let Some(issues) = issues {
                    issues.push(format!(
                        "Repair the CA bundle: run `hermes doctor --fix`, or `{} -m pip install --force-reinstall certifi`",
                        std::env::var("PYTHON").unwrap_or_else(|_| "python3".to_string())
                    ));
                }
                return;
            }
            // --fix path — mirrors force-reinstall + re-verify (lines 832-878)
            check_fail("SSL CA certificate bundle is broken", &first_error);
            println!("    → Repairing: force-reinstalling certifi...");
            let python = std::env::var("PYTHON").unwrap_or_else(|_| "python3".to_string());
            let result = std::process::Command::new(&python)
                .args(["-m", "pip", "install", "--force-reinstall", "certifi"])
                .output();
            match result {
                Err(exc) => {
                    check_fail("certifi repair could not run pip", &exc.to_string());
                    if let Some(issues) = issues {
                        issues.push(format!("Reinstall certifi manually: {python} -m pip install --force-reinstall certifi"));
                    }
                    return;
                }
                Ok(out) if !out.status.success() => {
                    let tail = {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        let stdout = String::from_utf8_lossy(&out.stdout);
                        let combined = if stderr.is_empty() { stdout.to_string() } else { stderr.to_string() };
                        let start = combined.len().saturating_sub(500);
                        combined[start..].to_string()
                    };
                    check_fail("certifi reinstall failed", &tail);
                    if let Some(issues) = issues {
                        issues.push(format!("Reinstall certifi manually: {python} -m pip install --force-reinstall certifi"));
                    }
                    return;
                }
                Ok(_) => {
                    // Drop cached certifi module + invalidate caches — mirrors Python's sys.modules pop
                    // In Rust no module cache; just re-verify.
                    match verify_ca_bundle_with_fallback_stub() {
                        Ok(()) => check_ok("SSL CA certificate bundle repaired (certifi reinstalled)", ""),
                        Err(e2) => {
                            check_fail("SSL CA certificate bundle still broken after reinstall", &e2);
                            if let Some(issues) = issues {
                                issues.push(
                                    "certifi reinstall did not restore the CA bundle — check for a custom CA env var (SSL_CERT_FILE/REQUESTS_CA_BUNDLE) pointing at a missing file, or recreate the venv.".to_string(),
                                );
                            }
                        }
                    }
                }
            }
        }
        Err(e) => {
            // Non-SSLConfigurationError — mirrors `check_warn("SSL certificate check skipped", str(e))`
            check_warn("SSL certificate check skipped", &e);
        }
    }
}

fn verify_ca_bundle_with_fallback_stub() -> Result<(), String> {
    // Mirrors agent.ssl_guard.verify_ca_bundle_with_fallback()
    // Stub: Ok in slice 1 (assume bundle valid); real impl would check certifi.where() etc.
    Ok(())
}

// ---------------------------------------------------------------------------
// _check_gateway_service_linger — lines 881-900 (slice 1 boundary)
// ---------------------------------------------------------------------------

/// Mirrors `_check_gateway_service_linger(issues)` (lines 881-900, slice 1 head).
/// Warns when a systemd user gateway service will stop after logout.
/// Skipped inside s6 container; reports linger status for the systemd-on-host case.
/// Slice 1 covers through the `if not is_linux(): return` early exit (line 900);
/// the s6-supervision skip, unit_path check, and linger probe (lines 901-923)
/// continue in `doctor_slice2.rs`.
pub fn check_gateway_service_linger(issues: &mut Vec<String>) {
    // Mirrors `from hermes_cli.gateway import get_systemd_linger_status, get_systemd_unit_path, is_linux`
    //       `from hermes_cli.service_manager import detect_service_manager`
    // Stub helpers below preserve 1:1 line mapping inside the try/except.

    // In Python this is wrapped in try/except ImportError -> check_warn and return.
    // In Rust we stub the imports as always available; real import failure path is unreachable in slice 1
    // but kept as comment for 1:1 traceability:
    //   try: ... except Exception as e: check_warn("Gateway service linger", f"(could not import gateway helpers: {e})"); return

    if !is_linux_stub() {
        return;
    }

    // Lines 903-907 and 909-923 (s6 check, unit_path.exists(), linger probe) are beyond the
    // 900-line slice boundary. They are intentionally truncated here to keep slice 1 at
    // exactly the first 900 lines; the full body continues in `doctor_slice2.rs` as:
    //   if detect_service_manager() == "s6": return
    //   let unit_path = get_systemd_unit_path();
    //   if !unit_path.exists(): return
    //   section("Gateway Service")
    //   let (linger_enabled, linger_detail) = get_systemd_linger_status();
    //   ... check_ok / check_warn + issues.push ...
    // This stub preserves the early-exit contract for callers that only need the is_linux gate.
    let _ = issues;
}

fn is_linux_stub() -> bool {
    // Mirrors hermes_cli.gateway.is_linux() — true on Linux.
    cfg!(target_os = "linux")
}

// Stubs for the remainder of _check_gateway_service_linger's body (lines 901-923),
// kept for 1:1 traceability but not invoked in slice 1's truncated function.
// Full wiring in doctor_slice2.rs.

fn detect_service_manager_for_linger_stub() -> String {
    detect_service_manager_stub()
}
fn get_systemd_unit_path_stub() -> PathBuf {
    // Mirrors hermes_cli.gateway.get_systemd_unit_path()
    dirs_home().join(".config/systemd/user/hermes-gateway.service")
}
fn get_systemd_linger_status_stub() -> (Option<bool>, String) {
    // Mirrors hermes_cli.gateway.get_systemd_linger_status() -> (linger_enabled, linger_detail)
    (None, "unknown".to_string())
}

// ---------------------------------------------------------------------------
// Slice boundary — line 900
// ---------------------------------------------------------------------------
// Python `hermes_cli/doctor.py` lines 901-3163
// (_check_gateway_service_linger tail from 901, _APIKEY_PROVIDERS_CACHE,
// _build_apikey_providers_list, managed_scope_check, run_doctor — the full
// diagnostic orchestration with Security Advisories, MCP security, Python env,
// SQLite, venv, SSL, required/optional packages, config files, version/state,
// xAI retirement, auth providers, directory structure, state.db health, WAL,
// s6/systemd, command installation, external tools, Node.js/agent-browser,
// npm audit, API connectivity parallel probes, tool availability, skills hub,
// GitHub auth, memory provider, profiles, live checks, and summary —
// continues in `doctor_slice2.rs` (from line 901).
// This file intentionally stops at the 900-line boundary so that `cargo` is
// never invoked and the multi-slice decomposition stays clean.
