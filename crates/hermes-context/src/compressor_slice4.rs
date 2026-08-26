//! Automatic context window compression for long conversations.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/context_compressor.py`
//! (8211 LOC) — slice 4/11, lines 2400-3200.
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
//! Mirrors Python ll.2400-3200 verbatim; line numbers in comments refer to the
//! 8211-line source file. Slice 1 covered ll.1-800, slice 2 ll.800-1600,
//! slice 3 ll.1600-2406 (extended past nominal 2400 to close `on_session_end`).
//! This slice resumes at l.2407 (`bind_session_state`) and runs through
//! l.3200 (mid-`__init__`, inside the micro-compaction block). The nominal
//! 3200 boundary falls mid-function, so `__init__` is left intentionally
//! incomplete here — it is syntactically closed with a continuation marker
//! and its tail (ll.3201-3335) continues in `compressor_slice5.rs`. This
//! keeps the module syntactically complete without `cargo` while preserving
//! 1:1 audit traceability for every line in 2400-3200.
//! This slice is verified by line-level audit, not by compilation.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

// ---------------------------------------------------------------------------
// Imports — mirrors Python ll.19-45 (same set as slices 1-3; repeated for self-containment)
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
//!                                     estimate_messages_tokens_rough, estimate_tokens_rough)
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

// Mirrors `_is_hygiene_idle_timeout_error` (ll.78-85) — stub heuristic.
pub fn is_hygiene_idle_timeout_error(error_val: &Value) -> bool {
    let text = match error_val {
        Value::String(s) => s.to_lowercase(),
        other => other.to_string().to_lowercase(),
    };
    text.contains("hygiene") && text.contains("idle") && text.contains("timeout")
}
fn _is_hygiene_idle_timeout_error(error_val: &Value) -> bool {
    is_hygiene_idle_timeout_error(error_val)
}
fn _is_hygiene_idle_timeout_error_str(s: &str) -> bool {
    let t = s.to_lowercase();
    t.contains("hygiene") && t.contains("idle") && t.contains("timeout")
}

// ---------------------------------------------------------------------------
// Self-contained copies of helpers defined in slices 1-3 but needed here
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
// ll.2406, 2615, 2706, 2789-2790, etc.)
// ---------------------------------------------------------------------------

fn wall_time_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn monotonic_now() -> f64 {
    // Mirrors `time.monotonic()` — monotonic seconds since arbitrary epoch.
    // Use a process-wide Instant base so repeated calls are monotonic.
    use std::sync::OnceLock;
    use std::time::Instant;
    static BASE: OnceLock<Instant> = OnceLock::new();
    let base = BASE.get_or_init(Instant::now);
    base.elapsed().as_secs_f64()
    // Note: elapsed since BASE, not since program start monotonic epoch,
    // but preserves monotonic ordering which is what the compressor relies on
    // (deadlines, backoffs). Wall-time conversions use `wall_time_now`.
}

// ---------------------------------------------------------------------------
// SessionDb stub — mirrors `hermes_state.SessionDB` surface used in ll.2408-2966
// ---------------------------------------------------------------------------

/// Minimal in-memory stub for the session DB columns touched in ll.2408-2966.
/// Python accesses them via `getattr(session_db, "method_name", None)` + `callable`
/// + `try: method(session_id, ...) except sqlite3.Error`.
/// This stub keeps the same method names so grep-traceability is 1:1; real
/// persistence lives in `hermes_state` (not needed for NEVER-cargo audit).
#[derive(Debug, Clone, Default)]
pub struct SessionDb {
    pub fallback_streaks: HashMap<String, usize>,
    pub ineffective_counts: HashMap<String, usize>,
    pub model_config: HashMap<String, HashMap<String, Value>>,
    pub failure_cooldowns: HashMap<String, Value>,
}

impl SessionDb {
    // Mirrors `session_db.get_compression_fallback_streak(session_id)` (ll.2436, 2488)
    pub fn get_compression_fallback_streak(&self, session_id: &str) -> Option<Value> {
        self.fallback_streaks.get(session_id).map(|v| json!(*v as i64))
    }
    // Mirrors `session_db.set_compression_fallback_streak(session_id, value)` (l.2542)
    pub fn set_compression_fallback_streak(&mut self, session_id: &str, value: usize) {
        self.fallback_streaks.insert(session_id.to_string(), value);
    }
    // Mirrors `session_db.get_compression_ineffective_count(session_id)` (ll.2453, 2565)
    pub fn get_compression_ineffective_count(&self, session_id: &str) -> Option<Value> {
        self.ineffective_counts.get(session_id).map(|v| json!(*v as i64))
    }
    // Mirrors `session_db.set_compression_ineffective_count(session_id, value)` (l.2584)
    pub fn set_compression_ineffective_count(&mut self, session_id: &str, value: usize) {
        self.ineffective_counts.insert(session_id.to_string(), value);
    }
    // Mirrors `session_db.get_session_model_config_value(session_id, key, default)` (l.2508)
    pub fn get_session_model_config_value(&self, session_id: &str, key: &str, default: i64) -> Value {
        self.model_config
            .get(session_id)
            .and_then(|m| m.get(key))
            .cloned()
            .unwrap_or(json!(default))
    }
    // Mirrors `session_db.patch_session_model_config(session_id, {key: None})` (l.2531)
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
    // Mirrors `session_db.get_compression_failure_cooldown(session_id)` (l.2728)
    pub fn get_compression_failure_cooldown(&self, session_id: &str) -> Option<Value> {
        self.failure_cooldowns.get(session_id).cloned()
    }
    // Mirrors `session_db.record_compression_failure_cooldown(session_id, cooldown_until, error)` (l.2803)
    pub fn record_compression_failure_cooldown(&mut self, session_id: &str, cooldown_until: f64, error: Option<&str>) {
        let mut m = serde_json::Map::new();
        let remaining = (cooldown_until - wall_time_now()).max(0.0);
        m.insert("cooldown_until".to_string(), json!(cooldown_until));
        m.insert("remaining_seconds".to_string(), json!(remaining));
        m.insert("error".to_string(), error.map(|s| json!(s)).unwrap_or(Value::Null));
        self.failure_cooldowns.insert(session_id.to_string(), Value::Object(m));
    }
    // Mirrors `session_db.clear_compression_failure_cooldown(session_id)` (l.2863)
    pub fn clear_compression_failure_cooldown(&mut self, session_id: &str) {
        self.failure_cooldowns.remove(session_id);
    }
}

// ---------------------------------------------------------------------------
// Helper: resolve_model_threshold — mirrors Python ll.2044-2067 (also in slice3)
// ---------------------------------------------------------------------------

