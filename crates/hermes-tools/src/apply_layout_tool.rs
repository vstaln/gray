//! Apply a layout preset in the Hermes desktop GUI.
//! Port of `tools/apply_layout_tool.py` (74 lines) — 1:1 behavior.
//!
//! Lives in the `desktop_ui` toolset (like `focus_pane`), which the GUI
//! gateway enables only for desktop-sourced sessions. Emits `layout.apply`
//! through the shared `desktop_ui` bridge; the renderer resolves the preset id
//! against its layouts registry (core presets, plugin presets, and user-saved
//! presets are all the same list) and applies the tree through the exact code
//! path the layout picker uses. Only the active window's session may act — a
//! background turn never rearranges the user's desktop.
//!
//! Preset ids are free-form on purpose: plugins and users mint their own. The
//! renderer answers with the applied preset's id/title on success and the list
//! of available ids when the id is unknown, so the model can self-correct
//! without a second registry-listing tool.

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Registry metadata — mirrors `registry.register(...)` kwargs in Python
// ---------------------------------------------------------------------------

/// Tool name as registered in `tools.registry`.
pub const TOOL_NAME: &str = "apply_layout";
/// Toolset that gates this tool (`toolset="desktop_ui"`).
pub const TOOLSET: &str = "desktop_ui";
/// Emoji for tool listing — mirrors `emoji="🧱"` in Python.
pub const EMOJI: &str = "🧱";
/// `requires_env` for this tool — none (desktop_ui is session-gated, not env-gated).
pub const REQUIRES_ENV: &[&str] = &[];

// ---------------------------------------------------------------------------
// Constants — mirrors Python module-level assignments
// ---------------------------------------------------------------------------

/// Renderer answer arrives via the blocking-prompt bridge with this timeout;
/// applying a layout is synchronous in the renderer, so this is generous.
///
/// Mirrors `_TIMEOUT_NOTE` in Python (line 25).
pub const TIMEOUT_NOTE: &str = "Layout apply is only available in the Hermes desktop app.";

/// Error when `preset` is missing/empty.
///
/// Mirrors `tool_error("preset is required — a layout preset id, e.g. 'default' or 'focus'.")`
pub const PRESET_REQUIRED_ERROR: &str =
    "preset is required — a layout preset id, e.g. 'default' or 'focus'.";

/// Full tool description — mirrors `APPLY_LAYOUT_SCHEMA["description"]`.
///
/// Joined from the Python multi-string literal (lines 46-53).
pub const DESCRIPTION: &str = "Apply a saved layout preset to the Hermes desktop app when the user asks to rearrange the workspace — e.g. \"set up my layout for coding\", \"give me a focused view\", \"put the terminal front and center\". Built-in presets: default (chat + sidebars), focus (chat only), terminal-deck (terminal forward), quad (four zones). Plugin and user-saved presets are addressed by their id. To reveal a single pane without rearranging everything, use focus_pane instead.";

/// Description for the `preset` parameter — mirrors `APPLY_LAYOUT_SCHEMA["parameters"]["properties"]["preset"]["description"]`.
pub const PRESET_DESCRIPTION: &str =
    "Layout preset id to apply (e.g. 'default', 'focus', 'terminal-deck', 'quad', or a user/plugin preset id).";

// ---------------------------------------------------------------------------
// Schema — mirrors `APPLY_LAYOUT_SCHEMA` dict in Python (lines 44-65)
// ---------------------------------------------------------------------------

/// Returns the JSON schema for `apply_layout` — mirrors `APPLY_LAYOUT_SCHEMA`.
///
/// In Python this is a dict literal; here we return a `serde_json::Value`
/// so callers can serialize or inspect it without owning a static JSON string.
pub fn apply_layout_schema() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": DESCRIPTION,
        "parameters": {
            "type": "object",
            "properties": {
                "preset": {
                    "type": "string",
                    "description": PRESET_DESCRIPTION
                }
            },
            "required": ["preset"]
        }
    })
}

