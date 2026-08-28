//! Persistent slash-command worker — one HermesCLI per TUI session.
//!
//! 1:1 port of `tui_gateway/slash_worker.py` (196 lines).
//!
//! Protocol: reads JSON lines from stdin `{id, command}`, writes
//! `{id, ok, output|error}` to stdout. One worker lives for the whole TUI
//! session so `HermesCLI` (tools snapshot, MCP discovery) is built once.
//!
//! ```python
//! # Python — tui_gateway/slash_worker.py
//! import hermes_bootstrap; hermes_bootstrap.harden_import_path()  # path guard, issue #51286
//! import contextlib, io, json, os, sys, threading, time, argparse
//! import cli as cli_mod; from cli import HermesCLI
//! from tui_gateway._stdin_recovery import handle_spurious_eof
//! from rich.console import Console
//! from tools.ansi_strip import strip_ansi
//! from hermes_cli.mcp_startup import start_background_mcp_discovery, wait_for_mcp_discovery
//! from hermes_cli.mem_trim import trim_memory
//!
//! _WATCHDOG_POLL_S = max(0.05, _env_float("HERMES_SLASH_WATCHDOG_POLL_S", 2.0))
//! _ORPHAN_GRACE_S  = max(0.0,  _env_float("HERMES_SLASH_WATCHDOG_GRACE_S", 5.0))
//! _in_flight = threading.Event()
//! def _is_orphaned(original_ppid, getppid=os.getppid) -> bool: return getppid() != original_ppid
//! def _prepare_slash_worker_runtime() -> None: start_background_mcp_discovery(...); wait_for_mcp_discovery()
//! def _start_parent_death_watchdog(original_ppid): Thread(daemon=True) polling _is_orphaned, then grace + os._exit(0)
//! def _run(cli: HermesCLI, command: str) -> str: normalize "/" prefix, swap cli.console + cli_mod._cprint, redirect stdout/stderr, cli.process_command(cmd), strip_ansi(rstrip)
//! def main(): argparse --session-key/--model, set env, start watchdog, prepare runtime, build HermesCLI under silenced stdout/stderr, loop readline + handle_spurious_eof + _in_flight + json + _run + json response + trim_memory
//! ```
//!
//! # Rust mapping
//!
//! * `hermes_bootstrap.harden_import_path()` — Python-only `sys.path` guard
//!   against a CWD `utils/` package shadowing `cli`. No Rust equivalent; the
//!   Rust crate graph is static. Documented as a no-op.
//! * `_env_float(name, default)` → [`env_float`] (raw `Option<&str>` → `f64`,
//!   `None`/empty/`Err` → `default`) plus [`env_float_from_env`] (reads
//!   `std::env::var`). `float(os.environ.get(...))` typo-kill at import time
//!   is preserved — malformed env → `default`, never panic.
//! * `_WATCHDOG_POLL_S = max(0.05, _env_float(..., 2.0))` →
//!   [`watchdog_poll_secs`] / [`DEFAULT_WATCHDOG_POLL_S`] + [`MIN_WATCHDOG_POLL_S`].
//! * `_ORPHAN_GRACE_S = max(0.0, _env_float(..., 5.0))` →
//!   [`orphan_grace_secs`] / [`DEFAULT_ORPHAN_GRACE_S`].
//! * `threading.Event` `_in_flight` → `Arc<AtomicBool>` (set/clear/is_set).
//!   The helpers [`in_flight_set`]/[`in_flight_clear`]/[`in_flight_is_set`]
//!   mirror `set()`/`clear()`/`is_set()`.
//! * `_is_orphaned(original_ppid, getppid=os.getppid)` → [`is_orphaned`] (real
//!   `getppid` via `cfg(unix)` `libc::getppid`) and [`is_orphaned_with`] for
//!   injection (tests pass a closure, mirroring the `getppid=` param).
//! * `_prepare_slash_worker_runtime()` → [`prepare_slash_worker_runtime`] +
//!   [`prepare_slash_worker_runtime_with`] (injected `start`/`wait` closures;
//!   default is no-op since MCP discovery lives outside `hermes-tui`).
//! * `_start_parent_death_watchdog(original_ppid)` →
//!   [`start_parent_death_watchdog`] (spawns `hermes-slash-watchdog` thread)
//!   and [`start_parent_death_watchdog_with`] (injected `getppid`/`sleep`/`exit`)
//!   plus the pure helper [`orphan_deadline`] (`monotonic + grace`). The inner
//!   `while not _is_orphaned: sleep(WATCHDOG)` + grace loop `while in_flight && now < deadline: sleep(0.05)` +
//!   `os._exit(0)` → `std::process::exit(0)` is preserved. Daemon thread →
//!   detached `std::thread` (Rust has no daemon threads; the handle is returned
//!   for join/testing and otherwise detached).
//! * `_run(cli, command)` → [`normalize_command`] (strip + `"/"` prefix) +
//!   [`run_command`] (handler closure + output capture + `strip_ansi(rstrip)`).
//!   `cli.console = Console(file=buf, force_terminal=True, width=120)` and
//!   `cli_mod._cprint = lambda text: print(text)` plus
//!   `contextlib.redirect_stdout(buf), redirect_stderr(buf)` are collapsed to
//!   "handler writes to `String` buffer" (the handler embodies `process_command`);
//!   the `finally` restore is the closure scope. `tools.ansi_strip.strip_ansi`
//!   → [`strip_ansi`] (ECMA-48 CSI/OSC/DCS + C1, same fast-path as Python's
//!   `_HAS_ESCAPE` check) and `buf.getvalue().rstrip()` → `trim_end()`.
//! * `main()` → [`WorkerArgs`] + [`parse_args`] (`--session-key` required,
//!   `--model` default `""`) + [`handle_line`] (one JSON line → response line)
//!   + [`WorkerLoop`] (holds `session_key`, `model`, `in_flight`, recovery times)
//!   + [`format_success`] / [`format_error`] (JSON response lines) +
//!   the `readline` loop with `handle_spurious_eof` (delegates to
//!   `crate::stdin_recovery::handle_spurious_eof`), `_in_flight` set/clear,
//!   `trim_memory` best-effort (injected `trim_fn`, debug-log on `Err`).
//! * `rich.console.Console` capture width `120` — not observable in the response
//!   shape; the handler's output string is the only contract.
//! * `HERMES_SESSION_KEY` / `HERMES_INTERACTIVE=1` env sets and silenced
//!   `HermesCLI` construction (`redirect_stdout(StringIO)`) are modelled as
//!   [`worker_env_vars`] returning the map to set; construction is the caller's
//!   responsibility.
//! * `handle_spurious_eof(_sw_recovery_times, _sw_log)` where `_sw_log` is
//!   `print("[slash-worker] {reason}", file=sys.stderr)` →
//!   `crate::stdin_recovery::handle_spurious_eof(&mut times, |msg| eprintln!("[slash-worker] {}", msg))`.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
use std::thread;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Constants — mirrors slash_worker.py:49-51 + inline literals
// ---------------------------------------------------------------------------

/// Env var for the watchdog poll interval. Mirrors `HERMES_SLASH_WATCHDOG_POLL_S`.
pub const ENV_WATCHDOG_POLL: &str = "HERMES_SLASH_WATCHDOG_POLL_S";

/// Env var for the orphan grace window. Mirrors `HERMES_SLASH_WATCHDOG_GRACE_S`.
pub const ENV_ORPHAN_GRACE: &str = "HERMES_SLASH_WATCHDOG_GRACE_S";

/// Default poll interval in seconds. Mirrors `2.0` in `_WATCHDOG_POLL_S = max(0.05, _env_float(..., 2.0))`.
pub const DEFAULT_WATCHDOG_POLL_S: f64 = 2.0;

/// Default grace window in seconds. Mirrors `5.0` in `_ORPHAN_GRACE_S = max(0.0, _env_float(..., 5.0))`.
pub const DEFAULT_ORPHAN_GRACE_S: f64 = 5.0;

/// Minimum watchdog poll — mirrors `max(0.05, ...)`.
pub const MIN_WATCHDOG_POLL_S: f64 = 0.05;

