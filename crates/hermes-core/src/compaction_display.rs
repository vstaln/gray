//! Client-facing projection helpers for model-only compaction carriers.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/compaction_display.py` (47 lines).
//!
//! Python source docstring (preserved):
//! ```text
//! Client-facing projection helpers for model-only compaction carriers.
//! ```
//!
//! Python source (preserved verbatim, lines 1-47):
//! ```python
//! """Client-facing projection helpers for model-only compaction carriers."""
//!
//! from __future__ import annotations
//!
//! from typing import Any, Dict, Optional
//!
//! from agent.context_compressor import (
//!     ContextCompressor,
//!     is_compaction_summary_message,
//! )
//!
//!
//! _COMPACTION_INTERNAL_FIELDS = (
//!     "tool_calls",
//!     "finish_reason",
//!     "reasoning",
//!     "reasoning_content",
//!     "reasoning_details",
//!     "codex_reasoning_items",
//!     "codex_message_items",
//! )
//!
//!
//! def project_compaction_message_for_display(
//!     message: Dict[str, Any],
//! ) -> Optional[Dict[str, Any]]:
//!     """Return authentic transcript content, or ``None`` for a pure handoff.
//!
//!     Model-facing recovery history retains the complete carrier. Display
//!     projections instead remove the handoff, inherited tool state, and internal
//!     reasoning while preserving any real prior-tail content or live user ask
//!     embedded in the carrier.
//!     """
//!     if not isinstance(message, dict):
//!         return None
//!     if not is_compaction_summary_message(message):
//!         return message.copy()
//!
//!     projected = ContextCompressor._strip_context_summary_handoff_message(message)
//!     if projected is None:
//!         return None
//!
//!     projected = projected.copy()
//!     for key in _COMPACTION_INTERNAL_FIELDS:
//!         projected.pop(key, None)
//!     projected.pop("display_kind", None)
//!     return projected
//! ```
//!
//! Rust notes:
//! - `Dict[str, Any]` → `serde_json::Value::Object` (`Map<String, Value>`). The
//!   Python `isinstance(message, dict)` guard maps to `Value::is_object()`.
//! - `is_compaction_summary_message` and `ContextCompressor._strip_context_summary_handoff_message`
//!   are ported inline from `agent/context_compressor.py` (lines 1505-1525, 5294-5333,
//!   5640-5756, 8083-8104) so this crate remains self-contained without a
//!   `hermes-context` dependency. Constants `SUMMARY_PREFIX`, `LEGACY_SUMMARY_PREFIX`,
//!   `_HISTORICAL_SUMMARY_PREFIXES`, `_MERGED_*`, `_SUMMARY_END_MARKER`, and
//!   `COMPRESSED_SUMMARY_METADATA_KEY` are byte-identical to the Python source.
//! - `message.copy()` → `Value::clone()` (shallow copy is deep for JSON values).
//! - `projected.pop(key, None)` → `Map::remove(key)`.
//! - Truthiness of `COMPRESSED_SUMMARY_METADATA_KEY` mirrors Python `bool(value)`
//!   via `is_truthy`.

use serde_json::Value;

// ---------------------------------------------------------------------------
// Constants — mirrors `agent/context_compressor.py` ll.115-173, 340-352, 505-636
// ---------------------------------------------------------------------------

/// Mirrors `COMPRESSED_SUMMARY_METADATA_KEY = "_compressed_summary"` (l.165).
pub const COMPRESSED_SUMMARY_METADATA_KEY: &str = "_compressed_summary";
#[allow(dead_code)]
const _COMPRESSED_SUMMARY_METADATA_KEY: &str = COMPRESSED_SUMMARY_METADATA_KEY;

/// Mirrors `SUMMARY_PREFIX = (...)` (ll.115-149).
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

/// Mirrors `LEGACY_SUMMARY_PREFIX = "[CONTEXT SUMMARY]:"` (l.150).
pub const LEGACY_SUMMARY_PREFIX: &str = "[CONTEXT SUMMARY]:";