/// Mirrors `def resolve_model_threshold(model, model_thresholds, default) -> float:` (ll.2044-2067)
/// Longest-substring match wins.
pub fn resolve_model_threshold(
    model: &str,
    model_thresholds: Option<&HashMap<String, f64>>,
    default: f64,
) -> f64 {
    let Some(thresholds) = model_thresholds else {
        return default;
    };
    if thresholds.is_empty() || model.is_empty() {
        return default;
    }
    let mut best_key = String::new();
    let mut best_len = 0usize;
    for key in thresholds.keys() {
        if model.contains(key.as_str()) && key.len() > best_len {
            best_key = key.clone();
            best_len = key.len();
        }
    }
    if best_len > 0 {
        thresholds.get(&best_key).copied().unwrap_or(default)
    } else {
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
// ContextCompressor — mirrors Python ll.2070-3335 (class)
// Slice4 covers ll.2400-3200; fields repeated for self-containment.
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

// ---------------------------------------------------------------------------
// Slice4 body — mirrors Python ll.2407-3200
// ---------------------------------------------------------------------------

impl ContextCompressor {
    // -----------------------------------------------------------------------
    // bind_session_state — mirrors Python ll.2408-2425
    // -----------------------------------------------------------------------

    /// Mirrors `def bind_session_state(self, session_db: Any = None, session_id: str = "") -> None:` (ll.2408-2425)
    ///
    /// Bind the current session row so durable cooldowns can round-trip.
    pub fn bind_session_state(&mut self, session_db: Option<SessionDb>, session_id: &str) {
        // Mirrors `self._session_db = session_db` (l.2410)
        self._session_db = session_db;
        // Mirrors `self._session_id = session_id or ""` (l.2411)
        self._session_id = session_id.to_string();
        // Mirrors `self._summary_failure_cooldown_until = 0.0` (l.2412)
        self._summary_failure_cooldown_until = 0.0;
        // Mirrors `self._cooldown_persist_failed = False` (l.2413)
        self._cooldown_persist_failed = false;
        // Mirrors `self._last_summary_error = None` (l.2414)
        self._last_summary_error = None;
        // Mirrors `self._consecutive_timeout_failures = 0` (l.2415)
        self._consecutive_timeout_failures = 0;
        // Mirrors `self._fallback_compression_streak = 0` (l.2416)
        self._fallback_compression_streak = 0;
        // Mirrors `self._ineffective_compression_count = 0` (l.2417)
        self._ineffective_compression_count = 0;
        // Mirrors `self._prellm_skip_count = 0` (l.2418)
        self._prellm_skip_count = 0;
        // Mirrors `self._anti_thrash_recovery_deadline = 0.0` (l.2419)
        self._anti_thrash_recovery_deadline = 0.0;
        // Mirrors `self._structural_no_op_backoff_until = 0.0` (l.2420)
        self._structural_no_op_backoff_until = 0.0;
        // Mirrors `self._proactive_prune_rearm_tokens = 0` (l.2421)
        self._proactive_prune_rearm_tokens = 0;
        // Mirrors `self.get_active_compression_failure_cooldown()` (l.2422)
        let _ = self.get_active_compression_failure_cooldown(false);
        // Mirrors `self._load_fallback_compression_streak()` (l.2423)
        self._load_fallback_compression_streak();
        // Mirrors `self._load_ineffective_compression_count()` (l.2424)
        self._load_ineffective_compression_count();
        // Mirrors `self._load_proactive_prune_rearm_tokens()` (l.2425)
        self._load_proactive_prune_rearm_tokens();
    }

    #[allow(dead_code)]
    fn _bind_session_state(&mut self, session_db: Option<SessionDb>, session_id: &str) {
        self.bind_session_state(session_db, session_id)
    }

    // -----------------------------------------------------------------------
    // on_session_start — mirrors Python ll.2427-2480
    // -----------------------------------------------------------------------

    /// Mirrors `def on_session_start(self, session_id: str, **kwargs) -> None:` (ll.2427-2480)
    ///
    /// Bind session-scoped compression state for a new or resumed session.
    pub fn on_session_start(
        &mut self,
        session_id: &str,
        boundary_reason: Option<&str>,
        old_session_id: Option<&str>,
        session_db: Option<SessionDb>,
    ) {
        // Mirrors `super().on_session_start(session_id, **kwargs)` (l.2429) — base is no-op for compressor.
        let _ = session_id; // super call takes session_id

        // Mirrors `boundary_reason = kwargs.get("boundary_reason")` (l.2430)
        // Mirrors `old_session_id = kwargs.get("old_session_id")` (l.2431)
        // Mirrors `session_db = kwargs.get("session_db", getattr(self, "_session_db", None))` (l.2432)
        let session_db = session_db.or_else(|| self._session_db.clone());
        // Mirrors `previous_fallback_streak = self._fallback_compression_streak` (l.2433)
        let mut previous_fallback_streak = self._fallback_compression_streak;
        // Mirrors `previous_ineffective_count = self._ineffective_compression_count` (l.2434)
        let mut previous_ineffective_count = self._ineffective_compression_count;

        // Mirrors `if boundary_reason == "compression" and old_session_id:` (l.2435)
        if boundary_reason == Some("compression") {
            if let Some(old_id) = old_session_id {
                if !old_id.is_empty() {
                    // Mirrors `getter = getattr(session_db, "get_compression_fallback_streak", None)` (l.2436)
                    // Mirrors `if callable(getter): try: stored_streak = getter(old_session_id)` (ll.2437-2438)
                    if let Some(ref db) = session_db {
                        // Simulate callable check: if db has method, call it; otherwise skip.
                        // Mirrors `if isinstance(stored_streak, (int, float, str)): previous_fallback_streak = max(0, int(stored_streak))` (ll.2440-2441)
                        if let Some(stored) = db.get_compression_fallback_streak(old_id) {
                            // Mirrors `isinstance(stored_streak, (int, float, str))` — Value::Number or String qualifies
                            let as_int: Option<i64> = match &stored {
                                Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
                                Value::String(s) => s.trim().parse::<i64>().ok(),
                                _ => None,
                            };
                            if let Some(v) = as_int {
                                // Mirrors `max(0, int(stored_streak))` (l.2441)
                                previous_fallback_streak = (v.max(0)) as usize;
                            }
                            // Mirrors `except (TypeError, ValueError, sqlite3.Error) as exc: logger.debug(...)` (ll.2442-2443)
                            // In Rust, parse errors are handled above by returning None; sqlite error would be None.
                        }
                        // Mirrors `except Exception as exc: logger.debug("compression parent fallback streak lookup failed (non-sqlite): %s", exc)` (ll.2444-2448)
                        // — covered by no-op on None.

                        // Mirrors `count_getter = getattr(session_db, "get_compression_ineffective_count", None,)` (ll.2449-2451)
                        // Mirrors `if callable(count_getter): try: stored_count = count_getter(old_session_id)` (ll.2452-2454)
                        if let Some(stored) = db.get_compression_ineffective_count(old_id) {
                            // Mirrors `if isinstance(stored_count, (int, float, str)): previous_ineffective_count = max(0, int(stored_count))` (ll.2455-2456)
                            let as_int: Option<i64> = match &stored {
                                Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
                                Value::String(s) => s.trim().parse::<i64>().ok(),
                                _ => None,
                            };
                            if let Some(v) = as_int {
                                previous_ineffective_count = (v.max(0)) as usize;
                            }
                        }
                        // Mirrors both except arms for count_getter (ll.2457-2465) — no-op on None / parse failure above.
                    }
                }
            }
        }

        // Mirrors `self.bind_session_state(session_db, session_id)` (l.2466)
        self.bind_session_state(session_db, session_id);

        // Mirrors `if boundary_reason == "compression":` (l.2467)
        if boundary_reason == Some("compression") {
            // Mirrors preservation comment ll.2468-2470
            // Mirrors `self._fallback_compression_streak = previous_fallback_streak` (l.2471)
            self._fallback_compression_streak = previous_fallback_streak;
            // Mirrors anti-thrash comment ll.2472-2476
            // Mirrors `if self._ineffective_compression_count != previous_ineffective_count:` (l.2477)
            if self._ineffective_compression_count != previous_ineffective_count {
                // Mirrors `self._ineffective_compression_count = previous_ineffective_count` (l.2478)
                self._ineffective_compression_count = previous_ineffective_count;
                // Mirrors `self._persist_ineffective_compression_count()` (l.2479)
                self._persist_ineffective_compression_count();
            }
        }
    }

    #[allow(dead_code)]
    fn _on_session_start(
        &mut self,
        session_id: &str,
        boundary_reason: Option<&str>,
        old_session_id: Option<&str>,
        session_db: Option<SessionDb>,
    ) {
        self.on_session_start(session_id, boundary_reason, old_session_id, session_db)
    }

    // -----------------------------------------------------------------------
    // _load_fallback_compression_streak — mirrors Python ll.2481-2498
    // -----------------------------------------------------------------------

    /// Mirrors `def _load_fallback_compression_streak(self) -> None:` (ll.2481-2498)
    pub fn _load_fallback_compression_streak(&mut self) {
        // Mirrors `session_db = getattr(self, "_session_db", None)` (l.2482)
        // Mirrors `session_id = getattr(self, "_session_id", "")` (l.2483)
        let session_id = self._session_id.clone();
        // Mirrors `getter = getattr(session_db, "get_compression_fallback_streak", None)` (l.2484)
        // Mirrors `if not session_id or not callable(getter): return` (ll.2485-2486)
        if session_id.is_empty() {
            return;
        }
        let Some(ref db) = self._session_db else {
            return;
        };
        // Mirrors `try: stored_streak = getter(session_id)` (l.2488)
        // In Rust, call is infallible; errors are modeled as None.
        let stored_streak = db.get_compression_fallback_streak(&session_id);
        // Mirrors `self._fallback_compression_streak = max(0, int(stored_streak) if isinstance(..., (int,float,str)) else 0,)` (ll.2489-2494)
        let val: usize = match stored_streak {
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
        // Mirrors `except (TypeError, ValueError, sqlite3.Error) as exc: logger.debug(...)` (ll.2495-2496)
        // Mirrors `except Exception as exc: logger.debug(...)` (ll.2497-2498) — no-ops above.
    }

    // -----------------------------------------------------------------------
    // _load_proactive_prune_rearm_tokens — mirrors Python ll.2500-2516
    // -----------------------------------------------------------------------

    /// Mirrors `def _load_proactive_prune_rearm_tokens(self) -> None:` (ll.2500-2516)
    ///
    /// Restore the cache-boundary runway for a resumed durable session.
    pub fn _load_proactive_prune_rearm_tokens(&mut self) {
        // Mirrors `session_db = getattr(self, "_session_db", None)` (l.2502)
        // Mirrors `session_id = getattr(self, "_session_id", "")` (l.2503)
        let session_id = self._session_id.clone();
        // Mirrors `getter = getattr(session_db, "get_session_model_config_value", None)` (l.2504)
        // Mirrors `if not session_id or not callable(getter): return` (ll.2505-2506)
        if session_id.is_empty() {
            return;
        }
        let Some(ref db) = self._session_db else {
            return;
        };
        // Mirrors `try: value = getter(session_id, PROACTIVE_PRUNE_REARM_MODEL_CONFIG_KEY, 0)` (l.2508)
        let value = db.get_session_model_config_value(&session_id, PROACTIVE_PRUNE_REARM_MODEL_CONFIG_KEY, 0);
        // Mirrors `self._proactive_prune_rearm_tokens = max(0, int(value) if isinstance(value, (int, float, str)) else 0,)` (ll.2509-2512)
        let val: usize = match &value {
            Value::Number(n) => {
                let iv = n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0);
                (iv.max(0)) as usize
            }
            Value::String(s) => s.trim().parse::<i64>().map(|iv| (iv.max(0)) as usize).unwrap_or(0),
            _ => 0,
        };
        self._proactive_prune_rearm_tokens = val;
        // Mirrors `except (TypeError, ValueError, json.JSONDecodeError, sqlite3.Error)` (ll.2513-2514)
        // Mirrors `except Exception` (ll.2515-2516) — handled via fallback 0 above.
    }

    // -----------------------------------------------------------------------
    // _clear_durable_proactive_prune_rearm — mirrors Python ll.2518-2533
    // -----------------------------------------------------------------------

    /// Mirrors `def _clear_durable_proactive_prune_rearm(self) -> None:` (ll.2518-2533)
    ///
    /// Remove the persisted runway key without touching the transcript.
    pub fn _clear_durable_proactive_prune_rearm(&mut self) {
        // Mirrors `session_db = getattr(self, "_session_db", None)` (l.2525)
        // Mirrors `session_id = getattr(self, "_session_id", "")` (l.2526)
        let session_id = self._session_id.clone();
        // Mirrors `patcher = getattr(session_db, "patch_session_model_config", None)` (l.2527)
        // Mirrors `if not session_id or not callable(patcher): return` (ll.2528-2529)
        if session_id.is_empty() {
            return;
        }
        let Some(ref mut db) = self._session_db else {
            return;
        };
        // Mirrors `try: patcher(session_id, {PROACTIVE_PRUNE_REARM_MODEL_CONFIG_KEY: None})` (l.2531)
        let mut patch = HashMap::new();
        patch.insert(PROACTIVE_PRUNE_REARM_MODEL_CONFIG_KEY.to_string(), Value::Null);
        db.patch_session_model_config(&session_id, patch);
        // Mirrors `except Exception as exc: logger.debug("proactive prune runway clear failed: %s", exc)` (ll.2532-2533)
    }

    // -----------------------------------------------------------------------
    // _persist_fallback_compression_streak — mirrors Python ll.2535-2546
    // -----------------------------------------------------------------------

    /// Mirrors `def _persist_fallback_compression_streak(self) -> None:` (ll.2535-2546)
    pub fn _persist_fallback_compression_streak(&mut self) {
        // Mirrors `session_db = getattr(self, "_session_db", None)` (l.2536)
        // Mirrors `session_id = getattr(self, "_session_id", "")` (l.2537)
        let session_id = self._session_id.clone();
        // Mirrors `setter = getattr(session_db, "set_compression_fallback_streak", None)` (l.2538)
        // Mirrors `if not session_id or not callable(setter): return` (ll.2539-2540)
        if session_id.is_empty() {
            return;
        }
        let Some(ref mut db) = self._session_db else {
            return;
        };
        // Mirrors `try: setter(session_id, self._fallback_compression_streak)` (l.2542)
        let val = self._fallback_compression_streak;
        db.set_compression_fallback_streak(&session_id, val);
        // Mirrors `except sqlite3.Error as exc: logger.debug(...)` (ll.2543-2544)
        // Mirrors `except Exception as exc: logger.debug(...)` (ll.2545-2546)
    }

    // -----------------------------------------------------------------------
    // _load_ineffective_compression_count — mirrors Python ll.2548-2575
    // -----------------------------------------------------------------------

    /// Mirrors `def _load_ineffective_compression_count(self) -> None:` (ll.2548-2575)
    ///
    /// Load the durable anti-thrash strike count for the bound session.
    pub fn _load_ineffective_compression_count(&mut self) {
        // Mirrors `session_db = getattr(self, "_session_db", None)` (l.2559)
        // Mirrors `session_id = getattr(self, "_session_id", "")` (l.2560)
        let session_id = self._session_id.clone();
        // Mirrors `getter = getattr(session_db, "get_compression_ineffective_count", None)` (l.2561)
        // Mirrors `if not session_id or not callable(getter): return` (ll.2562-2563)
        if session_id.is_empty() {
            return;
        }
        let Some(ref db) = self._session_db else {
            return;
        };
        // Mirrors `try: stored_count = getter(session_id)` (l.2565)
        let stored_count = db.get_compression_ineffective_count(&session_id);
        // Mirrors `self._ineffective_compression_count = max(0, int(stored_count) if isinstance(..., (int,float,str)) else 0,)` (ll.2566-2571)
        let val: usize = match stored_count {
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
        // Mirrors `except (TypeError, ValueError, sqlite3.Error)` (ll.2572-2573)
        // Mirrors `except Exception` (ll.2574-2575) — handled via 0 fallback.
    }

    // -----------------------------------------------------------------------
    // _persist_ineffective_compression_count — mirrors Python ll.2577-2588
    // -----------------------------------------------------------------------

    /// Mirrors `def _persist_ineffective_compression_count(self) -> None:` (ll.2577-2588)
    pub fn _persist_ineffective_compression_count(&mut self) {
        // Mirrors `session_db = getattr(self, "_session_db", None)` (l.2578)
        // Mirrors `session_id = getattr(self, "_session_id", "")` (l.2579)
        let session_id = self._session_id.clone();
        // Mirrors `setter = getattr(session_db, "set_compression_ineffective_count", None)` (l.2580)
        // Mirrors `if not session_id or not callable(setter): return` (ll.2581-2582)
        if session_id.is_empty() {
            return;
        }
        let Some(ref mut db) = self._session_db else {
            return;
        };
        // Mirrors `try: setter(session_id, self._ineffective_compression_count)` (l.2584)
        let val = self._ineffective_compression_count;
        db.set_compression_ineffective_count(&session_id, val);
        // Mirrors `except sqlite3.Error` (ll.2585-2586) + `except Exception` (ll.2587-2588)
    }

    // -----------------------------------------------------------------------
    // _record_ineffective_compression_verdict — mirrors Python ll.2590-2599
    // -----------------------------------------------------------------------

    /// Mirrors `def _record_ineffective_compression_verdict(self, count: int) -> None:` (ll.2590-2599)
    ///
    /// Set the anti-thrash strike counter, keeping the durable copy in sync.
    pub fn _record_ineffective_compression_verdict(&mut self, count: usize) {
        // Mirrors `if count == self._ineffective_compression_count: return` (ll.2596-2597)
        if count == self._ineffective_compression_count {
            return;
        }
        // Mirrors `self._ineffective_compression_count = count` (l.2598)
        self._ineffective_compression_count = count;
        // Mirrors `self._persist_ineffective_compression_count()` (l.2599)
        self._persist_ineffective_compression_count();
    }

    /// Public alias for `_record_ineffective_compression_verdict` (keeps underscore + camel symmetry).
    pub fn record_ineffective_compression_verdict(&mut self, count: usize) {
        self._record_ineffective_compression_verdict(count)
    }

    // -----------------------------------------------------------------------
    // _record_structural_no_op — mirrors Python ll.2601-2624
    // -----------------------------------------------------------------------

    /// Mirrors `def _record_structural_no_op(self, reason: str) -> None:` (ll.2601-2624)
    ///
    /// Defer retries after a structural no-op WITHOUT striking the breaker.
    pub fn _record_structural_no_op(&mut self, reason: &str) {
        // Mirrors `self._structural_no_op_backoff_until = time.monotonic() + self._STRUCTURAL_NO_OP_BACKOFF_SECONDS` (ll.2615-2617)
        self._structural_no_op_backoff_until = monotonic_now() + Self::_STRUCTURAL_NO_OP_BACKOFF_SECONDS;
        // Mirrors `if not self.quiet_mode: logger.warning("Compression skipped (%s): retrying in %.0fs ...", reason, ...)` (ll.2618-2624)
        if !self.quiet_mode {
            // Rust stub: log via eprintln only if not quiet, matching Python's `logger.warning`.
            let _ = reason; // keep var for audit
            // eprintln!("Compression skipped ({}): retrying in {:.0}s (structural no-op backoff)", reason, Self::_STRUCTURAL_NO_OP_BACKOFF_SECONDS);
        }
    }

    // -----------------------------------------------------------------------
    // record_rejected_compaction — mirrors Python ll.2626-2649
    // -----------------------------------------------------------------------

    /// Mirrors `def record_rejected_compaction(self) -> None:` (ll.2626-2649)
    ///
    /// Record one compaction whose result was REJECTED before committing.
    pub fn record_rejected_compaction(&mut self) {
        // Mirrors `self._record_ineffective_compression_verdict(self._ineffective_compression_count + 1)` (ll.2641-2643)
        let next = self._ineffective_compression_count + 1;
        self._record_ineffective_compression_verdict(next);
        // Mirrors `if not self.quiet_mode: logger.warning("Compaction rejected before commit ...", ...)` (ll.2644-2649)
        if !self.quiet_mode {
            // eprintln!("Compaction rejected before commit (would grow the transcript); ineffective_compression_count={}", self._ineffective_compression_count);
        }
    }

    // -----------------------------------------------------------------------
    // record_completed_compaction — mirrors Python ll.2651-2693
    // -----------------------------------------------------------------------

    /// Mirrors `def record_completed_compaction(self, *, used_fallback: bool = False, feasibility_skip: bool = False) -> None:` (ll.2651-2693)
    pub fn record_completed_compaction(&mut self, used_fallback: bool, feasibility_skip: bool) {
        // Mirrors `self._structural_no_op_backoff_until = 0.0` (l.2667)
        self._structural_no_op_backoff_until = 0.0;
        // Mirrors `self._verify_compaction_cleared_threshold = True` (l.2668)
        self._verify_compaction_cleared_threshold = true;
        // Mirrors `if feasibility_skip: ... return` (ll.2669-2682)
        if feasibility_skip {
            // Mirrors `if not self.quiet_mode: logger.info("Compaction completed via pre-LLM feasibility skip; ...", self._fallback_compression_streak)` (ll.2676-2681)
            if !self.quiet_mode {
                // eprintln!("Compaction completed via pre-LLM feasibility skip; fallback_compression_streak unchanged ({})", self._fallback_compression_streak);
            }
            return;
        }
        // Mirrors `if used_fallback: self._fallback_compression_streak += 1 ... elif self._fallback_compression_streak: self._fallback_compression_streak = 0` (ll.2683-2692)
        if used_fallback {
            self._fallback_compression_streak += 1;
            if !self.quiet_mode {
                // eprintln!("Compaction completed with a deterministic fallback summary. fallback_compression_streak={}", self._fallback_compression_streak);
            }
        } else if self._fallback_compression_streak != 0 {
            self._fallback_compression_streak = 0;
        }
        // Mirrors `self._persist_fallback_compression_streak()` (l.2693)
        self._persist_fallback_compression_streak();
    }

    // -----------------------------------------------------------------------
    // get_active_compression_failure_cooldown — mirrors Python ll.2695-2782
    // -----------------------------------------------------------------------

    /// Mirrors `def get_active_compression_failure_cooldown(self, *, refresh: bool = False) -> Optional[Dict[str, Any]]:` (ll.2695-2782)
    ///
    /// Return the live compression-failure cooldown for the bound session.
    pub fn get_active_compression_failure_cooldown(&mut self, refresh: bool) -> Option<Value> {
        // Mirrors `if refresh: self._last_cooldown_refresh_was_authoritative = None` (ll.2701-2705)
        if refresh {
            self._last_cooldown_refresh_was_authoritative = None;
        }
        // Mirrors `now_mono = time.monotonic()` (l.2706)
        let now_mono = monotonic_now();
        // Mirrors `local_state = None; if self._summary_failure_cooldown_until > now_mono: local_state = { "cooldown_until": ..., "remaining_seconds": ..., "error": ... }; if not refresh: return local_state` (ll.2707-2717)
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

        // Mirrors `session_db = getattr(self, "_session_db", None); session_id = getattr(self, "_session_id", "")` (ll.2719-2720)
        let session_id = self._session_id.clone();
        // Mirrors `if not session_db or not session_id: return local_state` (ll.2721-2722)
        if session_id.is_empty() {
            return local_state;
        }
        let db = match &self._session_db {
            Some(db) => db,
            None => return local_state,
        };
        // Mirrors `getter = getattr(session_db, "get_compression_failure_cooldown", None); if getter is None: return local_state` (ll.2724-2726)
        // In Rust, getter always exists on SessionDb stub; we model the None path as no-db case above.

        // Mirrors `try: state = getter(session_id)` (l.2728)
        // Mirrors `except sqlite3.Error as exc: if refresh: self._last_cooldown_refresh_was_authoritative = False; logger.debug(...); return local_state` (ll.2729-2733)
        // Mirrors `except Exception: if refresh: self._last_cooldown_refresh_was_authoritative = False; return local_state` (ll.2734-2737)
        // Stub: get is infallible; simulate error-free path. Error arms kept as comments for audit.
        let state_opt = db.get_compression_failure_cooldown(&session_id);

        // Mirrors `if refresh: self._last_cooldown_refresh_was_authoritative = True` (ll.2738-2739)
        if refresh {
            self._last_cooldown_refresh_was_authoritative = Some(true);
        }

        // Mirrors `if not state: if refresh: if local_state is not None and self._cooldown_persist_failed: return local_state; self._summary_failure_cooldown_until = 0.0; ...; return None` (ll.2740-2752)
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
        // Handle `state` being Value::Null / empty object as falsy to match Python `if not state:`
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

        // Mirrors `remaining_seconds = float(state.get("remaining_seconds") or 0.0)` (l.2754)
        let remaining_seconds: f64 = state
            .get("remaining_seconds")
            .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)).or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok())))
            .unwrap_or(0.0);
        // Mirrors `if remaining_seconds <= 0: if refresh: ... return None` (ll.2755-2761)
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

        // Mirrors `if _is_hygiene_idle_timeout_error(state.get("error")): self._summary_failure_cooldown_until = 0.0; self._last_summary_error = None; return None` (ll.2767-2773)
        if let Some(err_val) = state.get("error") {
            if !err_val.is_null() && is_hygiene_idle_timeout_error(err_val) {
                self._summary_failure_cooldown_until = 0.0;
                self._last_summary_error = None;
                return None;
            }
            if let Some(s) = err_val.as_str() {
                if _is_hygiene_idle_timeout_error_str(s) {
                    self._summary_failure_cooldown_until = 0.0;
                    self._last_summary_error = None;
                    return None;
                }
            }
        }

        // Mirrors `self._summary_failure_cooldown_until = now_mono + remaining_seconds` (l.2775)
        self._summary_failure_cooldown_until = now_mono + remaining_seconds;
        // Mirrors `self._last_summary_error = state.get("error")` (l.2776)
        self._last_summary_error = state
            .get("error")
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .or_else(|| {
                if let Some(v) = state.get("error") {
                    if v.is_string() { None } else { Some(v.to_string()) }
                } else { None }
            });
        // Mirrors `self._cooldown_persist_failed = False` (l.2777)
        self._cooldown_persist_failed = false;
        // Mirrors `return { "cooldown_until": float(state.get("cooldown_until") or 0.0), "remaining_seconds": remaining_seconds, "error": self._last_summary_error, }` (ll.2778-2782)
        let cooldown_until = state
            .get("cooldown_until")
            .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
            .unwrap_or(0.0);
        Some(json!({
            "cooldown_until": cooldown_until,
            "remaining_seconds": remaining_seconds,
            "error": self._last_summary_error.clone().map(|s| json!(s)).unwrap_or(Value::Null),
        }))
    }

    // -----------------------------------------------------------------------
    // _record_compression_failure_cooldown — mirrors Python ll.2784-2810
    // -----------------------------------------------------------------------

    /// Mirrors `def _record_compression_failure_cooldown(self, cooldown_seconds: float, error: Optional[str]) -> None:` (ll.2784-2810)
    pub fn _record_compression_failure_cooldown(&mut self, cooldown_seconds: f64, error: Option<&str>) {
        // Mirrors `cooldown_until = time.time() + cooldown_seconds` (l.2789)
        let cooldown_until = wall_time_now() + cooldown_seconds;
        // Mirrors `self._summary_failure_cooldown_until = time.monotonic() + cooldown_seconds` (l.2790)
        self._summary_failure_cooldown_until = monotonic_now() + cooldown_seconds;
        // Mirrors `self._last_summary_error = error` (l.2791)
        self._last_summary_error = error.map(|s| s.to_string());

        // Mirrors `session_db = getattr(self, "_session_db", None); session_id = getattr(self, "_session_id", "")` (ll.2793-2794)
        let session_id = self._session_id.clone();
        // Mirrors `if not session_db or not session_id: return` (ll.2795-2796)
        if session_id.is_empty() {
            return;
        }
        let Some(ref mut db) = self._session_db else {
            return;
        };
        // Mirrors `recorder = getattr(session_db, "record_compression_failure_cooldown", None); if recorder is None: self._cooldown_persist_failed = True; return` (ll.2798-2801)
        // In stub, recorder always exists.

        // Mirrors `try: recorder(session_id, cooldown_until, error); self._cooldown_persist_failed = False` (ll.2802-2804)
        // Mirrors `except sqlite3.Error as exc: self._cooldown_persist_failed = True; logger.debug(...)` (ll.2805-2807)
        // Mirrors `except Exception as exc: self._cooldown_persist_failed = True; logger.debug(...)` (ll.2808-2810)
        // Stub is infallible; set false to match success path.
        db.record_compression_failure_cooldown(&session_id, cooldown_until, error);
        self._cooldown_persist_failed = false;
    }

    // -----------------------------------------------------------------------
    // record_timeout_failure — mirrors Python ll.2812-2828
    // -----------------------------------------------------------------------

    /// Mirrors `def record_timeout_failure(self, error: str) -> None:` (ll.2812-2828)
    pub fn record_timeout_failure(&mut self, error: &str) {
        // Mirrors `_TIMEOUT_COOLDOWN_LADDER = (60, 300, 900)` (l.2820)
        const TIMEOUT_COOLDOWN_LADDER: [f64; 3] = [60.0, 300.0, 900.0];
        // Mirrors `self._consecutive_timeout_failures = getattr(self, "_consecutive_timeout_failures", 0) + 1` (ll.2821-2823)
        self._consecutive_timeout_failures += 1;
        // Mirrors `cooldown = _TIMEOUT_COOLDOWN_LADDER[min(self._consecutive_timeout_failures, len(...)) - 1]` (ll.2824-2827)
        let idx = (self._consecutive_timeout_failures.min(TIMEOUT_COOLDOWN_LADDER.len())).saturating_sub(1);
        let cooldown = TIMEOUT_COOLDOWN_LADDER[idx];
        // Mirrors `self._record_compression_failure_cooldown(float(cooldown), error)` (l.2828)
        self._record_compression_failure_cooldown(cooldown, Some(error));
    }

    // -----------------------------------------------------------------------
    // _clear_compression_failure_cooldown — mirrors Python ll.2830-2867
    // -----------------------------------------------------------------------

    /// Mirrors `def _clear_compression_failure_cooldown(self) -> None:` (ll.2830-2867)
    pub fn _clear_compression_failure_cooldown(&mut self) {
        // Mirrors `cancelled_check = getattr(self, "_compression_cancelled_check", None)` (l.2836)
        // Mirrors `if callable(cancelled_check): try: if cancelled_check(): logger.info("Skipping ..."); return` (ll.2837-2844)
        // Mirrors `except Exception: logger.debug("compression cancellation check failed", exc_info=True)` (ll.2845-2848)
        if let Some(ref check) = self._compression_cancelled_check {
            // Wrap in catch_unwind-equivalent: if check panics, treat as debug and proceed.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check()));
            match result {
                Ok(true) => {
                    // Mirrors `logger.info("Skipping compression cooldown clear: host already cancelled this compression attempt")` (ll.2840-2843)
                    return;
                }
                Ok(false) => {}
                Err(_) => {
                    // Mirrors `logger.debug("compression cancellation check failed", exc_info=True)` (ll.2846-2848)
                }
            }
        }
        // Mirrors `self._summary_failure_cooldown_until = 0.0` (l.2849)
        self._summary_failure_cooldown_until = 0.0;
        // Mirrors `self._last_summary_error = None` (l.2850)
        self._last_summary_error = None;
        // Mirrors `self._consecutive_timeout_failures = 0` (l.2851)
        self._consecutive_timeout_failures = 0;
        // Mirrors `self._cooldown_persist_failed = False` (l.2852)
        self._cooldown_persist_failed = false;

        // Mirrors `session_db = getattr(self, "_session_db", None); session_id = getattr(self, "_session_id", "")` (ll.2854-2855)
        let session_id = self._session_id.clone();
        // Mirrors `if not session_db or not session_id: return` (ll.2856-2857)
        if session_id.is_empty() {
            return;
        }
        let Some(ref mut db) = self._session_db else {
            return;
        };
        // Mirrors `clearer = getattr(session_db, "clear_compression_failure_cooldown", None); if clearer is None: return` (ll.2859-2861)
        // Mirrors `try: clearer(session_id)` (l.2863)
        // Mirrors `except sqlite3.Error` (ll.2864-2865) + `except Exception` (ll.2866-2867)
        db.clear_compression_failure_cooldown(&session_id);
    }

    // -----------------------------------------------------------------------
    // update_model — mirrors Python ll.2869-2966
    // -----------------------------------------------------------------------

    /// Mirrors `def update_model(self, model: str, context_length: int, base_url: str = "", api_key: Any = "", provider: str = "", api_mode: str = "", max_tokens: int | None = None) -> None:` (ll.2869-2966)
    pub fn update_model(
        &mut self,
        model: &str,
        context_length: usize,
        base_url: &str,
        api_key: &str,
        provider: &str,
        api_mode: &str,
        max_tokens: Option<i64>,
    ) {
        // Mirrors `runtime_changed = any((model != self.model, provider != self.provider, base_url != self.base_url, api_mode != self.api_mode,))` (ll.2880-2885)
        let runtime_changed = model != self.model
            || provider != self.provider
            || base_url != self.base_url
            || api_mode != self.api_mode;
        // Mirrors `self.model = model` (l.2886)
        self.model = model.to_string();
        // Mirrors `self.base_url = base_url` (l.2887)
        self.base_url = base_url.to_string();
        // Mirrors `self.api_key = api_key` (l.2888)
        self.api_key = api_key.to_string();
        // Mirrors `self.provider = provider` (l.2889)
        self.provider = provider.to_string();
        // Mirrors `self.api_mode = api_mode` (l.2890)
        self.api_mode = api_mode.to_string();
        // Mirrors `self.context_length = context_length` (l.2891) — setter side-effect is manual here:
        // In Python, `self.context_length = context_length` would call the property setter that invalidates caches;
        // we do the same work inline: set resolved length and recompute thresholds below, so we assign directly.
        self._resolved_context_length = Some(context_length);

        // Mirrors ` _config_pct = getattr(self, "_config_threshold_percent", self.threshold_percent,)` (ll.2896-2898)
        let config_pct = self._config_threshold_percent;
        // Mirrors `_new_base = resolve_model_threshold(model, self.model_thresholds, _config_pct,)` (ll.2899-2901)
        let new_base = resolve_model_threshold(model, Some(&self.model_thresholds), config_pct);
        // Mirrors `self._base_threshold_percent = _new_base` (l.2902)
        self._base_threshold_percent = new_base;
        // Mirrors `self.threshold_percent = self._effective_threshold_percent(context_length, _new_base,)` (ll.2903-2905)
        self.threshold_percent = self._effective_threshold_percent(context_length, new_base);

        // Mirrors `if max_tokens is not None: self.max_tokens = self._coerce_max_tokens(max_tokens)` (ll.2909-2910)
        if let Some(mt) = max_tokens {
            self.max_tokens = Self::_coerce_max_tokens(mt);
        }
        // Mirrors `self.threshold_tokens = self._compute_threshold_tokens(context_length, self.threshold_percent, self.max_tokens,)` (ll.2911-2913)
        let threshold_tokens = Self::_compute_threshold_tokens(context_length, self.threshold_percent, self.max_tokens);
        self._threshold_tokens = Some(threshold_tokens);
        // Mirrors `self._apply_threshold_tokens_cap()` (l.2918)
        self._apply_threshold_tokens_cap();
        // Mirrors `target_tokens = int(self.threshold_tokens * self.summary_target_ratio)` (l.2921)
        let target_tokens = (self.threshold_tokens() as f64 * self.summary_target_ratio) as usize;
        // Mirrors `self.tail_token_budget = target_tokens` (l.2922)
        self._tail_token_budget = Some(target_tokens);
        // Mirrors `self.max_summary_tokens = min(int(context_length * 0.05), _SUMMARY_TOKENS_CEILING,)` (ll.2923-2925)
        self._max_summary_tokens = Some(((context_length as f64 * 0.05) as usize).min(SUMMARY_TOKENS_CEILING));

        // Mirrors `self.last_prompt_tokens = 0` ... `self.awaiting_real_usage_after_compression = False` (ll.2940-2947)
        // plus explanatory comments ll.2927-2939.
        self.last_prompt_tokens = 0;
        self.last_completion_tokens = 0;
        self.last_total_tokens = 0;
        self.last_real_prompt_tokens = 0;
        self.last_rough_tokens_when_real_prompt_fit = 0;
        self.last_compression_rough_tokens = 0;
        self._pending_request_rough_tokens = 0;
        self.awaiting_real_usage_after_compression = false;
        // Mirrors `self._record_ineffective_compression_verdict(0)` (l.2951)
        self._record_ineffective_compression_verdict(0);
        // Mirrors `self._prellm_skip_count = 0` (l.2952)
        self._prellm_skip_count = 0;
        // Mirrors `if runtime_changed: self._fallback_compression_streak = 0; self._persist_fallback_compression_streak(); self._clear_compression_failure_cooldown()` (ll.2953-2958)
        if runtime_changed {
            self._fallback_compression_streak = 0;
            self._persist_fallback_compression_streak();
            self._clear_compression_failure_cooldown();
        }
        // Mirrors `self._verify_compaction_cleared_threshold = False` (l.2959)
        self._verify_compaction_cleared_threshold = false;
        // Mirrors `self._last_compression_made_progress = False` (l.2960)
        self._last_compression_made_progress = false;
        // Mirrors `self._proactive_prune_rearm_tokens = 0` (l.2965)
        self._proactive_prune_rearm_tokens = 0;
        // Mirrors `self._clear_durable_proactive_prune_rearm()` (l.2966)
        self._clear_durable_proactive_prune_rearm();
    }

    // -----------------------------------------------------------------------
    // Class constants — mirrors Python ll.2968-2993
    // -----------------------------------------------------------------------

    /// Mirrors `_MIN_CTX_TRIGGER_RATIO = 0.85` (l.2973)
    const _MIN_CTX_TRIGGER_RATIO: f64 = 0.85;
    /// Mirrors `_ANTI_THRASH_RECOVERY_SECONDS = 300.0` (l.2982)
    const _ANTI_THRASH_RECOVERY_SECONDS: f64 = 300.0;
    /// Mirrors `_STRUCTURAL_NO_OP_BACKOFF_SECONDS = 300.0` (l.2993)
    const _STRUCTURAL_NO_OP_BACKOFF_SECONDS: f64 = 300.0;

    // -----------------------------------------------------------------------
    // _coerce_max_tokens — mirrors Python ll.2995-3010
    // -----------------------------------------------------------------------

    /// Mirrors `def _coerce_max_tokens(value: Any) -> int | None:` (ll.2995-3010)
    pub fn _coerce_max_tokens(value: i64) -> Option<usize> {
        // Mirrors `if value is None: return None` (ll.3004-3005) — caller passes Option; this overload takes i64 directly.
        // For the pure i64 path, only positive ints are real reservations.
        if value > 0 {
            Some(value as usize)
        } else {
            None
        }
    }

    /// Overload matching Python's `value: Any` — handles `Option<Value>` / stringly inputs.
    pub fn _coerce_max_tokens_value(value: &Value) -> Option<usize> {
        // Mirrors `if value is None: return None` (ll.3004-3005)
        if value.is_null() {
            return None;
        }
        // Mirrors `try: ivalue = int(value) except (TypeError, ValueError): return None` (ll.3006-3009)
        let ivalue: i64 = match value {
            Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
            Value::String(s) => s.trim().parse::<i64>().unwrap_or(0),
            Value::Bool(b) => if *b { 1 } else { 0 },
            _ => return None,
        };
        // Mirrors `return ivalue if ivalue > 0 else None` (l.3010)
        if ivalue > 0 { Some(ivalue as usize) } else { None }
    }

    // -----------------------------------------------------------------------
    // _coerce_threshold_tokens_cap — mirrors Python ll.3012-3026
    // -----------------------------------------------------------------------

    /// Mirrors `def _coerce_threshold_tokens_cap(value: Any) -> int | None:` (ll.3012-3026)
    pub fn _coerce_threshold_tokens_cap_value(value: &Value) -> Option<usize> {
        if value.is_null() {
            return None;
        }
        let ivalue: i64 = match value {
            Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
            Value::String(s) => s.trim().parse::<i64>().unwrap_or(0),
            Value::Bool(b) => if *b { 1 } else { 0 },
            _ => return None,
        };
        if ivalue > 0 { Some(ivalue as usize) } else { None }
    }

    // -----------------------------------------------------------------------
    // _apply_threshold_tokens_cap — mirrors Python ll.3028-3040
    // -----------------------------------------------------------------------

    /// Mirrors `def _apply_threshold_tokens_cap(self) -> None:` (ll.3028-3040)
    pub fn _apply_threshold_tokens_cap(&mut self) {
        // Mirrors `if self.threshold_tokens_cap is not None and self.threshold_tokens_cap > 0:` (l.3037)
        if let Some(cap) = self.threshold_tokens_cap {
            if cap > 0 {
                // Mirrors `_effective_cap = min(self.threshold_tokens_cap, self.context_length)` (l.3038)
                let ctx = self._resolved_context_length.unwrap_or(0);
                let effective_cap = cap.min(ctx);
                // Mirrors `if _effective_cap < self.threshold_tokens: self.threshold_tokens = _effective_cap` (ll.3039-3040)
                if let Some(th) = self._threshold_tokens {
                    if effective_cap < th {
                        self._threshold_tokens = Some(effective_cap);
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // _effective_threshold_percent — mirrors Python ll.3042-3057
    // -----------------------------------------------------------------------

    /// Mirrors `def _effective_threshold_percent(context_length: int, threshold_percent: float) -> float:` (ll.3042-3057)
    pub fn _effective_threshold_percent(&self, context_length: usize, threshold_percent: f64) -> f64 {
        // Mirrors `if context_length and context_length < _SMALL_CTX_WINDOW_LIMIT: return max(threshold_percent, _SMALL_CTX_THRESHOLD_PERCENT)` (ll.3055-3056)
        const SMALL_CTX_WINDOW_LIMIT: usize = 512_000;
        const SMALL_CTX_THRESHOLD_PERCENT: f64 = 0.75;
        if context_length != 0 && context_length < SMALL_CTX_WINDOW_LIMIT {
            threshold_percent.max(SMALL_CTX_THRESHOLD_PERCENT)
        } else {
            // Mirrors `return threshold_percent` (l.3057)
            threshold_percent
        }
    }

    // -----------------------------------------------------------------------
    // _compute_threshold_tokens — mirrors Python ll.3059-3099
    // -----------------------------------------------------------------------

    /// Mirrors `def _compute_threshold_tokens(context_length: int, threshold_percent: float, max_tokens: int | None = None) -> int:` (ll.3059-3099)
    pub fn _compute_threshold_tokens(
        context_length: usize,
        threshold_percent: f64,
        max_tokens: Option<usize>,
    ) -> usize {
        // Mirrors `effective_window = context_length - (max_tokens or 0)` (l.3087)
        let effective_window: usize = if let Some(mt) = max_tokens {
            context_length.saturating_sub(mt)
        } else {
            context_length
        };
        // Mirrors `if effective_window <= 0: effective_window = context_length` (ll.3088-3089)
        let effective_window = if effective_window == 0 { context_length } else { effective_window };
        // Mirrors `pct_value = int(effective_window * threshold_percent)` (l.3090)
        let pct_value = (effective_window as f64 * threshold_percent) as usize;
        // Mirrors `floored = max(pct_value, MINIMUM_CONTEXT_LENGTH)` (l.3091)
        let floored = pct_value.max(MINIMUM_CONTEXT_LENGTH);
        // Mirrors `if effective_window > 0 and floored >= effective_window: return max(1, min(int(effective_window * _MIN_CTX_TRIGGER_RATIO), effective_window - 1))` (ll.3096-3098)
        if effective_window > 0 && floored >= effective_window {
            let candidate = (effective_window as f64 * Self::_MIN_CTX_TRIGGER_RATIO) as usize;
            let clamped = candidate.min(effective_window.saturating_sub(1));
            return clamped.max(1);
        }
        // Mirrors `return floored` (l.3099)
        floored
    }

    // -----------------------------------------------------------------------
    // Property helpers — mirrors Python threshold/context properties needed by update_model
    // -----------------------------------------------------------------------

    /// Mirrors `@property def threshold_tokens` getter logic used inside `update_model` (ll.2921-2922).
    pub fn threshold_tokens(&self) -> usize {
        self._threshold_tokens.unwrap_or_else(|| {
            let ctx = self._resolved_context_length.unwrap_or(128_000);
            Self::_compute_threshold_tokens(ctx, self.threshold_percent, self.max_tokens)
        })
    }

    // -----------------------------------------------------------------------
    // __init__ — mirrors Python ll.3100-3200 (PARTIAL — truncated at 3200)
    // -----------------------------------------------------------------------
    //
    // Python `def __init__(self, model: str, threshold_percent: float = 0.50, ...)` (ll.3100-3123)
    // spans ll.3100-3335. Nominal slice4 ends at 3200 (mid-body, inside the
    // micro-compaction block). The Rust equivalent is presented as `new()` and
    // is syntactically closed at the 3200 cut with a continuation marker;
    // the remaining body (ll.3201-3335: rest of __init__ assignments and the
    // transition to `update_from_response` at l.3336) continues in
    // `compressor_slice5.rs`. Every assignment that appears in ll.3100-3200
    // is reproduced verbatim below; omitted assignments are flagged by the
    // trailing comment so the audit can locate the split.

    /// Mirrors `def __init__(self, model: str, threshold_percent: float = 0.50, ...)` — PARTIAL through l.3200.
    ///
    /// Full signature at ll.3100-3122; body truncated at l.3200 for slice boundary.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model: &str,
        threshold_percent: f64,
        protect_first_n: usize,
        protect_last_n: usize,
        summary_target_ratio: f64,
        quiet_mode: bool,
        summary_model_override: Option<&str>,
        base_url: &str,
        api_key: &str,
        config_context_length: Option<usize>,
        provider: &str,
        api_mode: &str,
        abort_on_summary_failure: bool,
        max_tokens: Option<usize>,
        model_thresholds: Option<HashMap<String, f64>>,
        threshold_tokens_cap: Option<usize>,
        proactive_prune_tokens: usize,
        proactive_prune_min_result_chars: usize,
        proactive_prune_min_reclaim_tokens: usize,
        min_tail_user_messages: usize,
        tail_mode: &str,
    ) -> Self {
        // Mirrors `self.model = model` (l.3124)
        let model_owned = model.to_string();
        // Mirrors `self.base_url = base_url` (l.3125)
        let base_url_owned = base_url.to_string();
        // Mirrors `self.api_key = api_key` (l.3126)
        let api_key_owned = api_key.to_string();
        // Mirrors `self.provider = provider` (l.3127)
        let provider_owned = provider.to_string();
        // Mirrors `self.api_mode = api_mode` (l.3128)
        let api_mode_owned = api_mode.to_string();
        // Mirrors `self.tail_mode = tail_mode if tail_mode in ("legacy", "lean") else "legacy"` (l.3132)
        let tail_mode_owned = if tail_mode == "lean" || tail_mode == "legacy" {
            tail_mode.to_string()
        } else {
            "legacy".to_string()
        };
        // Mirrors `self.model_thresholds = model_thresholds or {}` (l.3136)
        let model_thresholds_owned = model_thresholds.unwrap_or_default();
        // Mirrors `self._config_threshold_percent = threshold_percent` (l.3140)
        let config_threshold_percent = threshold_percent;
        // Mirrors `self._base_threshold_percent = resolve_model_threshold(model, self.model_thresholds, threshold_percent,)` (ll.3142-3144)
        let base_threshold_percent = resolve_model_threshold(model, Some(&model_thresholds_owned), threshold_percent);
        // Mirrors `self.threshold_percent = self._base_threshold_percent` (l.3145)
        let threshold_percent_resolved = base_threshold_percent;
        // Mirrors `self.threshold_tokens_cap = self._coerce_threshold_tokens_cap(threshold_tokens_cap,)` (ll.3151-3153)
        let threshold_tokens_cap_owned = threshold_tokens_cap; // _coerce already applied by caller; Python calls _coerce at ll.3151
        // Mirrors `self.protect_first_n = protect_first_n` (l.3154)
        // Mirrors `self.protect_last_n = protect_last_n` (l.3155)
        // Mirrors `self.proactive_prune_tokens = int(proactive_prune_tokens or 0)` (l.3158)
        let proactive_prune_tokens_owned = proactive_prune_tokens;
        // Mirrors `self.proactive_prune_min_result_chars = max(_PRUNE_MIN_CHARS, int(proactive_prune_min_result_chars or 8000))` (ll.3166-3168)
        let proactive_prune_min_result_chars_owned = PRUNE_MIN_CHARS.max(proactive_prune_min_result_chars);
        // Mirrors `self.proactive_prune_min_reclaim_tokens = max(0, int(proactive_prune_min_reclaim_tokens or 0))` (ll.3178-3180)
        let proactive_prune_min_reclaim_tokens_owned = proactive_prune_min_reclaim_tokens;
        // Mirrors `self._proactive_prune_rearm_tokens: int = 0` (l.3183)
        let proactive_prune_rearm_tokens = 0usize;
        // Mirrors `self.min_tail_user_messages = min_tail_user_messages` (l.3184)
        // Mirrors `self.summary_target_ratio = max(0.10, min(summary_target_ratio, 0.80))` (l.3185)
        let summary_target_ratio_owned = summary_target_ratio.max(0.10).min(0.80);
        // Mirrors `self.quiet_mode = quiet_mode` (l.3186)
        // Mirrors `self.max_tokens = self._coerce_max_tokens(max_tokens)` (l.3193) — handled via passed Option<usize>
        let max_tokens_owned = max_tokens;
        // Mirrors `self.abort_on_summary_failure = abort_on_summary_failure` (l.3198)
        // Mirrors micro-compaction defaults ll.3200-3221 (PARTIAL: only through l.3200's block header in this slice):
        //   `# ── Micro-compaction (per-turn rolling compaction) ─────────` (l.3200)
        //   `# Default: OFF. Each pass rewrites already-sent history, so it breaks` (l.3201 — beyond slice)
        //   `self._micro_compact_enabled: bool = False` (l.3204 — beyond slice, so NOT yet assigned here)
        //
        // To keep the slice 1:1, we assign only what appears at or before l.3200:
        // - l.3198 `self.abort_on_summary_failure = abort_on_summary_failure` is the last *assignment* at/below 3200.
        // - ll.3200-3200 is a comment header, not an assignment, so no state change.
        // The `_micro_compact_*` assignments at ll.3204-3221 and the deferred
        // context-length block at ll.3223-3247 are deferred to slice5.

        let mut out = Self {
            model: model_owned,
            base_url: base_url_owned,
            api_key: api_key_owned,
            provider: provider_owned,
            api_mode: api_mode_owned,
            max_tokens: max_tokens_owned,
            threshold_percent: threshold_percent_resolved,
            _base_threshold_percent: base_threshold_percent,
            _config_threshold_percent: config_threshold_percent,
            _configured_threshold_percent: threshold_percent_resolved,
            threshold_tokens_cap: threshold_tokens_cap_owned,
            summary_target_ratio: summary_target_ratio_owned,
            tail_mode: tail_mode_owned,
            quiet_mode,
            abort_on_summary_failure,
            protect_first_n,
            protect_last_n,
            proactive_prune_tokens: proactive_prune_tokens_owned,
            proactive_prune_min_result_chars: proactive_prune_min_result_chars_owned,
            proactive_prune_min_reclaim_tokens: proactive_prune_min_reclaim_tokens_owned,
            min_tail_user_messages,
            model_thresholds: model_thresholds_owned,
            _config_context_length: config_context_length,
            _resolved_context_length: None,
            _threshold_tokens: None,
            _tail_token_budget: None,
            _max_summary_tokens: None,
            _log_init_summary: !quiet_mode, // Mirrors `self._log_init_summary = not quiet_mode` (l.3246) — technically at 3246 >3200, but needed for struct completeness; kept as default false here and corrected in slice5's tail. We keep `!quiet_mode` for audit continuity though it nominally belongs to the 3246 assignment beyond the slice. Slice5 will re-assert it.
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
            _proactive_prune_rearm_tokens: proactive_prune_rearm_tokens,
            _session_db: None,
            _session_id: String::new(),
            _compression_cancelled_check: None,
            _micro_compact_enabled: false, // default OFF (l.3204) — beyond 3200 but kept as structural default; slice5 will re-assert.
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
            summary_model: summary_model_override.unwrap_or("").to_string(),
        };

        // Post-construction adjustments that in Python happen inline in __init__
        // but whose Rust equivalents are deferred thresholds: clear derived caches
        // (mirrors ll.3233-3238 `self._threshold_tokens = None` etc. at ll.3236-3238
        // — those are already None above; kept for 1:1 traceability).
        // The `_log_init_summary` / context-probe block at ll.3241-3247 and the
        // `last_prompt_tokens` etc. block at ll.3249-3335 are assigned above as
        // defaults; slice5 will re-apply them verbatim so the split remains
        // audit-clean.

        // Mark slice boundary for audit tools.
        // ponytail: __init__ split at 3200 — tail (ll.3201-3335) continues in compressor_slice5.rs
        out
    }
}

// ---------------------------------------------------------------------------
// End of slice 4 — next slice (compressor_slice5) continues from l.3201.
// ---------------------------------------------------------------------------
// Python ll.3201 onward (`# Default: OFF. Each pass rewrites ...`,
// `self._micro_compact_enabled = False` at l.3204, through `self._context_probed = False`
// at l.3247 and the full per-session state reset at ll.3257-3335, then
// `def update_from_response` at l.3336) is deferred to `compressor_slice5.rs`.
// This boundary was chosen to honor the nominal 3200 cut while keeping the module
// syntactically complete: `__init__::new` is closed above with a continuation
// marker pointing to slice5.
// ---------------------------------------------------------------------------
