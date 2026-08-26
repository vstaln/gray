//! User-facing summaries for manual compression commands.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/manual_compression_feedback.py` (138 LOC).
//! T0020 — full file (lines 1-138).
//!
//! ```text
//! User-facing summaries for manual compression commands.
//! ```
//!
//! NEVER cargo — 1:1 translation only (no `cargo check` / `cargo build`).
//! Mirrors Python ll.1-138 verbatim; line numbers in comments refer to the
//! 138-line source file. Verified by line-level audit, not by compilation.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

// ---------------------------------------------------------------------------
// Imports — mirrors Python ll.3-8
// ---------------------------------------------------------------------------
use serde_json::{json, Value};

// Python imports (ll.3-7) — stdlib:
//   from __future__ import annotations (no-op)
//   from typing import Any, Sequence
//   from agent.redact import redact_sensitive_text
// Mapped: serde_json::Value for Any/Dict, Vec<Value>/&[Value] for Sequence[dict],
//   redact_sensitive_text stub below (canonical impl lives in hermes-core).

// ---------------------------------------------------------------------------
// Logger — mirrors implicit module logger (no explicit logger in py)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "manual_compression_feedback";

// ---------------------------------------------------------------------------
// Helpers mirroring Python l.7 import
// ---------------------------------------------------------------------------

/// Stub: mirrors `agent.redact.redact_sensitive_text` (ll.7, 127).
///
/// Real impl scrubs credentials before a provider exception crosses the
/// user-facing UI boundary (force=True). Stub returns input verbatim;
/// canonical impl in hermes-core replaces this when crates merge. Kept
/// grep-traceable for 1:1 audit.
pub fn redact_sensitive_text(text: &str, force: bool) -> String {
    let _ = force;
    text.to_string()
}

#[allow(dead_code)]
fn _redact_sensitive_text(text: &str, force: bool) -> String {
    redact_sensitive_text(text, force)
}

// ---------------------------------------------------------------------------
// Message type — mirrors `Sequence[dict[str, Any]]` (ll.5, 40-41)
// ---------------------------------------------------------------------------
/// Mirrors `dict[str, Any]` message shape (ll.5, 40-41).
/// Python messages are `{"role": "...", "content": ..., ...}`.
/// Rust: `serde_json::Value::Object` preserves the open-dict shape.
pub type Message = Value;
/// Mirrors `Sequence[dict[str, Any]]` / `List[Dict[str, Any]]`
pub type Messages = Vec<Message>;

// ---------------------------------------------------------------------------
// Helpers — mirrors formatting used at ll.90, 94-96
// ---------------------------------------------------------------------------

fn format_comma(n: i64) -> String {
    // Mirrors Python `f"{n:,}"` — comma-separated thousands.
    let neg = n < 0;
    let s = n.abs().to_string();
    let mut out = String::new();
    let mut count = 0;
    for ch in s.chars().rev() {
        if count == 3 {
            out.push(',');
            count = 0;
        }
        out.push(ch);
        count += 1;
    }
    let mut res: String = out.chars().rev().collect();
    if neg {
        res = format!("-{}", res);
    }
    res
}

#[allow(dead_code)]
fn _format_comma(n: i64) -> String {
    format_comma(n)
}

// ---------------------------------------------------------------------------
// describe_compression_lock_skip — mirrors Python ll.10-37
// ---------------------------------------------------------------------------

