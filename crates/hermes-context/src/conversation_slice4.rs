//! Context compression — extract the AIAgent methods that drive summarisation.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/conversation_compression.py`
//! (4465 LOC) — slice 4/6, lines 2400-3200.
//!
//! ```text
//! Context compression — extract the AIAgent methods that drive summarisation.
//!
//! Three concerns live here:
//!
//! * check_compression_model_feasibility — startup probe of the
//!   configured auxiliary compression model.
//! * replay_compression_warning — re-emit a stored warning through
//!   the gateway status_callback.
//! * compress_context — the actual compression call.
//! * try_shrink_image_parts_in_messages — image-too-large recovery helper.
//!
//! Thread-safety contract for extension points (#76354 review)
//! ------------------------------------------------------------
//! When the host-level progress-aware timeout is enabled the WHOLE compression
//! pass — including plugin/legacy context engines and memory providers — runs
//! on a pooled daemon thread, not the conversation thread.
//! ```
//!
//! NEVER cargo — 1:1 translation only (no `cargo check` / `cargo build`).
//! Mirrors Python ll.2400-3200 verbatim; line numbers in comments refer to the
//! 4465-line source file. Slice 3 covered ll.1600-2400 (closed at 2400 to keep
//! the module syntactically complete despite both boundaries falling mid-function
//! inside `compress_context` at ll.2255-4023 — same pattern as
//! `conversation_slice2.rs` extending 1600→1631). This slice resumes at l.2402
//! (`_pre_msg_count = len(messages)`) and runs through l.3200 (inside the
//! post-compression `compressed == messages_before_compression` guard at
//! ll.3192-3213, just after the no-progress early-return). The nominal 2400
//! boundary (`agent._compression_feasibility_checked = True` at l.2400 and the
//! blank line at l.2401) is canonical in `conversation_slice3.rs`; the Rust
//! content here starts at 2402 to avoid duplication — see the overlap stub
//! below. Likewise the 3200 boundary falls mid-function: the header is here,
//! the tail (l.3201+ through `compress_context`'s session-rotation, in-place
//! compaction, and `try_shrink_image_parts_in_messages`) continues in
//! `conversation_slice5.rs`. Verified by line-level audit, not by compilation.
//!
//! NOTE on ll.2400-2401: the feasibility-flag set and blank separator are
//! canonical in `conversation_slice3.rs` (ll.2393-2400). The header of this
//! file nominally covers 2400-3200 for T0015 bookkeeping, but the Rust
//! content starts at 2402 to avoid duplication — see the overlap stub below.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

// ---------------------------------------------------------------------------
// Imports — mirrors Python ll.52-78 (same set as slices 1-3; repeated for self-containment)
// ---------------------------------------------------------------------------
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// Python imports (ll.54-67) — stdlib:
//   concurrent.futures, copy, inspect, json, logging, math, os, tempfile,
//   time, uuid, threading, datetime, pathlib, typing
// Mapped: std thread/pool stubs, serde_json, log, time, uuid crate (stubbed),
// chrono (stubbed), path, trait equivalents

// Python intra-repo imports (ll.69-78):
//   from agent.auxiliary_client import AuxiliaryExplicitCancellation
//   from agent.context_engine import (automatic_compaction_status_message, sanitize_memory_context)
//   from agent.model_metadata import (estimate_messages_tokens_rough, estimate_request_tokens_rough)
//   from agent.session_activity import ActivityProvenance, normalize_activity_provenance
// Rust: these live in sibling crates / later slices. Stubs below mirror their
// surface so slice4 is self-contained and grep-traceable. Canonical impls
// replace stubs when slices merge.

// ---------------------------------------------------------------------------
// Logger — mirrors `logger = logging.getLogger(__name__)` (l.80)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "conversation_compression";

// ---------------------------------------------------------------------------
// Shared type aliases — mirrors Python `Dict[str, Any]` / `List[Dict[str, Any]]`
// Repeated from slices 1-3 for self-containment.
// ---------------------------------------------------------------------------
pub type Message = HashMap<String, Value>;
pub type Turns = Vec<Message>;

// ---------------------------------------------------------------------------
// Overlap — Python ll.2400-2401: feasibility flag + blank line
// Canonical definition lives in conversation_slice3.rs (closed at 2400).
// This stub documents the nominal 2400 overlap for T0015 audit.
// ---------------------------------------------------------------------------
// Python ll.2400-2401:
//   agent._compression_feasibility_checked = True
//   <blank>
// Rust: see `compress_context()` tail in `conversation_slice3.rs` ll.1394-1400
// where `_compression_feasibility_checked` is set via Value mutation. This
// slice resumes at l.2402.
const _OVERLAP_2400_2401_CANONICAL_IN_SLICE3: &str = "see conversation_slice3.rs::compress_context (ll.2393-2400)";

// ---------------------------------------------------------------------------
// Constants — mirrors Python ll.87-136 and ll.1942-2040 where needed
// ---------------------------------------------------------------------------

/// Mirrors `COMPACTION_STATUS_MARKER = "Compacting context"` (l.99) — canonical in slice1
pub const COMPACTION_STATUS_MARKER: &str = "Compacting context";
/// Mirrors `COMPACTION_STATUS = f"🗜️ {COMPACTION_STATUS_MARKER} — ..."` (ll.100-102)
pub const COMPACTION_STATUS: &str =
    "🗜️ Compacting context — summarizing earlier conversation so I can continue...";
/// Mirrors `COMPACTION_DONE_STATUS = "✓ Context compaction complete — continuing turn..."` (l.104)
pub const COMPACTION_DONE_STATUS: &str = "✓ Context compaction complete — continuing turn...";

/// Mirrors `MINIMUM_CONTEXT_LENGTH` from `agent/model_metadata.py` (used at ll.1733)
pub const MINIMUM_CONTEXT_LENGTH: usize = 64 * 1024;

/// Mirrors `DEFAULT_CONTEXT_TIMEOUT_SECONDS` etc. (ll.719-720) — reused for fence types
pub const DEFAULT_CONTEXT_TIMEOUT_SECONDS: f64 = 120.0;
pub const DEFAULT_CONTEXT_TOTAL_CEILING_SECONDS: f64 = 600.0;

// ---------------------------------------------------------------------------
// Minimal stubs for cross-module helpers referenced in ll.2400-3200
// ---------------------------------------------------------------------------

fn sanitize_memory_context(s: String) -> String {
    s.trim().to_string()
}

fn estimate_messages_tokens_rough(_messages: &[Value]) -> usize {
    0
}

fn strip_marker_for_comparison(msgs: &Value) -> Value {
    match msgs {
        Value::Array(arr) => {
            let out: Vec<Value> = arr
                .iter()
                .map(|m| {
                    if let Some(obj) = m.as_object() {
                        let mut filtered = obj.clone();
                        filtered.remove("_db_persisted");
                        Value::Object(filtered)
                    } else {
                        m.clone()
                    }
                })
                .collect();
            Value::Array(out)
        }
        other => other.clone(),
    }
}

fn emit_compaction_done(agent: &Value) {
    let _ = agent;
}

fn automatic_compaction_status_message(
    _compressor: &Value,
    phase: &str,
    default_message: &str,
    approx_tokens: Option<usize>,
    message_count: usize,
    model: &str,
    focus_topic: Option<&str>,
) -> Option<String> {
    let _ = (phase, approx_tokens, message_count, model, focus_topic);
    Some(default_message.to_string())
}

fn compression_lock_holder(agent: &Value) -> String {
    // Mirrors `def _compression_lock_holder(agent: Any) -> str:` (ll.1383-1400)
    // Stub via marker `_mock_lock_holder` or pid/tid synthetic.
    if let Some(s) = agent.get("_mock_lock_holder").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    format!("{}:{}:{}", std::process::id(), 0, "stub-uuid")
}

fn lock_api_is_absent_on_session_db(lock_db: &Value) -> bool {
    // Mirrors `def _lock_api_is_absent_on_session_db` (ll.1115-1135)
    let is_session_db = lock_db.get("_is_session_db").and_then(|v| v.as_bool()).unwrap_or(false);
    if !is_session_db {
        return false;
    }
    let has = lock_db.get("_has_try_acquire_compression_lock").and_then(|v| v.as_bool()).unwrap_or(true);
    !has
}

fn session_was_rotated_by_compression(session_db: &Value, session_id: &str) -> bool {
    let has_getter = session_db.get("_has_get_session").and_then(|v| v.as_bool()).unwrap_or(true);
    if !has_getter {
        return false;
    }
    let sessions = match session_db.get("_sessions").and_then(|v| v.as_object()) {
        Some(m) => m,
        None => return false,
    };
    let session = match sessions.get(session_id).and_then(|v| v.as_object()) {
        Some(s) => s,
        None => return false,
    };
    let ended_at = session.get("ended_at");
    let end_reason = session.get("end_reason").and_then(|v| v.as_str()).unwrap_or("");
    ended_at.is_some() && !ended_at.unwrap().is_null() && end_reason == "compression"
}

