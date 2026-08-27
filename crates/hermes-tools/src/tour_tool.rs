//! Run a guided tour (highlight + narrate UI elements) in the Hermes desktop GUI.
//! Port of `tools/tour_tool.py` (202 lines) — 1:1 behavior.
//!
//! One generic tool, no baked-in tour definitions: the agent discovers what is on
//! screen (`action="targets"`), then highlights any element by CSS selector with
//! its own title/text — either one step at a time (`show`, agent-paced) or as a
//! full step list the user pages through with Next/Prev (`start`).
//!
//! Two surfaces share the same engine (driver.js in the renderer):
//!
//! - `surface="app"` — the Hermes desktop app's own DOM (tours of Hermes itself).
//! - `surface="preview"` — the page loaded in the in-app browser/preview pane
//!   (tours of ANY web app, e.g. a project open via open_preview).
//!
//! Round-trips through the gateway's blocking-prompt bridge like `read_preview`:
//! tui_gateway emits `tour.request`, the renderer drives driver.js (injecting it
//! into the preview's webview when needed) and answers `tour.respond` with the
//! outcome, so the agent knows whether the selector matched. This module is just
//! schema + a thin dispatcher over the platform-injected callback.
//!
//! Lives in the `desktop_ui` toolset, which the GUI gateway enables only for
//! desktop-sourced sessions.

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Registry metadata — mirrors `registry.register(...)` kwargs in Python
// ---------------------------------------------------------------------------

/// Tool name as registered in `tools.registry`.
pub const TOOL_NAME: &str = "tour";
/// Toolset that gates this tool (`toolset="desktop_ui"`).
pub const TOOLSET: &str = "desktop_ui";
/// Emoji for tool listing — mirrors `emoji="🧭"` in Python.
pub const EMOJI: &str = "🧭";
/// `requires_env` for this tool — none (desktop_ui is session-gated, not env-gated).
pub const REQUIRES_ENV: &[&str] = &[];

// ---------------------------------------------------------------------------
// Constants — mirrors Python module-level assignments (lines 30-32)
// ---------------------------------------------------------------------------

/// Mirrors `ACTIONS = ("targets", "show", "start", "next", "prev", "stop")` (30).
pub const ACTIONS: &[&str] = &["targets", "show", "start", "next", "prev", "stop"];

/// Mirrors `SURFACES = ("app", "preview")` (31).
pub const SURFACES: &[&str] = &["app", "preview"];

/// Mirrors `SIDES = ("top", "right", "bottom", "left")` (32).
pub const SIDES: &[&str] = &["top", "right", "bottom", "left"];

// ---------------------------------------------------------------------------
// Error messages — mirrors inline `tool_error(...)` strings
// ---------------------------------------------------------------------------

/// Mirrors `tool_error("tour is only available in the Hermes desktop app.")` (48).
pub const NOT_AVAILABLE_ERROR: &str = "tour is only available in the Hermes desktop app.";

const ACTION_ERROR_PREFIX: &str = "action must be one of: ";
const SURFACE_ERROR_PREFIX: &str = "surface must be one of: ";
const SIDE_ERROR_PREFIX: &str = "side must be one of: ";
const SHOW_EMPTY_ERROR: &str = "show needs a selector (and/or title/text for the popover).";
const START_EMPTY_ERROR: &str = "start needs a non-empty steps array.";
const TOUR_FAILED_PREFIX: &str = "Tour action failed: ";
const EMPTY_ERROR: &str =
    "The tour request timed out, or no GUI window answered. For surface='preview' open a page in the preview pane first.";

/// Full tool description — mirrors `TOUR_SCHEMA["description"]` (129-145).
pub const DESCRIPTION: &str = "Give a live guided tour in the Hermes desktop GUI: dim the screen, highlight an element, and attach a popover with your own title/text. Works on two surfaces — 'app' (the Hermes app itself) and 'preview' (whatever page is open in the in-app browser, so any web app can be toured). ALWAYS call action='targets' first to discover what is on screen instead of guessing selectors; each target reports `stable: true` when its selector keys off identity (data-tour, id, data-testid, aria-label) and survives a re-render — prefer those, and re-scan if a selector stops matching. Then either narrate at your own pace with action='show' (one highlight per call — replaces the previous one; pair each with a chat message describing it), or hand control to the user with action='start' + a steps array (driver.js renders Next/Prev buttons; 'next'/'prev' also page it programmatically). action='stop' clears the tour. Use when the user asks how something works, where something is, or for a walkthrough of an app or workflow.";

