//! Supervised reconnect ladder for gateway adapters (move-only split).
//!
//! [`classify_connect_error`] sorts failures into [`Fatal::Retryable`]
//! (backoff + retry) vs [`Fatal::Terminal`] (log once, stop); the
//! crash-loop guard ([`crash_loop_tripped`]) gives up after
//! [`MAX_FAST_FAILURES`] fast failures. [`connect_adapter_with_retry`]
//! drives one adapter through the ladder and replays the delivery ledger.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::Platform;
use crate::daemon::{Adapter, GatewayRunner};
use crate::delivery::{DeliveryLedger, DeliveryRouter};
use crate::status::{GatewayStatusBoard, PlatformConnState};

// ---------------------------------------------------------------------------
// Supervised reconnect ladder
// ---------------------------------------------------------------------------

/// How a failed `connect()` (or a dropped shard) feeds the reconnect ladder.
pub enum Fatal {
    /// Transient failure: retry with [`crate::platform::backoff_delay`].
    Retryable(String),
    /// Auth/config failure: log once and STOP retrying.
    Terminal(String),
}

/// Upper bound on connect attempts per adapter (steady-state reconnects).
pub const MAX_RECONNECT_ATTEMPTS: u32 = 8;
/// Lower boot cap so one wedged platform can't stall startup; steady-state
/// reconnects use [`MAX_RECONNECT_ATTEMPTS`]. Crash-loop guard still applies.
pub const BOOT_MAX_ATTEMPTS: u32 = 3;
/// Give up after this many consecutive fast failures (crash-loop guard).
pub const MAX_FAST_FAILURES: u32 = 5;
/// Failures spaced closer than this count as "fast".
pub const FAST_FAILURE_WINDOW: Duration = Duration::from_secs(60);

/// Auth failures (bad token / forbidden) are terminal; everything else
/// (timeouts, resets, shard ends) is retryable.
pub fn classify_connect_error(err: &str) -> Fatal {
    let lower = err.to_ascii_lowercase();
    // Feature-stub refusal (platform not compiled in) is deterministic config,
    // never retry: surface the rebuild instruction immediately.
    if lower.contains("not compiled") {
        return Fatal::Terminal(err.to_string());
    }
    // Privileged-intents close (4014 contains "401") is reconnectable, never terminal.
    if lower.contains("4014") || lower.contains("intent") {
        return Fatal::Retryable(err.to_string());
    }
    // Revoked/dead Slack tokens never recover — no hot retry.
    if [
        "invalid_auth",
        "account_inactive",
        "token_revoked",
        "not_authed",
    ]
    .iter()
    .any(|m| lower.contains(m))
    {
        return Fatal::Terminal(err.to_string());
    }
    let auth = [
        "unauthorized",
        "forbidden",
        "token rejected",
        "bad token",
        "invalid token",
        "401",
        "403",
    ];
    if auth.iter().any(|m| lower.contains(m)) {
        Fatal::Terminal(err.to_string())
    } else {
        Fatal::Retryable(err.to_string())
    }
}

/// A dropped discord shard is never auth: always re-enter the ladder.
/// (The adapter stores the shard `JoinHandle`; when the task dies the next
/// supervised `connect_adapter_with_retry` restarts it, `disconnect()` aborts it.)
pub fn classify_shard_end() -> Fatal {
    Fatal::Retryable("shard ended".to_string())
}

/// Crash-loop guard: true once fast failures hit the limit.
pub fn crash_loop_tripped(consecutive_fast_failures: u32) -> bool {
    consecutive_fast_failures >= MAX_FAST_FAILURES
}

/// Steady-state reconnect delay: 30s, 60s, 120s, 240s, then capped at 300s
/// (hermes: `30*2^(n-1)`, cap 300). NOTE: the plan draft used
/// `attempt.min(4)`, which tops out at 240s and never reaches the cap —
/// `min(5)` is the smallest bound whose schedule actually hits 300s.
pub fn supervise_backoff(attempt: u32) -> std::time::Duration {
    std::time::Duration::from_secs(
        (30u64.saturating_mul(1u64 << attempt.min(5).saturating_sub(1))).min(300),
    )
}

