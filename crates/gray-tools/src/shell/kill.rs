//! shell/kill.rs — escalation, group safety, pid reuse, port (brief 2D).
//!
//! Group kills go only to process groups we created (verified against start
//! time); foreign pid/port kills signal the single pid, never a group, and
//! only after an explicit `ask_allow_once` yes (fail-closed).

use std::io;
use std::time::{Duration, Instant};

use gray_core::agent::ToolContext;

use super::contract::{ExitReport, KillMethod, KillReport, KillTarget, TaskId, TaskState};
use super::registry::registry;

/// Grace for SIGTERM → SIGKILL escalation on the kill paths below.
const GRACE: Duration = Duration::from_secs(2);
/// How long `kill(Task)` waits for the waiter's exit report after signalling.
const EXIT_WAIT: Duration = Duration::from_secs(3);

#[cfg(test)]
pub(crate) static SIGNAL_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Serializes every test that signals or counts signals (this file plus the
/// registry shutdown test, which now routes through `term_then_kill`): the
/// signal counter is process-global, so parallel tests would pollute it.
#[cfg(test)]
pub(crate) static KILL_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Single raw-signal choke point (brief pitfall: mock behind a trait or
/// cfg(test) hook — the counter is the hook; probes with sig 0 don't count).
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

/// True when no process answers at `target` (`kill(target, 0)` → ESRCH).
/// Anything else (including EPERM) counts as alive — fail closed.
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

/// Shared SIGTERM → 100 ms poll → SIGKILL escalation. `sig_target` is the
/// `kill(2)` first arg (`-pgid` for our groups, `pid` for foreign singles).
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

/// Linux `/proc/{pid}/stat` field 22 (starttime); `None` elsewhere
/// (macOS best-effort, per brief). Third copy (`spawn.rs`, `registry.rs`
/// hold the others — shared helper needs orchestrator sign-off to touch
/// those files, so it stays duplicated; see P2D report).
#[cfg(target_os = "linux")]
fn start_ticks_for(pid: u32) -> Option<u64> {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()?
        .rsplit(')')
        .next()?
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

#[cfg(not(target_os = "linux"))]
fn start_ticks_for(_pid: u32) -> Option<u64> {
    None
}

/// True when `pid` is still the process it was at spawn. `None` ticks
/// (macOS/unknown) is best-effort true; an unreadable `/proc` entry with a
/// known baseline is false (gone or reused — refuse either way).
pub fn still_same_process(pid: u32, start_ticks: Option<u64>) -> bool {
    let Some(want) = start_ticks else {
        return true;
    };
    match start_ticks_for(pid) {
        Some(got) => got == want,
        None => false,
    }
}

fn comm_for(pid: u32) -> String {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .map(|s| s.trim().to_string())
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::process::Command::new("ps")
            .args(["-o", "comm=", "-p", &pid.to_string()])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".to_string())
    }
}

/// Resolve a listening TCP port to `(pid, comm)`.
/// Linux: `/proc/net/tcp|tcp6` (hex port, `0A` = LISTEN) → inode →
/// `/proc/*/fd` `socket:[inode]` scan. No `lsof` dependency (musl static).
/// macOS: `lsof -nP -tiTCP:{port} -sTCP:LISTEN`, comm via `ps`.
pub fn pid_for_port(port: u16) -> io::Result<Option<(u32, String)>> {
    #[cfg(target_os = "macos")]
    {
        pid_for_port_macos(port)
    }
    #[cfg(not(target_os = "macos"))]
    {
        pid_for_port_linux(port)
    }
}

#[cfg(not(target_os = "macos"))]
fn pid_for_port_linux(port: u16) -> io::Result<Option<(u32, String)>> {
    let mut want: Vec<String> = Vec::new();
    for table in ["tcp", "tcp6"] {
        let text = std::fs::read_to_string(format!("/proc/net/{table}"))?;
        for line in text.lines().skip(1) {
            // sl local_address rem_address st … inode — local port is hex.
            let c: Vec<&str> = line.split_whitespace().collect();
            if c.len() < 10 || c[3] != "0A" {
                continue;
            }
            let p = c[1].rsplit(':').next().unwrap_or("");
            if u16::from_str_radix(p, 16).ok() == Some(port) {
                want.push(format!("socket:[{}]", c[9]));
            }
        }
    }
    if want.is_empty() {
        return Ok(None);
    }
    for proc in std::fs::read_dir("/proc")?.flatten() {
        let pid: u32 = match proc.file_name().to_str().and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => continue,
        };
        let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            let link = std::fs::read_link(fd.path())
                .ok()
                .and_then(|p| p.into_os_string().into_string().ok())
                .unwrap_or_default();
            if want.contains(&link) {
                return Ok(Some((pid, comm_for(pid))));
            }
        }
    }
    Ok(None)
}

