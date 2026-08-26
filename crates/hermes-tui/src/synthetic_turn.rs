//! Synthetic GIL-heavy turn driver for the AC-4 isolation certify harness.
//!
//! 1:1 port of `tui_gateway/synthetic_turn.py` (231 lines).
//!
//! Mechanism B (the class `docs/desktop/2026-07-04-dashboard-process-isolation-PRD.md`
//! targets) is interpreter-wide GIL starvation: concurrent heavy agent turns run
//! compute in threads of the SERVING process, and CPython's single GIL lets those
//! threads starve the event loop that flushes WebSocket frames for MINUTES. A
//! 2026-07-04 `sample` showed the loop thread parked in `take_gil` while worker
//! threads burned the interpreter — NOT blocked on I/O.
//!
//! To certify the fix (AC-4) without spending real tokens on 6 concurrent 100K+
//! context model calls, the harness needs a turn driver that reproduces THAT
//! regime: sustained pure-Python CPU that holds the GIL for the turn's duration.
//! A network/sleep stub is WRONG here — it would release the GIL during I/O and
//! never reproduce `take_gil` contention, so a dry-run green off it is a fake
//! green (the spec says so explicitly).
//!
//! This module is a **test seam**: it is dead unless `HERMES_ISO_CERTIFY_SYNTH_TURN`
//! is set. When armed, `tui_gateway.server._make_agent` returns a
//! `SyntheticHeavyAgent` instead of a real `AIAgent`. Because both the
//! in-process `_pool` path (isolation OFF) and the compute-host child path
//! (isolation ON) build their agent through `_make_agent`, the SAME synthetic
//! turn exercises whichever dispatch path is under test — the isolation boundary
//! is the only variable between an OFF run and an ON run.
//!
//! The per-turn intensity (wall duration, CPU chunk size, streamed-delta cadence,
//! token accounting) is carried in the prompt text as a small JSON spec so the
//! harness has full control and the server seam stays dumb. Any prompt that is not
//! a JSON object falls back to env / built-in defaults.
//!
//! ```python
//! # Python — tui_gateway/synthetic_turn.py
//! def synth_turn_armed() -> bool: return os.environ.get("HERMES_ISO_CERTIFY_SYNTH_TURN") == "1"
//! def _env_float(name: str, default: float) -> float: try: return float(os.environ.get(name, "") or default) ...
//! def _env_int(name: str, default: int) -> int: try: return int(os.environ.get(name, "") or default) ...
//! class SyntheticHeavyAgent:
//!     def __init__(self, session_id: str, *, model: str = "synthetic-heavy"): ...
//!     def clear_interrupt(self) -> None: self._interrupt.clear()
//!     def interrupt(self) -> None: self._interrupt.set()
//!     def _has_stream_consumers(self) -> bool: return True
//!     def close(self) -> None: self._interrupt.set()
//!     @staticmethod
//!     def _parse_spec(message: Any) -> dict[str, Any]: # JSON spec + env fallbacks
//!     def run_conversation(self, message: Any, *, conversation_history=None, stream_callback=None, task_id=None, **_kwargs) -> dict: ...
//! def maybe_build_synthetic_agent(session_id: str, model_override: Any = None) -> SyntheticHeavyAgent | None: ...
//! __all__ = ["SyntheticHeavyAgent", "maybe_build_synthetic_agent", "synth_turn_armed"]
//! ```
//!
//! # Rust mapping
//!
//! * `synth_turn_armed()` → [`synth_turn_armed`] (+ [`synth_turn_armed_with`] for tests).
//! * `_env_float(name, default)` → [`env_float`] (raw `Option<&str>` → `f64`, `None`/empty/`Err` → `default`) + [`env_float_from_env`] (reads `std::env::var`).
//! * `_env_int(name, default)` → [`env_int`] + [`env_int_from_env`].
//! * `threading.Event` `_interrupt` → `Arc<AtomicBool>` (set/clear/is_set). Mirrors `clear`/`set`/`is_set`.
//! * `_parse_spec(message)` → [`SyntheticSpec::parse`] / [`SyntheticHeavyAgent::parse_spec`]. JSON extraction is `std`-only (no `serde_json`); scans for `"key": <number>` like `methods_images::parse_generate_result`. Non-JSON or non-dict → env/defaults.
//! * `run_conversation` → [`SyntheticHeavyAgent::run_conversation`] (generic `FnMut(&str)` callback) + [`SyntheticHeavyAgent::run_conversation_boxed`] (trait-object). Wall clock via `Instant::now()` (mirrors `time.monotonic()`), `thread::sleep` for `sleep_s`. Integer burn is `wrapping_mul(1_000_003).wrapping_add(12_345)` masked to 64-bit (mirrors `& 0xFFFFFFFFFFFFFFFF`).
//! * `maybe_build_synthetic_agent` → [`maybe_build_synthetic_agent`] (str override) + [`maybe_build_synthetic_agent_with_map`] (dict-style) + [`ModelOverride`] enum for the `Any` union.
//! * `history: list[dict[str,str]]` → `Vec<ChatMessage>` (`role`/`content`). Python's `str(message)[:200]` truncation preserved as chars.
//! * `session_*` counters → `u64` fields.
//! * `close()` mirrors `self._interrupt.set()` (no-op teardown).

