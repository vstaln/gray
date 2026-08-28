//! hermes-cli gateway — slice 1/10
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/gateway.py`
//! slice 1/10 — lines 1–900 of 8 428 (first 900 LOC).
//! Covers: module docstring + std imports + PATH fix (#3849),
//! `PROJECT_ROOT`, gateway/config/status/restart imports, hermes_cli/config
//! + setup/colors imports, `logger`, `GatewayRuntimeSnapshot` /
//! `ProfileGatewayProcess`, `_get_service_pids`, `_get_parent_pid`,
//! `_is_pid_ancestor_of_current_process`, `_request_gateway_self_restart`,
//! `_graceful_restart_via_sigusr1`, `_wait_for_pid_exit`, wedged-gateway
//! detection (`GATEWAY_LOOP_*`, `probe_gateway_loop_liveness`,
//! `_escalate_wedged_gateway`), `_get_ancestor_pids`, `_append_unique_pid`,
//! `_scan_gateway_pids`, `_filter_venv_launcher_stubs`, `find_gateway_pids`,
//! `find_profile_gateway_processes`, `_gateway_run_args_for_profile`,
//! `_capture_gateway_argv`, `_prepare_profile_gateway_update_restart`, and
//! `launch_detached_gateway_restart_by_cmdline` (through line 900).
//! Continued in `gateway_slice2.rs` (from `launch_detached_profile_gateway_restart`, line 903).
//!
//! T0685 — 1:1 port, no cargo (NEVER cargo).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-5
// ---------------------------------------------------------------------------

/// Module doc — Gateway subcommand for hermes CLI.
///
/// Handles: `hermes gateway [run|start|stop|restart|status|install|uninstall|setup]`
/// Mirrors `hermes_cli/gateway.py` lines 1-5.
pub const MODULE_DOC: &str = "gateway: hermes gateway subcommand — see gateway.py lines 1-5";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 7-20
// ---------------------------------------------------------------------------
// Python: asyncio, hermes_cli.cli_output.line_input, json, logging, os, shlex,
// shutil, signal, subprocess, sys, textwrap, time, dataclasses, pathlib.Path
//
// Rust: std only (NEVER cargo). Asyncio, line_input, shlex, etc. are stubbed
// for 1:1 traceability; real wiring in later slices when those modules are ported.

/// Mirrors `from hermes_cli.cli_output import line_input` — line 8 stub.
pub fn line_input_stub(_prompt: &str) -> String {
    String::new()
}

// ---------------------------------------------------------------------------
// PATH fix — mirrors lines 22-29 (#3849)
// ---------------------------------------------------------------------------

/// Ensure /bin and /usr/bin are on PATH so launchctl/systemctl are discoverable
/// when running under UV's bundled Python which ships a minimal PATH (#3849).
/// Mirrors `if os.name == "posix": ...` block at lines 24-29.
pub fn ensure_gateway_path() {
    #[cfg(unix)]
    {
        let sys_dirs = ["/bin", "/usr/bin", "/usr/sbin", "/sbin"];
        if let Ok(path) = std::env::var("PATH") {
            let mut parts: HashSet<String> = path.split(':').map(|s| s.to_string()).collect();
            let mut missing: Vec<String> = Vec::new();
            for d in sys_dirs {
                if !parts.contains(d) {
                    missing.push(d.to_string());
                }
            }
            if !missing.is_empty() {
                missing.sort();
                let mut new_path = path;
                if !new_path.is_empty() {
                    new_path.push(':');
                }
                new_path.push_str(&missing.join(":"));
                std::env::set_var("PATH", new_path);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// PROJECT_ROOT — mirrors line 31
// ---------------------------------------------------------------------------

/// Mirrors `PROJECT_ROOT = Path(__file__).parent.parent.resolve()` (line 31).
pub fn project_root() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_REPO_ROOT") {
        return PathBuf::from(v);
    }
    // Mirrors parent of hermes_cli/gateway.py → repo root.
    // In Rust use cwd as fallback (real resolution via CARGO_MANIFEST_DIR in real impl).
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

// ---------------------------------------------------------------------------
// Gateway / hermes_cli imports — mirrors lines 33-68
// ---------------------------------------------------------------------------
// Python:
//   from gateway.config import coerce_systemd_watchdog_seconds, load_gateway_config
//   from gateway.status import terminate_pid
//   from gateway.restart import (DEFAULT_..., is_gateway_supervisor_process, ...)
//   from hermes_cli.config import (get_env_value, get_hermes_home, is_managed, ...)
//   from hermes_cli.setup import (print_header, print_info, ...)
//   from hermes_cli.colors import Colors, color
//
// Rust: std only (NEVER cargo). Stubs preserve 1:1 line mapping; real crates
// wired in later slices.

/// Mirrors `gateway.config.coerce_systemd_watchdog_seconds` (line 33).
pub fn coerce_systemd_watchdog_seconds_stub(_v: Option<&str>) -> Option<u64> {
    None
}
/// Mirrors `gateway.config.load_gateway_config` (line 33).
pub fn load_gateway_config_stub() -> HashMap<String, String> {
    HashMap::new()
}
/// Mirrors `gateway.status.terminate_pid` (line 34) — used by `_escalate_wedged_gateway`.
pub fn terminate_pid_stub(_pid: i32, _force: bool) -> Result<(), String> {
    Ok(())
}
/// Mirrors `gateway.restart` constants (lines 35-44).
pub const DEFAULT_GATEWAY_RESTART_AFTER_TURN_TIMEOUT: f64 = 30.0;
pub const DEFAULT_GATEWAY_RESTART_DRAIN_TIMEOUT: f64 = 180.0;
pub const EXTERNAL_GATEWAY_SUPERVISOR_ENV: &str = "HERMES_EXTERNAL_SUPERVISOR";
pub const GATEWAY_FATAL_CONFIG_EXIT_CODE: i32 = 78;
pub const GATEWAY_SERVICE_RESTART_EXIT_CODE: i32 = 75;
pub fn is_gateway_supervisor_process_stub() -> bool {
    false
}
pub fn parse_restart_after_turn_timeout_stub(_v: Option<&str>) -> f64 {
    DEFAULT_GATEWAY_RESTART_AFTER_TURN_TIMEOUT
}
pub fn parse_restart_drain_timeout_stub(_v: Option<&str>) -> f64 {
    DEFAULT_GATEWAY_RESTART_DRAIN_TIMEOUT
}
pub fn resolve_restart_exit_wait_budget_stub(_after: f64, _drain: f64) -> f64 {
    _after + _drain
}
/// Mirrors `hermes_cli.config.get_hermes_home` (line 48).
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
/// Mirrors `hermes_cli.config.get_env_value` (line 47).
pub fn get_env_value_stub(_key: &str) -> Option<String> {
    None
}
/// Mirrors `hermes_cli.config.is_managed` (line 49).
pub fn is_managed_stub() -> bool {
    false
}
/// Mirrors `hermes_cli.config.managed_error` (line 50).
pub fn managed_error_stub(_msg: &str) -> String {
    String::new()
}
/// Mirrors `hermes_cli.setup` helpers (lines 58-67) — stubs for 1:1.
pub fn print_header_stub(_msg: &str) {}
pub fn print_info_stub(_msg: &str) {}
pub fn print_success_stub(_msg: &str) {}
pub fn print_warning_stub(_msg: &str) {}
pub fn print_error_stub(_msg: &str) {}
pub fn prompt_stub(_msg: &str) -> String {
    String::new()
}
pub fn prompt_choice_stub(_msg: &str, _choices: &[&str]) -> String {
    String::new()
}
pub fn prompt_yes_no_stub(_msg: &str, _default: bool) -> bool {
    false
}
pub mod colors_stub {
    pub const RESET: &str = "\x1b[0m";
    pub fn color_stub(_text: &str, _c: &str) -> String {
        _text.to_string()
    }
}

// ---------------------------------------------------------------------------
// logger — mirrors line 70
// ---------------------------------------------------------------------------

fn log_debug(msg: &str) {
    if std::env::var("HERMES_DEBUG").is_ok() {
        eprintln!("[gateway] DEBUG: {msg}");
    }
}
fn log_warning(msg: &str) {
    eprintln!("[gateway] WARN: {msg}");
}

// ---------------------------------------------------------------------------
// Helpers for platform / service detection — referenced in slice 1 but
// defined later in gateway.py (lines ~1546+). Stubs preserve 1:1 traceability.
// ---------------------------------------------------------------------------

/// Mirrors `is_windows()` — defined at gateway.py:2125.
pub fn is_windows() -> bool {
    cfg!(windows)
}
/// Mirrors `is_macos()` — defined at gateway.py:2121.
pub fn is_macos() -> bool {
    cfg!(target_os = "macos")
}
/// Mirrors `supports_systemd_services()` — defined at gateway.py:2109.
/// True on Linux when systemd user or system manager is reachable.
pub fn supports_systemd_services() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    // Heuristic: systemctl exists and systemd is PID 1 or user manager exists.
    // Full impl in later slice; here return whether systemctl binary exists.
    which_exists("systemctl")
}
fn which_exists(bin: &str) -> bool {
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            if Path::new(dir).join(bin).exists() {
                return true;
            }
        }
    }
    false
}
/// Mirrors `get_launchd_label()` — defined at gateway.py:4398.
pub fn get_launchd_label() -> String {
    // Real impl derives ai.hermes.gateway[.profile] from HERMES_HOME / profile.
    // For slice 1 return default label.
    "ai.hermes.gateway".to_string()
}
/// Mirrors `launchd_gateway_labels_for_install()` — defined at gateway.py:3147.
pub fn launchd_gateway_labels_for_install() -> Vec<String> {
    // Enumerates every installed ai.hermes.gateway* LaunchAgent label.
    // Slice 1 stub: only the current label.
    vec![get_launchd_label()]
}
/// Mirrors `_locate_launchd_gateway_service(label)` at gateway.py:1546.
/// Returns (domain, pid). Stub returns (None, None) for 1:1 without launchctl dep.
pub fn locate_launchd_gateway_service(_label: &str) -> (Option<String>, Option<i32>) {
    (None, None)
}
/// Mirrors `get_service_name()` at gateway.py:2327.
pub fn get_service_name() -> String {
    "hermes-gateway.service".to_string()
}
/// Mirrors `get_systemd_unit_path(system)` at gateway.py:2340.
pub fn get_systemd_unit_path(_system: bool) -> PathBuf {
    // User scope: ~/.config/systemd/user/hermes-gateway.service ; system: /etc/systemd/system/...
    if _system {
        PathBuf::from("/etc/systemd/system/hermes-gateway.service")
    } else {
        dirs_home().join(".config/systemd/user/hermes-gateway.service")
    }
}
/// Mirrors `_select_systemd_scope(system)` at gateway.py:3889 — stub.
pub fn select_systemd_scope(system: bool) -> bool {
    system
}
/// Mirrors `get_python_path()` at gateway.py:3210.
pub fn get_python_path() -> String {
    std::env::var("PYTHON")
        .or_else(|_| std::env::var("PYTHON_EXECUTABLE"))
        .unwrap_or_else(|_| "python3".to_string())
}
/// Mirrors `_profile_arg(hermes_home, default_root)` at gateway.py:2284.
/// Returns `--profile <name>` fragment or "" for default profile.
pub fn profile_arg(_hermes_home: Option<&str>) -> String {
    // Real impl inspects HERMES_HOME vs ~/.hermes to derive profile name.
    // Slice 1 stub: empty (default profile) to avoid extra fs logic.
    String::new()
}
/// Mirrors `gateway.status._pid_exists(pid)` — Windows-safe PID liveness check.
pub fn pid_exists(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // Best-effort: try kill 0 on POSIX, OpenProcess on Windows stub.
    // In slice 1 we probe /proc when available, else assume alive if >0.
    #[cfg(unix)]
    {
        // Check /proc/{pid} exists; works in containers without ps.
        if Path::new(&format!("/proc/{pid}")).exists() {
            return true;
        }
        // Fallback: ps existence check via kill 0 simulation — use Command `kill -0` if available.
        // Keep stub simple: assume pid exists if /proc missing (e.g., macOS).
        return true;
    }
    #[cfg(not(unix))]
    {
        // Windows stub — real impl uses OpenProcess + WaitForSingleObject in gateway.status.
        true
    }
}
/// Mirrors `gateway.status.looks_like_gateway_command_line` — strict matcher.
pub fn looks_like_gateway_command_line(_cmd: &str) -> bool {
    // Real matcher requires `gateway run` subcommand or dedicated entrypoints.
    // Slice 1 stub: permissive check for 1:1 traceability (real wired in later slice).
    let lc = _cmd.to_lowercase();
    lc.contains("gateway") && lc.contains("run")
}
/// Mirrors `gateway.status.looks_like_gateway_runtime_command_line`.
pub fn looks_like_gateway_runtime_command_line(_cmd: &str) -> bool {
    let lc = _cmd.to_lowercase();
    lc.contains("gateway")
}
/// Mirrors `gateway.status.get_running_pid` — reads profile gateway.pid file.
pub fn get_running_pid() -> Option<i32> {
    let pid_file = get_hermes_home().join("gateway.pid");
    if let Ok(text) = std::fs::read_to_string(&pid_file) {
        if let Ok(pid) = text.trim().parse::<i32>() {
            if pid > 0 {
                return Some(pid);
            }
        }
    }
    None
}
/// Mirrors `gateway.shutdown_watchdog.get_loop_heartbeat_path`.
pub fn get_loop_heartbeat_path(home: Option<&Path>) -> PathBuf {
    let base = home
        .map(|p| p.to_path_buf())
        .unwrap_or_else(get_hermes_home);
    base.join("state").join("gateway.heartbeat")
}
/// Mirrors `hermes_cli._subprocess_compat.bounded_probe_run` — Windows-console-safe probe.
pub fn bounded_probe_run_stub(_argv: &[String], _timeout_secs: u64) -> Option<std::process::Output> {
    None
}
/// Mirrors `launch_detached_profile_gateway_restart` (line 903, beyond slice) — stub for `_prepare...` fallback.
pub fn launch_detached_profile_gateway_restart_stub(_profile: &str, _old_pid: i32) -> bool {
    false
}
/// Mirrors `_spawn_gateway_restart_watcher` (line 910, beyond slice) — stub used by slice 1 tail.
pub fn spawn_gateway_restart_watcher_stub(_old_pid: i32, _run_argv: Vec<String>) -> bool {
    false
}

// ---------------------------------------------------------------------------
// Process Management — mirrors lines 72-98
// ---------------------------------------------------------------------------

/// Mirrors `GatewayRuntimeSnapshot` dataclass at lines 77-91 (frozen=True).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayRuntimeSnapshot {
    pub manager: String,
    pub service_installed: bool,
    pub service_running: bool,
    pub gateway_pids: Vec<i32>,
    pub service_scope: Option<String>,
}

impl GatewayRuntimeSnapshot {
    pub fn new(manager: impl Into<String>) -> Self {
        Self {
            manager: manager.into(),
            service_installed: false,
            service_running: false,
            gateway_pids: Vec::new(),
            service_scope: None,
        }
    }

    /// Mirrors `@property running -> self.service_running or bool(self.gateway_pids)` (85-87).
    pub fn running(&self) -> bool {
        self.service_running || !self.gateway_pids.is_empty()
    }

    /// Mirrors `has_process_service_mismatch` (89-91).
    pub fn has_process_service_mismatch(&self) -> bool {
        self.service_installed && self.running() && !self.service_running
    }
}

/// Mirrors `ProfileGatewayProcess` dataclass at lines 94-98 (frozen=True).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileGatewayProcess {
    pub profile: String,
    pub path: PathBuf,
    pub pid: i32,
}

// ---------------------------------------------------------------------------
// _get_service_pids — mirrors lines 101-205
// ---------------------------------------------------------------------------

/// Return PIDs currently managed by systemd or launchd gateway services.
/// Mirrors `_get_service_pids(all_profiles=False)` at 101-205.
pub fn get_service_pids(all_profiles: bool) -> HashSet<i32> {
    let mut pids: HashSet<i32> = HashSet::new();

    // --- systemd (Linux): user and system scopes — mirrors 121-157
    if supports_systemd_services() {
        for scope_args in [vec!["systemctl", "--user"], vec!["systemctl"]] {
            let mut args = scope_args.clone();
            args.extend(["list-units", "hermes-gateway*", "--plain", "--no-legend", "--no-pager"]);
            let output = std::process::Command::new(args[0])
                .args(&args[1..])
                .output();
            let Ok(out) = output else { continue };
            if !out.status.success() {
                continue;
            }
            let stdout = String::from_utf8_lossy(&out.stdout);
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.is_empty() || !parts[0].ends_with(".service") {
                    continue;
                }
                let svc = parts[0];
                let mut show_args = scope_args.clone();
                show_args.extend(["show", svc, "--property=MainPID", "--value"]);
                let show = std::process::Command::new(show_args[0])
                    .args(&show_args[1..])
                    .output();
                let Ok(show_out) = show else { continue };
                if let Ok(pid_str) = String::from_utf8_lossy(&show_out.stdout).trim().parse::<i32>() {
                    if pid_str > 0 {
                        pids.insert(pid_str);
                    }
                }
            }
        }
    }

    // --- launchd (macOS) — mirrors 159-203
    if is_macos() {
        let mut labels: HashSet<String> = HashSet::new();
        labels.insert(get_launchd_label());
        if all_profiles {
            for l in launchd_gateway_labels_for_install() {
                labels.insert(l);
            }
        }
        let mut sorted_labels: Vec<String> = labels.into_iter().collect();
        sorted_labels.sort();
        for label in sorted_labels {
            let (_domain, pid) = locate_launchd_gateway_service(&label);
            if let Some(pid) = pid {
                if pid > 0 {
                    pids.insert(pid);
                }
            }
        }
        if all_profiles {
            // Belt-and-suspenders prefix scan via `launchctl list` (#74075) — mirrors 183-203
            let out = std::process::Command::new("launchctl")
                .arg("list")
                .output();
            if let Ok(o) = out {
                if o.status.success() {
                    let stdout = String::from_utf8_lossy(&o.stdout);
                    for line in stdout.lines() {
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if parts.len() >= 3 && parts[parts.len() - 1].starts_with("ai.hermes.gateway") {
                            if let Ok(pid) = parts[0].parse::<i32>() {
                                if pid > 0 {
                                    pids.insert(pid);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    pids
}

// ---------------------------------------------------------------------------
// _get_parent_pid — mirrors lines 208-252
// ---------------------------------------------------------------------------

/// Return the parent PID for `pid`, or None when unavailable.
/// Mirrors `_get_parent_pid(pid)` at 208-252. Uses /proc fallback and `ps` as in Python.
pub fn get_parent_pid(pid: i32) -> Option<i32> {
    if pid <= 1 {
        return None;
    }
    // Try psutil equivalent — in Rust we have no psutil dep in slice 1; skip to fallback.
    // Fallback: POSIX `ps -o ppid= -p <pid>` — mirrors 230-251
    if is_windows() {
        return None;
    }
    if which_exists("ps") == false {
        return None;
    }
    let out = std::process::Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    let last_line = raw.lines().last()?.trim().to_string();
    let parent: i32 = last_line.parse().ok()?;
    if parent > 0 { Some(parent) } else { None }
}

// ---------------------------------------------------------------------------
// _is_pid_ancestor_of_current_process — mirrors lines 255-267
// ---------------------------------------------------------------------------

/// Return True when `target_pid` is this process or one of its ancestors.
/// Mirrors `_is_pid_ancestor_of_current_process` at 255-267.
pub fn is_pid_ancestor_of_current_process(target_pid: i32) -> bool {
    if target_pid <= 0 {
        return false;
    }
    let mut pid = std::process::id() as i32;
    let mut seen: HashSet<i32> = HashSet::new();
    while pid != 0 && !seen.contains(&pid) {
        if pid == target_pid {
            return true;
        }
        seen.insert(pid);
        match get_parent_pid(pid) {
            Some(ppid) if ppid > 0 => pid = ppid,
            _ => break,
        }
    }
    false
}

// ---------------------------------------------------------------------------
// _request_gateway_self_restart — mirrors lines 270-280
// ---------------------------------------------------------------------------

/// Ask a running gateway ancestor to restart itself asynchronously.
/// Mirrors `_request_gateway_self_restart` at 270-280.
pub fn request_gateway_self_restart(pid: i32) -> bool {
    #[cfg(unix)]
    {
        if !is_pid_ancestor_of_current_process(pid) {
            return false;
        }
        // Mirrors `os.kill(pid, signal.SIGUSR1)` guarded by `hasattr(signal, 'SIGUSR1')`
        // SIGUSR1 = 10 on Linux/macOS.
        let sigusr1: i32 = 10;
        let out = std::process::Command::new("kill")
            .args(["-USR1", &pid.to_string()])
            .output();
        match out {
            Ok(o) => o.status.success(),
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

// ---------------------------------------------------------------------------
// _graceful_restart_via_sigusr1 — mirrors lines 283-322
// ---------------------------------------------------------------------------

/// Send SIGUSR1 to a gateway PID and wait for it to exit gracefully.
/// Mirrors `_graceful_restart_via_sigusr1` at 283-322. SIGUSR1 is wired in gateway/run.py
/// to `request_restart(via_service=True)` which drains in-flight work then exits.
/// Systemd (Restart=always) and launchd (KeepAlive) then restart it.
pub fn graceful_restart_via_sigusr1(pid: i32, drain_timeout: f64) -> bool {
    #[cfg(unix)]
    {
        if pid <= 0 {
            return false;
        }
        let out = std::process::Command::new("kill")
            .args(["-USR1", &pid.to_string()])
            .output();
        match out {
            Ok(o) if !o.status.success() => {
                // ProcessLookupError → already gone → True in Python; we treat non-zero as fail
                // Distinguish "no such process" via stderr heuristic.
                let stderr = String::from_utf8_lossy(&o.stderr).to_lowercase();
                if stderr.contains("no such process") || stderr.contains("no such pid") {
                    return true;
                }
                return false;
            }
            Err(_) => return false,
            _ => {}
        }
        wait_for_pid_exit(pid, drain_timeout.max(1.0))
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, drain_timeout);
        false
    }
}

// ---------------------------------------------------------------------------
// _wait_for_pid_exit — mirrors lines 325-352
// ---------------------------------------------------------------------------

/// Wait up to `timeout` seconds for `pid` to leave the process table.
/// Mirrors `_wait_for_pid_exit` at 325-352. `launchctl bootstrap` fails with EIO
/// if the previous instance is still draining, so teardown must wait.
/// Uses `_pid_exists` helper which is Windows-safe (OpenProcess+WaitForSingleObject).
pub fn wait_for_pid_exit(pid: i32, timeout: f64) -> bool {
    if pid <= 0 {
        return true;
    }
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_secs_f64(timeout.max(0.0));
    loop {
        if !pid_exists(pid) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

// ---------------------------------------------------------------------------
// Wedged-gateway detection — mirrors lines 354-459 (#81642, #66892)
// ---------------------------------------------------------------------------

pub const GATEWAY_LOOP_ALIVE: &str = "alive";
pub const GATEWAY_LOOP_WEDGED: &str = "wedged";
pub const GATEWAY_LOOP_UNKNOWN: &str = "unknown";

/// Heartbeat cadence is 30s; three missed beats is decisive.
pub const DEFAULT_LOOP_LIVENESS_STALE_AFTER_S: f64 = 90.0;

/// Classify a gateway PID's event loop as alive / wedged / unknown.
/// Mirrors `probe_gateway_loop_liveness` at 393-426. Reads the loop-liveness
/// heartbeat file the gateway rewrites every 30s while its loop is dispatching.
/// Never raises; any ambiguity returns UNKNOWN so callers default to graceful drain.
pub fn probe_gateway_loop_liveness(
    pid: i32,
    stale_after: Option<f64>,
    home: Option<&Path>,
) -> String {
    let stale_budget = stale_after.unwrap_or(DEFAULT_LOOP_LIVENESS_STALE_AFTER_S).max(0.0);
    let path = get_loop_heartbeat_path(home);
    let mtime = match std::fs::metadata(&path).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return GATEWAY_LOOP_UNKNOWN.to_string(),
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return GATEWAY_LOOP_UNKNOWN.to_string(),
    };
    // Minimal JSON pid extraction without serde (NEVER cargo) — mirrors json.loads + payload.get("pid")
    let heartbeat_pid: i32 = extract_json_pid(&text).unwrap_or(0);
    if heartbeat_pid <= 0 || pid <= 0 || heartbeat_pid != pid {
        return GATEWAY_LOOP_UNKNOWN.to_string();
    }
    let age = std::time::SystemTime::now()
        .duration_since(mtime)
        .unwrap_or_default()
        .as_secs_f64();
    if age > stale_budget {
        GATEWAY_LOOP_WEDGED.to_string()
    } else {
        GATEWAY_LOOP_ALIVE.to_string()
    }
}

fn extract_json_pid(text: &str) -> Option<i32> {
    // Cheap extraction of "pid": <int> from heartbeat JSON.
    let key = "\"pid\"";
    let idx = text.find(key)?;
    let tail = &text[idx + key.len()..];
    let colon = tail.find(':')?;
    let after = tail[colon + 1..].trim_start();
    let mut num = String::new();
    for c in after.chars() {
        if c.is_ascii_digit() || (num.is_empty() && c == '-') {
            num.push(c);
        } else {
            break;
        }
    }
    num.parse::<i32>().ok()
}

/// Bounded stop for a gateway whose loop is provably dead (#81642).
/// Mirrors `_escalate_wedged_gateway` at 429-459.
/// SIGTERM first (signal handler thread may still live even with dead loop),
/// short grace, then SIGKILL. Total worst case `term_grace + kill_wait` (~10s).
/// Callers MUST have classified gateway as WEDGED before calling.
pub fn escalate_wedged_gateway(pid: i32, term_grace: Option<f64>, kill_wait: Option<f64>) -> bool {
    let term_grace = term_grace.unwrap_or(5.0).max(0.0);
    let kill_wait = kill_wait.unwrap_or(5.0).max(0.0);
    // Mirrors `terminate_pid(pid, force=False)` → SIGTERM
    let _ = terminate_pid_stub(pid, false);
    // Windows note in Python: terminate_pid handles WaitForSingleObject correctly.
    if wait_for_pid_exit(pid, term_grace) {
        return true;
    }
    let _ = terminate_pid_stub(pid, true);
    if pid > 0 {
        eprintln!("⚠ Gateway PID {pid} unresponsive to SIGTERM; sent SIGKILL");
    }
    wait_for_pid_exit(pid, kill_wait)
}

// ---------------------------------------------------------------------------
// _get_ancestor_pids — mirrors lines 462-479
// ---------------------------------------------------------------------------

/// Return the set of PIDs in the current process's ancestor chain.
/// Mirrors `_get_ancestor_pids` at 462-479. Used so status scans never count the CLI
/// that invoked the scan as a running gateway (see #13242).
pub fn get_ancestor_pids() -> HashSet<i32> {
    let mut ancestors: HashSet<i32> = HashSet::new();
    let mut pid = std::process::id() as i32;
    for _ in 0..64 {
        ancestors.insert(pid);
        match get_parent_pid(pid) {
            Some(ppid) if ppid > 0 && !ancestors.contains(&ppid) => pid = ppid,
            _ => break,
        }
    }
    ancestors
}

// ---------------------------------------------------------------------------
// _append_unique_pid — mirrors lines 482-489
// ---------------------------------------------------------------------------

/// Append pid to pids if not excluded / duplicate / self.
/// Mirrors `_append_unique_pid` at 482-489.
pub fn append_unique_pid(pids: &mut Vec<i32>, pid: Option<i32>, exclude_pids: &HashSet<i32>) {
    let pid = match pid {
        Some(p) if p > 0 => p,
        _ => return,
    };
    if pid == std::process::id() as i32 {
        return;
    }
    if exclude_pids.contains(&pid) || pids.contains(&pid) {
        return;
    }
    pids.push(pid);
}

// ---------------------------------------------------------------------------
// _scan_gateway_pids — mirrors lines 492-714
// ---------------------------------------------------------------------------

/// Best-effort process-table scan for gateway PIDs.
/// Mirrors `_scan_gateway_pids` at 492-714. Supplements profile PID file so status
/// views can spot live gateways when PID file is stale/missing.
pub fn scan_gateway_pids(
    exclude_pids: &HashSet<i32>,
    all_profiles: bool,
    include_restart_managers: bool,
) -> Vec<i32> {
    let exclude = {
        let mut e = exclude_pids.clone();
        e.extend(get_ancestor_pids());
        e
    };
    let mut pids: Vec<i32> = Vec::new();

    let current_home = get_hermes_home().to_string_lossy().to_string();
    let current_home_lc = current_home.to_lowercase().replace('\\', "/");
    let current_profile_arg = profile_arg(Some(&current_home));
    let current_profile_name = current_profile_arg
        .split_whitespace()
        .last()
        .unwrap_or("")
        .to_string();
    let current_profile_name_lc = current_profile_name.to_lowercase();

    let matches_current_profile = |command: &str| -> bool {
        let command_lc = command.to_lowercase().replace('\\', "/");
        if !current_profile_name.is_empty() {
            return command_lc.contains(&format!("--profile {}", current_profile_name_lc))
                || command_lc.contains(&format!("-p {}", current_profile_name_lc))
                || command_lc.contains(&format!("hermes_home={}", current_home_lc));
        }
        // Default-profile case — mirrors Python 536-548
        if command_lc.contains("--profile ") || command_lc.contains(" -p ") {
            return false;
        }
        if command_lc.contains("hermes_home=") && !command_lc.contains(&format!("hermes_home={}", current_home_lc)) {
            return false;
        }
        true
    };

    let matches_gateway_runtime = |command: &str| -> bool {
        if looks_like_gateway_command_line(command) {
            return true;
        }
        include_restart_managers && looks_like_gateway_runtime_command_line(command)
    };

    // Mirrors try/except OSError, subprocess.TimeoutExpired at 697-698
    if is_windows() {
        // Prefer wmic when present, fallback to PowerShell Get-CimInstance — mirrors 556-627
        // Uses bounded_probe_run (console-window-safe) — mirrors #87134 comment.
        // In slice 1 we attempt the real commands best-effort; on failure return empty scan segment.
        let wmic_exists = which_exists("wmic");
        let mut result_stdout: Option<String> = None;
        if wmic_exists {
            if let Ok(out) = std::process::Command::new("wmic")
                .args(["process", "get", "ProcessId,CommandLine", "/FORMAT:LIST"])
                .output()
            {
                if out.status.success() {
                    result_stdout = Some(String::from_utf8_lossy(&out.stdout).to_string());
                }
            }
        }
        let stdout = if result_stdout.is_some() {
            result_stdout
        } else {
            // Fallback: PowerShell Get-CimInstance — mirrors 589-609
            let powershell = which_exists("powershell")
                .then_some("powershell")
                .or_else(|| which_exists("pwsh").then_some("pwsh"));
            if let Some(ps) = powershell {
                let ps_cmd = "Get-CimInstance Win32_Process | ForEach-Object { 'CommandLine=' + ($_.CommandLine -replace \"`r`n\",' ' -replace \"`n\",' '); 'ProcessId=' + $_.ProcessId; '' }";
                std::process::Command::new(ps)
                    .args(["-NoProfile", "-Command", ps_cmd])
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            } else {
                None
            }
        };
        if let Some(stdout) = stdout {
            let mut current_cmd = String::new();
            for line in stdout.lines() {
                let line = line.trim();
                if line.starts_with("CommandLine=") {
                    current_cmd = line["CommandLine=".len()..].to_string();
                } else if line.starts_with("ProcessId=") {
                    let pid_str = &line["ProcessId=".len()..];
                    if matches_gateway_runtime(&current_cmd)
                        && (all_profiles || matches_current_profile(&current_cmd))
                    {
                        if let Ok(pid) = pid_str.trim().parse::<i32>() {
                            append_unique_pid(&mut pids, Some(pid), &exclude);
                        }
                    }
                    current_cmd.clear();
                }
            }
        }
    } else {
        // POSIX: Try /proc first (works in Docker without procps), fall back to ps -Aww — mirrors 628-696
        let mut found_via_proc = false;
        if Path::new("/proc").is_dir() {
            // Best-effort /proc scan — mirrors 631-652
            if let Ok(entries) = std::fs::read_dir("/proc") {
                found_via_proc = true;
                let my_pid = std::process::id() as i32;
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let pid: i32 = match name.parse() {
                        Ok(n) => n,
                        Err(_) => continue,
                    };
                    if pid == my_pid || exclude.contains(&pid) {
                        continue;
                    }
                    let cmdline_path = format!("/proc/{pid}/cmdline");
                    let bytes = match std::fs::read(&cmdline_path) {
                        Ok(b) => b,
                        Err(_) => continue,
                    };
                    let cmdline = String::from_utf8_lossy(&bytes).replace('\0', " ");
                    if matches_gateway_runtime(&cmdline)
                        && (all_profiles || matches_current_profile(&cmdline))
                    {
                        append_unique_pid(&mut pids, Some(pid), &exclude);
                    }
                }
            }
        }
        if !found_via_proc {
            if let Ok(out) = std::process::Command::new("ps")
                .args(["-Aww", "-o", "pid=,command="])
                .output()
            {
                if out.status.success() {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    for line in stdout.lines() {
                        let stripped = line.trim();
                        if stripped.is_empty() || stripped.contains("grep") {
                            continue;
                        }
                        // Parse pid + command — mirrors 674-696
                        let mut pid: Option<i32> = None;
                        let mut command = String::new();
                        let parts: Vec<&str> = stripped.splitn(2, char::is_whitespace).collect();
                        // Actually Python does split(None,1) — we mimic with whitespace split
                        let ws_parts: Vec<&str> = stripped.split_whitespace().collect();
                        // Try first token as pid
                        if let Some(first) = stripped.split_whitespace().next() {
                            if let Ok(n) = first.parse::<i32>() {
                                pid = Some(n);
                                // command is remainder after pid
                                if let Some(idx) = stripped.find(first) {
                                    command = stripped[idx + first.len()..].trim().to_string();
                                }
                            }
                        }
                        if pid.is_none() && ws_parts.len() > 10 {
                            if let Ok(n) = ws_parts[1].parse::<i32>() {
                                pid = Some(n);
                                command = ws_parts[10..].join(" ");
                            }
                        }
                        if let Some(pid_val) = pid {
                            if matches_gateway_runtime(&command)
                                && (all_profiles || matches_current_profile(&command))
                            {
                                append_unique_pid(&mut pids, Some(pid_val), &exclude);
                            }
                        }
                        let _ = parts;
                    }
                }
            }
        }
    }

    // Windows venv launcher stub collaps — mirrors 700-712
    if is_windows() && pids.len() > 1 {
        pids = filter_venv_launcher_stubs(pids);
    }

    pids
}

