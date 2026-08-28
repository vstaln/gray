//! Reveal/focus a pane in the Hermes desktop GUI.
//! Port of `tools/focus_pane_tool.py` (66 lines) — 1:1 behavior.
//!
//! Lives in the `desktop_ui` toolset (like the other GUI affordances), which the
//! GUI gateway enables only for desktop-sourced sessions. Emits `pane.reveal`
//! through the shared `desktop_ui` bridge; the renderer runs each pane's own
//! reveal path and only acts on the active window (a background turn never moves
//! the user's focus). To show a URL/file, use `open_preview`; to close it, use
//! `close_preview`.

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Registry metadata — mirrors `registry.register(...)` kwargs in Python
// ---------------------------------------------------------------------------

/// Tool name as registered in `tools.registry`.
pub const TOOL_NAME: &str = "focus_pane";
/// Toolset that gates this tool (`toolset="desktop_ui"`).
pub const TOOLSET: &str = "desktop_ui";
/// Emoji for tool listing — mirrors `emoji="🪟"` in Python.
pub const EMOJI: &str = "🪟";
/// `requires_env` for this tool — none (desktop_ui is session-gated, not env-gated).
pub const REQUIRES_ENV: &[&str] = &[];

// ---------------------------------------------------------------------------
// Constants — mirrors Python module-level assignments
// ---------------------------------------------------------------------------

/// Valid pane ids — mirrors `PANES = ("chat", "files", "terminal", "review", "sessions")` (line 17).
pub const PANES: &[&str] = &["chat", "files", "terminal", "review", "sessions"];

/// Error when `pane` is not one of `PANES`.
///
/// Mirrors `tool_error(f"pane must be one of: {', '.join(PANES)}.")` (line 24).
pub fn panes_error() -> String {
    format!("pane must be one of: {}.", PANES.join(", "))
}

/// Error when no emitter is wired — not running in desktop app.
///
/// Mirrors `tool_error("Pane focus is only available in the Hermes desktop app.")` (line 31).
pub const NOT_AVAILABLE_ERROR: &str = "Pane focus is only available in the Hermes desktop app.";

const FAILED_PREFIX: &str = "Failed to focus the ";
const FAILED_SUFFIX: &str = " pane: ";

/// Full tool description — mirrors `FOCUS_PANE_SCHEMA["description"]`.
///
/// Joined from the Python multi-string literal (lines 38-45).
pub const DESCRIPTION: &str = "Reveal and focus a pane in the Hermes desktop app when the user asks to see it — e.g. \"show me the terminal\", \"open the file browser\", \"show the diff\". Panes: chat (the conversation), files (project file browser), terminal (embedded shell), review (git diff), sessions (the session list). To show a URL or file in the preview pane, use open_preview; to close it, use close_preview.";

/// Description for the `pane` parameter — mirrors `FOCUS_PANE_SCHEMA["parameters"]["properties"]["pane"]["description"]`.
pub const PANE_DESCRIPTION: &str = "Which pane to reveal.";

// ---------------------------------------------------------------------------
// Schema — mirrors `FOCUS_PANE_SCHEMA` dict in Python (lines 36-57)
// ---------------------------------------------------------------------------

/// Returns the JSON schema for `focus_pane` — mirrors `FOCUS_PANE_SCHEMA`.
///
/// In Python this is a dict literal; here we return a `serde_json::Value`
/// so callers can serialize or inspect it without owning a static JSON string.
pub fn focus_pane_schema() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": DESCRIPTION,
        "parameters": {
            "type": "object",
            "properties": {
                "pane": {
                    "type": "string",
                    "enum": PANES,
                    "description": PANE_DESCRIPTION
                }
            },
            "required": ["pane"]
        }
    })
}

