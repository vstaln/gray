//! Shared helpers for classifying tool result payloads.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/tool_result_classification.py` (40 lines).
//!
//! Python source docstring (preserved):
//! ```text
//! Shared helpers for classifying tool result payloads.
//! ```
//!
//! Python source (preserved verbatim, lines 1-40):
//! ```python
//! """Shared helpers for classifying tool result payloads."""
//!
//! from __future__ import annotations
//!
//! import json
//! from typing import Any
//!
//!
//! FILE_MUTATING_TOOL_NAMES = frozenset({"write_file", "patch"})
//!
//!
//! # Tools whose interrupted/dangling execution is safe to discard because they
//! # cannot mutate either external state or Hermes session state. Unknown/plugin/
//! # MCP tools stay effect-capable by default.
//! NO_EFFECT_TOOL_NAMES = frozenset({
//!     "read_file", "search_files", "session_search", "skill_view", "skills_list",
//!     "web_extract", "web_search", "vision_analyze", "browser_snapshot",
//!     "browser_get_images", "browser_console", "read_terminal",
//! })
//!
//!
//! def tool_may_have_side_effect(tool_name: str) -> bool:
//!     return tool_name not in NO_EFFECT_TOOL_NAMES
//!
//!
//! def file_mutation_result_landed(tool_name: str, result: Any) -> bool:
//!     """Return True when a file mutation result proves the write landed."""
//!     if tool_name not in FILE_MUTATING_TOOL_NAMES or not isinstance(result, str):
//!         return False
//!     try:
//!         data = json.loads(result.strip())
//!     except Exception:
//!         return False
//!     if not isinstance(data, dict) or data.get("error"):
//!         return False
//!     if tool_name == "write_file":
//!         return "bytes_written" in data
//!     if tool_name == "patch":
//!         return data.get("success") is True
//!     return False
//! ```

use serde_json::Value;

// ---------------------------------------------------------------------------
// Constants — mirrors lines 9 and 15-19
// ---------------------------------------------------------------------------

/// Tools that mutate files on disk — a write is only considered "landed"
/// when the tool's JSON result carries the expected success marker.
/// Mirrors `FILE_MUTATING_TOOL_NAMES = frozenset({"write_file", "patch"})` (line 9).
pub const FILE_MUTATING_TOOL_NAMES: &[&str] = &["write_file", "patch"];

/// Tools whose interrupted/dangling execution is safe to discard because they
/// cannot mutate either external state or Hermes session state. Unknown/plugin/
/// MCP tools stay effect-capable by default.
/// Mirrors `NO_EFFECT_TOOL_NAMES = frozenset({...})` (lines 15-19).
pub const NO_EFFECT_TOOL_NAMES: &[&str] = &[
    "read_file",
    "search_files",
    "session_search",
    "skill_view",
    "skills_list",
    "web_extract",
    "web_search",
    "vision_analyze",
    "browser_snapshot",
    "browser_get_images",
    "browser_console",
    "read_terminal",
];

// Keep underscore-prefixed aliases for 1:1 traceability with Python private names.
#[allow(dead_code)]
const _FILE_MUTATING_TOOL_NAMES: &[&str] = FILE_MUTATING_TOOL_NAMES;
#[allow(dead_code)]
const _NO_EFFECT_TOOL_NAMES: &[&str] = NO_EFFECT_TOOL_NAMES;

// ---------------------------------------------------------------------------
// Helpers — Python `bool(value)` truthiness for `serde_json::Value`
// ---------------------------------------------------------------------------

/// Python `bool(value)` for JSON values — mirrors `data.get("error")` truthiness
/// check (line 34): `None`/`False`/`0`/`""`/`[]`/`{}` are falsy, everything else truthy.
fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i != 0
            } else if let Some(u) = n.as_u64() {
                u != 0
            } else if let Some(f) = n.as_f64() {
                f != 0.0 && !f.is_nan()
            } else {
                false
            }
        }
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