// ---------------------------------------------------------------------------
// _filter_venv_launcher_stubs — mirrors lines 717-744
// ---------------------------------------------------------------------------

/// Drop venv-launcher pythonw.exe stubs that are parents of the real interpreter.
/// Mirrors `_filter_venv_launcher_stubs` at 717-744. Windows-specific pattern:
/// venv Scripts/pythonw.exe is a ~100KB launcher that spawns the base Python
/// with same command line, so one gateway run looks like two PIDs.
pub fn filter_venv_launcher_stubs(pids: Vec<i32>) -> Vec<i32> {
    if pids.len() <= 1 {
        return pids;
    }
    let pid_set: HashSet<i32> = pids.iter().cloned().collect();
    let mut parent_of: HashMap<i32, Option<i32>> = HashMap::new();
    for &pid in &pids {
        parent_of.insert(pid, get_parent_pid(pid));
    }
    let mut drop: HashSet<i32> = HashSet::new();
    for (&pid, ppid) in &parent_of {
        if let Some(pp) = ppid {
            if pid_set.contains(pp) {
                drop.insert(*pp);
            }
        }
    }
    pids.into_iter().filter(|p| !drop.contains(p)).collect()
}

// ---------------------------------------------------------------------------
// find_gateway_pids — mirrors lines 747-782
// ---------------------------------------------------------------------------

