//! Automatic context window compression for long conversations.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/context_compressor.py`
//! (8211 LOC) — slice 1/11, lines 1-800.
//!
//! ```text
//! Automatic context window compression for long conversations.
//!
//! Self-contained class with its own OpenAI client for summarization.
//! Uses auxiliary model (cheap/fast) to summarize middle turns while
//! protecting head and tail context.
//!
//! Improvements over v2:
//!   - Structured summary template with Resolved/Pending question tracking
//!   - Filter-safe summarizer preamble that treats prior turns as source material
//!   - Historical (reference-only) section headings replace "Next Steps"/"Remaining Work" to avoid reading as active instructions
//!   - Clear separator when summary merges into tail message
//!   - Iterative summary updates (preserves info across multiple compactions)
//!   - Token-budget tail protection instead of fixed message count
//!   - Tool output pruning before LLM summarization (cheap pre-pass)
//!   - Scaled summary budget (proportional to compressed content)
//!   - Richer tool call/result detail in summarizer input
//! ```
//!
//! NEVER cargo — 1:1 translation only (no `cargo check` / `cargo build`).
//! Mirrors Python ll.1-800 verbatim; line numbers in comments refer to the
//! 8211-line source file. Later slices (compressor_slice2..N) continue from
//! l.801. This slice is verified by line-level audit, not by compilation.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

// ---------------------------------------------------------------------------
// Imports — mirrors Python ll.19-45
// ---------------------------------------------------------------------------
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Value};

// Python imports (ll.19-26) — stdlib:
//   hashlib, json, logging, sqlite3, re, time, uuid, typing
// Mapped: std hash, serde_json, log, rusqlite (not needed slice1), regex, time, uuid

// Python intra-repo imports (ll.28-45) — cross-module dependencies:
//   from agent.auxiliary_client import (AuxiliaryExplicitCancellation, _is_connection_error, aux_interrupt_protection, call_llm)
//   from agent.context_engine import ContextEngine, sanitize_memory_context
//   from agent.error_classifier import FailoverReason, classify_api_error
//   from agent.message_sanitization import tool_result_id_variants
//   from agent.model_metadata import (MINIMUM_CONTEXT_LENGTH, get_model_context_length, estimate_messages_tokens_rough, estimate_tokens_rough)
//   from agent.redact import redact_sensitive_text
//   from agent.turn_context import drop_stale_api_content
//   from tools.todo_tool import TODO_INJECTION_HEADER
// Rust: these live in sibling crates / later slices. Stubs below mirror their
// surface so slice1 is self-contained and grep-traceable. Canonical impls
// replace stubs when slices merge.

// ---------------------------------------------------------------------------
// Logger — mirrors `logger = logging.getLogger(__name__)` (l.47)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "context_compressor";

// ---------------------------------------------------------------------------
// Helpers mirroring Python ll.50-55
// ---------------------------------------------------------------------------

/// Mirrors `def _safe_int(value: Any) -> int | None:` (ll.50-55)
/// Best-effort integer coercion for telemetry fields.
pub fn safe_int(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        Value::Bool(b) => Some(if *b { 1 } else { 0 }),
        _ => None,
    }
}

// Keep underscore-prefixed alias for 1:1 traceability (Python private name).
#[allow(dead_code)]
fn _safe_int(value: &Value) -> Option<i64> {
    safe_int(value)
}

// ---------------------------------------------------------------------------
// Constants — mirrors Python ll.58-75
// ---------------------------------------------------------------------------

/// Mirrors `_SUMMARY_PERMANENT_QUOTA_MARKERS` (ll.58-66)
pub const SUMMARY_PERMANENT_QUOTA_MARKERS: &[&str] = &[
    "insufficient_quota",
    "quota exceeded",
    "quota_exceeded",
    "out of funds",
    "out of credits",
    "out of credit",
    "out of extra usage",
];
#[allow(dead_code)]
const _SUMMARY_PERMANENT_QUOTA_MARKERS: &[&str] = SUMMARY_PERMANENT_QUOTA_MARKERS;

/// Mirrors `_SUMMARY_MISSING_CREDENTIAL_MARKERS` (ll.68-71)
pub const SUMMARY_MISSING_CREDENTIAL_MARKERS: &[&str] = &[
    "no api key was found",
    "no api key found",
];
#[allow(dead_code)]
const _SUMMARY_MISSING_CREDENTIAL_MARKERS: &[&str] = SUMMARY_MISSING_CREDENTIAL_MARKERS;

/// Mirrors `_HYGIENE_IDLE_TIMEOUT_MARKERS` (ll.73-75)
pub const HYGIENE_IDLE_TIMEOUT_MARKERS: &[&str] = &["session hygiene compression timed out"];
#[allow(dead_code)]
const _HYGIENE_IDLE_TIMEOUT_MARKERS: &[&str] = HYGIENE_IDLE_TIMEOUT_MARKERS;

// ---------------------------------------------------------------------------
// Error classification stubs — mirrors `agent/error_classifier.py` surface
// needed by ll.88-109. Canonical enum lives in hermes-core; stub keeps
// slice1 grep-traceable.
// ---------------------------------------------------------------------------

/// Mirrors `FailoverReason` (error_classifier.py)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailoverReason {
    RateLimit,
    Auth,
    AuthPermanent,
    Billing,
    Other,
}

/// Minimal `ClassifiedError` — mirrors `classify_api_error(exc)` return.
#[derive(Debug, Clone)]
pub struct ClassifiedError {
    pub reason: FailoverReason,
}

