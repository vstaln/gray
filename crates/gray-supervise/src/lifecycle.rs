//! Lifecycle ledger: `state/gateway.lifecycle.json`.
use std::path::Path;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Lifecycle {
    pub boot_id: String,
    pub started_at: String,
    pub clean_shutdown: bool,
}

impl Lifecycle {
    pub fn mark_boot(home: &Path) -> anyhow::Result<Self> {
        let (state, _, path) = crate::paths_for_home(home);
        std::fs::create_dir_all(&state)?;
        let lc = Lifecycle {
            boot_id: uuid::Uuid::new_v4().to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            clean_shutdown: false,
        };
        std::fs::write(&path, serde_json::to_string_pretty(&lc)?)?;
        Ok(lc)
    }

    pub fn mark_clean(home: &Path) -> anyhow::Result<()> {
        let (state, _, path) = crate::paths_for_home(home);
        std::fs::create_dir_all(&state)?;
        let mut lc = Self::read(home).unwrap_or(Lifecycle {
            boot_id: uuid::Uuid::new_v4().to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            clean_shutdown: false,
        });
        lc.clean_shutdown = true;
        std::fs::write(&path, serde_json::to_string_pretty(&lc)?)?;
        Ok(())
    }

    pub fn read(home: &Path) -> Option<Self> {
        let (_, _, path) = crate::paths_for_home(home);
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boot_then_clean_flips_flag() {
        let dir = tempfile::tempdir().unwrap();
        let b = Lifecycle::mark_boot(dir.path()).unwrap();
        assert!(!b.clean_shutdown);
        assert_eq!(Lifecycle::read(dir.path()).unwrap().clean_shutdown, false);
        Lifecycle::mark_clean(dir.path()).unwrap();
        assert_eq!(Lifecycle::read(dir.path()).unwrap().clean_shutdown, true);
    }
}
