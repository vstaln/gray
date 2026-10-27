use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use tokio::time::Instant;

use crate::agent::Provider;
use crate::event::{StreamEvent, Usage};
use crate::message::ChatRequest;

pub const MAX_WARMING_AGE: Duration = Duration::from_secs(60 * 60);

pub const MAX_IDLE_WARMING_AGE: Duration = Duration::from_secs(30 * 60);

pub const IDLE_CONTINUATION_PROBABILITY: f64 = 0.15;

pub const MIN_EXPECTED_SAVINGS: f64 = 0.05;

pub const MIN_WARM_TOKENS: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum WarmPhase {
    Streaming,
    Idle,
}

pub(crate) type WarmPhaseCell = Arc<Mutex<WarmPhase>>;

pub(crate) fn warm_phase() -> WarmPhaseCell {
    Arc::new(Mutex::new(WarmPhase::Streaming))
}

pub(crate) fn settle_warm_phase(phase: &WarmPhaseCell) {
    if let Ok(mut p) = phase.lock() {
        *p = WarmPhase::Idle;
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prices {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,

    pub cache_write: f64,
}

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

pub fn warming_delay(ttl: Duration) -> Option<Duration> {
    (ttl > Duration::from_secs(10)).then(|| (ttl.mul_f64(0.9)).min(ttl - Duration::from_secs(10)))
}

pub fn expected_savings(
    p: &Prices,
    prompt_tokens: usize,
    warm_output: u32,
    continuation_probability: f64,
) -> f64 {
    let n = prompt_tokens as f64;
    let hit = n * p.cache_read;
    let miss = if p.cache_write > 0.0 {
        n * p.cache_write
    } else {
        n * p.input
    };
    let warm = hit + f64::from(warm_output) * p.output;
    continuation_probability * (miss - hit).max(0.0) - warm
}

pub fn worth_refreshing(
    prices: Option<&Prices>,
    prompt_tokens: usize,
    warm_output: u32,
    cache_reported: bool,
    continuation_probability: f64,
) -> bool {
    if prompt_tokens == 0 {
        return false;
    }
    let big = prompt_tokens >= MIN_WARM_TOKENS;
    match prices.filter(|p| p.input > 0.0 || p.cache_read > 0.0 || p.cache_write > 0.0) {
        Some(p) => {
            let savings = expected_savings(p, prompt_tokens, warm_output, continuation_probability);
            savings >= MIN_EXPECTED_SAVINGS || (big && savings > 0.0)
        }
        None => big && cache_reported,
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WarmHint {
    pub(crate) prompt_tokens: usize,
    pub(crate) cache_reported: bool,
    pub(crate) warm_off: bool,
}

pub(crate) async fn keep_warm(
    provider: Arc<dyn Provider>,
    mut req: ChatRequest,
    policy: CacheWarmPolicy,
    hint: Arc<Mutex<WarmHint>>,
    sent: Instant,
    spent: Arc<Mutex<Usage>>,
    phase: WarmPhaseCell,
) {
    let Some(delay) = warming_delay(policy.ttl) else {
        return;
    };

    let warm_output = provider.warm_output_cap(&req);
    req.max_tokens = Some(warm_output);
    let mut next = sent + delay;
    loop {
        let deadline = next + (policy.ttl - delay) / 2;
        tokio::time::sleep_until(next).await;
        if Instant::now() > deadline {
            log::debug!(target: "gray_agent", "cache warm: refresh deadline missed");
            return;
        }

        let idle = phase.lock().map(|p| *p == WarmPhase::Idle).unwrap_or(true);
        let horizon = sent
            + if idle {
                MAX_IDLE_WARMING_AGE
            } else {
                MAX_WARMING_AGE
            };
        if Instant::now() > horizon {
            return;
        }

        let WarmHint {
            prompt_tokens,
            cache_reported,
            warm_off,
        } = hint.lock().map(|h| *h).unwrap_or_default();
        if warm_off {
            return;
        }
        if !worth_refreshing(
            (policy.prices)().as_ref(),
            prompt_tokens,
            warm_output,
            cache_reported,
            if idle {
                IDLE_CONTINUATION_PROBABILITY
            } else {
                1.0
            },
        ) {
            return;
        }

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
                    let cached = usage.cached_tokens.max(usage.cache_read_input_tokens);
                    if cache_reported
                        && usage.input_tokens >= MIN_WARM_TOKENS
                        && cached * 2 < usage.input_tokens
                    {
                        log::warn!(target: "gray_agent", "cache warm: refresh read {cached} of {} prompt tokens from cache; warming off for this session", usage.input_tokens);
                        if let Ok(mut h) = hint.lock() {
                            h.warm_off = true;
                        }
                        return;
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

pub(crate) struct WarmGuard(pub(crate) tokio::task::JoinHandle<()>);

impl Drop for WarmGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(crate) struct ActiveWarm {
    #[allow(dead_code)]
    pub(crate) guard: WarmGuard,

    pub(crate) phase: WarmPhaseCell,

    pub(crate) spent: Arc<Mutex<Usage>>,
}

impl ActiveWarm {
    pub(crate) fn settle(&self) {
        settle_warm_phase(&self.phase);
    }
}
