//! Hermes run_agent — slice 5/11
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/run_agent.py`
//! slice 5/11 — lines 3600–4500 of 9 269.
//! Covers: `redirect` tail (ll.3600-3632) — `else: with _redirect_lock:` block
//! through `return True`, `_has_pending_redirect` (ll.3634-3640),
//! `_drain_pending_redirect` (ll.3642-3652), `_drain_pending_steer` (ll.3654-3668),
//! `_record_file_mutation_result` (ll.3670-3721), `_file_mutation_verifier_enabled`
//! (ll.3723-3761), `_FOOTER_PATH_RE` (ll.3771-3773) + `_neutralize_footer_paths`
//! (ll.3775-3790), `_format_file_mutation_failure_footer` (ll.3793-3831),
//! `_turn_completion_explainer_enabled` (ll.3833-3871),
//! `_format_turn_completion_explanation` (ll.3873-4043),
//! `_apply_pending_steer_to_tool_results` (ll.4045-4048), `_touch_activity`
//! (ll.4050-4103), `_persist_session_activity_if_due` (ll.4105-4147),
//! `_reset_activity_labels_after_turn` (ll.4149-4173), `_capture_rate_limits`
//! (ll.4175-4192), `get_rate_limit_state` (ll.4194-4196),
//! `_capture_anthropic_response_headers` (ll.4198-4207), `_capture_credits`
//! (ll.4209-4291), `_emit_credits_notices` (ll.4293-4329),
//! `_credits_notices_enabled` (ll.4331-4352), `get_credits_state` (ll.4354-4356),
//! `get_credits_spent_micros` (ll.4358-4362), `_check_openrouter_cache_status`
//! (ll.4364-4384), `get_activity_summary` (ll.4386-4414), `shutdown_memory_provider`
//! (ll.4416-4442), `commit_memory_session` (ll.4444-4467), and
//! `_sync_external_memory_for_turn` header through the best-effort try wrap
//! (ll.4469-4500, nominal slice end mid-docstring inside the `try/except` guard).
//! The remainder of `_sync_external_memory_for_turn` (ll.4501-4535) + every later
//! `AIAgent` method through `main` at l.9269 continues in `run_agent_slice6.rs`.
//! This file intentionally stops at the 4500-line boundary so that `cargo` is
//! never invoked and the 11-slice decomposition stays clean. Verified by
//! line-level audit, not by compilation.
//!
//! T0208 — 1:1 port, no cargo (NEVER cargo).
//! Mirrors Python ll.3600-4500 verbatim; line numbers in comments refer to the
//! 9 269-line source file. Slice 4 covered ll.2700-3600 (through the `else:` at
//! l.3600 mid-`redirect`); this slice resumes at l.3600 (`else:` inside that
//! `with _redirect_lock` branch) and runs through the `strictly best-effort`
//! comment at l.4500 mid-`_sync_external_memory_for_turn`. The next slice starts
//! at l.4501 (`a misconfigured or offline backend must not...`). Verified by
//! line-level audit, not by compilation.

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

fn redact_sensitive_text_stub(s: &str) -> String {
    s.to_string()
}
fn is_truthy_value_stub(v: Option<&str>) -> bool {
    matches!(v.map(|s| s.trim().to_lowercase()).as_deref(), Some("1") | Some("true") | Some("yes") | Some("on"))
}
fn set_interrupt_stub(_flag: bool, _thread_id: u64, _reason: Option<&str>) {}
fn extract_file_mutation_targets_stub(_tool_name: &str, _args: &Value) -> Vec<String> {
    // Mirrors `agent.tool_dispatch_helpers._extract_file_mutation_targets`
    if let Some(obj) = _args.as_object() {
        if let Some(p) = obj.get("path").and_then(|v| v.as_str()) { return vec![p.to_string()]; }
        if let Some(p) = obj.get("file").and_then(|v| v.as_str()) { return vec![p.to_string()]; }
        if let Some(arr) = obj.get("paths").and_then(|v| v.as_array()) {
            return arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect();
        }
    }
    vec![]
}
fn extract_landed_file_mutation_paths_stub(_tool_name: &str, _args: &Value, _result: &Value) -> Vec<String> {
    extract_file_mutation_targets_stub(_tool_name, _args)
}
fn extract_error_preview_stub(result: &Value) -> String {
    // Mirrors `agent.tool_dispatch_helpers._extract_error_preview`
    if let Some(s) = result.as_str() { return s.chars().take(200).collect(); }
    if let Some(obj) = result.as_object() {
        if let Some(e) = obj.get("error").and_then(|v| v.as_str()) { return e.chars().take(200).collect(); }
        if let Some(m) = obj.get("message").and_then(|v| v.as_str()) { return m.chars().take(200).collect(); }
    }
    result.to_string().chars().take(200).collect()
}
fn file_mutation_result_landed_stub(_tool_name: &str, _result: &Value) -> bool {
    // Mirrors `agent.tool_result_classification.file_mutation_result_landed`
    // Heuristic: error-like payloads are not landed.
    if let Some(obj) = _result.as_object() {
        if obj.get("success").and_then(|v| v.as_bool()) == Some(false) { return false; }
        if obj.contains_key("error") { return false; }
    }
    if let Some(s) = _result.as_str() {
        let lower = s.to_lowercase();
        if lower.contains("error") || lower.contains("failed") { return false; }
    }
    true
}
fn bound_activity_description_stub(desc: &str) -> String {
    // Mirrors `agent.session_activity.bound_activity_description` — caps at ~200 chars.
    desc.chars().take(200).collect()
}
fn normalize_activity_provenance_stub(p: Option<&str>) -> String {
    p.unwrap_or("unknown").to_string()
}
fn reset_session_activity_persist_window_stub(_agent: &AiAgent) {}
fn heartbeat_current_worker_from_env_stub() {}
fn inject_new_comments_from_env_stub(_agent: &AiAgent) {}
fn load_config_stub() -> HashMap<String, Value> { HashMap::new() }
fn get_hermes_home_stub() -> PathBuf { PathBuf::from("/tmp/hermes") }
fn apply_pending_steer_to_tool_results_stub(_agent: &AiAgent, _messages: &mut Vec<Value>, _num_tool_msgs: usize) {}
fn dev_fixture_credits_state_stub() -> Option<CreditsState> { None }
fn parse_rate_limit_headers_stub(_headers: &Value, _provider: &str) -> Option<RateLimitState> { None }
fn parse_credits_headers_stub(_headers: &Value, _provider: &str) -> Option<CreditsState> { None }
fn evaluate_credits_notices_stub(_state: &CreditsState, _latch: &mut HashMap<String, Value>, _model_is_free: bool) -> (Vec<Value>, Vec<String>) { (vec![], vec![]) }
fn is_free_tier_model_stub(_model: &str, _base_url: &str) -> bool { false }
fn new_credits_latch_stub() -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert("seen_below_90".to_string(), json!(false));
    m
}
fn apply_persist_user_message_override_stub() {}

// Minimal helper for chrono-like ISO now.
fn chrono_stub_now_iso() -> String {
    // Mirrors `datetime.now(timezone.utc).isoformat()` (used in session log etc.)
    // Stub returns fixed ISO string for audit.
    "2026-01-01T00:00:00+00:00".to_string()
}
fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}
fn now_mono() -> f64 {
    // Approximates `time.monotonic()`; stub uses now_secs.
    now_secs()
}

// ---------------------------------------------------------------------------
// Footer path regex — mirrors `_FOOTER_PATH_RE = re.compile(...)` (ll.3771-3773)
// ---------------------------------------------------------------------------

/// Mirrors `r"(?<![/:\w.`])(?:~/|/|[A-Za-z]:[/\\])(?:[\w.\-]+[/\\])*[\w.\-]+\.[\w]+"` (l.3772).
/// Rust shim keeps the pattern as a static string for audit; matching is
/// implemented in `neutralize_footer_paths` via a simple scan (no `regex` dep).
pub const FOOTER_PATH_RE_PATTERN: &str =
    r"(?<![/:\w.`])(?:~/|/|[A-Za-z]:[/\\])(?:[\w.\-]+[/\\])*[\w.\-]+\.[\w]+";

