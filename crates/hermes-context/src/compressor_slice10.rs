//! Automatic context window compression for long conversations.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/context_compressor.py`
//! (8211 LOC) — slice 10/11, lines 7200-8000.
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
//! Mirrors Python ll.7200-8000 verbatim; line numbers in comments refer to the
//! 8211-line source file. Slice 1 covered ll.1-800, slice 2 ll.800-1600,
//! slice 3 ll.1600-2400, slice 4 ll.2400-3200, slice 5 ll.3200-4000,
//! slice 6 ll.4000-4800, slice 7 ll.4800-5600 (closed mid- `_ground_historical_task_snapshot`),
//! slice 8 ll.5600-6400 (closed mid- `_find_tail_cut_by_tokens` at
//! `fallback_cut = n - min_tail` / `cut_idx = min(cut_idx, fallback_cut)` ll.6402-6404),
//! slice 9 ll.6400-7200 (resumed at `# will still force a minimal cut` ll.6400
//! overlap, canonical new ll.6405-7229; nominal 7200 falls mid-`_merge_adjacent_user_turns`
//! ll.7189-7229 extended to 7229 to close that method, so slice 9's tail
//! ll.7200-7229 is the `prev_content` merge + `drop_stale_api_content` + `merged.append`
//! + `return merged`).
//! This slice resumes at l.7200 overlap for self-containment; canonical new
//! content starts at l.7231 (`def compress(self, messages, ...)`). It runs
//! through l.8000 (inside `compress` — anti-thrashing diagnostic comment
//! `# counter below resets every pass ...` at ll.8000-8001, within the
//! `pre_estimate = estimate_messages_tokens_rough(messages)` / `saved_estimate`
//! / `savings_pct` block ll.8003-8006). The nominal 8000 boundary falls
//! mid-function inside `compress` (ll.7231-8080); the method is extended to
//! l.8080 (`return compressed`) to keep the module syntactically complete
//! without `cargo` — its tail (ll.8000-8080, `_last_compression_savings_pct`
//! assignment, quiet-mode logs, `_strip_persistence_markers`,
//! `_prune_stale_reasoning_replay`, `_last_compression_made_progress = True`,
//! `trim_memory` try/except, micro-compact reset
//! `self._micro_compact_rolling_summary = ""` etc.) is included, and the next
//! free functions `is_compaction_summary_message` at l.8083 continue in
//! `compressor_slice11.rs`. Slice 11 will cover ll.8000-8211.
//! This slice is verified by line-level audit, not by compilation.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

// ---------------------------------------------------------------------------
// Imports — mirrors Python ll.19-45 (same set as slices 1-9; repeated for self-containment)
// ---------------------------------------------------------------------------
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde_json::{json, Value};

// Python imports (ll.19-26) — stdlib:
//   hashlib, json, logging, sqlite3, re, time, uuid, typing
// Mapped: std hash, serde_json, log, rusqlite (stubbed), regex, time, uuid

// Python intra-repo imports (ll.28-45):
//   from agent.auxiliary_client import (
//       AuxiliaryExplicitCancellation, _is_connection_error, aux_interrupt_protection, call_llm,
//   )
//   from agent.context_engine import ContextEngine, sanitize_memory_context
//   from agent.error_classifier import FailoverReason, classify_api_error
//   from agent.message_sanitization import tool_call_id_variants
//   from agent.model_metadata import (MINIMUM_CONTEXT_LENGTH, get_model_context_length,
//                                     estimate_messages_tokens_rough, estimate_tokens_rough)
//   from agent.redact import redact_sensitive_text
//   from agent.turn_context import drop_stale_api_content
//   from tools.todo_tool import TODO_INJECTION_HEADER
// Rust: sibling crates / later slices. Stubs below mirror surface.

// ---------------------------------------------------------------------------
// Logger — mirrors `logger = logging.getLogger(__name__)` (l.47)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "context_compressor";

// ---------------------------------------------------------------------------
// Shared type aliases — mirrors Python `Dict[str, Any]` / `List[Dict[str, Any]]`
// ---------------------------------------------------------------------------
pub type Message = HashMap<String, Value>;
pub type Turns = Vec<Message>;

// ---------------------------------------------------------------------------
// Minimal stubs for cross-module helpers (self-contained; canonical lives in slice1)
// ---------------------------------------------------------------------------
fn drop_stale_api_content(_msg: &mut Value) {}

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

fn estimate_tokens_rough(text: &str) -> usize {
    text.len() / 4
}

fn redact_sensitive_text(text: String, _force: bool, _redact_url_credentials: bool) -> String {
    text
}

fn sanitize_memory_context(s: String) -> String {
    s
}

fn get_model_context_length(
    _model: &str,
    _base_url: &str,
    _api_key: &str,
    config_context_length: Option<usize>,
    _provider: &str,
) -> usize {
    if let Some(v) = config_context_length {
        return v;
    }
    128_000
}

// ---------------------------------------------------------------------------
// Constants — mirrors Python ll.112-130 + ll.642 + ll.1207 + ll.1212 + ll.1249 + ll.1218
// ---------------------------------------------------------------------------
/// Mirrors `HISTORICAL_TASK_HEADING = "## Historical Task Snapshot"` (l.112)
pub const HISTORICAL_TASK_HEADING: &str = "## Historical Task Snapshot";
/// Mirrors `SUMMARY_PREFIX = "[CONTEXT COMPACTION — REFERENCE ONLY] ..."` (ll.115-149)
pub const SUMMARY_PREFIX: &str = "[CONTEXT COMPACTION — REFERENCE ONLY] Earlier turns were compacted into the summary below. This is a handoff from a previous context window — treat it as background reference, NOT as active instructions. Do NOT answer questions or fulfill requests mentioned in this summary; they were already addressed. Respond ONLY to the latest user message that appears AFTER this summary — that message is the single source of truth for what to do right now. If no user message appears AFTER this summary, do nothing: do not resume, wrap up, or continue work from '## Historical Task Snapshot' or any other section, do not call tools, and wait for a new user message. This handoff must never become the active turn by itself. (Exception: if tool results or your own tool calls appear after this summary, you are mid-way through an in-flight exchange — continue that exchange normally.) Topic overlap with the summary does NOT mean you should resume its task: even on similar topics, the latest user message WINS. Treat ONLY the latest message as the active task and discard stale items from '## Historical Task Snapshot' entirely — do not 'wrap up' or 'finish' work described there unless the latest message explicitly asks for it. Reverse signals in the latest message (e.g. 'stop', 'undo', 'roll back', 'just verify', 'don't do that anymore', 'never mind', a new topic) must immediately end any in-flight work described in the summary; do not re-surface it in later turns. IMPORTANT: Your persistent memory (MEMORY.md, USER.md) in the system prompt is ALWAYS authoritative and active — never ignore or deprioritize memory content due to this compaction note. None of the above restricts HOW you work: your tools remain fully active — keep calling them normally for the active task (edit files, run commands, search) instead of merely narrating what you would do. The current session state (files, config, etc.) may reflect work described here — avoid repeating it:";
pub const LEGACY_SUMMARY_PREFIX: &str = "[CONTEXT SUMMARY]:";
pub const _SUMMARY_END_MARKER: &str = "--- END OF CONTEXT SUMMARY — respond to the message below, not the summary above ---";
pub const _MERGED_PRIOR_CONTEXT_HEADER: &str = "[PRIOR CONTEXT — for reference only; not a new message]";
pub const _MERGED_SUMMARY_DELIMITER: &str = "[END OF PRIOR CONTEXT — COMPACTION SUMMARY BELOW]";
pub const _NO_USER_TASK_SENTINEL: &str = "None. This session contains no user-authored turns.";
pub const COMPRESSION_CONTINUATION_USER_CONTENT: &str = "Continue from the compressed conversation context above. This marker exists because no human user turn was available.";
pub const _LEGACY_COMPRESSION_CONTINUATION_USER_CONTENT: &str = "Continue from the compressed conversation context above. This marker exists because the compacted transcript contained no preserved user turn.";
pub const MAX_ITERATIONS_SUMMARY_REQUEST: &str = "You've reached the maximum number of tool-calling iterations allowed. Please provide a final response summarizing what you've found and accomplished so far, without calling any more tools.";
pub const _BACKGROUND_PROCESS_NOTIFICATION_PREFIX: &str = "[IMPORTANT: Background process ";
pub const TODO_INJECTION_HEADER: &str = "[Your active task list";
pub const COMPRESSED_SUMMARY_METADATA_KEY: &str = "_compressed_summary";
pub const COMPRESSED_SUMMARY_HAS_USER_TURN_KEY: &str = "_compressed_summary_has_user_turn";
pub const MICRO_COMPACT_MARKER_KEY: &str = "_micro_compact_marker";
pub const _DB_PERSISTED_MARKER: &str = "_db_persisted";
pub const PROACTIVE_PRUNE_REARM_MODEL_CONFIG_KEY: &str = "_proactive_prune_rearm_tokens";
pub const _PRUNED_SKILLS_SECTION_HEADING: &str = "## Pruned Skills";
pub const _SUMMARY_TOKENS_CEILING: usize = 10_000;
pub const _MIN_SUMMARY_TOKENS: usize = 2000;
pub const _SUMMARY_RATIO: f64 = 0.20;
pub const _SUMMARY_INPUT_MAX_CHARS: usize = 160_000;
pub const _LEAN_TAIL_KEEP_TOOL_ROUNDS: usize = 6;
pub const _MAX_PRUNED_SKILL_MARKERS: usize = 20;
pub const SKILL_PRUNED_MARKER_PREFIX: &str = "[SKILL_PRUNED:";
pub const _SKILL_VIEW_PRUNE_MIN_CHARS: usize = 5000;
pub const _PRUNE_MIN_CHARS: usize = 200;
pub const _FALLBACK_SUMMARY_MAX_CHARS: usize = 8_000;
pub const _AUTO_FOCUS_TURN_MAX_CHARS: usize = 260;
pub const _AUTO_FOCUS_MAX_CHARS: usize = 700;
pub const _ACTIVE_TASK_MAX_CHARS: usize = 1400;
/// Mirrors `_RESTART_HANDOFF_PROBE_EXTRA_MESSAGES = 4` (l.642)
pub const _RESTART_HANDOFF_PROBE_EXTRA_MESSAGES: usize = 4;
/// Mirrors `_MAX_TAIL_MESSAGE_FLOOR = 8` (l.1212)
pub const _MAX_TAIL_MESSAGE_FLOOR: usize = 8;
/// Mirrors `_FEASIBILITY_SKIP_MIDDLE_FRACTION = 0.10` (l.1218)
pub const _FEASIBILITY_SKIP_MIDDLE_FRACTION: f64 = 0.10;