// ---------------------------------------------------------------------------
// Public API — mirrors lines 22-40
// ---------------------------------------------------------------------------

/// Return `true` if `tool_name` may have a side effect.
///
/// Unknown/plugin/MCP tools are effect-capable by default — only the
/// allow-list in [`NO_EFFECT_TOOL_NAMES`] is considered side-effect-free.
///
/// Mirrors `tool_may_have_side_effect` (lines 22-23):
/// ```python
/// def tool_may_have_side_effect(tool_name: str) -> bool:
///     return tool_name not in NO_EFFECT_TOOL_NAMES
/// ```
pub fn tool_may_have_side_effect(tool_name: &str) -> bool {
    !NO_EFFECT_TOOL_NAMES.contains(&tool_name)
}

/// Return `true` when a file mutation result proves the write landed.
///
/// Mirrors `file_mutation_result_landed` (lines 26-40):
/// ```python
/// def file_mutation_result_landed(tool_name: str, result: Any) -> bool:
///     """Return True when a file mutation result proves the write landed."""
///     if tool_name not in FILE_MUTATING_TOOL_NAMES or not isinstance(result, str):
///         return False
///     try:
///         data = json.loads(result.strip())
///     except Exception:
///         return False
///     if not isinstance(data, dict) or data.get("error"):
///         return False
///     if tool_name == "write_file":
///         return "bytes_written" in data
///     if tool_name == "patch":
///         return data.get("success") is True
///     return False
/// ```
///
/// `result: Any` non-`str` in Python maps to `None` / caller not calling this
/// overload in Rust — any non-`&str` input is defined to return `false`. Use
/// [`file_mutation_result_landed_opt`] if the result may be absent.
pub fn file_mutation_result_landed(tool_name: &str, result: &str) -> bool {
    // Mirrors `if tool_name not in FILE_MUTATING_TOOL_NAMES or not isinstance(result, str): return False` (line 28)
    if !FILE_MUTATING_TOOL_NAMES.contains(&tool_name) {
        return false;
    }
    // Mirrors `data = json.loads(result.strip())` with `except Exception: return False` (lines 30-33)
    let trimmed = result.trim();
    let data: Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => return false,
    };
    // Mirrors `if not isinstance(data, dict) or data.get("error"): return False` (lines 34-35)
    let obj = match data.as_object() {
        Some(m) => m,
        None => return false,
    };
    if let Some(err) = obj.get("error") {
        if is_truthy(err) {
            return false;
        }
    }
    // Mirrors `if tool_name == "write_file": return "bytes_written" in data` (lines 36-37)
    if tool_name == "write_file" {
        return obj.contains_key("bytes_written");
    }
    // Mirrors `if tool_name == "patch": return data.get("success") is True` (lines 38-39)
    if tool_name == "patch" {
        return obj.get("success") == Some(&Value::Bool(true));
    }
    // Mirrors `return False` (line 40) — unreachable for known mutating tools but kept for 1:1.
    false
}

/// `Option<&str>` variant — `None` models Python's `not isinstance(result, str)` branch (line 28).
pub fn file_mutation_result_landed_opt(tool_name: &str, result: Option<&str>) -> bool {
    match result {
        Some(s) => file_mutation_result_landed(tool_name, s),
        None => false,
    }
}

/// `serde_json::Value` variant for callers that already have a parsed payload.
/// Returns `false` for non-object values and truthy `error` fields, mirroring lines 34-35.
pub fn file_mutation_result_landed_value(tool_name: &str, data: &Value) -> bool {
    if !FILE_MUTATING_TOOL_NAMES.contains(&tool_name) {
        return false;
    }
    let obj = match data.as_object() {
        Some(m) => m,
        None => return false,
    };
    if let Some(err) = obj.get("error") {
        if is_truthy(err) {
            return false;
        }
    }
    if tool_name == "write_file" {
        return obj.contains_key("bytes_written");
    }
    if tool_name == "patch" {
        return obj.get("success") == Some(&Value::Bool(true));
    }
    false
}
