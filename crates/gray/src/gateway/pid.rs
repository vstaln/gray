//! PID claim file: `$GRAY_HOME/gateway.pid`.
//!
//! Doctrine (ported from hermes' gateway): the claim is one JSON record
//! written with `create_new` (O_EXCL), so two `gray gateway run` cannot both
//! win. A record is live only when its pid exists *and* its `start_time`
//! matches the live process — /proc starttime is what makes PID reuse
//! detectable (`kill(pid, 0)` alone cannot tell a stale record from a
//! recycled pid). The control socket stays the preferred liveness probe; this
//! file is the lock and the fallback.

use std::io::Write as _;
use std::path::{Path, PathBuf};

/// One claim record. `start_time` is `None` on platforms without /proc.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PidRecord {
    pub kind: String,
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_time: Option<u64>,
    #[serde(default)]
    pub argv: Vec<String>,
    pub home: String,
    pub version: String,
    pub started_at: i64,
}

pub fn record_path(home: &Path) -> PathBuf {
    home.join("gateway.pid")
}

/// Boot-relative start tick of `pid` (field 22 of `/proc/<pid>/stat`), or
/// `None` when unreadable (dead process, or no /proc on this platform).
#[cfg(unix)]
pub fn proc_start_time(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm may contain spaces and parentheses: split after the *last* ')'.
    let rest = stat.rsplit_once(')')?.1;
    rest.split_whitespace().nth(19)?.parse().ok()
}

/// Non-unix fallback: no /proc starttime, so PID-reuse detection degrades
/// to the existence probe (same tradeoff the old doc comment states).
#[cfg(not(any(unix, windows)))]
pub fn proc_start_time(_pid: u32) -> Option<u64> {
    None
}

/// Signal-0 probe: exists (possibly owned by another user) == alive.
#[cfg(unix)]
pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    // SAFETY: kill with signal 0 delivers nothing; it only runs the
    // existence/permission checks for the given pid.
    unsafe {
        libc::kill(pid as libc::pid_t, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

/// Non-unix fallback: no signal-0 probe available. A record can only be
/// trusted when its pid is our own process (fresh claim in this session);
/// anything else reads as not-running so a new claim replaces it.
#[cfg(not(any(unix, windows)))]
pub fn pid_alive(pid: u32) -> bool {
    pid != 0 && pid == std::process::id()
}

/// Query a Windows process without requiring termination rights.
#[cfg(windows)]
fn windows_process(pid: u32) -> Option<(bool, Option<u64>)> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_ACCESS_DENIED, FILETIME, GetLastError},
        System::Threading::{
            GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        },
    };
    if pid == 0 {
        return None;
    }
    // SAFETY: handle is checked, used only for process queries, and closed.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return (GetLastError() == ERROR_ACCESS_DENIED).then_some((true, None));
        }
        // A failed query proves nothing about liveness: treat it as gone, so a
        // stale record is cleaned up instead of pinning the claim forever.
        let mut code = 0;
        let queried = GetExitCodeProcess(handle, &mut code) != 0;
        let alive = queried && code == 259; // STILL_ACTIVE
        let mut creation: FILETIME = std::mem::zeroed();
        let mut exit: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        let start = (GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user)
            != 0)
            .then_some(
                (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime),
            );
        CloseHandle(handle);
        Some((alive, start))
    }
}

#[cfg(windows)]
pub fn pid_alive(pid: u32) -> bool {
    windows_process(pid).is_some_and(|(alive, _)| alive)
}

#[cfg(windows)]
pub fn proc_start_time(pid: u32) -> Option<u64> {
    windows_process(pid).and_then(|(_, start)| start)
}

/// Live == the pid exists and its start time still matches the record.
pub fn alive(rec: &PidRecord) -> bool {
    if !pid_alive(rec.pid) {
        return false;
    }
    match (rec.start_time, proc_start_time(rec.pid)) {
        (Some(claimed), Some(now)) => claimed == now,
        // No /proc to compare against: existence is all this platform offers.
        _ => true,
    }
}