/// Mirrors `def describe_compression_lock_skip(lock_signal: Any) -> str:` (ll.10-37)
///
/// `lock_signal` is `agent._compression_skipped_due_to_lock` (or the `holder`
/// carried by the TUI's `CompressionLockHeld`): a descriptive holder string
/// when another compressor CONFIRMED holds the lock, or `True`/`None` when
/// acquisition failed without a confirmed holder (`hermes_state.try_acquire_compression_lock`
/// catches `sqlite3.Error` internally and returns `False`, so a failed acquire
/// is NOT proof that another compression is running). The two cases must be
/// worded differently: claiming "already in progress" on an unconfirmed failure
/// misdirects the user when the real problem is a broken lock subsystem.
pub fn describe_compression_lock_skip(lock_signal: Option<&str>) -> String {
    // Mirrors `holder = lock_signal if isinstance(lock_signal, str) and lock_signal.strip() else None` (ll.23-27)
    let holder: Option<String> = match lock_signal {
        Some(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    };
    // Mirrors `if holder: return f"⏳ Compression already in progress ... (holder: {holder})..."` (ll.28-32)
    if let Some(h) = holder {
        return format!(
            "⏳ Compression already in progress for this session (holder: {}). Please wait for it to finish.",
            h
        );
    }
    // Mirrors `return "⏳ Compression skipped: could not acquire ..."` (ll.33-37)
    "⏳ Compression skipped: could not acquire this session's compression lock. Another compression may still be running, or the lock check failed — try again shortly.".to_string()
}

#[allow(dead_code)]
fn _describe_compression_lock_skip(lock_signal: Option<&str>) -> String {
    describe_compression_lock_skip(lock_signal)
}

/// Value overload — mirrors `lock_signal: Any` where `Any` may be `str | True | None | holder str`.
///
/// Accepts a `serde_json::Value` so callers carrying the Python `Any` (e.g. `True` bool,
/// `Null` for `None`, or `String` holder) can call without pre-unwrapping.
/// Mirrors the `isinstance(lock_signal, str) and lock_signal.strip()` guard exactly:
/// only `Value::String` with non-whitespace content is treated as a holder.
pub fn describe_compression_lock_skip_value(lock_signal: &Value) -> String {
    // Mirrors `holder = lock_signal if isinstance(lock_signal, str) and lock_signal.strip() else None`
    let holder: Option<String> = match lock_signal {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    };
    if let Some(h) = holder {
        return format!(
            "⏳ Compression already in progress for this session (holder: {}). Please wait for it to finish.",
            h
        );
    }
    "⏳ Compression skipped: could not acquire this session's compression lock. Another compression may still be running, or the lock check failed — try again shortly.".to_string()
}

#[allow(dead_code)]
fn _describe_compression_lock_skip_value(lock_signal: &Value) -> String {
    describe_compression_lock_skip_value(lock_signal)
}

// ---------------------------------------------------------------------------
// CompressionState — mirrors `compression_state: Any` attrs (ll.46, 52-69, 108-111)
// ---------------------------------------------------------------------------

/// Mirrors the `compression_state` object's compression outcome flags read at
/// ll.52-69 and 108-111 via `getattr(compression_state, "_last_…", …)`.
///
/// Python uses duck-typed `Any` with `getattr(..., False)` and strict
/// `is True` identity checks. Rust models the same fields as typed bools so
/// only a literal `true` counts — mirrors `is True`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompressionState {
    /// Mirrors `getattr(compression_state, "_last_compress_aborted", False) is True` (ll.52-55)
    pub last_compress_aborted: bool,
    /// Mirrors `getattr(compression_state, "_last_compress_refused_would_grow", False) is True` (ll.56-60)
    pub last_compress_refused_would_grow: bool,
    /// Mirrors `getattr(compression_state, "_last_summary_fallback_used", False) is True` (ll.61-64)
    pub last_summary_fallback_used: bool,
    /// Mirrors `getattr(compression_state, "_last_summary_error", None)` then `isinstance(..., str)` guard (ll.65-71)
    pub last_summary_error: Option<String>,
    /// Mirrors `getattr(compression_state, "_last_summary_dropped_count", None)` with int/bool guard (ll.108-111)
    pub last_summary_dropped_count: Option<i64>,
}

impl CompressionState {
    /// Build from a `serde_json::Value` object mirroring Python's `getattr` fallback shape.
    /// `Value::Null` / missing keys map to `None`/`false` exactly as Python's default args do.
    pub fn from_value(v: &Value) -> Self {
        let obj = match v.as_object() {
            Some(o) => o,
            None => return Self::default(),
        };
        // Strict `is True` — only `Value::Bool(true)` counts, not truthy strings/ints.
        let aborted = matches!(obj.get("_last_compress_aborted"), Some(Value::Bool(true)));
        let refused = matches!(
            obj.get("_last_compress_refused_would_grow"),
            Some(Value::Bool(true))
        );
        let fallback = matches!(
            obj.get("_last_summary_fallback_used"),
            Some(Value::Bool(true))
        );
        // `isinstance(failure_reason, str) and strip` — keep only non-empty trimmed strings.
        let error: Option<String> = match obj.get("_last_summary_error") {
            Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
            _ => None,
        };
        // `isinstance(dropped_count, int) and not isinstance(dropped_count, bool)` — in JSON,
        // `Bool` is distinct from `Number`, so any `Value::Number` that parses as i64 counts.
        // Bool/complex types map to None and trigger the fallback `max(before - after, 0)`.
        let dropped: Option<i64> = match obj.get("_last_summary_dropped_count") {
            Some(Value::Number(n)) => n.as_i64(),
            // Also handle float numbers that are integer-ish? Python `isinstance(True, int)` is True
            // but excluded; JSON has no int/float distinction beyond Number. Keep as i64 only.
            _ => None,
        };
        Self {
            last_compress_aborted: aborted,
            last_compress_refused_would_grow: refused,
            last_summary_fallback_used: fallback,
            last_summary_error: error,
            last_summary_dropped_count: dropped,
        }
    }

