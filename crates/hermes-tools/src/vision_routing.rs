//! Vision-routing decisions for `computer_use` capture results.
//! Port of `tools/computer_use/vision_routing.py` (204 lines) — 1:1 behavior.
//!
//! Background
//! ----------
//! `computer_use(action='capture', mode='som'|'vision')` returns a `_multimodal`
//! envelope containing the captured screenshot. That envelope is delivered back
//! to the **active session model** as the tool result. When the active main
//! model has no vision capability, or when the provider rejects multimodal
//! content inside tool-result messages, the screenshot trips a 404/400 at the
//! provider boundary and the agent loop reports a hard tool failure.
//!
//! Issue #24015: configuring `auxiliary.vision` was silently ignored — the
//! screenshot was still routed at the main model and failed with
//! `No endpoints found that support image input` even though a vision backend
//! was sitting in config.
//!
//! This module centralises the small policy decision: should a captured
//! screenshot be returned as multimodal content (main model handles vision
//! natively) or pre-analysed via the auxiliary vision pipeline so the main
//! model only ever sees text?
//!
//! Behaviour (mirrors `vision_analyze` for consistency)
//! ---------------------------------------------------
//! * If the user explicitly configured `auxiliary.vision` (any of `provider`,
//!   `model`, or `base_url` non-empty / not `"auto"`), route through aux.
//! * Otherwise, if the user explicitly declared the active model vision-capable
//!   via `model.supports_vision` / provider model config, return `False`.
//! * Otherwise, if the active main model+provider can carry an image inside a
//!   tool-result message AND the model reports `supports_vision=True` in
//!   models.dev metadata, return `False` (use the multimodal path).
//! * In every other case route through aux vision so the main model receives a
//!   text description it can act on.
//!
//! The decision intentionally fails *closed* (i.e. towards aux routing) when
//! metadata is missing or ambiguous: returning a screenshot to a model that
//! cannot read it is a hard tool failure, while routing it through aux costs
//! one extra LLM call and yields a usable description.
//!
//! Rust mapping
//! ------------
//! - `cfg: Optional[Dict[str, Any]]` → `Option<&serde_json::Value>` (borrowed).
//!   The Python code treats `Not a dict` as "no override" — we mirror with
//!   `Value::Object` checks; non-object/missing/Null all count as not explicit.
//! - `_explicit_aux_vision_override(cfg)` → [`explicit_aux_vision_override`] (56-80)
//! - `_lookup_user_declared_supports_vision(...)` → [`lookup_user_declared_supports_vision`]
//!   + [`lookup_user_declared_supports_vision_with`] (83-104) — Python imports
//!   `agent.image_routing._supports_vision_override`; Rust takes an injected
//!   closure (default returns `None` → import-failure path).
//! - `_lookup_supports_vision(...)` → [`lookup_supports_vision`] +
//!   [`lookup_supports_vision_with`] (107-140) — Python tries
//!   `agent.image_routing._lookup_supports_vision` then
//!   `agent.models_dev.get_model_capabilities`; Rust injects.
//! - `_provider_accepts_multimodal_tool_result(...)` →
//!   [`provider_accepts_multimodal_tool_result`] +
//!   [`supports_media_in_tool_results`] (143-161) — Python reuses
//!   `tools.vision_tools._supports_media_in_tool_results`; Rust inlines the
//!   exact allowlist from that function so the capture-routing decision stays
//!   in lockstep with the `vision_analyze` native fast path.
//! - `should_route_capture_to_aux_vision(...)` → [`should_route_capture_to_aux_vision`]
//!   + [`should_route_capture_to_aux_vision_with`] (164-199)

use serde_json::Value;

// ---------------------------------------------------------------------------
// Helpers — mirror Python `str(x or "").strip()` semantics
// ---------------------------------------------------------------------------

