//! Supervised reconnect ladder for gateway adapters (move-only split).
//!
//! [`classify_connect_error`] sorts failures into [`Fatal::Retryable`]
//! (backoff + retry) vs [`Fatal::Terminal`] (log once, stop); the
//! crash-loop guard ([`crash_loop_tripped`]) gives up after
//! [`MAX_FAST_FAILURES`] fast failures. [`connect_adapter_with_retry`]
//! drives one adapter through the ladder and replays the delivery ledger.

use std::time::{Duration, Instant};

use crate::config::Platform;
use crate::daemon::Adapter;
use crate::delivery::{DeliveryLedger, DeliveryRouter};
use crate::status::GatewayStatusBoard;

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
    // Wire the board so adapters report staged progress (`validating token` → …).
    if let Some(b) = board {
        adapter.set_status_board(b.clone());
    }
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
                        b.mark_failed(plat, m);
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
