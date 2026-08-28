//! Shim for tool discovery. Registers `computer_use` with tools.registry.
//! Port of `tools/computer_use_tool.py` (42 lines) — 1:1 behavior.
//!
//! The real implementation lives in the `tools/computer_use/` package to keep
//! the file structure clean. This shim exists because tools.registry auto-imports
//! `tools/*.py` — we need a top-level module to trigger the registration.

/// Tool name as registered in `tools.registry`.
pub const TOOL_NAME: &str = "computer_use";
/// Toolset that gates this tool (`toolset="computer_use"`).
pub const TOOLSET: &str = "computer_use";
/// `requires_env` for this tool — none (empty).
pub const REQUIRES_ENV: &[&str] = &[];
/// Description registered for the tool (joined from the Python multi-string literal).
pub const DESCRIPTION: &str = "Universal desktop control via cua-driver (macOS, Windows, Linux). Works with any tool-capable model (Anthropic, OpenAI, OpenRouter, local vLLM, etc.). Background computer-use: does NOT steal the user's cursor or keyboard focus.";
/// Schema source — the real schema lives in `tools/computer_use/schema.py` (`COMPUTER_USE_SCHEMA`).
/// This shim re-exports that schema via the registry; in Rust the schema is owned by the
/// `computer_use` package and referenced here by name.
pub const SCHEMA_NAME: &str = "computer_use";

// ---------------------------------------------------------------------------
// Re-exported handlers — mirrors `__all__` in Python.
// The real implementations live in `tools/computer_use/tool.py`. These stubs
// preserve the public surface so `computer_use_tool` is a drop-in shim; wiring
// to the actual backend is done by the `computer_use` crate/package.
// ---------------------------------------------------------------------------

/// Mirrors `handle_computer_use(args, **kw)` in `tools/computer_use/tool.py`.
///
/// In Python this is the registry handler (`lambda args, **kw: handle_computer_use(args, **kw)`).
/// Rust callers should dispatch to the real `computer_use` backend; this stub exists to
/// preserve the shim's public API and to make the port 1:1 line-for-line traceable.
pub fn handle_computer_use(_args_json: &str) -> String {
    // ponytail: stub — real work lives in tools/computer_use/tool.py; wire via computer_use crate when ported
    "{\"error\": \"computer_use backend not wired in this crate — see tools/computer_use/\"}".to_string()
}

/// Mirrors `release_computer_use_session(session_id)` in `tools/computer_use/tool.py`.
pub fn release_computer_use_session(_session_id: &str) -> bool {
    false
}

/// Mirrors `set_approval_callback(cb)` in `tools/computer_use/tool.py`.
/// Registers a callback for computer_use approval prompts (matches `terminal_tool` pattern).
pub fn set_approval_callback() {
    // ponytail: no-op stub — real callback stored in tools/computer_use/tool.py
}

/// Mirrors `check_computer_use_requirements()` in `tools/computer_use/tool.py`.
///
/// Real check: `sys.platform in ("darwin", "win32", "linux") && cua_driver_binary_available()`.
/// Stub returns `false` until the `computer_use` backend crate is wired.
pub fn check_computer_use_requirements() -> bool {
    false
}

// `__all__` in Python lists (with a duplicated entry, preserved here as comment):
// "handle_computer_use", "release_computer_use_session", "set_approval_callback",
// "check_computer_use_requirements", "release_computer_use_session" (dup)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_python_registry_args() {
        assert_eq!(TOOL_NAME, "computer_use");
        assert_eq!(TOOLSET, "computer_use");
        assert!(REQUIRES_ENV.is_empty());
        assert_eq!(SCHEMA_NAME, "computer_use");
        assert!(DESCRIPTION.contains("cua-driver"));
        assert!(DESCRIPTION.contains("does NOT steal"));
        assert!(DESCRIPTION.starts_with("Universal desktop control"));
    }

    #[test]
    fn stubs_are_callable() {
        let out = handle_computer_use("{}");
        assert!(out.contains("error"));
        assert!(!release_computer_use_session("test-session"));
        // set_approval_callback is no-op but must not panic
        set_approval_callback();
        assert!(!check_computer_use_requirements());
    }
}
