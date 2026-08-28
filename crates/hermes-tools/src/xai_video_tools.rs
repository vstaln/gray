//! xAI-specific Imagine video edit and extend tools.
//! Port of `tools/xai_video_tools.py` (209 lines) — 1:1 behavior.
//!
//! Separate from `video_generate` because edit/extend are provider-specific.
//! `video_url` must be the public HTTPS MP4 URL from a prior Imagine result
//! (`video` / `public_url` on files-cdn), sent to xAI as `video.url`.
//!
//! Python mapping:
//! - `_configured_for_xai_video` → [`configured_for_xai_video`] / [`configured_for_xai_video_with_config`]
//! - `_check_xai_video_requirements` → [`check_xai_video_requirements`] / [`check_xai_video_requirements_with`]
//! - `_clean_string` → [`clean_string`]
//! - `_coerce_int` → [`coerce_int`]
//! - `_provider_not_configured_error` → [`provider_not_configured_error`] / [`PROVIDER_NOT_CONFIGURED_ERROR`]
//! - `_normalize_public_video_url` → [`normalize_public_video_url`]
//! - `XAI_VIDEO_EDIT_SCHEMA` → [`xai_video_edit_schema`] / [`XAI_VIDEO_EDIT_DESCRIPTION`] etc.
//! - `XAI_VIDEO_EXTEND_SCHEMA` → [`xai_video_extend_schema`]
//! - `_handle_xai_video_edit` → [`handle_xai_video_edit`] / [`handle_xai_video_edit_with`]
//! - `_handle_xai_video_extend` → [`handle_xai_video_extend`] / [`handle_xai_video_extend_with`]
//! - `registry.register(..., name="xai_video_edit")` → [`TOOL_NAME_EDIT`] / [`TOOLSET`] etc.
//! - `registry.register(..., name="xai_video_extend")` → [`TOOL_NAME_EXTEND`]

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Registry metadata — mirrors `registry.register(...)` kwargs in Python (189-209)
// ---------------------------------------------------------------------------

/// Tool name for edit — mirrors `registry.register(name="xai_video_edit", ...)` (189).
pub const TOOL_NAME_EDIT: &str = "xai_video_edit";
/// Tool name for extend — mirrors `registry.register(name="xai_video_extend", ...)` (200).
pub const TOOL_NAME_EXTEND: &str = "xai_video_extend";
/// Toolset that gates both tools — mirrors `toolset="video_gen"` (190, 201).
pub const TOOLSET: &str = "video_gen";
/// Emoji for tool listing — mirrors `emoji="video"` (197, 208).
pub const EMOJI: &str = "video";
/// `requires_env` for both tools — mirrors `requires_env=[]` (195, 206).
pub const REQUIRES_ENV: &[&str] = &[];

// ---------------------------------------------------------------------------
// Constants — mirrors Python module-level assignments
// ---------------------------------------------------------------------------

/// Full tool description for edit — mirrors `XAI_VIDEO_EDIT_SCHEMA["description"]` (72-77).
pub const XAI_VIDEO_EDIT_DESCRIPTION: &str = "Edit an existing video with xAI Imagine. This is separate from `video_generate` because video editing is provider-specific. `video_url` must be the public HTTPS MP4 URL from a prior Imagine result (`video` or `public_url` on files-cdn).";

/// Full tool description for extend — mirrors `XAI_VIDEO_EXTEND_SCHEMA["description"]` (104-108).
pub const XAI_VIDEO_EXTEND_DESCRIPTION: &str = "Extend an existing video with xAI Imagine. This is separate from `video_generate` because video extension is provider-specific. `video_url` must be the public HTTPS MP4 URL from a prior Imagine result (`video` or `public_url` on files-cdn).";

/// Description for `prompt` (edit) — mirrors `XAI_VIDEO_EDIT_SCHEMA["parameters"]["properties"]["prompt"]["description"]` (82-84).
pub const PROMPT_DESCRIPTION_EDIT: &str = "Instruction for how xAI should modify the source video.";
/// Description for `prompt` (extend) — mirrors `XAI_VIDEO_EXTEND_SCHEMA["parameters"]["properties"]["prompt"]["description"]` (114-116).
pub const PROMPT_DESCRIPTION_EXTEND: &str = "Instruction for how xAI should continue the source video.";
/// Description for `video_url` — mirrors both schemas (86-90, 118-122).
pub const VIDEO_URL_DESCRIPTION: &str = "Public HTTPS MP4 URL of the source video — the `video` or `public_url` from a prior xAI Imagine result.";
/// Description for `model` — mirrors both schemas (92-95, 131-134).
pub const MODEL_DESCRIPTION: &str = "Optional xAI Imagine model override.";
/// Description for `duration` (extend) — mirrors `XAI_VIDEO_EXTEND_SCHEMA["parameters"]["properties"]["duration"]["description"]` (124-128).
pub const DURATION_DESCRIPTION: &str = "Desired extension duration in seconds. xAI clamps this to its supported range.";