/// Mirrors `_MERGED_PRIOR_CONTEXT_HEADER = "[PRIOR CONTEXT — for reference only; not a new message]"` (l.351).
pub const MERGED_PRIOR_CONTEXT_HEADER: &str =
    "[PRIOR CONTEXT — for reference only; not a new message]";
#[allow(dead_code)]
const _MERGED_PRIOR_CONTEXT_HEADER: &str = MERGED_PRIOR_CONTEXT_HEADER;

/// Mirrors `_MERGED_SUMMARY_DELIMITER = "[END OF PRIOR CONTEXT — COMPACTION SUMMARY BELOW]"` (l.352).
pub const MERGED_SUMMARY_DELIMITER: &str =
    "[END OF PRIOR CONTEXT — COMPACTION SUMMARY BELOW]";
#[allow(dead_code)]
const _MERGED_SUMMARY_DELIMITER: &str = MERGED_SUMMARY_DELIMITER;

/// Mirrors `_SUMMARY_END_MARKER = "--- END OF CONTEXT SUMMARY — ..."` (ll.340-343).
pub const SUMMARY_END_MARKER: &str =
    "--- END OF CONTEXT SUMMARY — respond to the message below, not the summary above ---";
#[allow(dead_code)]
const _SUMMARY_END_MARKER: &str = SUMMARY_END_MARKER;

/// Mirrors `_HISTORICAL_SUMMARY_PREFIXES = (...)` (ll.505-636).
pub const HISTORICAL_SUMMARY_PREFIXES: &[&str] = &[
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
// _COMPACTION_INTERNAL_FIELDS — mirrors lines 13-21
// ---------------------------------------------------------------------------

/// Internal compaction/reasoning keys stripped from display projections.
///
/// Mirrors `_COMPACTION_INTERNAL_FIELDS = (...)` (lines 13-21):
/// ```python
/// _COMPACTION_INTERNAL_FIELDS = (
///     "tool_calls",
///     "finish_reason",
///     "reasoning",
///     "reasoning_content",
///     "reasoning_details",
///     "codex_reasoning_items",
///     "codex_message_items",
/// )
/// ```
pub const COMPACTION_INTERNAL_FIELDS: &[&str] = &[
    "tool_calls",
    "finish_reason",
    "reasoning",
    "reasoning_content",
    "reasoning_details",
    "codex_reasoning_items",
    "codex_message_items",
];
#[allow(dead_code)]
const _COMPACTION_INTERNAL_FIELDS: &[&str] = COMPACTION_INTERNAL_FIELDS;

// ---------------------------------------------------------------------------
// Helpers — mirrors `agent/context_compressor.py` ll.1505-1525, 5294-5343
// ---------------------------------------------------------------------------

/// Return a best-effort text view of message content for substring checks.
///
/// Mirrors `def _content_text_for_contains(content: Any) -> str:` (ll.1505-1525):
/// ```python
/// def _content_text_for_contains(content: Any) -> str:
///     if content is None:
///         return ""
///     if isinstance(content, str):
///         return content
///     if isinstance(content, list):
///         parts: list[str] = []
///         for item in content:
///             if isinstance(item, str):
///                 parts.append(item)
///             elif isinstance(item, dict):
///                 text = item.get("text")
///                 if isinstance(text, str):
///                     parts.append(text)
///         return "\n".join(part for part in parts if part)
///     return str(content)
/// ```
fn content_text_for_contains(content: &Value) -> String {
    // Mirrors `if content is None: return ""` (ll.1511-1512)
    if content.is_null() {
        return String::new();
    }
    // Mirrors `if isinstance(content, str): return content` (ll.1513-1514)
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    // Mirrors `if isinstance(content, list):` (ll.1515-1524)
    if let Some(arr) = content.as_array() {
        let mut parts: Vec<String> = Vec::new();
        for item in arr {
            if let Some(s) = item.as_str() {
                // Mirrors `if isinstance(item, str): parts.append(item)` (ll.1518-1519)
                parts.push(s.to_string());
            } else if let Some(obj) = item.as_object() {
                // Mirrors `elif isinstance(item, dict): text = item.get("text")` (ll.1520-1521)
                if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                    // Mirrors `if isinstance(text, str): parts.append(text)` (ll.1522-1523)
                    parts.push(text.to_string());
                }
            }
        }
        // Mirrors `return "\n".join(part for part in parts if part)` (l.1524)
        return parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n");
    }
    // Mirrors `return str(content)` (l.1525)
    // For Value::Object/Number/Bool this is JSON; Python would use `str(dict)` which is
    // Python repr — JSON is the closest lossless Rust equivalent for display checks.
    content.to_string()
}

