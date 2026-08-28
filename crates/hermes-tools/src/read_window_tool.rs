//! Read the OS window directly underneath the Hermes desktop window.
//! Port of `tools/read_window_tool.py` (71 lines) — 1:1 behavior.
//!
//! The window list lives with the OS, so this tool round-trips through the
//! gateway's blocking-prompt bridge — the same one `read_terminal` uses:
//! `tui_gateway` emits `window.read.request`, the desktop renderer asks its
//! main process (which owns native window enumeration) and answers with
//! `window.read.respond`. This module is just schema + a thin dispatcher over
//! the platform-injected callback.
//!
//! Lives in the `desktop_ui` toolset (like `read_terminal`), which the GUI
//! gateway enables only for desktop-sourced sessions.

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Registry metadata — mirrors `registry.register(...)` kwargs in Python
// ---------------------------------------------------------------------------

/// Tool name as registered in `tools.registry`.
pub const TOOL_NAME: &str = "read_window_below";
/// Toolset that gates this tool (`toolset="desktop_ui"`).
pub const TOOLSET: &str = "desktop_ui";
/// Emoji for tool listing — mirrors `emoji="🪟"` in Python.
pub const EMOJI: &str = "🪟";
/// `requires_env` for this tool — none (desktop_ui is session-gated, not env-gated).
pub const REQUIRES_ENV: &[&str] = &[];

// ---------------------------------------------------------------------------
// Constants — mirrors Python module-level assignments and inline strings
// ---------------------------------------------------------------------------

/// Error when no callback is wired — not running in desktop app.
///
/// Mirrors `tool_error("read_window_below is only available in the Hermes desktop app.")`
/// (line 21-23).
pub const NOT_AVAILABLE_ERROR: &str =
    "read_window_below is only available in the Hermes desktop app.";

/// Error when the desktop did not answer or enumeration is unavailable.
///
/// Mirrors `tool_error("Could not determine the window underneath ...")`
/// (lines 31-34).
pub const EMPTY_ERROR: &str =
    "Could not determine the window underneath (the desktop app did not answer, or window enumeration is unavailable on this system).";

/// Prefix for callback exception path.
///
/// Mirrors `tool_error(f"Failed to read the window below: {exc}")` (line 28).
pub const FAILED_PREFIX: &str = "Failed to read the window below: ";

/// Full tool description — mirrors `READ_WINDOW_BELOW_SCHEMA["description"]`.
///
/// Joined from the Python multi-string literal (lines 45-56).
pub const DESCRIPTION: &str = "Identify the application window directly underneath (behind) the Hermes desktop window — what the user is working in behind this app. Returns JSON: {window: {app, title, bounds{x,y,width,height}, id}, frontmost: {app, title}, platform}. `title` may be empty when the OS withholds window titles (e.g. macOS without the Screen Recording permission — never prompted for, noted in `note`). Other Hermes windows are skipped: the nearest non-Hermes window is reported. Returns {error, platform} instead where the OS cannot enumerate windows at all (e.g. a Wayland session); `error` says what would fix it, so relay it rather than retrying. Metadata only; this never captures pixels or content of other windows.";

// ---------------------------------------------------------------------------
// Schema — mirrors `READ_WINDOW_BELOW_SCHEMA` dict in Python (lines 43-62)
// ---------------------------------------------------------------------------

/// Returns the JSON schema for `read_window_below` — mirrors `READ_WINDOW_BELOW_SCHEMA`.
///
/// In Python this is a dict literal; here we return a `serde_json::Value`
/// so callers can serialize or inspect it without owning a static JSON string.
pub fn read_window_below_schema() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": DESCRIPTION,
        "parameters": {
            "type": "object",
            "properties": {}
        }
    })
}

/// Static schema value for registry consumers that need a serialized string.
/// Mirrors `READ_WINDOW_BELOW_SCHEMA` as a serialized string.
pub fn read_window_below_schema_json() -> String {
    read_window_below_schema().to_string()
}