/// Error when `prompt` is missing for edit — mirrors `tool_error("prompt is required for xAI video edit")` (147).
pub const PROMPT_REQUIRED_EDIT_ERROR: &str = "prompt is required for xAI video edit";
/// Error when `prompt` is missing for extend — mirrors `tool_error("prompt is required for xAI video extend")` (171).
pub const PROMPT_REQUIRED_EXTEND_ERROR: &str = "prompt is required for xAI video extend";
/// Error when `video_url` is missing/invalid — mirrors both handlers (149-151, 173-175).
pub const VIDEO_URL_REQUIRED_ERROR: &str =
    "video_url must be a public HTTPS MP4 URL (the `video`/`public_url` from a prior Imagine result)";

/// Error message for provider not configured — mirrors `_provider_not_configured_error()` (51-54).
pub const PROVIDER_NOT_CONFIGURED_MESSAGE: &str =
    "xAI video edit/extend tools require `video_gen.provider` to be configured as `xai` via `hermes tools` -> Video Generation.";
/// `error_type` for provider not configured — mirrors `_provider_not_configured_error()` (55).
pub const PROVIDER_NOT_CONFIGURED_ERROR_TYPE: &str = "provider_not_configured";

// ---------------------------------------------------------------------------
// Schema — mirrors `XAI_VIDEO_EDIT_SCHEMA` (70-99) and `XAI_VIDEO_EXTEND_SCHEMA` (102-138)
// ---------------------------------------------------------------------------

/// Returns the JSON schema for `xai_video_edit` — mirrors `XAI_VIDEO_EDIT_SCHEMA`.
pub fn xai_video_edit_schema() -> Value {
    json!({
        "name": TOOL_NAME_EDIT,
        "description": XAI_VIDEO_EDIT_DESCRIPTION,
        "parameters": {
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": PROMPT_DESCRIPTION_EDIT
                },
                "video_url": {
                    "type": "string",
                    "description": VIDEO_URL_DESCRIPTION
                },
                "model": {
                    "type": "string",
                    "description": MODEL_DESCRIPTION
                }
            },
            "required": ["prompt", "video_url"]
        }
    })
}

/// Returns the JSON schema for `xai_video_extend` — mirrors `XAI_VIDEO_EXTEND_SCHEMA`.
pub fn xai_video_extend_schema() -> Value {
    json!({
        "name": TOOL_NAME_EXTEND,
        "description": XAI_VIDEO_EXTEND_DESCRIPTION,
        "parameters": {
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": PROMPT_DESCRIPTION_EXTEND
                },
                "video_url": {
                    "type": "string",
                    "description": VIDEO_URL_DESCRIPTION
                },
                "duration": {
                    "type": "integer",
                    "description": DURATION_DESCRIPTION
                },
                "model": {
                    "type": "string",
                    "description": MODEL_DESCRIPTION
                }
            },
            "required": ["prompt", "video_url"]
        }
    })
}

/// Serialized schema for edit — mirrors `XAI_VIDEO_EDIT_SCHEMA` as JSON string.
pub fn xai_video_edit_schema_json() -> String {
    xai_video_edit_schema().to_string()
}

/// Serialized schema for extend — mirrors `XAI_VIDEO_EXTEND_SCHEMA` as JSON string.
pub fn xai_video_extend_schema_json() -> String {
    xai_video_extend_schema().to_string()
}

// ---------------------------------------------------------------------------
// Error helpers — mirrors `tools.registry.tool_error` and `_provider_not_configured_error`
// ---------------------------------------------------------------------------

const MAX_TOOL_ERROR_CHARS: usize = 2048;
const TOOL_ERROR_TRUNCATION_MARKER: &str = "… [truncated]";

fn bound_error_text(text: &str) -> String {
    let char_count = text.chars().count();
    if char_count <= MAX_TOOL_ERROR_CHARS {
        text.to_string()
    } else {
        let truncated: String = text.chars().take(MAX_TOOL_ERROR_CHARS).collect();
        format!("{truncated}{TOOL_ERROR_TRUNCATION_MARKER}")
    }
}