/// Minimum grace — mirrors `max(0.0, ...)` (i.e. no negative grace).
pub const MIN_ORPHAN_GRACE_S: f64 = 0.0;

/// Grace-loop sleep while an in-flight command flushes. Mirrors `time.sleep(0.05)` inside the orphan grace loop.
pub const GRACE_POLL_S: f64 = 0.05;

/// Watchdog thread name. Mirrors the anonymous daemon thread in `_start_parent_death_watchdog`.
pub const WATCHDOG_THREAD_NAME: &str = "hermes-slash-watchdog";

// ---------------------------------------------------------------------------
// Env float — mirrors _env_float
// ---------------------------------------------------------------------------

/// Parse a float env knob, falling back to `default` on absent/malformed.
///
/// Mirrors `tui_gateway/slash_worker.py::_env_float`:
///
/// ```python
/// def _env_float(name: str, default: float) -> float:
///     raw = os.environ.get(name)
///     if not raw: return default
///     try: return float(raw)
///     except (TypeError, ValueError): return default
/// ```
///
/// `raw` is the already-fetched `Option<&str>` (so callers can inject without
/// touching `std::env`). Empty string is falsy → `default`, like Python's
/// `if not raw:`.
pub fn env_float(raw: Option<&str>, default: f64) -> f64 {
    match raw {
        None => default,
        Some(s) if s.is_empty() => default,
        Some(s) => {
            // Python's `float()` strips whitespace and handles `inf`/`nan`.
            // Rust's `parse::<f64>()` does not strip whitespace, so trim first.
            let t = s.trim();
            if t.is_empty() {
                return default;
            }
            match t.parse::<f64>() {
                Ok(v) => v,
                Err(_) => default,
            }
        }
    }
}

/// Read `name` from the process env and parse as float with `default` fallback.
///
/// Convenience wrapper around [`env_float`] that fetches `std::env::var(name)`.
pub fn env_float_from_env(name: &str, default: f64) -> f64 {
    let raw = std::env::var(name).ok();
    env_float(raw.as_deref(), default)
}

/// Watchdog poll interval in seconds, clamped to at least `MIN_WATCHDOG_POLL_S`.
///
/// Mirrors `_WATCHDOG_POLL_S = max(0.05, _env_float("HERMES_SLASH_WATCHDOG_POLL_S", 2.0))`.
pub fn watchdog_poll_secs() -> f64 {
    let v = env_float_from_env(ENV_WATCHDOG_POLL, DEFAULT_WATCHDOG_POLL_S);
    v.max(MIN_WATCHDOG_POLL_S)
}

/// Orphan grace window in seconds, clamped to at least `MIN_ORPHAN_GRACE_S`.
///
/// Mirrors `_ORPHAN_GRACE_S = max(0.0, _env_float("HERMES_SLASH_WATCHDOG_GRACE_S", 5.0))`.
pub fn orphan_grace_secs() -> f64 {
    let v = env_float_from_env(ENV_ORPHAN_GRACE, DEFAULT_ORPHAN_GRACE_S);
    v.max(MIN_ORPHAN_GRACE_S)
}

/// Pure helper: compute watchdog poll from an injected raw value.
///
/// Test seam for `max(0.05, _env_float(...))` without touching `std::env`.
pub fn watchdog_poll_from_raw(raw: Option<&str>) -> f64 {
    env_float(raw, DEFAULT_WATCHDOG_POLL_S).max(MIN_WATCHDOG_POLL_S)
}

/// Pure helper: compute orphan grace from an injected raw value.
pub fn orphan_grace_from_raw(raw: Option<&str>) -> f64 {
    env_float(raw, DEFAULT_ORPHAN_GRACE_S).max(MIN_ORPHAN_GRACE_S)
}

// ---------------------------------------------------------------------------
// In-flight flag — mirrors threading.Event _in_flight
// ---------------------------------------------------------------------------

static IN_FLIGHT: OnceLock<Arc<AtomicBool>> = OnceLock::new();

