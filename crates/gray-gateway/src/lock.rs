//! Cross-process gateway singleton (flock on `~/.gray/gateway.lock`).
//!
//! The REPL's `GATEWAY_HANDLE` is per-process, so two `gray` sessions both
//! pass the "already running" check and connect to Discord with the same bot
//! token — Discord allows concurrent gateway sessions, so both receive every
//! message and both reply. This module is the cross-process guard: the first
//! session to start the gateway holds an exclusive flock for the gateway's
//! lifetime; later sessions see `WouldBlock` and run gray-only.
//!
//! flock releases on process death, so there is no stale `.pid` file to clean.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

/// Shown when a session skips the gateway because another session owns it.
pub const ALREADY_RUNNING_MESSAGE: &str = "Gray is already running on another session — gateway owned by that session, this session runs gray-only.";

pub fn gateway_lock_path() -> PathBuf {
    crate::config::gray_home_dir()
        .map(|h| h.join("gateway.lock"))
        .unwrap_or_else(|_| std::env::temp_dir().join("gray-gateway.lock"))
}

pub fn gateway_lock_path_in(home: &Path) -> PathBuf {
    home.join("gateway.lock")
}

/// Try to become the gateway owner. `Some(file)` = lock held — keep the
/// `File` alive for the gateway's lifetime. `None` = another process holds
/// it. Never blocks.
pub fn try_acquire_gateway_lock() -> Option<std::fs::File> {
    try_acquire_gateway_lock_at(&gateway_lock_path())
}

fn open_lock_file(path: &Path) -> Option<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    // truncate(false): the file is only a lock token, never wipe it.
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .ok()
}

pub fn try_acquire_gateway_lock_at(path: &Path) -> Option<std::fs::File> {
    let f = open_lock_file(path)?;
    match f.try_lock() {
        Ok(()) => Some(f),
        Err(std::fs::TryLockError::WouldBlock) => None,
        // Locking unsupported on this fs: degrade to old behavior (run).
        Err(_) => Some(f),
    }
}

/// Probe without acquiring: true while another process holds the lock.
pub fn gateway_locked_elsewhere() -> bool {
    gateway_locked_elsewhere_at(&gateway_lock_path())
}

pub fn gateway_locked_elsewhere_at(path: &Path) -> bool {
    let Some(f) = open_lock_file(path) else {
        return false;
    };
    match f.try_lock() {
        Ok(()) => {
            let _ = f.unlock();
            false
        }
        Err(std::fs::TryLockError::WouldBlock) => true,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_path_joins_home() {
        assert_eq!(
            gateway_lock_path_in(Path::new("/home/u/.gray")),
            PathBuf::from("/home/u/.gray/gateway.lock")
        );
    }

    #[test]
    fn already_running_message_names_other_session() {
        assert!(
            ALREADY_RUNNING_MESSAGE.contains("already running on another session"),
            "boot message must say: {ALREADY_RUNNING_MESSAGE}"
        );
    }

    #[test]
    fn second_acquire_fails_while_held() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gateway.lock");
        let held = try_acquire_gateway_lock_at(&path);
        assert!(held.is_some(), "first acquire must succeed");
        assert!(
            try_acquire_gateway_lock_at(&path).is_none(),
            "second acquire must fail while held"
        );
        assert!(gateway_locked_elsewhere_at(&path), "probe must report held");
        drop(held);
        assert!(
            try_acquire_gateway_lock_at(&path).is_some(),
            "acquire must succeed after release"
        );
        assert!(
            !gateway_locked_elsewhere_at(&path),
            "probe must report free after release"
        );
    }
}