use std::collections::HashMap;
use std::env;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Constants — mirrors synthetic_turn.py env names + defaults
// ---------------------------------------------------------------------------

/// Env var that arms the seam. Mirrors `HERMES_ISO_CERTIFY_SYNTH_TURN`.
pub const ENV_SYNTH_TURN: &str = "HERMES_ISO_CERTIFY_SYNTH_TURN";

/// Per-turn wall duration env. Mirrors `HERMES_ISO_CERTIFY_DURATION_S`.
pub const ENV_DURATION_S: &str = "HERMES_ISO_CERTIFY_DURATION_S";

/// CPU chunk size env. Mirrors `HERMES_ISO_CERTIFY_CHUNK`.
pub const ENV_CHUNK: &str = "HERMES_ISO_CERTIFY_CHUNK";

/// Delta cadence env. Mirrors `HERMES_ISO_CERTIFY_DELTA_S`.
pub const ENV_DELTA_S: &str = "HERMES_ISO_CERTIFY_DELTA_S";

/// Tokens per delta env. Mirrors `HERMES_ISO_CERTIFY_TPD`.
pub const ENV_TPD: &str = "HERMES_ISO_CERTIFY_TPD";

/// Default wall duration (seconds). Mirrors `8.0` in `_parse_spec`.
pub const DEFAULT_DURATION_S: f64 = 8.0;

/// Default chunk size. Mirrors `20_000`.
pub const DEFAULT_CHUNK: i64 = 20_000;

/// Default delta interval (seconds). Mirrors `0.05`.
pub const DEFAULT_DELTA_S: f64 = 0.05;

/// Default tokens per delta. Mirrors `512`.
pub const DEFAULT_TPD: i64 = 512;

/// Default sleep per chunk (seconds). Mirrors `0.0`.
pub const DEFAULT_SLEEP_S: f64 = 0.0;

// ---------------------------------------------------------------------------
// Env helpers — mirrors _env_float / _env_int
// ---------------------------------------------------------------------------

/// Parse `raw` as `f64`, falling back to `default` on `None`/empty/`Err`.
///
/// Mirrors `tui_gateway/synthetic_turn.py::_env_float`:
///
/// ```python
/// def _env_float(name: str, default: float) -> float:
///     try: return float(os.environ.get(name, "") or default)
///     except (TypeError, ValueError): return default
/// ```
///
/// `os.environ.get(name, "") or default` → `None` or empty string → `default`.
/// Otherwise `float(raw)` → `Ok` or `default`.
pub fn env_float(raw: Option<&str>, default: f64) -> f64 {
    match raw {
        None => default,
        Some(s) if s.is_empty() => default,
        Some(s) => {
            let t = s.trim();
            if t.is_empty() {
                return default;
            }
            match t.parse::<f64>() {
                Ok(v) => v,
                Err(_) => default,
            }
        }
    }
}

/// Read `name` from the process env as `f64` with `default` fallback.
pub fn env_float_from_env(name: &str, default: f64) -> f64 {
    let raw = env::var(name).ok();
    env_float(raw.as_deref(), default)
}

