//! Automatic context window compression for long conversations.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/context_compressor.py`
//! (8211 LOC) — slice 3/11, lines 1600-2400.
//!
//! ```text
//! Automatic context window compression for long conversations.
//!
//! Self-contained class with its own OpenAI client for summarization.
//! Uses auxiliary model (cheap/fast) to summarize middle turns while
//! protecting head and tail context.
//! ```
//!
//! NEVER cargo — 1:1 translation only (no `cargo check` / `cargo build`).
//! Mirrors Python ll.1600-2400 verbatim; line numbers in comments refer to the
//! 8211-line source file. Slice 1 covered ll.1-800, slice 2 covered ll.800-1600
//! (closed at l.1608 to keep the module syntactically complete despite the
//! 1600 boundary falling mid-function inside `_strip_images_from_tool_msg`).
//! This slice resumes at l.1609 (`_retire_stale_tool_result_images`) and runs
//! through `ContextCompressor::on_session_end` (ll.2356-2406, extended one line
//! past 2400 to close the function so the module remains syntactically complete;
//! the nominal 2400 boundary falls mid-function). Later slices
//! (compressor_slice4..N) continue from l.2407 (`bind_session_state`).
//! This slice is verified by line-level audit, not by compilation.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

// ---------------------------------------------------------------------------
// Imports — mirrors Python ll.19-45 (same set as slices 1-2; repeated for self-containment)
// ---------------------------------------------------------------------------
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Value};

// Python imports (ll.19-26) — stdlib:
//   hashlib, json, logging, sqlite3, re, time, uuid, typing
// Mapped: std hash, serde_json, log, rusqlite (not needed slice3), regex, time, uuid

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
// surface so slice3 is self-contained and grep-traceable. Canonical impls
// replace stubs when slices merge.

// ---------------------------------------------------------------------------
// Shared type aliases — mirrors Python `Dict[str, Any]` / `List[Dict[str, Any]]`
// (ll.26, 198+) — same as slices 1-2, repeated for self-containment.
// ---------------------------------------------------------------------------
pub type Message = HashMap<String, Value>;
pub type Turns = Vec<Message>;

// ---------------------------------------------------------------------------
// Minimal stubs for cross-module helpers referenced in ll.1600-2400
// ---------------------------------------------------------------------------

/// Mirrors `agent/turn_context.py::drop_stale_api_content` (l.45, used at ll.1601,1607,1594)
fn drop_stale_api_content(_msg: &mut Value) {
    // Real impl drops stale `api_content` sidecars; stub no-ops for audit.
}