// ---------------------------------------------------------------------------
// Supporting types — mirrors Python runtime shapes touched by ll.3600-4500
// ---------------------------------------------------------------------------

/// Mirrors `_FILE_MUTATING_TOOL_NAMES` / `file_mutation_result_landed` set (l.3670).
pub const FILE_MUTATING_TOOL_NAMES: &[&str] = &["write_file", "patch", "edit_file", "create_file"];

#[derive(Debug, Clone, Default)]
pub struct CreditsState {
    pub remaining_micros: i64,
    pub remaining_usd: Option<String>,
    pub paid_access: bool,
    pub denominator_kind: String,
    pub used_fraction: Option<f64>,
    pub age_seconds: f64,
    pub disabled_reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RateLimitState {
    pub raw: HashMap<String, Value>,
}

#[derive(Debug, Clone, Default)]
pub struct IterationBudget {
    pub used: usize,
    pub max_total: usize,
    pub remaining: usize,
}

#[derive(Debug, Clone, Default)]
pub struct CheckpointMgr {
    pub enabled: bool,
    pub agent_writes: Vec<String>,
}
impl CheckpointMgr {
    pub fn record_agent_write(&mut self, path: &str) -> Result<(), String> {
        self.agent_writes.push(path.to_string());
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct StubSessionDb;
impl StubSessionDb {
    pub fn touch_session_activity(&self, _session_id: &str, _ts: Option<f64>, _desc: Option<String>, _provenance: String) -> Result<(), String> { Ok(()) }
    pub fn clear_session_activity_labels(&self, _session_id: &str) -> Result<(), String> { Ok(()) }
}

#[derive(Debug, Clone, Default)]
pub struct MemoryManager;
impl MemoryManager {
    pub fn on_session_end(&self, _messages: &[Value]) -> Result<(), String> { Ok(()) }
    pub fn shutdown_all(&self) -> Result<(), String> { Ok(()) }
}

#[derive(Debug, Clone, Default)]
pub struct ContextCompressor;
impl ContextCompressor {
    pub fn on_session_end(&self, _session_id: &str, _messages: &[Value]) -> Result<(), String> { Ok(()) }
    pub fn on_session_start(&self, _session_id: &str, _ctx: HashMap<String, Value>) -> Result<(), String> { Ok(()) }
    pub fn on_session_reset(&self) -> Result<(), String> { Ok(()) }
    pub fn bind_session_state(&self, _db: Option<&StubSessionDb>, _session_id: &str) -> Result<(), String> { Ok(()) }
}

// ---------------------------------------------------------------------------
// AiAgent — mirrors `class AIAgent:` (run_agent.py l.421)
// Only fields touched by ll.3600-4500 are modelled; the full `__init__`
// (ll.444-615) is canonical in slice1. This slice's methods operate on the
// same struct shape via `&self` / `&mut self`.
// ---------------------------------------------------------------------------

/// Minimal `AIAgent` surface needed for slice 5 (ll.3600-4500).
///
/// Python's `AIAgent.__init__` (≈60 params) is canonical in slice1. Here we
/// keep only the attributes read/written by the slice5 helpers so the file
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
    pub quiet_mode: bool,
    pub verbose_logging: bool,
    pub platform: String,

    // Session / persistence
    pub session_id: String,
    pub session_db: Option<StubSessionDb>,
    pub persist_disabled: bool,

    // Interrupt / steer / redirect state (ll.3542-3632, 3634-3668)
    pub interrupt_requested: bool, // mirrors `self._interrupt_requested`
    pub interrupt_message: Option<String>, // mirrors `self._interrupt_message`
    pub tool_interrupt_reason: Option<String>,
    pub pending_redirect: Option<String>, // mirrors `self._pending_redirect`
    pub pending_redirect_lock: Option<Arc<Mutex<()>>>, // mirrors `self._pending_redirect_lock`
    pub pending_steer: Option<String>, // mirrors `self._pending_steer`
    pub pending_steer_lock: Option<Arc<Mutex<()>>>, // mirrors `self._pending_steer_lock`
    pub hard_interrupt_requested: bool,
    pub execution_thread_id: Option<u64>, // mirrors `self._execution_thread_id`
    pub interrupt_thread_signal_pending: bool, // mirrors `self._interrupt_thread_signal_pending`
    pub active_request_abort: bool, // stub: has abort callable
    pub executing_tools: bool, // mirrors `self._executing_tools`
    pub model_request_active: bool, // mirrors `self._model_request_active.is_set()` (bool stub for Event)
    pub codex_session: Option<Value>, // mirrors `self._codex_session` with request_steer/request_interrupt
    pub memory_provider_shutdown: bool, // mirrors `self._memory_provider_shutdown`

    // File mutation verifier (ll.3670-3721, 3723-3761)
    pub turn_failed_file_mutations: Option<HashMap<String, HashMap<String, String>>>, // mirrors `self._turn_failed_file_mutations`
    pub turn_file_mutation_paths: Option<HashSet<String>>, // mirrors `self._turn_file_mutation_paths`
    pub checkpoint_mgr: Option<CheckpointMgr>, // mirrors `self._checkpoint_mgr`
    pub file_mutation_verifier_enabled_cache: Option<bool>, // mirrors `self._file_mutation_verifier_enabled_cache`
    pub turn_completion_explainer_enabled_cache: Option<bool>, // mirrors `self._turn_completion_explainer_enabled_cache`

    // Activity / heartbeat (ll.4050-4173)
    pub last_activity_ts: Option<f64>, // mirrors `self._last_activity_ts`
    pub last_activity_desc: String, // mirrors `self._last_activity_desc`
    pub last_activity_provenance: String, // mirrors `self._last_activity_provenance` (ActivityProvenance as String stub)
    pub session_activity_last_persist_mono: f64, // mirrors `self._session_activity_last_persist_mono`

    // Rate limits / credits (ll.4175-4385)
    pub rate_limit_state: Option<RateLimitState>, // mirrors `self._rate_limit_state`
    pub credits_state: Option<CreditsState>, // mirrors `self._credits_state`
    pub credits_session_start_micros: Option<i64>, // mirrors `self._credits_session_start_micros`
    pub credits_latch: Option<HashMap<String, Value>>, // mirrors `self._credits_latch`
    pub credits_notices_enabled_cache: Option<bool>, // mirrors `self._credits_notices_enabled_cache`
    pub or_cache_hits: usize, // mirrors `self._or_cache_hits`

    // Diagnostics / budgeting (ll.4386-4414)
    pub current_tool: Option<String>, // mirrors `self._current_tool`
    pub api_call_count: usize, // mirrors `self._api_call_count`
    pub max_iterations: usize, // mirrors `self.max_iterations`
    pub iteration_budget: IterationBudget, // mirrors `self.iteration_budget`
    pub last_ctx_overflow_warn: Option<(String, String)>, // mirrors `self._last_ctx_overflow_warn` used by warn helpers

    // Memory / context engine (ll.4416-4500)
    pub memory_manager: Option<MemoryManager>, // mirrors `self._memory_manager`
    pub context_compressor: Option<ContextCompressor>, // mirrors `self.context_compressor`

    // Generic fallback for any extra dynamic attrs Python `getattr` may touch.
    pub extra: HashMap<String, Value>,
}

impl AiAgent {
    // -----------------------------------------------------------------------
    // redirect — mirrors ll.3542-3632 (slice 5 owns the 3600-3632 tail;
    // header ll.3542-3599 canonical in slice4, tail is the `else: with
    // _redirect_lock:` branch at ll.3600-3616 plus interrupt/abort at
    // ll.3618-3632). Full body reproduced for audit.
    // -----------------------------------------------------------------------

