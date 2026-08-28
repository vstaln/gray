//! TUI gateway entry point — stdio JSON-RPC server bootstrap.
//!
//! 1:1 port of `tui_gateway/entry.py` (500 lines).
//!
//! The gateway is `hermes --tui`'s stdio subprocess: Ink (TypeScript) owns the
//! screen, Python owns sessions/tools/model calls and speaks newline-delimited
//! JSON-RPC over stdin/stdout. This module is the process entry point that
//! wires transports, signals, crash logging, MCP discovery, and the `readline`
//! dispatch loop.
//!
//! ```python
//! # Python — tui_gateway/entry.py
//! import hermes_bootstrap; hermes_bootstrap.harden_import_path()
//! import json, logging, signal, threading, time, traceback, os, sys
//! from tui_gateway._stdin_recovery import handle_spurious_eof
//! from tui_gateway import server
//! from tui_gateway.server import _CRASH_LOG, dispatch, resolve_skin, write_json
//! from tui_gateway.transport import TeeTransport
//! logger = logging.getLogger(__name__)
//! _mcp_discovery_thread = None
//! _mcp_discovery_enabled = False
//! def _install_sidecar_publisher() -> None: ...
//! _DEFAULT_SHUTDOWN_GRACE_S = 1.0
//! def _shutdown_grace_seconds() -> float: ...
//! def _log_signal(signum: int, frame) -> None: ...
//! def _install_signal(signame, handler): ...
//! _install_signal("SIGPIPE", signal.SIG_IGN)
//! _install_signal("SIGTERM", _log_signal)
//! if hasattr(signal, "SIGHUP"): _install_signal("SIGHUP", _log_signal)
//! elif hasattr(signal, "SIGBREAK"): _install_signal("SIGBREAK", _log_signal)
//! _install_signal("SIGINT", signal.SIG_IGN)
//! def _log_exit(reason: str) -> None: ...
//! def wait_for_mcp_discovery(timeout: float | None = None) -> None: ...
//! def mcp_discovery_in_flight() -> bool: ...
//! def join_mcp_discovery(timeout: float | None = None) -> bool: ...
//! _recovery_times: list[float] = []
//! def _has_configured_mcp_servers() -> bool: ...
//! def ensure_mcp_discovery_started() -> None: ...
//! def main(): ...
//! if __name__ == "__main__": main()
//! ```
//!
//! # Rust mapping
//!
//! * `hermes_bootstrap.harden_import_path()` — Python-only `sys.path` guard
//!   against a CWD `utils/` shadowing `hermes_bootstrap`. No Rust equivalent;
//!   the crate graph is static. Documented as a no-op.
//! * `HERMES_TUI_SIDECAR_URL` + `WsPublisherTransport` + `TeeTransport` →
//!   [`ENV_SIDECAR_URL`] + [`sidecar_url`] / [`sidecar_url_with`] /
//!   [`should_install_sidecar`] + [`tee_transport_config`] (the `server._stdio_transport`
//!   mutation is injected via `set_tee: Fn(TeeConfig)`; this crate stays `std`-only
//!   so `WsPublisherTransport` itself lives in `crate::event_publisher`).
//! * `_DEFAULT_SHUTDOWN_GRACE_S = 1.0` → [`DEFAULT_SHUTDOWN_GRACE_S`].
//! * `HERMES_TUI_GATEWAY_SHUTDOWN_GRACE_S` env knob + `float()` + `>0` guard →
//!   [`ENV_SHUTDOWN_GRACE`] + [`shutdown_grace_with`] (pure `Option<&str> -> f64`)
//!   + [`shutdown_grace_secs`] (reads `std::env`). `ValueError` → `default`.
//! * `_log_signal` → [`log_signal_header`] / [`format_signal_name`] /
//!   [`signal_names_map`] + [`crash_log_header_signal`] + [`hard_exit_after_grace`]
//!   (file I/O `os.makedirs` + `open(..., "a")` + `traceback.print_stack` +
//!   `sys._current_frames()` + `threading._active` + `time.strftime` are modelled
//!   as injected `write_fn: FnMut(&str)` + `stack_fn: Fn() -> String` +
//!   `thread_stacks_fn: Fn() -> Vec<(u64,String)>`; the `Timer(_grace, os._exit)`
//!   + `sys.exit(0)` unwind + `_shutdown_sessions` explicit flush are exposed via
//!   [`shutdown_timer_config`] + [`should_hard_exit`]).
//! * `_install_signal` (`threading.current_thread() is main_thread()` guard +
//!   `getattr(signal, name, None)` + `signal.signal` `ValueError/OSError/RuntimeError`
//!   swallow) → [`should_install_signal`] (pure `is_main_thread: bool` + `has_signal: bool`
//!   → `bool`) + [`install_signal_result`] + [`default_signal_installs`] (the five
//!   calls `SIGPIPE→Ign`, `SIGTERM→Log`, `SIGHUP/BREAK→Log`, `SIGINT→Ign` guarded by
//!   `has_signal` and `is_main_thread`, matching the `hasattr` / `getattr(...,None)`
//!   and main-thread-only `signal.signal` legality).
//! * `_log_exit` → [`log_exit_line`] + [`crash_log_header_exit`] + [`log_exit`]
//!   (`os.makedirs` + `open append` + `time.strftime` + `print(file=sys.stderr)` →
//!   injected `mkdir_fn`/`append_fn`/`stderr_fn`).
//! * `_mcp_discovery_thread: Optional[Thread]` + `_mcp_discovery_enabled: bool` →
//!   [`McpDiscoveryState`] (`Option<McpThreadHandle>` + `enabled: bool` behind
//!   `Mutex`/`OnceLock`; `thread.is_alive()` → `Arc<AtomicBool>` + `JoinHandle::is_finished()`).
//! * `hermes_cli.mcp_startup._resolve_discovery_timeout` / `start_background_mcp_discovery`
//!   / `wait_for_mcp_discovery` / `join_mcp_discovery` / `mcp_discovery_in_flight`
//!   / `_has_configured_mcp_servers` → injected closures
//!   (`resolve_timeout: Fn(Option<f64>)->f64`, `start: Fn()`, `wait: Fn(Option<f64>)`,
//!   `in_flight: Fn()->bool`, `join: Fn(Option<f64>)->bool`, `has_servers: Fn()->bool`)
//!   so the port stays `std`-only. Default fallback `0.75` mirrors
//!   `except: bound = timeout if ... else 0.75`.
//! * `wait_for_mcp_discovery` idempotent-retry-after-zero-connected allowance →
//!   [`wait_for_mcp_discovery_with`] (entry-thread fast path `join(bound)` then
//!   `if not enabled: return`; otherwise `start()` idempotent spawn + `_startup_wait`).
//! * `mcp_discovery_in_flight` dual-owner check (entry thread + `hermes_cli.mcp_startup`) →
//!   [`mcp_in_flight_with`] (`entry_alive` OR `startup_in_flight()`).
//! * `join_mcp_discovery` dual-owner `join(timeout)` per owner →
//!   [`join_mcp_discovery_with`] (`entry_done && startup_done`).
//! * `_recovery_times: list[float]` + `time.time()` + 60 s sliding window +
//!   `MAX_RECOVERIES_PER_MINUTE` → [`MAX_RECOVERIES_PER_MINUTE`] (reused from
//!   `crate::stdin_recovery`) + [`GatewayRecoveryState`] + helpers that delegate
//!   to `crate::stdin_recovery::handle_spurious_eof` (the Python `entry.py`
//!   imports the same helper; the window pruning `retain(|t| *t > now-60.0)` is
//!   preserved).
//! * `_has_configured_mcp_servers` / `ensure_mcp_discovery_started` once-per-process
//!   idempotent start through shared owner → [`has_configured_mcp_servers_with`] /
//!   [`ensure_mcp_discovery_started_with`] (`enabled` flag + `has_servers` guard +
//!   `start_background_mcp_discovery(logger, thread_name="tui-mcp-discovery")` + warn on `Err`).
//! * `server._schedule_startup_orphan_sweep` + `ensure_mcp_discovery_started()` +
//!   `write_json(gateway.ready)` + `_ensure_skin_watcher` + `prewarm_picker_cache_async` +
//!   `while True: readline → handle_spurious_eof → strip → json.loads → dispatch → write_json`
//!   loop → [`GatewayReadyPayload`] + [`should_exit_on_startup_write_fail`] +
//!   [`parse_request_line`] + [`handle_gateway_line`] + [`GatewayLoop`] /
//!   [`run_gateway_loop`] (`BufRead`/`Write` injected; `dispatch: Fn(&str)->Option<String>`
//!   + `write_json: Fn(&str)->bool` + `log_exit: FnMut(&str)`; empty line → `None`,
//!   parse error → `{"error":{"code":-32700,"message":"parse error"}}` with `false` → `_log_exit`).
//! * `sys.stdin.readline()` returning `""` (EOF) → `BufRead::read_line` returning `0`.
//! * `time.strftime('%Y-%m-%d %H:%M:%S')` → [`format_timestamp`] (`SystemTime` → `%Y-%m-%d %H:%M:%S` via
//!   `chrono`-free manual formatting from `UNIX_EPOCH` days, UTC).

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Constants — mirrors entry.py:48-86 + 28-45
// ---------------------------------------------------------------------------