#[allow(dead_code)]
fn _content_text_for_contains(content: &Value) -> String {
    content_text_for_contains(content)
}

/// Return True if *text* begins with any known handoff prefix.
///
/// Mirrors `def _starts_with_summary_prefix(text: str) -> bool:` (ll.5294-5298):
/// ```python
/// @staticmethod
/// def _starts_with_summary_prefix(text: str) -> bool:
///     if text.startswith(SUMMARY_PREFIX) or text.startswith(LEGACY_SUMMARY_PREFIX):
///         return True
///     return any(text.startswith(p) for p in _HISTORICAL_SUMMARY_PREFIXES)
/// ```
fn starts_with_summary_prefix(text: &str) -> bool {
    // Mirrors `if text.startswith(SUMMARY_PREFIX) or text.startswith(LEGACY_SUMMARY_PREFIX):` (ll.5296-5297)
    if text.starts_with(SUMMARY_PREFIX) || text.starts_with(LEGACY_SUMMARY_PREFIX) {
        return true;
    }
    // Mirrors `return any(text.startswith(p) for p in _HISTORICAL_SUMMARY_PREFIXES)` (l.5298)
    HISTORICAL_SUMMARY_PREFIXES.iter().any(|p| text.starts_with(*p))
}

#[allow(dead_code)]
fn _starts_with_summary_prefix(text: &str) -> bool {
    starts_with_summary_prefix(text)
}

/// Classify how *content* relates to a compaction summary.
///
/// Mirrors `def classify_summary_content(cls, content: Any) -> Optional[str]:` (ll.5301-5326):
/// ```python
/// @classmethod
/// def classify_summary_content(cls, content: Any) -> Optional[str]:
///     text = _content_text_for_contains(content).lstrip()
///     if _MERGED_SUMMARY_DELIMITER in text:
///         after = text.split(_MERGED_SUMMARY_DELIMITER, 1)[1].lstrip()
///         return "merged" if cls._starts_with_summary_prefix(after) else None
///     return "standalone" if cls._starts_with_summary_prefix(text) else None
/// ```
fn classify_summary_content(content: &Value) -> Option<&'static str> {
    // Mirrors `text = _content_text_for_contains(content).lstrip()` (l.5317)
    let text = content_text_for_contains(content);
    let trimmed = text.trim_start();
    // Mirrors `if _MERGED_SUMMARY_DELIMITER in text:` (l.5323)
    if trimmed.contains(MERGED_SUMMARY_DELIMITER) {
        // Mirrors `after = text.split(_MERGED_SUMMARY_DELIMITER, 1)[1].lstrip()` (l.5324)
        if let Some(after) = trimmed.splitn(2, MERGED_SUMMARY_DELIMITER).nth(1) {
            let after_trimmed = after.trim_start();
            // Mirrors `return "merged" if cls._starts_with_summary_prefix(after) else None` (l.5325)
            return if starts_with_summary_prefix(after_trimmed) {
                Some("merged")
            } else {
                None
            };
        }
        return None;
    }
    // Mirrors `return "standalone" if cls._starts_with_summary_prefix(text) else None` (l.5326)
    if starts_with_summary_prefix(trimmed) {
        Some("standalone")
    } else {
        None
    }
}

#[allow(dead_code)]
fn _classify_summary_content(content: &Value) -> Option<&'static str> {
    classify_summary_content(content)
}

