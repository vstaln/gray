//! Persistent error registry backing the future Errors tab.
//!
//! Best-effort append-only log at `$GRAY_HOME/errors.json`, capped at
//! [`MAX_ENTRIES`] (oldest dropped). All entry points swallow IO errors:
//! recording must never break the operation that failed.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One recorded operation failure. On disk oldest-first; [`list`] flips.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEntry {
    pub ts_secs: u64,
    pub source: String,
    pub item: String,
    pub message: String,
}

/// Cap on stored entries; oldest are dropped beyond it.
pub const MAX_ENTRIES: usize = 100;

fn errors_path() -> PathBuf {
    crate::gray_home().join("errors.json")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

fn read_all() -> Vec<ErrorEntry> {
    match std::fs::read_to_string(errors_path()) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Append `{now, source, item, message}`, dropping oldest beyond the cap.
/// Never panics, never errors — IO failures are ignored.
pub fn record(source: &str, item: &str, message: String) {
    let mut entries = read_all();
    entries.push(ErrorEntry {
        ts_secs: now_secs(),
        source: source.to_string(),
        item: item.to_string(),
        message,
    });
    if entries.len() > MAX_ENTRIES {
        entries.drain(..entries.len() - MAX_ENTRIES);
    }
    if let Some(parent) = errors_path().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(raw) = serde_json::to_string(&entries) {
        let _ = std::fs::write(errors_path(), raw);
    }
}

/// Newest-first. Missing/corrupt file → empty vec, no error.
pub fn list() -> Vec<ErrorEntry> {
    let mut entries = read_all();
    entries.reverse();
    entries
}

/// Drop all entries (best-effort; missing file is a no-op).
pub fn clear() {
    let _ = std::fs::remove_file(errors_path());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Point `GRAY_HOME` at a fresh tempdir. Must be called under the
    /// shared `ops::tests::ENV_GUARD` (one process, one process-global env).
    fn use_errors_env() -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        // SAFETY: serialized by ENV_GUARD.
        unsafe {
            std::env::set_var("GRAY_HOME", home.path());
        }
        home
    }

    #[test]
    fn record_list_round_trip() {
        let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
        let _home = use_errors_env();
        record("index", "demo", "boom".to_string());
        let entries = list();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source, "index");
        assert_eq!(entries[0].item, "demo");
        assert_eq!(entries[0].message, "boom");
    }

    #[test]
    fn cap_evicts_oldest() {
        let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
        let _home = use_errors_env();
        for i in 0..105 {
            record("s", &format!("item-{i}"), format!("m{i}"));
        }
        let entries = list();
        assert_eq!(entries.len(), 100);
        assert_eq!(entries[0].item, "item-104");
        assert_eq!(entries.last().unwrap().item, "item-5");
        assert!(!entries.iter().any(|e| e.item == "item-0"));
    }

    #[test]
    fn clear_empties_registry() {
        let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
        let _home = use_errors_env();
        record("s", "i", "m".to_string());
        assert_eq!(list().len(), 1);
        clear();
        assert!(list().is_empty());
    }

    #[test]
    fn corrupt_file_returns_empty_and_heals_on_record() {
        let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
        let _home = use_errors_env();
        std::fs::create_dir_all(crate::gray_home()).unwrap();
        std::fs::write(crate::gray_home().join("errors.json"), "{not json").unwrap();
        assert!(list().is_empty());
        record("s", "i", "m".to_string());
        assert_eq!(list().len(), 1);
    }
}