/// Mirrors `tool_error(message)` in `tools/registry.py`.
///
/// Returns `{"error": <bounded message>}` as a JSON string with
/// `ensure_ascii=False` (Rust's `serde_json` preserves unicode by default).
pub fn tool_error(message: &str) -> String {
    let bounded = bound_error_text(message);
    json!({ "error": bounded }).to_string()
}

/// Mirrors `_provider_not_configured_error()` (48-57) — returns JSON string.
///
/// ```python
/// return json.dumps({
///     "success": False,
///     "error": "xAI video edit/extend tools require `video_gen.provider` ...",
///     "error_type": "provider_not_configured",
///     "provider": "xai",
/// })
/// ```
pub fn provider_not_configured_error() -> String {
    json!({
        "success": false,
        "error": PROVIDER_NOT_CONFIGURED_MESSAGE,
        "error_type": PROVIDER_NOT_CONFIGURED_ERROR_TYPE,
        "provider": "xai"
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// Helpers — mirrors `_clean_string`, `_coerce_int`, `_normalize_public_video_url`
// ---------------------------------------------------------------------------

/// Mirrors `_clean_string(value: Any) -> Optional[str]` (31-34):
/// ```python
/// if isinstance(value, str) and value.strip():
///     return value.strip()
/// return None
/// ```
pub fn clean_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        _ => None,
    }
}

/// Helper for `Option<&Value>` — mirrors `args.get("prompt")` where missing → `None` → `clean_string` returns `None`.
pub fn clean_string_opt(value: Option<&Value>) -> Option<String> {
    match value {
        Some(v) => clean_string(v),
        None => None,
    }
}

/// Mirrors `_coerce_int(value: Any) -> Optional[int]` (37-45):
/// ```python
/// if value is None: return None
/// if isinstance(value, bool): return None
/// try: return int(value)
/// except (TypeError, ValueError): return None
/// ```
pub fn coerce_int(value: &Value) -> Option<i64> {
    match value {
        Value::Null => None,
        Value::Bool(_) => None,
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
                if f.is_finite() && f.fract() == 0.0 {
                    Some(f as i64)
                } else {
                    // Python int(3.5) truncates → 3; mirror for float numbers
                    // but only when caller passed float; schema says integer, so truncate
                    if f.is_finite() {
                        Some(f.trunc() as i64)
                    } else {
                        None
                    }
                }
            } else {
                None
            }
        }
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return None;
            }
            // Python int("5") succeeds, int("5.0") raises ValueError, int(" 5 ") ok
            // We mirror strict integer parse; floats in strings are errors
            match trimmed.parse::<i64>() {
                Ok(i) => Some(i),
                Err(_) => {
                    // Try bool check already done; string float like "5.0" → None
                    // Python int(True) is 1 but we already returned None for bool
                    None
                }
            }
        }
        _ => None,
    }
}

/// Mirrors `Option<&Value>` variant — missing key → `None`.
pub fn coerce_int_opt(value: Option<&Value>) -> Option<i64> {
    match value {
        None => None,
        Some(v) => coerce_int(v),
    }
}

/// Mirrors `_normalize_public_video_url(video_url: Any) -> Optional[str]` (60-67):
/// ```python
/// cleaned = _clean_string(video_url)
/// if not cleaned: return None
/// if cleaned.lower().startswith(("http://", "https://")):
///     return cleaned
/// return None
/// ```
pub fn normalize_public_video_url(value: &Value) -> Option<String> {
    let cleaned = clean_string(value)?;
    let lower = cleaned.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Some(cleaned)
    } else {
        None
    }
}

/// Mirrors `Option<&Value>` variant — missing → `None`.
pub fn normalize_public_video_url_opt(value: Option<&Value>) -> Option<String> {
    match value {
        None => None,
        Some(v) => normalize_public_video_url(v),
    }
}

// ---------------------------------------------------------------------------
// Config checks — mirrors `_configured_for_xai_video` and `_check_xai_video_requirements`
// ---------------------------------------------------------------------------

/// Mirrors `_configured_for_xai_video() -> bool` (18-24) against a loaded config `Value`.
///
/// ```python
/// try: cfg = load_config()
/// except Exception: return False
/// section = cfg.get("video_gen") if isinstance(cfg, dict) else None
/// return isinstance(section, dict) and section.get("provider") == "xai"
/// ```
pub fn configured_for_xai_video(config: &Value) -> bool {
    match config {
        Value::Object(map) => {
            if let Some(section) = map.get("video_gen") {
                if let Value::Object(sec_map) = section {
                    if let Some(Value::String(provider)) = sec_map.get("provider") {
                        return provider == "xai";
                    }
                }
            }
            false
        }
        _ => false,
    }
}

