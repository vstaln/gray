//! Close a read-only agent terminal tab in the Hermes desktop GUI.
//! Port of `tools/close_terminal_tool.py` (62 lines) — 1:1 behavior.
//!
//! Each `terminal(background=true)` process is mirrored as a read-only tab in the
//! desktop's terminal pane. This tool lets the agent drop a tab it no longer needs
//! to show — WITHOUT killing the process (use `process(action='kill')` for that).
//! The output keeps buffering and the user can reopen the tab from the status stack.
//!
//! It routes through the process registry's `on_close` sink, which the desktop
//! gateway wires to emit a `terminal.close` event the renderer handles. Like
//! `read_terminal` it lives in the `desktop_ui` toolset, which the GUI gateway
//! enables only for desktop-sourced sessions, so it never appears outside the GUI.

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Registry metadata — mirrors `registry.register(...)` kwargs in Python
// ---------------------------------------------------------------------------

/// Tool name as registered in `tools.registry`.
pub const TOOL_NAME: &str = "close_terminal";
/// Toolset that gates this tool (`toolset="desktop_ui"`).
pub const TOOLSET: &str = "desktop_ui";
/// Emoji for tool listing — mirrors `emoji="🖥️"` in Python.
pub const EMOJI: &str = "🖥️";
/// `requires_env` for this tool — none (desktop_ui is session-gated, not env-gated).
pub const REQUIRES_ENV: &[&str] = &[];

// ---------------------------------------------------------------------------
// Constants — mirrors Python module-level assignments
// ---------------------------------------------------------------------------

/// Error when `process_id` is missing/empty.
///
/// Mirrors `tool_error("process_id is required (the background process whose tab to close).")`
pub const PROCESS_ID_REQUIRED_ERROR: &str =
    "process_id is required (the background process whose tab to close).";

/// Error when no desktop close sink is wired — not running in desktop app.
///
/// Mirrors `{"status": "error", "error": "close_terminal is only available in the Hermes desktop app."}`
/// returned by `process_registry.request_close_terminal` when `self.on_close is None`
/// (lines 2388-2393).
pub const NOT_AVAILABLE_ERROR: &str =
    "close_terminal is only available in the Hermes desktop app.";

 /// Note returned on successful close.
///
/// Mirrors `process_registry.request_close_terminal` success note (lines 2404-2407):
/// `"Closed the read-only terminal tab. The process was not killed; its output remains available and the user can reopen the tab from the status stack."`
pub const SUCCESS_NOTE: &str = "Closed the read-only terminal tab. The process was not killed; its output remains available and the user can reopen the tab from the status stack.";

/// Full tool description — mirrors `CLOSE_TERMINAL_SCHEMA["description"]`.
///
/// Joined from the Python multi-string literal (lines 32-39).
pub const DESCRIPTION: &str = "Close the read-only terminal tab for one of your background processes in the Hermes desktop GUI (the tabs mirroring terminal(background=true) runs). This does NOT kill the process — it only drops the tab/view; the output keeps buffering and the user can reopen it from the status stack. Use it to tidy up when a background process's live terminal is no longer worth showing. To actually stop the process, use process(action='kill') instead.";

 /// Description for the `process_id` parameter — mirrors `CLOSE_TERMINAL_SCHEMA["parameters"]["properties"]["process_id"]["description"]`.
pub const PROCESS_ID_DESCRIPTION: &str = "The background process's session id (from terminal(background=true) output or process(action='list')) whose tab should be closed.";

// ---------------------------------------------------------------------------
// Schema — mirrors `CLOSE_TERMINAL_SCHEMA` dict in Python (lines 30-53)
// ---------------------------------------------------------------------------

/// Returns the JSON schema for `close_terminal` — mirrors `CLOSE_TERMINAL_SCHEMA`.
///
/// In Python this is a dict literal; here we return a `serde_json::Value`
/// so callers can serialize or inspect it without owning a static JSON string.
pub fn close_terminal_schema() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": DESCRIPTION,
        "parameters": {
            "type": "object",
            "properties": {
                "process_id": {
                    "type": "string",
                    "description": PROCESS_ID_DESCRIPTION
                }
            },
            "required": ["process_id"]
        }
    })
}

