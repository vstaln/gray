use std::time::Duration;

#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::time::Instant;

#[cfg(unix)]
unsafe fn signal_pid(target: i32, sig: i32) -> i32 {
    unsafe { libc::kill(target, sig) }
}

#[cfg(all(not(unix), not(windows)))]
unsafe fn signal_pid(_target: i32, _sig: i32) -> i32 {
    -1
}

#[cfg(unix)]
static TRACKED_PGIDS: std::sync::Mutex<std::collections::BTreeSet<i32>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

#[cfg(unix)]
fn track_group(pgid: i32) {
    if pgid > 1 {
        TRACKED_PGIDS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(pgid);
    }
}

#[cfg(unix)]
pub(crate) fn untrack_group(pgid: i32) {
    TRACKED_PGIDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&pgid);
}

pub fn kill_tracked_groups() {
    #[cfg(unix)]
    {
        let pgids = {
            let mut set = TRACKED_PGIDS.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *set)
        };
        for pgid in pgids {
            if group_alive(pgid) {
                unsafe {
                    signal_pid(-pgid, libc::SIGKILL);
                }
            }
        }
    }
}

#[cfg(not(windows))]
#[cfg_attr(not(unix), allow(dead_code))]
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

#[cfg(unix)]
pub(crate) fn group_alive(pgid: i32) -> bool {
    pgid > 1
        && pgid != std::process::id() as i32
        && pgid != unsafe { libc::getpgrp() }
        && !gone(-pgid)
}

#[cfg(not(windows))]
async fn escalate(sig_target: i32, what: &str, grace: Duration) -> Result<(), String> {
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

#[cfg(windows)]
pub async fn term_then_kill(job: &super::windows::Job, _grace: Duration) -> Result<(), String> {
    job.terminate()
        .map_err(|e| format!("terminating shell job failed: {e}"))
}

#[cfg(not(windows))]
pub(crate) struct GroupGuard {
    pub(crate) pgid: i32,
    armed: bool,
}

#[cfg(not(windows))]
impl GroupGuard {
    pub(crate) fn new(pgid: i32) -> Self {
        #[cfg(unix)]
        track_group(pgid);
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
            untrack_group(self.pgid);
        }
    }
}