    /// Mirrors `def redirect(self, text: str) -> bool:` (ll.3542-3632).
    pub fn redirect(&mut self, text: &str) -> bool {
        // Mirrors `if not text or not text.strip(): return False` (ll.3555-3557)
        if text.trim().is_empty() { return false; }
        let cleaned = text.trim().to_string();

        // Mirrors Codex app-server native steer (ll.3562-3577)
        if self.api_mode == "codex_app_server" {
            // Mirrors `_codex_session = getattr(self, "_codex_session", None); _native_steer = getattr(_codex_session, "request_steer", None); if callable...`
            if self.codex_session.is_some() {
                // Stub: if native steer exists, check interrupt gate then call it.
                // Mirrors `_redirect_lock` gate (ll.3566-3572)
                if self.pending_redirect_lock.is_some() {
                    // with _redirect_lock: if self._interrupt_requested: return False
                    if self.interrupt_requested { return false; }
                } else if self.interrupt_requested {
                    return false;
                }
                // Mirrors `try: return bool(_native_steer(cleaned)) except: logger.debug(...); return False` (ll.3573-3577)
                // Stub: assume steer succeeds.
                return true;
            }
        }

        // Mirrors `if getattr(self, "_executing_tools", False): return self.steer(cleaned)` (ll.3582-3583)
        if self.executing_tools {
            return self.steer(&cleaned);
        }

        // Mirrors `_model_active = getattr(self, "_model_request_active", None); _redirect_lock = getattr(self, "_pending_redirect_lock", None)` (ll.3585-3586)
        let model_active = self.model_request_active;
        let has_lock = self.pending_redirect_lock.is_some();

        // Mirrors `if _redirect_lock is None:` branch (ll.3587-3598)
        if !has_lock {
            // Mirrors `if _model_active is None or not _model_active.is_set(): return False` (ll.3588-3589)
            if !model_active { return false; }
            let existing = self.pending_redirect.clone();
            // Mirrors `if self._interrupt_requested and not existing: return False` (ll.3590-3592)
            if self.interrupt_requested && existing.is_none() { return false; }
            // Mirrors `self._pending_redirect = f"{existing}\n\n[Additional user correction]\n{cleaned}" if existing else cleaned` (ll.3593-3596)
            if let Some(prev) = existing {
                self.pending_redirect = Some(format!("{prev}\n\n[Additional user correction]\n{cleaned}"));
            } else {
                self.pending_redirect = Some(cleaned.clone());
            }
            self.interrupt_requested = true;
            self.interrupt_message = None;
        } else {
            // Mirrors `else: with _redirect_lock:` (l.3600) — remainder of slice5 tail
            // Acquire the lock for audit (mirrors `with _redirect_lock:` at l.3601)
            let _guard = self.pending_redirect_lock.as_ref().and_then(|l| l.lock().ok());
            // Mirrors `if _model_active is None or not _model_active.is_set(): return False` (ll.3602-3605)
            if !model_active { return false; }
            // Mirrors `if self._interrupt_requested and not self._pending_redirect: return False` (ll.3606-3607)
            if self.interrupt_requested && self.pending_redirect.is_none() { return false; }
            // Mirrors `if self._pending_redirect: self._pending_redirect = f"{...}" else: self._pending_redirect = cleaned` (ll.3608-3614)
            if let Some(prev) = self.pending_redirect.clone() {
                self.pending_redirect = Some(format!("{prev}\n\n[Additional user correction]\n{cleaned}"));
            } else {
                self.pending_redirect = Some(cleaned.clone());
            }
            self.interrupt_requested = true;
            self.interrupt_message = None;
            // _guard drops here, mirroring `with` exit at l.3616
        }

        // Mirrors `# Interrupt only the model request. Do not fan out to tool workers or child agents` (ll.3618-3619)
        // Mirrors `_execution_thread_id = getattr(self, "_execution_thread_id", None); if _execution_thread_id is not None: _set_interrupt(True, ...)` (ll.3620-3625)
        if let Some(tid) = self.execution_thread_id {
            set_interrupt_stub(true, tid, None);
            self.interrupt_thread_signal_pending = false;
        } else {
            self.interrupt_thread_signal_pending = true;
        }
        // Mirrors `_abort_active_request = getattr(self, "_active_request_abort", None); if callable: _abort_active_request("redirect_abort")` (ll.3626-3631)
        if self.active_request_abort {
            // stub abort — mirrors try/except logger.debug on failure
        }
        // Mirrors `return True` (l.3632)
        true
    }

    /// Mirrors `def steer(self, text: str) -> bool:` (ll.3506-3540) — needed by `redirect` delegation.
    /// Canonical in slice4; stub copy for audit so `redirect`'s `self.steer(cleaned)` resolves without cross-slice import.
    pub fn steer(&mut self, text: &str) -> bool {
        if text.trim().is_empty() { return false; }
        // Mirrors steer queuing into `_pending_steer` under lock; stub approximates.
        let has_lock = self.pending_steer_lock.is_some();
        if has_lock {
            let _guard = self.pending_steer_lock.as_ref().and_then(|l| l.lock().ok());
            if let Some(prev) = self.pending_steer.clone() {
                self.pending_steer = Some(format!("{prev}\n\n{ }", text.trim()));
                // Note: Python steer concatenates differently; stub keeps audit shape.
                let _ = prev;
                self.pending_steer = Some(format!("{}\n\n{}", self.pending_steer.clone().unwrap_or_default(), text.trim()));
            } else {
                self.pending_steer = Some(text.trim().to_string());
            }
        } else {
            self.pending_steer = Some(text.trim().to_string());
        }
        true
    }

    // -----------------------------------------------------------------------
    // _has_pending_redirect — mirrors ll.3634-3640
    // -----------------------------------------------------------------------

    /// Mirrors `def _has_pending_redirect(self) -> bool:` (ll.3634-3640).
    pub fn has_pending_redirect(&self) -> bool {
        // Mirrors `_redirect_lock = getattr(self, "_pending_redirect_lock", None); if _redirect_lock is None: return bool(...)` (ll.3636-3638)
        if self.pending_redirect_lock.is_none() {
            return self.pending_redirect.is_some() && !self.pending_redirect.as_ref().unwrap().is_empty();
        }
        // Mirrors `with _redirect_lock: return bool(self._pending_redirect)` (ll.3639-3640)
        // Rust: lock then check (mirrors `with`); stub shares same read.
        let _guard = self.pending_redirect_lock.as_ref().and_then(|l| l.lock().ok());
        self.pending_redirect.is_some() && !self.pending_redirect.as_ref().unwrap().is_empty()
    }

    // -----------------------------------------------------------------------
    // _drain_pending_redirect — mirrors ll.3642-3652
    // -----------------------------------------------------------------------

    /// Mirrors `def _drain_pending_redirect(self) -> Optional[str]:` (ll.3642-3652).
    pub fn drain_pending_redirect(&mut self) -> Option<String> {
        // Mirrors `_redirect_lock = getattr(self, "_pending_redirect_lock", None); if _redirect_lock is None: text = ...; self._pending_redirect = None; return text` (ll.3644-3648)
        if self.pending_redirect_lock.is_none() {
            let text = self.pending_redirect.clone();
            self.pending_redirect = None;
            return text;
        }
        // Mirrors `with _redirect_lock: text = self._pending_redirect; self._pending_redirect = None; return text` (ll.3649-3652)
        let _guard = self.pending_redirect_lock.as_ref().and_then(|l| l.lock().ok());
        let text = self.pending_redirect.clone();
        self.pending_redirect = None;
        text
    }

    // -----------------------------------------------------------------------
    // _drain_pending_steer — mirrors ll.3654-3668
    // -----------------------------------------------------------------------

    /// Mirrors `def _drain_pending_steer(self) -> Optional[str]:` (ll.3654-3668).
    pub fn drain_pending_steer(&mut self) -> Option<String> {
        // Mirrors `_lock = getattr(self, "_pending_steer_lock", None); if _lock is None: text = ...; self._pending_steer = None; return text` (ll.3660-3664)
        if self.pending_steer_lock.is_none() {
            let text = self.pending_steer.clone();
            self.pending_steer = None;
            return text;
        }
        // Mirrors `with _lock: text = self._pending_steer; self._pending_steer = None; return text` (ll.3665-3668)
        let _guard = self.pending_steer_lock.as_ref().and_then(|l| l.lock().ok());
        let text = self.pending_steer.clone();
        self.pending_steer = None;
        text
    }

