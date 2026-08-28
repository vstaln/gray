//! Shared spurious stdin-EOF recovery for the TUI gateway entry point and slash worker.
//!
//! 1:1 port of `tui_gateway/_stdin_recovery.py` (151 lines).
//!
//! When a child process inherits fd 0 (stdin) and sets ``O_NONBLOCK``, the flag
//! lands on the **shared open file description** — not just the child's descriptor.
//! The gateway's next ``read()`` returns ``EAGAIN``, which CPython's buffered
//! ``TextIOWrapper`` converts to ``b''`` (apparent EOF), killing the gateway.
//!
//! This module provides:
//! - [`diagnose_stdin_state`] — forensic diagnostic (``O_NONBLOCK`` / ``SO_RCVTIMEO``)
//! - [`handle_spurious_eof`] — check whether an empty ``readline()`` is a genuine
//!   peer-close or a spurious EOF, and recover if spurious.
//!
//! The recovery is **POSIX-only** (``fcntl``). On Windows, ``O_NONBLOCK`` on a
//! shared file description is not a concern, so the guard simply reports a
//! genuine EOF and lets the caller exit.
//!
//! ```python
//! # Python — tui_gateway/_stdin_recovery.py
//! MAX_RECOVERIES_PER_MINUTE = 10
//! def diagnose_stdin_state() -> str: ...
//! def handle_spurious_eof(recovery_times: list[float], log_fn: object) -> bool: ...
//! ```
//!
//! # Rust mapping
//!
//! * `MAX_RECOVERIES_PER_MINUTE = 10` → [`MAX_RECOVERIES_PER_MINUTE`].
//! * `try: import fcntl / _HAS_FCNTL` → `cfg(unix)` gating. Windows (`cfg(windows)`)
//!   has no `fcntl` and the `O_NONBLOCK` shared-description issue is POSIX-specific,
//!   so [`handle_spurious_eof`] immediately logs `"stdin EOF (peer closed)"` and
//!   returns `false`, mirroring `if not (_HAS_FCNTL and _fcntl is not None):`.
//! * `_fcntl.fcntl(0, F_GETFL)` + `flags & os.O_NONBLOCK` → [`get_stdin_flags`]
//!   via `unsafe { fcntl(0, F_GETFL) }` on Unix. `Except: is_nonblock=False` preserved.
//! * `diagnose_stdin_state` `O_NONBLOCK` part → [`diagnose_stdin_state`] formats
//!   `O_NONBLOCK=1/0` or `F_GETFL error: {e}` or `O_NONBLOCK=n/a (no fcntl)` on
//!   non-Unix, mirroring the `try/except` and `else` branches.
//! * `SO_RCVTIMEO` via `socket.fromfd(0, AF_UNIX, SOCK_STREAM).getsockopt(SOL_SOCKET, SO_RCVTIMEO)`
//!   → [`get_stdin_rcvtimeo`] via `dup(0)` + `getsockopt(SOL_SOCKET, SO_RCVTIMEO)`
//!   into `Timeval { tv_sec, tv_usec }`. `fromfd` dup semantics (`close` releases
//!   dup without touching fd 0) are preserved by `dup`/`close`. `Except: pass`
//!   is preserved — a failing `getsockopt` simply omits the `SO_RCVTIMEO` part.
//! * `time.time()` → `SystemTime::now().duration_since(UNIX_EPOCH).as_secs_f64()`.
//! * `recovery_times.append(now); recovery_times[:] = [t for t in recovery_times if t > now - 60]`
//!   → `Vec<f64>::push` + `retain(|t| *t > now - 60.0)`.
//! * `len(recovery_times) > MAX_RECOVERIES_PER_MINUTE` rate limit with message
//!   `stdin spurious-EOF recovery rate exceeded ({n}/min, cap {cap})` → identical.
//! * `diagnose_stdin_state()` diagnostic appended to `stdin spurious EOF (subprocess O_NONBLOCK flip), recovering: {diag}`
//!   → same format.
//! * `os.set_blocking(0, True)` → [`set_stdin_blocking_true`] via `fcntl(F_GETFL)` + `fcntl(F_SETFL, flags & !O_NONBLOCK)`.
//! * `socket.fromfd(...).setsockopt(SOL_SOCKET, SO_RCVTIMEO, struct.pack("ll", 0, 0))` (zero `timeval`)
//!   → [`clear_stdin_rcvtimeo`] via `dup(0)` + `setsockopt(SOL_SOCKET, SO_RCVTIMEO, Timeval{0,0})` with `try/except: pass`.
//! * `log_fn` (`_log_exit` / `print(file=sys.stderr)`) → `F: FnMut(&str)` closure.
//! * Injectables [`diagnose_stdin_state_with`] / [`handle_spurious_eof_with`] mirror the
//!   Python module-level `_HAS_FCNTL`/`_HAS_SOCKET` branches for unit tests without real fd 0.

