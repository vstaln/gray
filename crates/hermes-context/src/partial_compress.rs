//! Boundary-aware partial compression — "summarize up to here".
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/partial_compress.py` (324 LOC).
//! T0019 — full file (lines 1-324).
//!
//! ```text
//! Boundary-aware partial compression — "summarize up to here".
//!
//! Inspired by Claude Code's Rewind menu "Summarize up to here" action
//! (v2.1.139–v2.1.142, Week 20, May 2026):
//! https://code.claude.com/docs/en/whats-new/2026-w20
//!
//! Hermes already has ``/compress`` (full-history compaction) and an
//! automatic token-budget tail-protection heuristic inside
//! ``ContextCompressor``. What was missing is *user-chosen* boundary
//! control: "fold everything before this point into a summary, but keep
//! my most recent N exchanges exactly as they are."
//! ```
//!
//! NEVER cargo — 1:1 translation only (no `cargo check` / `cargo build`).
//! Mirrors Python ll.1-324 verbatim; line numbers in comments refer to the
//! 324-line source file. Verified by line-level audit, not by compilation.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

// ---------------------------------------------------------------------------
// Imports — mirrors Python ll.42-44
// ---------------------------------------------------------------------------
use std::collections::HashMap;

use serde_json::{json, Value};

// Python imports (ll.42-44) — stdlib:
//   from typing import Any, Dict, List, Optional, Tuple
// Mapped: serde_json::Value for Any/Dict, Vec<Value> for List[Dict], Option,
//   tuples as Rust tuples. No external runtime imports; pure logic only.
//
// Python intra-repo imports: none at top-level (pure functions). The module
// doc (ll.1-40) references `cli.py::_manual_compress` and
// `gateway/run.py::_handle_compress_command` as callers, not imports.

// ---------------------------------------------------------------------------
// Logger — mirrors implicit module logger (no explicit logger in py)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "partial_compress";

// ---------------------------------------------------------------------------
// Message type — mirrors Python `Dict[str, Any]` / `List[Dict[str, Any]]`
// ---------------------------------------------------------------------------
/// Mirrors `Dict[str, Any]` message shape (ll.44, 145+).
/// Python messages are `{"role": "...", "content": ..., ...}`.
/// Rust: `serde_json::Value::Object` preserves the open-dict shape.
pub type Message = Value;
/// Mirrors `List[Dict[str, Any]]`
pub type History = Vec<Message>;

// ---------------------------------------------------------------------------
// Constants — mirrors Python ll.46-52
// ---------------------------------------------------------------------------

/// Mirrors `DEFAULT_KEEP_LAST = 2` (ll.48)
pub const DEFAULT_KEEP_LAST: i64 = 2;
#[allow(dead_code)]
const _DEFAULT_KEEP_LAST: i64 = DEFAULT_KEEP_LAST;

/// Mirrors `MAX_KEEP_LAST = 100` (ll.52)
pub const MAX_KEEP_LAST: i64 = 100;
#[allow(dead_code)]
const _MAX_KEEP_LAST: i64 = MAX_KEEP_LAST;

// ---------------------------------------------------------------------------
// _coerce_keep — mirrors Python ll.200-210
// ---------------------------------------------------------------------------

/// Mirrors `def _coerce_keep(value: str) -> int:` (ll.200-210)
///
/// Parse a keep-count token, clamping to [1, MAX_KEEP_LAST].
pub fn coerce_keep(value: &str) -> i64 {
    // Mirrors `try: n = int(value) except (TypeError, ValueError): return DEFAULT_KEEP_LAST` (ll.202-205)
    let n: i64 = match value.trim().parse() {
        Ok(v) => v,
        Err(_) => return DEFAULT_KEEP_LAST,
    };
    // Mirrors `if n < 1: return 1` (ll.206-207)
    if n < 1 {
        return 1;
    }
    // Mirrors `if n > MAX_KEEP_LAST: return MAX_KEEP_LAST` (ll.208-209)
    if n > MAX_KEEP_LAST {
        return MAX_KEEP_LAST;
    }
    // Mirrors `return n` (l.210)
    n
}