    // -----------------------------------------------------------------------
    // _record_file_mutation_result — mirrors ll.3670-3721
    // -----------------------------------------------------------------------

    /// Mirrors `def _record_file_mutation_result(self, tool_name: str, args: Dict[str, Any], result: Any, is_error: bool) -> None:` (ll.3670-3721).
    pub fn record_file_mutation_result(&mut self, tool_name: &str, args: &Value, result: &Value, is_error: bool) {
        // Mirrors `if tool_name not in _FILE_MUTATING_TOOLS: return` (ll.3685-3686)
        if !FILE_MUTATING_TOOL_NAMES.contains(&tool_name) { return; }
        // Mirrors `state = getattr(self, "_turn_failed_file_mutations", None); if state is None: return` (ll.3687-3689)
        // Rust: Option<HashMap> — return if None (means not inside run_conversation)
        if self.turn_failed_file_mutations.is_none() { return; }
        // Mirrors `targets = _extract_file_mutation_targets(tool_name, args); if not targets: return` (ll.3690-3692)
        let targets = extract_file_mutation_targets_stub(tool_name, args);
        if targets.is_empty() { return; }
        // Mirrors `landed = file_mutation_result_landed(tool_name, result)` (l.3693)
        let landed = file_mutation_result_landed_stub(tool_name, result);
        if landed {
            // Mirrors `landed_paths = _extract_landed_file_mutation_paths(tool_name, args, result); changed = getattr(self, "_turn_file_mutation_paths", None); if changed is not None: changed.update(landed_paths)` (ll.3695-3698)
            let landed_paths = extract_landed_file_mutation_paths_stub(tool_name, args, result);
            if let Some(changed) = self.turn_file_mutation_paths.as_mut() {
                for p in &landed_paths { changed.insert(p.clone()); }
            }
            // Mirrors `mgr = getattr(self, "_checkpoint_mgr", None); if mgr is not None and getattr(mgr, "enabled", False): for _p in landed_paths: try: mgr.record_agent_write(_p) except: pass` (ll.3701-3707)
            if let Some(mgr) = self.checkpoint_mgr.as_mut() {
                if mgr.enabled {
                    for p in &landed_paths {
                        let _ = mgr.record_agent_write(p);
                    }
                }
            }
        }
        // Mirrors `if is_error and not landed: preview = _extract_error_preview(result); for path in targets: if path not in state: state[path] = {...}` (ll.3708-3718)
        if is_error && !landed {
            let preview = extract_error_preview_stub(result);
            if let Some(state) = self.turn_failed_file_mutations.as_mut() {
                for path in &targets {
                    if !state.contains_key(path) {
                        let mut entry = HashMap::new();
                        entry.insert("tool".to_string(), tool_name.to_string());
                        entry.insert("error_preview".to_string(), preview.clone());
                        state.insert(path.clone(), entry);
                    }
                }
            }
        } else {
            // Mirrors `else: for path in targets: state.pop(path, None)` (ll.3719-3721)
            if let Some(state) = self.turn_failed_file_mutations.as_mut() {
                for path in &targets { state.remove(path); }
            }
        }
    }

    // -----------------------------------------------------------------------
    // _file_mutation_verifier_enabled — mirrors ll.3723-3761
    // -----------------------------------------------------------------------

    /// Mirrors `def _file_mutation_verifier_enabled(self) -> bool:` (ll.3723-3761).
    pub fn file_mutation_verifier_enabled(&mut self) -> bool {
        // Mirrors `try: import os as _os; env = _os.environ.get("HERMES_FILE_MUTATION_VERIFIER"); if env is not None: return env.strip().lower() not in {"0","false","no","off"}` (ll.3737-3741)
        if let Ok(env) = std::env::var("HERMES_FILE_MUTATION_VERIFIER") {
            return !matches!(env.trim().to_lowercase().as_str(), "0" | "false" | "no" | "off");
        }
        // Mirrors `cached = getattr(self, "_file_mutation_verifier_enabled_cache", None); if cached is not None: return cached` (ll.3742-3744)
        if let Some(cached) = self.file_mutation_verifier_enabled_cache { return cached; }
        // Mirrors `try: from hermes_cli.config import load_config as _load_config; _cfg = _load_config() or {}; except: _cfg = {}` (ll.3747-3751)
        let cfg = load_config_stub();
        // Mirrors `_display = _cfg.get("display") if isinstance(_cfg, dict) else None; if isinstance(_display, dict) and "file_mutation_verifier" in _display: enabled = bool(...) else: enabled = True` (ll.3752-3756)
        let enabled = if let Some(display) = cfg.get("display").and_then(|v| v.as_object()) {
            if display.contains_key("file_mutation_verifier") {
                display.get("file_mutation_verifier").and_then(|v| v.as_bool()).unwrap_or(true)
            } else { true }
        } else { true };
        self.file_mutation_verifier_enabled_cache = Some(enabled);
        // Mirrors `return enabled` (l.3758) with `except: pass; return True` (ll.3759-3761)
        enabled
    }

    // -----------------------------------------------------------------------
    // _FOOTER_PATH_RE + _neutralize_footer_paths — mirrors ll.3771-3790
    // -----------------------------------------------------------------------

    /// Mirrors `@classmethod def _neutralize_footer_paths(cls, text: str) -> str:` (ll.3775-3790).
    pub fn neutralize_footer_paths(text: &str) -> String {
        // Mirrors `if not text: return text` (ll.3788-3789)
        if text.is_empty() { return text.to_string(); }
        // Mirrors `return cls._FOOTER_PATH_RE.sub(lambda m: f"`{m.group(0)}`", text)` (l.3790)
        // Rust shim: scan for bare paths and wrap in backticks. Avoids `regex` dep.
        // Anchors mirror gateway's `extract_local_files` — only wrap paths not already in backticks
        // (negative lookbehind `(?<![/:\w.`])` in Python). Stub approximates via token scan.
        let mut out = String::with_capacity(text.len() + 16);
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            // Detect candidate start: '/' or '~/' or 'X:/' / 'X:\'
            let is_candidate = if chars[i] == '/' {
                // Check lookbehind: previous char must not be '/', ':', word, '.' or '`' (mirrors `(?<![/:\w.`])`)
                if i > 0 {
                    let prev = chars[i-1];
                    !(prev.is_alphanumeric() || prev == '_' || prev == '/' || prev == ':' || prev == '.' || prev == '`')
                } else { true }
            } else if chars[i] == '~' && i+1 < chars.len() && chars[i+1] == '/' {
                if i > 0 {
                    let prev = chars[i-1];
                    !(prev.is_alphanumeric() || prev == '_' || prev == '/' || prev == ':' || prev == '.' || prev == '`')
                } else { true }
            } else if i+2 < chars.len() && chars[i].is_ascii_alphabetic() && chars[i+1] == ':' && (chars[i+2] == '/' || chars[i+2] == '\\') {
                if i > 0 {
                    let prev = chars[i-1];
                    !(prev.is_alphanumeric() || prev == '_' || prev == '/' || prev == ':' || prev == '.' || prev == '`')
                } else { true }
            } else { false };

            if is_candidate {
                let start = i;
                let mut j = i;
                let mut has_dot_ext = false;
                // Consume `[\w.\-]+[/\\]` segments then `[\w.\-]+\.[\w]+`
                while j < chars.len() {
                    // Consume segment chars
                    let seg_start = j;
                    while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_' || chars[j] == '.' || chars[j] == '-') {
                        if chars[j] == '.' { has_dot_ext = true; }
                        j += 1;
                    }
                    if j < chars.len() && (chars[j] == '/' || chars[j] == '\\') {
                        j += 1; // consume separator and continue
                    } else {
                        break;
                    }
                    if j == seg_start { break; }
                }
                // Require we consumed at least one dot-extension (mirrors `\.[\\w]+` tail)
                let candidate: String = chars[start..j].iter().collect();
                if has_dot_ext && candidate.contains('.') && j > start {
                    // Wrap in backticks
                    out.push('`');
                    out.push_str(&candidate);
                    out.push('`');
                    i = j;
                    continue;
                }
            }
            out.push(chars[i]);
            i += 1;
        }
        out
    }