/// Supervised `connect()` for one adapter: timeout-bounded attempts through
/// the ladder, terminal errors stop immediately, fast-failure crash loop
/// gives up with a log. Reports progress on `board` like before.
/// On success replays the delivery ledger (boot and reconnects alike).
/// `max_attempts` is [`BOOT_MAX_ATTEMPTS`] at boot, [`MAX_RECONNECT_ATTEMPTS`]
/// for steady-state reconnects.
pub(crate) async fn connect_adapter_with_retry(
    adapter: &Adapter,
    plat: Platform,
    board: Option<&GatewayStatusBoard>,
    router: &DeliveryRouter,
    ledger: &DeliveryLedger,
    max_attempts: u32,
) {
    let cap = max_attempts.max(1);
    let mut fast_failures = 0u32;
    let mut last_failure: Option<Instant> = None;
    for attempt in 1..=cap {
        let res = tokio::time::timeout(Duration::from_secs(45), adapter.connect()).await;
        let err: Option<String> = match res {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e.to_string()),
            Err(_) => Some("connect timeout 45s".to_string()),
        };
        match err {
            None => {
                log::info!("gateway {plat} connected");
                if let Some(b) = board {
                    b.mark_connected(plat, adapter.bot_identity());
                }
                // Reconnect (and boot) replay: a fresh connection may have
                // fixed the cause of pending obligations.
                router.sweep_ledger(ledger).await;
                return;
            }
            Some(e) => match classify_connect_error(&e) {
                Fatal::Terminal(m) => {
                    log::error!("gateway {plat} connect failed (terminal, not retrying): {m}");
                    if let Some(b) = board {
                        b.mark_failed(
                            plat,
                            format!(
                                "{m} (parked — fix gateway.yaml, then `systemctl --user restart gray-gateway`)"
                            ),
                        );
                    }
                    return;
                }
                Fatal::Retryable(m) => {
                    let fast = last_failure.is_some_and(|t| t.elapsed() < FAST_FAILURE_WINDOW);
                    fast_failures = if fast { fast_failures + 1 } else { 1 };
                    last_failure = Some(Instant::now());
                    if crash_loop_tripped(fast_failures) {
                        log::error!(
                            "gateway {plat} crash-loop ({fast_failures} fast failures), giving up: {m}"
                        );
                        if let Some(b) = board {
                            b.mark_failed(plat, m);
                        }
                        return;
                    }
                    if attempt == cap {
                        log::error!(
                            "gateway {plat} connect failed after {attempt} attempts, giving up: {m}"
                        );
                        if let Some(b) = board {
                            b.mark_failed(plat, m);
                        }
                        return;
                    }
                    let d = crate::platform::backoff_delay(attempt);
                    log::warn!(
                        "gateway {plat} connect failed (attempt {attempt}): {m}; retry in {d:?}"
                    );
                    tokio::time::sleep(d).await;
                }
            },
        }
    }
}

