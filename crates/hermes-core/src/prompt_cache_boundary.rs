//! Builder-declared stable prefixes for Anthropic prompt caching (#81867).
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/prompt_cache_boundary.py` (94 lines).
//!
//! Skill, webhook, and cron builders concatenate a large static scaffold
//! (activation note + expanded skill body) with a small volatile invocation
//! tail (ticket payload, timestamps, run context) into one user-message
//! string. Only the builder knows the exact byte where the volatile tail
//! begins, so it registers the stable prefix here at construction time; the
//! cache planner consults the registry to place a cache breakpoint at that
//! boundary instead of caching the whole message as one atomic block.
//!
//! This deliberately avoids re-parsing scaffold marker strings out of the
//! message at request time: markers can legitimately appear inside skill
//! bodies or inside event payloads (e.g. a helpdesk ticket quoting an agent
//! transcript), and any delimiter-search heuristic then either shrinks the
//! cached prefix or — worse — silently absorbs volatile bytes into it,
//! reintroducing the per-invocation cache miss this exists to fix.
//!
//! The registry is process-local by design. A freshly fired webhook/cron
//! invocation is always built and sent by the same process, which is the
//! only window where the split pays off. Any miss (restart, eviction,
//! historic message) falls back to the pre-existing whole-message policy.
//!
//! Split-shape lifetime: the split is applied only while the skill message is
//! one of the plan's marked endpoints (the last few cacheable messages). Once
//! later turns rotate it out of that window it ships as a single string block
//! again, which changes the block boundary once and re-ingests the prefix from
//! that message onward exactly one time in a long-lived session. Webhook/cron
//! invocations — the workload this exists for — send the skill turn as the
//! newest message every time, so they always hit the split shape; the one-time
//! re-ingest only affects long interactive sessions and nets out far below the
//! per-invocation full rewrite this removes.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Constants — mirrors lines 38-50
// ---------------------------------------------------------------------------

/// A couple dozen distinct active scaffolds (webhook routes x skills x cron
/// jobs) is generous for one gateway process; beyond that, oldest entries
/// fall back to whole-message caching rather than growing unboundedly.
/// Mirrors `_MAX_ENTRIES = 32` (line 41).
pub const MAX_ENTRIES: usize = 32;

/// Entries hold whole expanded skill bodies, so an entry count alone does not
/// bound memory — a handful of large skills can retain tens of MB in a
/// long-lived gateway process. Evict by total retained characters too (a
/// conservative proxy for bytes: actual memory is 1–4x depending on the
/// string's widest code point), always keeping the newest entry so a single
/// oversized scaffold still gets a boundary instead of silently disabling
/// the split.
/// Mirrors `_MAX_CHARS = 4 * 1024 * 1024` (line 50).
pub const MAX_CHARS: usize = 4 * 1024 * 1024;

// Keep underscore-prefixed aliases for 1:1 traceability with Python private names.
#[allow(dead_code)]
const _MAX_ENTRIES: usize = MAX_ENTRIES;
#[allow(dead_code)]
const _MAX_CHARS: usize = MAX_CHARS;

// ---------------------------------------------------------------------------
// Global registry — mirrors `_lock = threading.Lock()` + `_prefixes: OrderedDict`
// (lines 52-53). OrderedDict is modelled as a VecDeque<String> with LRU
// ordering: front = oldest, back = newest. Max 32 entries so linear scans are
// trivially cheap; no external crate needed.
// ---------------------------------------------------------------------------

fn registry() -> &'static Mutex<VecDeque<String>> {
    static REGISTRY: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(VecDeque::new()))
}

#[allow(dead_code)]
fn _lock() -> &'static Mutex<VecDeque<String>> {
    registry()
}

fn total_chars(q: &VecDeque<String>) -> usize {
    q.iter().map(|s| s.chars().count()).sum()
}

// ---------------------------------------------------------------------------
// Public API — mirrors lines 56-94
// ---------------------------------------------------------------------------

/// Record `prefix` as the stable scaffold of a just-built message.
/// Mirrors `register_stable_prefix` (lines 56-66).
pub fn register_stable_prefix(prefix: &str) {
    if prefix.is_empty() {
        return;
    }
    let lock = registry();
    let mut q = lock.lock().unwrap();
    // Mirrors `_prefixes[prefix] = None; _prefixes.move_to_end(prefix)`
    if let Some(pos) = q.iter().position(|p| p == prefix) {
        q.remove(pos);
    }
    q.push_back(prefix.to_string());
    // Mirrors `while len(_prefixes) > _MAX_ENTRIES: _prefixes.popitem(last=False)`
    while q.len() > MAX_ENTRIES {
        q.pop_front();
    }
    // Mirrors `while len(_prefixes) > 1 and sum(map(len, _prefixes)) > _MAX_CHARS:`
    // Keep newest entry even if it alone exceeds _MAX_CHARS.
    while q.len() > 1 && total_chars(&q) > MAX_CHARS {
        q.pop_front();
    }
}

/// Longest registered prefix that is a *proper* prefix of `content`.
///
/// Proper (`len(content) > len(prefix)`) so the split never produces an
/// empty volatile text block, which Anthropic rejects on the wire.
///
/// A hit refreshes the entry's LRU position: a scaffold fired every minute
/// by cron must not be evicted by a burst of one-off skill invocations,
/// which would silently drop it back to whole-message caching.
///
/// Mirrors `find_stable_prefix` (lines 69-88).
pub fn find_stable_prefix(content: &str) -> Option<String> {
    let lock = registry();
    let mut q = lock.lock().unwrap();
    let content_len = content.chars().count();
    let mut best: Option<String> = None;
    let mut best_len: usize = 0;
    // Mirrors the scan loop (lines 80-84) — never mutates OrderedDict mid-iteration.
    for prefix in q.iter() {
        let plen = prefix.chars().count();
        if content_len > plen && content.starts_with(prefix.as_str()) {
            if best.is_none() || plen > best_len {
                best = Some(prefix.clone());
                best_len = plen;
            }
        }
    }
    if let Some(ref b) = best {
        // Mirrors `_prefixes.move_to_end(best)` after the scan (line 87)
        if let Some(pos) = q.iter().position(|p| p == b) {
            // Remove and re-append to mark as most-recently used
            if let Some(val) = q.remove(pos) {
                q.push_back(val);
            }
        }
    }
    best
}

/// Test isolation helper.
/// Mirrors `clear_stable_prefixes` (lines 91-94).
pub fn clear_stable_prefixes() {
    let lock = registry();
    let mut q = lock.lock().unwrap();
    q.clear();
}