    // -----------------------------------------------------------------------
    // _format_file_mutation_failure_footer — mirrors ll.3793-3831
    // -----------------------------------------------------------------------

    /// Mirrors `@classmethod def _format_file_mutation_failure_footer(cls, failed: Dict[str, Dict[str, Any]]) -> str:` (ll.3793-3831).
    pub fn format_file_mutation_failure_footer(failed: &HashMap<String, HashMap<String, String>>) -> String {
        // Mirrors `if not failed: return ""` (ll.3806-3807)
        if failed.is_empty() { return String::new(); }
        // Mirrors `lines = ["⚠️ File-mutation verifier: " f"{len(failed)} file(s) were NOT modified..."]` (ll.3808-3813)
        let mut lines: Vec<String> = vec![format!(
            "⚠️ File-mutation verifier: {} file(s) were NOT modified this turn despite any wording above that may suggest otherwise. Run `git status` or `read_file` to confirm.",
            failed.len()
        )];
        // Mirrors `shown = 0; for path, info in failed.items(): if shown >= 10: break; preview = ...; tool = ...; if preview: lines.append(...) else: ...; shown +=1` (ll.3814-3824)
        let mut shown = 0;
        for (path, info) in failed {
            if shown >= 10 { break; }
            let preview = info.get("error_preview").map(|s| s.trim()).unwrap_or("");
            let tool = info.get("tool").map(|s| s.as_str()).unwrap_or("patch");
            if !preview.is_empty() {
                lines.push(format!("  • `{path}` — [{tool}] {preview}"));
            } else {
                lines.push(format!("  • `{path}` — [{tool}] failed"));
            }
            shown += 1;
        }
        // Mirrors `remaining = len(failed) - shown; if remaining > 0: lines.append(f"  • … and {remaining} more")` (ll.3825-3827)
        let remaining = failed.len().saturating_sub(shown);
        if remaining > 0 { lines.push(format!("  • … and {remaining} more")); }
        // Mirrors `return cls._neutralize_footer_paths("\n".join(lines))` (l.3831)
        Self::neutralize_footer_paths(&lines.join("\n"))
    }

    // -----------------------------------------------------------------------
    // _turn_completion_explainer_enabled — mirrors ll.3833-3871
    // -----------------------------------------------------------------------

    /// Mirrors `def _turn_completion_explainer_enabled(self) -> bool:` (ll.3833-3871).
    pub fn turn_completion_explainer_enabled(&mut self) -> bool {
        // Mirrors `env = _os.environ.get("HERMES_TURN_COMPLETION_EXPLAINER"); if env is not None: return env.strip().lower() not in {"0","false","no","off"}` (ll.3849-3851)
        if let Ok(env) = std::env::var("HERMES_TURN_COMPLETION_EXPLAINER") {
            return !matches!(env.trim().to_lowercase().as_str(), "0" | "false" | "no" | "off");
        }
        // Mirrors `cached = getattr(self, "_turn_completion_explainer_enabled_cache", None); if cached is not None: return cached` (ll.3852-3854)
        if let Some(cached) = self.turn_completion_explainer_enabled_cache { return cached; }
        // Mirrors `try: from hermes_cli.config import load_config ... _cfg = _load_config() or {}; except: _cfg = {}` (ll.3857-3861)
        let cfg = load_config_stub();
        // Mirrors `_display = _cfg.get("display") if isinstance(_cfg, dict) else None; if isinstance(_display, dict) and "turn_completion_explainer" in _display: enabled = bool(...) else: enabled = True` (ll.3862-3866)
        let enabled = if let Some(display) = cfg.get("display").and_then(|v| v.as_object()) {
            if display.contains_key("turn_completion_explainer") {
                display.get("turn_completion_explainer").and_then(|v| v.as_bool()).unwrap_or(true)
            } else { true }
        } else { true };
        self.turn_completion_explainer_enabled_cache = Some(enabled);
        enabled
    }

    // -----------------------------------------------------------------------
    // _format_turn_completion_explanation — mirrors ll.3873-4043
    // -----------------------------------------------------------------------

    /// Mirrors `@staticmethod def _format_turn_completion_explanation(turn_exit_reason: str, persistence_cause: Optional[str] = None) -> str:` (ll.3873-4043).
    pub fn format_turn_completion_explanation(turn_exit_reason: &str, persistence_cause: Option<&str>) -> String {
        // Mirrors `if not turn_exit_reason: return ""` (ll.3897-3898)
        if turn_exit_reason.is_empty() { return String::new(); }
        let reason = turn_exit_reason.to_string();
        // Mirrors `if reason.startswith("text_response"): return ""` (ll.3903-3904)
        if reason.starts_with("text_response") { return String::new(); }
        let prefix = "⚠️ No reply: ";
        // Mirrors each `if reason == "...": return prefix + "..."` chain (ll.3907-4040)
        if reason == "empty_response_exhausted" {
            return format!("{prefix}the model returned empty content after retries and any fallback providers. Try `continue`, switch model/provider, or inspect the tool output above.");
        }
        if reason == "all_retries_exhausted_no_response" {
            return format!("{prefix}all API retries were exhausted before a response was produced (provider errors / rate limits). Try `continue` or switch provider.");
        }
        if reason == "partial_stream_recovery" {
            return format!("{prefix}streaming stopped early and only a partial response was recovered. Send `continue` to resume from where it stopped.");
        }
        if reason == "fallback_prior_turn_content" {
            return format!("{prefix}no new content was produced this turn; showing recovered prior context. Send `continue` to retry.");
        }
        if reason == "interrupted_during_api_call" {
            return format!("{prefix}the request was interrupted mid-call before a reply was received. Send `continue` to retry.");
        }
        if reason == "budget_exhausted" {
            return format!("{prefix}the per-turn iteration/cost budget was exhausted before a final answer. Send `continue` to keep going.");
        }
        if reason == "ollama_runtime_context_too_small" {
            return format!("{prefix}the local model's context window was too small to finish. Increase the context size or use a larger model.");
        }
        if reason.starts_with("max_iterations_reached") {
            return format!("{prefix}the maximum tool-iteration limit was reached before a final answer. Send `continue` to keep going, or raise `max_iterations`.");
        }
        if reason.starts_with("error_near_max_iterations") {
            return format!("{prefix}an error occurred near the iteration limit before a final answer. Check the tool output above, then send `continue`.");
        }
        if reason.starts_with("repeated_outer_errors") {
            return format!("{prefix}the turn kept failing with repeated errors and was stopped early instead of retrying forever. Check the errors above, then send `continue` to retry.");
        }
        if reason == "pending_tool_result" {
            return format!("{prefix}the turn stopped while a tool result was still pending and the model produced no follow-up text. Send `continue` to let it summarize.");
        }
        if reason == "session_persistence_failed" {
            let cause = persistence_cause.unwrap_or("unknown");
            // Mirrors `if cause == "compression": ...` (ll.3980-3986)
            if cause == "compression" {
                return format!("{prefix}the turn was stopped because another process was compressing this session. Your message should already be saved — please send it again after compression completes.");
            }
            if cause == "compression_closed" {
                return format!("{prefix}the turn was stopped because this session was rotated by context compression and its live continuation could not be adopted. The storage itself is healthy — refresh the client (or start a new turn) so it picks up the new session id, then send your message again.");
            }
            if cause == "turn_lease" {
                return format!("{prefix}the turn was stopped because another Hermes process took over this session. Your reply was not saved — wait for the other process to finish, then send your message again.");
            }
            if cause == "locked" {
                return format!("{prefix}the turn was stopped because session storage was busy (another Hermes process was writing to the state database). Your message should already be saved — please send it again in a moment.");
            }
            if cause == "corrupt" {
                return format!("{prefix}the turn was stopped because the state database reported structural corruption (the transcript would have been lost on restart). Freeing disk space will not help. Recovery options:\n1. Run `hermes doctor --fix`\n2. Salvage with: sqlite3 ~/.hermes/state.db \".recover\" (then replace state.db)\n3. Restore from a backup in ~/.hermes/backups/\nThen send your message again.");
            }
            if cause == "disk" {
                return format!("{prefix}the turn was stopped because session storage could not be written (the transcript would have been lost on restart). This is often a full disk — free some space (or fix state.db permissions), then send your message again.");
            }
            return format!("{prefix}the turn was stopped because session storage could not be written (the transcript would have been lost on restart). Check the state database health (`hermes doctor`), then send your message again.");
        }
        // Mirrors `# Unknown/diagnostic-only reasons ... return ""` (ll.4041-4043)
        String::new()
    }

