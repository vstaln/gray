//! Interact with the in-app browser / preview pane in the Hermes desktop GUI.
//! Port of `tools/drive_preview_tool.py` (234 lines) — 1:1 behavior.
//!
//! ``open_preview`` shows a page and ``read_preview`` reads it; this tool is the
//! third leg — clicking, typing, scrolling, and history — so the agent can drive
//! the same page the user is looking at instead of narrating from the outside.
//!
//! Elements are addressed by refs from ``action="elements"`` that say what they
//! are: ``btn-sign-in``, ``inp-email``. A ref lasts as long as the page is open,
//! including across a re-render that destroys and rebuilds the element, and only a
//! navigation retires it — the renderer says so rather than acting on whatever now
//! occupies the spot.
//!
//! Because the refs hold, the renderer answers with a *delta* — what appeared,
//! what went, what changed, and what was rebound — instead of re-sending the whole
//! inventory after every click. That is the cheap half of the arrangement, and it
//! only works because the refs are legible enough to read on their own three turns
//! later.
//!
//! Round-trips through the gateway's blocking-prompt bridge like ``read_preview``:
//! tui_gateway emits ``preview.act.request``, the renderer injects the interaction
//! engine into the pane's webview and answers ``preview.act.respond`` with the
//! outcome plus whatever moved. This module is just schema + a thin dispatcher
//! over the platform-injected callback.
//!
//! Lives in the ``desktop_ui`` toolset, which the GUI gateway enables only for
//! desktop-sourced sessions.

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Registry metadata — mirrors `registry.register(...)` kwargs in Python
// ---------------------------------------------------------------------------

/// Tool name as registered in `tools.registry`.
pub const TOOL_NAME: &str = "drive_preview";
/// Toolset that gates this tool (`toolset="desktop_ui"`).
pub const TOOLSET: &str = "desktop_ui";
/// Emoji for tool listing — mirrors `emoji="🖱️"` in Python.
pub const EMOJI: &str = "🖱️";
/// `requires_env` for this tool — none (desktop_ui is session-gated, not env-gated).
pub const REQUIRES_ENV: &[&str] = &[];

// ---------------------------------------------------------------------------
// Constants — mirrors Python module-level assignments (lines 35-51)
// ---------------------------------------------------------------------------

/// Mirrors `ACTIONS = ("elements", "click", "hover", "type", "scroll", "press", "strobe", "back", "forward", "reload")` (35-46).
pub const ACTIONS: &[&str] = &[
    "elements", "click", "hover", "type", "scroll", "press", "strobe", "back", "forward", "reload",
];

/// Mirrors `SCROLL_TO = ("top", "bottom")` (47).
pub const SCROLL_TO: &[&str] = &["top", "bottom"];

/// Mirrors `NEEDS_TARGET = ("click", "hover", "type", "press")` (51).
pub const NEEDS_TARGET: &[&str] = &["click", "hover", "type", "press"];

// ---------------------------------------------------------------------------
// Error messages — mirrors inline `tool_error(...)` strings
// ---------------------------------------------------------------------------

/// Mirrors `tool_error("drive_preview is only available in the Hermes desktop app.")` (69).
pub const NOT_AVAILABLE_ERROR: &str = "drive_preview is only available in the Hermes desktop app.";

const ACTION_ERROR_PREFIX: &str = "action must be one of: ";
const NEEDS_TARGET_SUFFIX: &str =
    " needs a ref from action='elements' (e.g. 'btn-sign-in') or a CSS selector.";
const TYPE_NEEDS_TEXT_ERROR: &str = "type needs the text to enter.";
const PRESS_NEEDS_KEY_ERROR: &str = "press needs a key, e.g. 'Enter' or 'Escape'.";
const TO_ERROR_PREFIX: &str = "to must be one of: ";
const AMOUNT_ERROR: &str = "amount and max must be integers.";
const FAILED_PREFIX: &str = "Failed to act on the in-app browser: ";
const EMPTY_ERROR: &str =
    "The action timed out, or no GUI window answered. Open a page with open_preview first.";

/// Full tool description — mirrors `ACT_PREVIEW_SCHEMA["description"]` (129-167).
pub const DESCRIPTION: &str = "Interact with the page open in the in-app browser / preview pane of the Hermes desktop GUI — the pane open_preview opens beside this chat. This is how you USE a web app the user is looking at: log in, fill a form, click through a flow, page a long document. ALWAYS call action='elements' first to get the current inventory of clickable and typable things — each carries a ref like 'btn-sign-in' or 'inp-email' plus its role, label, and value — then act with that ref instead of guessing a selector. A ref keeps working for as long as the page is open, INCLUDING across a re-render that rebuilds the element, so hold onto the ones you were given. Every action answers with the live url/title plus what moved: the first look at a page returns the full 'elements' inventory, and after that you get a 'delta' instead — 'added' entries in full, 'changed' entries carrying only the ref and whichever of label/value/disabled actually moved, 'removed' and 'rebound' as bare ref lists, and 'same' counting the refs that held. A 'rebound' ref needs NO action from you; it means the page rebuilt that element and your ref already follows it. Anything not mentioned in a delta is unchanged, so do not re-read the page to check. Only a navigation invalidates refs; when told they are stale, call elements again. The mouse and keyboard are real: the pointer travels to its target and the page sees genuine input, so hover menus open and hover-only controls work. Actions: 'elements' (inventory), 'click', 'hover' (move the pointer onto something and leave it there — use it to open a dropdown or reveal a tooltip before clicking inside it), 'type' (set a field's text; submit=true also presses Enter and submits the form), 'scroll' (the page, or a ref'd scrollable), 'press' (a named key), 'strobe' (touch nothing — just rattle the highlight through the page again; 'elements' already does this once, so reach for it only when asked to flick, flash, or bounce around the page some more, and note one call runs a whole multi-second burst, so never loop it per element), and 'back'/'forward'/'reload' for history. The pane draws every move as it happens so the user can follow along; those marks fade on their own, and annotate_preview is how you leave one up on purpose. Use read_preview when you only need the page's text, and the browser_* tools when the work belongs in a separate automated browser rather than the user's own pane.";