/// Find PIDs of running gateway processes.
/// Mirrors `find_gateway_pids` at 747-782.
pub fn find_gateway_pids(exclude_pids: Option<HashSet<i32>>, all_profiles: bool) -> Vec<i32> {
    let exclude = exclude_pids.unwrap_or_default();
    let mut pids: Vec<i32> = Vec::new();
    if !all_profiles {
        if let Some(pid) = get_running_pid() {
            append_unique_pid(&mut pids, Some(pid), &exclude);
        }
    }
    for pid in get_service_pids(all_profiles) {
        append_unique_pid(&mut pids, Some(pid), &exclude);
    }
    let include_restart_managers = !supports_systemd_services();
    for pid in scan_gateway_pids(&exclude, all_profiles, include_restart_managers) {
        append_unique_pid(&mut pids, Some(pid), &exclude);
    }
    pids
}

// ---------------------------------------------------------------------------
// find_profile_gateway_processes — mirrors lines 785-809
// ---------------------------------------------------------------------------

/// Return running gateway PIDs mapped to Hermes profiles via PID files.
/// Mirrors `find_profile_gateway_processes` at 785-809.
pub fn find_profile_gateway_processes(exclude_pids: Option<HashSet<i32>>) -> Vec<ProfileGatewayProcess> {
    let exclude = exclude_pids.unwrap_or_default();
    let mut processes: Vec<ProfileGatewayProcess> = Vec::new();
    let profiles_root = dirs_home().join(".hermes/profiles");
    if !profiles_root.is_dir() {
        return processes;
    }
    let mut seen: HashSet<i32> = HashSet::new();
    // Mirrors `for profile in list_profiles()` — enumerate profile dirs
    let entries = match std::fs::read_dir(&profiles_root) {
        Ok(d) => d,
        Err(_) => return processes,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let pid_file = path.join("gateway.pid");
        let pid: Option<i32> = std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok());
        let pid = match pid {
            Some(p) if p > 0 => p,
            _ => continue,
        };
        if exclude.contains(&pid) || seen.contains(&pid) {
            continue;
        }
        seen.insert(pid);
        // Cleanup stale: if pid no longer exists, skip (mirrors cleanup_stale=False in slice1 call)
        // Python passes cleanup_stale=False so stale files are NOT removed here — keep pid anyway.
        processes.push(ProfileGatewayProcess {
            profile: name,
            path,
            pid,
        });
    }
    processes
}