// ---------------------------------------------------------------------------
// Error helpers — mirrors `tools.registry.tool_error` (1:1 truncation)
// ---------------------------------------------------------------------------

const MAX_TOOL_ERROR_CHARS: usize = 2048;
const TOOL_ERROR_TRUNCATION_MARKER: &str = "… [truncated]";

fn bound_error_text(text: &str) -> String {
    // Python: len(text) is char count, not byte count.
    let char_count = text.chars().count();
    if char_count <= MAX_TOOL_ERROR_CHARS {
        text.to_string()
    } else {
        let truncated: String = text.chars().take(MAX_TOOL_ERROR_CHARS).collect();
        format!("{truncated}{TOOL_ERROR_TRUNCATION_MARKER}")
    }
}

/// Mirrors `tool_error(message, **extra)` in `tools/registry.py`.
///
/// Returns `{"error": <bounded message>}` as a JSON string with
/// `ensure_ascii=False` (Rust's `serde_json` preserves unicode by default).
pub fn tool_error(message: &str) -> String {
    let bounded = bound_error_text(message);
    json!({ "error": bounded }).to_string()
}

// ---------------------------------------------------------------------------
// Core handler — mirrors `read_window_below_tool(callback) -> str` (lines 18-40)
// ---------------------------------------------------------------------------

/// Return the window underneath the Hermes window as a JSON string (no callback).
///
/// Mirrors Python `def read_window_below_tool(callback=None) -> str:` when
/// `callback is None` (lines 20-23):
/// ```python
/// if callback is None:
///     return tool_error("read_window_below is only available in the Hermes desktop app.")
/// ```
///
/// This variant is the default outside the desktop app — the callback has not
/// been wired by the gateway's blocking-prompt bridge.
pub fn read_window_below_tool() -> String {
    tool_error(NOT_AVAILABLE_ERROR)
}

/// Core with injected callback — mirrors the `callback is not None` branch.
///
/// `callback` mirrors Python `Optional[Callable]` that returns a raw string.
///   - `Ok(raw)`  — raw text from desktop (JSON object or plain text).
///   - `Err(exc)` — exception path, mapped to `Failed to read the window below: {exc}`.
///
/// Steps (lines 25-40):
/// ```python
/// try:
///     raw = callback()
/// except Exception as exc:
///     return tool_error(f"Failed to read the window below: {exc}")
/// if not raw:
///     return tool_error("Could not determine the window underneath ...")
/// try:
///     return json.dumps(json.loads(raw), ensure_ascii=False)
/// except (TypeError, ValueError):
///     return json.dumps({"text": str(raw)}, ensure_ascii=False)
/// ```
pub fn read_window_below_tool_with_callback<F>(callback: F) -> String
where
    F: FnOnce() -> Result<String, String>,
{
    let raw = match callback() {
        Ok(v) => v,
        Err(exc) => {
            return tool_error(&format!("{FAILED_PREFIX}{exc}"));
        }
    };

    if raw.is_empty() {
        return tool_error(EMPTY_ERROR);
    }

    // Desktop answers with a JSON object; pass it through, else wrap the raw text.
    // Mirrors `json.dumps(json.loads(raw), ensure_ascii=False)` with fallback to `{"text": ...}`.
    match serde_json::from_str::<Value>(&raw) {
        Ok(parsed) => {
            // Re-serialize to normalize (mirrors json.loads -> json.dumps round-trip).
            // `serde_json::Value::to_string` preserves unicode (ensure_ascii=False).
            parsed.to_string()
        }
        Err(_) => {
            json!({ "text": raw }).to_string()
        }
    }
}

/// Variant that takes an optional callback — mirrors Python `callback: Optional[Callable] = None`.
///
/// `None` → `NOT_AVAILABLE_ERROR`, `Some(cb)` → `read_window_below_tool_with_callback(cb)`.
/// This is the most direct 1:1 of the Python signature for callers that already have
/// an `Option` (e.g. gateway `kw.get("callback")` may be `None`).
pub fn read_window_below_tool_with_optional_callback<F>(callback: Option<F>) -> String
where
    F: FnOnce() -> Result<String, String>,
{
    match callback {
        Some(cb) => read_window_below_tool_with_callback(cb),
        None => read_window_below_tool(),
    }
}