#[allow(dead_code)]
fn _coerce_keep(value: &str) -> i64 {
    coerce_keep(value)
}

// Legacy underscore alias for 1:1 grep traceability (Python private name)
#[allow(dead_code)]
pub fn _coerce_keep_py(value: &str) -> i64 {
    coerce_keep(value)
}

// ---------------------------------------------------------------------------
// parse_partial_compress_args — mirrors Python ll.55-108
// ---------------------------------------------------------------------------

/// Mirrors `def parse_partial_compress_args(raw_args: str) -> Tuple[bool, int, Optional[str]]:` (ll.55-108)
///
/// Recognizes the boundary-aware forms:
/// * `here`            → partial compress, keep `DEFAULT_KEEP_LAST`
/// * `here 4`          → partial compress, keep 4 exchanges
/// * `--keep 4`        → partial compress, keep 4 exchanges
/// * `up to here`      → alias for `here` (matches Claude Code's menu label)
///
/// Anything else is treated as a focus topic for the existing full
/// `/compress <focus>` behavior.
///
/// Returns `(partial, keep_last, focus_topic)`:
/// * `partial` — True when a boundary-aware form was requested.
/// * `keep_last` — exchanges to preserve verbatim (only meaningful when `partial` is True).
/// * `focus_topic` — focus string for full compression, or None.
pub fn parse_partial_compress_args(raw_args: &str) -> (bool, i64, Option<String>) {
    // Mirrors `text = (raw_args or "").strip()` (l.81)
    let text = raw_args.trim().to_string();
    // Mirrors `if not text: return False, DEFAULT_KEEP_LAST, None` (ll.82-83)
    if text.is_empty() {
        return (false, DEFAULT_KEEP_LAST, None);
    }

    // Mirrors `lowered = text.lower()` (l.85)
    let mut lowered = text.to_lowercase();
    let mut text_mut = text.clone();

    // Mirrors `if lowered.startswith("up to here"): lowered = lowered[len("up to "):]; text = text[len("up to "):]` (ll.88-90)
    // Python uses `len("up to ") == 6` as slice offset, preserving case of `text` remainder.
    if lowered.starts_with("up to here") {
        // "up to " is 6 bytes (ASCII) — slice at byte 6 to mirror Python str slicing on ASCII.
        let offset = "up to ".len();
        if lowered.len() >= offset {
            lowered = lowered[offset..].to_string();
        }
        if text_mut.len() >= offset {
            text_mut = text_mut[offset..].to_string();
        }
    }

    // Mirrors `tokens = lowered.split()` (l.92)
    let tokens: Vec<String> = lowered.split_whitespace().map(|s| s.to_string()).collect();

    // Mirrors `if tokens and tokens[0] == "here": keep = DEFAULT_KEEP_LAST; if len(tokens) >=2: keep = _coerce_keep(tokens[1]); return True, keep, None` (ll.95-99)
    if !tokens.is_empty() && tokens[0] == "here" {
        let mut keep = DEFAULT_KEEP_LAST;
        if tokens.len() >= 2 {
            keep = coerce_keep(&tokens[1]);
        }
        return (true, keep, None);
    }

    // Mirrors `if tokens and tokens[0] in ("--keep", "-k") and len(tokens) >= 2: return True, _coerce_keep(tokens[1]), None` (ll.102-103)
    if !tokens.is_empty() && (tokens[0] == "--keep" || tokens[0] == "-k") && tokens.len() >= 2 {
        return (true, coerce_keep(&tokens[1]), None);
    }

    // Mirrors `if tokens and tokens[0].startswith("--keep="): return True, _coerce_keep(tokens[0].split("=",1)[1]), None` (ll.104-105)
    if !tokens.is_empty() && tokens[0].starts_with("--keep=") {
        // Split on first '=' — mirrors `tokens[0].split("=", 1)[1]`
        if let Some(eq_idx) = tokens[0].find('=') {
            let val = &tokens[0][eq_idx + 1..];
            return (true, coerce_keep(val), None);
        }
    }

    // Mirrors `return False, DEFAULT_KEEP_LAST, text or None` (l.108)
    // `text or None` means None if empty else Some(text). text_mut carries stripped "up to " alias.
    let focus = if text_mut.trim().is_empty() {
        None
    } else {
        // Python returns `text or None` where `text` is the stripped remainder after alias handling.
        // But note: `text` here is `text_mut` after possible alias strip, trimmed? Original `text`
        // was already stripped. After alias strip, we did not re-strip, but Python's slice preserves
        // remainder as-is (e.g. "here" without re-strip). We replicate by returning text_mut
        // trimmed? Python's `text or None` would return text_mut even if it has leading space?
        // However `text_mut` after slice on stripped `text` cannot have leading space before "here"
        // — slice removed exactly "up to ". So we mirror by returning text_mut if non-empty.
        // For non-alias case, text_mut == original stripped text, so we return that.
        Some(text_mut)
    };
    // Handle `text or None` for empty remainder — but remainder after alias was checked above
    // as `if not text` already. So for non-alias empty can't reach here. Keep parity:
    let focus = match focus {
        Some(s) if s.is_empty() => None,
        other => other,
    };
    (false, DEFAULT_KEEP_LAST, focus)
}

