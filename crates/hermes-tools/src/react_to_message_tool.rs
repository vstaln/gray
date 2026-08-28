//! Let the agent react to a message with an emoji in the Hermes desktop app.
//! Port of `tools/react_to_message_tool.py` (193 lines) — 1:1 behavior.
//!
//! The conversational counterpart to the user's tapback: the same reaction store,
//! the same one-per-author semantics, just written with `author="agent"`.
//!
//! Lives in the `desktop_ui` toolset (like the other GUI affordances) so it costs
//! nothing on every other surface — the platform adapters already expose reactions
//! through `send_message(action="react")`, and this is the desktop's equivalent.
//!
//! Defaults to the message that triggered this turn (the photon precedent: the
//! model shouldn't have to thread row ids through tool calls), and emits
//! `message.reaction` so the renderer paints it without waiting for a resume.

use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Registry metadata — mirrors `registry.register(...)` kwargs in Python
// ---------------------------------------------------------------------------

/// Tool name as registered in `tools.registry`.
pub const TOOL_NAME: &str = "react_to_message";
/// Toolset that gates this tool (`toolset="desktop_ui"`).
pub const TOOLSET: &str = "desktop_ui";
/// Emoji for tool listing — mirrors `emoji="💛"` in Python.
pub const EMOJI: &str = "💛";
/// `requires_env` for this tool — none (desktop_ui is session-gated, not env-gated).
pub const REQUIRES_ENV: &[&str] = &[];

// ---------------------------------------------------------------------------
// Constants — mirrors Python module-level strings and inline literals
// ---------------------------------------------------------------------------

/// Mirrors `tool_error("No active session — reactions need a persisted conversation.")`
/// (lines 44, 98).
pub const NO_SESSION_ERROR: &str = "No active session — reactions need a persisted conversation.";

 /// Mirrors `tool_error("Session storage is unavailable.")` (line 102).
pub const NO_STORAGE_ERROR: &str = "Session storage is unavailable.";

 /// Mirrors `tool_error("No user message to react to yet.")` (line 57, back==0).
pub const NO_USER_MESSAGE_ERROR: &str = "No user message to react to yet.";

 /// Mirrors `tool_error(f"No user message found {back} back.")` (line 57, back>0).
pub fn no_user_message_back_error(back: i64) -> String {
    format!("No user message found {back} back.")
}

/// Mirrors `tool_error(f"Failed to set the reaction: {exc}")` (line 68).
pub const FAILED_PREFIX: &str = "Failed to set the reaction: ";

/// Mirrors `tool_error(f"Message {row_id} is not part of this conversation.")` (line 71).
pub fn not_part_error(row_id: i64) -> String {
    format!("Message {row_id} is not part of this conversation.")
}

/// Full tool description — mirrors `REACT_TO_MESSAGE_SCHEMA["description"]` (lines 138-149).
pub const DESCRIPTION: &str = "React to a message with a single emoji, the way you'd tapback in iMessage. Reach for it when a reaction is what a person would do: something funny gets a 😂, warmth gets a ❤️, a plan you're on board with gets a 👍 — then just carry on with whatever the message actually needs. If a reaction says it all, it can BE the reply (skip the redundant 'sounds good!' turn). Use it like a person would: occasionally, when felt — not on every message, and never as a status signal. NEVER narrate or explain a reaction ('I reacted with...', 'Reacting now') — the emoji appearing on the bubble is the whole point, and commentary kills it. Defaults to the user's most recent message. One reaction per message: a different emoji replaces yours, an empty string retracts it.";

 /// Description for the `emoji` parameter.
pub const EMOJI_DESCRIPTION: &str = "The emoji to react with (e.g. '❤️', '😂', '👍'). Pass an empty string to remove your reaction.";

 /// Description for the `message_row_id` parameter.
pub const MESSAGE_ROW_ID_DESCRIPTION: &str = "Optional. The specific message to react to. Omit to react to the user's latest message, which is almost always what you want.";

 /// Description for the `messages_back` parameter.
pub const MESSAGES_BACK_DESCRIPTION: &str = "Optional. React to an EARLIER user message: 1 = the one before the latest, 2 = two before, and so on. For when something lands late — the joke you only got after answering.";

// ---------------------------------------------------------------------------
// Schema — mirrors `REACT_TO_MESSAGE_SCHEMA` dict in Python (lines 136-179)
// ---------------------------------------------------------------------------

/// Returns the JSON schema for `react_to_message` — mirrors `REACT_TO_MESSAGE_SCHEMA`.
///
/// In Python this is a dict literal; here we return a `serde_json::Value`
/// so callers can serialize or inspect it without owning a static JSON string.
pub fn react_to_message_schema() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": DESCRIPTION,
        "parameters": {
            "type": "object",
            "properties": {
                "emoji": {
                    "type": "string",
                    "description": EMOJI_DESCRIPTION
                },
                "message_row_id": {
                    "type": "integer",
                    "description": MESSAGE_ROW_ID_DESCRIPTION
                },
                "messages_back": {
                    "type": "integer",
                    "description": MESSAGES_BACK_DESCRIPTION
                }
            },
            "required": ["emoji"]
        }
    })
}