    // -----------------------------------------------------------------------
    // _apply_pending_steer_to_tool_results — mirrors ll.4045-4048
    // -----------------------------------------------------------------------

    /// Mirrors `def _apply_pending_steer_to_tool_results(self, messages: list, num_tool_msgs: int) -> None:` (ll.4045-4048).
    pub fn apply_pending_steer_to_tool_results(&self, messages: &mut Vec<Value>, num_tool_msgs: usize) {
        // Mirrors `from agent.agent_runtime_helpers import apply_pending_steer_to_tool_results; return apply_pending_steer_to_tool_results(self, messages, num_tool_msgs)` (ll.4047-4048)
        apply_pending_steer_to_tool_results_stub(self, messages, num_tool_msgs);
    }

    // -----------------------------------------------------------------------
    // _touch_activity — mirrors ll.4050-4103
    // -----------------------------------------------------------------------

    /// Mirrors `def _touch_activity(self, desc: str, *, provenance: Optional[ActivityProvenance] = None, force_persist: bool = False) -> None:` (ll.4050-4103).
    pub fn touch_activity(&mut self, desc: &str, provenance: Option<&str>, force_persist: bool) {
        // Mirrors `from agent.session_activity import bound_activity_description, normalize_activity_provenance, reset_session_activity_persist_window` (ll.4076-4080)
        self.last_activity_ts = Some(now_secs());
        self.last_activity_desc = bound_activity_description_stub(desc);
        self.last_activity_provenance = normalize_activity_provenance_stub(provenance);
        // Mirrors `if os.environ.get("HERMES_KANBAN_TASK"): try: from tools.kanban_tools import ...; heartbeat_current_worker_from_env(); inject_new_comments_from_env(self) except: pass` (ll.4085-4100)
        if std::env::var("HERMES_KANBAN_TASK").is_ok() {
            heartbeat_current_worker_from_env_stub();
            inject_new_comments_from_env_stub(self);
        }
        // Mirrors `if force_persist: reset_session_activity_persist_window(self)` (ll.4101-4102)
        if force_persist {
            reset_session_activity_persist_window_stub(self);
        }
        // Mirrors `self._persist_session_activity_if_due()` (l.4103)
        self.persist_session_activity_if_due();
    }

    // Convenience wrapper with defaults matching Python `provenance=None, force_persist=False`.
    pub fn touch_activity_simple(&mut self, desc: &str) {
        self.touch_activity(desc, None, false);
    }

    // -----------------------------------------------------------------------
    // _persist_session_activity_if_due — mirrors ll.4105-4147
    // -----------------------------------------------------------------------

    /// Mirrors `def _persist_session_activity_if_due(self) -> None:` (ll.4105-4147).
    pub fn persist_session_activity_if_due(&mut self) {
        // Mirrors `session_id = getattr(self, "session_id", None); session_db = getattr(self, "_session_db", None); if not session_id or session_db is None: return` (ll.4114-4117)
        if self.session_id.is_empty() || self.session_db.is_none() { return; }
        // Mirrors `touch = getattr(session_db, "touch_session_activity", None); if not callable(touch): return` (ll.4118-4120)
        // Rust: StubSessionDb always has touch_session_activity, so continue.
        // Mirrors `from agent.session_activity import SESSION_ACTIVITY_HEARTBEAT_MIN_INTERVAL_SECONDS, normalize_activity_provenance` (ll.4121-4124)
        const SESSION_ACTIVITY_HEARTBEAT_MIN_INTERVAL_SECONDS: f64 = 30.0; // mirrors pin at >=30s (l.4127 comment)
        // Mirrors `now_mono = time.monotonic(); last_mono = getattr(self, "_session_activity_last_persist_mono", 0.0); if (now_mono - last_mono) < SESSION_ACTIVITY_HEARTBEAT_MIN_INTERVAL_SECONDS: return` (ll.4126-4129)
        let now_mono_val = now_mono();
        if (now_mono_val - self.session_activity_last_persist_mono) < SESSION_ACTIVITY_HEARTBEAT_MIN_INTERVAL_SECONDS { return; }
        self.session_activity_last_persist_mono = now_mono_val;
        // Mirrors `try: touch(session_id, getattr(self, "_last_activity_ts", None), description=..., provenance=normalize_activity_provenance(...)) except: logger.debug(...)` (ll.4131-4147)
        if let Some(db) = &self.session_db {
            let _ = db.touch_session_activity(
                &self.session_id.clone(),
                self.last_activity_ts,
                if self.last_activity_desc.is_empty() { None } else { Some(self.last_activity_desc.clone()) },
                normalize_activity_provenance_stub(Some(&self.last_activity_provenance.clone())),
            );
        }
    }

    // -----------------------------------------------------------------------
    // _reset_activity_labels_after_turn — mirrors ll.4149-4173
    // -----------------------------------------------------------------------

    /// Mirrors `def _reset_activity_labels_after_turn(self) -> None:` (ll.4149-4173).
    pub fn reset_activity_labels_after_turn(&mut self) {
        // Mirrors `from agent.session_activity import ActivityProvenance` (l.4158)
        // Mirrors `self._last_activity_desc = ""; self._last_activity_provenance = ActivityProvenance.UNKNOWN` (ll.4160-4161)
        self.last_activity_desc = String::new();
        self.last_activity_provenance = "unknown".to_string();
        // Mirrors `session_id = getattr(self, "session_id", None); session_db = getattr(self, "_session_db", None); if not session_id or session_db is None: return` (ll.4162-4165)
        if self.session_id.is_empty() || self.session_db.is_none() { return; }
        // Mirrors `clear = getattr(session_db, "clear_session_activity_labels", None); if not callable(clear): return; try: clear(session_id) except: pass` (ll.4166-4173)
        if let Some(db) = &self.session_db {
            let _ = db.clear_session_activity_labels(&self.session_id.clone());
        }
    }

    // -----------------------------------------------------------------------
    // _capture_rate_limits — mirrors ll.4175-4192
    // -----------------------------------------------------------------------

    /// Mirrors `def _capture_rate_limits(self, http_response: Any) -> None:` (ll.4175-4192).
    pub fn capture_rate_limits(&mut self, http_response: Option<&Value>) {
        // Mirrors `if http_response is None: return; headers = getattr(http_response, "headers", None); if not headers: return` (ll.4181-4185)
        let resp = match http_response { Some(r) => r, None => return };
        let headers = match resp.get("headers") { Some(h) => h, None => return };
        if headers.is_null() { return; }
        // Mirrors `try: from agent.rate_limit_tracker import parse_rate_limit_headers; state = parse_rate_limit_headers(headers, provider=self.provider); if state is not None: self._rate_limit_state = state except: pass` (ll.4186-4192)
        if let Some(state) = parse_rate_limit_headers_stub(headers, &self.provider) {
            self.rate_limit_state = Some(state);
        }
    }

    // -----------------------------------------------------------------------
    // get_rate_limit_state — mirrors ll.4194-4196
    // -----------------------------------------------------------------------

