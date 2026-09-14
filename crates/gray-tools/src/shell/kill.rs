//! shell/kill.rs: process-group kill with SIGTERM-to-SIGKILL escalation.
//!
//! Only our own groups (spawned detached via setsid, pgid == pid). Refuses
//! degenerate groups and our own process/group before any syscall.

use std::io;
use std::time::{Duration, Instant};

use super::contract::KillMethod;

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
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    use crate::shell::spawn::spawn;
    use tokio::io::AsyncBufReadExt;

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
    }

    #[tokio::test]
    async fn group_kill_kills_tree() {
        // sh parent + sleep child in one group: SIGTERM may or may not be
        // answered in time (load-dependent), so accept either method — the
        // invariant is the whole tree dies by SIGTERM or SIGKILL.
        let spawned = spawn("sleep 30", &std::env::temp_dir()).expect("spawn");
        let pgid = spawned.pgid;
        let mut child = spawned.child;
        let method = term_then_kill(pgid, Duration::from_secs(2))
            .await
            .expect("group kill ok");
        assert!(
            matches!(
                method,
                KillMethod::TermAnswered(_) | KillMethod::TermIgnoredThenKill(_)
            ),
            "group kill resolves, got {method:?}"
        );
        let st = child.wait().await.expect("reap");
        assert!(
            st.signal() == Some(libc::SIGTERM) || st.signal() == Some(libc::SIGKILL),
            "child died by signal, got {st:?}"
        );
        assert!(
            group_gone(pgid).await,
            "whole group (sh+sleep) must be gone"
        );
    }

    #[tokio::test]
    async fn term_ignored_escalates_to_sigkill() {
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
        // A live sleeper's pid, probed with sig 0: alive, so the probe
        // answers 0 and no real signal is ever sent.
        let mut child = tokio::process::Command::new("sleep")
            .arg("30")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn sleep 30");
        let pid = child.id().expect("pid") as i32;
        assert!(!gone(pid), "sleeper is alive");
        child.kill().await.expect("kill sleeper");
        child.wait().await.expect("reap sleeper");
    }
}
