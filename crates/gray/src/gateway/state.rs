//! Runtime state snapshot: `$GRAY_HOME/gateway.state.json`.
//!
//! The PID file is the lock and dies with the process; this record survives
//! (crash included), so `gateway status` can say *why* the last run ended
//! while nothing is running. Hermes keeps the same split: pid record vs
//! runtime status.

use std::path::{Path, PathBuf};

pub const STATE_STARTING: &str = "starting";
pub const STATE_RUNNING: &str = "running";
pub const STATE_STOPPED: &str = "stopped";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RuntimeState {
    pub gateway_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_reason: Option<String>,
    pub pid: u32,
    pub started_at: i64,
    pub updated_at: i64,
    pub version: String,
}

pub fn path(home: &Path) -> PathBuf {
    home.join("gateway.state.json")
}

pub fn write(home: &Path, state: &RuntimeState) -> anyhow::Result<()> {
    crate::cron::store::atomic_write_json(&path(home), state)
}

pub fn read(home: &Path) -> Option<RuntimeState> {
    let body = std::fs::read_to_string(path(home)).ok()?;
    serde_json::from_str(&body).ok()
}

/// Convenience constructor: now, this process, this version.
pub fn record(gateway_state: &str, exit_reason: Option<&str>, started_at: i64) -> RuntimeState {
    RuntimeState {
        gateway_state: gateway_state.to_string(),
        exit_reason: exit_reason.map(str::to_string),
        pid: std::process::id(),
        started_at,
        updated_at: crate::cron::now_secs(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

#[path = "state_tests.rs"]
#[cfg(test)]
mod tests;