/// Static schema value for registry consumers that need a `&'static` reference
/// to the JSON text. Mirrors `FOCUS_PANE_SCHEMA` as a serialized string.
pub fn focus_pane_schema_json() -> String {
    focus_pane_schema().to_string()
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
/// `focus_pane_tool_with_emit`.
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
/// `focus_pane_tool` path hits `NOT_AVAILABLE_ERROR` — identical to running the
/// Python tool outside the desktop app. Tests and real gateways inject a
/// closure via `focus_pane_tool_with_emit`.
pub fn desktop_ui_emit(_event: &str, _payload: Value) -> Result<bool, String> {
    Ok(false)
}

// ---------------------------------------------------------------------------
// Core handler — mirrors `focus_pane_tool(pane: str) -> str` (lines 20-33)
// ---------------------------------------------------------------------------

/// Ask the desktop GUI to reveal and focus `pane`.
///
/// Mirrors Python `def focus_pane_tool(pane: str) -> str:` (lines 20-33):
/// ```python
/// name = (pane or "").strip().lower()
/// if name not in PANES:
///     return tool_error(f"pane must be one of: {', '.join(PANES)}.")
/// try:
///     ok = desktop_ui.emit("pane.reveal", {"pane": name})
/// except Exception as exc:
///     return tool_error(f"Failed to focus the {name} pane: {exc}")
/// if not ok:
///     return tool_error("Pane focus is only available in the Hermes desktop app.")
/// return json.dumps({"success": True, "pane": name}, ensure_ascii=False)
/// ```
///
/// The `pane` arg mirrors Python's `pane or ""` — callers that have an
/// `Option<String>` should pass `opt.as_deref().unwrap_or("")`. Empty or
/// whitespace-only strings trigger the invalid-pane error.
///
/// The desktop-ui call is delegated to `desktop_ui_emit`; override via
/// `focus_pane_tool_with_emit` in tests or when the gateway has wired a
/// real emitter.
pub fn focus_pane_tool(pane: &str) -> String {
    focus_pane_tool_with_emit(pane, desktop_ui_emit)
}

/// Testable core: same as `focus_pane_tool` but with an injected emit fn.
///
/// `emit` mirrors `desktop_ui.emit`: `Fn(&str, Value) -> Result<bool, String>`
/// where `Ok(true)` = focused, `Ok(false)` = no desktop (not available), `Err(msg)`
/// = exception path.
pub fn focus_pane_tool_with_emit<F>(pane: &str, emit: F) -> String
where
    F: FnOnce(&str, Value) -> Result<bool, String>,
{
    let name = pane.trim().to_lowercase();
    if !PANES.contains(&name.as_str()) {
        return tool_error(&panes_error());
    }

    let payload = json!({ "pane": name });
    let ok = match emit("pane.reveal", payload) {
        Ok(v) => v,
        Err(exc) => {
            return tool_error(&format!("{FAILED_PREFIX}{name}{FAILED_SUFFIX}{exc}"));
        }
    };
    if !ok {
        return tool_error(NOT_AVAILABLE_ERROR);
    }

    // Mirrors `json.dumps({"success": True, "pane": name}, ensure_ascii=False)`
    json!({ "success": true, "pane": name }).to_string()
}

/// Mirrors the registry handler lambda:
/// `lambda args, **kw: focus_pane_tool(pane=args.get("pane", ""))`
///
/// Extracts `pane` as string (missing/non-string → `""`) and delegates to
/// `focus_pane_tool`. The `Option`-to-string fallback preserves Python's
/// `args.get("pane", "")` semantics.
pub fn handler(args: &Value) -> String {
    let pane = args
        .get("pane")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    focus_pane_tool(pane)
}

/// Variant that injects an emitter — mirrors `handler` but testable.
pub fn handler_with_emit<F>(args: &Value, emit: F) -> String
where
    F: FnOnce(&str, Value) -> Result<bool, String>,
{
    let pane = args
        .get("pane")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    focus_pane_tool_with_emit(pane, emit)
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
        assert_eq!(TOOL_NAME, "focus_pane");
        assert_eq!(TOOLSET, "desktop_ui");
        assert_eq!(EMOJI, "🪟");
        assert!(REQUIRES_ENV.is_empty());
        assert_eq!(PANES, &["chat", "files", "terminal", "review", "sessions"]);
        assert_eq!(panes_error(), "pane must be one of: chat, files, terminal, review, sessions.");
        assert_eq!(
            NOT_AVAILABLE_ERROR,
            "Pane focus is only available in the Hermes desktop app."
        );
        assert!(DESCRIPTION.starts_with("Reveal and focus a pane in the Hermes desktop app"));
        assert!(DESCRIPTION.contains("show me the terminal"));
        assert!(DESCRIPTION.contains("open_preview"));
        assert!(DESCRIPTION.contains("close_preview"));
        assert_eq!(PANE_DESCRIPTION, "Which pane to reveal.");
    }

    #[test]
    fn schema_matches_python() {
        let schema = focus_pane_schema();
        assert_eq!(schema["name"], "focus_pane");
        assert_eq!(schema["description"], DESCRIPTION);
        assert_eq!(schema["parameters"]["type"], "object");
        assert_eq!(
            schema["parameters"]["properties"]["pane"]["type"],
            "string"
        );
        assert_eq!(
            schema["parameters"]["properties"]["pane"]["description"],
            PANE_DESCRIPTION
        );
        // enum must be exactly PANES in order
        let enum_vals = schema["parameters"]["properties"]["pane"]["enum"]
            .as_array()
            .unwrap();
        let expected: Vec<Value> = PANES.iter().map(|p| json!(*p)).collect();
        assert_eq!(enum_vals, &expected);
        let required = schema["parameters"]["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "pane");
        // Ensure JSON serialization round-trips (mirrors Python dict)
        let s = focus_pane_schema_json();
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, schema);
    }

    #[test]
    fn invalid_pane_returns_tool_error() {
        let invalids = ["", "   ", "\t\n", "unknown", "chat1", "file", "term"];
        for pane in invalids {
            let out = focus_pane_tool_with_emit(pane, |_, _| Ok(true));
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["error"], panes_error(), "pane={pane:?}");
        }
        // Also via handler with missing key → "" → same error
        let out = handler(&json!({}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], panes_error());

        let out = handler(&json!({"pane": 42}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], panes_error());

        let out = focus_pane_tool("");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], panes_error());
    }

    #[test]
    fn trims_and_lowercases_before_emit_and_success() {
        let cases = [
            ("  chat  ", "chat"),
            ("CHAT", "chat"),
            ("  Terminal ", "terminal"),
            ("REVIEW", "review"),
            ("Files", "files"),
            ("SESSIONS", "sessions"),
            ("\t\n review \n", "review"),
        ];
        for (input, expected) in cases {
            let out = focus_pane_tool_with_emit(input, |event, payload| {
                assert_eq!(event, "pane.reveal");
                assert_eq!(payload["pane"], expected);
                Ok(true)
            });
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["success"], true);
            assert_eq!(v["pane"], expected, "input={input:?}");
        }
    }

    #[test]
    fn success_path_returns_pane() {
        for pane in PANES {
            let out = focus_pane_tool_with_emit(pane, |_, _| Ok(true));
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["success"], true);
            assert_eq!(v["pane"], *pane);
            assert!(v.get("error").is_none());
        }
        // Specific check for terminal
        let out = focus_pane_tool_with_emit("terminal", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["pane"], "terminal");
    }

    #[test]
    fn timeout_when_not_on_desktop() {
        // Ok(false) mirrors desktop_ui.emit returning False (no emitter)
        let out = focus_pane_tool_with_emit("chat", |_, _| Ok(false));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);

        // Default stub also returns timeout (no desktop)
        let out = focus_pane_tool("chat");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);
    }

    #[test]
    fn exception_path_returns_failed_error() {
        let out = focus_pane_tool_with_emit("chat", |_, _| Err("boom".to_string()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], "Failed to focus the chat pane: boom");

        // Handler variant with emit
        let out = handler_with_emit(&json!({"pane": "terminal"}), |_, _| {
            Err("transport closed".to_string())
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(
            v["error"]
                .as_str()
                .unwrap()
                .contains("Failed to focus the terminal pane")
        );

        // Ensure pane name is lowercased in error (input "CHAT" -> "chat" in message)
        let out = focus_pane_tool_with_emit("CHAT", |_, _| Err("oops".to_string()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], "Failed to focus the chat pane: oops");
    }

    #[test]
    fn handler_extracts_pane_like_python_lambda() {
        let out = handler_with_emit(&json!({"pane": "review"}), |_, p| {
            assert_eq!(p["pane"], "review");
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["pane"], "review");

        // Missing → "" → panes error, emit never called
        let out = handler_with_emit(&json!({}), |_, _| {
            panic!("should not emit on invalid pane");
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], panes_error());

        // Lowercases via handler as well
        let out = handler_with_emit(&json!({"pane": "  FILES "}), |_, p| {
            assert_eq!(p["pane"], "files");
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["pane"], "files");
    }

    #[test]
    fn json_preserves_unicode_ensure_ascii_false() {
        // Mirrors ensure_ascii=False: error messages preserve unicode directly.
        // Pane itself is ascii, but tool_error should preserve unicode.
        let out = tool_error("pane café 🪟 error");
        assert!(out.contains("café"));
        assert!(out.contains('🪟'));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], "pane café 🪟 error");

        // Success payload also preserves unicode in surrounding text if pane were unicode
        // (pane validation would fail, but ensure json encoding is unicode-preserving)
        let out = focus_pane_tool_with_emit("chat", |_, _| Ok(true));
        assert!(!out.contains("\\u"));
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

    #[test]
    fn all_panes_are_valid() {
        // Exhaustive validity check: every entry in PANES must succeed with Ok(true)
        for pane in PANES {
            let out = focus_pane_tool_with_emit(pane, |event, payload| {
                assert_eq!(event, "pane.reveal");
                assert_eq!(payload["pane"], *pane);
                Ok(true)
            });
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["success"], true);
        }
        // Any non-PANES string must be rejected
        let out = focus_pane_tool_with_emit("preview", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v.get("error").is_some());
    }
}
