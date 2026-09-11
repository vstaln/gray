use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::gray_home;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatConfig {
    pub enabled: bool,
    pub schedule: String,
    pub deliver: String,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            schedule: "every 30m".into(),
            deliver: "local".into(),
        }
    }
}

pub fn config_path() -> anyhow::Result<PathBuf> {
    Ok(gray_home()?.join("heartbeat.json"))
}

pub fn load_config() -> anyhow::Result<HeartbeatConfig> {
    let path = config_path()?;
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(serde_json::from_str(&s)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HeartbeatConfig::default()),
        Err(e) => Err(e.into()),
    }
}

pub fn save_config(cfg: &HeartbeatConfig) -> anyhow::Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(cfg)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_and_defaults() {
        let _guard = crate::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("GRAY_HOME", dir.path()) };
        assert_eq!(load_config().unwrap().schedule, "every 30m");
        let cfg = HeartbeatConfig {
            enabled: true,
            schedule: "every 1h".into(),
            deliver: "telegram:1".into(),
        };
        save_config(&cfg).unwrap();
        let got = load_config().unwrap();
        assert!(got.enabled);
        assert_eq!(got.schedule, "every 1h");
        assert_eq!(got.deliver, "telegram:1");
    }
}