/// Static schema value for registry consumers that need a `&'static` reference
/// to the JSON text. Mirrors `APPLY_LAYOUT_SCHEMA` as a serialized string.
pub fn apply_layout_schema_json() -> String {
    apply_layout_schema().to_string()
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
// desktop_ui bridge — mirrors `tools/desktop_ui.py` (1:1 semantics)
// ---------------------------------------------------------------------------

/// Mirrors `desktop_ui.available()` — true when a renderer emitter is wired.
///
/// In Python this checks `_emit is not None`. Here it's a stub that returns
/// `false` until the gateway wires a real emitter via `set_desktop_ui_emitter`.
///
/// For 1:1 traceability the stub is kept; tests inject an emitter via
/// `apply_layout_tool_with_emit`.
pub fn desktop_ui_available() -> bool {
    false
}

/// Mirrors `desktop_ui.emit(event, payload) -> bool`.
///
/// Python signature: `emit(event: str, payload: dict) -> bool`
///   - Returns `False` when no emitter is wired (not desktop app).
///   - Calls `_emit(session_id, event, payload)` and returns `True` otherwise.
///   - May raise — caller must map exceptions to `tool_error`.
///
/// This stub returns `Ok(false)` (no desktop) so the default
/// `apply_layout_tool` path hits `TIMEOUT_NOTE` — identical to running the
/// Python tool outside the desktop app. Tests and real gateways inject a
/// closure via `apply_layout_tool_with_emit`.
pub fn desktop_ui_emit(_event: &str, _payload: Value) -> Result<bool, String> {
    Ok(false)
}

// ---------------------------------------------------------------------------
// Core handler — mirrors `apply_layout_tool(preset: str) -> str` (lines 28-41)
// ---------------------------------------------------------------------------

/// Ask the desktop GUI to apply layout preset `preset`.
///
/// Mirrors Python `def apply_layout_tool(preset: str) -> str:` (lines 28-41):
/// ```python
/// name = (preset or "").strip()
/// if not name:
///     return tool_error("preset is required — ...")
/// try:
///     ok = desktop_ui.emit("layout.apply", {"preset": name})
/// except Exception as exc:
///     return tool_error(f"Failed to apply layout '{name}': {exc}")
/// if not ok:
///     return tool_error(_TIMEOUT_NOTE)
/// return json.dumps({"success": True, "preset": name}, ensure_ascii=False)
/// ```
///
/// The `preset` arg mirrors Python's `preset or ""` — callers that have an
/// `Option<String>` should pass `opt.as_deref().unwrap_or("")`. Empty or
/// whitespace-only strings trigger the required-field error.
///
/// The desktop-ui call is delegated to `desktop_ui_emit`; override via
/// `apply_layout_tool_with_emit` in tests or when the gateway has wired a
/// real emitter.
pub fn apply_layout_tool(preset: &str) -> String {
    apply_layout_tool_with_emit(preset, desktop_ui_emit)
}

/// Testable core: same as `apply_layout_tool` but with an injected emit fn.
///
/// `emit` mirrors `desktop_ui.emit`: `Fn(&str, Value) -> Result<bool, String>`
/// where `Ok(true)` = applied, `Ok(false)` = no desktop (timeout), `Err(msg)`
/// = exception path.
pub fn apply_layout_tool_with_emit<F>(preset: &str, emit: F) -> String
where
    F: FnOnce(&str, Value) -> Result<bool, String>,
{
    let name = preset.trim();
    if name.is_empty() {
        return tool_error(PRESET_REQUIRED_ERROR);
    }

    let payload = json!({ "preset": name });
    let ok = match emit("layout.apply", payload) {
        Ok(v) => v,
        Err(exc) => {
            return tool_error(&format!("Failed to apply layout '{name}': {exc}"));
        }
    };
    if !ok {
        return tool_error(TIMEOUT_NOTE);
    }

    // Mirrors `json.dumps({"success": True, "preset": name}, ensure_ascii=False)`
    json!({ "success": true, "preset": name }).to_string()
}

/// Mirrors the registry handler lambda:
/// `lambda args, **kw: apply_layout_tool(preset=args.get("preset", ""))`
///
/// Extracts `preset` as string (missing/non-string → `""`) and delegates to
/// `apply_layout_tool`. The `Option`-to-string fallback preserves Python's
/// `args.get("preset", "")` semantics.
pub fn handler(args: &Value) -> String {
    let preset = args
        .get("preset")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    apply_layout_tool(preset)
}

/// Variant that injects an emitter — mirrors `handler` but testable.
pub fn handler_with_emit<F>(args: &Value, emit: F) -> String
where
    F: FnOnce(&str, Value) -> Result<bool, String>,
{
    let preset = args
        .get("preset")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    apply_layout_tool_with_emit(preset, emit)
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
        assert_eq!(TOOL_NAME, "apply_layout");
        assert_eq!(TOOLSET, "desktop_ui");
        assert_eq!(EMOJI, "🧱");
        assert!(REQUIRES_ENV.is_empty());
        assert_eq!(
            TIMEOUT_NOTE,
            "Layout apply is only available in the Hermes desktop app."
        );
        assert_eq!(
            PRESET_REQUIRED_ERROR,
            "preset is required — a layout preset id, e.g. 'default' or 'focus'."
        );
        assert!(DESCRIPTION.starts_with("Apply a saved layout preset"));
        assert!(DESCRIPTION.contains("terminal-deck"));
        assert!(DESCRIPTION.contains("focus_pane instead"));
        assert!(PRESET_DESCRIPTION.contains("'default'"));
    }

    #[test]
    fn schema_matches_python() {
        let schema = apply_layout_schema();
        assert_eq!(schema["name"], "apply_layout");
        assert_eq!(schema["parameters"]["type"], "object");
        assert_eq!(
            schema["parameters"]["properties"]["preset"]["type"],
            "string"
        );
        assert_eq!(
            schema["parameters"]["properties"]["preset"]["description"],
            PRESET_DESCRIPTION
        );
        let required = schema["parameters"]["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "preset");
        // Ensure JSON serialization round-trips (mirrors Python dict)
        let s = apply_layout_schema_json();
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, schema);
    }

    #[test]
    fn empty_preset_returns_tool_error() {
        for preset in ["", "   ", "\t\n"] {
            let out = apply_layout_tool_with_emit(preset, |_, _| Ok(true));
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["error"], PRESET_REQUIRED_ERROR, "preset={preset:?}");
        }
        // Also via handler with missing key → "" → same error
        let out = handler(&json!({}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], PRESET_REQUIRED_ERROR);

        let out = handler(&json!({"preset": 42}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], PRESET_REQUIRED_ERROR);

        let out = apply_layout_tool("");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], PRESET_REQUIRED_ERROR);
    }

    #[test]
    fn trims_preset_before_emit_and_success() {
        let out = apply_layout_tool_with_emit("  focus  ", |event, payload| {
            assert_eq!(event, "layout.apply");
            assert_eq!(payload["preset"], "focus");
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
        assert_eq!(v["preset"], "focus");
    }

    #[test]
    fn success_path_returns_preset() {
        let out = apply_layout_tool_with_emit("default", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
        assert_eq!(v["preset"], "default");
        assert!(v.get("error").is_none());

        let out = apply_layout_tool_with_emit("quad", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["preset"], "quad");
    }

    #[test]
    fn timeout_when_not_on_desktop() {
        // Ok(false) mirrors desktop_ui.emit returning False (no emitter)
        let out = apply_layout_tool_with_emit("default", |_, _| Ok(false));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], TIMEOUT_NOTE);

        // Default stub also returns timeout (no desktop)
        let out = apply_layout_tool("default");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], TIMEOUT_NOTE);
    }

    #[test]
    fn exception_path_returns_failed_error() {
        let out = apply_layout_tool_with_emit("default", |_, _| {
            Err("boom".to_string())
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], "Failed to apply layout 'default': boom");

        // Handler variant with emit
        let out = handler_with_emit(&json!({"preset": "focus"}), |_, _| {
            Err("transport closed".to_string())
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(
            v["error"].as_str().unwrap().contains("Failed to apply layout 'focus'")
        );
    }

    #[test]
    fn handler_extracts_preset_like_python_lambda() {
        let out = handler_with_emit(&json!({"preset": "terminal-deck"}), |_, p| {
            assert_eq!(p["preset"], "terminal-deck");
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["preset"], "terminal-deck");

        // Missing → "" → required error, emit never called
        let out = handler_with_emit(&json!({}), |_, _| {
            panic!("should not emit on empty preset");
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], PRESET_REQUIRED_ERROR);
    }

    #[test]
    fn json_preserves_unicode_ensure_ascii_false() {
        // Mirrors ensure_ascii=False: emoji and non-ascii survive without \u escapes.
        let preset = "my🧱preset";
        let out = apply_layout_tool_with_emit(preset, |_, _| Ok(true));
        // serde_json preserves unicode by default; assert raw string contains emoji
        assert!(out.contains('🧱'));
        assert!(out.contains(preset));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["preset"], preset);
    }

    #[test]
    fn tool_error_truncates_long_messages() {
        let long = "x".repeat(3000);
        let out = tool_error(&long);
        let v: Value = serde_json::from_str(&out).unwrap();
        let err = v["error"].as_str().unwrap();
        assert!(err.len() > MAX_TOOL_ERROR_CHARS);
        assert!(err.ends_with(TOOL_ERROR_TRUNCATION_MARKER));
        // Bounded to 2048 chars + marker
        assert_eq!(
            err.chars().count(),
            MAX_TOOL_ERROR_CHARS + TOOL_ERROR_TRUNCATION_MARKER.chars().count()
        );
    }
}