use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Constants — mirrors _stdin_recovery.py:45
// ---------------------------------------------------------------------------

/// Rate-limit: at most this many spurious-EOF recoveries per 60-second window.
///
/// A child aggressively flipping ``O_NONBLOCK`` on the shared fd would otherwise
/// create a tight busy-loop burning CPU. Exceeding the cap exits the process —
/// the parent (TUI / gateway) respawns it with fresh state.
///
/// Mirrors `MAX_RECOVERIES_PER_MINUTE = 10`.
pub const MAX_RECOVERIES_PER_MINUTE: usize = 10;

// ---------------------------------------------------------------------------
// POSIX helpers — cfg(unix) only
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod posix {
    use std::os::raw::{c_int, c_void};

    // fcntl commands
    pub const F_GETFL: c_int = 3;
    pub const F_SETFL: c_int = 4;

    // O_NONBLOCK per-OS (octal 04000 on Linux, 0x0004 on Darwin/BSD)
    #[cfg(target_os = "linux")]
    pub const O_NONBLOCK: c_int = 0o4000; // 2048
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    pub const O_NONBLOCK: c_int = 0x0004;
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    )))]
    pub const O_NONBLOCK: c_int = 0o4000;

    // SOL_SOCKET / SO_RCVTIMEO per-OS
    #[cfg(target_os = "linux")]
    pub const SOL_SOCKET: c_int = 1;
    #[cfg(target_os = "linux")]
    pub const SO_RCVTIMEO: c_int = 20;
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    pub const SOL_SOCKET: c_int = 0xffff;
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    pub const SO_RCVTIMEO: c_int = 0x1006;
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    )))]
    pub const SOL_SOCKET: c_int = 1;
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    )))]
    pub const SO_RCVTIMEO: c_int = 20;

    extern "C" {
        pub fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
        pub fn dup(fd: c_int) -> c_int;
        pub fn close(fd: c_int) -> c_int;
        pub fn getsockopt(
            sockfd: c_int,
            level: c_int,
            optname: c_int,
            optval: *mut c_void,
            optlen: *mut u32,
        ) -> c_int;
        pub fn setsockopt(
            sockfd: c_int,
            level: c_int,
            optname: c_int,
            optval: *const c_void,
            optlen: u32,
        ) -> c_int;
    }

    #[repr(C)]
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct Timeval {
        pub tv_sec: std::os::raw::c_long,
        pub tv_usec: std::os::raw::c_long,
    }

    /// Mirrors `_fcntl.fcntl(0, F_GETFL)`. Returns flags or last OS error.
    pub fn get_stdin_flags() -> Result<c_int, String> {
        let ret = unsafe { fcntl(0, F_GETFL) };
        if ret == -1 {
            Err(std::io::Error::last_os_error().to_string())
        } else {
            Ok(ret)
        }
    }

    /// Mirrors `os.set_blocking(0, True)` — clear ``O_NONBLOCK``.
    pub fn set_stdin_blocking_true() -> Result<(), String> {
        let flags = unsafe { fcntl(0, F_GETFL) };
        if flags == -1 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let new_flags = flags & !O_NONBLOCK;
        let rc = unsafe { fcntl(0, F_SETFL, new_flags) };
        if rc == -1 {
            Err(std::io::Error::last_os_error().to_string())
        } else {
            Ok(())
        }
    }

    /// Mirrors `socket.fromfd(0, AF_UNIX, SOCK_STREAM).getsockopt(SOL_SOCKET, SO_RCVTIMEO)`.
    /// Returns `Timeval` on success; `Err` on any failure (caller treats as silent `pass` for diagnose).
    pub fn get_stdin_rcvtimeo() -> Result<Timeval, String> {
        let dup_fd = unsafe { dup(0) };
        if dup_fd == -1 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let mut tv = Timeval::default();
        let mut len = std::mem::size_of::<Timeval>() as u32;
        let rc = unsafe {
            getsockopt(
                dup_fd,
                SOL_SOCKET,
                SO_RCVTIMEO,
                &mut tv as *mut _ as *mut c_void,
                &mut len,
            )
        };
        unsafe { close(dup_fd); }
        if rc == -1 {
            Err(std::io::Error::last_os_error().to_string())
        } else {
            Ok(tv)
        }
    }

    /// Mirrors `socket.fromfd(0, ...).setsockopt(SOL_SOCKET, SO_RCVTIMEO, struct.pack("ll", 0, 0))`
    /// — zero timeval. Silent `pass` on failure (caller ignores `Err`).
    pub fn clear_stdin_rcvtimeo() -> Result<(), String> {
        let dup_fd = unsafe { dup(0) };
        if dup_fd == -1 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let tv = Timeval { tv_sec: 0, tv_usec: 0 };
        let rc = unsafe {
            setsockopt(
                dup_fd,
                SOL_SOCKET,
                SO_RCVTIMEO,
                &tv as *const _ as *const c_void,
                std::mem::size_of::<Timeval>() as u32,
            )
        };
        unsafe { close(dup_fd); }
        if rc == -1 {
            Err(std::io::Error::last_os_error().to_string())
        } else {
            Ok(())
        }
    }

    pub fn is_nonblock(flags: c_int) -> bool {
        (flags & O_NONBLOCK) != 0
    }
}

