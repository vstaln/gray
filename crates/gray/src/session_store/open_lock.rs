use super::*;

pub(super) static OPEN_LOCKS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<PathBuf, std::sync::Weak<OpenLockInner>>>,
> = std::sync::LazyLock::new(Default::default);

#[derive(Debug)]
pub(super) struct OpenLockInner {
    pub(super) _file: std::fs::File,
}

#[derive(Debug)]
pub struct SessionOpenGuard {
    pub(super) _inner: std::sync::Arc<OpenLockInner>,
}

pub(super) fn take_registered_open_lock(open_path: &Path) -> Option<std::sync::Arc<OpenLockInner>> {
    let mut held = OPEN_LOCKS.lock().expect("open-lock registry poisoned");
    if let Some(inner) = held.get(open_path).and_then(std::sync::Weak::upgrade) {
        return Some(inner);
    }
    held.remove(open_path);
    None
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct OpenHolder {
    pub(super) pid: u32,
    pub(super) started_ms: u64,
}

pub(super) fn write_open_holder(file: &mut std::fs::File, path: &Path) {
    use std::io::{Seek, SeekFrom, Write};
    #[cfg(not(windows))]
    let _ = path;
    let holder = OpenHolder {
        pid: std::process::id(),
        started_ms: now_millis(),
    };
    let Ok(json) = serde_json::to_string(&holder) else {
        return;
    };
    let _ = file.seek(SeekFrom::Start(0));
    let _ = file.set_len(0);
    let _ = file.write_all(json.as_bytes());
    let _ = file.write_all(b"\n");
    let _ = file.sync_all();
    #[cfg(windows)]
    {
        let hint_path = path.with_extension("open-holder");
        let _ = std::fs::remove_file(&hint_path);
        if let Some(parent) = path.parent()
            && let Ok(mut hint) = tempfile::NamedTempFile::new_in(parent)
            && hint.write_all(json.as_bytes()).is_ok()
            && hint.as_file().sync_all().is_ok()
        {
            let _ = hint.persist(hint_path);
        }
    }
}

pub(crate) fn running_sessions(root: &Path) -> Vec<(String, Option<u32>)> {
    let Ok(rd) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    rd.flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension()? != "open" {
                return None;
            }
            let id = path.file_stem()?.to_str()?.to_string();
            if !valid_session_id(&id) || !open_lock_held_at(&path) {
                return None;
            }
            Some((id, read_open_holder(&path)))
        })
        .collect()
}

pub(super) fn open_lock_held_at(open_path: &Path) -> bool {
    if let Ok(held) = OPEN_LOCKS.lock()
        && held
            .get(open_path)
            .and_then(std::sync::Weak::upgrade)
            .is_some()
    {
        return true;
    }
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(open_path)
    else {
        return false;
    };
    match file.try_lock() {
        Ok(()) => false,
        Err(std::fs::TryLockError::WouldBlock) => true,
        Err(std::fs::TryLockError::Error(_)) => false,
    }
}

/// On Windows `/restart` spawns the new process while the old one is
/// still exiting, so the session open-lock can briefly remain held by
/// the dying parent. Give the holder a short grace period to release
/// the lock before reporting the session as locked. A restarting parent
/// that sets RESTART_PARENT_ENV gets a longer budget, and a lock held
/// by any other pid then fails immediately; without the hint (e.g. the
/// parent is an older build) a few seconds still covers the exit.
pub(super) async fn wait_out_restart_parent(file: &mut std::fs::File, open_path: &Path) -> bool {
    let wait_pid = RESTART_PARENT_PID.get().copied().flatten();
    let budget = if wait_pid.is_some() {
        RESTART_PARENT_WAIT
    } else {
        std::time::Duration::from_secs(3)
    };
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        match file.try_lock() {
            Ok(()) => return true,
            Err(std::fs::TryLockError::WouldBlock) => {
                if let (Some(wp), Some(holder)) = (wait_pid, read_open_holder(open_path))
                    && holder != wp
                {
                    return false;
                }
            }
            Err(std::fs::TryLockError::Error(e)) => {
                log::warn!(
                    "session open-lock unsupported on {} ({e}); proceeding unlocked",
                    open_path.display()
                );
                return true;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Releases the OS-level lock on every session open-file this process
/// holds, without dropping the guards. Windows `/restart` needs this:
/// the spawned child has to acquire the same lock while the parent is
/// still alive supervising it. The parent stays in `child.wait()` so
/// the launching shell cannot reclaim the console and steal input.
#[cfg(not(unix))]
pub(crate) fn release_open_locks() {
    let Ok(held) = OPEN_LOCKS.lock() else {
        return;
    };
    for inner in held.values().filter_map(std::sync::Weak::upgrade) {
        if let Err(e) = inner._file.unlock() {
            log::warn!("failed to release session open-lock before restart: {e}");
        }
    }
}

pub(super) fn read_open_holder(path: &Path) -> Option<u32> {
    #[cfg(windows)]
    let path = path.with_extension("open-holder");
    let content = std::fs::read_to_string(path).ok()?;
    let content = content.trim();
    serde_json::from_str::<OpenHolder>(content)
        .ok()
        .map(|h| h.pid)
        .or_else(|| content.parse::<u32>().ok())
}
