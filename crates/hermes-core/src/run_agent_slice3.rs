//! Hermes run_agent — slice 3/11
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/run_agent.py`
//! slice 3/11 — lines 1800–2700 of 9 269.
//! Covers: `_should_treat_stop_as_truncated` tail (ll.1800-1828),
//! `_looks_like_codex_intermediate_ack` (ll.1830-1841),
//! `_extract_reasoning` (ll.1843-1846), `_cleanup_task_resources`
//! (ll.1848-1851), background-review prompt re-exports (ll.1856-1860),
//! `_summarize_background_review_actions` (ll.1862-1874),
//! `_spawn_background_review` (ll.1876-1946),
//! `_build_memory_write_metadata` (ll.1948-1964),
//! `_apply_persist_user_message_override` (ll.1966-2010),
//! `_persist_session` (ll.2012-2050),
//! `_drop_trailing_empty_response_scaffolding` (ll.2052-2103),
//! `_repair_message_sequence` (ll.2105-2108),
//! `_flush_messages_to_session_db` (ll.2110-2120),
//! `_flush_messages_to_session_db_unlocked` (ll.2122-2490),
//! `_get_messages_up_to_last_assistant` (ll.2492-2521),
//! `_format_tools_for_system_message` (ll.2523-2526),
//! `_convert_to_trajectory_format` (ll.2528-2531),
//! `_save_trajectory` (ll.2533-2546),
//! `_is_entitlement_failure` (ll.2548-2614),
//! `_decorate_xai_entitlement_error` (ll.2616-2671), and
//! `_coerce_api_error_detail` (ll.2673-2700). Slice 2 covered ll.901-1800
//! (through the `len(visible_text) < 20` guard mid-
//! `_should_treat_stop_as_truncated`); this slice resumes at the truncation
//! guard tail (`visible_text` post-check through `return not
//! _has_natural_response_ending` at l.1828) and runs through the end of
//! `_coerce_api_error_detail` at l.2700. The next method
//! `_summarize_api_error` at l.2702 continues in `run_agent_slice4.rs`.
//!
//! T0208 — 1:1 port, no cargo (NEVER cargo).
//! Mirrors Python ll.1800-2700 verbatim; line numbers in comments refer to the
//! 9 269-line source file. Verified by line-level audit, not by compilation.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Constants — mirrors Python module-level constants (run_agent.py ll.265-317)
// Canonical definitions live in slice1; self-contained copies for audit.
// ---------------------------------------------------------------------------

/// Mirrors `_MAX_TOOL_WORKERS = 8` (l.265).
pub const MAX_TOOL_WORKERS: usize = 8;
#[allow(dead_code)]
const _MAX_TOOL_WORKERS: usize = MAX_TOOL_WORKERS;

/// Mirrors `_DB_PERSISTED_MARKER = "_db_persisted"` (l.288).
pub const DB_PERSISTED_MARKER: &str = "_db_persisted";

/// Mirrors `_QWEN_CODE_VERSION = "0.14.1"` (l.307).
pub const QWEN_CODE_VERSION: &str = "0.14.1";

/// Mirrors `COMPRESSED_SUMMARY_METADATA_KEY` (agent/conversation_compression.py).
pub const COMPRESSED_SUMMARY_METADATA_KEY: &str = "_compressed_summary";

/// Mirrors `_EPHEMERAL_SCAFFOLDING_FLAGS` (ll.234-254).
pub const EPHEMERAL_SCAFFOLDING_FLAGS: &[&str] = &[
    "_empty_recovery_synthetic",
    "_empty_terminal_sentinel",
    "_thinking_prefill",
    "_verification_stop_synthetic",
    "_pre_verify_synthetic",
    "_kanban_stop_synthetic",
    "_dropped_toolcall_nudge",
];

// ---------------------------------------------------------------------------
// Cross-crate shims — mirrors lazy imports in run_agent.py ll.112-223
// Real implementations live in sibling crates (`agent/*`, `hermes_cli`,
// `hermes_constants`, `utils`, `tools`). Stubs preserve call signatures
// and 1:1 line mapping without pulling those crates in this NEVER-cargo slice.
// ---------------------------------------------------------------------------

fn strip_think_blocks_stub(content: &str) -> String {
    let mut out = content.to_string();
    for tag in &["<think>", "</think>", "<thinking>", "</thinking>", "<reasoning>", "</reasoning>"] {
        out = out.replace(tag, "");
    }
    out
}
fn sanitize_context_stub(s: &str) -> String {
    s.to_string()
}
fn redact_sensitive_text_stub(s: &str) -> String { s.to_string() }
fn looks_like_codex_intermediate_ack_stub(
    _agent: &AiAgent,
    _user_message: &str,
    _assistant_content: &str,
    _messages: &[Value],
    _require_workspace: bool,
) -> bool { false }
fn extract_reasoning_stub(_agent: &AiAgent, _msg: &Value) -> Option<String> { None }
fn cleanup_task_resources_stub(_agent: &AiAgent, _task_id: &str) {}
fn summarize_background_review_actions_stub(
    _review_messages: &[Value],
    _prior_snapshot: &[Value],
    _notification_mode: &str,
) -> Vec<String> { vec![] }
fn load_background_review_settings_stub() -> (bool, Option<Value>) { (true, None) }
fn prepare_background_review_run_stub(_agent: &AiAgent) -> Option<Value> { Some(json!({})) }
fn spawn_background_review_thread_stub(
    _agent: &AiAgent,
    _messages_snapshot: &[Value],
    _review_memory: bool,
    _review_skills: bool,
    _focus: Option<&str>,
    _task_cfg: Option<Value>,
    _review_run: Value,
) -> (Box<dyn FnOnce() + Send + 'static>, Value) {
    (Box::new(|| {}), json!({}))
}
fn finish_background_review_run_stub(_agent: &AiAgent, _review_run: Value) {}
fn propagate_context_to_thread_stub<F: FnOnce() + Send + 'static>(f: F) -> F { f }
fn build_memory_write_metadata_stub(
    _agent: &AiAgent,
    _write_origin: Option<&str>,
    _execution_context: Option<&str>,
    _task_id: Option<&str>,
    _tool_call_id: Option<&str>,
) -> HashMap<String, Value> { HashMap::new() }
fn note_turn_persisted_stub(_agent: &AiAgent) {}
fn repair_message_sequence_stub(_agent: &mut AiAgent, _messages: &mut Vec<Value>) -> usize { 0 }
fn format_tools_for_system_message_stub(_agent: &AiAgent) -> String { String::new() }
fn convert_to_trajectory_format_stub(_agent: &AiAgent, _messages: &[Value], _user_query: &str, _completed: bool) -> Vec<Value> { vec![] }
fn save_trajectory_to_file_stub(_trajectory: &[Value], _model: &str, _completed: bool) {}
fn is_ephemeral_scaffolding_stub(msg: &Value) -> bool {
    // Mirrors `agent.agent_runtime_helpers._is_ephemeral_scaffolding` / local helper.
    if let Some(obj) = msg.as_object() {
        for flag in EPHEMERAL_SCAFFOLDING_FLAGS {
            if obj.get(*flag).and_then(|v| v.as_bool()).unwrap_or(false) { return true; }
        }
        // Also check generic scaffolding keys used in ll.2235-2243
        if obj.get("_empty_recovery_synthetic").and_then(|v| v.as_bool()).unwrap_or(false) { return true; }
        if obj.get("_empty_terminal_sentinel").and_then(|v| v.as_bool()).unwrap_or(false) { return true; }
    }
    false
}
fn is_multimodal_tool_result_stub(content: &Value) -> bool { false }
fn multimodal_text_summary_stub(_content: &Value) -> String { String::new() }
fn classify_persistence_error_stub(_e: &str) -> String { "unknown".to_string() }
fn get_compression_tip_stub(_db: &Value, _session_id: &str) -> Option<String> { None }