#[allow(dead_code)]
fn _parse_partial_compress_args(raw_args: &str) -> (bool, i64, Option<String>) {
    parse_partial_compress_args(raw_args)
}

/// Option-aware overload — mirrors `raw_args or ""` where None maps to "".
pub fn parse_partial_compress_args_opt(raw_args: Option<&str>) -> (bool, i64, Option<String>) {
    parse_partial_compress_args(raw_args.unwrap_or(""))
}

// ---------------------------------------------------------------------------
// extract_compress_flags — mirrors Python ll.111-142
// ---------------------------------------------------------------------------

/// Mirrors `def extract_compress_flags(raw_args: str) -> Tuple[str, bool, bool]:` (ll.111-142)
///
/// Strip `--preview`/`--dry-run`/`--aggressive` flags from the argument string
/// after `/compress` (or its `/compact` alias).
///
/// Returns `(remaining_args, preview, aggressive_requested)`.
pub fn extract_compress_flags(raw_args: &str) -> (String, bool, bool) {
    // Mirrors `preview = False; aggressive = False; kept: List[str] = []` (ll.131-133)
    let mut preview = false;
    let mut aggressive = false;
    let mut kept: Vec<String> = Vec::new();

    // Mirrors `for tok in (raw_args or "").split():` (l.134)
    for tok in raw_args.split_whitespace() {
        // Mirrors `low = tok.lower()` (l.135)
        let low = tok.to_lowercase();
        // Mirrors `if low in ("--preview", "--dry-run", "--dryrun"): preview = True` (ll.136-137)
        if low == "--preview" || low == "--dry-run" || low == "--dryrun" {
            preview = true;
        // Mirrors `elif low == "--aggressive": aggressive = True` (ll.138-139)
        } else if low == "--aggressive" {
            aggressive = true;
        // Mirrors `else: kept.append(tok)` (ll.140-141) — keep original case token
        } else {
            kept.push(tok.to_string());
        }
    }

    // Mirrors `return " ".join(kept), preview, aggressive` (l.142)
    (kept.join(" "), preview, aggressive)
}

#[allow(dead_code)]
fn _extract_compress_flags(raw_args: &str) -> (String, bool, bool) {
    extract_compress_flags(raw_args)
}

pub fn extract_compress_flags_opt(raw_args: Option<&str>) -> (String, bool, bool) {
    extract_compress_flags(raw_args.unwrap_or(""))
}

