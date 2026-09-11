use std::path::{Path, PathBuf};

use gray_gateway::config::gray_home_dir;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PulseConfig {
    pub enabled: bool,
    pub schedule: String,
    pub deliver: String,
}

impl Default for PulseConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            schedule: "every 30m".into(),
            deliver: "local".into(),
        }
    }
}

pub(crate) fn config_path_at(home: &Path) -> PathBuf {
    home.join("pulse.json")
}

pub fn config_path() -> anyhow::Result<PathBuf> {
    Ok(config_path_at(&gray_home_dir()?))
}

pub(crate) fn load_config_at(path: &Path) -> anyhow::Result<PulseConfig> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(serde_json::from_str(&s)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PulseConfig::default()),
        Err(e) => Err(e.into()),
    }
}

pub fn load_config() -> anyhow::Result<PulseConfig> {
    load_config_at(&config_path()?)
}

pub(crate) fn save_config_at(path: &Path, cfg: &PulseConfig) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(cfg)?)?;
    Ok(())
}

pub fn save_config(cfg: &PulseConfig) -> anyhow::Result<()> {
    save_config_at(&config_path()?, cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = config_path_at(dir.path());
        assert_eq!(load_config_at(&path).unwrap().schedule, "every 30m");
        let cfg = PulseConfig {
            enabled: true,
            schedule: "every 1h".into(),
            deliver: "telegram:1".into(),
        };
        save_config_at(&path, &cfg).unwrap();
        let got = load_config_at(&path).unwrap();
        assert!(got.enabled);
        assert_eq!(got.schedule, "every 1h");
        assert_eq!(got.deliver, "telegram:1");
    }
}