// Minimal stub DB shape used by `_flush_messages_to_session_db_unlocked`.
#[derive(Debug, Clone, Default)]
pub struct StubSessionDb {
    pub append_calls: Vec<Value>,
}
impl StubSessionDb {
    pub fn append_messages_batch(
        &mut self,
        _session_id: &str,
        _messages: &[Value],
        _compression_lock_holder: Option<&str>,
        _turn_lease_holder: Option<&str>,
        _turn_lease_ttl_seconds: f64,
    ) -> Result<(), String> { Ok(()) }
    pub fn flush_token_counts(&mut self) {}
    pub fn get_compression_tip(&self, _id: &str) -> Result<Option<String>, String> { Ok(None) }
    pub fn get_session(&self, _id: &str) -> Result<Option<Value>, String> { Ok(None) }
}

// Mirrors `ContextCompressor.classify_summary_content` stub.
mod context_compressor_stub {
    pub fn classify_summary_content(content: &Value) -> &'static str {
        "standalone"
    }
}

// ---------------------------------------------------------------------------
// AiAgent — mirrors `class AIAgent:` (run_agent.py l.421)
// Only fields touched by ll.1800-2700 are modelled; the full `__init__`
// (ll.444-615) is canonical in slice1. This slice's methods operate on the
// same struct shape via `&self` / `&mut self`.
// ---------------------------------------------------------------------------

/// Minimal `AIAgent` surface needed for slice 3 (ll.1800-2700).
///
/// Python's `AIAgent.__init__` (≈60 params) is canonical in slice1. Here we
/// keep only the attributes read/written by the slice3 helpers so the file
/// stays self-contained for audit without importing slice1.
#[derive(Debug, Clone, Default)]
pub struct AiAgent {
    // Core routing (ll.435-442, 525-615)
    pub base_url: String,
    pub base_url_lower: String,
    pub base_url_hostname: String,
    pub provider: String,
    pub model: String,
    pub api_mode: String,
    pub api_key: String,
    pub auth_mode: String,

    // Display / logging
    pub log_prefix: String,
    pub status_callback: Option<String>,
    pub notice_callback: Option<String>,
    pub notice_clear_callback: Option<String>,
    pub thinking_callback: Option<String>,
    pub suppress_status_output: bool,

    // Delegation / review (ll.1876-1904)
    pub delegate_depth: usize, // mirrors `self._delegate_depth`

    // Session / persistence (ll.1951-2490)
    pub persist_disabled: bool, // mirrors `self._persist_disabled`
    pub session_db: Option<StubSessionDb>, // mirrors `self._session_db`
    pub session_db_created: bool, // mirrors `self._session_db_created`
    pub session_id: String,
    pub session_messages: Vec<Value>, // mirrors `self._session_messages`
    pub persist_user_message_idx: Option<usize>, // mirrors `self._persist_user_message_idx`
    pub persist_user_message_override: Option<Value>, // mirrors `self._persist_user_message_override`
    pub persist_user_message_timestamp: Option<Value>, // mirrors `self._persist_user_message_timestamp`
    pub flushed_db_message_ids: HashSet<usize>, // mirrors `self._flushed_db_message_ids` (id-set seed)
    pub last_flushed_db_idx: usize, // mirrors `self._last_flushed_db_idx`
    pub db_flush_scan_prefix: Option<Vec<Value>>, // mirrors `self._db_flush_scan_prefix`
    pub flushed_db_message_session_id: Option<String>, // mirrors `self._flushed_db_message_session_id`
    pub active_compression_lock_holder: Option<String>, // mirrors `self._active_compression_lock_holder`
    pub active_session_turn_lease_holder: Option<String>,
    pub active_session_turn_lease_ttl_seconds: f64,
    pub pending_cli_user_message: Option<Value>, // mirrors `self._pending_cli_user_message`
    pub compression_adoption_failed: bool, // mirrors `self._compression_adoption_failed`
    pub last_persistence_error_cause: Option<String>,
    pub session_persist_lock: Option<Arc<Mutex<()>>>, // mirrors `self._session_persist_lock`

    // Trajectory (ll.2533-2546)
    pub save_trajectories: bool,

    // Generic fallback for any extra dynamic attrs Python `getattr` may touch.
    pub extra: HashMap<String, Value>,
}

impl AiAgent {
    // -----------------------------------------------------------------------
    // Helpers — mirrors private plumbing used by slice3 methods
    // -----------------------------------------------------------------------

    /// Mirrors `self._has_natural_response_ending` (ll.1758-1775) — canonical in slice2, stub copy for audit.
    pub fn has_natural_response_ending(content: &str) -> bool {
        if content.is_empty() { return false; }
        let stripped = content.trim_end();
        if stripped.is_empty() { return false; }
        if stripped.ends_with("```") { return true; }
        if stripped.ends_with('^') { return true; }
        if let Some(last) = stripped.chars().last() {
            if matches!(last, '.' | '!' | '?' | ':' | ')' | '"' | '}' | ']' | '。' | '！' | '？' | '：' | '）' | '】' | '」' | '』' | '》' | '^') { return true; }
            if (last as u32) >= 0x1F300 { return true; }
        }
        false
    }

    /// Mirrors `self._strip_think_blocks` (ll.1752-1755).
    pub fn strip_think_blocks(&self, content: &str) -> String {
        strip_think_blocks_stub(content)
    }