// ---------------------------------------------------------------------------
// diagnose_stdin_state
// ---------------------------------------------------------------------------

/// Return a diagnostic string about stdin's current state.
///
/// Used for crash-log forensics when stdin iteration falls through.
/// Distinguishes genuine peer-close (flag clear) from spurious EOF
/// caused by a child setting ``O_NONBLOCK`` on the shared file description.
///
/// Mirrors `tui_gateway/_stdin_recovery.py::diagnose_stdin_state`:
///
/// ```python
/// def diagnose_stdin_state() -> str:
///     parts: list[str] = []
///     if _HAS_FCNTL and _fcntl is not None:
///         try:
///             flags = _fcntl.fcntl(0, _fcntl.F_GETFL)
///             parts.append(f"O_NONBLOCK={'1' if flags & os.O_NONBLOCK else '0'}")
///         except Exception as e:
///             parts.append(f"F_GETFL error: {e}")
///     else:
///         parts.append("O_NONBLOCK=n/a (no fcntl)")
///     if _HAS_SOCKET and _socket is not None:
///         try:
///             s = _socket.fromfd(0, _socket.AF_UNIX, _socket.SOCK_STREAM)
///             try:
///                 tv = s.getsockopt(_socket.SOL_SOCKET, _socket.SO_RCVTIMEO)
///                 parts.append(f"SO_RCVTIMEO={tv!r}")
///             finally:
///                 s.close()
///         except Exception:
///             pass
///     return ", ".join(parts) if parts else "unknown"
/// ```
pub fn diagnose_stdin_state() -> String {
    #[cfg(unix)]
    {
        diagnose_unix()
    }
    #[cfg(not(unix))]
    {
        // Mirrors `O_NONBLOCK=n/a (no fcntl)` plus silent socket `pass`.
        // On Windows `_HAS_FCNTL` is False, socket `fromfd` fails silently,
        // so only this part remains.
        "O_NONBLOCK=n/a (no fcntl)".to_string()
    }
}