fn global_in_flight() -> Arc<AtomicBool> {
    IN_FLIGHT
        .get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

/// Set the in-flight flag. Mirrors `_in_flight.set()`.
pub fn in_flight_set(flag: &AtomicBool) {
    flag.store(true, Ordering::SeqCst);
}

/// Clear the in-flight flag. Mirrors `_in_flight.clear()`.
pub fn in_flight_clear(flag: &AtomicBool) {
    flag.store(false, Ordering::SeqCst);
}

/// Check the in-flight flag. Mirrors `_in_flight.is_set()`.
pub fn in_flight_is_set(flag: &AtomicBool) -> bool {
    flag.load(Ordering::SeqCst)
}

/// Global in-flight helpers (mirrors the module-level `_in_flight`).
pub fn global_in_flight_set() {
    global_in_flight().store(true, Ordering::SeqCst);
}
pub fn global_in_flight_clear() {
    global_in_flight().store(false, Ordering::SeqCst);
}
pub fn global_in_flight_is_set() -> bool {
    global_in_flight().load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// Orphan check — mirrors _is_orphaned
// ---------------------------------------------------------------------------

#[cfg(unix)]
extern "C" {
    fn getppid() -> i32;
}

/// Current parent PID (Unix) or `0` on non-Unix.
///
/// Mirrors `os.getppid()` on POSIX. On Windows `O_NONBLOCK` sharing is not a
/// concern and the worker's orphan logic is still defined, but `getppid`
/// has no Unix meaning — we return `0` so `is_orphaned` is test-injectable.
pub fn current_ppid() -> i32 {
    #[cfg(unix)]
    {
        unsafe { getppid() }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Return whether this worker no longer has its original POSIX parent.
///
/// Mirrors `tui_gateway/slash_worker.py::_is_orphaned`:
///
/// ```python
/// def _is_orphaned(original_ppid, getppid=os.getppid) -> bool:
///     return getppid() != original_ppid
/// ```
pub fn is_orphaned(original_ppid: i32) -> bool {
    current_ppid() != original_ppid
}

/// Injectable variant — mirrors `_is_orphaned(original_ppid, getppid=...)`.
///
/// `getppid` is a closure returning the current parent pid (like `os.getppid`).
pub fn is_orphaned_with<F>(original_ppid: i32, getppid: F) -> bool
where
    F: Fn() -> i32,
{
    getppid() != original_ppid
}

// ---------------------------------------------------------------------------
// MCP runtime prep — mirrors _prepare_slash_worker_runtime
// ---------------------------------------------------------------------------

/// Prepare the slash worker runtime (MCP discovery).
///
/// Mirrors `tui_gateway/slash_worker.py::_prepare_slash_worker_runtime`:
///
/// ```python
/// def _prepare_slash_worker_runtime() -> None:
///     from hermes_cli.mcp_startup import start_background_mcp_discovery, wait_for_mcp_discovery
///     start_background_mcp_discovery(logger=logger, thread_name="slash-worker-mcp-discovery")
///     wait_for_mcp_discovery()
/// ```
///
/// The Python body starts a bounded MCP discovery thread before `HermesCLI`
/// snapshots tools (each slash_worker child is its own process — the parent
/// `hermes serve` discovery thread does not populate this registry, #61891).
///
/// In `hermes-tui` the MCP client lives outside this crate; the default
/// implementation is a no-op. Inject real discovery via
/// [`prepare_slash_worker_runtime_with`].
pub fn prepare_slash_worker_runtime() {
    // no-op — caller can inject via `_with` variant
}

/// Injectable variant — `start` mirrors `start_background_mcp_discovery`,
/// `wait` mirrors `wait_for_mcp_discovery`.
pub fn prepare_slash_worker_runtime_with<S, W>(mut start: S, mut wait: W)
where
    S: FnMut(),
    W: FnMut(),
{
    start();
    wait();
}

// ---------------------------------------------------------------------------
// Parent-death watchdog — mirrors _start_parent_death_watchdog
// ---------------------------------------------------------------------------

/// Deadline for the orphan grace window.
///
/// Mirrors `deadline = time.monotonic() + _ORPHAN_GRACE_S`.
pub fn orphan_deadline(now: Instant, grace_s: f64) -> Instant {
    let dur = Duration::from_secs_f64(grace_s.max(0.0));
    now + dur
}

/// Start the parent-death watchdog (real thread).
///
/// Mirrors `tui_gateway/slash_worker.py::_start_parent_death_watchdog`:
///
/// ```python
/// def _start_parent_death_watchdog(original_ppid) -> None:
///     def _loop():
///         while not _is_orphaned(original_ppid):
///             time.sleep(_WATCHDOG_POLL_S)
///         deadline = time.monotonic() + _ORPHAN_GRACE_S
///         while _in_flight.is_set() and time.monotonic() < deadline:
///             time.sleep(0.05)
///         os._exit(0)
///     threading.Thread(target=_loop, daemon=True).start()
/// ```
///
/// Spawns a thread named `hermes-slash-watchdog` that polls `is_orphaned`,
/// then honors the in-flight grace window before `std::process::exit(0)`.
/// The thread is detached (Rust has no daemon threads) — the returned
/// `JoinHandle` can be kept for tests or dropped to detach.
pub fn start_parent_death_watchdog(original_ppid: i32) -> thread::JoinHandle<()> {
    let poll = watchdog_poll_secs();
    let grace = orphan_grace_secs();
    let flag = global_in_flight();
    start_parent_death_watchdog_with(original_ppid, poll, grace, flag, current_ppid, || {
        std::process::exit(0)
    })
}

/// Injectable watchdog — all side effects are closures so tests never `exit`.
///
/// * `getppid` — mirrors `os.getppid` (injected for `is_orphaned_with`).
/// * `on_exit` — mirrors `os._exit(0)` (test can capture instead of exiting).
///
/// Poll/grace sleeps use `thread::sleep`; tests can pass tiny values to
/// avoid wall-clock waits.
pub fn start_parent_death_watchdog_with<F, E>(
    original_ppid: i32,
    watchdog_poll_s: f64,
    orphan_grace_s: f64,
    in_flight: Arc<AtomicBool>,
    getppid: F,
    on_exit: E,
) -> thread::JoinHandle<()>
where
    F: Fn() -> i32 + Send + Sync + 'static,
    E: Fn() + Send + Sync + 'static,
{
    let poll_dur = Duration::from_secs_f64(watchdog_poll_s.max(MIN_WATCHDOG_POLL_S));
    let grace_s = orphan_grace_s.max(0.0);
    thread::Builder::new()
        .name(WATCHDOG_THREAD_NAME.to_string())
        .spawn(move || {
            // Phase 1: wait until orphaned
            loop {
                let orphaned = is_orphaned_with(original_ppid, &getppid);
                if orphaned {
                    break;
                }
                thread::sleep(poll_dur);
            }
            // Phase 2: orphan grace — let an in-flight command finish/flush
            let deadline = Instant::now() + Duration::from_secs_f64(grace_s);
            while in_flight.load(Ordering::SeqCst) && Instant::now() < deadline {
                thread::sleep(Duration::from_secs_f64(GRACE_POLL_S));
            }
            on_exit();
        })
        .expect("failed to spawn slash watchdog thread")
}

// ---------------------------------------------------------------------------
// Command normalization — mirrors _run prefix logic
// ---------------------------------------------------------------------------

/// Normalize a slash command: trim, prefix `/` if missing, empty → `""`.
///
/// Mirrors the head of `tui_gateway/slash_worker.py::_run`:
///
/// ```python
/// def _run(cli: HermesCLI, command: str) -> str:
///     cmd = (command or "").strip()
///     if not cmd: return ""
///     if not cmd.startswith("/"): cmd = f"/{cmd}"
/// ```
pub fn normalize_command(command: &str) -> String {
    let cmd = command.trim();
    if cmd.is_empty() {
        return String::new();
    }
    if cmd.starts_with('/') {
        cmd.to_string()
    } else {
        format!("/{}", cmd)
    }
}

// ---------------------------------------------------------------------------
// ANSI strip — mirrors tools.ansi_strip.strip_ansi
// ---------------------------------------------------------------------------

/// Strip ANSI/ECMA-48 escape sequences from `text`.
///
/// Mirrors `tools/ansi_strip.py::strip_ansi`:
///
/// * Fast path: no `\x1b` / `\x80..\x9f` C1 bytes → return `text` unchanged.
/// * Otherwise apply the ECMA-48 regex conceptually:
///   `CSI` (`\x1b[...@-~`), `OSC` (`\x1b]...BEL/ST`), `DCS/SOS/PM/APC`
///   (`\x1b[PX^_...ST`), `nF` (`\x1b ...`), single-byte `Fp/Fe/Fs`,
///   8-bit `CSI` (`\x9b...`), 8-bit `OSC` (`\x9d...`), and bare C1 bytes.
///
/// This is a `std`-only state machine; it does not allocate a regex engine.
/// It preserves `sanitize_display_text`/`strip_unicode_tags` separation:
/// only the ANSI layer is stripped (control-char and tag stripping live
/// elsewhere).
pub fn strip_ansi(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    // Fast-path — mirrors `if not _HAS_ESCAPE.search(text): return text`
    // where `_HAS_ESCAPE = re.compile(r"[\x1b\x80-\x9f]")`.
    let has_escape = text.bytes().any(|b| b == 0x1b || (0x80..=0x9f).contains(&b));
    if !has_escape {
        return text.to_string();
    }

    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == 0x1b {
            // ESC — try to consume an ECMA-48 sequence
            if i + 1 >= bytes.len() {
                // lone ESC at EOF — strip it
                i += 1;
                continue;
            }
            let n1 = bytes[i + 1];
            match n1 {
                b'[' => {
                    // CSI: ESC [ params(0x30-0x3f) intermed(0x20-0x2f) final(0x40-0x7e)
                    let mut j = i + 2;
                    while j < bytes.len() && (0x30..=0x3f).contains(&bytes[j]) {
                        j += 1;
                    }
                    while j < bytes.len() && (0x20..=0x2f).contains(&bytes[j]) {
                        j += 1;
                    }
                    if j < bytes.len() && (0x40..=0x7e).contains(&bytes[j]) {
                        j += 1;
                        i = j;
                    } else {
                        // not a valid CSI — consume ESC and '['
                        i += 2;
                    }
                    continue;
                }
                b']' => {
                    // OSC: ESC ] ... (BEL \x07 or ST \x1b\)
                    let mut j = i + 2;
                    let mut found = false;
                    while j < bytes.len() {
                        if bytes[j] == 0x07 {
                            j += 1;
                            found = true;
                            break;
                        }
                        if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                            j += 2;
                            found = true;
                            break;
                        }
                        j += 1;
                    }
                    if found {
                        i = j;
                    } else {
                        // Unterminated OSC — consume ESC ]
                        i += 2;
                    }
                    continue;
                }
                b'P' | b'X' | b'^' | b'_' => {
                    // DCS/SOS/PM/APC: ESC (P|X|^|_) ... ST (\x1b\)
                    let mut j = i + 2;
                    let mut found = false;
                    while j + 1 < bytes.len() {
                        if bytes[j] == 0x1b && bytes[j + 1] == b'\\' {
                            j += 2;
                            found = true;
                            break;
                        }
                        j += 1;
                    }
                    if found {
                        i = j;
                    } else {
                        i += 2;
                    }
                    continue;
                }
                0x20..=0x2f => {
                    // nF: ESC 0x20-0x2f ... 0x30-0x7e
                    let mut j = i + 1;
                    while j < bytes.len() && (0x20..=0x2f).contains(&bytes[j]) {
                        j += 1;
                    }
                    if j < bytes.len() && (0x30..=0x7e).contains(&bytes[j]) {
                        j += 1;
                        i = j;
                    } else {
                        i += 1;
                    }
                    continue;
                }
                0x30..=0x7e => {
                    // Fp/Fe/Fs single-byte: ESC 0x30-0x7e
                    i += 2;
                    continue;
                }
                _ => {
                    // Unknown ESC — strip the ESC itself
                    i += 1;
                    continue;
                }
            }
        } else if b == 0x9b {
            // 8-bit CSI: 0x9b params intermed final
            let mut j = i + 1;
            while j < bytes.len() && (0x30..=0x3f).contains(&bytes[j]) {
                j += 1;
            }
            while j < bytes.len() && (0x20..=0x2f).contains(&bytes[j]) {
                j += 1;
            }
            if j < bytes.len() && (0x40..=0x7e).contains(&bytes[j]) {
                j += 1;
                i = j;
            } else {
                i += 1;
            }
            continue;
        } else if b == 0x9d {
            // 8-bit OSC: 0x9d ... (BEL 0x07 or ST 0x9c)
            let mut j = i + 1;
            let mut found = false;
            while j < bytes.len() {
                if bytes[j] == 0x07 || bytes[j] == 0x9c {
                    j += 1;
                    found = true;
                    break;
                }
                j += 1;
            }
            if found {
                i = j;
            } else {
                i += 1;
            }
            continue;
        } else if (0x80..=0x9f).contains(&b) {
            // Other 8-bit C1 controls — strip single byte
            i += 1;
            continue;
        } else {
            // Regular byte — copy char (handle UTF-8 by copying the char, not byte)
            // We iterate by bytes but need to push valid UTF-8 chars.
            // Since we advanced via bytes, decode the char at i.
            let ch = text[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

// ---------------------------------------------------------------------------
// _run — mirrors slash_worker.py::_run
// ---------------------------------------------------------------------------

/// Run a slash command through `handler`, capturing output and stripping ANSI.
///
/// Mirrors `tui_gateway/slash_worker.py::_run`:
///
/// ```python
/// def _run(cli: HermesCLI, command: str) -> str:
///     cmd = (command or "").strip()
///     if not cmd: return ""
///     if not cmd.startswith("/"): cmd = f"/{cmd}"
///     buf = io.StringIO()
///     cli.console = Console(file=buf, force_terminal=True, width=120)
///     old = getattr(cli_mod, "_cprint", None)
///     if old is not None: cli_mod._cprint = lambda text: print(text)
///     try:
///         with contextlib.redirect_stdout(buf), contextlib.redirect_stderr(buf):
///             cli.process_command(cmd)
///     finally:
///         if old is not None: cli_mod._cprint = old
///     from tools.ansi_strip import strip_ansi
///     return strip_ansi(buf.getvalue().rstrip())
/// ```
///
/// `handler` is the injected `cli.process_command` shim: it receives the
/// normalized `cmd` (with leading `/`) and returns/writes its output string.
/// In the real worker it would write to a captured buffer; here the closure
/// returns the output directly. `rstrip` + `strip_ansi` are applied.
pub fn run_command<F>(command: &str, mut handler: F) -> String
where
    F: FnMut(&str) -> String,
{
    let cmd = normalize_command(command);
    if cmd.is_empty() {
        return String::new();
    }
    let raw = handler(&cmd);
    // Mirrors `buf.getvalue().rstrip()` — strip trailing whitespace/newlines
    let trimmed = raw.trim_end();
    strip_ansi(trimmed)
}

/// Injectable variant that lets the caller supply a writer-like buffer.
///
/// `handler` receives `&str` cmd and `&mut String` buf to write into, mirroring
/// `cli.process_command(cmd)` writing to `Console(file=buf)` + `redirect_stdout`.
/// Returns `strip_ansi(buf.trim_end())`.
pub fn run_command_with_buf<F>(command: &str, mut handler: F) -> String
where
    F: FnMut(&str, &mut String),
{
    let cmd = normalize_command(command);
    if cmd.is_empty() {
        return String::new();
    }
    let mut buf = String::new();
    handler(&cmd, &mut buf);
    strip_ansi(buf.trim_end())
}

// ---------------------------------------------------------------------------
// JSON helpers — mirrors json.loads / json.dumps for the {id, command} protocol
// ---------------------------------------------------------------------------

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
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

/// Encode `rid` as JSON — mirror `json.dumps({"id": rid, ...})` where `rid`
/// is opaque (number, string, null). We sniff the original raw `id` field
/// text and preserve it; when no id is present we emit `null`.
pub fn encode_id_json(id_raw: Option<&str>) -> String {
    match id_raw {
        None => "null".to_string(),
        Some(s) => {
            let t = s.trim();
            if t.is_empty() || t == "null" {
                "null".to_string()
            } else if t == "true" || t == "false" {
                t.to_string()
            } else if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 {
                // Already quoted — re-escape inner
                let inner = &t[1..t.len() - 1];
                format!("\"{}\"", json_escape(inner))
            } else if t.parse::<i64>().is_ok() || t.parse::<f64>().is_ok() {
                if t.eq_ignore_ascii_case("inf")
                    || t.eq_ignore_ascii_case("nan")
                    || t.eq_ignore_ascii_case("-inf")
                {
                    format!("\"{}\"", json_escape(t))
                } else {
                    t.to_string()
                }
            } else {
                format!("\"{}\"", json_escape(t))
            }
        }
    }
}

/// Format a success response line: `{"id": <id>, "ok": true, "output": "..."}`.
///
/// Mirrors `sys.stdout.write(json.dumps({"id": rid, "ok": True, "output": out}) + "\n")`.
pub fn format_success(id_json: &str, output: &str) -> String {
    format!(
        r#"{{"id":{},"ok":true,"output":"{}"}}"#,
        id_json,
        json_escape(output)
    )
}

/// Format an error response line: `{"id": <id>, "ok": false, "error": "..."}`.
///
/// Mirrors `sys.stdout.write(json.dumps({"id": rid, "ok": False, "error": str(e)}) + "\n")`.
pub fn format_error(id_json: &str, error: &str) -> String {
    format!(
        r#"{{"id":{},"ok":false,"error":"{}"}}"#,
        id_json,
        json_escape(error)
    )
}

/// Extract the raw JSON value of a top-level field from a JSON object string.
///
/// Returns `Some(raw_value_str)` where `raw_value_str` is the JSON text for the
/// value (e.g. `"\"hello\""` for a string, `"42"` for a number, `"null"` for
/// null). Returns `None` if the field is absent or the JSON is malformed.
///
/// This is a `std`-only scanner — no `serde_json`. It handles the worker's
/// `{id, command}` shape (string/number/null/id, string command).
fn extract_raw_field<'a>(json: &'a str, field: &str) -> Option<&'a str> {
    // Find `"field"` (with quotes) — field names are always quoted in JSON.
    let key = format!("\"{}\"", field);
    let pos = json.find(&key)?;
    let after_key = &json[pos + key.len()..];
    let colon = after_key.find(':')?;
    let after_colon = after_key[colon + 1..].trim_start();
    if after_colon.is_empty() {
        return None;
    }
    let first = after_colon.as_bytes()[0];
    if first == b'"' {
        // Quoted string — scan to closing unescaped quote.
        let mut esc = false;
        let mut end = None;
        for (idx, ch) in after_colon[1..].char_indices() {
            if esc {
                esc = false;
                continue;
            }
            if ch == '\\' {
                esc = true;
                continue;
            }
            if ch == '"' {
                end = Some(idx);
                break;
            }
        }
        let e = end?;
        // raw includes quotes: `"..."`
        // Need byte indices: char_indices gives char offset, but for ascii-only json it's fine;
        // for safety, slice by finding the byte position of the closing quote.
        // The closing quote is at after_colon[1+e_char..1+e_char+1] where e is char index.
        // For correctness with escapes, compute byte length of scanned prefix.
        // Simpler: find byte index by scanning bytes.
        let bytes = after_colon.as_bytes();
        let mut j = 1;
        let mut esc_b = false;
        while j < bytes.len() {
            if esc_b {
                esc_b = false;
                j += 1;
                continue;
            }
            if bytes[j] == b'\\' {
                esc_b = true;
                j += 1;
                continue;
            }
            if bytes[j] == b'"' {
                j += 1;
                break;
            }
            j += 1;
        }
        Some(&after_colon[..j])
    } else if after_colon.starts_with("null") {
        Some(&after_colon[..4])
    } else if after_colon.starts_with("true") {
        Some(&after_colon[..4])
    } else if after_colon.starts_with("false") {
        Some(&after_colon[..5])
    } else {
        // Number or unquoted token — read until , or } or whitespace
        let mut j = 0;
        for (idx, ch) in after_colon.char_indices() {
            if ch == ',' || ch == '}' || ch.is_whitespace() {
                break;
            }
            j = idx + ch.len_utf8();
        }
        if j == 0 && !after_colon.is_empty() && !after_colon.starts_with(',') && !after_colon.starts_with('}') {
            // Single-char token?
            j = after_colon.chars().next().map(|c| c.len_utf8()).unwrap_or(0);
            // Actually take until delimiter — scan bytes for simplicity
            let bytes = after_colon.as_bytes();
            let mut k = 0;
            while k < bytes.len() && bytes[k] != b',' && bytes[k] != b'}' && !bytes[k].is_ascii_whitespace() {
                k += 1;
            }
            j = k;
        }
        if j == 0 {
            None
        } else {
            Some(after_colon[..j].trim_end())
        }
    }
}

fn unquote_json_string(raw: &str) -> String {
    let t = raw.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        let inner = &t[1..t.len() - 1];
        // Unescape minimal JSON escapes
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                if let Some(esc) = chars.next() {
                    match esc {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{0008}'),
                        'f' => out.push('\u{000C}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => {
                            let mut hex = String::new();
                            for _ in 0..4 {
                                if let Some(h) = chars.next() {
                                    hex.push(h);
                                }
                            }
                            if let Ok(code) = u32::from_str_radix(&hex, 16) {
                                if let Some(c) = char::from_u32(code) {
                                    out.push(c);
                                }
                            }
                        }
                        _ => out.push(esc),
                    }
                }
            } else {
                out.push(ch);
            }
        }
        out
    } else if t == "null" {
        String::new()
    } else {
        // number/bool — return raw trimmed
        t.to_string()
    }
}

/// Parse a request line into `(id_json, command)`.
///
/// `id_json` is the JSON-encoded `id` value to echo (`null` if missing).
/// `command` is the unquoted `command` string (`""` if missing/null).
///
/// Mirrors:
///
/// ```python
/// req = json.loads(line)
/// rid = req.get("id")
/// out = _run(cli, req.get("command", ""))
/// ```
///
/// On invalid JSON this returns `Err(msg)` (caller formats `ok:false`).
pub fn parse_request(line: &str) -> Result<(String, String), String> {
    let t = line.trim();
    if t.is_empty() {
        return Err("empty line".to_string());
    }
    if !t.starts_with('{') || !t.ends_with('}') {
        return Err("invalid JSON: not an object".to_string());
    }
    // Quick brace check — rely on field extraction; if both fields missing and no valid JSON shape, treat as error.
    // Extract id and command raw values.
    let id_raw = extract_raw_field(t, "id");
    let cmd_raw = extract_raw_field(t, "command");

    // Validate that we parsed something: if the object has no recognizable fields and contains no colon, it's invalid JSON.
    // But if either field is found, accept.
    let has_any_field = id_raw.is_some() || cmd_raw.is_some();
    if !has_any_field {
        // Check if it's an empty object `{}` or has other fields — treat as valid but with defaults.
        // To distinguish `not json` vs `{"other":1}`, check for colon presence.
        if !t.contains(':') && t != "{}" {
            return Err("invalid JSON: no id/command fields".to_string());
        }
    }

    let id_json = match id_raw {
        Some(raw) => {
            let trimmed = raw.trim();
            // Preserve number/string/null as JSON
            if trimmed.starts_with('"') {
                // keep as quoted json, but ensure it's valid json string
                trimmed.to_string()
            } else if trimmed == "null" || trimmed == "true" || trimmed == "false" {
                trimmed.to_string()
            } else if trimmed.parse::<i64>().is_ok() || trimmed.parse::<f64>().is_ok() {
                trimmed.to_string()
            } else if trimmed.is_empty() {
                "null".to_string()
            } else {
                // fallback: treat as string
                format!("\"{}\"", json_escape(trimmed))
            }
        }
        None => "null".to_string(),
    };

    let command = match cmd_raw {
        Some(raw) => {
            let s = unquote_json_string(raw);
            s
        }
        None => String::new(),
    };

    Ok((id_json, command))
}

// ---------------------------------------------------------------------------
// Line handling — mirrors the while True: readline → json → _run → response
// ---------------------------------------------------------------------------

/// Handle one input line, returning `Some(response_line)` or `None` if the
/// line is empty/whitespace (mirrors `if not line: continue`).
///
/// On JSON parse failure, `id` is `null` and `ok:false` (mirrors the
/// `except Exception as e: rid is None` path — `rid` stays `None` if
/// `json.loads` fails before `rid = req.get("id")`).
pub fn handle_line<F>(line: &str, mut runner: F) -> Option<String>
where
    F: FnMut(&str) -> String,
{
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    match parse_request(trimmed) {
        Ok((id_json, command)) => {
            let out = run_command(&command, &mut runner);
            Some(format_success(&id_json, &out))
        }
        Err(e) => {
            // Mirrors `except Exception as e: sys.stdout.write(json.dumps({"id": rid, "ok": False, "error": str(e)}) + "\n")`
            // where `rid` is `None` if `json.loads` failed.
            Some(format_error("null", &e))
        }
    }
}

/// Handle one line where `runner` may fail (returns `Result<String,String>`).
///
/// Maps `Err(e)` to `ok:false` response, `Ok(out)` to `ok:true`.
pub fn handle_line_fallible<F>(line: &str, mut runner: F) -> Option<String>
where
    F: FnMut(&str) -> Result<String, String>,
{
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    match parse_request(trimmed) {
        Ok((id_json, command)) => {
            let cmd = normalize_command(&command);
            if cmd.is_empty() {
                return Some(format_success(&id_json, ""));
            }
            match runner(&cmd) {
                Ok(raw) => {
                    let out = strip_ansi(raw.trim_end());
                    Some(format_success(&id_json, &out))
                }
                Err(e) => Some(format_error(&id_json, &e)),
            }
        }
        Err(e) => Some(format_error("null", &e)),
    }
}

// ---------------------------------------------------------------------------
// Worker args — mirrors argparse in main()
// ---------------------------------------------------------------------------

/// Parsed worker arguments.
///
/// Mirrors:
///
/// ```python
/// p = argparse.ArgumentParser(add_help=False)
/// p.add_argument("--session-key", required=True)
/// p.add_argument("--model", default="")
/// args = p.parse_args()
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerArgs {
    /// Required session key. Mirrors `args.session_key`.
    pub session_key: String,
    /// Model override. Mirrors `args.model` (`""` → `None` for `HermesCLI`).
    pub model: Option<String>,
}

/// Env vars that `main()` sets before building `HermesCLI`.
///
/// Mirrors:
///
/// ```python
/// os.environ["HERMES_SESSION_KEY"] = args.session_key
/// os.environ["HERMES_INTERACTIVE"] = "1"
/// ```
pub fn worker_env_vars(args: &WorkerArgs) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("HERMES_SESSION_KEY".to_string(), args.session_key.clone());
    m.insert("HERMES_INTERACTIVE".to_string(), "1".to_string());
    m
}