const ACTION_DESCRIPTION: &str = "targets: list tourable elements. show: highlight one element. start: begin a multi-step user-paced tour. next/prev: page a started tour. stop: end the tour.";
const SURFACE_DESCRIPTION: &str = "Where the tour runs: 'app' (Hermes desktop UI, default) or 'preview' (the page in the in-app browser pane).";
const SELECTOR_DESCRIPTION: &str = "For show: CSS selector of the element to highlight (from action='targets', preferring a stable one). Omit for a centered narration popover.";
const TITLE_DESCRIPTION: &str = "For show: popover title.";
const TEXT_DESCRIPTION: &str = "For show: popover body text.";
const SIDE_DESCRIPTION: &str = "For show: preferred popover side. Omit to auto-place.";
const STEPS_DESCRIPTION: &str = "For start: the ordered tour steps.";
const STEP_INDEX_DESCRIPTION: &str = "For start: 0-indexed step to begin at (default 0).";

// ---------------------------------------------------------------------------
// Schema — mirrors `TOUR_SCHEMA` dict in Python (110-183)
// ---------------------------------------------------------------------------

/// Returns the JSON schema for `tour` — mirrors `TOUR_SCHEMA`.
///
/// In Python this is a dict literal; here we return a `serde_json::Value`
/// so callers can serialize or inspect it without owning a static JSON string.
pub fn tour_schema() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": DESCRIPTION,
        "parameters": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ACTIONS,
                    "description": ACTION_DESCRIPTION
                },
                "surface": {
                    "type": "string",
                    "enum": SURFACES,
                    "description": SURFACE_DESCRIPTION
                },
                "selector": {
                    "type": "string",
                    "description": SELECTOR_DESCRIPTION
                },
                "title": {
                    "type": "string",
                    "description": TITLE_DESCRIPTION
                },
                "text": {
                    "type": "string",
                    "description": TEXT_DESCRIPTION
                },
                "side": {
                    "type": "string",
                    "enum": SIDES,
                    "description": SIDE_DESCRIPTION
                },
                "steps": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "selector": {
                                "type": "string",
                                "description": "CSS selector of the element this step highlights. Omit for a centered narration-only step."
                            },
                            "title": {
                                "type": "string",
                                "description": "Popover title."
                            },
                            "text": {
                                "type": "string",
                                "description": "Popover body text."
                            },
                            "side": {
                                "type": "string",
                                "enum": SIDES,
                                "description": "Preferred popover side. Omit to auto-place."
                            }
                        }
                    },
                    "description": STEPS_DESCRIPTION
                },
                "step_index": {
                    "type": "integer",
                    "description": STEP_INDEX_DESCRIPTION
                }
            },
            "required": ["action"]
        }
    })
}

/// Serialized schema string — mirrors `TOUR_SCHEMA` as JSON.
pub fn tour_schema_json() -> String {
    tour_schema().to_string()
}

// ---------------------------------------------------------------------------
// Error helpers — mirrors `tools.registry.tool_error` (1:1 truncation)
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

/// Mirrors `tool_error(message, **extra)` in `tools/registry.py`.
///
/// Returns `{"error": <bounded message>}` as a JSON string with
/// `ensure_ascii=False` (Rust's `serde_json` preserves unicode by default).
pub fn tool_error(message: &str) -> String {
    let bounded = bound_error_text(message);
    json!({ "error": bounded }).to_string()
}

fn action_error() -> String {
    format!("{}{}.", ACTION_ERROR_PREFIX, ACTIONS.join(", "))
}

fn surface_error() -> String {
    format!("{}{}.", SURFACE_ERROR_PREFIX, SURFACES.join(", "))
}

fn side_error() -> String {
    format!("{}{}.", SIDE_ERROR_PREFIX, SIDES.join(", "))
}

