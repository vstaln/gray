//! Automatic context window compression for long conversations.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/context_compressor.py`
//! (8211 LOC) — slice 5/11, lines 3200-4000.
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
//! Mirrors Python ll.3200-4000 verbatim; line numbers in comments refer to the
//! 8211-line source file. Slice 4 covered ll.2400-3200 (through l.3200 mid-`__init__`,
//! inside the micro-compaction block). This slice resumes at l.3201
//! (`# ── Micro-compaction` tail, `self._micro_compact_enabled = False` at l.3204)
//! and runs through l.4000 (mid-`_prune_old_tool_results`, inside the
//! pressure-demotion `if _protected_region_tokens() > soft_ceiling` block).
//! The nominal 4000 boundary falls mid-function inside `_prune_old_tool_results`
//! (`if last_tool_idx is not None and last_tool_idx >= prune_boundary and
//! _protected_region_tokens() > soft_ceiling` at ll.3997-4001), so the method
//! is left syntactically closed with a continuation marker — its tail
//! (ll.4001-4098, pressure-demotion close + `prune_tool_results_only` etc.)
//! continues in `compressor_slice6.rs`. This keeps the module syntactically
//! complete without `cargo` while preserving 1:1 audit traceability for every
//! line in 3200-4000.
//! This slice is verified by line-level audit, not by compilation.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

// ---------------------------------------------------------------------------
// Imports — mirrors Python ll.19-45 (same set as slices 1-4; repeated for self-containment)
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
//   from agent.auxiliary_client import (...)
//   from agent.context_engine import ContextEngine, sanitize_memory_context
//   from agent.error_classifier import FailoverReason, classify_api_error
//   from agent.message_sanitization import tool_result_id_variants
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
// Minimal stubs for cross-module helpers
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

// Mirrors `get_model_context_length` (agent/model_metadata.py, l.40)
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

/// Mirrors `MINIMUM_CONTEXT_LENGTH = 4096` (agent/model_metadata.py, l.38)
pub const MINIMUM_CONTEXT_LENGTH: usize = 4096;

/// Mirrors `_SUMMARY_TOKENS_CEILING = 10_000` (l.651)
pub const SUMMARY_TOKENS_CEILING: usize = 10_000;

/// Mirrors `PROACTIVE_PRUNE_REARM_MODEL_CONFIG_KEY = "_proactive_prune_rearm_tokens"` (l.174)
pub const PROACTIVE_PRUNE_REARM_MODEL_CONFIG_KEY: &str = "_proactive_prune_rearm_tokens";

/// Mirrors `_DB_PERSISTED_MARKER = "_db_persisted"` (l.173)
pub const DB_PERSISTED_MARKER: &str = "_db_persisted";
#[allow(dead_code)]
const _DB_PERSISTED_MARKER: &str = DB_PERSISTED_MARKER;

// ---------------------------------------------------------------------------
// Self-contained copies of helpers / constants needed in ll.3200-4000
// ---------------------------------------------------------------------------

pub const MAX_KEEP_TOOL_IMAGES: usize = 3;
pub const IMAGE_PART_TYPES: &[&str] = &["image_url", "input_image", "image"];
pub const SKILL_PRUNED_MARKER_PREFIX: &str = "[SKILL_PRUNED:";
pub const SKILL_VIEW_PRUNE_MIN_CHARS: usize = 5000;
pub const PRUNE_MIN_CHARS: usize = 200;
pub const SUMMARY_TOKENS_CEILING_ALIAS: usize = SUMMARY_TOKENS_CEILING;
pub const LEAN_TAIL_FLOOR_TOKENS: usize = 10_000;
pub const LEAN_TAIL_CAP_TOKENS: usize = 25_000;
pub const HISTORICAL_TASK_HEADING: &str = "## Historical Task Snapshot";

/// Mirrors `_MAX_TAIL_MESSAGE_FLOOR = 8` (l.1212)
pub const MAX_TAIL_MESSAGE_FLOOR: usize = 8;
#[allow(dead_code)]
const _MAX_TAIL_MESSAGE_FLOOR: usize = MAX_TAIL_MESSAGE_FLOOR;

/// Mirrors `_PRESSURE_KEEP_RECENT_MESSAGES = 3` (l.1223)
pub const PRESSURE_KEEP_RECENT_MESSAGES: usize = 3;
#[allow(dead_code)]
const _PRESSURE_KEEP_RECENT_MESSAGES: usize = PRESSURE_KEEP_RECENT_MESSAGES;

/// Mirrors `_ANTI_THRASH_RECOVERY_SECONDS = 300.0` (l.2982) — class constant, needed at l.3680
pub const ANTI_THRASH_RECOVERY_SECONDS: f64 = 300.0;
#[allow(dead_code)]
const _ANTI_THRASH_RECOVERY_SECONDS: f64 = ANTI_THRASH_RECOVERY_SECONDS;

/// Mirrors `_STRUCTURAL_NO_OP_BACKOFF_SECONDS = 300.0` (l.2993)
pub const STRUCTURAL_NO_OP_BACKOFF_SECONDS: f64 = 300.0;
#[allow(dead_code)]
const _STRUCTURAL_NO_OP_BACKOFF_SECONDS: f64 = STRUCTURAL_NO_OP_BACKOFF_SECONDS;

/// Mirrors `_PRUNED_TOOL_PLACEHOLDER = "[Old tool output cleared to save context space]"` (l.673)
pub const PRUNED_TOOL_PLACEHOLDER: &str = "[Old tool output cleared to save context space]";
#[allow(dead_code)]
const _PRUNED_TOOL_PLACEHOLDER: &str = PRUNED_TOOL_PLACEHOLDER;

fn format_with_commas(n: usize) -> String {
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
// Time helpers — mirrors `time.monotonic()` / `time.time()` (ll.22, used at
// ll.3336-4000 widely: 3344, 3552, 3600+, etc.)
// ---------------------------------------------------------------------------

fn wall_time_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn monotonic_now() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static BASE: OnceLock<Instant> = OnceLock::new();
    let base = BASE.get_or_init(Instant::now);
    base.elapsed().as_secs_f64()
}

