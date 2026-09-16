//! shell/kill.rs: process-group kill with SIGTERM-to-SIGKILL escalation.
//!
//! Only our own groups (spawned detached via setsid, pgid == pid). Refuses
//! degenerate groups and our own process/group before any syscall.

use std::io;
use std::time::{Duration, Instant};

/// Single raw-signal choke point. Probes with sig 0 never count.
/// SAFETY: `kill(2)` is async-signal-safe; targets are validated by callers.
#[cfg(unix)]
unsafe fn signal_pid(target: i32, sig: i32) -> i32 {
    unsafe { libc::kill(target, sig) }
}

/// Windows has no POSIX signals: every signal attempt fails (-1), so all
/// callers fail closed via their existing error paths (never ESRCH).
#[cfg(not(unix))]
unsafe fn signal_pid(_target: i32, _sig: i32) -> i32 {
    -1
}

/// True when no process answers at `target` (`kill(target, 0)` gives ESRCH).
/// Anything else (including EPERM) counts as alive: fail closed.
/// Non-unix: no signal probe exists, so never report gone (fail closed).
#[cfg_attr(not(unix), allow(dead_code))] // windows `escalate` stub never probes
fn gone(target: i32) -> bool {
    if unsafe { signal_pid(target, 0) } == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Shared SIGTERM, 100 ms poll, SIGKILL escalation. `sig_target` is the
/// `kill(2)` first arg (`-pgid` for our groups).
async fn escalate(sig_target: i32, what: &str, grace: Duration) -> Result<(), String> {
    // Windows has no SIGTERM/SIGKILL escalation: refuse instead of signalling.
    #[cfg(not(unix))]
    {
        let _ = (sig_target, grace);
        return Err(format!(
            "cannot signal {what}: POSIX signals are unavailable on Windows"
        ));
    }
    #[cfg(unix)]
    {
        let t0 = Instant::now();
        if unsafe { signal_pid(sig_target, libc::SIGTERM) } != 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            return Err(format!("SIGTERM to {what} failed: {e}"));
        }
        loop {
            if gone(sig_target) {
                return Ok(());
            }
            if t0.elapsed() >= grace {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if gone(sig_target) {
            return Ok(());
        }
        if unsafe { signal_pid(sig_target, libc::SIGKILL) } != 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            return Err(format!("SIGKILL to {what} failed: {e}"));
        }
        Ok(())
    }
}

/// SIGTERM a group we created, SIGKILL after `grace` when ignored.
/// Refuses degenerate groups and our own process/group before any syscall
/// (never broadcast). Returns `Err` on refusal.
pub async fn term_then_kill(pgid: i32, grace: Duration) -> Result<(), String> {
    if pgid <= 1 {
        return Err(format!(
            "refusing to signal process group {pgid}: never broadcast"
        ));
    }
    let me = std::process::id() as i32;
    if pgid == me {
        return Err(format!("refusing to signal our own pid ({me})"));
    }
    #[cfg(unix)]
    if pgid == unsafe { libc::getpgrp() } {
        return Err(format!("refusing to signal our own process group ({pgid})"));
    }
    escalate(-pgid, &format!("process group {pgid}"), grace).await
}

#[path = "kill_tests.rs"]
#[cfg(all(test, unix))] // signal/sh/sleep fixtures are unix-only
mod tests;