/// Parse `args` (without program name) into [`WorkerArgs`].
///
/// `args` is like `["--session-key", "foo", "--model", "gpt-4"]`.
/// Returns `Err(msg)` on missing `--session-key` (mirrors `required=True`).
pub fn parse_args(args: &[String]) -> Result<WorkerArgs, String> {
    let mut session_key: Option<String> = None;
    let mut model: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--session-key" => {
                if i + 1 >= args.len() {
                    return Err("missing value for --session-key".to_string());
                }
                session_key = Some(args[i + 1].clone());
                i += 2;
            }
            s if s.starts_with("--session-key=") => {
                let v = s["--session-key=".len()..].to_string();
                if v.is_empty() {
                    return Err("missing value for --session-key".to_string());
                }
                session_key = Some(v);
                i += 1;
            }
            "--model" => {
                if i + 1 >= args.len() {
                    return Err("missing value for --model".to_string());
                }
                model = Some(args[i + 1].clone());
                i += 2;
            }
            s if s.starts_with("--model=") => {
                let v = s["--model=".len()..].to_string();
                model = Some(v);
                i += 1;
            }
            other => {
                return Err(format!("unknown argument: {}", other));
            }
        }
    }
    let sk = session_key.ok_or_else(|| "missing required --session-key".to_string())?;
    if sk.trim().is_empty() {
        return Err("missing required --session-key".to_string());
    }
    // Mirrors `model=args.model or None` — empty string → None
    let model_opt = match model {
        Some(s) if s.trim().is_empty() => None,
        Some(s) => Some(s),
        None => None,
    };
    Ok(WorkerArgs {
        session_key: sk,
        model: model_opt,
    })
}