/// Serialized schema string — mirrors `REACT_TO_MESSAGE_SCHEMA` as JSON.
pub fn react_to_message_schema_json() -> String {
    react_to_message_schema().to_string()
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
// Session env — mirrors `gateway.session_context.get_session_env`
// ---------------------------------------------------------------------------

thread_local! {
    static SESSION_VARS: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

/// Set a session env var for the current thread (test helper).
///
/// Mirrors `gateway.session_context.set_session_vars` for the keys this
/// tool reads: `HERMES_SESSION_KEY`, `HERMES_SESSION_ID`.
pub fn set_session_env(key: &str, value: &str) {
    SESSION_VARS.with(|m| {
        m.borrow_mut().insert(key.to_string(), value.to_string());
    });
}

/// Clear all thread-local session vars.
pub fn clear_session_env() {
    SESSION_VARS.with(|m| m.borrow_mut().clear());
}

/// Mirrors `gateway.session_context.get_session_env(key, default)`.
///
/// Thread-local vars take precedence, then `std::env::var`, then `default`.
pub fn get_session_env(key: &str, default: &str) -> String {
    SESSION_VARS.with(|m| {
        if let Some(v) = m.borrow().get(key) {
            return v.clone();
        }
        std::env::var(key).unwrap_or_else(|_| default.to_string())
    })
}

// ---------------------------------------------------------------------------
// SessionDB trait — mirrors `hermes_state.SessionDB` surface used here
// ---------------------------------------------------------------------------

/// Minimal `SessionDB` surface used by `react_to_message`.
///
/// Mirrors the three methods the Python tool calls:
/// - `latest_message_row_id(session_key, role="user", offset=back)`
/// - `get_message_role(session_key, row_id)`
/// - `set_message_reaction(session_key, row_id, emoji, author="agent")`
/// - `close()`
pub trait SessionDb {
    /// Mirrors `db.latest_message_row_id(session_key, role="user", offset=back)`.
    fn latest_message_row_id(&self, session_key: &str, role: &str, offset: i64) -> Option<i64>;

    /// Mirrors `db.get_message_role(session_key, int(row_id))`.
    fn get_message_role(&self, session_key: &str, row_id: i64) -> Option<String>;

    /// Mirrors `db.set_message_reaction(session_key, int(row_id), emoji or None, author="agent")`.
    ///
    /// `emoji` is `None` for retraction (empty string). Returns `None` when
    /// the row is not part of the conversation. `Err` mirrors an exception.
    fn set_message_reaction(
        &mut self,
        session_key: &str,
        row_id: i64,
        emoji: Option<&str>,
        author: &str,
    ) -> Result<Option<Value>, String>;

    /// Mirrors `db.close()` — may be a no-op for in-memory fakes.
    fn close(&mut self) -> Result<(), String> {
        Ok(())
    }
}

/// Mirrors `_open_session_db()` (lines 24-31).
///
/// Tries to open the `SessionDB` for the profile owning this turn, or `None`
/// on failure. This stub returns `None` until the gateway wires a real DB;
/// tests inject a `SessionDb` via `react_to_message_with_db`.
pub fn open_session_db() -> Option<Box<dyn SessionDb>> {
    None
}

// ---------------------------------------------------------------------------
// desktop_ui bridge — mirrors `tools/desktop_ui.py` (1:1 semantics)
// ---------------------------------------------------------------------------

/// Mirrors `desktop_ui.available()` — true when a renderer emitter is wired.
pub fn desktop_ui_available() -> bool {
    false
}

/// Mirrors `desktop_ui.emit(event, payload) -> bool`.
///
/// Python signature: `emit(event: str, payload: dict) -> bool`
///   - Returns `False` when no emitter is wired (not desktop app).
///   - May raise — caller ignores exceptions (paint-live is best-effort).
///
/// This stub returns `Ok(false)` (no desktop) so the default path still
/// succeeds — persistence is the contract, live paint is best-effort (lines 73-83).
pub fn desktop_ui_emit(_event: &str, _payload: Value) -> Result<bool, String> {
    Ok(false)
}

// ---------------------------------------------------------------------------
// Helpers — int parsing mirrors Python `int(...)` for row_id / messages_back
// ---------------------------------------------------------------------------

fn parse_i64_value(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i)
            } else if let Some(u) = n.as_u64() {
                if u <= i64::MAX as u64 { Some(u as i64) } else { None }
            } else if let Some(f) = n.as_f64() {
                if f.is_finite() { Some(f.trunc() as i64) } else { None }
            } else {
                None
            }
        }
        Value::String(s) => s.trim().parse::<i64>().ok(),
        Value::Bool(b) => Some(if *b { 1 } else { 0 }),
        _ => None,
    }
}

fn parse_messages_back(v: Option<&Value>) -> i64 {
    match v {
        None => 0,
        Some(Value::Null) => 0,
        Some(val) => parse_i64_value(val).unwrap_or(0).max(0),
    }
}

fn parse_row_id(v: &Value) -> Option<i64> {
    // Value::Null is handled by caller as None; here we parse actual values.
    parse_i64_value(v)
}

// ---------------------------------------------------------------------------
// check_react_requirements — mirrors `check_react_requirements()` (lines 119-133)
// ---------------------------------------------------------------------------

