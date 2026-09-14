//! shell/kill.rs: process-group kill with SIGTERM-to-SIGKILL escalation.
//!
//! Only our own groups (spawned detached via setsid, pgid == pid). Refuses
//! degenerate groups and our own process/group before any syscall.

use std::io;
use std::time::{Duration, Instant};

use super::contract::KillMethod;

#[cfg(test)]
pub(crate) static SIGNAL_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Serializes every test that signals or counts signals: the signal
/// counter is process-global, so parallel tests would pollute it.
#[cfg(test)]
pub(crate) static KILL_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Single raw-signal choke point. Probes with sig 0 never count.
/// SAFETY: `kill(2)` is async-signal-safe; targets are validated by callers.
#[cfg(unix)]
unsafe fn signal_pid(target: i32, sig: i32) -> i32 {
    #[cfg(test)]
    if sig != 0 {
        SIGNAL_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
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
async fn escalate(sig_target: i32, what: &str, grace: Duration) -> Result<KillMethod, String> {
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
                return Ok(KillMethod::AlreadyExited);
            }
            return Err(format!("SIGTERM to {what} failed: {e}"));
        }
        loop {
            if gone(sig_target) {
                return Ok(KillMethod::TermAnswered(t0.elapsed()));
            }
            if t0.elapsed() >= grace {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if gone(sig_target) {
            return Ok(KillMethod::TermAnswered(t0.elapsed()));
        }
        if unsafe { signal_pid(sig_target, libc::SIGKILL) } != 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::ESRCH) {
                return Ok(KillMethod::TermAnswered(t0.elapsed()));
            }
            return Err(format!("SIGKILL to {what} failed: {e}"));
        }
        Ok(KillMethod::TermIgnoredThenKill(grace))
    }
}

/// SIGTERM a group we created, SIGKILL after `grace` when ignored.
/// Refuses degenerate groups and our own process/group before any syscall
/// (never broadcast). Returns `Err`, never a method, on refusal.
pub async fn term_then_kill(pgid: i32, grace: Duration) -> Result<KillMethod, String> {
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

#[cfg(all(test, unix))] // signal/sh/sleep fixtures are unix-only
#[allow(clippy::await_holding_lock)] // KILL_SERIAL is test-only serialization; holding it across await is its job
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::sync::atomic::Ordering;

    use crate::shell::spawn::spawn;
    use tokio::io::AsyncBufReadExt;

    fn sig_calls() -> u64 {
        SIGNAL_CALLS.load(Ordering::Relaxed)
    }

    /// Hold for the whole test: signals + the process-global counter must
    /// not interleave with other kill tests.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        KILL_SERIAL.lock().expect("kill serial")
    }

    /// Whole group reaped (orphans go to init, which reaps promptly).
    async fn group_gone(pgid: i32) -> bool {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(3) {
            if unsafe { libc::kill(-pgid, 0) } != 0
                && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    #[tokio::test]
    async fn guards_reject_degenerate_pgid() {
        let _serial = serial();
        let before = sig_calls();
        for pgid in [0, 1, -1, i32::MIN] {
            assert!(
                term_then_kill(pgid, Duration::from_millis(100))
                    .await
                    .is_err(),
                "pgid {pgid}"
            );
        }
        let me = std::process::id() as i32;
        assert!(
            term_then_kill(me, Duration::from_millis(100))
                .await
                .is_err()
        );
        assert!(
            term_then_kill(unsafe { libc::getpgrp() }, Duration::from_millis(100))
                .await
                .is_err()
        );
        assert_eq!(sig_calls(), before, "refusal happens before any syscall");
    }

    #[tokio::test]
    async fn group_kill_kills_tree() {
        use gray_core::agent::{Tool, ToolContext};
        use serde_json::json;
        use std::path::PathBuf;
        let _serial = serial();
        // slow.sh: sh parent + sleep child in one group.
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/shell")
            .join("slow.sh");
        let spawned = spawn(
            &format!("sh {} 30", fixture.display()),
            &std::env::temp_dir(),
        )
        .expect("spawn");
        let (pid, pgid) = (spawned.pid, spawned.pgid);
        let mut child = spawned.child;
        let _ = crate::shell::tools::bash::BashTool
            .execute(&ToolContext::default(), json!({"command": "true"}))
            .await;
        let _ = pid;
        let method = term_then_kill(pgid, Duration::from_secs(2))
            .await
            .expect("group kill ok");
        assert!(
            matches!(method, KillMethod::TermAnswered(_)),
            "slow.sh answers SIGTERM"
        );
        let st = child.wait().await.expect("reap");
        assert_eq!(st.signal(), Some(libc::SIGTERM));
        assert!(
            group_gone(pgid).await,
            "whole group (sh+sleep) must be gone"
        );
    }

    #[tokio::test]
    async fn term_ignored_escalates_to_sigkill() {
        let _serial = serial();
        // Ignored TERM survives exec: sleep never answers, SIGKILL ends it.
        // `echo ready` is the handshake: without it TERM could land before
        // the trap installs and the test would lie. The reaper keeps the
        // liveness probe honest (no zombie window).
        let spawned = spawn(
            "trap '' TERM; echo ready; exec sleep 30",
            &std::env::temp_dir(),
        )
        .expect("spawn trap");
        let pgid = spawned.pgid;
        let mut child = spawned.child;
        let stdout = child.stdout.take().expect("piped stdout");
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        let ready = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .expect("ready arrives")
            .expect("read ok");
        assert_eq!(ready.as_deref(), Some("ready"));
        let reaper = tokio::spawn(async move { child.wait().await });
        let m = term_then_kill(pgid, Duration::from_millis(300))
            .await
            .expect("escalates");
        assert!(
            matches!(m, KillMethod::TermIgnoredThenKill(_)),
            "expected escalation"
        );
        let st = tokio::time::timeout(Duration::from_secs(3), reaper)
            .await
            .expect("reaped")
            .expect("join ok")
            .expect("wait ok");
        assert_eq!(st.signal(), Some(libc::SIGKILL));
    }

    #[tokio::test]
    async fn already_exited_sends_no_signal() {
        let _serial = serial();
        let mut child = tokio::process::Command::new("true")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn true");
        let pid = child.id().expect("pid");
        child.wait().await.expect("wait true");
        SIGNAL_CALLS.store(0, Ordering::Relaxed);
        let m = escalate(pid as i32, "probe", Duration::from_millis(100))
            .await
            .expect("exited is ok");
        assert!(
            matches!(m, KillMethod::AlreadyExited),
            "exited sends no signal"
        );
        assert_eq!(sig_calls(), 0, "no signal for an exited pid");
    }
}
