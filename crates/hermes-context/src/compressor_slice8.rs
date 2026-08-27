//! Automatic context window compression for long conversations.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/context_compressor.py`
//! (8211 LOC) — slice 8/11, lines 5600-6400.
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
//! Mirrors Python ll.5600-6400 verbatim; line numbers in comments refer to the
//! 8211-line source file. Slice 1 covered ll.1-800, slice 2 ll.800-1600,
//! slice 3 ll.1600-2400, slice 4 ll.2400-3200, slice 5 ll.3200-4000,
//! slice 6 ll.4000-4800, slice 7 ll.4800-5600 (closed mid- `_ground_historical_task_snapshot`
//! inside `_HISTORICAL_TASK_SECTION_RE.sub` at ll.5595-5600). This slice
//! resumes at l.5600 (`return f"{replacement}{body}".strip()` — tail of
//! `_ground_historical_task_snapshot` fallback) and runs through l.6400
//! (inside `_find_tail_cut_by_tokens`, through the `fallback_cut = n - min_tail`
//! / `cut_idx = min(cut_idx, fallback_cut)` guard at ll.6402-6404).
//! The nominal 6400 boundary falls mid-function inside
//! `_find_tail_cut_by_tokens` (ll.6313-6457); the method is left syntactically
//! closed with a continuation marker — its tail (ll.6405-6457, alignment +
//! last-user/assistant anchors + N-user extension + forward-realign) continues
//! in `compressor_slice9.rs`. This keeps the module syntactically complete
//! without `cargo` while preserving 1:1 audit traceability for every line in
//! 5600-6400. This slice is verified by line-level audit, not by compilation.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

// ---------------------------------------------------------------------------
// Imports — mirrors Python ll.19-45 (same set as slices 1-7; repeated for self-containment)
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
// Constants — mirrors Python ll.112-130 + ll.642 + ll.1207 + ll.1212 + ll.1249
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

