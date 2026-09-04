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
            .map(|p| (*p, PlatformConnState::Connecting { stage: "connecting" }))
            .collect();
        Self { inner: Arc::new(Mutex::new(inner)), notify: Arc::new(tokio::sync::Notify::new()) }
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
        assert!(snap.iter().all(|(_, s)| matches!(s, PlatformConnState::Connecting { .. })));

        b.mark_connected(Platform::Discord, Some("GrayBot".into()));
        assert!(!b.all_terminal());
        b.mark_failed(Platform::Telegram, "timeout");
        assert!(b.all_terminal());
        let snap = b.snapshot();
        assert_eq!(
            snap[0].1,
            PlatformConnState::Failed("timeout".to_string())
        );
        assert_eq!(
            snap[1].1,
            PlatformConnState::Connected { identity: Some("GrayBot".into()) }
        );
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
    fn connecting_carries_default_stage() {
        let b = GatewayStatusBoard::new(&[Platform::Discord]);
        assert_eq!(
            b.snapshot(),
            vec![(Platform::Discord, PlatformConnState::Connecting { stage: "connecting" })]
        );
        assert!(!b.snapshot()[0].1.terminal(), "staged Connecting stays non-terminal");
        assert!(!b.all_terminal());
    }

    #[test]
    fn mark_stage_updates_connecting_only() {
        let b = GatewayStatusBoard::new(&[Platform::Discord, Platform::Telegram]);
        b.mark_stage(Platform::Telegram, "validating token");
        let snap = b.snapshot();
        assert_eq!(snap[0].1, PlatformConnState::Connecting { stage: "validating token" });
        assert!(!b.all_terminal(), "staged Connecting stays non-terminal");
        // Terminal states are never clobbered by a late stage.
        b.mark_connected(Platform::Telegram, Some("GrayBot".into()));
        b.mark_stage(Platform::Telegram, "polling");
        assert_eq!(
            b.snapshot()[0].1,
            PlatformConnState::Connected { identity: Some("GrayBot".into()) }
        );
        b.mark_failed(Platform::Discord, "boom");
        b.mark_stage(Platform::Discord, "polling");
        assert_eq!(
            b.snapshot()[1].1,
            PlatformConnState::Failed("boom".to_string())
        );
    }

    #[test]
    fn fail_unresolved_covers_staged_connecting() {        let b = GatewayStatusBoard::new(&[Platform::Discord, Platform::Slack]);
        b.mark_stage(Platform::Discord, "waiting for ready");
        b.mark_connected(Platform::Slack, None);
        b.fail_unresolved("gateway exited");
        let snap = b.snapshot();
        // Canonical order has no Telegram here; match by platform.
        for (p, s) in &snap {
            match p {
                Platform::Discord => assert_eq!(*s, PlatformConnState::Failed("gateway exited".to_string())),
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
