//! Prompt-cache warmth timer + cache-miss warning.
//!
//! Two UI affordances over one piece of state — the last provider request
//! this process observed ([`CacheTracker`]):
//!
//! 1. A warmth countdown for the composer footer. After a request, the
//!    provider's prompt cache stays warm for a short idle window (both
//!    Anthropic's `cache_control` and OpenAI's automatic prefix caching
//!    expire after roughly five idle minutes), so the footer shows
//!    `◷ 4m` — "prompt cache warm, about 4 min left". It freezes for the
//!    duration of an active turn, then rearms to five minutes when the turn
//!    returns to idle, and hides once the cache is cold (or the provider
//!    never reports cache activity).
//! 2. A transcript warning when a request re-billed prompt tokens the
//!    previous request should have served from cache ("Cache miss after
//!    6m idle: 45.2k tokens re-billed (~$0.12)"), which is what an idle
//!    gap past the TTL, a model switch, or a provider-side cache eviction
//!    looks like on the bill.
//!
//! Port of pi's `packages/coding-agent/src/core/cache-stats.ts`
//! (reference checkout under `reference/pi-mono`): same detection rules,
//! same thresholds, same notice wording, in gray's per-round `StepUsage`
//! shape (each round's report already carries the full prompt).
//!
//! Known limitation: the agent loop's in-turn budgeted compaction
//! (overflow recovery inside gray-core, no event) is invisible here, so
//! at most one spurious notice can follow it — pi resets on its
//! compaction session entries for the same reason.

use std::time::{Duration, Instant};

use gray_core::event::Usage;

use crate::setup::ModelRate;

/// Idle TTL of a warm prompt cache, in seconds. Anthropic's default
/// `cache_control` expiry (5 minutes) is the default; OpenAI's automatic
/// prefix cache is in the same ballpark. A plugin provider can declare its
/// real lifetime (`request.cache_ttl_secs`) — `build_agent` sets it on
/// every build so a provider switch resets to the default. Past it, the
/// next request re-bills the whole prompt.
static CACHE_TTL_SECS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(300);

/// The connected provider's prompt-cache idle lifetime (see
/// [`CACHE_TTL_SECS`]).
pub fn cache_ttl() -> Duration {
    Duration::from_secs(CACHE_TTL_SECS.load(std::sync::atomic::Ordering::Relaxed))
}

/// Sets the prompt-cache idle lifetime; `build_agent` calls it per build.
pub fn set_cache_ttl(d: Duration) {
    CACHE_TTL_SECS.store(d.as_secs(), std::sync::atomic::Ordering::Relaxed);
}

/// Per-request misses at or below this are cache breakpoint granularity
/// noise (a few tokens of re-tokenized tail), not a real miss.
const NOISE_FLOOR_TOKENS: usize = 1024;

/// A miss only earns a transcript warning at this size: the footer timer
/// covers the routine case, the warning is for the "huge" ones.
pub const WARN_MISS_TOKENS: usize = 20_000;

/// ... or this extra cost vs. a full cache hit (pi's `$0.10` bar).
pub const WARN_MISS_COST: f64 = 0.10;

/// One counted cache miss: prompt tokens that were in the previous
/// request's prompt but were not read from cache, plus what that cost
/// above a full hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CacheMiss {
    /// Prompt tokens re-billed instead of served from cache.
    pub missed_tokens: usize,
    /// Extra dollars paid vs. a full cache hit; 0 when pricing is unknown.
    pub missed_cost: f64,
    /// Idle time since the previous request (which last refreshed the cache).
    pub idle: Duration,
    /// True when the model changed relative to the previous request.
    pub model_changed: bool,
}

