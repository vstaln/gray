use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEntry {
    pub ts_secs: u64,
    pub source: String,
    pub item: String,
    pub message: String,
}

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

pub fn list() -> Vec<ErrorEntry> {
    let mut entries = read_all();
    entries.reverse();
    entries
}

pub fn clear() {
    let _ = std::fs::remove_file(errors_path());
}
