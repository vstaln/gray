//! LM Studio reasoning-effort resolution shared by the chat-completions
//! transport and run_agent's iteration-limit summary path.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/lmstudio_reasoning.py` (60 lines).
//!
//! LM Studio publishes per-model `capabilities.reasoning.allowed_options` (e.g.
//! `["off","on"]` for toggle-style models, `["off","minimal","low"]` for
//! graduated models). We map the user's `reasoning_config` onto LM Studio's
//! OpenAI-compatible vocabulary, then clamp against the model's allowed set so
//! the server doesn't 400 on an unsupported effort.
//!
//! Python source docstring (preserved):
//! ```text
//! LM Studio reasoning-effort resolution shared by the chat-completions
//! transport and run_agent's iteration-limit summary path.
//!
//! LM Studio publishes per-model ``capabilities.reasoning.allowed_options`` (e.g.
//! ``["off","on"]`` for toggle-style models, ``["off","minimal","low"]`` for
//! graduated models). We map the user's ``reasoning_config`` onto LM Studio's
//! OpenAI-compatible vocabulary, then clamp against the model's allowed set so
//! the server doesn't 400 on an unsupported effort.
//! ```

use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Constants — mirrors lines 17-32
// ---------------------------------------------------------------------------

/// LM Studio accepts these top-level reasoning_effort values via its
/// OpenAI-compatible chat.completions endpoint.
/// Mirrors `_LM_VALID_EFFORTS = {"none", "minimal", "low", "medium", "high", "xhigh"}` (line 17).
const LM_VALID_EFFORTS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh"];

/// Toggle-style models publish allowed_options as ["off","on"] in /api/v1/models.
/// Map them onto the OpenAI-compatible request vocabulary.
/// Mirrors `_LM_EFFORT_ALIASES = {"off": "none", "on": "medium"}` (line 21).
fn lm_effort_alias(raw: &str) -> Option<&'static str> {
    match raw {
        "off" => Some("none"),
        "on" => Some("medium"),
        _ => None,
    }
}

