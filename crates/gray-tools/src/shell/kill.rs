//! shell/kill.rs: process-group kill with SIGTERM-to-SIGKILL escalation.
//!
//! Only our own groups (spawned detached via setsid, pgid == pid). Refuses
//! degenerate groups and our own process/group before any syscall.

use std::time::Duration;

// POSIX signal probing is separate from the Windows Job Object path.
#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::time::Instant;

/// Single raw-signal choke point. Probes with sig 0 never count.
/// SAFETY: `kill(2)` is async-signal-safe; targets are validated by callers.
#[cfg(unix)]
unsafe fn signal_pid(target: i32, sig: i32) -> i32 {
    unsafe { libc::kill(target, sig) }
}

/// Unsupported platforms have no signal backend; fail closed.
#[cfg(all(not(unix), not(windows)))]
unsafe fn signal_pid(_target: i32, _sig: i32) -> i32 {
    -1
}

/// True when no process answers at `target` (`kill(target, 0)` gives ESRCH).
/// Anything else (including EPERM) counts as alive: fail closed.
/// Non-unix: no signal probe exists, so never report gone (fail closed).
#[cfg(not(windows))]
#[cfg_attr(not(unix), allow(dead_code))] // unsupported-platform stub never probes
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
#[cfg(not(windows))]
async fn escalate(sig_target: i32, what: &str, grace: Duration) -> Result<(), String> {
    // Unsupported platforms must not pretend to have POSIX signals.
    #[cfg(not(unix))]
    {
        let _ = (sig_target, grace);
        return Err(format!(
            "cannot signal {what}: POSIX signals are unavailable on this platform"
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
#[cfg(not(windows))]
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

/// Windows has no safe POSIX group-signaling equivalent. The job handle, not
/// a reusable PID, identifies exactly the shell tree owned by this call.
#[cfg(windows)]
pub async fn term_then_kill(job: &super::windows::Job, _grace: Duration) -> Result<(), String> {
    job.terminate()
        .map_err(|e| format!("terminating shell job failed: {e}"))
}

/// Last-resort cleanup when the runtime drops a running command future.
/// Normal completion disarms this after reaping; ordinary cancel uses TERM/KILL.
#[cfg(not(windows))]
pub(crate) struct GroupGuard {
    pub(crate) pgid: i32,
    armed: bool,
}

#[cfg(not(windows))]
impl GroupGuard {
    pub(crate) fn new(pgid: i32) -> Self {
        Self { pgid, armed: true }
    }
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

#[cfg(not(windows))]
impl Drop for GroupGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if self.armed
            && self.pgid > 1
            && self.pgid != std::process::id() as i32
            && self.pgid != unsafe { libc::getpgrp() }
        {
            unsafe {
                signal_pid(-self.pgid, libc::SIGKILL);
            }
        }
    }
}