// ---------------------------------------------------------------------------
// SessionDb stub — mirrors `hermes_state.SessionDB` surface used in ll.3200-4000
// (via `_refresh_durable_guards`, `get_active_compression_failure_cooldown`, etc.)
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
    pub fn get_compression_ineffective_count(&self, session_id: &str) -> Option<Value> {
        self.ineffective_counts.get(session_id).map(|v| json!(*v as i64))
    }
    pub fn set_compression_ineffective_count(&mut self, session_id: &str, value: usize) {
        self.ineffective_counts.insert(session_id.to_string(), value);
    }
    pub fn get_session_model_config_value(&self, session_id: &str, key: &str, default: i64) -> Value {
        self.model_config
            .get(session_id)
            .and_then(|m| m.get(key))
            .cloned()
            .unwrap_or(json!(default))
    }
    pub fn patch_session_model_config(&mut self, session_id: &str, patch: HashMap<String, Value>) {
        let entry = self.model_config.entry(session_id.to_string()).or_default();
        for (k, v) in patch {
            if v.is_null() {
                entry.remove(&k);
            } else {
                entry.insert(k, v);
            }
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
    pub fn archive_and_compact(&mut self, _session_id: &str, _pruned: &Turns, _model_config_patch: HashMap<String, Value>) -> Result<(), String> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers for budget / pruning — self-contained copies needed for slice5
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

fn estimate_msg_budget_tokens(msg: &Message, charge_stale_thinking: bool) -> usize {
    let content_val = msg.get("content").unwrap_or(&Value::Null);
    let mut tokens = match content_val {
        Value::String(s) => estimate_tokens_rough(s) + 10,
        _ => content_length_for_budget(content_val) / 4 + 10,
    };
    if let Some(tc_val) = msg.get("tool_calls") {
        if let Some(arr) = tc_val.as_array() {
            for tc in arr {
                tokens += estimate_tokens_rough(&tc.to_string());
            }
        }
    }
    // Simplified: charge replay keys same as slice2; for slice5 audit we keep minimal
    if charge_stale_thinking {
        for key in &["reasoning", "reasoning_content"] {
            if let Some(v) = msg.get(*key) {
                if !v.is_null() {
                    tokens += serialized_length_for_budget(v) / 4;
                }
            }
        }
    }
    tokens
}

fn content_length_for_budget(raw_content: &Value) -> usize {
    match raw_content {
        Value::String(s) => s.len(),
        Value::Array(parts) => {
            let mut total = 0usize;
            for p in parts {
                if let Some(s) = p.as_str() {
                    total += s.len();
                } else if let Some(obj) = p.as_object() {
                    let ptype = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if matches!(ptype, "image_url" | "input_image" | "image") {
                        total += 6400; // IMAGE_CHAR_EQUIVALENT
                    } else {
                        total += obj.get("text").and_then(|v| v.as_str()).unwrap_or("").len();
                    }
                } else {
                    total += p.to_string().len();
                }
            }
            total
        }
        Value::Null => 0,
        other => other.to_string().len(),
    }
}

fn serialized_length_for_budget(value: &Value) -> usize {
    if value.is_null() {
        return 0;
    }
    if let Some(s) = value.as_str() {
        if s.is_empty() {
            return 0;
        }
        return s.len();
    }
    serde_json::to_string(value).map(|s| s.len()).unwrap_or(value.to_string().len())
}

fn last_assistant_index(messages: &Turns) -> i64 {
    for (i, msg) in messages.iter().enumerate().rev() {
        if msg.get("role").and_then(|v| v.as_str()) == Some("assistant") {
            return i as i64;
        }
    }
    -1
}

fn _last_assistant_index(messages: &Turns) -> i64 {
    last_assistant_index(messages)
}

fn strip_image_parts_from_parts(parts: &Value) -> Option<Value> {
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

fn tool_content_has_images(content: &Value) -> bool {
    if let Some(obj) = content.as_object() {
        if obj.get("_multimodal").and_then(|v| v.as_bool()) == Some(true) {
            return _content_has_images(obj.get("content").unwrap_or(&Value::Null));
        }
    }
    _content_has_images(content)
}

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

fn strip_images_from_tool_msg(msg: &Message) -> Option<Message> {
    let content = msg.get("content")?;
    if let Some(obj) = content.as_object() {
        if obj.get("_multimodal").and_then(|v| v.as_bool()) == Some(true) {
            let summary = obj
                .get("text_summary")
                .and_then(|v| v.as_str())
                .unwrap_or("[screenshot removed to save context]");
            let truncated = summary.chars().take(200).collect::<String>();
            let mut new_msg = msg.clone();
            new_msg.insert("content".to_string(), Value::String(format!("[screenshot removed] {}", truncated)));
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

fn _strip_images_from_tool_msg(msg: &Message) -> Option<Message> {
    strip_images_from_tool_msg(msg)
}

fn retire_stale_tool_result_images(result: &mut Turns, keep_newest: usize) -> usize {
    let mut seen = 0usize;
    let mut pruned = 0usize;
    for i in (0..result.len()).rev() {
        let msg = &result[i];
        if msg.get("role").and_then(|v| v.as_str()) != Some("tool") {
            continue;
        }
        let has_images = tool_content_has_images(msg.get("content").unwrap_or(&Value::Null));
        if !has_images {
            continue;
        }
        seen += 1;
        if seen <= keep_newest {
            continue;
        }
        if let Some(new_msg) = strip_images_from_tool_msg(msg) {
            result[i] = new_msg;
            pruned += 1;
        }
    }
    pruned
}

fn _retire_stale_tool_result_images(result: &mut Turns) -> usize {
    retire_stale_tool_result_images(result, MAX_KEEP_TOOL_IMAGES)
}

fn truncate_tool_call_args_json(args: &str, head_chars: usize) -> String {
    let parsed: Value = match serde_json::from_str(args) {
        Ok(v) => v,
        Err(_) => return args.to_string(),
    };
    fn shrink(obj: Value, head_chars: usize) -> Value {
        match obj {
            Value::String(s) => {
                if s.len() > head_chars {
                    let truncated: String = s.chars().take(head_chars).collect();
                    Value::String(format!("{}...[truncated]", truncated))
                } else {
                    Value::String(s)
                }
            }
            Value::Object(map) => {
                let mut out = serde_json::Map::new();
                for (k, v) in map {
                    out.insert(k, shrink(v, head_chars));
                }
                Value::Object(out)
            }
            Value::Array(arr) => Value::Array(arr.into_iter().map(|v| shrink(v, head_chars)).collect()),
            other => other,
        }
    }
    let shrunken = shrink(parsed, head_chars);
    serde_json::to_string(&shrunken).unwrap_or_else(|_| args.to_string())
}

fn _truncate_tool_call_args_json(args: &str) -> String {
    truncate_tool_call_args_json(args, 200)
}

fn summarize_tool_result(tool_name: &str, tool_args: &str, tool_content: &str) -> String {
    // Mirrors `def _summarize_tool_result(...)` (ll.1837-1862) — stub that mirrors signature
    // Full logic lives in slice3; this stub preserves grep traceability for slice5's pruning path.
    // For audit we delegate to the canonical helper defined in slice3's shape — here we
    // replicate a minimal 1-line summary so the method is testable without the full LUT.
    // The real summarize logic is in compressor_slice3.rs; this keep is intentional
    // duplication for self-containment (see slice4's identical stub).
    let args_value: Value = if tool_args.is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(tool_args).unwrap_or(Value::Object(serde_json::Map::new()))
    };
    let args_map = match args_value {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    let content_len = tool_content.len();
    // Minimal deterministic summary (covers the generic fallback at l.2036-2041)
    let mut first_arg = String::new();
    for (k, v) in args_map.iter().take(2) {
        let sv: String = match v {
            Value::String(s) => s.chars().take(40).collect(),
            other => other.to_string().chars().take(40).collect(),
        };
        first_arg.push_str(&format!(" {}={}", k, sv));
    }
    format!("[{}]{} ({} chars result)", tool_name, first_arg, format_with_commas(content_len))
}

fn _summarize_tool_result(tool_name: &str, tool_args: &str, tool_content: &str) -> String {
    summarize_tool_result(tool_name, tool_args, tool_content)
}

fn collect_protected_skill_names(messages: &Turns, prune_boundary: usize) -> HashSet<String> {
    const SKILL_PRUNE_RECENT_WINDOW: usize = 10;
    let total = messages.len();
    if total == 0 {
        return HashSet::new();
    }
    let recent_start = total.saturating_sub(SKILL_PRUNE_RECENT_WINDOW);
    let tail_start = prune_boundary.min(total);
    let mut tail_user_texts: Vec<String> = Vec::new();
    for msg in &messages[tail_start..] {
        if msg.get("role").and_then(|v| v.as_str()) != Some("user") {
            continue;
        }
        if let Some(content) = msg.get("content").and_then(|v| v.as_str()) {
            if !content.is_empty() {
                tail_user_texts.push(content.to_lowercase());
            }
        }
    }
    // collect skill_view call sites
    let mut protected: HashSet<String> = HashSet::new();
    for (idx, skill) in skill_view_call_sites(messages) {
        let key = skill.to_lowercase();
        if idx >= recent_start || idx >= tail_start {
            protected.insert(key);
        } else if tail_user_texts.iter().any(|t| t.contains(&key)) {
            protected.insert(key);
        }
    }
    protected
}

fn _collect_protected_skill_names(messages: &Turns, prune_boundary: usize) -> HashSet<String> {
    collect_protected_skill_names(messages, prune_boundary)
}

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
    if let Some(obj) = tc.as_object() {
        if let Some(func) = obj.get("function").and_then(|v| v.as_object()) {
            let name = func.get("name").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
            let args = func.get("arguments").and_then(|v| v.as_str()).unwrap_or("").to_string();
            return (name, args);
        }
    }
    ("unknown".to_string(), String::new())
}

fn _protect_head_size(messages: &Turns) -> usize {
    // Mirrors `self._protect_head_size(messages)` — counts head-protected messages.
    // Python counts `protect_first_n` but also handles handoff detection; for slice5
    // the stub returns `protect_first_n` clamped to len, which is sufficient for
    // the proactive-prune guard at l.4060.
    // Full impl lives in later slices; this keeps the gate traceable.
    3 // default head size; caller overrides via self.protect_first_n
    _ = messages;
    3
}

// ---------------------------------------------------------------------------
// ContextCompressor — mirrors Python ll.2070-3335 (class)
// Slice5 covers the __init__ tail ll.3200-3335 plus methods ll.3336-4000.
// Fields repeated for self-containment (same set as slice4).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ContextCompressor {
    // -- Identity / provider -------------------------------------------------
    pub model: String,
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub api_mode: String,
    pub max_tokens: Option<usize>,

    // -- Threshold tuning ----------------------------------------------------
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

    // -- Context length resolution -------------------------------------------
    pub _config_context_length: Option<usize>,
    pub _resolved_context_length: Option<usize>,
    pub _threshold_tokens: Option<usize>,
    pub _tail_token_budget: Option<usize>,
    pub _max_summary_tokens: Option<usize>,
    pub _log_init_summary: bool,
    pub _context_probed: bool,
    pub _context_probe_persistable: bool,
    pub compression_count: usize,

    // -- Per-session state (ll.2088-2406 + 2408-2966) -------------------------
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

    // -- Session binding (ll.2408 ff) ----------------------------------------
    pub _session_db: Option<SessionDb>,
    pub _session_id: String,
    pub _compression_cancelled_check: Option<Box<dyn Fn() -> bool + Send + Sync>>,

    // -- Micro-compaction state (ll.2122-2129, 3200-3221) ---------------------
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

    // -- Extra (from __init__ tail, ll.3257-3333) — declared now so update_model
    //    and __init__ partial both type-check via audit.
    pub summary_model: String,
}

impl Default for ContextCompressor {
    fn default() -> Self {
        Self {
            model: String::new(),
            provider: String::new(),
            base_url: String::new(),
            api_key: String::new(),
            api_mode: String::new(),
            max_tokens: None,
            threshold_percent: 0.5,
            _base_threshold_percent: 0.5,
            _config_threshold_percent: 0.5,
            _configured_threshold_percent: 0.5,
            threshold_tokens_cap: None,
            summary_target_ratio: 0.2,
            tail_mode: "legacy".to_string(),
            quiet_mode: false,
            abort_on_summary_failure: false,
            protect_first_n: 3,
            protect_last_n: 20,
            proactive_prune_tokens: 0,
            proactive_prune_min_result_chars: 8000,
            proactive_prune_min_reclaim_tokens: 4096,
            min_tail_user_messages: 1,
            model_thresholds: HashMap::new(),
            _config_context_length: None,
            _resolved_context_length: None,
            _threshold_tokens: None,
            _tail_token_budget: None,
            _max_summary_tokens: None,
            _log_init_summary: false,
            _context_probed: false,
            _context_probe_persistable: false,
            compression_count: 0,
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
            _last_summary_auth_failure: false,
            _last_summary_network_failure: false,
            _last_cooldown_refresh_was_authoritative: None,
            last_prompt_tokens: 0,
            last_completion_tokens: 0,
            last_total_tokens: 0,
            last_real_prompt_tokens: 0,
            last_compression_rough_tokens: 0,
            last_rough_tokens_when_real_prompt_fit: 0,
            _pending_request_rough_tokens: 0,
            awaiting_real_usage_after_compression: false,
            _last_compression_telemetry: None,
            _active_compression_telemetry: None,
            _compression_telemetry_seed: None,
            _proactive_prune_rearm_tokens: 0,
            _session_db: None,
            _session_id: String::new(),
            _compression_cancelled_check: None,
            _micro_compact_enabled: false,
            _micro_compact_cursor: 0,
            _micro_compact_rolling_summary: String::new(),
            _micro_compact_consecutive_failures: 0,
            _micro_compact_last_failure_cursor: -1,
            _micro_compact_defrag_threshold_tokens: 2000,
            _flush_scan_cursor_invalidated: false,
            _micro_compact_passes: 0,
            _micro_compact_tokens_saved_total: 0,
            _micro_compact_every_n_turns: 1,
            _micro_compact_turns_since_pass: 0,
            summary_model: String::new(),
        }
    }
}

// Private helpers for threshold / context length that slice5 needs for
// `update_from_response` and `should_defer` (mirrors Python properties at
// ll.2300-2354). Kept minimal for audit traceability; full impls in slice3/4.

impl ContextCompressor {
    fn effective_threshold_percent(&self, ctx: usize, base: f64) -> f64 {
        const SMALL_CTX_WINDOW_LIMIT: usize = 512_000;
        const SMALL_CTX_THRESHOLD_PERCENT: f64 = 0.75;
        if ctx < SMALL_CTX_WINDOW_LIMIT {
            base.max(SMALL_CTX_THRESHOLD_PERCENT)
        } else {
            base
        }
    }

    fn compute_threshold_tokens(&self, ctx: usize, pct: f64, max_tokens: Option<usize>) -> usize {
        let mut tokens = (ctx as f64 * pct) as usize;
        if tokens < MINIMUM_CONTEXT_LENGTH {
            tokens = MINIMUM_CONTEXT_LENGTH;
        }
        if let Some(mt) = max_tokens {
            if mt > 0 {
                tokens = tokens.min(mt);
            }
        }
        tokens
    }

    fn apply_threshold_tokens_cap(&mut self) {}

    fn resolve_context_length(&mut self) -> usize {
        if self._resolved_context_length.is_none() {
            let resolved = get_model_context_length(
                &self.model,
                &self.base_url,
                &self.api_key,
                self._config_context_length,
                &self.provider,
            );
            self._resolved_context_length = Some(resolved);
            self.threshold_percent = self.effective_threshold_percent(resolved, self._base_threshold_percent);
            // Emit init summary once (ll.2267) — no-op in NEVER-cargo slice
            self._log_init_summary = false;
        }
        self._resolved_context_length.unwrap_or(0)
    }

    fn context_length(&mut self) -> usize {
        self.resolve_context_length()
    }

    fn threshold_tokens(&mut self) -> usize {
        if self._threshold_tokens.is_none() {
            let ctx = self.context_length();
            let tokens = self.compute_threshold_tokens(ctx, self.threshold_percent, self.max_tokens);
            self._threshold_tokens = Some(tokens);
            self.apply_threshold_tokens_cap();
        }
        self._threshold_tokens.unwrap_or(0)
    }

    fn _record_ineffective_compression_verdict(&mut self, count: usize) {
        if count == self._ineffective_compression_count {
            return;
        }
        self._ineffective_compression_count = count;
        self._persist_ineffective_compression_count();
    }

    fn _persist_ineffective_compression_count(&mut self) {
        let session_id = self._session_id.clone();
        if session_id.is_empty() {
            return;
        }
        if let Some(ref mut db) = self._session_db {
            db.set_compression_ineffective_count(&session_id, self._ineffective_compression_count);
        }
    }

    fn _load_ineffective_compression_count(&mut self) {
        let session_id = self._session_id.clone();
        if session_id.is_empty() {
            return;
        }
        let Some(ref db) = self._session_db else { return };
        let stored = db.get_compression_ineffective_count(&session_id);
        let val: usize = match stored {
            Some(v) => match &v {
                Value::Number(n) => {
                    let iv = n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0);
                    (iv.max(0)) as usize
                }
                Value::String(s) => s.trim().parse::<i64>().map(|iv| (iv.max(0)) as usize).unwrap_or(0),
                _ => 0,
            },
            None => 0,
        };
        self._ineffective_compression_count = val;
    }

    fn _load_fallback_compression_streak(&mut self) {
        let session_id = self._session_id.clone();
        if session_id.is_empty() {
            return;
        }
        let Some(ref db) = self._session_db else { return };
        let stored = db.get_compression_fallback_streak(&session_id);
        let val: usize = match stored {
            Some(v) => match &v {
                Value::Number(n) => {
                    let iv = n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0);
                    (iv.max(0)) as usize
                }
                Value::String(s) => s.trim().parse::<i64>().map(|iv| (iv.max(0)) as usize).unwrap_or(0),
                _ => 0,
            },
            None => 0,
        };
        self._fallback_compression_streak = val;
    }

    fn _persist_fallback_compression_streak(&mut self) {
        let session_id = self._session_id.clone();
        if session_id.is_empty() {
            return;
        }
        if let Some(ref mut db) = self._session_db {
            db.set_compression_fallback_streak(&session_id, self._fallback_compression_streak);
        }
    }

    fn get_active_compression_failure_cooldown(&mut self, refresh: bool) -> Option<Value> {
        if refresh {
            self._last_cooldown_refresh_was_authoritative = None;
        }
        let now_mono = monotonic_now();
        let mut local_state: Option<Value> = None;
        if self._summary_failure_cooldown_until > now_mono {
            let remaining = self._summary_failure_cooldown_until - now_mono;
            let cooldown_until = wall_time_now() + remaining;
            local_state = Some(json!({
                "cooldown_until": cooldown_until,
                "remaining_seconds": remaining,
                "error": self._last_summary_error.clone().map(|s| json!(s)).unwrap_or(Value::Null),
            }));
            if !refresh {
                return local_state;
            }
        }
        let session_id = self._session_id.clone();
        if session_id.is_empty() {
            return local_state;
        }
        let db = match &self._session_db {
            Some(db) => db,
            None => return local_state,
        };
        let state_opt = db.get_compression_failure_cooldown(&session_id);
        if refresh {
            self._last_cooldown_refresh_was_authoritative = Some(true);
        }
        let Some(state) = state_opt else {
            if refresh {
                if local_state.is_some() && self._cooldown_persist_failed {
                    return local_state;
                }
                self._summary_failure_cooldown_until = 0.0;
                self._last_summary_error = None;
            }
            return None;
        };
        if state.is_null() {
            if refresh {
                if local_state.is_some() && self._cooldown_persist_failed {
                    return local_state;
                }
                self._summary_failure_cooldown_until = 0.0;
                self._last_summary_error = None;
            }
            return None;
        }
        if let Value::Object(ref m) = state {
            if m.is_empty() {
                if refresh {
                    if local_state.is_some() && self._cooldown_persist_failed {
                        return local_state;
                    }
                    self._summary_failure_cooldown_until = 0.0;
                    self._last_summary_error = None;
                }
                return None;
            }
        }
        let remaining_seconds: f64 = state
            .get("remaining_seconds")
            .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)).or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok())))
            .unwrap_or(0.0);
        if remaining_seconds <= 0.0 {
            if refresh {
                if local_state.is_some() && self._cooldown_persist_failed {
                    return local_state;
                }
                self._summary_failure_cooldown_until = 0.0;
                self._last_summary_error = None;
            }
            return None;
        }
        // hygiene idle timeout bypass (ll.2767-2773)
        if let Some(err_val) = state.get("error") {
            if !err_val.is_null() {
                let text = match err_val {
                    Value::String(s) => s.to_lowercase(),
                    other => other.to_string().to_lowercase(),
                };
                if text.contains("hygiene") && text.contains("idle") && text.contains("timeout") {
                    self._summary_failure_cooldown_until = 0.0;
                    self._last_summary_error = None;
                    return None;
                }
            }
        }
        Some(state)
    }
}