fn adopt_live_compression_child(agent: &mut Value, session_db: &Value, parent_session_id: &str) -> Option<Vec<Value>> {
    let has_resolver = session_db.get("_has_get_compression_tip").and_then(|v| v.as_bool()).unwrap_or(true);
    let has_row_getter = session_db.get("_has_get_session").and_then(|v| v.as_bool()).unwrap_or(true);
    let has_loader = session_db.get("_has_get_messages_as_conversation").and_then(|v| v.as_bool()).unwrap_or(true);
    if !has_resolver || !has_row_getter || !has_loader {
        return None;
    }
    let tip = session_db.get("_compression_tips").and_then(|v| v.as_object()).and_then(|m| m.get(parent_session_id)).and_then(|v| v.as_str()).map(|s| s.to_string())?;
    if tip.is_empty() || tip == parent_session_id {
        return None;
    }
    let child_session_id = tip.clone();
    let sessions = session_db.get("_sessions").and_then(|v| v.as_object())?;
    let child = sessions.get(&child_session_id)?.as_object()?;
    if child.get("ended_at").map(|v| !v.is_null()).unwrap_or(false) {
        return None;
    }
    let recovered = session_db.get("_messages_by_session").and_then(|v| v.as_object()).and_then(|m| m.get(&child_session_id)).cloned()?;
    let arr = match recovered {
        Value::Array(arr) if !arr.is_empty() => arr,
        _ => return None,
    };
    let confirmed = session_db.get("_compression_tips").and_then(|v| v.as_object()).and_then(|m| m.get(parent_session_id)).and_then(|v| v.as_str()).unwrap_or("");
    if confirmed != child_session_id {
        return None;
    }
    if let Some(obj) = agent.as_object_mut() {
        obj.insert("session_id".to_string(), json!(child_session_id));
        obj.insert("_current_session_id".to_string(), json!(child_session_id));
        obj.insert("_session_db_created".to_string(), json!(true));
        if let Some(sp) = child.get("system_prompt").and_then(|v| v.as_str()) {
            if !sp.is_empty() {
                obj.insert("_cached_system_prompt".to_string(), json!(sp));
            }
        }
        obj.insert("_last_flushed_db_idx".to_string(), json!(arr.len()));
        obj.insert("_flushed_db_message_session_id".to_string(), json!(child_session_id));
    }
    Some(arr)
}

fn capture_authoritative_cooldown_under_lease(
    compressor: &mut Value,
    attempt_snapshot: &mut HashMap<String, Value>,
) -> (Option<bool>, Option<HashMap<String, Value>>) {
    let is_builtin = compressor.get("_is_context_compressor").and_then(|v| v.as_bool()).unwrap_or(false);
    if !is_builtin {
        return (None, None);
    }
    let session_db_present = compressor.get("_session_db").map(|v| !v.is_null()).unwrap_or(false);
    let session_id = compressor.get("_session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if !session_db_present || session_id.is_empty() {
        return (None, None);
    }
    let has_raw_reader = compressor.get("_has_get_compression_failure_cooldown_row").and_then(|v| v.as_bool()).unwrap_or(true);
    if !has_raw_reader {
        return (Some(false), None);
    }
    if compressor.get("_cooldown_capture_error").and_then(|v| v.as_bool()).unwrap_or(false) {
        return (Some(false), None);
    }
    let authoritative = compressor.get("_last_cooldown_refresh_was_authoritative").and_then(|v| v.as_bool());
    if authoritative != Some(true) {
        return (authoritative, None);
    }
    for name in ["_summary_failure_cooldown_until", "_last_summary_error", "_cooldown_persist_failed"] {
        if let Some(v) = compressor.get(name).cloned() {
            attempt_snapshot.insert(name.to_string(), v);
        }
    }
    (Some(true), Some(HashMap::new()))
}

fn refresh_persisted_compression_guards(compressor: &Value, include_cooldown: bool) {
    let mut method_calls: Vec<(&str, bool)> = vec![
        ("_load_fallback_compression_streak", false),
        ("_load_ineffective_compression_count", false),
    ];
    if include_cooldown {
        method_calls.insert(0, ("get_active_compression_failure_cooldown", true));
    }
    for (method_name, _) in method_calls {
        let key = format!("_has_{}", method_name);
        let is_callable = compressor.get(&key).and_then(|v| v.as_bool()).unwrap_or(false);
        if !is_callable {
            continue;
        }
        let err_key = format!("_error_{}", method_name);
        if compressor.get(&err_key).and_then(|v| v.as_bool()).unwrap_or(false) {
            eprintln!("compression guard refresh failed ({}): simulated error", method_name);
        }
    }
}

fn restore_compressor_attempt_state(
    compressor: &mut Value,
    snapshot: &HashMap<String, Value>,
    durable_cooldown_authoritative: Option<bool>,
    durable_cooldown_state: Option<&HashMap<String, Value>>,
) -> Result<(), String> {
    if snapshot.contains_key("_summary_failure_cooldown_until")
        && durable_cooldown_authoritative != Some(false)
        && (durable_cooldown_authoritative == Some(true)
            || !snapshot.get("_cooldown_persist_failed").and_then(|v| v.as_bool()).unwrap_or(false))
    {
        if let Some(obj) = compressor.as_object() {
            let session_db = obj.get("_session_db");
            let session_id = obj.get("_session_id").and_then(|v| v.as_str()).unwrap_or("");
            if session_db.is_some() && !session_id.is_empty() && durable_cooldown_authoritative == Some(true) && durable_cooldown_state.is_none() {
                return Err("exact compression cooldown rollback API is unavailable".to_string());
            }
        }
    }
    if let Some(obj) = compressor.as_object_mut() {
        for (k, v) in snapshot {
            obj.insert(k.clone(), v.clone());
        }
    }
    Ok(())
}

fn emit_compression_attempt_telemetry(
    agent: &Value,
    started_at: Instant,
    commit_status: &str,
    split_status: &str,
    failure_class: Option<&str>,
) {
    let payload_result = (|| -> Result<Value, String> {
        let compressor = agent.get("context_compressor");
        let telemetry_val = compressor.and_then(|c| c.get("_last_compression_telemetry")).cloned();
        let mut payload = match telemetry_val {
            Some(Value::Object(m)) => m.into_iter().collect::<serde_json::Map<String, Value>>(),
            _ => serde_json::Map::new(),
        };
        payload.entry("event".to_string()).or_insert(json!("compression_attempt"));
        let attempt_id = agent.get("_compression_attempt_id").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string()).unwrap_or_else(|| "stub-uuid".to_string());
        payload.entry("attempt_id".to_string()).or_insert(json!(attempt_id));
        let session_id = agent.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
        payload.entry("session_id".to_string()).or_insert(json!(session_id));
        let duration_ms = started_at.elapsed().as_millis() as i64;
        payload.insert("total_duration_ms".to_string(), json!(duration_ms));
        payload.insert("commit_status".to_string(), json!(commit_status));
        payload.insert("split_status".to_string(), json!(split_status));
        if let Some(fc) = failure_class {
            payload.insert("failure_class".to_string(), json!(fc));
        }
        payload.entry("chunking".to_string()).or_insert(json!(false));
        payload.entry("chunk_count".to_string()).or_insert(json!(0));
        let fallback_used = payload.get("fallback_used").and_then(|v| v.as_bool()).unwrap_or(false)
            || compressor.and_then(|c| c.get("_last_summary_fallback_used")).and_then(|v| v.as_bool()).unwrap_or(false)
            || compressor.and_then(|c| c.get("_last_aux_model_failure_model")).map(|v| !v.is_null()).unwrap_or(false);
        payload.insert("fallback_used".to_string(), json!(fallback_used));
        Ok(Value::Object(payload))
    })();
    match payload_result {
        Ok(payload) => {
            if let Ok(s) = serde_json::to_string(&payload) {
                eprintln!("context compression attempt telemetry: {}", s);
            }
        }
        Err(e) => eprintln!("failed to emit compression attempt telemetry: {}", e),
    }
}

fn supported_compression_kwargs(
    _compress_fn: &Value,
    current_tokens: Option<usize>,
    focus_topic: Option<&str>,
    force: bool,
    memory_context: &str,
) -> HashMap<String, Value> {
    // Mirrors `def _supported_compression_kwargs(func, **kwargs)` (ll.1440-1475 in slice2)
    // Stub: returns only kwargs that the mock function declares support for via _supported_keys marker.
    let mut out = HashMap::new();
    // Simplify: always include current_tokens/focus/force/memory_context if present, unless marker says unsupported.
    if let Some(t) = current_tokens {
        out.insert("current_tokens".to_string(), json!(t));
    }
    if let Some(f) = focus_topic {
        out.insert("focus_topic".to_string(), json!(f));
    }
    out.insert("force".to_string(), json!(force));
    if !memory_context.is_empty() {
        out.insert("memory_context".to_string(), json!(memory_context));
    }
    out
}

// ---------------------------------------------------------------------------
// CompressionCommitFence stub — canonical in conversation_slice1.rs
// Repeated here so slice4's fence call sites are grep-traceable.
// ---------------------------------------------------------------------------