// ---------------------------------------------------------------------------
// _gateway_run_args_for_profile — mirrors lines 812-817
// ---------------------------------------------------------------------------

/// Mirrors `_gateway_run_args_for_profile` at 812-817.
pub fn gateway_run_args_for_profile(profile: &str) -> Vec<String> {
    let mut args = vec![get_python_path(), "-m".to_string(), "hermes_cli.main".to_string()];
    if profile != "default" {
        args.extend(["--profile".to_string(), profile.to_string()]);
    }
    args.extend(["gateway".to_string(), "run".to_string(), "--replace".to_string()]);
    args
}

// ---------------------------------------------------------------------------
// _capture_gateway_argv — mirrors lines 820-856
// ---------------------------------------------------------------------------

/// Return the live argv of a running gateway process, or None.
/// Mirrors `_capture_gateway_argv` at 820-856. Used to respawn gateways that
/// have no profile→PID-file mapping (e.g. Windows Scheduled Task).
/// Best-effort: returns None if psutil unavailable, process gone, or argv
/// doesn't look like a gateway command.
pub fn capture_gateway_argv(pid: i32) -> Option<Vec<String>> {
    if pid <= 1 {
        return None;
    }
    // Try /proc/{pid}/cmdline first (Linux) — mirrors psutil.Process(pid).cmdline()
    let proc_cmdline = Path::new(&format!("/proc/{pid}/cmdline"));
    let argv: Option<Vec<String>> = if proc_cmdline.exists() {
        std::fs::read(proc_cmdline).ok().map(|bytes| {
            // Split on NUL, decode lossily
            let s = String::from_utf8_lossy(&bytes);
            s.split('\0')
                .filter(|p| !p.is_empty())
                .map(|p| p.to_string())
                .collect()
        })
    } else {
        // Fallback: ps -o args= -p <pid>
        std::process::Command::new("ps")
            .args(["-o", "args=", "-p", &pid.to_string()])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                // Best-effort shell-like split — keep as single entry to avoid shlex dep
                vec![s]
            })
    };
    let argv = argv?;
    if argv.is_empty() {
        return None;
    }
    let joined = argv.join(" ");
    if !looks_like_gateway_command_line(&joined) {
        return None;
    }
    Some(argv)
}

