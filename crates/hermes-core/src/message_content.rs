//! Visible-text extraction from chat/Responses message content shapes.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/message_content.py` (50 lines).
//!
//! Python source docstring (preserved):
//! ```text
//! Return the visible text from common chat/Responses message content shapes.
//! ```
//!
//! Python source (preserved verbatim, lines 1-50):
//! ```python
//! from __future__ import annotations
//!
//! from collections.abc import Mapping
//! from typing import Any
//!
//!
//! _NON_TEXT_PART_TYPES = {"image", "image_url", "input_image", "audio", "input_audio"}
//! _TEXT_KEYS = ("text", "content", "input_text", "output_text", "summary_text")
//!
//!
//! def _field(value: Any, key: str) -> Any:
//!     if isinstance(value, Mapping):
//!         return value.get(key)
//!     return getattr(value, key, None)
//!
//!
//! def _text_from_part(part: Any) -> str:
//!     if part is None:
//!         return ""
//!     if isinstance(part, str):
//!         return part
//!
//!     part_type = str(_field(part, "type") or "").strip().lower()
//!     if part_type in _NON_TEXT_PART_TYPES:
//!         return ""
//!
//!     for key in _TEXT_KEYS:
//!         text = _field(part, key)
//!         if isinstance(text, str):
//!             return text
//!     return ""
//!
//!
//! def flatten_message_text(content: Any, *, sep: str = "\n") -> str:
//!     """Return the visible text from common chat/Responses message content shapes."""
//!     if content is None:
//!         return ""
//!     if isinstance(content, str):
//!         return content
//!     if isinstance(content, list):
//!         chunks = [_text_from_part(part) for part in content]
//!         return sep.join(chunk for chunk in chunks if chunk)
//!
//!     text = _text_from_part(content)
//!     if text:
//!         return text
//!     try:
//!         return str(content)
//!     except Exception:
//!         return ""
//! ```

use serde_json::Value;

// ---------------------------------------------------------------------------
// Constants — mirrors lines 7-8
// ---------------------------------------------------------------------------

/// Parts whose `type` signals non-text media — never surface as text.
/// Mirrors `_NON_TEXT_PART_TYPES = {"image", "image_url", "input_image", "audio", "input_audio"}` (line 7).
pub const NON_TEXT_PART_TYPES: &[&str] = &[
    "image",
    "image_url",
    "input_image",
    "audio",
    "input_audio",
];

/// Keys probed for visible text, in priority order.
/// Mirrors `_TEXT_KEYS = ("text", "content", "input_text", "output_text", "summary_text")` (line 8).
pub const TEXT_KEYS: &[&str] = &["text", "content", "input_text", "output_text", "summary_text"];

// Keep underscore-prefixed aliases for 1:1 traceability with Python private names.
#[allow(dead_code)]
const _NON_TEXT_PART_TYPES: &[&str] = NON_TEXT_PART_TYPES;
#[allow(dead_code)]
const _TEXT_KEYS: &[&str] = TEXT_KEYS;

// ---------------------------------------------------------------------------
// _field — mirrors lines 11-14
// ---------------------------------------------------------------------------

/// Retrieve `key` from `value`.
///
/// Mirrors `_field(value, key)` (lines 11-14):
/// ```python
/// def _field(value: Any, key: str) -> Any:
///     if isinstance(value, Mapping):
///         return value.get(key)
///     return getattr(value, key, None)
/// ```
///
/// In Rust `Any` is `serde_json::Value`. The `Mapping` branch maps to
/// `Value::Object` lookup. The `getattr` branch (arbitrary Python objects with
/// attributes, e.g. `SimpleNamespace(type="...", text="...")` in tests) has no
/// direct Rust equivalent; `Value::Object` is the JSON-serialised form. Callers
/// holding a typed struct should convert it to `Value` or use the generic
/// `field_from_map` helper.
#[inline]
pub fn field(value: &Value, key: &str) -> Option<Value> {
    // Mirrors `if isinstance(value, Mapping): return value.get(key)` (lines 12-13)
    if let Value::Object(map) = value {
        map.get(key).cloned()
    } else {
        // Mirrors `return getattr(value, key, None)` (line 14) — no attribute path in `Value`
        None
    }
}

#[allow(dead_code)]
fn _field(value: &Value, key: &str) -> Option<Value> {
    field(value, key)
}

/// Lookup `key` in a borrowed `Value` without cloning — returns a reference
/// when `value` is an Object. Prefer this for hot paths.
#[inline]
pub fn field_ref<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    if let Value::Object(map) = value {
        map.get(key)
    } else {
        None
    }
}

#[allow(dead_code)]
fn _field_ref<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    field_ref(value, key)
}

// ---------------------------------------------------------------------------
// _text_from_part — mirrors lines 17-31
// ---------------------------------------------------------------------------