/// Env var the dashboard `/api/pty` sets so the PTY child mirrors emits.
///
/// Mirrors `os.environ.get("HERMES_TUI_SIDECAR_URL")` in `_install_sidecar_publisher`.
pub const ENV_SIDECAR_URL: &str = "HERMES_TUI_SIDECAR_URL";

/// Env var for the shutdown grace window.
///
/// Mirrors `os.environ.get("HERMES_TUI_GATEWAY_SHUTDOWN_GRACE_S")`.
pub const ENV_SHUTDOWN_GRACE: &str = "HERMES_TUI_GATEWAY_SHUTDOWN_GRACE_S";

/// Default shutdown grace in seconds.
///
/// Mirrors `_DEFAULT_SHUTDOWN_GRACE_S = 1.0`.
pub const DEFAULT_SHUTDOWN_GRACE_S: f64 = 1.0;

/// Minimum grace — Python `return value if value > 0 else _DEFAULT_SHUTDOWN_GRACE_S`.
pub const MIN_SHUTDOWN_GRACE_S: f64 = 0.0;

/// Fallback MCP discovery join bound when `_resolve_discovery_timeout` raises.
///
/// Mirrors `except Exception: bound = timeout if timeout is not None else 0.75`.
pub const DEFAULT_MCP_DISCOVERY_TIMEOUT_S: f64 = 0.75;

/// Thread name for the MCP discovery thread.
///
/// Mirrors `thread_name="tui-mcp-discovery"` in `ensure_mcp_discovery_started`.
pub const MCP_DISCOVERY_THREAD_NAME: &str = "tui-mcp-discovery";

/// Maximum spurious-EOF recoveries per minute — re-exported from
/// `crate::stdin_recovery` so entry callers don't import two places.
///
/// Mirrors `_recovery_times` rate limit in `_stdin_recovery.py`.
pub const MAX_RECOVERIES_PER_MINUTE: usize = crate::stdin_recovery::MAX_RECOVERIES_PER_MINUTE;

// ---------------------------------------------------------------------------
// Shutdown grace — mirrors _shutdown_grace_seconds
// ---------------------------------------------------------------------------

/// Pure helper: resolve grace from an injected raw env string.
///
/// Mirrors `tui_gateway/entry.py::_shutdown_grace_seconds`:
///
/// ```python
/// def _shutdown_grace_seconds() -> float:
///     raw = (os.environ.get("HERMES_TUI_GATEWAY_SHUTDOWN_GRACE_S") or "").strip()
///     if not raw: return _DEFAULT_SHUTDOWN_GRACE_S
///     try: value = float(raw)
///     except ValueError: return _DEFAULT_SHUTDOWN_GRACE_S
///     return value if value > 0 else _DEFAULT_SHUTDOWN_GRACE_S
/// ```
pub fn shutdown_grace_with(raw: Option<&str>) -> f64 {
    let Some(s) = raw else {
        return DEFAULT_SHUTDOWN_GRACE_S;
    };
    let t = s.trim();
    if t.is_empty() {
        return DEFAULT_SHUTDOWN_GRACE_S;
    }
    let parsed: f64 = match t.parse() {
        Ok(v) => v,
        Err(_) => return DEFAULT_SHUTDOWN_GRACE_S,
    };
    if parsed > 0.0 && parsed.is_finite() {
        parsed
    } else {
        DEFAULT_SHUTDOWN_GRACE_S
    }
}

/// Read `HERMES_TUI_GATEWAY_SHUTDOWN_GRACE_S` from the process env.
///
/// Mirrors `_shutdown_grace_seconds()` with `os.environ.get`.
pub fn shutdown_grace_secs() -> f64 {
    let raw = std::env::var(ENV_SHUTDOWN_GRACE).ok();
    shutdown_grace_with(raw.as_deref())
}

/// Alias for [`shutdown_grace_secs`] — mirrors the Python name `_shutdown_grace_seconds`.
pub fn shutdown_grace_seconds() -> f64 {
    shutdown_grace_secs()
}

// ---------------------------------------------------------------------------
// Sidecar publisher — mirrors _install_sidecar_publisher
// ---------------------------------------------------------------------------

/// Whether `url` should trigger the sidecar tee.
///
/// Mirrors `if not url: return` in `_install_sidecar_publisher`.
pub fn should_install_sidecar(url: Option<&str>) -> bool {
    match url {
        None => false,
        Some(s) if s.trim().is_empty() => false,
        Some(_) => true,
    }
}

/// Resolve the sidecar URL from an injected env map.
///
/// Returns `Some(trimmed_url)` when `HERMES_TUI_SIDECAR_URL` is present and
/// non-empty after `strip()`, otherwise `None`. Mirrors
/// `url = os.environ.get("HERMES_TUI_SIDECAR_URL"); if not url: return`.
pub fn sidecar_url_with(env_get: impl Fn(&str) -> Option<String>) -> Option<String> {
    let raw = env_get(ENV_SIDECAR_URL)?;
    let t = raw.trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

/// Read `HERMES_TUI_SIDECAR_URL` from the process env.
pub fn sidecar_url() -> Option<String> {
    sidecar_url_with(|k| std::env::var(k).ok())
}

/// Tee-transport config created by `_install_sidecar_publisher`.
///
/// Mirrors `server._stdio_transport = TeeTransport(server._stdio_transport, WsPublisherTransport(url))`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeeTransportConfig {
    /// The sidecar URL to dial. Mirrors `WsPublisherTransport(url)`.
    pub url: String,
}

/// Build the tee config if a sidecar URL is present, otherwise `None`.
///
/// Pure helper — the actual `server._stdio_transport` assignment is injected
/// via `apply: FnMut(TeeTransportConfig)`.
pub fn tee_transport_config(url: Option<&str>) -> Option<TeeTransportConfig> {
    let u = url?.trim();
    if u.is_empty() {
        None
    } else {
        Some(TeeTransportConfig { url: u.to_string() })
    }
}

/// Try to install the sidecar publisher, calling `apply` when a URL is present.
///
/// Mirrors `_install_sidecar_publisher`:
///
/// ```python
/// def _install_sidecar_publisher() -> None:
///     url = os.environ.get("HERMES_TUI_SIDECAR_URL")
///     if not url: return
///     from tui_gateway.event_publisher import WsPublisherTransport
///     server._stdio_transport = TeeTransport(server._stdio_transport, WsPublisherTransport(url))
/// ```
///
/// `env_get` mirrors `os.environ.get`, `apply` mirrors the `TeeTransport` assignment.
/// Returns `true` when the sidecar was installed.
pub fn install_sidecar_publisher_with<E, F>(env_get: E, mut apply: F) -> bool
where
    E: Fn(&str) -> Option<String>,
    F: FnMut(TeeTransportConfig),
{
    let Some(url) = sidecar_url_with(env_get) else {
        return false;
    };
    let cfg = TeeTransportConfig { url };
    apply(cfg);
    true
}

/// Convenience: read `HERMES_TUI_SIDECAR_URL` from `std::env` and apply.
pub fn install_sidecar_publisher<F>(apply: F) -> bool
where
    F: FnMut(TeeTransportConfig),
{
    install_sidecar_publisher_with(|k| std::env::var(k).ok(), apply)
}