const ACTION_DESCRIPTION: &str = "What to do. Start with 'elements'.";
const REF_DESCRIPTION: &str =
    "Element reference from any earlier elements call (e.g. 'btn-sign-in'). Good until the page navigates.";
const SELECTOR_DESCRIPTION: &str = "CSS selector, as a fallback when no ref fits. Prefer ref.";
const TEXT_DESCRIPTION: &str = "For 'type': the text to enter.";
const SUBMIT_DESCRIPTION: &str =
    "For 'type': press Enter and submit the owning form afterwards.";
const KEY_DESCRIPTION: &str =
    "For 'press': the key name, e.g. 'Enter', 'Escape', 'ArrowDown'.";
const AMOUNT_DESCRIPTION: &str =
    "For 'scroll': pixels to scroll (negative scrolls up). Defaults to about one screen.";
const TO_DESCRIPTION: &str =
    "For 'scroll': jump to the top or bottom instead of a distance.";
const MAX_DESCRIPTION: &str =
    "For 'elements': cap the inventory. Defaults to the per-call maximum.";
const FULL_DESCRIPTION: &str =
    "For 'elements': re-read the whole page instead of a delta. Rarely needed.";

// ---------------------------------------------------------------------------
// Schema — mirrors `ACT_PREVIEW_SCHEMA` dict in Python (127-213)
// ---------------------------------------------------------------------------

/// Returns the JSON schema for `drive_preview` — mirrors `ACT_PREVIEW_SCHEMA`.
///
/// In Python this is a dict literal; here we return a `serde_json::Value`
/// so callers can serialize or inspect it without owning a static JSON string.
pub fn drive_preview_schema() -> Value {
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
                "ref": {
                    "type": "string",
                    "description": REF_DESCRIPTION
                },
                "selector": {
                    "type": "string",
                    "description": SELECTOR_DESCRIPTION
                },
                "text": {
                    "type": "string",
                    "description": TEXT_DESCRIPTION
                },
                "submit": {
                    "type": "boolean",
                    "description": SUBMIT_DESCRIPTION
                },
                "key": {
                    "type": "string",
                    "description": KEY_DESCRIPTION
                },
                "amount": {
                    "type": "integer",
                    "description": AMOUNT_DESCRIPTION
                },
                "to": {
                    "type": "string",
                    "enum": SCROLL_TO,
                    "description": TO_DESCRIPTION
                },
                "max": {
                    "type": "integer",
                    "description": MAX_DESCRIPTION
                },
                "full": {
                    "type": "boolean",
                    "description": FULL_DESCRIPTION
                }
            },
            "required": ["action"]
        }
    })
}

/// Serialized schema string — mirrors `ACT_PREVIEW_SCHEMA` as JSON.
pub fn drive_preview_schema_json() -> String {
    drive_preview_schema().to_string()
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

fn to_error() -> String {
    format!("{}{}.", TO_ERROR_PREFIX, SCROLL_TO.join(", "))
}

// ---------------------------------------------------------------------------
// Integer parsing — mirrors `int(amount)` / `int(limit)` with TypeError/ValueError
// ---------------------------------------------------------------------------

fn parse_int_value(v: &Value) -> Result<i64, ()> {
    match v {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else if let Some(u) = n.as_u64() {
                if u <= i64::MAX as u64 {
                    Ok(u as i64)
                } else {
                    Err(())
                }
            } else if let Some(f) = n.as_f64() {
                // Python int(1.5) -> 1 truncates; mirror for float numbers
                if f.is_finite() && f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64 {
                    Ok(f as i64)
                } else if f.is_finite() && f.fract() != 0.0 {
                    // Python would truncate e.g. int(1.9) -> 1; allow truncation for parity
                    // but only if caller intentionally passed float — schema says integer,
                    // so truncate rather than error to match Python's permissive int().
                    Ok(f.trunc() as i64)
                } else {
                    Err(())
                }
            } else {
                Err(())
            }
        }
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Err(());
            }
            // Python int(" 5 ") succeeds, int("5.0") fails -> we mirror strict i64 parse
            trimmed.parse::<i64>().map_err(|_| ())
        }
        Value::Bool(b) => Ok(if *b { 1 } else { 0 }),
        _ => Err(()),
    }
}

// ---------------------------------------------------------------------------
// Core handler — mirrors `drive_preview_tool(...) -> str` (54-124)
// ---------------------------------------------------------------------------

