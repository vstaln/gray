//! Cheap content-sanity checks for the truncated-response continuation path.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/repetition_guard.py` (95 lines).
//!
//! Issue #86581: a model in a degenerate repetition loop can spend its ENTIRE
//! output budget echoing one fragment. The `finish_reason=length`
//! continuation path would then retry with a "continue, don't repeat" nudge —
//! stitching a pathological fragment into the final response with no
//! content-sanity check. In the incident behind #86581 a single turn produced
//! a 60,698-char response delivered as 31 Discord messages.
//!
//! These helpers detect repetition-dominated fragments BEFORE the continuation
//! nudge is appended so the turn can abort with a clear user-facing error
//! (mirroring the existing `_thinking_exhausted` guard) instead of flooding.
//!
//! The detection is deliberately conservative: only LONG verbatim repeats
//! (60+ chars) whose occurrences cover a majority of the fragment trip the
//! guard, so ordinary truncated responses (a sentence cut mid-word, a heading
//! repeated, code with similar-looking lines) are never blocked.

use std::collections::HashMap;

/// A fragment must be at least this long before the repetition check runs at
/// all. Short truncations (a sentence cut mid-word) can trivially contain
/// repeated tokens and are legitimately continued.
/// Mirrors `MIN_FRAGMENT_LENGTH = 400` (line 28).
pub const MIN_FRAGMENT_LENGTH: usize = 400;

/// Length of the exact-repeat window. A verbatim repeat of this many chars
/// is far beyond ordinary phrasing reuse (citations, headings, similar code).
/// Mirrors `_REPEAT_WINDOW = 60` (line 32).
const REPEAT_WINDOW: usize = 60;

/// A window that repeats at least this many times is a repetition signal,
/// even for short fragments. Mirrors `_MIN_REPEAT_COUNT = 5` (line 36).
const MIN_REPEAT_COUNT: usize = 5;

/// A fragment is "repetition-dominated" when repeated windows account for at
/// least this fraction of its characters. Mirrors `_DOMINANCE_RATIO = 0.5`
/// (line 40).
const DOMINANCE_RATIO: f64 = 0.5;

/// True when `text` is dominated by verbatim repeated fragments.
///
/// A truncated response is "repetition-dominated" when a single 60+ char
/// substring appears often enough that its occurrences cover at least half
/// of the fragment. That shape is the signature of a model repetition
/// loop (issue #86581), and continuing such a fragment is pointless — the
/// continuation nudge would just stitch more repeated text into the final
/// response.
///
/// Returns false for empty / short inputs (fail-open: never blocks a
/// continuation the guard cannot confidently judge). The Python original also
/// returns false for non-string inputs; in Rust the type system enforces
/// `&str` so that branch is implicit.
///
/// Mirrors `is_repetition_dominated` (lines 43-81).
pub fn is_repetition_dominated(text: &str) -> bool {
    // Python: n = len(text)  — counts Unicode codepoints; use chars().count()
    let n = text.chars().count();
    if n < MIN_FRAGMENT_LENGTH {
        return false;
    }

    // Fast path: one normalized line duplicated often enough to cover half
    // the fragment (the most common echo shape — a repeated paragraph or
    // sentence on its own line). Cheap, no big allocations.
    // Mirrors lines 65-66.
    if line_repetition_dominated(text, n) {
        return true;
    }

    // General path: fixed-size exact-repeat windows, sliding one char at a
    // time. Catches repetition loops that do not align to line boundaries.
    // Mirrors lines 68-81.
    let window = REPEAT_WINDOW;
    if n < window {
        return false;
    }
    // A window must appear this many times for its occurrences to cover
    // >= DOMINANCE_RATIO of the fragment (and at least _MIN_REPEAT_COUNT).
    // Mirrors line 73: max(_MIN_REPEAT_COUNT, math.ceil(n * _DOMINANCE_RATIO / window))
    let needed = std::cmp::max(
        MIN_REPEAT_COUNT,
        (n as f64 * DOMINANCE_RATIO / window as f64).ceil() as usize,
    );
    // Collect chars once so slicing is O(1) and respects Unicode boundaries
    // (Python slices by codepoints; Rust &str slices by bytes would be wrong).
    let chars: Vec<char> = text.chars().collect();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for i in 0..=n - window {
        let key: String = chars[i..i + window].iter().collect();
        let c = counts.get(&key).copied().unwrap_or(0) + 1;
        if c >= needed {
            return true;
        }
        counts.insert(key, c);
    }
    false
}

/// True when a single normalized line covers half the fragment via repeats.
/// Mirrors `_line_repetition_dominated` (lines 84-95).
fn line_repetition_dominated(text: &str, n: usize) -> bool {
    let mut counts: HashMap<String, usize> = HashMap::new();
    // Python: text.splitlines() — split on universal newlines
    for line in text.lines() {
        let norm = line.trim();
        if norm.is_empty() {
            continue;
        }
        *counts.entry(norm.to_string()).or_insert(0) += 1;
    }
    for (line, c) in &counts {
        // Python: c >= _MIN_REPEAT_COUNT and c * len(line) >= n * _DOMINANCE_RATIO
        // len(line) is codepoints; use chars().count() for 1:1.
        if *c >= MIN_REPEAT_COUNT
            && (*c * line.chars().count()) as f64 >= n as f64 * DOMINANCE_RATIO
        {
            return true;
        }
    }
    false
}

// Keep underscore-prefixed alias for 1:1 traceability with Python private name.
#[allow(dead_code)]
fn _line_repetition_dominated(text: &str, n: usize) -> bool {
    line_repetition_dominated(text, n)
}