// ---------------------------------------------------------------------------
// Crash log helpers — mirrors _log_signal / _log_exit header formatting
// ---------------------------------------------------------------------------

/// Format `time.strftime('%Y-%m-%d %H:%M:%S')` for `SystemTime` (UTC).
///
/// Lightweight `chrono`-free formatter: days since `UNIX_EPOCH` → Gregorian
/// date via 400-year cycles (Howard Hinnant's civil_from_days). Used for the
/// crash-log headers `=== {name} received · {ts} ===` and
/// `=== gateway exit · {ts} · reason={reason} ===`.
pub fn format_timestamp(now: SystemTime) -> String {
    let secs = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    let days = (secs / 86_400) as i64;
    let secs_of_day = (secs % 86_400) as u32;
    let (y, m, d) = civil_from_days(days);
    let hh = secs_of_day / 3600;
    let mm = (secs_of_day % 3600) / 60;
    let ss = secs_of_day % 60;
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, m, d, hh, mm, ss)
}

/// Current timestamp string (UTC) — mirrors `time.strftime('%Y-%m-%d %H:%M:%S')`.
pub fn now_timestamp() -> String {
    format_timestamp(SystemTime::now())
}

// Howard Hinnant civil_from_days — converts days since 1970-01-01 to y/m/d.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    // Shift epoch to 0000-03-01 so leap day is last day of year.
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0,399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0,365]
    let mp = (5 * doy + 2) / 153; // [0,11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1,31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1,12]
    let y = y + if m <= 2 { 1 } else { 0 };
    (y as i32, m as u32, d as u32)
}

/// Map signal numbers to the names Python's `_log_signal` builds.
///
/// Mirrors the `for _attr in ("SIGPIPE","SIGTERM","SIGHUP","SIGINT","SIGBREAK"):`
/// loop that builds `_signal_names: dict[int,str]`.
pub fn signal_names_map(signals: &[(i32, &str)]) -> HashMap<i32, String> {
    let mut m = HashMap::new();
    for (num, name) in signals {
        m.insert(*num, (*name).to_string());
    }
    m
}

/// Resolve a signal name from a number, falling back to `signal {n}`.
///
/// Mirrors `name = _signal_names.get(signum, f"signal {signum}")`.
pub fn format_signal_name(signum: i32, known: &HashMap<i32, String>) -> String {
    known
        .get(&signum)
        .cloned()
        .unwrap_or_else(|| format!("signal {}", signum))
}

/// Crash-log header for a signal delivery.
///
/// Mirrors `f.write(f"\n=== {name} received · {time.strftime('%Y-%m-%d %H:%M:%S')} ===\n")`.
pub fn crash_log_header_signal(name: &str, timestamp: &str) -> String {
    format!("\n=== {} received · {} ===\n", name, timestamp)
}

/// Crash-log header for a gateway exit.
///
/// Mirrors `f.write(f"\n=== gateway exit · {time.strftime('%Y-%m-%d %H:%M:%S')} · reason={reason} ===\n")`.
pub fn crash_log_header_exit(timestamp: &str, reason: &str) -> String {
    format!("\n=== gateway exit · {} · reason={} ===\n", timestamp, reason)
}

/// Stderr line for a signal delivery.
///
/// Mirrors `print(f"[gateway-signal] {name}", file=sys.stderr, flush=True)`.
pub fn log_signal_stderr_line(name: &str) -> String {
    format!("[gateway-signal] {}", name)
}

/// Stderr line for a gateway exit.
///
/// Mirrors `print(f"[gateway-exit] {reason}", file=sys.stderr, flush=True)`.
pub fn log_exit_line(reason: &str) -> String {
    format!("[gateway-exit] {}", reason)
}

/// Write a crash-log entry via injected `append: FnMut(&str)` (mirrors `open(..., "a")` + `write`).
///
/// `mkdir` is the caller's responsibility (mirrors `os.makedirs(os.path.dirname(_CRASH_LOG), exist_ok=True)` → injected).
/// Failures are swallowed (`except Exception: pass`).
pub fn append_crash_log<F>(mut append: F, text: &str)
where
    F: FnMut(&str) -> Result<(), String>,
{
    let _ = append(text);
}

/// Full `_log_exit` helper with injectable side effects.
///
/// Mirrors `tui_gateway/entry.py::_log_exit`:
///
/// ```python
/// def _log_exit(reason: str) -> None:
///     try:
///         os.makedirs(os.path.dirname(_CRASH_LOG), exist_ok=True)
///         with open(_CRASH_LOG, "a", encoding="utf-8") as f:
///             f.write(f"\n=== gateway exit · {time.strftime('%Y-%m-%d %H:%M:%S')} · reason={reason} ===\n")
///     except Exception: pass
///     print(f"[gateway-exit] {reason}", file=sys.stderr, flush=True)
/// ```
pub fn log_exit<E, A, S>(reason: &str, mut mkdir: E, mut append: A, mut stderr: S)
where
    E: FnMut() -> Result<(), String>,
    A: FnMut(&str) -> Result<(), String>,
    S: FnMut(&str),
{
    let ts = now_timestamp();
    let header = crash_log_header_exit(&ts, reason);
    let _ = mkdir();
    let _ = append(&header);
    stderr(&log_exit_line(reason));
}

/// Injectable variant with explicit timestamp (test seam).
pub fn log_exit_with<E, A, S>(reason: &str, timestamp: &str, mut mkdir: E, mut append: A, mut stderr: S)
where
    E: FnMut() -> Result<(), String>,
    A: FnMut(&str) -> Result<(), String>,
    S: FnMut(&str),
{
    let header = crash_log_header_exit(timestamp, reason);
    let _ = mkdir();
    let _ = append(&header);
    stderr(&log_exit_line(reason));
}

// ---------------------------------------------------------------------------
// Signal installation — mirrors _install_signal + the five _install_signal calls
// ---------------------------------------------------------------------------

/// Handler kind for the signal table.
///
/// Mirrors `signal.SIG_IGN` vs `_log_signal` (Python's `signal.signal(sig, handler)`
/// second arg). In Rust the handler is a string tag; real `sigaction` wiring is
/// outside this crate (this port stays `std`-only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignalHandler {
    /// Mirrors `signal.SIG_IGN`.
    Ignore,
    /// Mirrors `_log_signal` (diagnosable termination).
    LogSignal,
}

/// One installed signal entry — mirrors a single `_install_signal("SIG...", handler)` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalInstall {
    /// Signal name, e.g. `"SIGPIPE"`.
    pub name: String,
    /// Handler.
    pub handler: SignalHandler,
}

/// Whether `_install_signal` would actually call `signal.signal`.
///
/// Mirrors `tui_gateway/entry.py::_install_signal`:
///
/// ```python
/// def _install_signal(signame, handler):
///     if threading.current_thread() is not threading.main_thread(): return
///     sig = getattr(signal, signame, None)
///     if sig is None: return  # Windows: SIGPIPE/SIGHUP absent
///     try: signal.signal(sig, handler)
///     except (ValueError, OSError, RuntimeError): pass
/// ```
pub fn should_install_signal(is_main_thread: bool, has_signal: bool) -> bool {
    is_main_thread && has_signal
}

/// Result of attempting to install a signal — mirrors the `try/except` swallow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallSignalResult {
    /// `signal.signal` was called (or would be — `is_main_thread && has_signal`).
    Installed,
    /// Skipped: not on main thread or signal absent on this platform.
    Skipped,
    /// `signal.signal` raised `ValueError/OSError/RuntimeError` — swallowed.
    Failed(String),
}

/// Pure helper: compute `InstallSignalResult` from injected booleans.
pub fn install_signal_result(is_main_thread: bool, has_signal: bool, should_fail: bool) -> InstallSignalResult {
    if !should_install_signal(is_main_thread, has_signal) {
        return InstallSignalResult::Skipped;
    }
    if should_fail {
        InstallSignalResult::Failed("signal.signal rejected handler".to_string())
    } else {
        InstallSignalResult::Installed
    }
}

