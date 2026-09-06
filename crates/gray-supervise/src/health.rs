//! File-based readiness: heartbeat mtime < 60s. No ports, no secrets in reason.
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
}