/// Mirrors `def _is_context_summary_content(cls, content: Any) -> bool:` (ll.5329-5330).
fn is_context_summary_content(content: &Value) -> bool {
    // Mirrors `return cls.classify_summary_content(content) is not None` (l.5330)
    classify_summary_content(content).is_some()
}

#[allow(dead_code)]
fn _is_context_summary_content(content: &Value) -> bool {
    is_context_summary_content(content)
}

/// Python `bool(value)` truthiness for `serde_json::Value`.
fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i != 0
            } else if let Some(f) = n.as_f64() {
                f != 0.0
            } else {
                // u64
                n.as_u64().map(|u| u != 0).unwrap_or(false)
            }
        }
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Mirrors `def _has_compressed_summary_metadata(message: Any) -> bool:` (ll.5333-5343).
fn has_compressed_summary_metadata(message: &Value) -> bool {
    // Mirrors `if not isinstance(message, dict): return False` (ll.5341-5342)
    let Some(map) = message.as_object() else {
        return false;
    };
    // Mirrors `return bool(message.get(COMPRESSED_SUMMARY_METADATA_KEY))` (l.5343)
    if let Some(v) = map.get(COMPRESSED_SUMMARY_METADATA_KEY) {
        is_truthy(v)
    } else {
        false
    }
}

#[allow(dead_code)]
fn _has_compressed_summary_metadata(message: &Value) -> bool {
    has_compressed_summary_metadata(message)
}

// ---------------------------------------------------------------------------
// is_compaction_summary_message — mirrors ll.8083-8104
// ---------------------------------------------------------------------------

/// Return True when *message* is a context-compaction handoff summary.
///
/// Mirrors `def is_compaction_summary_message(message: Any) -> bool:` (ll.8083-8104):
/// ```python
/// def is_compaction_summary_message(message: Any) -> bool:
///     if isinstance(message, dict):
///         if message.get(COMPRESSED_SUMMARY_METADATA_KEY):
///             return True
///         content = message.get("content")
///     else:
///         content = message
///     return ContextCompressor._is_context_summary_content(content)
/// ```
pub fn is_compaction_summary_message(message: &Value) -> bool {
    // Mirrors `if isinstance(message, dict): if message.get(COMPRESSED_SUMMARY_METADATA_KEY): return True` (ll.8097-8099)
    if let Some(map) = message.as_object() {
        if let Some(v) = map.get(COMPRESSED_SUMMARY_METADATA_KEY) {
            if is_truthy(v) {
                return true;
            }
        }
        // Mirrors `content = message.get("content")` (l.8100)
        let content = map.get("content").unwrap_or(&Value::Null);
        // Mirrors `return ContextCompressor._is_context_summary_content(content)` (l.8103)
        return is_context_summary_content(content);
    }
    // Mirrors `else: content = message` + `return ...` (ll.8101-8103)
    is_context_summary_content(message)
}

#[allow(dead_code)]
fn _is_compaction_summary_message(message: &Value) -> bool {
    is_compaction_summary_message(message)
}

// ---------------------------------------------------------------------------
// _strip_context_summary_handoff_message — mirrors ll.5640-5756
// ---------------------------------------------------------------------------

