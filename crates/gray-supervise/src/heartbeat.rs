//! Heartbeat file: `state/gateway.heartbeat` holds RFC3339 UTC now.
use std::path::Path;

pub fn write_heartbeat(home: &Path) -> anyhow::Result<()> {
    let (state, beat, _) = crate::paths_for_home(home);
    std::fs::create_dir_all(&state)?;
    std::fs::write(&beat, chrono::Utc::now().to_rfc3339())?;
    Ok(())
}

/// Seconds since heartbeat mtime. `None` when missing or on any IO error.
pub fn heartbeat_age_secs(home: &Path) -> Option<u64> {
    let (_, beat, _) = crate::paths_for_home(home);
    let meta = std::fs::metadata(&beat).ok()?;
    let mtime = meta.modified().ok()?;
    std::time::SystemTime::now()
        .duration_since(mtime)
        .map(|d| d.as_secs())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn heartbeat_roundtrip_age_is_fresh() {
        let dir = tempfile::tempdir().unwrap();
        write_heartbeat(dir.path()).unwrap();
        let age = heartbeat_age_secs(dir.path()).unwrap();
        assert!(age < 60, "fresh heartbeat must be <60s, got {age}");
    }
    #[test]
    fn missing_heartbeat_has_no_age() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(heartbeat_age_secs(dir.path()), None);
    }
}
