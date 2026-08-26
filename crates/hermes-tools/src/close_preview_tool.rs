//! Close the Hermes desktop GUI's preview pane, or one of its tabs.
//! Port of `tools/close_preview_tool.py` (64 lines) — 1:1 behavior.
//!
//! Lives in the `desktop_ui` toolset (same as `open_preview`), which the GUI
//! gateway enables only for a session whose source is the desktop app. Emits
//! `preview.close` through the shared `desktop_ui` bridge; the renderer drops
//! the matching tab — or the whole pane when no url is given — for the window
//! that asked and never steals a background session's view.

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Registry metadata — mirrors `registry.register(...)` kwargs in Python
// ---------------------------------------------------------------------------

/// Tool name as registered in `tools.registry`.
pub const TOOL_NAME: &str = "close_preview";
/// Toolset that gates this tool (`toolset="desktop_ui"`).
pub const TOOLSET: &str = "desktop_ui";
/// Emoji for tool listing — mirrors `emoji="🖼️"` in Python.
pub const EMOJI: &str = "🖼️";
/// `requires_env` for this tool — none (desktop_ui is session-gated, not env-gated).
pub const REQUIRES_ENV: &[&str] = &[];

// ---------------------------------------------------------------------------
// Constants — mirrors Python module-level assignments
// ---------------------------------------------------------------------------

/// Error when no desktop emitter is wired.
///
/// Mirrors `tool_error("The preview pane is only available in the Hermes desktop app.")`
/// (lines 26-27).
pub const NOT_AVAILABLE_ERROR: &str =
    "The preview pane is only available in the Hermes desktop app.";

/// Prefix for the exception path.
///
/// Mirrors `tool_error(f"Failed to close the preview pane: {exc}")` (line 25).
pub const FAILED_PREFIX: &str = "Failed to close the preview pane: ";

/// Full tool description — mirrors `CLOSE_PREVIEW_SCHEMA["description"]`.
///
/// Joined from the Python multi-string literal (lines 34-41).
pub const DESCRIPTION: &str = "Close the preview pane beside the chat in the Hermes desktop app, or one tab inside it. Use this when the user asks to close, hide, or dismiss the preview — e.g. \"close the preview pane\", \"close cnn.com\", \"hide the preview\". Omit url to close the whole pane (every tab). Pass a web URL, localhost address, or file path to close only that tab. Counterpart of open_preview.";

/// Description for the `url` parameter — mirrors `CLOSE_PREVIEW_SCHEMA["parameters"]["properties"]["url"]["description"]`.
pub const URL_DESCRIPTION: &str = "Optional. The tab to close: a web URL (https://… or a bare domain), a localhost URL, or a file path. Omit to close the whole preview pane.";

// ---------------------------------------------------------------------------
// Schema — mirrors `CLOSE_PREVIEW_SCHEMA` dict in Python (lines 32-55)
// ---------------------------------------------------------------------------

/// Returns the JSON schema for `close_preview` — mirrors `CLOSE_PREVIEW_SCHEMA`.
///
/// In Python this is a dict literal; here we return a `serde_json::Value`
/// so callers can serialize or inspect it without owning a static JSON string.
pub fn close_preview_schema() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": DESCRIPTION,
        "parameters": {
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": URL_DESCRIPTION
                }
            }
        }
    })
}

