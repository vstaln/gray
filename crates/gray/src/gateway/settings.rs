//! `~/.gray/gateway/config.json`: how the always-on gateway behaves.
//!
//! Every field has a default, so a missing or partial file is a working
//! configuration: heartbeat on but idle until `HEARTBEAT.md` has content,
//! no owner (every chat is its own session), two concurrent turns.

use std::path::Path;

use crate::cron::store::Origin as Route;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Owner surfaces; the first is where autonomous output goes when the
    /// session has no route of its own yet.
    pub owners: Vec<Route>,
    pub heartbeat: Heartbeat,
    /// Turns running at once across all sessions.
    pub max_turns: usize,
    /// Wall clock for one turn before it is killed.
    pub turn_timeout_secs: u64,
    /// Working directory for turns (default: the user's home).
    pub workdir: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Heartbeat {
    pub enabled: bool,
    pub every_mins: u64,
    /// `HH:MM-HH:MM` local; outside it the heartbeat does not wake.
    pub active_hours: Option<String>,
}

impl Default for Heartbeat {
    fn default() -> Self {
        Self {
            enabled: true,
            every_mins: 30,
            active_hours: Some("08:00-22:00".to_string()),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            owners: Vec::new(),
            heartbeat: Heartbeat::default(),
            max_turns: 2,
            turn_timeout_secs: 1800,
            workdir: None,
        }
    }
}

impl Settings {
    /// Read the file; any problem reads as defaults (logged), never a crash
    /// loop for a typo.
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("config.json");
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                log::warn!(
                    "gateway: {} unreadable ({e}); using defaults",
                    path.display()
                );
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }
}

/// A local `HH:MM-HH:MM` window; wraps midnight when end < start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub start: u32,
    pub end: u32,
}

impl Window {
    pub fn parse(raw: &str) -> Option<Self> {
        let (a, b) = raw.trim().split_once('-')?;
        Some(Self {
            start: minutes(a)?,
            end: minutes(b)?,
        })
    }

    /// `minute` of the day (0..1440) is inside the window.
    pub fn contains(&self, minute: u32) -> bool {
        if self.start == self.end {
            return true;
        }
        if self.start < self.end {
            (self.start..self.end).contains(&minute)
        } else {
            minute >= self.start || minute < self.end
        }
    }
}

fn minutes(raw: &str) -> Option<u32> {
    let (h, m) = raw.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h <= 24 && m < 60 && h * 60 + m <= 1440).then_some((h * 60 + m) % 1440)
}

/// Local minute of the day for a unix timestamp.
pub fn local_minute(at: i64) -> u32 {
    use chrono::Timelike as _;
    let t = chrono::DateTime::from_timestamp(at, 0)
        .unwrap_or_default()
        .with_timezone(&chrono::Local);
    t.hour() * 60 + t.minute()
}

#[path = "settings_tests.rs"]
#[cfg(test)]
mod tests;