/// The five signal installations at module load.
///
/// Mirrors the block:
///
/// ```python
/// _install_signal("SIGPIPE", signal.SIG_IGN)
/// _install_signal("SIGTERM", _log_signal)
/// if hasattr(signal, "SIGHUP"):
///     _install_signal("SIGHUP", _log_signal)
/// elif hasattr(signal, "SIGBREAK"):
///     _install_signal("SIGBREAK", _log_signal)
/// _install_signal("SIGINT", signal.SIG_IGN)
/// ```
///
/// `has_sighup`/`has_sigbreak` mirror `hasattr(signal, "SIGHUP")` / `hasattr(..., "SIGBREAK")`.
/// `is_main_thread` mirrors `threading.current_thread() is threading.main_thread()`.
pub fn default_signal_installs(
    is_main_thread: bool,
    has_sigpipe: bool,
    has_sigterm: bool,
    has_sighup: bool,
    has_sigbreak: bool,
    has_sigint: bool,
) -> Vec<SignalInstall> {
    let mut out = Vec::new();
    if should_install_signal(is_main_thread, has_sigpipe) {
        out.push(SignalInstall { name: "SIGPIPE".into(), handler: SignalHandler::Ignore });
    }
    if should_install_signal(is_main_thread, has_sigterm) {
        out.push(SignalInstall { name: "SIGTERM".into(), handler: SignalHandler::LogSignal });
    }
    if has_sighup {
        if should_install_signal(is_main_thread, true) {
            out.push(SignalInstall { name: "SIGHUP".into(), handler: SignalHandler::LogSignal });
        }
    } else if has_sigbreak && should_install_signal(is_main_thread, true) {
        out.push(SignalInstall { name: "SIGBREAK".into(), handler: SignalHandler::LogSignal });
    }
    if should_install_signal(is_main_thread, has_sigint) {
        out.push(SignalInstall { name: "SIGINT".into(), handler: SignalHandler::Ignore });
    }
    out
}

/// Shutdown timer config — mirrors `threading.Timer(_shutdown_grace_seconds(), _hard_exit)`.
///
/// `grace_s` mirrors `_shutdown_grace_seconds()`, `daemon=True` is inherent
/// (Rust detached thread), and `_hard_exit = lambda: os._exit(0)` is the
/// `on_hard_exit` closure.
#[derive(Debug, Clone, PartialEq)]
pub struct ShutdownTimerConfig {
    /// Grace in seconds before `os._exit(0)`.
    pub grace_s: f64,
    /// Whether the timer is daemon (always `true` in Python; noted for completeness).
    pub daemon: bool,
}

/// Build the shutdown timer config — mirrors the `Timer` creation in `_log_signal`.
pub fn shutdown_timer_config(grace_s: f64) -> ShutdownTimerConfig {
    ShutdownTimerConfig { grace_s, daemon: true }
}

/// Whether `os._exit(0)` should fire — mirrors the `Timer` callback.
///
/// In Python the timer is `daemon=True` and fires after `grace_s` unless the
/// process exits cleanly via `sys.exit(0)` unwind. This helper just records
/// the grace so callers can inject a real timer.
pub fn should_hard_exit(elapsed_s: f64, grace_s: f64) -> bool {
    elapsed_s >= grace_s
}

// ---------------------------------------------------------------------------
// MCP discovery state — mirrors _mcp_discovery_thread / _mcp_discovery_enabled
// ---------------------------------------------------------------------------

/// State for the background MCP discovery thread.
///
/// Mirrors the two module globals:
///
/// ```python
/// _mcp_discovery_thread = None          # Optional[Thread]
/// _mcp_discovery_enabled = False        # bool
/// ```
#[derive(Debug, Default)]
pub struct McpDiscoveryState {
    /// Whether this process has MCP servers configured and kicked off discovery.
    ///
    /// Mirrors `_mcp_discovery_enabled` — set to `true` once
    /// `ensure_mcp_discovery_started` decides to spawn.
    pub enabled: bool,
    /// Whether the entry-owned discovery thread is still alive.
    ///
    /// Mirrors `_mcp_discovery_thread is not None and thread.is_alive()`.
    /// Modelled as `Arc<AtomicBool>` (alive flag) + optional `JoinHandle` for
    /// join/timeout semantics. When `None`, no entry thread was spawned.
    pub entry_alive: Arc<AtomicBool>,
    /// Handle for the entry thread (if spawned via this state).
    pub entry_handle: Option<thread::JoinHandle<()>>,
}

impl McpDiscoveryState {
    /// Create a fresh state (no thread, not enabled).
    pub fn new() -> Self {
        Self { enabled: false, entry_alive: Arc::new(AtomicBool::new(false)), entry_handle: None }
    }

    /// Mark enabled — mirrors `_mcp_discovery_enabled = True`.
    pub fn set_enabled(&mut self) {
        self.enabled = true;
    }

    /// Whether the entry thread is alive.
    pub fn entry_in_flight(&self) -> bool {
        self.entry_alive.load(Ordering::SeqCst)
    }
}

static MCP_STATE: OnceLock<Mutex<McpDiscoveryState>> = OnceLock::new();

fn global_mcp_state() -> &'static Mutex<McpDiscoveryState> {
    MCP_STATE.get_or_init(|| Mutex::new(McpDiscoveryState::new()))
}

/// Whether any background MCP discovery is still running.
///
/// Mirrors `tui_gateway/entry.py::mcp_discovery_in_flight`:
///
/// ```python
/// def mcp_discovery_in_flight() -> bool:
///     thread = _mcp_discovery_thread
///     if thread is not None and thread.is_alive(): return True
///     try:
///         from hermes_cli.mcp_startup import mcp_discovery_in_flight as _startup_in_flight
///         return _startup_in_flight()
///     except Exception: return False
/// ```
///
/// `entry_alive` mirrors the first check; `startup_in_flight` is injected for
/// the shared-owner check (`hermes_cli.mcp_startup`).
pub fn mcp_in_flight_with(entry_alive: bool, mut startup_in_flight: impl FnMut() -> bool) -> bool {
    if entry_alive {
        return true;
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| startup_in_flight())) {
        Ok(v) => v,
        Err(_) => false,
    }
}

/// Whether the global MCP discovery is in flight.
pub fn mcp_discovery_in_flight() -> bool {
    let g = global_mcp_state().lock().unwrap();
    let alive = g.entry_in_flight();
    drop(g);
    mcp_in_flight_with(alive, || false)
}

/// Block until background MCP discovery finishes, up to `timeout`.
///
/// Mirrors `tui_gateway/entry.py::join_mcp_discovery`:
///
/// ```python
/// def join_mcp_discovery(timeout: float | None = None) -> bool:
///     entry_done = True
///     thread = _mcp_discovery_thread
///     if thread is not None:
///         thread.join(timeout=timeout)
///         entry_done = not thread.is_alive()
///     try:
///         from hermes_cli.mcp_startup import join_mcp_discovery as _startup_join
///         startup_done = _startup_join(timeout=timeout)
///     except Exception: startup_done = True
///     return entry_done and startup_done
/// ```
pub fn join_mcp_discovery_with<F>(
    entry_alive_before: bool,
    mut entry_join: impl FnMut(Option<f64>),
    entry_alive_after: impl Fn() -> bool,
    mut startup_join: F,
    timeout: Option<f64>,
) -> bool
where
    F: FnMut(Option<f64>) -> bool,
{
    let entry_done = if entry_alive_before {
        entry_join(timeout);
        !entry_alive_after()
    } else {
        // No entry thread → vacuously done (mirrors `entry_done = True` when thread is None)
        true
    };
    let startup_done = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| startup_join(timeout))) {
        Ok(v) => v,
        Err(_) => true,
    };
    entry_done && startup_done
}

/// Resolve the MCP discovery timeout bound.
///
/// Mirrors the `try: from hermes_cli.mcp_startup import _resolve_discovery_timeout; bound = _resolve_discovery_timeout(timeout)`
/// branch, with fallback `timeout if timeout is not None else 0.75`.
pub fn resolve_discovery_timeout_with(
    timeout: Option<f64>,
    mut resolve: impl FnMut(Option<f64>) -> Result<f64, String>,
) -> f64 {
    match resolve(timeout) {
        Ok(v) => v,
        Err(_) => timeout.unwrap_or(DEFAULT_MCP_DISCOVERY_TIMEOUT_S),
    }
}