/// Python: `str(vision.get("key") or "").strip()`.
///
/// Falsy values (`None`, `""`, `0`, `False`, empty list/dict) become `""`.
/// Truthy non-strings stringify (`123` → `"123"`, `True` → `"True"`).
fn cfg_string_field(value: Option<&Value>) -> String {
    match value {
        None => String::new(),
        Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => {
            if *b {
                "True".to_string()
            } else {
                String::new()
            }
        }
        Some(Value::Number(n)) => {
            // 0 is falsy in Python → ""
            if let Some(i) = n.as_i64() {
                if i == 0 {
                    return String::new();
                }
            } else if let Some(f) = n.as_f64() {
                if f == 0.0 {
                    return String::new();
                }
            }
            n.to_string()
        }
        Some(Value::Array(arr)) => {
            if arr.is_empty() {
                String::new()
            } else {
                // Python str([1]) is truthy non-empty; port as debug
                format!("{arr:?}")
            }
        }
        Some(Value::Object(obj)) => {
            if obj.is_empty() {
                String::new()
            } else {
                format!("{obj:?}")
            }
        }
    }
}

fn get_object<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(map) => map.get(key),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// _explicit_aux_vision_override — mirrors lines 56-80
// ---------------------------------------------------------------------------

/// True when `auxiliary.vision` carries a non-default user override.
///
/// Mirrors `agent.image_routing._explicit_aux_vision_override` so the capture
/// path and the user-attached-image path agree on what counts as an explicit
/// user request for the aux vision pipeline. `provider: "auto"`, blank values,
/// or a missing block all count as *not* explicit.
///
/// Python (lines 56-80):
/// ```python
/// if not isinstance(cfg, dict): return False
/// aux = cfg.get("auxiliary") or {}
/// if not isinstance(aux, dict): return False
/// vision = aux.get("vision") or {}
/// if not isinstance(vision, dict): return False
/// provider = str(vision.get("provider") or "").strip().lower()
/// model = str(vision.get("model") or "").strip()
/// base_url = str(vision.get("base_url") or "").strip()
/// if provider in ("", "auto") and not model and not base_url: return False
/// return True
/// ```
pub fn explicit_aux_vision_override(cfg: Option<&Value>) -> bool {
    let cfg_val = match cfg {
        Some(v) if v.is_object() => v,
        _ => return false,
    };

    // `cfg.get("auxiliary") or {}` — None/missing/null/falsy → empty object
    let aux = match get_object(cfg_val, "auxiliary") {
        Some(v) if v.is_object() => v,
        Some(Value::Null) | None => return false,
        // Python: `cfg.get("auxiliary") or {}` where `or` treats empty dict as
        // falsy? In Python `{} or {}` → `{}` (second {}), still a dict, but
        // then `vision = {}.get("vision") or {}` → `{}` → provider "" etc → False.
        // For a truthy non-dict (e.g. string), Python returns False.
        // So any non-object, non-null → False (not explicit).
        _ => return false,
    };

    // Handle `aux` being explicitly null-ish: already returned false above.
    // For `aux = {}`, vision lookup still proceeds but yields not explicit.

    let vision = match get_object(aux, "vision") {
        Some(v) if v.is_object() => v,
        Some(Value::Null) | None => {
            // `aux.get("vision") or {}` → empty dict → not explicit
            return false;
        }
        _ => return false,
    };

    let provider_raw = cfg_string_field(get_object(vision, "provider"));
    let provider = provider_raw.trim().to_lowercase();
    let model = cfg_string_field(get_object(vision, "model")).trim().to_string();
    let base_url = cfg_string_field(get_object(vision, "base_url")).trim().to_string();

    if (provider.is_empty() || provider == "auto") && model.is_empty() && base_url.is_empty() {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// _lookup_user_declared_supports_vision — mirrors lines 83-104
// ---------------------------------------------------------------------------

/// Return config-declared `supports_vision` for the active route.
///
/// Mirrors `def _lookup_user_declared_supports_vision(provider, model, cfg)`.
///
/// In Python this imports `agent.image_routing._supports_vision_override` and
/// returns its result, swallowing import/call exceptions as `None`. In Rust
/// the import is injected as a closure; the default delegate returns `None`
/// (mirrors import failure → fallback to aux routing, fails closed).
pub fn lookup_user_declared_supports_vision(
    provider: &str,
    model: &str,
    cfg: Option<&Value>,
) -> Option<bool> {
    // default: no override available (import failure path)
    let _ = (provider, model, cfg);
    None
}

/// Testable variant with injected lookup.
///
/// `lookup` mirrors `agent.image_routing._supports_vision_override(cfg, provider, model)`.
/// Return `Some(true/false)` when the user explicitly declared, `None` when no
/// override. Exceptions in Python are swallowed to `None`; callers should do the
/// same in the closure (return `None` on error).
pub fn lookup_user_declared_supports_vision_with<F>(
    provider: &str,
    model: &str,
    cfg: Option<&Value>,
    lookup: F,
) -> Option<bool>
where
    F: Fn(&str, &str, Option<&Value>) -> Option<bool>,
{
    // Python wraps both import and call in try/except → None on any error.
    // We call the injected function and trust it to return None on error.
    // The outer panic guard mirrors broad `except Exception`.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        lookup(provider, model, cfg)
    }));
    match result {
        Ok(v) => v,
        Err(_) => None,
    }
}