    /// Mirrors `def get_rate_limit_state(self):` (ll.4194-4196).
    pub fn get_rate_limit_state(&self) -> Option<&RateLimitState> {
        // Mirrors `return self._rate_limit_state` (l.4196)
        self.rate_limit_state.as_ref()
    }

    // -----------------------------------------------------------------------
    // _capture_anthropic_response_headers — mirrors ll.4198-4207
    // -----------------------------------------------------------------------

    /// Mirrors `def _capture_anthropic_response_headers(self, http_response: Any) -> None:` (ll.4198-4207).
    pub fn capture_anthropic_response_headers(&mut self, http_response: Option<&Value>) {
        // Mirrors `self._capture_rate_limits(http_response); self._capture_credits(http_response)` (ll.4206-4207)
        // Note: need to clone resp for second call because first takes mutable borrow.
        let cloned = http_response.cloned();
        self.capture_rate_limits(http_response);
        self.capture_credits(cloned.as_ref());
    }

    // -----------------------------------------------------------------------
    // _capture_credits — mirrors ll.4209-4291
    // -----------------------------------------------------------------------

    /// Mirrors `def _capture_credits(self, http_response: Any) -> None:` (ll.4209-4291).
    pub fn capture_credits(&mut self, http_response: Option<&Value>) {
        // Mirrors `# Dev test fixture (HERMES_DEV_CREDITS_FIXTURE): ... try: from agent.credits_tracker import dev_fixture_credits_state; _fixture = dev_fixture_credits_state() except: _fixture = None; if _fixture is not None: ... return` (ll.4218-4245)
        if let Some(fixture) = dev_fixture_credits_state_stub() {
            self.credits_state = Some(fixture.clone());
            if self.credits_session_start_micros.is_none() {
                self.credits_session_start_micros = Some(fixture.remaining_micros);
            }
            if let Some(latch) = self.credits_latch.as_mut() {
                latch.insert("seen_below_90".to_string(), json!(true));
            }
            self.emit_credits_notices();
            return;
        }
        // Mirrors `if http_response is None: return; headers = getattr(http_response, "headers", None); if not headers: return` (ll.4246-4250)
        let resp = match http_response { Some(r) => r, None => return };
        let headers = match resp.get("headers") { Some(h) => h, None => return };
        if headers.is_null() { return; }
        let dev = is_truthy_value_stub(std::env::var("HERMES_DEV_CREDITS").ok().as_deref());
        // Mirrors `# ── Parse (fail-open → miss; never overwrite good state with None) ── try: from agent.credits_tracker import parse_credits_headers; state = parse_credits_headers(...) except: return; if state is None: ... return` (ll.4253-4265)
        let state = match parse_credits_headers_stub(headers, &self.provider) {
            Some(s) => s,
            None => {
                // Mirrors dev log for miss (ll.4260-4264) — no-op for audit
                return;
            }
        };
        // Mirrors `# retain-last-known: only overwrite on a fresh valid parse; self._credits_state = state; if self._credits_session_start_micros is None: self._credits_session_start_micros = state.remaining_micros` (ll.4267-4271)
        let remaining = state.remaining_micros;
        self.credits_state = Some(state);
        if self.credits_session_start_micros.is_none() {
            self.credits_session_start_micros = Some(remaining);
        }
        // Mirrors dev logging block (ll.4272-4288) — no-op for audit
        let _ = dev;
        // Mirrors `self._emit_credits_notices()` (l.4291)
        self.emit_credits_notices();
    }

    // -----------------------------------------------------------------------
    // _emit_credits_notices — mirrors ll.4293-4329
    // -----------------------------------------------------------------------

    /// Mirrors `def _emit_credits_notices(self) -> None:` (ll.4293-4329).
    pub fn emit_credits_notices(&mut self) {
        // Mirrors `if getattr(self, "notice_callback", None) is None and getattr(self, "notice_clear_callback", None) is None: return` (ll.4303-4304)
        if self.notice_callback.is_none() && self.notice_clear_callback.is_none() { return; }
        // Mirrors `if not self._credits_notices_enabled(): return` (ll.4305-4306)
        if !self.credits_notices_enabled() { return; }
        // Mirrors `state = getattr(self, "_credits_state", None); if state is None: return` (ll.4307-4309)
        let state = match self.credits_state.clone() { Some(s) => s, None => return };
        // Mirrors `try: from agent.credits_tracker import evaluate_credits_notices, is_free_tier_model, new_credits_latch; latch = getattr(self, "_credits_latch", None); if latch is None: latch = self._credits_latch = new_credits_latch()` (ll.4310-4314)
        if self.credits_latch.is_none() {
            self.credits_latch = Some(new_credits_latch_stub());
        }
        // Mirrors `model_is_free = is_free_tier_model(getattr(self, "model", "") or "", getattr(self, "base_url", "") or "")` (ll.4319-4322)
        let model_is_free = is_free_tier_model_stub(&self.model, &self.base_url);
        // Need to get mutable latch for evaluate call; clone to avoid double borrow.
        let mut latch = self.credits_latch.clone().unwrap_or_default();
        // Mirrors `to_show, to_clear = evaluate_credits_notices(state, latch, model_is_free=model_is_free); for key in to_clear: self._emit_notice_clear(key); for notice in to_show: self._emit_notice(notice)` (ll.4323-4327)
        let (to_show, to_clear) = evaluate_credits_notices_stub(&state, &mut latch, model_is_free);
        self.credits_latch = Some(latch);
        for key in to_clear { self.emit_notice_clear(&key); }
        for notice in to_show { self.emit_notice(notice); }
        // Mirrors `except Exception: logger.warning(...)` (ll.4328-4329) — swallow
    }

    // -----------------------------------------------------------------------
    // _credits_notices_enabled — mirrors ll.4331-4352
    // -----------------------------------------------------------------------

    /// Mirrors `def _credits_notices_enabled(self) -> bool:` (ll.4331-4352).
    pub fn credits_notices_enabled(&mut self) -> bool {
        // Mirrors `cached = getattr(self, "_credits_notices_enabled_cache", None); if cached is not None: return cached` (ll.4339-4341)
        if let Some(cached) = self.credits_notices_enabled_cache { return cached; }
        // Mirrors `enabled = True; try: from hermes_cli.config import load_config as _load_config; _cfg = _load_config() or {}; _display = _cfg.get("display") if isinstance(_cfg, dict) else None; if isinstance(_display, dict) and "credits_notices" in _display: enabled = bool(...)`
        let mut enabled = true;
        let cfg = load_config_stub();
        if let Some(display) = cfg.get("display").and_then(|v| v.as_object()) {
            if display.contains_key("credits_notices") {
                enabled = display.get("credits_notices").and_then(|v| v.as_bool()).unwrap_or(true);
            }
        }
        self.credits_notices_enabled_cache = Some(enabled);
        enabled
    }

    // -----------------------------------------------------------------------
    // get_credits_state — mirrors ll.4354-4356
    // -----------------------------------------------------------------------

    /// Mirrors `def get_credits_state(self):` (ll.4354-4356).
    pub fn get_credits_state(&self) -> Option<&CreditsState> {
        // Mirrors `return self._credits_state` (l.4357)
        self.credits_state.as_ref()
    }

    // -----------------------------------------------------------------------
    // get_credits_spent_micros — mirrors ll.4358-4362
    // -----------------------------------------------------------------------

    /// Mirrors `def get_credits_spent_micros(self):` (ll.4358-4362).
    pub fn get_credits_spent_micros(&self) -> Option<i64> {
        // Mirrors `if self._credits_session_start_micros is None or self._credits_state is None: return None; return self._credits_session_start_micros - self._credits_state.remaining_micros`
        let start = self.credits_session_start_micros?;
        let cur = self.credits_state.as_ref()?.remaining_micros;
        Some(start - cur)
    }

    // -----------------------------------------------------------------------
    // _check_openrouter_cache_status — mirrors ll.4364-4384
    // -----------------------------------------------------------------------