fn is_empty_step(selector: Option<&str>, title: Option<&str>, text: Option<&str>) -> bool {
    let has_selector = selector.map(|s| !s.is_empty()).unwrap_or(false);
    let has_title = title.map(|s| !s.is_empty()).unwrap_or(false);
    let has_text = text.map(|s| !s.is_empty()).unwrap_or(false);
    !has_selector && !has_title && !has_text
}

fn is_empty_value_step(v: &Value) -> bool {
    // Mirrors `not (step.get("selector") or step.get("title") or step.get("text"))`
    // Python: missing or falsy ("" , None, 0, false) counts as empty. For 1:1 we treat
    // empty string and null/missing as falsy; non-string truthy values count as present.
    let has_selector = match v.get("selector") {
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Null) | None => false,
        Some(_) => true,
    };
    let has_title = match v.get("title") {
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Null) | None => false,
        Some(_) => true,
    };
    let has_text = match v.get("text") {
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Null) | None => false,
        Some(_) => true,
    };
    !has_selector && !has_title && !has_text
}

// ---------------------------------------------------------------------------
// Core handler — mirrors `tour_tool(...) -> str` (35-107)
// ---------------------------------------------------------------------------

/// Dispatch one tour action without a callback — always unavailable.
///
/// Mirrors the `if callback is None:` early return (lines 47-48):
/// ```python
/// if callback is None:
///     return tool_error("tour is only available in the Hermes desktop app.")
/// ```
/// Validation is intentionally skipped here, matching Python's early return before verb checks.
pub fn tour_tool(
    _action: &str,
    _surface: Option<&str>,
    _selector: Option<&str>,
    _title: Option<&str>,
    _text: Option<&str>,
    _side: Option<&str>,
    _steps: Option<&Value>,
    _step_index: Option<&Value>,
) -> String {
    tool_error(NOT_AVAILABLE_ERROR)
}