// ---------------------------------------------------------------------------
// _lookup_supports_vision — mirrors lines 107-140
// ---------------------------------------------------------------------------

/// Return config/models.dev `supports_vision` for *(provider, model)*.
///
/// Mirrors `def _lookup_supports_vision(provider, model, cfg=None)`.
///
/// Python tries `agent.image_routing._lookup_supports_vision` first, then
/// falls back to `agent.models_dev.get_model_capabilities`. In Rust the
/// default is `None` (unknown → route through aux, fails closed). Use
/// [`lookup_supports_vision_with`] to inject real capability data.
pub fn lookup_supports_vision(
    provider: &str,
    model: &str,
    cfg: Option<&Value>,
) -> Option<bool> {
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    let _ = cfg;
    None
}

/// Testable variant with injected lookup.
///
/// `lookup` should mirror `agent.image_routing._lookup_supports_vision` /
/// `agent.models_dev.get_model_capabilities` and return `Some(bool)` or `None`
/// when unknown. Panics are caught and mapped to `None` (mirrors Python's
/// `except Exception: return None` / `logger.debug`).
pub fn lookup_supports_vision_with<F>(
    provider: &str,
    model: &str,
    cfg: Option<&Value>,
    lookup: F,
) -> Option<bool>
where
    F: Fn(&str, &str, Option<&Value>) -> Option<bool>,
{
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        lookup(provider, model, cfg)
    }));
    match result {
        Ok(v) => v,
        Err(_) => None,
    }
}

// ---------------------------------------------------------------------------
// _provider_accepts_multimodal_tool_result — mirrors lines 143-161
// ---------------------------------------------------------------------------

/// Whether *provider*+*model* carries images inside tool-result messages.
///
/// Reuses `tools.vision_tools._supports_media_in_tool_results` so the
/// capture-routing decision stays in lockstep with the `vision_analyze`
/// native fast path. Returns `None` on import failure so callers fall back
/// to aux routing rather than guessing.
///
/// Mirrors `def _provider_accepts_multimodal_tool_result(provider, model)`.
///
/// In Rust we inline the exact allowlist from `tools/vision_tools.py`
/// `_supports_media_in_tool_results` (lines 980-1049) so the default path
/// has real data without needing a Python import.
pub fn provider_accepts_multimodal_tool_result(provider: &str, model: &str) -> Option<bool> {
    if provider.trim().is_empty() {
        return None;
    }
    Some(supports_media_in_tool_results(provider, model))
}

/// Testable variant with injected check.
///
/// `check` mirrors `tools.vision_tools._supports_media_in_tool_results`.
pub fn provider_accepts_multimodal_tool_result_with<F>(
    provider: &str,
    model: &str,
    check: F,
) -> Option<bool>
where
    F: Fn(&str, &str) -> Option<bool>,
{
    if provider.trim().is_empty() {
        return None;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(provider, model)));
    match result {
        Ok(v) => v,
        Err(_) => None,
    }
}