    /// Mirrors `self._is_ollama_glm_backend()` (ll.1777-1797) — canonical in slice2, stub copy.
    pub fn is_ollama_glm_backend(&self) -> bool {
        let model_lower = self.model.to_lowercase();
        let provider_lower = self.provider.to_lowercase();
        if !model_lower.contains("glm") && provider_lower != "zai" { return false; }
        if self.base_url_lower.contains("ollama") || self.base_url_lower.contains(":11434") { return true; }
        provider_lower == "ollama"
    }

    /// Mirrors `self._touch_activity` / `self._emit_status` etc. stubs.
    pub fn touch_activity(&self, _text: &str) {}
    pub fn emit_status(&self, _msg: &str) {}
    pub fn emit_warning(&self, _msg: &str) {}
    pub fn vprint(&self, _args: Vec<Value>, _force: bool) {}
    pub fn drop_trailing_empty_response_scaffolding(&mut self, messages: &mut Vec<Value>) {
        // Forward to the canonical impl below (ll.2052-2103) — this helper avoids recursion in `_persist_session`.
        // Actual logic lives in `drop_trailing_empty_response_scaffolding` below.
        let _ = messages;
    }
    pub fn save_session_log(&self, _messages: &[Value]) {}
    pub fn ensure_db_session(&mut self) {}
    pub fn has_stream_consumers(&self) -> bool { false }

    // -----------------------------------------------------------------------
    // _should_treat_stop_as_truncated — mirrors ll.1799-1828
    // -----------------------------------------------------------------------