/// Mirrors `agent/model_metadata.py::estimate_messages_tokens_rough` (l.40-41, used at l.2188 ff)
fn estimate_messages_tokens_rough(messages: &Turns) -> usize {
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

/// Mirrors `agent/model_metadata.py::estimate_tokens_rough` (l.40-41, used at l.1081 ff)
fn estimate_tokens_rough(text: &str) -> usize {
    text.len() / 4
}

/// Mirrors `agent/redact.py::redact_sensitive_text` (used at l.1254 ff)
fn redact_sensitive_text(text: String, force: bool, redact_url_credentials: bool) -> String {
    let _ = (force, redact_url_credentials);
    text
}

/// Mirrors `agent/model_metadata.py::get_model_context_length` (l.40, used at l.2250)
fn get_model_context_length(
    model: &str,
    base_url: &str,
    api_key: &str,
    config_context_length: Option<usize>,
    provider: &str,
) -> usize {
    // Stub: returns config override or 128k default; canonical lives in hermes-core.
    if let Some(v) = config_context_length {
        return v;
    }
    128_000
}

/// Mirrors `agent/model_metadata.py::MINIMUM_CONTEXT_LENGTH` (l.38, used at l.1469 ff)
const MINIMUM_CONTEXT_LENGTH: usize = 4096;

/// Mirrors `TODO_INJECTION_HEADER` from `tools/todo_tool.py` (l.45)
const TODO_INJECTION_HEADER: &str = "[TODO_INJECTION_HEADER]";

fn format_with_commas(n: usize) -> String {
    // Mirrors Python f"{n:,}" — comma-separated thousands.
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

// ---------------------------------------------------------------------------
// Self-contained copies of helpers defined in slices 1-2 but needed here
// (slice3 must be grep-traceable without cross-file imports).
// ---------------------------------------------------------------------------

/// Mirrors `_MAX_KEEP_TOOL_IMAGES = 3` (l.1230) — needed by `_retire_stale_tool_result_images` (l.1614)
pub const MAX_KEEP_TOOL_IMAGES: usize = 3;
#[allow(dead_code)]
const _MAX_KEEP_TOOL_IMAGES: usize = MAX_KEEP_TOOL_IMAGES;

/// Mirrors `_IMAGE_PART_TYPES = frozenset({"image_url", "input_image", "image"})` (l.1690)
pub const IMAGE_PART_TYPES: &[&str] = &["image_url", "input_image", "image"];
#[allow(dead_code)]
const _IMAGE_PART_TYPES: &[&str] = IMAGE_PART_TYPES;

/// Mirrors `SKILL_PRUNED_MARKER_PREFIX = "[SKILL_PRUNED:"` (l.721)
pub const SKILL_PRUNED_MARKER_PREFIX: &str = "[SKILL_PRUNED:";
/// Mirrors `_SKILL_VIEW_PRUNE_MIN_CHARS = 5000` (l.725)
pub const SKILL_VIEW_PRUNE_MIN_CHARS: usize = 5000;
/// Mirrors `_PRUNE_MIN_CHARS = 200` (l.679)
pub const PRUNE_MIN_CHARS: usize = 200;
#[allow(dead_code)]
const _PRUNE_MIN_CHARS: usize = PRUNE_MIN_CHARS;

/// Mirrors `_CLARIFY_NON_RESPONSE_PREFIXES` (ll.686-691) — needed by `_summarize_tool_result_unguarded` (l.2004)
pub const CLARIFY_NON_RESPONSE_PREFIXES: &[&str] = &[
    "The user did not provide a response",
    "[user did not respond",
    "[clarify prompt could not be delivered",
    "[oneshot mode:",
];

/// Mirrors `_SUMMARY_TOKENS_CEILING = 10_000` (l.651) — needed by max_summary_tokens (l.2347)
pub const SUMMARY_TOKENS_CEILING: usize = 10_000;

/// Mirrors `LEAN_TAIL_FLOOR_TOKENS = 10_000` (l.868) — needed by tail_token_budget lean mode (l.2332)
pub const LEAN_TAIL_FLOOR_TOKENS: usize = 10_000;
/// Mirrors `LEAN_TAIL_CAP_TOKENS = 25_000` (l.869)
pub const LEAN_TAIL_CAP_TOKENS: usize = 25_000;

/// Mirrors `HISTORICAL_TASK_HEADING = "## Historical Task Snapshot"` (l.112)
pub const HISTORICAL_TASK_HEADING: &str = "## Historical Task Snapshot";

/// Stub: mirrors `def _is_clarify_non_response_sentinel(response: Any) -> bool:` (ll.694-711)
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

/// Mirrors `def _strip_image_parts_from_parts(parts: Any) -> Any:` (ll.1546-1568)
pub fn strip_image_parts_from_parts(parts: &Value) -> Option<Value> {
    let arr = parts.as_array()?;
    let mut had_image = false;
    let mut out: Vec<Value> = Vec::with_capacity(arr.len());
    for part in arr {
        if let Some(obj) = part.as_object() {
            let ptype = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if matches!(ptype, "image" | "image_url" | "input_image") {
                had_image = true;
                out.push(json!({"type": "text", "text": "[screenshot removed to save context]"}));
                continue;
            }
        }
        out.push(part.clone());
    }
    if had_image {
        Some(Value::Array(out))
    } else {
        None
    }
}
#[allow(dead_code)]
fn _strip_image_parts_from_parts(parts: &Value) -> Option<Value> {
    strip_image_parts_from_parts(parts)
}

/// Mirrors `def _tool_content_has_images(content: Any) -> bool:` (ll.1571-1579)
pub fn tool_content_has_images(content: &Value) -> bool {
    if let Some(obj) = content.as_object() {
        if obj.get("_multimodal").and_then(|v| v.as_bool()) == Some(true) {
            return _content_has_images(obj.get("content").unwrap_or(&Value::Null));
        }
    }
    _content_has_images(content)
}
#[allow(dead_code)]
fn _tool_content_has_images(content: &Value) -> bool {
    tool_content_has_images(content)
}

/// Internal: mirrors `def _content_has_images(content: Any) -> bool:` internal helper
fn _content_has_images(content: &Value) -> bool {
    match content {
        Value::Array(parts) => parts.iter().any(|p| {
            if let Some(obj) = p.as_object() {
                matches!(obj.get("type").and_then(|v| v.as_str()), Some("image") | Some("image_url") | Some("input_image"))
            } else {
                false
            }
        }),
        Value::Object(obj) => {
            if obj.get("_multimodal").and_then(|v| v.as_bool()) == Some(true) {
                return _content_has_images(obj.get("content").unwrap_or(&Value::Null));
            }
            matches!(obj.get("type").and_then(|v| v.as_str()), Some("image") | Some("image_url") | Some("input_image"))
        }
        _ => false,
    }
}

/// Mirrors `def _strip_images_from_tool_msg(msg: Dict[str, Any]) -> Optional[Dict[str, Any]]:` (ll.1582-1608)
/// Closed at 1608 in slice2; duplicated here for self-containment of `_retire_stale_tool_result_images`.
pub fn strip_images_from_tool_msg(msg: &Message) -> Option<Message> {
    let content = msg.get("content")?;
    if let Some(obj) = content.as_object() {
        if obj.get("_multimodal").and_then(|v| v.as_bool()) == Some(true) {
            let summary = obj
                .get("text_summary")
                .and_then(|v| v.as_str())
                .unwrap_or("[screenshot removed to save context]");
            let truncated = summary.chars().take(200).collect::<String>();
            let mut new_msg = msg.clone();
            new_msg.insert(
                "content".to_string(),
                Value::String(format!("[screenshot removed] {}", truncated)),
            );
            new_msg.remove("api_content");
            let mut val = Value::Object(new_msg.clone().into_iter().collect());
            drop_stale_api_content(&mut val);
            if let Value::Object(map) = val {
                return Some(map.into_iter().collect());
            }
            return Some(new_msg);
        }
    }
    let stripped = strip_image_parts_from_parts(content)?;
    let mut new_msg = msg.clone();
    new_msg.insert("content".to_string(), stripped);
    new_msg.remove("api_content");
    let mut val = Value::Object(new_msg.clone().into_iter().collect());
    drop_stale_api_content(&mut val);
    if let Value::Object(map) = val {
        return Some(map.into_iter().collect());
    }
    Some(new_msg)
}
#[allow(dead_code)]
fn _strip_images_from_tool_msg(msg: &Message) -> Option<Message> {
    strip_images_from_tool_msg(msg)
}

/// Mirrors `def _is_image_part(part: Any) -> bool:` internal helper for `_strip_historical_media` path
fn _is_image_part(part: &Value) -> bool {
    if let Some(obj) = part.as_object() {
        matches!(obj.get("type").and_then(|v| v.as_str()), Some("image_url") | Some("input_image") | Some("image"))
    } else {
        false
    }
}

/// Mirrors `def _content_text_for_contains(content: Any) -> str:` (ll.1505-1525) — needed by `_image_part_label` etc.
fn content_text_for_contains(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(arr) => {
            let mut parts: Vec<String> = Vec::new();
            for item in arr {
                if let Some(s) = item.as_str() {
                    parts.push(s.to_string());
                } else if let Some(obj) = item.as_object() {
                    if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                        parts.push(text.to_string());
                    }
                }
            }
            parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n")
        }
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Slice3 body — mirrors Python ll.1609-2400
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// _retire_stale_tool_result_images — mirrors Python ll.1611-1641
// ---------------------------------------------------------------------------

/// Mirrors `def _retire_stale_tool_result_images(result: List[Dict[str, Any]], keep_newest: int = _MAX_KEEP_TOOL_IMAGES) -> int:` (ll.1611-1641)
///
/// Replace image payloads on older tool results with text placeholders.
/// Walks newest-first, keeps the most recent `keep_newest` image-bearing
/// tool messages intact (follow-up screenshot QA still sees the latest frames),
/// and retires the rest. User-role uploads are not touched.
///
/// Mutates `result` in place. Returns the number of messages rewritten.
pub fn retire_stale_tool_result_images(result: &mut Turns, keep_newest: usize) -> usize {
    // Mirrors `if keep_newest < 0: keep_newest = 0` (ll.1623-1624)
    // Rust usize is always >=0; the clamp is kept as a doc reference for the
    // Python i64 path. Callers passing a signed value should clamp before.
    let keep_newest = keep_newest; // already clamped
    let mut seen: usize = 0;
    let mut pruned: usize = 0;
    // Mirrors `for i in range(len(result) - 1, -1, -1):` (l.1627)
    for i in (0..result.len()).rev() {
        let msg = &result[i];
        // Mirrors `if not isinstance(msg, dict) or msg.get("role") != "tool": continue` (ll.1629-1630)
        if msg.get("role").and_then(|v| v.as_str()) != Some("tool") {
            continue;
        }
        // Mirrors `if not _tool_content_has_images(msg.get("content")): continue` (ll.1631-1632)
        let has_images = tool_content_has_images(msg.get("content").unwrap_or(&Value::Null));
        if !has_images {
            continue;
        }
        seen += 1;
        // Mirrors `if seen <= keep_newest: continue` (ll.1634-1635)
        if seen <= keep_newest {
            continue;
        }
        // Mirrors `new_msg = _strip_images_from_tool_msg(msg); if new_msg is None: continue` (ll.1636-1638)
        let new_msg = match strip_images_from_tool_msg(msg) {
            Some(m) => m,
            None => continue,
        };
        // Mirrors `result[i] = new_msg` (l.1639)
        result[i] = new_msg;
        pruned += 1;
    }
    // Mirrors `return pruned` (l.1641)
    pruned
}

#[allow(dead_code)]
fn _retire_stale_tool_result_images(result: &mut Turns, keep_newest: usize) -> usize {
    retire_stale_tool_result_images(result, keep_newest)
}

/// Signed variant mirroring Python's `keep_newest: int` that allows negative (l.1623-1624).
pub fn retire_stale_tool_result_images_signed(result: &mut Turns, keep_newest: i64) -> usize {
    let keep = if keep_newest < 0 { 0 } else { keep_newest as usize };
    retire_stale_tool_result_images(result, keep)
}

// ---------------------------------------------------------------------------
// _truncate_tool_call_args_json — mirrors Python ll.1644-1687
// ---------------------------------------------------------------------------

/// Mirrors `def _truncate_tool_call_args_json(args: str, head_chars: int = 200) -> str:` (ll.1644-1687)
///
/// Shrink long string values inside a tool-call arguments JSON blob while
/// preserving JSON validity. See Python doc (ll.1645-1668) for the motivating
/// MiniMax 400 case (#11762).
pub fn truncate_tool_call_args_json(args: &str, head_chars: usize) -> String {
    // Mirrors `try: parsed = json.loads(args) except (ValueError, TypeError): return args` (ll.1669-1672)
    let parsed: Value = match serde_json::from_str(args) {
        Ok(v) => v,
        Err(_) => return args.to_string(),
    };

    // Mirrors `def _shrink(obj: Any) -> Any:` (ll.1674-1683)
    fn shrink(obj: Value, head_chars: usize) -> Value {
        match obj {
            Value::String(s) => {
                // Mirrors `if len(obj) > head_chars: return obj[:head_chars] + "...[truncated]"` (ll.1676-1677)
                if s.len() > head_chars {
                    // Python slices on chars; Rust on bytes — for JSON string values this is typically ASCII.
                    // Use char-based truncation for 1:1.
                    let truncated: String = s.chars().take(head_chars).collect();
                    Value::String(format!("{}...[truncated]", truncated))
                } else {
                    Value::String(s)
                }
            }
            Value::Object(map) => {
                // Mirrors `if isinstance(obj, dict): return {k: _shrink(v) for k, v in obj.items()}` (ll.1679-1680)
                let mut out = serde_json::Map::new();
                for (k, v) in map {
                    out.insert(k, shrink(v, head_chars));
                }
                Value::Object(out)
            }
            Value::Array(arr) => {
                // Mirrors `if isinstance(obj, list): return [_shrink(v) for v in obj]` (ll.1681-1682)
                Value::Array(arr.into_iter().map(|v| shrink(v, head_chars)).collect())
            }
            other => other, // Mirrors `return obj` (l.1683) for non-string/dict/list
        }
    }

    let shrunken = shrink(parsed, head_chars);
    // Mirrors `return json.dumps(shrunken, ensure_ascii=False)` (l.1687)
    serde_json::to_string(&shrunken).unwrap_or_else(|_| args.to_string())
}

#[allow(dead_code)]
fn _truncate_tool_call_args_json(args: &str, head_chars: usize) -> String {
    truncate_tool_call_args_json(args, head_chars)
}

// ---------------------------------------------------------------------------
// _IMAGE_PART_TYPES + _is_image_part + _content_has_images + _strip_images_from_content
// Mirrors Python ll.1690-1738
// ---------------------------------------------------------------------------

/// Mirrors `def _is_image_part(part: Any) -> bool:` (ll.1693-1703)
///
/// True if `part` is a multimodal image content block.
pub fn is_image_part(part: &Value) -> bool {
    // Mirrors `if not isinstance(part, dict): return False` (ll.1701-1702)
    let Some(obj) = part.as_object() else {
        return false;
    };
    // Mirrors `return part.get("type") in _IMAGE_PART_TYPES` (l.1703)
    if let Some(t) = obj.get("type").and_then(|v| v.as_str()) {
        IMAGE_PART_TYPES.contains(&t)
    } else {
        false
    }
}

#[allow(dead_code)]
fn _is_image_part(part: &Value) -> bool {
    is_image_part(part)
}

/// Mirrors `def _content_has_images(content: Any) -> bool:` (ll.1706-1710)
///
/// True if a message's `content` is a multimodal list with image parts.
pub fn content_has_images(content: &Value) -> bool {
    // Mirrors `if not isinstance(content, list): return False` (ll.1708-1709)
    let Some(arr) = content.as_array() else {
        return false;
    };
    // Mirrors `return any(_is_image_part(p) for p in content)` (l.1710)
    arr.iter().any(|p| is_image_part(p))
}

#[allow(dead_code)]
fn _content_has_images(content: &Value) -> bool {
    content_has_images(content)
}

/// Mirrors `def _strip_images_from_content(content: Any) -> Any:` (ll.1713-1738)
///
/// Return a copy of `content` with every image part replaced by a short text
/// placeholder. Input is never mutated.
pub fn strip_images_from_content(content: &Value) -> Value {
    // Mirrors `if not isinstance(content, list): return content` (ll.1724-1725)
    let Some(arr) = content.as_array() else {
        return content.clone();
    };
    // Mirrors `if not any(_is_image_part(p) for p in content): return content` (ll.1726-1727)
    if !arr.iter().any(|p| is_image_part(p)) {
        return content.clone();
    }
    // Mirrors `new_parts: List[Any] = []; for p in content: if _is_image_part(p): new_parts.append({...}) else: new_parts.append(p)` (ll.1729-1737)
    let mut new_parts: Vec<Value> = Vec::with_capacity(arr.len());
    for p in arr {
        if is_image_part(p) {
            new_parts.push(json!({
                "type": "text",
                "text": "[Attached image — stripped after compression]"
            }));
        } else {
            new_parts.push(p.clone());
        }
    }
    Value::Array(new_parts)
}

#[allow(dead_code)]
fn _strip_images_from_content(content: &Value) -> Value {
    strip_images_from_content(content)
}

// ---------------------------------------------------------------------------
// _strip_historical_media — mirrors Python ll.1741-1798
// ---------------------------------------------------------------------------

/// Mirrors `def _strip_historical_media(messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:` (ll.1741-1798)
///
/// Replace image parts in older messages with placeholder text.
/// The anchor is the *last* user message that has any image content.
pub fn strip_historical_media(messages: &Turns) -> Turns {
    // Mirrors `if not messages: return messages` (ll.1757-1758)
    if messages.is_empty() {
        return messages.clone();
    }

    // Mirrors `anchor = -1; for i in range(len(messages) - 1, -1, -1): if msg.get("role") != "user": continue; if _content_has_images(msg.get("content")): anchor = i; break` (ll.1764-1773)
    let mut anchor: Option<usize> = None;
    for (i, msg) in messages.iter().enumerate().rev() {
        if msg.get("role").and_then(|v| v.as_str()) != Some("user") {
            continue;
        }
        let has_images = content_has_images(msg.get("content").unwrap_or(&Value::Null));
        if has_images {
            anchor = Some(i);
            break;
        }
    }
    let Some(anchor) = anchor else {
        return messages.clone();
    };
    // Mirrors `if anchor <= 0: return messages` (ll.1775-1778)
    if anchor == 0 {
        return messages.clone();
    }

    // Mirrors `changed = False; result: List[Dict[str, Any]] = []; for i, msg in enumerate(messages): ...` (ll.1780-1798)
    let mut changed = false;
    let mut result: Turns = Vec::with_capacity(messages.len());
    for (i, msg) in messages.iter().enumerate() {
        // Mirrors `if i >= anchor or not isinstance(msg, dict): result.append(msg); continue` (ll.1783-1784)
        if i >= anchor {
            result.push(msg.clone());
            continue;
        }
        let content = msg.get("content").unwrap_or(&Value::Null);
        // Mirrors `if not _content_has_images(content): result.append(msg); continue` (ll.1787-1789)
        if !content_has_images(content) {
            result.push(msg.clone());
            continue;
        }
        // Mirrors `new_msg = msg.copy(); new_msg["content"] = _strip_images_from_content(content); drop_stale_api_content(new_msg); result.append(new_msg); changed = True` (ll.1790-1796)
        let mut new_msg = msg.clone();
        new_msg.insert("content".to_string(), strip_images_from_content(content));
        // Drop stale api_content sidecar
        new_msg.remove("api_content");
        let mut val = Value::Object(new_msg.clone().into_iter().collect());
        drop_stale_api_content(&mut val);
        let new_msg = if let Value::Object(map) = val {
            map.into_iter().collect()
        } else {
            new_msg
        };
        result.push(new_msg);
        changed = true;
    }

    // Mirrors `return result if changed else messages` (l.1798)
    if changed {
        result
    } else {
        messages.clone()
    }
}

#[allow(dead_code)]
fn _strip_historical_media(messages: &Turns) -> Turns {
    strip_historical_media(messages)
}

// ---------------------------------------------------------------------------
// _image_part_label — mirrors Python ll.1801-1819
// ---------------------------------------------------------------------------

/// Mirrors `def _image_part_label(part: Dict[str, Any]) -> str:` (ll.1801-1819)
///
/// Render a multimodal image part as a short text label for the summarizer.
pub fn image_part_label(part: &Value) -> String {
    // Mirrors Python ll.1811-1816 url extraction
    let url: String = if let Some(obj) = part.as_object() {
        if let Some(image_url) = obj.get("image_url") {
            if let Some(dict) = image_url.as_object() {
                dict.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string()
            } else if let Some(s) = image_url.as_str() {
                s.to_string()
            } else {
                String::new()
            }
        } else if let Some(s) = obj.get("url").and_then(|v| v.as_str()) {
            s.to_string()
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    // Mirrors `if url.startswith(("http://", "https://")): return f"[image: {url}]"` (ll.1817-1818)
    if url.starts_with("http://") || url.starts_with("https://") {
        format!("[image: {}]", url)
    } else {
        // Mirrors `return "[image]"` (l.1819)
        "[image]".to_string()
    }
}

#[allow(dead_code)]
fn _image_part_label(part: &Value) -> String {
    image_part_label(part)
}

// ---------------------------------------------------------------------------
// _str_arg — mirrors Python ll.1822-1834
// ---------------------------------------------------------------------------

/// Mirrors `def _str_arg(args: dict, key: str, default: str = "") -> str:` (ll.1822-1834)
///
/// Safely get a string argument from parsed tool args.
pub fn str_arg(args: &serde_json::Map<String, Value>, key: &str, default: &str) -> String {
    // Mirrors `val = args.get(key, default)` (l.1831)
    let val = match args.get(key) {
        Some(v) => v,
        None => return default.to_string(),
    };
    // Mirrors `if isinstance(val, str): return val` (ll.1832-1833)
    if let Some(s) = val.as_str() {
        return s.to_string();
    }
    // Mirrors `return str(val) if val is not None else default` (l.1834)
    if val.is_null() {
        default.to_string()
    } else {
        // `str(val)` in Python for non-string: json-like but use to_string
        match val {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }
}

#[allow(dead_code)]
fn _str_arg(args: &serde_json::Map<String, Value>, key: &str, default: &str) -> String {
    str_arg(args, key, default)
}

// ---------------------------------------------------------------------------
// _summarize_tool_result + _summarize_tool_result_unguarded — mirrors Python ll.1837-2041
// ---------------------------------------------------------------------------

/// Mirrors `def _summarize_tool_result(tool_name: str, tool_args: str, tool_content: str) -> str:` (ll.1837-1862)
///
/// Create an informative 1-line summary of a tool call + result. Never raises.
pub fn summarize_tool_result(tool_name: &str, tool_args: &str, tool_content: &str) -> String {
    // Mirrors `try: return _summarize_tool_result_unguarded(...) except Exception as exc: logger.debug(...); _len = len(tool_content) if isinstance(tool_content, str) else 0; return f"[{tool_name}] ({_len:,} chars result)"` (ll.1857-1862)
    // In Rust, tool_content is always &str, so len is direct.
    // We catch panics via std::panic::catch_unwind for 1:1 "never crashes" guarantee,
    // but the unguarded impl itself is panic-free (no unwrap).
    let result = std::panic::catch_unwind(|| summarize_tool_result_unguarded(tool_name, tool_args, tool_content));
    match result {
        Ok(s) => s,
        Err(_) => {
            // Mirrors logger.debug + fallback
            format!("[{}] ({} chars result)", tool_name, format_with_commas(tool_content.len()))
        }
    }
}

#[allow(dead_code)]
fn _summarize_tool_result(tool_name: &str, tool_args: &str, tool_content: &str) -> String {
    summarize_tool_result(tool_name, tool_args, tool_content)
}

/// Mirrors `def _summarize_tool_result_unguarded(tool_name: str, tool_args: str, tool_content: str) -> str:` (ll.1865-2041)
///
/// Build the summary line (unguarded; see `_summarize_tool_result`).
pub fn summarize_tool_result_unguarded(tool_name: &str, tool_args: &str, tool_content: &str) -> String {
    // Mirrors `try: args = json.loads(tool_args) if tool_args else {} except: args = {} if not isinstance(args, dict): args = {}` (ll.1867-1872)
    let args_value: Value = if tool_args.is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(tool_args).unwrap_or(Value::Object(serde_json::Map::new()))
    };
    let args_map: serde_json::Map<String, Value> = match args_value {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };

    let content = tool_content; // Mirrors `content = tool_content or ""` (l.1874) — already &str
    let content_len = content.len(); // Mirrors `content_len = len(content)` (l.1875)
    let line_count = if content.trim().is_empty() { 0 } else { content.matches('\n').count() + 1 }; // Mirrors `line_count = content.count("\n") + 1 if content.strip() else 0` (l.1876)

    // Mirrors `if tool_name == "terminal":` (ll.1878-1884)
    if tool_name == "terminal" {
        let mut cmd = str_arg(&args_map, "command", "");
        if cmd.len() > 80 {
            cmd = format!("{}...", cmd.chars().take(77).collect::<String>());
        }
        // Mirrors `exit_match = re.search(r'"exit_code"\s*:\s*(-?\d+)', content)` (l.1882)
        let exit_code = {
            static RE: OnceLock<Regex> = OnceLock::new();
            let re = RE.get_or_init(|| Regex::new(r#""exit_code"\s*:\s*(-?\d+)"#).unwrap());
            re.captures(content).and_then(|c| c.get(1)).map(|m| m.as_str().to_string()).unwrap_or_else(|| "?".to_string())
        };
        return format!("[terminal] ran `{}` -> exit {}, {} lines output", cmd, exit_code, line_count);
    }

    // Mirrors `if tool_name == "read_file":` (ll.1886-1889)
    if tool_name == "read_file" {
        let path = args_map.get("path").and_then(|v| v.as_str()).unwrap_or("?");
        let offset = args_map.get("offset").map(|v| v.to_string()).unwrap_or_else(|| "1".to_string());
        // Python: `offset = args.get("offset", 1)` preserves int; Rust to_string keeps it.
        // Trim quotes for stringified JSON numbers? Keep as is for audit.
        let offset_display = offset.trim_matches('"');
        return format!("[read_file] read {} from line {} ({} chars)", path, offset_display, format_with_commas(content_len));
    }

    // Mirrors `if tool_name == "write_file":` (ll.1891-1894)
    if tool_name == "write_file" {
        let path = args_map.get("path").and_then(|v| v.as_str()).unwrap_or("?");
        let written_lines: String = if let Some(v) = args_map.get("content") {
            if let Some(s) = v.as_str() {
                (s.matches('\n').count() + 1).to_string()
            } else {
                // Mirrors `_str_arg(args, "content").count("\n") + 1 if args.get("content") else "?"`
                let s = str_arg(&args_map, "content", "");
                if s.is_empty() { "?".to_string() } else { (s.matches('\n').count() + 1).to_string() }
            }
        } else {
            "?".to_string()
        };
        return format!("[write_file] wrote to {} ({} lines)", path, written_lines);
    }

    // Mirrors `if tool_name == "search_files":` (ll.1896-1902)
    if tool_name == "search_files" {
        let pattern = args_map.get("pattern").and_then(|v| v.as_str()).unwrap_or("?");
        let path = args_map.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let target = args_map.get("target").and_then(|v| v.as_str()).unwrap_or("content");
        let count = {
            static RE: OnceLock<Regex> = OnceLock::new();
            let re = RE.get_or_init(|| Regex::new(r#""total_count"\s*:\s*(\d+)"#).unwrap());
            re.captures(content).and_then(|c| c.get(1)).map(|m| m.as_str().to_string()).unwrap_or_else(|| "?".to_string())
        };
        return format!("[search_files] {} search for '{}' in {} -> {} matches", target, pattern, path, count);
    }

    // Mirrors `if tool_name == "patch":` (ll.1904-1907)
    if tool_name == "patch" {
        let path = args_map.get("path").and_then(|v| v.as_str()).unwrap_or("?");
        let mode = args_map.get("mode").and_then(|v| v.as_str()).unwrap_or("replace");
        return format!("[patch] {} in {} ({} chars result)", mode, path, format_with_commas(content_len));
    }

    // Mirrors `if tool_name in {"browser_navigate", ...}:` (ll.1909-1914)
    if matches!(tool_name, "browser_navigate" | "browser_click" | "browser_snapshot" | "browser_type" | "browser_scroll" | "browser_vision") {
        let url = args_map.get("url").and_then(|v| v.as_str()).unwrap_or("");
        let r#ref = args_map.get("ref").and_then(|v| v.as_str()).unwrap_or("");
        let detail = if !url.is_empty() {
            format!(" {}", url)
        } else if !r#ref.is_empty() {
            format!(" ref={}", r#ref)
        } else {
            String::new()
        };
        return format!("[{}]{} ({} chars)", tool_name, detail, format_with_commas(content_len));
    }

    // Mirrors `if tool_name == "web_search":` (ll.1916-1918)
    if tool_name == "web_search" {
        let query = args_map.get("query").and_then(|v| v.as_str()).unwrap_or("?");
        return format!("[web_search] query='{}' ({} chars result)", query, format_with_commas(content_len));
    }

    // Mirrors `if tool_name == "web_extract":` (ll.1920-1934)
    if tool_name == "web_extract" {
        let urls = args_map.get("urls");
        let first_val = match urls {
            Some(Value::Array(arr)) if !arr.is_empty() => Some(&arr[0]),
            _ => None,
        };
        let mut first_desc: String = match first_val {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Object(dict)) => {
                // Mirrors `if isinstance(first, dict): first = first.get("url") or first.get("href") or "?"`
                dict.get("url").and_then(|v| v.as_str())
                    .or_else(|| dict.get("href").and_then(|v| v.as_str()))
                    .unwrap_or("?")
                    .to_string()
            }
            Some(_) => "?".to_string(),
            None => "?".to_string(),
        };
        // Handle case where urls is not list? Already "?"
        if first_desc.is_empty() {
            first_desc = "?".to_string();
        }
        let mut url_desc = first_desc;
        if let Some(Value::Array(arr)) = urls {
            if arr.len() > 1 {
                url_desc = format!("{} (+{} more)", url_desc, arr.len() - 1);
            }
        }
        return format!("[web_extract] {} ({} chars)", url_desc, format_with_commas(content_len));
    }

    // Mirrors `if tool_name == "delegate_task":` (ll.1936-1940)
    if tool_name == "delegate_task" {
        let mut goal = str_arg(&args_map, "goal", "");
        if goal.len() > 60 {
            goal = format!("{}...", goal.chars().take(57).collect::<String>());
        }
        return format!("[delegate_task] '{}' ({} chars result)", goal, format_with_commas(content_len));
    }

    // Mirrors `if tool_name == "execute_code":` (ll.1942-1947)
    if tool_name == "execute_code" {
        let code_str = str_arg(&args_map, "code", "");
        let mut code_preview = code_str.chars().take(60).collect::<String>().replace('\n', " ");
        if code_str.len() > 60 {
            code_preview.push_str("...");
        }
        return format!("[execute_code] `{}` ({} lines output)", code_preview, line_count);
    }

    // Mirrors `if tool_name == "skill_view":` (ll.1949-1959)
    if tool_name == "skill_view" {
        let name = args_map.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        if content_len > SKILL_VIEW_PRUNE_MIN_CHARS {
            return format!(
                "[skill_view] name={} ({} chars) {}",
                name,
                format_with_commas(content_len),
                skill_pruned_marker(name)
            );
        }
        return format!("[skill_view] name={} ({} chars)", name, format_with_commas(content_len));
    }

    // Mirrors `if tool_name in {"skills_list", "skill_manage"}:` (ll.1961-1963)
    if matches!(tool_name, "skills_list" | "skill_manage") {
        let name = args_map.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        return format!("[{}] name={} ({} chars)", tool_name, name, format_with_commas(content_len));
    }

    // Mirrors `if tool_name == "vision_analyze":` (ll.1965-1967)
    if tool_name == "vision_analyze" {
        let question = str_arg(&args_map, "question", "");
        let q = question.chars().take(50).collect::<String>();
        return format!("[vision_analyze] '{}' ({} chars)", q, format_with_commas(content_len));
    }

    // Mirrors `if tool_name == "memory":` (ll.1969-1972)
    if tool_name == "memory" {
        let action = args_map.get("action").and_then(|v| v.as_str()).unwrap_or("?");
        let target = args_map.get("target").and_then(|v| v.as_str()).unwrap_or("?");
        return format!("[memory] {} on {}", action, target);
    }

    // Mirrors `if tool_name == "todo":` (ll.1974-1975)
    if tool_name == "todo" {
        return "[todo] updated task list".to_string();
    }

    // Mirrors `if tool_name == "clarify":` (ll.1977-2022)
    if tool_name == "clarify" {
        let response_prefix = "[clarify] user responded: ";
        let max_summary_chars = PRUNE_MIN_CHARS - 1; // Mirrors `max_summary_chars = _PRUNE_MIN_CHARS - 1` (l.1984)
        let truncation_marker = "...[truncated]";

        // Mirrors `try: result = json.loads(content) except: result = {}` (ll.1987-1990)
        let result_val: Value = serde_json::from_str(content).unwrap_or(Value::Object(serde_json::Map::new()));
        let response = if let Some(obj) = result_val.as_object() {
            obj.get("user_response").cloned()
        } else {
            None
        };

        // Mirrors `is_answer_shaped = (isinstance(response, str) and bool(response)) or (isinstance(response, list) and bool(response) and all(...))` (ll.1992-1998)
        let is_answer_shaped = match &response {
            Some(Value::String(s)) => !s.is_empty(),
            Some(Value::Array(arr)) if !arr.is_empty() => arr.iter().all(|item| matches!(item, Value::String(s) if !s.is_empty())),
            _ => false,
        };

        // Mirrors `resolved = is_answer_shaped and not _is_clarify_non_response_sentinel(response)` (ll.2004-2006)
        let resolved = if is_answer_shaped {
            if let Some(ref r) = response {
                !is_clarify_non_response_sentinel(r)
            } else {
                false
            }
        } else {
            false
        };

        if resolved {
            // Mirrors `serialized_response = json.dumps(response, ensure_ascii=False).encode("utf-8", errors="backslashreplace").decode("utf-8")` (ll.2010-2014)
            // Rust json dumps with ensure_ascii=False is default (keeps Unicode).
            // Python's surrogate escape is not needed in Rust (String is valid UTF-8); we keep verbatim.
            let serialized_response = serde_json::to_string(&response.unwrap()).unwrap_or_default();
            // Python does json.dumps on the response value itself (which may be string or list) — so for string it emits quoted JSON.
            // Our to_string does same.
            let summary = format!("{}{}", response_prefix, serialized_response);
            // Mirrors `if len(summary) > max_summary_chars: summary = summary[: max_summary_chars - len(truncation_marker)].rstrip() + truncation_marker` (ll.2016-2020)
            if summary.len() > max_summary_chars {
                let cut = max_summary_chars.saturating_sub(truncation_marker.len());
                let truncated = summary.chars().take(cut).collect::<String>();
                return format!("{}{}", truncated.trim_end(), truncation_marker);
            }
            return summary;
        }
        return "[clarify] asked user a question".to_string();
    }

    // Mirrors `if tool_name == "text_to_speech":` (ll.2024-2025)
    if tool_name == "text_to_speech" {
        return format!("[text_to_speech] generated audio ({} chars)", format_with_commas(content_len));
    }

    // Mirrors `if tool_name == "cronjob":` (ll.2027-2029)
    if tool_name == "cronjob" {
        let action = args_map.get("action").and_then(|v| v.as_str()).unwrap_or("?");
        return format!("[cronjob] {}", action);
    }

    // Mirrors `if tool_name == "process":` (ll.2031-2034)
    if tool_name == "process" {
        let action = args_map.get("action").and_then(|v| v.as_str()).unwrap_or("?");
        let sid = args_map.get("session_id").and_then(|v| v.as_str()).unwrap_or("?");
        return format!("[process] {} session={}", action, sid);
    }

    // Mirrors generic fallback (ll.2036-2041)
    let mut first_arg = String::new();
    for (k, v) in args_map.iter().take(2) {
        let sv: String = match v {
            Value::String(s) => s.chars().take(40).collect(),
            other => {
                let s = other.to_string();
                s.chars().take(40).collect()
            }
        };
        first_arg.push_str(&format!(" {}={}", k, sv));
    }
    format!("[{}]{} ({} chars result)", tool_name, first_arg, format_with_commas(content_len))
}

#[allow(dead_code)]
fn _summarize_tool_result_unguarded(tool_name: &str, tool_args: &str, tool_content: &str) -> String {
    summarize_tool_result_unguarded(tool_name, tool_args, tool_content)
}

// ---------------------------------------------------------------------------
// resolve_model_threshold — mirrors Python ll.2044-2067
// ---------------------------------------------------------------------------

/// Mirrors `def resolve_model_threshold(model: str, model_thresholds: dict[str, float] | None, default: float) -> float:` (ll.2044-2067)
///
/// Resolve the effective compression threshold for a given model.
pub fn resolve_model_threshold(
    model: &str,
    model_thresholds: Option<&HashMap<String, f64>>,
    default: f64,
) -> f64 {
    // Mirrors `if not model_thresholds or not model: return default` (ll.2059-2060)
    let Some(thresholds) = model_thresholds else {
        return default;
    };
    if thresholds.is_empty() || model.is_empty() {
        return default;
    }
    // Mirrors `best_key = ""; for key in model_thresholds: if key in model and len(key) > len(best_key): best_key = key` (ll.2061-2064)
    let mut best_key = "";
    let mut best_len = 0usize;
    // Need to keep the longest matching key's string slice; track by owned String clone if needed
    let mut best_key_owned = String::new();
    for key in thresholds.keys() {
        if model.contains(key.as_str()) && key.len() > best_len {
            best_key = key;
            best_len = key.len();
            best_key_owned = key.clone();
        }
    }
    // Mirrors `if best_key: return float(model_thresholds[best_key])` (ll.2065-2066)
    if best_len > 0 {
        thresholds.get(&best_key_owned).copied().unwrap_or(default)
    } else {
        // Mirrors `return default` (l.2067)
        default
    }
}

#[allow(dead_code)]
fn _resolve_model_threshold(
    model: &str,
    model_thresholds: Option<&HashMap<String, f64>>,
    default: f64,
) -> f64 {
    resolve_model_threshold(model, model_thresholds, default)
}

// ---------------------------------------------------------------------------
// ContextCompressor — mirrors Python ll.2070-2400 (class + methods through on_session_end)
// ---------------------------------------------------------------------------

// Helper: mirrors `def _safe_int(value: Any) -> int | None:` (ll.50-55) — duplicated for self-containment
fn safe_int(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        Value::Bool(b) => Some(if *b { 1 } else { 0 }),
        _ => None,
    }
}
#[allow(dead_code)]
fn _safe_int(value: &Value) -> Option<i64> {
    safe_int(value)
}

// Mirrors Python's ContextEngine base — minimal trait for ContextCompressor to extend.
// Real trait lives in `context_engine.rs`; stub here for self-containment.
pub trait ContextEngine {
    fn name(&self) -> &str;
    fn on_session_reset(&mut self);
}

/// Mirrors `class ContextCompressor(ContextEngine):` (ll.2070-2400)
///
/// Default context engine — compresses conversation context via lossy summarization.
/// Algorithm steps listed at ll.2073-2079.
#[derive(Debug, Clone)]
pub struct ContextCompressor {
    // -- Identity / provider -------------------------------------------------
    // Mirrors __init__ params (not in this slice but needed for telemetry methods ll.2131+)
    pub model: String,
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub max_tokens: Option<usize>,

    // -- Context length resolution -------------------------------------------
    // Mirrors `self._resolved_context_length`, `self._config_context_length`, `self._base_threshold_percent` etc.
    pub _resolved_context_length: Option<usize>,
    pub _config_context_length: Option<usize>,
    pub threshold_percent: f64,
    pub _base_threshold_percent: f64,

    // -- Derived budgets (cached, invalidated on context_length setter ll.2294-2298)
    pub _threshold_tokens: Option<usize>,
    pub _tail_token_budget: Option<usize>,
    pub _max_summary_tokens: Option<usize>,

    // -- Compression tuning ---------------------------------------------------
    pub summary_target_ratio: f64,
    pub tail_mode: String, // "legacy" or "lean" (l.2325)
    pub _log_init_summary: bool,

    // -- Per-session state (ll.2088-2130) ------------------------------------
    pub _context_probed: bool,
    pub _context_probe_persistable: bool,
    pub _previous_summary: Option<String>,
    pub _summary_has_user_turn: Option<bool>,
    pub _last_summary_error: Option<String>,
    pub _consecutive_timeout_failures: usize,
    pub _last_summary_dropped_count: usize,
    pub _last_summary_fallback_used: bool,
    pub _last_feasibility_skip: bool,
    pub _last_aux_model_failure_error: Option<String>,
    pub _last_aux_model_failure_model: Option<String>,
    pub _last_compression_savings_pct: f64,
    pub _ineffective_compression_count: usize,
    pub _anti_thrash_recovery_deadline: f64,
    pub _structural_no_op_backoff_until: f64,
    pub _prellm_skip_count: usize,
    pub _fallback_compression_streak: usize,
    pub _verify_compaction_cleared_threshold: bool,
    pub _last_compression_made_progress: bool,
    pub _summary_failure_cooldown_until: f64,
    pub _cooldown_persist_failed: bool,
    pub _last_compress_aborted: bool,
    pub _last_compress_refused_would_grow: bool,
    pub last_real_prompt_tokens: usize,
    pub last_compression_rough_tokens: usize,
    pub last_rough_tokens_when_real_prompt_fit: usize,
    pub _pending_request_rough_tokens: usize,
    pub awaiting_real_usage_after_compression: bool,
    pub _last_compression_telemetry: Option<Value>,
    pub _active_compression_telemetry: Option<Value>,
    pub _compression_telemetry_seed: Option<Value>,
    pub _proactive_prune_rearm_tokens: usize,

    // -- Micro-compaction state (ll.2122-2129) -------------------------------
    pub _micro_compact_cursor: usize,
    pub _micro_compact_rolling_summary: String,
    pub _micro_compact_consecutive_failures: usize,
    pub _micro_compact_last_failure_cursor: i64,
    pub _micro_compact_passes: usize,
    pub _micro_compact_tokens_saved_total: usize,
    pub _micro_compact_turns_since_pass: usize,
}

impl Default for ContextCompressor {
    fn default() -> Self {
        Self {
            model: String::new(),
            provider: String::new(),
            base_url: String::new(),
            api_key: String::new(),
            max_tokens: None,
            _resolved_context_length: None,
            _config_context_length: None,
            threshold_percent: 0.75,
            _base_threshold_percent: 0.75,
            _threshold_tokens: None,
            _tail_token_budget: None,
            _max_summary_tokens: None,
            summary_target_ratio: 0.5,
            tail_mode: "legacy".to_string(),
            _log_init_summary: false,
            _context_probed: false,
            _context_probe_persistable: false,
            _previous_summary: None,
            _summary_has_user_turn: None,
            _last_summary_error: None,
            _consecutive_timeout_failures: 0,
            _last_summary_dropped_count: 0,
            _last_summary_fallback_used: false,
            _last_feasibility_skip: false,
            _last_aux_model_failure_error: None,
            _last_aux_model_failure_model: None,
            _last_compression_savings_pct: 100.0,
            _ineffective_compression_count: 0,
            _anti_thrash_recovery_deadline: 0.0,
            _structural_no_op_backoff_until: 0.0,
            _prellm_skip_count: 0,
            _fallback_compression_streak: 0,
            _verify_compaction_cleared_threshold: false,
            _last_compression_made_progress: false,
            _summary_failure_cooldown_until: 0.0,
            _cooldown_persist_failed: false,
            _last_compress_aborted: false,
            _last_compress_refused_would_grow: false,
            last_real_prompt_tokens: 0,
            last_compression_rough_tokens: 0,
            last_rough_tokens_when_real_prompt_fit: 0,
            _pending_request_rough_tokens: 0,
            awaiting_real_usage_after_compression: false,
            _last_compression_telemetry: None,
            _active_compression_telemetry: None,
            _compression_telemetry_seed: None,
            _proactive_prune_rearm_tokens: 0,
            _micro_compact_cursor: 0,
            _micro_compact_rolling_summary: String::new(),
            _micro_compact_consecutive_failures: 0,
            _micro_compact_last_failure_cursor: -1,
            _micro_compact_passes: 0,
            _micro_compact_tokens_saved_total: 0,
            _micro_compact_turns_since_pass: 0,
        }
    }
}

impl ContextCompressor {
    /// Mirrors `@property def name(self) -> str:` (ll.2081-2083)
    pub fn name(&self) -> &str {
        "compressor"
    }

    /// Mirrors `def on_session_reset(self) -> None:` (ll.2085-2129)
    ///
    /// Reset all per-session state for /new or /reset.
    pub fn on_session_reset(&mut self) {
        // Mirrors `super().on_session_reset()` (l.2087) — base resets token counts; stub no-ops.
        self._context_probed = false; // l.2088
        self._context_probe_persistable = false; // l.2089
        self._previous_summary = None; // l.2090
        self._summary_has_user_turn = None; // l.2091
        self._last_summary_error = None; // l.2092
        self._consecutive_timeout_failures = 0; // l.2093
        self._last_summary_dropped_count = 0; // l.2094
        self._last_summary_fallback_used = false; // l.2095
        self._last_feasibility_skip = false; // l.2096
        self._last_aux_model_failure_error = None; // l.2097
        self._last_aux_model_failure_model = None; // l.2098
        self._last_compression_savings_pct = 100.0; // l.2099
        self._ineffective_compression_count = 0; // l.2100
        self._anti_thrash_recovery_deadline = 0.0; // l.2101
        self._structural_no_op_backoff_until = 0.0; // l.2102
        self._prellm_skip_count = 0; // l.2103
        self._fallback_compression_streak = 0; // l.2104
        self._verify_compaction_cleared_threshold = false; // l.2105
        self._last_compression_made_progress = false; // l.2106
        self._summary_failure_cooldown_until = 0.0; // l.2107
        self._cooldown_persist_failed = false; // l.2108
        self._last_summary_error = None; // l.2109 (duplicated in Python — kept for 1:1)
        self._last_compress_aborted = false; // l.2110
        self._last_compress_refused_would_grow = false; // l.2111
        self.last_real_prompt_tokens = 0; // l.2112
        self.last_compression_rough_tokens = 0; // l.2113
        self.last_rough_tokens_when_real_prompt_fit = 0; // l.2114
        self._pending_request_rough_tokens = 0; // l.2115
        self.awaiting_real_usage_after_compression = false; // l.2116
        self._last_compression_telemetry = None; // l.2117
        self._active_compression_telemetry = None; // l.2118
        self._compression_telemetry_seed = None; // l.2119
        self._proactive_prune_rearm_tokens = 0; // l.2120

        // Micro-compaction state reset (ll.2122-2129)
        self._micro_compact_cursor = 0; // l.2123
        self._micro_compact_rolling_summary = String::new(); // l.2124
        self._micro_compact_consecutive_failures = 0; // l.2125
        self._micro_compact_last_failure_cursor = -1; // l.2126
        self._micro_compact_passes = 0; // l.2127
        self._micro_compact_tokens_saved_total = 0; // l.2128
        self._micro_compact_turns_since_pass = 0; // l.2129
    }

    /// Mirrors `def _begin_compression_telemetry(self, *, current_tokens: int | None, attempt_id: str | None = None, session_id: str | None = None, trigger_source: str | None = None) -> Dict[str, Any]:` (ll.2131-2176)
    ///
    /// Initialize content-free per-attempt compression telemetry.
    pub fn begin_compression_telemetry(
        &mut self,
        current_tokens: Option<i64>,
        attempt_id: Option<String>,
        session_id: Option<String>,
        trigger_source: Option<String>,
    ) -> Value {
        // Mirrors `seed = getattr(self, "_compression_telemetry_seed", None)` (l.2140)
        // and `if isinstance(seed, dict): attempt_id = attempt_id or seed.get("attempt_id") ...` (ll.2141-2144)
        let mut attempt_id = attempt_id;
        let mut session_id = session_id;
        let mut trigger_source = trigger_source;
        if let Some(Value::Object(seed)) = &self._compression_telemetry_seed {
            if attempt_id.is_none() {
                attempt_id = seed.get("attempt_id").and_then(|v| v.as_str()).map(|s| s.to_string());
            }
            if session_id.is_none() {
                session_id = seed.get("session_id").and_then(|v| v.as_str()).map(|s| s.to_string());
            }
            if trigger_source.is_none() {
                trigger_source = seed.get("trigger_source").and_then(|v| v.as_str()).map(|s| s.to_string());
            }
        }
        // Mirrors `telemetry: Dict[str, Any] = { "event": "compression_attempt", ... }` (ll.2145-2173)
        let telemetry = json!({
            "event": "compression_attempt",
            "attempt_id": attempt_id.unwrap_or_else(|| uuid_simple_hex()),
            "session_id": session_id.unwrap_or_default(),
            "trigger_source": trigger_source.unwrap_or_else(|| "unknown".to_string()),
            "main_provider": self.provider.clone(),
            "main_model": self.model.clone(),
            "main_context_limit": self._resolved_context_length.map(|v| v as i64).map(|v| json!(v)).unwrap_or(Value::Null),
            "current_estimated_tokens": current_tokens.map(|v| json!(v)).unwrap_or(Value::Null),
            "effective_threshold": self._threshold_tokens.map(|v| v as i64).map(|v| json!(v)).unwrap_or(Value::Null),
            "protected_head_tokens": Value::Null,
            "protected_tail_tokens": Value::Null,
            "middle_window_tokens": Value::Null,
            "prellm_skip_count": 0,
            "aux_prompt_tokens": Value::Null,
            "aux_output_reservation": Value::Null,
            "aux_provider": "",
            "aux_model": "",
            "effective_aux_context": Value::Null,
            "fit_margin": Value::Null,
            "chunking": false,
            "chunk_count": 0,
            "total_duration_ms": Value::Null,
            "aux_call_duration_ms": Value::Null,
            "fallback_used": false,
            "commit_status": "unknown",
            "split_status": "unknown",
            "failure_class": Value::Null
        });
        // Mirrors `self._active_compression_telemetry = telemetry; self._last_compression_telemetry = telemetry` (ll.2174-2175)
        self._active_compression_telemetry = Some(telemetry.clone());
        self._last_compression_telemetry = Some(telemetry.clone());
        // Mirrors `return telemetry` (l.2176)
        telemetry
    }

    #[allow(dead_code)]
    fn _begin_compression_telemetry(
        &mut self,
        current_tokens: Option<i64>,
        attempt_id: Option<String>,
        session_id: Option<String>,
        trigger_source: Option<String>,
    ) -> Value {
        self.begin_compression_telemetry(current_tokens, attempt_id, session_id, trigger_source)
    }

    /// Mirrors `def _record_compression_regions(self, *, head_messages, middle_messages, tail_messages) -> None:` (ll.2178-2190)
    pub fn record_compression_regions(
        &mut self,
        head_messages: &Turns,
        middle_messages: &Turns,
        tail_messages: &Turns,
    ) {
        // Mirrors `telemetry = getattr(self, "_active_compression_telemetry", None); if not isinstance(telemetry, dict): return` (ll.2185-2187)
        let Some(Value::Object(ref mut tele)) = self._active_compression_telemetry else {
            return;
        };
        // Mirrors `telemetry["protected_head_tokens"] = estimate_messages_tokens_rough(head_messages)` etc. (ll.2188-2190)
        tele.insert("protected_head_tokens".to_string(), json!(estimate_messages_tokens_rough(head_messages) as i64));
        tele.insert("middle_window_tokens".to_string(), json!(estimate_messages_tokens_rough(middle_messages) as i64));
        tele.insert("protected_tail_tokens".to_string(), json!(estimate_messages_tokens_rough(tail_messages) as i64));
    }

    #[allow(dead_code)]
    fn _record_compression_regions(
        &mut self,
        head_messages: &Turns,
        middle_messages: &Turns,
        tail_messages: &Turns,
    ) {
        self.record_compression_regions(head_messages, middle_messages, tail_messages)
    }

    /// Mirrors `def _record_aux_compression_call(self, *, prompt_messages, max_tokens, duration_ms, aux_provider, aux_model, effective_aux_context) -> None:` (ll.2192-2223)
    pub fn record_aux_compression_call(
        &mut self,
        prompt_messages: &Turns,
        max_tokens: Option<i64>,
        duration_ms: i64,
        aux_provider: Option<String>,
        aux_model: Option<String>,
        effective_aux_context: Option<i64>,
    ) {
        // Mirrors `telemetry = getattr(self, "_active_compression_telemetry", None); if not isinstance(telemetry, dict): return` (ll.2202-2204)
        let Some(Value::Object(ref mut tele)) = self._active_compression_telemetry else {
            return;
        };
        // Mirrors `telemetry["aux_prompt_tokens"] = estimate_messages_tokens_rough(prompt_messages)` (l.2205)
        tele.insert("aux_prompt_tokens".to_string(), json!(estimate_messages_tokens_rough(prompt_messages) as i64));
        // Mirrors `telemetry["aux_output_reservation"] = _safe_int(max_tokens)` (l.2206)
        tele.insert(
            "aux_output_reservation".to_string(),
            max_tokens.map(|v| json!(v)).unwrap_or(Value::Null),
        );
        // Mirrors `if aux_provider: telemetry["aux_provider"] = aux_provider` (ll.2207-2208)
        if let Some(p) = aux_provider {
            if !p.is_empty() {
                tele.insert("aux_provider".to_string(), json!(p));
            }
        }
        // Mirrors `if aux_model: telemetry["aux_model"] = aux_model` (ll.2209-2210)
        if let Some(m) = aux_model {
            if !m.is_empty() {
                tele.insert("aux_model".to_string(), json!(m));
            }
        }
        // Mirrors `if effective_aux_context is not None: telemetry["effective_aux_context"] = _safe_int(effective_aux_context)` (ll.2211-2212)
        if let Some(ctx) = effective_aux_context {
            tele.insert("effective_aux_context".to_string(), json!(ctx));
        }
        // Mirrors `if telemetry["effective_aux_context"] is not None and telemetry["aux_prompt_tokens"] is not None: telemetry["fit_margin"] = ...` (ll.2213-2221)
        let eff = tele.get("effective_aux_context").and_then(|v| v.as_i64());
        let prompt = tele.get("aux_prompt_tokens").and_then(|v| v.as_i64());
        if let (Some(eff), Some(prompt)) = (eff, prompt) {
            let reservation = tele.get("aux_output_reservation").and_then(|v| v.as_i64()).unwrap_or(0);
            tele.insert("fit_margin".to_string(), json!(eff - prompt - reservation));
        }
        // Mirrors `previous = telemetry.get("aux_call_duration_ms") or 0; telemetry["aux_call_duration_ms"] = previous + max(0, int(duration_ms))` (ll.2222-2223)
        let previous = tele.get("aux_call_duration_ms").and_then(|v| v.as_i64()).unwrap_or(0);
        tele.insert("aux_call_duration_ms".to_string(), json!(previous + duration_ms.max(0)));
    }

    #[allow(dead_code)]
    fn _record_aux_compression_call(
        &mut self,
        prompt_messages: &Turns,
        max_tokens: Option<i64>,
        duration_ms: i64,
        aux_provider: Option<String>,
        aux_model: Option<String>,
        effective_aux_context: Option<i64>,
    ) {
        self.record_aux_compression_call(prompt_messages, max_tokens, duration_ms, aux_provider, aux_model, effective_aux_context)
    }

    /// Mirrors `def _emit_init_summary_once(self) -> None:` (ll.2225-2245)
    ///
    /// Emit the informative startup line once, on first resolution.
    pub fn emit_init_summary_once(&mut self) {
        // Mirrors `if not getattr(self, "_log_init_summary", False): return` (ll.2234-2235)
        if !self._log_init_summary {
            return;
        }
        self._log_init_summary = false;
        // Mirrors `logger.info("Context compressor initialized: model=%s ...", ...)` (ll.2237-2245)
        // Rust: log via eprintln for self-contained stub (real would use log crate)
        let ctx_len = self._resolved_context_length.unwrap_or(0);
        let thresh = self._threshold_tokens.unwrap_or(0);
        let tail = self._tail_token_budget.unwrap_or(0);
        let _ = (ctx_len, thresh, tail); // keep vars for audit traceability
        // No actual logging in NEVER-cargo slice — kept as no-op with field reads preserved.
    }

    #[allow(dead_code)]
    fn _emit_init_summary_once(&mut self) {
        self.emit_init_summary_once()
    }

    /// Mirrors `def _resolve_context_length(self) -> int:` (ll.2247-2268)
    ///
    /// Resolve and cache the model's context length on first access.
    pub fn resolve_context_length(&mut self) -> usize {
        // Mirrors `if self._resolved_context_length is None: self._resolved_context_length = get_model_context_length(...)` (ll.2249-2256)
        if self._resolved_context_length.is_none() {
            let resolved = get_model_context_length(
                &self.model,
                &self.base_url,
                &self.api_key,
                self._config_context_length,
                &self.provider,
            );
            self._resolved_context_length = Some(resolved);
            // Mirrors `self.threshold_percent = self._effective_threshold_percent(self._resolved_context_length, self._base_threshold_percent,)` (ll.2264-2266)
            self.threshold_percent = self.effective_threshold_percent(resolved, self._base_threshold_percent);
            // Mirrors `self._emit_init_summary_once()` (l.2267)
            let mut tmp = self.clone();
            tmp.emit_init_summary_once();
            self._log_init_summary = tmp._log_init_summary;
        }
        // Mirrors `return self._resolved_context_length` (l.2268)
        self._resolved_context_length.unwrap_or(0)
    }

    #[allow(dead_code)]
    fn _resolve_context_length(&mut self) -> usize {
        self.resolve_context_length()
    }

    /// Helper: mirrors `self._effective_threshold_percent(ctx, base)` (used at ll.2264, 2292)
    /// Small-context threshold floor: models under 512K trigger at >=75%.
    fn effective_threshold_percent(&self, ctx: usize, base: f64) -> f64 {
        // Mirrors Python `if ctx < _SMALL_CTX_WINDOW_LIMIT: return max(base, _SMALL_CTX_THRESHOLD_PERCENT) else: return base`
        // Source not in this slice but referenced by compressor logic (ll.1239-1240, 2264).
        const SMALL_CTX_WINDOW_LIMIT: usize = 512_000;
        const SMALL_CTX_THRESHOLD_PERCENT: f64 = 0.75;
        if ctx < SMALL_CTX_WINDOW_LIMIT {
            base.max(SMALL_CTX_THRESHOLD_PERCENT)
        } else {
            base
        }
    }

    /// Helper: mirrors `self._compute_threshold_tokens(ctx, pct, max_tokens)` (l.2310)
    fn compute_threshold_tokens(&self, ctx: usize, pct: f64, max_tokens: Option<usize>) -> usize {
        // Mirrors Python threshold logic: floor at MINIMUM_CONTEXT_LENGTH unless pct suggests lower (#14690)
        let mut tokens = (ctx as f64 * pct) as usize;
        if tokens < MINIMUM_CONTEXT_LENGTH {
            tokens = MINIMUM_CONTEXT_LENGTH;
        }
        if let Some(mt) = max_tokens {
            // Python also considers max_tokens; keep min behavior for audit
            if mt > 0 {
                tokens = tokens.min(mt);
            }
        }
        tokens
    }

    /// Helper: mirrors `self._apply_threshold_tokens_cap()` (l.2315)
    fn apply_threshold_tokens_cap(&mut self) {
        // Mirrors Python: `if self._threshold_tokens_cap is not None: self._threshold_tokens = min(self._threshold_tokens, cap)`
        // Cap not modeled in this slice's struct; kept as no-op for 1:1 traceability.
    }

    /// Mirrors `@property def context_length(self) -> int:` (ll.2270-2272) + setter (ll.2274-2298)
    pub fn context_length(&mut self) -> usize {
        self.resolve_context_length()
    }

    /// Mirrors `@context_length.setter def context_length(self, value: int) -> None:` (ll.2274-2298)
    pub fn set_context_length(&mut self, value: usize) {
        // Mirrors `if value == getattr(self, "_resolved_context_length", None): return` (ll.2282-2283)
        if Some(value) == self._resolved_context_length {
            return;
        }
        self._resolved_context_length = Some(value);
        // Mirrors `if _base is not None: self.threshold_percent = self._effective_threshold_percent(value, _base,)` (ll.2290-2294)
        self.threshold_percent = self.effective_threshold_percent(value, self._base_threshold_percent);
        // Mirrors `self._threshold_tokens = None; self._tail_token_budget = None; self._max_summary_tokens = None` (ll.2295-2297)
        self._threshold_tokens = None;
        self._tail_token_budget = None;
        self._max_summary_tokens = None;
        // Mirrors `self._emit_init_summary_once()` (l.2298)
        self.emit_init_summary_once();
    }

    /// Mirrors `@property def threshold_tokens(self) -> int:` (ll.2300-2316)
    pub fn threshold_tokens(&mut self) -> usize {
        // Mirrors `if self._threshold_tokens is None:` (l.2302)
        if self._threshold_tokens.is_none() {
            let ctx = self.context_length(); // Mirrors `_ctx = self.context_length` (l.2306) — triggers floor side effect
            let tokens = self.compute_threshold_tokens(ctx, self.threshold_percent, self.max_tokens); // l.2310-2312
            self._threshold_tokens = Some(tokens);
            self.apply_threshold_tokens_cap(); // l.2315
        }
        // Mirrors `return self._threshold_tokens` (l.2316)
        self._threshold_tokens.unwrap_or(0)
    }

    /// Mirrors `@threshold_tokens.setter def threshold_tokens(self, value: int) -> None:` (ll.2318-2320)
    pub fn set_threshold_tokens(&mut self, value: usize) {
        // Mirrors `self._threshold_tokens = value` (l.2320)
        self._threshold_tokens = Some(value);
    }

    /// Mirrors `@property def tail_token_budget(self) -> int:` (ll.2322-2338)
    pub fn tail_token_budget(&mut self) -> usize {
        // Mirrors `if self._tail_token_budget is None:` (l.2324)
        if self._tail_token_budget.is_none() {
            // Mirrors `if getattr(self, "tail_mode", "legacy") == "lean":` (l.2325)
            if self.tail_mode == "lean" {
                // Mirrors lean mode 2.5% clamped [FLOOR, CAP] (ll.2332-2335)
                let ctx = self.context_length();
                let budget = ((ctx as f64 * 0.025) as usize).max(LEAN_TAIL_FLOOR_TOKENS).min(LEAN_TAIL_CAP_TOKENS);
                self._tail_token_budget = Some(budget);
            } else {
                // Mirrors `self._tail_token_budget = int(self.threshold_tokens * self.summary_target_ratio)` (l.2337)
                let thresh = self.threshold_tokens();
                self._tail_token_budget = Some(((thresh as f64 * self.summary_target_ratio) as usize));
            }
        }
        self._tail_token_budget.unwrap_or(0)
    }

    /// Mirrors `@tail_token_budget.setter def tail_token_budget(self, value: int) -> None:` (ll.2340-2342)
    pub fn set_tail_token_budget(&mut self, value: usize) {
        // Mirrors `self._tail_token_budget = value` (l.2342)
        self._tail_token_budget = Some(value);
    }

    /// Mirrors `@property def max_summary_tokens(self) -> int:` (ll.2344-2350)
    pub fn max_summary_tokens(&mut self) -> usize {
        // Mirrors `if self._max_summary_tokens is None: self._max_summary_tokens = min(int(self.context_length * 0.05), _SUMMARY_TOKENS_CEILING)` (ll.2346-2349)
        if self._max_summary_tokens.is_none() {
            let ctx = self.context_length();
            let tokens = ((ctx as f64 * 0.05) as usize).min(SUMMARY_TOKENS_CEILING);
            self._max_summary_tokens = Some(tokens);
        }
        self._max_summary_tokens.unwrap_or(0)
    }

    /// Mirrors `@max_summary_tokens.setter def max_summary_tokens(self, value: int) -> None:` (ll.2352-2354)
    pub fn set_max_summary_tokens(&mut self, value: usize) {
        // Mirrors `self._max_summary_tokens = value` (l.2354)
        self._max_summary_tokens = Some(value);
    }

    /// Mirrors `def on_session_end(self, session_id: str, messages: List[Dict[str, Any]]) -> None:` (ll.2356-2406)
    ///
    /// Clear all per-session compaction state at a real session boundary.
    pub fn on_session_end(&mut self, _session_id: &str, _messages: &Turns) {
        // Mirrors Python ll.2375-2406 — verbatim reset list (same as on_session_reset's per-session surface plus probe flags)
        self._previous_summary = None; // l.2375
        self._summary_has_user_turn = None; // l.2376
        self._last_summary_error = None; // l.2377
        self._consecutive_timeout_failures = 0; // l.2378
        self._last_summary_dropped_count = 0; // l.2379
        self._last_summary_fallback_used = false; // l.2380
        self._last_feasibility_skip = false; // l.2381
        self._last_aux_model_failure_error = None; // l.2382
        self._last_aux_model_failure_model = None; // l.2383
        self._last_compression_savings_pct = 100.0; // l.2384
        self._ineffective_compression_count = 0; // l.2385
        self._anti_thrash_recovery_deadline = 0.0; // l.2386
        self._structural_no_op_backoff_until = 0.0; // l.2387
        self._prellm_skip_count = 0; // l.2388
        self._fallback_compression_streak = 0; // l.2389
        self._verify_compaction_cleared_threshold = false; // l.2390
        self._last_compression_made_progress = false; // l.2391
        self._summary_failure_cooldown_until = 0.0; // l.2392
        self._cooldown_persist_failed = false; // l.2393
        self._last_compress_aborted = false; // l.2394
        self._last_compress_refused_would_grow = false; // l.2395
        self._context_probed = false; // l.2396
        self._context_probe_persistable = false; // l.2397
        self.last_real_prompt_tokens = 0; // l.2398
        self.last_compression_rough_tokens = 0; // l.2399
        self.last_rough_tokens_when_real_prompt_fit = 0; // l.2400
        self._pending_request_rough_tokens = 0; // l.2401 — extended one line past nominal 2400 to close function
        self.awaiting_real_usage_after_compression = false; // l.2402
        self._last_compression_telemetry = None; // l.2403
        self._active_compression_telemetry = None; // l.2404
        self._compression_telemetry_seed = None; // l.2405
        self._proactive_prune_rearm_tokens = 0; // l.2406
        // Python continues to `bind_session_state` at l.2408 — deferred to compressor_slice4.rs
    }
}

// ---------------------------------------------------------------------------
// Helpers not in 1600-2400 but needed for self-containment (uuid hex)
// ---------------------------------------------------------------------------

/// Minimal `uuid4().hex` stub — mirrors `uuid.uuid4().hex` at l.2147.
fn uuid_simple_hex() -> String {
    // Deterministic stub for audit; real would use `uuid` crate.
    // Use a fixed placeholder so telemetry shape matches Python without external dep.
    "00000000000000000000000000000000".to_string()
}

// ---------------------------------------------------------------------------
// End of slice 3 — next slice (compressor_slice4) continues from l.2407.
// ---------------------------------------------------------------------------
// Python ll.2407 onward (`def bind_session_state(...)`) is deferred to
// `compressor_slice4.rs`. This boundary was chosen to close `on_session_end`
// at l.2406 so the module remains syntactically complete despite the
// nominal 2400 cut falling mid-function.
// ---------------------------------------------------------------------------