/// Hermes' generic effort ladder grew past LM Studio's vocabulary ("max",
/// "ultra"). Clamp the stronger generic levels onto LM Studio's ceiling: left
/// alone they miss _LM_VALID_EFFORTS, keep the initialized "medium" default and
/// are thereby conflated with unparseable input, so asking for more reasoning
/// yields less than "xhigh". Mirrors the ceiling clamp every other provider
/// applies (see agent/transports/codex.py).
///
/// Deliberately separate from _LM_EFFORT_ALIASES: that mapping is also applied
/// to the model's published allowed_options, which must not be rewritten.
/// Mirrors `_LM_EFFORT_CLAMP = {"max": "xhigh", "ultra": "xhigh"}` (line 32).
fn lm_effort_clamp(raw: &str) -> Option<&'static str> {
    match raw {
        "max" => Some("xhigh"),
        "ultra" => Some("xhigh"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// ReasoningConfig — mirrors `reasoning_config: Optional[dict]` (lines 36-37)
// ---------------------------------------------------------------------------

/// Mirrors Python `reasoning_config: Optional[dict]` with `{"enabled": bool, "effort": str}`.
///
/// Python checks `reasoning_config.get("enabled") is False` (strict identity)
/// and `(reasoning_config.get("effort") or "").strip().lower()`. This struct
/// captures the same two keys with Rust types; `None` mirrors a missing key.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReasoningConfig {
    /// Mirrors `reasoning_config.get("enabled")`. `Some(false)` is the only
    /// value that triggers the `"none"` path; `None` / `Some(true)` fall
    /// through to effort parsing.
    pub enabled: Option<bool>,
    /// Mirrors `reasoning_config.get("effort")`. `None` mirrors missing/None.
    pub effort: Option<String>,
}

impl ReasoningConfig {
    pub fn new(enabled: Option<bool>, effort: Option<impl Into<String>>) -> Self {
        Self {
            enabled,
            effort: effort.map(Into::into),
        }
    }

    pub fn with_effort(effort: impl Into<String>) -> Self {
        Self {
            enabled: None,
            effort: Some(effort.into()),
        }
    }

    pub fn disabled() -> Self {
        Self {
            enabled: Some(false),
            effort: None,
        }
    }
}

// ---------------------------------------------------------------------------
// resolve_lmstudio_effort — mirrors lines 35-60
// ---------------------------------------------------------------------------

/// Return the `reasoning_effort` string to send to LM Studio, or `None`.
///
/// `None` means "omit the field": the user picked a level the model can't
/// honor, so let LM Studio fall back to the model's declared default rather
/// than silently substituting a different effort. When `allowed_options` is
/// falsy (probe failed), skip clamping and send the resolved effort anyway.
///
/// Mirrors `resolve_lmstudio_effort` (lines 35-60):
/// ```python
/// def resolve_lmstudio_effort(
///     reasoning_config: Optional[dict],
///     allowed_options: Optional[List[str]],
/// ) -> Optional[str]:
///     effort = "medium"
///     if reasoning_config and isinstance(reasoning_config, dict):
///         if reasoning_config.get("enabled") is False:
///             effort = "none"
///         else:
///             raw = (reasoning_config.get("effort") or "").strip().lower()
///             raw = _LM_EFFORT_ALIASES.get(raw, raw)
///             raw = _LM_EFFORT_CLAMP.get(raw, raw)
///             if raw in _LM_VALID_EFFORTS:
///                 effort = raw
///     if allowed_options:
///         allowed = {_LM_EFFORT_ALIASES.get(opt, opt) for opt in allowed_options}
///         if effort not in allowed:
///             return None
///     return effort
/// ```
pub fn resolve_lmstudio_effort(
    reasoning_config: Option<&ReasoningConfig>,
    allowed_options: Option<&[String]>,
) -> Option<String> {
    // Mirrors `effort = "medium"` (line 46)
    let mut effort = "medium".to_string();

    // Mirrors `if reasoning_config and isinstance(reasoning_config, dict):` (line 47)
    // In Rust the type system guarantees dict-shape; `Some` mirrors truthy dict.
    // Empty dict is falsy in Python (keeps "medium"); in Rust an empty
    // ReasoningConfig (both None) reaches the `else` branch but parses to the
    // same result (raw "" not in valid -> keeps "medium"), so behaviour is identical.
    if let Some(cfg) = reasoning_config {
        // Mirrors `if reasoning_config.get("enabled") is False: effort = "none"` (lines 48-49)
        if cfg.enabled == Some(false) {
            effort = "none".to_string();
        } else {
            // Mirrors `raw = (reasoning_config.get("effort") or "").strip().lower()` (line 51)
            let raw_lower = cfg
                .effort
                .as_deref()
                .unwrap_or("")
                .trim()
                .to_lowercase();

            // Mirrors `raw = _LM_EFFORT_ALIASES.get(raw, raw)` (line 52)
            let after_alias = lm_effort_alias(raw_lower.as_str())
                .map(|s| s.to_string())
                .unwrap_or(raw_lower);

            // Mirrors `raw = _LM_EFFORT_CLAMP.get(raw, raw)` (line 53)
            let after_clamp = lm_effort_clamp(after_alias.as_str())
                .map(|s| s.to_string())
                .unwrap_or(after_alias);

            // Mirrors `if raw in _LM_VALID_EFFORTS: effort = raw` (lines 54-55)
            if LM_VALID_EFFORTS.contains(&after_clamp.as_str()) {
                effort = after_clamp;
            }
        }
    }

    // Mirrors `if allowed_options:` (line 56) — falsy when None or empty
    if let Some(opts) = allowed_options {
        if !opts.is_empty() {
            // Mirrors `allowed = {_LM_EFFORT_ALIASES.get(opt, opt) for opt in allowed_options}` (line 57)
            let allowed: HashSet<String> = opts
                .iter()
                .map(|opt| {
                    lm_effort_alias(opt.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| opt.clone())
                })
                .collect();

            // Mirrors `if effort not in allowed: return None` (lines 58-59)
            if !allowed.contains(&effort) {
                return None;
            }
        }
    }

    // Mirrors `return effort` (line 60)
    Some(effort)
}

/// Convenience overload accepting `&[&str]` for `allowed_options`.
/// Mirrors the same Python logic; useful for tests/callers without owned `String`s.
pub fn resolve_lmstudio_effort_strs(
    reasoning_config: Option<&ReasoningConfig>,
    allowed_options: Option<&[&str]>,
) -> Option<String> {
    let owned: Option<Vec<String>> = allowed_options.map(|opts| opts.iter().map(|s| s.to_string()).collect());
    resolve_lmstudio_effort(reasoning_config, owned.as_deref())
}

// Keep underscore-prefixed aliases for 1:1 traceability with Python private names.
#[allow(dead_code)]
const _LM_VALID_EFFORTS: &[&str] = LM_VALID_EFFORTS;
#[allow(dead_code)]
fn _lm_effort_alias(raw: &str) -> Option<&'static str> {
    lm_effort_alias(raw)
}
#[allow(dead_code)]
fn _lm_effort_clamp(raw: &str) -> Option<&'static str> {
    lm_effort_clamp(raw)
}