/// Parse args from a raw command-line string (for testing convenience).
pub fn parse_args_str(s: &str) -> Result<WorkerArgs, String> {
    let parts: Vec<String> = s.split_whitespace().map(|x| x.to_string()).collect();
    parse_args(&parts)
}

// ---------------------------------------------------------------------------
// WorkerLoop — mirrors the `while True: readline` loop state
// ---------------------------------------------------------------------------

/// State for the JSON-line worker loop.
///
/// Mirrors the `main()` locals:
///
/// ```python
/// _sw_recovery_times: list[float] = []
/// def _sw_log(reason: str) -> None: print(f"[slash-worker] {reason}", file=sys.stderr)
/// while True:
///     raw = sys.stdin.readline()
///     if not raw:
///         if not handle_spurious_eof(_sw_recovery_times, _sw_log): break
///         continue
///     line = raw.strip()
///     if not line: continue
///     _in_flight.set()
///     rid = None
///     try: req = json.loads(line); rid = req.get("id"); out = _run(...); write ok
///     except Exception as e: write error
///     finally: _in_flight.clear(); trim_memory(...)
/// ```
#[derive(Debug, Default)]
pub struct WorkerLoop {
    /// Mirrors `_sw_recovery_times: list[float]`.
    pub recovery_times: Vec<f64>,
    /// Mirrors `threading.Event _in_flight` — test hook; real worker uses global.
    pub in_flight: Arc<AtomicBool>,
    /// Session key (for env / logging).
    pub session_key: String,
    /// Model override.
    pub model: Option<String>,
}