/// Static schema value for registry consumers that need a serialized string.
/// Mirrors `CLOSE_PREVIEW_SCHEMA` as a serialized string.
pub fn close_preview_schema_json() -> String {
    close_preview_schema().to_string()
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
/// `close_preview_tool_with_emit`.
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
/// `close_preview_tool` path hits `NOT_AVAILABLE_ERROR` — identical to running the
/// Python tool outside the desktop app. Tests and real gateways inject a
/// closure via `close_preview_tool_with_emit`.
pub fn desktop_ui_emit(_event: &str, _payload: Value) -> Result<bool, String> {
    Ok(false)
}

// ---------------------------------------------------------------------------
// _normalize_target — mirrors `tools/open_preview_tool.py:_normalize_target`
// ---------------------------------------------------------------------------

/// Coax a bare host/domain into a fetchable URL; leave paths + schemes alone.
///
/// Mirrors `tools/open_preview_tool.py:_normalize_target` (lines 19-33):
/// ```python
/// v = raw.strip().strip("`").strip()
/// if not v or "://" in v or v.startswith(("/", "./", "../", "~", "file:")):
///     return v
/// if re.match(r"^(localhost|127\\.0\\.0\\.1|0\\.0\\.0\\.0|\\[::1\\])(:\\d+)?(/|$)", v, re.I):
///     return "http://" + v
/// if re.match(r"^[\\w.-]+\\.[a-z]{2,}(:\\d+)?(/.*)?$", v, re.I):
///     return "https://" + v
/// return v
/// ```
///
/// `www.cnn.com` → `https://www.cnn.com`; `localhost:3000` → `http://localhost:3000`.
/// File paths and explicit schemes pass through for the renderer's preview
/// normalizer to classify.
///
/// Implemented without the `regex` crate to avoid adding a dependency — the
/// two regexes are hand-rolled to 1:1 semantics and tested against the Python
/// implementation.
pub fn normalize_target(raw: &str) -> String {
    // v = raw.strip().strip("`").strip()
    let v = raw.trim().trim_matches('`').trim().to_string();
    if v.is_empty()
        || v.contains("://")
        || v.starts_with('/')
        || v.starts_with("./")
        || v.starts_with("../")
        || v.starts_with('~')
        || v.starts_with("file:")
    {
        return v;
    }

    // localhost family: ^(localhost|127.0.0.1|0.0.0.0|\[::1\])(:\d+)?(/|$)
    let lower = v.to_ascii_lowercase();
    const HOSTS: &[&str] = &["localhost", "127.0.0.1", "0.0.0.0", "[::1]"];
    for host in HOSTS {
        if lower.starts_with(host) {
            let rest = &v[host.len()..];
            if rest.is_empty() || rest.starts_with('/') {
                return format!("http://{v}");
            }
            if let Some(after_colon) = rest.strip_prefix(':') {
                let digits_len = after_colon
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .count();
                if digits_len == 0 {
                    continue;
                }
                let after_digits = &after_colon[digits_len..];
                if after_digits.is_empty() || after_digits.starts_with('/') {
                    return format!("http://{v}");
                }
            }
        }
    }

    // domain: ^[\w.-]+\.[a-z]{2,}(:\d+)?(/.*)?$  (case-insensitive)
    // Split at first '/' — the "/.*" suffix is arbitrary.
    let slash_idx = v.find('/');
    let host_port = match slash_idx {
        Some(idx) => &v[..idx],
        None => v.as_str(),
    };

    // Extract optional :\d+ port. If colon present, digits required.
    let host_str = if let Some(colon_idx) = host_port.rfind(':') {
        let port_part = &host_port[colon_idx + 1..];
        if port_part.is_empty() || !port_part.chars().all(|c| c.is_ascii_digit()) {
            return v;
        }
        let h = &host_port[..colon_idx];
        if h.is_empty() {
            return v;
        }
        h
    } else {
        host_port
    };

    // host must be [\w.-]+  — word chars (alphanumeric + underscore) plus dot/hyphen.
    // Python \w is unicode alnum + underscore; use Rust is_alphanumeric (unicode) + '_'.
    if !host_str
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '-')
    {
        return v;
    }
    if !host_str.contains('.') {
        return v;
    }
    let last_dot = match host_str.rfind('.') {
        Some(idx) => idx,
        None => return v,
    };
    // ".com" (last_dot==0) is not a match; "..com" (last_dot==1) is per Python.
    if last_dot == 0 {
        return v;
    }
    let tld = &host_str[last_dot + 1..];
    if tld.len() < 2 || !tld.chars().all(|c| c.is_ascii_alphabetic()) {
        return v;
    }

    format!("https://{v}")
}