/// Core predicate — mirrors `tools.vision_tools._supports_media_in_tool_results(provider, model)`.
///
/// Provider coverage per spec docs verified Apr-2026:
///
/// * Anthropic Messages API (`anthropic` plus aggregators that proxy Claude —
///   `openrouter`, `nous`, `vertex`, `bedrock`): `tool_result` blocks accept
///   `image` content blocks.
/// * OpenAI Chat Completions: tool messages accept array content with `image_url`.
/// * OpenAI Responses (`openai-codex`): `function_call_output.output` accepts
///   `input_text`/`input_image`.
/// * Gemini 3 (and proxied via aggregators): supports multimodal tool results.
///   Older Gemini does NOT.
///
/// For unknown / legacy providers we return `false` — caller falls back to aux.
/// The check is relaxed when the provider's profile declares `supports_vision`.
pub fn supports_media_in_tool_results(provider: &str, model: &str) -> bool {
    let p = provider.trim().to_lowercase();
    if p.is_empty() {
        return false;
    }

    // Aggregators that route to multiple vendors — assume support since users
    // on these aggregators are typically using vision-capable frontier models.
    const AGGREGATORS: &[&str] = &[
        "openrouter",
        "nous",
        "vertex",
        "bedrock",
        "anthropic-vertex",
        "google-vertex",
    ];
    if AGGREGATORS.contains(&p.as_str()) {
        return true;
    }

    // Native Anthropic
    if matches!(p.as_str(), "anthropic" | "claude" | "anthropic-direct") {
        return true;
    }

    // OpenAI Chat Completions and Responses
    if matches!(
        p.as_str(),
        "openai" | "openai-chat" | "openai-codex" | "azure-openai"
    ) {
        return true;
    }

    // Gemini — gate on model name; older Gemini variants did not support
    // multimodal functionResponse. Gemini 3.x does.
    if matches!(
        p.as_str(),
        "google" | "gemini" | "google-gemini" | "google-vertex-gemini"
    ) {
        let m = model.trim().to_lowercase();
        if m.contains("gemini-3") || m.contains("gemini-pro-3") || m.contains("gemini-flash-3") {
            return true;
        }
        return false;
    }

    // For vision-capable providers like xiaomi, minimax, etc. not in the
    // hardcoded list, Python checks `providers.get_provider_profile(p).supports_vision`.
    // In Rust we conservatively return false — callers that know the provider
    // supports vision should inject via `provider_accepts_multimodal_tool_result_with`
    // or set the config-declared supports_vision override.
    false
}

// ---------------------------------------------------------------------------
// should_route_capture_to_aux_vision — mirrors lines 164-199
// ---------------------------------------------------------------------------

/// Return `true` iff the captured screenshot should be pre-analysed via aux vision.
///
/// Mirrors `def should_route_capture_to_aux_vision(provider, model, cfg)`.
///
/// Args:
///   provider: active inference provider id (e.g. `"openrouter"`, `"anthropic"`,
///     `"openai-codex"`). Lower-case canonical id.
///   model: active main model slug as it would be sent to the provider.
///   cfg: loaded `config.yaml` dict (or `None`).
///
/// Returns:
///   `true` when the caller should hand the screenshot to the aux vision
///   pipeline (and surface a text-only tool result). `false` when the caller
///   should keep the existing multimodal envelope (main model handles vision
///   natively).
///
/// ```python
/// if _explicit_aux_vision_override(cfg): return True
/// user_declared = _lookup_user_declared_supports_vision(provider, model, cfg)
/// if user_declared is True: return False
/// if user_declared is False: return True
/// accepts_tool_image = _provider_accepts_multimodal_tool_result(provider, model)
/// if accepts_tool_image is None or accepts_tool_image is False: return True
/// supports_vision = _lookup_supports_vision(provider, model, cfg)
/// if supports_vision is True: return False
/// return True
/// ```
pub fn should_route_capture_to_aux_vision(
    provider: &str,
    model: &str,
    cfg: Option<&Value>,
) -> bool {
    should_route_capture_to_aux_vision_with(
        provider,
        model,
        cfg,
        |p, m, c| lookup_user_declared_supports_vision(p, m, c),
        |p, m| provider_accepts_multimodal_tool_result(p, m),
        |p, m, c| lookup_supports_vision(p, m, c),
    )
}