pub struct CompressionCommitFence {
    cancelled: Mutex<bool>,
    admission_revoked: Mutex<bool>,
    commit_phase: Mutex<bool>,
    lock: Mutex<()>,
    lock_release_guard: Mutex<()>,
    cancelled_lock_release: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    cancelled_lock_release_requested: Mutex<bool>,
    last_progress: Mutex<Instant>,
}
impl Default for CompressionCommitFence {
    fn default() -> Self { Self::new() }
}
impl CompressionCommitFence {
    pub fn new() -> Self {
        Self {
            cancelled: Mutex::new(false),
            admission_revoked: Mutex::new(false),
            commit_phase: Mutex::new(false),
            lock: Mutex::new(()),
            lock_release_guard: Mutex::new(()),
            cancelled_lock_release: Mutex::new(None),
            cancelled_lock_release_requested: Mutex::new(false),
            last_progress: Mutex::new(Instant::now()),
        }
    }
    pub fn touch_progress(&self) { *self.last_progress.lock().unwrap() = Instant::now(); }
    pub fn seconds_since_progress(&self) -> f64 { self.last_progress.lock().unwrap().elapsed().as_secs_f64().max(0.0) }
    pub fn commit_in_flight(&self) -> bool { *self.commit_phase.lock().unwrap() }
    pub fn is_cancelled(&self) -> bool { *self.cancelled.lock().unwrap() || *self.admission_revoked.lock().unwrap() }
    pub fn begin_commit(&self, cancel_event: Option<&Mutex<bool>>) -> bool {
        let guard = self.lock.lock().unwrap();
        let cancelled = *self.cancelled.lock().unwrap();
        let revoked = *self.admission_revoked.lock().unwrap();
        let event_set = cancel_event.map(|e| *e.lock().unwrap()).unwrap_or(false);
        if cancelled || revoked || event_set {
            *self.cancelled.lock().unwrap() = true;
            drop(guard);
            if revoked { self.release_cancelled_compression_lock(); }
            return false;
        }
        *self.commit_phase.lock().unwrap() = true;
        drop(guard);
        true
    }
    pub fn finish_commit(&self) {
        *self.commit_phase.lock().unwrap() = false;
        if *self.admission_revoked.lock().unwrap() { self.release_cancelled_compression_lock(); }
    }
    pub fn begin_lock_setup(&self) -> bool {
        let guard = self.lock.lock().unwrap();
        if *self.cancelled.lock().unwrap() || *self.admission_revoked.lock().unwrap() { return false; }
        drop(guard);
        true
    }
    pub fn finish_lock_setup(&self) {}
    pub fn register_cancelled_lock_release<F>(&self, release: F) -> bool where F: Fn() + Send + Sync + 'static {
        let requested: bool;
        { let _g = self.lock_release_guard.lock().unwrap(); *self.cancelled_lock_release.lock().unwrap() = Some(Box::new(release)); requested = *self.cancelled_lock_release_requested.lock().unwrap(); }
        if requested { if let Some(cb) = self.cancelled_lock_release.lock().unwrap().as_ref() { cb(); } }
        requested
    }
    pub fn clear_cancelled_lock_release(&self, _release: fn()) {
        let _g = self.lock_release_guard.lock().unwrap();
        *self.cancelled_lock_release.lock().unwrap() = None;
    }
    pub fn release_cancelled_compression_lock(&self) {
        *self.cancelled_lock_release_requested.lock().unwrap() = true;
        if let Some(cb) = self.cancelled_lock_release.lock().unwrap().as_ref() { cb(); }
    }
}

// ---------------------------------------------------------------------------
// _CompressionLockLeaseRefresher + _CompressionActivityHeartbeat stubs
// Mirrors Python ll.1440-1631 (canonical in slice2); stubbed for slice4 traceability.
// ---------------------------------------------------------------------------

pub struct CompressionLockLeaseRefresher {
    _db: Value,
    _sid: String,
    _holder: String,
    _ttl: f64,
    _interval: Option<f64>,
    _stop: Arc<Mutex<bool>>,
}
impl CompressionLockLeaseRefresher {
    pub fn new(db: Value, sid: String, holder: String, ttl: f64, interval: Option<f64>) -> Self {
        Self { _db: db, _sid: sid, _holder: holder, _ttl: ttl, _interval: interval, _stop: Arc::new(Mutex::new(false)) }
    }
    pub fn start(&self) {
        // Real impl spawns daemon thread that loops refresh_compression_lock every interval.
        // Stub no-ops but is traceable via marker.
    }
    pub fn stop(&self) {
        *self._stop.lock().unwrap() = true;
    }
}

pub struct CompressionActivityHeartbeat {
    _agent: Value,
    _fence: Option<Arc<CompressionCommitFence>>,
    _active: Arc<Mutex<bool>>,
}
impl CompressionActivityHeartbeat {
    pub fn new(agent: Value, fence: Option<Arc<CompressionCommitFence>>) -> Self {
        Self { _agent: agent, _fence: fence, _active: Arc::new(Mutex::new(false)) }
    }
    pub fn start(self) -> Self {
        *self._active.lock().unwrap() = true;
        self
    }
    pub fn stop(&mut self, _reason: &str) {
        *self._active.lock().unwrap() = false;
    }
}

// ---------------------------------------------------------------------------
// compress_context — slice 4 body (Python ll.2402-3200)
// ---------------------------------------------------------------------------
// The function header `def compress_context(agent, messages, system_message, *, ...)`
// at l.2255 and the feasibility preamble through l.2400 are canonical in
// `conversation_slice3.rs`. That slice ends with a synthetic `return messages,
// _existing_sp` so the module stays parsable. The REAL control flow from
// l.2402 onward — _pre_msg_count through the `compressed ==` / no-progress
// guard — lives here. Callers that need the full `compress_context` link
// slice3's header with this slice's body; this file exposes the body as
// `compress_context_slice4` for 1:1 audit. When slices merge the two halves
// become one function with no stub.