/// Drop stale handoff data while preserving merged prior-tail content.
///
/// Mirrors `def _strip_context_summary_handoff_message(cls, message: Dict[str, Any]) -> Optional[Dict[str, Any]]:` (ll.5640-5756):
/// ```python
/// @classmethod
/// def _strip_context_summary_handoff_message(
///     cls,
///     message: Dict[str, Any],
/// ) -> Optional[Dict[str, Any]]:
///     if not isinstance(message, dict):
///         return message
///     content = message.get("content")
///     is_summary = (
///         cls._is_context_summary_content(content)
///         or cls._has_compressed_summary_metadata(message)
///     )
///     if not is_summary:
///         return message.copy()
///     ...
///     return None
/// ```
pub fn strip_context_summary_handoff_message(message: &Value) -> Option<Value> {
    // Mirrors `if not isinstance(message, dict): return message` (ll.5645-5646)
    let Some(map) = message.as_object() else {
        return Some(message.clone());
    };

    // Mirrors `content = message.get("content")` + `is_summary = (...)` (ll.5648-5652)
    let content = map.get("content").unwrap_or(&Value::Null);
    let is_summary =
        is_context_summary_content(content) || has_compressed_summary_metadata(message);
    // Mirrors `if not is_summary: return message.copy()` (ll.5653-5654)
    if !is_summary {
        return Some(message.clone());
    }

    // Mirrors `if isinstance(content, str):` (l.5656)
    if let Some(s) = content.as_str() {
        // Mirrors `if _MERGED_SUMMARY_DELIMITER in content:` (l.5657)
        if s.contains(MERGED_SUMMARY_DELIMITER) {
            // Mirrors `prior = content.split(_MERGED_SUMMARY_DELIMITER, 1)[0].strip()` (l.5658)
            let prior = s.splitn(2, MERGED_SUMMARY_DELIMITER).next().unwrap_or("").trim();
            // Mirrors `if prior.startswith(_MERGED_PRIOR_CONTEXT_HEADER): prior = prior[len(...):].lstrip()` (ll.5659-5660)
            let mut prior_owned = prior.to_string();
            if prior_owned.starts_with(MERGED_PRIOR_CONTEXT_HEADER) {
                prior_owned = prior_owned[MERGED_PRIOR_CONTEXT_HEADER.len()..]
                    .trim_start()
                    .to_string();
            }
            // Mirrors `if prior: unwrapped = message.copy(); unwrapped["content"] = prior; ... return unwrapped` (ll.5661-5665)
            if !prior_owned.is_empty() {
                let mut unwrapped = message.clone();
                if let Some(obj) = unwrapped.as_object_mut() {
                    obj.insert("content".to_string(), Value::String(prior_owned));
                    obj.remove(COMPRESSED_SUMMARY_METADATA_KEY);
                }
                return Some(unwrapped);
            }
        } else {
            // Mirrors `marker_idx = content.find(_SUMMARY_END_MARKER)` (l.5667)
            if let Some(idx) = s.find(SUMMARY_END_MARKER) {
                // Mirrors `remainder = content[marker_idx + len(...):].lstrip()` (l.5669)
                let remainder = s[idx + SUMMARY_END_MARKER.len()..].trim_start().to_string();
                // Mirrors `if remainder: unwrapped = message.copy(); ... return unwrapped` (ll.5670-5674)
                if !remainder.is_empty() {
                    let mut unwrapped = message.clone();
                    if let Some(obj) = unwrapped.as_object_mut() {
                        obj.insert("content".to_string(), Value::String(remainder));
                        obj.remove(COMPRESSED_SUMMARY_METADATA_KEY);
                    }
                    return Some(unwrapped);
                }
            }
        }
    }

    // Mirrors `if isinstance(content, list):` (l.5676)
    if let Some(arr) = content.as_array() {
        let mut prior_blocks: Vec<Value> = Vec::new();
        let mut found_delimiter = false;

        // Mirrors `for item in content:` (l.5679) — scan for merged delimiter
        for item in arr {
            if let Some(s) = item.as_str() {
                // Mirrors `if isinstance(item, str): if _MERGED_SUMMARY_DELIMITER in item:` (ll.5680-5681)
                if s.contains(MERGED_SUMMARY_DELIMITER) {
                    // Mirrors `before = item.split(_MERGED_SUMMARY_DELIMITER, 1)[0]` (l.5682)
                    let before = s.splitn(2, MERGED_SUMMARY_DELIMITER).next().unwrap_or("");
                    // Mirrors `if before.strip(): prior_blocks.append(before)` (ll.5683-5684)
                    if !before.trim().is_empty() {
                        prior_blocks.push(Value::String(before.to_string()));
                    }
                    found_delimiter = true;
                    break;
                }
                prior_blocks.push(item.clone());
                continue;
            }
            if let Some(obj) = item.as_object() {
                // Mirrors `if isinstance(item, dict): text = item.get("text")` (ll.5689-5690)
                if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                    if text.contains(MERGED_SUMMARY_DELIMITER) {
                        // Mirrors `before = text.split(..., 1)[0]` (l.5692)
                        let before = text.splitn(2, MERGED_SUMMARY_DELIMITER).next().unwrap_or("");
                        if !before.trim().is_empty() {
                            let mut copied = obj.clone();
                            copied.insert("text".to_string(), Value::String(before.to_string()));
                            prior_blocks.push(Value::Object(copied));
                        }
                        found_delimiter = true;
                        break;
                    }
                }
                // Mirrors `prior_blocks.append(item.copy())` (l.5699)
                prior_blocks.push(item.clone());
                continue;
            }
            prior_blocks.push(item.clone());
        }

        if !found_delimiter {
            // Mirrors `if not found_delimiter: legacy_blocks...` (ll.5703-5726)
            let mut legacy_blocks: Vec<Value> = Vec::new();
            let mut found_marker = false;
            for (index, item) in arr.iter().enumerate() {
                // Mirrors `text = item if isinstance(item, str) else item.get("text") if isinstance(item, dict) else None` (l.5707)
                let text_opt: Option<String> = if let Some(s) = item.as_str() {
                    Some(s.to_string())
                } else if let Some(obj) = item.as_object() {
                    obj.get("text").and_then(|v| v.as_str()).map(|s| s.to_string())
                } else {
                    None
                };
                // Mirrors `if not isinstance(text, str) or _SUMMARY_END_MARKER not in text: continue` (ll.5708-5709)
                let Some(text) = text_opt else { continue };
                if !text.contains(SUMMARY_END_MARKER) {
                    continue;
                }
                // Mirrors `remainder = text.split(_SUMMARY_END_MARKER, 1)[1].lstrip()` (l.5710)
                let remainder = text
                    .splitn(2, SUMMARY_END_MARKER)
                    .nth(1)
                    .unwrap_or("")
                    .trim_start()
                    .to_string();
                // Mirrors `if remainder: ... legacy_blocks.append(...)` (ll.5711-5717)
                if !remainder.is_empty() {
                    if let Some(obj) = item.as_object() {
                        let mut copied = obj.clone();
                        copied.insert("text".to_string(), Value::String(remainder));
                        legacy_blocks.push(Value::Object(copied));
                    } else {
                        legacy_blocks.push(Value::String(remainder));
                    }
                }
                // Mirrors `for later in content[index + 1:]: legacy_blocks.append(later.copy() if isinstance(later, dict) else later)` (ll.5718-5719)
                for later in arr.iter().skip(index + 1) {
                    legacy_blocks.push(later.clone());
                }
                found_marker = true;
                break;
            }
            // Mirrors `if found_marker and legacy_blocks: unwrapped = message.copy(); ... return unwrapped` (ll.5722-5726)
            if found_marker && !legacy_blocks.is_empty() {
                let mut unwrapped = message.clone();
                if let Some(obj) = unwrapped.as_object_mut() {
                    obj.insert("content".to_string(), Value::Array(legacy_blocks));
                    obj.remove(COMPRESSED_SUMMARY_METADATA_KEY);
                }
                return Some(unwrapped);
            }
        }

        if found_delimiter {
            // Mirrors `if found_delimiter: for index, item in enumerate(prior_blocks): ...` (ll.5728-5748)
            for (index, item) in prior_blocks.clone().iter().enumerate() {
                if let Some(s) = item.as_str() {
                    // Mirrors `if isinstance(item, str): if item.lstrip().startswith(_MERGED_PRIOR_CONTEXT_HEADER):` (ll.5730-5731)
                    if s.trim_start().starts_with(MERGED_PRIOR_CONTEXT_HEADER) {
                        let leading = s.trim_start()[MERGED_PRIOR_CONTEXT_HEADER.len()..]
                            .trim_start()
                            .to_string();
                        if !leading.is_empty() {
                            prior_blocks[index] = Value::String(leading);
                        } else {
                            prior_blocks.remove(index);
                        }
                        break;
                    }
                } else if let Some(obj) = item.as_object() {
                    if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                        // Mirrors `elif isinstance(item, dict) and isinstance(item.get("text"), str): if text.lstrip().startswith(...):` (ll.5738-5740)
                        if text.trim_start().starts_with(MERGED_PRIOR_CONTEXT_HEADER) {
                            let leading = text.trim_start()[MERGED_PRIOR_CONTEXT_HEADER.len()..]
                                .trim_start()
                                .to_string();
                            if !leading.is_empty() {
                                let mut copied = obj.clone();
                                copied.insert("text".to_string(), Value::String(leading));
                                prior_blocks[index] = Value::Object(copied);
                            } else {
                                prior_blocks.remove(index);
                            }
                            break;
                        }
                    }
                }
            }

            // Mirrors `if prior_blocks: unwrapped = message.copy(); ... return unwrapped` (ll.5750-5754)
            if !prior_blocks.is_empty() {
                let mut unwrapped = message.clone();
                if let Some(obj) = unwrapped.as_object_mut() {
                    obj.insert("content".to_string(), Value::Array(prior_blocks));
                    obj.remove(COMPRESSED_SUMMARY_METADATA_KEY);
                }
                return Some(unwrapped);
            }
        }
    }

    // Mirrors `return None` (l.5756)
    None
}