// ---------------------------------------------------------------------------
// Core handler — mirrors `close_preview_tool(url: str = "") -> str` (lines 18-29)
// ---------------------------------------------------------------------------

/// Ask the desktop GUI to close the preview pane, or the tab for `url`.
///
/// Mirrors Python `def close_preview_tool(url: str = "") -> str:` (lines 18-29):
/// ```python
/// target = _normalize_target(url or "")
/// try:
///     ok = desktop_ui.emit("preview.close", {"url": target})
/// except Exception as exc:
///     return tool_error(f"Failed to close the preview pane: {exc}")
/// if not ok:
///     return tool_error("The preview pane is only available in the Hermes desktop app.")
/// return json.dumps({"success": True, "url": target}, ensure_ascii=False)
/// ```
///
/// The `url` arg mirrors Python's `url or ""` — callers that have an
/// `Option<String>` should pass `opt.as_deref().unwrap_or("")`. Empty strings
/// close the whole pane (every tab).
///
/// The desktop-ui call is delegated to `desktop_ui_emit`; override via
/// `close_preview_tool_with_emit` in tests or when the gateway has wired a
/// real emitter.
pub fn close_preview_tool(url: &str) -> String {
    close_preview_tool_with_emit(url, desktop_ui_emit)
}

/// Testable core: same as `close_preview_tool` but with an injected emit fn.
///
/// `emit` mirrors `desktop_ui.emit`: `Fn(&str, Value) -> Result<bool, String>`
/// where `Ok(true)` = closed, `Ok(false)` = no desktop (not available), `Err(msg)`
/// = exception path.
pub fn close_preview_tool_with_emit<F>(url: &str, emit: F) -> String
where
    F: FnOnce(&str, Value) -> Result<bool, String>,
{
    let target = normalize_target(url);

    let payload = json!({ "url": target });
    let ok = match emit("preview.close", payload) {
        Ok(v) => v,
        Err(exc) => {
            return tool_error(&format!("{FAILED_PREFIX}{exc}"));
        }
    };
    if !ok {
        return tool_error(NOT_AVAILABLE_ERROR);
    }

    // Mirrors `json.dumps({"success": True, "url": target}, ensure_ascii=False)`
    json!({ "success": true, "url": target }).to_string()
}

/// Mirrors the registry handler lambda:
/// `lambda args, **kw: close_preview_tool(url=args.get("url") or "")`
///
/// Extracts `url` as string (missing/non-string → `""`) and delegates to
/// `close_preview_tool`. The `Option`-to-string fallback preserves Python's
/// `args.get("url") or ""` semantics.
pub fn handler(args: &Value) -> String {
    let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
    close_preview_tool(url)
}