// ---------------------------------------------------------------------------
// split_history_for_partial_compress — mirrors Python ll.213-266
// ---------------------------------------------------------------------------

/// Mirrors `def split_history_for_partial_compress(history: List[Dict[str, Any]], keep_last: int) -> Tuple[List[Dict[str, Any]], List[Dict[str, Any]]]:` (ll.213-266)
///
/// Split `history` into `(head, tail)` for partial compression.
///
/// An *exchange* is counted by `user`-role messages: keeping N exchanges means
/// keeping everything from the Nth-most-recent `user` message onward.
pub fn split_history_for_partial_compress(history: &[Value], keep_last: i64) -> (Vec<Value>, Vec<Value>) {
    // Mirrors `if keep_last < 1: keep_last = 1` (ll.234-235)
    let mut keep = keep_last;
    if keep < 1 {
        keep = 1;
    }

    // Mirrors `n = len(history); if n == 0: return [], []` (ll.237-239)
    let n = history.len();
    if n == 0 {
        return (Vec::new(), Vec::new());
    }

    // Mirrors `user_starts: List[int] = []; for idx in range(n-1, -1, -1): if history[idx].get("role") == "user": user_starts.append(idx); if len(user_starts) >= keep_last: break` (ll.243-248)
    let mut user_starts: Vec<usize> = Vec::new();
    for idx in (0..n).rev() {
        // Mirrors `history[idx].get("role") == "user"` — Value::Object access
        let is_user = history[idx]
            .get("role")
            .and_then(|v| v.as_str())
            == Some("user");
        if is_user {
            user_starts.push(idx);
            if user_starts.len() as i64 >= keep {
                break;
            }
        }
    }

    // Mirrors `if not user_starts: return list(history), []` (ll.250-253)
    if user_starts.is_empty() {
        return (history.to_vec(), Vec::new());
    }

    // Mirrors `boundary = user_starts[-1]` (l.255) — earliest of the kept user starts
    let boundary = *user_starts.last().unwrap();

    // Mirrors `head = history[:boundary]; tail = history[boundary:]` (ll.257-258)
    let head = history[..boundary].to_vec();
    let tail = history[boundary..].to_vec();

    // Mirrors `if not head: return list(history), []` (ll.263-264)
    if head.is_empty() {
        return (history.to_vec(), Vec::new());
    }

    // Mirrors `return head, tail` (l.266)
    (head, tail)
}

#[allow(dead_code)]
fn _split_history_for_partial_compress(history: &[Value], keep_last: i64) -> (Vec<Value>, Vec<Value>) {
    split_history_for_partial_compress(history, keep_last)
}

// ---------------------------------------------------------------------------
// rejoin_compressed_head_and_tail — mirrors Python ll.269-324
// ---------------------------------------------------------------------------