/// Drive the preview pane without a callback — always unavailable.
///
/// Mirrors the `if callback is None:` early return (lines 68-69):
/// ```python
/// if callback is None:
///     return tool_error("drive_preview is only available in the Hermes desktop app.")
/// ```
/// Validation is intentionally skipped here, matching Python's early return before verb checks.
pub fn drive_preview_tool(
    _action: &str,
    _ref_: Option<&str>,
    _selector: Option<&str>,
    _text: Option<&str>,
    _key: Option<&str>,
    _submit: Option<bool>,
    _amount: Option<&Value>,
    _to: Option<&str>,
    _limit: Option<&Value>,
    _full: Option<bool>,
) -> String {
    tool_error(NOT_AVAILABLE_ERROR)
}

/// Core with injected callback — mirrors the full `drive_preview_tool` after the None guard.
///
/// `callback` mirrors Python `Callable[[dict], str]` that takes the payload dict and returns raw JSON.
/// In Rust it is `FnOnce(Value) -> Result<String, String>` where `Ok(raw)` is success and `Err(exc)`
/// maps to `tool_error(f"Failed to act on the in-app browser: {exc}")`.
///
/// Steps (lines 71-124):
/// ```python
/// verb = (action or "").strip().lower()
/// if verb not in ACTIONS: return tool_error(...)
/// if verb in NEEDS_TARGET and not (ref or selector): return tool_error(...)
/// if verb == "type" and text is None: return tool_error(...)
/// if verb == "press" and not key: return tool_error(...)
/// if to is not None and to not in SCROLL_TO: return tool_error(...)
/// try: payload = { ... int(amount), int(limit) ... } except: return tool_error("amount and max must be integers.")
/// try: raw = callback(payload) except: return tool_error(f"Failed to act ... {exc}")
/// if not raw: return tool_error("The action timed out ...")
/// try: return json.dumps(json.loads(raw), ensure_ascii=False) except: return json.dumps({"text": str(raw)}, ensure_ascii=False)
/// ```
pub fn drive_preview_tool_with_callback<F>(
    action: &str,
    ref_: Option<&str>,
    selector: Option<&str>,
    text: Option<&str>,
    key: Option<&str>,
    submit: Option<bool>,
    amount: Option<&Value>,
    to: Option<&str>,
    limit: Option<&Value>,
    full: Option<bool>,
    callback: F,
) -> String
where
    F: FnOnce(Value) -> Result<String, String>,
{
    let verb = action.trim().to_lowercase();
    if !ACTIONS.contains(&verb.as_str()) {
        return tool_error(&action_error());
    }

    // NEEDS_TARGET check mirrors `if verb in NEEDS_TARGET and not (ref or selector):`
    // Python treats "" as falsy, so empty strings count as missing.
    let has_ref = ref_.map(|s| !s.is_empty()).unwrap_or(false);
    let has_selector = selector.map(|s| !s.is_empty()).unwrap_or(false);
    if NEEDS_TARGET.contains(&verb.as_str()) && !has_ref && !has_selector {
        return tool_error(&format!("{verb}{NEEDS_TARGET_SUFFIX}"));
    }

    if verb == "type" && text.is_none() {
        return tool_error(TYPE_NEEDS_TEXT_ERROR);
    }

    if verb == "press" {
        let has_key = key.map(|k| !k.is_empty()).unwrap_or(false);
        if !has_key {
            return tool_error(PRESS_NEEDS_KEY_ERROR);
        }
    }

    if let Some(to_val) = to {
        if !SCROLL_TO.contains(&to_val) {
            return tool_error(&to_error());
        }
    }

    // Build payload — mirrors lines 89-107, with int conversion for amount/limit
    let mut amount_int: Option<i64> = None;
    if let Some(v) = amount {
        match parse_int_value(v) {
            Ok(i) => amount_int = Some(i),
            Err(_) => return tool_error(AMOUNT_ERROR),
        }
    }
    let mut limit_int: Option<i64> = None;
    if let Some(v) = limit {
        match parse_int_value(v) {
            Ok(i) => limit_int = Some(i),
            Err(_) => return tool_error(AMOUNT_ERROR),
        }
    }

    let mut map = serde_json::Map::new();
    map.insert("action".to_string(), Value::String(verb.clone()));
    if let Some(r) = ref_ {
        map.insert("ref".to_string(), Value::String(r.to_string()));
    }
    if let Some(s) = selector {
        map.insert("selector".to_string(), Value::String(s.to_string()));
    }
    if let Some(t) = text {
        map.insert("text".to_string(), Value::String(t.to_string()));
    }
    if let Some(k) = key {
        map.insert("key".to_string(), Value::String(k.to_string()));
    }
    if let Some(sb) = submit {
        map.insert("submit".to_string(), Value::Bool(sb));
    }
    if let Some(f) = full {
        map.insert("full".to_string(), Value::Bool(f));
    }
    if let Some(tv) = to {
        map.insert("to".to_string(), Value::String(tv.to_string()));
    }
    if let Some(ai) = amount_int {
        map.insert("amount".to_string(), json!(ai));
    }
    if let Some(li) = limit_int {
        map.insert("max".to_string(), json!(li));
    }
    let payload = Value::Object(map);

    // Invoke callback — mirrors `raw = callback(payload)` with exception handling
    let raw = match callback(payload) {
        Ok(v) => v,
        Err(exc) => return tool_error(&format!("{FAILED_PREFIX}{exc}")),
    };

    if raw.is_empty() {
        return tool_error(EMPTY_ERROR);
    }

    // Renderer answers with a JSON object; pass it through, else wrap it.
    // Mirrors lines 121-124.
    match serde_json::from_str::<Value>(&raw) {
        Ok(parsed) => parsed.to_string(),
        Err(_) => json!({ "text": raw }).to_string(),
    }
}