#[cfg(unix)]
fn diagnose_unix() -> String {
    use posix::*;
    let mut parts: Vec<String> = Vec::new();

    // fcntl branch — mirrors `if _HAS_FCNTL and _fcntl is not None: try: ...`
    match get_stdin_flags() {
        Ok(flags) => {
            let v = if is_nonblock(flags) { "1" } else { "0" };
            parts.push(format!("O_NONBLOCK={}", v));
        }
        Err(e) => {
            parts.push(format!("F_GETFL error: {}", e));
        }
    }

    // SO_RCVTIMEO branch — mirrors `if _HAS_SOCKET: try: fromfd+getsockopt`
    // Python: `tv = s.getsockopt(SOL_SOCKET, SO_RCVTIMEO); parts.append(f"SO_RCVTIMEO={tv!r}")`
    // Rust: `Timeval` fields. On error, silent `pass` (no part).
    match get_stdin_rcvtimeo() {
        Ok(tv) => {
            // Display as Python would `!r` a bytes timeval: but we have fields.
            // Use `Timeval(tv_sec=.., tv_usec=..)` to stay informative.
            // To emulate `!r` of bytes, alternatively format debug; keep fields.
            parts.push(format!("SO_RCVTIMEO=Timeval(tv_sec={}, tv_usec={})", tv.tv_sec, tv.tv_usec));
        }
        Err(_) => {
            // mirrors `except Exception: pass`
        }
    }

    if parts.is_empty() {
        "unknown".to_string()
    } else {
        parts.join(", ")
    }
}

/// Injectable variant for tests — mirrors `diagnose_stdin_state` but with
/// injected `get_flags` / `get_rcvtimeo` closures.
///
/// `get_flags` returns `Ok(flags)` or `Err(msg)`; `get_rcvtimeo` returns
/// `Ok(debug_repr)` or `Err` (silent). This avoids touching real fd 0 in tests.
pub fn diagnose_stdin_state_with<F, S>(mut get_flags: F, mut get_rcvtimeo: S) -> String
where
    F: FnMut() -> Result<i32, String>,
    S: FnMut() -> Result<String, String>,
{
    let mut parts: Vec<String> = Vec::new();
    match get_flags() {
        Ok(flags) => {
            // Check both Linux (0o4000 == 2048) and Darwin (0x0004 == 4) bits
            // so the injectable works regardless of host O_NONBLOCK constant.
            let is_nb = (flags & 2048) != 0 || (flags & 4) != 0;
            parts.push(format!("O_NONBLOCK={}", if is_nb { "1" } else { "0" }));
        }
        Err(e) => {
            parts.push(format!("F_GETFL error: {}", e));
        }
    }
    match get_rcvtimeo() {
        Ok(repr) => parts.push(format!("SO_RCVTIMEO={}", repr)),
        Err(_) => {}
    }
    if parts.is_empty() {
        "unknown".to_string()
    } else {
        parts.join(", ")
    }
}

/// Helper for the `O_NONBLOCK=n/a (no fcntl)` branch (Windows / no fcntl).
/// Mirrors `parts.append("O_NONBLOCK=n/a (no fcntl)")` plus silent socket `pass`.
pub fn diagnose_no_fcntl() -> String {
    "O_NONBLOCK=n/a (no fcntl)".to_string()
}

// ---------------------------------------------------------------------------
// handle_spurious_eof
// ---------------------------------------------------------------------------