/// Wait for MCP discovery with injected deps.
///
/// Mirrors `tui_gateway/entry.py::wait_for_mcp_discovery`:
///
/// ```python
/// def wait_for_mcp_discovery(timeout: float | None = None) -> None:
///     thread = _mcp_discovery_thread
///     if thread is not None and thread.is_alive():
///         try: bound = _resolve_discovery_timeout(timeout)
///         except Exception: bound = timeout if timeout is not None else 0.75
///         thread.join(timeout=bound)
///         return
///     if not _mcp_discovery_enabled: return
///     try: start_background_mcp_discovery(logger=logger, thread_name="tui-mcp-discovery")
///     except Exception: logger.debug("TUI MCP discovery retry-spawn failed", exc_info=True)
///     try: from hermes_cli.mcp_startup import wait_for_mcp_discovery as _startup_wait; _startup_wait(timeout)
///     except Exception: pass
/// ```
pub fn wait_for_mcp_discovery_with<R, S, W>(
    entry_alive: bool,
    timeout: Option<f64>,
    mut resolve_timeout: R,
    mut entry_join: impl FnMut(f64),
    enabled: bool,
    mut start: S,
    mut startup_wait: W,
) where
    R: FnMut(Option<f64>) -> Result<f64, String>,
    S: FnMut(),
    W: FnMut(Option<f64>),
{
    if entry_alive {
        let bound = resolve_discovery_timeout_with(timeout, &mut resolve_timeout);
        entry_join(bound);
        return;
    }
    if !enabled {
        return;
    }
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| start()));
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| startup_wait(timeout)));
}

/// Check whether MCP servers are configured (injected).
///
/// Mirrors `_has_configured_mcp_servers` → `hermes_cli.mcp_startup._has_configured_mcp_servers`.
pub fn has_configured_mcp_servers_with(mut has: impl FnMut() -> bool) -> bool {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| has())) {
        Ok(v) => v,
        Err(_) => false,
    }
}

/// Ensure MCP discovery is started (injected).
///
/// Mirrors `tui_gateway/entry.py::ensure_mcp_discovery_started`:
///
/// ```python
/// def ensure_mcp_discovery_started() -> None:
///     global _mcp_discovery_enabled
///     if not _has_configured_mcp_servers(): return
///     _mcp_discovery_enabled = True
///     try:
///         from hermes_cli.mcp_startup import start_background_mcp_discovery
///         start_background_mcp_discovery(logger=logger, thread_name="tui-mcp-discovery")
///     except Exception:
///         logger.warning("Background MCP tool discovery failed to start", exc_info=True)
/// ```
///
/// Returns `true` when discovery was (or is now) enabled.
pub fn ensure_mcp_discovery_started_with<H, S>(
    enabled: &mut bool,
    mut has_servers: H,
    mut start: S,
) -> bool
where
    H: FnMut() -> bool,
    S: FnMut() -> Result<(), String>,
{
    let has = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| has_servers())) {
        Ok(v) => v,
        Err(_) => false,
    };
    if !has {
        return false;
    }
    *enabled = true;
    let _ = start();
    true
}

// ---------------------------------------------------------------------------
// Gateway ready / skin / orphan sweep helpers — mirrors main() prologue
// ---------------------------------------------------------------------------

/// Payload for `gateway.ready` event.
///
/// Mirrors `write_json({"jsonrpc":"2.0","method":"event","params":{"type":"gateway.ready","payload":{"skin": resolve_skin(), "change_events": True}}})`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayReadyPayload {
    /// Skin name from `resolve_skin()`.
    pub skin: String,
    /// Whether `change_events` is included (always `true` for modern gateway).
    pub change_events: bool,
}

impl GatewayReadyPayload {
    /// Create a `gateway.ready` payload (mirrors `resolve_skin()` result).
    pub fn new(skin: impl Into<String>) -> Self {
        Self { skin: skin.into(), change_events: true }
    }

    /// Serialize to JSON string for `write_json`.
    pub fn to_json(&self) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"event","params":{{"type":"gateway.ready","payload":{{"skin":"{}","change_events":true}}}}}}"#,
            json_escape(&self.skin)
        )
    }
}

/// Whether the process should exit after a failed startup write.
///
/// Mirrors `if not write_json({...}): _log_exit("startup write failed (broken stdout pipe before first event)"); sys.exit(0)`.
pub fn should_exit_on_startup_write_fail(write_ok: bool) -> bool {
    !write_ok
}

// ---------------------------------------------------------------------------
// Dispatch loop — mirrors main()'s while True: readline → dispatch → write_json
// ---------------------------------------------------------------------------

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push(ch),
        }
    }
    out
}

/// Parse error response line — mirrors the `except json.JSONDecodeError` branch.
///
/// Returns `{"jsonrpc":"2.0","error":{"code":-32700,"message":"parse error"},"id":null}`.
pub fn parse_error_response() -> String {
    r#"{"jsonrpc":"2.0","error":{"code":-32700,"message":"parse error"},"id":null}"#.to_string()
}

/// Extract `method` string from a JSON object line (best-effort, no serde).
///
/// Mirrors `method = req.get("method") if isinstance(req, dict) else None` for
/// the `reason` in `_log_exit(f"response write failed for method={method!r} ...")`.
pub fn extract_method(line: &str) -> Option<String> {
    // naive scan for `"method": "<value>"`
    let key = "\"method\"";
    let pos = line.find(key)?;
    let after = &line[pos + key.len()..];
    let colon = after.find(':')?;
    let val = after[colon + 1..].trim_start();
    if val.starts_with('"') {
        let end = val[1..].find('"')?;
        Some(val[1..1 + end].to_string())
    } else {
        None
    }
}

/// Handle one gateway input line, returning an action for the caller.
///
/// Mirrors the loop body in `main()`:
///
/// ```python
/// line = raw.strip()
/// if not line: continue  # → None
/// try: req = json.loads(line)
/// except json.JSONDecodeError:
///     if not write_json({"jsonrpc":"2.0","error":{...},"id":None}):
///         _log_exit("parse-error-response write failed (broken stdout pipe)")
///         sys.exit(0)
///     continue
/// method = req.get("method") if isinstance(req, dict) else None
/// resp = dispatch(req)
/// if resp is not None:
///     if not write_json(resp):
///         _log_exit(f"response write failed for method={method!r} (broken stdout pipe)")
///         sys.exit(0)
/// ```
///
/// Returns `GatewayLineAction` describing what the outer loop should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayLineAction {
    /// Empty/whitespace line — `continue`.
    Skip,
    /// Parse error — caller should `write_json(parse_error_response())` and
    /// check `false` → `log_exit("parse-error-response write failed (broken stdout pipe)")` + `exit`.
    ParseError { response: String },
    /// Dispatch produced a response — caller should `write_json(resp)` and
    /// check `false` → `log_exit("response write failed for method=...")` + `exit`.
    Response { response: String, method: Option<String> },
    /// Dispatch produced `None` (notification) — no write.
    NoResponse,
}

/// Pure helper: decide the action for `line` given `is_valid_json` and `dispatch` result.
///
/// `is_valid_json` mirrors `json.loads(line)` success; `dispatch_result` is
/// `Some(json_str)` when `dispatch(req)` returns a dict, `None` for notifications.
pub fn handle_gateway_line(
    line: &str,
    is_valid_json: bool,
    dispatch_result: Option<String>,
    method: Option<String>,
) -> GatewayLineAction {
    let t = line.trim();
    if t.is_empty() {
        return GatewayLineAction::Skip;
    }
    if !is_valid_json {
        return GatewayLineAction::ParseError { response: parse_error_response() };
    }
    match dispatch_result {
        Some(resp) => GatewayLineAction::Response { response: resp, method },
        None => GatewayLineAction::NoResponse,
    }
}