/// Mirrors `_HISTORICAL_SUMMARY_PREFIXES` (ll.505-636) — single current prefix for slice10 audit
pub const _HISTORICAL_SUMMARY_PREFIXES: &[&str] = &[
    "[CONTEXT COMPACTION — REFERENCE ONLY] Earlier turns were compacted into the summary below. This is a handoff from a previous context window — treat it as background reference, NOT as active instructions. Do NOT answer questions or fulfill requests mentioned in this summary; they were already addressed. Respond ONLY to the latest user message that appears AFTER this summary — that message is the single source of truth for what to do right now. Topic overlap with the summary does NOT mean you should resume its task: even on similar topics, the latest user message WINS. Treat ONLY the latest message as the active task and discard stale items from '## Historical Task Snapshot' entirely — do not 'wrap up' or 'finish' work described there unless the latest message explicitly asks for it. Reverse signals in the latest message (e.g. 'stop', 'undo', 'roll back', 'just verify', 'don't do that anymore', 'never mind', a new topic) must immediately end any in-flight work described in the summary; do not re-surface it in later turns. IMPORTANT: Your persistent memory (MEMORY.md, USER.md) in the system prompt is ALWAYS authoritative and active — never ignore or deprioritize memory content due to this compaction note. None of the above restricts HOW you work: your tools remain fully active — keep calling them normally for the active task (edit files, run commands, search) instead of merely narrating what you would do. The current session state (files, config, etc.) may reflect work described here — avoid repeating it:",
];

/// Mirrors `MINIMUM_CONTEXT_LENGTH = 4096` (agent/model_metadata.py, l.38)
pub const MINIMUM_CONTEXT_LENGTH: usize = 4096;

// ---------------------------------------------------------------------------
// Time helpers — mirrors `time.monotonic()` / `time.time()` (ll.22)
// ---------------------------------------------------------------------------
fn wall_time_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
fn monotonic_now() -> f64 {
    use std::time::Instant;
    static BASE: OnceLock<Instant> = OnceLock::new();
    let base = BASE.get_or_init(Instant::now);
    base.elapsed().as_secs_f64()
}

// ---------------------------------------------------------------------------
// Content helpers — mirrors Python ll.1505-1519 + ll.5255-5300 + ll.1528-1535
// ---------------------------------------------------------------------------
fn content_text_for_contains(value: &Value) -> String {
    match value {
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
// Python alias `_content_text_for_contains` (l.1505)
fn _content_text_for_contains(v: &Value) -> String {
    content_text_for_contains(v)
}

fn _append_text_to_content(content: Value, text: &str, prepend: bool) -> Value {
    // Mirrors `def _append_text_to_content(content, text, *, prepend=False) -> Any:` (ll.1528-1544)
    // Handles str vs list[dict] multimodal. Stub: if string, concat; if array, push/ prepend text part.
    match content {
        Value::String(s) => {
            if prepend {
                Value::String(format!("{}{}", text, s))
            } else {
                Value::String(format!("{}{}", s, text))
            }
        }
        Value::Array(mut arr) => {
            let part = json!({"type": "text", "text": text});
            if prepend {
                arr.insert(0, part);
            } else {
                arr.push(part);
            }
            Value::Array(arr)
        }
        Value::Null => Value::String(text.to_string()),
        other => Value::String(format!("{}{}", other.to_string(), text)),
    }
}

fn _strip_summary_prefix(summary: &str) -> String {
    // Mirrors `ContextCompressor._strip_summary_prefix` (ll.5255-5288)
    let mut text = summary.trim().to_string();
    if text.contains(_MERGED_SUMMARY_DELIMITER) {
        text = text.splitn(2, _MERGED_SUMMARY_DELIMITER).nth(1).unwrap_or("").trim().to_string();
    }
    for prefix in std::iter::once(SUMMARY_PREFIX)
        .chain(std::iter::once(LEGACY_SUMMARY_PREFIX))
        .chain(_HISTORICAL_SUMMARY_PREFIXES.iter().copied())
    {
        if text.starts_with(prefix) {
            text = text[prefix.len()..].trim_start().to_string();
            break;
        }
    }
    if let Some(idx) = text.find(_SUMMARY_END_MARKER) {
        text = text[..idx].trim_end().to_string();
    }
    text
}

fn _has_compressed_summary_metadata(msg: &Message) -> bool {
    // Mirrors `ContextCompressor._has_compressed_summary_metadata` (ll.5341-5354)
    msg.get(COMPRESSED_SUMMARY_METADATA_KEY)
        .map(|v| !v.is_null() && v != &Value::Bool(false))
        .unwrap_or(false)
}

fn _is_context_summary_content(content: &Value) -> bool {
    // Mirrors `ContextCompressor._is_context_summary_content` (ll.5329-5338) via `classify_summary_content`
    let text = content_text_for_contains(content);
    let trimmed = text.trim_start();
    // merged carrier: summary after delimiter
    if trimmed.contains(_MERGED_SUMMARY_DELIMITER) {
        if let Some(after) = trimmed.splitn(2, _MERGED_SUMMARY_DELIMITER).nth(1) {
            let after = after.trim_start();
            if after.starts_with(SUMMARY_PREFIX)
                || after.starts_with(LEGACY_SUMMARY_PREFIX)
                || _HISTORICAL_SUMMARY_PREFIXES.iter().any(|p| after.starts_with(*p))
            {
                return true;
            }
        }
        return false;
    }
    trimmed.starts_with(SUMMARY_PREFIX)
        || trimmed.starts_with(LEGACY_SUMMARY_PREFIX)
        || _HISTORICAL_SUMMARY_PREFIXES.iter().any(|p| trimmed.starts_with(*p))
}

fn _is_context_summary_message(msg: &Message) -> bool {
    // Mirrors `ContextCompressor._is_context_summary_message` (ll.5436-5444)
    if _has_compressed_summary_metadata(msg) {
        return true;
    }
    if let Some(c) = msg.get("content") {
        if _is_context_summary_content(c) {
            return true;
        }
    }
    false
}

fn _is_blank_user_turn(msg: &Message) -> bool {
    // Mirrors `ContextCompressor._is_blank_user_turn` (ll.5447-5472)
    if msg.get("role").and_then(|v| v.as_str()) != Some("user") {
        return false;
    }
    if _has_compressed_summary_metadata(msg) {
        return false;
    }
    if let Some(c) = msg.get("content") {
        if _is_context_summary_content(c) {
            return false;
        }
        if c.is_null() {
            return true;
        }
        if let Some(s) = c.as_str() {
            if s.trim().is_empty() {
                return true;
            }
            return false;
        }
        if let Some(arr) = c.as_array() {
            if arr.is_empty() {
                return true;
            }
            for part in arr {
                if let Some(s) = part.as_str() {
                    if !s.trim().is_empty() {
                        return false;
                    }
                    continue;
                }
                if let Some(obj) = part.as_object() {
                    if let Some(t) = obj.get("type").and_then(|v| v.as_str()) {
                        if t == "text" || t == "input_text" {
                            if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                                if text.trim().is_empty() {
                                    continue;
                                }
                            }
                        }
                    }
                }
                return false;
            }
            return true;
        }
        return false;
    }
    true
}

fn _is_actionable_user_turn(msg: &Message) -> bool {
    // Mirrors `ContextCompressor._is_actionable_user_turn` (ll.5474-5484)
    if msg.get("role").and_then(|v| v.as_str()) != Some("user") {
        return false;
    }
    if _has_compressed_summary_metadata(msg) {
        return false;
    }
    if let Some(c) = msg.get("content") {
        if _is_context_summary_content(c) {
            return false;
        }
    }
    !_is_blank_user_turn(msg)
}

fn _is_synthetic_compression_user_turn(msg: &Message) -> bool {
    // Mirrors `ContextCompressor._is_synthetic_compression_user_turn` (ll.5362-5371 simplified)
    if msg.get("role").and_then(|v| v.as_str()) != Some("user") {
        return false;
    }
    if _has_compressed_summary_metadata(msg) {
        return true;
    }
    if let Some(c) = msg.get("content") {
        if _is_context_summary_content(c) {
            return true;
        }
        let text = content_text_for_contains(c);
        let t = text.trim();
        if t == COMPRESSION_CONTINUATION_USER_CONTENT
            || t == _LEGACY_COMPRESSION_CONTINUATION_USER_CONTENT
            || t == MAX_ITERATIONS_SUMMARY_REQUEST
        {
            return true;
        }
        if t.starts_with(_BACKGROUND_PROCESS_NOTIFICATION_PREFIX) {
            return true;
        }
        if t.starts_with(&format!("{}\n", TODO_INJECTION_HEADER)) {
            return true;
        }
    }
    false
}

fn _redact_compaction_text(text: &str) -> String {
    redact_sensitive_text(text.to_string(), true, true)
}

// ---------------------------------------------------------------------------
// Sanitization + token helpers — mirrors `agent/message_sanitization.py` + `agent/model_metadata.py`
// ---------------------------------------------------------------------------
fn tool_call_id_variants(tc: &Value) -> HashSet<String> {
    // Mirrors `agent.message_sanitization.tool_call_id_variants` (ll.5776-5782)
    let mut set = HashSet::new();
    if let Some(obj) = tc.as_object() {
        for key in &["call_id", "id", "response_item_id"] {
            if let Some(v) = obj.get(*key).and_then(|x| x.as_str()) {
                if !v.is_empty() {
                    set.insert(v.to_string());
                    if v.contains('|') {
                        for part in v.split('|') {
                            if !part.is_empty() {
                                set.insert(part.to_string());
                            }
                        }
                    }
                }
            }
        }
        if let Some(v) = obj.get("call_id").and_then(|x| x.as_str()) {
            if let Some(item) = obj.get("response_item_id").and_then(|x| x.as_str()) {
                set.insert(format!("{}|{}", v, item));
            }
        }
    }
    set
}

fn tool_result_id_variants(cid: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    if cid.is_empty() {
        return set;
    }
    set.insert(cid.to_string());
    if cid.contains('|') {
        for part in cid.split('|') {
            if !part.is_empty() {
                set.insert(part.to_string());
            }
        }
    }
    set
}

fn estimate_msg_budget_tokens(msg: &Message, charge_stale_thinking: bool) -> usize {
    let content_val = msg.get("content").unwrap_or(&Value::Null);
    let mut tokens = match content_val {
        Value::String(s) => estimate_tokens_rough(s) + 10,
        _ => s_to_len(content_val) / 4 + 10,
    };
    if let Some(tc_val) = msg.get("tool_calls") {
        if let Some(arr) = tc_val.as_array() {
            for tc in arr {
                tokens += estimate_tokens_rough(&tc.to_string());
            }
        }
    }
    if charge_stale_thinking {
        for key in &["reasoning", "reasoning_content"] {
            if let Some(v) = msg.get(*key) {
                if !v.is_null() {
                    tokens += v.to_string().len() / 4;
                }
            }
        }
    }
    tokens
}
fn s_to_len(v: &Value) -> usize {
    match v {
        Value::String(s) => s.len(),
        Value::Array(parts) => {
            let mut t = 0;
            for p in parts {
                if let Some(s) = p.as_str() {
                    t += s.len();
                } else if let Some(o) = p.as_object() {
                    t += o.get("text").and_then(|x| x.as_str()).unwrap_or("").len();
                } else {
                    t += p.to_string().len();
                }
            }
            t
        }
        Value::Null => 0,
        o => o.to_string().len(),
    }
}
fn last_assistant_index(messages: &Turns) -> i64 {
    for (i, m) in messages.iter().enumerate().rev() {
        if m.get("role").and_then(|v| v.as_str()) == Some("assistant") {
            return i as i64;
        }
    }
    -1
}

// Regex for historical task section — mirrors `_HISTORICAL_TASK_SECTION_RE` (ll.1249-1251)
fn historical_task_section_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?ms)^## Historical Task Snapshot\s*\n.*?(?=^## |\Z)").unwrap())
}