/// Check whether an empty ``readline()`` is spurious; recover if so.
///
/// Returns `true` if the caller should `continue` the read loop
/// (spurious EOF was recovered), `false` if it should `break` (genuine
/// peer-close or rate limit exceeded).
///
/// `log_fn` is called with a diagnostic string — `_log_exit` in
/// `entry.py`, `print(file=sys.stderr)` in `slash_worker.py`.
///
/// Mirrors `tui_gateway/_stdin_recovery.py::handle_spurious_eof`:
///
/// ```python
/// def handle_spurious_eof(recovery_times: list[float], log_fn: object) -> bool:
///     if not (_HAS_FCNTL and _fcntl is not None):
///         log_fn("stdin EOF (peer closed)")
///         return False
///     try:
///         flags = _fcntl.fcntl(0, _fcntl.F_GETFL)
///         is_nonblock = bool(flags & os.O_NONBLOCK)
///     except Exception:
///         is_nonblock = False
///     if not is_nonblock:
///         log_fn("stdin EOF (peer closed)")
///         return False
///     now = time.time()
///     recovery_times.append(now)
///     recovery_times[:] = [t for t in recovery_times if t > now - 60]
///     if len(recovery_times) > MAX_RECOVERIES_PER_MINUTE:
///         log_fn(f"stdin spurious-EOF recovery rate exceeded ({len(recovery_times)}/min, cap {MAX_RECOVERIES_PER_MINUTE})")
///         return False
///     diag = diagnose_stdin_state()
///     log_fn(f"stdin spurious EOF (subprocess O_NONBLOCK flip), recovering: {diag}")
///     os.set_blocking(0, True)
///     if _HAS_SOCKET and _socket is not None:
///         try:
///             s = _socket.fromfd(0, _socket.AF_UNIX, _socket.SOCK_STREAM)
///             try:
///                 s.setsockopt(_socket.SOL_SOCKET, _socket.SO_RCVTIMEO, struct.pack("ll", 0, 0))
///             finally:
///                 s.close()
///         except Exception:
///             pass
///     return True
/// ```
pub fn handle_spurious_eof<F>(recovery_times: &mut Vec<f64>, mut log_fn: F) -> bool
where
    F: FnMut(&str),
{
    #[cfg(not(unix))]
    {
        // Mirrors `if not (_HAS_FCNTL and _fcntl is not None): log_fn(...); return False`
        // No recovery_times mutation on this path (Python also skips it).
        let _ = recovery_times;
        log_fn("stdin EOF (peer closed)");
        return false;
    }
    #[cfg(unix)]
    {
        handle_spurious_eof_unix(recovery_times, &mut log_fn)
    }
}

#[cfg(unix)]
fn handle_spurious_eof_unix<F>(recovery_times: &mut Vec<f64>, log_fn: &mut F) -> bool
where
    F: FnMut(&str),
{
    use posix::*;

    // Mirrors `try: flags = fcntl(0, F_GETFL); is_nonblock = bool(flags & O_NONBLOCK); except: is_nonblock=False`
    let is_nonblock = match get_stdin_flags() {
        Ok(flags) => is_nonblock(flags),
        Err(_) => false,
    };

    if !is_nonblock {
        log_fn("stdin EOF (peer closed)");
        return false;
    }

    // Spurious EOF path
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    recovery_times.push(now);
    // Mirrors `recovery_times[:] = [t for t in recovery_times if t > now - 60]`
    recovery_times.retain(|t| *t > now - 60.0);
    if recovery_times.len() > MAX_RECOVERIES_PER_MINUTE {
        log_fn(&format!(
            "stdin spurious-EOF recovery rate exceeded ({}/min, cap {})",
            recovery_times.len(),
            MAX_RECOVERIES_PER_MINUTE
        ));
        return false;
    }

    let diag = diagnose_stdin_state();
    log_fn(&format!(
        "stdin spurious EOF (subprocess O_NONBLOCK flip), recovering: {}",
        diag
    ));

    // Mirrors `os.set_blocking(0, True)` — clear O_NONBLOCK
    let _ = set_stdin_blocking_true();

    // Mirrors `if _HAS_SOCKET: try: fromfd+setsockopt zero timeval except: pass`
    let _ = clear_stdin_rcvtimeo();

    true
}