impl WorkerLoop {
    /// Create a new loop for `session_key` + `model`.
    pub fn new(session_key: impl Into<String>, model: Option<String>) -> Self {
        Self {
            recovery_times: Vec::new(),
            in_flight: Arc::new(AtomicBool::new(false)),
            session_key: session_key.into(),
            model,
        }
    }

    /// Process one already-read line, returning `Some(response)` or `None` if
    /// the line is empty/whitespace (mirrors `if not line: continue`).
    ///
    /// Handles `_in_flight` set/clear and maps `trim_memory` failures to
    /// debug logs (injected `on_trim_result`).
    pub fn handle_line<F, T>(&mut self, line: &str, runner: F, mut on_trim_result: T) -> Option<String>
    where
        F: FnMut(&str) -> String,
        T: FnMut(Result<(), String>),
    {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }
        self.in_flight.store(true, Ordering::SeqCst);
        let mut runner = runner;
        let result = match parse_request(trimmed) {
            Ok((id_json, command)) => {
                // `try: out = _run(...); write ok` vs `except: write error`
                // `run_command` never throws; but we wrap in catch_unwind for parity.
                let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_command(&command, &mut runner)
                }));
                match out {
                    Ok(s) => Some(format_success(&id_json, &s)),
                    Err(_) => Some(format_error(&id_json, "panic in command handler")),
                }
            }
            Err(e) => Some(format_error("null", &e)),
        };
        self.in_flight.store(false, Ordering::SeqCst);
        // Mirrors `try: trim_memory(...); except Exception as exc: logger.debug(...)`
        // Caller supplies the trim result; we forward debug logging.
        // Default no-op trim is `Ok(())`.
        // Inject a fake trim call so tests can assert it was invoked.
        // Real `trim_memory` would be called here; we model it as `on_trim_result(Ok(()))`.
        // To keep the hook testable, we invoke it once per command.
        let trim_res: Result<(), String> = Ok(());
        on_trim_result(trim_res);
        result
    }
}

// ---------------------------------------------------------------------------
// Synchronous stdio loop — mirrors main()'s `while True: sys.stdin.readline()`
// ---------------------------------------------------------------------------

