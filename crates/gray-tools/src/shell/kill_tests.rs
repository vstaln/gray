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
    // answered in time (load-dependent), so accept either escalation
    // path — the invariant is the whole tree dies by SIGTERM or SIGKILL.
    let spawned = spawn("sleep 30", &std::env::temp_dir(), None).expect("spawn");
    let pgid = spawned.pgid;
    let mut child = spawned.child;
    // Reap concurrently, like the real caller: a zombie left unreaped
    // makes kill(-pgid, SIGKILL) return EPERM on macOS (nothing
    // signalable remains), which is not a kill failure. Reaping as soon
    // as the child dies empties the group before escalation fires.
    let (res, st) = tokio::join!(term_then_kill(pgid, Duration::from_secs(2)), child.wait());
    res.expect("group kill ok");
    let st = st.expect("reap");
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
        None,
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
    term_then_kill(pgid, Duration::from_millis(300))
        .await
        .expect("escalates");
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