/// Variant that injects an emitter — mirrors `handler` but testable.
pub fn handler_with_emit<F>(args: &Value, emit: F) -> String
where
    F: FnOnce(&str, Value) -> Result<bool, String>,
{
    let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
    close_preview_tool_with_emit(url, emit)
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
        assert_eq!(TOOL_NAME, "close_preview");
        assert_eq!(TOOLSET, "desktop_ui");
        assert_eq!(EMOJI, "🖼️");
        assert!(REQUIRES_ENV.is_empty());
        assert_eq!(
            NOT_AVAILABLE_ERROR,
            "The preview pane is only available in the Hermes desktop app."
        );
        assert_eq!(FAILED_PREFIX, "Failed to close the preview pane: ");
        assert!(DESCRIPTION.starts_with("Close the preview pane beside the chat"));
        assert!(DESCRIPTION.contains("close cnn.com"));
        assert!(DESCRIPTION.contains("hide the preview"));
        assert!(DESCRIPTION.contains("Counterpart of open_preview"));
        assert_eq!(
            URL_DESCRIPTION,
            "Optional. The tab to close: a web URL (https://… or a bare domain), a localhost URL, or a file path. Omit to close the whole preview pane."
        );
    }

    #[test]
    fn schema_matches_python() {
        let schema = close_preview_schema();
        assert_eq!(schema["name"], "close_preview");
        assert_eq!(schema["description"], DESCRIPTION);
        assert_eq!(schema["parameters"]["type"], "object");
        assert_eq!(
            schema["parameters"]["properties"]["url"]["type"],
            "string"
        );
        assert_eq!(
            schema["parameters"]["properties"]["url"]["description"],
            URL_DESCRIPTION
        );
        // No required fields — url is optional
        assert!(schema["parameters"].get("required").is_none()
            || schema["parameters"]["required"]
                .as_array()
                .map(|a| a.is_empty())
                .unwrap_or(true));
        // Ensure JSON serialization round-trips (mirrors Python dict)
        let s = close_preview_schema_json();
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, schema);
        assert!(s.contains("close_preview"));
    }

    #[test]
    fn normalize_target_matches_python() {
        let cases = [
            ("www.cnn.com", "https://www.cnn.com"),
            ("cnn.com", "https://cnn.com"),
            (" localhost:3000 ", "http://localhost:3000"),
            ("localhost:3000/path", "http://localhost:3000/path"),
            ("127.0.0.1:8000", "http://127.0.0.1:8000"),
            ("0.0.0.0:8080", "http://0.0.0.0:8080"),
            ("[::1]:3000", "http://[::1]:3000"),
            ("[::1]", "http://[::1]"),
            ("file:///tmp/foo", "file:///tmp/foo"),
            ("/tmp/foo", "/tmp/foo"),
            ("./relative", "./relative"),
            ("../up", "../up"),
            ("~/home", "~/home"),
            ("https://example.com", "https://example.com"),
            ("http://example.com", "http://example.com"),
            ("example", "example"),
            ("foo.a", "foo.a"),
            ("example.com:abc", "example.com:abc"),
            ("example.com:8080/path", "https://example.com:8080/path"),
            ("example.com/path?query", "https://example.com/path?query"),
            (".com", ".com"),
            ("www.cnn.com:8080", "https://www.cnn.com:8080"),
            ("LOCALHOST:3000", "http://LOCALHOST:3000"),
            ("WWW.CNN.COM", "https://WWW.CNN.COM"),
            ("www_cnn.com", "https://www_cnn.com"),
            ("my-site.example.com", "https://my-site.example.com"),
            ("  `www.cnn.com`  ", "https://www.cnn.com"),
            ("", ""),
            ("   ", ""),
            ("localhostfoo", "localhostfoo"),
            ("localhost:abc", "localhost:abc"),
            ("example.com.", "example.com."),
            ("example.com:8080", "https://example.com:8080"),
            ("foo-bar.example.co.uk", "https://foo-bar.example.co.uk"),
            ("127.0.0.1", "http://127.0.0.1"),
            ("localhost", "http://localhost"),
            ("a.b", "a.b"),
            ("a.bc", "https://a.bc"),
            ("..com", "https://..com"),
            ("example..com", "https://example..com"),
            ("example.com..", "example.com.."),
            ("example.com:", "example.com:"),
            ("ex ample.com", "ex ample.com"),
            ("localhost:3000?query", "localhost:3000?query"),
            ("localhost/abc", "http://localhost/abc"),
            ("[::1]/abc", "http://[::1]/abc"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                normalize_target(input),
                expected,
                "input={input:?}"
            );
        }
    }

    #[test]
    fn success_path_close_whole_pane_when_empty() {
        let out = close_preview_tool_with_emit("", |event, payload| {
            assert_eq!(event, "preview.close");
            assert_eq!(payload["url"], "");
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
        assert_eq!(v["url"], "");
        assert!(v.get("error").is_none());
    }

    #[test]
    fn success_path_normalizes_url() {
        let out = close_preview_tool_with_emit("www.cnn.com", |event, payload| {
            assert_eq!(event, "preview.close");
            assert_eq!(payload["url"], "https://www.cnn.com");
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
        assert_eq!(v["url"], "https://www.cnn.com");

        let out = close_preview_tool_with_emit("localhost:3000", |_, payload| {
            assert_eq!(payload["url"], "http://localhost:3000");
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["url"], "http://localhost:3000");

        let out = close_preview_tool_with_emit("/tmp/foo.html", |_, payload| {
            assert_eq!(payload["url"], "/tmp/foo.html");
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["url"], "/tmp/foo.html");
    }

    #[test]
    fn success_path_preserves_explicit_scheme() {
        let out = close_preview_tool_with_emit("https://example.com", |_, payload| {
            assert_eq!(payload["url"], "https://example.com");
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["url"], "https://example.com");
    }

    #[test]
    fn trims_and_strips_backticks_before_emit() {
        let out = close_preview_tool_with_emit("  `www.cnn.com`  ", |_, payload| {
            assert_eq!(payload["url"], "https://www.cnn.com");
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["url"], "https://www.cnn.com");
    }

    #[test]
    fn not_available_when_not_on_desktop() {
        let out = close_preview_tool_with_emit("https://example.com", |_, _| Ok(false));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);

        let out = close_preview_tool_with_emit("", |_, _| Ok(false));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);

        // Default stub also returns not available
        let out = close_preview_tool("https://example.com");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);

        let out = close_preview_tool("");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);
    }

    #[test]
    fn exception_path_returns_failed_error() {
        let out = close_preview_tool_with_emit("https://example.com", |_, _| {
            Err("boom".to_string())
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], "Failed to close the preview pane: boom");

        let out = handler_with_emit(&json!({"url": "www.cnn.com"}), |_, _| {
            Err("transport closed".to_string())
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["error"]
            .as_str()
            .unwrap()
            .contains("Failed to close the preview pane: transport closed"));
        // Ensure normalization still happened before error
        let out = close_preview_tool_with_emit("www.cnn.com", |_, payload| {
            assert_eq!(payload["url"], "https://www.cnn.com");
            Err("oops".to_string())
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["error"].as_str().unwrap().contains("oops"));
    }

    #[test]
    fn handler_extracts_url_like_python_lambda() {
        let out = handler_with_emit(&json!({"url": "https://example.com"}), |_, p| {
            assert_eq!(p["url"], "https://example.com");
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["url"], "https://example.com");

        // Missing → "" → close whole pane
        let out = handler_with_emit(&json!({}), |_, p| {
            assert_eq!(p["url"], "");
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["url"], "");
        assert_eq!(v["success"], true);

        // Non-string → "" per and_then(...).unwrap_or("")
        let out = handler_with_emit(&json!({"url": 42}), |_, p| {
            assert_eq!(p["url"], "");
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["url"], "");

        // Bare handler without desktop → not available
        let out = handler(&json!({"url": "https://example.com"}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], NOT_AVAILABLE_ERROR);
    }

    #[test]
    fn handler_normalizes_like_tool() {
        let out = handler_with_emit(&json!({"url": "www.cnn.com"}), |_, p| {
            assert_eq!(p["url"], "https://www.cnn.com");
            Ok(true)
        });
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["url"], "https://www.cnn.com");
    }

    #[test]
    fn json_preserves_unicode_ensure_ascii_false() {
        let out = close_preview_tool_with_emit("https://example.com/ café 🖼️", |_, _| Ok(true));
        assert!(out.contains("café"));
        assert!(out.contains('🖼️'));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["url"], "https://example.com/ café 🖼️");

        // tool_error preserves unicode
        let out = tool_error("café 🖼️ error");
        assert!(out.contains("café"));
        assert!(out.contains('🖼️'));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], "café 🖼️ error");

        // success payload unicode not escaped
        let out = close_preview_tool_with_emit("https://example.com/ café", |_, _| Ok(true));
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
        assert_eq!(
            err.chars().count(),
            MAX_TOOL_ERROR_CHARS + TOOL_ERROR_TRUNCATION_MARKER.chars().count()
        );
    }

    #[test]
    fn empty_url_closes_whole_pane() {
        for url in ["", "   ", "  ``  "] {
            let out = close_preview_tool_with_emit(url, |_, payload| {
                assert_eq!(payload["url"], "");
                Ok(true)
            });
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["success"], true);
            assert_eq!(v["url"], "");
        }
    }
}