/// Core with injected callback — mirrors the full `tour_tool` after the None guard.
///
/// `callback` mirrors Python `Callable[[dict], str]` that takes the payload dict and returns raw JSON.
/// In Rust it is `FnOnce(Value) -> Result<String, String>` where `Ok(raw)` is success and `Err(exc)`
/// maps to `tool_error(f"Tour action failed: {exc}")`.
///
/// Steps (lines 50-107):
/// ```python
/// verb = (action or "").strip().lower()
/// if verb not in ACTIONS: return tool_error(...)
/// where = (surface or "app").strip().lower()
/// if where not in SURFACES: return tool_error(...)
/// if side is not None and side not in SIDES: return tool_error(...)
/// def _empty(step): return not (step.get("selector") or step.get("title") or step.get("text"))
/// if verb == "show" and _empty(...): return tool_error(...)
/// if verb == "start":
///     if not isinstance(steps, list) or not steps: return tool_error(...)
///     for i, step in enumerate(steps):
///         if not isinstance(step, dict): return tool_error(f"steps[{i}] must be an object.")
///         if _empty(step): return tool_error(f"steps[{i}] needs a selector and/or title/text.")
/// payload = {k: v for k, v in (...) if v is not None}
/// try: raw = callback(payload) except: return tool_error(...)
/// if not raw: return tool_error("The tour request timed out ...")
/// try: return json.dumps(json.loads(raw), ensure_ascii=False) except: return json.dumps({"text": str(raw)}, ensure_ascii=False)
/// ```
pub fn tour_tool_with_callback<F>(
    action: &str,
    surface: Option<&str>,
    selector: Option<&str>,
    title: Option<&str>,
    text: Option<&str>,
    side: Option<&str>,
    steps: Option<&Value>,
    step_index: Option<&Value>,
    callback: F,
) -> String
where
    F: FnOnce(Value) -> Result<String, String>,
{
    let verb = action.trim().to_lowercase();
    if !ACTIONS.contains(&verb.as_str()) {
        return tool_error(&action_error());
    }

    let where_val = surface.unwrap_or("app").trim().to_lowercase();
    if !SURFACES.contains(&where_val.as_str()) {
        return tool_error(&surface_error());
    }

    if let Some(s) = side {
        if !SIDES.contains(&s) {
            return tool_error(&side_error());
        }
    }

    if verb == "show" && is_empty_step(selector, title, text) {
        return tool_error(SHOW_EMPTY_ERROR);
    }

    if verb == "start" {
        let steps_val = match steps {
            Some(v) => v,
            None => return tool_error(START_EMPTY_ERROR),
        };
        let arr = match steps_val.as_array() {
            Some(a) => a,
            None => return tool_error(START_EMPTY_ERROR),
        };
        if arr.is_empty() {
            return tool_error(START_EMPTY_ERROR);
        }
        for (i, step) in arr.iter().enumerate() {
            if !step.is_object() {
                return tool_error(&format!("steps[{i}] must be an object."));
            }
            if is_empty_value_step(step) {
                return tool_error(&format!("steps[{i}] needs a selector and/or title/text."));
            }
        }
    }

    // Build payload — mirrors lines 77-90, filter `if val is not None`
    let mut map = serde_json::Map::new();
    map.insert("action".to_string(), Value::String(verb.clone()));
    map.insert("surface".to_string(), Value::String(where_val.clone()));
    if let Some(s) = selector {
        map.insert("selector".to_string(), Value::String(s.to_string()));
    }
    if let Some(t) = title {
        map.insert("title".to_string(), Value::String(t.to_string()));
    }
    if let Some(tx) = text {
        map.insert("text".to_string(), Value::String(tx.to_string()));
    }
    if let Some(s) = side {
        map.insert("side".to_string(), Value::String(s.to_string()));
    }
    if let Some(st) = steps {
        map.insert("steps".to_string(), st.clone());
    }
    if let Some(idx) = step_index {
        map.insert("step_index".to_string(), idx.clone());
    }
    let payload = Value::Object(map);

    // Invoke callback — mirrors `raw = callback(payload)` with exception handling
    let raw = match callback(payload) {
        Ok(v) => v,
        Err(exc) => return tool_error(&format!("{TOUR_FAILED_PREFIX}{exc}")),
    };

    if raw.is_empty() {
        return tool_error(EMPTY_ERROR);
    }

    // The renderer answers with a JSON object; pass it through, else wrap it.
    // Mirrors lines 103-107.
    match serde_json::from_str::<Value>(&raw) {
        Ok(parsed) => parsed.to_string(),
        Err(_) => json!({ "text": raw }).to_string(),
    }
}

/// Variant with optional callback — mirrors `callback: Optional[Callable] = None`.
///
/// `None` → `NOT_AVAILABLE_ERROR`, `Some(cb)` → `tour_tool_with_callback(...)`.
pub fn tour_tool_with_optional_callback<F>(
    action: &str,
    surface: Option<&str>,
    selector: Option<&str>,
    title: Option<&str>,
    text: Option<&str>,
    side: Option<&str>,
    steps: Option<&Value>,
    step_index: Option<&Value>,
    callback: Option<F>,
) -> String
where
    F: FnOnce(Value) -> Result<String, String>,
{
    match callback {
        Some(cb) => tour_tool_with_callback(action, surface, selector, title, text, side, steps, step_index, cb),
        None => tool_error(NOT_AVAILABLE_ERROR),
    }
}

// ---------------------------------------------------------------------------
// Registry handler — mirrors `registry.register(..., handler=lambda args, **kw: ...)`
// ---------------------------------------------------------------------------

/// Mirrors the registry handler without a callback (no desktop) — always `NOT_AVAILABLE_ERROR`.
///
/// In Python: `lambda args, **kw: tour_tool(..., callback=kw.get("callback"))`
/// when `kw.get("callback")` is `None`, the early return fires before validation.
pub fn handler(args: &Value) -> String {
    let _ = args;
    tool_error(NOT_AVAILABLE_ERROR)
}