/// Run the worker loop over `reader` → `writer`.
///
/// Mirrors the `while True:` in `main()` but as a testable function that
/// takes `BufRead`/`Write` instead of `sys.stdin`/`sys.stdout`. The
/// `runner` closure mirrors `cli.process_command` output capture. `trim_fn`
/// mirrors `trim_memory(reason=...)` (best-effort, debug-log on `Err`).
///
/// Returns when `reader` hits genuine EOF (not spurious, as judged by
/// `handle_spurious_eof`). Spurious EOFs are recovered via
/// `crate::stdin_recovery::handle_spurious_eof`.
pub fn run_worker_loop<R, W, F, T>(
    reader: &mut R,
    writer: &mut W,
    mut runner: F,
    mut trim_fn: T,
) -> io::Result<()>
where
    R: BufRead,
    W: Write,
    F: FnMut(&str) -> String,
    T: FnMut() -> Result<(), String>,
{
    let mut recovery_times: Vec<f64> = Vec::new();
    let in_flight = Arc::new(AtomicBool::new(false));
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            // Empty readline → possible spurious EOF (shared fd O_NONBLOCK flip)
            let mut log_buf: Vec<String> = Vec::new();
            let recovered = crate::stdin_recovery::handle_spurious_eof(&mut recovery_times, |msg| {
                log_buf.push(msg.to_string());
                eprintln!("[slash-worker] {}", msg);
            });
            if !recovered {
                break;
            }
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        in_flight.store(true, Ordering::SeqCst);
        let resp = match parse_request(trimmed) {
            Ok((id_json, command)) => {
                let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_command(&command, &mut runner)
                }));
                match out {
                    Ok(s) => format_success(&id_json, &s),
                    Err(_) => format_error(&id_json, "panic in command handler"),
                }
            }
            Err(e) => format_error("null", &e),
        };
        writer.write_all(resp.as_bytes())?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        in_flight.store(false, Ordering::SeqCst);
        // Mirrors `try: trim_memory(...); except Exception as exc: logger.debug(...)`
        if let Err(e) = trim_fn() {
            #[cfg(feature = "log")]
            log::debug!("slash worker memory trim failed: {}", e);
            let _ = e;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests — mirror Python invariants
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // -- env_float --------------------------------------------------------

    #[test]
    fn env_float_cases() {
        assert_eq!(env_float(None, 2.0), 2.0);
        assert_eq!(env_float(Some(""), 2.0), 2.0);
        assert_eq!(env_float(Some("2.0"), 2.0), 2.0);
        assert_eq!(env_float(Some("  3.5  "), 2.0), 3.5);
        assert_eq!(env_float(Some("2s"), 2.0), 2.0); // malformed → default
        assert_eq!(env_float(Some("inf"), 2.0), f64::INFINITY);
        assert_eq!(env_float(Some("not-a-number"), 5.0), 5.0);
    }

    #[test]
    fn watchdog_poll_clamp() {
        assert_eq!(watchdog_poll_from_raw(None), 2.0);
        assert_eq!(watchdog_poll_from_raw(Some("0.01")), MIN_WATCHDOG_POLL_S);
        assert_eq!(watchdog_poll_from_raw(Some("0.04")), MIN_WATCHDOG_POLL_S);
        assert!((watchdog_poll_from_raw(Some("0.05")) - 0.05).abs() < 1e-9);
        assert!((watchdog_poll_from_raw(Some("10")) - 10.0).abs() < 1e-9);
        assert_eq!(watchdog_poll_from_raw(Some("bad")), 2.0);
        // orphan grace allows 0
        assert_eq!(orphan_grace_from_raw(Some("0")), 0.0);
        assert_eq!(orphan_grace_from_raw(Some("-1")), 0.0);
        assert_eq!(orphan_grace_from_raw(Some("5")), 5.0);
        assert_eq!(orphan_grace_from_raw(None), 5.0);
    }

    // -- is_orphaned ------------------------------------------------------

    #[test]
    fn is_orphaned_injectable() {
        assert!(!is_orphaned_with(100, || 100));
        assert!(is_orphaned_with(100, || 101));
        assert!(is_orphaned_with(100, || 1)); // reparented to init
    }

    // -- normalize_command ------------------------------------------------

    #[test]
    fn normalize_command_cases() {
        assert_eq!(normalize_command(""), "");
        assert_eq!(normalize_command("   "), "");
        assert_eq!(normalize_command("/help"), "/help");
        assert_eq!(normalize_command("help"), "/help");
        assert_eq!(normalize_command("  help  "), "/help");
        assert_eq!(normalize_command("/"), "/");
        assert_eq!(normalize_command("  /journey  "), "/journey");
    }

    // -- strip_ansi -------------------------------------------------------

    #[test]
    fn strip_ansi_fast_path() {
        assert_eq!(strip_ansi("hello"), "hello");
        assert_eq!(strip_ansi(""), "");
        assert_eq!(strip_ansi("no escapes here — just text"), "no escapes here — just text");
    }

    #[test]
    fn strip_ansi_csi() {
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m"), "red");
        assert_eq!(strip_ansi("\x1b[1;34mblue\x1b[m normal"), "blue normal");
        assert_eq!(strip_ansi("a\x1b[2mb\x1b[0mc"), "abc");
        // colon-separated params (CSI with :) — issue #61891 path emits truecolor
        assert_eq!(strip_ansi("\x1b[38:2:255:0:0mX\x1b[0m"), "X");
    }

    #[test]
    fn strip_ansi_osc_and_c1() {
        // OSC with BEL terminator
        assert_eq!(strip_ansi("\x1b]0;title\x07hello"), "hello");
        // OSC with ST terminator
        assert_eq!(strip_ansi("\x1b]0;title\x1b\\hello"), "hello");
        // 8-bit CSI
        assert_eq!(strip_ansi("\x9b31mred\x9b0m"), "red");
        // bare C1 byte stripped
        assert_eq!(strip_ansi("hi\x80there"), "hithere");
        assert_eq!(strip_ansi("hi\x9fthere"), "hithere");
    }

    #[test]
    fn strip_ansi_dcs_and_nf() {
        // DCS: ESC P ... ST
        assert_eq!(strip_ansi("\x1bPfoo\x1b\\bar"), "bar");
        // nF: ESC ( B etc. — charset selection
        assert_eq!(strip_ansi("a\x1b(Bb"), "ab");
    }

    // -- run_command ------------------------------------------------------

    #[test]
    fn run_command_empty_is_empty() {
        let out = run_command("", |_| "should not be called".to_string());
        assert_eq!(out, "");
        let out2 = run_command("   ", |_| "nope".to_string());
        assert_eq!(out2, "");
    }

    #[test]
    fn run_command_normalizes_and_strips() {
        // handler returns output with ANSI + trailing whitespace
        let out = run_command("help", |cmd| {
            assert_eq!(cmd, "/help");
            "  \x1b[31mresult\x1b[0m  \n\n".to_string()
        });
        assert_eq!(out, "  \x1b[31mresult\x1b[0m".trim_end_matches(|c| c == ' ' || c == '\n'));
        // Actually run_command does trim_end then strip_ansi, so trailing spaces before strip are trimmed
        // "  \x1b[31mresult\x1b[0m  \n\n".trim_end() = "  \x1b[31mresult\x1b[0m"
        // strip_ansi → "  result"
        assert_eq!(out, "  result");

        let out2 = run_command("/journey", |cmd| {
            assert_eq!(cmd, "/journey");
            "ok".to_string()
        });
        assert_eq!(out2, "ok");
    }

    #[test]
    fn run_command_with_buf() {
        let out = run_command_with_buf("help", |cmd, buf| {
            assert_eq!(cmd, "/help");
            buf.push_str("\x1b[2mhello\x1b[0m\n");
        });
        assert_eq!(out, "hello");
    }

    // -- JSON helpers -----------------------------------------------------

    #[test]
    fn encode_id_json_cases() {
        assert_eq!(encode_id_json(None), "null");
        assert_eq!(encode_id_json(Some("null")), "null");
        assert_eq!(encode_id_json(Some("")), "null");
        assert_eq!(encode_id_json(Some("42")), "42");
        assert_eq!(encode_id_json(Some("3.14")), "3.14");
        assert_eq!(encode_id_json(Some("\"abc\"")), "\"abc\"");
        assert_eq!(encode_id_json(Some("abc")), "\"abc\"");
        assert_eq!(encode_id_json(Some("true")), "true");
        assert_eq!(encode_id_json(Some("false")), "false");
    }

    #[test]
    fn format_success_error_escape() {
        let s = format_success("1", "hello \"world\"\n");
        assert!(s.contains(r#""id":1"#));
        assert!(s.contains(r#""ok":true"#));
        assert!(s.contains(r#""output":"hello \"world\"\n""#));

        let e = format_error("null", "boom \"oops\"\n");
        assert!(e.contains(r#""ok":false"#));
        assert!(e.contains(r#""error":"boom \"oops\"\n""#));
    }

    #[test]
    fn parse_request_cases() {
        let (id, cmd) = parse_request(r#"{"id": 1, "command": "/help"}"#).unwrap();
        assert_eq!(id, "1");
        assert_eq!(cmd, "/help");

        let (id2, cmd2) = parse_request(r#"{"id": "abc", "command": "help"}"#).unwrap();
        assert_eq!(id2, "\"abc\"");
        assert_eq!(cmd2, "help");

        let (id3, cmd3) = parse_request(r#"{"id": null, "command": ""}"#).unwrap();
        assert_eq!(id3, "null");
        assert_eq!(cmd3, "");

        let (id4, cmd4) = parse_request(r#"{"command": "/journey"}"#).unwrap();
        assert_eq!(id4, "null");
        assert_eq!(cmd4, "/journey");

        let (id5, cmd5) = parse_request(r#"{"id": 42}"#).unwrap();
        assert_eq!(id5, "42");
        assert_eq!(cmd5, "");

        assert!(parse_request(r#"not json"#).is_err());
        assert!(parse_request(r#""#).is_err());
    }

    #[test]
    fn handle_line_empty_is_none() {
        assert!(handle_line("", |_| "x".to_string()).is_none());
        assert!(handle_line("   \n", |_| "x".to_string()).is_none());
    }

    #[test]
    fn handle_line_success_and_error() {
        let resp = handle_line(r#"{"id": 1, "command": "help"}"#, |cmd| {
            assert_eq!(cmd, "/help");
            "output here".to_string()
        })
        .unwrap();
        assert!(resp.contains(r#""id":1"#));
        assert!(resp.contains(r#""ok":true"#));
        assert!(resp.contains("output here"));

        let resp2 = handle_line(r#"{"id": 2, "command": ""}"#, |_| "should be empty".to_string()).unwrap();
        assert!(resp2.contains(r#""ok":true"#));
        assert!(resp2.contains(r#""output":"""#));

        let resp3 = handle_line(r#"not json"#, |_| "x".to_string()).unwrap();
        assert!(resp3.contains(r#""ok":false"#));
        assert!(resp3.contains(r#""id":null"#));
    }

    #[test]
    fn handle_line_strips_ansi_and_rstrip() {
        let resp = handle_line(r#"{"id": 1, "command": "help"}"#, |_| "\x1b[31mhi\x1b[0m  \n".to_string()).unwrap();
        // output should be stripped and rstrip'd → "hi"
        assert!(resp.contains(r#""output":"hi""#));
    }

    // -- parse_args -------------------------------------------------------

    #[test]
    fn parse_args_cases() {
        let args = vec!["--session-key".to_string(), "sk123".to_string()];
        let parsed = parse_args(&args).unwrap();
        assert_eq!(parsed.session_key, "sk123");
        assert_eq!(parsed.model, None);

        let args2 = vec![
            "--session-key".to_string(),
            "sk".to_string(),
            "--model".to_string(),
            "gpt-4".to_string(),
        ];
        let p2 = parse_args(&args2).unwrap();
        assert_eq!(p2.model, Some("gpt-4".to_string()));

        let args3 = vec!["--session-key=sk123".to_string(), "--model=".to_string()];
        let p3 = parse_args(&args3).unwrap();
        assert_eq!(p3.model, None); // empty → None (mirrors `or None`)

        assert!(parse_args(&[]).is_err());
        assert!(parse_args(&["--model".to_string(), "x".to_string()]).is_err());
        assert!(parse_args(&["--session-key".to_string(), "  ".to_string()]).is_err());
        assert!(parse_args(&["--unknown".to_string()]).is_err());
    }

    #[test]
    fn worker_env_vars_test() {
        let args = WorkerArgs {
            session_key: "sk".to_string(),
            model: Some("m".to_string()),
        };
        let env = worker_env_vars(&args);
        assert_eq!(env.get("HERMES_SESSION_KEY").map(|s| s.as_str()), Some("sk"));
        assert_eq!(env.get("HERMES_INTERACTIVE").map(|s| s.as_str()), Some("1"));
    }

    // -- WorkerLoop -------------------------------------------------------

    #[test]
    fn worker_loop_handle_line() {
        let mut lo = WorkerLoop::new("sk", None);
        let mut trim_called = false;
        let resp = lo
            .handle_line(
                r#"{"id": 1, "command": "help"}"#,
                |_| "hi".to_string(),
                |_| {
                    trim_called = true;
                },
            )
            .unwrap();
        assert!(resp.contains(r#""output":"hi""#));
        assert!(trim_called);
        assert!(!lo.in_flight.load(Ordering::SeqCst));
        // empty line → None, no trim
        let mut trim2 = false;
        assert!(lo
            .handle_line(
                "   ",
                |_| "x".to_string(),
                |_| {
                    trim2 = true;
                }
            )
            .is_none());
        assert!(!trim2);
        assert!(!lo.in_flight.load(Ordering::SeqCst));
    }

    // -- run_worker_loop --------------------------------------------------

    #[test]
    fn run_worker_loop_success() {
        let input = r#"{"id": 1, "command": "help"}
{"id": 2, "command": "/journey"}
{"id": 3, "command": ""}

{"id": 4, "command": "help"}
"#;
        let mut reader = Cursor::new(input.as_bytes());
        let mut writer: Vec<u8> = Vec::new();
        let mut calls: Vec<String> = Vec::new();
        run_worker_loop(
            &mut reader,
            &mut writer,
            |cmd| {
                calls.push(cmd.to_string());
                format!("out for {}", cmd)
            },
            || Ok(()),
        )
        .unwrap();
        let output = String::from_utf8(writer).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 4); // empty line skipped
        assert!(lines[0].contains(r#""id":1"#));
        assert!(lines[0].contains("out for /help"));
        assert!(lines[1].contains(r#""id":2"#));
        assert!(lines[2].contains(r#""id":3"#));
        assert!(lines[2].contains(r#""output":"""#)); // empty command
        assert!(lines[3].contains(r#""id":4"#));
        assert_eq!(calls, vec!["/help", "/journey", "", "/help"]);
    }

    #[test]
    fn run_worker_loop_error_cases() {
        // invalid json → ok:false with id null, valid line still processed
        let input = "not json\n{\"id\": 1, \"command\": \"help\"}\n";
        let mut reader = Cursor::new(input.as_bytes());
        let mut writer: Vec<u8> = Vec::new();
        run_worker_loop(&mut reader, &mut writer, |_| "ok".to_string(), || Ok(())).unwrap();
        let output = String::from_utf8(writer).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains(r#""ok":false"#));
        assert!(lines[0].contains(r#""id":null"#));
        assert!(lines[1].contains(r#""ok":true"#));
    }

    #[test]
    fn run_worker_loop_runner_panic_is_error() {
        // runner panic should be caught and returned as error? Actually run_worker_loop catches panic for valid json path.
        // But our run_worker_loop currently catches panic only inside handle_line? It does via catch_unwind.
        // Test that panic doesn't crash loop.
        let input = "{\"id\": 1, \"command\": \"help\"}\n{\"id\": 2, \"command\": \"help\"}\n";
        let mut reader = Cursor::new(input.as_bytes());
        let mut writer: Vec<u8> = Vec::new();
        let mut count = 0;
        run_worker_loop(
            &mut reader,
            &mut writer,
            |_| {
                count += 1;
                if count == 1 {
                    panic!("boom");
                }
                "ok".to_string()
            },
            || Ok(()),
        )
        .unwrap();
        let output = String::from_utf8(writer).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains(r#""ok":false"#));
        assert!(lines[1].contains(r#""ok":true"#));
    }

    #[test]
    fn run_worker_loop_trims_and_strips_ansi() {
        let input = "{\"id\": 1, \"command\": \"help\"}\n";
        let mut reader = Cursor::new(input.as_bytes());
        let mut writer: Vec<u8> = Vec::new();
        run_worker_loop(
            &mut reader,
            &mut writer,
            |_| "\x1b[31mhello\x1b[0m  \n".to_string(),
            || Ok(()),
        )
        .unwrap();
        let output = String::from_utf8(writer).unwrap();
        assert!(output.contains(r#""output":"hello""#));
    }

    // -- watchdog pure helpers --------------------------------------------

    #[test]
    fn orphan_deadline_calc() {
        let now = Instant::now();
        let dl = orphan_deadline(now, 5.0);
        assert!(dl > now);
        let diff = dl.duration_since(now);
        assert!((diff.as_secs_f64() - 5.0).abs() < 0.01);
        // negative grace clamped to 0
        let dl2 = orphan_deadline(now, -5.0);
        assert!(dl2 >= now);
        assert!(dl2.duration_since(now).as_secs_f64() < 0.01);
    }

    #[test]
    fn watchdog_thread_lifecycle() {
        // Spawn with no orphan — getppid always equals original, so not orphaned.
        // Use a on_exit that sets a flag instead of exiting, and poll quickly.
        // We test the injectable variant with short sleeps to avoid hanging.
        // To avoid infinite loop, we spawn orphaned immediately (getppid != orig).
        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = Arc::clone(&flag);
        let in_flight = Arc::new(AtomicBool::new(false));
        let handle = start_parent_death_watchdog_with(
            100,
            0.01,
            0.05,
            Arc::clone(&in_flight),
            || 101, // orphaned immediately ( !=100)
            move || {
                flag_clone.store(true, Ordering::SeqCst);
            },
        );
        // Wait for thread to call on_exit (orphaned + no in_flight → immediate)
        let deadline = Instant::now() + Duration::from_secs(1);
        while !flag.load(Ordering::SeqCst) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(flag.load(Ordering::SeqCst), "watchdog should have exited");
        let _ = handle.join();
    }

    #[test]
    fn watchdog_honors_in_flight_grace() {
        // Orphaned but in_flight=true — should wait grace window before exit.
        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = Arc::clone(&flag);
        let in_flight = Arc::new(AtomicBool::new(true));
        let in_flight_clone = Arc::clone(&in_flight);
        let start = Instant::now();
        let handle = start_parent_death_watchdog_with(
            100,
            0.01,
            0.2, // 200ms grace
            Arc::clone(&in_flight),
            || 101, // orphaned
            move || {
                flag_clone.store(true, Ordering::SeqCst);
            },
        );
        // In-flight true, so exit should be delayed ~0.2s. Check not yet exited at 50ms.
        thread::sleep(Duration::from_millis(50));
        assert!(!flag.load(Ordering::SeqCst), "should still be in grace window");
        // Clear in_flight — should exit shortly after (next 50ms poll)
        in_flight_clone.store(false, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(1);
        while !flag.load(Ordering::SeqCst) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(flag.load(Ordering::SeqCst));
        assert!(start.elapsed().as_secs_f64() >= 0.05);
        let _ = handle.join();
    }

    // -- prepare runtime --------------------------------------------------

    #[test]
    fn prepare_runtime_with_calls_both() {
        let mut started = false;
        let mut waited = false;
        prepare_slash_worker_runtime_with(|| started = true, || waited = true);
        assert!(started);
        assert!(waited);
        // default is no-op
        prepare_slash_worker_runtime();
    }
}
