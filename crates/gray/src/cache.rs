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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CacheMiss {
    pub missed_tokens: usize,

    pub missed_cost: f64,

    pub idle: Duration,

    pub model_changed: bool,

    pub reconnected: bool,
}

impl CacheMiss {
    pub fn notice(&self) -> Option<String> {
        if self.missed_tokens < WARN_MISS_TOKENS && self.missed_cost < WARN_MISS_COST {
            return None;
        }
        let label = if self.model_changed {
            "Cache miss after model switch".to_string()
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
}

#[derive(Debug, Clone, Default)]
pub struct CacheTracker {
    last: Option<LastRequest>,

    paused_at: Option<Instant>,

    reconnected: bool,
}

impl CacheTracker {
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

        let miss = match &self.last {
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
        };

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
#[path = "cache_tests.rs"]
mod tests;