/// Variant with optional callback — mirrors `callback: Optional[Callable] = None`.
///
/// `None` → `NOT_AVAILABLE_ERROR`, `Some(cb)` → `drive_preview_tool_with_callback(...)`.
pub fn drive_preview_tool_with_optional_callback<F>(
    action: &str,
    ref_: Option<&str>,
    selector: Option<&str>,
    text: Option<&str>,
    key: Option<&str>,
    submit: Option<bool>,
    amount: Option<&Value>,
    to: Option<&str>,
    limit: Option<&Value>,
    full: Option<bool>,
    callback: Option<F>,
) -> String
where
    F: FnOnce(Value) -> Result<String, String>,
{
    match callback {
        Some(cb) => drive_preview_tool_with_callback(
            action, ref_, selector, text, key, submit, amount, to, limit, full, cb,
        ),
        None => tool_error(NOT_AVAILABLE_ERROR),
    }
}

// ---------------------------------------------------------------------------
// Typed convenience — `amount`/`limit` as `Option<i64>` (no parse error path)
// ---------------------------------------------------------------------------

/// Typed wrapper where `amount`/`limit` are already integers.
///
/// Skips the `int()` parse step; useful for callers that already have `i64`.
/// Validation and payload shape are otherwise identical to `drive_preview_tool_with_callback`.
pub fn drive_preview_tool_with_callback_ints<F>(
    action: &str,
    ref_: Option<&str>,
    selector: Option<&str>,
    text: Option<&str>,
    key: Option<&str>,
    submit: Option<bool>,
    amount: Option<i64>,
    to: Option<&str>,
    limit: Option<i64>,
    full: Option<bool>,
    callback: F,
) -> String
where
    F: FnOnce(Value) -> Result<String, String>,
{
    let verb = action.trim().to_lowercase();
    if !ACTIONS.contains(&verb.as_str()) {
        return tool_error(&action_error());
    }
    let has_ref = ref_.map(|s| !s.is_empty()).unwrap_or(false);
    let has_selector = selector.map(|s| !s.is_empty()).unwrap_or(false);
    if NEEDS_TARGET.contains(&verb.as_str()) && !has_ref && !has_selector {
        return tool_error(&format!("{verb}{NEEDS_TARGET_SUFFIX}"));
    }
    if verb == "type" && text.is_none() {
        return tool_error(TYPE_NEEDS_TEXT_ERROR);
    }
    if verb == "press" {
        let has_key = key.map(|k| !k.is_empty()).unwrap_or(false);
        if !has_key {
            return tool_error(PRESS_NEEDS_KEY_ERROR);
        }
    }
    if let Some(to_val) = to {
        if !SCROLL_TO.contains(&to_val) {
            return tool_error(&to_error());
        }
    }
    let mut map = serde_json::Map::new();
    map.insert("action".to_string(), Value::String(verb.clone()));
    if let Some(r) = ref_ {
        map.insert("ref".to_string(), Value::String(r.to_string()));
    }
    if let Some(s) = selector {
        map.insert("selector".to_string(), Value::String(s.to_string()));
    }
    if let Some(t) = text {
        map.insert("text".to_string(), Value::String(t.to_string()));
    }
    if let Some(k) = key {
        map.insert("key".to_string(), Value::String(k.to_string()));
    }
    if let Some(sb) = submit {
        map.insert("submit".to_string(), Value::Bool(sb));
    }
    if let Some(f) = full {
        map.insert("full".to_string(), Value::Bool(f));
    }
    if let Some(tv) = to {
        map.insert("to".to_string(), Value::String(tv.to_string()));
    }
    if let Some(ai) = amount {
        map.insert("amount".to_string(), json!(ai));
    }
    if let Some(li) = limit {
        map.insert("max".to_string(), json!(li));
    }
    let payload = Value::Object(map);
    let raw = match callback(payload) {
        Ok(v) => v,
        Err(exc) => return tool_error(&format!("{FAILED_PREFIX}{exc}")),
    };
    if raw.is_empty() {
        return tool_error(EMPTY_ERROR);
    }
    match serde_json::from_str::<Value>(&raw) {
        Ok(parsed) => parsed.to_string(),
        Err(_) => json!({ "text": raw }).to_string(),
    }
}

// ---------------------------------------------------------------------------
// Registry handler — mirrors `registry.register(..., handler=lambda args, **kw: ...)`
// ---------------------------------------------------------------------------

/// Mirrors the registry handler without a callback (no desktop) — always `NOT_AVAILABLE_ERROR`.
///
/// In Python: `lambda args, **kw: drive_preview_tool(..., callback=kw.get("callback"))`
/// when `kw.get("callback")` is `None`, the early return fires before validation.
pub fn handler(args: &Value) -> String {
    // Still parse action to keep 1:1 signature extraction, but short-circuit to not-available
    // to match Python's early return. We ignore all args.
    let _ = args;
    tool_error(NOT_AVAILABLE_ERROR)
}

