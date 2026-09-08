//! Live per-platform connection board.
//!
//! The in-process gateway connects to each platform sequentially (up to 45s
//! per platform). The daemon marks results here; the REPL paints one
//! live-updating boot card (`connecting…` → `connected as <name>`).
//! Every mutating mark wakes [`tokio::sync::Notify`] waiters, so the REPL
//! repaints on each stage transition instead of polling-luck (a stage with
//! less dwell than the tick interval would otherwise never paint).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::config::Platform;

/// Connection state of one platform, as shown on the REPL boot card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformConnState {
    Connecting { stage: &'static str },
    Connected { identity: Option<String> },
    Failed(String),
}

impl PlatformConnState {
    pub fn terminal(&self) -> bool {
        !matches!(self, Self::Connecting { .. })
    }
}

/// Shareable board: the daemon writes, the REPL waits-and-paints. Every
/// method locks briefly and never blocks on I/O, so either side can call
/// from any task.
#[derive(Debug, Clone, Default)]
pub struct GatewayStatusBoard {
    inner: Arc<Mutex<HashMap<Platform, PlatformConnState>>>,
    notify: Arc<tokio::sync::Notify>,
}

impl GatewayStatusBoard {
    /// Board with every listed platform in [`PlatformConnState::Connecting`].
    pub fn new(platforms: &[Platform]) -> Self {
        let inner = platforms
            .iter()
            .map(|p| {
                (
                    *p,
                    PlatformConnState::Connecting {
                        stage: "connecting",
                    },
                )
            })
            .collect();
        Self {
            inner: Arc::new(Mutex::new(inner)),
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Advance the [`PlatformConnState::Connecting`] stage (e.g. `"polling"`).
    /// Terminal states are never clobbered; unknown platforms start connecting.
    /// Wakes [`Self::notified`] waiters when the stored state actually changes.
    pub fn mark_stage(&self, plat: Platform, stage: &'static str) {
        let changed = if let Ok(mut m) = self.inner.lock() {
            match m.get_mut(&plat) {
                Some(PlatformConnState::Connecting { stage: s }) if *s != stage => {
                    *s = stage;
                    true
                }
                Some(PlatformConnState::Connecting { .. }) => false,
                Some(_) => false,
                None => {
                    m.insert(plat, PlatformConnState::Connecting { stage });
                    true
                }
            }
        } else {
            false
        };
        if changed {
            self.notify.notify_waiters();
        }
    }

    pub fn mark_connected(&self, plat: Platform, identity: Option<String>) {
        if let Ok(mut m) = self.inner.lock() {
            m.insert(plat, PlatformConnState::Connected { identity });
        }
        self.notify.notify_waiters();
    }

    pub fn mark_failed(&self, plat: Platform, err: impl Into<String>) {
        if let Ok(mut m) = self.inner.lock() {
            m.insert(plat, PlatformConnState::Failed(err.into()));
        }
        self.notify.notify_waiters();
    }

    /// Anything still [`PlatformConnState::Connecting`] becomes failed (the
    /// gateway task exited before reporting — never leave the card spinning).
    pub fn fail_unresolved(&self, err: &str) {
        let changed = if let Ok(mut m) = self.inner.lock() {
            let mut changed = false;
            for st in m.values_mut() {
                if matches!(*st, PlatformConnState::Connecting { .. }) {
                    *st = PlatformConnState::Failed(err.to_string());
                    changed = true;
                }
            }
            changed
        } else {
            false
        };
        if changed {
            self.notify.notify_waiters();
        }
    }

    /// Resolves on the next board mutation ([`Self::mark_stage`] and friends).
    /// One-shot per call: create it, then mutate, then await. Missed signals
    /// are harmless — callers also poll on an interval as backstop.
    pub fn notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.notify.notified()
    }

    /// `(platform, state)` pairs in canonical platform order.
    pub fn snapshot(&self) -> Vec<(Platform, PlatformConnState)> {
        let guard = self.inner.lock().ok();
        Platform::ALL
            .into_iter()
            .filter_map(|p| guard.as_ref()?.get(&p).cloned().map(|s| (p, s)))
            .collect()
    }

    /// True once every tracked platform resolved (empty board never counts).
    pub fn all_terminal(&self) -> bool {
        self.inner
            .lock()
            .map(|m| !m.is_empty() && m.values().all(|s| s.terminal()))
            .unwrap_or(false)
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
                    PlatformConnState::Connecting { .. } => "connecting",
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

/// Probe verdict over a live board: healthy when any adapter is connected or
/// still (re)trying; all-failed or empty means the process has nothing left
/// to serve (hang-with-fresh-heartbeat now fails instead of lying healthy).
pub fn probe_board_healthy(board: &GatewayStatusBoard) -> bool {
    board.snapshot().into_iter().any(|(_, s)| {
        matches!(
            s,
            PlatformConnState::Connected { .. } | PlatformConnState::Connecting { .. }
        )
    })
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

/// One boot-card row per platform: `  └─ Discord — connecting…` →
/// `  └─ Discord — connected as GrayBot`. The two-space indent matches the
/// card header (`format_tool_box_lines`); shared verbatim by the live
/// viewport panel and the committed final card.
pub fn gateway_boot_rows(board: &GatewayStatusBoard) -> Vec<String> {
    let snap = board.snapshot();
    snap.iter()
        .enumerate()
        .map(|(i, (plat, st))| {
            let branch = if i + 1 == snap.len() {
                "└─"
            } else {
                "├─"
            };
            let status = match st {
                PlatformConnState::Connecting { stage } => format!("{stage}…"),
                PlatformConnState::Connected { identity: Some(id) } => {
                    format!("connected as {id}")
                }
                PlatformConnState::Connected { identity: None } => "connected".to_string(),
                PlatformConnState::Failed(e) => format!("connect failed: {e}"),
            };
            format!("  {branch} {} — {status}", plat.label())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_lifecycle() {
        let b = GatewayStatusBoard::new(&[Platform::Discord, Platform::Telegram]);
        assert!(!b.all_terminal());
        // Canonical order regardless of construction order.
        let snap = b.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].0, Platform::Telegram);
        assert_eq!(snap[1].0, Platform::Discord);
        assert!(
            snap.iter()
                .all(|(_, s)| matches!(s, PlatformConnState::Connecting { .. }))
        );

        b.mark_connected(Platform::Discord, Some("GrayBot".into()));
        assert!(!b.all_terminal());
        b.mark_failed(Platform::Telegram, "timeout");
        assert!(b.all_terminal());
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
    fn boot_rows_render_all_states() {
        let b = GatewayStatusBoard::new(&[Platform::Discord, Platform::Telegram]);
        b.mark_connected(Platform::Discord, Some("GrayBot".into()));
        b.mark_failed(Platform::Telegram, "timeout");
        let rows = super::gateway_boot_rows(&b);
        assert_eq!(rows.len(), 2);
        // Canonical platform order: Telegram first, Discord last (└─).
        assert!(rows[0].starts_with("  ├─ Telegram — "), "row: {}", rows[0]);
        assert!(
            rows[0].contains("connect failed: timeout"),
            "row: {}",
            rows[0]
        );
        assert_eq!(rows[1], "  └─ Discord — connected as GrayBot");
    }

    #[test]
    fn fail_unresolved_only_touches_connecting() {
        let b = GatewayStatusBoard::new(&[Platform::Discord, Platform::Slack]);
        b.mark_connected(Platform::Discord, None);
        b.fail_unresolved("gateway exited");
        let snap = b.snapshot();
        assert_eq!(snap[0].1, PlatformConnState::Connected { identity: None });
        assert_eq!(
            snap[1].1,
            PlatformConnState::Failed("gateway exited".to_string())
        );
    }

    #[test]
    fn empty_board_never_terminal() {
        let b = GatewayStatusBoard::default();
        assert!(!b.all_terminal());
        assert!(b.snapshot().is_empty());
    }

    #[test]
    fn probe_fails_when_all_adapters_dead() {
        let board = GatewayStatusBoard::new(&[Platform::Telegram]);
        board.mark_failed(Platform::Telegram, "revoked");
        assert!(!probe_board_healthy(&board));
    }

    #[test]
    fn probe_passes_when_any_adapter_live_or_retrying() {
        let board = GatewayStatusBoard::new(&[Platform::Telegram, Platform::Discord]);
        assert!(probe_board_healthy(&board), "connecting counts as retrying");
        board.mark_failed(Platform::Telegram, "revoked");
        assert!(probe_board_healthy(&board), "one live adapter suffices");
        board.mark_connected(Platform::Discord, None);
        assert!(probe_board_healthy(&board));
        board.mark_failed(Platform::Discord, "boom");
        assert!(!probe_board_healthy(&board));
    }

    #[test]
    fn probe_empty_board_is_not_healthy() {
        assert!(!probe_board_healthy(&GatewayStatusBoard::default()));
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
    #[test]
    fn connecting_carries_default_stage() {
        let b = GatewayStatusBoard::new(&[Platform::Discord]);
        assert_eq!(
            b.snapshot(),
            vec![(
                Platform::Discord,
                PlatformConnState::Connecting {
                    stage: "connecting"
                }
            )]
        );
        assert!(
            !b.snapshot()[0].1.terminal(),
            "staged Connecting stays non-terminal"
        );
        assert!(!b.all_terminal());
    }

    #[test]
    fn mark_stage_updates_connecting_only() {
        let b = GatewayStatusBoard::new(&[Platform::Discord, Platform::Telegram]);
        b.mark_stage(Platform::Telegram, "validating token");
        let snap = b.snapshot();
        assert_eq!(
            snap[0].1,
            PlatformConnState::Connecting {
                stage: "validating token"
            }
        );
        assert!(!b.all_terminal(), "staged Connecting stays non-terminal");
        // Terminal states are never clobbered by a late stage.
        b.mark_connected(Platform::Telegram, Some("GrayBot".into()));
        b.mark_stage(Platform::Telegram, "polling");
        assert_eq!(
            b.snapshot()[0].1,
            PlatformConnState::Connected {
                identity: Some("GrayBot".into())
            }
        );
        b.mark_failed(Platform::Discord, "boom");
        b.mark_stage(Platform::Discord, "polling");
        assert_eq!(
            b.snapshot()[1].1,
            PlatformConnState::Failed("boom".to_string())
        );
    }

    #[test]
    fn fail_unresolved_covers_staged_connecting() {
        let b = GatewayStatusBoard::new(&[Platform::Discord, Platform::Slack]);
        b.mark_stage(Platform::Discord, "waiting for ready");
        b.mark_connected(Platform::Slack, None);
        b.fail_unresolved("gateway exited");
        let snap = b.snapshot();
        // Canonical order has no Telegram here; match by platform.
        for (p, s) in &snap {
            match p {
                Platform::Discord => {
                    assert_eq!(*s, PlatformConnState::Failed("gateway exited".to_string()))
                }
                Platform::Slack => assert_eq!(*s, PlatformConnState::Connected { identity: None }),
                Platform::Telegram => unreachable!(),
            }
        }
    }

    #[tokio::test]
    async fn mutations_wake_notified_waiters() {
        let b = GatewayStatusBoard::new(&[Platform::Discord]);
        // Idle board: no wake.
        let n = b.notified();
        tokio::pin!(n);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut n)
                .await
                .is_err(),
            "no mutation, no wake"
        );
        // Stage advance wakes.
        b.mark_stage(Platform::Discord, "validating token");
        tokio::time::timeout(std::time::Duration::from_secs(1), n)
            .await
            .expect("stage mark must wake waiter");
        // Same stage twice: second mark is a no-op, no spurious wake.
        let n2 = b.notified();
        tokio::pin!(n2);
        b.mark_stage(Platform::Discord, "validating token");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut n2)
                .await
                .is_err(),
            "no-op mark must not wake"
        );
        // Terminal marks wake too.
        let n3 = b.notified();
        tokio::pin!(n3);
        b.mark_connected(Platform::Discord, None);
        tokio::time::timeout(std::time::Duration::from_secs(1), n3)
            .await
            .expect("connect mark must wake waiter");
    }
}