/// Mirrors the registry handler lambda (line 69):
/// `lambda args, **kw: read_window_below_tool(callback=kw.get("callback"))`
///
/// `args` is ignored (schema has no properties). In Rust the callback is
/// injected via `handler_with_callback` / `handler_with_optional_callback`;
/// this bare `handler` mirrors the `callback=None` (no desktop) path and
/// returns `NOT_AVAILABLE_ERROR`.
pub fn handler(_args: &Value) -> String {
    read_window_below_tool()
}

/// Variant that injects a callback — mirrors `handler` with `callback` kwarg.
pub fn handler_with_callback<F>(_args: &Value, callback: F) -> String
where
    F: FnOnce() -> Result<String, String>,
{
    read_window_below_tool_with_callback(callback)
}

/// Variant that injects an optional callback — mirrors `kw.get("callback")` may be `None`.
pub fn handler_with_optional_callback<F>(_args: &Value, callback: Option<F>) -> String
where
    F: FnOnce() -> Result<String, String>,
{
    read_window_below_tool_with_optional_callback(callback)
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
        assert_eq!(TOOL_NAME, "read_window_below");
        assert_eq!(TOOLSET, "desktop_ui");
        assert_eq!(EMOJI, "🪟");
        assert!(REQUIRES_ENV.is_empty());
        assert_eq!(
            NOT_AVAILABLE_ERROR,
            "read_window_below is only available in the Hermes desktop app."
        );
        assert_eq!(
            EMPTY_ERROR,
            "Could not determine the window underneath (the desktop app did not answer, or window enumeration is unavailable on this system)."
        );
        assert_eq!(FAILED_PREFIX, "Failed to read the window below: ");
        assert!(DESCRIPTION.starts_with("Identify the application window directly underneath"));
        assert!(DESCRIPTION.contains("Hermes desktop window"));
        assert!(DESCRIPTION.contains("{window: {app, title, bounds{x,y,width,height}, id}"));
        assert!(DESCRIPTION.contains("frontmost: {app, title}"));
        assert!(DESCRIPTION.contains("Screen Recording"));
        assert!(DESCRIPTION.contains("Other Hermes windows are skipped"));
        assert!(DESCRIPTION.contains("Wayland session"));
        assert!(DESCRIPTION.contains("Metadata only; this never captures pixels"));
    }

    #[test]
    fn schema_matches_python() {
        let schema = read_window_below_schema();
        assert_eq!(schema["name"], "read_window_below");
        assert_eq!(schema["description"], DESCRIPTION);
        assert_eq!(schema["parameters"]["type"], "object");
        assert!(schema["parameters"]["properties"].as_object().unwrap().is_empty());
        // Ensure JSON serialization round-trips (mirrors Python dict)
        let s = read_window_below_schema_json();
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, schema);
        // Name appears in json string
        assert!(s.contains("read_window_below"));
    }

    #[test]
    fn no_callback_returns_not_available() {
        let out = read_window_below_tool();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);

        // handler bare also returns same
        let out = handler(&json!({}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);

        // optional None also
        let out = read_window_below_tool_with_optional_callback::<fn() -> Result<String, String>>(None);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);

        let out = handler_with_optional_callback::<fn() -> Result<String, String>>(&json!({}), None);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);
    }

    #[test]
    fn exception_path_returns_failed_error() {
        let out = read_window_below_tool_with_callback(|| Err("boom".to_string()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], "Failed to read the window below: boom");

        let out = handler_with_callback(&json!({}), || Err("transport closed".to_string()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["error"].as_str().unwrap().contains("Failed to read the window below: transport closed"));
    }

    #[test]
    fn empty_raw_returns_empty_error() {
        let out = read_window_below_tool_with_callback(|| Ok(String::new()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], EMPTY_ERROR);

        // also via handler_with_callback
        let out = handler_with_callback(&json!({}), || Ok("".to_string()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], EMPTY_ERROR);
    }

    #[test]
    fn valid_json_raw_passes_through_normalized() {
        let raw = r#"{"window":{"app":"Code","title":"main.rs","bounds":{"x":0,"y":0,"width":100,"height":100},"id":123},"frontmost":{"app":"Code","title":"main.rs"},"platform":"darwin"}"#;
        let out = read_window_below_tool_with_callback(|| Ok(raw.to_string()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["window"]["app"], "Code");
        assert_eq!(v["window"]["bounds"]["width"], 100);
        assert_eq!(v["platform"], "darwin");
        // round-trip: parsing raw and parsing out are equal
        let expected: Value = serde_json::from_str(raw).unwrap();
        assert_eq!(v, expected);
    }

    #[test]
    fn invalid_json_raw_wrapped_as_text() {
        let raw = "not json at all";
        let out = read_window_below_tool_with_callback(|| Ok(raw.to_string()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["text"], raw);
        // ensure no error key
        assert!(v.get("error").is_none());
    }

    #[test]
    fn json_preserves_unicode_ensure_ascii_false() {
        // Mirrors ensure_ascii=False: emoji and non-ascii survive without \u escapes.
        let raw = r#"{"window":{"app":"Hermes🪟","title":"café"},"platform":"darwin"}"#;
        let out = read_window_below_tool_with_callback(|| Ok(raw.to_string()));
        // serde_json preserves unicode by default; assert raw string contains emoji directly
        assert!(out.contains('🪟'));
        assert!(out.contains("café"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["window"]["app"], "Hermes🪟");
    }

    #[test]
    fn plain_text_unicode_wrapped() {
        let raw = "café 🪟 plain text";
        let out = read_window_below_tool_with_callback(|| Ok(raw.to_string()));
        // wrap path should preserve unicode directly (no \u escapes)
        assert!(out.contains("café"));
        assert!(out.contains('🪟'));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["text"], raw);
    }

    #[test]
    fn error_with_platform_json_passes_through() {
        // Wayland error case: {error, platform}
        let raw = r#"{"error":"Wayland does not support window enumeration — use X11 or enable something","platform":"linux"}"#;
        let out = read_window_below_tool_with_callback(|| Ok(raw.to_string()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v.get("error").is_some());
        assert_eq!(v["platform"], "linux");
        let expected: Value = serde_json::from_str(raw).unwrap();
        assert_eq!(v, expected);
    }

    #[test]
    fn handler_ignores_args_and_uses_callback() {
        // schema has no properties, but handler should ignore any args
        let args = json!({"ignored": "value", "extra": 123});
        let raw = r#"{"window":{"app":"Finder"},"platform":"darwin"}"#;
        let out = handler_with_callback(&args, || Ok(raw.to_string()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["window"]["app"], "Finder");
    }

    #[test]
    fn optional_callback_some_uses_callback() {
        let raw = r#"{"platform":"win32"}"#;
        let out = read_window_below_tool_with_optional_callback(Some(|| Ok(raw.to_string())));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["platform"], "win32");
    }

    #[test]
    fn tool_error_truncates_long_messages() {
        let long = "x".repeat(3000);
        let out = tool_error(&long);
        let v: Value = serde_json::from_str(&out).unwrap();
        let err = v["error"].as_str().unwrap();
        assert!(err.len() > MAX_TOOL_ERROR_CHARS);
        assert!(err.ends_with(TOOL_ERROR_TRUNCATION_MARKER));
        assert_eq!(
            err.chars().count(),
            MAX_TOOL_ERROR_CHARS + TOOL_ERROR_TRUNCATION_MARKER.chars().count()
        );
    }

    #[test]
    fn handler_with_optional_callback_none_is_not_available() {
        let out = handler_with_optional_callback::<fn() -> Result<String, String>>(&json!({}), None);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);
    }
}