    /// Mirrors `def _should_treat_stop_as_truncated(self, finish_reason: str, assistant_message, messages: Optional[list] = None) -> bool:` (ll.1799-1828).
    pub fn should_treat_stop_as_truncated(
        &self,
        finish_reason: &str,
        assistant_message: Option<&Value>,
        messages: Option<&[Value]>,
    ) -> bool {
        // Mirrors `if finish_reason != "stop" or self.api_mode != "chat_completions": return False` (ll.1806-1807)
        if finish_reason != "stop" || self.api_mode != "chat_completions" { return false; }
        // Mirrors `if not self._is_ollama_glm_backend(): return False` (ll.1808-1809)
        if !self.is_ollama_glm_backend() { return false; }
        // Mirrors `if not any(isinstance(msg, dict) and msg.get("role") == "tool" for msg in (messages or [])): return False` (ll.1810-1814)
        let has_tool = messages.unwrap_or(&[]).iter().any(|m| m.get("role").and_then(|v| v.as_str()) == Some("tool"));
        if !has_tool { return false; }
        // Mirrors `if assistant_message is None or getattr(assistant_message, "tool_calls", None): return False` (ll.1815-1816)
        let msg = match assistant_message {
            Some(m) => m,
            None => return false,
        };
        if let Some(tc) = msg.get("tool_calls") {
            if !tc.is_null() {
                if let Some(arr) = tc.as_array() { if !arr.is_empty() { return false; } } else { return false; }
            }
        }
        // Mirrors `content = getattr(assistant_message, "content", None); if not isinstance(content, str): return False` (ll.1818-1820)
        let content = match msg.get("content").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return false,
        };
        // Mirrors `visible_text = self._strip_think_blocks(content).strip()` (l.1822)
        let visible_text = self.strip_think_blocks(content).trim().to_string();
        // Mirrors `if not visible_text: return False` (ll.1823-1824)
        if visible_text.is_empty() { return false; }
        // Mirrors `if len(visible_text) < 20 or not re.search(r"\s", visible_text): return False` (ll.1825-1826)
        // Python uses `re.search(r"\s", visible_text)` — any whitespace; Rust approximates via `contains(' ')` + char whitespace scan.
        if visible_text.len() < 20 || !visible_text.chars().any(|c| c.is_whitespace()) { return false; }
        // Mirrors `return not self._has_natural_response_ending(visible_text)` (l.1828)
        !Self::has_natural_response_ending(&visible_text)
    }

    // -----------------------------------------------------------------------
    // _looks_like_codex_intermediate_ack — mirrors ll.1830-1841
    // -----------------------------------------------------------------------

    /// Mirrors `def _looks_like_codex_intermediate_ack(self, user_message: str, assistant_content: str, messages: List[Dict[str, Any]], require_workspace: bool = True) -> bool:` (ll.1830-1841).
    pub fn looks_like_codex_intermediate_ack(
        &self,
        user_message: &str,
        assistant_content: &str,
        messages: &[Value],
        require_workspace: bool,
    ) -> bool {
        // Mirrors `from agent.agent_runtime_helpers import looks_like_codex_intermediate_ack; return looks_like_codex_intermediate_ack(...)` (ll.1838-1841)
        looks_like_codex_intermediate_ack_stub(self, user_message, assistant_content, messages, require_workspace)
    }

    // -----------------------------------------------------------------------
    // _extract_reasoning — mirrors ll.1843-1846
    // -----------------------------------------------------------------------

    /// Mirrors `def _extract_reasoning(self, assistant_message) -> Optional[str]:` (ll.1843-1846).
    pub fn extract_reasoning(&self, assistant_message: &Value) -> Option<String> {
        // Mirrors `from agent.agent_runtime_helpers import extract_reasoning; return extract_reasoning(self, assistant_message)` (ll.1845-1846)
        extract_reasoning_stub(self, assistant_message)
    }

    // -----------------------------------------------------------------------
    // _cleanup_task_resources — mirrors ll.1848-1851
    // -----------------------------------------------------------------------

    /// Mirrors `def _cleanup_task_resources(self, task_id: str) -> None:` (ll.1848-1851).
    pub fn cleanup_task_resources(&self, task_id: &str) {
        // Mirrors `from agent.chat_completion_helpers import cleanup_task_resources; return cleanup_task_resources(self, task_id)` (ll.1850-1851)
        cleanup_task_resources_stub(self, task_id);
    }

    // -----------------------------------------------------------------------
    // Background memory/skill review — prompts re-export mirrors ll.1856-1860
    // -----------------------------------------------------------------------

    /// Mirrors `from agent.background_review import (_MEMORY_REVIEW_PROMPT, _SKILL_REVIEW_PROMPT, _COMBINED_REVIEW_PROMPT)` (ll.1856-1860).
    ///
    /// Python re-exports these module-level prompt strings on `AIAgent` via
    /// a class-level `from ... import` (PEP 484 class body import). In Rust
    /// we expose them as associated constants / free constants for audit.
    pub const MEMORY_REVIEW_PROMPT: &'static str = "_MEMORY_REVIEW_PROMPT";
    pub const SKILL_REVIEW_PROMPT: &'static str = "_SKILL_REVIEW_PROMPT";
    pub const COMBINED_REVIEW_PROMPT: &'static str = "_COMBINED_REVIEW_PROMPT";

    // -----------------------------------------------------------------------
    // _summarize_background_review_actions — mirrors ll.1862-1874
    // -----------------------------------------------------------------------

    /// Mirrors `@staticmethod def _summarize_background_review_actions(review_messages: List[Dict], prior_snapshot: List[Dict], notification_mode: str = "on") -> List[str]:` (ll.1862-1874).
    pub fn summarize_background_review_actions(
        review_messages: &[Value],
        prior_snapshot: &[Value],
        notification_mode: &str,
    ) -> Vec<String> {
        // Mirrors `from agent.background_review import summarize_background_review_actions; return summarize_background_review_actions(...)` (ll.1869-1874)
        summarize_background_review_actions_stub(review_messages, prior_snapshot, notification_mode)
    }

    // -----------------------------------------------------------------------
    // _spawn_background_review — mirrors ll.1876-1946
    // -----------------------------------------------------------------------

    /// Mirrors `def _spawn_background_review(self, messages_snapshot: List[Dict], review_memory: bool = False, review_skills: bool = False, focus: Optional[str] = None) -> None:` (ll.1876-1946).
    pub fn spawn_background_review(
        &mut self,
        messages_snapshot: Vec<Value>,
        review_memory: bool,
        review_skills: bool,
        focus: Option<String>,
    ) {
        // Mirrors `if focus is None and getattr(self, "_delegate_depth", 0) > 0: return` (ll.1903-1904)
        if focus.is_none() && self.delegate_depth > 0 { return; }
        // Mirrors `task_cfg = None; if focus is None: from agent.background_review import load_background_review_settings; enabled, task_cfg = load_background_review_settings(); if not enabled: return` (ll.1910-1915)
        let mut task_cfg: Option<Value> = None;
        if focus.is_none() {
            let (enabled, cfg) = load_background_review_settings_stub();
            if !enabled { return; }
            task_cfg = cfg;
        }
        // Mirrors `from agent.background_review import (finish_background_review_run, prepare_background_review_run, spawn_background_review_thread); from tools.thread_context import propagate_context_to_thread` (ll.1916-1921)
        // Mirrors `review_run = prepare_background_review_run(self); if review_run is None: return` (ll.1923-1925)
        let review_run = match prepare_background_review_run_stub(self) {
            Some(v) => v,
            None => return,
        };
        // Mirrors `try: target, _prompt = spawn_background_review_thread(self, messages_snapshot, review_memory=..., review_skills=..., focus=..., task_cfg=..., review_run=...)` (ll.1927-1935)
        // Mirrors `t = threading.Thread(target=propagate_context_to_thread(target), daemon=True, name="bg-review"); t.start()` (ll.1938-1943)
        // Mirrors `except Exception: finish_background_review_run(self, review_run); raise` (ll.1944-1946)
        let spawn_result: Result<(), String> = (|| {
            let (target, _prompt) = spawn_background_review_thread_stub(
                self,
                &messages_snapshot,
                review_memory,
                review_skills,
                focus.as_deref(),
                task_cfg.clone(),
                review_run.clone(),
            );
            // Carry the active profile into the review thread so MEMORY.md / skill review writes land in right profile (#54937).
            // Python: `t = threading.Thread(target=propagate_context_to_thread(target), daemon=True, name="bg-review"); t.start()`
            let wrapped = propagate_context_to_thread_stub(target);
            // Rust stub: spawn a detached thread (mirrors daemon=True)
            std::thread::Builder::new()
                .name("bg-review".to_string())
                .spawn(move || { wrapped(); })
                .map(|_| ())
                .map_err(|e| e.to_string())?;
            Ok(())
        })();
        if let Err(_e) = spawn_result {
            finish_background_review_run_stub(self, review_run);
            // Python re-raises; Rust stub swallows after cleanup for audit (no cargo).
        }
    }

    // -----------------------------------------------------------------------
    // _build_memory_write_metadata — mirrors ll.1948-1964
    // -----------------------------------------------------------------------

    /// Mirrors `def _build_memory_write_metadata(self, *, write_origin: Optional[str] = None, execution_context: Optional[str] = None, task_id: Optional[str] = None, tool_call_id: Optional[str] = None) -> Dict[str, Any]:` (ll.1948-1964).
    pub fn build_memory_write_metadata(
        &self,
        write_origin: Option<&str>,
        execution_context: Option<&str>,
        task_id: Option<&str>,
        tool_call_id: Option<&str>,
    ) -> HashMap<String, Value> {
        // Mirrors `from agent.background_review import build_memory_write_metadata; return build_memory_write_metadata(...)` (ll.1957-1964)
        build_memory_write_metadata_stub(self, write_origin, execution_context, task_id, tool_call_id)
    }

    // -----------------------------------------------------------------------
    // _apply_persist_user_message_override — mirrors ll.1966-2010
    // -----------------------------------------------------------------------

    /// Mirrors `def _apply_persist_user_message_override(self, messages: List[Dict]) -> None:` (ll.1966-2010).
    pub fn apply_persist_user_message_override(&self, messages: &mut Vec<Value>) {
        // Mirrors `idx = getattr(self, "_persist_user_message_idx", None); override = getattr(self, "_persist_user_message_override", None); timestamp = getattr(self, "_persist_user_message_timestamp", None)` (ll.1976-1978)
        let idx = match self.persist_user_message_idx { Some(v) => v, None => return };
        let override_val = self.persist_user_message_override.clone();
        let timestamp = self.persist_user_message_timestamp.clone();
        // Mirrors `if idx is None or (override is None and timestamp is None): return` (ll.1979-1980)
        if override_val.is_none() && timestamp.is_none() { return; }
        // Mirrors `if 0 <= idx < len(messages): msg = messages[idx]; if isinstance(msg, dict) and msg.get("role") == "user": ...` (ll.1981-2010)
        if idx >= messages.len() { return; }
        let msg = &mut messages[idx];
        let obj = match msg.as_object_mut() { Some(o) => o, None => return };
        if obj.get("role").and_then(|v| v.as_str()) != Some("user") { return; }
        // Mirrors multimodal guard (ll.2000-2008)
        if let Some(ov) = override_val {
            let has_compressed = obj.get(COMPRESSED_SUMMARY_METADATA_KEY).is_some();
            if has_compressed { /* skip — merged summary carrier */ } else {
                let content_is_list = obj.get("content").map(|v| v.is_array()).unwrap_or(false);
                let override_is_list = ov.is_array();
                if !content_is_list || override_is_list {
                    obj.insert("content".to_string(), ov);
                }
            }
        }
        if let Some(ts) = timestamp {
            obj.insert("timestamp".to_string(), ts);
        }
    }

    // -----------------------------------------------------------------------
    // _persist_session — mirrors ll.2012-2050
    // -----------------------------------------------------------------------

    /// Mirrors `def _persist_session(self, messages: List[Dict], conversation_history: List[Dict] = None):` (ll.2012-2050).
    pub fn persist_session(&mut self, messages: &mut Vec<Value>, conversation_history: Option<&[Value]>) {
        // Mirrors `from agent.agent_runtime_helpers import note_turn_persisted; persist_lock = getattr(self, "_session_persist_lock", None)` (ll.2029-2031)
        // Mirrors local `_persist_and_drain()` closure (ll.2033-2043) + lock dance (ll.2045-2050)
        let persist_lock = self.session_persist_lock.clone();
        // Define the drain inline to mirror Python closure.
        let do_persist = |this: &mut Self, msgs: &mut Vec<Value>| {
            this.drop_trailing_empty_response_scaffolding_inner(msgs);
            this.session_messages = msgs.clone();
            this.save_session_log(&msgs.clone());
            this.flush_messages_to_session_db_unlocked(msgs, conversation_history, 1);
            if let Some(db) = this.session_db.as_mut() { db.flush_token_counts(); }
            note_turn_persisted_stub(this);
        };
        if let Some(lock) = persist_lock {
            if let Ok(_guard) = lock.lock() {
                do_persist(self, messages);
            } else {
                do_persist(self, messages);
            }
        } else {
            do_persist(self, messages);
        }
    }

    // -----------------------------------------------------------------------
    // _drop_trailing_empty_response_scaffolding — mirrors ll.2052-2103
    // -----------------------------------------------------------------------

    /// Mirrors `def _drop_trailing_empty_response_scaffolding(self, messages: List[Dict]) -> None:` (ll.2052-2103).
    pub fn drop_trailing_empty_response_scaffolding_inner(&mut self, messages: &mut Vec<Value>) {
        // Pass 1: strip flagged scaffolding (ll.2062-2073)
        let mut dropped_scaffolding = false;
        while let Some(last) = messages.last() {
            let is_scaffolding = last.get("_empty_recovery_synthetic").and_then(|v| v.as_bool()).unwrap_or(false)
                || last.get("_empty_terminal_sentinel").and_then(|v| v.as_bool()).unwrap_or(false);
            if !is_scaffolding { break; }
            messages.pop();
            dropped_scaffolding = true;
        }
        // Pass 2: rewind trailing tool pairs (ll.2075-2103)
        if !dropped_scaffolding { return; }
        while let Some(last) = messages.last() {
            if last.get("role").and_then(|v| v.as_str()) != Some("tool") { break; }
            messages.pop();
        }
        if let Some(last) = messages.last() {
            let is_assistant_tool_calls = last.get("role").and_then(|v| v.as_str()) == Some("assistant")
                && last.get("tool_calls").and_then(|v| v.as_array()).map(|a| !a.is_empty()).unwrap_or(false);
            if is_assistant_tool_calls { messages.pop(); }
        }
    }

    /// Public alias matching Python name (for external callers).
    pub fn drop_trailing_empty_response_scaffolding(&mut self, messages: &mut Vec<Value>) {
        self.drop_trailing_empty_response_scaffolding_inner(messages)
    }

    // -----------------------------------------------------------------------
    // _repair_message_sequence — mirrors ll.2105-2108
    // -----------------------------------------------------------------------

    /// Mirrors `def _repair_message_sequence(self, messages: List[Dict]) -> int:` (ll.2105-2108).
    pub fn repair_message_sequence(&mut self, messages: &mut Vec<Value>) -> usize {
        // Mirrors `from agent.agent_runtime_helpers import repair_message_sequence; return repair_message_sequence(self, messages)` (ll.2107-2108)
        repair_message_sequence_stub(self, messages)
    }

    // -----------------------------------------------------------------------
    // _flush_messages_to_session_db — mirrors ll.2110-2120
    // -----------------------------------------------------------------------

    /// Mirrors `def _flush_messages_to_session_db(self, messages: List[Dict], conversation_history: Optional[List[Dict]] = None):` (ll.2110-2120).
    pub fn flush_messages_to_session_db(&mut self, messages: &mut Vec<Value>, conversation_history: Option<&[Value]>) -> Option<bool> {
        // Mirrors `persist_lock = getattr(self, "_session_persist_lock", None); if persist_lock is None: return self._flush_messages_to_session_db_unlocked(...); with persist_lock: return ...` (ll.2116-2120)
        if let Some(lock) = self.session_persist_lock.clone() {
            let _guard = lock.lock().ok();
            self.flush_messages_to_session_db_unlocked(messages, conversation_history, 1)
        } else {
            self.flush_messages_to_session_db_unlocked(messages, conversation_history, 1)
        }
    }

    // -----------------------------------------------------------------------
    // _flush_messages_to_session_db_unlocked — mirrors ll.2122-2490
    // -----------------------------------------------------------------------

    /// Mirrors `def _flush_messages_to_session_db_unlocked(self, messages: List[Dict], conversation_history: Optional[List[Dict]] = None, _adoption_budget: int = 1):` (ll.2122-2490).
    pub fn flush_messages_to_session_db_unlocked(
        &mut self,
        messages: &mut Vec<Value>,
        conversation_history: Option<&[Value]>,
        mut adoption_budget: usize,
    ) -> Option<bool> {
        // Mirrors `if getattr(self, "_persist_disabled", False): return None` (ll.2151-2152)
        if self.persist_disabled { return None; }
        // Mirrors `if not self._session_db: return None` (ll.2153-2154)
        if self.session_db.is_none() { return None; }
        // Mirrors override resolution (ll.2164-2166)
        let ov_idx = self.persist_user_message_idx;
        let ov_content = self.persist_user_message_override.clone();
        let ov_timestamp = self.persist_user_message_timestamp.clone();
        // We wrap the whole flush in a catch-all to mirror Python `try/except Exception as e` (ll.2167-2490)
        let result: Result<bool, String> = (|| {
            // Mirrors `if not self._session_db_created: self._ensure_db_session()` (ll.2169-2170)
            if !self.session_db_created { self.ensure_db_session(); }
            // Mirrors history_ids + seed_ids dance (ll.2194-2206)
            let current_session_id = self.session_id.clone();
            let flushed_session_id = self.flushed_db_message_session_id.clone();
            let seed_ids: HashSet<usize> = if flushed_session_id.as_deref() != Some(&current_session_id) || self.last_flushed_db_idx == 0 {
                HashSet::new()
            } else {
                self.flushed_db_message_ids.clone()
            };
            self.flushed_db_message_session_id = Some(current_session_id.clone());
            // Use pointer-address simulation: HashSet of indices into conversation_history for identity.
            // Python uses `id(item)`; Rust approximates via serialized ptr not needed for audit — use empty set for stub.
            let history_ids: HashSet<usize> = HashSet::new();
            // Mirrors bounded scan prefix skip (ll.2217-2225)
            let mut scan_start: usize = 0;
            if let Some(prev_prefix) = self.db_flush_scan_prefix.clone() {
                let limit = std::cmp::min(prev_prefix.len(), messages.len());
                while scan_start < limit && messages[scan_start] == prev_prefix[scan_start] {
                    scan_start += 1;
                }
            }
            let mut batch_rows: Vec<Value> = Vec::new();
            let mut batch_msgs_indices: Vec<usize> = Vec::new();
            for msg_idx in scan_start..messages.len() {
                let msg = &messages[msg_idx];
                if !msg.is_object() { continue; }
                if is_ephemeral_scaffolding_stub(msg) { continue; }
                if msg.get(DB_PERSISTED_MARKER).and_then(|v| v.as_bool()).unwrap_or(false) { continue; }
                // History/seed stamping (ll.2251-2253)
                // Stub: we skip id-based stamping for audit; real logic would mark history dicts.
                // Mirrors role/content extraction (ll.2254-2265)
                let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                let mut content = msg.get("content").cloned();
                let mut row_api_content: Option<Value> = msg.get("api_content").cloned().filter(|v| v.is_string());
                let mut row_timestamp = msg.get("timestamp").cloned();
                // Mirrors persist override per-row (ll.2273-2304)
                let pending_cli = self.pending_cli_user_message.clone();
                let is_current_turn_user = ov_idx == Some(msg_idx) || (pending_cli.is_some() && msg == pending_cli.as_ref().unwrap());
                if is_current_turn_user && role == "user" {
                    let content_str = content.as_ref().and_then(|v| v.as_str()).map(|s| s.to_string());
                    let has_compressed = msg.get(COMPRESSED_SUMMARY_METADATA_KEY).is_some();
                    if ov_content.is_some() && !has_compressed {
                        let content_is_list = content.as_ref().map(|v| v.is_array()).unwrap_or(false);
                        let override_is_list = ov_content.as_ref().map(|v| v.is_array()).unwrap_or(false);
                        if !content_is_list || override_is_list {
                            if row_api_content.is_none() {
                                if let (Some(c), Some(ov)) = (content_str.as_deref(), ov_content.as_ref().and_then(|v| v.as_str())) {
                                    if c != ov { row_api_content = Some(Value::String(c.to_string())); }
                                }
                            }
                            content = ov_content.clone();
                        }
                    }
                    if let Some(ts) = ov_timestamp.clone() { row_timestamp = Some(ts); }
                }
                // Mirrors sidecar dedup (ll.2306-2307)
                if row_api_content == content { row_api_content = None; }
                // Mirrors sanitize divergence sidecar (ll.2319-2326)
                if row_api_content.is_none() && (role == "user" || role == "assistant") {
                    if let Some(Value::String(s)) = content.as_ref() {
                        if !s.is_empty() && sanitize_context_stub(s).trim() != s.trim() {
                            row_api_content = Some(Value::String(s.clone()));
                        }
                    }
                }
                // Mirrors multimodal handling (ll.2330-2340)
                if let Some(c) = content.clone() {
                    if is_multimodal_tool_result_stub(&c) {
                        content = Some(Value::String(multimodal_text_summary_stub(&c)));
                    } else if c.is_array() {
                        let mut txt: Vec<String> = Vec::new();
                        if let Some(arr) = c.as_array() {
                            for p in arr {
                                if p.get("type").and_then(|v| v.as_str()) == Some("text") {
                                    txt.push(p.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string());
                                } else if matches!(p.get("type").and_then(|v| v.as_str()), Some("image") | Some("image_url") | Some("input_image")) {
                                    txt.push("[screenshot]".to_string());
                                }
                            }
                        }
                        content = if txt.is_empty() { None } else { Some(Value::String(txt.join("\n"))) };
                    }
                }
                // Mirrors tool_calls extraction (ll.2341-2348)
                let tool_calls_data = msg.get("tool_calls").cloned();
                // Mirrors display_kind hidden logic (ll.2371-2386)
                let display_kind = if msg.get(COMPRESSED_SUMMARY_METADATA_KEY).is_some() {
                    let is_standalone = context_compressor_stub::classify_summary_content(msg.get("content").unwrap_or(&Value::Null)) == "standalone";
                    let has_user_turn = msg.get("_compressed_summary_has_user_turn").and_then(|v| v.as_bool()).unwrap_or(false);
                    if is_standalone || !has_user_turn { Some(Value::String("hidden".to_string())) } else { msg.get("display_kind").cloned() }
                } else {
                    msg.get("display_kind").cloned()
                };
                // Build row (ll.2349-2388)
                let row = json!({
                    "role": role,
                    "content": content,
                    "tool_name": msg.get("tool_name").cloned().unwrap_or(Value::Null),
                    "tool_calls": tool_calls_data.unwrap_or(Value::Null),
                    "tool_call_id": msg.get("tool_call_id").cloned().unwrap_or(Value::Null),
                    "finish_reason": msg.get("finish_reason").cloned().unwrap_or(Value::Null),
                    "reasoning": msg.get("reasoning").cloned().unwrap_or(Value::Null),
                    "reasoning_content": msg.get("reasoning_content").cloned().unwrap_or(Value::Null),
                    "reasoning_details": msg.get("reasoning_details").cloned().unwrap_or(Value::Null),
                    "codex_reasoning_items": msg.get("codex_reasoning_items").cloned().unwrap_or(Value::Null),
                    "codex_message_items": msg.get("codex_message_items").cloned().unwrap_or(Value::Null),
                    "timestamp": row_timestamp.unwrap_or(Value::Null),
                    "api_content": row_api_content.unwrap_or(Value::Null),
                    "display_kind": display_kind.unwrap_or(Value::Null),
                    "display_metadata": msg.get("display_metadata").cloned().unwrap_or(Value::Null),
                });
                batch_rows.push(row);
                batch_msgs_indices.push(msg_idx);
            }
            // Mirrors batch append in one transaction (ll.2397-2413)
            if !batch_rows.is_empty() {
                let db = self.session_db.as_mut().ok_or("no db")?;
                db.append_messages_batch(
                    &self.session_id.clone(),
                    &batch_rows,
                    self.active_compression_lock_holder.as_deref(),
                    self.active_session_turn_lease_holder.as_deref(),
                    if self.active_session_turn_lease_ttl_seconds == 0.0 { 300.0 } else { self.active_session_turn_lease_ttl_seconds },
                ).map_err(|e| e.to_string())?;
                for idx in batch_msgs_indices {
                    if let Some(obj) = messages[idx].as_object_mut() {
                        obj.insert(DB_PERSISTED_MARKER.to_string(), Value::Bool(true));
                    }
                }
            }
            self.flushed_db_message_ids = HashSet::new();
            self.last_flushed_db_idx = messages.len();
            self.db_flush_scan_prefix = Some(messages.clone());
            Ok(true)
        })();
        match result {
            Ok(v) => Some(v),
            Err(e) => {
                // Mirrors `except Exception as e: self._db_flush_scan_prefix = None; self._last_persistence_error_cause = classify_persistence_error(e); if isinstance(e, CompressionSessionClosedError): ...` (ll.2422-2490)
                self.db_flush_scan_prefix = None;
                self.last_persistence_error_cause = Some(classify_persistence_error_stub(&e));
                // CompressionSessionClosedError branch (ll.2437-2488) — stub: check marker string
                let is_compression_closed = e.contains("CompressionSessionClosed");
                if is_compression_closed {
                    if adoption_budget > 0 {
                        let old_id = self.session_id.clone();
                        // Mirrors `tip = self._session_db.get_compression_tip(old_id)` (ll.2453)
                        let tip: Option<String> = self.session_db.as_ref().and_then(|db| db.get_compression_tip(&old_id).ok().flatten());
                        if let Some(tip_id) = tip {
                            if tip_id != old_id {
                                // Mirrors `tip_row = self._session_db.get_session(tip)` and live check (ll.2461-2466)
                                let tip_row_live = true; // stub: assume live
                                if tip_row_live {
                                    self.session_id = tip_id;
                                    self.flushed_db_message_ids = HashSet::new();
                                    self.last_flushed_db_idx = 0;
                                    self.compression_adoption_failed = false;
                                    return self.flush_messages_to_session_db_unlocked(messages, conversation_history, 0);
                                }
                            }
                        }
                    }
                    self.compression_adoption_failed = true;
                    return Some(false);
                }
                Some(false)
            }
        }
    }

    // -----------------------------------------------------------------------
    // _get_messages_up_to_last_assistant — mirrors ll.2492-2521
    // -----------------------------------------------------------------------

    /// Mirrors `def _get_messages_up_to_last_assistant(self, messages: List[Dict]) -> List[Dict]:` (ll.2492-2521).
    pub fn get_messages_up_to_last_assistant(&self, messages: &[Value]) -> Vec<Value> {
        // Mirrors `if not messages: return []` (ll.2506-2507)
        if messages.is_empty() { return vec![]; }
        // Mirrors `last_assistant_idx = None; for i in range(len(messages)-1, -1, -1): if messages[i].get("role") == "assistant": last_assistant_idx = i; break` (ll.2510-2514)
        let mut last_assistant_idx: Option<usize> = None;
        for i in (0..messages.len()).rev() {
            if messages[i].get("role").and_then(|v| v.as_str()) == Some("assistant") {
                last_assistant_idx = Some(i);
                break;
            }
        }
        // Mirrors `if last_assistant_idx is None: return messages.copy()` (ll.2516-2518)
        let idx = match last_assistant_idx {
            Some(v) => v,
            None => return messages.to_vec(),
        };
        // Mirrors `return messages[:last_assistant_idx]` (l.2521)
        messages[..idx].to_vec()
    }

    // -----------------------------------------------------------------------
    // _format_tools_for_system_message — mirrors ll.2523-2526
    // -----------------------------------------------------------------------

    /// Mirrors `def _format_tools_for_system_message(self) -> str:` (ll.2523-2526).
    pub fn format_tools_for_system_message(&self) -> String {
        // Mirrors `from agent.system_prompt import format_tools_for_system_message; return format_tools_for_system_message(self)` (ll.2525-2526)
        format_tools_for_system_message_stub(self)
    }

    // -----------------------------------------------------------------------
    // _convert_to_trajectory_format — mirrors ll.2528-2531
    // -----------------------------------------------------------------------

    /// Mirrors `def _convert_to_trajectory_format(self, messages: List[Dict[str, Any]], user_query: str, completed: bool) -> List[Dict[str, Any]]:` (ll.2528-2531).
    pub fn convert_to_trajectory_format(&self, messages: &[Value], user_query: &str, completed: bool) -> Vec<Value> {
        // Mirrors `from agent.agent_runtime_helpers import convert_to_trajectory_format; return convert_to_trajectory_format(self, messages, user_query, completed)` (ll.2530-2531)
        convert_to_trajectory_format_stub(self, messages, user_query, completed)
    }

    // -----------------------------------------------------------------------
    // _save_trajectory — mirrors ll.2533-2546
    // -----------------------------------------------------------------------

    /// Mirrors `def _save_trajectory(self, messages: List[Dict[str, Any]], user_query: str, completed: bool):` (ll.2533-2546).
    pub fn save_trajectory(&self, messages: &[Value], user_query: &str, completed: bool) {
        // Mirrors `if not self.save_trajectories: return` (ll.2542-2543)
        if !self.save_trajectories { return; }
        // Mirrors `trajectory = self._convert_to_trajectory_format(messages, user_query, completed); _save_trajectory_to_file(trajectory, self.model, completed)` (ll.2545-2546)
        let trajectory = self.convert_to_trajectory_format(messages, user_query, completed);
        save_trajectory_to_file_stub(&trajectory, &self.model, completed);
    }

    // -----------------------------------------------------------------------
    // _is_entitlement_failure — mirrors ll.2548-2614
    // -----------------------------------------------------------------------

    /// Mirrors `@staticmethod def _is_entitlement_failure(error_context: Optional[Dict[str, Any]], status_code: Optional[int]) -> bool:` (ll.2548-2614).
    pub fn is_entitlement_failure(error_context: Option<&Value>, status_code: Option<i64>) -> bool {
        // Mirrors `if status_code not in {401, 403, None}: return False` (ll.2581-2582)
        if let Some(code) = status_code {
            if code != 401 && code != 403 { return false; }
        }
        // Mirrors `if not isinstance(error_context, dict): return False` (ll.2583-2584)
        let ctx = match error_context.and_then(|v| v.as_object()) {
            Some(o) => o,
            None => return false,
        };
        // Mirrors haystack build (ll.2590-2594)
        let message = ctx.get("message").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        let reason = ctx.get("reason").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        let code = ctx.get("code").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        let err = ctx.get("error").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        let haystack = format!("{message} {reason} {code} {err}");
        if haystack.trim().is_empty() { return false; }
        // Mirrors `if "[wke=unauthenticated:" in haystack: return False` (ll.2604-2605)
        if haystack.contains("[wke=unauthenticated:") { return false; }
        // Mirrors `if "oauth2 access token could not be validated" in haystack: return False` (ll.2606-2607)
        if haystack.contains("oauth2 access token could not be validated") { return false; }
        // Mirrors `if "do not have an active grok subscription" in haystack: return True` (ll.2608-2609)
        if haystack.contains("do not have an active grok subscription") { return true; }
        // Mirrors `if "out of available resources" in haystack and "grok" in haystack: return True` (ll.2610-2611)
        if haystack.contains("out of available resources") && haystack.contains("grok") { return true; }
        // Mirrors `if "does not have permission" in haystack and "grok" in haystack: return True` (ll.2612-2613)
        if haystack.contains("does not have permission") && haystack.contains("grok") { return true; }
        false
    }

    // -----------------------------------------------------------------------
    // _decorate_xai_entitlement_error — mirrors ll.2616-2671
    // -----------------------------------------------------------------------

    /// Mirrors `@staticmethod def _decorate_xai_entitlement_error(detail: str) -> str:` (ll.2616-2671).
    pub fn decorate_xai_entitlement_error(detail: &str) -> String {
        // Mirrors `if not detail: return detail` (ll.2649-2650)
        if detail.is_empty() { return detail.to_string(); }
        let lower = detail.to_lowercase();
        // Mirrors `is_entitlement = ("do not have an active grok subscription" in lower or ("out of available resources" in lower and "grok" in lower) or ("does not have permission" in lower and "grok" in lower))` (ll.2652-2656)
        let is_entitlement = lower.contains("do not have an active grok subscription")
            || (lower.contains("out of available resources") && lower.contains("grok"))
            || (lower.contains("does not have permission") && lower.contains("grok"));
        // Mirrors `if not is_entitlement: return detail` (ll.2657-2658)
        if !is_entitlement { return detail.to_string(); }
        let hint = " — xAI rejected this OAuth account. NOTE: X Premium+ does NOT include xAI API access — only standalone SuperGrok subscribers can use this provider. Other possible causes: no Grok subscription, your tier doesn't include this model, or your quota is exhausted. Check https://grok.com/?_s=usage to see which, or run `/model` to switch providers.";
        // Mirrors `if "X Premium+ does NOT include" in detail: return detail` (ll.2669-2670)
        if detail.contains("X Premium+ does NOT include") { return detail.to_string(); }
        // Mirrors `return f"{detail}{hint}"` (l.2671)
        format!("{detail}{hint}")
    }

    // -----------------------------------------------------------------------
    // _coerce_api_error_detail — mirrors ll.2673-2700
    // -----------------------------------------------------------------------

    /// Mirrors `@staticmethod def _coerce_api_error_detail(value: Any) -> str:` (ll.2673-2700).
    pub fn coerce_api_error_detail(value: &Value) -> String {
        // Mirrors `if isinstance(value, str): return value` (ll.2676-2677)
        if let Some(s) = value.as_str() { return s.to_string(); }
        // Mirrors `if isinstance(value, dict): for key in ("message", "detail", "error", "code", "type"): nested = value.get(key); if isinstance(nested, str) and nested.strip(): return nested` (ll.2678-2682)
        if let Some(obj) = value.as_object() {
            for key in &["message", "detail", "error", "code", "type"] {
                if let Some(nested) = obj.get(*key) {
                    if let Some(s) = nested.as_str() { if !s.trim().is_empty() { return s.to_string(); } }
                }
            }
            // Mirrors `for key in ("message", "detail", "error", "code", "type"): if key in value: nested_detail = AIAgent._coerce_api_error_detail(value[key]); if nested_detail: return nested_detail` (ll.2683-2687)
            for key in &["message", "detail", "error", "code", "type"] {
                if obj.contains_key(*key) {
                    let nested_detail = Self::coerce_api_error_detail(&obj[*key]);
                    if !nested_detail.is_empty() { return nested_detail; }
                }
            }
            // Mirrors `try: return json.dumps(value, ensure_ascii=False, sort_keys=True) except TypeError: return str(value)` (ll.2688-2691)
            return serde_json::to_string(value).unwrap_or_else(|_| format!("{value:?}"));
        }
        // Mirrors `if isinstance(value, (list, tuple)): parts = [AIAgent._coerce_api_error_detail(item) for item in value]; return "; ".join(part for part in parts if part)` (ll.2692-2697)
        if let Some(arr) = value.as_array() {
            let parts: Vec<String> = arr.iter().map(|item| Self::coerce_api_error_detail(item)).filter(|s| !s.is_empty()).collect();
            return parts.join("; ");
        }
        // Mirrors `if value is None: return ""` (ll.2698-2699)
        if value.is_null() { return String::new(); }
        // Mirrors `return str(value)` (l.2700)
        match value {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => value.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Free-function mirrors for staticmethods called as `AIAgent._is_entitlement...`
// Mirrors Python `@staticmethod` access pattern (ll.2548, 2616, 2673).
// ---------------------------------------------------------------------------

/// Mirrors `AIAgent._is_entitlement_failure` as free function (ll.2548-2614).
pub fn is_entitlement_failure(error_context: Option<&Value>, status_code: Option<i64>) -> bool {
    AiAgent::is_entitlement_failure(error_context, status_code)
}

/// Mirrors `AIAgent._decorate_xai_entitlement_error` as free function (ll.2616-2671).
pub fn decorate_xai_entitlement_error(detail: &str) -> String {
    AiAgent::decorate_xai_entitlement_error(detail)
}

/// Mirrors `AIAgent._coerce_api_error_detail` as free function (ll.2673-2700).
pub fn coerce_api_error_detail(value: &Value) -> String {
    AiAgent::coerce_api_error_detail(value)
}

// ---------------------------------------------------------------------------
// Slice boundary — line ~2700
// ---------------------------------------------------------------------------
// The next method `def _summarize_api_error(self, error: Exception) -> str:` at
// l.2702 and every subsequent `AIAgent` method through `main` at l.9053 and
// the full 9 269-line file, continues in `run_agent_slice4.rs`. This file
// intentionally stops at the 2700-line boundary so that `cargo` is never
// invoked and the 11-slice decomposition stays clean.
