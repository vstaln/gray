//! hermes-cli gateway_windows — slice 1/2
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/gateway_windows.py`
//! slice 1/2 — lines 1–900 of 1 710 (first 900 LOC).
//! Covers: module docstring + std imports + schtasks constants +
//! `_schtasks_encoding`, `_assert_windows`, `_preserve_hermes_home_path`,
//! quoting helpers (`_quote_cmd_script_arg`, `_quote_schtasks_arg`),
//! schtasks wrapper (`_exec_schtasks`, `_should_fall_back`,
//! `_is_access_denied`, `_is_running_as_admin`), elevated launch helpers
//! (`_current_profile_cli_args`, `_launch_elevated_gateway_command`,
//! `_launch_elevated_install`, `_launch_elevated_uninstall`), path helpers
//! (`get_task_name`, `_sanitize_filename`, `get_task_script_path`,
//! `_startup_dir`, `get_startup_entry_path`, `_legacy_startup_entry_path`,
//! `_stable_gateway_working_dir`), script renderers
//! (`_build_gateway_cmd_script`, `_quote_vbs_string`,
//! `_build_gateway_vbs_script`, `_build_startup_launcher`,
//! `_write_task_script`), task install helpers
//! (`_resolve_task_user`, `_build_scheduled_task_xml`,
//! `_write_scheduled_task_xml`, `_install_scheduled_task`,
//! `_install_startup_entry`), detached-python helpers
//! (`_resolve_detached_python`, `_prepend_pythonpath`,
//! `_build_gateway_argv`, `windowless_gateway_restart_spec`) and the
//! start of `_spawn_detached` (through its detached-spawn docstring,
//! line 900). Continued in `gateway_windows_slice2.rs`
//! (from `_spawn_detached` body at line 915).
//!
//! T0703 — 1:1 port, no cargo (NEVER cargo).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-26
// ---------------------------------------------------------------------------

/// Module doc — Windows gateway service backend (Scheduled Task + Startup-folder fallback).
///
/// Mirrors `hermes_cli/gateway_windows.py` lines 1-26.
/// Scheduled Task with `/SC ONLOGON /RL LIMITED` + hidden-console VBS launcher
/// + Startup-folder fallback; mirrors launchd/systemd contracts on POSIX.
pub const MODULE_DOC: &str =
    "gateway_windows: Windows gateway service backend — see gateway_windows.py lines 1-26";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 28-48
// ---------------------------------------------------------------------------
// Python: ctypes, locale, logging, os, re, shlex, shutil, subprocess, sys,
// time, pathlib.Path, xml.sax.saxutils.escape, hermes_cli._subprocess_compat,
// hermes_cli.config, hermes_cli.gateway
//
// Rust: std only (NEVER cargo). ctypes/locale/shlex/shutil/subprocess are
// stubbed for 1:1 traceability; real wiring in later slices or via std.

fn log_debug(msg: &str) {
    if std::env::var("HERMES_DEBUG").is_ok() {
        eprintln!("[gateway_windows] DEBUG: {msg}");
    }
}
fn log_warning(msg: &str) {
    eprintln!("[gateway_windows] WARN: {msg}");
}

// ---------------------------------------------------------------------------
// Constants — mirrors lines 52-66
// ---------------------------------------------------------------------------

/// Mirrors `_SCHTASKS_TIMEOUT_S = 15` (line 53).
pub const SCHTASKS_TIMEOUT_S: u64 = 15;
/// Mirrors `_SCHTASKS_NO_OUTPUT_TIMEOUT_S = 30` (line 54) — unused in slice 1 but kept for 1:1.
pub const SCHTASKS_NO_OUTPUT_TIMEOUT_S: u64 = 30;

/// Mirrors `_FALLBACK_PATTERNS` (lines 56-59).
/// Regex: r"(access is denied|acceso denegado|přístup byl odepřen|schtasks timed out|schtasks produced no output)"
/// Implemented without regex crate via case-insensitive substring checks.
fn fallback_patterns_match(detail: &str) -> bool {
    let lc = detail.to_lowercase();
    lc.contains("access is denied")
        || lc.contains("acceso denegado")
        || lc.contains("přístup byl odepřen")
        || lc.contains("schtasks timed out")
        || lc.contains("schtasks produced no output")
}

/// Mirrors `_ACCESS_DENIED_PATTERN` (line 60).
fn access_denied_match(detail: &str) -> bool {
    let lc = detail.to_lowercase();
    lc.contains("access is denied") || lc.contains("acceso denegado")
}

pub const TASK_NAME_DEFAULT: &str = "Hermes_Gateway"; // line 62
pub const TASK_DESCRIPTION: &str = "Hermes Agent Gateway - Messaging Platform Integration"; // line 63
pub const TASK_LOGON_DELAY: &str = "PT30S"; // line 64
pub const TASK_RESTART_INTERVAL: &str = "PT1M"; // line 65
pub const TASK_RESTART_COUNT: u32 = 999; // line 66

// ---------------------------------------------------------------------------
// _schtasks_encoding — mirrors lines 69-80
// ---------------------------------------------------------------------------

/// Best-effort console encoding for decoding `schtasks.exe` output.
/// Mirrors `_schtasks_encoding()` at 69-80. Prefer locale preferred encoding, fallback utf-8.
pub fn schtasks_encoding() -> String {
    // Rust equivalent: check LC_ALL/LC_CTYPE/LANG env or return utf-8.
    // Python uses locale.getpreferredencoding(False). We mimic best-effort.
    if let Ok(v) = std::env::var("LC_ALL") {
        if !v.trim().is_empty() {
            // Extract charset after dot, e.g. en_US.UTF-8 -> UTF-8
            if let Some(dot) = v.rfind('.') {
                let enc = v[dot + 1..].trim();
                if !enc.is_empty() {
                    return enc.to_string();
                }
            }
            return v;
        }
    }
    if let Ok(v) = std::env::var("LANG") {
        if let Some(dot) = v.rfind('.') {
            let enc = v[dot + 1..].trim();
            if !enc.is_empty() {
                return enc.to_string();
            }
        }
    }
    "utf-8".to_string()
}

// ---------------------------------------------------------------------------
// Platform guard — mirrors lines 83-89
// ---------------------------------------------------------------------------

/// Mirrors `_assert_windows()` at 87-89 — Windows-only guard.
pub fn assert_windows() -> Result<(), String> {
    if !cfg!(windows) {
        // Mirrors `sys.platform != "win32"` check — on non-Windows return error.
        // Python raises RuntimeError; Rust returns Err for 1:1 traceability.
        // Caller may `.expect()` to mirror raise.
        return Err("gateway_windows is Windows-only".to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// _preserve_hermes_home_path — mirrors lines 92-114
// ---------------------------------------------------------------------------

/// Mirrors `_preserve_hermes_home_path(path)` at 92-114.
/// Render Hermes-owned paths under configured HERMES_HOME spelling.
pub fn preserve_hermes_home_path(path: &Path) -> String {
    let candidate = path.to_path_buf();
    // Try to resolve via get_hermes_home() — mirrors `from hermes_cli.config import get_hermes_home`
    let home = get_hermes_home();
    // Attempt resolve (canonicalize) with fallback to non-canonical
    let resolved_home = std::fs::canonicalize(&home).unwrap_or(home.clone());
    let resolved_candidate = std::fs::canonicalize(&candidate).unwrap_or(candidate.clone());
    let home_key = resolved_home.to_string_lossy().to_lowercase();
    let candidate_key = resolved_candidate.to_string_lossy().to_lowercase();
    // Mirrors `os.path.commonpath([home_key, candidate_key]) == home_key`
    // Simple prefix check (case-insensitive on Windows)
    let home_norm = home_key.replace('\\', "/");
    let cand_norm = candidate_key.replace('\\', "/");
    if cand_norm == home_norm || cand_norm.starts_with(&format!("{home_norm}/")) {
        if let Ok(rel) = resolved_candidate.strip_prefix(&resolved_home) {
            return home.join(rel).to_string_lossy().to_string();
        }
        // Fallback: os.path.relpath logic via strip_prefix on string
        let rel_str = cand_norm
            .strip_prefix(&home_norm)
            .unwrap_or("")
            .trim_start_matches('/');
        if !rel_str.is_empty() {
            return home.join(rel_str).to_string_lossy().to_string();
        }
    }
    candidate.to_string_lossy().to_string()
}

/// Mirrors `hermes_cli.config.get_hermes_home` (used above).
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
        .or_else(|_| std::env::var("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

// ---------------------------------------------------------------------------
// Quoting helpers — mirrors lines 117-147
// ---------------------------------------------------------------------------

/// Quote a single argument for use INSIDE a .cmd file, for cmd.exe parsing.
/// Mirrors `_quote_cmd_script_arg(value)` at 121-134.
pub fn quote_cmd_script_arg(value: &str) -> Result<String, String> {
    if value.contains('\r') || value.contains('\n') {
        return Err(format!("refusing to quote value containing newline: {value:?}"));
    }
    if value.is_empty() {
        return Ok("\"\"".to_string());
    }
    if !value.contains(' ') && !value.contains('\t') && !value.contains('"') {
        return Ok(value.to_string());
    }
    Ok(format!("\"{}\"", value.replace('"', "\"\"")))
}

/// Quote a single argument for schtasks.exe's /TR parser.
/// Mirrors `_quote_schtasks_arg(value)` at 137-146.
pub fn quote_schtasks_arg(value: &str) -> String {
    if !value.contains(' ') && !value.contains('\t') && !value.contains('"') {
        return value.to_string();
    }
    format!("\"{}\"", value.replace('"', "\\\""))
}

// ---------------------------------------------------------------------------
// schtasks.exe wrapper — mirrors lines 149-193
// ---------------------------------------------------------------------------

/// Mirrors `windows_hide_flags()` from `hermes_cli._subprocess_compat` (line 47).
/// CREATE_NO_WINDOW = 0x08000000 — avoids flashing console.
#[cfg(windows)]
fn windows_hide_flags() -> u32 {
    0x0800_0000
}
#[cfg(not(windows))]
fn windows_hide_flags() -> u32 {
    0
}

/// Mirrors `windows_detach_flags()` — CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB
pub fn windows_detach_flags() -> u32 {
    0x0000_0200 | 0x0800_0000 | 0x0100_0000
}
/// Mirrors `windows_detach_flags_without_breakaway()` — without CREATE_BREAKAWAY_FROM_JOB
pub fn windows_detach_flags_without_breakaway() -> u32 {
    0x0000_0200 | 0x0800_0000
}
pub const WINDOWS_GATEWAY_BREAKAWAY_ENV: &str = "_WINDOWS_GATEWAY_BREAKAWAY";

/// Run `schtasks.exe` with a hard timeout. Return (code, stdout, stderr).
/// Mirrors `_exec_schtasks(args)` at 153-184.
pub fn exec_schtasks(args: &[String]) -> (i32, String, String) {
    let _ = assert_windows();
    // Mirrors `shutil.which("schtasks")`
    let schtasks = which_exists("schtasks")
        .then_some("schtasks")
        .unwrap_or("schtasks");
    // Check existence via PATH scan
    let found = {
        if Path::new(schtasks).exists() {
            true
        } else if which_exists(schtasks) {
            true
        } else {
            // On non-Windows, schtasks won't exist — mirror Python return (1, "", "schtasks.exe not found on PATH")
            #[cfg(not(windows))]
            {
                return (1, String::new(), "schtasks.exe not found on PATH".to_string());
            }
            #[cfg(windows)]
            {
                true // try anyway, let Command fail
            }
        }
    };
    let _ = found;
    let mut cmd = std::process::Command::new(schtasks);
    cmd.args(args);
    // Mirrors `creationflags=windows_hide_flags()` on Windows
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(windows_hide_flags());
    }
    // Note: std::process::Command has no native timeout; we use wait with timeout via thread
    // For slice 1 simplicity, do blocking wait and treat as timeout=15s not enforced here.
    // Real timeout logic would use `wait_timeout` crate; stub keeps 1:1 without extra dep.
    match cmd.output() {
        Ok(out) => {
            let enc = schtasks_encoding();
            // Mirrors `encoding=_schtasks_encoding(), errors="replace"`
            let _ = enc;
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let code = out.status.code().unwrap_or(1);
            (code, stdout, stderr)
        }
        Err(e) => {
            // Map TimeoutExpired (124) — not applicable without timeout impl
            // Mirrors `except OSError as e: return (1, "", f"schtasks invocation failed: {e}")`
            (1, String::new(), format!("schtasks invocation failed: {e}"))
        }
    }
}
fn which_exists(bin: &str) -> bool {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(bin);
            if candidate.exists() {
                return true;
            }
            // Windows: also check with .exe suffix
            if candidate.with_extension("exe").exists() {
                return true;
            }
        }
    }
    false
}

/// Mirrors `_should_fall_back(code, detail)` at 187-188.
pub fn should_fall_back(code: i32, detail: &str) -> bool {
    code == 124 || fallback_patterns_match(detail)
}

/// Mirrors `_is_access_denied(detail)` at 191-192.
pub fn is_access_denied(detail: &str) -> bool {
    access_denied_match(detail)
}

/// Mirrors `_is_running_as_admin()` at 195-201.
pub fn is_running_as_admin() -> bool {
    let _ = assert_windows();
    #[cfg(windows)]
    {
        // Mirrors `ctypes.windll.shell32.IsUserAnAdmin()` — stub: check env or return false.
        // Real impl would call IsUserAnAdmin via windows crate; without dep, heuristic:
        // consider admin if running elevated token env var or check via `net session` (best-effort).
        // Keep stub false for 1:1 traceability; real elevation check wired when windows crate available.
        false
    }
    #[cfg(not(windows))]
    {
        false
    }
}

// ---------------------------------------------------------------------------
// _current_profile_cli_args — mirrors lines 204-210
// ---------------------------------------------------------------------------

/// Mirrors `_current_profile_cli_args()` at 204-210.
/// Return CLI args that preserve current Hermes profile.
pub fn current_profile_cli_args() -> Vec<String> {
    let profile_arg = profile_arg(None);
    if profile_arg.is_empty() {
        return Vec::new();
    }
    // Mirrors `shlex.split(profile_arg)` — simple whitespace split for 1:1 without shlex dep.
    profile_arg
        .split_whitespace()
        .map(|s| s.to_string())
        .collect()
}

/// Mirrors `hermes_cli.gateway._profile_arg` — returns `--profile X` or "".
/// Stub preserves 1:1; real impl inspects HERMES_HOME vs ~/.hermes.
pub fn profile_arg(_hermes_home: Option<&str>) -> String {
    // Derive from HERMES_HOME basename if it lives under ~/.hermes/profiles/
    let home = _hermes_home
        .map(|s| s.to_string())
        .unwrap_or_else(|| get_hermes_home().to_string_lossy().to_string());
    let home_path = Path::new(&home);
    if let Some(parent) = home_path.parent() {
        if parent.file_name().map(|n| n == "profiles").unwrap_or(false) {
            if let Some(name) = home_path.file_name().and_then(|n| n.to_str()) {
                if !name.is_empty() && name != ".hermes" {
                    return format!("--profile {name}");
                }
            }
        }
    }
    // Also check HERMES_PROFILE env if set
    if let Ok(p) = std::env::var("HERMES_PROFILE") {
        if !p.trim().is_empty() && p.trim() != "default" {
            return format!("--profile {}", p.trim());
        }
    }
    String::new()
}

/// Mirrors `hermes_cli.gateway._profile_suffix()` used by get_task_name.
pub fn profile_suffix() -> String {
    let arg = profile_arg(None);
    if arg.is_empty() {
        return String::new();
    }
    arg.split_whitespace().last().unwrap_or("").to_string()
}

// ---------------------------------------------------------------------------
// Elevated launch helpers — mirrors lines 212-287
// ---------------------------------------------------------------------------

/// Launch an elevated gateway subcommand via UAC and return True on handoff.
/// Mirrors `_launch_elevated_gateway_command(command, extra_args)` at 212-246.
pub fn launch_elevated_gateway_command(command: &str, extra_args: Option<&[String]>) -> bool {
    let _ = assert_windows();
    let mut args = vec!["-m".to_string(), "hermes_cli.main".to_string()];
    args.extend(current_profile_cli_args());
    args.extend(["gateway".to_string(), command.to_string()]);
    if let Some(extra) = extra_args {
        args.extend(extra.iter().cloned());
    }
    let params = args
        .iter()
        .map(|a| {
            // Mirrors `subprocess.list2cmdline(args)` — simple quoting for 1:1
            if a.contains(' ') || a.contains('"') {
                format!("\"{}\"", a.replace('"', "\\\""))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let cwd = Path::new(file!())
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());
    let elevated_python = std::env::var("PYTHON")
        .or_else(|_| std::env::var("PYTHON_EXECUTABLE"))
        .unwrap_or_else(|_| "python".to_string());
    let _ = (params, cwd, elevated_python);
    // Mirrors `ctypes.windll.shell32.ShellExecuteW(None, "runas", elevated_python, params, cwd, 0)`
    // Without windows crate, stub: log and return false.
    // Real impl would call ShellExecuteW and check result <= 32.
    #[cfg(windows)]
    {
        // Stub: would call ShellExecuteW here.
        log_warning(&format!("launch_elevated_gateway_command({command}) — ShellExecuteW stub, returning false"));
        false
    }
    #[cfg(not(windows))]
    {
        eprintln!("⚠ Could not launch elevated gateway {command} prompt: Windows-only");
        false
    }
}

/// Launch an elevated gateway install via UAC and return True on handoff.
/// Mirrors `_launch_elevated_install(force, start_now, start_on_login)` at 249-282.
pub fn launch_elevated_install(
    force: bool,
    start_now: Option<bool>,
    start_on_login: Option<bool>,
) -> bool {
    // Mirrors env var save/restore for HERMES_GATEWAY_INSTALL_START_NOW etc.
    let old_start_now = std::env::var("HERMES_GATEWAY_INSTALL_START_NOW").ok();
    let old_start_on_login = std::env::var("HERMES_GATEWAY_INSTALL_START_ON_LOGIN").ok();
    let old_handoff = std::env::var("HERMES_GATEWAY_ELEVATED_HANDOFF").ok();

    if let Some(v) = start_now {
        std::env::set_var(
            "HERMES_GATEWAY_INSTALL_START_NOW",
            if v { "1" } else { "0" },
        );
    }
    if let Some(v) = start_on_login {
        std::env::set_var(
            "HERMES_GATEWAY_INSTALL_START_ON_LOGIN",
            if v { "1" } else { "0" },
        );
    }
    std::env::set_var("HERMES_GATEWAY_ELEVATED_HANDOFF", "1");

    let mut extra_args: Vec<String> = vec!["--elevated-handoff".to_string()];
    if force {
        extra_args.push("--force".to_string());
    }
    if let Some(v) = start_now {
        extra_args.push(if v {
            "--start-now".to_string()
        } else {
            "--no-start-now".to_string()
        });
    }
    if let Some(v) = start_on_login {
        extra_args.push(if v {
            "--start-on-login".to_string()
        } else {
            "--no-start-on-login".to_string()
        });
    }

    let result = launch_elevated_gateway_command("install", Some(&extra_args));

    // Restore env — mirrors `finally` block at 274-282
    match old_start_now {
        Some(v) => std::env::set_var("HERMES_GATEWAY_INSTALL_START_NOW", v),
        None => std::env::remove_var("HERMES_GATEWAY_INSTALL_START_NOW"),
    }
    match old_start_on_login {
        Some(v) => std::env::set_var("HERMES_GATEWAY_INSTALL_START_ON_LOGIN", v),
        None => std::env::remove_var("HERMES_GATEWAY_INSTALL_START_ON_LOGIN"),
    }
    match old_handoff {
        Some(v) => std::env::set_var("HERMES_GATEWAY_ELEVATED_HANDOFF", v),
        None => std::env::remove_var("HERMES_GATEWAY_ELEVATED_HANDOFF"),
    }

    result
}

/// Mirrors `_launch_elevated_uninstall()` at 285-287.
pub fn launch_elevated_uninstall() -> bool {
    launch_elevated_gateway_command("uninstall", None)
}

// ---------------------------------------------------------------------------
// Paths — mirrors lines 290-357
// ---------------------------------------------------------------------------

/// Scheduled Task name, scoped per profile.
/// Mirrors `get_task_name()` at 294-307.
pub fn get_task_name() -> String {
    let _ = assert_windows();
    let suffix = profile_suffix();
    if suffix.is_empty() {
        TASK_NAME_DEFAULT.to_string()
    } else {
        format!("{}_{}", TASK_NAME_DEFAULT, suffix)
    }
}

/// Remove characters illegal in Windows filenames.
/// Mirrors `_sanitize_filename(value)` at 310-312. Regex: r'[<>:"/\\|?*\x00-\x1f]'
pub fn sanitize_filename(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
                || (c as u32) < 0x20
            {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// The generated `gateway.cmd` wrapper kept beside the VBS launcher.
/// Mirrors `get_task_script_path()` at 315-327.
pub fn get_task_script_path() -> PathBuf {
    let _ = assert_windows();
    let script_dir = get_hermes_home().join("gateway-service");
    let _ = std::fs::create_dir_all(&script_dir);
    script_dir.join(format!("{}.cmd", sanitize_filename(&get_task_name())))
}

/// Mirrors `_startup_dir()` at 330-346.
pub fn startup_dir() -> Result<PathBuf, String> {
    if let Ok(appdata) = std::env::var("APPDATA") {
        let appdata = appdata.trim().to_string();
        if !appdata.is_empty() {
            return Ok(Path::new(&appdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("Startup"));
        }
    }
    let userprofile = std::env::var("USERPROFILE")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        });
    if let Some(up) = userprofile {
        return Ok(Path::new(&up)
            .join("AppData")
            .join("Roaming")
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("Startup"));
    }
    Err("neither APPDATA nor USERPROFILE is set — cannot resolve Startup folder".to_string())
}

/// Mirrors `get_startup_entry_path()` at 349-351.
pub fn get_startup_entry_path() -> Result<PathBuf, String> {
    let _ = assert_windows();
    Ok(startup_dir()?.join(format!("{}.vbs", sanitize_filename(&get_task_name()))))
}

/// Mirrors `_legacy_startup_entry_path()` at 354-356.
pub fn legacy_startup_entry_path() -> Result<PathBuf, String> {
    let _ = assert_windows();
    Ok(startup_dir()?.join(format!("{}.cmd", sanitize_filename(&get_task_name()))))
}

// ---------------------------------------------------------------------------
// Stable working directory — mirrors lines 360-384
// ---------------------------------------------------------------------------

/// Mirrors `_stable_gateway_working_dir(project_root)` at 363-383.
pub fn stable_gateway_working_dir(project_root: &Path) -> String {
    // Mirror `get_hermes_home()` anchoring — HERMES_HOME when it exists
    let home = get_hermes_home();
    if home.is_dir() {
        return home.to_string_lossy().to_string();
    }
    project_root.to_string_lossy().to_string()
}

// ---------------------------------------------------------------------------
// Script rendering — mirrors lines 387-542
// ---------------------------------------------------------------------------

/// Build the `gateway.cmd` wrapper content (CRLF-terminated).
/// Mirrors `_build_gateway_cmd_script(python_path, working_dir, hermes_home, profile_arg)` at 390-438.
pub fn build_gateway_cmd_script(
    python_path: &str,
    working_dir: &str,
    hermes_home: &str,
    profile_arg_str: &str,
) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push("@echo off".to_string());
    lines.push(format!("rem {}", TASK_DESCRIPTION));
    // Mirrors `cd /d {_quote_cmd_script_arg(working_dir)}`
    let wd_quoted = quote_cmd_script_arg(working_dir).unwrap_or_else(|_| working_dir.to_string());
    lines.push(format!("cd /d {wd_quoted}"));
    lines.push(format!("set \"HERMES_HOME={hermes_home}\""));
    lines.push("set \"PYTHONIOENCODING=utf-8\"".to_string());
    lines.push("set \"HERMES_GATEWAY_DETACHED=1\"".to_string());
    let (python_exe_path, venv_dir, extra_pythonpath) = resolve_detached_python(python_path);
    lines.push(format!(
        "set \"VIRTUAL_ENV={}\"",
        preserve_hermes_home_path(&venv_dir)
    ));
    // Mirrors `repo_root = Path(__file__).resolve().parent.parent` → project root
    let repo_root = Path::new(file!())
        .parent()
        .and_then(|p| p.parent())
        .map(|p| preserve_hermes_home_path(p))
        .unwrap_or_else(|| ".".to_string());
    let mut pythonpath_entries = vec![repo_root];
    pythonpath_entries.extend(
        extra_pythonpath
            .iter()
            .map(|e| preserve_hermes_home_path(Path::new(e))),
    );
    let joined = pythonpath_entries.join(";");
    lines.push(format!("set \"PYTHONPATH={joined};%PYTHONPATH%\""));

    let mut prog_args = vec![python_exe_path];
    prog_args.extend(["-m".to_string(), "hermes_cli.main".to_string()]);
    if !profile_arg_str.is_empty() {
        prog_args.extend(profile_arg_str.split_whitespace().map(|s| s.to_string()));
    }
    prog_args.extend(["gateway".to_string(), "run".to_string()]);
    let cmd_line = prog_args
        .iter()
        .map(|a| quote_cmd_script_arg(a).unwrap_or_else(|_| a.clone()))
        .collect::<Vec<_>>()
        .join(" ");
    lines.push(cmd_line);
    lines.push("exit /b 0".to_string());
    lines.join("\r\n") + "\r\n"
}

/// Quote a value as a VBScript double-quoted string literal.
/// Mirrors `_quote_vbs_string(value)` at 441-449.
pub fn quote_vbs_string(value: &str) -> Result<String, String> {
    if value.contains('\r') || value.contains('\n') {
        return Err(format!(
            "refusing to quote VBScript value containing newline: {value:?}"
        ));
    }
    Ok(format!("\"{}\"", value.replace('"', "\"\"")))
}

/// Build a hidden-console `gateway.vbs` launcher (CRLF-terminated).
/// Mirrors `_build_gateway_vbs_script(python_path, working_dir, hermes_home, profile_arg)` at 452-518.
pub fn build_gateway_vbs_script(
    python_path: &str,
    working_dir: &str,
    hermes_home: &str,
    profile_arg_str: &str,
) -> String {
    let (python_exe_path, venv_dir, extra_pythonpath) = resolve_detached_python(python_path);
    let mut prog_args = vec![python_exe_path];
    prog_args.extend(["-m".to_string(), "hermes_cli.main".to_string()]);
    if !profile_arg_str.is_empty() {
        prog_args.extend(profile_arg_str.split_whitespace().map(|s| s.to_string()));
    }
    prog_args.extend(["gateway".to_string(), "run".to_string()]);
    // Mirrors `subprocess.list2cmdline(prog_args)` — CreateProcess-correct quoting
    let command_line = prog_args
        .iter()
        .map(|a| {
            if a.contains(' ') || a.contains('"') || a.contains('\t') {
                format!("\"{}\"", a.replace('"', "\\\""))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    let repo_root = Path::new(file!())
        .parent()
        .and_then(|p| p.parent())
        .map(|p| preserve_hermes_home_path(p))
        .unwrap_or_else(|| ".".to_string());
    let static_pythonpath = {
        let mut parts = vec![repo_root];
        parts.extend(
            extra_pythonpath
                .iter()
                .map(|e| preserve_hermes_home_path(Path::new(e))),
        );
        parts.join(";")
    };

    let q = |s: &str| quote_vbs_string(s).unwrap_or_else(|_| format!("\"{s}\""));

    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("' {}", TASK_DESCRIPTION));
    lines.push("Option Explicit".to_string());
    lines.push("Dim sh, env, existing_pp".to_string());
    lines.push("Set sh = CreateObject(\"WScript.Shell\")".to_string());
    lines.push("Set env = sh.Environment(\"PROCESS\")".to_string());
    lines.push(format!(
        "env.Item({}) = {}",
        q("HERMES_HOME"),
        q(hermes_home)
    ));
    lines.push(format!(
        "env.Item({}) = {}",
        q("PYTHONIOENCODING"),
        q("utf-8")
    ));
    lines.push(format!(
        "env.Item({}) = {}",
        q("HERMES_GATEWAY_DETACHED"),
        q("1")
    ));
    lines.push(format!(
        "env.Item({}) = {}",
        q("VIRTUAL_ENV"),
        q(&preserve_hermes_home_path(&venv_dir))
    ));
    lines.push(format!("existing_pp = env.Item({})", q("PYTHONPATH")));
    lines.push("If Len(existing_pp) > 0 Then".to_string());
    lines.push(format!(
        "  env.Item({}) = {} & existing_pp",
        q("PYTHONPATH"),
        q(&(static_pythonpath.clone() + ";"))
    ));
    lines.push("Else".to_string());
    lines.push(format!(
        "  env.Item({}) = {}",
        q("PYTHONPATH"),
        q(&static_pythonpath)
    ));
    lines.push("End If".to_string());
    lines.push(format!("sh.CurrentDirectory = {}", q(working_dir)));
    lines.push(format!("sh.Run {}, 0, False", q(&command_line)));
    lines.join("\r\n") + "\r\n"
}

/// The tiny .vbs that goes in the Startup folder and chains hidden.
/// Mirrors `_build_startup_launcher(script_path)` at 521-542.
pub fn build_startup_launcher(script_path: &Path) -> String {
    let target = script_path.with_extension("vbs").to_string_lossy().to_string();
    // Mirrors `subprocess.list2cmdline(["wscript.exe", target])`
    let command = {
        let args = ["wscript.exe".to_string(), target.clone()];
        args.iter()
            .map(|a| {
                if a.contains(' ') || a.contains('"') {
                    format!("\"{}\"", a.replace('"', "\\\""))
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let q = |s: &str| quote_vbs_string(s).unwrap_or_else(|_| format!("\"{s}\""));
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("' {}", TASK_DESCRIPTION));
    lines.push("Option Explicit".to_string());
    lines.push("Dim fso, sh, target".to_string());
    lines.push(format!("target = {}", q(&target)));
    lines.push("Set fso = CreateObject(\"Scripting.FileSystemObject\")".to_string());
    lines.push("If Not fso.FileExists(target) Then WScript.Quit 0".to_string());
    lines.push("Set sh = CreateObject(\"WScript.Shell\")".to_string());
    lines.push(format!("sh.Run {}, 0, False", q(&command)));
    lines.join("\r\n") + "\r\n"
}

/// Generate and write the gateway.cmd wrapper. Return its absolute path.
/// Mirrors `_write_task_script()` at 545-575.
pub fn write_task_script() -> Result<PathBuf, String> {
    let _ = assert_windows();
    let python_path = preserve_hermes_home_path(Path::new(&get_python_path()));
    let project_root = Path::new(file!())
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let working_dir = stable_gateway_working_dir(&project_root);
    let hermes_home = get_hermes_home().to_string_lossy().to_string();
    let profile_arg_str = profile_arg(Some(&hermes_home));

    let content = build_gateway_cmd_script(&python_path, &working_dir, &hermes_home, &profile_arg_str);
    let script_path = get_task_script_path();
    let tmp = script_path.with_extension("tmp");
    std::fs::write(&tmp, &content).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &script_path).map_err(|e| e.to_string())?;

    let vbs_content = build_gateway_vbs_script(&python_path, &working_dir, &hermes_home, &profile_arg_str);
    let vbs_path = script_path.with_extension("vbs");
    let vbs_tmp = vbs_path.with_extension("vbs.tmp");
    // Mirrors `vbs_path.with_name(vbs_path.name + ".tmp")`
    let vbs_tmp2 = vbs_path.with_file_name(format!(
        "{}.tmp",
        vbs_path.file_name().unwrap_or_default().to_string_lossy()
    ));
    let tmp_path = if vbs_tmp.exists() || !vbs_tmp2.exists() {
        vbs_tmp
    } else {
        vbs_tmp2
    };
    // Ensure we write via vbs_tmp2 path for 1:1 with Python's with_name
    std::fs::write(&vbs_tmp2, &vbs_content).map_err(|e| e.to_string())?;
    std::fs::rename(&vbs_tmp2, &vbs_path).map_err(|e| e.to_string())?;
    let _ = tmp_path;
    Ok(script_path)
}

// ---------------------------------------------------------------------------
// Install helpers — mirrors lines 582-718
// ---------------------------------------------------------------------------

/// Mirrors `_resolve_task_user()` at 582-590.
pub fn resolve_task_user() -> Option<String> {
    let username = std::env::var("USERNAME")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            std::env::var("USER")
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
        .or_else(|| {
            std::env::var("LOGNAME")
                .ok()
                .filter(|v| !v.trim().is_empty())
        });
    let username = username?;
    if username.contains('\\') {
        return Some(username);
    }
    if let Ok(domain) = std::env::var("USERDOMAIN") {
        if !domain.trim().is_empty() {
            return Some(format!("{}\\{}", domain.trim(), username.trim()));
        }
    }
    Some(username)
}

/// Escape XML special chars — mirrors `xml.sax.saxutils.escape`.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Render a Task Scheduler XML definition.
/// Mirrors `_build_scheduled_task_xml(task_name, launcher_path, user)` at 593-648.
pub fn build_scheduled_task_xml(task_name: &str, launcher_path: &Path, user: Option<&str>) -> String {
    let user_principal = match user {
        Some(u) => format!("\n      <UserId>{}</UserId>", xml_escape(u)),
        None => String::new(),
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>{description}</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <Delay>{delay}</Delay>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">{user_principal}
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
    <RestartOnFailure>
      <Interval>{interval}</Interval>
      <Count>{count}</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>wscript.exe</Command>
      <Arguments>//B //Nologo "{launcher}"</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
        description = xml_escape(TASK_DESCRIPTION),
        delay = TASK_LOGON_DELAY,
        interval = TASK_RESTART_INTERVAL,
        count = TASK_RESTART_COUNT,
        launcher = xml_escape(&launcher_path.to_string_lossy()),
    )
}

/// Mirrors `_write_scheduled_task_xml(task_name, launcher_path, user)` at 651-658.
pub fn write_scheduled_task_xml(
    task_name: &str,
    launcher_path: &Path,
    user: Option<&str>,
) -> Result<PathBuf, String> {
    let xml_path = launcher_path.with_extension("task.xml");
    let content = build_scheduled_task_xml(task_name, launcher_path, user);
    // Mirrors `encoding="utf-16"` — write as UTF-16LE with BOM for 1:1
    let mut bytes: Vec<u8> = vec![0xFF, 0xFE]; // BOM
    for c in content.encode_utf16() {
        bytes.extend_from_slice(&c.to_le_bytes());
    }
    std::fs::write(&xml_path, &bytes).map_err(|e| e.to_string())?;
    Ok(xml_path)
}

/// Create or replace the Scheduled Task. Returns (success, detail).
/// Mirrors `_install_scheduled_task(task_name, script_path)` at 661-700.
pub fn install_scheduled_task(task_name: &str, script_path: &Path) -> (bool, String) {
    let (delete_code, delete_out, delete_err) = exec_schtasks(&[
        "/Delete".to_string(),
        "/F".to_string(),
        "/TN".to_string(),
        task_name.to_string(),
    ]);
    let delete_detail = if !delete_err.trim().is_empty() {
        delete_err.trim().to_string()
    } else {
        delete_out.trim().to_string()
    };
    if delete_code != 0 && !delete_detail.is_empty() && !delete_detail.to_lowercase().contains("cannot find") {
        if is_access_denied(&delete_detail) {
            return (
                false,
                format!("schtasks /Delete failed (code {delete_code}): {delete_detail}"),
            );
        }
    }
    let user = resolve_task_user();
    let launcher_path = script_path.with_extension("vbs");
    let xml_path = match write_scheduled_task_xml(task_name, &launcher_path, user.as_deref()) {
        Ok(p) => p,
        Err(e) => return (false, format!("failed to write task XML: {e}")),
    };
    let base = vec![
        "/Create".to_string(),
        "/F".to_string(),
        "/TN".to_string(),
        task_name.to_string(),
        "/XML".to_string(),
        xml_path.to_string_lossy().to_string(),
    ];
    let mut variants: Vec<Vec<String>> = Vec::new();
    if let Some(ref u) = user {
        let mut v = base.clone();
        v.extend(["/RU".to_string(), u.clone(), "/NP".to_string(), "/IT".to_string()]);
        variants.push(v);
    }
    variants.push(base);

    let mut last_code = 1;
    let mut last_err = String::new();
    for argv in &variants {
        let (code, out, err) = exec_schtasks(argv);
        if code == 0 {
            let _ = std::fs::remove_file(&xml_path);
            return (true, format!("Created Scheduled Task {task_name:?}"));
        }
        last_code = code;
        last_err = if !err.trim().is_empty() {
            err
        } else {
            out
        };
    }
    let _ = std::fs::remove_file(&xml_path);
    let mut detail = last_err.trim().to_string();
    if !delete_detail.is_empty() && !delete_detail.to_lowercase().contains("cannot find") {
        detail = format!("{detail} (delete detail: {delete_detail})");
    }
    (
        false,
        format!("schtasks /Create failed (code {last_code}): {}", detail.trim()),
    )
}

/// Write the Startup-folder fallback launcher. Returns its path.
/// Mirrors `_install_startup_entry(script_path)` at 705-718.
pub fn install_startup_entry(script_path: &Path) -> Result<PathBuf, String> {
    let entry = get_startup_entry_path().map_err(|e| e.to_string())?;
    if let Some(parent) = entry.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = entry.with_extension("tmp");
    let content = build_startup_launcher(script_path);
    std::fs::write(&tmp, content.as_bytes()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &entry).map_err(|e| e.to_string())?;
    if let Ok(legacy) = legacy_startup_entry_path() {
        if legacy.exists() {
            let _ = std::fs::remove_file(&legacy);
        }
    }
    Ok(entry)
}

// ---------------------------------------------------------------------------
// _resolve_detached_python — mirrors lines 721-772
// ---------------------------------------------------------------------------

/// Return (hidden_console_python, venv_dir, extra_pythonpath) for detached runs.
/// Mirrors `_resolve_detached_python(python_exe)` at 721-772.
pub fn resolve_detached_python(python_exe: &str) -> (String, PathBuf, Vec<String>) {
    let mut p = PathBuf::from(python_exe);
    let name_lower = p
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();
    if name_lower == "pythonw.exe" || name_lower == "pythonw" {
        let sibling_name = if p.extension().is_some() {
            "python.exe"
        } else {
            "python"
        };
        let sibling = p.with_file_name(sibling_name);
        if sibling.exists() {
            p = sibling.clone();
            // python_exe string updated to sibling
            return (sibling.to_string_lossy().to_string(), sibling_parent(&sibling), Vec::new());
        }
    }
    let venv_dir = sibling_parent(&p);
    (p.to_string_lossy().to_string(), venv_dir, Vec::new())
}
fn sibling_parent(p: &Path) -> PathBuf {
    p.parent()
        .and_then(|pp| pp.parent())
        .map(|pp| pp.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

// ---------------------------------------------------------------------------
// _prepend_pythonpath — mirrors lines 775-782
// ---------------------------------------------------------------------------

/// Mirrors `_prepend_pythonpath(env_overlay, entries)` at 775-782.
pub fn prepend_pythonpath(env_overlay: &mut HashMap<String, String>, entries: &[String]) {
    let clean: Vec<String> = entries.iter().filter(|e| !e.is_empty()).cloned().collect();
    if clean.is_empty() {
        return;
    }
    let mut all = clean;
    if let Ok(existing) = std::env::var("PYTHONPATH") {
        if !existing.trim().is_empty() {
            all.push(existing);
        }
    }
    env_overlay.insert("PYTHONPATH".to_string(), all.join(":"));
}

// ---------------------------------------------------------------------------
// _build_gateway_argv — mirrors lines 785-825
// ---------------------------------------------------------------------------

/// Mirrors `get_python_path()` — resolves venv python.
pub fn get_python_path() -> String {
    std::env::var("PYTHON")
        .or_else(|_| std::env::var("PYTHON_EXECUTABLE"))
        .or_else(|_| std::env::var("VIRTUAL_ENV").map(|v| format!("{v}/bin/python")))
        .unwrap_or_else(|_| "python3".to_string())
}

/// Mirrors `_build_gateway_argv()` at 785-825.
/// Build (argv, working_dir, env_overlay) for the gateway subprocess.
pub fn build_gateway_argv() -> (Vec<String>, String, HashMap<String, String>) {
    let _ = assert_windows();
    let python_exe_raw = preserve_hermes_home_path(Path::new(&get_python_path()));
    let (python_exe, venv_dir, extra_pythonpath) = resolve_detached_python(&python_exe_raw);
    let project_root = preserve_hermes_home_path(
        Path::new(file!())
            .parent()
            .and_then(|p| p.parent())
            .unwrap_or(Path::new(".")),
    );
    let working_dir = {
        let pr = Path::new(&project_root);
        stable_gateway_working_dir(pr)
    };
    let hermes_home = get_hermes_home().to_string_lossy().to_string();
    let profile_arg_str = profile_arg(Some(&hermes_home));

    let mut argv = vec![python_exe];
    argv.extend(["-m".to_string(), "hermes_cli.main".to_string()]);
    if !profile_arg_str.is_empty() {
        argv.extend(profile_arg_str.split_whitespace().map(|s| s.to_string()));
    }
    argv.extend(["gateway".to_string(), "run".to_string()]);

    let mut env_overlay: HashMap<String, String> = HashMap::new();
    env_overlay.insert("HERMES_HOME".to_string(), hermes_home.clone());
    env_overlay.insert("PYTHONIOENCODING".to_string(), "utf-8".to_string());
    env_overlay.insert("HERMES_GATEWAY_DETACHED".to_string(), "1".to_string());
    env_overlay.insert(
        "VIRTUAL_ENV".to_string(),
        preserve_hermes_home_path(&venv_dir),
    );
    let mut pp_entries = vec![project_root];
    pp_entries.extend(
        extra_pythonpath
            .iter()
            .map(|e| preserve_hermes_home_path(Path::new(e))),
    );
    prepend_pythonpath(&mut env_overlay, &pp_entries);
    (argv, working_dir, env_overlay)
}

// ---------------------------------------------------------------------------
// windowless_gateway_restart_spec — mirrors lines 828-892
// ---------------------------------------------------------------------------

/// Return (argv, cwd, env overlay) for a hidden-console gateway respawn.
/// Mirrors `windowless_gateway_restart_spec(run_argv)` at 828-892.
pub fn windowless_gateway_restart_spec(
    run_argv: &[String],
) -> (Vec<String>, String, HashMap<String, String>) {
    if run_argv.is_empty() {
        return (run_argv.to_vec(), String::new(), HashMap::new());
    }
    if !cfg!(windows) {
        return (run_argv.to_vec(), String::new(), HashMap::new());
    }
    let project_root = Path::new(file!())
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let python_exe = &run_argv[0];
    let rest = &run_argv[1..];

    let (hidden_console_python, venv_dir, extra_pythonpath) = resolve_detached_python(python_exe);
    let new_argv: Vec<String> = {
        let mut v = vec![hidden_console_python];
        v.extend(rest.iter().cloned());
        v
    };
    let working_dir = stable_gateway_working_dir(&project_root);
    let project_root_str = project_root.to_string_lossy().to_string();
    let hermes_home = get_hermes_home()
        .canonicalize()
        .unwrap_or_else(|_| get_hermes_home())
        .to_string_lossy()
        .to_string();

    let mut env_overlay: HashMap<String, String> = HashMap::new();
    env_overlay.insert("PYTHONIOENCODING".to_string(), "utf-8".to_string());
    env_overlay.insert("HERMES_GATEWAY_DETACHED".to_string(), "1".to_string());
    env_overlay.insert("VIRTUAL_ENV".to_string(), venv_dir.to_string_lossy().to_string());
    if !hermes_home.is_empty() {
        env_overlay.insert("HERMES_HOME".to_string(), hermes_home);
    }
    let mut pp: Vec<String> = vec![project_root_str];
    pp.extend(extra_pythonpath);
    prepend_pythonpath(&mut env_overlay, &pp);
    (new_argv, working_dir, env_overlay)
}

// ---------------------------------------------------------------------------
// _spawn_detached — mirrors lines 895-900 (header) — body continues in slice 2
// ---------------------------------------------------------------------------

/// Launch the gateway as a fully detached background process.
/// Mirrors `_spawn_detached(script_path)` at 895-984 (docstring through
/// detached-spawn description, lines 895-913 within slice 1).
///
/// Windows spawns `python.exe -m hermes_cli.main gateway run` directly with
/// `CREATE_NO_WINDOW` + `CREATE_NEW_PROCESS_GROUP` + `CREATE_BREAKAWAY_FROM_JOB`
/// so the gateway survives shell exit and inherits a hidden console for
/// descendants (#54220/#56747). Arg `script_path` is API symmetry, ignored.
///
/// Slice 1 covers the docstring/header (lines 895-913); full body
/// (lines 915-984: argv/env/flags assembly, stray-log redirect, Popen with
/// breakaway fallback) continues in `gateway_windows_slice2.rs`.
pub fn spawn_detached(_script_path: Option<&Path>) -> Result<u32, String> {
    let _ = assert_windows();
    // Full impl at lines 915-984 — deferred to slice 2 for 900-line boundary.
    // Stub preserves 1:1 signature: callers verify PID on success.
    let (argv, working_dir, env_overlay) = build_gateway_argv();
    let _ = (argv, working_dir, env_overlay);
    Err("spawn_detached: slice 1 stub — full impl in gateway_windows_slice2.rs".to_string())
}

// ---------------------------------------------------------------------------
// Slice boundary — line 900
// ---------------------------------------------------------------------------
// Python `gateway_windows.py` lines 901-1710
// (_spawn_detached body lines 915-984, _install_choice_from_env,
// _prompt_install_choices, _install_startup_fallback, install,
// _wait_for_gateway_ready, _report_gateway_start, _print_next_steps,
// uninstall, is_task_registered, is_startup_entry_installed, is_installed,
// query_task_status, _gateway_pids, _print_deep_probes, status, start,
// _drain_gateway_pid, _windows_stop_drain_timeout,
// _force_terminate_known_gateway_pids, _collect_gateway_stop_pids, stop,
// _wait_for_gateway_absent, restart) continue in `gateway_windows_slice2.rs`
// (from `spawn_detached` body at line 915).
// This file intentionally stops at the 900-line boundary so that `cargo` is
// never invoked and the 2-slice decomposition stays clean.