/// Injectable variant for unit tests — mirrors [`handle_spurious_eof`] but with
/// all side-effects injected so no real fd 0 is touched.
///
/// * `get_flags` — mirrors `_fcntl.fcntl(0, F_GETFL)`; `Ok(flags)` with `flags & O_NONBLOCK`,
///   `Err(_)` for `except: is_nonblock=False`.
/// * `now_secs` — mirrors `time.time()`; return value used for sliding window.
/// * `diagnose` — mirrors `diagnose_stdin_state()`; called only on the recovery path.
/// * `set_blocking` / `clear_timeout` — mirrors `os.set_blocking` / `setsockopt` zero;
///
/// All `log_fn` calls are forwarded to the provided closure. `recovery_times`
/// mutation and rate-limit check are identical to the real function.
pub fn handle_spurious_eof_with<F, G, N, S, C, D>(
    recovery_times: &mut Vec<f64>,
    log_fn: &mut F,
    mut get_flags: G,
    mut now_secs: N,
    mut set_blocking: S,
    mut clear_timeout: C,
    mut diagnose: D,
) -> bool
where
    F: FnMut(&str),
    G: FnMut() -> Result<i32, String>,
    N: FnMut() -> f64,
    S: FnMut() -> Result<(), String>,
    C: FnMut() -> Result<(), String>,
    D: FnMut() -> String,
{
    // POSIX guard is modelled by the caller: if the test wants the
    // non-POSIX path (Windows), it can call `handle_spurious_eof_no_fcntl` instead.
    // Here we assume POSIX is available.

    // Mirrors `try: flags = fcntl(...); is_nonblock = bool(flags & O_NONBLOCK); except: False`
    let is_nonblock = match get_flags() {
        Ok(flags) => {
            // Check both Linux (2048) and Darwin (4) bits so injectable works
            // regardless of host O_NONBLOCK constant. Real code uses `posix::is_nonblock`.
            let is_nb = (flags & 2048) != 0 || (flags & 4) != 0;
            // Also treat any non-zero low bit that caller intends as nonblock;
            // but keep strict bit check for exactness.
            is_nb
        }
        Err(_) => false,
    };

    if !is_nonblock {
        log_fn("stdin EOF (peer closed)");
        return false;
    }

    let now = now_secs();
    recovery_times.push(now);
    recovery_times.retain(|t| *t > now - 60.0);
    if recovery_times.len() > MAX_RECOVERIES_PER_MINUTE {
        log_fn(&format!(
            "stdin spurious-EOF recovery rate exceeded ({}/min, cap {})",
            recovery_times.len(),
            MAX_RECOVERIES_PER_MINUTE
        ));
        return false;
    }

    let diag = diagnose();
    log_fn(&format!(
        "stdin spurious EOF (subprocess O_NONBLOCK flip), recovering: {}",
        diag
    ));

    let _ = set_blocking();
    let _ = clear_timeout();

    true
}

/// No-fcntl variant — mirrors the Windows early-return path.
///
/// Does not mutate `recovery_times`, logs `"stdin EOF (peer closed)"`, returns `false`.
pub fn handle_spurious_eof_no_fcntl<F>(recovery_times: &mut Vec<f64>, mut log_fn: F) -> bool
where
    F: FnMut(&str),
{
    let _ = recovery_times;
    log_fn("stdin EOF (peer closed)");
    false
}