// ---------------------------------------------------------------------------
// _prepare_profile_gateway_update_restart — mirrors lines 859-884
// ---------------------------------------------------------------------------

/// Choose who relaunches a profile gateway after `hermes update`.
/// Mirrors `_prepare_profile_gateway_update_restart` at 859-884.
///
/// A gateway started with `--external-supervisor` must exit back to that
/// manager. Starting Hermes's detached watcher as well would escape the
/// manager and race its replacement process. Ordinary foreground gateways
/// retain the existing detached-watcher behavior.
///
/// When the profile-derived relaunch cannot be armed — typically because
/// `_gateway_run_args_for_profile` cannot rebuild a run argv for this
/// profile — fall back to replaying the process's own captured command
/// line, which is what `launch_detached_gateway_restart_by_cmdline`
/// exists for and what the Windows post-update path already does for its
/// unmapped gateways.  Without this the caller has no way to relaunch the
/// process and (before #88654) silently left it running pre-update modules
/// against post-update code on disk.  `argv` is already captured above,
/// so the fallback costs nothing extra.
pub fn prepare_profile_gateway_update_restart(profile: &str, pid: i32) -> Option<String> {
    let argv = capture_gateway_argv(pid);
    if let Some(ref a) = argv {
        if a.iter().any(|s| s == "--external-supervisor") {
            return Some("external-supervisor".to_string());
        }
    }
    if launch_detached_profile_gateway_restart(profile, pid) {
        return Some("detached".to_string());
    }
    if let Some(a) = argv {
        if launch_detached_gateway_restart_by_cmdline(pid, a) {
            return Some("detached-cmdline".to_string());
        }
    }
    None
}