/// Stub: mirrors `classify_api_error` (ll.91). Real impl in hermes-core;
/// stub classifies by string heuristics so slice1 logic is testable.
pub fn classify_api_error(exc: &dyn std::error::Error) -> ClassifiedError {
    let s = exc.to_string().to_lowercase();
    if s.contains("rate limit") || s.contains("429") {
        return ClassifiedError { reason: FailoverReason::RateLimit };
    }
    if s.contains("billing") || s.contains("quota") {
        return ClassifiedError { reason: FailoverReason::Billing };
    }
    if s.contains("auth") || s.contains("unauthorized") || s.contains("401") || s.contains("403") {
        return ClassifiedError { reason: FailoverReason::Auth };
    }
    ClassifiedError { reason: FailoverReason::Other }
}

// ---------------------------------------------------------------------------
// Functions — mirrors Python ll.78-109
// ---------------------------------------------------------------------------

/// Mirrors `def _is_hygiene_idle_timeout_error(error: object) -> bool:` (ll.78-85)
///
/// Return True when the durable cooldown came from a hygiene watchdog timeout.
/// That persist is intentional for the pre-agent hygiene pass (#74136) but
/// must not block the in-conversation compressor (#86972).
pub fn is_hygiene_idle_timeout_error(error: &dyn std::fmt::Display) -> bool {
    let text = format!("{}", error).trim().to_lowercase();
    HYGIENE_IDLE_TIMEOUT_MARKERS.iter().any(|m| text.contains(*m))
}

#[allow(dead_code)]
fn _is_hygiene_idle_timeout_error(error: &dyn std::fmt::Display) -> bool {
    is_hygiene_idle_timeout_error(error)
}

