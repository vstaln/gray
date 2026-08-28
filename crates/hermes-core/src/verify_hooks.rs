//! Verification-loop helpers for the ``pre_verify`` round-end gate.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/verify_hooks.py` (69 lines).
//!
//! When the agent has edited code and is about to verify/finish, the loop fires the
//! ``pre_verify`` hook (user directives resolved by
//! :func:`hermes_cli.plugins.get_pre_verify_continue_message`). A directive keeps
//! the agent going one more turn — run a check, defer it, tidy the diff — instead of
//! stopping immediately.
//!
//! The shipped coding guidance lives on the evidence-based verification-stop nudge
//! (``agent/verification_stop.py``), not as a second default stop gate. That keeps
//! the default token cost tied to the existing "missing verification evidence"
//! decision while preserving ``pre_verify`` for user/plugin policy.
//!
//! Python source docstring (preserved):
//! ```text
//! Verification-loop helpers for the ``pre_verify`` round-end gate.
//!
//! When the agent has edited code and is about to verify/finish, the loop fires the
//! ``pre_verify`` hook (user directives resolved by
//! :func:`hermes_cli.plugins.get_pre_verify_continue_message`). A directive keeps
//! the agent going one more turn — run a check, defer it, tidy the diff — instead of
//! stopping immediately.
//!
//! The shipped coding guidance lives on the evidence-based verification-stop nudge
//! (``agent/verification_stop.py``), not as a second default stop gate. That keeps
//! the default token cost tied to the existing "missing verification evidence"
//! decision while preserving ``pre_verify`` for user/plugin policy.
//! ```

use serde_json::Value;

// ---------------------------------------------------------------------------
// Constants — mirrors lines 21-32
// ---------------------------------------------------------------------------

/// Bound on consecutive ``pre_verify`` continue directives per turn (>= 0).
/// Mirrors `DEFAULT_MAX_VERIFY_NUDGES = 3` (line 21).
pub const DEFAULT_MAX_VERIFY_NUDGES: i64 = 3;

// Keep underscore-prefixed alias for 1:1 traceability with Python name.
#[allow(dead_code)]
const _DEFAULT_MAX_VERIFY_NUDGES: i64 = DEFAULT_MAX_VERIFY_NUDGES;

/// Shipped guidance appended to the verification-stop nudge when code lacks fresh
/// verification evidence. Wording mirrors the user-facing "clean your work"
/// workflow, but does not create its own extra model turn.
///
/// Mirrors `CODING_VERIFY_GUIDANCE = (...)` (lines 26-32).
pub const CODING_VERIFY_GUIDANCE: &str = "[Coding] Before you run tests/linters or call this done: if this is creative UI/visual work, hold off on tests and linters until the user says they like the result or you're about to commit. And before every commit, clean your work: keep it KISS/DRY, match the surrounding code style, and be elitist, shorthand, clever, concise, efficient, and elegant.";

#[allow(dead_code)]
const _CODING_VERIFY_GUIDANCE: &str = CODING_VERIFY_GUIDANCE;

// ---------------------------------------------------------------------------
// is_truthy_value — mirrors `utils.is_truthy_value` (utils.py lines 20-31)
// `from utils import is_truthy_value` (line 19)
// ---------------------------------------------------------------------------

/// Mirrors `TRUTHY_STRINGS = frozenset({"1", "true", "yes", "on"})` (utils.py line 20).
const TRUTHY_STRINGS: &[&str] = &["1", "true", "yes", "on"];

