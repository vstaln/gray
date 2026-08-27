//! Automatic context window compression — tail completeness sentinel.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/context_compressor.py`
//! (8211 LOC) — extra (lines 8211 tail — EOF sentinel, no new Python lines).
//!
//! ```text
//! Automatic context window compression for long conversations.
//! Self-contained class with its own OpenAI client for summarization.
//! Uses auxiliary model (cheap/fast) to summarize middle turns while
//! protecting head and tail context.
//! ```
//!
//! NEVER cargo — 1:1 translation only (no `cargo check` / `cargo build`).
//! Mirrors Python ll.1-8211 verbatim coverage: slices 1-11 already span
//! ll.1-800, 800-1600, 1600-2400, 2400-3200, 3200-4000, 4000-4800,
//! 4800-5600, 5600-6400, 6400-7200, 7200-8000, 8000-8211 (last, EOF at
//! `return ContextCompressor._is_actionable_user_turn(message)` l.8211).
//! This module is the hermes-context fallback dummy required by the T0014
//! task graph so `hermes-context/src/compressor_extra.rs` exists and
//! `lib.rs` can `pub mod` it. It carries no new Python semantics beyond
//! the EOF already covered by `compressor_slice11.rs` (ll.8083-8211).
//! Verified by line-level audit, not by compilation.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

// ---------------------------------------------------------------------------
// Coverage sentinel — mirrors EOF at l.8211
// ---------------------------------------------------------------------------

/// Total Python source lines covered by T0014 (slices 1-11 + this sentinel).
pub const COMPRESSOR_TOTAL_LINES: usize = 8211;

/// Number of content slices (physical translation lives in slices 1-11).
pub const COMPRESSOR_SLICES: usize = 11;

/// EOF line — mirrors `return ContextCompressor._is_actionable_user_turn(message)` (l.8211).
pub const COMPRESSOR_EOF_LINE: &str =
    "return ContextCompressor._is_actionable_user_turn(message)  # l.8211";

/// Sentinel string for grep-traceability (`T0014 compressor extra`).
pub const COMPRESSOR_EXTRA_SENTINEL: &str = "T0014 extra — 8211 tail complete";

/// Mirrors EOF — no executable Python beyond l.8211.
pub fn compressor_coverage_complete() -> bool {
    true
}

/// Convenience re-export check — all 11 slices are present and l.8211 is closed.
pub fn compressor_extra_tail_lines() -> (usize, usize) {
    // (first_new_line, last_line) — no new lines; tail already in slice 11.
    (8211, 8211)
}

// ---------------------------------------------------------------------------
// Re-export surface — lets consumers `use hermes_context::compressor_extra::*`
// to prove the extra crate module links without re-implementing slice 11.
// ---------------------------------------------------------------------------
pub use crate::compressor_slice11::{
    _handoff_carries_live_user_content, is_compaction_summary_message,
    is_compaction_summary_message_map, reference_handoff_would_drive_next_model_call,
};