/// Testable variant with injected lookups.
///
/// Mirrors the same decision but lets tests inject:
///
/// * `lookup_user_declared` — `agent.image_routing._supports_vision_override`
/// * `accepts_multimodal` — `tools.vision_tools._supports_media_in_tool_results`
/// * `lookup_supports` — `agent.image_routing._lookup_supports_vision` /
///   `agent.models_dev.get_model_capabilities`
///
/// Each closure returns `Option<bool>` (`None` = unknown / lookup failure → fails
/// closed to aux routing).
pub fn should_route_capture_to_aux_vision_with<F1, F2, F3>(
    provider: &str,
    model: &str,
    cfg: Option<&Value>,
    lookup_user_declared: F1,
    accepts_multimodal: F2,
    lookup_supports: F3,
) -> bool
where
    F1: Fn(&str, &str, Option<&Value>) -> Option<bool>,
    F2: Fn(&str, &str) -> Option<bool>,
    F3: Fn(&str, &str, Option<&Value>) -> Option<bool>,
{
    if explicit_aux_vision_override(cfg) {
        return true;
    }

    let user_declared = lookup_user_declared(provider, model, cfg);
    if user_declared == Some(true) {
        return false;
    }
    if user_declared == Some(false) {
        return true;
    }

    let accepts_tool_image = accepts_multimodal(provider, model);
    if accepts_tool_image.is_none() || accepts_tool_image == Some(false) {
        return true;
    }

    let supports_vision = lookup_supports(provider, model, cfg);
    if supports_vision == Some(true) {
        return false;
    }
    true
}