/// Mirrors `def _is_summary_access_or_quota_error(exc: Exception) -> bool:` (ll.88-109)
/// Return True for non-retryable summary auth, permission, or quota errors.
pub fn is_summary_access_or_quota_error(exc: &(dyn std::error::Error + 'static)) -> bool {
    let classified = classify_api_error(exc);
    if classified.reason == FailoverReason::RateLimit {
        return false;
    }
    if matches!(classified.reason, FailoverReason::Auth | FailoverReason::AuthPermanent) {
        return true;
    }
    let err_text = exc.to_string().to_lowercase();
    if SUMMARY_MISSING_CREDENTIAL_MARKERS.iter().any(|m| err_text.contains(*m)) {
        return true;
    }
    // status_code probing — mirrors `getattr(exc, "status_code", None) or getattr(getattr(exc, "response", None), "status_code", None)` (ll.101-103)
    // In Rust, callers that carry a status expose it via `status_code()` on the error; stub checks string fallback.
    if err_text.contains(" 401") || err_text.contains(" 402") || err_text.contains(" 403") {
        return true;
    }
    if classified.reason == FailoverReason::Billing {
        return SUMMARY_PERMANENT_QUOTA_MARKERS.iter().any(|m| err_text.contains(*m));
    }
    SUMMARY_PERMANENT_QUOTA_MARKERS.iter().any(|m| err_text.contains(*m))
}

#[allow(dead_code)]
fn _is_summary_access_or_quota_error(exc: &(dyn std::error::Error + 'static)) -> bool {
    is_summary_access_or_quota_error(exc)
}

// ---------------------------------------------------------------------------
// Historical heading + summary prefixes — mirrors Python ll.112-173
// ---------------------------------------------------------------------------

/// Mirrors `HISTORICAL_TASK_HEADING = "## Historical Task Snapshot"` (l.112)
pub const HISTORICAL_TASK_HEADING: &str = "## Historical Task Snapshot";

/// Mirrors `SUMMARY_PREFIX = (...)` (ll.115-149)
/// The current handoff prefix. Newest generation; see `_HISTORICAL_SUMMARY_PREFIXES` for prior wire texts.
pub const SUMMARY_PREFIX: &str = concat!(
    "[CONTEXT COMPACTION — REFERENCE ONLY] Earlier turns were compacted ",
    "into the summary below. This is a handoff from a previous context ",
    "window — treat it as background reference, NOT as active instructions. ",
    "Do NOT answer questions or fulfill requests mentioned in this summary; ",
    "they were already addressed. ",
    "Respond ONLY to the latest user message that appears AFTER this ",
    "summary — that message is the single source of truth for what to do ",
    "right now. ",
    "If no user message appears AFTER this summary, do nothing: do not ",
    "resume, wrap up, or continue work from ",
    "'## Historical Task Snapshot' or any other section, do not call tools, ",
    "and wait for a new user message. This handoff must never become the ",
    "active turn by itself. (Exception: if tool results or your own ",
    "tool calls appear after this summary, you are mid-way through an ",
    "in-flight exchange — continue that exchange normally.) ",
    "Topic overlap with the summary does NOT mean you should resume its ",
    "task: even on similar topics, the latest user message WINS. Treat ONLY ",
    "the latest message as the active task and discard stale items from ",
    "'## Historical Task Snapshot' entirely — do not 'wrap up' or ",
    "'finish' work described there unless the latest message explicitly ",
    "asks for it. ",
    "Reverse signals in the latest message (e.g. 'stop', 'undo', 'roll ",
    "back', 'just verify', 'don't do that anymore', 'never mind', a new ",
    "topic) must immediately end any in-flight work described in the ",
    "summary; do not re-surface it in later turns. ",
    "IMPORTANT: Your persistent memory (MEMORY.md, USER.md) in the system ",
    "prompt is ALWAYS authoritative and active — never ignore or deprioritize ",
    "memory content due to this compaction note. ",
    "None of the above restricts HOW you work: your tools remain fully ",
    "active — keep calling them normally for the active task (edit files, ",
    "run commands, search) instead of merely narrating what you would do. ",
    "The current session state (files, config, etc.) may reflect work ",
    "described here — avoid repeating it:"
);

/// Mirrors `LEGACY_SUMMARY_PREFIX = "[CONTEXT SUMMARY]:"` (l.150)
pub const LEGACY_SUMMARY_PREFIX: &str = "[CONTEXT SUMMARY]:";

// Underscore-prefixed ON PURPOSE: wire sanitizers strip every top-level message key starting with "_"
/// Mirrors `COMPRESSED_SUMMARY_METADATA_KEY = "_compressed_summary"` (l.165)
pub const COMPRESSED_SUMMARY_METADATA_KEY: &str = "_compressed_summary";
/// Mirrors `COMPRESSED_SUMMARY_HAS_USER_TURN_KEY = "_compressed_summary_has_user_turn"` (l.166)
pub const COMPRESSED_SUMMARY_HAS_USER_TURN_KEY: &str = "_compressed_summary_has_user_turn";
/// Mirrors `MICRO_COMPACT_MARKER_KEY = "_micro_compact_marker"` (l.172)
pub const MICRO_COMPACT_MARKER_KEY: &str = "_micro_compact_marker";
/// Mirrors `_DB_PERSISTED_MARKER = "_db_persisted"` (l.173)
pub const DB_PERSISTED_MARKER: &str = "_db_persisted";
#[allow(dead_code)]
const _DB_PERSISTED_MARKER: &str = DB_PERSISTED_MARKER;

/// Mirrors `PROACTIVE_PRUNE_REARM_MODEL_CONFIG_KEY = "_proactive_prune_rearm_tokens"` (l.174)
pub const PROACTIVE_PRUNE_REARM_MODEL_CONFIG_KEY: &str = "_proactive_prune_rearm_tokens";

// ---------------------------------------------------------------------------
// User-task sentinels — mirrors Python ll.176-194
// ---------------------------------------------------------------------------
/// Mirrors `_NO_USER_TASK_SENTINEL = "None. This session contains no user-authored turns."` (l.176)
pub const NO_USER_TASK_SENTINEL: &str = "None. This session contains no user-authored turns.";
#[allow(dead_code)]
const _NO_USER_TASK_SENTINEL: &str = NO_USER_TASK_SENTINEL;

/// Mirrors `COMPRESSION_CONTINUATION_USER_CONTENT = (...)` (ll.177-180)
pub const COMPRESSION_CONTINUATION_USER_CONTENT: &str =
    "Continue from the compressed conversation context above. This marker exists because no human user turn was available.";

#[allow(dead_code)]
const _LEGACY_COMPRESSION_CONTINUATION_USER_CONTENT: &str =
    "Continue from the compressed conversation context above. This marker exists because the compacted transcript contained no preserved user turn.";

/// Mirrors `MAX_ITERATIONS_SUMMARY_REQUEST = (...)` (ll.190-194)
pub const MAX_ITERATIONS_SUMMARY_REQUEST: &str = "You've reached the maximum number of tool-calling iterations allowed. Please provide a final response summarizing what you've found and accomplished so far, without calling any more tools.";

/// Mirrors `_BACKGROUND_PROCESS_NOTIFICATION_PREFIX = "[IMPORTANT: Background process "` (l.195)
pub const BACKGROUND_PROCESS_NOTIFICATION_PREFIX: &str = "[IMPORTANT: Background process ";

// ---------------------------------------------------------------------------
// Message type — mirrors Python `Dict[str, Any]` (ll.26, 198+)
// Python messages are `{"role": "...", "content": ..., "tool_calls": ..., etc}`
// Rust: `HashMap<String, Value>` preserves the open-dict shape; `Value::Object`
// helpers exist where needed. This alias keeps line-level parity with
// `List[Dict[str, Any]]` in signatures.
// ---------------------------------------------------------------------------
pub type Message = HashMap<String, Value>;
pub type Turns = Vec<Message>;

// ---------------------------------------------------------------------------
// Compaction assembly helpers — mirrors Python ll.198-263
// ---------------------------------------------------------------------------

/// Mirrors `def _fresh_compaction_message_copy(msg: Dict[str, Any]) -> Dict[str, Any]:` (ll.198-214)
/// Copy a message for compaction assembly without persistence markers.
/// Live cached-gateway transcripts stamp `_db_persisted` during incremental flushes.
pub fn fresh_compaction_message_copy(msg: &Message) -> Message {
    let mut fresh = msg.clone();
    fresh.remove(DB_PERSISTED_MARKER);
    fresh
}

#[allow(dead_code)]
fn _fresh_compaction_message_copy(msg: &Message) -> Message {
    fresh_compaction_message_copy(msg)
}

/// Mirrors `def _template_visible_role(message: Any) -> Optional[str]:` (ll.217-243)
/// Role as counted by strict chat-template alternation checks.
pub fn template_visible_role(message: &Message) -> Option<String> {
    let role = message.get("role").and_then(|v| v.as_str())?;
    if role == "tool" {
        return None;
    }
    if role == "assistant" && message.get("tool_calls").is_some() {
        // `assistant` with `tool_calls` is skipped by Mistral-family alternation checks
        // (ll.240-242). Python checks `message.get("tool_calls")` truthiness; we mirror
        // by checking presence + non-null + non-empty.
        let tc = message.get("tool_calls")?;
        if tc.is_null() {
            return Some(role.to_string());
        }
        if let Some(arr) = tc.as_array() {
            if arr.is_empty() {
                return Some(role.to_string());
            }
            return None;
        }
        return None;
    }
    Some(role.to_string())
}

#[allow(dead_code)]
fn _template_visible_role(message: &Message) -> Option<String> {
    template_visible_role(message)
}

/// Mirrors `def _strip_persistence_markers(messages: List[Dict[str, Any]]) -> None:` (ll.246-262)
/// Enforce the compaction invariant: no assembled message carries a persistence marker.
pub fn strip_persistence_markers(messages: &mut Turns) {
    for msg in messages.iter_mut() {
        msg.remove(DB_PERSISTED_MARKER);
    }
}

#[allow(dead_code)]
fn _strip_persistence_markers(messages: &mut Turns) {
    strip_persistence_markers(messages)
}

// ---------------------------------------------------------------------------
// Stale reasoning replay pruning — mirrors Python ll.265-332
// ---------------------------------------------------------------------------

/// Mirrors `_STALE_REPLAY_PRUNE_KEYS = ("codex_reasoning_items",)` (l.1393-1395)
/// Defined here for forward reference from `_prune_stale_reasoning_replay` (ll.316)
pub const STALE_REPLAY_PRUNE_KEYS: &[&str] = &["codex_reasoning_items"];

/// Mirrors `def _prune_stale_reasoning_replay(messages: List[Dict[str, Any]]) -> int:` (ll.265-332)
///
/// Strip stale per-turn replay items (`codex_reasoning_items`) from assistant
/// messages that belong to turns older than the active one.
pub fn prune_stale_reasoning_replay(messages: &mut Turns) -> usize {
    // Find last real user message — everything after it is the active turn.
    let mut last_user_idx: Option<usize> = None;
    for (i, msg) in messages.iter().enumerate().rev() {
        if msg.get("role").and_then(|v| v.as_str()) == Some("user") {
            last_user_idx = Some(i);
            break;
        }
    }
    let Some(last_user_idx) = last_user_idx else {
        return 0;
    };
    let mut pruned = 0usize;
    for i in 0..last_user_idx {
        let Some(msg) = messages.get_mut(i) else { continue };
        if msg.get("role").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        for key in STALE_REPLAY_PRUNE_KEYS {
            let Some(items) = msg.get(*key) else { continue };
            let Some(arr) = items.as_array() else { continue };
            if arr.is_empty() {
                continue;
            }
            let kept: Vec<Value> = arr
                .iter()
                .filter(|item| item.get("type").and_then(|v| v.as_str()) == Some("compaction"))
                .cloned()
                .collect();
            if kept.len() == arr.len() {
                continue; // nothing stale in this sidecar
            }
            if kept.is_empty() {
                msg.remove(*key);
            } else {
                msg.insert((*key).to_string(), Value::Array(kept));
            }
            pruned += 1;
        }
    }
    pruned
}

#[allow(dead_code)]
fn _prune_stale_reasoning_replay(messages: &mut Turns) -> usize {
    prune_stale_reasoning_replay(messages)
}

// ---------------------------------------------------------------------------
// Summary boundary markers + salvage — mirrors Python ll.335-493
// ---------------------------------------------------------------------------

/// Mirrors `_SUMMARY_END_MARKER = (...)` (ll.340-343)
pub const SUMMARY_END_MARKER: &str =
    "--- END OF CONTEXT SUMMARY — respond to the message below, not the summary above ---";

/// Mirrors `_MERGED_PRIOR_CONTEXT_HEADER = "[PRIOR CONTEXT — for reference only; not a new message]"` (l.351)
pub const MERGED_PRIOR_CONTEXT_HEADER: &str =
    "[PRIOR CONTEXT — for reference only; not a new message]";
/// Mirrors `_MERGED_SUMMARY_DELIMITER = "[END OF PRIOR CONTEXT — COMPACTION SUMMARY BELOW]"` (l.352)
pub const MERGED_SUMMARY_DELIMITER: &str = "[END OF PRIOR CONTEXT — COMPACTION SUMMARY BELOW]";

/// Mirrors `_SALVAGE_SUMMARY_MAX_CHARS = 8_000` (l.354)
pub const SALVAGE_SUMMARY_MAX_CHARS: usize = 8_000;
/// Mirrors `_SALVAGE_KEEP_RECENT_TOOLS = 2` (l.355)
pub const SALVAGE_KEEP_RECENT_TOOLS: usize = 2;

// ---------------------------------------------------------------------------
// Helpers for salvage — mirrors Python ll.358-413
// ---------------------------------------------------------------------------

/// Mirrors `def _looks_like_compaction_summary(msg: Dict[str, Any], content: str) -> bool:` (ll.358-383)
pub fn looks_like_compaction_summary(msg: &Message, content: &str) -> bool {
    if !content.trim_end().ends_with(SUMMARY_END_MARKER) {
        return false;
    }
    if content.starts_with(MERGED_PRIOR_CONTEXT_HEADER) {
        return false;
    }
    if msg.get("role").and_then(|v| v.as_str()) == Some("tool") {
        return false;
    }
    if matches!(msg.get("role").and_then(|v| v.as_str()), Some("user") | Some("assistant"))
        && !msg.contains_key(COMPRESSED_SUMMARY_METADATA_KEY)
    {
        return false;
    }
    let head = &content[..content.len().min(280)];
    msg.contains_key(COMPRESSED_SUMMARY_METADATA_KEY)
        || head.contains("CONTEXT COMPACTION")
        || head.contains("[CONTEXT COMPACTION]")
        || head.contains("Conversation Summary")
}

#[allow(dead_code)]
fn _looks_like_compaction_summary(msg: &Message, content: &str) -> bool {
    looks_like_compaction_summary(msg, content)
}

/// Stub for `_PRUNED_SKILL_RELOAD_NOTICE_HEADER` — canonical lives in
/// `agent/conversation_compression.py` (imported inside function at l.396).
/// Kept here so `_salvage_reduce_todo_snapshot` mirrors Python's lazy import.
const PRUNED_SKILL_RELOAD_NOTICE_HEADER: &str = "[PRUNED_SKILL_RELOAD_NOTICE]";

/// Mirrors `def _salvage_reduce_todo_snapshot(out: List[Dict[str, Any]]) -> None:` (ll.386-413)
/// Last-resort shrink: reduce or drop the synthetic todo snapshot.
pub fn salvage_reduce_todo_snapshot(out: &mut Turns) {
    for i in (0..out.len()).rev() {
        let msg = &out[i];
        if !msg.contains_key("_todo_snapshot_synthetic") {
            continue;
        }
        if msg.get("role").and_then(|v| v.as_str()) != Some("user") {
            continue;
        }
        let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(idx) = content.find(PRUNED_SKILL_RELOAD_NOTICE_HEADER) {
            let trimmed = content[idx..].to_string();
            if let Some(m) = out.get_mut(i) {
                m.insert("content".to_string(), Value::String(trimmed));
            }
        } else {
            out.remove(i);
        }
        return;
    }
}

#[allow(dead_code)]
fn _salvage_reduce_todo_snapshot(out: &mut Turns) {
    salvage_reduce_todo_snapshot(out)
}

// Stubs for token estimation — mirrors `agent/model_metadata.py` (ll.40-41)
// Real impls count tokens via model-specific heuristics; stubs use chars/4.
fn estimate_messages_tokens_rough(messages: &Turns) -> usize {
    // Mirrors `estimate_messages_tokens_rough` — rough chars/4 + overhead
    let mut chars = 0usize;
    for m in messages {
        if let Some(c) = m.get("content").and_then(|v| v.as_str()) {
            chars += c.len();
        } else if let Some(v) = m.get("content") {
            chars += v.to_string().len();
        }
        if let Some(tc) = m.get("tool_calls") {
            chars += tc.to_string().len();
        }
    }
    chars / 4 + messages.len() * 4
}

/// Mirrors `def salvage_grown_transcript(...) -> Optional[List[Dict[str, Any]]]:` (ll.416-493)
/// Mechanically shrink a compression candidate, or return `None`.
pub fn salvage_grown_transcript(
    original: &Turns,
    candidate: &Turns,
    budget: Option<usize>,
) -> Option<Turns> {
    if candidate.is_empty() || original.is_empty() {
        return None;
    }
    let budget = budget.unwrap_or_else(|| estimate_messages_tokens_rough(original));
    if budget == 0 {
        return None;
    }

    let mut out: Turns = Vec::with_capacity(candidate.len());
    let mut tool_indices: Vec<usize> = Vec::new();
    let mut last_assistant_idx: Option<usize> = None;
    for msg in candidate {
        let copied = msg.clone();
        out.push(copied);
        let idx = out.len() - 1;
        let role = out[idx].get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role == "tool" {
            tool_indices.push(idx);
        } else if role == "assistant" {
            last_assistant_idx = Some(idx);
        }
    }

    // Mirrors `salvage_reasoning_keys = _NEWEST_TURN_ONLY_BUDGET_KEYS + ("reasoning_details",)` (l.456)
    const SALVAGE_REASONING_KEYS: &[&str] = &["reasoning", "reasoning_content", "reasoning_details"];
    let keep_tools: HashSet<usize> = tool_indices
        .iter()
        .rev()
        .take(SALVAGE_KEEP_RECENT_TOOLS)
        .copied()
        .collect();

    for (index, msg) in out.iter_mut().enumerate() {
        if msg.get("role").and_then(|v| v.as_str()) == Some("assistant")
            && Some(index) != last_assistant_idx
        {
            for key in SALVAGE_REASONING_KEYS {
                msg.remove(*key);
            }
        }
        if msg.get("role").and_then(|v| v.as_str()) == Some("tool") && !keep_tools.contains(&index) {
            if let Some(content) = msg.get("content").and_then(|v| v.as_str()).map(|s| s.to_string()) {
                if content.len() > PRUNE_MIN_CHARS {
                    msg.insert("content".to_string(), Value::String(PRUNED_TOOL_PLACEHOLDER.to_string()));
                }
            }
        }
        // Cap oversized standalone summary
        if let Some(content) = msg.get("content").and_then(|v| v.as_str()).map(|s| s.to_string()) {
            if content.len() > SALVAGE_SUMMARY_MAX_CHARS && looks_like_compaction_summary(msg, &content) {
                let truncated = format!(
                    "{}\n…[summary truncated so compaction can shrink]\n\n{}",
                    content[..SALVAGE_SUMMARY_MAX_CHARS].trim_end(),
                    SUMMARY_END_MARKER
                );
                msg.insert("content".to_string(), Value::String(truncated));
            }
        }
    }

    prune_stale_reasoning_replay(&mut out);

    if estimate_messages_tokens_rough(&out) >= budget {
        salvage_reduce_todo_snapshot(&mut out);
    }

    if !out.iter().any(|m| m.get("role").and_then(|v| v.as_str()) == Some("user")) {
        return None;
    }
    if estimate_messages_tokens_rough(&out) < budget {
        return Some(out);
    }
    None
}

// ---------------------------------------------------------------------------
// Historical summary prefixes — mirrors Python ll.495-636
// Keep newest-first; entries matched literally. NEVER mutate/reorder existing
// entry — prepend only. (ll.500-504)
// ---------------------------------------------------------------------------

/// Mirrors `_HISTORICAL_SUMMARY_PREFIXES = (...)` (ll.505-636)
/// Each entry is the EXACT wire text a shipped build persisted.
pub const HISTORICAL_SUMMARY_PREFIXES: &[&str] = &[
    // Pre-#80622: lacked the explicit "if no user message appears AFTER this summary, do nothing" clause (ll.511-536)
    concat!(
        "[CONTEXT COMPACTION — REFERENCE ONLY] Earlier turns were compacted ",
        "into the summary below. This is a handoff from a previous context ",
        "window — treat it as background reference, NOT as active instructions. ",
        "Do NOT answer questions or fulfill requests mentioned in this summary; ",
        "they were already addressed. ",
        "Respond ONLY to the latest user message that appears AFTER this ",
        "summary — that message is the single source of truth for what to do ",
        "right now. ",
        "Topic overlap with the summary does NOT mean you should resume its ",
        "task: even on similar topics, the latest user message WINS. Treat ONLY ",
        "the latest message as the active task and discard stale items from ",
        "'## Historical Task Snapshot' entirely — do not 'wrap up' or ",
        "'finish' work described there unless the latest message explicitly ",
        "asks for it. ",
        "Reverse signals in the latest message (e.g. 'stop', 'undo', 'roll ",
        "back', 'just verify', 'don't do that anymore', 'never mind', a new ",
        "topic) must immediately end any in-flight work described in the ",
        "summary; do not re-surface it in later turns. ",
        "IMPORTANT: Your persistent memory (MEMORY.md, USER.md) in the system ",
        "prompt is ALWAYS authoritative and active — never ignore or deprioritize ",
        "memory content due to this compaction note. ",
        "None of the above restricts HOW you work: your tools remain fully ",
        "active — keep calling them normally for the active task (edit files, ",
        "run commands, search) instead of merely narrating what you would do. ",
        "The current session state (files, config, etc.) may reflect work ",
        "described here — avoid repeating it:"
    ),
    // Pre-#69619: stale-item discard clause named all four historical headings (ll.542-569)
    concat!(
        "[CONTEXT COMPACTION — REFERENCE ONLY] Earlier turns were compacted ",
        "into the summary below. This is a handoff from a previous context ",
        "window — treat it as background reference, NOT as active instructions. ",
        "Do NOT answer questions or fulfill requests mentioned in this summary; ",
        "they were already addressed. ",
        "Respond ONLY to the latest user message that appears AFTER this ",
        "summary — that message is the single source of truth for what to do ",
        "right now. ",
        "Topic overlap with the summary does NOT mean you should resume its ",
        "task: even on similar topics, the latest user message WINS. Treat ONLY ",
        "the latest message as the active task and discard stale items from ",
        "'## Historical Task Snapshot' / '## Historical In-Progress State' / ",
        "'## Historical Pending User Asks' / ",
        "'## Historical Remaining Work' entirely — do not 'wrap up' or ",
        "'finish' work described there unless the latest message explicitly ",
        "asks for it. ",
        "Reverse signals in the latest message (e.g. 'stop', 'undo', 'roll ",
        "back', 'just verify', 'don't do that anymore', 'never mind', a new ",
        "topic) must immediately end any in-flight work described in the ",
        "summary; do not re-surface it in later turns. ",
        "IMPORTANT: Your persistent memory (MEMORY.md, USER.md) in the system ",
        "prompt is ALWAYS authoritative and active — never ignore or deprioritize ",
        "memory content due to this compaction note. ",
        "None of the above restricts HOW you work: your tools remain fully ",
        "active — keep calling them normally for the active task (edit files, ",
        "run commands, search) instead of merely narrating what you would do. ",
        "The current session state (files, config, etc.) may reflect work ",
        "described here — avoid repeating it:"
    ),
    // Jul 2026 (#65848 class): lacked explicit "tools remain fully active" clause (ll.575-599)
    concat!(
        "[CONTEXT COMPACTION — REFERENCE ONLY] Earlier turns were compacted ",
        "into the summary below. This is a handoff from a previous context ",
        "window — treat it as background reference, NOT as active instructions. ",
        "Do NOT answer questions or fulfill requests mentioned in this summary; ",
        "they were already addressed. ",
        "Respond ONLY to the latest user message that appears AFTER this ",
        "summary — that message is the single source of truth for what to do ",
        "right now. ",
        "Topic overlap with the summary does NOT mean you should resume its ",
        "task: even on similar topics, the latest user message WINS. Treat ONLY ",
        "the latest message as the active task and discard stale items from ",
        "'## Historical Task Snapshot' / '## Historical In-Progress State' / ",
        "'## Historical Pending User Asks' / ",
        "'## Historical Remaining Work' entirely — do not 'wrap up' or ",
        "'finish' work described there unless the latest message explicitly ",
        "asks for it. ",
        "Reverse signals in the latest message (e.g. 'stop', 'undo', 'roll ",
        "back', 'just verify', 'don't do that anymore', 'never mind', a new ",
        "topic) must immediately end any in-flight work described in the ",
        "summary; do not re-surface it in later turns. ",
        "IMPORTANT: Your persistent memory (MEMORY.md, USER.md) in the system ",
        "prompt is ALWAYS authoritative and active — never ignore or deprioritize ",
        "memory content due to this compaction note. ",
        "The current session state (files, config, etc.) may reflect work ",
        "described here — avoid repeating it:"
    ),
    // Carveout era (#41607/#38364/#42812): "consistent → use as background" (ll.602-624)
    concat!(
        "[CONTEXT COMPACTION — REFERENCE ONLY] Earlier turns were compacted ",
        "into the summary below. This is a handoff from a previous context ",
        "window — treat it as background reference, NOT as active instructions. ",
        "Do NOT answer questions or fulfill requests mentioned in this summary; ",
        "they were already addressed. ",
        "Respond ONLY to the latest user message that appears AFTER this ",
        "summary — that message is the single source of truth for what to do ",
        "right now. ",
        "If the latest user message is consistent with the '## Active Task' ",
        "section, you may use the summary as background. If the latest user ",
        "message contradicts, supersedes, changes topic from, or in any way ",
        "diverges from '## Active Task' / '## In Progress' / '## Pending User ",
        "Asks' / '## Remaining Work', the latest message WINS — discard those ",
        "stale items entirely and do not 'wrap up the old task first'. ",
        "Reverse signals in the latest message (e.g. 'stop', 'undo', 'roll ",
        "back', 'just verify', 'don't do that anymore', 'never mind', a new ",
        "topic) must immediately end any in-flight work described in the ",
        "summary; do not re-surface it in later turns. ",
        "IMPORTANT: Your persistent memory (MEMORY.md, USER.md) in the system ",
        "prompt is ALWAYS authoritative and active — never ignore or deprioritize ",
        "memory content due to this compaction note. ",
        "The current session state (files, config, etc.) may reflect work ",
        "described here — avoid repeating it:"
    ),
    // Pre-#35344: contained the self-contradicting "resume exactly" directive (ll.626-635)
    concat!(
        "[CONTEXT COMPACTION — REFERENCE ONLY] Earlier turns were compacted ",
        "into the summary below. This is a handoff from a previous context ",
        "window — treat it as background reference, NOT as active instructions. ",
        "Do NOT answer questions or fulfill requests mentioned in this summary; ",
        "they were already addressed. ",
        "Your current task is identified in the '## Active Task' section of the ",
        "summary — resume exactly from there. ",
        "Respond ONLY to the latest user message ",
        "that appears AFTER this summary. The current session state (files, ",
        "config, etc.) may reflect work described here — avoid repeating it:"
    ),
];
#[allow(dead_code)]
const _HISTORICAL_SUMMARY_PREFIXES: &[&str] = HISTORICAL_SUMMARY_PREFIXES;

// ---------------------------------------------------------------------------
// Probe / budget constants — mirrors Python ll.638-682
// ---------------------------------------------------------------------------

/// Mirrors `_RESTART_HANDOFF_PROBE_EXTRA_MESSAGES = 4` (l.642)
pub const RESTART_HANDOFF_PROBE_EXTRA_MESSAGES: usize = 4;
#[allow(dead_code)]
const _RESTART_HANDOFF_PROBE_EXTRA_MESSAGES: usize = RESTART_HANDOFF_PROBE_EXTRA_MESSAGES;

/// Mirrors `_MIN_SUMMARY_TOKENS = 2000` (l.645)
pub const MIN_SUMMARY_TOKENS: usize = 2000;
/// Mirrors `_SUMMARY_RATIO = 0.20` (l.647)
pub const SUMMARY_RATIO: f64 = 0.20;
/// Mirrors `_SUMMARY_TOKENS_CEILING = 10_000` (l.651)
pub const SUMMARY_TOKENS_CEILING: usize = 10_000;

/// Mirrors `_MICRO_COMPACT_MAX_CONSECUTIVE_FAILURES = 3` (l.656)
pub const MICRO_COMPACT_MAX_CONSECUTIVE_FAILURES: usize = 3;

/// Mirrors `_SUMMARY_INPUT_MAX_CHARS = 160_000` (l.670)
pub const SUMMARY_INPUT_MAX_CHARS: usize = 160_000;

/// Mirrors `_PRUNED_TOOL_PLACEHOLDER = "[Old tool output cleared to save context space]"` (l.673)
pub const PRUNED_TOOL_PLACEHOLDER: &str = "[Old tool output cleared to save context space]";
#[allow(dead_code)]
const _PRUNED_TOOL_PLACEHOLDER: &str = PRUNED_TOOL_PLACEHOLDER;

/// Mirrors `_PRUNE_MIN_CHARS = 200` (l.679)
pub const PRUNE_MIN_CHARS: usize = 200;
#[allow(dead_code)]
const _PRUNE_MIN_CHARS: usize = PRUNE_MIN_CHARS;

// ---------------------------------------------------------------------------
// Clarify non-response sentinels — mirrors Python ll.686-711
// ---------------------------------------------------------------------------

/// Mirrors `_CLARIFY_NON_RESPONSE_PREFIXES = (...)` (ll.686-691)
pub const CLARIFY_NON_RESPONSE_PREFIXES: &[&str] = &[
    "The user did not provide a response",
    "[user did not respond",
    "[clarify prompt could not be delivered",
    "[oneshot mode:",
];
#[allow(dead_code)]
const _CLARIFY_NON_RESPONSE_PREFIXES: &[&str] = CLARIFY_NON_RESPONSE_PREFIXES;

/// Mirrors `def _is_clarify_non_response_sentinel(response: Any) -> bool:` (ll.694-711)
pub fn is_clarify_non_response_sentinel(response: &Value) -> bool {
    match response {
        Value::String(s) => CLARIFY_NON_RESPONSE_PREFIXES.iter().any(|p| s.trim_start().starts_with(*p)),
        Value::Array(arr) => arr.iter().any(|item| {
            if let Some(s) = item.as_str() {
                CLARIFY_NON_RESPONSE_PREFIXES.iter().any(|p| s.trim_start().starts_with(*p))
            } else {
                false
            }
        }),
        _ => false,
    }
}

#[allow(dead_code)]
fn _is_clarify_non_response_sentinel(response: &Value) -> bool {
    is_clarify_non_response_sentinel(response)
}

// ---------------------------------------------------------------------------
// Ghost-skill defense — mirrors Python ll.714-847
// ---------------------------------------------------------------------------

/// Mirrors `SKILL_PRUNED_MARKER_PREFIX = "[SKILL_PRUNED:"` (l.721)
pub const SKILL_PRUNED_MARKER_PREFIX: &str = "[SKILL_PRUNED:";
/// Mirrors `_SKILL_VIEW_PRUNE_MIN_CHARS = 5000` (l.725)
pub const SKILL_VIEW_PRUNE_MIN_CHARS: usize = 5000;
/// Mirrors `_MAX_PRUNED_SKILL_MARKERS = 20` (l.729)
pub const MAX_PRUNED_SKILL_MARKERS: usize = 20;

/// Mirrors `def _skill_pruned_marker(skill_name: str) -> str:` (ll.732-742)
pub fn skill_pruned_marker(skill_name: &str) -> String {
    format!(
        "{} content lost in compression; reload with skill_view(name='{}')]",
        SKILL_PRUNED_MARKER_PREFIX, skill_name
    )
}

#[allow(dead_code)]
fn _skill_pruned_marker(skill_name: &str) -> String {
    skill_pruned_marker(skill_name)
}

fn skill_pruned_marker_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"{}[^\]]*?reload with skill_view\(name='([^']+)'\)",
            regex::escape(SKILL_PRUNED_MARKER_PREFIX)
        ))
        .expect("skill pruned marker regex")
    })
}