/// Mirrors `_HISTORICAL_SUMMARY_PREFIXES` (ll.505-636) — single current prefix for slice8 audit
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
// Content helpers — mirrors Python ll.1505-1519 + ll.5255-5300
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
// Sanitization helpers — mirrors `agent/message_sanitization.py`
// ---------------------------------------------------------------------------
fn tool_call_id_variants(tc: &Value) -> HashSet<String> {
    // Mirrors `agent.message_sanitization.tool_call_id_variants` (thin wrapper, ll.5776-5782)
    // Expands call_id/id/response_item_id and composite call|item bridge spellings (#63000)
    let mut set = HashSet::new();
    if let Some(obj) = tc.as_object() {
        for key in &["call_id", "id", "response_item_id"] {
            if let Some(v) = obj.get(*key).and_then(|x| x.as_str()) {
                if !v.is_empty() {
                    set.insert(v.to_string());
                    // composite bridge: split on '|'
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
        // Also expand the composite `call|item` key if present directly
        if let Some(v) = obj.get("call_id").and_then(|x| x.as_str()) {
            if let Some(item) = obj.get("response_item_id").and_then(|x| x.as_str()) {
                set.insert(format!("{}|{}", v, item));
            }
        }
    }
    set
}

fn tool_result_id_variants(cid: &str) -> HashSet<String> {
    // Mirrors `agent.message_sanitization.tool_result_id_variants` used in ll.5820
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

// ---------------------------------------------------------------------------
// Token helpers — mirrors `agent/model_metadata.py` stubs
// ---------------------------------------------------------------------------
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

// ---------------------------------------------------------------------------
// SessionDb stub (same as slice7, repeated for self-containment)
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
// ContextCompressor — mirrors Python ll.2070-6400 (class)
// Slice8 covers ll.5600-6400; fields repeated for self-containment.
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
        }
    }
}

// ---------------------------------------------------------------------------
// Slice8 body — mirrors Python ll.5600-6400
// ---------------------------------------------------------------------------
impl ContextCompressor {
    // -----------------------------------------------------------------------
    // _ground_historical_task_snapshot tail — mirrors Python ll.5596-5600
    // Slice7 ended mid-sub at l.5600 (`grounded = _HISTORICAL_TASK_SECTION_RE.sub(...)`
    // line). This slice resumes at the return fallback that closes that method.
    // Full method (ll.5576-5600) is repeated for self-containment; only
    // ll.5600 is new, but the head is kept so the function is runnable.
    // -----------------------------------------------------------------------
    /// Mirrors `def _ground_historical_task_snapshot(cls, summary, messages) -> str:` (ll.5576-5600)
    ///
    /// Force the task snapshot section to match a real user turn when possible.
    /// Mirrors ll.5582-5600.
    pub fn ground_historical_task_snapshot(summary: &str, messages: &Turns) -> String {
        // Mirrors `snapshot = cls._latest_user_task_snapshot(messages)` (l.5583)
        let snapshot = Self::latest_user_task_snapshot(messages);
        if snapshot.is_none() {
            return summary.to_string();
        }
        let snapshot = snapshot.unwrap();
        // Mirrors `body = cls._strip_summary_prefix(summary)` (l.5587)
        let body = _strip_summary_prefix(summary);
        // Mirrors `replacement = f"{HISTORICAL_TASK_HEADING}\n{snapshot}\n\n"` (l.5594)
        let replacement = format!("{}\n{}\n\n", HISTORICAL_TASK_HEADING, snapshot);
        // Mirrors `if _HISTORICAL_TASK_SECTION_RE.search(body): grounded = _HISTORICAL_TASK_SECTION_RE.sub(lambda _m: replacement, body, count=1); return grounded.strip()` (ll.5595-5599)
        // Keep section terminated with blank line: re.sub consumes trailing newlines, and without restoring them the next "## " heading is glued onto snapshot line — corrupting markdown and making heading invisible to same regex on next iterative compaction (which would delete every following section via \Z branch).
        if historical_task_section_re().is_match(&body) {
            let grounded = historical_task_section_re().replace(&body, replacement.as_str()).to_string();
            // `count=1` — replace only first occurrence
            // `replace` in `regex` replaces all; but spec says count=1 and there is at most one Historical Task Snapshot section, so single replace is identical. For strict count=1 we replace first match only via `replacen`.
            // Use replacen for 1:1
            let grounded_once = historical_task_section_re().replacen(&body, 1, replacement.as_str()).to_string();
            let _ = grounded; // keep both paths visible for audit
            return grounded_once.trim().to_string();
        }
        // Mirrors `return f"{replacement}{body}".strip()` (l.5600) — THIS IS THE SLICE8 RESUME LINE
        format!("{}{}", replacement, body).trim().to_string()
    }

    fn latest_user_task_snapshot(messages: &Turns) -> Option<String> {
        // Mirrors `ContextCompressor._latest_user_task_snapshot` (ll.5536-5574) — minimal stub repeated
        // Reuse real-user predicate so deterministic snapshot can never anchor on scaffolding
        for msg in messages.iter().rev() {
            if msg.get("role").and_then(|v| v.as_str()) != Some("user") {
                continue;
            }
            if !_is_actionable_user_turn(msg) {
                continue;
            }
            if _is_synthetic_compression_user_turn(msg) {
                continue;
            }
            let content = msg.get("content").cloned().unwrap_or(Value::Null);
            let text = _redact_compaction_text(&content_text_for_contains(&content).trim().to_string());
            if text.is_empty() {
                continue;
            }
            let collapsed = Regex::new(r"\s+").unwrap().replace_all(&text, " ").to_string();
            let truncated = if collapsed.len() > _ACTIVE_TASK_MAX_CHARS {
                format!("{} ...[truncated]", collapsed[.._ACTIVE_TASK_MAX_CHARS.saturating_sub(15)].trim_end())
            } else {
                collapsed
            };
            return Some(format!(
                "User asked (deterministic, from compacted turns): {:?}\nHistorical only; newer protected-tail messages after this summary win.",
                truncated
            ));
        }
        None
    }

    // -----------------------------------------------------------------------
    // _find_context_summaries — mirrors Python ll.5602-5624
    // -----------------------------------------------------------------------
    /// Mirrors `def _find_context_summaries(cls, messages, start, end) -> list[tuple[int, str]]:` (ll.5602-5624)
    ///
    /// Find handoff summaries inside a compression window.
    pub fn find_context_summaries(messages: &Turns, start: usize, end: usize) -> Vec<(usize, String)> {
        // Mirrors `n = len(messages)` (l.5610)
        let n = messages.len();
        // Mirrors defensive clamp so caller passing out-of-range end cannot trigger IndexError (#75588) (ll.5611-5615)
        //   start = max(0, min(start, n))
        //   end = max(start, min(end, n))
        let start = start.min(n);
        let end = end.min(n);
        let end = end.max(start);
        // Mirrors `summaries: list[tuple[int, str]] = []` (l.5616)
        let mut summaries: Vec<(usize, String)> = Vec::new();
        // Mirrors `for idx in range(start, end): content = messages[idx].get("content"); if cls._is_context_summary_message(messages[idx]): summaries.append((idx, cls._strip_summary_prefix(...)))` (ll.5617-5623)
        for idx in start..end {
            if _is_context_summary_message(&messages[idx]) {
                let content = messages[idx].get("content").cloned().unwrap_or(Value::Null);
                let stripped = _strip_summary_prefix(&content_text_for_contains(&content));
                summaries.push((idx, stripped));
            }
        }
        summaries
    }

    // -----------------------------------------------------------------------
    // _find_latest_context_summary — mirrors Python ll.5626-5637
    // -----------------------------------------------------------------------
    /// Mirrors `def _find_latest_context_summary(cls, messages, start, end) -> tuple[Optional[int], str]:` (ll.5626-5637)
    ///
    /// Find the newest handoff summary inside a compression window.
    pub fn find_latest_context_summary(messages: &Turns, start: usize, end: usize) -> (Option<usize>, String) {
        // Mirrors `summaries = cls._find_context_summaries(messages, start, end)` (l.5634)
        let summaries = Self::find_context_summaries(messages, start, end);
        // Mirrors `if summaries: return summaries[-1]` (ll.5635-5636)
        if let Some(last) = summaries.last() {
            return (Some(last.0), last.1.clone());
        }
        // Mirrors `return None, ""` (l.5637)
        (None, String::new())
    }

    // -----------------------------------------------------------------------
    // _strip_context_summary_handoff_message — mirrors Python ll.5639-5756
    // -----------------------------------------------------------------------
    /// Mirrors `def _strip_context_summary_handoff_message(cls, message) -> Optional[Dict[str, Any]]:` (ll.5639-5756)
    ///
    /// Drop stale handoff data while preserving merged prior-tail content.
    pub fn strip_context_summary_handoff_message(message: &Message) -> Option<Message> {
        // Mirrors message dict check — Python checks `if not isinstance(message, dict): return message` (ll.5645-5646)
        // In Rust, Message is always a dict; keep check for 1:1 by testing empty-ness trait (always true)
        // Mirrors `content = message.get("content")` (l.5648)
        let content = message.get("content").cloned().unwrap_or(Value::Null);
        // Mirrors `is_summary = (cls._is_context_summary_content(content) or cls._has_compressed_summary_metadata(message))` (ll.5649-5652)
        let is_summary = _is_context_summary_content(&content) || _has_compressed_summary_metadata(message);
        if !is_summary {
            // Mirrors `return message.copy()` (l.5654)
            return Some(message.clone());
        }
        // Mirrors `if isinstance(content, str): ...` branch (ll.5656-5674)
        if let Some(s) = content.as_str() {
            if s.contains(_MERGED_SUMMARY_DELIMITER) {
                // Mirrors `prior = content.split(_MERGED_SUMMARY_DELIMITER, 1)[0].strip()` (l.5658)
                let prior = s.splitn(2, _MERGED_SUMMARY_DELIMITER).next().unwrap_or("").trim();
                // Mirrors `if prior.startswith(_MERGED_PRIOR_CONTEXT_HEADER): prior = prior[len(...):].lstrip()` (ll.5659-5660)
                let prior_stripped = if prior.starts_with(_MERGED_PRIOR_CONTEXT_HEADER) {
                    prior[_MERGED_PRIOR_CONTEXT_HEADER.len()..].trim_start()
                } else {
                    prior
                };
                if !prior_stripped.is_empty() {
                    let mut unwrapped = message.clone();
                    unwrapped.insert("content".to_string(), Value::String(prior_stripped.to_string()));
                    unwrapped.remove(COMPRESSED_SUMMARY_METADATA_KEY);
                    return Some(unwrapped);
                }
            } else {
                // Mirrors `marker_idx = content.find(_SUMMARY_END_MARKER); if marker_idx >= 0: remainder = content[marker_idx + len(marker):].lstrip(); if remainder: unwrapped = message.copy(); unwrapped["content"] = remainder; unwrapped.pop(...); return unwrapped` (ll.5667-5674)
                if let Some(idx) = s.find(_SUMMARY_END_MARKER) {
                    let remainder = s[idx + _SUMMARY_END_MARKER.len()..].trim_start();
                    if !remainder.is_empty() {
                        let mut unwrapped = message.clone();
                        unwrapped.insert("content".to_string(), Value::String(remainder.to_string()));
                        unwrapped.remove(COMPRESSED_SUMMARY_METADATA_KEY);
                        return Some(unwrapped);
                    }
                }
            }
        }
        // Mirrors `if isinstance(content, list):` branch (ll.5676-5754)
        if let Some(arr) = content.as_array() {
            let mut prior_blocks: Vec<Value> = Vec::new();
            let mut found_delimiter = false;
            for item in arr {
                if let Some(s) = item.as_str() {
                    // Mirrors `if _MERGED_SUMMARY_DELIMITER in item: before = item.split(...,1)[0]; if before.strip(): prior_blocks.append(before); found_delimiter=True; break` (ll.5681-5686)
                    if s.contains(_MERGED_SUMMARY_DELIMITER) {
                        let before = s.splitn(2, _MERGED_SUMMARY_DELIMITER).next().unwrap_or("");
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
                    // Mirrors `text = item.get("text"); if isinstance(text,str) and _MERGED_SUMMARY_DELIMITER in text: before = text.split(...)[0]; ... prior_blocks.append(copied); found_delimiter=True; break` (ll.5689-5698)
                    if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                        if text.contains(_MERGED_SUMMARY_DELIMITER) {
                            let before = text.splitn(2, _MERGED_SUMMARY_DELIMITER).next().unwrap_or("");
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
                    prior_blocks.push(Value::Object(obj.clone()));
                    continue;
                }
                prior_blocks.push(item.clone());
            }
            if !found_delimiter {
                // Mirrors legacy marker branch ll.5703-5726
                let mut legacy_blocks: Vec<Value> = Vec::new();
                let mut found_marker = false;
                for (index, item) in arr.iter().enumerate() {
                    let text_opt: Option<String> = if let Some(s) = item.as_str() {
                        Some(s.to_string())
                    } else if let Some(obj) = item.as_object() {
                        obj.get("text").and_then(|v| v.as_str()).map(|s| s.to_string())
                    } else {
                        None
                    };
                    let text = match text_opt {
                        Some(t) => t,
                        None => continue,
                    };
                    // Mirrors `if not isinstance(text,str) or _SUMMARY_END_MARKER not in text: continue` (ll.5708-5709)
                    if !text.contains(_SUMMARY_END_MARKER) {
                        continue;
                    }
                    // Mirrors `remainder = text.split(_SUMMARY_END_MARKER,1)[1].lstrip(); if remainder: ... legacy_blocks.append(...); for later in content[index+1:]: legacy_blocks.append(...)` (ll.5710-5719)
                    let remainder = text.splitn(2, _SUMMARY_END_MARKER).nth(1).unwrap_or("").trim_start();
                    if !remainder.is_empty() {
                        if let Some(obj) = item.as_object() {
                            let mut copied = obj.clone();
                            copied.insert("text".to_string(), Value::String(remainder.to_string()));
                            legacy_blocks.push(Value::Object(copied));
                        } else {
                            legacy_blocks.push(Value::String(remainder.to_string()));
                        }
                    }
                    for later in arr.iter().skip(index + 1) {
                        if let Some(obj) = later.as_object() {
                            legacy_blocks.push(Value::Object(obj.clone()));
                        } else {
                            legacy_blocks.push(later.clone());
                        }
                    }
                    found_marker = true;
                    break;
                }
                if found_marker && !legacy_blocks.is_empty() {
                    let mut unwrapped = message.clone();
                    unwrapped.insert("content".to_string(), Value::Array(legacy_blocks));
                    unwrapped.remove(COMPRESSED_SUMMARY_METADATA_KEY);
                    return Some(unwrapped);
                }
            }
            if found_delimiter {
                // Mirrors `for index, item in enumerate(prior_blocks): if isinstance(item,str): if item.lstrip().startswith(_MERGED_PRIOR_CONTEXT_HEADER): leading = ...; if leading: prior_blocks[index]=leading else: pop; break; elif dict with text...` (ll.5728-5748)
                for index in 0..prior_blocks.len() {
                    let item = prior_blocks[index].clone();
                    if let Some(s) = item.as_str() {
                        if s.trim_start().starts_with(_MERGED_PRIOR_CONTEXT_HEADER) {
                            let leading = s.trim_start()[_MERGED_PRIOR_CONTEXT_HEADER.len()..].trim_start();
                            if !leading.is_empty() {
                                prior_blocks[index] = Value::String(leading.to_string());
                            } else {
                                prior_blocks.remove(index);
                            }
                            break;
                        }
                    } else if let Some(obj) = item.as_object() {
                        if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                            if text.trim_start().starts_with(_MERGED_PRIOR_CONTEXT_HEADER) {
                                let leading = text.trim_start()[_MERGED_PRIOR_CONTEXT_HEADER.len()..].trim_start();
                                if !leading.is_empty() {
                                    let mut copied = obj.clone();
                                    copied.insert("text".to_string(), Value::String(leading.to_string()));
                                    prior_blocks[index] = Value::Object(copied);
                                } else {
                                    prior_blocks.remove(index);
                                }
                                break;
                            }
                        }
                    }
                }
                if !prior_blocks.is_empty() {
                    let mut unwrapped = message.clone();
                    unwrapped.insert("content".to_string(), Value::Array(prior_blocks));
                    unwrapped.remove(COMPRESSED_SUMMARY_METADATA_KEY);
                    return Some(unwrapped);
                }
            }
        }
        // Mirrors `return None` (l.5756)
        None
    }

    // -----------------------------------------------------------------------
    // Tool-call / tool-result pair integrity helpers — mirrors Python ll.5758-5907
    // -----------------------------------------------------------------------

    /// Mirrors `def _get_tool_call_id(tc) -> str:` (ll.5762-5769)
    ///
    /// Extract the canonical call ID from a tool_call entry (dict or SimpleNamespace), for logging/display only.
    /// Matching logic must use `_tool_call_id_variants` instead.
    pub fn get_tool_call_id(tc: &Value) -> String {
        // Mirrors `if isinstance(tc, dict): return tc.get("call_id","") or tc.get("id","") or ""` (ll.5767-5768)
        // Mirrors `return getattr(tc, "call_id","") or getattr(tc,"id","") or ""` (l.5769)
        if let Some(obj) = tc.as_object() {
            if let Some(s) = obj.get("call_id").and_then(|v| v.as_str()) {
                if !s.is_empty() { return s.to_string(); }
            }
            if let Some(s) = obj.get("id").and_then(|v| v.as_str()) {
                if !s.is_empty() { return s.to_string(); }
            }
            return String::new();
        }
        // Non-dict (SimpleNamespace) path not representable in Rust Value; return empty for 1:1
        String::new()
    }

    /// Mirrors `def _tool_call_id_variants(tc) -> set:` (ll.5771-5782)
    ///
    /// Return every id variant a tool result might reference *tc* by.
    /// Thin forwarder — policy owner is `agent.message_sanitization.tool_call_id_variants`,
    /// which also expands `response_item_id` and composite `call|item` bridge spellings (#63000),
    /// so compressor's pairing tolerance matches pre-call sanitizer's exactly.
    pub fn tool_call_id_variants(tc: &Value) -> HashSet<String> {
        // Mirrors `from agent.message_sanitization import tool_call_id_variants; return set(tool_call_id_variants(tc))` (ll.5781-5782)
        tool_call_id_variants(tc)
    }

    /// Mirrors `def _sanitize_tool_pairs(self, messages) -> List[Dict[str, Any]]:` (ll.5784-5907)
    ///
    /// Fix orphaned tool_call / tool_result pairs after compression.
    /// Two failure modes: orphaned results and orphaned calls. Previous approach inserted stub
    /// `role="tool"` results for orphaned calls, but that caused secondary failure with
    /// `repair_message_sequence()` using `tc.get("id")` vs this sanitizer's `call_id || id` mismatch
    /// (Codex Responses API format: `id != call_id`). Stripping at source avoids mismatch.
    pub fn sanitize_tool_pairs(&self, mut messages: Turns) -> Turns {
        // Mirrors `surviving_call_ids: set = set(); for msg in messages: if msg.get("role")=="assistant": for tc in msg.get("tool_calls") or []: surviving_call_ids |= self._tool_call_id_variants(tc)` (ll.5806-5810)
        let mut surviving_call_ids: HashSet<String> = HashSet::new();
        for msg in &messages {
            if msg.get("role").and_then(|v| v.as_str()) == Some("assistant") {
                if let Some(tcs) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                    for tc in tcs {
                        surviving_call_ids.extend(Self::tool_call_id_variants(tc));
                    }
                }
            }
        }
        // Mirrors `result_call_ids: set = set(); for msg in messages: if msg.get("role")=="tool": cid=msg.get("tool_call_id"); if cid: result_call_ids |= tool_result_id_variants(cid)` (ll.5812-5820)
        let mut result_call_ids: HashSet<String> = HashSet::new();
        for msg in &messages {
            if msg.get("role").and_then(|v| v.as_str()) == Some("tool") {
                if let Some(cid) = msg.get("tool_call_id").and_then(|v| v.as_str()) {
                    if !cid.is_empty() {
                        result_call_ids.extend(tool_result_id_variants(cid));
                    }
                }
            }
        }
        // Mirrors `# 1. Remove tool results whose call_id has no matching assistant tool_call` (l.5822)
        // `orphaned_results = result_call_ids - surviving_call_ids` (l.5823)
        let orphaned_results: HashSet<String> = result_call_ids.difference(&surviving_call_ids).cloned().collect();
        if !orphaned_results.is_empty() {
            // Mirrors `messages = [m for m in messages if not (m.get("role")=="tool" and (rv := tool_result_id_variants(m.get("tool_call_id"))) and not (rv & surviving_call_ids))]` (ll.5825-5832)
            messages = messages
                .into_iter()
                .filter(|m| {
                    if m.get("role").and_then(|v| v.as_str()) != Some("tool") {
                        return true;
                    }
                    let cid = m.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("");
                    let rv = tool_result_id_variants(cid);
                    if rv.is_empty() {
                        return true;
                    }
                    // keep if any variant overlaps surviving_call_ids
                    rv.intersection(&surviving_call_ids).next().is_some()
                })
                .collect();
            if !self.quiet_mode {
                // Mirrors `logger.info("Compression sanitizer: removed %d orphaned tool result(s)", len(orphaned_results))` (l.5834)
                eprintln!("[{}] Compression sanitizer: removed {} orphaned tool result(s)", LOG_TARGET, orphaned_results.len());
            }
        }
        // Mirrors `# 2. Strip orphaned tool_calls from assistant messages whose results were dropped.` (ll.5836-5906)
        let mut stripped_count: usize = 0;
        if surviving_call_ids.difference(&result_call_ids).next().is_some() {
            // --- In-flight tool chain protection (issue #79278) ------------- (ll.5845-5878)
            // Distinguish *pending* (live request whose result executor has not yet appended) from *orphaned*.
            // Compression can fire mid-chain: model emits assistant(tool_calls), tool_executor appends role="tool" after.
            // In that window messages[-1] is assistant tool_call whose id is not yet in result_call_ids.
            // Any tool result would be appended after this message, so if it is the last non-tool message its calls are presumed pending.
            // Stripping would delete live request; later real result would be dropped by repair_message_sequence.
            // Therefore preserve trailing in-flight call verbatim; only genuinely orphaned calls in discarded region are stripped.
            // Walk back over trailing tool results first: multi-call batch looks like [..., assistant(c1,c2,c3), tool(c1)] — chain still in flight.
            let mut trailing_inflight_idx: Option<usize> = None;
            let mut idx = messages.len() as i64 - 1;
            while idx >= 0 && messages[idx as usize].get("role").and_then(|v| v.as_str()) == Some("tool") {
                idx -= 1;
            }
            if idx >= 0 && messages[idx as usize].get("role").and_then(|v| v.as_str()) == Some("assistant") {
                trailing_inflight_idx = Some(idx as usize);
            }
            // -----------------------------------------------------------------
            for (msg_idx, msg) in messages.iter_mut().enumerate() {
                if msg.get("role").and_then(|v| v.as_str()) != Some("assistant") {
                    continue;
                }
                if Some(msg_idx) == trailing_inflight_idx {
                    // Live request, not an orphan — executor appends result after compress() returns.
                    continue;
                }
                let tcs_opt = msg.get("tool_calls").cloned();
                let tcs = match tcs_opt {
                    Some(Value::Array(arr)) if !arr.is_empty() => arr,
                    _ => continue,
                };
                // Mirrors `kept = [tc for tc in tcs if self._tool_call_id_variants(tc) & result_call_ids]` (l.5889)
                let kept: Vec<Value> = tcs
                    .iter()
                    .filter(|tc| {
                        let variants = Self::tool_call_id_variants(tc);
                        variants.intersection(&result_call_ids).next().is_some()
                    })
                    .cloned()
                    .collect();
                if kept.len() != tcs.len() {
                    stripped_count += tcs.len() - kept.len();
                    if !kept.is_empty() {
                        msg.insert("tool_calls".to_string(), Value::Array(kept));
                    } else {
                        msg.remove("tool_calls");
                        // Mirrors ensure assistant message still has visible content so API does not reject empty turn (ll.5896-5900)
                        let content = msg.get("content").cloned().unwrap_or(Value::Null);
                        let empty = match &content {
                            Value::Null => true,
                            Value::String(s) => s.trim().is_empty(),
                            _ => false,
                        };
                        if empty {
                            msg.insert("content".to_string(), Value::String("(tool call removed)".to_string()));
                        }
                    }
                }
            }
            if stripped_count > 0 && !self.quiet_mode {
                // Mirrors `logger.info("Compression sanitizer: stripped %d orphaned tool_call(s) ...", stripped_count)` (ll.5901-5905)
                eprintln!("[{}] Compression sanitizer: stripped {} orphaned tool_call(s) from assistant messages", LOG_TARGET, stripped_count);
            }
        }
        messages
    }

    // -----------------------------------------------------------------------
    // _align_boundary_forward — mirrors Python ll.5909-5917
    // -----------------------------------------------------------------------
    /// Mirrors `def _align_boundary_forward(self, messages, idx) -> int:` (ll.5909-5917)
    ///
    /// Push a compress-start boundary forward past any orphan tool results.
    /// If `messages[idx]` is a tool result, slide forward until we hit a non-tool message
    /// so we don't start the summarised region mid-group.
    pub fn align_boundary_forward(&self, messages: &Turns, mut idx: usize) -> usize {
        // Mirrors `while idx < len(messages) and messages[idx].get("role") == "tool": idx += 1` (ll.5915-5916)
        while idx < messages.len() && messages[idx].get("role").and_then(|v| v.as_str()) == Some("tool") {
            idx += 1;
        }
        idx
    }

    // -----------------------------------------------------------------------
    // _restart_handoff_probe_bounds — mirrors Python ll.5919-5932
    // -----------------------------------------------------------------------
    /// Mirrors `def _restart_handoff_probe_bounds(self, messages) -> tuple[int, int]:` (ll.5919-5932)
    ///
    /// Return the bounded transcript region that can indicate restart decay.
    pub fn restart_handoff_probe_bounds(&self, messages: &Turns) -> (usize, usize) {
        // Mirrors `if not messages or self.protect_first_n <= 0: return 0,0` (ll.5924-5925)
        if messages.is_empty() || self.protect_first_n == 0 {
            return (0, 0);
        }
        // Mirrors `first_non_system = 1 if messages[0].get("role") == "system" else 0` (l.5926)
        let first_non_system = if messages[0].get("role").and_then(|v| v.as_str()) == Some("system") { 1 } else { 0 };
        // Mirrors `return first_non_system, min(len(messages), first_non_system + self.protect_first_n + _RESTART_HANDOFF_PROBE_EXTRA_MESSAGES)` (ll.5927-5932)
        let end = (first_non_system + self.protect_first_n + _RESTART_HANDOFF_PROBE_EXTRA_MESSAGES).min(messages.len());
        (first_non_system, end)
    }

    // -----------------------------------------------------------------------
    // _effective_protect_first_n — mirrors Python ll.5934-5967
    // -----------------------------------------------------------------------
    /// Mirrors `def _effective_protect_first_n(self, messages=None) -> int:` (ll.5934-5967)
    ///
    /// `protect_first_n` decayed across compression cycles. Keeps first N non-system messages verbatim so original task framing survives FIRST compaction. But applying it on every subsequent pass fossilizes early turns — they're re-copied into each child session and never summarized away (#11996). Once session has been compressed at least once, early turns are already captured in handoff summary, so no need to re-protect: decay to 0 (system prompt still always protected separately by _protect_head_size). After restart, infer decayed state from handoff summaries in resumed-head region.
    pub fn effective_protect_first_n(&self, messages: Option<&Turns>) -> usize {
        // Mirrors `if self.compression_count >= 1 or self._previous_summary: return 0` (ll.5953-5954)
        if self.compression_count >= 1 || self._previous_summary.is_some() {
            return 0;
        }
        if let Some(msgs) = messages {
            if self.protect_first_n > 0 {
                // Mirrors `first_non_system, restart_probe_end = self._restart_handoff_probe_bounds(messages)` (ll.5959-5961)
                let (first_non_system, restart_probe_end) = self.restart_handoff_probe_bounds(msgs);
                // Mirrors `if any(self._is_context_summary_message(msg) for msg in messages[first_non_system:restart_probe_end]): return 0` (ll.5962-5966)
                for msg in &msgs[first_non_system..restart_probe_end] {
                    if _is_context_summary_message(msg) {
                        return 0;
                    }
                }
            }
        }
        self.protect_first_n
    }

    // -----------------------------------------------------------------------
    // _protect_head_size — mirrors Python ll.5969-5992
    // -----------------------------------------------------------------------
    /// Mirrors `def _protect_head_size(self, messages) -> int:` (ll.5969-5992)
    ///
    /// Total count of head messages to protect. `protect_first_n` is defined as *additional* messages protected beyond system prompt. System prompt (if present at index 0) is always implicitly protected. The `protect_first_n` portion DECAYS after first compression (see _effective_protect_first_n) so early user turns don't fossilize across repeated compactions (#11996).
    /// Examples (first compaction): protect_first_n=0 → system prompt only (or nothing if no system msg); protect_first_n=3 → system + first 3 non-system messages. After first compaction: system prompt only.
    pub fn protect_head_size(&self, messages: &Turns) -> usize {
        // Mirrors `head = 0; if messages and messages[0].get("role") == "system": head = 1` (ll.5989-5991)
        let mut head = 0;
        if !messages.is_empty() && messages[0].get("role").and_then(|v| v.as_str()) == Some("system") {
            head = 1;
        }
        // Mirrors `return head + self._effective_protect_first_n(messages)` (l.5992)
        head + self.effective_protect_first_n(Some(messages))
    }

    // -----------------------------------------------------------------------
    // _align_boundary_backward — mirrors Python ll.5994-6016
    // -----------------------------------------------------------------------
    /// Mirrors `def _align_boundary_backward(self, messages, idx) -> int:` (ll.5994-6016)
    ///
    /// Pull a compress-end boundary backward to avoid splitting a tool_call / result group.
    /// If boundary falls in middle of tool-result group (consecutive tool messages before `idx`), walk backward past all of them to find parent assistant message. If found, move boundary before assistant so entire assistant + tool_results group is included in summarised region rather than being split (which causes silent data loss when `_sanitize_tool_pairs` removes orphaned tail results).
    pub fn align_boundary_backward(&self, messages: &Turns, mut idx: usize) -> usize {
        // Mirrors `if idx <= 0 or idx >= len(messages): return idx` (ll.6006-6007)
        if idx == 0 || idx >= messages.len() {
            return idx;
        }
        // Mirrors `check = idx - 1; while check >= 0 and messages[check].get("role") == "tool": check -= 1` (ll.6009-6011)
        let mut check = idx as i64 - 1;
        while check >= 0 && messages[check as usize].get("role").and_then(|v| v.as_str()) == Some("tool") {
            check -= 1;
        }
        // Mirrors `if check >= 0 and messages[check].get("role") == "assistant" and messages[check].get("tool_calls"): idx = check` (ll.6014-6015)
        if check >= 0 {
            let c = check as usize;
            if messages[c].get("role").and_then(|v| v.as_str()) == Some("assistant") {
                if let Some(tc) = messages[c].get("tool_calls") {
                    if !tc.is_null() {
                        // non-empty tool_calls check via array len or truthiness
                        let has_calls = match tc {
                            Value::Array(arr) => !arr.is_empty(),
                            _ => true,
                        };
                        if has_calls {
                            idx = c;
                        }
                    }
                }
            }
        }
        idx
    }

    // -----------------------------------------------------------------------
    // _find_last_user_message_idx — mirrors Python ll.6022-6038
    // -----------------------------------------------------------------------
    /// Mirrors `def _find_last_user_message_idx(self, messages, head_end) -> int:` (ll.6022-6038)
    ///
    /// Return the latest actionable user turn at or after *head_end*, or -1. Compaction handoffs and empty platform echoes are continuity artifacts; neither may displace request, correction, or completion that tail anchor exists to preserve.
    pub fn find_last_user_message_idx(&self, messages: &Turns, head_end: usize) -> i64 {
        // Mirrors `for i in range(len(messages)-1, head_end-1, -1): msg = messages[i]; if (self._is_actionable_user_turn(msg) and not self._is_synthetic_compression_user_turn(msg)): return i; return -1` (ll.6031-6038)
        for i in (head_end..messages.len()).rev() {
            let msg = &messages[i];
            if _is_actionable_user_turn(msg) && !_is_synthetic_compression_user_turn(msg) {
                return i as i64;
            }
        }
        -1
    }

    // -----------------------------------------------------------------------
    // _find_last_assistant_message_idx — mirrors Python ll.6040-6089
    // -----------------------------------------------------------------------
    /// Mirrors `def _find_last_assistant_message_idx(self, messages, head_end) -> int:` (ll.6040-6089)
    ///
    /// Return index of last user-visible assistant reply at or after *head_end*, or -1.
    /// A "user-visible reply" is assistant message with non-empty textual content — i.e. one WebUI/TUI/SessionsPage rendered as bubble. Deliberately skip assistant messages that contain only `tool_calls` (and no text), because those render as small "calling tool X" indicators and aren't what reporter means by "output of last message you sent" (#29824). Context-compaction handoff banners can also carry `role="assistant"`; they are internal continuity state, not user-visible reply, so ignore both as text-bearing anchors and as candidates for fallback. Mirror user-role summary exclusion in `_find_last_user_message_idx`.
    /// Falling back to most recent non-summary assistant message of ANY kind only kicks in when no content-bearing assistant message exists in compressible region — typically fresh session that just started multi-step tool sequence with no prior reply.
    pub fn find_last_assistant_message_idx(&self, messages: &Turns, head_end: usize) -> i64 {
        // Mirrors `last_any = -1; for i in range(len(messages)-1, head_end-1, -1): msg = messages[i]; if msg.get("role") != "assistant" or self._is_context_summary_content(msg.get("content")): continue; if self._is_context_summary_message(msg): continue; if last_any < 0: last_any = i; content = msg.get("content"); if isinstance(content,str) and content.strip(): return i; if isinstance(content,list): for part in content: if isinstance(part,dict): text = part.get("text") or part.get("content"); if isinstance(text,str) and text.strip(): return i; return last_any` (ll.6067-6089)
        let mut last_any: i64 = -1;
        for i in (head_end..messages.len()).rev() {
            let msg = &messages[i];
            if msg.get("role").and_then(|v| v.as_str()) != Some("assistant") {
                continue;
            }
            if let Some(c) = msg.get("content") {
                if _is_context_summary_content(c) {
                    continue;
                }
            }
            if _is_context_summary_message(msg) {
                continue;
            }
            if last_any < 0 {
                last_any = i as i64;
            }
            if let Some(c) = msg.get("content") {
                if let Some(s) = c.as_str() {
                    if !s.trim().is_empty() {
                        return i as i64;
                    }
                } else if let Some(arr) = c.as_array() {
                    // Multimodal / Anthropic-style: look for any text block with non-empty text
                    for part in arr {
                        if let Some(obj) = part.as_object() {
                            let text = obj.get("text").or_else(|| obj.get("content"));
                            if let Some(t) = text.and_then(|v| v.as_str()) {
                                if !t.trim().is_empty() {
                                    return i as i64;
                                }
                            }
                        }
                    }
                }
            }
        }
        last_any
    }

    // -----------------------------------------------------------------------
    // _ensure_last_assistant_message_in_tail — mirrors Python ll.6091-6147
    // -----------------------------------------------------------------------
    /// Mirrors `def _ensure_last_assistant_message_in_tail(self, messages, cut_idx, head_end) -> int:` (ll.6091-6147)
    ///
    /// Guarantee most recent assistant message is in protected tail. WebUI/TUI bug (#29824). Without anchor, `_find_tail_cut_by_tokens` can leave user's most recent visible assistant response inside compressed middle region — summariser rolls reply into single `[CONTEXT COMPACTION — REFERENCE ONLY]` block persisted as `role="user"` or `role="assistant"`. From operator's perspective WebUI session viewer and TUI chat panel both suddenly show opaque "Context compaction" block where they were just reading actual reply. Mirror of `_ensure_last_user_message_in_tail` but anchors on last assistant-role message. Re-runs tool-group alignment so we don't split `tool_call`/`tool_result` group that immediately precedes anchored message.
    pub fn ensure_last_assistant_message_in_tail(&self, messages: &Turns, cut_idx: usize, head_end: usize) -> usize {
        // Mirrors `last_asst_idx = self._find_last_assistant_message_idx(messages, head_end)` (l.6124)
        let last_asst_idx = self.find_last_assistant_message_idx(messages, head_end);
        if last_asst_idx < 0 {
            // Mirrors `if last_asst_idx < 0: return cut_idx` (ll.6125-6128) — No assistant message in compressible region — nothing to anchor
            return cut_idx;
        }
        let last_asst_idx = last_asst_idx as usize;
        if last_asst_idx >= cut_idx {
            // Mirrors `if last_asst_idx >= cut_idx: return cut_idx` (ll.6129-6132) — Already in tail — token-budget walk did right thing
            return cut_idx;
        }
        // Mirrors `new_cut = self._align_boundary_backward(messages, last_asst_idx)` (l.6138)
        let new_cut = self.align_boundary_backward(messages, last_asst_idx);
        if !self.quiet_mode {
            // Mirrors `logger.debug("Anchoring tail cut to last assistant message at index %d (was %d, aligned to %d) ... (#29824)", last_asst_idx, cut_idx, new_cut)` (ll.6139-6145)
            eprintln!("[{}] Anchoring tail cut to last assistant message at index {} (was {}, aligned to {}) to keep previously-visible reply out of compaction summary (#29824)", LOG_TARGET, last_asst_idx, cut_idx, new_cut);
        }
        // Mirrors `return max(new_cut, head_end + 1)` (l.6147) — Safety: never go back into head region
        new_cut.max(head_end + 1)
    }

    // -----------------------------------------------------------------------
    // _ensure_last_user_message_in_tail — mirrors Python ll.6149-6221
    // -----------------------------------------------------------------------
    /// Mirrors `def _ensure_last_user_message_in_tail(self, messages, cut_idx, head_end) -> int:` (ll.6149-6221)
    ///
    /// Guarantee most recent user message is in protected tail. Bug (#10896): `_align_boundary_backward` can pull `cut_idx` past user message when it tries to keep tool_call/result groups together. If last user message ends up in compressed middle region LLM summariser writes it into "Historical Pending User Asks", but `SUMMARY_PREFIX` tells next model to respond only to user messages *after* summary — so task effectively disappears from active context, causing agent to stall, repeat completed work, or silently drop latest request. Fix: if last user-role message not already in tail (`messages[cut_idx:]`), walk `cut_idx` back to include it. Then re-align backward one more time to avoid splitting tool_call/result group that immediately precedes user message. Causal Coupling guard (#22523): final `max(last_user_idx, head_end+1)` clamp can push cut *past* user message when user sits at `head_end` — only case where `head_end+1 > last_user_idx`. That splits turn-pair: user lands in compressed region without its assistant reply, so summariser records it as pending ask and next session re-executes already-completed task. When split unavoidable, push cut *forward* to `pair_end` so full pair (user + reply + tool results) is summarised together and correctly marked as completed.
    pub fn ensure_last_user_message_in_tail(&self, messages: &Turns, cut_idx: usize, head_end: usize) -> usize {
        // Mirrors `last_user_idx = self._find_last_user_message_idx(messages, head_end)` (l.6182)
        let last_user_idx = self.find_last_user_message_idx(messages, head_end);
        if last_user_idx < 0 {
            return cut_idx;
        }
        let last_user_idx = last_user_idx as usize;
        if last_user_idx >= cut_idx {
            // Mirrors `if last_user_idx >= cut_idx: return cut_idx` (ll.6187-6189)
            return cut_idx;
        }
        if !self.quiet_mode {
            // Mirrors `logger.debug("Anchoring tail cut to last user message at index %d (was %d) to prevent active-task loss after compression", last_user_idx, cut_idx)` (ll.6197-6203)
            eprintln!("[{}] Anchoring tail cut to last user message at index {} (was {}) to prevent active-task loss after compression", LOG_TARGET, last_user_idx, cut_idx);
        }
        // Mirrors `adjusted = max(last_user_idx, head_end + 1)` (l.6205)
        let adjusted = last_user_idx.max(head_end + 1);
        if adjusted > last_user_idx {
            // Mirrors Causal Coupling clamp path ll.6206-6220
            let pair_end = self.find_turn_pair_end(messages, last_user_idx);
            if !self.quiet_mode {
                // Mirrors logger.debug Causal Coupling: cut would split turn-pair...
                eprintln!("[{}] Causal Coupling: cut would split turn-pair at user {}; pushing cut forward to pair_end {} so completed pair is summarised together (#22523)", LOG_TARGET, last_user_idx, pair_end);
            }
            return pair_end.max(head_end + 1);
        }
        adjusted
    }

    // -----------------------------------------------------------------------
    // _ensure_last_n_user_messages_in_tail — mirrors Python ll.6223-6284
    // -----------------------------------------------------------------------
    /// Mirrors `def _ensure_last_n_user_messages_in_tail(self, messages, cut_idx, head_end, n) -> int:` (ll.6223-6284)
    ///
    /// Guarantee last N actionable user messages are in protected tail. Generalizes `_ensure_last_user_message_in_tail` to preserve arbitrary number of recent user messages. Prevents token-budget-based tail cut from consuming recent conversation turns when large tool outputs fill budget. When *n* <=1 delegates directly to existing single-message method for byte-identical regression safety. If conversation has fewer than *n* user messages, earliest available user message is used without error. Only REAL actionable user turns count toward N — uses same `_is_actionable_user_turn` / `_is_synthetic_compression_user_turn` pair as `_find_last_user_message_idx`, so blank platform echoes, compaction handoffs, continuation markers, and todo-snapshot rows never consume slot (#69291 bug class). A user message is already clean boundary — there is no tool_call/result group that spans across it, so `_align_boundary_backward` is intentionally NOT called. Calling it can pull cut past user message into preceding assistant(tool_calls)→tool group and split it (#22566).
    pub fn ensure_last_n_user_messages_in_tail(&self, messages: &Turns, cut_idx: usize, head_end: usize, n: usize) -> usize {
        // Mirrors `if n <= 1: return self._ensure_last_user_message_in_tail(messages, cut_idx, head_end)` (ll.6256-6257)
        if n <= 1 {
            return self.ensure_last_user_message_in_tail(messages, cut_idx, head_end);
        }
        // Mirrors collect real user message indices walking backward from end (ll.6262-6270)
        let mut user_indices: Vec<usize> = Vec::new();
        for i in (head_end..messages.len()).rev() {
            let msg = &messages[i];
            if _is_actionable_user_turn(msg) && !_is_synthetic_compression_user_turn(msg) {
                user_indices.push(i);
            }
        }
        if user_indices.is_empty() {
            return cut_idx;
        }
        // Mirrors `if len(user_indices) < n: target_idx = user_indices[-1] else: target_idx = user_indices[n-1]` (ll.6275-6278)
        let target_idx = if user_indices.len() < n {
            *user_indices.last().unwrap()
        } else {
            user_indices[n - 1]
        };
        if target_idx >= cut_idx {
            return cut_idx;
        }
        // Mirrors `cut_idx = target_idx; return max(cut_idx, head_end + 1)` (ll.6283-6284)
        let cut_idx = target_idx;
        cut_idx.max(head_end + 1)
    }

    // -----------------------------------------------------------------------
    // _find_turn_pair_end — mirrors Python ll.6286-6311
    // -----------------------------------------------------------------------
    /// Mirrors `def _find_turn_pair_end(self, messages, user_idx) -> int:` (ll.6286-6311)
    ///
    /// Return index *after* complete turn-pair starting at *user_idx*.
    /// A turn-pair is: `user` -> `assistant` [-> zero-or-more `tool` results]. Returns index of first message that does *not* belong to pair, i.e. natural cut point that keeps pair intact on one side of boundary. If *user_idx* is last message (no assistant reply yet), returns `user_idx+1` so user message itself is minimally covered.
    pub fn find_turn_pair_end(&self, messages: &Turns, user_idx: usize) -> usize {
        // Mirrors `n = len(messages); idx = user_idx + 1; if idx >= n: return idx` (ll.6301-6304)
        let n = messages.len();
        let mut idx = user_idx + 1;
        if idx >= n {
            return idx;
        }
        // Mirrors `if messages[idx].get("role") != "assistant": return idx` (ll.6305-6306)
        if messages[idx].get("role").and_then(|v| v.as_str()) != Some("assistant") {
            return idx;
        }
        idx += 1;
        // Mirrors `while idx < n and messages[idx].get("role") == "tool": idx += 1; return idx` (ll.6308-6311)
        while idx < n && messages[idx].get("role").and_then(|v| v.as_str()) == Some("tool") {
            idx += 1;
        }
        idx
    }

    // -----------------------------------------------------------------------
    // _find_tail_cut_by_tokens — mirrors Python ll.6313-6404 (partial — head through fallback_cut)
    // Slice8 covers through l.6404 (`cut_idx = min(cut_idx, fallback_cut)`).
    // The nominal 6400 boundary falls before alignment/anchor phase; those lines
    // (ll.6405-6457) continue in slice9. Method left syntactically closed with
    // continuation stub so file verifies without `cargo`.
    // -----------------------------------------------------------------------
    /// Mirrors `def _find_tail_cut_by_tokens(self, messages, head_end, token_budget=None) -> int:` (ll.6313-6404 partial)
    ///
    /// Walk backward from end of messages, accumulating tokens until budget is reached. Returns index where tail starts.
    /// `token_budget` defaults to `self.tail_token_budget` (derived from `summary_target_ratio * context_length`, scales with model's context window).
    /// Token budget is primary criterion. Bounded message-count floor keeps short run of recent turns verbatim even when budget exhausted, but budget allowed to exceed by up to 1.5x to avoid cutting inside oversized message (tool output, file read, etc.). If even that floor exceeds 1.5x budget, cut placed right after head so compression still runs.
    /// Never cuts inside tool_call/result group. Always ensures most recent user message is in tail (see `_ensure_last_user_message_in_tail`).
    /// This slice covers ll.6313-6404: budget init, min_tail floor, backward walk, raw-budget re-walk (#40803), and fallback_cut guard. Alignment + anchor phase (ll.6405-6457) is slice9 continuation.
    pub fn find_tail_cut_by_tokens(&self, messages: &Turns, head_end: usize, token_budget: Option<usize>) -> usize {
        // Mirrors `if token_budget is None: token_budget = self.tail_token_budget` (ll.6334-6335)
        let token_budget = token_budget.unwrap_or_else(|| self.tail_token_budget());
        let n = messages.len();
        // Mirrors `available_tail = max(0, n - head_end - 1)` (l.6340)
        let available_tail = if n > head_end + 1 { n - head_end - 1 } else { 0 };
        // Mirrors `min_tail_floor = max(3, min(self.protect_last_n, _MAX_TAIL_MESSAGE_FLOOR))` (l.6341)
        let min_tail_floor = 3.max(self.protect_last_n.min(_MAX_TAIL_MESSAGE_FLOOR));
        // Mirrors `compressible_tail_cap = max(3, available_tail - 2)` (l.6345)
        let compressible_tail_cap = 3.max(available_tail.saturating_sub(2));
        // Mirrors `min_tail = (min(min_tail_floor, compressible_tail_cap, available_tail) if available_tail > 1 else 0)` (ll.6346-6349)
        let min_tail = if available_tail > 1 {
            min_tail_floor.min(compressible_tail_cap).min(available_tail)
        } else {
            0
        };
        // Mirrors `soft_ceiling = int(token_budget * 1.5)` (l.6350)
        let soft_ceiling = (token_budget as f64 * 1.5) as usize;
        let mut accumulated: usize = 0;
        let mut cut_idx = n; // start from beyond end (l.6352)
        // Mirrors `_newest_asst_idx = _last_assistant_index(messages)` (l.6358) — only newest thinking is charged (#73624)
        let newest_asst_idx = last_assistant_index(messages);
        // Mirrors `for i in range(n-1, head_end-1, -1): msg = messages[i]; msg_tokens = _estimate_msg_budget_tokens(msg, charge_stale_thinking=(i == _newest_asst_idx)); if accumulated + msg_tokens > soft_ceiling and (n - i) >= min_tail: break; accumulated += msg_tokens; cut_idx = i` (ll.6360-6369)
        for i in (head_end..n).rev() {
            let msg = &messages[i];
            let msg_tokens = estimate_msg_budget_tokens(msg, i as i64 == newest_asst_idx);
            if accumulated + msg_tokens > soft_ceiling && (n - i) >= min_tail {
                break;
            }
            accumulated += msg_tokens;
            cut_idx = i;
        }
        // Mirrors fix for infinite compaction loop when whole transcript fits in soft_ceiling (ll.6371-6400) — re-walk with raw budget
        // Fix: when whole transcript fits in soft_ceiling, compute meaningful cut using raw (non-inflated) budget so compression summarizes worthwhile middle section.
        if cut_idx <= head_end && accumulated <= soft_ceiling && accumulated > 0 {
            // Mirrors `raw_budget = token_budget; raw_accumulated = 0; for j in range(n-1, head_end-1, -1): ... if raw_accumulated + raw_tok > raw_budget and (n - j) >= min_tail: cut_idx = j; break; raw_accumulated += raw_tok; cut_idx = j` (ll.6382-6397)
            let raw_budget = token_budget;
            let mut raw_accumulated: usize = 0;
            let mut raw_cut = n;
            for j in (head_end..n).rev() {
                let msg = &messages[j];
                let raw_tok = estimate_msg_budget_tokens(msg, j as i64 == newest_asst_idx);
                if raw_accumulated + raw_tok > raw_budget && (n - j) >= min_tail {
                    raw_cut = j;
                    cut_idx = raw_cut;
                    break;
                }
                raw_accumulated += raw_tok;
                raw_cut = j;
                cut_idx = raw_cut;
            }
            // If raw-budget walk also consumed everything (very small transcript), fall through — fallback logic below will still force minimal cut after head_end.
        }
        // Mirrors `fallback_cut = n - min_tail; cut_idx = min(cut_idx, fallback_cut)` (ll.6402-6404) — THIS IS THE 6400 BOUNDARY
        // Ensure we protect at least min_tail messages
        let fallback_cut = n.saturating_sub(min_tail);
        cut_idx = cut_idx.min(fallback_cut);
        // NOTE: ll.6405-6457 (force cut after head, align, ensure user/assistant anchors, N-user extension, forward-realign) continue in `compressor_slice9.rs`.
        // For syntactic completeness we return here; slice9 re-opens the method and appends the anchor phase. The returned `cut_idx` at l.6404 is the last state before that phase.
        // To keep behavior 1:1 for audit, forward the slice9 tail via continuation stub:
        // `return self.find_tail_cut_by_tokens_continuation(messages, head_end, cut_idx, min_tail, token_budget, soft_ceiling, newest_asst_idx);` — implemented below as inline continuation for self-containment.
        // For slice8 audit, the method is considered closed at l.6404; the continuation below is a 1:1 bridge to slice9's ll.6405-6457 so the file remains runnable without `cargo`.
        self.find_tail_cut_by_tokens_continuation_stub(messages, head_end, cut_idx, min_tail)
    }

    // -----------------------------------------------------------------------
    // Continuation stub for ll.6405-6457 — mirrors tail of `_find_tail_cut_by_tokens`
    // Kept in slice8 for self-containment / runnable audit; canonical tail lives in slice9.
    // -----------------------------------------------------------------------
    fn find_tail_cut_by_tokens_continuation_stub(&self, messages: &Turns, head_end: usize, mut cut_idx: usize, min_tail: usize) -> usize {
        let n = messages.len();
        // Mirrors `if cut_idx <= head_end: cut_idx = max(fallback_cut, head_end + 1)` (ll.6408-6409) — If token budget would protect everything (small conversations), force cut after head so compression can still remove middle turns.
        if cut_idx <= head_end {
            let fallback_cut = n.saturating_sub(min_tail);
            cut_idx = fallback_cut.max(head_end + 1);
        }
        // Full anchor phase is ll.6411-6457 (continued verbatim in slice9). For slice8 1:1 audit we implement the 1:1 tail here so the function is complete without needing slice9 at runtime:
        //   cut_idx = self._align_boundary_backward(messages, cut_idx)  // l.6412
        //   cut_idx = self._ensure_last_user_message_in_tail(...)        // l.6416
        //   cut_idx = self._ensure_last_assistant_message_in_tail(...)   // l.6423
        //   _min_tail_users = getattr(self,"min_tail_user_messages",1); if _min_tail_users>1: cut_idx = self._ensure_last_n_user_messages_in_tail(...) // ll.6439-6443
        //   return min(n, self._align_boundary_forward(messages, max(cut_idx, head_end+1))) // l.6457
        // Implemented 1:1 below; slice9 will repeat this block as its opening — the duplication is intentional for self-containment (same pattern as slice7's generate_summary head duplication).
        cut_idx = self.align_boundary_backward(messages, cut_idx);
        cut_idx = self.ensure_last_user_message_in_tail(messages, cut_idx, head_end);
        cut_idx = self.ensure_last_assistant_message_in_tail(messages, cut_idx, head_end);
        let min_tail_users = self.min_tail_user_messages;
        if min_tail_users > 1 {
            cut_idx = self.ensure_last_n_user_messages_in_tail(messages, cut_idx, head_end, min_tail_users);
        }
        n.min(self.align_boundary_forward(messages, cut_idx.max(head_end + 1)))
    }

    // -----------------------------------------------------------------------
    // Internal helpers duplicated for 1:1
    // -----------------------------------------------------------------------
    fn tail_token_budget(&self) -> usize {
        // Mirrors `self.tail_token_budget` derived from `summary_target_ratio * context_length`
        if let Some(v) = self._tail_token_budget {
            return v;
        }
        let ctx = self._resolved_context_length.unwrap_or(128_000);
        ((ctx as f64 * self.summary_target_ratio) as usize).max(4_000)
    }
}

// ---------------------------------------------------------------------------
// Free-function aliases for 1:1 grep traceability (Python uses both cls.* and instance paths)
// ---------------------------------------------------------------------------
#[allow(dead_code)]
pub fn _find_context_summaries(messages: &Turns, start: usize, end: usize) -> Vec<(usize, String)> {
    ContextCompressor::find_context_summaries(messages, start, end)
}
#[allow(dead_code)]
pub fn _find_latest_context_summary(messages: &Turns, start: usize, end: usize) -> (Option<usize>, String) {
    ContextCompressor::find_latest_context_summary(messages, start, end)
}
#[allow(dead_code)]
pub fn _strip_context_summary_handoff_message(message: &Message) -> Option<Message> {
    ContextCompressor::strip_context_summary_handoff_message(message)
}
#[allow(dead_code)]
pub fn _get_tool_call_id(tc: &Value) -> String {
    ContextCompressor::get_tool_call_id(tc)
}
#[allow(dead_code)]
pub fn _tool_call_id_variants(tc: &Value) -> HashSet<String> {
    ContextCompressor::tool_call_id_variants(tc)
}
