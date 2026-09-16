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
        ts_secs: crate::now_secs(),
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

#[path = "errors_tests.rs"]
#[cfg(test)]
mod tests;