/// Parse `raw` as `i64`, falling back to `default` on `None`/empty/`Err`.
///
/// Mirrors `tui_gateway/synthetic_turn.py::_env_int`:
///
/// ```python
/// def _env_int(name: str, default: int) -> int:
///     try: return int(os.environ.get(name, "") or default)
///     except (TypeError, ValueError): return default
/// ```
pub fn env_int(raw: Option<&str>, default: i64) -> i64 {
    match raw {
        None => default,
        Some(s) if s.is_empty() => default,
        Some(s) => {
            let t = s.trim();
            if t.is_empty() {
                return default;
            }
            // Python's int() handles float-like "8.0"? No, it raises. We mirror strict int parse.
            // Try int first, then fallback if it looks like float (to match Python's ValueError → default).
            match t.parse::<i64>() {
                Ok(v) => v,
                Err(_) => default,
            }
        }
    }
}

/// Read `name` from the process env as `i64` with `default` fallback.
pub fn env_int_from_env(name: &str, default: i64) -> i64 {
    let raw = env::var(name).ok();
    env_int(raw.as_deref(), default)
}

// ---------------------------------------------------------------------------
// synth_turn_armed — mirrors synth_turn_armed()
// ---------------------------------------------------------------------------

/// True when the synthetic-turn test seam is armed via env.
///
/// Mirrors `tui_gateway/synthetic_turn.py::synth_turn_armed`:
///
/// ```python
/// def synth_turn_armed() -> bool: return os.environ.get("HERMES_ISO_CERTIFY_SYNTH_TURN") == "1"
/// ```
pub fn synth_turn_armed() -> bool {
    env::var(ENV_SYNTH_TURN).as_deref() == Ok("1")
}

/// Pure helper for tests: is `raw` the armed value `"1"`?
pub fn synth_turn_armed_with(raw: Option<&str>) -> bool {
    raw == Some("1")
}

// ---------------------------------------------------------------------------
// Spec — mirrors SyntheticHeavyAgent._parse_spec
// ---------------------------------------------------------------------------

/// Per-turn intensity spec. Mirrors the dict returned by `_parse_spec`.
#[derive(Debug, Clone, PartialEq)]
pub struct SyntheticSpec {
    /// Wall-clock seconds of GIL-holding compute. Mirrors `duration_s`.
    pub duration_s: f64,
    /// Pure-Python integer ops per interrupt-check chunk. Mirrors `chunk`.
    pub chunk: i64,
    /// Streamed-delta cadence (seconds). Mirrors `delta_interval_s`.
    pub delta_interval_s: f64,
    /// Notional output tokens per delta. Mirrors `tokens_per_delta`.
    pub tokens_per_delta: i64,
    /// Optional per-chunk sleep (0 = pure burn). Mirrors `sleep_s`.
    pub sleep_s: f64,
}

impl Default for SyntheticSpec {
    fn default() -> Self {
        Self {
            duration_s: DEFAULT_DURATION_S,
            chunk: DEFAULT_CHUNK,
            delta_interval_s: DEFAULT_DELTA_S,
            tokens_per_delta: DEFAULT_TPD,
            sleep_s: DEFAULT_SLEEP_S,
        }
    }
}

impl SyntheticSpec {
    /// Build a spec from env/defaults, without any message JSON.
    ///
    /// Mirrors the fallback path of `_parse_spec` when `message` is not a JSON dict.
    pub fn from_env() -> Self {
        Self {
            duration_s: env_float_from_env(ENV_DURATION_S, DEFAULT_DURATION_S),
            chunk: env_int_from_env(ENV_CHUNK, DEFAULT_CHUNK),
            delta_interval_s: env_float_from_env(ENV_DELTA_S, DEFAULT_DELTA_S),
            tokens_per_delta: env_int_from_env(ENV_TPD, DEFAULT_TPD),
            sleep_s: DEFAULT_SLEEP_S,
        }
    }

    /// Parse `message` as JSON spec, overlaying env/defaults.
    ///
    /// Mirrors `SyntheticHeavyAgent._parse_spec`:
    ///
    /// ```python
    /// spec: dict = {}
    /// if isinstance(message, str):
    ///     text = message.strip()
    ///     if text.startswith("{"):
    ///         try: parsed = json.loads(text)
    ///              if isinstance(parsed, dict): spec = parsed
    ///         except: spec = {}
    /// return {
    ///     "duration_s": float(spec.get("duration_s", _env_float(...))),
    ///     "chunk": int(spec.get("chunk", _env_int(...))),
    ///     "delta_interval_s": float(spec.get("delta_interval_s", _env_float(...))),
    ///     "tokens_per_delta": int(spec.get("tokens_per_delta", _env_int(...))),
    ///     "sleep_s": float(spec.get("sleep_s", 0.0)),
    /// }
    /// ```
    pub fn parse(message: &str) -> Self {
        let base = Self::from_env();
        let text = message.trim();
        if !text.starts_with('{') {
            return base;
        }
        // If JSON parsing fails or root is not an object, return env/defaults.
        let Some(map) = try_parse_json_object(text) else {
            return base;
        };
        Self {
            duration_s: parse_float_field(&map, "duration_s", base.duration_s),
            chunk: parse_int_field(&map, "chunk", base.chunk),
            delta_interval_s: parse_float_field(&map, "delta_interval_s", base.delta_interval_s),
            tokens_per_delta: parse_int_field(&map, "tokens_per_delta", base.tokens_per_delta),
            sleep_s: parse_float_field(&map, "sleep_s", base.sleep_s),
        }
    }
}