impl CacheMiss {
    /// The warning row for this miss, or `None` below the "huge" bar
    /// ([`WARN_MISS_TOKENS`] / [`WARN_MISS_COST`]). States observable
    /// facts only: the miss itself, a model switch, or an idle gap past
    /// the TTL — never a guess at the cause.
    pub fn notice(&self) -> Option<String> {
        if self.missed_tokens < WARN_MISS_TOKENS && self.missed_cost < WARN_MISS_COST {
            return None;
        }
        let label = if self.model_changed {
            "Cache miss after model switch".to_string()
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

/// The last request seen by the tracker: everything in its prompt should
/// be cached for the next one.
#[derive(Debug, Clone)]
struct LastRequest {
    /// Effective idle anchor for the request that last warmed the cache.
    /// It starts when the response lands and moves past active-turn pauses.
    at: Instant,
    /// Inclusive prompt tokens of that request.
    prompt_tokens: usize,
    model: String,
    /// Sticky: some earlier request reported cache activity. Distinguishes
    /// a total miss on a cache-read-only provider (OpenAI-style, writes
    /// unreported) from a provider that never reports caching at all.
    reported_cache: bool,
    /// The exact report, so a re-emission of the same request (the loop
    /// keeps the previous report when a round carries no usage of its
    /// own) is never counted as a second request.
    usage: Usage,
}

/// Tracks the last provider request to drive the footer warmth timer and
/// detect cache misses. One instance per composer session; reset on `/new`
/// and at every compaction (the context legitimately changed, so the next
/// request's prompt is new content, not re-billed content).
#[derive(Debug, Clone, Default)]
pub struct CacheTracker {
    last: Option<LastRequest>,
    /// Start of the current active-turn pause, or the latest in-turn usage
    /// report. While set, wall-clock time cannot consume the warmth TTL.
    paused_at: Option<Instant>,
}

impl CacheTracker {
    /// Records one per-round usage report and returns the cache miss it
    /// paid for, if any. `rate` is the model's per-token pricing (unknown
    /// price = zero cost, never a guess).
    pub fn note(
        &mut self,
        usage: &Usage,
        model: &str,
        rate: Option<ModelRate>,
        now: Instant,
    ) -> Option<CacheMiss> {
        // Inclusive prompt total; the breakdown is non-overlapping parts of it.
        let prompt_tokens = usage.input_tokens;
        let read = usage.cache_read_input_tokens.max(usage.cached_tokens);
        let write = usage.cache_write_input_tokens;
        let reported = read + write > 0;
        // A request reported during an active turn compares against the time
        // that turn began: generation/tool time is not idle cache time.
        let miss_at = self.paused_at.unwrap_or(now);

        let miss = match &self.last {
            // Same report twice: the agent loop re-emits the last round's
            // usage when a later round reports none, so the tracker would
            // otherwise see one request as two and invent a miss.
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
            ),
            // First request of the session: nothing was cached before it.
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
            // This report starts a fresh full TTL. Freeze that new value for
            // whatever remains of the active turn instead of aging it.
            if was_paused {
                self.paused_at = Some(now);
            }
        }
        miss
    }

    /// Time left before the warm cache goes cold, frozen while a turn is
    /// active, or `None` when no cache activity has ever been reported
    /// (provider without caching) or the TTL already elapsed.
    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        let last = self.last.as_ref()?;
        if !last.reported_cache {
            return None;
        }
        // `paused_at` is the effective clock: active turns neither consume
        // warmth nor make the footer repaint a lower value.
        let observed_at = self.paused_at.unwrap_or(now);
        let age = observed_at.saturating_duration_since(last.at);
        (age < cache_ttl()).then(|| cache_ttl() - age)
    }

    /// True when the warm cache this process last saw has since expired, so
    /// the next request re-bills the whole prefix whatever it contains —
    /// which makes a context rewrite (the stale-output mask) free.
    ///
    /// Distinct from [`remaining`](Self::remaining) returning `None`: that
    /// also covers a provider which never reported caching, where nothing is
    /// known to be lost. `paused_at` is the right clock here: it freezes at
    /// the turn boundary, so the answer is the idle gap the turn is starting
    /// after.
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

    /// Freezes the countdown for an active turn. Idempotent because setup
    /// and the normal request path can both reassert the Working status.
    pub(crate) fn pause(&mut self, now: Instant) {
        if self.paused_at.is_none() {
            self.paused_at = Some(now);
        }
    }

    /// Resumes from the frozen value without refreshing it. This is only for
    /// setup/build aborts, where no completed provider turn can restart the
    /// cache window.
    pub(crate) fn resume(&mut self, now: Instant) {
        let Some(paused_at) = self.paused_at.take() else {
            return;
        };
        let Some(last) = self.last.as_mut() else {
            return;
        };
        last.at += now.saturating_duration_since(paused_at);
    }

    /// Ends an active turn and starts a fresh warmth window. The completed
    /// provider turn is the new idle anchor, so active time is never charged
    /// against the five-minute TTL.
    pub(crate) fn rearm(&mut self, now: Instant) {
        self.paused_at = None;
        if let Some(last) = self.last.as_mut() {
            last.at = now;
        }
    }

    /// Forgets the last request: new conversation, or a compaction (the
    /// next request's prompt is new content, not re-billed content). An
    /// active-turn pause is orthogonal and deliberately survives the reset.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

/// The miss one request paid relative to the previous one. `None` when
/// nothing is counted: first turn, no cache activity ever reported, or a
/// miss below the noise floor.
// Request-shape comparison: every field is a scalar read straight off the
// two requests being compared; a struct would only rename the argument list.
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
) -> Option<CacheMiss> {
    if prompt_tokens == 0 {
        return None;
    }
    // A zero-cache request only counts when cache activity was reported
    // before: on cache-read-only providers that is a total miss, while on
    // providers that never report caching it means nothing.
    if read + write == 0 && !prev.reported_cache {
        return None;
    }
    // Only tokens the previous request already carried can be a miss —
    // everything newer is fresh content, correctly billed as input.
    let missed_tokens = prev.prompt_tokens.min(prompt_tokens).saturating_sub(read);
    if missed_tokens <= NOISE_FLOOR_TOKENS {
        return None;
    }
    Some(CacheMiss {
        missed_tokens,
        missed_cost: missed_cost(missed_tokens, usage, read, write, rate),
        idle: now.saturating_duration_since(prev.at),
        model_changed: !model.is_empty() && !prev.model.is_empty() && model != prev.model,
    })
}

/// Extra dollars the miss cost vs. a full cache hit: missed tokens land in
/// the input or cache-write buckets, so they are billed at the paid rate
/// (blended across this request's own fresh input + cache write) instead
/// of the cache-read rate. Zero when the model is unpriced or has no
/// cache prices (then there is no delta to pay).
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
    // Fresh input for this request: the breakdown field when the provider
    // fills it, else derived from the inclusive total.
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

/// Footer countdown text for a warm cache: `4m`, fading to `48s` inside
/// the last minute.
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