/// Extract visible text from a single content part.
///
/// Mirrors `_text_from_part(part)` (lines 17-31):
/// ```python
/// def _text_from_part(part: Any) -> str:
///     if part is None:
///         return ""
///     if isinstance(part, str):
///         return part
///     part_type = str(_field(part, "type") or "").strip().lower()
///     if part_type in _NON_TEXT_PART_TYPES:
///         return ""
///     for key in _TEXT_KEYS:
///         text = _field(part, key)
///         if isinstance(text, str):
///             return text
///     return ""
/// ```
pub fn text_from_part(part: &Value) -> String {
    // Mirrors `if part is None: return ""` (lines 18-19) — `Value::Null` is JSON null
    if part.is_null() {
        return String::new();
    }
    // Mirrors `if isinstance(part, str): return part` (lines 20-21)
    if let Value::String(s) = part {
        return s.clone();
    }

    // Mirrors `part_type = str(_field(part, "type") or "").strip().lower()` (line 23)
    // `_field` may return Null/missing; Python does `str(None or "")` → `""`.
    // For `Value`, `field_ref` returns `Option<&Value>`; non-string values are
    // stringified via `to_string` for the `str(...)` equivalent, with JSON
    // quoting stripped for strings (already handled).
    let part_type_raw = match field_ref(part, "type") {
        None => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(v) if v.is_null() => String::new(),
        Some(v) => {
            // `str(value)` in Python for non-string/non-None (e.g. number) → Rust `to_string()`
            // `serde_json::Value::to_string` for Number/Bool is already `str`-like.
            // For String we already handled; for other variants this mirrors `str(...)`.
            match v {
                Value::String(s) => s.clone(),
                _ => v.to_string(),
            }
        }
    };
    let part_type = part_type_raw.trim().to_ascii_lowercase();
    // Mirrors `if part_type in _NON_TEXT_PART_TYPES: return ""` (lines 24-25)
    // Comparison is lower-cased, matching Python `.lower()`.
    if NON_TEXT_PART_TYPES.contains(&part_type.as_str()) {
        return String::new();
    }

    // Mirrors `for key in _TEXT_KEYS: text = _field(part, key); if isinstance(text, str): return text` (lines 27-30)
    for key in TEXT_KEYS {
        if let Some(Value::String(s)) = field_ref(part, key) {
            return s.clone();
        }
    }
    // Mirrors `return ""` (line 31)
    String::new()
}

#[allow(dead_code)]
fn _text_from_part(part: &Value) -> String {
    text_from_part(part)
}

// ---------------------------------------------------------------------------
// flatten_message_text — mirrors lines 34-50
// ---------------------------------------------------------------------------

/// Return the visible text from common chat/Responses message content shapes.
///
/// Mirrors `flatten_message_text(content, *, sep="\n")` (lines 34-50):
/// ```python
/// def flatten_message_text(content: Any, *, sep: str = "\n") -> str:
///     """Return the visible text from common chat/Responses message content shapes."""
///     if content is None:
///         return ""
///     if isinstance(content, str):
///         return content
///     if isinstance(content, list):
///         chunks = [_text_from_part(part) for part in content]
///         return sep.join(chunk for chunk in chunks if chunk)
///
///     text = _text_from_part(content)
///     if text:
///         return text
///     try:
///         return str(content)
///     except Exception:
///         return ""
/// ```
///
/// `content` is `serde_json::Value` as the Rust `Any` for JSON-shaped
/// message content (`None` → `Value::Null`, `str` → `Value::String`,
/// `list` → `Value::Array`, `dict`/object → `Value::Object`). Numbers and
/// booleans fall through to the `str(content)` fallback and are stringified
/// via `Value::to_string` (Python would produce `str(42)` / `str(True)` →
/// Rust gives `"42"` / `"true"`).
pub fn flatten_message_text_with_sep(content: &Value, sep: &str) -> String {
    // Mirrors `if content is None: return ""` (lines 36-37)
    if content.is_null() {
        return String::new();
    }
    // Mirrors `if isinstance(content, str): return content` (lines 38-39)
    if let Value::String(s) = content {
        return s.clone();
    }
    // Mirrors `if isinstance(content, list): chunks = [...]; return sep.join(chunk for chunk in chunks if chunk)` (lines 40-42)
    if let Value::Array(arr) = content {
        let chunks: Vec<String> = arr.iter().map(text_from_part).collect();
        // `sep.join(chunk for chunk in chunks if chunk)` — filter empty
        let non_empty: Vec<&str> = chunks.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect();
        return non_empty.join(sep);
    }

    // Mirrors `text = _text_from_part(content); if text: return text` (lines 44-46)
    let text = text_from_part(content);
    if !text.is_empty() {
        return text;
    }
    // Mirrors `try: return str(content); except Exception: return ""` (lines 47-50)
    // `serde_json::Value::to_string` never throws; for `Null`/`String`/`Array`
    // we already returned, so this is `Object`/`Number`/`Bool` fallback.
    // For `Value::String` we handled above; for `Object` this is JSON encoding
    // (Python would give `"{'k': 'v'}"`; JSON is the closest lossless form in Rust).
    // To keep the `str(...)` shape for objects, prefer `to_string` which is infallible.
    // Return empty only if we somehow cannot stringify (treated as infallible here).
    content.to_string()
}

/// Default-separator overload (`sep = "\n"`).
/// Mirrors `flatten_message_text(content)` with Python's default `sep="\n"` (line 34).
pub fn flatten_message_text(content: &Value) -> String {
    flatten_message_text_with_sep(content, "\n")
}

#[allow(dead_code)]
fn _flatten_message_text(content: &Value) -> String {
    flatten_message_text(content)
}

/// Convenience for `Option<&Value>` where `None` mirrors Python `content=None`.
/// Mirrors `if content is None: return ""` without requiring `Value::Null`.
pub fn flatten_message_text_opt(content: Option<&Value>, sep: &str) -> String {
    match content {
        None => String::new(),
        Some(v) => flatten_message_text_with_sep(v, sep),
    }
}

/// Convenience for `Option<&Value>` with default `sep="\n"`.
pub fn flatten_message_text_opt_default(content: Option<&Value>) -> String {
    flatten_message_text_opt(content, "\n")
}

/// Convenience for plain `&str` — mirrors `isinstance(content, str)` direct path.
pub fn flatten_message_text_str(content: &str, _sep: &str) -> String {
    content.to_string()
}

/// Convenience for `Option<&str>` where `None` → `""`.
pub fn flatten_message_text_str_opt(content: Option<&str>, _sep: &str) -> String {
    content.unwrap_or("").to_string()
}