// --- minimal JSON helpers (std-only, no serde) --------------------------------

/// Try to parse `s` as a flat JSON object with string keys and primitive values.
/// Returns `None` if `s` is not a JSON object or parsing fails.
/// Only the fields we care about are extracted; nested structures are ignored.
fn try_parse_json_object(s: &str) -> Option<HashMap<String, String>> {
    let s = s.trim();
    if !s.starts_with('{') || !s.ends_with('}') {
        return None;
    }
    // Quick validity: attempt to extract raw fields; if we can't find any
    // colon and it's not "{}", treat as invalid.
    // For our 5 known keys we just scan for each; if none found but object is
    // non-empty, still consider it a valid (empty) spec rather than error,
    // mirroring Python's json.loads success with unrelated keys.
    // If the string is not valid JSON at all (e.g. "{bad"), return None.
    // We do a cheap brace/quote balance check: parse with a tiny state machine.
    if !is_valid_json_object_shallow(s) {
        return None;
    }
    let mut map = HashMap::new();
    for key in ["duration_s", "chunk", "delta_interval_s", "tokens_per_delta", "sleep_s"] {
        if let Some(raw) = extract_json_number_raw(s, key) {
            map.insert(key.to_string(), raw);
        } else if let Some(raw) = extract_json_string_raw(s, key) {
            // Python json could give string values for numbers; handle "8.0" strings.
            map.insert(key.to_string(), raw);
        }
    }
    Some(map)
}