    pub fn to_value(&self) -> Value {
        json!({
            "_last_compress_aborted": self.last_compress_aborted,
            "_last_compress_refused_would_grow": self.last_compress_refused_would_grow,
            "_last_summary_fallback_used": self.last_summary_fallback_used,
            "_last_summary_error": self.last_summary_error,
            "_last_summary_dropped_count": self.last_summary_dropped_count,
        })
    }
}

// ---------------------------------------------------------------------------
// summarize_manual_compression — mirrors Python ll.40-138
// ---------------------------------------------------------------------------

/// Mirrors the return dict of `summarize_manual_compression` (ll.130-138).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualCompressionFeedback {
    pub noop: bool,
    pub aborted: bool,
    pub refused_would_grow: bool,
    pub fallback_used: bool,
    pub headline: String,
    pub token_line: String,
    pub note: Option<String>,
}

impl ManualCompressionFeedback {
    pub fn to_value(&self) -> Value {
        json!({
            "noop": self.noop,
            "aborted": self.aborted,
            "refused_would_grow": self.refused_would_grow,
            "fallback_used": self.fallback_used,
            "headline": self.headline,
            "token_line": self.token_line,
            "note": self.note,
        })
    }

    pub fn from_value(v: &Value) -> Self {
        Self {
            noop: v.get("noop").and_then(|x| x.as_bool()).unwrap_or(false),
            aborted: v.get("aborted").and_then(|x| x.as_bool()).unwrap_or(false),
            refused_would_grow: v
                .get("refused_would_grow")
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
            fallback_used: v.get("fallback_used").and_then(|x| x.as_bool()).unwrap_or(false),
            headline: v.get("headline").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            token_line: v.get("token_line").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            note: v.get("note").and_then(|x| x.as_str()).map(|s| s.to_string()),
        }
    }
}