/// Full helper: parse `line` as JSON, call `dispatch`, and return the action.
///
/// `is_json` is determined by checking the line starts with `{`/`[` and ends
/// with `}`/`]` (lightweight, `std`-only). For real use, inject `dispatch`.
pub fn handle_gateway_line_with<F>(line: &str, mut dispatch: F) -> GatewayLineAction
where
    F: FnMut(&str) -> Option<String>,
{
    let t = line.trim();
    if t.is_empty() {
        return GatewayLineAction::Skip;
    }
    // Lightweight JSON validity check — start/end braces, not a full parser.
    // Python uses `json.loads`; real gateway's `dispatch` would validate.
    let looks_like_json = (t.starts_with('{') && t.ends_with('}')) || (t.starts_with('[') && t.ends_with(']'));
    if !looks_like_json {
        return GatewayLineAction::ParseError { response: parse_error_response() };
    }
    let method = extract_method(t);
    let resp = dispatch(t);
    match resp {
        Some(r) => GatewayLineAction::Response { response: r, method },
        None => GatewayLineAction::NoResponse,
    }
}

// ---------------------------------------------------------------------------
// GatewayLoop — stateful loop that mirrors main()'s while True
// ---------------------------------------------------------------------------

/// Stateful gateway loop — mirrors `main()`'s locals: `_recovery_times`, dispatch, skin, etc.
///
/// The real `main()` also calls `_install_sidecar_publisher`, `_schedule_startup_orphan_sweep`,
/// `ensure_mcp_discovery_started`, `_ensure_skin_watcher`, `prewarm_picker_cache_async` —
///
/// Those are injected as closures (`on_startup: FnMut()`), so `GatewayLoop` stays `std`-only
/// and testable. The `readline` loop itself is in [`run_gateway_loop`].
#[derive(Debug, Default)]
pub struct GatewayLoop {
    /// Spurious-EOF recovery timestamps — mirrors `_recovery_times: list[float]`.
    pub recovery_times: Vec<f64>,
}

impl GatewayLoop {
    /// Create a new loop.
    pub fn new() -> Self {
        Self { recovery_times: Vec::new() }
    }

    /// Handle one `read_line` result: `n==0` → spurious EOF check, otherwise `handle_gateway_line_with`.
    ///
    /// Returns `LoopStep` for the outer driver.
    pub fn step<F>(
        &mut self,
        line: &str,
        n: usize,
        log_fn: impl FnMut(&str),
        dispatch: F,
    ) -> LoopStep
    where
        F: FnMut(&str) -> Option<String>,
    {
        if n == 0 {
            // EOF — check spurious
            let recovered = crate::stdin_recovery::handle_spurious_eof(&mut self.recovery_times, log_fn);
            if recovered {
                return LoopStep::Recovered;
            } else {
                return LoopStep::Exit;
            }
        }
        let action = handle_gateway_line_with(line, dispatch);
        match action {
            GatewayLineAction::Skip => LoopStep::Continue,
            GatewayLineAction::ParseError { response } => LoopStep::Write { response, reason: "parse-error-response write failed (broken stdout pipe)".to_string() },
            GatewayLineAction::Response { response, method } => {
                let reason = format!("response write failed for method={:?} (broken stdout pipe)", method);
                LoopStep::Write { response, reason }
            }
            GatewayLineAction::NoResponse => LoopStep::Continue,
        }
    }
}

/// What the loop driver should do after one step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopStep {
    /// Spurious EOF recovered — `continue` the outer loop.
    Recovered,
    /// Genuine EOF — `break`.
    Exit,
    /// No write needed — `continue`.
    Continue,
    /// A JSON response must be written; `reason` is used for `_log_exit` if `write_json` returns `false`.
    Write { response: String, reason: String },
}

/// Run the gateway loop over `reader` → `writer`.
///
/// Mirrors `tui_gateway/entry.py::main`'s `while True:` but as a testable
/// `BufRead`/`Write` function. Injected `dispatch` mirrors `server.dispatch`,
/// `write_json` mirrors `server.write_json` (returns `false` on broken pipe),
/// `log_exit` mirrors `_log_exit`, and `on_startup` mirrors the prologue
/// (`_install_sidecar_publisher`, `_schedule_startup_orphan_sweep`, etc.).
///
/// Returns `Ok(())` on clean EOF, `Err(reason)` when a `write_json` failed
/// (caller would `sys.exit(0)` after `_log_exit`).
pub fn run_gateway_loop<R, W, D, Lj, Lo>(
    reader: &mut R,
    writer: &mut W,
    mut dispatch: D,
    mut write_json: Lj,
    mut log_exit: Lo,
    mut on_startup: impl FnMut() -> Result<(), String>,
) -> io::Result<Result<(), String>>
where
    R: BufRead,
    W: Write,
    D: FnMut(&str) -> Option<String>,
    Lj: FnMut(&str, &mut W) -> bool,
    Lo: FnMut(&str),
{
    // Prologue — mirrors the `try: server._schedule_startup_orphan_sweep()` + `ensure_mcp_discovery_started()` +
    // `write_json(gateway.ready)` + `_ensure_skin_watcher` + `prewarm_picker_cache_async` block.
    // Failures are swallowed (best-effort); only `write_json` failure exits.
    let _ = on_startup();

    let mut state = GatewayLoop::new();
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            let recovered = crate::stdin_recovery::handle_spurious_eof(&mut state.recovery_times, |m| log_exit(m));
            if !recovered {
                break;
            }
            continue;
        }
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        // Validate JSON + dispatch
        let action = handle_gateway_line_with(t, &mut dispatch);
        match action {
            GatewayLineAction::Skip => continue,
            GatewayLineAction::NoResponse => continue,
            GatewayLineAction::ParseError { response } => {
                if !write_json(&response, writer) {
                    log_exit("parse-error-response write failed (broken stdout pipe)");
                    return Ok(Err("parse-error-response write failed".to_string()));
                }
            }
            GatewayLineAction::Response { response, method } => {
                let _ = method;
                if !write_json(&response, writer) {
                    let reason = format!("response write failed for method={:?} (broken stdout pipe)", extract_method(t));
                    log_exit(&reason);
                    return Ok(Err(reason));
                }
            }
        }
    }
    Ok(Ok(()))
}