/// Mirrors the `try/except` wrapper around `load_config()` (19-22).
///
/// `config_result` is `Ok(config)` from `load_config()` or `Err` when it raised.
/// `Err` → `false`, otherwise delegates to [`configured_for_xai_video`].
pub fn configured_for_xai_video_with_result(config_result: Result<Value, String>) -> bool {
    match config_result {
        Ok(cfg) => configured_for_xai_video(&cfg),
        Err(_) => false,
    }
}

/// Mirrors `_configured_for_xai_video()` with an injected loader `Fn() -> Result<Value, String>`.
///
/// The loader mirrors `load_config()` — `Ok(Value)` on success, `Err` on exception.
pub fn configured_for_xai_video_with_loader<F>(loader: F) -> bool
where
    F: FnOnce() -> Result<Value, String>,
{
    match loader() {
        Ok(cfg) => configured_for_xai_video(&cfg),
        Err(_) => false,
    }
}

/// Mirrors `_check_xai_video_requirements() -> bool` (27-28):
/// ```python
/// return _configured_for_xai_video() and has_xai_video_credentials()
/// ```
pub fn check_xai_video_requirements(config: &Value, has_credentials: bool) -> bool {
    configured_for_xai_video(config) && has_credentials
}

/// Variant that mirrors the full check with injected loader and credential fn.
///
/// `loader` mirrors `load_config` try/except, `has_credentials` mirrors `has_xai_video_credentials()`.
pub fn check_xai_video_requirements_with<F, G>(loader: F, has_credentials_fn: G) -> bool
where
    F: FnOnce() -> Result<Value, String>,
    G: FnOnce() -> bool,
{
    configured_for_xai_video_with_loader(loader) && has_credentials_fn()
}

// ---------------------------------------------------------------------------
// Core handlers — mirrors `_handle_xai_video_edit` (141-161) and `_handle_xai_video_extend` (164-186)
// ---------------------------------------------------------------------------

/// Handler for `xai_video_edit` without dependencies — provider check fails (no config).
///
/// Mirrors `_handle_xai_video_edit(args: Dict[str, Any], **_kw: Any) -> str` (141):
/// ```python
/// prompt = _clean_string(args.get("prompt"))
/// video_url = _normalize_public_video_url(args.get("video_url"))
/// model = _clean_string(args.get("model"))
/// if not prompt: return tool_error("prompt is required for xAI video edit")
/// if not video_url: return tool_error("video_url must be a public HTTPS ...")
/// if not _configured_for_xai_video(): return _provider_not_configured_error()
/// result = run_xai_video_edit(prompt=prompt, video_url=video_url, model=model)
/// return json.dumps(result)
/// ```
/// This stub always hits the provider-not-configured path (no real config/credentials wired).
/// Use [`handle_xai_video_edit_with`] to inject real runners in prod or tests.
pub fn handle_xai_video_edit(args: &Value) -> String {
    handle_xai_video_edit_with(
        args,
        || Err("no config".to_string()),
        || false,
        |_prompt, _video_url, _model| json!({"success": false, "error": "not wired"}),
    )
}

/// Testable / prod-injectable core for `xai_video_edit`.
///
/// `load_config` mirrors `load_config()` try/except (returns `Ok(Value)` or `Err`),
/// `has_credentials` mirrors `has_xai_video_credentials()`,
/// `runner` mirrors `run_xai_video_edit(prompt, video_url, model)` returning a `Value` dict.
pub fn handle_xai_video_edit_with<F, G, R>(args: &Value, load_config: F, has_credentials: G, runner: R) -> String
where
    F: FnOnce() -> Result<Value, String>,
    G: FnOnce() -> bool,
    R: FnOnce(&str, &str, Option<String>) -> Value,
{
    let prompt = clean_string_opt(args.get("prompt"));
    let video_url = normalize_public_video_url_opt(args.get("video_url"));
    let model = clean_string_opt(args.get("model"));

    if prompt.is_none() {
        return tool_error(PROMPT_REQUIRED_EDIT_ERROR);
    }
    if video_url.is_none() {
        return tool_error(VIDEO_URL_REQUIRED_ERROR);
    }
    if !configured_for_xai_video_with_loader(load_config) {
        return provider_not_configured_error();
    }
    // Note: Python edit handler checks `_configured_for_xai_video()` only, not `has_credentials`.
    // The `has_xai_video_credentials` gate is for `check_fn` (tool visibility), not for the handler.
    // But we still accept `has_credentials` for symmetry; the handler itself does not gate on it
    // to stay 1:1 with Python lines 153-154. Suppress unused.
    let _ = has_credentials;

    let result = runner(prompt.unwrap().as_str(), video_url.unwrap().as_str(), model);
    result.to_string()
}