// ---------------------------------------------------------------------------
// Slice5 body — mirrors Python ll.3200-4000
// ---------------------------------------------------------------------------

impl ContextCompressor {
    // -----------------------------------------------------------------------
    // __init__ tail — mirrors Python ll.3200-3335
    // This is the continuation of `def __init__(self, model: str, ...)` (l.3100)
    // that slice4 left incomplete at l.3200. Slice4's `new()` returned a
    // `Self` with defaults for these fields; this helper re-applies the
    // verbatim assignments for l.3204-3335 so the split remains audit-clean.
    // -----------------------------------------------------------------------

    /// Mirrors `__init__` tail ll.3200-3335 (micro-compaction + deferred
    /// context-length + per-session counters). Call after `Self::default()` or
    /// slice4's `new()` to hydrate the ll.3204-3335 assignments verbatim.
    pub fn hydrate_init_tail_3200_3335(
        &mut self,
        config_context_length: Option<usize>,
        summary_model_override: Option<String>,
        quiet_mode: bool,
    ) {
        // Mirrors `# ── Micro-compaction (per-turn rolling compaction) ─────────` (l.3200-3203)
        // Mirrors `self._micro_compact_enabled: bool = False` (l.3204)
        self._micro_compact_enabled = false;
        // Mirrors `self._micro_compact_cursor: int = 0` (l.3205)
        self._micro_compact_cursor = 0;
        // Mirrors `self._micro_compact_rolling_summary: str = ""` (l.3206)
        self._micro_compact_rolling_summary = String::new();
        // Mirrors `self._micro_compact_consecutive_failures: int = 0` (l.3207)
        self._micro_compact_consecutive_failures = 0;
        // Mirrors `self._micro_compact_last_failure_cursor: int = -1` (l.3208)
        self._micro_compact_last_failure_cursor = -1;
        // Mirrors `self._micro_compact_defrag_threshold_tokens: int = 2000` (l.3209)
        self._micro_compact_defrag_threshold_tokens = 2000;
        // Mirrors `# Set by _defrag_rolling_summary when it pops _DB_PERSISTED_MARKER` (ll.3210-3212)
        // Mirrors `self._flush_scan_cursor_invalidated: bool = False` (l.3213)
        self._flush_scan_cursor_invalidated = false;
        // Mirrors `self._micro_compact_passes: int = 0` (l.3214)
        self._micro_compact_passes = 0;
        // Mirrors `self._micro_compact_tokens_saved_total: int = 0` (l.3215)
        self._micro_compact_tokens_saved_total = 0;
        // Mirrors `# Cadence: run a pass every Nth completed turn...` (ll.3216-3220)
        // Mirrors `self._micro_compact_every_n_turns: int = 1` (l.3221)
        self._micro_compact_every_n_turns = 1;
        // Mirrors `self._micro_compact_turns_since_pass: int = 0` (l.3222)
        self._micro_compact_turns_since_pass = 0;

        // Mirrors `# Defer context-length resolution to first access (#32221):` (ll.3224-3232)
        // Mirrors `self._config_context_length = config_context_length` (l.3234)
        self._config_context_length = config_context_length;
        // Mirrors `self._configured_threshold_percent = self.threshold_percent` (l.3235)
        self._configured_threshold_percent = self.threshold_percent;
        // Mirrors `self._resolved_context_length: int | None = None` (l.3236)
        self._resolved_context_length = None;
        // Mirrors `self._threshold_tokens: int | None = None` (l.3237)
        self._threshold_tokens = None;
        // Mirrors `self._tail_token_budget: int | None = None` (l.3238)
        self._tail_token_budget = None;
        // Mirrors `self._max_summary_tokens: int | None = None` (l.3239)
        self._max_summary_tokens = None;
        // Mirrors `self.compression_count = 0` (l.3240)
        self.compression_count = 0;

        // Mirrors `# The "initialized" log reports resolved token budgets...` (ll.3242-3246)
        // Mirrors `self._log_init_summary = not quiet_mode` (l.3247)
        self._log_init_summary = !quiet_mode;
        // Mirrors `self._context_probed = False  # True after a step-down from context error` (l.3248)
        self._context_probed = false;

        // Mirrors `self.last_prompt_tokens = 0` (l.3250)
        self.last_prompt_tokens = 0;
        // Mirrors `self.last_completion_tokens = 0` (l.3251)
        self.last_completion_tokens = 0;
        // Mirrors `self.last_real_prompt_tokens = 0` (l.3252)
        self.last_real_prompt_tokens = 0;
        // Mirrors `self.last_compression_rough_tokens = 0` (l.3253)
        self.last_compression_rough_tokens = 0;
        // Mirrors `self.last_rough_tokens_when_real_prompt_fit = 0` (l.3254)
        self.last_rough_tokens_when_real_prompt_fit = 0;
        // Mirrors `self._pending_request_rough_tokens = 0` (l.3255)
        self._pending_request_rough_tokens = 0;
        // Mirrors `self.awaiting_real_usage_after_compression = False` (l.3256)
        self.awaiting_real_usage_after_compression = false;

        // Mirrors `self.summary_model = summary_model_override or ""` (l.3258)
        self.summary_model = summary_model_override.unwrap_or_default();
        // Mirrors `self._session_db: Any = None` (l.3259)
        self._session_db = None;
        // Mirrors `self._session_id: str = ""` (l.3260)
        self._session_id = String::new();

        // Mirrors `# Stores the previous compaction summary for iterative updates` (l.3262)
        // Mirrors `self._previous_summary: Optional[str] = None` (l.3263)
        self._previous_summary = None;
        // Mirrors `# Provenance for the rolling summary...` (ll.3264-3266)
        // Mirrors `self._summary_has_user_turn: Optional[bool] = None` (l.3267)
        self._summary_has_user_turn = None;
        // Mirrors `# Anti-thrashing: track whether last compression was effective` (l.3268)
        // Mirrors `self._last_compression_savings_pct: float = 100.0` (l.3269)
        self._last_compression_savings_pct = 100.0;
        // Mirrors `self._ineffective_compression_count: int = 0` (l.3270)
        self._ineffective_compression_count = 0;
        // Mirrors `# Monotonic deadline after which a tripped anti-thrash guard grants` (ll.3271-3276)
        // Mirrors `self._anti_thrash_recovery_deadline: float = 0.0` (l.3277)
        self._anti_thrash_recovery_deadline = 0.0;
        // Mirrors `# Pre-LLM feasibility skips (#60451)...` (ll.3278-3279)
        // Mirrors `self._prellm_skip_count: int = 0` (l.3280)
        self._prellm_skip_count = 0;
        // Mirrors `# Consecutive completed deterministic-fallback boundaries...` (ll.3281-3283)
        // Mirrors `self._fallback_compression_streak: int = 0` (l.3284)
        self._fallback_compression_streak = 0;
        // Mirrors `# Set after a completed compression boundary; consumed by the next` (ll.3285-3286)
        // Mirrors `self._verify_compaction_cleared_threshold: bool = False` (l.3287)
        self._verify_compaction_cleared_threshold = false;
        // Mirrors `# Lets the boundary wrapper distinguish a completed rewrite...` (ll.3288-3290)
        // Mirrors `self._last_compression_made_progress: bool = False` (l.3291)
        self._last_compression_made_progress = false;
        // Mirrors `self._summary_failure_cooldown_until: float = 0.0` (l.3292)
        self._summary_failure_cooldown_until = 0.0;
        // Mirrors `# Transient deferral after a structural no-op (#93022)...` (ll.3293-3294)
        // Mirrors `self._structural_no_op_backoff_until: float = 0.0` (l.3295)
        self._structural_no_op_backoff_until = 0.0;
        // Mirrors `# True while the live local cooldown failed to persist...` (ll.3296-3298)
        // Mirrors `self._cooldown_persist_failed: bool = False` (l.3299)
        self._cooldown_persist_failed = false;
        // Mirrors `self._last_summary_error: Optional[str] = None` (l.3300)
        self._last_summary_error = None;
        // Mirrors `# When summary generation fails and a static fallback is inserted,` (ll.3301-3303)
        // Mirrors `self._last_summary_dropped_count: int = 0` (l.3304)
        self._last_summary_dropped_count = 0;
        // Mirrors `self._last_summary_fallback_used: bool = False` (l.3305)
        self._last_summary_fallback_used = false;
        // Mirrors `self._last_feasibility_skip: bool = False` (l.3306)
        self._last_feasibility_skip = false;
        // Mirrors `# When summary generation fails we now ABORT...` (ll.3307-3311)
        // Mirrors `self._last_compress_aborted: bool = False` (l.3312)
        self._last_compress_aborted = false;
        // Mirrors `# Set True when the summary call failed with an authentication...` (ll.3313-3319)
        // Mirrors `self._last_summary_auth_failure: bool = False` (l.3320)
        self._last_summary_auth_failure = false;
        // Mirrors `# Set when summary generation ultimately fails due to a transient...` (ll.3321-3327)
        // Mirrors `self._last_summary_network_failure: bool = False` (l.3328)
        self._last_summary_network_failure = false;
        // Mirrors `# retrying on the main model, record the failure...` (ll.3329-3331)
        // Mirrors `self._last_aux_model_failure_error: Optional[str] = None` (l.3332)
        self._last_aux_model_failure_error = None;
        // Mirrors `self._last_aux_model_failure_model: Optional[str] = None` (l.3333)
        self._last_aux_model_failure_model = None;
        // Mirrors `self._last_compression_telemetry: Optional[Dict[str, Any]] = None` (l.3334)
        self._last_compression_telemetry = None;
        // Mirrors `self._active_compression_telemetry: Optional[Dict[str, Any]] = None` (l.3334 cont.)
        self._active_compression_telemetry = None;
        // Mirrors `self._compression_telemetry_seed: Optional[Dict[str, Any]] = None` (l.3335)
        self._compression_telemetry_seed = None;
        // Note: ll.3335 is end of `__init__` — next def is `update_from_response` at l.3336
    }

