//! Events: everything that can wake the agent, spooled to disk before it runs.
//!
//! A `gateway send`, a heartbeat, a `gateway wake` and a turn cut short
//! by a restart are all the same thing here: an
//! [`Event`] for a session key, admitted as one file under `events/` so a
//! crash between "accepted" and "answered" loses nothing.

use std::path::{Path, PathBuf};

use crate::cron::store::Origin as Route;

/// Why a turn exists (provenance). User turns are held to the strict silence
/// rule; every other kind is autonomous output and goes through policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    User,
    Heartbeat,
    Trigger,
    System,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::User => "user",
            Kind::Heartbeat => "heartbeat",
            Kind::Trigger => "trigger",
            Kind::System => "system",
        }
    }

    /// Anything not typed by a person right now.
    pub fn autonomous(self) -> bool {
        self != Kind::User
    }
}

/// The session every owner surface, the CLI and the heartbeat share.
pub const MAIN: &str = "main";

/// Biggest event text we keep (argv-safe once several are batched).
pub const MAX_TEXT: usize = 24 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Event {
    pub id: String,
    pub kind: Kind,
    /// Session key: `main`, `chat:<platform>:<chat>[:<thread>]`, `job:<id>`.
    pub key: String,
    pub text: String,
    /// Where a reply goes; `None` for autonomous wakes (the session's
    /// last route or the owner's is used).
    #[serde(default)]
    pub route: Option<Route>,
    pub created_at: i64,
    /// Times a turn carrying this event was started and cut short.
    #[serde(default)]
    pub attempt: u32,
}

impl Event {
    pub fn new(kind: Kind, key: &str, text: &str, route: Option<Route>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().simple().to_string(),
            kind,
            key: key.to_string(),
            text: cap(text, MAX_TEXT),
            route,
            created_at: crate::cron::now_secs(),
            attempt: 0,
        }
    }
}

/// Char-boundary-safe truncation with a marker.
pub fn cap(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[… truncated]", &text[..end])
}

/// Files in a spool dir, oldest first (names sort by time).
pub fn spool_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            !n.starts_with('.') && e.path().is_file()
        })
        .map(|e| e.path())
        .collect();
    paths.sort();
    paths
}

/// Spool name that sorts by admission time.
pub fn spool_name(id: &str) -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{ms:020}-{id}.json")
}

/// Admit one event: atomic 0600 file in `events/`.
pub fn admit(dir: &Path, event: &Event) -> anyhow::Result<PathBuf> {
    let path = dir.join("events").join(spool_name(&event.id));
    crate::cron::store::atomic_write_json(&path, event)?;
    Ok(path)
}

/// Every admitted event, oldest first. Unreadable files are moved aside
/// (`events/bad/`) so one corrupt entry cannot wedge the queue.
pub fn pending(dir: &Path) -> Vec<(PathBuf, Event)> {
    let mut out = Vec::new();
    for path in spool_files(&dir.join("events")) {
        let parsed = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Event>(&b).ok());
        match parsed {
            Some(ev) => out.push((path, ev)),
            None => {
                log::warn!("gateway: unreadable event {}, moved aside", path.display());
                let bad = dir.join("events").join("bad");
                let _ = std::fs::create_dir_all(&bad);
                if let Some(name) = path.file_name() {
                    let _ = std::fs::rename(&path, bad.join(name));
                }
            }
        }
    }
    out
}

#[path = "event_tests.rs"]
#[cfg(test)]
mod tests;