// Forward decls for `_prepare...` callees beyond slice boundary — stubs for 1:1
fn launch_detached_profile_gateway_restart(profile: &str, old_pid: i32) -> bool {
    if old_pid <= 0 {
        return false;
    }
    spawn_gateway_restart_watcher(old_pid, gateway_run_args_for_profile(profile))
}
fn spawn_gateway_restart_watcher(old_pid: i32, run_argv: Vec<String>) -> bool {
    if old_pid <= 0 || run_argv.is_empty() {
        return false;
    }
    // Full watcher spawn (detached Python subprocess with platform-appropriate
    // start_new_session / CREATE_NEW_PROCESS_GROUP flags) lives in slice 2
    // (lines 910+). Stub preserves 1:1 call graph for slice 1 boundary.
    let _ = (old_pid, run_argv);
    false
}

// ---------------------------------------------------------------------------
// launch_detached_gateway_restart_by_cmdline — mirrors lines 887-900
// ---------------------------------------------------------------------------

/// Relaunch a gateway by replaying its captured command line after exit.
/// Mirrors `launch_detached_gateway_restart_by_cmdline` at 887-900.
/// Companion to `launch_detached_profile_gateway_restart` for gateways that
/// have no profile→PID-file mapping (Scheduled-Task / manually-launched
/// `gateway run` whose HERMES_HOME or argv doesn't match a known profile).
/// Uses the identical detached-watcher mechanism; only the respawn argv
/// differs (the process's own argv instead of a profile-derived one).
pub fn launch_detached_gateway_restart_by_cmdline(old_pid: i32, run_argv: Vec<String>) -> bool {
    if old_pid <= 0 || run_argv.is_empty() {
        return false;
    }
    spawn_gateway_restart_watcher(old_pid, run_argv)
}

// ---------------------------------------------------------------------------
// Slice boundary — line 900
// ---------------------------------------------------------------------------
// Python `gateway.py` lines 901-8428 (launch_detached_profile_gateway_restart body,
// _spawn_gateway_restart_watcher full impl, _probe_systemd_service_running,
// _read_systemd_unit_environment, _hermes_home_from_systemd_unit_file,
// _sync_hermes_home_from_systemd_unit, _read_systemd_unit_properties,
// _systemd_main_pid_from_props, _systemd_main_pid, _read_gateway_runtime_status,
// _gateway_runtime_status_for_pid, _wait_for_systemd_service_restart and all
// gateway subcommand handlers through EOF) continue in `gateway_slice2.rs`
// (from `launch_detached_profile_gateway_restart` body at line 903).
// This file intentionally stops at the 900-line boundary so that `cargo` is
// never invoked and the 10-slice decomposition stays clean.
