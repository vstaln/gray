//! Live per-platform connection board.
//!
//! The in-process gateway connects to each platform sequentially (up to 45s
//! per platform). The daemon marks results here; the REPL polls the board and
//! paints one live-updating boot card (`connecting…` → `connected as <name>`)
//! instead of a static "connecting in background" line.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::config::Platform;

/// Connection state of one platform, as shown on the REPL boot card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformConnState {
    Connecting,
    Connected { identity: Option<String> },
    Failed(String),
}

impl PlatformConnState {
    pub fn terminal(&self) -> bool {
        !matches!(self, Self::Connecting)
    }
}

/// Shareable board: the daemon writes, the REPL polls. Every method locks
/// briefly and never blocks on I/O, so either side can call from any task.
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
        Self { inner: Arc::new(Mutex::new(inner)) }
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

    /// Anything still [`PlatformConnState::Connecting`] becomes failed (the
    /// gateway task exited before reporting — never leave the card spinning).
    pub fn fail_unresolved(&self, err: &str) {
        if let Ok(mut m) = self.inner.lock() {
            for st in m.values_mut() {
                if *st == PlatformConnState::Connecting {
                    *st = PlatformConnState::Failed(err.to_string());
                }
            }
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
        assert!(snap.iter().all(|(_, s)| *s == PlatformConnState::Connecting));

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
}