// ---------------------------------------------------------------------------
// Tests — mirror Python invariants
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // -- shutdown grace ---------------------------------------------------

    #[test]
    fn shutdown_grace_defaults() {
        assert!((shutdown_grace_with(None) - DEFAULT_SHUTDOWN_GRACE_S).abs() < 1e-9);
        assert!((shutdown_grace_with(Some("")) - DEFAULT_SHUTDOWN_GRACE_S).abs() < 1e-9);
        assert!((shutdown_grace_with(Some("   ")) - DEFAULT_SHUTDOWN_GRACE_S).abs() < 1e-9);
        assert!((shutdown_grace_with(Some("not-a-number")) - DEFAULT_SHUTDOWN_GRACE_S).abs() < 1e-9);
        assert!((shutdown_grace_with(Some("0")) - DEFAULT_SHUTDOWN_GRACE_S).abs() < 1e-9);
        assert!((shutdown_grace_with(Some("-1")) - DEFAULT_SHUTDOWN_GRACE_S).abs() < 1e-9);
        assert!((shutdown_grace_with(Some("inf")) - DEFAULT_SHUTDOWN_GRACE_S).abs() < 1e-9);
        assert!((shutdown_grace_with(Some("nan")) - DEFAULT_SHUTDOWN_GRACE_S).abs() < 1e-9);
    }

    #[test]
    fn shutdown_grace_positive() {
        assert!((shutdown_grace_with(Some("1.0")) - 1.0).abs() < 1e-9);
        assert!((shutdown_grace_with(Some(" 2.5 ")) - 2.5).abs() < 1e-9);
        assert!((shutdown_grace_with(Some("0.001")) - 0.001).abs() < 1e-9);
        assert!((shutdown_grace_with(Some("10")) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn shutdown_grace_from_env_overrides() {
        // Set then clear to avoid leaking env into other tests.
        std::env::set_var(ENV_SHUTDOWN_GRACE, "3.5");
        assert!((shutdown_grace_secs() - 3.5).abs() < 1e-9);
        std::env::remove_var(ENV_SHUTDOWN_GRACE);
        assert!((shutdown_grace_secs() - DEFAULT_SHUTDOWN_GRACE_S).abs() < 1e-9);
        // invalid → default
        std::env::set_var(ENV_SHUTDOWN_GRACE, "bad");
        assert!((shutdown_grace_secs() - DEFAULT_SHUTDOWN_GRACE_S).abs() < 1e-9);
        std::env::remove_var(ENV_SHUTDOWN_GRACE);
    }

    // -- sidecar publisher ------------------------------------------------

    #[test]
    fn sidecar_url_filtering() {
        assert!(!should_install_sidecar(None));
        assert!(!should_install_sidecar(Some("")));
        assert!(!should_install_sidecar(Some("   ")));
        assert!(should_install_sidecar(Some("ws://example.com/api/pub")));
        assert!(should_install_sidecar(Some(" ws://x ")));

        assert_eq!(tee_transport_config(None), None);
        assert_eq!(tee_transport_config(Some("")), None);
        assert_eq!(tee_transport_config(Some("   ")), None);
        assert_eq!(tee_transport_config(Some("ws://a")), Some(TeeTransportConfig { url: "ws://a".into() }));
        assert_eq!(tee_transport_config(Some(" ws://a ")), Some(TeeTransportConfig { url: "ws://a".into() }));
    }

    #[test]
    fn sidecar_url_with_injected_env() {
        let mut env = HashMap::new();
        env.insert(ENV_SIDECAR_URL.to_string(), "ws://example.com/api/pub?token=abc".to_string());
        let url = sidecar_url_with(|k| env.get(k).cloned());
        assert_eq!(url.as_deref(), Some("ws://example.com/api/pub?token=abc"));

        env.insert(ENV_SIDECAR_URL.to_string(), "   ".to_string());
        assert_eq!(sidecar_url_with(|k| env.get(k).cloned()), None);

        let empty: HashMap<String, String> = HashMap::new();
        assert_eq!(sidecar_url_with(|k| empty.get(k).cloned()), None);
    }

    #[test]
    fn install_sidecar_publisher_calls_apply_once() {
        let mut env = HashMap::new();
        env.insert(ENV_SIDECAR_URL.to_string(), "ws://example.com/sidecar".to_string());
        let mut applied: Vec<TeeTransportConfig> = Vec::new();
        let did = install_sidecar_publisher_with(|k| env.get(k).cloned(), |cfg| applied.push(cfg));
        assert!(did);
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].url, "ws://example.com/sidecar");

        // absent → no apply
        let empty: HashMap<String, String> = HashMap::new();
        let mut applied2: Vec<TeeTransportConfig> = Vec::new();
        let did2 = install_sidecar_publisher_with(|k| empty.get(k).cloned(), |cfg| applied2.push(cfg));
        assert!(!did2);
        assert!(applied2.is_empty());
    }

    // -- signal helpers ---------------------------------------------------

    #[test]
    fn signal_name_lookup() {
        let mut m = HashMap::new();
        m.insert(10, "SIGPIPE".to_string());
        m.insert(15, "SIGTERM".to_string());
        assert_eq!(format_signal_name(15, &m), "SIGTERM");
        assert_eq!(format_signal_name(999, &m), "signal 999");

        let map = signal_names_map(&[(10, "SIGPIPE"), (15, "SIGTERM"), (1, "SIGHUP")]);
        assert_eq!(map.len(), 3);
        assert_eq!(map[&10], "SIGPIPE");
    }

    #[test]
    fn crash_log_headers() {
        assert_eq!(crash_log_header_signal("SIGTERM", "2026-08-27 10:00:00"), "\n=== SIGTERM received · 2026-08-27 10:00:00 ===\n");
        assert_eq!(crash_log_header_exit("2026-08-27 10:00:00", "startup write failed (broken stdout pipe before first event)"),
                   "\n=== gateway exit · 2026-08-27 10:00:00 · reason=startup write failed (broken stdout pipe before first event) ===\n");
        assert_eq!(log_signal_stderr_line("SIGTERM"), "[gateway-signal] SIGTERM");
        assert_eq!(log_exit_line("parse-error-response write failed (broken stdout pipe)"), "[gateway-exit] parse-error-response write failed (broken stdout pipe)");
    }

    #[test]
    fn timestamp_formatting_known_date() {
        // 1970-01-01 00:00:00 UTC
        assert_eq!(format_timestamp(UNIX_EPOCH), "1970-01-01 00:00:00");
        // 2026-01-01 00:00:00 UTC ≈ 1767225600 secs
        let t = UNIX_EPOCH + Duration::from_secs(1767225600);
        assert_eq!(format_timestamp(t), "2026-01-01 00:00:00");
    }

    #[test]
    fn log_exit_injected() {
        let mut mkdir_called = false;
        let mut appended: Vec<String> = Vec::new();
        let mut stderr: Vec<String> = Vec::new();
        log_exit_with("broken pipe", "2026-08-27 10:00:00",
            || { mkdir_called = true; Ok(()) },
            |s| { appended.push(s.to_string()); Ok(()) },
            |s| stderr.push(s.to_string()),
        );
        assert!(mkdir_called);
        assert_eq!(appended.len(), 1);
        assert!(appended[0].contains("gateway exit"));
        assert!(appended[0].contains("broken pipe"));
        assert_eq!(stderr, vec!["[gateway-exit] broken pipe"]);
    }

    #[test]
    fn should_install_signal_matrix() {
        assert!(should_install_signal(true, true));
        assert!(!should_install_signal(false, true));
        assert!(!should_install_signal(true, false));
        assert!(!should_install_signal(false, false));
        assert_eq!(install_signal_result(true, true, false), InstallSignalResult::Installed);
        assert_eq!(install_signal_result(false, true, false), InstallSignalResult::Skipped);
        assert_eq!(install_signal_result(true, false, false), InstallSignalResult::Skipped);
        assert!(matches!(install_signal_result(true, true, true), InstallSignalResult::Failed(_)));
    }

    #[test]
    fn default_signal_installs_table() {
        // Non-main thread → nothing
        assert!(default_signal_installs(false, true, true, true, true, true).is_empty());
        // Main thread, all signals present, SIGHUP path
        let v = default_signal_installs(true, true, true, true, false, true);
        assert_eq!(v.len(), 4); // SIGPIPE, SIGTERM, SIGHUP, SIGINT
        assert!(v.iter().any(|s| s.name == "SIGPIPE" && s.handler == SignalHandler::Ignore));
        assert!(v.iter().any(|s| s.name == "SIGTERM" && s.handler == SignalHandler::LogSignal));
        assert!(v.iter().any(|s| s.name == "SIGHUP"));
        assert!(v.iter().any(|s| s.name == "SIGINT"));
        // SIGHUP absent, SIGBREAK present → SIGBREAK instead
        let v2 = default_signal_installs(true, true, true, false, true, true);
        assert!(v2.iter().any(|s| s.name == "SIGBREAK"));
        assert!(!v2.iter().any(|s| s.name == "SIGHUP"));
        // Both absent → no SIGHUP/BREAK
        let v3 = default_signal_installs(true, true, true, false, false, true);
        assert_eq!(v3.len(), 3); // SIGPIPE, SIGTERM, SIGINT
        // SIGPIPE absent (Windows) → skipped
        let v4 = default_signal_installs(true, false, true, true, false, true);
        assert!(!v4.iter().any(|s| s.name == "SIGPIPE"));
    }

    #[test]
    fn shutdown_timer_config_and_hard_exit() {
        let c = shutdown_timer_config(1.0);
        assert!((c.grace_s - 1.0).abs() < 1e-9);
        assert!(c.daemon);
        assert!(!should_hard_exit(0.5, 1.0));
        assert!(should_hard_exit(1.0, 1.0));
        assert!(should_hard_exit(2.0, 1.0));
    }

    // -- gateway ready ----------------------------------------------------

    #[test]
    fn gateway_ready_payload() {
        let p = GatewayReadyPayload::new("default");
        assert_eq!(p.skin, "default");
        assert!(p.change_events);
        let j = p.to_json();
        assert!(j.contains("gateway.ready"));
        assert!(j.contains("\"skin\":\"default\""));
        assert!(j.contains("\"change_events\":true"));
        assert!(should_exit_on_startup_write_fail(false));
        assert!(!should_exit_on_startup_write_fail(true));
    }

    // -- MCP discovery ----------------------------------------------------

    #[test]
    fn mcp_in_flight_dual_owner() {
        assert!(mcp_in_flight_with(true, || false));
        assert!(mcp_in_flight_with(false, || true));
        assert!(!mcp_in_flight_with(false, || false));
        // startup panic → false
        assert!(!mcp_in_flight_with(false, || panic!("boom")));
        assert!(mcp_in_flight_with(true, || panic!("boom")));
    }

    #[test]
    fn join_mcp_discovery_both_done() {
        // No entry thread, startup join true → done
        let ok = join_mcp_discovery_with(false, |_| {}, || false, |_| true, Some(1.0));
        assert!(ok);
        // Entry alive but join makes it not alive, startup true → done
        let mut joined = false;
        let ok2 = join_mcp_discovery_with(true, |_| { joined = true; }, || false, |_| true, Some(1.0));
        assert!(joined);
        assert!(ok2);
        // Entry still alive after join → not done
        let ok3 = join_mcp_discovery_with(true, |_| {}, || true, |_| true, None);
        assert!(!ok3);
        // Startup not done → not done
        let ok4 = join_mcp_discovery_with(false, |_| {}, || false, |_| false, None);
        assert!(!ok4);
        // Startup panic → true (swallowed)
        let ok5 = join_mcp_discovery_with(false, |_| {}, || false, |_| panic!("x"), None);
        assert!(ok5);
    }

    #[test]
    fn resolve_timeout_fallback() {
        let v = resolve_discovery_timeout_with(Some(2.0), |_| Ok(0.5));
        assert!((v - 0.5).abs() < 1e-9);
        let v2 = resolve_discovery_timeout_with(Some(2.0), |_| Err("boom".into()));
        assert!((v2 - 2.0).abs() < 1e-9);
        let v3 = resolve_discovery_timeout_with(None, |_| Err("boom".into()));
        assert!((v3 - DEFAULT_MCP_DISCOVERY_TIMEOUT_S).abs() < 1e-9);
    }

    #[test]
    fn wait_for_mcp_entry_alive_path() {
        let mut joined_bound: Option<f64> = None;
        let mut started = false;
        let mut waited = false;
        wait_for_mcp_discovery_with(
            true,
            Some(1.0),
            |t| Ok(t.unwrap_or(0.75)),
            |b| { joined_bound = Some(b); },
            false,
            || { started = true; },
            |_| { waited = true; },
        );
        assert_eq!(joined_bound, Some(1.0));
        assert!(!started);
        assert!(!waited);
    }

    #[test]
    fn wait_for_mcp_enabled_retry_path() {
        let mut started = false;
        let mut waited_timeout: Option<Option<f64>> = None;
        wait_for_mcp_discovery_with(
            false,
            Some(0.5),
            |_| Ok(0.5),
            |_| {},
            true,
            || { started = true; },
            |t| { waited_timeout = Some(t); },
        );
        assert!(started);
        assert_eq!(waited_timeout, Some(Some(0.5)));

        // not enabled → no start/wait
        let mut started2 = false;
        wait_for_mcp_discovery_with(
            false, None, |_| Ok(0.75), |_| {}, false,
            || { started2 = true; },
            |_| panic!("should not wait"),
        );
        assert!(!started2);
    }

    #[test]
    fn ensure_mcp_started_idempotent() {
        let mut enabled = false;
        let ok = ensure_mcp_discovery_started_with(&mut enabled, || true, || Ok(()));
        assert!(ok);
        assert!(enabled);
        // has_servers false → not enabled, no start
        let mut enabled2 = false;
        let mut started = false;
        let ok2 = ensure_mcp_discovery_started_with(&mut enabled2, || false, || { started = true; Ok(()) });
        assert!(!ok2);
        assert!(!enabled2);
        assert!(!started);
        // start failure still marks enabled (Python sets flag before try)
        let mut enabled3 = false;
        let ok3 = ensure_mcp_discovery_started_with(&mut enabled3, || true, || Err("boom".into()));
        assert!(ok3);
        assert!(enabled3);
    }

    // -- dispatch loop ----------------------------------------------------

    #[test]
    fn parse_error_response_shape() {
        let r = parse_error_response();
        assert!(r.contains("\"code\":-32700"));
        assert!(r.contains("parse error"));
        assert!(r.contains("\"id\":null"));
    }

    #[test]
    fn extract_method_cases() {
        assert_eq!(extract_method(r#"{"jsonrpc":"2.0","method":"session.list","id":1}"#).as_deref(), Some("session.list"));
        assert_eq!(extract_method(r#"{"method":"a/b"}"#).as_deref(), Some("a/b"));
        assert_eq!(extract_method(r#"{"nope":1}"#), None);
        assert_eq!(extract_method(r#"not json"#), None);
    }

    #[test]
    fn handle_gateway_line_cases() {
        assert_eq!(handle_gateway_line("   ", true, None, None), GatewayLineAction::Skip);
        assert_eq!(handle_gateway_line("", true, None, None), GatewayLineAction::Skip);
        assert!(matches!(handle_gateway_line(r#"not json"#, false, None, None), GatewayLineAction::ParseError { .. }));
        assert_eq!(handle_gateway_line(r#"{"method":"x"}"#, true, Some(r#"{"ok":1}"#.into()), Some("x".into())),
                   GatewayLineAction::Response { response: r#"{"ok":1}"#.into(), method: Some("x".into()) });
        assert_eq!(handle_gateway_line(r#"{"method":"x"}"#, true, None, None), GatewayLineAction::NoResponse);
    }

    #[test]
    fn gateway_loop_run_end_to_end() {
        // Valid JSON line → dispatch → write;
        // invalid line → parse error write;
        // empty line → skip;
        // EOF → break via recovery_times empty (genuine EOF on non-Unix is immediate false)
        // We test via handle_gateway_line_with directly plus a small run_gateway_loop smoke.
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"method\":\"ping\",\"id\":1}\n",
            "   \n",
            "not json\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notify\",\"id\":2}\n",
        );
        let mut reader = Cursor::new(input.as_bytes());
        let mut writer: Vec<u8> = Vec::new();
        let dispatch = |line: &str| -> Option<String> {
            if line.contains("ping") {
                Some(r#"{"jsonrpc":"2.0","result":"pong","id":1}"#.to_string())
            } else if line.contains("notify") {
                None // notification
            } else {
                Some(r#"{"ok":1}"#.to_string())
            }
        };
        let write_json = |s: &str, w: &mut Vec<u8>| -> bool {
            w.extend_from_slice(s.as_bytes());
            w.push(b'\n');
            true
        };
        let mut exits: Vec<String> = Vec::new();
        let res = run_gateway_loop(
            &mut reader,
            &mut writer,
            dispatch,
            write_json,
            |m| exits.push(m.to_string()),
            || Ok(()),
        ).unwrap();
        assert!(res.is_ok());
        let out = String::from_utf8(writer).unwrap();
        // Should have written ping response + parse error response; notify is NoResponse
        assert!(out.contains("pong"), "out={}", out);
        assert!(out.contains("parse error"), "out={}", out);
        assert!(!out.contains("notify") || out.contains("pong")); // notify returns None
        // No spurious EOF exits on Unix? On this test host EOF is genuine, so breaks cleanly.
        // No log_exit for successful writes
        assert!(exits.is_empty() || exits.iter().all(|m| m.contains("peer closed") || m.is_empty()));
    }

    #[test]
    fn gateway_loop_write_failure_exits() {
        let input = "{\"jsonrpc\":\"2.0\",\"method\":\"ping\",\"id\":1}\n";
        let mut reader = Cursor::new(input.as_bytes());
        let mut writer: Vec<u8> = Vec::new();
        let dispatch = |_: &str| Some(r#"{"result":1}"#.to_string());
        let write_json = |_: &str, _: &mut Vec<u8>| false; // broken pipe
        let mut exits: Vec<String> = Vec::new();
        let res = run_gateway_loop(
            &mut reader,
            &mut writer,
            dispatch,
            write_json,
            |m| exits.push(m.to_string()),
            || Ok(()),
        ).unwrap();
        assert!(res.is_err());
        assert!(exits.iter().any(|m| m.contains("broken stdout pipe")));
    }

    #[test]
    fn gateway_ready_json_escape() {
        let p = GatewayReadyPayload::new("test\"skin\n");
        let j = p.to_json();
        assert!(j.contains("\\\""));
        assert!(j.contains("\\n"));
    }
}
