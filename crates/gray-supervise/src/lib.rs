//! Platform-free supervision core (no telegram/discord/slack deps).
pub mod exit;
pub mod health;
pub mod heartbeat;
pub mod lifecycle;
pub mod rotation;
pub mod units;
pub mod watchdog;

use std::path::{Path, PathBuf};

/// `(state_dir, heartbeat_file, lifecycle_file)` under `$GRAY_HOME/state/`.
pub fn paths_for_home(home: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let state = home.join("state");
    let beat = state.join("gateway.heartbeat");
    let lc = state.join("gateway.lifecycle.json");
    (state, beat, lc)
}

/// Heartbeat period: `GRAY_HEARTBEAT_SECS`, default 15, clamped to min 5.
pub fn heartbeat_interval_secs() -> u64 {
    std::env::var("GRAY_HEARTBEAT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(15)
        .max(5)
}