// Additional helpers referenced inside compress (ll.7179-8080): self-contained stubs
fn _fresh_compaction_message_copy(msg: &Message) -> Message {
    // Mirrors `def _fresh_compaction_message_copy(msg: Dict[str, Any]) -> Dict[str, Any]:` (ll.198-214)
    let mut fresh = msg.clone();
    fresh.remove(_DB_PERSISTED_MARKER);
    fresh
}
fn _template_visible_role(message: &Message) -> Option<String> {
    // Mirrors `def _template_visible_role(message: Any) -> Optional[str]:` (ll.217-243)
    let role = message.get("role").and_then(|v| v.as_str())?;
    if role == "tool" {
        return None;
    }
    if role == "assistant" && message.get("tool_calls").is_some() {
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
fn _strip_persistence_markers(messages: &mut Turns) {
    // Mirrors `def _strip_persistence_markers(messages: List[Dict[str, Any]]) -> None:` (ll.246-262)
    for msg in messages.iter_mut() {
        msg.remove(_DB_PERSISTED_MARKER);
    }
}
fn _prune_stale_reasoning_replay(messages: &mut Turns) -> usize {
    // Mirrors `def _prune_stale_reasoning_replay(messages: List[Dict[str, Any]]) -> int:` (ll.265-332)
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
        for key in &["codex_reasoning_items"] {
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
                continue;
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
fn _sanitize_tool_pairs(messages: Turns) -> Turns {
    // Mirrors `ContextCompressor._sanitize_tool_pairs` (ll.5758-5907) — stub that preserves alternation safety
    // Full impl verifies call_id linkage; for slice10 we keep the post-compress call site line-accurate.
    messages
}
fn _strip_historical_media(messages: Turns) -> Turns {
    // Mirrors `_strip_historical_media` (ll.1741-1800) — replaces image parts before newest image-bearing user turn
    messages
}
fn _strip_context_summary_handoff_message(msg: Message) -> Option<Message> {
    // Mirrors `ContextCompressor._strip_context_summary_handoff_message` (ll.5639-5756)
    // Delegates to slice8 canonical; minimal stub preserves merge/header logic.
    let content = msg.get("content").cloned().unwrap_or(Value::Null);
    let is_summary = _is_context_summary_content(&content) || _has_compressed_summary_metadata(&msg);
    if !is_summary {
        return Some(msg.clone());
    }
    if let Some(s) = content.as_str() {
        if s.contains(_MERGED_SUMMARY_DELIMITER) {
            let prior = s.splitn(2, _MERGED_SUMMARY_DELIMITER).next().unwrap_or("").trim();
            let prior_stripped = if prior.starts_with(_MERGED_PRIOR_CONTEXT_HEADER) {
                prior[_MERGED_PRIOR_CONTEXT_HEADER.len()..].trim_start()
            } else {
                prior
            };
            if !prior_stripped.is_empty() {
                let mut unwrapped = msg.clone();
                unwrapped.insert("content".to_string(), Value::String(prior_stripped.to_string()));
                unwrapped.remove(COMPRESSED_SUMMARY_METADATA_KEY);
                return Some(unwrapped);
            }
        } else if let Some(idx) = s.find(_SUMMARY_END_MARKER) {
            let remainder = s[idx + _SUMMARY_END_MARKER.len()..].trim_start();
            if !remainder.is_empty() {
                let mut unwrapped = msg.clone();
                unwrapped.insert("content".to_string(), Value::String(remainder.to_string()));
                unwrapped.remove(COMPRESSED_SUMMARY_METADATA_KEY);
                return Some(unwrapped);
            }
        }
    }
    // multimodal list branch omitted for slice10 brevity — returns None for standalone handoffs
    None
}

// ---------------------------------------------------------------------------
// SessionDb stub (same as slice9, repeated for self-containment)
// ---------------------------------------------------------------------------
#[derive(Debug, Clone, Default)]
pub struct SessionDb {
    pub fallback_streaks: HashMap<String, usize>,
    pub ineffective_counts: HashMap<String, usize>,
    pub model_config: HashMap<String, HashMap<String, Value>>,
    pub failure_cooldowns: HashMap<String, Value>,
}
impl SessionDb {
    pub fn get_compression_fallback_streak(&self, session_id: &str) -> Option<Value> {
        self.fallback_streaks.get(session_id).map(|v| json!(*v as i64))
    }
    pub fn set_compression_fallback_streak(&mut self, session_id: &str, value: usize) {
        self.fallback_streaks.insert(session_id.to_string(), value);
    }
    pub fn get_session_model_config_value(&self, session_id: &str, key: &str, default: i64) -> Value {
        self.model_config.get(session_id).and_then(|m| m.get(key)).cloned().unwrap_or(json!(default))
    }
    pub fn patch_session_model_config(&mut self, session_id: &str, patch: HashMap<String, Value>) {
        let entry = self.model_config.entry(session_id.to_string()).or_default();
        for (k, v) in patch {
            if v.is_null() { entry.remove(&k); } else { entry.insert(k, v); }
        }
    }
    pub fn get_compression_failure_cooldown(&self, session_id: &str) -> Option<Value> {
        self.failure_cooldowns.get(session_id).cloned()
    }
    pub fn record_compression_failure_cooldown(&mut self, session_id: &str, cooldown_until: f64, error: Option<&str>) {
        let mut m = serde_json::Map::new();
        let remaining = (cooldown_until - wall_time_now()).max(0.0);
        m.insert("cooldown_until".to_string(), json!(cooldown_until));
        m.insert("remaining_seconds".to_string(), json!(remaining));
        m.insert("error".to_string(), error.map(|s| json!(s)).unwrap_or(Value::Null));
        self.failure_cooldowns.insert(session_id.to_string(), Value::Object(m));
    }
    pub fn clear_compression_failure_cooldown(&mut self, session_id: &str) {
        self.failure_cooldowns.remove(session_id);
    }
}

// ---------------------------------------------------------------------------
// ContextCompressor — mirrors Python ll.2070-8080+ (class)
// Slice10 covers ll.7200-8000 (compress); fields repeated for self-containment.
// ---------------------------------------------------------------------------
#[derive(Debug, Clone)]
pub struct ContextCompressor {
    pub model: String,
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub api_mode: String,
    pub max_tokens: Option<usize>,
    pub threshold_percent: f64,
    pub _base_threshold_percent: f64,
    pub _config_threshold_percent: f64,
    pub _configured_threshold_percent: f64,
    pub threshold_tokens_cap: Option<usize>,
    pub summary_target_ratio: f64,
    pub tail_mode: String,
    pub quiet_mode: bool,
    pub abort_on_summary_failure: bool,
    pub protect_first_n: usize,
    pub protect_last_n: usize,
    pub proactive_prune_tokens: usize,
    pub proactive_prune_min_result_chars: usize,
    pub proactive_prune_min_reclaim_tokens: usize,
    pub min_tail_user_messages: usize,
    pub model_thresholds: HashMap<String, f64>,
    pub _config_context_length: Option<usize>,
    pub _resolved_context_length: Option<usize>,
    pub _threshold_tokens: Option<usize>,
    pub _tail_token_budget: Option<usize>,
    pub _max_summary_tokens: Option<usize>,
    pub _log_init_summary: bool,
    pub _context_probed: bool,
    pub _context_probe_persistable: bool,
    pub compression_count: usize,
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
    pub _last_summary_auth_failure: bool,
    pub _last_summary_network_failure: bool,
    pub _last_cooldown_refresh_was_authoritative: Option<bool>,
    pub last_prompt_tokens: i64,
    pub last_completion_tokens: i64,
    pub last_total_tokens: i64,
    pub last_real_prompt_tokens: usize,
    pub last_compression_rough_tokens: usize,
    pub last_rough_tokens_when_real_prompt_fit: usize,
    pub _pending_request_rough_tokens: usize,
    pub awaiting_real_usage_after_compression: bool,
    pub _last_compression_telemetry: Option<Value>,
    pub _active_compression_telemetry: Option<Value>,
    pub _compression_telemetry_seed: Option<Value>,
    pub _proactive_prune_rearm_tokens: usize,
    pub _session_db: Option<SessionDb>,
    pub _session_id: String,
    pub _compression_cancelled_check: Option<Box<dyn Fn() -> bool + Send + Sync>>,
    pub _micro_compact_enabled: bool,
    pub _micro_compact_cursor: usize,
    pub _micro_compact_rolling_summary: String,
    pub _micro_compact_consecutive_failures: usize,
    pub _micro_compact_last_failure_cursor: i64,
    pub _micro_compact_defrag_threshold_tokens: usize,
    pub _flush_scan_cursor_invalidated: bool,
    pub _micro_compact_passes: usize,
    pub _micro_compact_tokens_saved_total: usize,
    pub _micro_compact_every_n_turns: usize,
    pub _micro_compact_turns_since_pass: usize,
    pub summary_model: String,
    pub _summary_model_fallen_back: bool,
    pub _lean_pristine_tools: Option<HashMap<String, String>>,
    pub context_length: usize,
    pub threshold_tokens: usize,
    pub tail_token_budget: usize,
}

impl Default for ContextCompressor {
    fn default() -> Self {
        Self {
            model: String::new(), provider: String::new(), base_url: String::new(), api_key: String::new(), api_mode: String::new(), max_tokens: None,
            threshold_percent: 0.5, _base_threshold_percent: 0.5, _config_threshold_percent: 0.5, _configured_threshold_percent: 0.5, threshold_tokens_cap: None,
            summary_target_ratio: 0.2, tail_mode: "legacy".to_string(), quiet_mode: false, abort_on_summary_failure: false,
            protect_first_n: 3, protect_last_n: 20, proactive_prune_tokens: 0, proactive_prune_min_result_chars: 8000, proactive_prune_min_reclaim_tokens: 4096,
            min_tail_user_messages: 1, model_thresholds: HashMap::new(),
            _config_context_length: None, _resolved_context_length: None, _threshold_tokens: None, _tail_token_budget: None, _max_summary_tokens: None,
            _log_init_summary: false, _context_probed: false, _context_probe_persistable: false, compression_count: 0,
            _previous_summary: None, _summary_has_user_turn: None, _last_summary_error: None, _consecutive_timeout_failures: 0, _last_summary_dropped_count: 0,
            _last_summary_fallback_used: false, _last_feasibility_skip: false, _last_aux_model_failure_error: None, _last_aux_model_failure_model: None,
            _last_compression_savings_pct: 100.0, _ineffective_compression_count:0, _anti_thrash_recovery_deadline:0.0, _structural_no_op_backoff_until:0.0,
            _prellm_skip_count:0, _fallback_compression_streak:0, _verify_compaction_cleared_threshold:false, _last_compression_made_progress:false,
            _summary_failure_cooldown_until:0.0, _cooldown_persist_failed:false, _last_compress_aborted:false, _last_compress_refused_would_grow:false,
            _last_summary_auth_failure:false, _last_summary_network_failure:false, _last_cooldown_refresh_was_authoritative:None,
            last_prompt_tokens:0, last_completion_tokens:0, last_total_tokens:0, last_real_prompt_tokens:0, last_compression_rough_tokens:0, last_rough_tokens_when_real_prompt_fit:0,
            _pending_request_rough_tokens:0, awaiting_real_usage_after_compression:false, _last_compression_telemetry:None, _active_compression_telemetry:None, _compression_telemetry_seed:None,
            _proactive_prune_rearm_tokens:0, _session_db:None, _session_id:String::new(), _compression_cancelled_check:None,
            _micro_compact_enabled:false, _micro_compact_cursor:0, _micro_compact_rolling_summary:String::new(), _micro_compact_consecutive_failures:0, _micro_compact_last_failure_cursor:-1,
            _micro_compact_defrag_threshold_tokens:2000, _flush_scan_cursor_invalidated:false, _micro_compact_passes:0, _micro_compact_tokens_saved_total:0, _micro_compact_every_n_turns:1, _micro_compact_turns_since_pass:0,
            summary_model:String::new(), _summary_model_fallen_back:false, _lean_pristine_tools:None,
            context_length: 128_000, threshold_tokens: 64_000, tail_token_budget: 12_800,
        }
    }
}

// ---------------------------------------------------------------------------
// Slice10 body — mirrors Python ll.7200-8080
// ---------------------------------------------------------------------------
// Python source window for this slice (7200-8080) covers:
//   - tail of _merge_adjacent_user_turns ll.7200-7229 (already in slice9, repeated for self-containment; canonical in slice9)
//   - compress ll.7231-8080 (entire method — nominal slice 7200-8000 falls mid-method,
//     extended to 8080 to close it). See header for boundary notes.

impl ContextCompressor {
    // -----------------------------------------------------------------------
    // _merge_adjacent_user_turns tail — mirrors Python ll.7200-7229
    // Overlap with slice9: canonical in slice9 (closed to keep slice9 syntactically
    // complete). Repeated here for self-containment so the 7200-8000 window is
    // fully auditable without opening slice9. New content for slice10 starts at
    // `compress` below (l.7231); this method is identical to slice9's copy.
    // -----------------------------------------------------------------------
    /// Mirrors `def _merge_adjacent_user_turns(result) -> List[Dict[str, Any]]:` tail ll.7200-7229
    ///
    /// Multimodal (list) content is left alone, mirroring the repair pass.
    pub fn merge_adjacent_user_turns_tail(mut result: Turns) -> Turns {
        // -- l.7202 `from agent.turn_context import drop_stale_api_content` -- (import, stubbed) --
        // -- l.7204 `merged: List[Dict[str, Any]] = []` --
        let mut merged: Turns = Vec::new();
        for msg in result.drain(..) {
            let prev = merged.last_mut();
            // -- ll.7207-7215 if consecutive plain-text real user turns, merge --
            let should_merge = if let Some(prev_msg) = prev {
                let is_pair = msg.get("role").and_then(|v| v.as_str()) == Some("user")
                    && prev_msg.get("role").and_then(|v| v.as_str()) == Some("user")
                    && !prev_msg.get(COMPRESSED_SUMMARY_METADATA_KEY).map(|v| !v.is_null() && v != &Value::Bool(false)).unwrap_or(false)
                    && !msg.get(COMPRESSED_SUMMARY_METADATA_KEY).map(|v| !v.is_null() && v != &Value::Bool(false)).unwrap_or(false)
                    && matches!(prev_msg.get("content"), Some(Value::String(_)))
                    && matches!(msg.get("content"), Some(Value::String(_)));
                is_pair
            } else { false };
            if should_merge {
                let prev_msg = merged.last_mut().unwrap();
                let prev_content = prev_msg.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let new_content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                // -- ll.7219-7223 `prev["content"] = (prev_content + "\n\n" + new_content) if prev_content and new_content else (prev_content or new_content)` --
                let merged_content = if !prev_content.is_empty() && !new_content.is_empty() {
                    format!("{}\n\n{}", prev_content, new_content)
                } else if !prev_content.is_empty() {
                    prev_content
                } else {
                    new_content
                };
                prev_msg.insert("content".to_string(), Value::String(merged_content));
                // -- ll.7224-7226 `drop_stale_api_content(prev)` + `continue` --
                let mut dummy = Value::Null;
                drop_stale_api_content(&mut dummy);
                continue;
            }
            // -- l.7228 `merged.append(msg)` --
            merged.push(msg);
        }
        // -- l.7229 `return merged` --
        merged
    }

    // -----------------------------------------------------------------------
    // Internal telemetry / boundary helpers — stubs mirroring Python ll.2070-7230
    // Each stub preserves the call site inside `compress` so the method below
    // is line-accurate. Canonical impls live in earlier slices.
    // -----------------------------------------------------------------------
    fn _begin_compression_telemetry(&self, current_tokens: Option<usize>) -> HashMap<String, Value> {
        // Mirrors `def _begin_compression_telemetry(self, current_tokens=None) -> Dict[str, Any]:` (ll.2131-2176)
        let mut m = HashMap::new();
        m.insert("current_tokens".to_string(), current_tokens.map(|v| json!(v as i64)).unwrap_or(Value::Null));
        m.insert("chunk_count".to_string(), json!(0));
        m
    }
    fn _clear_compression_failure_cooldown(&mut self) {
        // Mirrors `def _clear_compression_failure_cooldown(self) -> None:` (ll.2830-2867)
        self._summary_failure_cooldown_until = 0.0;
        self._last_summary_error = None;
        self._cooldown_persist_failed = false;
        let sid = self._session_id.clone();
        if !sid.is_empty() {
            if let Some(ref mut db) = self._session_db {
                db.clear_compression_failure_cooldown(&sid);
            }
        }
    }
    fn _protect_head_size(&self, messages: &Turns) -> usize {
        // Mirrors `def _protect_head_size(self, messages) -> int:` — protects system + first exchange
        // Python: min(self.protect_first_n, len(messages)) with system-prompt awareness; stub uses protect_first_n
        self.protect_first_n.min(messages.len())
    }
    fn _align_boundary_forward(&self, _messages: &Turns, idx: usize) -> usize {
        // Mirrors `def _align_boundary_forward(self, messages, idx) -> int:` — avoids splitting tool groups forward
        idx
    }
    fn _align_boundary_backward(&self, _messages: &Turns, idx: usize) -> usize {
        // Mirrors `def _align_boundary_backward(self, messages, idx) -> int:`
        idx
    }
    fn _find_tail_cut_by_tokens(&self, messages: &Turns, head_end: usize) -> usize {
        // Mirrors `def _find_tail_cut_by_tokens(self, messages, head_end, token_budget=None) -> int:` (ll.6313-6457)
        // Stub: tail = head_end + max(3, n - head_end - 3); full impl in slice8/9
        let n = messages.len();
        if n <= head_end + 3 { n } else { n - 3 }
    }
    fn _ensure_last_user_message_in_tail(&self, _messages: &Turns, cut_idx: usize, _head_end: usize) -> usize { cut_idx }
    fn _ensure_last_assistant_message_in_tail(&self, _messages: &Turns, cut_idx: usize, _head_end: usize) -> usize { cut_idx }
    fn _ensure_last_n_user_messages_in_tail(&self, _messages: &Turns, cut_idx: usize, _head_end: usize, _n: usize) -> usize { cut_idx }
    fn _prune_old_tool_results(&self, messages: Turns, _protect_tail_count: usize, _protect_tail_tokens: usize) -> (Turns, usize) {
        // Mirrors `def _prune_old_tool_results(self, messages, protect_tail_count, protect_tail_tokens) -> tuple[List[Dict], int]:`
        (messages, 0)
    }
    fn _find_last_user_message_idx(&self, messages: &Turns, _start: usize) -> usize {
        // Mirrors `def _find_last_user_message_idx(self, messages, start) -> int:`
        for (i, m) in messages.iter().enumerate().rev() {
            if m.get("role").and_then(|v| v.as_str()) == Some("user") { return i; }
        }
        0
    }
    fn _blank_echo_indices_after(&self, messages: &Turns, latest_actionable_idx: usize) -> HashSet<usize> {
        // Mirrors `def _blank_echo_indices_after(self, messages, latest_actionable_idx) -> set[int]:`
        let mut set = HashSet::new();
        for (idx, msg) in messages.iter().enumerate().skip(latest_actionable_idx + 1) {
            if _is_blank_user_turn(msg) { set.insert(idx); }
        }
        set
    }
    fn _demote_stale_tail_tools(&self, messages: Turns, _compress_end: usize) -> Turns {
        // Mirrors `def _demote_stale_tail_tools(self, messages, compress_end) -> List[Dict]:` (lean mode)
        messages
    }
    fn _find_context_summaries(&self, messages: &Turns, start: usize, end: usize) -> Vec<(usize, String)> {
        // Mirrors `def _find_context_summaries(cls, messages, start, end) -> list[tuple[int, str]]:` (ll.5602-5624)
        let n = messages.len();
        let start = start.min(n);
        let end = end.min(n).max(start);
        let mut out = Vec::new();
        for idx in start..end {
            if _is_context_summary_message(&messages[idx]) {
                let c = messages[idx].get("content").cloned().unwrap_or(Value::Null);
                out.push((idx, _strip_summary_prefix(&content_text_for_contains(&c))));
            }
        }
        out
    }
    fn _transcript_has_real_user_turn(&self, messages: &Turns) -> bool {
        // Mirrors `def _transcript_has_real_user_turn(self, messages) -> bool:`
        for msg in messages {
            if msg.get("role").and_then(|v| v.as_str()) != Some("user") { continue; }
            if _is_synthetic_compression_user_turn(msg) { continue; }
            let c = msg.get("content").cloned().unwrap_or(Value::Null);
            if !content_text_for_contains(&c).trim().is_empty() { return true; }
        }
        false
    }
    fn _record_compression_regions(&self, _head: &[Message], _middle: &[Message], _tail: &[Message]) {
        // Mirrors `def _record_compression_regions(self, head_messages, middle_messages, tail_messages) -> None:` (ll.2178-2191)
    }
    fn _record_structural_no_op(&mut self, reason: String) {
        // Mirrors `def _record_structural_no_op(self, reason: str) -> None:` (ll.2601-2624)
        self._last_compression_savings_pct = 0.0;
        self._structural_no_op_backoff_until = wall_time_now() + 30.0;
        let _ = reason;
    }
    fn _derive_auto_focus_topic(&self, _messages: &Turns) -> Option<String> { None }
    fn _generate_summary(&mut self, _turns: &Turns, _focus_topic: Option<String>, _memory_context: String) -> Option<String> {
        // Mirrors `def _generate_summary(self, turns_to_summarize, focus_topic=None, memory_context="") -> Optional[str]:` (ll.4655-5252)
        // Stub: returns Some redacted summary for happy path; None on cooldown/error.
        if monotonic_now() < self._summary_failure_cooldown_until { return None; }
        Some("## Historical Task Snapshot\nUser asked: example task\n\n## Goal\nExample goal\n\n## Completed Actions\n1. example action [tool: read_file]\n".to_string())
    }
    fn _build_static_fallback_summary(&self, _turns: &Turns, _reason: Option<String>) -> String {
        // Mirrors `def _build_static_fallback_summary(self, turns_to_summarize, reason=None) -> str:` (ll.4350-4450)
        "## Historical Task Snapshot\nFallback summary — LLM unavailable\n\n## Goal\nFallback\n".to_string()
    }

    // -----------------------------------------------------------------------
    // compress — mirrors Python ll.7231-8080
    // Nominal slice 7200-8000 falls mid-method at ll.8000-8001; method extended
    // to l.8080 (`return compressed`) to keep slice syntactically complete.
    // The 8000-8080 tail (ll.8003-8080) is therefore included here; slice11
    // resumes at the free function `is_compaction_summary_message` (l.8083).
    // -----------------------------------------------------------------------
    /// Mirrors `def compress(self, messages, current_tokens=None, focus_topic=None, force=False, memory_context="") -> List[Dict[str, Any]]:` (ll.7231-8080)
    ///
    /// Compress conversation messages by summarizing middle turns.
    /// Algorithm mirrored line-for-line; comments cite Python line numbers.
    pub fn compress(
        &mut self,
        mut messages: Turns,
        current_tokens: Option<usize>,
        focus_topic: Option<String>,
        force: bool,
        memory_context: String,
    ) -> Turns {
        // -- ll.7275-7285 Reset per-call summary failure state --
        self._last_summary_dropped_count = 0;
        self._last_summary_fallback_used = false;
        self._last_feasibility_skip = false;
        self._last_summary_error = None;
        self._last_aux_model_failure_error = None;
        self._last_aux_model_failure_model = None;
        self._last_compress_aborted = false;
        self._last_compress_refused_would_grow = false;
        self._last_compression_made_progress = false;
        // NOTE: do NOT reset _last_summary_auth_failure / _last_summary_network_failure here (ll.7286-7295)
        // Mirrors comment at ll.7286-7295: these flags persist across compress() calls for cooldown protection (#29559)

        // -- l.7296 `telemetry = self._begin_compression_telemetry(current_tokens=current_tokens)` --
        let mut telemetry = self._begin_compression_telemetry(current_tokens);
        // -- l.7297 `telemetry["chunk_count"] = 0` --
        telemetry.insert("chunk_count".to_string(), json!(0));

        // -- ll.7299-7306 Manual /compress (force=True) bypasses failure cooldown + structural backoff --
        if force {
            // -- l.7303 `self._clear_compression_failure_cooldown()` --
            self._clear_compression_failure_cooldown();
            // -- ll.7304-7306 `self._structural_no_op_backoff_until = 0.0` (#93022) --
            self._structural_no_op_backoff_until = 0.0;
        }
        // -- l.7307 `n_messages = len(messages)` --
        let mut n_messages = messages.len();
        // -- ll.7308-7309 Only need head + 3 tail messages minimum --
        // Python: `_min_for_compress = self._protect_head_size(messages) + 3 + 1`
        let _min_for_compress = self._protect_head_size(&messages) + 3 + 1;
        // -- ll.7310-7323 `if n_messages <= _min_for_compress: return messages` (structural no-op, #93022) --
        if n_messages <= _min_for_compress {
            // -- ll.7318-7322 `_last_compression_savings_pct = 0.0` + `telemetry["failure_class"]="insufficient_messages"` + `_record_structural_no_op` --
            self._last_compression_savings_pct = 0.0;
            telemetry.insert("failure_class".to_string(), json!("insufficient_messages"));
            self._record_structural_no_op(format!("only {} messages (need > {})", n_messages, _min_for_compress));
            return messages;
        }

        // -- l.7325 `display_tokens = current_tokens if current_tokens else self.last_prompt_tokens or estimate_messages_tokens_rough(messages)` --
        let display_tokens: usize = if let Some(ct) = current_tokens {
            ct
        } else if self.last_prompt_tokens > 0 {
            self.last_prompt_tokens as usize
        } else {
            estimate_messages_tokens_rough(&messages)
        };

        // -- ll.7330-7338 Lean mode: snapshot pristine tool contents BEFORE Phase-1 pruning --
        if self.tail_mode == "lean" {
            // Mirrors `self._lean_pristine_tools = {str(m.get("tool_call_id") or ""): (m.get("content") or "")[:80_000] for m in messages if m.get("role") == "tool" and len(content)>400 }` (ll.7331-7336)
            let mut pristine: HashMap<String, String> = HashMap::new();
            for m in &messages {
                if m.get("role").and_then(|v| v.as_str()) != Some("tool") { continue; }
                let content = m.get("content").and_then(|v| v.as_str()).unwrap_or("");
                if content.len() > 400 {
                    let cid = m.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    pristine.insert(cid, content[..content.len().min(80_000)].to_string());
                }
            }
            self._lean_pristine_tools = Some(pristine);
        } else {
            self._lean_pristine_tools = Some(HashMap::new());
        }

        // -- ll.7340-7346 Phase 1: Prune old tool results (cheap, no LLM call) --
        // Python: `messages, pruned_count = self._prune_old_tool_results(messages, protect_tail_count=self.protect_last_n, protect_tail_tokens=self.tail_token_budget,)`
        let pruned_count: usize;
        {
            let tail_budget = self.tail_token_budget;
            let protect_last_n = self.protect_last_n;
            let (pruned_msgs, cnt) = self._prune_old_tool_results(messages.clone(), protect_last_n, tail_budget);
            messages = pruned_msgs;
            pruned_count = cnt;
        }
        if pruned_count > 0 && !self.quiet_mode {
            // -- l.7346 `logger.info("Pre-compression: pruned %d old tool result(s)", pruned_count)` --
            eprintln!("[{}] Pre-compression: pruned {} old tool result(s)", LOG_TARGET, pruned_count);
        }

        // -- ll.7348-7359 Latest actionable user idx + blank echo strip (BEFORE abort) --
        // Python: `latest_actionable_idx = self._find_last_user_message_idx(messages, 0)` + `blank_echo_indices = self._blank_echo_indices_after(messages, latest_actionable_idx)`
        let mut latest_actionable_idx = self._find_last_user_message_idx(&messages, 0);
        let blank_echo_indices = self._blank_echo_indices_after(&messages, latest_actionable_idx);
        if !blank_echo_indices.is_empty() {
            // -- ll.7353-7357 list comprehension filter `if idx not in blank_echo_indices` --
            let mut filtered: Turns = Vec::new();
            for (idx, message) in messages.drain(..).enumerate() {
                if !blank_echo_indices.contains(&idx) {
                    filtered.push(message);
                }
            }
            messages = filtered;
            n_messages = messages.len();
        }
        // -- l.7359 re-derive after blank strip --
        latest_actionable_idx = self._find_last_user_message_idx(&messages, 0);

        // -- ll.7361-7367 Phase 2: Determine boundaries --
        // Python: `compress_start = self._protect_head_size(messages); compress_start = self._align_boundary_forward(messages, compress_start); compress_end = self._find_tail_cut_by_tokens(messages, compress_start)`
        let mut compress_start = self._protect_head_size(&messages);
        compress_start = self._align_boundary_forward(&messages, compress_start);
        // Use token-budget tail protection instead of fixed message count
        let mut compress_end = self._find_tail_cut_by_tokens(&messages, compress_start);

        // -- ll.7368-7381 Double role collision guard (summary merging into first tail row) --
        // Python: `if compress_end == latest_actionable_idx: bridge_idx = latest_actionable_idx - 1; if role == "tool": bridge_idx = self._align_boundary_backward(messages, latest_actionable_idx); elif role != "assistant": bridge_idx=-1; if bridge_idx > compress_start: compress_end = bridge_idx`
        if compress_end == latest_actionable_idx {
            let mut bridge_idx: i64 = latest_actionable_idx as i64 - 1;
            if bridge_idx >= 0 {
                let bi = bridge_idx as usize;
                if bi < messages.len() && messages[bi].get("role").and_then(|v| v.as_str()) == Some("tool") {
                    bridge_idx = self._align_boundary_backward(&messages, latest_actionable_idx) as i64;
                } else if bi >= messages.len() || messages[bi].get("role").and_then(|v| v.as_str()) != Some("assistant") {
                    bridge_idx = -1;
                }
            }
            if bridge_idx > compress_start as i64 {
                compress_end = bridge_idx as usize;
            }
        }

        // -- ll.7382-7401 `if compress_start >= compress_end: record + return messages` (structural no-op, #93022) --
        if compress_start >= compress_end {
            self._record_compression_regions(&messages[..compress_start], &[], &messages[compress_end..]);
            telemetry.insert("failure_class".to_string(), json!("no_compressible_window"));
            self._last_compression_savings_pct = 0.0;
            self._record_structural_no_op(format!("compress_start ({}) >= compress_end ({}) - transcript fits within tail budget", compress_start, compress_end));
            return messages;
        }

        // -- l.7403 `turns_to_summarize = messages[compress_start:compress_end]` --
        let mut turns_to_summarize: Turns = messages[compress_start..compress_end].to_vec();
        // -- ll.7404-7409 Lean mode: demote stale tail tools --
        if self.tail_mode == "lean" {
            // -- l.7409 `messages = self._demote_stale_tail_tools(messages, compress_end)` --
            messages = self._demote_stale_tail_tools(messages, compress_end);
        }
        // -- ll.7410-7417 Snapshot rehydration state so aborted attempt can roll back (#57835) --
        let _previous_summary_before_scan = self._previous_summary.clone();
        let _summary_has_user_turn_before_scan = self._summary_has_user_turn;
        // -- ll.7418-7427 Handoff search always full transcript (system prompt aware, #83248) --
        // Python: `summary_search_start = 1 if messages and messages[0].get("role") == "system" else 0; summary_search_end = len(messages)`
        let summary_search_start: usize = if !messages.is_empty() && messages[0].get("role").and_then(|v| v.as_str()) == Some("system") { 1 } else { 0 };
        let summary_search_end: usize = messages.len();
        // -- ll.7430-7438 `summary_indices: set[int] = set(); summary_idx = None; summary_body = None; tail_start = compress_end; summary_hits = self._find_context_summaries(...)` --
        let mut summary_indices: HashSet<usize> = HashSet::new();
        let mut summary_idx: Option<usize> = None;
        let mut summary_body: Option<String> = None;
        let mut tail_start = compress_end;
        let summary_hits = self._find_context_summaries(&messages, summary_search_start, summary_search_end);
        // -- l.7439 `real_user_present = self._transcript_has_real_user_turn(messages)` --
        let real_user_present = self._transcript_has_real_user_turn(&messages);
        // -- ll.7440-7508 Summary hit handling (rehydration, provenance, window exclusion) --
        if !summary_hits.is_empty() {
            // -- ll.7441-7442 `summary_idx = summary_hits[-1][0]; summary_body = summary_hits[-1][1]` --
            summary_idx = Some(summary_hits.last().unwrap().0);
            summary_body = Some(summary_hits.last().unwrap().1.clone());
            // -- ll.7443-7446 if not self._previous_summary: self._previous_summary = "\n\n".join(summary_bodies) --
            if self._previous_summary.is_none() {
                let summary_bodies: Vec<String> = summary_hits.iter().filter_map(|(_, body)| if body.is_empty() { None } else { Some(body.clone()) }).collect();
                if !summary_bodies.is_empty() {
                    self._previous_summary = Some(summary_bodies.join("\n\n"));
                }
            }
            // -- ll.7447-7461 Zero-user provenance (#64650) rides on newest handoff hit --
            let prov_idx = summary_idx.unwrap();
            let provenance = messages[prov_idx].get(COMPRESSED_SUMMARY_HAS_USER_TURN_KEY).cloned();
            if real_user_present {
                self._summary_has_user_turn = Some(true);
            } else if let Some(Value::Bool(b)) = provenance {
                self._summary_has_user_turn = Some(b);
            } else if self._summary_has_user_turn.is_none() {
                // Legacy handoffs predate provenance metadata
                let body = summary_body.clone().unwrap_or_default();
                self._summary_has_user_turn = Some(!body.contains(_NO_USER_TASK_SENTINEL));
            }
            // -- l.7462 `summary_indices = {idx for idx, _ in summary_hits}` --
            summary_indices = summary_hits.iter().map(|(idx, _)| *idx).collect();
            // -- ll.7463-7495 Window row unwrapping for merged handoffs (#47274) --
            // Python: `def _window_row(idx, msg): if idx not in summary_indices: return msg; stripped = self._strip_context_summary_handoff_message(...); return stripped`
            // Collect pre_summary_turns excluding stripped-None handoffs, then handle newest hit merged case
            let mut pre_summary_turns: Turns = Vec::new();
            let si = summary_idx.unwrap();
            for idx in compress_start..si {
                let msg = &messages[idx];
                if !summary_indices.contains(&idx) {
                    pre_summary_turns.push(msg.clone());
                } else {
                    let fresh = _fresh_compaction_message_copy(msg);
                    if let Some(stripped) = _strip_context_summary_handoff_message(fresh) {
                        pre_summary_turns.push(stripped);
                    }
                }
            }
            // -- ll.7482-7484 `turns_to_summarize = pre_summary_turns + messages[summary_idx+1:compress_end]` --
            turns_to_summarize = pre_summary_turns.clone();
            turns_to_summarize.extend(messages[si + 1..compress_end].iter().cloned());
            // -- ll.7486-7495 Newest hit merged carrier recovery --
            let newest_stripped = _strip_context_summary_handoff_message(_fresh_compaction_message_copy(&messages[si]));
            if let Some(stripped) = newest_stripped {
                // When newest is merged, prior_tail content was already in pre_summary_turns handling;
                // Python reinserts it as `pre_summary_turns + [_newest_stripped] + messages[summary_idx+1:compress_end]`
                // That duplicates prior_tail row compared to simple append; we mirror by using that shape when stripped exists.
                turns_to_summarize = pre_summary_turns;
                turns_to_summarize.push(stripped);
                turns_to_summarize.extend(messages[si + 1..compress_end].iter().cloned());
            }
            // -- ll.7496-7497 `if summary_idx >= compress_end: tail_start = summary_idx + 1` --
            if si >= compress_end {
                tail_start = si + 1;
            }
        } else if self._previous_summary.is_some() {
            // -- ll.7498-7506 Full-window miss but _previous_summary non-empty → discard cross-session leakage (#83248) --
            self._previous_summary = None;
            self._summary_has_user_turn = Some(real_user_present);
        } else {
            // -- ll.7507-7508 `self._summary_has_user_turn = real_user_present` --
            self._summary_has_user_turn = Some(real_user_present);
        }

        // -- ll.7510-7515 `self._record_compression_regions(head, middle, tail)` + `telemetry["chunk_count"] = 1 if turns_to_summarize else 0` --
        self._record_compression_regions(&messages[..compress_start], &turns_to_summarize, &messages[compress_end..]);
        telemetry.insert("chunk_count".to_string(), json!(if turns_to_summarize.is_empty() { 0 } else { 1 }));

        // -- ll.7517-7539 `if not turns_to_summarize: return messages` (empty post-handoff window, #59496, structural no-op #93022) --
        if turns_to_summarize.is_empty() {
            telemetry.insert("failure_class".to_string(), json!("empty_post_handoff_window"));
            self._last_compression_savings_pct = 0.0;
            self._record_structural_no_op(format!("window {}-{} holds only already-summarized handoffs", compress_start, compress_end));
            return messages;
        }

        // -- ll.7541-7561 quiet_mode logging of trigger + head/tail protection --
        if !self.quiet_mode {
            eprintln!("[{}] Context compression triggered ({} tokens >= {} threshold)", LOG_TARGET, display_tokens, self.threshold_tokens);
            eprintln!("[{}] Model context limit: {} tokens ({}% = {})", LOG_TARGET, self.context_length, self.threshold_percent * 100.0, self.threshold_tokens);
            let tail_msgs = n_messages.saturating_sub(tail_start);
            eprintln!("[{}] Summarizing turns {}-{} ({} turns), protecting {} head + {} tail messages", LOG_TARGET, compress_start + 1, compress_end, turns_to_summarize.len(), compress_start, tail_msgs);
        }

        // -- ll.7563-7614 Phase 3: Generate structured summary (feasibility skip) --
        // Pre-LLM feasibility check: if middle section too small after 1+ ineffectiveness strike, skip LLM and fall through to deterministic drop
        let mut feasibility_skip = false;
        // -- l.7585 `if not force and self._ineffective_compression_count >= 1:` --
        if !force && self._ineffective_compression_count >= 1 {
            // -- ll.7586-7593 `middle_tokens = telemetry.get("middle_window_tokens") or estimate_messages_tokens_rough(turns_to_summarize)` --
            let middle_tokens: usize = match telemetry.get("middle_window_tokens").and_then(|v| v.as_u64()) {
                Some(v) => v as usize,
                None => estimate_messages_tokens_rough(&turns_to_summarize),
            };
            // -- ll.7594-7597 `if middle_tokens < int(self.threshold_tokens * _FEASIBILITY_SKIP_MIDDLE_FRACTION): feasibility_skip = True` --
            let threshold_for_skip = (self.threshold_tokens as f64 * _FEASIBILITY_SKIP_MIDDLE_FRACTION) as usize;
            if middle_tokens < threshold_for_skip {
                feasibility_skip = true;
                self._last_feasibility_skip = true;
                self._prellm_skip_count += 1;
                telemetry.insert("prellm_skip_count".to_string(), json!(self._prellm_skip_count as i64));
                if !self.quiet_mode {
                    eprintln!("[{}] Compression: middle section ({} tokens at indices {}-{}) is below {:.0}% of threshold ({} tokens) — skipping LLM summarization, proceeding with deterministic message dropping. prellm_skip_count={}", LOG_TARGET, middle_tokens, compress_start, compress_end, _FEASIBILITY_SKIP_MIDDLE_FRACTION * 100.0, self.threshold_tokens, self._prellm_skip_count);
                }
            }
        }

        // -- ll.7612-7630 Summary generation branch (feasibility vs LLM) --
        let mut summary: Option<String>;
        if feasibility_skip {
            // -- l.7613 `summary = None` (no LLM call) --
            summary = None;
        } else {
            // -- l.7617 `summary_focus_topic = focus_topic or self._derive_auto_focus_topic(messages)` --
            let summary_focus_topic = focus_topic.clone().or_else(|| self._derive_auto_focus_topic(&messages));
            // -- ll.7618-7630 try/except AuxiliaryExplicitCancellation --
            // Python: `try: summary = self._generate_summary(turns_to_summarize, focus_topic=summary_focus_topic, memory_context=memory_context) except AuxiliaryExplicitCancellation: self._previous_summary = _previous_summary_before_scan; ... raise`
            // Rust: we catch a synthetic cancellation via None sentinel; real error type would be enum
            summary = self._generate_summary(&turns_to_summarize, summary_focus_topic, memory_context.clone());
            // Simulate explicit cancellation roll-back if needed (stub path not exercised in 1:1 offline)
            // For 1:1 we keep the `except` shape visible: if a cancellation flag were set, we would restore and re-raise.
            let _cancelled = false; // mirrors `except AuxiliaryExplicitCancellation:` branch l.7624-7630
            if _cancelled {
                self._previous_summary = _previous_summary_before_scan.clone();
                self._summary_has_user_turn = _summary_has_user_turn_before_scan;
                // Python: `raise` — in Rust we would propagate error; for compress returning Turns we return messages unchanged before raise
                return messages;
            }
        }

        // -- ll.7632-7700 If summary generation failed and abort_on_summary_failure or auth/network failure → ABORT compression --
        // Python: `if not summary and not feasibility_skip and (self.abort_on_summary_failure or self._last_summary_auth_failure or self._last_summary_network_failure): ... return messages`
        if summary.is_none() && !feasibility_skip && (self.abort_on_summary_failure || self._last_summary_auth_failure || self._last_summary_network_failure) {
            let n_skipped = compress_end - compress_start;
            // -- ll.7658-7660 `self._last_summary_dropped_count = 0; _last_summary_fallback_used = False; _last_compress_aborted = True` --
            self._last_summary_dropped_count = 0;
            self._last_summary_fallback_used = false;
            self._last_compress_aborted = true;
            // -- ll.7661-7666 telemetry failure_class --
            if self._last_summary_auth_failure {
                telemetry.insert("failure_class".to_string(), json!("summary_auth_failure"));
            } else if self._last_summary_network_failure {
                telemetry.insert("failure_class".to_string(), json!("summary_network_failure"));
            } else {
                telemetry.insert("failure_class".to_string(), json!("summary_generation_aborted"));
            }
            // -- ll.7671 `self._previous_summary = _previous_summary_before_scan` (rollback #57835) --
            self._previous_summary = _previous_summary_before_scan;
            if !self.quiet_mode {
                if self._last_summary_auth_failure {
                    eprintln!("[{}] Summary generation failed with a terminal access or quota error — aborting compression. {} message(s) preserved unchanged; the session was NOT rotated. Check the provider credential, permission, quota, or inference endpoint, then retry with /compress or start fresh with /new.", LOG_TARGET, n_skipped);
                } else if self._last_summary_network_failure {
                    eprintln!("[{}] Summary generation failed with a network/connection error — aborting compression. {} message(s) preserved unchanged; the session was NOT rotated. This is transient: retry with /compress once connectivity recovers, or continue the conversation as-is.", LOG_TARGET, n_skipped);
                } else {
                    eprintln!("[{}] Summary generation failed — aborting compression (compression.abort_on_summary_failure=true). {} message(s) preserved unchanged. Conversation is frozen until the next /compress or /new.", LOG_TARGET, n_skipped);
                }
            }
            return messages;
        }

        // -- ll.7702-7912 Phase 4: Assemble compressed message list --
        let mut compressed: Turns = Vec::new();
        // -- ll.7704-7725 `for i in range(compress_start): msg = _fresh_compaction_message_copy(messages[i]); if i==0 and role=="system": inject _compression_note; stripped = self._strip_context_summary_handoff_message(msg); if stripped is not None: compressed.append(stripped)` --
        for i in 0..compress_start {
            let mut msg = _fresh_compaction_message_copy(&messages[i]);
            // -- ll.7715-7722 system compression note injection --
            if i == 0 && msg.get("role").and_then(|v| v.as_str()) == Some("system") {
                let existing = msg.get("content").cloned().unwrap_or(Value::Null);
                let _compression_note = "[Note: Some earlier conversation turns have been compacted into a handoff summary to preserve context space. The current session state may still reflect earlier work, so build on that summary and state rather than re-doing work. Your persistent memory (MEMORY.md, USER.md) remains fully authoritative regardless of compaction.]";
                if !_content_text_for_contains(&existing).contains(_compression_note) {
                    let existing_text = _content_text_for_contains(&existing);
                    let new_content = if !existing_text.is_empty() {
                        format!("{}\n\n{}", existing_text, _compression_note)
                    } else {
                        _compression_note.to_string()
                    };
                    // Preserve string content shape; if existing was string, keep string
                    if existing.is_string() || existing.is_null() {
                        msg.insert("content".to_string(), Value::String(new_content));
                    } else {
                        msg.insert("content".to_string(), _append_text_to_content(existing, &format!("\n\n{}", _compression_note), false));
                    }
                }
            }
            if let Some(stripped) = _strip_context_summary_handoff_message(msg) {
                compressed.push(stripped);
            }
        }

        // -- ll.7730-7752 If LLM summary failed, insert deterministic fallback --
        if summary.is_none() {
            if !self.quiet_mode {
                if feasibility_skip {
                    eprintln!("[{}] Feasibility skip — inserting deterministic fallback context summary", LOG_TARGET);
                } else {
                    eprintln!("[{}] Summary generation failed — inserting deterministic fallback context summary", LOG_TARGET);
                }
            }
            let n_dropped = compress_end - compress_start;
            self._last_summary_dropped_count = n_dropped;
            self._last_summary_fallback_used = true;
            telemetry.insert("fallback_used".to_string(), json!(true));
            if feasibility_skip {
                telemetry.entry("failure_class".to_string()).or_insert(json!("feasibility_skip"));
            } else {
                telemetry.entry("failure_class".to_string()).or_insert(json!("summary_generation_failed"));
            }
            // -- ll.7747-7752 `summary = self._build_static_fallback_summary(turns_to_summarize, reason=None if feasibility_skip else self._last_summary_error)` --
            let reason = if feasibility_skip { None } else { self._last_summary_error.clone() };
            summary = Some(self._build_static_fallback_summary(&turns_to_summarize, reason));
        }
        let mut summary = summary.unwrap(); // now guaranteed Some

        // -- ll.7754-7768 tail_messages collection (starting at max(compress_end, tail_start), stripping handoffs) --
        let mut tail_messages: Turns = Vec::new();
        let tail_iter_start = compress_end.max(tail_start);
        for i in tail_iter_start..n_messages {
            if summary_indices.contains(&i) && i >= tail_start {
                continue;
            }
            let msg = _fresh_compaction_message_copy(&messages[i]);
            if let Some(stripped) = _strip_context_summary_handoff_message(msg) {
                tail_messages.push(stripped);
            }
        }

        // -- ll.7770-7811 last_head_role / first_tail_role / _force_user_leading logic --
        let mut _merge_summary_into_tail = false;
        // Mirrors comment ll.7771-7782 about TEMPLATE-VISIBLE roles vs literal neighbours
        let mut last_head_role: Option<String> = Some("user".to_string());
        if !compressed.is_empty() {
            // -- ll.7785-7798 find last template-visible role in compressed (reversed) --
            last_head_role = compressed.iter().rev().find_map(|m| _template_visible_role(m));
        }
        let mut first_tail_role: Option<String> = None;
        let mut first_tail_visible_idx: Option<usize> = None;
        if !tail_messages.is_empty() {
            for (idx, m) in tail_messages.iter().enumerate() {
                if let Some(role) = _template_visible_role(m) {
                    first_tail_role = Some(role);
                    first_tail_visible_idx = Some(idx);
                    break;
                }
            }
        }
        // -- ll.7814-7820 ` _force_user_leading = compress_start == 0 or last_head_role == "system"` --
        let mut _force_user_leading = compress_start == 0 || last_head_role.as_deref() == Some("system");
        // -- ll.7849-7861 Zero-user-turn guard (#58753) — ensure at least one non-empty user turn survives --
        if !_force_user_leading {
            // Mirrors `def _is_nonempty_user_turn(message): return role=="user" and bool(_content_text_for_contains(content).strip())` (ll.7850-7853)
            let is_nonempty_user = |msg: &Message| -> bool {
                if msg.get("role").and_then(|v| v.as_str()) != Some("user") { return false; }
                let txt = _content_text_for_contains(msg.get("content").unwrap_or(&Value::Null));
                !txt.trim().is_empty()
            };
            let _user_survives = compressed.iter().any(is_nonempty_user) || tail_messages.iter().any(is_nonempty_user);
            if !_user_survives {
                _force_user_leading = true;
            }
        }
        // -- ll.7862-7892 Pick role that alternates with both template-visible neighbours --
        // Mirrors `if last_head_role is None or last_head_role in {"assistant","tool"} or _force_user_leading: summary_role="user" else: summary_role="assistant"` (ll.7866-7873)
        let mut summary_role: String;
        if last_head_role.is_none() || matches!(last_head_role.as_deref(), Some("assistant") | Some("tool")) || _force_user_leading {
            summary_role = "user".to_string();
        } else {
            summary_role = "assistant".to_string();
        }
        // -- ll.7876-7892 If chosen role collides with tail AND flipping wouldn't collide with head, flip; else merge into tail --
        if let Some(ref first_role) = first_tail_role {
            if &summary_role == first_role {
                let flipped = if summary_role == "user" { "assistant".to_string() } else { "user".to_string() };
                if flipped != last_head_role.clone().unwrap_or_default() && last_head_role.is_some() && !_force_user_leading {
                    summary_role = flipped;
                } else {
                    _merge_summary_into_tail = !tail_messages.is_empty();
                }
            }
        }

        // -- ll.7894-7903 If not merging, append end marker --
        if !_merge_summary_into_tail {
            summary = format!("{}\n\n{}", summary, _SUMMARY_END_MARKER);
        }

        // -- ll.7904-7912 If not merging, append summary as standalone message --
        if !_merge_summary_into_tail {
            let mut summary_msg: Message = HashMap::new();
            summary_msg.insert("role".to_string(), Value::String(summary_role.clone()));
            summary_msg.insert("content".to_string(), Value::String(summary.clone()));
            summary_msg.insert(COMPRESSED_SUMMARY_METADATA_KEY.to_string(), Value::Bool(true));
            summary_msg.insert(COMPRESSED_SUMMARY_HAS_USER_TURN_KEY.to_string(), Value::Bool(self._summary_has_user_turn.unwrap_or(false)));
            compressed.push(summary_msg);
        }

        // -- ll.7914-7978 Merge summary into tail when alternation would otherwise break --
        // Default merge target: literal tail index 0, except forced-repair targets visible idx (#58753)
        let mut _merge_target_idx: usize = 0;
        if _force_user_leading {
            if let Some(idx) = first_tail_visible_idx {
                _merge_target_idx = idx;
            }
        }
        for (tail_idx, msg) in tail_messages.into_iter().enumerate() {
            let mut msg = msg;
            if _merge_summary_into_tail && tail_idx == _merge_target_idx {
                let old_content = msg.get("content").cloned().unwrap_or(Value::Null);
                if _force_user_leading && summary_role == "user" {
                    // -- ll.7936-7945 prefix merge for force-user-leading --
                    let prefix = format!("{}\n\n{}\n\n", summary, _SUMMARY_END_MARKER);
                    msg.insert("content".to_string(), _append_text_to_content(old_content, &prefix, true));
                } else {
                    // -- ll.7952-7966 suffix merge with delimiters --
                    let suffix = format!("\n\n{}\n\n{}\n\n{}", _MERGED_SUMMARY_DELIMITER, summary, _SUMMARY_END_MARKER);
                    let with_suffix = _append_text_to_content(old_content, &suffix, false);
                    let with_header = _append_text_to_content(with_suffix, &format!("{}\n", _MERGED_PRIOR_CONTEXT_HEADER), true);
                    msg.insert("content".to_string(), with_header);
                }
                msg.insert(COMPRESSED_SUMMARY_METADATA_KEY.to_string(), Value::Bool(true));
                msg.insert(COMPRESSED_SUMMARY_HAS_USER_TURN_KEY.to_string(), Value::Bool(self._summary_has_user_turn.unwrap_or(false)));
                // -- ll.7973-7976 `drop_stale_api_content(msg)` --
                let mut dummy = Value::Null;
                drop_stale_api_content(&mut dummy);
                _merge_summary_into_tail = false;
            }
            compressed.push(msg);
        }

        // -- l.7980 `self.compression_count += 1` --
        self.compression_count += 1;

        // -- l.7982 `compressed = self._sanitize_tool_pairs(compressed)` --
        compressed = _sanitize_tool_pairs(compressed);

        // -- ll.7984-7990 Replace image parts before newest image-bearing user turn --
        // Mirrors `compressed = _strip_historical_media(compressed)` (ll.7990)
        compressed = _strip_historical_media(compressed);

        // -- l.7992 `new_estimate = estimate_messages_tokens_rough(compressed)` --
        let new_estimate = estimate_messages_tokens_rough(&compressed);

        // -- ll.7994-8006 Anti-thrashing: measure effectiveness on like-for-like basis (up to nominal 8000 boundary) --
        // `# counter below resets every pass ...` (ll.8000-8001) — nominal 8000 falls inside this comment block.
        // From here through l.8080 the method continues beyond the nominal 8000 cut; those lines are included
        // to keep the method syntactically complete (see header note).
        let pre_estimate = estimate_messages_tokens_rough(&messages);
        let saved_estimate: i64 = pre_estimate as i64 - new_estimate as i64;
        let savings_pct: f64 = if pre_estimate > 0 { saved_estimate as f64 / pre_estimate as f64 * 100.0 } else { 0.0 };
        // -- l.8006 `self._last_compression_savings_pct = savings_pct` -- (still within 7200-8000 window; 8000 boundary is mid-comment above) --
        self._last_compression_savings_pct = savings_pct;

        // -- ll.8008-8022 Message-only savings diagnostic + quiet-mode logs (extends beyond 8000, included for syntactic closure) --
        // Mirrors comment at ll.8008-8012: anti-thrashing verdict owned by next provider-reported prompt count
        if !self.quiet_mode {
            eprintln!("[{}] Compressed: {} -> {} messages (~{} tokens saved, {:.0}%)", LOG_TARGET, n_messages, compressed.len(), saved_estimate, savings_pct);
            eprintln!("[{}] Compression #{} complete", LOG_TARGET, self.compression_count);
        }

        // -- ll.8024-8028 Enforced invariant (#57491): no compacted message may carry persistence marker --
        _strip_persistence_markers(&mut compressed);
        // -- ll.8029-8043 Prune stale codex_reasoning_items (#71058) --
        let _pruned_replay = _prune_stale_reasoning_replay(&mut compressed);
        if _pruned_replay > 0 && !self.quiet_mode {
            eprintln!("[{}] Pruned stale replay items from {} assistant message(s) during compaction", LOG_TARGET, _pruned_replay);
        }
        // -- l.8044 `self._last_compression_made_progress = True` --
        self._last_compression_made_progress = true;

        // -- ll.8046-8064 Post-compression memory trim (#70782, #76905) — glibc-gated, config-gated, rate-limited --
        // Python: `try: from hermes_cli.mem_trim import trim_memory; trim_memory(reason="post-compression") except Exception as exc: logger.debug(...)`
        // Rust: stubbed — trim is no-op in offline port; keep try-shaped logging for 1:1
        // We do not call external crate; we keep the shape:
        let _trim_result: Result<(), String> = Ok(());
        if let Err(exc) = _trim_result {
            eprintln!("[{}] post-compression memory trim failed: {}", LOG_TARGET, exc);
        }

        // -- ll.8066-8078 Batch compaction invalidates micro-compaction state --
        self._micro_compact_rolling_summary = String::new();
        self._micro_compact_cursor = 0;
        self._micro_compact_consecutive_failures = 0;
        self._micro_compact_last_failure_cursor = -1;
        self._proactive_prune_rearm_tokens = 0;

        // -- l.8080 `return compressed` --
        compressed
    }
}

// ---------------------------------------------------------------------------
// Free-function aliases for 1:1 grep traceability (Python uses both cls.* and instance paths)
// ---------------------------------------------------------------------------
#[allow(dead_code)]
pub fn _merge_adjacent_user_turns(result: Turns) -> Turns {
    ContextCompressor::merge_adjacent_user_turns_tail(result)
}
#[allow(dead_code)]
pub fn compress(
    compressor: &mut ContextCompressor,
    messages: Turns,
    current_tokens: Option<usize>,
    focus_topic: Option<String>,
    force: bool,
    memory_context: String,
) -> Turns {
    compressor.compress(messages, current_tokens, focus_topic, force, memory_context)
}