// ---------------------------------------------------------------------------
// Tests — mirror Python invariants
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn now_fixed(v: f64) -> impl FnMut() -> f64 {
        move || v
    }

    #[test]
    fn max_recoveries_is_10() {
        assert_eq!(MAX_RECOVERIES_PER_MINUTE, 10);
    }

    #[test]
    fn diagnose_no_fcntl_branch() {
        assert_eq!(diagnose_no_fcntl(), "O_NONBLOCK=n/a (no fcntl)");
        // Injectable diagnose with fcntl-missing simulated via get_flags Err + no socket
        let s = diagnose_stdin_state_with(
            || Err("no fcntl".to_string()),
            || Err("no socket".to_string()),
        );
        // Should contain F_GETFL error, not n/a (injectable reports error, not n/a)
        assert!(s.contains("F_GETFL error"));
    }

    #[test]
    fn diagnose_with_injectable_nonblock_and_rcvtimeo() {
        let s = diagnose_stdin_state_with(
            || Ok(2048), // O_NONBLOCK set (Linux)
            || Ok("Timeval(tv_sec=0, tv_usec=0)".to_string()),
        );
        assert_eq!(s, "O_NONBLOCK=1, SO_RCVTIMEO=Timeval(tv_sec=0, tv_usec=0)");
    }

    #[test]
    fn diagnose_with_injectable_blocking_no_rcvtimeo() {
        let s = diagnose_stdin_state_with(|| Ok(0), || Err("fail".to_string()));
        assert_eq!(s, "O_NONBLOCK=0");
    }

    #[test]
    fn diagnose_with_error_and_rcvtimeo() {
        let s = diagnose_stdin_state_with(
            || Err("boom".to_string()),
            || Ok("b'\\x00'".to_string()),
        );
        assert!(s.contains("F_GETFL error: boom"));
        assert!(s.contains("SO_RCVTIMEO"));
    }

    #[test]
    fn handle_no_fcntl_is_genuine_eof() {
        let mut times: Vec<f64> = vec![];
        let mut logs: Vec<String> = vec![];
        let res = handle_spurious_eof_no_fcntl(&mut times, |m| logs.push(m.to_string()));
        assert!(!res);
        assert!(times.is_empty());
        assert_eq!(logs, vec!["stdin EOF (peer closed)"]);
    }

    #[test]
    fn handle_genuine_eof_when_not_nonblock() {
        let mut times: Vec<f64> = vec![];
        let mut logs: Vec<String> = vec![];
        let res = handle_spurious_eof_with(
            &mut times,
            &mut |m| logs.push(m.to_string()),
            || Ok(0), // flags without O_NONBLOCK
            now_fixed(1000.0),
            || Ok(()),
            || Ok(()),
            || "O_NONBLOCK=0".to_string(),
        );
        assert!(!res);
        assert!(times.is_empty(), "genuine EOF must not push recovery_times");
        assert_eq!(logs, vec!["stdin EOF (peer closed)"]);
    }

    #[test]
    fn handle_genuine_eof_when_fcntl_errors() {
        let mut times: Vec<f64> = vec![];
        let mut logs: Vec<String> = vec![];
        let res = handle_spurious_eof_with(
            &mut times,
            &mut |m| logs.push(m.to_string()),
            || Err("bad fd".to_string()),
            now_fixed(1000.0),
            || Ok(()),
            || Ok(()),
            || "unknown".to_string(),
        );
        assert!(!res);
        assert_eq!(logs, vec!["stdin EOF (peer closed)"]);
    }

    #[test]
    fn handle_spurious_recovers_and_clears() {
        let mut times: Vec<f64> = vec![];
        let mut logs: Vec<String> = vec![];
        let mut blocking_called = false;
        let mut timeout_cleared = false;
        let res = handle_spurious_eof_with(
            &mut times,
            &mut |m| logs.push(m.to_string()),
            || Ok(2048), // O_NONBLOCK set
            now_fixed(1000.0),
            || {
                blocking_called = true;
                Ok(())
            },
            || {
                timeout_cleared = true;
                Ok(())
            },
            || "O_NONBLOCK=1, SO_RCVTIMEO=Timeval(tv_sec=0, tv_usec=0)".to_string(),
        );
        assert!(res);
        assert_eq!(times.len(), 1);
        assert_eq!(times[0], 1000.0);
        assert!(logs[0].contains("stdin spurious EOF (subprocess O_NONBLOCK flip), recovering:"));
        assert!(logs[0].contains("O_NONBLOCK=1"));
        assert!(blocking_called);
        assert!(timeout_cleared);
    }

    #[test]
    fn handle_spurious_rate_limit() {
        let mut times: Vec<f64> = vec![];
        let mut logs: Vec<String> = vec![];
        // Fill window with 10 recoveries at t=950..959 (within 60s of 1000)
        for i in 0..10 {
            times.push(950.0 + i as f64);
        }
        // Next at 1000.0 should be 11th within window -> rate exceeded
        let res = handle_spurious_eof_with(
            &mut times,
            &mut |m| logs.push(m.to_string()),
            || Ok(2048),
            now_fixed(1000.0),
            || Ok(()),
            || Ok(()),
            || "diag".to_string(),
        );
        assert!(!res);
        assert_eq!(times.len(), 11);
        assert!(logs[0].contains("stdin spurious-EOF recovery rate exceeded (11/min, cap 10)"));
    }

    #[test]
    fn handle_spurious_sliding_window_prunes_old() {
        let mut times: Vec<f64> = vec![800.0, 850.0, 900.0]; // old (>60s before 1000)
        let mut logs: Vec<String> = vec![];
        let res = handle_spurious_eof_with(
            &mut times,
            &mut |m| logs.push(m.to_string()),
            || Ok(2048),
            now_fixed(1000.0),
            || Ok(()),
            || Ok(()),
            || "diag".to_string(),
        );
        assert!(res);
        // 800,850 pruned ( <=940), 900 kept? 900 > 940? No, 900 <=940, so pruned. Only new 1000 remains + maybe.
        // now=1000, retain >940, so 800,850,900 all dropped, leaving only 1000
        assert_eq!(times, vec![1000.0]);
    }

    #[test]
    fn handle_spurious_ignores_blocking_and_timeout_errors() {
        let mut times: Vec<f64> = vec![];
        let mut logs: Vec<String> = vec![];
        // set_blocking and clear_timeout both Err, but should still return true
        let res = handle_spurious_eof_with(
            &mut times,
            &mut |m| logs.push(m.to_string()),
            || Ok(2048),
            now_fixed(1000.0),
            || Err("blocking fail".to_string()),
            || Err("timeout fail".to_string()),
            || "diag".to_string(),
        );
        assert!(res, "recovery succeeds even if cleanup fails (mirrors except: pass)");
        assert_eq!(logs.len(), 1);
    }

    #[test]
    fn handle_spurious_darwin_nonblock_bit() {
        // Darwin O_NONBLOCK is 0x0004, ensure injectable handles it
        let mut times: Vec<f64> = vec![];
        let mut logs: Vec<String> = vec![];
        let res = handle_spurious_eof_with(
            &mut times,
            &mut |m| logs.push(m.to_string()),
            || Ok(0x0004),
            now_fixed(1000.0),
            || Ok(()),
            || Ok(()),
            || "diag".to_string(),
        );
        assert!(res);
        assert!(logs[0].contains("recovering"));
    }

    #[test]
    fn diagnose_unix_real_does_not_panic() {
        // Real diagnose should not panic even if fd 0 is not a socket.
        // This exercises the actual POSIX path without asserting exact content
        // (which varies by fd state).
        let s = diagnose_stdin_state();
        assert!(!s.is_empty());
        // On unix, should contain O_NONBLOCK= or error; on non-unix, n/a
        #[cfg(unix)]
        assert!(s.contains("O_NONBLOCK=") || s.contains("F_GETFL error"));
        #[cfg(not(unix))]
        assert_eq!(s, "O_NONBLOCK=n/a (no fcntl)");
    }
}