#[cfg(target_os = "macos")]
fn pid_for_port_macos(port: u16) -> io::Result<Option<(u32, String)>> {
    let out = std::process::Command::new("lsof")
        .args(["-nP", &format!("-tiTCP:{port}"), "-sTCP:LISTEN"])
        .output()?;
    let pid: u32 = match String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .and_then(|l| l.trim().parse().ok())
    {
        Some(p) => p,
        None => return Ok(None),
    };
    Ok(Some((pid, comm_for(pid))))
}

/// Kill per the frozen contract: task → group kill (verified); foreign
/// pid/port → prompt, fail closed, single-pid signal only.
pub async fn kill(
    target: KillTarget,
    session: &str,
    ctx: &ToolContext,
) -> Result<KillReport, String> {
    match target {
        KillTarget::Task(id) => kill_task(id, session).await,
        KillTarget::Pid(pid) => kill_pid(pid, session, ctx).await,
        KillTarget::Port(port) => {
            let (pid, comm) = pid_for_port(port)
                .map_err(|e| format!("port lookup failed: {e}"))?
                .ok_or_else(|| format!("nothing is listening on port {port}"))?;
            let mut rep = kill_pid(pid, session, ctx).await?;
            rep.describe = format!("port {port} → pid {pid} ({comm}): {}", rep.describe);
            Ok(rep)
        }
    }
}