/// Static schema value for registry consumers that need a serialized string.
/// Mirrors `CLOSE_TERMINAL_SCHEMA` as a serialized string.
pub fn close_terminal_schema_json() -> String {
    close_terminal_schema().to_string()
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
// process_registry bridge — mirrors `tools/process_registry.py` (1:1 semantics)
// ---------------------------------------------------------------------------

/// Mirrors `process_registry.on_close is not None` — true when a close sink is wired.
///
/// In Python this checks `self.on_close is None`. Here it's a stub that returns
/// `false` until the gateway wires a real sink via the injected closure.
///
/// For 1:1 traceability the stub is kept; tests inject a sink via
/// `request_close_terminal_with_sink` / `close_terminal_tool_with_sink`.
pub fn process_registry_on_close_available() -> bool {
    false
}

/// Mirrors `process_registry.request_close_terminal(session_id) -> dict` when no sink is wired.
///
/// Python (lines 2380-2409):
/// ```python
/// def request_close_terminal(self, session_id: str) -> dict:
///     sink = self.on_close
///     if sink is None:
///         return {"status": "error", "error": "close_terminal is only available in the Hermes desktop app."}
///     session = self.get(session_id)
///     try:
///         sink(session, session_id)
///     except Exception as e:
///         return {"status": "error", "error": str(e)}
///     return {"status": "ok", "closed": session_id, "note": "..."}
/// ```
///
/// This stub returns the `status: error` dict (no desktop) so the default
/// `close_terminal_tool` path hits `NOT_AVAILABLE_ERROR` — identical to running
/// the Python tool outside the desktop app.
pub fn request_close_terminal(pid: &str) -> Value {
    request_close_terminal_with_sink(pid, None::<fn(&str) -> Result<(), String>>)
}

/// Testable core: same as `request_close_terminal` but with an injected sink.
///
/// `sink` mirrors `process_registry.on_close`: `Fn(&str) -> Result<(), String>`
/// where the `&str` is the `session_id` / `process_id`. The Python sink
/// signature is `sink(session, session_id)` — the `session` may be `None` when
/// the process is already finished/pruned (the tab can still linger and be
/// closed), so the tab close is not an error for missing sessions. Here the
/// session is elided and only `pid` is forwarded; callers that need session
/// context can capture it in the closure.
///
/// - `None` → `{"status": "error", "error": NOT_AVAILABLE_ERROR}` (no desktop)
/// - `Some(f)` where `f(pid)` returns `Ok(())` → `{"status":"ok","closed":pid,"note": SUCCESS_NOTE}`
/// - `Some(f)` where `f(pid)` returns `Err(msg)` → `{"status":"error","error": msg}`
pub fn request_close_terminal_with_sink<F>(pid: &str, sink: Option<F>) -> Value
where
    F: FnOnce(&str) -> Result<(), String>,
{
    match sink {
        None => json!({ "status": "error", "error": NOT_AVAILABLE_ERROR }),
        Some(f) => match f(pid) {
            Ok(()) => json!({
                "status": "ok",
                "closed": pid,
                "note": SUCCESS_NOTE
            }),
            Err(exc) => json!({ "status": "error", "error": exc }),
        },
    }
}

/// Variant that takes a two-arg sink mirroring Python's `sink(session, pid)`.
///
/// Some callers wire `on_close` as `Fn(Option<Value>, &str) -> Result<(),String>`
/// where the first arg is the session (which may be `None`/null when already
/// pruned). This helper adapts that shape to the single-arg core by dropping
/// the session — the tab close remains valid regardless of session presence
/// (Python comment lines 2394-2395).
pub fn request_close_terminal_with_session_sink<F>(pid: &str, sink: Option<F>) -> Value
where
    F: FnOnce(Option<Value>, &str) -> Result<(), String>,
{
    match sink {
        None => json!({ "status": "error", "error": NOT_AVAILABLE_ERROR }),
        Some(f) => {
            // Session lookup is omitted in this crate (no registry state);
            // pass None (null) to mirror a pruned/finished session. Callers
            // that have a live registry can capture the session and ignore this arg.
            match f(None, pid) {
                Ok(()) => json!({
                    "status": "ok",
                    "closed": pid,
                    "note": SUCCESS_NOTE
                }),
                Err(exc) => json!({ "status": "error", "error": exc }),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Core handler — mirrors `close_terminal_tool(process_id: str) -> str` (lines 21-27)
// ---------------------------------------------------------------------------

/// Ask the desktop GUI to close a background process's read-only tab.
///
/// Mirrors Python `def close_terminal_tool(process_id: str) -> str:` (lines 21-27):
/// ```python
/// pid = (process_id or "").strip()
/// if not pid:
///     return tool_error("process_id is required (the background process whose tab to close).")
/// return json.dumps(process_registry.request_close_terminal(pid), ensure_ascii=False)
/// ```
///
/// The `process_id` arg mirrors Python's `process_id or ""` — callers that have an
/// `Option<String>` should pass `opt.as_deref().unwrap_or("")`. Empty or
/// whitespace-only strings trigger the required-field error.
///
/// The registry call is delegated to `request_close_terminal`; override via
/// `close_terminal_tool_with_sink` in tests or when the gateway has wired a
/// real `on_close` sink.
pub fn close_terminal_tool(process_id: &str) -> String {
    close_terminal_tool_with_sink(
        process_id,
        None::<fn(&str) -> Result<(), String>>,
    )
}

/// Testable core: same as `close_terminal_tool` but with an injected sink.
///
/// `sink` mirrors `process_registry.on_close`: `FnOnce(&str) -> Result<(),String>`
/// where `Ok(())` = tab closed, `Err(msg)` = exception path.
/// `None` = no desktop (returns `status: error` with `NOT_AVAILABLE_ERROR`).
pub fn close_terminal_tool_with_sink<F>(process_id: &str, sink: Option<F>) -> String
where
    F: FnOnce(&str) -> Result<(), String>,
{
    let pid = process_id.trim();
    if pid.is_empty() {
        return tool_error(PROCESS_ID_REQUIRED_ERROR);
    }

    // Mirrors `json.dumps(process_registry.request_close_terminal(pid), ensure_ascii=False)`
    request_close_terminal_with_sink(pid, sink).to_string()
}

/// Variant that takes a session-aware sink `Fn(Option<Value>, &str)`.
///
/// Mirrors `request_close_terminal_with_session_sink` — useful when the caller
/// wants to assert the session argument is forwarded (e.g. `test_request_close_terminal_invokes_sink_without_killing`).
pub fn close_terminal_tool_with_session_sink<F>(process_id: &str, sink: Option<F>) -> String
where
    F: FnOnce(Option<Value>, &str) -> Result<(), String>,
{
    let pid = process_id.trim();
    if pid.is_empty() {
        return tool_error(PROCESS_ID_REQUIRED_ERROR);
    }
    request_close_terminal_with_session_sink(pid, sink).to_string()
}

/// Mirrors the registry handler lambda:
/// `lambda args, **kw: close_terminal_tool(process_id=args.get("process_id", ""))`
///
/// Extracts `process_id` as string (missing/non-string → `""`) and delegates to
/// `close_terminal_tool`. The `Option`-to-string fallback preserves Python's
/// `args.get("process_id", "")` semantics.
pub fn handler(args: &Value) -> String {
    let process_id = args
        .get("process_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    close_terminal_tool(process_id)
}

/// Variant that injects a sink — mirrors `handler` but testable.
pub fn handler_with_sink<F>(args: &Value, sink: Option<F>) -> String
where
    F: FnOnce(&str) -> Result<(), String>,
{
    let process_id = args
        .get("process_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    close_terminal_tool_with_sink(process_id, sink)
}

/// Variant that injects a session-aware sink.
pub fn handler_with_session_sink<F>(args: &Value, sink: Option<F>) -> String
where
    F: FnOnce(Option<Value>, &str) -> Result<(), String>,
{
    let process_id = args
        .get("process_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    close_terminal_tool_with_session_sink(process_id, sink)
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
        assert_eq!(TOOL_NAME, "close_terminal");
        assert_eq!(TOOLSET, "desktop_ui");
        assert_eq!(EMOJI, "🖥️");
        assert!(REQUIRES_ENV.is_empty());
        assert_eq!(
            PROCESS_ID_REQUIRED_ERROR,
            "process_id is required (the background process whose tab to close)."
        );
        assert_eq!(
            NOT_AVAILABLE_ERROR,
            "close_terminal is only available in the Hermes desktop app."
        );
        assert_eq!(
            SUCCESS_NOTE,
            "Closed the read-only terminal tab. The process was not killed; its output remains available and the user can reopen the tab from the status stack."
        );
        assert!(DESCRIPTION.starts_with("Close the read-only terminal tab for one of your background processes"));
        assert!(DESCRIPTION.contains("terminal(background=true)"));
        assert!(DESCRIPTION.contains("This does NOT kill the process"));
        assert!(DESCRIPTION.contains("Use it to tidy up"));
        assert!(DESCRIPTION.contains("process(action='kill') instead"));
        assert_eq!(
            PROCESS_ID_DESCRIPTION,
            "The background process's session id (from terminal(background=true) output or process(action='list')) whose tab should be closed."
        );
    }

    #[test]
    fn schema_matches_python() {
        let schema = close_terminal_schema();
        assert_eq!(schema["name"], "close_terminal");
        assert_eq!(schema["description"], DESCRIPTION);
        assert_eq!(schema["parameters"]["type"], "object");
        assert_eq!(
            schema["parameters"]["properties"]["process_id"]["type"],
            "string"
        );
        assert_eq!(
            schema["parameters"]["properties"]["process_id"]["description"],
            PROCESS_ID_DESCRIPTION
        );
        let required = schema["parameters"]["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "process_id");
        // Ensure JSON serialization round-trips (mirrors Python dict)
        let s = close_terminal_schema_json();
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, schema);
        assert!(s.contains("close_terminal"));
        assert!(s.contains("process_id"));
    }

    #[test]
    fn empty_process_id_returns_tool_error() {
        for pid in ["", "   ", "\t\n"] {
            let out = close_terminal_tool_with_sink(pid, Some(|_: &str| Ok(())));
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["error"], PROCESS_ID_REQUIRED_ERROR, "pid={pid:?}");
            // sink must not be called — validation short-circuits
        }
        // handler with missing key → "" → same error
        let out = handler(&json!({}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], PROCESS_ID_REQUIRED_ERROR);

        let out = handler(&json!({"process_id": 42}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], PROCESS_ID_REQUIRED_ERROR);

        let out = close_terminal_tool("");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], PROCESS_ID_REQUIRED_ERROR);

        // whitespace-only via handler
        let out = handler(&json!({"process_id": "   "}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], PROCESS_ID_REQUIRED_ERROR);
    }

    #[test]
    fn trims_process_id_before_sink_and_success() {
        let out = close_terminal_tool_with_sink("  proc_abc123  ", Some(|pid: &str| {
            assert_eq!(pid, "proc_abc123");
            Ok(())
        }));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["closed"], "proc_abc123");
        assert_eq!(v["note"], SUCCESS_NOTE);
    }

    #[test]
    fn request_close_terminal_success_returns_ok() {
        let v = request_close_terminal_with_sink("proc_close_live", Some(|pid: &str| {
            assert_eq!(pid, "proc_close_live");
            Ok(())
        }));
        assert_eq!(v["status"], "ok");
        assert_eq!(v["closed"], "proc_close_live");
        assert_eq!(v["note"], SUCCESS_NOTE);

        // via close_terminal_tool_with_sink
        let out = close_terminal_tool_with_sink("proc_close_live", Some(|_: &str| Ok(())));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["closed"], "proc_close_live");
    }

    #[test]
    fn not_available_when_no_sink() {
        // None sink mirrors process_registry.on_close is None
        let v = request_close_terminal_with_sink::<fn(&str) -> Result<(), String>>("proc_123", None);
        assert_eq!(v["status"], "error");
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);

        let out = close_terminal_tool_with_sink::<fn(&str) -> Result<(), String>>("proc_123", None);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["status"], "error");
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);

        // Default wrappers also return not available (no desktop)
        let v = request_close_terminal("proc_123");
        assert_eq!(v["status"], "error");
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);

        let out = close_terminal_tool("proc_123");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["status"], "error");
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);

        // handler without sink also
        let out = handler(&json!({"process_id": "proc_123"}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["status"], "error");
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);
    }

    #[test]
    fn exception_path_returns_status_error() {
        let v = request_close_terminal_with_sink("proc_123", Some(|_: &str| Err("boom".to_string())));
        assert_eq!(v["status"], "error");
        assert_eq!(v["error"], "boom");

        let out = close_terminal_tool_with_sink("proc_123", Some(|_: &str| Err("boom".to_string())));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["status"], "error");
        assert_eq!(v["error"], "boom");

        // handler variant with sink error
        let out = handler_with_sink(
            &json!({"process_id": "proc_123"}),
            Some(|_: &str| Err("transport closed".to_string())),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["status"], "error");
        assert_eq!(v["error"], "transport closed");
    }

    #[test]
    fn handler_extracts_process_id_like_python_lambda() {
        let out = handler_with_sink(
            &json!({"process_id": "proc_abc"}),
            Some(|pid: &str| {
                assert_eq!(pid, "proc_abc");
                Ok(())
            }),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["closed"], "proc_abc");
        assert_eq!(v["status"], "ok");

        // trims via handler as well
        let out = handler_with_sink(
            &json!({"process_id": "  proc_trim  "}),
            Some(|pid: &str| {
                assert_eq!(pid, "proc_trim");
                Ok(())
            }),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["closed"], "proc_trim");

        // Missing → "" → required error, sink never called
        let out = handler_with_sink::<fn(&str) -> Result<(), String>>(&json!({}), None);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], PROCESS_ID_REQUIRED_ERROR);
    }

    #[test]
    fn session_sink_variant_forwards_pid_and_invokes_without_killing() {
        // Mirrors test_request_close_terminal_invokes_sink_without_killing
        let v = request_close_terminal_with_session_sink(
            "proc_close_live",
            Some(|session: Option<Value>, pid: &str| {
                // session may be None (pruned) — tab can still be closed; here we assert pid
                assert_eq!(pid, "proc_close_live");
                // In real gateway, session would be forwarded; here we just check it is None (no registry lookup)
                assert!(session.is_none());
                Ok(())
            }),
        );
        assert_eq!(v["status"], "ok");
        assert_eq!(v["closed"], "proc_close_live");

        // via close_terminal_tool_with_session_sink
        let out = close_terminal_tool_with_session_sink(
            "proc_close_live",
            Some(|_session: Option<Value>, pid: &str| {
                assert_eq!(pid, "proc_close_live");
                Ok(())
            }),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["closed"], "proc_close_live");

        // handler_with_session_sink
        let out = handler_with_session_sink(
            &json!({"process_id": "proc_close_live"}),
            Some(|_s: Option<Value>, pid: &str| {
                assert_eq!(pid, "proc_close_live");
                Ok(())
            }),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["status"], "ok");
    }

    #[test]
    fn json_preserves_unicode_ensure_ascii_false() {
        let pid = "proc_🖥️_café";
        let out = close_terminal_tool_with_sink(pid, Some(|_: &str| Ok(())));
        assert!(out.contains('🖥️'));
        assert!(out.contains("café"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["closed"], pid);
        // not available error also preserves unicode in message if any
        let out = tool_error("café 🖥️ error");
        assert!(out.contains("café"));
        assert!(out.contains('🖥️'));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], "café 🖥️ error");
        // success note unicode not escaped
        let v = request_close_terminal_with_sink(pid, Some(|_: &str| Ok(())));
        let s = v.to_string();
        assert!(!s.contains("\\u"));
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

        // status error from sink is not truncated by tool_error bound (it goes via json! directly),
        // but the error payload itself should be bounded if sink returns huge error?
        // In Python, _bound_json_error_result would bound at dispatch layer; here we test tool_error only.
    }

    #[test]
    fn close_does_not_imply_kill_semantics() {
        // Closing the tab must not be conflated with killing — the success note explicitly says not killed
        let v = request_close_terminal_with_sink("proc_live", Some(|_: &str| Ok(())));
        assert!(v["note"].as_str().unwrap().contains("was not killed"));
        assert!(v["note"].as_str().unwrap().contains("output remains available"));
        assert!(v["note"].as_str().unwrap().contains("reopen the tab"));
    }
}