/// Handler for `xai_video_extend` without dependencies — provider check fails (no config).
///
/// Mirrors `_handle_xai_video_extend(args: Dict[str, Any], **_kw: Any) -> str` (164):
/// ```python
/// prompt = _clean_string(args.get("prompt"))
/// video_url = _normalize_public_video_url(args.get("video_url"))
/// model = _clean_string(args.get("model"))
/// duration = _coerce_int(args.get("duration"))
/// if not prompt: return tool_error("prompt is required for xAI video extend")
/// if not video_url: return tool_error("video_url must be a public HTTPS ...")
/// if not _configured_for_xai_video(): return _provider_not_configured_error()
/// result = run_xai_video_extend(prompt=prompt, video_url=video_url, duration=duration, model=model)
/// return json.dumps(result)
/// ```
pub fn handle_xai_video_extend(args: &Value) -> String {
    handle_xai_video_extend_with(
        args,
        || Err("no config".to_string()),
        || false,
        |_prompt, _video_url, _duration, _model| json!({"success": false, "error": "not wired"}),
    )
}

/// Testable / prod-injectable core for `xai_video_extend`.
///
/// `load_config` mirrors `load_config()`, `has_credentials` mirrors `has_xai_video_credentials()`,
/// `runner` mirrors `run_xai_video_extend(prompt, video_url, duration, model)`.
pub fn handle_xai_video_extend_with<F, G, R>(args: &Value, load_config: F, has_credentials: G, runner: R) -> String
where
    F: FnOnce() -> Result<Value, String>,
    G: FnOnce() -> bool,
    R: FnOnce(&str, &str, Option<i64>, Option<String>) -> Value,
{
    let prompt = clean_string_opt(args.get("prompt"));
    let video_url = normalize_public_video_url_opt(args.get("video_url"));
    let model = clean_string_opt(args.get("model"));
    let duration = coerce_int_opt(args.get("duration"));

    if prompt.is_none() {
        return tool_error(PROMPT_REQUIRED_EXTEND_ERROR);
    }
    if video_url.is_none() {
        return tool_error(VIDEO_URL_REQUIRED_ERROR);
    }
    if !configured_for_xai_video_with_loader(load_config) {
        return provider_not_configured_error();
    }
    let _ = has_credentials;

    let result = runner(
        prompt.unwrap().as_str(),
        video_url.unwrap().as_str(),
        duration,
        model,
    );
    result.to_string()
}

// ---------------------------------------------------------------------------
// Registry handler aliases — mirrors `registry.register(..., handler=...)` (189-209)
// ---------------------------------------------------------------------------

/// Registry handler alias for `xai_video_edit` — mirrors `lambda args, **kw: _handle_xai_video_edit(args)`.
pub fn xai_video_edit_handler(args: &Value) -> String {
    handle_xai_video_edit(args)
}

/// Registry handler alias for `xai_video_extend` — mirrors `lambda args, **kw: _handle_xai_video_extend(args)`.
pub fn xai_video_extend_handler(args: &Value) -> String {
    handle_xai_video_extend(args)
}