async fn kill_task(id: TaskId, session: &str) -> Result<KillReport, String> {
    let reg = registry();
    let info = reg.get(session, id).ok_or_else(|| {
        let ids = reg
            .list(session)
            .iter()
            .map(|t| t.id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        if ids.is_empty() {
            format!("unknown task {id}: no tasks this session")
        } else {
            format!("unknown task {id}; tasks: {ids}")
        }
    })?;
    if let TaskState::Exited { report, .. } = &info.state {
        return Ok(KillReport {
            pid: info.pid,
            method: KillMethod::AlreadyExited,
            report: Some(report.clone()),
            describe: format!("{id} already exited ({}) — nothing signalled", report.label),
        });
    }
    let ticks = reg.start_ticks(session, id).flatten();
    if !still_same_process(info.pid, ticks) {
        return Err(format!(
            "pid {} is gone or was reused; not signalling",
            info.pid
        ));
    }
    let method = term_then_kill(info.pgid, GRACE).await?;
    let report = wait_exit(session, id).await;
    let describe = match &method {
        KillMethod::TermAnswered(d) => {
            format!(
                "{id} (pid {}) terminated after {:.1}s",
                info.pid,
                d.as_secs_f32()
            )
        }
        KillMethod::TermIgnoredThenKill(g) => format!(
            "{id} (pid {}) ignored SIGTERM; SIGKILL sent after {:.0}s",
            info.pid,
            g.as_secs_f32()
        ),
        KillMethod::AlreadyExited => format!("{id} exited on its own — nothing signalled"),
    };
    Ok(KillReport {
        pid: info.pid,
        method,
        report,
        describe,
    })
}

async fn kill_pid(pid: u32, session: &str, ctx: &ToolContext) -> Result<KillReport, String> {
    let reg = registry();
    // Ours → the task path (group kill, start-time verified).
    if let Some(id) = reg.find_task_by_pid(session, pid) {
        return kill_task(id, session).await;
    }
    // Foreign: prompt, fail closed; signal the single pid, never its group.
    let comm = comm_for(pid);
    let before = start_ticks_for(pid);
    if !super::guard::ask_allow_once(
        ctx,
        &format!("kill pid {pid} ({comm})"),
        "foreign-kill",
        &format!(
            "pid {pid} ({comm}) is not a gray task; it may belong to something you didn't start"
        ),
        "shell_kill(task_id=...) for gray tasks",
    )
    .await
    {
        return Err(format!(
            "kill pid {pid} ({comm}) needs user approval (foreign process) — refusing"
        ));
    }
    // The pid may have been recycled while the user was deciding.
    if before.is_some() && start_ticks_for(pid) != before {
        return Err(format!("pid {pid} changed while asking; not signalling"));
    }
    let method = escalate(pid as i32, &format!("pid {pid}"), GRACE).await?;
    let describe = match &method {
        KillMethod::TermAnswered(d) => {
            format!(
                "foreign pid {pid} ({comm}) terminated after {:.1}s",
                d.as_secs_f32()
            )
        }
        KillMethod::TermIgnoredThenKill(g) => format!(
            "foreign pid {pid} ({comm}) ignored SIGTERM; SIGKILL sent after {:.0}s",
            g.as_secs_f32()
        ),
        KillMethod::AlreadyExited => format!("foreign pid {pid} ({comm}) already gone"),
    };
    Ok(KillReport {
        pid,
        method,
        report: None,
        describe,
    })
}

/// Await the waiter's exit report (≤3 s) so task kills carry it; `None`
/// when the waiter hasn't reaped yet (the report lands via `mark_exited`).
async fn wait_exit(session: &str, id: TaskId) -> Option<ExitReport> {
    let reg = registry();
    // NOTE: a tokio watch send is LOST when zero receivers are subscribed,
    // so a waiter that marks before anyone subscribes leaves the channel at
    // None. The stored state (set under the lock before the sends) is
    // authoritative; the channel is only a wake-up. (Same trap awaits 2C's
    // wait=exit — it must check get() first, not just the channel.)
    let mut rx = reg.exit_rx(session, id)?;
    let deadline = Instant::now() + EXIT_WAIT;
    loop {
        match reg.get(session, id) {
            Some(info) => {
                if let TaskState::Exited { report, .. } = info.state {
                    return Some(report);
                }
            }
            None => return None, // gc'd mid-wait; don't hang
        }
        let now = Instant::now();
        if now >= deadline {
            return None;
        }
        let _ = tokio::time::timeout(deadline - now, rx.changed()).await;
    }
}

#[cfg(all(test, unix))] // signal/sh/sleep/lsof fixtures are unix-only (T3 windows gate)
#[allow(clippy::await_holding_lock)] // KILL_SERIAL is test-only serialization; holding it across await is its job
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::path::PathBuf;
    use std::process::Stdio;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::shell::exit::exit_report;
    use crate::shell::spawn::spawn;
    use tokio::io::AsyncBufReadExt;

    static SESS_N: AtomicU64 = AtomicU64::new(0);

    fn sess(tag: &str) -> String {
        format!(
            "killtest-{tag}-{}-{}",
            std::process::id(),
            SESS_N.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn sig_calls() -> u64 {
        SIGNAL_CALLS.load(Ordering::Relaxed)
    }

    /// Hold for the whole test: signals + the process-global counter must
    /// not interleave with other kill/registry tests.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        KILL_SERIAL.lock().expect("kill serial")
    }

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/shell")
            .join(name)
    }

    /// Spawn via the shell path and register; caller owns the child.
    fn spawn_reg(sess: &str, cmd: &str, task_n: u32) -> (TaskId, u32, i32, tokio::process::Child) {
        let spawned = spawn(cmd, &std::env::temp_dir(), TaskId(task_n)).expect("spawn");
        let (pid, pgid, child) = (spawned.pid, spawned.pgid, spawned.child);
        let id = registry().register(
            sess,
            &child,
            cmd,
            std::env::temp_dir().join(format!("killtest-{task_n}.log")),
        );
        (id, pid, pgid, child)
    }

    /// Mirror of the bash waiter: reap, then mark exited (fills exit_rx).
    fn detach_waiter(sess: String, id: TaskId, cmd: String, mut child: tokio::process::Child) {
        tokio::spawn(async move {
            if let Ok(st) = child.wait().await {
                registry().mark_exited(&sess, id, exit_report(st, &cmd));
            }
        });
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

    fn direct_child() -> tokio::process::Child {
        tokio::process::Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleep 30")
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

    #[test]
    fn still_same_process_self_and_gone() {
        let me = std::process::id();
        assert!(still_same_process(me, None));
        #[cfg(target_os = "linux")]
        {
            let t = start_ticks_for(me).expect("own ticks readable");
            assert!(still_same_process(me, Some(t)));
            assert!(!still_same_process(me, Some(t.wrapping_add(1))));
        }
        // No such pid → refuse (gone or reused, either way don't signal).
        assert!(!still_same_process(999_999_999, Some(12345)));
    }

    #[tokio::test]
    async fn task_group_kill_kills_tree() {
        let _serial = serial();
        let s = sess("tree");
        // slow.sh: sh parent + sleep child in one group.
        let cmd = format!("sh {} 30", fixture("slow.sh").display());
        let (id, pid, pgid, child) = spawn_reg(&s, &cmd, 7101);
        detach_waiter(s.clone(), id, cmd, child);
        let rep = tokio::time::timeout(
            Duration::from_secs(10),
            kill(KillTarget::Task(id), &s, &ToolContext::default()),
        )
        .await
        .expect("kill returns")
        .expect("kill ok");
        assert!(
            matches!(rep.method, KillMethod::TermAnswered(_)),
            "{}",
            rep.describe
        );
        assert!(
            group_gone(pgid).await,
            "whole group (sh+sleep) must be gone"
        );
        let body = rep.report.expect("waiter filled the exit report");
        assert_eq!(body.effective, 143, "{:?}", body.label);
        assert!(rep.describe.contains(&pid.to_string()), "{}", rep.describe);
    }

    #[tokio::test]
    async fn term_ignored_escalates_to_sigkill() {
        let _serial = serial();
        // Ignored TERM survives exec: sleep never answers, SIGKILL ends it.
        // `echo ready` is the handshake — without it TERM could land before
        // the trap installs (µs race) and the test would lie. The reaper
        // keeps the liveness probe honest (no zombie window).
        let spawned = spawn(
            "trap '' TERM; echo ready; exec sleep 30",
            &std::env::temp_dir(),
            TaskId(7109),
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
        let s = sess("exited");
        let mut child = tokio::process::Command::new("true")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn true");
        let id = registry().register(
            &s,
            &child,
            "true",
            PathBuf::from("/tmp/killtest-exited.log"),
        );
        let st = child.wait().await.expect("wait true");
        registry().mark_exited(&s, id, exit_report(st, "true"));
        SIGNAL_CALLS.store(0, Ordering::Relaxed);
        let rep = kill(KillTarget::Task(id), &s, &ToolContext::default())
            .await
            .expect("already-exited is ok");
        assert!(
            matches!(rep.method, KillMethod::AlreadyExited),
            "{}",
            rep.describe
        );
        assert_eq!(sig_calls(), 0, "no signal for an exited task");
    }

    #[tokio::test]
    async fn reused_pid_refused() {
        let _serial = serial();
        let s = sess("reused");
        let (id, pid, _pgid, child) = spawn_reg(&s, "sleep 30", 7102);
        registry().debug_set_start_ticks(&s, id, Some(0xdead_beef));
        let before = sig_calls();
        let err = kill(KillTarget::Task(id), &s, &ToolContext::default())
            .await
            .err()
            .expect("must refuse a reused pid");
        assert!(err.contains("gone or was reused"), "{err}");
        assert_eq!(sig_calls(), before, "refusal sends nothing");
        // Cleanup: restore ticks, reap via the waiter path, kill for real.
        registry().debug_set_start_ticks(&s, id, start_ticks_for(pid));
        detach_waiter(s.clone(), id, "sleep 30".to_string(), child);
        let rep = kill(KillTarget::Task(id), &s, &ToolContext::default())
            .await
            .expect("cleanup kill");
        assert!(
            matches!(rep.method, KillMethod::TermAnswered(_)),
            "{}",
            rep.describe
        );
    }

    #[tokio::test]
    async fn port_linux_resolves_listener() {
        let _serial = serial();
        let (listener, port) = hold_port().await;
        let found = pid_for_port(port)
            .expect("lookup runs")
            .expect("listener resolves");
        assert_eq!(found.0, std::process::id(), "our own listener");
        assert!(!found.1.is_empty());
        drop(listener);
    }

    #[tokio::test]
    async fn port_without_listener_errors() {
        let _serial = serial();
        let s = sess("port-empty");
        let (held, port) = hold_port().await;
        drop(held); // released: free barring an outside race
        let err = kill(KillTarget::Port(port), &s, &ToolContext::default())
            .await
            .err()
            .expect("nothing listening");
        assert!(
            err.contains(&format!("nothing is listening on port {port}")),
            "{err}"
        );
    }

    #[tokio::test]
    async fn port_foreign_fail_closed_without_bridge() {
        let _serial = serial();
        let (listener, port) = hold_port().await;
        let s = sess("port-deny");
        let before = sig_calls();
        let err = kill(KillTarget::Port(port), &s, &ToolContext::default())
            .await
            .err()
            .expect("fail closed");
        assert!(err.contains("approval"), "{err}");
        assert_eq!(sig_calls(), before, "denied prompt signals nothing");
        assert!(
            pid_for_port(port).expect("lookup runs").is_some(),
            "denied kill leaves the listener alone"
        );
        drop(listener);
    }

    #[tokio::test]
    async fn foreign_pid_fail_closed_without_bridge() {
        let _serial = serial();
        let mut child = direct_child();
        let pid = child.id().expect("pid");
        let s = sess("foreign-deny");
        let before = sig_calls();
        let err = kill(KillTarget::Pid(pid), &s, &ToolContext::default())
            .await
            .err()
            .expect("fail closed");
        assert!(err.contains("approval"), "{err}");
        assert_eq!(sig_calls(), before, "denied prompt signals nothing");
        assert_eq!(
            unsafe { libc::kill(pid as i32, 0) },
            0,
            "foreign process untouched"
        );
        child.kill().await.ok();
        let _ = child.wait().await;
    }

    struct YesAsker;
    impl gray_core::questions::QuestionAsker for YesAsker {
        fn ask(
            &self,
            _q: Vec<gray_core::questions::UserQuestion>,
            _b: bool,
        ) -> futures::future::BoxFuture<
            'static,
            Result<Vec<gray_core::questions::UserAnswer>, gray_core::error::CoreError>,
        > {
            Box::pin(async {
                Ok(vec![gray_core::questions::UserAnswer {
                    id: "q".to_string(),
                    answers: vec!["Run once".to_string()],
                }])
            })
        }
    }

    #[tokio::test]
    async fn foreign_pid_kills_with_approval() {
        let _serial = serial();
        let mut child = direct_child();
        let pid = child.id().expect("pid");
        // Reap concurrently: no zombie window, so the probe is exact and the
        // kill answers TERM instead of timing out into SIGKILL.
        let reaper = tokio::spawn(async move { child.wait().await });
        let s = sess("foreign-yes");
        let ctx = ToolContext {
            cwd: std::env::temp_dir(),
            questions: Some(gray_core::questions::QuestionBridge(std::sync::Arc::new(
                YesAsker,
            ))),
            session_id: Some(s.clone()),
            ..ToolContext::default()
        };
        let rep = tokio::time::timeout(
            Duration::from_secs(10),
            kill(KillTarget::Pid(pid), &s, &ctx),
        )
        .await
        .expect("kill returns")
        .expect("approved kill ok");
        assert!(
            matches!(rep.method, KillMethod::TermAnswered(_)),
            "{}",
            rep.describe
        );
        assert!(rep.describe.contains("foreign pid"), "{}", rep.describe);
        assert!(rep.describe.contains(&pid.to_string()), "{}", rep.describe);
        let st = tokio::time::timeout(Duration::from_secs(3), reaper)
            .await
            .expect("reaped after kill")
            .expect("join ok")
            .expect("wait ok");
        assert_eq!(st.signal(), Some(libc::SIGTERM));
    }

    /// Hold an ephemeral port (bind 0, keep the guard: no release race).
    async fn hold_port() -> (tokio::net::TcpListener, u16) {
        let l = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind ephemeral");
        let port = l.local_addr().expect("addr").port();
        (l, port)
    }
}