/// Mirrors `def rejoin_compressed_head_and_tail(compressed_head: List[Dict[str, Any]], tail: List[Dict[str, Any]]) -> List[Dict[str, Any]]:` (ll.269-324)
///
/// Concatenate a compressed head with the verbatim tail, defending the seam
/// against an illegal user→user / assistant→assistant adjacency.
pub fn rejoin_compressed_head_and_tail(compressed_head: &[Value], tail: &[Value]) -> Vec<Value> {
    // Mirrors `if not tail: return list(compressed_head)` (ll.291-292)
    if tail.is_empty() {
        return compressed_head.to_vec();
    }
    // Mirrors `if not compressed_head: return list(tail)` (ll.293-294)
    if compressed_head.is_empty() {
        return tail.to_vec();
    }

    // Mirrors `head = list(compressed_head); rest = list(tail)` (ll.296-297)
    let mut head = compressed_head.to_vec();
    let mut rest = tail.to_vec();

    // Mirrors `last = head[-1]; first = rest[0]; last_role = last.get("role"); first_role = first.get("role")` (ll.299-302)
    let last = head.last().cloned().unwrap();
    let first = rest.first().cloned().unwrap();
    let last_role = last.get("role").and_then(|v| v.as_str());
    let first_role = first.get("role").and_then(|v| v.as_str());

    // Mirrors `if last_role == first_role and last_role in ("user", "assistant"):` (l.304)
    if last_role == first_role && matches!(last_role, Some("user") | Some("assistant")) {
        // Mirrors `last_content = last.get("content"); first_content = first.get("content")` (ll.310-311)
        let last_content = last.get("content").cloned();
        let first_content = first.get("content").cloned();

        // Mirrors `if isinstance(last_content, str) and isinstance(first_content, str):` (l.312)
        let both_strings = matches!(&last_content, Some(Value::String(_)))
            && matches!(&first_content, Some(Value::String(_)));

        if both_strings {
            // Mirrors `merged = dict(last); merged["content"] = f"{last_content}\n\n{first_content}"; head[-1] = merged; rest = rest[1:]` (ll.313-316)
            if let (Some(Value::String(ls)), Some(Value::String(fs))) = (last_content, first_content) {
                // `merged` is a clone of last's object with content replaced
                if let Some(obj) = head.last_mut().and_then(|v| v.as_object_mut()) {
                    obj.insert("content".to_string(), Value::String(format!("{}\n\n{}", ls, fs)));
                } else {
                    // Fallback if last wasn't an object (should not happen in normal history)
                    // Recreate as object with merged content — preserve role seam minimally
                    let mut merged = last.clone();
                    if let Some(mobj) = merged.as_object_mut() {
                        mobj.insert("content".to_string(), Value::String(format!("{}\n\n{}", ls, fs)));
                    }
                    if let Some(last_mut) = head.last_mut() {
                        *last_mut = merged;
                    }
                }
                // Mirrors `rest = rest[1:]` — drop first tail element after merge
                rest = rest.into_iter().skip(1).collect();
            }
        } else {
            // Mirrors `else: # Can't safely string-merge multimodal content. Insert a minimal bridging turn ...` (ll.317-322)
            // `bridge_role = "assistant" if first_role == "user" else "user"`
            let bridge_role = if first_role == Some("user") {
                "assistant"
            } else {
                "user"
            };
            // Mirrors `head.append({"role": bridge_role, "content": ""})`
            head.push(json!({"role": bridge_role, "content": ""}));
        }
    }

    // Mirrors `return head + rest` (l.324)
    let mut out = head;
    out.extend(rest);
    out
}

#[allow(dead_code)]
fn _rejoin_compressed_head_and_tail(compressed_head: &[Value], tail: &[Value]) -> Vec<Value> {
    rejoin_compressed_head_and_tail(compressed_head, tail)
}

// ---------------------------------------------------------------------------
// summarize_compress_preview — mirrors Python ll.145-197
// ---------------------------------------------------------------------------

/// Mirrors return dict of `summarize_compress_preview` (ll.191-197).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressPreview {
    pub head_count: usize,
    pub tail_count: usize,
    pub total: usize,
    pub partial: bool,
    pub lines: Vec<String>,
}

impl CompressPreview {
    pub fn to_value(&self) -> Value {
        json!({
            "head_count": self.head_count,
            "tail_count": self.tail_count,
            "total": self.total,
            "partial": self.partial,
            "lines": self.lines,
        })
    }

