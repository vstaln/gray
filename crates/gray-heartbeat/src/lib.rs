//! gray-heartbeat: a standing goal run on a schedule by the gateway cron ticker.
pub mod config;
pub mod goal;
pub mod job;
pub mod plugin;

pub use job::enable;

#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

use std::path::PathBuf;

/// `$GRAY_HOME`, else `$HOME/.gray`.
pub fn gray_home() -> anyhow::Result<PathBuf> {
    if let Ok(h) = std::env::var("GRAY_HOME") {
        return Ok(PathBuf::from(h));
    }
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("cannot resolve HOME"))?;
    Ok(PathBuf::from(home).join(".gray"))
}