fn is_valid_json_object_shallow(s: &str) -> bool {
    // Very small validator: must start with {, end with }, quotes balanced,
    // braces balanced. Rejects "{not json" etc.
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut depth: i32 = 0;
    let mut in_str = false;
    let mut esc = false;
    for &b in bytes {
        if esc {
            esc = false;
            continue;
        }
        if in_str {
            if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    !in_str && depth == 0
}

/// Extract a numeric (or unquoted) raw value for `key` from a JSON object string.
/// Handles `"key": 123`, `"key": 1.5`, `"key": -2`, exponent etc.
fn extract_json_number_raw(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let pos = json.find(&needle)?;
    let after = &json[pos + needle.len()..];
    let colon = after.find(':')?;
    let val = after[colon + 1..].trim_start();
    if val.is_empty() {
        return None;
    }
    // If value is quoted string, let the string extractor handle it.
    if val.starts_with('"') {
        return None;
    }
    // Handle literals: true/false/null preceded by key => not our number
    if val.starts_with("true") || val.starts_with("false") || val.starts_with("null") {
        return None;
    }
    // Number: read until , or } or whitespace
    let mut end = val.len();
    for (i, ch) in val.char_indices() {
        if ch == ',' || ch == '}' || ch.is_whitespace() {
            end = i;
            break;
        }
    }
    let raw = val[..end].trim();
    if raw.is_empty() {
        return None;
    }
    // Validate it looks like a number (allow - digits . e E + -)
    let is_num = raw.chars().all(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E'));
    if !is_num || raw.chars().all(|c| matches!(c, '-' | '+' | '.' | 'e' | 'E')) {
        return None;
    }
    Some(raw.to_string())
}

fn extract_json_string_raw(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let pos = json.find(&needle)?;
    let after = &json[pos + needle.len()..];
    let colon = after.find(':')?;
    let val = after[colon + 1..].trim_start();
    if !val.starts_with('"') {
        return None;
    }
    // Extract quoted string content (unescaped minimal)
    let mut out = String::new();
    let mut chars = val[1..].chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(esc) = chars.next() {
                match esc {
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    _ => out.push(esc),
                }
            }
        } else if ch == '"' {
            return Some(out);
        } else {
            out.push(ch);
        }
    }
    None
}

fn parse_float_field(map: &HashMap<String, String>, key: &str, default: f64) -> f64 {
    match map.get(key) {
        None => default,
        Some(raw) => {
            let t = raw.trim();
            if t.is_empty() {
                return default;
            }
            match t.parse::<f64>() {
                Ok(v) => v,
                Err(_) => default,
            }
        }
    }
}

fn parse_int_field(map: &HashMap<String, String>, key: &str, default: i64) -> i64 {
    match map.get(key) {
        None => default,
        Some(raw) => {
            let t = raw.trim();
            if t.is_empty() {
                return default;
            }
            // Allow float strings like "8.0" for int fields? Python int("8.0") would raise → default.
            // So strict int parse only.
            match t.parse::<i64>() {
                Ok(v) => v,
                Err(_) => {
                    // Try parse as f64 then trunc? No - Python would ValueError → default.
                    default
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ChatMessage / TurnResult — mirrors Python dict shapes
// ---------------------------------------------------------------------------

/// A chat message. Mirrors `{"role": "...", "content": "..."}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self::new("user", content)
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new("assistant", content)
    }
}

/// Result of a synthetic turn. Mirrors the dict returned by `run_conversation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnResult {
    /// Mirrors `final_response`.
    pub final_response: String,
    /// Mirrors `messages`.
    pub messages: Vec<ChatMessage>,
    /// Mirrors `interrupted`.
    pub interrupted: bool,
    /// Mirrors `error` (always `None` in the synthetic path).
    pub error: Option<String>,
    /// Mirrors `last_reasoning` (always `None`).
    pub last_reasoning: Option<String>,
}

// ---------------------------------------------------------------------------
// SyntheticHeavyAgent — mirrors Python class
// ---------------------------------------------------------------------------

/// An `AIAgent`-shaped object whose turn is a GIL-holding CPU burn.
///
/// Presents only the surface `tui_gateway.server`'s turn path and status
/// helpers read: `run_conversation`/`interrupt`/`clear_interrupt` plus a
/// handful of `model`/`provider`/`session_*` attributes consumed by
/// `_get_usage` and `_session_info`. It never opens a socket or spawns a
/// subprocess, so the only work it does is the deterministic loop below —
/// exactly the `take_gil` regime under test.
///
/// Mirrors `tui_gateway/synthetic_turn.py::SyntheticHeavyAgent`.
#[derive(Debug)]
pub struct SyntheticHeavyAgent {
    /// Mirrors `session_id`.
    pub session_id: String,
    /// Mirrors `model` (default `"synthetic-heavy"`).
    pub model: String,
    /// Mirrors `provider` (`"synthetic"`).
    pub provider: String,
    /// Mirrors `api_mode` (`"chat_completions"`).
    pub api_mode: String,
    /// Mirrors `base_url` (`""`).
    pub base_url: String,
    /// Mirrors `api_key` (`""`).
    pub api_key: String,
    /// Mirrors `platform` (`""`).
    pub platform: String,
    /// Mirrors `tools` (empty).
    pub tools: Vec<String>,
    /// Mirrors `reasoning_config` (`None`).
    pub reasoning_config: Option<String>,
    /// Mirrors `service_tier` (`None`).
    pub service_tier: Option<String>,
    /// Mirrors `context_compressor` (`None`).
    pub context_compressor: Option<()>,
    /// Mirrors `_config_context_length` (`200_000`).
    pub config_context_length: usize,
    /// Mirrors `_cached_system_prompt` (`""`).
    pub cached_system_prompt: String,
    /// Mirrors `session_input_tokens`.
    pub session_input_tokens: u64,
    /// Mirrors `session_output_tokens`.
    pub session_output_tokens: u64,
    /// Mirrors `session_prompt_tokens`.
    pub session_prompt_tokens: u64,
    /// Mirrors `session_completion_tokens`.
    pub session_completion_tokens: u64,
    /// Mirrors `session_reasoning_tokens`.
    pub session_reasoning_tokens: u64,
    /// Mirrors `session_total_tokens`.
    pub session_total_tokens: u64,
    /// Mirrors `session_api_calls`.
    pub session_api_calls: u64,
    /// Mirrors `history: list[dict[str,str]]`.
    pub history: Vec<ChatMessage>,
    /// Mirrors `_interrupt = threading.Event()` → `AtomicBool`.
    interrupt: Arc<AtomicBool>,
}

impl Clone for SyntheticHeavyAgent {
    fn clone(&self) -> Self {
        Self {
            session_id: self.session_id.clone(),
            model: self.model.clone(),
            provider: self.provider.clone(),
            api_mode: self.api_mode.clone(),
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            platform: self.platform.clone(),
            tools: self.tools.clone(),
            reasoning_config: self.reasoning_config.clone(),
            service_tier: self.service_tier.clone(),
            context_compressor: self.context_compressor,
            config_context_length: self.config_context_length,
            cached_system_prompt: self.cached_system_prompt.clone(),
            session_input_tokens: self.session_input_tokens,
            session_output_tokens: self.session_output_tokens,
            session_prompt_tokens: self.session_prompt_tokens,
            session_completion_tokens: self.session_completion_tokens,
            session_reasoning_tokens: self.session_reasoning_tokens,
            session_total_tokens: self.session_total_tokens,
            session_api_calls: self.session_api_calls,
            history: self.history.clone(),
            interrupt: Arc::clone(&self.interrupt),
        }
    }
}

impl SyntheticHeavyAgent {
    /// Create a new agent. Mirrors `SyntheticHeavyAgent.__init__`.
    ///
    /// ```python
    /// def __init__(self, session_id: str, *, model: str = "synthetic-heavy") -> None:
    ///     self.session_id = session_id
    ///     self.model = model
    ///     self.provider = "synthetic"
    ///     ...
    ///     self._interrupt = threading.Event()
    /// ```
    pub fn new(session_id: impl Into<String>, model: Option<String>) -> Self {
        Self {
            session_id: session_id.into(),
            model: model.unwrap_or_else(|| "synthetic-heavy".to_string()),
            provider: "synthetic".to_string(),
            api_mode: "chat_completions".to_string(),
            base_url: String::new(),
            api_key: String::new(),
            platform: String::new(),
            tools: Vec::new(),
            reasoning_config: None,
            service_tier: None,
            context_compressor: None,
            config_context_length: 200_000,
            cached_system_prompt: String::new(),
            session_input_tokens: 0,
            session_output_tokens: 0,
            session_prompt_tokens: 0,
            session_completion_tokens: 0,
            session_reasoning_tokens: 0,
            session_total_tokens: 0,
            session_api_calls: 0,
            history: Vec::new(),
            interrupt: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Convenience: `SyntheticHeavyAgent(session_id)` with default model.
    pub fn with_session(session_id: impl Into<String>) -> Self {
        Self::new(session_id, None)
    }

    // ── interrupt contract (mirrors AIAgent) ───────────────────────────────

    /// Mirrors `clear_interrupt`: `self._interrupt.clear()`.
    pub fn clear_interrupt(&self) {
        self.interrupt.store(false, Ordering::SeqCst);
    }

    /// Mirrors `interrupt`: `self._interrupt.set()`.
    pub fn interrupt(&self) {
        self.interrupt.store(true, Ordering::SeqCst);
    }

    /// Mirrors `_has_stream_consumers`: always `true` (defensive; not used by loop).
    pub fn has_stream_consumers(&self) -> bool {
        true
    }

    /// No-op teardown (session lifecycle calls `agent.close()` on some paths).
    ///
    /// Mirrors `def close(self) -> None: self._interrupt.set()`.
    pub fn close(&self) {
        self.interrupt.store(true, Ordering::SeqCst);
    }

    /// Whether an interrupt has been requested.
    pub fn is_interrupted(&self) -> bool {
        self.interrupt.load(Ordering::SeqCst)
    }

    /// Shareable handle to the interrupt flag (so other threads can signal).
    pub fn interrupt_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.interrupt)
    }

    // ── spec parsing ───────────────────────────────────────────────────────

    /// Parse `message` into a [`SyntheticSpec`].
    ///
    /// Mirrors `SyntheticHeavyAgent._parse_spec` (static method).
    pub fn parse_spec(message: &str) -> SyntheticSpec {
        SyntheticSpec::parse(message)
    }

    // ── the turn ───────────────────────────────────────────────────────────

    /// Run a synthetic heavy turn.
    ///
    /// Mirrors `SyntheticHeavyAgent.run_conversation`:
    ///
    /// * `duration_s` clamped `max(0.0, spec["duration_s"])`
    /// * `chunk` clamped `max(1, spec["chunk"])`
    /// * `interval` clamped `max(0.001, spec["delta_interval_s"])`
    /// * `tokens_per_delta` clamped `max(0, spec["tokens_per_delta"])`
    /// * `sleep_s` clamped `max(0.0, spec["sleep_s"])`
    /// * `base_history = conversation_history if not None else self.history`
    /// * monotonic clock (`Instant::now()` mirrors `time.monotonic()`)
    /// * tight integer loop `acc = (acc * 1_000_003 + 12_345) & 0xFFFFFFFFFFFFFFFF`
    ///   (wrapping 64-bit) — the GIL-holding hot loop
    /// * `stream_callback` invoked as `synthtok-{deltas:05d} ` each interval
    /// * token counters bumped, `session_api_calls += 1`, deterministic `final`
    ///   with `deltas`, `out_tokens`, `interrupted`, `checksum` (`acc & 0xFFFF` as `04x`)
    /// * `messages = [*base_history, {"role":"user", ...}, {"role":"assistant", ...}]`
    /// * `self.history = messages`, returns `final_response`/`messages`/`interrupted`/`error`/`last_reasoning`
    pub fn run_conversation<F>(
        &mut self,
        message: &str,
        conversation_history: Option<&[ChatMessage]>,
        mut stream_callback: Option<F>,
        _task_id: Option<&str>,
    ) -> TurnResult
    where
        F: FnMut(&str),
    {
        let spec = Self::parse_spec(message);
        let duration = spec.duration_s.max(0.0);
        let chunk = (spec.chunk.max(1)) as usize;
        let interval = spec.delta_interval_s.max(0.001);
        let tokens_per_delta = (spec.tokens_per_delta.max(0)) as u64;
        let sleep_s = spec.sleep_s.max(0.0);

        let base_history: Vec<ChatMessage> = match conversation_history {
            Some(h) => h.to_vec(),
            None => self.history.clone(),
        };

        let start = Instant::now();
        let mut last_delta = start;
        let mut acc: u64 = 0;
        let mut deltas: u64 = 0;
        let mut interrupted = false;

        loop {
            if self.interrupt.load(Ordering::SeqCst) {
                interrupted = true;
                break;
            }
            let now = Instant::now();
            if now.duration_since(start).as_secs_f64() >= duration {
                break;
            }
            // GIL-holding pure-Python work. A tight integer loop runs one
            // bytecode step per iteration and NEVER releases the GIL — this is
            // the exact interpreter contention that starves the serving loop.
            for _ in 0..chunk {
                acc = acc.wrapping_mul(1_000_003).wrapping_add(12_345);
            }
            if sleep_s > 0.0 {
                thread::sleep(Duration::from_secs_f64(sleep_s));
            }
            if now.duration_since(last_delta).as_secs_f64() >= interval {
                deltas += 1;
                self.session_output_tokens = self.session_output_tokens.wrapping_add(tokens_per_delta);
                self.session_completion_tokens = self.session_completion_tokens.wrapping_add(tokens_per_delta);
                self.session_total_tokens = self.session_total_tokens.wrapping_add(tokens_per_delta);
                if let Some(cb) = stream_callback.as_mut() {
                    // Mirrors stream_callback(f"synthtok-{deltas:05d} ")
                    let delta_str = format!("synthtok-{deltas:05} ");
                    cb(&delta_str);
                }
                last_delta = now;
            }
        }

        self.session_api_calls = self.session_api_calls.wrapping_add(1);
        // Fold the checksum into the reply so the loop is not dead-code-eliminated
        // and the turn produces a deterministic, inspectable result.
        let checksum = acc & 0xFFFF;
        let final_response = format!(
            "[synthetic heavy turn] deltas={deltas} out_tokens={} interrupted={interrupted} checksum={checksum:04x}",
            self.session_output_tokens
        );
        // Python: str(message)[:200] — chars truncated, message may be Any.
        let user_content: String = message.chars().take(200).collect();
        let mut messages = base_history;
        messages.push(ChatMessage::user(user_content));
        messages.push(ChatMessage::assistant(final_response.clone()));
        self.history = messages.clone();
        TurnResult {
            final_response,
            messages,
            interrupted,
            error: None,
            last_reasoning: None,
        }
    }

    /// Trait-object variant for callers that cannot name the closure type.
    ///
    /// Mirrors the same logic but takes `Option<&mut dyn FnMut(&str)>`.
    pub fn run_conversation_boxed(
        &mut self,
        message: &str,
        conversation_history: Option<&[ChatMessage]>,
        stream_callback: Option<&mut dyn FnMut(&str)>,
        task_id: Option<&str>,
    ) -> TurnResult {
        // Delegate to generic impl via an adapter closure
        let mut cb_opt = stream_callback;
        self.run_conversation(message, conversation_history, cb_opt.as_deref_mut(), task_id)
    }
}

// ---------------------------------------------------------------------------
// maybe_build_synthetic_agent — mirrors Python function
// ---------------------------------------------------------------------------

/// What `model_override` may be. Mirrors `model_override: Any = None` where
/// `Any` is `dict` with `"model"` or `str`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelOverride {
    /// No override (None).
    None,
    /// String override (`str`).
    Str(String),
    /// Dict override (`{"model": "..."}`).
    Map(HashMap<String, String>),
}

impl ModelOverride {
    /// Build from an optional string (convenience).
    pub fn from_str_opt(s: Option<&str>) -> Self {
        match s {
            None => Self::None,
            Some(v) if v.is_empty() => Self::None,
            Some(v) => Self::Str(v.to_string()),
        }
    }

    /// Build from a map (e.g. `{"model": "foo"}`).
    pub fn from_map(map: HashMap<String, String>) -> Self {
        if map.get("model").is_some_and(|v| !v.is_empty()) {
            Self::Map(map)
        } else {
            Self::None
        }
    }

    fn resolve_model(&self) -> Option<String> {
        match self {
            Self::None => None,
            Self::Str(s) if s.is_empty() => None,
            Self::Str(s) => Some(s.clone()),
            Self::Map(m) => m.get("model").filter(|v| !v.is_empty()).cloned(),
        }
    }
}

/// Return a `SyntheticHeavyAgent` when the seam is armed, else `None`.
///
/// Mirrors `tui_gateway/synthetic_turn.py::maybe_build_synthetic_agent`:
///
/// ```python
/// def maybe_build_synthetic_agent(session_id: str, model_override: Any = None) -> SyntheticHeavyAgent | None:
///     if not synth_turn_armed(): return None
///     model = "synthetic-heavy"
///     if isinstance(model_override, dict) and model_override.get("model"): model = str(model_override["model"])
///     elif isinstance(model_override, str) and model_override: model = model_override
///     return SyntheticHeavyAgent(session_id, model=model)
/// ```
pub fn maybe_build_synthetic_agent(
    session_id: &str,
    model_override: Option<&str>,
) -> Option<SyntheticHeavyAgent> {
    maybe_build_synthetic_agent_with_override(session_id, &ModelOverride::from_str_opt(model_override))
}

/// Full `Any`-mirroring variant that accepts the [`ModelOverride`] enum.
pub fn maybe_build_synthetic_agent_with_override(
    session_id: &str,
    model_override: &ModelOverride,
) -> Option<SyntheticHeavyAgent> {
    if !synth_turn_armed() {
        return None;
    }
    let model = model_override
        .resolve_model()
        .unwrap_or_else(|| "synthetic-heavy".to_string());
    Some(SyntheticHeavyAgent::new(session_id, Some(model)))
}

/// Dict-style convenience: `model_override` as `HashMap<String,String>`.
pub fn maybe_build_synthetic_agent_with_map(
    session_id: &str,
    model_override: Option<&HashMap<String, String>>,
) -> Option<SyntheticHeavyAgent> {
    if !synth_turn_armed() {
        return None;
    }
    let mut model = "synthetic-heavy".to_string();
    if let Some(map) = model_override {
        if let Some(m) = map.get("model") {
            if !m.is_empty() {
                model = m.clone();
            }
        }
    }
    Some(SyntheticHeavyAgent::new(session_id, Some(model)))
}

/// Pure helper for tests (inject `armed` without touching env).
pub fn maybe_build_synthetic_agent_with_armed(
    session_id: &str,
    model_override: &ModelOverride,
    armed: bool,
) -> Option<SyntheticHeavyAgent> {
    if !armed {
        return None;
    }
    let model = model_override
        .resolve_model()
        .unwrap_or_else(|| "synthetic-heavy".to_string());
    Some(SyntheticHeavyAgent::new(session_id, Some(model)))
}