/// Mirrors `def summarize_manual_compression(before_messages: Sequence[dict[str, Any]], after_messages: Sequence[dict[str, Any]], before_tokens: int, after_tokens: int, *, compression_state: Any = None) -> dict[str, Any]:` (ll.40-138)
///
/// Return consistent user-facing feedback for manual compression.
pub fn summarize_manual_compression(
    before_messages: &[Value],
    after_messages: &[Value],
    before_tokens: i64,
    after_tokens: i64,
    compression_state: Option<&CompressionState>,
) -> ManualCompressionFeedback {
    // Mirrors `before_count = len(before_messages)` (l.49)
    let before_count = before_messages.len() as i64;
    // Mirrors `after_count = len(after_messages)` (l.50)
    let after_count = after_messages.len() as i64;
    // Mirrors `noop = list(after_messages) == list(before_messages)` (l.51)
    // Python compares list equality (deep). Rust slices implement PartialEq for Value.
    let noop = after_messages == before_messages;

    // Mirrors `aborted = compression_state is not None and getattr(..., "_last_compress_aborted", False) is True` (ll.52-55)
    let aborted = compression_state
        .map(|s| s.last_compress_aborted)
        .unwrap_or(false);
    // Mirrors `refused_would_grow = ... "_last_compress_refused_would_grow" ... is True` (ll.56-60)
    let refused_would_grow = compression_state
        .map(|s| s.last_compress_refused_would_grow)
        .unwrap_or(false);
    // Mirrors `fallback_used = ... "_last_summary_fallback_used" ... is True` (ll.61-64)
    let fallback_used = compression_state
        .map(|s| s.last_summary_fallback_used)
        .unwrap_or(false);
    // Mirrors `failure_reason = getattr(compression_state, "_last_summary_error", None) if compression_state is not None else None` (ll.65-69)
    // plus `if not isinstance(failure_reason, str) or not failure_reason.strip(): failure_reason = None` (ll.70-71)
    let mut failure_reason: Option<String> = match compression_state.and_then(|s| s.last_summary_error.clone()) {
        Some(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    };

    // Mirrors headline branching (ll.73-87)
    let headline = if refused_would_grow {
        // `f"Compression refused (summary would grow the conversation): {before_count} messages preserved"`
        format!(
            "Compression refused (summary would grow the conversation): {} messages preserved",
            before_count
        )
    } else if aborted {
        // `f"Compression aborted: {before_count} messages preserved"`
        format!("Compression aborted: {} messages preserved", before_count)
    } else if fallback_used {
        // `f"Compressed with fallback: {before_count} → {after_count} messages"`
        format!("Compressed with fallback: {} → {} messages", before_count, after_count)
    } else if noop {
        // `f"No changes from compression: {before_count} messages"`
        format!("No changes from compression: {} messages", before_count)
    } else {
        // `f"Compressed: {before_count} → {after_count} messages"`
        format!("Compressed: {} → {} messages", before_count, after_count)
    };

    // Mirrors token_line branching (ll.89-97)
    let token_line = if noop && after_tokens == before_tokens {
        // `f"Approx request size: ~{before_tokens:,} tokens (unchanged)"`
        format!("Approx request size: ~{} tokens (unchanged)", format_comma(before_tokens))
    } else if refused_would_grow {
        format!("Approx request size: ~{} tokens (unchanged)", format_comma(before_tokens))
    } else {
        // `f"Approx request size: ~{before_tokens:,} → ~{after_tokens:,} tokens"`
        format!(
            "Approx request size: ~{} → ~{} tokens",
            format_comma(before_tokens),
            format_comma(after_tokens)
        )
    };

    // Mirrors note branching (ll.99-121)
    let mut note: Option<String> = None;
    if refused_would_grow {
        // `note = "The generated summary was larger than what it would replace; no messages were removed."`
        note = Some(
            "The generated summary was larger than what it would replace; no messages were removed.".to_string(),
        );
    } else if aborted {
        // `note = "Summary generation failed; no messages were removed."`
        note = Some("Summary generation failed; no messages were removed.".to_string());
    } else if fallback_used {
        // `dropped_count = getattr(compression_state, "_last_summary_dropped_count", None)` (ll.108-110)
        // `if not isinstance(dropped_count, int) or isinstance(dropped_count, bool): dropped_count = max(before_count - after_count, 0)` (ll.111-112)
        let dropped_count: i64 = match compression_state.and_then(|s| s.last_summary_dropped_count) {
            Some(v) => v,
            None => (before_count - after_count).max(0),
        };
        // `note = "Summary generation failed; Hermes used limited fallback context and removed {dropped_count} message(s)."`
        note = Some(format!(
            "Summary generation failed; Hermes used limited fallback context and removed {} message(s).",
            dropped_count
        ));
    } else if !noop && after_count < before_count && after_tokens > before_tokens {
        // `note = "Note: fewer messages can still raise this estimate when compression rewrites the transcript into denser summaries."`
        note = Some(
            "Note: fewer messages can still raise this estimate when compression rewrites the transcript into denser summaries.".to_string(),
        );
    }

    // Mirrors `if failure_reason and (aborted or fallback_used):` (l.123)
    // `safe_reason = redact_sensitive_text(failure_reason.strip(), force=True)` (l.127)
    // `note = f"{note} Reason: {safe_reason}"` (l.128)
    if failure_reason.is_some() && (aborted || fallback_used) {
        let reason = failure_reason.take().unwrap();
        // This text crosses a user-facing UI boundary. Never let a disabled global
        // redaction preference expose credentials embedded in provider exception text.
        let safe_reason = redact_sensitive_text(reason.trim(), true);
        // Python does `f"{note} Reason: {safe_reason}"` where `note` is str (always Some in this branch).
        // Rust: if note is Some, append; if None (defensive), produce "Reason: ..." without leading "None".
        note = Some(match note {
            Some(n) => format!("{} Reason: {}", n, safe_reason),
            None => format!("Reason: {}", safe_reason),
        });
    }

    // Mirrors `return {"noop": ..., "aborted": ..., ...}` (ll.130-138)
    ManualCompressionFeedback {
        noop,
        aborted,
        refused_would_grow,
        fallback_used,
        headline,
        token_line,
        note,
    }
}

#[allow(dead_code)]
fn _summarize_manual_compression(
    before_messages: &[Value],
    after_messages: &[Value],
    before_tokens: i64,
    after_tokens: i64,
    compression_state: Option<&CompressionState>,
) -> ManualCompressionFeedback {
    summarize_manual_compression(
        before_messages,
        after_messages,
        before_tokens,
        after_tokens,
        compression_state,
    )
}

/// Value overload — mirrors `compression_state: Any` as `serde_json::Value` and return as `Value` dict.
///
/// Accepts `before_messages`/`after_messages` as `&[Value]` and `compression_state` as
/// `Option<&Value>` (where Value is an object with `_last_*` keys, mirroring
/// Python's duck-typed `Any`). Returns a `Value::Object` with the same seven keys
/// as `summarize_manual_compression`'s `ManualCompressionFeedback::to_value()`.
pub fn summarize_manual_compression_value(
    before_messages: &[Value],
    after_messages: &[Value],
    before_tokens: i64,
    after_tokens: i64,
    compression_state: Option<&Value>,
) -> Value {
    let typed: Option<CompressionState> = compression_state.map(CompressionState::from_value);
    let fb = summarize_manual_compression(
        before_messages,
        after_messages,
        before_tokens,
        after_tokens,
        typed.as_ref(),
    );
    fb.to_value()
}

#[allow(dead_code)]
fn _summarize_manual_compression_value(
    before_messages: &[Value],
    after_messages: &[Value],
    before_tokens: i64,
    after_tokens: i64,
    compression_state: Option<&Value>,
) -> Value {
    summarize_manual_compression_value(
        before_messages,
        after_messages,
        before_tokens,
        after_tokens,
        compression_state,
    )
}