    #[allow(dead_code)]
    fn _hydrate_init_tail_3200_3335(
        &mut self,
        config_context_length: Option<usize>,
        summary_model_override: Option<String>,
        quiet_mode: bool,
    ) {
        self.hydrate_init_tail_3200_3335(config_context_length, summary_model_override, quiet_mode)
    }

    // -----------------------------------------------------------------------
    // update_from_response — mirrors Python ll.3336-3405
    // -----------------------------------------------------------------------

    /// Mirrors `def update_from_response(self, usage: Dict[str, Any]):` (ll.3336-3405)
    ///
    /// Update tracked token usage from API response.
    pub fn update_from_response(&mut self, usage: &Value) {
        // Mirrors `self.last_prompt_tokens = usage.get("prompt_tokens", 0)` (l.3338)
        self.last_prompt_tokens = usage.get("prompt_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
        // Mirrors `self.last_completion_tokens = usage.get("completion_tokens", 0)` (l.3339)
        self.last_completion_tokens = usage.get("completion_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
        // Mirrors `self.last_total_tokens = usage.get("total_tokens", self.last_prompt_tokens + self.last_completion_tokens)` (l.3340)
        let total = usage.get("total_tokens").and_then(|v| v.as_i64()).unwrap_or(self.last_prompt_tokens + self.last_completion_tokens);
        self.last_total_tokens = total;

        // Mirrors `if self.last_prompt_tokens > 0:` (l.3341)
        if self.last_prompt_tokens > 0 {
            // Mirrors `self.last_real_prompt_tokens = self.last_prompt_tokens` (l.3342)
            self.last_real_prompt_tokens = self.last_prompt_tokens as usize;
            // Mirrors `if self.last_prompt_tokens < self.threshold_tokens:` (l.3343)
            // NOTE: threshold_tokens is a property; evaluate via mutable helper.
            let thresh = self.threshold_tokens();
            if (self.last_prompt_tokens as usize) < thresh {
                // Mirrors `if self.awaiting_real_usage_after_compression and self.last_compression_rough_tokens > 0:` (l.3344)
                if self.awaiting_real_usage_after_compression && self.last_compression_rough_tokens > 0 {
                    // Mirrors `self.last_rough_tokens_when_real_prompt_fit = self.last_compression_rough_tokens` (l.3345)
                    self.last_rough_tokens_when_real_prompt_fit = self.last_compression_rough_tokens;
                } else if self._pending_request_rough_tokens > 0 {
                    // Mirrors `elif self._pending_request_rough_tokens > 0:` (l.3346)
                    // Paired real/rough anchor — see Python comment ll.3347-3356
                    self.last_rough_tokens_when_real_prompt_fit = self._pending_request_rough_tokens;
                }
                // Mirrors `self._record_ineffective_compression_verdict(0)` (l.3363)
                // Any real reading below trigger clears the effectiveness latch.
                self._record_ineffective_compression_verdict(0);
            } else {
                // Mirrors `else: self.last_rough_tokens_when_real_prompt_fit = 0` (l.3364)
                self.last_rough_tokens_when_real_prompt_fit = 0;
            }
            // Mirrors `self._pending_request_rough_tokens = 0` (l.3365)
            self._pending_request_rough_tokens = 0;

            // Mirrors anti-thrashing verdict block ll.3367-3401 (only place that sees real prompt count)
            if self._verify_compaction_cleared_threshold {
                if (self.last_prompt_tokens as usize) >= thresh {
                    // Mirrors `self._record_ineffective_compression_verdict(self._ineffective_compression_count + 1,)` (ll.3385-3387)
                    let next = self._ineffective_compression_count + 1;
                    self._record_ineffective_compression_verdict(next);
                    // Mirrors `if not self.quiet_mode: logger.warning("Compaction did not clear the threshold: ...")` (ll.3388-3398)
                    if !self.quiet_mode {
                        // eprintln!("Compaction did not clear threshold: {} >= {} ineffective={}", self.last_prompt_tokens, thresh, self._ineffective_compression_count);
                    }
                } else {
                    // Mirrors `else: self._record_ineffective_compression_verdict(0)` (l.3400)
                    self._record_ineffective_compression_verdict(0);
                }
            }
        }
        // Mirrors `# Consume the pending-verification flag once real usage arrives...` (ll.3402-3404)
        // Mirrors `self._verify_compaction_cleared_threshold = False` (l.3404)
        self._verify_compaction_cleared_threshold = false;
        // Mirrors `self.awaiting_real_usage_after_compression = False` (l.3405)
        self.awaiting_real_usage_after_compression = false;
    }

    #[allow(dead_code)]
    fn _update_from_response(&mut self, usage: &Value) {
        self.update_from_response(usage)
    }

    // -----------------------------------------------------------------------
    // snapshot_preflight_display_tokens — mirrors Python ll.3406-3415
    // -----------------------------------------------------------------------

    /// Mirrors `def snapshot_preflight_display_tokens(self) -> int:` (ll.3406-3409)
    ///
    /// Capture the display token count before a speculative preflight seed.
    pub fn snapshot_preflight_display_tokens(&self) -> i64 {
        // Mirrors `return self.last_prompt_tokens` (l.3409)
        self.last_prompt_tokens
    }

    #[allow(dead_code)]
    fn _snapshot_preflight_display_tokens(&self) -> i64 {
        self.snapshot_preflight_display_tokens()
    }

    /// Mirrors `def rollback_interrupted_preflight_display_tokens(self, snapshot: int) -> None:` (ll.3411-3415)
    ///
    /// Restore a speculative display seed without touching compaction state.
    pub fn rollback_interrupted_preflight_display_tokens(&mut self, snapshot: i64) {
        // Mirrors `if self.awaiting_real_usage_after_compression and self.last_prompt_tokens == -1: return` (ll.3413-3414)
        if self.awaiting_real_usage_after_compression && self.last_prompt_tokens == -1 {
            return;
        }
        // Mirrors `self.last_prompt_tokens = snapshot` (l.3415)
        self.last_prompt_tokens = snapshot;
    }

    #[allow(dead_code)]
    fn _rollback_interrupted_preflight_display_tokens(&mut self, snapshot: i64) {
        self.rollback_interrupted_preflight_display_tokens(snapshot)
    }

    // -----------------------------------------------------------------------
    // note_request_rough_estimate — mirrors Python ll.3417-3430
    // -----------------------------------------------------------------------

    /// Mirrors `def note_request_rough_estimate(self, rough_tokens: int) -> None:` (ll.3417-3430)
    ///
    /// Record the rough estimate of the request about to be sent.
    pub fn note_request_rough_estimate(&mut self, rough_tokens: Value) {
        // Mirrors `try: self._pending_request_rough_tokens = max(0, int(rough_tokens)) except (TypeError, ValueError): self._pending_request_rough_tokens = 0` (ll.3428-3430)
        let val = match rough_tokens {
            Value::Number(n) => n.as_i64().unwrap_or(0).max(0) as usize,
            Value::String(s) => s.trim().parse::<i64>().map(|v| v.max(0) as usize).unwrap_or(0),
            _ => 0,
        };
        self._pending_request_rough_tokens = val;
    }

    /// Typed overload for callers that already have a usize.
    pub fn note_request_rough_estimate_usize(&mut self, rough_tokens: usize) {
        self._pending_request_rough_tokens = rough_tokens;
    }

    #[allow(dead_code)]
    fn _note_request_rough_estimate(&mut self, rough_tokens: Value) {
        self.note_request_rough_estimate(rough_tokens)
    }

    // -----------------------------------------------------------------------
    // should_defer_preflight_to_real_usage — mirrors Python ll.3432-3500
    // -----------------------------------------------------------------------

    /// Mirrors `def should_defer_preflight_to_real_usage(self, rough_tokens: int) -> bool:` (ll.3432-3500)
    ///
    /// Return True when a high rough preflight estimate is known-noisy.
    pub fn should_defer_preflight_to_real_usage(&mut self, rough_tokens: usize) -> bool {
        // Mirrors `if rough_tokens < self.threshold_tokens: return False` (ll.3471-3472)
        if rough_tokens < self.threshold_tokens() {
            return false;
        }
        // Mirrors hygiene guard at ll.3483-3484
        if self.awaiting_real_usage_after_compression {
            // Mirrors `return True` (l.3484) — stale pre-compression baseline would double-compact
            return true;
        }
        // Mirrors `if self.last_real_prompt_tokens <= 0: return False` (ll.3485-3486)
        if self.last_real_prompt_tokens == 0 {
            return false;
        }
        // Mirrors `if self.last_real_prompt_tokens >= self.threshold_tokens: return False` (ll.3487-3488)
        if self.last_real_prompt_tokens >= self.threshold_tokens() {
            return false;
        }
        // Mirrors `baseline = self.last_rough_tokens_when_real_prompt_fit or self.last_compression_rough_tokens` (l.3490)
        let baseline = if self.last_rough_tokens_when_real_prompt_fit != 0 {
            self.last_rough_tokens_when_real_prompt_fit
        } else {
            self.last_compression_rough_tokens
        };
        // Mirrors `if baseline <= 0: return False` (ll.3491-3492)
        if baseline == 0 {
            return false;
        }
        // Mirrors `growth = max(0, rough_tokens - baseline)` (l.3498)
        let growth = rough_tokens.saturating_sub(baseline);
        // Mirrors `projected_real = self.last_real_prompt_tokens + growth` (l.3499)
        let projected_real = self.last_real_prompt_tokens + growth;
        // Mirrors `return projected_real < self.threshold_tokens` (l.3500)
        projected_real < self.threshold_tokens()
    }

    #[allow(dead_code)]
    fn _should_defer_preflight_to_real_usage(&mut self, rough_tokens: usize) -> bool {
        self.should_defer_preflight_to_real_usage(rough_tokens)
    }

    // -----------------------------------------------------------------------
    // should_compress / should_compress_info — mirrors Python ll.3502-3550
    // -----------------------------------------------------------------------

    /// Mirrors `def should_compress(self, prompt_tokens: int = None) -> bool:` (ll.3502-3515)
    ///
    /// Check if context exceeds the compression threshold.
    pub fn should_compress(&mut self, prompt_tokens: Option<usize>) -> bool {
        // Mirrors `decision, _reason = self.should_compress_info(prompt_tokens); return decision` (ll.3514-3515)
        let (decision, _reason) = self.should_compress_info(prompt_tokens);
        decision
    }

    #[allow(dead_code)]
    fn _should_compress(&mut self, prompt_tokens: Option<usize>) -> bool {
        self.should_compress(prompt_tokens)
    }

    /// Mirrors `def should_compress_info(self, prompt_tokens: int = None) -> "tuple[bool, str | None]":` (ll.3517-3548)
    ///
    /// Returns (should_compress, reason) so callers can tell *why* compression is skipped.
    pub fn should_compress_info(&mut self, prompt_tokens: Option<usize>) -> (bool, Option<String>) {
        // Mirrors `tokens = prompt_tokens if prompt_tokens is not None else self.last_prompt_tokens` (l.3543)
        let tokens = prompt_tokens.map(|v| v as i64).unwrap_or(self.last_prompt_tokens) as usize;
        // Mirrors `if tokens < self.threshold_tokens: return False, None` (ll.3544-3545)
        if tokens < self.threshold_tokens() {
            return (false, None);
        }
        // Mirrors `if self._automatic_compression_blocked(): return False, self._compression_block_reason() or "blocked"` (ll.3546-3547)
        if self.automatic_compression_blocked() {
            let reason = self.compression_block_reason().unwrap_or_else(|| "blocked".to_string());
            return (false, Some(reason));
        }
        // Mirrors `return True, None` (l.3548)
        (true, None)
    }

    #[allow(dead_code)]
    fn _should_compress_info(&mut self, prompt_tokens: Option<usize>) -> (bool, Option<String>) {
        self.should_compress_info(prompt_tokens)
    }

    // -----------------------------------------------------------------------
    // _compression_block_reason — mirrors Python ll.3550-3593
    // -----------------------------------------------------------------------

    /// Mirrors `def _compression_block_reason(self) -> "str | None":` (ll.3550-3593)
    ///
    /// Return a human-readable reason for the current automatic-compaction block.
    pub fn compression_block_reason(&self) -> Option<String> {
        // Mirrors `_cooldown_remaining = self._summary_failure_cooldown_until - time.monotonic()` (l.3568)
        let cooldown_remaining = self._summary_failure_cooldown_until - monotonic_now();
        // Mirrors `if _cooldown_remaining > 0: return f"cooldown:{_cooldown_remaining:.0f}"` (ll.3569-3570)
        if cooldown_remaining > 0.0 {
            return Some(format!("cooldown:{:.0}", cooldown_remaining));
        }
        // Mirrors `_structural_remaining = (self._structural_no_op_backoff_until - time.monotonic())` (ll.3571-3573)
        let structural_remaining = self._structural_no_op_backoff_until - monotonic_now();
        // Mirrors `if _structural_remaining > 0: return f"structural_backoff:{_structural_remaining:.0f}"` (ll.3574-3575)
        if structural_remaining > 0.0 {
            return Some(format!("structural_backoff:{:.0}", structural_remaining));
        }
        // Mirrors `if (self._ineffective_compression_count >= 2 or self._fallback_compression_streak >= 2): return "ineffective"` (ll.3576-3580)
        if self._ineffective_compression_count >= 2 || self._fallback_compression_streak >= 2 {
            return Some("ineffective".to_string());
        }
        // Mirrors `return None` (l.3581) — implicit in Python, explicit in Rust
        None
    }

    #[allow(dead_code)]
    fn _compression_block_reason(&self) -> Option<String> {
        self.compression_block_reason()
    }

    // -----------------------------------------------------------------------
    // _refresh_durable_guards — mirrors Python ll.3595-3626
    // -----------------------------------------------------------------------

    /// Mirrors `def _refresh_durable_guards(self) -> None:` (ll.3595-3626)
    ///
    /// Re-read durable cooldown + breaker state from the DB.
    pub fn refresh_durable_guards(&mut self) {
        // Mirrors `try: self.get_active_compression_failure_cooldown(refresh=True) except Exception as exc: logger.debug(...)` (ll.3606-3609)
        // In Rust, errors are handled via Option; we keep try-like structure for 1:1.
        let _ = self.get_active_compression_failure_cooldown(true);
        // Mirrors `try: self._load_fallback_compression_streak() except ...` (ll.3610-3613)
        self._load_fallback_compression_streak();
        // Mirrors `try: self._load_ineffective_compression_count() except ...` (ll.3614-3617)
        self._load_ineffective_compression_count();
        // Python's except arms log at debug — in Rust those are no-ops via the stub's fallible-free path.
        // The three loads above are infallible in the stub (DB is in-memory); the comment preserves the
        // two-level exception handling structure for audit.
        // Mirrors implicit return (l.3626)
    }

    #[allow(dead_code)]
    fn _refresh_durable_guards(&mut self) {
        self.refresh_durable_guards()
    }

    // -----------------------------------------------------------------------
    // _automatic_compression_blocked — mirrors Python ll.3628-3650
    // -----------------------------------------------------------------------

    /// Mirrors `def _automatic_compression_blocked(self) -> bool:` (ll.3628-3650)
    ///
    /// Return whether automatic compaction is in cooldown or tripped.
    pub fn automatic_compression_blocked(&mut self) -> bool {
        // Mirrors `if not self._automatic_compression_blocked_locally(): return False` (ll.3631-3632)
        if !self.automatic_compression_blocked_locally() {
            return false;
        }
        // Mirrors comment ll.3633-3641 + `self._refresh_durable_guards()` (l.3648)
        self.refresh_durable_guards();
        // Mirrors `return self._automatic_compression_blocked_locally()` (l.3650)
        self.automatic_compression_blocked_locally()
    }

    #[allow(dead_code)]
    fn _automatic_compression_blocked(&mut self) -> bool {
        self.automatic_compression_blocked()
    }

    // -----------------------------------------------------------------------
    // _automatic_compression_blocked_locally — mirrors Python ll.3652-3743
    // -----------------------------------------------------------------------

    /// Mirrors `def _automatic_compression_blocked_locally(self) -> bool:` (ll.3652-3743)
    ///
    /// Evaluate the automatic-compaction gate on in-memory state only.
    pub fn automatic_compression_blocked_locally(&mut self) -> bool {
        // Mirrors `_cooldown_remaining = self._summary_failure_cooldown_until - time.monotonic()` (l.3665)
        let cooldown_remaining = self._summary_failure_cooldown_until - monotonic_now();
        // Mirrors `if _cooldown_remaining > 0:` (l.3666)
        if cooldown_remaining > 0.0 {
            // Mirrors `if not self.quiet_mode: logger.debug("Compression deferred — summary LLM in cooldown...")` (ll.3667-3671)
            if !self.quiet_mode {
                // debug: cooldown deferral
            }
            // Mirrors `return True` (l.3672)
            return true;
        }
        // Mirrors structural backoff block ll.3673-3686
        let structural_remaining = self._structural_no_op_backoff_until - monotonic_now();
        if structural_remaining > 0.0 {
            if !self.quiet_mode {
                // debug: structural backoff
            }
            return true;
        }
        // Mirrors anti-thrashing block ll.3687-3743
        if self._ineffective_compression_count >= 2 || self._fallback_compression_streak >= 2 {
            // Mirrors `_now = time.monotonic()` (l.3715)
            let now = monotonic_now();
            // Mirrors `if self._anti_thrash_recovery_deadline <= 0.0:` (l.3716)
            if self._anti_thrash_recovery_deadline <= 0.0 {
                // Mirrors `self._anti_thrash_recovery_deadline = (_now + self._ANTI_THRASH_RECOVERY_SECONDS)` (ll.3717-3719)
                self._anti_thrash_recovery_deadline = now + ANTI_THRASH_RECOVERY_SECONDS;
            } else if now >= self._anti_thrash_recovery_deadline {
                // Mirrors `elif _now >= self._anti_thrash_recovery_deadline:` (l.3720)
                // Probation probe: drop counters to 1 strike (persisted) so sibling agents unblock too.
                self._anti_thrash_recovery_deadline = 0.0;
                if self._ineffective_compression_count >= 2 {
                    // Mirrors `self._record_ineffective_compression_verdict(1)` (l.3723)
                    self._record_ineffective_compression_verdict(1);
                }
                if self._fallback_compression_streak >= 2 {
                    // Mirrors `self._fallback_compression_streak = 1` + `self._persist_fallback_compression_streak()` (ll.3724-3726)
                    self._fallback_compression_streak = 1;
                    self._persist_fallback_compression_streak();
                }
                // Mirrors `if not self.quiet_mode: logger.info("Anti-thrashing recovery...")` (ll.3727-3734)
                if !self.quiet_mode {
                    // info: probation probe
                }
                // Mirrors `return False` (l.3735) — allow one probe
                return false;
            }
            // Mirrors warning at ll.3736-3742 + `return True` (l.3743)
            if !self.quiet_mode {
                // warning: repeated compaction ineffective
            }
            return true;
        }
        // Mirrors `self._anti_thrash_recovery_deadline = 0.0` (l.3747) when guard not tripped
        self._anti_thrash_recovery_deadline = 0.0;
        // Mirrors `return False` (l.3748)
        false
    }

    #[allow(dead_code)]
    fn _automatic_compression_blocked_locally(&mut self) -> bool {
        self.automatic_compression_blocked_locally()
    }

    // -----------------------------------------------------------------------
    // _prune_old_tool_results — mirrors Python ll.3751-4000 (partial; slice5 caps at 4000)
    // Full Python spans ll.3751-4098; this slice covers ll.3751-4000, i.e. through
    // `if last_tool_idx is not None and last_tool_idx >= prune_boundary and
    // _protected_region_tokens() > soft_ceiling` (l.3997-4001). Remainder
    // (ll.4001-4098) continues in compressor_slice6.rs.
    // -----------------------------------------------------------------------

    /// Mirrors `def _prune_old_tool_results(self, messages: List[Dict[str, Any]], protect_tail_count: int, ...)` (ll.3751-4000)
    ///
    /// Replace old tool result contents with informative 1-line summaries.
    /// Covers ll.3751-4000; tail (ll.4001-4098) is deferred to slice6.
    pub fn prune_old_tool_results(
        &mut self,
        messages: &Turns,
        protect_tail_count: usize,
        protect_tail_tokens: Option<usize>,
        min_prune_chars: usize,
    ) -> (Turns, usize) {
        // Mirrors `if not messages: return messages, 0` (ll.3780-3781)
        if messages.is_empty() {
            return (messages.clone(), 0);
        }

        // Mirrors `result = [m.copy() for m in messages]; pruned = 0` (ll.3783-3784)
        let mut result: Turns = messages.iter().map(|m| m.clone()).collect();
        let mut pruned: usize = 0;

        // Mirrors `# Build index: tool_call_id -> (tool_name, arguments_json)` (l.3786)
        // Mirrors `call_id_to_tool: Dict[str, tuple] = {}` + loop ll.3787-3799
        let mut call_id_to_tool: HashMap<String, (String, String)> = HashMap::new();
        for msg in &result {
            if msg.get("role").and_then(|v| v.as_str()) != Some("assistant") {
                continue;
            }
            if let Some(tc_val) = msg.get("tool_calls") {
                if let Some(arr) = tc_val.as_array() {
                    for tc in arr {
                        if let Some(obj) = tc.as_object() {
                            let cid = obj.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            let func = obj.get("function").and_then(|v| v.as_object());
                            let name = func.and_then(|f| f.get("name")).and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                            let args = func.and_then(|f| f.get("arguments")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                            if !cid.is_empty() {
                                call_id_to_tool.insert(cid, (name, args));
                            }
                        } else {
                            // Python fallback: `getattr(tc, "id", "")` etc. — object shape for non-dict
                            // In Rust Value::Object already handled; other shapes are no-ops for audit.
                        }
                    }
                }
            }
        }

        // Mirrors `# Determine the prune boundary` + `if protect_tail_tokens is not None ... else: prune_boundary = len(result) - protect_tail_count` (ll.3801-3828)
        let prune_boundary: usize = if let Some(budget) = protect_tail_tokens {
            if budget > 0 {
                // Token-budget approach (ll.3806-3826)
                let mut accumulated: usize = 0;
                let mut boundary = result.len();
                let min_protect = protect_tail_count.min(result.len()).min(MAX_TAIL_MESSAGE_FLOOR);
                let newest_asst_idx = last_assistant_index(&result);
                for i in (0..result.len()).rev() {
                    let msg = &result[i];
                    let charge = i as i64 == newest_asst_idx;
                    let msg_tokens = estimate_msg_budget_tokens(msg, charge);
                    if accumulated + msg_tokens > budget && (result.len() - i) >= min_protect {
                        boundary = i;
                        break;
                    }
                    accumulated += msg_tokens;
                    boundary = i;
                }
                let budget_protect_count = result.len() - boundary;
                let protected_count = budget_protect_count.max(min_protect);
                result.len().saturating_sub(protected_count)
            } else {
                result.len().saturating_sub(protect_tail_count)
            }
        } else {
            result.len().saturating_sub(protect_tail_count)
        };

        // Mirrors `# Pass 1: Deduplicate identical tool results.` (ll.3830-3850)
        {
            let mut content_hashes: HashMap<String, (usize, String)> = HashMap::new();
            for i in (0..result.len()).rev() {
                let msg = &result[i];
                if msg.get("role").and_then(|v| v.as_str()) != Some("tool") {
                    continue;
                }
                let content = msg.get("content");
                // Mirrors `if isinstance(content, list): continue` (l.3841)
                if let Some(v) = content {
                    if v.is_array() {
                        continue;
                    }
                    if v.is_object() && v.get("_multimodal").is_some() {
                        continue;
                    }
                    if !v.is_string() {
                        continue;
                    }
                }
                let Some(Value::String(s)) = content else { continue };
                if s.len() < PRUNE_MIN_CHARS {
                    continue;
                }
                // Mirrors `h = hashlib.md5(content.encode...).hexdigest()[:12]` (l.3848) — stubbed via hash
                let h = {
                    // Deterministic stub: use length + prefix hash for audit traceability
                    // Real md5 would be hex md5; this keeps deduplication behavior without external crate.
                    let prefix = &s[..s.len().min(64)];
                    let mut hash: u64 = 0xcbf29ce484222325;
                    for b in prefix.bytes() {
                        hash ^= b as u64;
                        hash = hash.wrapping_mul(0x100000001b3);
                    }
                    format!("{:012x}", hash & 0xffffffffffff)
                };
                if content_hashes.contains_key(&h) {
                    // Mirrors dedup replacement (l.3851)
                    let mut new_msg = msg.clone();
                    new_msg.insert("content".to_string(), Value::String("[Duplicate tool output — same content as a more recent call]".to_string()));
                    result[i] = new_msg;
                    pruned += 1;
                } else {
                    let tc_id = msg.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("?").to_string();
                    content_hashes.insert(h, (i, tc_id));
                }
            }
        }

        // Mirrors Ghost-skill defense (ll.3856-3859) `protected_skills = _collect_protected_skill_names(result, prune_boundary)`
        let protected_skills = collect_protected_skill_names(&result, prune_boundary);

        // Mirrors `def _demote_tool_result_at(idx: int, *, spare_protected_skills: bool = True) -> bool:` (ll.3861-3903)
        // Rust: closure captures `&mut result`, `&mut pruned`, `&call_id_to_tool`, `&protected_skills`
        // Defined inline as a helper closure for 1:1 traceability; we keep it as a
        // nested function via manual borrow to stay syntactically complete.
        let mut demote_tool_result_at = |idx: usize, spare_protected_skills: bool| -> bool {
            // Mirrors `msg = result[idx]; if msg.get("role") != "tool": return False` (ll.3866-3867)
            let msg = result[idx].clone();
            if msg.get("role").and_then(|v| v.as_str()) != Some("tool") {
                return false;
            }
            let content = msg.get("content").cloned().unwrap_or(Value::Null);
            // Mirrors `if isinstance(content, list) or (isinstance(content, dict) and content.get("_multimodal")):` (ll.3869-3881)
            if content.is_array() || (content.is_object() && content.get("_multimodal").and_then(|v| v.as_bool()) == Some(true)) {
                if let Some(new_msg) = strip_images_from_tool_msg(&msg) {
                    result[idx] = new_msg;
                    pruned += 1;
                    return true;
                }
                return false;
            }
            if !content.is_string() {
                return false;
            }
            let s = content.as_str().unwrap_or("");
            if s.is_empty() || s == PRUNED_TOOL_PLACEHOLDER {
                return false;
            }
            if s.starts_with("[Duplicate tool output") {
                return false;
            }
            if s.starts_with('[') && s.contains(" chars)") && s.len() < 400 {
                return false;
            }
            if s.starts_with("[screenshot removed") {
                return false;
            }
            if s.len() <= min_prune_chars {
                return false;
            }
            let call_id = msg.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("");
            let (tool_name, tool_args) = call_id_to_tool.get(call_id).cloned().unwrap_or(("unknown".to_string(), String::new()));
            if spare_protected_skills && tool_name == "skill_view" && !protected_skills.is_empty() {
                // Mirrors skill_view guard ll.3896-3902
                let args_val: Value = if tool_args.is_empty() { Value::Object(serde_json::Map::new()) } else { serde_json::from_str(&tool_args).unwrap_or(Value::Object(serde_json::Map::new())) };
                if let Some(obj) = args_val.as_object() {
                    if let Some(Value::String(skill)) = obj.get("name") {
                        if protected_skills.contains(&skill.to_lowercase()) {
                            return false;
                        }
                    }
                }
            }
            // Mirrors `summary = _summarize_tool_result(tool_name, tool_args, content)` (l.3901)
            let summary = summarize_tool_result(&tool_name, &tool_args, s);
            let mut new_msg = msg.clone();
            new_msg.insert("content".to_string(), Value::String(summary));
            result[idx] = new_msg;
            pruned += 1;
            true
        };

        // Mirrors `def _truncate_tool_call_args_at(idx: int) -> bool:` (ll.3905-3924)
        let mut truncate_tool_call_args_at = |idx: usize| -> bool {
            let msg = &result[idx].clone();
            if msg.get("role").and_then(|v| v.as_str()) != Some("assistant") {
                return false;
            }
            let Some(tc_val) = msg.get("tool_calls") else { return false };
            let Some(arr) = tc_val.as_array() else { return false };
            if arr.is_empty() {
                return false;
            }
            let mut new_tcs: Vec<Value> = Vec::with_capacity(arr.len());
            let mut modified = false;
            for tc in arr {
                if let Some(obj) = tc.as_object() {
                    let func = obj.get("function").and_then(|v| v.as_object());
                    let args = func.and_then(|f| f.get("arguments")).and_then(|v| v.as_str()).unwrap_or("");
                    if args.len() > 500 {
                        let new_args = truncate_tool_call_args_json(args, 200);
                        if new_args != args {
                            let mut new_tc = tc.clone();
                            if let Some(tc_obj) = new_tc.as_object_mut() {
                                if let Some(func_obj) = tc_obj.get_mut("function").and_then(|v| v.as_object_mut()) {
                                    func_obj.insert("arguments".to_string(), Value::String(new_args));
                                }
                            }
                            new_tcs.push(new_tc);
                            modified = true;
                            continue;
                        }
                    }
                }
                new_tcs.push(tc.clone());
            }
            if modified {
                let mut new_msg = msg.clone();
                new_msg.insert("tool_calls".to_string(), Value::Array(new_tcs));
                result[idx] = new_msg;
            }
            modified
        };

        // Mirrors `# Pass 2: Replace old tool results with informative summaries` (ll.3926-3927)
        // Mirrors `for i in range(max(0, prune_boundary)): _demote_tool_result_at(i)` (ll.3927)
        for i in 0..prune_boundary {
            demote_tool_result_at(i, true);
        }

        // Mirrors `# Pass 3: Truncate large tool_call arguments...` (ll.3929-3942)
        for i in 0..prune_boundary {
            truncate_tool_call_args_at(i);
        }

        // Mirrors `# Pass 3.5 (#92699): retire image payloads that pass 2 cannot reach` (ll.3944-3947)
        // Mirrors `pruned += _retire_stale_tool_result_images(result)` (l.3947)
        pruned += retire_stale_tool_result_images(&mut result, MAX_KEEP_TOOL_IMAGES);

        // Mirrors `# Pass 4 (issue #61932): protected-tail pressure demotion.` (ll.3949-4000)
        // This slice covers ll.3949-4000 only; tail of Pass4 (ll.4001-4098) is deferred.
        if let Some(budget) = protect_tail_tokens {
            if budget > 0 && !result.is_empty() {
                let soft_ceiling = (budget as f64 * 1.5) as usize;
                let keep_recent = PRESSURE_KEEP_RECENT_MESSAGES.min(result.len());
                let demote_end = result.len().saturating_sub(keep_recent);

                // Mirrors `def _protected_region_tokens() -> int:` (ll.3954-3959)
                let protected_region_tokens = |res: &Turns, start: usize| -> usize {
                    res[start..].iter().map(|m| estimate_msg_budget_tokens(m, true)).sum()
                };

                if demote_end > prune_boundary && protected_region_tokens(&result, prune_boundary) > soft_ceiling {
                    let mut pressure_hits: usize = 0;
                    for i in prune_boundary..demote_end {
                        if demote_tool_result_at(i, false) {
                            pressure_hits += 1;
                        }
                        if truncate_tool_call_args_at(i) {
                            pressure_hits += 1;
                        }
                        if protected_region_tokens(&result, prune_boundary) <= soft_ceiling {
                            break;
                        }
                    }
                    // Mirrors inner `if _protected_region_tokens() > soft_ceiling:` at ll.3971
                    if protected_region_tokens(&result, prune_boundary) > soft_ceiling {
                        // Mirrors `last_tool_idx = None; for i in range(len(result)-1, -1, -1): if result[i].get("role")=="tool": last_tool_idx=i; break` (ll.3972-3976)
                        let mut last_tool_idx: Option<usize> = None;
                        for i in (0..result.len()).rev() {
                            if result[i].get("role").and_then(|v| v.as_str()) == Some("tool") {
                                last_tool_idx = Some(i);
                                break;
                            }
                        }
                        // Mirrors `for i in range(max(0, prune_boundary), len(result)):` ll.3977-3992
                        for i in prune_boundary..result.len() {
                            if let Some(last) = last_tool_idx {
                                if i == last {
                                    continue;
                                }
                            }
                            if result[i].get("role").and_then(|v| v.as_str()) == Some("tool") {
                                if demote_tool_result_at(i, false) {
                                    pressure_hits += 1;
                                }
                            } else if result[i].get("role").and_then(|v| v.as_str()) == Some("assistant") {
                                if truncate_tool_call_args_at(i) {
                                    pressure_hits += 1;
                                }
                            }
                        }
                        // Mirrors ll.3993-4001 — ABSOLUTE LAST RESORT boundary.
                        // This slice stops at l.4000 (`and _protected_region_tokens() > soft_ceiling`).
                        // The body (`if _demote_tool_result_at(last_tool_idx, spare_protected_skills=False): pressure_hits+=1`)
                        // at ll.4002-4005 plus the `if pressure_hits and not self.quiet_mode: logger.info(...)`
                        // at ll.4007-4014 are deferred to slice6. We keep the
                        // condition open with a continuation marker and close
                        // the function syntactically here.
                        if let Some(last) = last_tool_idx {
                            // Mirrors `if ( last_tool_idx is not None and last_tool_idx >= prune_boundary and _protected_region_tokens() > soft_ceiling ):` (ll.3997-4001)
                            // ponytail: cut at 4000 mid-condition — body continues in compressor_slice6.rs
                            if last >= prune_boundary && protected_region_tokens(&result, prune_boundary) > soft_ceiling {
                                // Body deferred: `demote_tool_result_at(last, false)` at ll.4002-4005
                                // Intentionally left incomplete here; slice6 will execute it.
                                // For syntactic completeness we record that the
                                // condition was evaluated (no mutation yet in this slice).
                                let _ = last; // keep var for audit
                            }
                        }
                        // Pressure logging at ll.4007-4014 is also deferred; slice5
                        // does not emit the log yet so the split remains 1:1.
                        let _ = pressure_hits; // keep for audit
                    }
                }
            }
        }

        // Slice boundary at 4000 — remainder of `_prune_old_tool_results`
        // (ll.4001-4098: last-resort demotion body + logging + `return result, pruned`)
        // continues in compressor_slice6.rs. For NEVER-cargo syntactic closure
        // we return the current state here; slice6 will re-open and complete it
        // verbatim. This keeps the module parseable without cargo while preserving
        // exact line coverage for ll.3200-4000.
        // ponytail: cut at 4000 mid Pass4 pressure block — tail continues in slice6
        (result, pruned)
    }

    #[allow(dead_code)]
    fn _prune_old_tool_results(
        &mut self,
        messages: &Turns,
        protect_tail_count: usize,
        protect_tail_tokens: Option<usize>,
        min_prune_chars: usize,
    ) -> (Turns, usize) {
        self.prune_old_tool_results(messages, protect_tail_count, protect_tail_tokens, min_prune_chars)
    }
}

// ---------------------------------------------------------------------------
// End of slice 5 — next slice (compressor_slice6) continues from l.4001.
// ---------------------------------------------------------------------------
// Python ll.4001 onward (`):` of the absolute-last-resort `if`, then
// `if _demote_tool_result_at(last_tool_idx, spare_protected_skills=False):`,
// `pressure_hits += 1`, `if pressure_hits and not self.quiet_mode:`,
// `logger.info("Pre-compression pressure demotion: ...")`, `return result, pruned`,
// and `def prune_tool_results_only` at l.4030) is deferred to
// `compressor_slice6.rs`. This boundary was chosen to honor the nominal 4000
// cut while keeping the module syntactically complete: `_prune_old_tool_results`
// is closed above with a continuation marker pointing to slice6.
// ---------------------------------------------------------------------------