// ---------------------------------------------------------------------------
// `__all__` equivalent — public surface mirrors Python `registry.register`
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn constants_match_python_registry_args() {
        assert_eq!(TOOL_NAME_EDIT, "xai_video_edit");
        assert_eq!(TOOL_NAME_EXTEND, "xai_video_extend");
        assert_eq!(TOOLSET, "video_gen");
        assert_eq!(EMOJI, "video");
        assert!(REQUIRES_ENV.is_empty());
        assert_eq!(PROMPT_REQUIRED_EDIT_ERROR, "prompt is required for xAI video edit");
        assert_eq!(PROMPT_REQUIRED_EXTEND_ERROR, "prompt is required for xAI video extend");
        assert_eq!(VIDEO_URL_REQUIRED_ERROR, "video_url must be a public HTTPS MP4 URL (the `video`/`public_url` from a prior Imagine result)");
        assert!(PROVIDER_NOT_CONFIGURED_MESSAGE.contains("video_gen.provider"));
        assert!(PROVIDER_NOT_CONFIGURED_MESSAGE.contains("xai"));
        assert_eq!(PROVIDER_NOT_CONFIGURED_ERROR_TYPE, "provider_not_configured");
        assert!(XAI_VIDEO_EDIT_DESCRIPTION.contains("xAI Imagine"));
        assert!(XAI_VIDEO_EXTEND_DESCRIPTION.contains("xAI Imagine"));
    }

    #[test]
    fn schemas_match_python() {
        let edit = xai_video_edit_schema();
        assert_eq!(edit["name"], "xai_video_edit");
        assert_eq!(edit["description"], XAI_VIDEO_EDIT_DESCRIPTION);
        assert_eq!(edit["parameters"]["type"], "object");
        assert_eq!(edit["parameters"]["properties"]["prompt"]["type"], "string");
        assert_eq!(edit["parameters"]["properties"]["video_url"]["type"], "string");
        assert_eq!(edit["parameters"]["properties"]["model"]["type"], "string");
        let req = edit["parameters"]["required"].as_array().unwrap();
        assert_eq!(req, &vec![json!("prompt"), json!("video_url")]);

        let extend = xai_video_extend_schema();
        assert_eq!(extend["name"], "xai_video_extend");
        assert_eq!(extend["parameters"]["properties"]["duration"]["type"], "integer");
        assert_eq!(extend["parameters"]["properties"]["duration"]["description"], DURATION_DESCRIPTION);
        let req2 = extend["parameters"]["required"].as_array().unwrap();
        assert_eq!(req2, &vec![json!("prompt"), json!("video_url")]);

        // round-trip serialization
        let s = xai_video_edit_schema_json();
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, edit);
        let s2 = xai_video_extend_schema_json();
        let parsed2: Value = serde_json::from_str(&s2).unwrap();
        assert_eq!(parsed2, extend);
    }

    #[test]
    fn clean_string_mirrors_python() {
        assert_eq!(clean_string(&json!("  hello  ")), Some("hello".to_string()));
        assert_eq!(clean_string(&json!("   ")), None);
        assert_eq!(clean_string(&json!("")), None);
        assert_eq!(clean_string(&json!(null)), None);
        assert_eq!(clean_string(&json!(42)), None);
        assert_eq!(clean_string(&json!(true)), None);
        assert_eq!(clean_string_opt(None), None);
        assert_eq!(clean_string_opt(Some(&json!(" x "))), Some("x".to_string()));
    }

    #[test]
    fn coerce_int_mirrors_python() {
        assert_eq!(coerce_int(&json!(null)), None);
        assert_eq!(coerce_int(&json!(true)), None);
        assert_eq!(coerce_int(&json!(false)), None);
        assert_eq!(coerce_int(&json!(5)), Some(5));
        assert_eq!(coerce_int(&json!(-3)), Some(-3));
        assert_eq!(coerce_int(&json!("42")), Some(42));
        assert_eq!(coerce_int(&json!("  7  ")), Some(7));
        assert_eq!(coerce_int(&json!("5.0")), None);
        assert_eq!(coerce_int(&json!("abc")), None);
        assert_eq!(coerce_int(&json!("")), None);
        assert_eq!(coerce_int_opt(None), None);
        assert_eq!(coerce_int_opt(Some(&json!(10))), Some(10));
        // bool as string "true" → None (int("true") raises)
        assert_eq!(coerce_int(&json!("true")), None);
    }

    #[test]
    fn normalize_public_video_url_mirrors_python() {
        assert_eq!(
            normalize_public_video_url(&json!("https://files-cdn.example/video.mp4")),
            Some("https://files-cdn.example/video.mp4".to_string())
        );
        assert_eq!(
            normalize_public_video_url(&json!("http://example.com/a.mp4")),
            Some("http://example.com/a.mp4".to_string())
        );
        // case-insensitive check but preserves original casing
        assert_eq!(
            normalize_public_video_url(&json!("HTTPS://example.com/vid.mp4")),
            Some("HTTPS://example.com/vid.mp4".to_string())
        );
        assert_eq!(normalize_public_video_url(&json!("  https://example.com/v.mp4  ")), Some("https://example.com/v.mp4".to_string()));
        assert_eq!(normalize_public_video_url(&json!("ftp://example.com/v.mp4")), None);
        assert_eq!(normalize_public_video_url(&json!("")), None);
        assert_eq!(normalize_public_video_url(&json!("   ")), None);
        assert_eq!(normalize_public_video_url(&json!(null)), None);
        assert_eq!(normalize_public_video_url_opt(None), None);
        assert_eq!(normalize_public_video_url_opt(Some(&json!("not a url"))), None);
    }

    #[test]
    fn configured_for_xai_video_mirrors_python() {
        assert!(configured_for_xai_video(&json!({"video_gen": {"provider": "xai"}})));
        assert!(!configured_for_xai_video(&json!({"video_gen": {"provider": "veo"}})));
        assert!(!configured_for_xai_video(&json!({"video_gen": {}})));
        assert!(!configured_for_xai_video(&json!({})));
        assert!(!configured_for_xai_video(&json!(null)));
        assert!(!configured_for_xai_video(&json!("string")));
        // not a dict section
        assert!(!configured_for_xai_video(&json!({"video_gen": "xai"})));
        // with_result wrapper: Err → false
        assert!(!configured_for_xai_video_with_result(Err("fail".to_string())));
        assert!(configured_for_xai_video_with_result(Ok(json!({"video_gen": {"provider": "xai"}}))));
        assert!(!configured_for_xai_video_with_result(Ok(json!({}))));

        // with loader
        assert!(configured_for_xai_video_with_loader(|| Ok(json!({"video_gen": {"provider": "xai"}}))));
        assert!(!configured_for_xai_video_with_loader(|| Err("load failed".to_string())));
        assert!(!configured_for_xai_video_with_loader(|| Ok(json!({"video_gen": {"provider": "other"}}))));
    }

    #[test]
    fn check_requirements_mirrors_python() {
        let cfg_xai = json!({"video_gen": {"provider": "xai"}});
        let cfg_other = json!({"video_gen": {"provider": "fal"}});
        assert!(check_xai_video_requirements(&cfg_xai, true));
        assert!(!check_xai_video_requirements(&cfg_xai, false));
        assert!(!check_xai_video_requirements(&cfg_other, true));
        assert!(!check_xai_video_requirements(&cfg_other, false));

        assert!(check_xai_video_requirements_with(|| Ok(cfg_xai.clone()), || true));
        assert!(!check_xai_video_requirements_with(|| Ok(cfg_xai.clone()), || false));
        assert!(!check_xai_video_requirements_with(|| Err("err".to_string()), || true));
    }

    #[test]
    fn provider_not_configured_error_shape() {
        let s = provider_not_configured_error();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["success"], false);
        assert_eq!(v["error"], PROVIDER_NOT_CONFIGURED_MESSAGE);
        assert_eq!(v["error_type"], "provider_not_configured");
        assert_eq!(v["provider"], "xai");
    }

    #[test]
    fn handle_edit_validates_prompt_and_url() {
        // missing prompt
        let out = handle_xai_video_edit_with(
            &json!({"video_url": "https://example.com/v.mp4"}),
            || Ok(json!({"video_gen": {"provider": "xai"}})),
            || true,
            |_, _, _| panic!("should not call runner on missing prompt"),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], PROMPT_REQUIRED_EDIT_ERROR);

        // prompt whitespace only
        let out = handle_xai_video_edit_with(
            &json!({"prompt": "   ", "video_url": "https://example.com/v.mp4"}),
            || Ok(json!({"video_gen": {"provider": "xai"}})),
            || true,
            |_, _, _| panic!("should not call"),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], PROMPT_REQUIRED_EDIT_ERROR);

        // missing video_url
        let out = handle_xai_video_edit_with(
            &json!({"prompt": "make it cinematic"}),
            || Ok(json!({"video_gen": {"provider": "xai"}})),
            || true,
            |_, _, _| panic!("should not call"),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], VIDEO_URL_REQUIRED_ERROR);

        // invalid video_url (not http/https)
        let out = handle_xai_video_edit_with(
            &json!({"prompt": "edit this", "video_url": "ftp://example.com/v.mp4"}),
            || Ok(json!({"video_gen": {"provider": "xai"}})),
            || true,
            |_, _, _| panic!("should not call"),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], VIDEO_URL_REQUIRED_ERROR);

        // provider not configured
        let out = handle_xai_video_edit_with(
            &json!({"prompt": "edit", "video_url": "https://example.com/v.mp4"}),
            || Ok(json!({"video_gen": {"provider": "other"}})),
            || true,
            |_, _, _| panic!("should not call when provider wrong"),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], false);
        assert_eq!(v["error_type"], "provider_not_configured");
        assert_eq!(v["provider"], "xai");

        // load_config error also maps to provider_not_configured
        let out = handle_xai_video_edit_with(
            &json!({"prompt": "edit", "video_url": "https://example.com/v.mp4"}),
            || Err("config load failed".to_string()),
            || true,
            |_, _, _| panic!("should not call"),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error_type"], "provider_not_configured");
    }

    #[test]
    fn handle_edit_success_trims_and_passes_model() {
        let out = handle_xai_video_edit_with(
            &json!({"prompt": "  make it brighter  ", "video_url": "  https://example.com/v.mp4  ", "model": "  grok-imagine-video  "}),
            || Ok(json!({"video_gen": {"provider": "xai"}})),
            || true,
            |prompt, video_url, model| {
                assert_eq!(prompt, "make it brighter");
                assert_eq!(video_url, "https://example.com/v.mp4");
                assert_eq!(model, Some("grok-imagine-video".to_string()));
                json!({"success": true, "video": video_url})
            },
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
        assert_eq!(v["video"], "https://example.com/v.mp4");

        // model optional, empty string → None
        let out = handle_xai_video_edit_with(
            &json!({"prompt": "edit", "video_url": "https://example.com/v.mp4", "model": "   "}),
            || Ok(json!({"video_gen": {"provider": "xai"}})),
            || true,
            |_, _, model| {
                assert_eq!(model, None);
                json!({"success": true})
            },
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
    }

    #[test]
    fn handle_extend_validates_and_coerces_duration() {
        // missing prompt
        let out = handle_xai_video_extend_with(
            &json!({"video_url": "https://example.com/v.mp4"}),
            || Ok(json!({"video_gen": {"provider": "xai"}})),
            || true,
            |_, _, _, _| panic!("should not call"),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], PROMPT_REQUIRED_EXTEND_ERROR);

        // provider not configured
        let out = handle_xai_video_extend_with(
            &json!({"prompt": "extend", "video_url": "https://example.com/v.mp4"}),
            || Ok(json!({"video_gen": {"provider": "other"}})),
            || true,
            |_, _, _, _| panic!("should not call"),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error_type"], "provider_not_configured");

        // success with duration string coercion
        let out = handle_xai_video_extend_with(
            &json!({"prompt": "continue", "video_url": "https://example.com/v.mp4", "duration": "8", "model": "grok-imagine-video"}),
            || Ok(json!({"video_gen": {"provider": "xai"}})),
            || true,
            |prompt, video_url, duration, model| {
                assert_eq!(prompt, "continue");
                assert_eq!(video_url, "https://example.com/v.mp4");
                assert_eq!(duration, Some(8));
                assert_eq!(model, Some("grok-imagine-video".to_string()));
                json!({"success": true, "video": video_url})
            },
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);

        // duration None when missing, coerced correctly for null/bool/invalid
        let out = handle_xai_video_extend_with(
            &json!({"prompt": "extend", "video_url": "https://example.com/v.mp4", "duration": null}),
            || Ok(json!({"video_gen": {"provider": "xai"}})),
            || true,
            |_, _, duration, _| {
                assert_eq!(duration, None);
                json!({"success": true})
            },
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);

        let out = handle_xai_video_extend_with(
            &json!({"prompt": "extend", "video_url": "https://example.com/v.mp4", "duration": true}),
            || Ok(json!({"video_gen": {"provider": "xai"}})),
            || true,
            |_, _, duration, _| {
                assert_eq!(duration, None);
                json!({"success": true})
            },
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
    }

    #[test]
    fn default_handlers_without_wiring_hit_provider_error() {
        // default stubs have no config → provider_not_configured
        let out = handle_xai_video_edit(&json!({"prompt": "edit", "video_url": "https://example.com/v.mp4"}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error_type"], "provider_not_configured");

        let out = handle_xai_video_extend(&json!({"prompt": "extend", "video_url": "https://example.com/v.mp4"}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error_type"], "provider_not_configured");

        // validation still fires before provider check
        let out = handle_xai_video_edit(&json!({"video_url": "https://example.com/v.mp4"}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], PROMPT_REQUIRED_EDIT_ERROR);
    }

    #[test]
    fn tool_error_truncates_long_messages() {
        let long = "x".repeat(3000);
        let out = tool_error(&long);
        let v: Value = serde_json::from_str(&out).unwrap();
        let err = v["error"].as_str().unwrap();
        assert!(err.ends_with(TOOL_ERROR_TRUNCATION_MARKER));
        assert_eq!(err.chars().count(), MAX_TOOL_ERROR_CHARS + TOOL_ERROR_TRUNCATION_MARKER.chars().count());
    }

    #[test]
    fn json_preserves_unicode() {
        let out = handle_xai_video_edit_with(
            &json!({"prompt": "café edit 🎬", "video_url": "https://example.com/v.mp4"}),
            || Ok(json!({"video_gen": {"provider": "xai"}})),
            || true,
            |prompt, _, _| {
                assert!(prompt.contains("café"));
                json!({"success": true, "prompt": prompt})
            },
        );
        assert!(out.contains("café"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
    }
}