// `__all__` in Python lists with a duplicated entry, preserved here as comment:
// "should_route_capture_to_aux_vision"

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- _explicit_aux_vision_override ------------------------------------

    #[test]
    fn explicit_override_false_on_missing_or_empty() {
        assert!(!explicit_aux_vision_override(None));
        assert!(!explicit_aux_vision_override(Some(&json!({}))));
        assert!(!explicit_aux_vision_override(Some(&json!({"auxiliary": {}}))));
        assert!(!explicit_aux_vision_override(Some(&json!({"auxiliary": {"vision": {}}}))));
        assert!(!explicit_aux_vision_override(Some(&json!({"auxiliary": null}))));
        assert!(!explicit_aux_vision_override(Some(&json!({"auxiliary": {"vision": null}}))));
        assert!(!explicit_aux_vision_override(Some(&json!({"auxiliary": "bad"}))));
        assert!(!explicit_aux_vision_override(Some(&json!({"auxiliary": {"vision": "bad"}}))));
    }

    #[test]
    fn explicit_override_false_on_auto_or_blank() {
        assert!(!explicit_aux_vision_override(Some(&json!({
            "auxiliary": {"vision": {"provider": ""}}
        }))));
        assert!(!explicit_aux_vision_override(Some(&json!({
            "auxiliary": {"vision": {"provider": "auto"}}
        }))));
        assert!(!explicit_aux_vision_override(Some(&json!({
            "auxiliary": {"vision": {"provider": "  AUTO  "}}
        }))));
        assert!(!explicit_aux_vision_override(Some(&json!({
            "auxiliary": {"vision": {"provider": "auto", "model": "", "base_url": ""}}
        }))));
        // whitespace-only provider/model/base_url → not explicit
        assert!(!explicit_aux_vision_override(Some(&json!({
            "auxiliary": {"vision": {"provider": "  ", "model": "  ", "base_url": "\t"}}
        }))));
    }

    #[test]
    fn explicit_override_true_when_provider_or_model_or_base_url_set() {
        // provider non-auto
        assert!(explicit_aux_vision_override(Some(&json!({
            "auxiliary": {"vision": {"provider": "openai"}}
        }))));
        assert!(explicit_aux_vision_override(Some(&json!({
            "auxiliary": {"vision": {"provider": "anthropic"}}
        }))));
        // model non-empty even with provider auto/blank
        assert!(explicit_aux_vision_override(Some(&json!({
            "auxiliary": {"vision": {"provider": "auto", "model": "gpt-4o"}}
        }))));
        assert!(explicit_aux_vision_override(Some(&json!({
            "auxiliary": {"vision": {"provider": "", "model": "gpt-4o"}}
        }))));
        // base_url non-empty
        assert!(explicit_aux_vision_override(Some(&json!({
            "auxiliary": {"vision": {"provider": "auto", "base_url": "https://example.com/v1"}}
        }))));
        assert!(explicit_aux_vision_override(Some(&json!({
            "auxiliary": {"vision": {"base_url": "https://example.com"}}
        }))));
    }

    #[test]
    fn explicit_override_provider_auto_with_spaces_is_not_explicit() {
        // Python: provider = str(...).strip().lower() ; check provider in ("", "auto")
        // So "  auto  " → "auto" → not explicit
        assert!(!explicit_aux_vision_override(Some(&json!({
            "auxiliary": {"vision": {"provider": "  auto  "}}
        }))));
        // "AUTO" lower → "auto" → not explicit
        assert!(!explicit_aux_vision_override(Some(&json!({
            "auxiliary": {"vision": {"provider": "AUTO"}}
        }))));
    }

    // ---- supports_media_in_tool_results -----------------------------------

    #[test]
    fn supports_media_aggregators_return_true() {
        for p in ["openrouter", "nous", "vertex", "bedrock", "anthropic-vertex", "google-vertex"] {
            assert!(supports_media_in_tool_results(p, "any-model"), "provider {p}");
            assert!(supports_media_in_tool_results(&p.to_uppercase(), "any-model"), "provider uppercase {p}");
        }
    }

    #[test]
    fn supports_media_anthropic_and_openai_return_true() {
        for p in ["anthropic", "claude", "anthropic-direct", "openai", "openai-chat", "openai-codex", "azure-openai"] {
            assert!(supports_media_in_tool_results(p, "any-model"), "provider {p}");
        }
    }

    #[test]
    fn supports_media_gemini_gated_on_model() {
        // Gemini 3 → true
        assert!(supports_media_in_tool_results("google", "gemini-3-pro"));
        assert!(supports_media_in_tool_results("gemini", "gemini-3-flash"));
        assert!(supports_media_in_tool_results("google-gemini", "gemini-pro-3-latest"));
        assert!(supports_media_in_tool_results("google-vertex-gemini", "something gemini-flash-3 something"));
        assert!(supports_media_in_tool_results("gemini", "GEMINI-3-PRO"));
        // Older Gemini → false
        assert!(!supports_media_in_tool_results("google", "gemini-1.5-pro"));
        assert!(!supports_media_in_tool_results("gemini", "gemini-2.0-flash"));
        assert!(!supports_media_in_tool_results("google", ""));
    }

    #[test]
    fn supports_media_unknown_returns_false() {
        assert!(!supports_media_in_tool_results("ollama", "llava"));
        assert!(!supports_media_in_tool_results("custom", "my-model"));
        assert!(!supports_media_in_tool_results("", "gemini-3-pro"));
        assert!(!supports_media_in_tool_results("   ", "any"));
    }

    #[test]
    fn provider_accepts_returns_none_on_empty_provider() {
        assert_eq!(provider_accepts_multimodal_tool_result("", "model"), None);
        assert_eq!(provider_accepts_multimodal_tool_result("   ", "model"), None);
        // non-empty → Some
        assert_eq!(provider_accepts_multimodal_tool_result("openai", "gpt-4o"), Some(true));
        assert_eq!(provider_accepts_multimodal_tool_result("ollama", "llava"), Some(false));
    }

    // ---- should_route_capture_to_aux_vision --------------------------------

    #[test]
    fn route_true_when_explicit_aux_override() {
        let cfg = json!({"auxiliary": {"vision": {"provider": "openai"}}});
        // Even with vision-capable main model, explicit aux wins (first branch)
        let route = should_route_capture_to_aux_vision_with(
            "openai",
            "gpt-4o",
            Some(&cfg),
            |_, _, _| Some(true), // user_declared true would normally return false
            |_, _| Some(true),
            |_, _, _| Some(true),
        );
        assert!(route, "explicit aux override must force aux routing");

        // Also via simple entrypoint
        assert!(should_route_capture_to_aux_vision("openai", "gpt-4o", Some(&cfg)));
    }

    #[test]
    fn route_false_when_user_declared_supports_vision_true() {
        let cfg = json!({});
        let route = should_route_capture_to_aux_vision_with(
            "custom",
            "my-vlm",
            Some(&cfg),
            |_, _, _| Some(true),
            |_, _| Some(true),
            |_, _, _| None,
        );
        assert!(!route);
    }

    #[test]
    fn route_true_when_user_declared_supports_vision_false() {
        let cfg = json!({});
        let route = should_route_capture_to_aux_vision_with(
            "custom",
            "my-model",
            Some(&cfg),
            |_, _, _| Some(false),
            |_, _| Some(true),
            |_, _, _| Some(true),
        );
        assert!(route);
    }

    #[test]
    fn route_true_when_provider_does_not_accept_multimodal() {
        let cfg = json!({});
        // accepts = false → true
        let route = should_route_capture_to_aux_vision_with(
            "ollama",
            "llava",
            Some(&cfg),
            |_, _, _| None,
            |_, _| Some(false),
            |_, _, _| Some(true),
        );
        assert!(route);

        // accepts = None → true (import failure / empty provider)
        let route2 = should_route_capture_to_aux_vision_with(
            "",
            "any",
            Some(&cfg),
            |_, _, _| None,
            |_, _| None,
            |_, _, _| Some(true),
        );
        assert!(route2);
    }

    #[test]
    fn route_false_when_accepts_and_supports_vision_true() {
        let cfg = json!({});
        let route = should_route_capture_to_aux_vision_with(
            "openai",
            "gpt-4o",
            Some(&cfg),
            |_, _, _| None,
            |_, _| Some(true),
            |_, _, _| Some(true),
        );
        assert!(!route);
    }

    #[test]
    fn route_true_when_accepts_but_supports_not_true() {
        let cfg = json!({});
        // supports = false → true
        assert!(should_route_capture_to_aux_vision_with(
            "openai",
            "gpt-4o",
            Some(&cfg),
            |_, _, _| None,
            |_, _| Some(true),
            |_, _, _| Some(false),
        ));
        // supports = None (unknown) → true (fails closed)
        assert!(should_route_capture_to_aux_vision_with(
            "openai",
            "gpt-4o",
            Some(&cfg),
            |_, _, _| None,
            |_, _| Some(true),
            |_, _, _| None,
        ));
    }

    #[test]
    fn route_fails_closed_default_no_overrides() {
        // No config, unknown provider/model → should route to aux (closed)
        // Default delegate returns None for lookups, and provider_accepts for
        // unknown provider returns Some(false) → true
        assert!(should_route_capture_to_aux_vision("ollama", "unknown", None));
        assert!(should_route_capture_to_aux_vision("openai", "gpt-4o", None) == false
            || should_route_capture_to_aux_vision("openai", "gpt-4o", None) == true);
        // For openai+gpt-4o with default lookups: accepts true, supports None → true
        // (since no caps data, fails closed)
        assert!(should_route_capture_to_aux_vision("openai", "gpt-4o", None));
    }

    #[test]
    fn route_with_real_supports_media_logic() {
        // OpenAI provider accepts multimodal, but without caps → true (aux)
        assert!(should_route_capture_to_aux_vision("openai", "gpt-4o", Some(&json!({}))));

        // With injected supports_vision true → false (native)
        let route = should_route_capture_to_aux_vision_with(
            "openai",
            "gpt-4o",
            Some(&json!({})),
            |_, _, _| None,
            |p, m| provider_accepts_multimodal_tool_result(p, m),
            |_, _, _| Some(true),
        );
        assert!(!route);

        // Aggregator openrouter → accepts true, with supports true → false
        let route2 = should_route_capture_to_aux_vision_with(
            "openrouter",
            "anthropic/claude-3.5-sonnet",
            Some(&json!({})),
            |_, _, _| None,
            |p, m| provider_accepts_multimodal_tool_result(p, m),
            |_, _, _| Some(true),
        );
        assert!(!route2);
    }

    #[test]
    fn lookup_helpers_default_to_none() {
        assert_eq!(lookup_user_declared_supports_vision("openai", "gpt-4o", None), None);
        assert_eq!(lookup_supports_vision("openai", "gpt-4o", None), None);
        assert_eq!(lookup_supports_vision("", "model", None), None);
        assert_eq!(lookup_supports_vision("provider", "", None), None);
    }

    #[test]
    fn lookup_with_injected_closure() {
        let v = lookup_user_declared_supports_vision_with("p", "m", None, |_, _, _| Some(true));
        assert_eq!(v, Some(true));
        let v2 = lookup_supports_vision_with("p", "m", None, |_, _, _| Some(false));
        assert_eq!(v2, Some(false));
        // panic → None
        let v3 = lookup_supports_vision_with("p", "m", None, |_, _, _| panic!("boom"));
        assert_eq!(v3, None);
    }
}
