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
#[cfg(not(unix))]
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
#[cfg(not(unix))]
pub fn pid_alive(pid: u32) -> bool {
    pid != 0 && pid == std::process::id()
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
        started_at: gray_cron::now_secs(),
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_read_and_remove_roundtrip() {
        let home = tempfile::tempdir().unwrap();
        let rec = claim(home.path()).unwrap();
        assert_eq!(rec.pid, std::process::id());
        let seen = read(home.path()).unwrap();
        assert_eq!(seen.pid, rec.pid);
        assert!(alive(&seen));
        assert!(running(home.path()).is_some());
        remove_owned(home.path(), rec.pid);
        assert!(read(home.path()).is_none());
    }

    #[test]
    fn live_record_refuses_a_second_claim() {
        let home = tempfile::tempdir().unwrap();
        claim(home.path()).unwrap(); // ours, and it is alive
        let err = claim(home.path()).unwrap_err().to_string();
        assert!(err.contains("already running"), "{err}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recycled_pid_reads_as_stale_and_is_replaced() {
        let home = tempfile::tempdir().unwrap();
        let mut rec = claim(home.path()).unwrap();
        // Same pid, different start time: the pid was recycled since.
        rec.start_time = Some(rec.start_time.unwrap_or(1) + 1_000_000);
        std::fs::write(
            record_path(home.path()),
            serde_json::to_string(&rec).unwrap(),
        )
        .unwrap();
        assert!(running(home.path()).is_none());
        let fresh = claim(home.path()).unwrap();
        assert_eq!(fresh.pid, std::process::id());
        assert!(alive(&read(home.path()).unwrap()));
    }

    #[test]
    fn corrupt_record_is_replaced_on_claim() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(record_path(home.path()), "{not json").unwrap();
        assert!(read(home.path()).is_none());
        assert!(claim(home.path()).is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn proc_start_time_matches_our_own_process() {
        assert!(proc_start_time(std::process::id()).is_some());
    }
}