/// Mirrors `is_truthy_value(value, default=False)` (utils.py lines 23-31).
///
/// - `bool` returns as-is.
/// - `str` trimmed lowercased membership in `TRUTHY_STRINGS`.
/// - other types: `bool(value)` → numbers 0→false else true; arrays/objects empty→false.
/// - `Null` is treated as false here; missing-key default is handled by `is_truthy_value_with_default`.
pub fn is_truthy_value(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => TRUTHY_STRINGS.contains(&s.trim().to_lowercase().as_str()),
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

#[allow(dead_code)]
fn _is_truthy_value(value: &Value) -> bool {
    is_truthy_value(value)
}

/// Mirrors `is_truthy_value(value, default=...)` default handling (utils.py lines 23-31).
///
/// `None` (missing key) or `Value::Null` (explicit null) returns `default`,
/// matching Python `if value is None: return default`.
pub fn is_truthy_value_with_default(value: Option<&Value>, default: bool) -> bool {
    match value {
        None => default,
        Some(v) if v.is_null() => default,
        Some(v) => is_truthy_value(v),
    }
}

#[allow(dead_code)]
fn _is_truthy_value_with_default(value: Option<&Value>, default: bool) -> bool {
    is_truthy_value_with_default(value, default)
}

// ---------------------------------------------------------------------------
// _agent_cfg — mirrors lines 52-61
// ---------------------------------------------------------------------------

/// Mirrors `_agent_cfg(config)` (lines 52-61):
/// ```python
/// def _agent_cfg(config: Optional[dict[str, Any]]) -> dict[str, Any]:
///     if config is None:
///         try:
///             from hermes_cli.config import load_config
///             config = load_config()
///         except Exception:
///             config = {}
///     agent_cfg = (config or {}).get("agent") if isinstance(config, dict) else None
///     return agent_cfg if isinstance(agent_cfg, dict) else {}
/// ```
///
/// In Rust `config` is `Option<&Value>` (None = Python `None`). `load_config`
/// is not linked in this crate; the `None` path returns an empty object, which
/// preserves the default-value contract (Python falls back to `{}` on any
/// `load_config` exception).
pub fn agent_cfg(config: Option<&Value>) -> Value {
    let cfg = match config {
        None => return Value::Object(Default::default()),
        Some(v) => v,
    };
    if let Some(obj) = cfg.as_object() {
        if let Some(agent_val) = obj.get("agent") {
            if let Some(agent_obj) = agent_val.as_object() {
                return Value::Object(agent_obj.clone());
            }
        }
        Value::Object(Default::default())
    } else {
        Value::Object(Default::default())
    }
}

#[allow(dead_code)]
fn _agent_cfg(config: Option<&Value>) -> Value {
    agent_cfg(config)
}

// ---------------------------------------------------------------------------
// Public API — mirrors lines 35-49
// ---------------------------------------------------------------------------

/// Bound on consecutive ``pre_verify`` continue directives per turn (>= 0).
///
/// Mirrors `max_verify_nudges(config=None) -> int` (lines 35-42):
/// ```python
/// def max_verify_nudges(config: Optional[dict[str, Any]] = None) -> int:
///     agent_cfg = _agent_cfg(config)
///     raw = agent_cfg.get("max_verify_nudges")
///     try:
///         return max(0, int(raw))
///     except (TypeError, ValueError):
///         return DEFAULT_MAX_VERIFY_NUDGES
/// ```
pub fn max_verify_nudges(config: Option<&Value>) -> i64 {
    let agent = agent_cfg(config);
    let raw_opt = agent.get("max_verify_nudges");
    match raw_opt {
        None => DEFAULT_MAX_VERIFY_NUDGES,
        Some(raw) => match parse_int_raw(raw) {
            Some(n) => n.max(0),
            None => DEFAULT_MAX_VERIFY_NUDGES,
        },
    }
}

#[allow(dead_code)]
fn _max_verify_nudges(config: Option<&Value>) -> i64 {
    max_verify_nudges(config)
}

/// Return the optional guidance appended to verification-stop nudges.
///
/// Mirrors `coding_verify_guidance(config=None) -> Optional[str]` (lines 45-49):
/// ```python
/// def coding_verify_guidance(config: Optional[dict[str, Any]] = None) -> Optional[str]:
///     if not is_truthy_value(_agent_cfg(config).get("verify_guidance", True), default=True):
///         return None
///     return CODING_VERIFY_GUIDANCE
/// ```
pub fn coding_verify_guidance(config: Option<&Value>) -> Option<&'static str> {
    let agent = agent_cfg(config);
    let raw = agent.get("verify_guidance");
    if !is_truthy_value_with_default(raw, true) {
        return None;
    }
    Some(CODING_VERIFY_GUIDANCE)
}

#[allow(dead_code)]
fn _coding_verify_guidance(config: Option<&Value>) -> Option<&'static str> {
    coding_verify_guidance(config)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Mirrors `int(raw)` coercion with `TypeError`/`ValueError` fallback (lines 39-42).
///
/// - `bool` → 1/0 (Python `int(True)==1`)
/// - `Number` → truncates floats toward zero (Python `int(3.7)==3`)
/// - `String` → trimmed `parse::<i64>()`, fails on float strings (Python `int("3.0")` raises)
/// - other → `None` (Python `TypeError`)
fn parse_int_raw(value: &Value) -> Option<i64> {
    match value {
        Value::Bool(b) => Some(if *b { 1 } else { 0 }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i)
            } else if let Some(u) = n.as_u64() {
                if u <= i64::MAX as u64 {
                    Some(u as i64)
                } else {
                    None
                }
            } else if let Some(f) = n.as_f64() {
                Some(f.trunc() as i64)
            } else {
                None
            }
        }
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                t.parse::<i64>().ok()
            }
        }
        _ => None,
    }
}

#[allow(dead_code)]
fn _parse_int_raw(value: &Value) -> Option<i64> {
    parse_int_raw(value)
}
