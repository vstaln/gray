//! Per-platform connection board.
//!
//! The in-process gateway connects to each platform sequentially (up to 45s
//! per platform). The daemon marks results here and persists a snapshot for
//! the cross-process `gateway status --probe`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::config::Platform;

/// Connection state of one platform, as tracked for the status snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformConnState {
    Connecting,
    Connected { identity: Option<String> },
    Failed(String),
}

/// Shareable board: the daemon writes, the status snapshot reads. Every
/// method locks briefly and never blocks on I/O, so either side can call
/// from any task.
#[derive(Debug, Clone, Default)]
pub struct GatewayStatusBoard {
    inner: Arc<Mutex<HashMap<Platform, PlatformConnState>>>,
}

impl GatewayStatusBoard {
    /// Board with every listed platform in [`PlatformConnState::Connecting`].
    pub fn new(platforms: &[Platform]) -> Self {
        let inner = platforms
            .iter()
            .map(|p| (*p, PlatformConnState::Connecting))
            .collect();
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    pub fn mark_connected(&self, plat: Platform, identity: Option<String>) {
        if let Ok(mut m) = self.inner.lock() {
            m.insert(plat, PlatformConnState::Connected { identity });
        }
    }

    pub fn mark_failed(&self, plat: Platform, err: impl Into<String>) {
        if let Ok(mut m) = self.inner.lock() {
            m.insert(plat, PlatformConnState::Failed(err.into()));
        }
    }

    /// `(platform, state)` pairs in canonical platform order.
    pub fn snapshot(&self) -> Vec<(Platform, PlatformConnState)> {
        let guard = self.inner.lock().ok();
        Platform::ALL
            .into_iter()
            .filter_map(|p| guard.as_ref()?.get(&p).cloned().map(|s| (p, s)))
            .collect()
    }

    /// Persist platform states for the probe (best-effort, never fails boot).
    /// See [`status_snapshot_path`].
    pub fn save_snapshot(&self, home: &std::path::Path) {
        let map: std::collections::BTreeMap<String, String> = self
            .snapshot()
            .into_iter()
            .map(|(p, s)| {
                let v = match &s {
                    PlatformConnState::Connected { .. } => "connected",
                    PlatformConnState::Connecting => "connecting",
                    PlatformConnState::Failed(_) => "failed",
                };
                (p.to_string(), v.to_string())
            })
            .collect();
        crate::delivery::atomic_write_json(&status_snapshot_path(home), &map);
    }
}

/// Snapshot file (`state/gateway.status.json`): platform →
/// `connected` | `connecting` | `failed`. Written by the daemon supervisor,
/// read cross-process by `gateway status --probe`. Failure reasons are
/// deliberately omitted (they may name tokens; freshness + state suffice).
fn status_snapshot_path(home: &std::path::Path) -> std::path::PathBuf {
    home.join("state").join("gateway.status.json")
}

/// Cross-process read of [`GatewayStatusBoard::save_snapshot`]: `None` = no
/// snapshot yet (old daemon / not booted) → the probe falls back to the
/// heartbeat instead of failing closed on missing data.
pub fn read_board_healthy(home: &std::path::Path) -> Option<bool> {
    let text = std::fs::read_to_string(status_snapshot_path(home)).ok()?;
    let map: std::collections::BTreeMap<String, String> = serde_json::from_str(&text).ok()?;
    if map.is_empty() {
        return None;
    }
    Some(map.values().any(|s| s == "connected" || s == "connecting"))
}

/// `gateway.yaml` parses into [`crate::config::GatewayConfig`]. An absent
/// file counts as ok (it means defaults; `gateway run` reports that case).
pub fn gateway_config_parses(home: &std::path::Path) -> bool {
    let path = home.join("gateway.yaml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return true;
    };
    serde_yaml_ng::from_str::<crate::config::GatewayConfig>(&text).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_lifecycle() {
        let b = GatewayStatusBoard::new(&[Platform::Discord, Platform::Telegram]);
        // Canonical order regardless of construction order.
        let snap = b.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].0, Platform::Telegram);
        assert_eq!(snap[1].0, Platform::Discord);
        assert!(
            snap.iter()
                .all(|(_, s)| matches!(s, PlatformConnState::Connecting))
        );

        b.mark_connected(Platform::Discord, Some("GrayBot".into()));
        b.mark_failed(Platform::Telegram, "timeout");
        let snap = b.snapshot();
        assert_eq!(snap[0].1, PlatformConnState::Failed("timeout".to_string()));
        assert_eq!(
            snap[1].1,
            PlatformConnState::Connected {
                identity: Some("GrayBot".into())
            }
        );
    }

    #[test]
    fn status_snapshot_roundtrip_for_probe() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_board_healthy(dir.path()), None, "no snapshot yet");
        let board = GatewayStatusBoard::new(&[Platform::Telegram]);
        board.save_snapshot(dir.path());
        assert_eq!(read_board_healthy(dir.path()), Some(true));
        board.mark_failed(Platform::Telegram, "revoked");
        board.save_snapshot(dir.path());
        assert_eq!(read_board_healthy(dir.path()), Some(false));
    }

    #[test]
    fn gateway_config_parses_accepts_missing_rejects_garbage() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            gateway_config_parses(dir.path()),
            "absent file uses defaults"
        );
        std::fs::write(dir.path().join("gateway.yaml"), "not: [valid").unwrap();
        assert!(!gateway_config_parses(dir.path()));
    }
}
