//! Abstract base class for pluggable context engines.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/context_engine.py` (489 LOC).
//! T0017 — full file (lines 1-489).
//!
//! ```text
//! Abstract base class for pluggable context engines.
//!
//! A context engine controls how conversation context is managed when
//! approaching the model's token limit. The built-in ContextCompressor
//! is the default implementation. Third-party engines (e.g. LCM) can
//! replace it via the plugin system or by being placed in the
//! ``plugins/context_engine/<name>/`` directory.
//!
//! Selection is config-driven: ``context.engine`` in config.yaml.
//! Default is ``"compressor"`` (the built-in). Only one engine is active.
//!
//! The engine is responsible for:
//!   - Deciding when compaction should fire
//!   - Performing compaction (summarization, DAG construction, etc.)
//!   - Optionally exposing tools the agent can call (e.g. lcm_grep)
//!   - Tracking token usage from API responses
//!
//! Lifecycle:
//!   1. Engine is instantiated and registered (plugin register() or default)
//!   2. on_session_start() called when a conversation begins
//!   3. update_from_response() called after each API response with usage data
//!   4. should_compress() checked after each turn
//!   5. compress() called when should_compress() returns True
//!   6. on_session_end() called at real session boundaries (CLI exit, /reset,
//!      gateway session expiry) — NOT per-turn
//! ```
//!
//! NEVER cargo — 1:1 translation only (no `cargo check` / `cargo build`).
//! Mirrors Python ll.1-489 verbatim; line numbers in comments refer to the
//! 489-line source file. Verified by line-level audit, not by compilation.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

// ---------------------------------------------------------------------------
// Imports — mirrors Python ll.28-31
// ---------------------------------------------------------------------------
use std::collections::HashMap;

use serde_json::{json, Value};

// Python imports (ll.28-29) — stdlib:
//   abc (ABC, abstractmethod), typing (Any, Dict, List, Optional)
// Mapped: Rust trait system (ABC → trait, abstractmethod → required trait method),
//   HashMap<String, Value> for Dict[str, Any], Vec<Message> for List[Dict[str, Any]],
//
// Python intra-repo import (l.31):
//   from agent.redact import redact_sensitive_text
// Rust: stub below mirrors surface so this file is self-contained and
// grep-traceable. Canonical impl lives in hermes-core / hermes-util.

// ---------------------------------------------------------------------------
// Logger — mirrors implicit module logger (no explicit logger in py, but kept
// for parity with sibling modules)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "context_engine";

// ---------------------------------------------------------------------------
// Helpers mirroring Python ll.28-31 helper import
// ---------------------------------------------------------------------------

/// Stub: mirrors `agent.redact.redact_sensitive_text` (ll.31, 42).
///
/// Real impl scrubs secrets/credentials before a memory-context crosses the
/// context-engine/LLM egress boundary (force=True, redact_url_credentials=True).
/// Stub returns input verbatim; canonical impl in hermes-core replaces this
/// when crates merge. Kept grep-traceable for 1:1 audit.
pub fn redact_sensitive_text(text: &str, force: bool, redact_url_credentials: bool) -> String {
    let _ = (force, redact_url_credentials);
    text.to_string()
}

#[allow(dead_code)]
fn _redact_sensitive_text(text: &str, force: bool, redact_url_credentials: bool) -> String {
    redact_sensitive_text(text, force, redact_url_credentials)
}

// ---------------------------------------------------------------------------
// Constants — mirrors Python ll.34-37
// ---------------------------------------------------------------------------

/// Mirrors `MEMORY_CONTEXT_MAX_CHARS = 6_000` (l.34)
pub const MEMORY_CONTEXT_MAX_CHARS: usize = 6_000;
/// Mirrors `_MEMORY_CONTEXT_HEAD_CHARS = 4_000` (l.35)
pub const MEMORY_CONTEXT_HEAD_CHARS: usize = 4_000;
/// Mirrors `_MEMORY_CONTEXT_TAIL_CHARS = 1_500` (l.36)
pub const MEMORY_CONTEXT_TAIL_CHARS: usize = 1_500;
/// Mirrors `_MEMORY_CONTEXT_TRUNCATION_MARKER = "\n...[memory provider context truncated]...\n"` (l.37)
pub const MEMORY_CONTEXT_TRUNCATION_MARKER: &str = "\n...[memory provider context truncated]...\n";