/// Handler with injected callback — mirrors `handler` with `callback` kwarg.
///
/// Extracts `action`, `surface`, `selector`, `title`, `text`, `side`, `steps`, `step_index`
/// from `args` (missing/non-string → defaults) and delegates to `tour_tool_with_callback`.
pub fn handler_with_callback<F>(args: &Value, callback: F) -> String
where
    F: FnOnce(Value) -> Result<String, String>,
{
    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let surface = args.get("surface").and_then(|v| v.as_str());
    let selector = args.get("selector").and_then(|v| v.as_str());
    let title = args.get("title").and_then(|v| v.as_str());
    let text = args.get("text").and_then(|v| v.as_str());
    let side = args.get("side").and_then(|v| v.as_str());
    let steps = args.get("steps");
    let step_index = args.get("step_index");
    tour_tool_with_callback(action, surface, selector, title, text, side, steps, step_index, callback)
}

/// Handler with optional callback — mirrors `kw.get("callback")` may be `None`.
pub fn handler_with_optional_callback<F>(args: &Value, callback: Option<F>) -> String
where
    F: FnOnce(Value) -> Result<String, String>,
{
    match callback {
        Some(cb) => handler_with_callback(args, cb),
        None => handler(args),
    }
}

// ---------------------------------------------------------------------------
// Tests — mirrors Python behavior (lines 35-107) and schema (110-183)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ok_callback(expected: Value, raw: &str) -> impl FnOnce(Value) -> Result<String, String> {
        let raw = raw.to_string();
        move |payload: Value| {
            assert_eq!(payload, expected);
            Ok(raw.clone())
        }
    }

    #[test]
    fn constants_match_python() {
        assert_eq!(ACTIONS, &["targets", "show", "start", "next", "prev", "stop"]);
        assert_eq!(SURFACES, &["app", "preview"]);
        assert_eq!(SIDES, &["top", "right", "bottom", "left"]);
        assert_eq!(TOOL_NAME, "tour");
        assert_eq!(TOOLSET, "desktop_ui");
        assert_eq!(EMOJI, "🧭");
        assert!(REQUIRES_ENV.is_empty());
        assert_eq!(NOT_AVAILABLE_ERROR, "tour is only available in the Hermes desktop app.");
        assert_eq!(action_error(), "action must be one of: targets, show, start, next, prev, stop.");
        assert_eq!(surface_error(), "surface must be one of: app, preview.");
        assert_eq!(side_error(), "side must be one of: top, right, bottom, left.");
        assert_eq!(SHOW_EMPTY_ERROR, "show needs a selector (and/or title/text for the popover).");
        assert_eq!(START_EMPTY_ERROR, "start needs a non-empty steps array.");
        assert_eq!(TOUR_FAILED_PREFIX, "Tour action failed: ");
        assert_eq!(EMPTY_ERROR, "The tour request timed out, or no GUI window answered. For surface='preview' open a page in the preview pane first.");
        assert!(DESCRIPTION.starts_with("Give a live guided tour in the Hermes desktop GUI"));
        assert!(DESCRIPTION.contains("dim the screen"));
        assert!(DESCRIPTION.contains("driver.js"));
        assert!(DESCRIPTION.contains("stable: true"));
    }

    #[test]
    fn schema_matches_python() {
        let schema = tour_schema();
        assert_eq!(schema["name"], "tour");
        assert_eq!(schema["description"], DESCRIPTION);
        assert_eq!(schema["parameters"]["type"], "object");
        assert_eq!(schema["parameters"]["properties"]["action"]["type"], "string");
        let enum_vals = schema["parameters"]["properties"]["action"]["enum"].as_array().unwrap();
        let expected: Vec<Value> = ACTIONS.iter().map(|a| json!(*a)).collect();
        assert_eq!(enum_vals, &expected);
        assert_eq!(schema["parameters"]["properties"]["action"]["description"], ACTION_DESCRIPTION);
        assert_eq!(schema["parameters"]["properties"]["surface"]["enum"], json!(SURFACES));
        assert_eq!(schema["parameters"]["properties"]["surface"]["description"], SURFACE_DESCRIPTION);
        assert_eq!(schema["parameters"]["properties"]["selector"]["description"], SELECTOR_DESCRIPTION);
        assert_eq!(schema["parameters"]["properties"]["title"]["description"], TITLE_DESCRIPTION);
        assert_eq!(schema["parameters"]["properties"]["text"]["description"], TEXT_DESCRIPTION);
        assert_eq!(schema["parameters"]["properties"]["side"]["enum"], json!(SIDES));
        assert_eq!(schema["parameters"]["properties"]["side"]["description"], SIDE_DESCRIPTION);
        assert_eq!(schema["parameters"]["properties"]["steps"]["type"], "array");
        assert_eq!(schema["parameters"]["properties"]["steps"]["description"], STEPS_DESCRIPTION);
        assert_eq!(schema["parameters"]["properties"]["steps"]["items"]["type"], "object");
        assert_eq!(schema["parameters"]["properties"]["steps"]["items"]["properties"]["selector"]["type"], "string");
        assert_eq!(schema["parameters"]["properties"]["steps"]["items"]["properties"]["side"]["enum"], json!(SIDES));
        assert_eq!(schema["parameters"]["properties"]["step_index"]["type"], "integer");
        assert_eq!(schema["parameters"]["properties"]["step_index"]["description"], STEP_INDEX_DESCRIPTION);
        let required = schema["parameters"]["required"].as_array().unwrap();
        assert_eq!(required, &vec![json!("action")]);
        let s = tour_schema_json();
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, schema);
    }

    #[test]
    fn not_available_without_callback() {
        let out = tour_tool("targets", None, None, None, None, None, None, None);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);

        let out = handler(&json!({"action": "targets"}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);

        let out = tour_tool_with_optional_callback::<fn(Value) -> Result<String, String>>(
            "targets", None, None, None, None, None, None, None, None,
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);

        let out = handler_with_optional_callback::<fn(Value) -> Result<String, String>>(
            &json!({"action": "show", "selector": "#x"}),
            None,
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);
    }

    #[test]
    fn invalid_action_returns_error() {
        for action in ["", "   ", "unknown", "targ"] {
            let out = tour_tool_with_callback(action, None, None, None, None, None, None, None, |_| Ok("{}".to_string()));
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["error"], action_error(), "action={action:?}");
        }
        // case insensitive after trim
        let out = tour_tool_with_callback("  TARGETS ", None, None, None, None, None, None, None, |payload| {
            assert_eq!(payload["action"], "targets");
            Ok(r#"{"ok":true}"#.to_string())
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);

        let out = handler_with_callback(&json!({"action": "bad"}), |_| Ok("{}".to_string()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], action_error());
    }

    #[test]
    fn surface_validation() {
        let out = tour_tool_with_callback("targets", Some("bad"), None, None, None, None, None, None, |_| panic!("should not call"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], surface_error());

        // default surface app
        let out = tour_tool_with_callback("targets", None, None, None, None, None, None, None, |payload| {
            assert_eq!(payload["surface"], "app");
            Ok(r#"{"ok":true}"#.to_string())
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);

        // trims and lowercases
        let out = tour_tool_with_callback("targets", Some("  PREVIEW "), None, None, None, None, None, None, |payload| {
            assert_eq!(payload["surface"], "preview");
            Ok(r#"{"ok":true}"#.to_string())
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn side_validation() {
        let out = tour_tool_with_callback("show", None, Some("#x"), Some("t"), None, Some("bad"), None, None, |_| panic!("should not call"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], side_error());

        for valid in SIDES {
            let out = tour_tool_with_callback("show", None, Some("#x"), None, None, Some(valid), None, None, |payload| {
                assert_eq!(payload["side"], *valid);
                Ok(r#"{"ok":true}"#.to_string())
            });
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["ok"], true);
        }

        // None side is allowed (no field)
        let out = tour_tool_with_callback("targets", None, None, None, None, None, None, None, |payload| {
            assert!(payload.get("side").is_none());
            Ok(r#"{"ok":true}"#.to_string())
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn show_needs_selector_or_title_text() {
        let out = tour_tool_with_callback("show", None, None, None, None, None, None, None, |_| panic!("should not call"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], SHOW_EMPTY_ERROR);

        // empty strings also count as empty
        let out = tour_tool_with_callback("show", None, Some(""), Some(""), Some(""), None, None, None, |_| panic!("should not call"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], SHOW_EMPTY_ERROR);

        // selector alone ok
        let out = tour_tool_with_callback("show", None, Some("#x"), None, None, None, None, None, |payload| {
            assert_eq!(payload["selector"], "#x");
            Ok(r#"{"ok":true}"#.to_string())
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);

        // title alone ok
        let out = tour_tool_with_callback("show", None, None, Some("hello"), None, None, None, None, |payload| {
            assert_eq!(payload["title"], "hello");
            Ok(r#"{"ok":true}"#.to_string())
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);

        // text alone ok
        let out = tour_tool_with_callback("show", None, None, None, Some("body"), None, None, None, |payload| {
            assert_eq!(payload["text"], "body");
            Ok(r#"{"ok":true}"#.to_string())
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn start_validation() {
        // missing steps
        let out = tour_tool_with_callback("start", None, None, None, None, None, None, None, |_| panic!("should not call"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], START_EMPTY_ERROR);

        // steps not array
        let out = tour_tool_with_callback("start", None, None, None, None, None, Some(&json!({"selector": "#x"})), None, |_| panic!("should not call"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], START_EMPTY_ERROR);

        // empty array
        let out = tour_tool_with_callback("start", None, None, None, None, None, Some(&json!([])), None, |_| panic!("should not call"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], START_EMPTY_ERROR);

        // step not object
        let out = tour_tool_with_callback("start", None, None, None, None, None, Some(&json!(["not an object"])), None, |_| panic!("should not call"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], "steps[0] must be an object.");

        // step empty
        let out = tour_tool_with_callback("start", None, None, None, None, None, Some(&json!([{}])), None, |_| panic!("should not call"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], "steps[0] needs a selector and/or title/text.");

        // step with empty strings also empty
        let out = tour_tool_with_callback("start", None, None, None, None, None, Some(&json!([{"selector": "", "title": "", "text": ""}])), None, |_| panic!("should not call"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], "steps[0] needs a selector and/or title/text.");

        // second step empty reports index 1
        let out = tour_tool_with_callback("start", None, None, None, None, None, Some(&json!([{"selector": "#a"}, {}])), None, |_| panic!("should not call"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], "steps[1] needs a selector and/or title/text.");

        // valid steps
        let steps = json!([{"selector": "#a", "title": "t"}, {"title": "only title"}, {"text": "body"}]);
        let out = tour_tool_with_callback("start", None, None, None, None, None, Some(&steps), None, |payload| {
            assert_eq!(payload["steps"], steps);
            Ok(r#"{"ok":true}"#.to_string())
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn payload_filters_none() {
        let steps = json!([{"selector": "#a"}]);
        let out = tour_tool_with_callback(
            "start",
            Some("preview"),
            None,
            None,
            None,
            None,
            Some(&steps),
            Some(&json!(1)),
            |payload| {
                assert_eq!(payload["action"], "start");
                assert_eq!(payload["surface"], "preview");
                assert_eq!(payload["steps"], steps);
                assert_eq!(payload["step_index"], 1);
                assert!(payload.get("selector").is_none());
                assert!(payload.get("title").is_none());
                assert!(payload.get("text").is_none());
                assert!(payload.get("side").is_none());
                Ok(r#"{"ok":true}"#.to_string())
            },
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);

        // show with all fields
        let out = tour_tool_with_callback(
            "show",
            None,
            Some("#x"),
            Some("t"),
            Some("b"),
            Some("top"),
            None,
            None,
            |payload| {
                assert_eq!(payload["action"], "show");
                assert_eq!(payload["surface"], "app");
                assert_eq!(payload["selector"], "#x");
                assert_eq!(payload["title"], "t");
                assert_eq!(payload["text"], "b");
                assert_eq!(payload["side"], "top");
                assert!(payload.get("steps").is_none());
                assert!(payload.get("step_index").is_none());
                Ok(r#"{"ok":true}"#.to_string())
            },
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn callback_exception_and_empty() {
        let out = tour_tool_with_callback("targets", None, None, None, None, None, None, None, |_| Err("boom".to_string()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], "Tour action failed: boom");

        let out = tour_tool_with_callback("targets", None, None, None, None, None, None, None, |_| Ok(String::new()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], EMPTY_ERROR);

        let out = handler_with_callback(&json!({"action": "targets"}), |_| Err("transport".to_string()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["error"].as_str().unwrap().contains("Tour action failed"));

        let out = handler_with_callback(&json!({"action": "targets"}), |_| Ok("".to_string()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], EMPTY_ERROR);
    }

    #[test]
    fn json_pass_through_and_wrap() {
        let raw = r#"{"found":true,"selector":"#x"}"#;
        let out = tour_tool_with_callback("show", None, Some("#x"), None, None, None, None, None, |_| Ok(raw.to_string()));
        let v: Value = serde_json::from_str(&out).unwrap();
        let expected: Value = serde_json::from_str(raw).unwrap();
        assert_eq!(v, expected);

        // unicode preserved
        let raw_unicode = r#"{"text":"café 🧭"}"#;
        let out = tour_tool_with_callback("targets", None, None, None, None, None, None, None, |_| Ok(raw_unicode.to_string()));
        assert!(out.contains("café"));
        assert!(out.contains("🧭"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["text"], "café 🧭");

        // invalid JSON wrapped
        let raw_plain = "not json";
        let out = tour_tool_with_callback("targets", None, None, None, None, None, None, None, |_| Ok(raw_plain.to_string()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["text"], raw_plain);

        let raw_plain2 = "plain café 🧭";
        let out = tour_tool_with_callback("targets", None, None, None, None, None, None, None, |_| Ok(raw_plain2.to_string()));
        assert!(out.contains("café"));
        assert!(out.contains("🧭"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["text"], raw_plain2);
    }

    #[test]
    fn handler_extracts_like_python_lambda() {
        let steps = json!([{"selector": "#a"}]);
        let args = json!({
            "action": "start",
            "surface": "preview",
            "selector": "#x",
            "title": "t",
            "text": "b",
            "side": "left",
            "steps": steps,
            "step_index": 2
        });
        let out = handler_with_callback(&args, |payload| {
            assert_eq!(payload["action"], "start");
            assert_eq!(payload["surface"], "preview");
            assert_eq!(payload["selector"], "#x");
            assert_eq!(payload["title"], "t");
            assert_eq!(payload["text"], "b");
            assert_eq!(payload["side"], "left");
            assert_eq!(payload["steps"], steps);
            assert_eq!(payload["step_index"], 2);
            Ok(r#"{"ok":true}"#.to_string())
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);

        // missing action -> error
        let out = handler_with_callback(&json!({}), |_| Ok("{}".to_string()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], action_error());

        // targets without extra fields
        let out = handler_with_callback(&json!({"action": "targets"}), |payload| {
            assert_eq!(payload["action"], "targets");
            assert_eq!(payload["surface"], "app");
            Ok(r#"{"targets":[]}"#.to_string())
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["targets"], json!([]));
    }

    #[test]
    fn valid_actions_normalize() {
        for action in ["targets", "  SHOW ", "Start", "NEXT", " prev ", "STOP"] {
            let normalized = action.trim().to_lowercase();
            let need_show = normalized == "show";
            let need_start = normalized == "start";
            let selector = if need_show { Some("#x") } else { None };
            let steps = if need_start { Some(json!([{"selector": "#a"}])) } else { None };
            let steps_ref = steps.as_ref();
            let out = tour_tool_with_callback(action, None, selector, None, None, None, steps_ref, None, |payload| {
                assert_eq!(payload["action"], normalized);
                Ok(r#"{"ok":true}"#.to_string())
            });
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["ok"], true, "action={action:?}");
        }
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
    fn handler_bare_always_not_available() {
        let out = handler(&json!({"action": "bad"}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);
        let out = handler(&json!({}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);
    }

    #[test]
    fn other_verbs_no_empty_check() {
        for verb in ["targets", "next", "prev", "stop"] {
            let out = tour_tool_with_callback(verb, None, None, None, None, None, None, None, |payload| {
                assert_eq!(payload["action"], verb);
                Ok(r#"{"ok":true}"#.to_string())
            });
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["ok"], true, "verb={verb}");
        }
    }
}