/// Handler with injected callback — mirrors `handler` with `callback` kwarg.
///
/// Extracts `action`, `ref`, `selector`, `text`, `key`, `submit`, `amount`, `to`, `max` (limit), `full`
/// from `args` (missing/non-string → defaults) and delegates to `drive_preview_tool_with_callback`.
pub fn handler_with_callback<F>(args: &Value, callback: F) -> String
where
    F: FnOnce(Value) -> Result<String, String>,
{
    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let ref_ = args.get("ref").and_then(|v| v.as_str());
    let selector = args.get("selector").and_then(|v| v.as_str());
    let text = args.get("text").and_then(|v| v.as_str());
    let key = args.get("key").and_then(|v| v.as_str());
    let submit = args.get("submit").and_then(|v| v.as_bool());
    let amount = args.get("amount");
    let to = args.get("to").and_then(|v| v.as_str());
    let limit = args.get("max");
    let full = args.get("full").and_then(|v| v.as_bool());
    drive_preview_tool_with_callback(
        action, ref_, selector, text, key, submit, amount, to, limit, full, callback,
    )
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
// Tests — mirrors Python behavior (lines 54-124) and schema (127-213)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ok_callback(expected_payload: Value, raw_response: &str) -> impl FnOnce(Value) -> Result<String, String> {
        let raw = raw_response.to_string();
        move |payload: Value| {
            assert_eq!(payload, expected_payload);
            Ok(raw.clone())
        }
    }

    #[test]
    fn constants_match_python() {
        assert_eq!(ACTIONS, &["elements", "click", "hover", "type", "scroll", "press", "strobe", "back", "forward", "reload"]);
        assert_eq!(SCROLL_TO, &["top", "bottom"]);
        assert_eq!(NEEDS_TARGET, &["click", "hover", "type", "press"]);
        assert_eq!(TOOL_NAME, "drive_preview");
        assert_eq!(TOOLSET, "desktop_ui");
        assert_eq!(EMOJI, "🖱️");
        assert!(REQUIRES_ENV.is_empty());
        assert_eq!(NOT_AVAILABLE_ERROR, "drive_preview is only available in the Hermes desktop app.");
        assert_eq!(TYPE_NEEDS_TEXT_ERROR, "type needs the text to enter.");
        assert_eq!(PRESS_NEEDS_KEY_ERROR, "press needs a key, e.g. 'Enter' or 'Escape'.");
        assert_eq!(AMOUNT_ERROR, "amount and max must be integers.");
        assert_eq!(FAILED_PREFIX, "Failed to act on the in-app browser: ");
        assert_eq!(EMPTY_ERROR, "The action timed out, or no GUI window answered. Open a page with open_preview first.");
        assert_eq!(action_error(), "action must be one of: elements, click, hover, type, scroll, press, strobe, back, forward, reload.");
        assert_eq!(to_error(), "to must be one of: top, bottom.");
        assert!(DESCRIPTION.starts_with("Interact with the page open in the in-app browser"));
        assert!(DESCRIPTION.contains("btn-sign-in"));
        assert!(DESCRIPTION.contains("delta"));
        assert!(DESCRIPTION.contains("rebound"));
    }

    #[test]
    fn schema_matches_python() {
        let schema = drive_preview_schema();
        assert_eq!(schema["name"], "drive_preview");
        assert_eq!(schema["description"], DESCRIPTION);
        assert_eq!(schema["parameters"]["type"], "object");
        assert_eq!(schema["parameters"]["properties"]["action"]["type"], "string");
        let enum_vals = schema["parameters"]["properties"]["action"]["enum"].as_array().unwrap();
        let expected: Vec<Value> = ACTIONS.iter().map(|a| json!(*a)).collect();
        assert_eq!(enum_vals, &expected);
        assert_eq!(schema["parameters"]["properties"]["action"]["description"], ACTION_DESCRIPTION);
        assert_eq!(schema["parameters"]["properties"]["ref"]["type"], "string");
        assert_eq!(schema["parameters"]["properties"]["ref"]["description"], REF_DESCRIPTION);
        assert_eq!(schema["parameters"]["properties"]["selector"]["description"], SELECTOR_DESCRIPTION);
        assert_eq!(schema["parameters"]["properties"]["text"]["description"], TEXT_DESCRIPTION);
        assert_eq!(schema["parameters"]["properties"]["submit"]["type"], "boolean");
        assert_eq!(schema["parameters"]["properties"]["key"]["description"], KEY_DESCRIPTION);
        assert_eq!(schema["parameters"]["properties"]["amount"]["type"], "integer");
        assert_eq!(schema["parameters"]["properties"]["amount"]["description"], AMOUNT_DESCRIPTION);
        assert_eq!(schema["parameters"]["properties"]["to"]["enum"], json!(SCROLL_TO));
        assert_eq!(schema["parameters"]["properties"]["to"]["description"], TO_DESCRIPTION);
        assert_eq!(schema["parameters"]["properties"]["max"]["type"], "integer");
        assert_eq!(schema["parameters"]["properties"]["max"]["description"], MAX_DESCRIPTION);
        assert_eq!(schema["parameters"]["properties"]["full"]["type"], "boolean");
        let required = schema["parameters"]["required"].as_array().unwrap();
        assert_eq!(required, &vec![json!("action")]);
        let s = drive_preview_schema_json();
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, schema);
    }

    #[test]
    fn not_available_without_callback() {
        let out = drive_preview_tool("elements", None, None, None, None, None, None, None, None, None);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);

        // handler without callback also not available, even with valid action
        let out = handler(&json!({"action": "elements"}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);

        // optional None also
        let out = drive_preview_tool_with_optional_callback::<fn(Value) -> Result<String, String>>(
            "elements", None, None, None, None, None, None, None, None, None, None,
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);

        let out = handler_with_optional_callback::<fn(Value) -> Result<String, String>>(
            &json!({"action": "click", "ref": "btn-x"}),
            None,
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);
    }

    #[test]
    fn invalid_action_returns_error() {
        for action in ["", "   ", "unknown", "clicky", "ELEMENTS "] {
            // Note "ELEMENTS " with trailing space would be valid after trim/lower -> "elements" -> valid
            // So test without that
            if action.trim().to_lowercase() == "elements" {
                continue;
            }
            let out = drive_preview_tool_with_callback(
                action, None, None, None, None, None, None, None, None, None,
                |_| Ok(r#"{"ok": true}"#.to_string()),
            );
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["error"], action_error(), "action={action:?}");
        }
        // handler variant
        let out = handler_with_callback(&json!({"action": "bad"}), |_| Ok("{}".to_string()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], action_error());
    }

    #[test]
    fn valid_actions_normalize_case_and_trim() {
        for action in ["elements", "  ELEMENTS  ", "Click", "HOVER", " TyPe "] {
            let trimmed_lower = action.trim().to_lowercase();
            // type and click etc need target, so provide ref for those to avoid needs_target error
            let needs = NEEDS_TARGET.contains(&trimmed_lower.as_str());
            let ref_opt = if needs { Some("btn-x") } else { None };
            let text_opt = if trimmed_lower == "type" { Some("hello") } else { None };
            let key_opt = if trimmed_lower == "press" { Some("Enter") } else { None };
            let expected_payload = {
                let mut m = serde_json::Map::new();
                m.insert("action".to_string(), json!(trimmed_lower));
                if let Some(r) = ref_opt {
                    m.insert("ref".to_string(), json!(r));
                }
                if let Some(t) = text_opt {
                    m.insert("text".to_string(), json!(t));
                }
                if let Some(k) = key_opt {
                    m.insert("key".to_string(), json!(k));
                }
                Value::Object(m)
            };
            let out = drive_preview_tool_with_callback(
                action, ref_opt, None, text_opt, key_opt, None, None, None, None, None,
                ok_callback(expected_payload, r#"{"url":"https://example.com"}"#),
            );
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["url"], "https://example.com", "action={action:?}");
        }
        // press with missing key should error before callback
        let out = drive_preview_tool_with_callback(
            "press", Some("btn-x"), None, None, None, None, None, None, None, None,
            |_| panic!("should not call callback on missing key"),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], PRESS_NEEDS_KEY_ERROR);
    }

    #[test]
    fn needs_target_validation() {
        for verb in NEEDS_TARGET {
            // no ref, no selector -> error
            let out = drive_preview_tool_with_callback(
                verb, None, None, Some("hello"), Some("Enter"), None, None, None, None, None,
                |_| panic!("should not call callback when target missing"),
            );
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["error"], format!("{verb}{NEEDS_TARGET_SUFFIX}"), "verb={verb}");

            // empty strings also count as missing
            let out = drive_preview_tool_with_callback(
                verb, Some(""), Some(""), Some("hello"), Some("Enter"), None, None, None, None, None,
                |_| panic!("should not call"),
            );
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["error"], format!("{verb}{NEEDS_TARGET_SUFFIX}"));

            // ref present -> ok (for type need text etc)
            let text = if *verb == "type" { Some("hi") } else { None };
            let key = if *verb == "press" { Some("Enter") } else { None };
            let out = drive_preview_tool_with_callback(
                verb, Some("btn-sign-in"), None, text, key, None, None, None, None, None,
                |payload| {
                    assert_eq!(payload["ref"], "btn-sign-in");
                    Ok(r#"{"ok":true}"#.to_string())
                },
            );
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["ok"], true);

            // selector fallback -> ok
            let out = drive_preview_tool_with_callback(
                verb, None, Some("#email"), text, key, None, None, None, None, None,
                |payload| {
                    assert_eq!(payload["selector"], "#email");
                    Ok(r#"{"ok":true}"#.to_string())
                },
            );
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["ok"], true);
        }

        // scroll does NOT need target — bare scroll is allowed (page scroll)
        let out = drive_preview_tool_with_callback(
            "scroll", None, None, None, None, None, None, None, None, None,
            |payload| {
                assert_eq!(payload["action"], "scroll");
                assert!(payload.get("ref").is_none());
                Ok(r#"{"scrolled":true}"#.to_string())
            },
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["scrolled"], true);
    }

    #[test]
    fn type_needs_text() {
        let out = drive_preview_tool_with_callback(
            "type", Some("inp-email"), None, None, None, None, None, None, None, None,
            |_| panic!("should not call when text missing"),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], TYPE_NEEDS_TEXT_ERROR);

        // Some("") is allowed (text is not None) -> should call callback with text=""
        let out = drive_preview_tool_with_callback(
            "type", Some("inp-email"), None, Some(""), None, None, None, None, None, None,
            |payload| {
                assert_eq!(payload["text"], "");
                Ok(r#"{"typed":""}"#.to_string())
            },
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["typed"], "");

        // valid text
        let out = drive_preview_tool_with_callback(
            "type", Some("inp-email"), None, Some("hello@example.com"), None, Some(true), None, None, None, None,
            |payload| {
                assert_eq!(payload["text"], "hello@example.com");
                assert_eq!(payload["submit"], true);
                Ok(r#"{"ok":true}"#.to_string())
            },
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn press_needs_key() {
        for key in [None, Some(""), Some("   ")] {
            // empty string check: we treat "   " as non-empty? Python `not key` where key="   " is truthy (non-empty), so it would NOT error.
            // Our Rust checks !k.is_empty(), so "   " would be considered has_key=true -> no error.
            // To match Python, "   " should be considered present (truthy). We error only on None or "".
            // So skip "   " from expected error.
            let is_empty = key.map(|k| k.is_empty()).unwrap_or(true);
            if key == Some("   ") {
                continue;
            }
            let out = drive_preview_tool_with_callback(
                "press", Some("btn-x"), None, None, key, None, None, None, None, None,
                |_| panic!("should not call when key missing"),
            );
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["error"], PRESS_NEEDS_KEY_ERROR, "key={key:?} empty={is_empty}");
        }
        // valid key
        let out = drive_preview_tool_with_callback(
            "press", Some("btn-x"), None, None, Some("Enter"), None, None, None, None, None,
            |payload| {
                assert_eq!(payload["key"], "Enter");
                Ok(r#"{"pressed":true}"#.to_string())
            },
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["pressed"], true);

        // key with spaces "   " is truthy in Python, should be sent as-is
        let out = drive_preview_tool_with_callback(
            "press", Some("btn-x"), None, None, Some("   "), None, None, None, None, None,
            |payload| {
                assert_eq!(payload["key"], "   ");
                Ok(r#"{"ok":true}"#.to_string())
            },
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn to_validation() {
        let out = drive_preview_tool_with_callback(
            "scroll", None, None, None, None, None, None, Some("middle"), None, None,
            |_| panic!("should not call on invalid to"),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], to_error());

        for valid in SCROLL_TO {
            let out = drive_preview_tool_with_callback(
                "scroll", None, None, None, None, None, None, Some(valid), None, None,
                |payload| {
                    assert_eq!(payload["to"], *valid);
                    Ok(r#"{"ok":true}"#.to_string())
                },
            );
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["ok"], true);
        }

        // None is allowed (no to field)
        let out = drive_preview_tool_with_callback(
            "scroll", None, None, None, None, None, None, None, None, None,
            |payload| {
                assert!(payload.get("to").is_none());
                Ok(r#"{"ok":true}"#.to_string())
            },
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn amount_and_max_must_be_integers() {
        // string non-numeric
        let out = drive_preview_tool_with_callback(
            "scroll", None, None, None, None, None, Some(&json!("abc")), None, None, None,
            |_| panic!("should not call on bad amount"),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], AMOUNT_ERROR);

        let out = drive_preview_tool_with_callback(
            "elements", None, None, None, None, None, None, None, Some(&json!("xyz")), None,
            |_| panic!("should not call"),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], AMOUNT_ERROR);

        // float string "5.0" -> parse fails (strict)
        let out = drive_preview_tool_with_callback(
            "scroll", None, None, None, None, None, Some(&json!("5.0")), None, None, None,
            |_| panic!("should not call"),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], AMOUNT_ERROR);

        // valid ints as numbers
        let out = drive_preview_tool_with_callback(
            "scroll", None, None, None, None, None, Some(&json!(100)), None, None, None,
            |payload| {
                assert_eq!(payload["amount"], 100);
                Ok(r#"{"ok":true}"#.to_string())
            },
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);

        // valid ints as string numeric
        let out = drive_preview_tool_with_callback(
            "scroll", None, None, None, None, None, Some(&json!("42")), None, None, None,
            |payload| {
                assert_eq!(payload["amount"], 42);
                Ok(r#"{"ok":true}"#.to_string())
            },
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);

        // negative
        let out = drive_preview_tool_with_callback(
            "scroll", None, None, None, None, None, Some(&json!(-50)), None, None, None,
            |payload| {
                assert_eq!(payload["amount"], -50);
                Ok(r#"{"ok":true}"#.to_string())
            },
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);

        // limit maps to "max"
        let out = drive_preview_tool_with_callback(
            "elements", None, None, None, None, None, None, None, Some(&json!(10)), None,
            |payload| {
                assert_eq!(payload["max"], 10);
                assert!(payload.get("amount").is_none());
                Ok(r#"{"ok":true}"#.to_string())
            },
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);

        // both present
        let out = drive_preview_tool_with_callback(
            "scroll", Some("pane"), None, None, None, None, Some(&json!(20)), None, Some(&json!(5)), None,
            |payload| {
                assert_eq!(payload["amount"], 20);
                assert_eq!(payload["max"], 5);
                Ok(r#"{"ok":true}"#.to_string())
            },
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn payload_filters_none_and_maps_keys() {
        // full coverage of payload keys
        let out = drive_preview_tool_with_callback(
            "type",
            Some("inp-email"),
            Some("#email"),
            Some("hello"),
            None,
            Some(true),
            None,
            None,
            None,
            Some(true),
            |payload| {
                assert_eq!(payload["action"], "type");
                assert_eq!(payload["ref"], "inp-email");
                assert_eq!(payload["selector"], "#email");
                assert_eq!(payload["text"], "hello");
                assert_eq!(payload["submit"], true);
                assert_eq!(payload["full"], true);
                assert!(payload.get("amount").is_none());
                assert!(payload.get("max").is_none());
                assert!(payload.get("to").is_none());
                Ok(r#"{"url":"https://example.com","title":"Example"}"#.to_string())
            },
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["url"], "https://example.com");
    }

    #[test]
    fn callback_exception_and_empty() {
        let out = drive_preview_tool_with_callback(
            "elements", None, None, None, None, None, None, None, None, None,
            |_| Err("boom".to_string()),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], "Failed to act on the in-app browser: boom");

        let out = drive_preview_tool_with_callback(
            "elements", None, None, None, None, None, None, None, None, None,
            |_| Ok(String::new()),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], EMPTY_ERROR);

        // handler variant also
        let out = handler_with_callback(&json!({"action": "elements"}), |_| Err("transport closed".to_string()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["error"].as_str().unwrap().contains("Failed to act on the in-app browser"));

        let out = handler_with_callback(&json!({"action": "elements"}), |_| Ok("".to_string()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], EMPTY_ERROR);
    }

    #[test]
    fn json_pass_through_and_wrap() {
        // valid JSON passes through normalized
        let raw = r#"{"url":"https://example.com","title":"Example","elements":[{"ref":"btn-x"}]}"#;
        let out = drive_preview_tool_with_callback(
            "elements", None, None, None, None, None, None, None, None, None,
            |_| Ok(raw.to_string()),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        let expected: Value = serde_json::from_str(raw).unwrap();
        assert_eq!(v, expected);
        // ensure unicode preserved (ensure_ascii=False)
        let raw_unicode = r#"{"text":"café 🖱️"}"#;
        let out = drive_preview_tool_with_callback(
            "elements", None, None, None, None, None, None, None, None, None,
            |_| Ok(raw_unicode.to_string()),
        );
        assert!(out.contains("café"));
        assert!(out.contains("🖱️"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["text"], "café 🖱️");

        // invalid JSON wrapped as {"text": raw}
        let raw_plain = "not json at all";
        let out = drive_preview_tool_with_callback(
            "elements", None, None, None, None, None, None, None, None, None,
            |_| Ok(raw_plain.to_string()),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["text"], raw_plain);

        let raw_plain2 = "plain text with café 🖱️";
        let out = drive_preview_tool_with_callback(
            "elements", None, None, None, None, None, None, None, None, None,
            |_| Ok(raw_plain2.to_string()),
        );
        assert!(out.contains("café"));
        assert!(out.contains("🖱️"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["text"], raw_plain2);
    }

    #[test]
    fn handler_extracts_like_python_lambda() {
        // handler_with_callback should extract fields correctly, including max -> limit
        let args = json!({
            "action": "type",
            "ref": "inp-email",
            "selector": "#email",
            "text": "hello",
            "submit": true,
            "key": "Enter",
            "amount": 100,
            "to": "top",
            "max": 5,
            "full": true
        });
        let out = handler_with_callback(&args, |payload| {
            assert_eq!(payload["action"], "type");
            assert_eq!(payload["ref"], "inp-email");
            assert_eq!(payload["selector"], "#email");
            assert_eq!(payload["text"], "hello");
            assert_eq!(payload["submit"], true);
            assert_eq!(payload["key"], "Enter");
            assert_eq!(payload["amount"], 100);
            assert_eq!(payload["to"], "top");
            assert_eq!(payload["max"], 5);
            assert_eq!(payload["full"], true);
            Ok(r#"{"ok":true}"#.to_string())
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);

        // missing action -> error
        let out = handler_with_callback(&json!({}), |_| Ok("{}".to_string()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], action_error());

        // amount as string "100" should parse
        let out = handler_with_callback(&json!({"action": "scroll", "amount": "100"}), |payload| {
            assert_eq!(payload["amount"], 100);
            Ok(r#"{"ok":true}"#.to_string())
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);

        // amount invalid string -> error
        let out = handler_with_callback(&json!({"action": "scroll", "amount": "bad"}), |_| panic!("should not call"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], AMOUNT_ERROR);
    }

    #[test]
    fn typed_ints_wrapper() {
        let out = drive_preview_tool_with_callback_ints(
            "scroll", None, None, None, None, None, Some(120), Some("bottom"), None, None,
            |payload| {
                assert_eq!(payload["amount"], 120);
                assert_eq!(payload["to"], "bottom");
                Ok(r#"{"scrolled":true}"#.to_string())
            },
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["scrolled"], true);
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
        // Even with invalid action, bare handler returns not_available (early return before validation in python)
        let out = handler(&json!({"action": "bad"}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);

        // Same for empty
        let out = handler(&json!({}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);
    }

    #[test]
    fn strobe_and_navigation_actions() {
        for action in ["strobe", "back", "forward", "reload", "elements"] {
            let out = drive_preview_tool_with_callback(
                action, None, None, None, None, None, None, None, None, None,
                |payload| {
                    assert_eq!(payload["action"], action);
                    Ok(r#"{"ok":true}"#.to_string())
                },
            );
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["ok"], true, "action={action}");
        }
    }
}