#[allow(dead_code)]
fn _strip_context_summary_handoff_message(message: &Value) -> Option<Value> {
    strip_context_summary_handoff_message(message)
}

// ---------------------------------------------------------------------------
// project_compaction_message_for_display — mirrors lines 24-47
// ---------------------------------------------------------------------------

/// Return authentic transcript content, or `None` for a pure handoff.
///
/// Model-facing recovery history retains the complete carrier. Display
/// projections instead remove the handoff, inherited tool state, and internal
/// reasoning while preserving any real prior-tail content or live user ask
/// embedded in the carrier.
///
/// Mirrors `def project_compaction_message_for_display(message: Dict[str, Any]) -> Optional[Dict[str, Any]]:` (lines 24-47):
/// ```python
/// def project_compaction_message_for_display(
///     message: Dict[str, Any],
/// ) -> Optional[Dict[str, Any]]:
///     if not isinstance(message, dict):
///         return None
///     if not is_compaction_summary_message(message):
///         return message.copy()
///     projected = ContextCompressor._strip_context_summary_handoff_message(message)
///     if projected is None:
///         return None
///     projected = projected.copy()
///     for key in _COMPACTION_INTERNAL_FIELDS:
///         projected.pop(key, None)
///     projected.pop("display_kind", None)
///     return projected
/// ```
pub fn project_compaction_message_for_display(message: &Value) -> Option<Value> {
    // Mirrors `if not isinstance(message, dict): return None` (lines 34-35)
    if !message.is_object() {
        return None;
    }
    // Mirrors `if not is_compaction_summary_message(message): return message.copy()` (lines 36-37)
    if !is_compaction_summary_message(message) {
        return Some(message.clone());
    }

    // Mirrors `projected = ContextCompressor._strip_context_summary_handoff_message(message)` (line 39)
    let projected = strip_context_summary_handoff_message(message)?;
    // Mirrors `if projected is None: return None` (lines 40-41)
    // `?` above already returns None if strip returned None.

    // Mirrors `projected = projected.copy()` (line 43) — clone for mutation
    let mut out = projected.clone();
    // Mirrors `for key in _COMPACTION_INTERNAL_FIELDS: projected.pop(key, None)` (lines 44-45)
    if let Some(obj) = out.as_object_mut() {
        for key in COMPACTION_INTERNAL_FIELDS {
            obj.remove(*key);
        }
        // Mirrors `projected.pop("display_kind", None)` (line 46)
        obj.remove("display_kind");
    }
    // Mirrors `return projected` (line 47)
    Some(out)
}

#[allow(dead_code)]
fn _project_compaction_message_for_display(message: &Value) -> Option<Value> {
    project_compaction_message_for_display(message)
}