    pub fn from_value(v: &Value) -> Self {
        Self {
            head_count: v.get("head_count").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
            tail_count: v.get("tail_count").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
            total: v.get("total").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
            partial: v.get("partial").and_then(|x| x.as_bool()).unwrap_or(false),
            lines: v
                .get("lines")
                .and_then(|x| x.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|e| e.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

fn format_comma(n: usize) -> String {
    let s = n.to_string();
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
    out.chars().rev().collect()
}

fn format_comma_i64(n: i64) -> String {
    if n < 0 {
        format!("-{}", format_comma((-n) as usize))
    } else {
        format_comma(n as usize)
    }
}

/// Mirrors `def summarize_compress_preview(history: List[Dict[str, Any]], partial: bool, keep_last: int, focus_topic: Optional[str], approx_tokens: int) -> Dict[str, Any]:` (ll.145-197)
///
/// Build the `/compress --preview` report — pure, no side effects.
pub fn summarize_compress_preview(
    history: &[Value],
    partial: bool,
    keep_last: i64,
    focus_topic: Option<&str>,
    approx_tokens: i64,
) -> CompressPreview {
    // Mirrors `total = len(history); head = list(history); tail: List[Dict[str, Any]] = []; effective_partial = partial` (ll.161-164)
    let total = history.len();
    let mut head = history.to_vec();
    let mut tail: Vec<Value> = Vec::new();
    let mut effective_partial = partial;

    // Mirrors `if partial: head, tail = split_history_for_partial_compress(history, keep_last); if not tail: effective_partial = False; head, tail = list(history), []` (ll.165-170)
    if partial {
        let (h, t) = split_history_for_partial_compress(history, keep_last);
        head = h;
        tail = t;
        if tail.is_empty() {
            // Same degenerate-split fallback the real run applies.
            effective_partial = false;
            head = history.to_vec();
            tail = Vec::new();
        }
    }

    // Mirrors `lines = ["Preview — no changes made.", f"Would compress {len(head)} of {total} message(s) (~{approx_tokens:,} tokens currently in context).",]` (ll.172-176)
    let mut lines: Vec<String> = Vec::new();
    lines.push("Preview — no changes made.".to_string());
    lines.push(format!(
        "Would compress {} of {} message(s) (~{} tokens currently in context).",
        head.len(),
        total,
        format_comma_i64(approx_tokens)
    ));

    // Mirrors `if effective_partial: lines.append(f"Boundary: keeping the last {keep_last} exchange(s) ({len(tail)} message(s)) verbatim.")` (ll.177-181)
    if effective_partial {
        lines.push(format!(
            "Boundary: keeping the last {} exchange(s) ({} message(s)) verbatim.",
            keep_last,
            tail.len()
        ));
    // Mirrors `elif partial: lines.append("Boundary: 'here' split would keep everything — falling back to full compression.")` (ll.182-186)
    } else if partial {
        lines.push(
            "Boundary: 'here' split would keep everything — falling back to full compression."
                .to_string(),
        );
    }

    // Mirrors `if focus_topic: lines.append(f'Focus topic: "{focus_topic}"')` (ll.187-188)
    if let Some(topic) = focus_topic {
        if !topic.is_empty() {
            lines.push(format!(r#"Focus topic: "{}""#, topic));
        }
    }

    // Mirrors `lines.append("Run the command again without --preview to apply.")` (l.189)
    lines.push("Run the command again without --preview to apply.".to_string());

    // Mirrors `return {"head_count": len(head), "tail_count": len(tail), "total": total, "partial": effective_partial, "lines": lines}` (ll.191-197)
    CompressPreview {
        head_count: head.len(),
        tail_count: tail.len(),
        total,
        partial: effective_partial,
        lines,
    }
}

#[allow(dead_code)]
fn _summarize_compress_preview(
    history: &[Value],
    partial: bool,
    keep_last: i64,
    focus_topic: Option<&str>,
    approx_tokens: i64,
) -> CompressPreview {
    summarize_compress_preview(history, partial, keep_last, focus_topic, approx_tokens)
}

/// Value-wrapped overload — mirrors Python dict return for callers using `Value` history.
pub fn summarize_compress_preview_value(
    history: &[Value],
    partial: bool,
    keep_last: i64,
    focus_topic: Option<&str>,
    approx_tokens: i64,
) -> Value {
    summarize_compress_preview(history, partial, keep_last, focus_topic, approx_tokens).to_value()
}
