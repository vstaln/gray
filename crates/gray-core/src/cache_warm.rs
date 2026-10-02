//! Prompt-cache warming during long tool runs (pi `cache-warmer.ts`,
//! `"streaming"` mode).
//!
//! A provider's prompt cache entry expires a fixed time after its last use.
//! When a tool runs past that, the next request re-bills the whole prompt as
//! a cache write. While a round's tools run, this re-sends the round's exact
//! request with a one-token output cap shortly before the entry expires, as
//! long as the expected saving clears a floor. The refresh never enters the
//! conversation; its usage is billed with the turn.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use tokio::time::Instant;

use crate::agent::Provider;
use crate::event::{StreamEvent, Usage};
use crate::message::ChatRequest;

/// Warming never continues past this long after the real request.
pub const MAX_WARMING_AGE: Duration = Duration::from_secs(60 * 60);
/// A refresh is sent only when it is expected to save at least this many dollars.
pub const MIN_EXPECTED_SAVINGS: f64 = 0.05;

/// USD per token.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prices {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    /// 0 when the model has no separate cache-write price.
    pub cache_write: f64,
}

/// Host policy: the cache lifetime of the provider's requests and, looked up
/// at decision time, the model's prices (`None` = economics unavailable).
#[derive(Clone)]
pub struct CacheWarmPolicy {
    pub ttl: Duration,
    pub prices: Arc<dyn Fn() -> Option<Prices> + Send + Sync>,
}

impl std::fmt::Debug for CacheWarmPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheWarmPolicy")
            .field("ttl", &self.ttl)
            .finish_non_exhaustive()
    }
}

/// Refresh at 90% of the TTL while keeping at least ten seconds of margin.
pub fn warming_delay(ttl: Duration) -> Option<Duration> {
    (ttl > Duration::from_secs(10)).then(|| (ttl.mul_f64(0.9)).min(ttl - Duration::from_secs(10)))
}

/// pi `evaluate`, streaming phase (continuation probability 1): the extra
/// price of a cache miss on a prompt this size, minus the refresh's own price
/// (a cache read of the prompt plus one output token).
pub fn expected_savings(p: &Prices, prompt_tokens: usize) -> f64 {
    let n = prompt_tokens as f64;
    let hit = n * p.cache_read;
    let miss = if p.cache_write > 0.0 {
        n * p.cache_write
    } else {
        n * p.input
    };
    let warm = hit + p.output;
    (miss - hit).max(0.0) - warm
}

/// Keeps the cache entry `req` wrote warm until aborted. `sent` is when the
/// real request went out; `spent` collects each refresh's usage.
pub(crate) async fn keep_warm(
    provider: Arc<dyn Provider>,
    mut req: ChatRequest,
    policy: CacheWarmPolicy,
    prompt_tokens: usize,
    sent: Instant,
    spent: Arc<Mutex<Usage>>,
) {
    let Some(delay) = warming_delay(policy.ttl) else {
        return;
    };
    req.max_tokens = Some(1);
    let mut next = sent + delay;
    loop {
        if next > sent + MAX_WARMING_AGE {
            return;
        }
        // A timer can run late after sleep or a blocked runtime. Keep half
        // the planned pre-expiry margin for that; a later refresh is likely a
        // full-price cache write, not a warm.
        let deadline = next + (policy.ttl - delay) / 2;
        tokio::time::sleep_until(next).await;
        if Instant::now() > deadline {
            log::debug!(target: "gray_agent", "cache warm: refresh deadline missed");
            return;
        }
        let Some(prices) = (policy.prices)() else {
            return;
        };
        if prompt_tokens == 0 || expected_savings(&prices, prompt_tokens) < MIN_EXPECTED_SAVINGS {
            return;
        }
        // Best effort: a failed refresh never touches the run.
        let mut stream = provider.stream(req.clone());
        while let Some(event) = stream.next().await {
            match event {
                Ok(StreamEvent::MessageComplete {
                    usage: Some(usage), ..
                }) => {
                    log::debug!(target: "gray_agent", "cache warm: refreshed {} prompt tokens", usage.input_tokens);
                    if let Ok(mut total) = spent.lock() {
                        total.accumulate(&usage);
                    }
                }
                Err(e) => {
                    log::debug!(target: "gray_agent", "cache warm: refresh failed: {e}");
                    break;
                }
                _ => {}
            }
        }
        next = Instant::now() + delay;
    }
}

/// Aborts the warming task when the round's tool phase ends, on every exit.
pub(crate) struct WarmGuard(pub(crate) tokio::task::JoinHandle<()>);

impl Drop for WarmGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[path = "cache_warm_tests.rs"]
#[cfg(test)]
mod tests;