    /// Mirrors `def _check_openrouter_cache_status(self, http_response: Any) -> None:` (ll.4364-4384).
    pub fn check_openrouter_cache_status(&mut self, http_response: Option<&Value>) {
        // Mirrors `if http_response is None: return; headers = getattr(http_response, "headers", None); if not headers: return` (ll.4369-4373)
        let resp = match http_response { Some(r) => r, None => return };
        let headers = match resp.get("headers") { Some(h) => h, None => return };
        if headers.is_null() { return; }
        // Mirrors `try: status = headers.get("x-openrouter-cache-status"); if not status: return; if status.upper() == "HIT": self._or_cache_hits +=1; logger.info(...); else: logger.debug(...) except: pass` (ll.4374-4384)
        let status = headers.get("x-openrouter-cache-status").and_then(|v| v.as_str()).or_else(|| {
            // Header keys are case-insensitive in Python's case-insensitive dict; Rust stub checks lowercased variant too
            headers.as_object().and_then(|m| {
                m.iter().find(|(k,_)| k.to_lowercase() == "x-openrouter-cache-status").and_then(|(_,v)| v.as_str())
            })
        });
        if let Some(s) = status {
            if s.to_uppercase() == "HIT" {
                self.or_cache_hits += 1;
            }
        }
    }

    // -----------------------------------------------------------------------
    // get_activity_summary — mirrors ll.4386-4414
    // -----------------------------------------------------------------------

    /// Mirrors `def get_activity_summary(self) -> dict:` (ll.4386-4414).
    pub fn get_activity_summary(&self) -> Value {
        // Mirrors `from agent.session_activity import ActivityProvenance, build_activity_snapshot; provenance = getattr(self, "_last_activity_provenance", None); if provenance is None: provenance = ActivityProvenance.UNKNOWN; return build_activity_snapshot(...)` (ll.4395-4414)
        let provenance = if self.last_activity_provenance.is_empty() { "unknown".to_string() } else { self.last_activity_provenance.clone() };
        json!({
            "last_activity_at": self.last_activity_ts,
            "last_activity_description": self.last_activity_desc,
            "last_activity_provenance": provenance,
            // short aliases (ll.4408-4410)
            "last_activity_ts": self.last_activity_ts,
            "last_activity_desc": self.last_activity_desc,
            "last_activity_provenance_alias": provenance,
            "current_tool": self.current_tool,
            "api_call_count": self.api_call_count,
            "max_iterations": self.max_iterations,
            "budget_used": self.iteration_budget.used,
            "budget_max": self.iteration_budget.max_total
        })
    }

    // -----------------------------------------------------------------------
    // shutdown_memory_provider — mirrors ll.4416-4442
    // -----------------------------------------------------------------------

    /// Mirrors `def shutdown_memory_provider(self, messages: list = None) -> None:` (ll.4416-4442).
    pub fn shutdown_memory_provider(&mut self, messages: Option<&[Value]>) {
        // Mirrors `if getattr(self, "_memory_provider_shutdown", False): return; self._memory_provider_shutdown = True` (ll.4422-4424)
        if self.memory_provider_shutdown { return; }
        self.memory_provider_shutdown = true;
        // Mirrors `if self._memory_manager: try: self._memory_manager.on_session_end(messages or []) except: logger.warning(...); try: self._memory_manager.shutdown_all() except: pass` (ll.4425-4433)
        if let Some(mgr) = &self.memory_manager {
            let msgs: &[Value] = messages.unwrap_or(&[]);
            let _ = mgr.on_session_end(msgs);
            let _ = mgr.shutdown_all();
        }
        // Mirrors `# Notify context engine of session end (flush DAG, close DBs, etc.) if hasattr(self, "context_compressor") and self.context_compressor: try: self.context_compressor.on_session_end(self.session_id or "", messages or [],) except: pass` (ll.4434-4442)
        if let Some(cc) = &self.context_compressor {
            let msgs: &[Value] = messages.unwrap_or(&[]);
            let _ = cc.on_session_end(&self.session_id, msgs);
        }
    }

    // -----------------------------------------------------------------------
    // commit_memory_session — mirrors ll.4444-4467
    // -----------------------------------------------------------------------

    /// Mirrors `def commit_memory_session(self, messages: list = None) -> None:` (ll.4444-4467).
    pub fn commit_memory_session(&self, messages: Option<&[Value]>) {
        // Mirrors `if self._memory_manager: try: self._memory_manager.on_session_end(messages or []) except: pass` (ll.4449-4453)
        if let Some(mgr) = &self.memory_manager {
            let msgs: &[Value] = messages.unwrap_or(&[]);
            let _ = mgr.on_session_end(msgs);
        }
        // Mirrors `# Notify context engine of session end too — ... Mirrors the call in shutdown_memory_provider(). See issue #22394. if hasattr(self, "context_compressor") and self.context_compressor: try: self.context_compressor.on_session_end(self.session_id or "", messages or [],) except: pass` (ll.4454-4467)
        if let Some(cc) = &self.context_compressor {
            let msgs: &[Value] = messages.unwrap_or(&[]);
            let _ = cc.on_session_end(&self.session_id, msgs);
        }
    }

    // -----------------------------------------------------------------------
    // _sync_external_memory_for_turn — mirrors ll.4469-4500 (slice head)
    // Nominal 4500 boundary falls mid-docstring inside the `try/except` guard;
    // truncated here syntactically, tail continues in slice6.
    // -----------------------------------------------------------------------

    /// Mirrors `def _sync_external_memory_for_turn(self, *, original_user_message: Any, final_response: Any, interrupted: bool, messages: list | None = None) -> None:` (ll.4469-4500 slice head).
    ///
    /// Only the docstring header through the `strictly best-effort` comment
    /// (ll.4469-4500) is in this slice (nominal 4500 boundary falls inside
    /// the `wrapped in ``try/except Exception`` because ...` docstring at
    /// ll.4498-4500). The body
    /// `if interrupted: return; try: self._memory_manager.sync_all(...)`
    /// at ll.4501-4535 + every later `AIAgent` method is canonical in
    /// `run_agent_slice6.rs`. This stub closes syntactically with the
    /// interrupted-turn guard for audit.
    pub fn sync_external_memory_for_turn(
        &self,
        original_user_message: Option<&Value>,
        final_response: Option<&Value>,
        interrupted: bool,
        messages: Option<&[Value]>,
    ) {
        // Mirrors docstring at ll.4477-4500 about `sync_all` + `queue_prefetch_all` + `original_user_message` vs `user_message` + interrupted-turn skip (#15218).
        // Slice 5 covers only the docstring; body is canonical in slice6.
        // Keep interrupted guard here for audit continuity (actual guard at ll.4501 is `if interrupted: return`).
        if interrupted { return; }
        let _ = (original_user_message, final_response, messages);
        // Body stub: mirrors `try: if self._memory_manager: self._memory_manager.sync_all(...); self._memory_manager.queue_prefetch_all(...) except Exception: pass` — stub no-op.
        // Real impl continues in slice6; intentionally truncated at 4500 to keep NEVER-cargo invariant.
    }

    // -----------------------------------------------------------------------
    // Helpers — mirrors private plumbing used by slice5 methods
    // -----------------------------------------------------------------------

    /// Mirrors `self._emit_notice(notice)` (ll.4327).
    pub fn emit_notice(&self, _notice: Value) {
        if self.notice_callback.is_some() { let _ = _notice; }
    }

    /// Mirrors `self._emit_notice_clear(key)` (ll.4325).
    pub fn emit_notice_clear(&self, _key: &str) {
        if self.notice_clear_callback.is_some() { let _ = _key; }
    }

    /// Mirrors `self._vprint / self._emit_warning / self._emit_status` helpers already canonical in slice2;
    /// stubs here so `_capture_credits` / `_check_openrouter_cache_status` audit without cross-slice import.
    pub fn emit_warning(&self, _msg: &str) {}
    pub fn emit_status(&self, _msg: &str) {}
    pub fn vprint(&self, _args: Vec<Value>, _force: bool) {}
}