/// Read the record as-is; missing or corrupt reads as `None`.
pub fn read(home: &Path) -> Option<PidRecord> {
    let body = std::fs::read_to_string(record_path(home)).ok()?;
    serde_json::from_str(&body).ok()
}

/// The live record owning `home`, if any. Stale records read as `None`.
pub fn running(home: &Path) -> Option<PidRecord> {
    read(home).filter(alive)
}

/// How long a starter waits for a competing claim before failing (the
/// claim decision itself must not be observable in two halves by two
/// processes).
const CLAIM_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Exclusive cross-process guard for the whole claim decision (audit #1):
/// the stale-record replacement path *reads* the record and *removes* it in
/// separate filesystem operations, so two starters can each decide the
/// record is stale, and the loser's `remove_file` can delete the winner's
/// freshly created claim — both then believe they own the home. Holding one
/// lock across read -> remove -> create makes the decision atomic. The
/// lock file itself is never removed (it is the lock), and a filesystem
/// without flock degrades to no guard rather than failing startup.
fn hold_claim_lock(home: &Path) -> Option<std::fs::File> {
    let path = record_path(home).with_extension("pid.lock");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // The lock names the gateway's pid and start time: keep it owner-only
    // whatever the umask is.
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).truncate(false).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let f = opts.open(&path).ok()?;
    let deadline = std::time::Instant::now() + CLAIM_LOCK_TIMEOUT;
    loop {
        match f.try_lock() {
            Ok(()) => return Some(f),
            Err(std::fs::TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    log::warn!(
                        "another starter is claiming {}; proceeding unlocked",
                        home.display()
                    );
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(std::fs::TryLockError::Error(e)) => {
                log::warn!(
                    "gateway claim locking unsupported on {} ({e}); proceeding unlocked",
                    path.display()
                );
                return None;
            }
        }
    }
}

/// Claim `home` for this process; `Err` when a live sibling already owns it.
/// A stale or corrupt record is replaced — the retry keeps the race window
/// exactly one `create_new` wide.
pub fn claim(home: &Path) -> anyhow::Result<PidRecord> {
    std::fs::create_dir_all(home)?;
    let me = std::process::id();
    let rec = PidRecord {
        kind: "gray-gateway".to_string(),
        pid: me,
        start_time: proc_start_time(me),
        argv: std::env::args().collect(),
        home: home.display().to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        started_at: crate::cron::now_secs(),
    };
    // The whole read -> remove -> create decision runs under one lock (the
    // retry loop is unchanged; only the window is now atomic).
    let _guard = hold_claim_lock(home);
    for _ in 0..2 {
        match write_new(home, &rec) {
            Ok(()) => return Ok(rec),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if let Some(old) = read(home).filter(alive) {
                    anyhow::bail!(
                        "another gateway is already running (pid {}, home {})",
                        old.pid,
                        old.home
                    );
                }
                // Stale (dead pid, recycled pid, or corrupt): replace it.
                let _ = std::fs::remove_file(record_path(home));
            }
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!(
        "gateway pid file is contested by another starter: {}",
        record_path(home).display()
    )
}

/// Remove the claim file when this process owns it. Best-effort: shutdown
/// must never fail on file cleanup.
pub fn remove_owned(home: &Path, pid: u32) {
    if read(home).is_some_and(|rec| rec.pid == pid) {
        let _ = std::fs::remove_file(record_path(home));
    }
}

fn write_new(home: &Path, rec: &PidRecord) -> std::io::Result<()> {
    let path = record_path(home);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let body = serde_json::to_string_pretty(rec).map_err(std::io::Error::other)?;
    f.write_all(body.as_bytes())?;
    f.write_all(b"\n")?;
    f.sync_all()?;
    Ok(())
}

#[path = "pid_tests.rs"]
#[cfg(test)]
mod tests;