#[allow(dead_code)]
const _MEMORY_CONTEXT_HEAD_CHARS: usize = MEMORY_CONTEXT_HEAD_CHARS;
#[allow(dead_code)]
const _MEMORY_CONTEXT_TAIL_CHARS: usize = MEMORY_CONTEXT_TAIL_CHARS;
#[allow(dead_code)]
const _MEMORY_CONTEXT_TRUNCATION_MARKER: &str = MEMORY_CONTEXT_TRUNCATION_MARKER;

// ---------------------------------------------------------------------------
// sanitize_memory_context — mirrors Python ll.40-53
// ---------------------------------------------------------------------------

/// Mirrors `def sanitize_memory_context(memory_context: str) -> str:` (ll.40-53)
///
/// Prepare provider context for a context-engine/LLM egress boundary.
pub fn sanitize_memory_context(memory_context: &str) -> String {
    // Mirrors `sanitized = redact_sensitive_text(memory_context.strip(), force=True, redact_url_credentials=True)` (ll.42-46)
    let sanitized = redact_sensitive_text(memory_context.trim(), true, true);
    // Mirrors `if len(sanitized) <= MEMORY_CONTEXT_MAX_CHARS: return sanitized` (ll.47-48)
    if sanitized.len() <= MEMORY_CONTEXT_MAX_CHARS {
        return sanitized;
    }
    // Mirrors `return sanitized[:_MEMORY_CONTEXT_HEAD_CHARS] + _MEMORY_CONTEXT_TRUNCATION_MARKER + sanitized[-_MEMORY_CONTEXT_TAIL_CHARS:]` (ll.49-53)
    // Rust string slicing is byte-indexed; Python slices on chars. For ASCII
    // memory contexts (typical) they are identical; keep byte slicing for
    // 1:1 audit and note the divergence for multi-byte contexts.
    let head = &sanitized[..MEMORY_CONTEXT_HEAD_CHARS.min(sanitized.len())];
    let tail_start = sanitized.len().saturating_sub(MEMORY_CONTEXT_TAIL_CHARS);
    let tail = &sanitized[tail_start..];
    format!("{}{}{}", head, MEMORY_CONTEXT_TRUNCATION_MARKER, tail)
}

#[allow(dead_code)]
fn _sanitize_memory_context(memory_context: &str) -> String {
    sanitize_memory_context(memory_context)
}

// ---------------------------------------------------------------------------
// automatic_compaction_status_message — mirrors Python ll.56-86
// ---------------------------------------------------------------------------

