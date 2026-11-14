use std::time::{Duration, Instant};

use gray_core::event::Usage;

use crate::setup::ModelRate;

static CACHE_TTL_SECS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(300);

pub fn cache_ttl() -> Duration {
    Duration::from_secs(CACHE_TTL_SECS.load(std::sync::atomic::Ordering::Relaxed))
}

pub fn set_cache_ttl(d: Duration) {
    CACHE_TTL_SECS.store(d.as_secs(), std::sync::atomic::Ordering::Relaxed);
}

const NOISE_FLOOR_TOKENS: usize = 1024;

pub const WARN_MISS_TOKENS: usize = 20_000;

pub const WARN_MISS_COST: f64 = 0.10;

/// Default cost (USD) a cold-cache resend must reach before the composer asks
/// for a second Enter. Saved config `cold_confirm_usd` overrides; 0 is off.
pub const COLD_CONFIRM_USD: f64 = 0.50;

static COLD_CONFIRM_MICROS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new((COLD_CONFIRM_USD * 1e6) as u64);

pub fn cold_confirm_usd() -> f64 {
    COLD_CONFIRM_MICROS.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6
}

pub fn set_cold_confirm_usd(usd: f64) {
    COLD_CONFIRM_MICROS.store(
        (usd.max(0.0) * 1e6) as u64,
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// What sending now would re-bill because the cache has gone cold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColdResend {
    pub tokens: usize,
    pub cost: f64,
    pub idle: Duration,
}

impl ColdResend {
    pub fn confirm_line(&self) -> String {
        format!(
            "Cache cold after {}m idle: sending re-bills {} tokens (~{}). Enter again to send",
            self.idle.as_secs() / 60,
            crate::setup::format_context_length(self.tokens),
            crate::setup::format_cost(self.cost),
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CacheMiss {
    pub missed_tokens: usize,

    pub missed_cost: f64,

    pub idle: Duration,

    pub model_changed: bool,

    pub reconnected: bool,

    /// Why the prompt prefix changed, as reported by the agent
    /// ("tool list changed"); `None` when it did not change.
    pub cause: Option<String>,
}

impl CacheMiss {
    pub fn notice(&self) -> Option<String> {
        if self.missed_tokens < WARN_MISS_TOKENS && self.missed_cost < WARN_MISS_COST {
            return None;
        }
        let label = if self.model_changed {
            "Cache miss after model switch".to_string()
        } else if let Some(cause) = &self.cause {
            format!("Cache miss ({cause})")
        } else if self.reconnected {
            "Cache miss after reconnect".to_string()
        } else if self.idle >= cache_ttl() {
            format!("Cache miss after {}m idle", self.idle.as_secs() / 60)
        } else {
            "Cache miss".to_string()
        };
        let tokens = crate::setup::format_context_length(self.missed_tokens);
        let mut notice = format!("{label}: {tokens} tokens re-billed");
        if self.missed_cost >= 0.01 {
            notice.push_str(&format!(
                " (~{})",
                crate::setup::format_cost(self.missed_cost)
            ));
        }
        Some(notice)
    }
}

#[derive(Debug, Clone)]
struct LastRequest {
    at: Instant,

    prompt_tokens: usize,
    model: String,

    reported_cache: bool,

    usage: Usage,

    /// Extra cost per prompt token of a cold resend over a cached one.
    resend_per_token: f64,
}

#[derive(Debug, Clone, Default)]
pub struct CacheTracker {
    last: Option<LastRequest>,

    paused_at: Option<Instant>,

    reconnected: bool,

    prefix_cause: Option<String>,
}

impl CacheTracker {
    /// The agent saw the prompt prefix change before the next request; the
    /// miss that request reports (if any) is labelled with this cause.
    pub fn note_prefix_change(&mut self, cause: String) {
        self.prefix_cause = Some(cause);
    }

    pub fn note(
        &mut self,
        usage: &Usage,
        model: &str,
        rate: Option<ModelRate>,
        now: Instant,
    ) -> Option<CacheMiss> {
        let prompt_tokens = usage.input_tokens;
        let read = usage.cache_read_input_tokens.max(usage.cached_tokens);
        let write = usage.cache_write_input_tokens;
        let reported = read + write > 0;

        let miss_at = self.paused_at.unwrap_or(now);

        let cause = self.prefix_cause.take();
        // First request after a resume: there is no previous request to
        // compare with, but the agent compared against the saved session's
        // last prefix, so a known change still gets judged.
        let resumed_prev = (self.last.is_none() && cause.is_some()).then(|| LastRequest {
            at: miss_at,
            prompt_tokens,
            model: model.to_string(),
            reported_cache: false,
            usage: Usage::default(),
            resend_per_token: 0.0,
        });
        let resend_per_token = rate
            .as_ref()
            .filter(|r| r.has_cache_prices)
            .map_or(0.0, |r| {
                let cold = if r.cache_write > 0.0 {
                    r.cache_write
                } else {
                    r.input
                };
                (cold - r.cache_read).max(0.0)
            });
        let miss = match self.last.as_ref().or(resumed_prev.as_ref()) {
            Some(prev) if prev.usage == *usage => None,
            Some(prev) => detect_miss(
                prev,
                usage,
                prompt_tokens,
                read,
                write,
                rate,
                miss_at,
                model,
                std::mem::take(&mut self.reconnected),
            ),

            None => None,
        }
        .map(|miss| CacheMiss { cause, ..miss });

        let reported_cache = self
            .last
            .as_ref()
            .map(|p| p.reported_cache)
            .unwrap_or(false)
            || reported;
        if prompt_tokens > 0 {
            let was_paused = self.paused_at.is_some();
            self.last = Some(LastRequest {
                at: now,
                prompt_tokens,
                model: model.to_string(),
                reported_cache,
                usage: *usage,
                resend_per_token,
            });

            if was_paused {
                self.paused_at = Some(now);
            }
        }
        miss
    }

    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        let last = self.last.as_ref()?;
        if !last.reported_cache {
            return None;
        }

        let observed_at = self.paused_at.unwrap_or(now);
        let age = observed_at.saturating_duration_since(last.at);
        (age < cache_ttl()).then(|| cache_ttl() - age)
    }

    pub fn is_cold(&self, now: Instant) -> bool {
        let Some(last) = self.last.as_ref() else {
            return false;
        };
        if !last.reported_cache {
            return false;
        }
        self.paused_at
            .unwrap_or(now)
            .saturating_duration_since(last.at)
            >= cache_ttl()
    }

    /// A warm ping refreshed the provider cache at `at`: restart the idle
    /// clock from there so a warmed cache isn't reported cold.
    pub fn note_warm(&mut self, at: Instant) {
        if self.paused_at.is_some() {
            return;
        }
        if let Some(last) = self.last.as_mut().filter(|l| l.reported_cache && at > l.at) {
            last.at = at;
        }
    }

    /// The resend a cold cache would cost now; `None` while warm or unpriced.
    pub fn cold_resend(&self, now: Instant) -> Option<ColdResend> {
        if !self.is_cold(now) {
            return None;
        }
        let last = self.last.as_ref()?;
        Some(ColdResend {
            tokens: last.prompt_tokens,
            cost: last.prompt_tokens as f64 * last.resend_per_token,
            idle: now.saturating_duration_since(last.at),
        })
    }

    pub(crate) fn pause(&mut self, now: Instant) {
        if self.paused_at.is_none() {
            self.paused_at = Some(now);
        }
    }

    pub(crate) fn resume(&mut self, now: Instant) {
        let Some(paused_at) = self.paused_at.take() else {
            return;
        };
        let Some(last) = self.last.as_mut() else {
            return;
        };
        last.at += now.saturating_duration_since(paused_at);
    }

    pub(crate) fn rearm(&mut self, now: Instant) {
        self.paused_at = None;
        if let Some(last) = self.last.as_mut() {
            last.at = now;
        }
    }

    pub(crate) fn note_reconnect(&mut self) {
        self.reconnected = true;
    }

    pub fn reset(&mut self) {
        self.last = None;
        self.reconnected = false;
        self.prefix_cause = None;
    }
}

#[allow(clippy::too_many_arguments)]
fn detect_miss(
    prev: &LastRequest,
    usage: &Usage,
    prompt_tokens: usize,
    read: usize,
    write: usize,
    rate: Option<ModelRate>,
    now: Instant,
    model: &str,
    reconnected: bool,
) -> Option<CacheMiss> {
    if prompt_tokens == 0 {
        return None;
    }

    if read + write == 0 && !prev.reported_cache {
        return None;
    }

    let missed_tokens = prev.prompt_tokens.min(prompt_tokens).saturating_sub(read);
    if missed_tokens <= NOISE_FLOOR_TOKENS {
        return None;
    }
    Some(CacheMiss {
        missed_tokens,
        missed_cost: missed_cost(missed_tokens, usage, read, write, rate),
        idle: now.saturating_duration_since(prev.at),
        model_changed: !model.is_empty() && !prev.model.is_empty() && model != prev.model,
        reconnected,
        cause: None,
    })
}

fn missed_cost(
    missed_tokens: usize,
    usage: &Usage,
    read: usize,
    write: usize,
    rate: Option<ModelRate>,
) -> f64 {
    let Some(rate) = rate else {
        return 0.0;
    };
    if !rate.has_cache_prices {
        return 0.0;
    }

    let mut fresh = usage.non_cached_input_tokens as f64;
    if fresh == 0.0 {
        fresh = (usage.input_tokens as f64 - read as f64 - write as f64).max(0.0);
    }
    let paid_tokens = fresh + write as f64;
    if paid_tokens <= 0.0 {
        return 0.0;
    }
    let paid_per_token = (fresh * rate.input + write as f64 * rate.cache_write) / paid_tokens;
    missed_tokens as f64 * (paid_per_token - rate.cache_read).max(0.0)
}

pub fn format_remaining(remaining: Duration) -> String {
    let secs = remaining.as_secs();
    if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod cold_tests {
    use super::*;

    fn usage(input: usize, read: usize) -> Usage {
        Usage {
            input_tokens: input,
            cache_read_input_tokens: read,
            ..Usage::default()
        }
    }

    fn rate() -> ModelRate {
        ModelRate {
            input: 3e-6,
            output: 15e-6,
            cache_read: 0.3e-6,
            cache_write: 3.75e-6,
            has_cache_prices: true,
            tiers: Vec::new(),
        }
    }

    #[test]
    fn warm_ping_keeps_cache_warm_and_cold_resend_is_priced() {
        let t0 = Instant::now();
        let mut t = CacheTracker::default();
        t.note(&usage(200_000, 190_000), "m", Some(rate()), t0);
        let ttl = cache_ttl();

        let later = t0 + ttl + Duration::from_secs(1);
        let cold = t.cold_resend(later).expect("cold after ttl");
        assert_eq!(cold.tokens, 200_000);
        assert!((cold.cost - 200_000.0 * 3.45e-6).abs() < 1e-9);

        t.note_warm(t0 + ttl / 2);
        assert!(!t.is_cold(later), "warm ping restarts the idle clock");
        assert!(t.cold_resend(later).is_none());
        assert!(t.is_cold(t0 + ttl / 2 + ttl));
    }

    #[test]
    fn warm_stamp_ignored_while_paused_or_unreported() {
        let t0 = Instant::now();
        let mut t = CacheTracker::default();
        t.note(&usage(50_000, 0), "m", Some(rate()), t0);
        t.note_warm(t0 + Duration::from_secs(10));
        assert!(
            t.cold_resend(t0 + cache_ttl() * 2).is_none(),
            "no cache reported"
        );

        let mut t = CacheTracker::default();
        t.note(&usage(50_000, 40_000), "m", Some(rate()), t0);
        t.pause(t0);
        t.note_warm(t0 + cache_ttl() * 3);
        t.resume(t0);
        assert!(t.is_cold(t0 + cache_ttl()));
    }
}