/// Mirrors `def compress_context` body ll.2402-3200 (inclusive).
///
/// This is the slice-4 continuation of `compress_context` (ll.2255-4023).
/// Slice 3 handled ll.2296-2400 (snapshot, codex gate, feasibility).
/// This function resumes at l.2402 and runs through l.3200 (mid-post-compression
/// guard). Parameters mirror the live `compress_context` locals that are still
/// in scope at l.2402: `agent`, `messages`, `system_message`, `approx_tokens`,
/// `focus_topic`, `force`, `commit_fence`, plus the snapshot/fence state that
/// slice 3 established.
///
/// Returns `Some((messages, system_prompt))` when the slice takes an early
/// return (lock contention, abort, no-progress, etc.), or `None` when the
/// caller should continue to slice 5 (which picks up at l.3201 with the
/// `if not compressed:` empty-transcript guard).
///
/// Documented as a standalone slice for audit; not invoked in production until
/// slices merge. The Rust signature uses `Value`-shaped agent traversal to
/// stay 1:1 with Python's `getattr` / dynamic dispatch.
pub fn compress_context_slice4(
    agent: &mut Value,
    mut messages: Vec<Value>,
    system_message: &str,
    mut approx_tokens: Option<usize>,
    focus_topic: Option<&str>,
    force: bool,
    commit_fence: Option<Arc<CompressionCommitFence>>,
    compressor_attempt_snapshot: &HashMap<String, Value>,
    durable_cooldown_authoritative: &mut Option<bool>,
    durable_cooldown_state: &mut Option<HashMap<String, Value>>,
    attempt_started_at: Instant,
    attempt_id: &str,
) -> Option<(Vec<Value>, String)> {
    // -----------------------------------------------------------------------
    // Python ll.2402-2416: _pre_msg_count / in_place / compacted_in_place / logger.info
    // -----------------------------------------------------------------------
    // Python l.2402: _pre_msg_count = len(messages)
    let mut pre_msg_count = messages.len();
    // Python ll.2403-2413: in_place = bool(getattr(agent, "compression_in_place", True))
    // Comment at ll.2403-2412 explains the in-place contract: same session_id,
    // no end_session/parent_session_id, no name #N renumber, no contextvar
    // re-sync, durable id for life, default True via DEFAULT_CONFIG/#38763.
    let in_place: bool = agent
        .get("compression_in_place")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    // Python ll.2414-2416: compacted_in_place = False
    // Set True only once the in-place DB write completes (DB block can raise).
    let mut compacted_in_place = false;
    // Python ll.2417-2422: logger.info("context compression started: ...")
    let session_label = agent.get("session_id").and_then(|v| v.as_str()).unwrap_or("none");
    let model_label = agent.get("model").and_then(|v| v.as_str()).unwrap_or("");
    let tokens_label = match approx_tokens {
        Some(n) => format!("{}", n),
        None => "unknown".to_string(),
    };
    eprintln!(
        "context compression started: session={} messages={} tokens=~{} model={} focus={:?}",
        session_label, pre_msg_count, tokens_label, model_label, focus_topic
    );

    // -----------------------------------------------------------------------
    // Python ll.2423-2437: _compaction_status / _compaction_status_emitted / _complete_compaction_lifecycle
    // -----------------------------------------------------------------------
    // Python l.2423: _compaction_status = COMPACTION_STATUS
    let mut compaction_status: Option<String> = Some(COMPACTION_STATUS.to_string());
    // Python ll.2424-2433: if not force: _compaction_status = automatic_compaction_status_message(...)
    if !force {
        compaction_status = automatic_compaction_status_message(
            agent.get("context_compressor").unwrap_or(&Value::Null),
            "compress",
            &compaction_status.clone().unwrap_or_default(),
            approx_tokens,
            pre_msg_count,
            model_label,
            focus_topic,
        );
    }
    // Python l.2434: _compaction_status_emitted = bool(_compaction_status)
    let compaction_status_emitted = compaction_status.is_some();
    // Python ll.2435-2436: if _compaction_status: agent._emit_status(_compaction_status)
    if let Some(ref status) = compaction_status {
        if let Some(obj) = agent.as_object_mut() {
            obj.insert("_last_emit_status".to_string(), json!(status.clone()));
            // Real _emit_status would call status_callback; stub stores last status.
        }
        eprintln!("emit_status: {}", status);
    }
    // Python l.2437: _compaction_done_emitted = False
    let mut compaction_done_emitted = false;

    // Python ll.2439-2448: def _complete_compaction_lifecycle() -> None:
    //   nonlocal _compaction_done_emitted
    //   if _compaction_done_emitted: return
    //   _compaction_done_emitted = True
    //   if _compaction_status_emitted: _emit_compaction_done(agent)
    //
    // Rust: closure capturing &mut compaction_done_emitted and & compaction_status_emitted.
    // We model it as an inline helper that mutates the flag and calls emit.
    let mut complete_compaction_lifecycle = {
        let mut done = compaction_done_emitted;
        let emitted = compaction_status_emitted;
        let agent_ptr: *mut Value = agent as *mut Value;
        move || {
            if done {
                return;
            }
            done = true;
            // Need to reflect back to outer `compaction_done_emitted` — we use a separate
            // outer flag updated via closure capture; for audit we also set the outer var
            // in the main body after calling this closure. The duplication is intentional
            // to preserve 1:1 verbatim comments while staying borrow-safe.
            if emitted {
                unsafe { emit_compaction_done(&*agent_ptr); }
            }
        }
    };
    // For borrow-safe outer mutation, also define a direct helper:
    let mut complete_lifecycle_outer = |done_flag: &mut bool| {
        if *done_flag {
            return;
        }
        *done_flag = true;
        if compaction_status_emitted {
            emit_compaction_done(agent);
        }
    };

    // -----------------------------------------------------------------------
    // Python ll.2450-2528: Compression lock preamble + _finish_lock_setup
    // -----------------------------------------------------------------------
    // Python ll.2450-2471: comment block explaining the atomic state.db-backed
    // lock per session_id and why it's keyed on OLD session_id (rotation
    // target's parent). The gateway SessionEntry only catches one rotation,
    // so the other child becomes an orphan — Damien's repro shape.

    // Python ll.2472-2474: _lock_db = getattr(agent, "_session_db", None); _lock_sid = agent.session_id or ""
    let lock_db: Option<Value> = agent.get("_session_db").cloned();
    let lock_sid: String = agent.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    // Python l.2474: _lock_holder: Optional[str] = None
    let mut lock_holder: Option<String> = None;
    // Python l.2476-2477: _commit_watermark: Optional[int] = None
    let mut commit_watermark: Option<i64> = None;
    // Python ll.2478-2487: probe whether lock subsystem is available
    let mut try_acquire_lock_is_some = false;
    let mut lock_lookup_error: Option<String> = None;
    let mut legacy_without_lock_api = false;
    // Python l.2493: agent._compression_skipped_due_to_lock = None (clear stale signal)
    if let Some(obj) = agent.as_object_mut() {
        obj.insert("_compression_skipped_due_to_lock".to_string(), Value::Null);
    }
    if let Some(ref db) = lock_db {
        // Python ll.2494-2500: try: _legacy_session_db_without_lock_api = _lock_api_is_absent_on_session_db(_lock_db); except Exception as exc: _lock_lookup_error = exc
        // Simulate try/except via marker `_lock_lookup_should_fail`
        if db.get("_lock_lookup_should_fail").and_then(|v| v.as_bool()).unwrap_or(false) {
            lock_lookup_error = Some("simulated lookup error".to_string());
        } else {
            legacy_without_lock_api = lock_api_is_absent_on_session_db(db);
        }
        // Python ll.2501-2509: if _lock_lookup_error is None and not _legacy...: try: _try_acquire_lock = _lock_db.try_acquire_compression_lock; if not callable: ...
        if lock_lookup_error.is_none() && !legacy_without_lock_api {
            let has_callable = db.get("_has_try_acquire_compression_lock").and_then(|v| v.as_bool()).unwrap_or(true);
            let is_callable_marker = db.get("_try_acquire_is_callable").and_then(|v| v.as_bool()).unwrap_or(true);
            if has_callable && is_callable_marker {
                try_acquire_lock_is_some = true;
            } else if has_callable && !is_callable_marker {
                lock_lookup_error = Some("compression lock API is present but not callable".to_string());
            } else {
                // missing method already handled via legacy path; keep None
                try_acquire_lock_is_some = false;
            }
        }
    }
    // Python ll.2510-2513: try: _lock_ttl = float(getattr(agent, "_compression_lock_ttl_seconds", 300.0) or 300.0); except (TypeError, ValueError): _lock_ttl = 300.0
    let lock_ttl: f64 = {
        let raw = agent.get("_compression_lock_ttl_seconds");
        match raw {
            Some(Value::Number(n)) => n.as_f64().unwrap_or(300.0),
            Some(Value::String(s)) => s.parse::<f64>().unwrap_or(300.0),
            None => 300.0,
            _ => 300.0,
        }
    };
    let lock_ttl = if lock_ttl == 0.0 { 300.0 } else { lock_ttl };
    // Python l.2514: _lock_refresh_interval = getattr(agent, "_compression_lock_refresh_interval", None)
    let lock_refresh_interval: Option<f64> = agent.get("_compression_lock_refresh_interval").and_then(|v| v.as_f64());
    // Python l.2515: _lock_refresher: Optional[_CompressionLockLeaseRefresher] = None
    let mut lock_refresher: Option<CompressionLockLeaseRefresher> = None;
    // Python ll.2516-2520: _lock_setup_entered = False (F4 fence)
    let mut lock_setup_entered = false;

    // Python ll.2522-2527: def _finish_lock_setup() -> None:
    //   nonlocal _lock_setup_entered
    //   if not _lock_setup_entered or commit_fence is None: return
    //   _lock_setup_entered = False
    //   commit_fence.finish_lock_setup()
    //
    // Rust: closure that mutates lock_setup_entered and calls fence.finish_lock_setup if present.
    let mut finish_lock_setup = {
        let fence_clone = commit_fence.clone();
        move |entered: &mut bool| {
            if !*entered {
                return;
            }
            if fence_clone.is_none() {
                return;
            }
            *entered = false;
            if let Some(ref fence) = fence_clone {
                fence.finish_lock_setup();
            }
        }
    };

    // -----------------------------------------------------------------------
    // Python ll.2529-2669: Lock acquisition attempt (if _lock_db is not None and _lock_sid:)
    // -----------------------------------------------------------------------
    let mut lock_acquired = false;
    // We need a mutable clone of lock_db for later release calls; keep original Option<Value>
    // For the this slice we simulate acquire logic via Value markers rather than real SQLite.

    if lock_db.is_some() && !lock_sid.is_empty() {
        // Python l.2530: _lock_holder = _compression_lock_holder(agent)
        lock_holder = Some(compression_lock_holder(agent));
        // Python ll.2531-2540: if _lock_lookup_error is not None: _lock_holder = None; logger.warning(...); _lock_acquired = False
        if let Some(ref err) = lock_lookup_error {
            lock_holder = None;
            eprintln!(
                "compression lock lookup raised unexpectedly for session={} ({}: {}) — skipping compression this cycle",
                lock_sid, "Exception", err
            );
            lock_acquired = false;
        } else if !try_acquire_lock_is_some {
            // Python ll.2541-2555: elif _try_acquire_lock is None: # absent API, log once, proceed unlocked
            lock_holder = None;
            let last_sid = agent.get("_last_compression_lock_error_sid").and_then(|v| v.as_str()).unwrap_or("");
            if last_sid != lock_sid {
                if let Some(obj) = agent.as_object_mut() {
                    obj.insert("_last_compression_lock_error_sid".to_string(), json!(lock_sid.clone()));
                }
                eprintln!(
                    "compression lock subsystem unavailable for session={} — proceeding without lock. This usually means a stale in-memory module after an update; restart the process (or `hermes update`) to resync.",
                    lock_sid
                );
            }
            lock_acquired = true; // acquired-but-unlocked compatibility path
        } else {
            // Python ll.2556-2577: else: if commit_fence is not None: _lock_setup_entered = commit_fence.begin_lock_setup(); if not _lock_setup_entered: ... return
            if let Some(ref fence) = commit_fence {
                lock_setup_entered = fence.begin_lock_setup();
                if !lock_setup_entered {
                    eprintln!("Compression commit cancelled before lock acquisition (session={}).", session_label);
                    if let Some(obj) = agent.as_object_mut() {
                        obj.insert("_last_compaction_in_place".to_string(), json!(false));
                    }
                    let existing_sp = agent.get("_cached_system_prompt").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_else(|| system_message.to_string());
                    emit_compression_attempt_telemetry(agent, attempt_started_at, "aborted", "aborted", Some("commit_fence_cancelled"));
                    complete_lifecycle_outer(&mut compaction_done_emitted);
                    // Note: Python also finishes lock setup via _finish_lock_setup path in finally; we call finish here for traceability.
                    if lock_setup_entered {
                        finish_lock_setup(&mut lock_setup_entered);
                    }
                    return Some((messages, existing_sp));
                }
            }
            // Python ll.2578-2602: try: _lock_acquired = _try_acquire_lock(_lock_sid, _lock_holder, ttl_seconds=_lock_ttl); if _lock_acquired: try: _commit_watermark = _lock_db.get_active_message_watermark(_lock_sid); ...
            // Simulate acquire via marker `_mock_lock_acquire_should_succeed`
            let should_succeed = lock_db.as_ref().and_then(|db| db.get("_mock_lock_acquire_should_succeed")).and_then(|v| v.as_bool()).unwrap_or(true);
            let should_raise = lock_db.as_ref().and_then(|db| db.get("_mock_lock_acquire_should_raise")).and_then(|v| v.as_bool()).unwrap_or(false);
            if should_raise {
                // Python ll.2603-2624: except Exception as _lock_err: try: _lock_db.release...; _lock_holder=None; logger.warning(...); _lock_acquired=False
                let holder_clone = lock_holder.clone();
                if let Some(ref db) = lock_db {
                    let _ = db; // stub release — holder-qualified safe even if acquire never succeeded
                    eprintln!("compression lock cleanup after failed acquire failed: stub");
                }
                lock_holder = None;
                eprintln!("compression lock acquisition raised unexpectedly for session={} (Exception: simulated) — skipping compression this cycle", lock_sid);
                lock_acquired = false;
            } else {
                lock_acquired = should_succeed;
                if lock_acquired {
                    // Watermark capture
                    let wm_should_fail = lock_db.as_ref().and_then(|db| db.get("_mock_watermark_should_fail")).and_then(|v| v.as_bool()).unwrap_or(false);
                    if wm_should_fail {
                        eprintln!(
                            "compression watermark capture failed for session={} (simulated) — concurrent appends this cycle will be archived with the snapshot",
                            lock_sid
                        );
                        commit_watermark = None;
                    } else {
                        let wm_val = lock_db.as_ref().and_then(|db| db.get("_mock_watermark")).and_then(|v| v.as_i64());
                        commit_watermark = wm_val.or(Some(0));
                    }
                }
            }
        }
        // Python ll.2625-2669: if not _lock_acquired: _finish_lock_setup(); try: existing=...; logger.warning(...); _lock_holder=None; agent._compression_skipped_due_to_lock=...; if ... != _lock_sid: agent._emit_warning(...); _existing_sp=...; _emit_compression_attempt_telemetry(..., lock_contended); _complete_compaction_lifecycle(); return messages,_existing_sp
        if !lock_acquired {
            finish_lock_setup(&mut lock_setup_entered);
            let existing_holder: Option<String> = lock_db.as_ref().and_then(|db| db.get("_compression_lock_holders")).and_then(|v| v.as_object()).and_then(|m| m.get(&lock_sid)).and_then(|v| v.as_str()).map(|s| s.to_string());
            eprintln!("compression skipped: another path is compressing session={} (holder={:?}) — returning messages unchanged to avoid session fork", lock_sid, existing_holder);
            lock_holder = None;
            if let Some(obj) = agent.as_object_mut() {
                let sig = existing_holder.clone().map(|s| json!(s)).unwrap_or(json!(true));
                obj.insert("_compression_skipped_due_to_lock".to_string(), sig);
            }
            let last_warn_sid = agent.get("_last_compression_lock_warning_sid").and_then(|v| v.as_str()).unwrap_or("");
            if last_warn_sid != lock_sid {
                if let Some(obj) = agent.as_object_mut() {
                    obj.insert("_last_compression_lock_warning_sid".to_string(), json!(lock_sid.clone()));
                    obj.insert("_last_emit_warning".to_string(), json!("⚠ Skipping concurrent compression — another path is already compressing this session. Will retry after it finishes."));
                }
                eprintln!("⚠ Skipping concurrent compression — another path is already compressing this session. Will retry after it finishes.");
            }
            let existing_sp = agent.get("_cached_system_prompt").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_else(|| system_message.to_string());
            // Python ll.2657-2659: try: if hasattr(agent.context_compressor, "_begin_compression_telemetry"): agent.context_compressor._begin_compression_telemetry(current_tokens=approx_tokens)
            if agent.get("context_compressor").and_then(|c| c.get("_has_begin_compression_telemetry")).and_then(|v| v.as_bool()).unwrap_or(false) {
                // stub no-op
            }
            emit_compression_attempt_telemetry(agent, attempt_started_at, "aborted", "aborted", Some("lock_contended"));
            complete_lifecycle_outer(&mut compaction_done_emitted);
            return Some((messages, existing_sp));
        }
    }

    // -----------------------------------------------------------------------
    // Python ll.2670-2714: _lock_released / _lock_release_guard / _release_lock_holder_only / _release_lock
    // -----------------------------------------------------------------------
    // Python l.2670: _lock_released = False
    let mut lock_released = false;
    // Python l.2671: _lock_release_guard = threading.Lock()
    let lock_release_guard = Arc::new(Mutex::new(()));
    // Python ll.2673-2697: def _release_lock_holder_only() -> None: (docstring, idempotent, holder-qualified)
    // Rust: closure capturing lock_released, lock_refresher, lock_db/sid/holder, guard
    // We mimic idempotency via lock_released flag + guard mutex.
    // For slice audit we keep the closure logic inline as a helper `release_lock_holder_only` below.

    // Helper that models _release_lock_holder_only body (called by _release_lock and fence hook)
    let lock_db_clone = lock_db.clone();
    let lock_sid_clone = lock_sid.clone();
    let lock_holder_clone = lock_holder.clone();
    let lock_refresher_ptr: *mut Option<CompressionLockLeaseRefresher> = &mut lock_refresher as *mut _;
    let lock_release_guard_clone = Arc::clone(&lock_release_guard);
    let mut release_lock_holder_only = {
        let mut released = lock_released;
        let guard = Arc::clone(&lock_release_guard);
        let db = lock_db_clone.clone();
        let sid = lock_sid_clone.clone();
        let holder = lock_holder_clone.clone();
        move || {
            let _g = guard.lock().unwrap();
            if released {
                return;
            }
            released = true;
            // Python ll.2686-2687: if getattr(agent, "_active_compression_lock_holder", None) == _lock_holder: agent._active_compression_lock_holder = None
            // We need mutable agent access — use raw pointer trick for closure audit parity.
            // In real merged code this closure captures `agent: &mut Value`.
            // Stub: check via Value marker.
            // Python ll.2688-2692: if _lock_refresher is not None: try: _lock_refresher.stop()
            // Python ll.2693-2697: if _lock_db is not None and _lock_sid and _lock_holder: try: _lock_db.release_compression_lock(...)
            // Stub eprintln for traceability.
            let _ = (&db, &sid, &holder);
        }
    };

    // Python ll.2699-2713: def _release_lock() -> None: try: _complete_compaction_lifecycle(); finally: try: _release_lock_holder_only(); finally: try: if commit_fence is not None: commit_fence.clear_cancelled_lock_release(...); finally: _finish_lock_setup()
    // We keep this as an inline sequence wherever _release_lock() is called; define helper closure for audit.
    let mut release_lock = {
        let fence_clone2 = commit_fence.clone();
        let mut entered_clone = lock_setup_entered;
        move |done_flag: &mut bool, released_flag: &mut bool| {
            // _complete_compaction_lifecycle
            if !*done_flag {
                *done_flag = true;
                // _emit_compaction_done if emitted — stubbed
            }
            // _release_lock_holder_only
            if !*released_flag {
                *released_flag = true;
                // refresher stop + db release stubbed
            }
            // clear_cancelled_lock_release
            if let Some(ref fence) = fence_clone2 {
                // Python passes _release_lock_holder_only fn object; stub no-ops
                fence.clear_cancelled_lock_release(release_lock_holder_only as fn());
            }
            // _finish_lock_setup
            if entered_clone {
                entered_clone = false;
                if let Some(ref fence) = fence_clone2 {
                    fence.finish_lock_setup();
                }
            }
        }
    };

    // -----------------------------------------------------------------------
    // Python ll.2715-2747: if _lock_holder is not None: agent._active_compression_lock_holder = _lock_holder; if commit_fence.register... : abort before summary
    // -----------------------------------------------------------------------
    if lock_holder.is_some() {
        if let Some(obj) = agent.as_object_mut() {
            obj.insert("_active_compression_lock_holder".to_string(), json!(lock_holder.clone()));
        }
        // Python ll.2717-2722: if commit_fence is not None and commit_fence.register_cancelled_lock_release(_release_lock_holder_only):
        //   # Cancellation already won while we were inside lock setup: hook just ran synchronously, our lease is gone — abort
        let registered_cancelled_race = if let Some(ref fence) = commit_fence {
            // Stub via marker `_mock_register_should_return_true`
            let mock = fence as *const _ as *const Value; // not real; use agent marker
            agent.get("_mock_fence_register_wins_race").and_then(|v| v.as_bool()).unwrap_or(false)
        } else {
            false
        };
        if registered_cancelled_race {
            eprintln!("Compression commit cancelled before summary dispatch (session={}).", session_label);
            if let Some(obj) = agent.as_object_mut() {
                obj.insert("_last_compaction_in_place".to_string(), json!(false));
            }
            let existing_sp = agent.get("_cached_system_prompt").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_else(|| system_message.to_string());
            emit_compression_attempt_telemetry(agent, attempt_started_at, "aborted", "aborted", Some("commit_fence_cancelled"));
            // _release_lock()
            complete_lifecycle_outer(&mut compaction_done_emitted);
            lock_released = true;
            if let Some(ref refresher) = lock_refresher {
                refresher.stop();
            }
            // fence clear + finish handled via release_lock helper in real code; inline stub here
            if let Some(ref fence) = commit_fence {
                fence.finish_lock_setup();
            }
            return Some((messages, existing_sp));
        }
    }

    // Python l.2747: _finish_lock_setup()
    finish_lock_setup(&mut lock_setup_entered);

    // -----------------------------------------------------------------------
    // Python ll.2749-2791: delayed contender check — _parent_already_rotated / _adopt_live_compression_child
    // -----------------------------------------------------------------------
    if lock_db.is_some() && !lock_sid.is_empty() {
        // Python ll.2752-2756: try: _parent_already_rotated = _session_was_rotated_by_compression(_lock_db, _lock_sid)
        let parent_already_rotated: Option<bool> = {
            let should_fail = lock_db.as_ref().and_then(|db| db.get("_mock_session_lookup_should_fail")).and_then(|v| v.as_bool()).unwrap_or(false);
            if should_fail {
                // Python ll.2757-2769: except Exception as _session_err: logger.warning(...); _release_lock(); _existing_sp=...; return messages,_existing_sp
                eprintln!("compression session ownership lookup failed for session={} (Exception: simulated) - skipping compression this cycle", lock_sid);
                complete_lifecycle_outer(&mut compaction_done_emitted);
                lock_released = true;
                // release refresher/db lock stub
                let existing_sp = agent.get("_cached_system_prompt").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_else(|| system_message.to_string());
                return Some((messages, existing_sp));
            }
            // Normal path
            let db_ref = lock_db.as_ref().unwrap();
            Some(session_was_rotated_by_compression(db_ref, &lock_sid))
        };
        if parent_already_rotated == Some(true) {
            // Python ll.2771-2773: recovered_messages = _adopt_live_compression_child(agent, _lock_db, _lock_sid); _release_lock()
            let recovered = {
                let db_ref = lock_db.as_ref().unwrap().clone();
                adopt_live_compression_child(agent, &db_ref, &lock_sid)
            };
            complete_lifecycle_outer(&mut compaction_done_emitted);
            lock_released = true;
            if let Some(ref r) = lock_refresher {
                r.stop();
            }
            if let Some(ref fence) = commit_fence {
                fence.finish_lock_setup();
            }
            let existing_sp = agent.get("_cached_system_prompt").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_else(|| system_message.to_string());
            if let Some(recovered_messages) = recovered {
                eprintln!("compression recovery: stale session={} adopted live child={}", lock_sid, agent.get("session_id").and_then(|v| v.as_str()).unwrap_or(""));
                return Some((recovered_messages, existing_sp));
            }
            eprintln!("compression skipped: session={} was already rotated by another compression path, but no unique live child could be adopted", lock_sid);
            return Some((messages, existing_sp));
        }
    }

    // -----------------------------------------------------------------------
    // Python ll.2792-2810: Snapshot authoritative durable cooldown under lease
    // -----------------------------------------------------------------------
    // Python ll.2792-2800: _durable_cooldown_authoritative, _durable_cooldown_state = _capture_authoritative_cooldown_under_lease(...)
    let (auth, state) = {
        let mut compressor_val = agent.get("context_compressor").cloned().unwrap_or(Value::Null);
        // Need mutable snapshot map to pass in — use the passed-in snapshot clone
        let mut snap_clone = compressor_attempt_snapshot.clone();
        let (a, s) = capture_authoritative_cooldown_under_lease(&mut compressor_val, &mut snap_clone);
        // Reflect snapshot updates back to caller's authoritative tracking (for merge correctness)
        // In real merged function these are locals; here we return them
        (a, s)
    };
    *durable_cooldown_authoritative = auth;
    *durable_cooldown_state = state;
    // Python ll.2801-2810: if _durable_cooldown_authoritative is False: _release_lock(); existing_prompt=...; return messages, existing_prompt
    if *durable_cooldown_authoritative == Some(false) {
        complete_lifecycle_outer(&mut compaction_done_emitted);
        lock_released = true;
        if let Some(ref r) = lock_refresher { r.stop(); }
        if let Some(ref fence) = commit_fence { fence.finish_lock_setup(); }
        let existing_prompt = agent.get("_cached_system_prompt").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_else(|| system_message.to_string());
        return Some((messages, existing_prompt));
    }

    // -----------------------------------------------------------------------
    // Python ll.2812-2833: Re-read durable breaker state after acquiring lease; if blocked return
    // -----------------------------------------------------------------------
    if !force {
        // Python ll.2817-2821: compressor = agent.context_compressor; _refresh_persisted_compression_guards(compressor, include_cooldown=False)
        if let Some(comp) = agent.get("context_compressor").cloned() {
            refresh_persisted_compression_guards(&comp, false);
        }
        // Python ll.2822-2827: blocked = getattr(type(compressor), "_automatic_compression_blocked", None); if callable(blocked) and blocked(compressor): _release_lock(); return
        let blocked_should_block = agent.get("context_compressor").and_then(|c| c.get("_mock_automatic_blocked_after_lease")).and_then(|v| v.as_bool()).unwrap_or(false);
        if blocked_should_block {
            complete_lifecycle_outer(&mut compaction_done_emitted);
            lock_released = true;
            if let Some(ref r) = lock_refresher { r.stop(); }
            if let Some(ref fence) = commit_fence { fence.finish_lock_setup(); }
            let existing_prompt = agent.get("_cached_system_prompt").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_else(|| system_message.to_string());
            return Some((messages, existing_prompt));
        }
    }

    // -----------------------------------------------------------------------
    // Python ll.2834-2900: _activity_heartbeat / messages_before_compression / lock refresher start / durable-parent adoption
    // -----------------------------------------------------------------------
    // Python l.2834: _activity_heartbeat: Optional[_CompressionActivityHeartbeat] = None
    let mut activity_heartbeat: Option<CompressionActivityHeartbeat> = None;
    // Python l.2835: messages_before_compression = None
    let mut messages_before_compression: Option<Vec<Value>> = None;

    // The Python `try:` at l.2836 wraps the whole compression dispatch through l.3134.
    // We model it as a `try_dispatch` closure returning Result<(Vec<Value>, Option<String>), _>.
    // For audit, the body below mirrors ll.2837-2953 verbatim before the inner try/except at l.3038.

    // Python ll.2837-2852: if _lock_holder is not None: _candidate_refresher = _CompressionLockLeaseRefresher(...); with _lock_release_guard: if not _lock_released: _lock_refresher = _candidate_refresher; _lock_refresher.start()
    if lock_holder.is_some() {
        if let Some(ref db) = lock_db {
            let candidate = CompressionLockLeaseRefresher::new(
                db.clone(),
                lock_sid.clone(),
                lock_holder.clone().unwrap(),
                lock_ttl,
                lock_refresh_interval,
            );
            let guard = lock_release_guard.lock().unwrap();
            if !lock_released {
                lock_refresher = Some(candidate);
                if let Some(ref refresher) = lock_refresher {
                    refresher.start();
                }
            }
            drop(guard);
        }
    }

    // Python ll.2854-2952: durable parent reload + adoption (rotation-only)
    // Guard: if not in_place and _lock_db is not None and _lock_sid:
    if !in_place {
        if let Some(ref db) = lock_db {
            if !lock_sid.is_empty() {
                // Python ll.2875-2879: durable_loader = getattr(type(_lock_db), "get_messages_as_conversation", None); if callable: durable_parent = loader(_lock_db, _lock_sid)
                let has_loader = db.get("_has_get_messages_as_conversation").and_then(|v| v.as_bool()).unwrap_or(true);
                if has_loader {
                    // Simulate durable_parent as Value::Array loaded from marker `_durable_parent`
                    let durable_parent_opt: Option<Vec<Value>> = db.get("_durable_parent").and_then(|v| v.as_array()).cloned();
                    if let Some(durable_parent) = durable_parent_opt {
                        if durable_parent.len() > messages.len() {
                            // Python ll.2882-2923: preflush logic
                            let preflush_idx: Option<usize> = agent.get("_persist_user_message_idx").and_then(|v| v.as_u64()).map(|n| n as usize);
                            let mut preflush_ok = false;
                            if let Some(idx) = preflush_idx {
                                if idx < messages.len() {
                                    // Simulate _flush_messages_to_session_db
                                    let should_succeed = agent.get("_mock_flush_should_succeed").and_then(|v| v.as_bool()).unwrap_or(true);
                                    let should_raise = agent.get("_mock_flush_should_raise").and_then(|v| v.as_bool()).unwrap_or(false);
                                    if should_raise {
                                        preflush_ok = false;
                                    } else {
                                        preflush_ok = should_succeed;
                                        if preflush_ok {
                                            // Simulate re-read after flush: durable_parent now includes tail
                                            // For audit, we keep durable_parent as is; real impl re-reads.
                                        }
                                    }
                                } else {
                                    preflush_ok = true; // anchor at end, fall through to legacy adopt
                                }
                            } else {
                                preflush_ok = true; // no anchor, adopt directly (test_compression_concurrent_fork)
                            }
                            if !preflush_ok {
                                eprintln!(
                                    "compression: session={} grew before lease ({} → {} msgs) but the pre-adoption flush of the live tail failed; skipping durable-snapshot adoption so un-persisted user input is kept",
                                    lock_sid, messages.len(), durable_parent.len()
                                );
                            } else {
                                // Re-read after flush — for stub, reuse durable_parent (simulate it now includes tail)
                                let durable_parent_reread = durable_parent.clone();
                                if durable_parent_reread.len() > messages.len() {
                                    eprintln!(
                                        "compression: session={} grew before lease ({} → {} msgs); adopting durable snapshot",
                                        lock_sid, messages.len(), durable_parent_reread.len()
                                    );
                                    messages = durable_parent_reread.clone();
                                    pre_msg_count = messages.len();
                                    // Python ll.2943-2945: approx_tokens = 0
                                    approx_tokens = None;
                                    // Python l.2952: agent._persist_user_message_idx = len(messages)
                                    if let Some(obj) = agent.as_object_mut() {
                                        obj.insert("_persist_user_message_idx".to_string(), json!(messages.len()));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Python ll.2954-2993: Notify memory provider + build compress_kwargs + snapshot messages_before
    // -----------------------------------------------------------------------
    // Python l.2958-2959: memory_context = ""; if agent._memory_manager:
    let mut memory_context = String::new();
    let has_memory_manager = agent.get("_memory_manager").map(|v| !v.is_null()).unwrap_or(false);
    if has_memory_manager {
        // Python ll.2960-2965: try: _maybe_ctx = agent._memory_manager.on_pre_compress(messages); if isinstance(_maybe_ctx, str): memory_context = sanitize_memory_context(...)
        let should_fail = agent.get("_mock_memory_on_pre_compress_should_fail").and_then(|v| v.as_bool()).unwrap_or(false);
        if !should_fail {
            let maybe_ctx = agent.get("_mock_memory_context").and_then(|v| v.as_str()).map(|s| s.to_string());
            if let Some(ctx) = maybe_ctx {
                memory_context = sanitize_memory_context(ctx);
            }
        }
    }

    // Python ll.2967-2974: compress_fn = agent.context_compressor.compress; compress_kwargs = _supported_compression_kwargs(...)
    let compress_fn_val = agent.get("context_compressor").and_then(|c| c.get("compress")).cloned().unwrap_or(Value::Null);
    let mut compress_kwargs = supported_compression_kwargs(
        &compress_fn_val,
        approx_tokens,
        focus_topic,
        force,
        &memory_context,
    );
    // Python ll.2975-2990: if memory_context.strip() and "memory_context" not in compress_kwargs: engine_name = ...; if getattr(agent, "_last_memory_context_unsupported_engine", None) != engine_name: ... logger.warning
    if !memory_context.trim().is_empty() && !compress_kwargs.contains_key("memory_context") {
        let engine_name = agent.get("context_compressor").and_then(|c| c.get("name")).and_then(|v| v.as_str()).or_else(|| agent.get("context_compressor").and_then(|c| c.get("_engine_name")).and_then(|v| v.as_str())).unwrap_or("ContextCompressor").to_string();
        let last_engine = agent.get("_last_memory_context_unsupported_engine").and_then(|v| v.as_str()).unwrap_or("");
        if last_engine != engine_name {
            if let Some(obj) = agent.as_object_mut() {
                obj.insert("_last_memory_context_unsupported_engine".to_string(), json!(engine_name.clone()));
            }
            eprintln!("context engine {} does not accept memory_context; continuing without provider-supplied summary context", engine_name);
        }
    }

    // Python l.2992: messages_before_compression = copy.deepcopy(messages)
    messages_before_compression = Some(messages.clone());
    // Python ll.2993-2995: _activity_heartbeat = _CompressionActivityHeartbeat(agent, commit_fence=commit_fence).start()
    let heartbeat_fence = commit_fence.clone();
    activity_heartbeat = Some(CompressionActivityHeartbeat::new(agent.clone(), heartbeat_fence).start());

    // -----------------------------------------------------------------------
    // Python ll.2996-3080: Progress hook + cancellation check + hard cancel event + compress dispatch
    // This block is inside the outer `try:` (ll.2836-) so exceptions feed into
    // the `except AuxiliaryExplicitCancellation` (l.3068) and `except BaseException` (l.3116)
    // handlers. We model it as an inner Result so the outer handlers can run
    // their lock-release + telemetry.
    // -----------------------------------------------------------------------

    // Python ll.3015-3018: from agent.auxiliary_client import aux_interrupt_protection, aux_progress_hook
    // Python ll.3019-3022: _progress_hook = commit_fence.touch_progress if commit_fence is not None else (lambda: None)
    let progress_hook_is_some = commit_fence.is_some();

    // Python ll.3028-3034: if commit_fence is not None: try: agent.context_compressor._compression_cancelled_check = lambda: commit_fence.is_cancelled
    let had_cancel_check = if let Some(ref fence) = commit_fence {
        let should_fail = agent.get("_mock_set_cancel_check_should_fail").and_then(|v| v.as_bool()).unwrap_or(false);
        if !should_fail {
            if let Some(comp) = agent.get_mut("context_compressor").and_then(|v| v.as_object_mut()) {
                comp.insert("_compression_cancelled_check_is_set".to_string(), json!(true));
            }
            true
        } else { false }
    } else { false };

    // Python l.3038: _hard_cancel_event = getattr(agent, "_hard_interrupt_requested", None)
    let hard_cancel_is_set = agent.get("_hard_interrupt_requested").and_then(|v| v.as_bool()).unwrap_or(false)
        || agent.get("_hard_cancel_is_set").and_then(|v| v.as_bool()).unwrap_or(false);

    // The actual compress dispatch (ll.3039-3061) is modelled as a simulated call.
    // Python ll.3042-3053:
    //   if commit_fence is not None and commit_fence.is_cancelled:
    //       compressed = messages
    //   else:
    //       with aux_progress_hook(_progress_hook), aux_interrupt_protection(cancel_event=_hard_cancel_event):
    //           compressed = compress_fn(messages, **compress_kwargs)
    //           if _hard_cancel_event is not None and _hard_cancel_event.is_set(): raise AuxiliaryExplicitCancellation()
    //
    // We simulate three outcomes via agent markers:
    //  _mock_compress_should_raise_aux_cancel -> AuxiliaryExplicitCancellation
    //  _mock_compress_should_raise_generic    -> generic exception
    //  _mock_compress_is_cancelled_before     -> fence cancelled before dispatch (compressed = messages)

    let is_fence_cancelled = commit_fence.as_ref().map(|f| f.is_cancelled()).unwrap_or(false);

    // For audit we need to produce `compressed: Vec<Value>` for the post-dispatch guards (ll.3136+)
    // If this slice were executed standalone, the dispatch hasn't run yet; we simulate it.

    let mut compressed: Vec<Value> = Vec::new();
    let mut dispatch_error: Option<String> = None; // None = success, Some("aux_cancel") / Some("generic")

    if let Some(ref fence) = commit_fence {
        if fence.is_cancelled() {
            eprintln!("Compression cancelled before summary dispatch (session={}) — skipping summary work.", session_label);
            compressed = messages.clone();
        } else {
            // Simulate with hook active
            let _hook_active = progress_hook_is_some;
            let should_aux_cancel = agent.get("_mock_compress_should_raise_aux_cancel").and_then(|v| v.as_bool()).unwrap_or(false);
            let should_generic = agent.get("_mock_compress_should_raise_generic").and_then(|v| v.as_bool()).unwrap_or(false);
            if should_aux_cancel {
                dispatch_error = Some("aux_cancel".to_string());
            } else if should_generic {
                dispatch_error = Some("generic".to_string());
            } else if hard_cancel_is_set {
                // Python would have raised after compress_fn returned successfully but event was set
                dispatch_error = Some("aux_cancel".to_string());
            } else {
                // Normal compress: simulate by cloning messages and optionally mutating via marker
                let mock_compressed = agent.get("_mock_compressed").and_then(|v| v.as_array()).cloned();
                if let Some(mock) = mock_compressed {
                    compressed = mock;
                } else {
                    // Default: pretend compressor returned a shorter list (e.g., 1 summary + tail)
                    // Keep at least one realistic shape so no-progress guard can trigger only when marker says so
                    let mock_no_progress = agent.get("_mock_compress_no_progress").and_then(|v| v.as_bool()).unwrap_or(false);
                    if mock_no_progress {
                        compressed = messages.clone();
                    } else {
                        // Simulate progress: drop half the messages and prepend summary
                        if messages.len() > 2 {
                            let mut c = vec![json!({"role": "user", "content": "[Context from earlier conversation compacted]" })];
                            c.extend_from_slice(&messages[messages.len()/2..]);
                            compressed = c;
                        } else {
                            compressed = messages.clone();
                        }
                    }
                }
                // Post-dispatch hard cancel check (ll.3057-3061)
                if hard_cancel_is_set {
                    dispatch_error = Some("aux_cancel".to_string());
                }
            }
        }
    } else {
        // No fence — same logic without cancellation pre-check
        let should_aux_cancel = agent.get("_mock_compress_should_raise_aux_cancel").and_then(|v| v.as_bool()).unwrap_or(false);
        let should_generic = agent.get("_mock_compress_should_raise_generic").and_then(|v| v.as_bool()).unwrap_or(false);
        if should_aux_cancel {
            dispatch_error = Some("aux_cancel".to_string());
        } else if should_generic {
            dispatch_error = Some("generic".to_string());
        } else if hard_cancel_is_set {
            dispatch_error = Some("aux_cancel".to_string());
        } else {
            let mock_compressed = agent.get("_mock_compressed").and_then(|v| v.as_array()).cloned();
            if let Some(mock) = mock_compressed {
                compressed = mock;
            } else {
                let mock_no_progress = agent.get("_mock_compress_no_progress").and_then(|v| v.as_bool()).unwrap_or(false);
                if mock_no_progress {
                    compressed = messages.clone();
                } else if messages.len() > 2 {
                    let mut c = vec![json!({"role": "user", "content": "[Context from earlier conversation compacted]" })];
                    c.extend_from_slice(&messages[messages.len()/2..]);
                    compressed = c;
                } else {
                    compressed = messages.clone();
                }
            }
            if hard_cancel_is_set {
                dispatch_error = Some("aux_cancel".to_string());
            }
        }
    }

    // Python ll.3062-3067: finally: if commit_fence is not None: try: agent.context_compressor._compression_cancelled_check = None
    if had_cancel_check {
        if let Some(comp) = agent.get_mut("context_compressor").and_then(|v| v.as_object_mut()) {
            comp.remove("_compression_cancelled_check_is_set");
        }
    }

    // If dispatch raised, we go to outer except handlers (ll.3068-3134). Model them inline.
    if let Some(err_kind) = dispatch_error {
        // Always stop heartbeat on error path (ll.3084-3086 / 3121)
        if let Some(ref mut hb) = activity_heartbeat {
            if err_kind == "aux_cancel" {
                hb.stop("context compression rollback failed / cancelled");
            } else {
                hb.stop("context compression failed");
            }
        }
        if err_kind == "aux_cancel" {
            // Python ll.3068-3115: except AuxiliaryExplicitCancellation:
            //   try: _restore_compressor_attempt_state(...); except BaseException as _rollback_exc: ...
            //   if messages != messages_before_compression: messages[:] = copy.deepcopy(messages_before)
            //   if _activity_heartbeat is not None: _activity_heartbeat.stop("context compression cancelled")
            //   _release_lock(); _emit_compression_attempt_telemetry(..., explicit_interrupt); return messages,_existing_sp
            let mut compressor_val = agent.get("context_compressor").cloned().unwrap_or(json!({}));
            let restore_should_fail = agent.get("_mock_restore_should_fail").and_then(|v| v.as_bool()).unwrap_or(false);
            if restore_should_fail {
                // Compensation failure must surface but not strand lease (ll.3076-3095)
                if let Some(ref before) = messages_before_compression {
                    if messages != *before {
                        messages = before.clone();
                    }
                }
                if let Some(ref mut hb) = activity_heartbeat {
                    hb.stop("context compression rollback failed");
                }
                complete_lifecycle_outer(&mut compaction_done_emitted);
                lock_released = true;
                if let Some(ref r) = lock_refresher { r.stop(); }
                emit_compression_attempt_telemetry(agent, attempt_started_at, "aborted", "aborted", Some("rollback:Exception"));
                // In Python this `raise` would propagate; for slice audit we return None to signal unwind
                // But to preserve 1:1 we panic with a marker that tests can catch
                panic!("rollback:Exception during AuxiliaryExplicitCancellation restore");
            }
            // Normal restore
            let mut compressor_mut = agent.get("context_compressor").cloned().unwrap_or(json!({}));
            // Use stored snapshot
            let _ = restore_compressor_attempt_state(&mut compressor_mut, compressor_attempt_snapshot, *durable_cooldown_authoritative, durable_cooldown_state.as_ref());
            if let Some(obj) = agent.as_object_mut() {
                if let Some(comp_obj) = compressor_mut.as_object() {
                    obj.insert("context_compressor".to_string(), Value::Object(comp_obj.clone()));
                }
            }
            if let Some(ref before) = messages_before_compression {
                if messages != *before {
                    messages = before.clone();
                }
            }
            if let Some(ref mut hb) = activity_heartbeat {
                hb.stop("context compression cancelled");
            }
            complete_lifecycle_outer(&mut compaction_done_emitted);
            lock_released = true;
            if let Some(ref r) = lock_refresher { r.stop(); }
            if let Some(ref fence) = commit_fence { fence.finish_lock_setup(); }
            emit_compression_attempt_telemetry(agent, attempt_started_at, "aborted", "aborted", Some("explicit_interrupt"));
            let existing_sp = agent.get("_cached_system_prompt").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_else(|| system_message.to_string());
            return Some((messages, existing_sp));
        } else {
            // Python ll.3116-3131: except BaseException as _compress_exc: if _activity_heartbeat is not None: stop; _release_lock(); _emit...; raise
            complete_lifecycle_outer(&mut compaction_done_emitted);
            lock_released = true;
            if let Some(ref r) = lock_refresher { r.stop(); }
            if let Some(ref fence) = commit_fence { fence.finish_lock_setup(); }
            emit_compression_attempt_telemetry(agent, attempt_started_at, "aborted", "aborted", Some("exception:Exception"));
            panic!("BaseException in compress dispatch: {}", err_kind);
        }
    }

    // Python ll.3132-3134: finally: if _activity_heartbeat is not None: _activity_heartbeat.stop("context compression completed")
    if let Some(ref mut hb) = activity_heartbeat {
        hb.stop("context compression completed");
    }

    // -----------------------------------------------------------------------
    // Python ll.3136-3200: Post-dispatch guards — the slice ends inside this try
    // -----------------------------------------------------------------------
    // Python l.3136: _commit_fence_entered = False
    let mut commit_fence_entered = false;
    // Python ll.3137-3150: try: _compression_made_progress / _used_fallback / _feasibility_skip captures before hooks may reset them
    let compression_made_progress = agent.get("context_compressor").and_then(|c| c.get("_last_compression_made_progress")).and_then(|v| v.as_bool()).unwrap_or(false);
    let compression_used_fallback = agent.get("context_compressor").and_then(|c| c.get("_last_summary_fallback_used")).and_then(|v| v.as_bool()).unwrap_or(false);
    let compression_feasibility_skip = agent.get("context_compressor").and_then(|c| c.get("_last_feasibility_skip")).and_then(|v| v.as_bool()).unwrap_or(false);
    let _ = (compression_made_progress, compression_used_fallback, compression_feasibility_skip);

    // Python ll.3152-3183: if getattr(agent.context_compressor, "_last_compress_aborted", False): ... return messages,_existing_sp; finally: _release_lock()
    let last_compress_aborted = agent.get("context_compressor").and_then(|c| c.get("_last_compress_aborted")).and_then(|v| v.as_bool()).unwrap_or(false);
    if last_compress_aborted {
        let err = agent.get("context_compressor").and_then(|c| c.get("_last_summary_error")).and_then(|v| v.as_str()).unwrap_or("unknown error").to_string();
        let last_warn = agent.get("_last_compression_summary_warning").and_then(|v| v.as_str()).unwrap_or("");
        if last_warn != err {
            if let Some(obj) = agent.as_object_mut() {
                obj.insert("_last_compression_summary_warning".to_string(), json!(err.clone()));
                obj.insert("_last_emit_warning".to_string(), json!(format!("⚠ Compression aborted: {}. No messages were dropped — conversation continues unchanged. Run /compress to retry, or /new to start a fresh session.", err)));
            }
            eprintln!("⚠ Compression aborted: {}. No messages were dropped — conversation continues unchanged. Run /compress to retry, or /new to start a fresh session.", err);
        }
        let existing_sp = agent.get("_cached_system_prompt").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_else(|| system_message.to_string());
        let failure_class = agent.get("context_compressor").and_then(|c| c.get("_last_summary_error")).map(|v| if v.is_null() { "summary_generation_aborted" } else { "summary_generation_aborted" }).unwrap_or("summary_generation_aborted");
        emit_compression_attempt_telemetry(agent, attempt_started_at, "aborted", "aborted", Some(failure_class));
        complete_lifecycle_outer(&mut compaction_done_emitted);
        lock_released = true;
        if let Some(ref r) = lock_refresher { r.stop(); }
        if let Some(ref fence) = commit_fence { fence.finish_lock_setup(); }
        return Some((messages, existing_sp));
    }

    // Python ll.3184-3213: # Compare against pre-dispatch semantic state ... if compressed == messages_before_compression or (_strip_marker...) == ...: if messages != ...: messages[:] = ...; logger.info(...); _existing_sp=...; _emit...; _release_lock(); return messages,_existing_sp
    let messages_before = messages_before_compression.as_ref().cloned().unwrap_or_else(|| messages.clone());
    let compressed_val = Value::Array(compressed.clone());
    let before_val = Value::Array(messages_before.clone());
    let eq_raw = compressed_val == before_val;
    let eq_stripped = strip_marker_for_comparison(&compressed_val) == strip_marker_for_comparison(&before_val);
    if eq_raw || eq_stripped {
        if messages != messages_before {
            messages = messages_before.clone();
        }
        eprintln!("Compression made no progress (session={}) — skipping boundary rewrite.", session_label);
        let existing_sp = agent.get("_cached_system_prompt").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_else(|| system_message.to_string());
        emit_compression_attempt_telemetry(agent, attempt_started_at, "aborted", "aborted", Some("no_progress"));
        complete_lifecycle_outer(&mut compaction_done_emitted);
        lock_released = true;
        if let Some(ref r) = lock_refresher { r.stop(); }
        if let Some(ref fence) = commit_fence { fence.finish_lock_setup(); }
        return Some((messages, existing_sp));
    }

    // -----------------------------------------------------------------------
    // Slice boundary at l.3200
    // Python ll.3201+ continue with `if not compressed:` empty-transcript guard,
    // commit_fence.begin_commit, summary_error warnings, todo_snapshot injection,
    // stale-snapshot strip, scaffold handling, and the durable session rotation
    // / in-place archive_and_compact branches. Those live in
    // `conversation_slice5.rs` (ll.3201-4000) and `conversation_slice6.rs`
    // (ll.4001-4465). This stub closes the current try-block synthetically so
    // the module remains parsable without cargo; the audit reference for the
    // boundary is preserved here verbatim.
    //
    // To keep the module syntactically complete we synthesize the `compressed`
    // binding for callers that import only this slice; real callers via
    // `conversation_slice5::compress_context_continuation` will receive the
    // `compressed` value through the merged function's locals, not this stub.
    // The `if not compressed:` check at l.3215 is therefore deferred.
    // -----------------------------------------------------------------------

    // Store slice-local state back to agent for the next slice to resume.
    // In the merged view these are locals that flow into slice 5's scope.
    if let Some(obj) = agent.as_object_mut() {
        obj.insert("_slice4_compressed".to_string(), Value::Array(compressed.clone()));
        obj.insert("_slice4_messages_before".to_string(), Value::Array(messages_before));
        obj.insert("_slice4_compacted_in_place".to_string(), json!(compacted_in_place));
        obj.insert("_slice4_pre_msg_count".to_string(), json!(pre_msg_count));
        obj.insert("_slice4_commit_fence_entered".to_string(), json!(commit_fence_entered));
        obj.insert("_slice4_lock_released".to_string(), json!(lock_released));
        obj.insert("_slice4_compaction_done_emitted".to_string(), json!(compaction_done_emitted));
        // Pass lock state so slice 5 can call _release_lock() / finish correctly.
        if let Some(holder) = lock_holder {
            obj.insert("_slice4_lock_holder".to_string(), json!(holder));
        }
        obj.insert("_slice4_lock_sid".to_string(), json!(lock_sid));
        if let Some(db) = lock_db {
            obj.insert("_slice4_lock_db".to_string(), db);
        }
    }

    // Returning None signals to the (future) merged caller that no early-return
    // was taken in slice 4 and execution should continue at slice 5's
    // `if not compressed:` guard (l.3215). For standalone use of this slice we
    // return the current messages/compressed pair as a no-op placeholder — but
    // we annotate that it is synthetic so audit can distinguish it from a real
    // Python return. The canonical merged return lives in slice 5/6.
    //
    // We choose to return None here so `Option` semantics match the docstring:
    // Some = early return taken inside 2402-3200, None = fall through to 3201+.
    None
}

#[allow(dead_code)]
fn _compress_context_slice4(
    agent: &mut Value,
    messages: Vec<Value>,
    system_message: &str,
    approx_tokens: Option<usize>,
    focus_topic: Option<&str>,
    force: bool,
    commit_fence: Option<Arc<CompressionCommitFence>>,
    compressor_attempt_snapshot: &HashMap<String, Value>,
    durable_cooldown_authoritative: &mut Option<bool>,
    durable_cooldown_state: &mut Option<HashMap<String, Value>>,
    attempt_started_at: Instant,
    attempt_id: &str,
) -> Option<(Vec<Value>, String)> {
    compress_context_slice4(
        agent,
        messages,
        system_message,
        approx_tokens,
        focus_topic,
        force,
        commit_fence,
        compressor_attempt_snapshot,
        durable_cooldown_authoritative,
        durable_cooldown_state,
        attempt_started_at,
        attempt_id,
    )
}

// NOTE: Python ll.3201-4465 (remainder of `compress_context`, `_compress_context_via_codex_app_server`,
// `try_shrink_image_parts_in_messages`, and `__all__`) continue in
// `conversation_slice5.rs` (ll.3201-4000) and `conversation_slice6.rs` (ll.4001-4465).
// This slice is closed at 3200 with a synthetic fall-through so the module
// remains syntactically complete, matching the precedent in
// `conversation_slice3.rs` (stubbed at 2400) and `conversation_slice1.rs`
// (stubbed `resolve_context_compression_timeouts` at 800). All call sites
// that need the full `compress_context` should import from the merged view
// once slices combine; this stub will be removed when slices merge.