/// Mirrors `def _extract_pruned_skill_names(text: str) -> list[str]:` (ll.754-761)
pub fn extract_pruned_skill_names(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for cap in skill_pruned_marker_re().captures_iter(text) {
        if let Some(m) = cap.get(1) {
            let name = m.as_str().to_string();
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

#[allow(dead_code)]
fn _extract_pruned_skill_names(text: &str) -> Vec<String> {
    extract_pruned_skill_names(text)
}

// Stubs for helpers referenced by `_collect_ghosted_skill_names` but defined
// later in Python (l.1114 ff). Canonical impls live in slice2; stubs keep
// slice1 self-contained.

/// Stub: mirrors `def _content_text_for_contains(content: Any) -> str:` (l.1505 ff)
/// Returns best-effort text view of message content for substring checks.
fn content_text_for_contains(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(arr) => arr
            .iter()
            .filter_map(|item| {
                if let Some(s) = item.as_str() {
                    Some(s.to_string())
                } else if let Some(obj) = item.as_object() {
                    obj.get("text").and_then(|v| v.as_str()).map(|s| s.to_string())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Stub: mirrors `def _skill_view_call_sites(messages) -> list[tuple[int, str]]:` (l.1114 ff)
/// Yield `(message_index, skill_name)` for every skill_view tool call.
fn skill_view_call_sites(messages: &Turns) -> Vec<(usize, String)> {
    let mut sites: Vec<(usize, String)> = Vec::new();
    for (i, msg) in messages.iter().enumerate() {
        if msg.get("role").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let Some(tc_val) = msg.get("tool_calls") else { continue };
        let Some(arr) = tc_val.as_array() else { continue };
        for tc in arr {
            let (name, args_str) = extract_tool_call_name_and_args(tc);
            if name != "skill_view" || args_str.is_empty() {
                continue;
            }
            if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&args_str) {
                if let Some(Value::String(skill)) = map.get("name") {
                    if !skill.is_empty() {
                        sites.push((i, skill.clone()));
                    }
                }
            }
        }
    }
    sites
}

fn extract_tool_call_name_and_args(tc: &Value) -> (String, String) {
    // Mirrors `_extract_tool_call_name_and_args` (l.1281 ff) — minimal for slice1
    if let Some(obj) = tc.as_object() {
        if let Some(func) = obj.get("function").and_then(|v| v.as_object()) {
            let name = func.get("name").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
            let args = func.get("arguments").and_then(|v| v.as_str()).unwrap_or("").to_string();
            return (name, args);
        }
    }
    ("unknown".to_string(), String::new())
}

/// Stub: mirrors `def _redact_compaction_text(text: Any) -> str:` (l.1254 ff)
/// Redacts text that crosses a compaction summary boundary.
fn redact_compaction_text(text: &str) -> String {
    // Real impl calls `redact_sensitive_text(force=True, redact_url_credentials=True)`
    // from `agent/redact.py`. Stub returns input verbatim; canonical impl in sliceN.
    text.to_string()
}

/// Mirrors `def _collect_ghosted_skill_names(turns: List[Dict[str, Any]]) -> list[str]:` (ll.764-806)
///
/// Skill names whose instructions are about to be lost in compaction.
pub fn collect_ghosted_skill_names(turns: &Turns) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut add = |name: String| {
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    };
    // Map call_id -> skill_name from assistant tool_calls
    let mut call_id_to_skill: HashMap<String, String> = HashMap::new();
    for (idx, skill) in skill_view_call_sites(turns) {
        let Some(msg) = turns.get(idx) else { continue };
        let Some(tc_val) = msg.get("tool_calls") else { continue };
        let Some(arr) = tc_val.as_array() else { continue };
        for tc in arr {
            let Some(obj) = tc.as_object() else { continue };
            let func = obj.get("function").and_then(|v| v.as_object());
            let tc_name = func.and_then(|f| f.get("name")).and_then(|v| v.as_str()).unwrap_or("");
            if tc_name != "skill_view" {
                continue;
            }
            let cid = obj.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !cid.is_empty() {
                call_id_to_skill.insert(cid, skill.clone());
            }
        }
    }
    for msg in turns {
        let content_val = msg.get("content").unwrap_or(&Value::Null);
        let text = if let Some(s) = content_val.as_str() {
            s.to_string()
        } else {
            content_text_for_contains(content_val)
        };
        for name in extract_pruned_skill_names(&text) {
            add(name);
        }
        if msg.get("role").and_then(|v| v.as_str()) == Some("tool") {
            if let Some(content) = content_val.as_str() {
                if content.len() > SKILL_VIEW_PRUNE_MIN_CHARS {
                    if let Some(skill) = call_id_to_skill.get(msg.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("")) {
                        add(skill.clone());
                    }
                }
            }
        }
    }
    names
}

#[allow(dead_code)]
fn _collect_ghosted_skill_names(turns: &Turns) -> Vec<String> {
    collect_ghosted_skill_names(turns)
}

// NOTE: `l.807-847` (`_PRUNED_SKILLS_SECTION_HEADING` + `_reinject_pruned_skill_markers`)
// and all lean-tail machinery from `l.850` onward (`LEAN_TAIL_*`, `_lean_recovery_stub`,
// `_synthetic_user_row`, `_build_verbatim_user_section`, `_build_recovery_footer`,
// `_LEAN_DIGEST_*`, `_LEAN_DIGEST_PROMPT`, `_LOW_SIGNAL_TOOL_RE`, anchor ledger, etc.)
// start at Python l.807 and are deferred to `compressor_slice2.rs` (slice 2/11).
// The boundary at l.800 falls mid-function inside `_collect_ghosted_skill_names`
// (Python ll.800-806); that function is completed here so the Rust slice is
// syntactically closed. The next constant `_PRUNED_SKILLS_SECTION_HEADING`
// (l.809) is the first item of slice2.