/// Steady-state supervisor (spawned by `daemon_boot` after boot, on the main
/// runtime): every 30s, each adapter that is still board-`Failed` from boot
/// re-enters [`connect_adapter_with_retry`]
/// with [`MAX_RECONNECT_ATTEMPTS`]. Reconnect rounds per adapter are spaced
/// by [`supervise_backoff`] so a persistently dead platform backs off to one
/// ladder per 5 minutes (and retryable failures self-heal — no restart
/// needed). Terminal failures (bad/revoked token, adapter not compiled) are
/// parked until process restart, which re-reads gateway.yaml; reviving them
/// would only re-log the same terminal error forever. When every tracked
/// adapter is terminally `Failed` and the delivery queue is empty, the
/// process exits 75 so systemd revives it fresh (obligations survive in the
/// persistent ledger and replay at boot).
pub(crate) async fn supervise_adapters(
    runner: Arc<GatewayRunner>,
    board: GatewayStatusBoard,
    home: std::path::PathBuf,
) {
    const TICK: Duration = Duration::from_secs(30);
    // Consecutive dead rounds + last ladder-entry per platform (backoff spacing).
    let mut state: HashMap<Platform, (u32, Instant)> = HashMap::new();
    loop {
        tokio::time::sleep(TICK).await;
        let snap = board.snapshot();
        for (plat, adapter) in runner.adapters.iter() {
            let row = snap.iter().find(|(p, _)| p == plat).map(|(_, s)| s);
            let failed = matches!(row, Some(PlatformConnState::Failed(_)));
            if !failed {
                state.remove(plat);
                // Boot used no board: an adapter stuck on `Connecting`
                // really is connected — record it. Failed rows are never
                // touched here; only the ladder rewrites them.
                if matches!(row, Some(PlatformConnState::Connecting)) {
                    board.mark_connected(*plat, adapter.bot_identity());
                }
                continue;
            }
            // Terminal failure rows are parked: gateway.yaml is only read at
            // boot, so re-entering the ladder just re-logs the same terminal
            // error forever and never heals a revoked token.
            if parked_failure(row) {
                continue;
            }
            let round = state.get(plat).map(|(n, _)| n + 1).unwrap_or(1);
            let due = state
                .get(plat)
                .map(|(_, t)| t.elapsed() >= supervise_backoff(round))
                .unwrap_or(true);
            if !due {
                continue;
            }
            state.insert(*plat, (round, Instant::now()));
            log::warn!("gateway {plat} not alive (round {round}): re-entering connect ladder");
            connect_adapter_with_retry(
                adapter,
                *plat,
                Some(&board),
                &runner.router,
                &runner.ledger,
                MAX_RECONNECT_ATTEMPTS,
            )
            .await;
            if !board_shows_failed(&board, *plat) {
                state.remove(plat);
                log::info!("gateway {plat} recovered in steady state");
            }
        }
        board.save_snapshot(&home);
        if all_adapters_failed(&runner, &board) && runner.ledger.sweep().is_empty() {
            log::error!(
                "gateway: all adapters terminally failed, queue empty; exiting 75 for systemd restart"
            );
            std::process::exit(gray_supervise::exit::EXIT_RESTART);
        }
    }
}

/// True when a `Failed` board row is terminal (auth/config): the ladder must
/// not revive it. Retryable failures still re-enter the ladder and self-heal.
fn parked_failure(row: Option<&PlatformConnState>) -> bool {
    matches!(row, Some(PlatformConnState::Failed(m)) if matches!(classify_connect_error(m), Fatal::Terminal(_)))
}

fn board_shows_failed(board: &GatewayStatusBoard, plat: Platform) -> bool {
    board
        .snapshot()
        .into_iter()
        .any(|(p, s)| p == plat && matches!(s, PlatformConnState::Failed(_)))
}

fn all_adapters_failed(runner: &GatewayRunner, board: &GatewayStatusBoard) -> bool {
    !runner.adapters.is_empty()
        && runner
            .adapters
            .keys()
            .all(|p| board_shows_failed(board, *p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_failed_rows_are_parked_but_retryable_ones_are_not() {
        for msg in [
            "unauthorized",
            "invalid_auth: token revoked",
            "telegram adapter not compiled in this build",
        ] {
            let row = PlatformConnState::Failed(msg.to_string());
            assert!(parked_failure(Some(&row)), "{msg:?} must be parked");
        }
        let retryable = PlatformConnState::Failed("connect timeout 45s".to_string());
        assert!(!parked_failure(Some(&retryable)));
        assert!(!parked_failure(Some(&PlatformConnState::Connected {
            identity: None
        })));
        assert!(!parked_failure(None));
    }

    #[test]
    fn backoff_schedule_caps_at_five_minutes() {
        assert_eq!(supervise_backoff(1).as_secs(), 30);
        assert_eq!(supervise_backoff(2).as_secs(), 60);
        assert_eq!(supervise_backoff(10).as_secs(), 300);
    }

    #[test]
    fn backoff_ramps_then_holds_cap() {
        assert_eq!(supervise_backoff(0).as_secs(), 30);
        assert_eq!(supervise_backoff(3).as_secs(), 120);
        assert_eq!(supervise_backoff(4).as_secs(), 240);
        assert_eq!(supervise_backoff(5).as_secs(), 300);
        assert_eq!(supervise_backoff(100).as_secs(), 300);
    }
}