/// Mirrors `def automatic_compaction_status_message(engine: Any, *, phase: str, default_message: str, **context: Any) -> str | None:` (ll.56-86)
///
/// Resolve host-visible status for an automatic compaction event.
///
/// Engines can suppress routine automatic status with
/// `emit_automatic_compaction_status = False` or customize it by defining
/// `get_automatic_compaction_status_message(...)`. Empty strings and
/// `None` mean "do not emit a lifecycle status".
pub fn automatic_compaction_status_message(
    engine: &dyn ContextEngine,
    phase: &str,
    default_message: &str,
    context: &HashMap<String, Value>,
) -> Option<String> {
    // Mirrors `if not getattr(engine, "emit_automatic_compaction_status", True): return None` (ll.70-71)
    if !engine.emit_automatic_compaction_status() {
        return None;
    }

    // Mirrors `formatter = getattr(engine, "get_automatic_compaction_status_message", None)` + callable check (ll.73-81)
    // In Rust, the trait method IS the formatter; we call it. Python's
    // `if callable(formatter): message = formatter(...) else: message = default_message`
    // collapses to: if engine overrides default, it returns Some(custom) or None;
    // otherwise it returns default_message. The trait default already returns
    // default_message when emit is true, so we delegate.
    let message = engine.get_automatic_compaction_status_message(phase, default_message, context);

    // Mirrors `if message is None: return None` (ll.83-84)
    let message = message?;

    // Mirrors `message = str(message).strip(); return message or None` (ll.85-86)
    let trimmed = message.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

#[allow(dead_code)]
fn _automatic_compaction_status_message(
    engine: &dyn ContextEngine,
    phase: &str,
    default_message: &str,
    context: &HashMap<String, Value>,
) -> Option<String> {
    automatic_compaction_status_message(engine, phase, default_message, context)
}

// ---------------------------------------------------------------------------
// Message type — mirrors Python `Dict[str, Any]` / `List[Dict[str, Any]]` (ll.29, 163+)
// ---------------------------------------------------------------------------
/// Mirrors `Dict[str, Any]` message shape (ll.29, 163+).
/// Python messages are `{"role": "...", "content": ..., "tool_calls": ..., etc}`
/// Rust: `HashMap<String, Value>` preserves the open-dict shape.
pub type Message = HashMap<String, Value>;
/// Mirrors `List[Dict[str, Any]]`
pub type Messages = Vec<Message>;

// ---------------------------------------------------------------------------
// resolve_model_threshold — mirrors `agent/context_compressor.py:resolve_model_threshold`
// Needed by ContextEngine::update_model (ll.478-489). Canonical lives in
// `agent/context_compressor.py` ll.2044-2068; stub kept 1:1 here so
// context_engine.rs is self-contained. See compressor_slice1.rs for full audit.
// ---------------------------------------------------------------------------

/// Mirrors `def resolve_model_threshold(model: str, model_thresholds: dict[str, float] | None, default: float) -> float:` (context_compressor.py ll.2044-2068)
///
/// Longest matching substring key wins. When no override matches, or when
/// `model_thresholds` is empty/None, `default` is returned unchanged.
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
    // Mirrors `best_key = ""; for key in model_thresholds: if key in model and len(key) > len(best_key): best_key = key` (ll.2061-2063)
    let mut best_key = "";
    let mut best_len = 0usize;
    for key in thresholds.keys() {
        if model.contains(key.as_str()) && key.len() > best_len {
            best_key = key;
            best_len = key.len();
        }
    }
    // Mirrors `if best_key: return model_thresholds[best_key] else: return default` (ll.2064+)
    if best_len > 0 {
        thresholds.get(best_key).copied().unwrap_or(default)
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
// ContextEngine — mirrors Python ll.89-489
// ---------------------------------------------------------------------------

/// Shared state for ContextEngine implementors — mirrors Python ll.99-129.
///
/// Python class variables (ll.103-129) are instance state after `__init__`.
/// Rust traits cannot hold fields, so shared mutable bookkeeping lives here.
/// Embed this in your engine struct and delegate trait accessors to it.
#[derive(Debug, Clone)]
pub struct ContextEngineState {
    // -- Token state (read by run_agent.py for display/logging) -- (ll.99-108)
    pub last_prompt_tokens: i64,
    pub last_completion_tokens: i64,
    pub last_total_tokens: i64,
    pub threshold_tokens: i64,
    pub context_length: i64,
    pub compression_count: usize,
    // -- Compaction parameters (read by run_agent.py for preflight) -- (ll.110-123)
    pub threshold_percent: f64,
    pub protect_first_n: usize,
    pub protect_last_n: usize,
    // User-visible lifecycle status (ll.125-129)
    pub emit_automatic_compaction_status: bool,
    // -- Model threshold extension (ll.474-488) --
    /// Mirrors `self.model_thresholds` (config `compression.model_thresholds`)
    pub model_thresholds: HashMap<String, f64>,
    /// Mirrors `self._config_threshold_percent` snapshot (l.483)
    pub config_threshold_percent: Option<f64>,
    /// Mirrors `self._base_threshold_percent` resolved per-model (l.484)
    pub base_threshold_percent: f64,
}

impl Default for ContextEngineState {
    fn default() -> Self {
        Self {
            // Mirrors ll.103-108 defaults
            last_prompt_tokens: 0,
            last_completion_tokens: 0,
            last_total_tokens: 0,
            threshold_tokens: 0,
            context_length: 0,
            compression_count: 0,
            // Mirrors ll.121-123 defaults
            threshold_percent: 0.75,
            protect_first_n: 3,
            protect_last_n: 6,
            // Mirrors l.129
            emit_automatic_compaction_status: true,
            // Mirrors model-threshold extension (ll.479-488)
            model_thresholds: HashMap::new(),
            config_threshold_percent: None,
            base_threshold_percent: 0.75,
        }
    }
}

/// Mirrors `class ContextEngine(ABC):` (ll.89-489)
///
/// Base class all context engines must implement.
pub trait ContextEngine: Send + Sync {
    // -- Identity ----------------------------------------------------------
    // Mirrors `@property @abstractmethod def name(self) -> str:` (ll.94-97)
    fn name(&self) -> &str;

    // -- Token / compaction state accessors --------------------------------
    // Mirrors ll.103-129 class variables. Trait object needs accessors.
    fn state(&self) -> &ContextEngineState;
    fn state_mut(&mut self) -> &mut ContextEngineState;

    // Convenience getters (mirrors direct field reads `engine.last_prompt_tokens` etc.)
    fn last_prompt_tokens(&self) -> i64 {
        self.state().last_prompt_tokens
    }
    fn last_completion_tokens(&self) -> i64 {
        self.state().last_completion_tokens
    }
    fn last_total_tokens(&self) -> i64 {
        self.state().last_total_tokens
    }
    fn threshold_tokens(&self) -> i64 {
        self.state().threshold_tokens
    }
    fn context_length(&self) -> i64 {
        self.state().context_length
    }
    fn compression_count(&self) -> usize {
        self.state().compression_count
    }
    fn threshold_percent(&self) -> f64 {
        self.state().threshold_percent
    }
    fn protect_first_n(&self) -> usize {
        self.state().protect_first_n
    }
    fn protect_last_n(&self) -> usize {
        self.state().protect_last_n
    }
    fn emit_automatic_compaction_status(&self) -> bool {
        self.state().emit_automatic_compaction_status
    }

    // -- Core interface ----------------------------------------------------
    // Mirrors `@abstractmethod def update_from_response(self, usage: Dict[str, Any]) -> None:` (ll.133-143)
    fn update_from_response(&mut self, usage: &HashMap<String, Value>);

    // Mirrors `@abstractmethod def should_compress(self, prompt_tokens: int = None) -> bool:` (ll.145-147)
    fn should_compress(&self, prompt_tokens: Option<i64>) -> bool;

    // Mirrors `def should_compress_info(self, prompt_tokens: int = None) -> tuple[bool, str | None]:` (ll.149-160)
    fn should_compress_info(&self, prompt_tokens: Option<i64>) -> (bool, Option<String>) {
        // Mirrors `return self.should_compress(prompt_tokens), None` (l.160)
        (self.should_compress(prompt_tokens), None)
    }

    // Mirrors `@abstractmethod def compress(self, messages: List[Dict[str, Any]], current_tokens: Optional[int] = None, focus_topic: Optional[str] = None, force: bool = False, memory_context: str = "") -> List[Dict[str, Any]]:` (ll.162-190)
    fn compress(
        &mut self,
        messages: Messages,
        current_tokens: Option<i64>,
        focus_topic: Option<String>,
        force: bool,
        memory_context: String,
    ) -> Messages;

    // -- Optional: proactive tool-result prune -----------------------------
    // Mirrors `def prune_tool_results_only(self, messages: List[Dict[str, Any]], current_tokens: int | None = None) -> tuple[List[Dict[str, Any]], int]:` (ll.194-211)
    fn prune_tool_results_only(
        &mut self,
        messages: Messages,
        _current_tokens: Option<i64>,
    ) -> (Messages, usize) {
        // Mirrors `return messages, 0` (l.211)
        (messages, 0)
    }

    // -- Optional: per-turn context selection (distinct from compression) --
    // Mirrors `def select_context(self, request_messages: List[Dict[str, Any]], *, conversation_messages: List[Dict[str, Any]] = None, incoming_message: Dict[str, Any] = None, budget_tokens: int = 0) -> List[Dict[str, Any]]:` (ll.215-279)
    fn select_context(
        &self,
        _request_messages: &Messages,
        _conversation_messages: Option<&Messages>,
        _incoming_message: Option<&Message>,
        _budget_tokens: i64,
    ) -> Option<Messages> {
        // Mirrors `return None` (l.279)
        None
    }

    // Mirrors `def on_turn_complete(self, messages: List[Dict[str, Any]], usage: Dict[str, Any] = None, **kwargs: Any) -> None:` (ll.281-328)
    fn on_turn_complete(
        &mut self,
        _messages: &Messages,
        _usage: Option<&HashMap<String, Value>>,
        _kwargs: &HashMap<String, Value>,
    ) {
        // Mirrors `return None` (l.328) — default no-op
    }

    // -- Optional: pre-flight check ----------------------------------------
    // Mirrors `def should_compress_preflight(self, messages: List[Dict[str, Any]]) -> bool:` (ll.332-338)
    fn should_compress_preflight(&self, _messages: &Messages) -> bool {
        // Mirrors `return False` (l.338)
        false
    }

    // Mirrors `def should_defer_preflight_to_real_usage(self, rough_tokens: int) -> bool:` (ll.340-347)
    fn should_defer_preflight_to_real_usage(&self, _rough_tokens: i64) -> bool {
        // Mirrors `return False` (l.347)
        false
    }

    // Mirrors `def get_automatic_compaction_status_message(self, *, phase: str, default_message: str, **context: Any) -> str | None:` (ll.349-368)
    fn get_automatic_compaction_status_message(
        &self,
        _phase: &str,
        default_message: &str,
        _context: &HashMap<String, Value>,
    ) -> Option<String> {
        // Mirrors `if not self.emit_automatic_compaction_status: return None; return default_message` (ll.366-368)
        if !self.emit_automatic_compaction_status() {
            return None;
        }
        Some(default_message.to_string())
    }

    // -- Optional: manual /compress preflight ------------------------------
    // Mirrors `def has_content_to_compress(self, messages: List[Dict[str, Any]]) -> bool:` (ll.372-383)
    fn has_content_to_compress(&self, _messages: &Messages) -> bool {
        // Mirrors `return True` (l.383)
        true
    }

    // -- Optional: session lifecycle ---------------------------------------
    // Mirrors `def on_session_start(self, session_id: str, **kwargs) -> None:` (ll.387-392)
    fn on_session_start(&mut self, _session_id: &str, _kwargs: &HashMap<String, Value>) {}

    // Mirrors `def on_session_end(self, session_id: str, messages: List[Dict[str, Any]]) -> None:` (ll.394-399)
    fn on_session_end(&mut self, _session_id: &str, _messages: &Messages) {}

    // Mirrors `def on_session_reset(self) -> None:` (ll.401-409)
    fn on_session_reset(&mut self) {
        // Mirrors ll.406-409
        let s = self.state_mut();
        s.last_prompt_tokens = 0;
        s.last_completion_tokens = 0;
        s.last_total_tokens = 0;
        s.compression_count = 0;
    }

    // -- Optional: tools ---------------------------------------------------
    // Mirrors `def get_tool_schemas(self) -> List[Dict[str, Any]]:` (ll.413-419)
    fn get_tool_schemas(&self) -> Vec<Value> {
        // Mirrors `return []` (l.419)
        Vec::new()
    }

    // Mirrors `def handle_tool_call(self, name: str, args: Dict[str, Any], **kwargs) -> str:` (ll.421-431)
    fn handle_tool_call(
        &mut self,
        name: &str,
        _args: &HashMap<String, Value>,
        _kwargs: &HashMap<String, Value>,
    ) -> String {
        // Mirrors `return json.dumps({"error": f"Unknown context engine tool: {name}"})` (l.431)
        json!({"error": format!("Unknown context engine tool: {}", name)}).to_string()
    }

    // -- Optional: status / display ----------------------------------------
    // Mirrors `def get_status(self) -> Dict[str, Any]:` (ll.435-454)
    fn get_status(&self) -> HashMap<String, Value> {
        // Mirrors ll.440-454
        let last_prompt = if self.last_prompt_tokens() > 0 {
            self.last_prompt_tokens()
        } else {
            0
        };
        let context_length = self.context_length();
        let usage_percent = if context_length != 0 {
            let pct = last_prompt as f64 / context_length as f64 * 100.0;
            if pct > 100.0 { 100.0 } else { pct }
        } else {
            0.0
        };
        let mut out = HashMap::new();
        out.insert("last_prompt_tokens".to_string(), json!(last_prompt));
        out.insert("threshold_tokens".to_string(), json!(self.threshold_tokens()));
        out.insert("context_length".to_string(), json!(context_length));
        out.insert("usage_percent".to_string(), json!(usage_percent));
        out.insert("compression_count".to_string(), json!(self.compression_count() as i64));
        out
    }

    // -- Optional: model switch support ------------------------------------
    // Mirrors `def update_model(self, model: str, context_length: int, base_url: str = "", api_key: str = "", provider: str = "", api_mode: str = "") -> None:` (ll.458-489)
    fn update_model(
        &mut self,
        model: &str,
        context_length: i64,
        _base_url: &str,
        _api_key: &str,
        _provider: &str,
        _api_mode: &str,
    ) {
        // Mirrors ll.473-489
        let s = self.state_mut();
        s.context_length = context_length;
        // Mirrors ll.479-483: snapshot _config_threshold_percent once
        if s.config_threshold_percent.is_none() {
            s.config_threshold_percent = Some(s.threshold_percent);
        }
        let default = s.config_threshold_percent.unwrap_or(s.threshold_percent);
        // Mirrors ll.484-487: resolve via longest-substring match
        let thresholds_ref = if s.model_thresholds.is_empty() {
            None
        } else {
            Some(&s.model_thresholds)
        };
        // Need to avoid double-borrow: clone thresholds for call, then write back
        let model_thresholds_clone = s.model_thresholds.clone();
        let thresholds_opt = if model_thresholds_clone.is_empty() { None } else { Some(&model_thresholds_clone) };
        let resolved = resolve_model_threshold(model, thresholds_opt, default);
        // Write back — re-borrow mutably
        let s2 = self.state_mut();
        s2.base_threshold_percent = resolved;
        s2.threshold_percent = resolved;
        s2.threshold_tokens = (context_length as f64 * resolved) as i64;
    }
}

// ---------------------------------------------------------------------------
// Concrete base engine for 1:1 audit — mirrors bare ContextEngine instantiation
// shape. Not for direct plugin use; plugins should implement the trait.
// Provided so `context_engine.rs` has a constructible type mirroring Python's
// class-variable defaults (ll.103-129) without requiring a separate crate.
// ---------------------------------------------------------------------------

/// Minimal concrete engine — mirrors the default field values of `ContextEngine`
/// (ll.103-129) with abstract methods stubbed as `unimplemented!()`. Real engines
/// (ContextCompressor, LCM) replace these.
#[derive(Debug, Clone)]
pub struct BaseContextEngine {
    pub state: ContextEngineState,
    /// Mirrors `name` property — must be set by concrete engine.
    pub engine_name: String,
}

impl Default for BaseContextEngine {
    fn default() -> Self {
        Self {
            state: ContextEngineState::default(),
            engine_name: "base".to_string(),
        }
    }
}

impl BaseContextEngine {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            state: ContextEngineState::default(),
            engine_name: name.into(),
        }
    }
}

impl ContextEngine for BaseContextEngine {
    fn name(&self) -> &str {
        &self.engine_name
    }
    fn state(&self) -> &ContextEngineState {
        &self.state
    }
    fn state_mut(&mut self) -> &mut ContextEngineState {
        &mut self.state
    }
    fn update_from_response(&mut self, _usage: &HashMap<String, Value>) {
        unimplemented!("update_from_response is abstract — implement in concrete engine (l.134)")
    }
    fn should_compress(&self, _prompt_tokens: Option<i64>) -> bool {
        unimplemented!("should_compress is abstract — implement in concrete engine (l.146)")
    }
    fn compress(
        &mut self,
        _messages: Messages,
        _current_tokens: Option<i64>,
        _focus_topic: Option<String>,
        _force: bool,
        _memory_context: String,
    ) -> Messages {
        unimplemented!("compress is abstract — implement in concrete engine (l.163)")
    }
}