/// Mirrors `check_react_requirements()` with injected config value.
///
/// `config` mirrors `load_config_readonly()` return (a dict-like `Value`).
/// Returns `true` only when `display` is an object and `message_reactions` is truthy.
pub fn check_react_requirements_with_config(config: Option<&Value>) -> bool {
    let cfg = match config {
        Some(v) => v,
        None => return false,
    };
    let display = match cfg.get("display") {
        Some(v) => v,
        None => return false,
    };
    let obj = match display.as_object() {
        Some(m) => m,
        None => return false,
    };
    match obj.get("message_reactions") {
        Some(Value::Bool(b)) => *b,
        Some(Value::Null) | None => false,
        Some(Value::Number(n)) => {
            if let Some(i) = n.as_i64() { i != 0 } else if let Some(u) = n.as_u64() { u != 0 } else if let Some(f) = n.as_f64() { f != 0.0 } else { false }
        }
        Some(Value::String(s)) => !s.is_empty() && s != "0" && s.to_ascii_lowercase() != "false",
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// Mirrors `check_react_requirements() -> bool` (lines 119-133).
///
/// Reads the `display.message_reactions` flag. In Python this calls
/// `load_config_readonly().get("display")` and handles exceptions by returning `False`.
/// This stub returns `False` (opt-in flag off) until the gateway wires real config;
/// use `check_react_requirements_with_config` in tests or when config is available.
pub fn check_react_requirements() -> bool {
    false
}

// ---------------------------------------------------------------------------
// Core handler — mirrors `_react_to_message_with_db` (lines 34-87)
// ---------------------------------------------------------------------------

/// Attach (or with an empty `emoji` retract) the agent's reaction — testable core.
///
/// Mirrors `def _react_to_message_with_db(emoji, message_row_id, messages_back, *, db, session_key) -> str:`
/// (lines 34-87). `emoji` should already be trimmed (mirrors line 92).
/// `message_row_id` and `messages_back` are `Option<&Value>` where `None`/`Null`
/// means omitted. `emit` mirrors `desktop_ui.emit` and is ignored on error (lines 77-83).
pub fn react_to_message_with_db<F>(
    emoji: &str,
    message_row_id: Option<&Value>,
    messages_back: Option<&Value>,
    db: &mut dyn SessionDb,
    session_key: &str,
    emit: F,
) -> String
where
    F: FnOnce(&str, Value) -> Result<bool, String>,
{
    if session_key.is_empty() {
        return tool_error(NO_SESSION_ERROR);
    }

    // Resolve target row_id and role.
    let mut row_id: i64;
    let mut target_role = "user".to_string();

    match message_row_id {
        None => {
            let back = parse_messages_back(messages_back);
            match db.latest_message_row_id(session_key, "user", back) {
                Some(id) => row_id = id,
                None => {
                    if back == 0 {
                        return tool_error(NO_USER_MESSAGE_ERROR);
                    } else {
                        return tool_error(&no_user_message_back_error(back));
                    }
                }
            }
        }
        Some(Value::Null) => {
            // Treat JSON null as omitted (same as Python `None`).
            let back = parse_messages_back(messages_back);
            match db.latest_message_row_id(session_key, "user", back) {
                Some(id) => row_id = id,
                None => {
                    if back == 0 {
                        return tool_error(NO_USER_MESSAGE_ERROR);
                    } else {
                        return tool_error(&no_user_message_back_error(back));
                    }
                }
            }
        }
        Some(v) => {
            match parse_row_id(v) {
                Some(id) => row_id = id,
                None => {
                    // Python `int(row_id)` would raise; we map to failed-reaction.
                    return tool_error(&format!("{FAILED_PREFIX}invalid message_row_id: {v}"));
                }
            }
            // Mirrors `row = db.get_message_role(session_key, int(row_id)); target_role = row or "user"`
            if let Some(role) = db.get_message_role(session_key, row_id) {
                target_role = role;
            } else {
                target_role = "user".to_string();
            }
        }
    }

    // Attach reaction — mirrors `db.set_message_reaction(..., author="agent")`.
    let emoji_opt = if emoji.is_empty() { None } else { Some(emoji) };
    let reactions = match db.set_message_reaction(session_key, row_id, emoji_opt, "agent") {
        Ok(v) => v,
        Err(exc) => return tool_error(&format!("{FAILED_PREFIX}{exc}")),
    };

    let reactions_val = match reactions {
        Some(v) => v,
        None => return tool_error(&not_part_error(row_id)),
    };

    // Paint it live — missing bridge is not an error (lines 73-83).
    let payload = json!({ "row_id": row_id, "reactions": reactions_val, "role": target_role });
    let _ = emit("message.reaction", payload);

    // Mirrors `json.dumps({"success": True, "row_id": int(row_id), "reactions": reactions}, ensure_ascii=False)`
    json!({ "success": true, "row_id": row_id, "reactions": reactions_val }).to_string()
}

/// Variant that uses the default `desktop_ui_emit` bridge.
pub fn react_to_message_with_db_default_emit(
    emoji: &str,
    message_row_id: Option<&Value>,
    messages_back: Option<&Value>,
    db: &mut dyn SessionDb,
    session_key: &str,
) -> String {
    react_to_message_with_db(emoji, message_row_id, messages_back, db, session_key, desktop_ui_emit)
}

// ---------------------------------------------------------------------------
// Public tool — mirrors `react_to_message_tool` (lines 90-116)
// ---------------------------------------------------------------------------

/// Attach (or with an empty `emoji` retract) the agent's reaction.
///
/// Mirrors `def react_to_message_tool(emoji, message_row_id, messages_back) -> str:` (lines 90-116):
/// ```python
/// emoji = (emoji or "").strip()
/// session_key = get_session_env("HERMES_SESSION_KEY", "") or get_session_env("HERMES_SESSION_ID", "")
/// if not session_key: return tool_error("No active session — ...")
/// db = _open_session_db()
/// if db is None: return tool_error("Session storage is unavailable.")
/// try: return _react_to_message_with_db(...)
/// finally: db.close()
/// ```
pub fn react_to_message_tool(
    emoji: &str,
    message_row_id: Option<&Value>,
    messages_back: Option<&Value>,
) -> String {
    react_to_message_tool_with_db_emit(emoji, message_row_id, messages_back, desktop_ui_emit)
}

/// Testable core with injected emit — mirrors `react_to_message_tool` but
/// with a provided `emit` for live-paint.
pub fn react_to_message_tool_with_emit<F>(
    emoji: &str,
    message_row_id: Option<&Value>,
    messages_back: Option<&Value>,
    emit: F,
) -> String
where
    F: FnOnce(&str, Value) -> Result<bool, String>,
{
    let trimmed = emoji.trim();

    // Mirrors `session_key = get_session_env("HERMES_SESSION_KEY", "") or get_session_env("HERMES_SESSION_ID", "")`
    let mut session_key = get_session_env("HERMES_SESSION_KEY", "");
    if session_key.is_empty() {
        session_key = get_session_env("HERMES_SESSION_ID", "");
    }
    if session_key.is_empty() {
        return tool_error(NO_SESSION_ERROR);
    }

    let mut db_opt = open_session_db();
    let db = match db_opt.as_mut() {
        Some(b) => b.as_mut(),
        None => return tool_error(NO_STORAGE_ERROR),
    };

    let result = react_to_message_with_db(trimmed, message_row_id, messages_back, db, &session_key, emit);
    // Mirrors `finally: db.close()` with ignored exceptions.
    let _ = db.close();
    result
}

/// Testable core with injected db and emit — mirrors `react_to_message_tool`
/// but bypasses `open_session_db` and session env lookup.
///
/// `session_key` is injected directly for tests. When `db` is `None`,
/// returns `NO_STORAGE_ERROR`.
pub fn react_to_message_tool_with_db_emit<F>(
    emoji: &str,
    message_row_id: Option<&Value>,
    messages_back: Option<&Value>,
    emit: F,
) -> String
where
    F: FnOnce(&str, Value) -> Result<bool, String>,
{
    // This entry mirrors the public `react_to_message_tool` but with session
    // env lookup (so it can be used as the handler's direct impl without duplication).
    react_to_message_tool_with_emit(emoji, message_row_id, messages_back, emit)
}

/// Direct injection used by handler tests — session_key and db are supplied.
pub fn react_to_message_tool_with_session_db<F>(
    emoji: &str,
    message_row_id: Option<&Value>,
    messages_back: Option<&Value>,
    db: Option<&mut dyn SessionDb>,
    session_key: &str,
    emit: F,
) -> String
where
    F: FnOnce(&str, Value) -> Result<bool, String>,
{
    let trimmed = emoji.trim();
    if session_key.is_empty() {
        return tool_error(NO_SESSION_ERROR);
    }
    let db_mut = match db {
        Some(d) => d,
        None => return tool_error(NO_STORAGE_ERROR),
    };
    let result = react_to_message_with_db(trimmed, message_row_id, messages_back, db_mut, session_key, emit);
    let _ = db_mut.close();
    result
}

// ---------------------------------------------------------------------------
// Registry handler — mirrors `registry.register(..., handler=lambda args, **kw: ...)`
// ---------------------------------------------------------------------------

/// Mirrors the registry handler lambda (lines 186-190):
/// `lambda args, **kw: react_to_message_tool(emoji=args.get("emoji", ""), message_row_id=args.get("message_row_id"), messages_back=args.get("messages_back"))`
pub fn handler(args: &Value) -> String {
    let emoji = args.get("emoji").and_then(|v| v.as_str()).unwrap_or("");
    // Treat JSON null as absent (Python None).
    let row_id_opt = match args.get("message_row_id") {
        Some(Value::Null) | None => None,
        Some(v) => Some(v),
    };
    let back_opt = match args.get("messages_back") {
        Some(Value::Null) | None => None,
        Some(v) => Some(v),
    };
    react_to_message_tool(emoji, row_id_opt, back_opt)
}

/// Handler with injected db+emit for tests — mirrors `handler` but uses provided deps.
pub fn handler_with_session_db<F>(
    args: &Value,
    db: Option<&mut dyn SessionDb>,
    session_key: &str,
    emit: F,
) -> String
where
    F: FnOnce(&str, Value) -> Result<bool, String>,
{
    let emoji = args.get("emoji").and_then(|v| v.as_str()).unwrap_or("");
    let row_id_opt = match args.get("message_row_id") {
        Some(Value::Null) | None => None,
        Some(v) => Some(v),
    };
    let back_opt = match args.get("messages_back") {
        Some(Value::Null) | None => None,
        Some(v) => Some(v),
    };
    react_to_message_tool_with_session_db(emoji, row_id_opt, back_opt, db, session_key, emit)
}

/// Handler with injected emit (uses live session env + open_session_db stub).
pub fn handler_with_emit<F>(args: &Value, emit: F) -> String
where
    F: FnOnce(&str, Value) -> Result<bool, String>,
{
    let emoji = args.get("emoji").and_then(|v| v.as_str()).unwrap_or("");
    let row_id_opt = match args.get("message_row_id") {
        Some(Value::Null) | None => None,
        Some(v) => Some(v),
    };
    let back_opt = match args.get("messages_back") {
        Some(Value::Null) | None => None,
        Some(v) => Some(v),
    };
    react_to_message_tool_with_emit(emoji, row_id_opt, back_opt, emit)
}

// ---------------------------------------------------------------------------
// Tests — mirrors Python behavior (193 lines) and schema
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Fake DB for tests.

    struct FakeDb {
        latest: Option<i64>,
        latest_calls: Vec<(String, String, i64)>,
        role_map: HashMap<i64, String>,
        role_calls: Vec<(String, i64)>,
        reaction_result: Result<Option<Value>, String>,
        reaction_calls: Vec<(String, i64, Option<String>, String)>,
        close_called: bool,
        close_should_error: bool,
    }

    impl FakeDb {
        fn new() -> Self {
            Self {
                latest: None,
                latest_calls: Vec::new(),
                role_map: HashMap::new(),
                role_calls: Vec::new(),
                reaction_result: Ok(Some(json!({}))),
                reaction_calls: Vec::new(),
                close_called: false,
                close_should_error: false,
            }
        }

        fn with_latest(mut self, id: Option<i64>) -> Self {
            self.latest = id;
            self
        }

        fn with_role(mut self, row_id: i64, role: &str) -> Self {
            self.role_map.insert(row_id, role.to_string());
            self
        }

        fn with_reaction_ok(mut self, reactions: Value) -> Self {
            self.reaction_result = Ok(Some(reactions));
            self
        }

        fn with_reaction_none(mut self) -> Self {
            self.reaction_result = Ok(None);
            self
        }

        fn with_reaction_err(mut self, msg: &str) -> Self {
            self.reaction_result = Err(msg.to_string());
            self
        }
    }

    impl SessionDb for FakeDb {
        fn latest_message_row_id(&self, session_key: &str, role: &str, offset: i64) -> Option<i64> {
            // interior mutability via unsafe for call tracking: use RefCell for calls would need &mut.
            // We use a trick: cast self to &mut via raw pointer for tracking (test-only).
            let self_mut = unsafe { &mut *(self as *const Self as *mut Self) };
            self_mut.latest_calls.push((session_key.to_string(), role.to_string(), offset));
            self.latest
        }

        fn get_message_role(&self, session_key: &str, row_id: i64) -> Option<String> {
            let self_mut = unsafe { &mut *(self as *const Self as *mut Self) };
            self_mut.role_calls.push((session_key.to_string(), row_id));
            self.role_map.get(&row_id).cloned()
        }

        fn set_message_reaction(
            &mut self,
            session_key: &str,
            row_id: i64,
            emoji: Option<&str>,
            author: &str,
        ) -> Result<Option<Value>, String> {
            self.reaction_calls.push((
                session_key.to_string(),
                row_id,
                emoji.map(|s| s.to_string()),
                author.to_string(),
            ));
            match &self.reaction_result {
                Ok(Some(v)) => Ok(Some(v.clone())),
                Ok(None) => Ok(None),
                Err(e) => Err(e.clone()),
            }
        }

        fn close(&mut self) -> Result<(), String> {
            self.close_called = true;
            if self.close_should_error {
                Err("close failed".to_string())
            } else {
                Ok(())
            }
        }
    }

    // Helper emit that records payload.

    fn ok_emit(expected_event: &str, expected_payload: Option<Value>) -> impl FnOnce(&str, Value) -> Result<bool, String> {
        move |event, payload| {
            assert_eq!(event, expected_event);
            if let Some(exp) = expected_payload {
                assert_eq!(payload, exp);
            }
            Ok(true)
        }
    }

    #[test]
    fn constants_match_python_registry_args() {
        assert_eq!(TOOL_NAME, "react_to_message");
        assert_eq!(TOOLSET, "desktop_ui");
        assert_eq!(EMOJI, "💛");
        assert!(REQUIRES_ENV.is_empty());
        assert_eq!(NO_SESSION_ERROR, "No active session — reactions need a persisted conversation.");
        assert_eq!(NO_STORAGE_ERROR, "Session storage is unavailable.");
        assert_eq!(NO_USER_MESSAGE_ERROR, "No user message to react to yet.");
        assert_eq!(no_user_message_back_error(1), "No user message found 1 back.");
        assert_eq!(no_user_message_back_error(2), "No user message found 2 back.");
        assert_eq!(FAILED_PREFIX, "Failed to set the reaction: ");
        assert_eq!(not_part_error(42), "Message 42 is not part of this conversation.");
        assert!(DESCRIPTION.starts_with("React to a message with a single emoji"));
        assert!(DESCRIPTION.contains("tapback in iMessage"));
        assert!(DESCRIPTION.contains("😂"));
        assert!(DESCRIPTION.contains("❤️"));
        assert!(DESCRIPTION.contains("👍"));
        assert!(DESCRIPTION.contains("NEVER narrate"));
        assert!(DESCRIPTION.contains("One reaction per message"));
        assert_eq!(EMOJI_DESCRIPTION, "The emoji to react with (e.g. '❤️', '😂', '👍'). Pass an empty string to remove your reaction.");
        assert_eq!(MESSAGE_ROW_ID_DESCRIPTION, "Optional. The specific message to react to. Omit to react to the user's latest message, which is almost always what you want.");
        assert_eq!(MESSAGES_BACK_DESCRIPTION, "Optional. React to an EARLIER user message: 1 = the one before the latest, 2 = two before, and so on. For when something lands late — the joke you only got after answering.");
    }

    #[test]
    fn schema_matches_python() {
        let schema = react_to_message_schema();
        assert_eq!(schema["name"], "react_to_message");
        assert_eq!(schema["description"], DESCRIPTION);
        assert_eq!(schema["parameters"]["type"], "object");
        assert_eq!(schema["parameters"]["properties"]["emoji"]["type"], "string");
        assert_eq!(schema["parameters"]["properties"]["emoji"]["description"], EMOJI_DESCRIPTION);
        assert_eq!(schema["parameters"]["properties"]["message_row_id"]["type"], "integer");
        assert_eq!(schema["parameters"]["properties"]["message_row_id"]["description"], MESSAGE_ROW_ID_DESCRIPTION);
        assert_eq!(schema["parameters"]["properties"]["messages_back"]["type"], "integer");
        assert_eq!(schema["parameters"]["properties"]["messages_back"]["description"], MESSAGES_BACK_DESCRIPTION);
        let required = schema["parameters"]["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "emoji");
        let s = react_to_message_schema_json();
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, schema);
        assert!(s.contains("react_to_message"));
    }

    #[test]
    fn session_env_thread_local() {
        clear_session_env();
        assert_eq!(get_session_env("HERMES_SESSION_KEY", ""), "");
        assert_eq!(get_session_env("HERMES_SESSION_ID", "def"), "def");
        set_session_env("HERMES_SESSION_KEY", "key123");
        assert_eq!(get_session_env("HERMES_SESSION_KEY", ""), "key123");
        set_session_env("HERMES_SESSION_ID", "id999");
        // HERMES_SESSION_KEY still takes precedence via get_session_env, but fallback logic tests separately
        assert_eq!(get_session_env("HERMES_SESSION_ID", ""), "id999");
        clear_session_env();
        assert_eq!(get_session_env("HERMES_SESSION_KEY", ""), "");
    }

    #[test]
    fn check_react_requirements_logic() {
        // Direct check_react_requirements stub returns false (no config wired)
        assert!(!check_react_requirements());
        // With config injection
        assert!(!check_react_requirements_with_config(None));
        assert!(!check_react_requirements_with_config(Some(&json!({}))));
        assert!(!check_react_requirements_with_config(Some(&json!({"display": null}))));
        assert!(!check_react_requirements_with_config(Some(&json!({"display": "not a dict"}))));
        assert!(!check_react_requirements_with_config(Some(&json!({"display": {}}))));
        assert!(!check_react_requirements_with_config(Some(&json!({"display": {"message_reactions": false}}))));
        assert!(!check_react_requirements_with_config(Some(&json!({"display": {"message_reactions": null}}))));
        assert!(!check_react_requirements_with_config(Some(&json!({"display": {"message_reactions": 0}}))));
        assert!(check_react_requirements_with_config(Some(&json!({"display": {"message_reactions": true}}))));
        assert!(check_react_requirements_with_config(Some(&json!({"display": {"message_reactions": 1}}))));
        assert!(check_react_requirements_with_config(Some(&json!({"display": {"message_reactions": "yes"}}))));
        assert!(check_react_requirements_with_config(Some(&json!({"display": {"message_reactions": {"some": "obj"}}))));
    }

    #[test]
    fn no_session_error_when_key_empty() {
        let mut db = FakeDb::new().with_latest(Some(10)).with_reaction_ok(json!({"agent": "❤️"}));
        let out = react_to_message_tool_with_session_db("❤️", None, None, Some(&mut db), "", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NO_SESSION_ERROR);

        let out = react_to_message_with_db("❤️", None, None, &mut db, "", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NO_SESSION_ERROR);
    }

    #[test]
    fn no_storage_error_when_db_none() {
        let out = react_to_message_tool_with_session_db("❤️", None, None, None, "sess-key", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NO_STORAGE_ERROR);
    }

    #[test]
    fn no_user_message_yet_when_back_zero_and_none() {
        let mut db = FakeDb::new().with_latest(None);
        let out = react_to_message_with_db("❤️", None, None, &mut db, "sess", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NO_USER_MESSAGE_ERROR);
        assert_eq!(db.latest_calls[0].2, 0);

        // messages_back=0 explicit also same
        let mut db2 = FakeDb::new().with_latest(None);
        let out = react_to_message_with_db("❤️", None, Some(&json!(0)), &mut db2, "sess", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NO_USER_MESSAGE_ERROR);
    }

    #[test]
    fn no_user_message_back_error() {
        let mut db = FakeDb::new().with_latest(None);
        let out = react_to_message_with_db("❤️", None, Some(&json!(1)), &mut db, "sess", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], no_user_message_back_error(1));

        let mut db2 = FakeDb::new().with_latest(None);
        let out = react_to_message_with_db("❤️", None, Some(&json!(2)), &mut db2, "sess", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], no_user_message_back_error(2));
        assert_eq!(db2.latest_calls[0].2, 2);
    }

    #[test]
    fn messages_back_negative_clamped_to_zero() {
        let mut db = FakeDb::new().with_latest(Some(7)).with_reaction_ok(json!({"agent":"👍"}));
        let out = react_to_message_with_db("👍", None, Some(&json!(-5)), &mut db, "sess", |_, p| {
            assert_eq!(p["row_id"], 7);
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
        assert_eq!(db.latest_calls[0].2, 0);
    }

    #[test]
    fn success_with_latest_message_no_back() {
        let reactions = json!({"agent": "❤️", "user": "👍"});
        let mut db = FakeDb::new().with_latest(Some(42)).with_reaction_ok(reactions.clone());
        let out = react_to_message_with_db("❤️", None, None, &mut db, "sess", |event, payload| {
            assert_eq!(event, "message.reaction");
            assert_eq!(payload["row_id"], 42);
            assert_eq!(payload["reactions"], reactions);
            assert_eq!(payload["role"], "user");
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
        assert_eq!(v["row_id"], 42);
        assert_eq!(v["reactions"], reactions);
        assert_eq!(db.latest_calls.len(), 1);
        assert_eq!(db.reaction_calls[0].2, Some("❤️".to_string()));
        assert_eq!(db.reaction_calls[0].3, "agent");
    }

    #[test]
    fn success_with_messages_back() {
        let mut db = FakeDb::new().with_latest(Some(10)).with_reaction_ok(json!({"agent": "😂"}));
        let out = react_to_message_with_db("😂", None, Some(&json!(2)), &mut db, "sess", |_, p| {
            assert_eq!(p["row_id"], 10);
            assert_eq!(p["role"], "user");
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["row_id"], 10);
        assert_eq!(db.latest_calls[0].2, 2);
    }

    #[test]
    fn success_with_explicit_row_id_uses_role_lookup() {
        let mut db = FakeDb::new().with_role(99, "assistant").with_reaction_ok(json!({"agent":"👍"}));
        let out = react_to_message_with_db("👍", Some(&json!(99)), None, &mut db, "sess", |_, payload| {
            assert_eq!(payload["row_id"], 99);
            assert_eq!(payload["role"], "assistant");
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
        assert_eq!(v["row_id"], 99);
        assert_eq!(db.role_calls[0].1, 99);
        // latest should not be called
        assert!(db.latest_calls.is_empty());
    }

    #[test]
    fn explicit_row_id_falls_back_to_user_when_role_missing() {
        let mut db = FakeDb::new().with_reaction_ok(json!({"agent":"👍"}));
        let out = react_to_message_with_db("👍", Some(&json!(55)), None, &mut db, "sess", |_, p| {
            assert_eq!(p["role"], "user");
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["row_id"], 55);
    }

    #[test]
    fn empty_emoji_retracts_none() {
        let mut db = FakeDb::new().with_latest(Some(5)).with_reaction_ok(json!({}));
        let out = react_to_message_with_db("", None, None, &mut db, "sess", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
        assert_eq!(db.reaction_calls[0].2, None);

        // whitespace also retracts after trim
        let mut db2 = FakeDb::new().with_latest(Some(5)).with_reaction_ok(json!({}));
        let out = react_to_message_with_db("   ", None, None, &mut db2, "sess", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(db2.reaction_calls[0].2, None);

        // via tool's trim: emoji with spaces around heart
        let mut db3 = FakeDb::new().with_latest(Some(5)).with_reaction_ok(json!({"agent":"❤️"}));
        let out = react_to_message_tool_with_session_db("  ❤️  ", None, None, Some(&mut db3), "sess", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
        assert_eq!(db3.reaction_calls[0].2, Some("❤️".to_string()));
    }

    #[test]
    fn emoji_trim_in_tool_wrapper() {
        let mut db = FakeDb::new().with_latest(Some(10)).with_reaction_ok(json!({"agent":"❤️"}));
        let out = react_to_message_tool_with_session_db("  ❤️  ", None, None, Some(&mut db), "sess", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
        assert_eq!(db.reaction_calls[0].2, Some("❤️".to_string()));
    }

    #[test]
    fn set_reaction_error_propagated() {
        let mut db = FakeDb::new().with_latest(Some(10)).with_reaction_err("db connection lost");
        let out = react_to_message_with_db("❤️", None, None, &mut db, "sess", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], "Failed to set the reaction: db connection lost");
    }

    #[test]
    fn not_part_error_when_reaction_none() {
        let mut db = FakeDb::new().with_latest(Some(999)).with_reaction_none();
        let out = react_to_message_with_db("❤️", None, None, &mut db, "sess", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], not_part_error(999));

        // explicit row id also
        let mut db2 = FakeDb::new().with_reaction_none();
        let out = react_to_message_with_db("❤️", Some(&json!(123)), None, &mut db2, "sess", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], not_part_error(123));
    }

    #[test]
    fn emit_failure_is_ignored_and_still_success() {
        let mut db = FakeDb::new().with_latest(Some(7)).with_reaction_ok(json!({"agent":"👍"}));
        let out = react_to_message_with_db("👍", None, None, &mut db, "sess", |_, _| Err("bridge down".to_string()));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
        assert_eq!(v["row_id"], 7);

        // also when emit returns Ok(false) (no desktop) — still success
        let mut db2 = FakeDb::new().with_latest(Some(7)).with_reaction_ok(json!({"agent":"👍"}));
        let out = react_to_message_with_db("👍", None, None, &mut db2, "sess", |_, _| Ok(false));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
    }

    #[test]
    fn handler_extracts_like_python_lambda() {
        // emoji required
        let args = json!({"emoji": "❤️"});
        let mut db = FakeDb::new().with_latest(Some(42)).with_reaction_ok(json!({"agent":"❤️"}));
        let out = handler_with_session_db(&args, Some(&mut db), "sess", |_, p| {
            assert_eq!(p["row_id"], 42);
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);

        // missing emoji -> "" -> retract (None)
        let args2 = json!({});
        let mut db2 = FakeDb::new().with_latest(Some(10)).with_reaction_ok(json!({}));
        let out = handler_with_session_db(&args2, Some(&mut db2), "sess", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
        assert_eq!(db2.reaction_calls[0].2, None);

        // with row_id and back
        let args3 = json!({"emoji": "👍", "message_row_id": 99, "messages_back": 2});
        let mut db3 = FakeDb::new().with_role(99, "user").with_reaction_ok(json!({"agent":"👍"}));
        let out = handler_with_session_db(&args3, Some(&mut db3), "sess", |_, p| {
            assert_eq!(p["row_id"], 99);
            // when explicit row_id, messages_back is ignored (like python)
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["row_id"], 99);
        // ensure latest not called when row_id explicit
        assert!(db3.latest_calls.is_empty());

        // null values treated as absent
        let args4 = json!({"emoji": "❤️", "message_row_id": null, "messages_back": null});
        let mut db4 = FakeDb::new().with_latest(Some(5)).with_reaction_ok(json!({"agent":"❤️"}));
        let out = handler_with_session_db(&args4, Some(&mut db4), "sess", |_, p| {
            assert_eq!(p["row_id"], 5);
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
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
    fn json_preserves_unicode_ensure_ascii_false() {
        let reactions = json!({"agent": "❤️", "text": "café"});
        let mut db = FakeDb::new().with_latest(Some(1)).with_reaction_ok(reactions.clone());
        let out = react_to_message_with_db("❤️", None, None, &mut db, "sess", |_, _| Ok(true));
        assert!(out.contains('❤️'));
        assert!(out.contains("café"));
        assert!(!out.contains("\\u"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["reactions"], reactions);

        // tool_error preserves unicode
        let out = tool_error("café 💛 error");
        assert!(out.contains("café"));
        assert!(out.contains('💛'));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], "café 💛 error");
    }

    #[test]
    fn close_is_called_even_on_error() {
        let mut db = FakeDb::new().with_latest(None);
        let out = react_to_message_tool_with_session_db("❤️", None, None, Some(&mut db), "sess", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v.get("error").is_some());
        assert!(db.close_called);

        // also on success
        let mut db2 = FakeDb::new().with_latest(Some(1)).with_reaction_ok(json!({"agent":"❤️"}));
        let out = react_to_message_tool_with_session_db("❤️", None, None, Some(&mut db2), "sess", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
        assert!(db2.close_called);

        // close error is ignored (like python `except: pass` in finally)
        let mut db3 = FakeDb::new().with_latest(Some(1)).with_reaction_ok(json!({"agent":"❤️"}));
        db3.close_should_error = true;
        let out = react_to_message_tool_with_session_db("❤️", None, None, Some(&mut db3), "sess", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
        assert!(db3.close_called);
    }

    #[test]
    fn messages_back_as_string_and_bool() {
        let mut db = FakeDb::new().with_latest(Some(10)).with_reaction_ok(json!({"agent":"❤️"}));
        let out = react_to_message_with_db("❤️", None, Some(&json!("2")), &mut db, "sess", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
        assert_eq!(db.latest_calls[0].2, 2);

        let mut db2 = FakeDb::new().with_latest(Some(10)).with_reaction_ok(json!({"agent":"❤️"}));
        let out = react_to_message_with_db("❤️", None, Some(&json!(true)), &mut db2, "sess", |_, _| Ok(true));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
        assert_eq!(db2.latest_calls[0].2, 1);
    }
}
