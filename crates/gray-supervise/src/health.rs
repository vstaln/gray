//! File-based readiness: heartbeat mtime < 60s, plus adapter-board and
//! config folding for `gateway status --probe`. No ports, no secrets in reason.
use std::path::Path;

pub struct Health {
    pub healthy: bool,
    pub reason: String,
}

pub fn probe(home: &Path) -> Health {
    match crate::heartbeat::heartbeat_age_secs(home) {
        None => Health {
            healthy: false,
            reason: "unhealthy: no heartbeat yet (gateway not running?)".into(),
        },
        Some(age) if age < 60 => Health {
            healthy: true,
            reason: format!("healthy: heartbeat {age}s ago"),
        },
        Some(age) => Health {
            healthy: false,
            reason: format!("unhealthy: heartbeat stale ({age}s ago)"),
        },
    }
}

/// Hardened probe: heartbeat freshness AND `gateway.yaml` parseability AND
/// (when the daemon has published one) adapter-board health. `board_healthy`
/// is `None` when no status snapshot exists yet — that never fails closed,
/// the heartbeat decides. A snapshot saying all adapters terminally failed
/// fails the probe even with a fresh heartbeat (no more lying healthy).
pub fn probe_full(home: &Path, board_healthy: Option<bool>, config_ok: bool) -> Health {
    let base = probe(home);
    if !base.healthy {
        return base;
    }
    if !config_ok {
        return Health {
            healthy: false,
            reason: "unhealthy: gateway.yaml does not parse".into(),
        };
    }
    if board_healthy == Some(false) {
        return Health {
            healthy: false,
            reason: "unhealthy: all gateway adapters terminally failed".into(),
        };
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_heartbeat_is_unhealthy_with_reason() {
        let dir = tempfile::tempdir().unwrap();
        let h = probe(dir.path());
        assert!(!h.healthy);
        assert!(
            h.reason.contains("heartbeat"),
            "reason must name heartbeat, got: {}",
            h.reason
        );
    }
    #[test]
    fn fresh_heartbeat_is_healthy() {
        let dir = tempfile::tempdir().unwrap();
        crate::heartbeat::write_heartbeat(dir.path()).unwrap();
        assert!(probe(dir.path()).healthy);
    }
    #[test]
    fn full_probe_fails_when_all_adapters_dead() {
        let dir = tempfile::tempdir().unwrap();
        crate::heartbeat::write_heartbeat(dir.path()).unwrap();
        let h = probe_full(dir.path(), Some(false), true);
        assert!(!h.healthy, "all-dead board must fail, got: {}", h.reason);
    }
    #[test]
    fn full_probe_fails_on_unparseable_config() {
        let dir = tempfile::tempdir().unwrap();
        crate::heartbeat::write_heartbeat(dir.path()).unwrap();
        let h = probe_full(dir.path(), Some(true), false);
        assert!(!h.healthy);
        assert!(h.reason.contains("gateway.yaml"), "got: {}", h.reason);
    }
    #[test]
    fn full_probe_missing_snapshot_falls_back_to_heartbeat() {
        let dir = tempfile::tempdir().unwrap();
        crate::heartbeat::write_heartbeat(dir.path()).unwrap();
        assert!(probe_full(dir.path(), None, true).healthy);
    }
    #[test]
    fn full_probe_stale_heartbeat_fails_first() {
        let dir = tempfile::tempdir().unwrap();
        let h = probe_full(dir.path(), Some(true), true);
        assert!(!h.healthy, "no heartbeat must fail, got: {}", h.reason);
    }
}
