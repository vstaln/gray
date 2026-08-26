//! Internal metadata attached to durable conversation messages.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/message_metadata.py` (41 lines).
//!
//! Python source docstring (preserved):
//! ```text
//! Internal metadata attached to durable conversation messages.
//! ```
//!
//! Python source (preserved verbatim, lines 1-41):
//! ```python
//! """Internal metadata attached to durable conversation messages."""
//!
//! from __future__ import annotations
//!
//! from time import time as wall_time
//! from typing import Any, MutableMapping, Optional, TypeVar
//!
//!
//! # These fields describe Hermes' durable record, not provider-visible message
//! # content. They must not influence context-pressure decisions.
//! PERSISTENCE_ONLY_MESSAGE_FIELDS = frozenset({"timestamp"})
//!
//! _Message = TypeVar("_Message", bound=MutableMapping[str, Any])
//!
//!
//! def stamp_message_timestamp(
//!     message: _Message,
//!     *,
//!     timestamp: Optional[float] = None,
//! ) -> _Message:
//!     """Attach a creation timestamp without replacing source-provided time.
//!
//!     Gateway adapters can supply the platform event time. All other callers use
//!     the local wall clock at the point the message enters the live transcript.
//!     Returning the same mapping keeps the helper convenient at append sites.
//!     """
//!     if message.get("timestamp") is None:
//!         message["timestamp"] = wall_time() if timestamp is None else timestamp
//!     return message
//!
//!
//! def append_message(
//!     messages: list[Any],
//!     message: _Message,
//!     *,
//!     timestamp: Optional[float] = None,
//! ) -> _Message:
//!     """Stamp and append one live transcript message."""
//!     stamp_message_timestamp(message, timestamp=timestamp)
//!     messages.append(message)
//!     return message
//! ```
//!
//! Rust notes:
//! - `MutableMapping[str, Any]` → `serde_json::Value::Object` (`Map<String, Value>`).
//!   The Python `message.get("timestamp") is None` guard maps to: missing key OR
//!   `Value::Null` → needs stamping; any other JSON value (including `0`, `0.0`,
//!   `false`, `""`) is treated as source-provided and preserved, matching `is None`.
//! - `time.time as wall_time` → `SystemTime::now().duration_since(UNIX_EPOCH).as_secs_f64()`.
//!   Both are wall-clock seconds since epoch as `f64`.
//! - `_Message` TypeVar (bound `MutableMapping`) → `&mut Value` for the
//!   in-place variant and `Value` (owned) for the move variant; callers holding
//!   a typed `Map<String, Value>` can use the `*_map` helpers.
//! - `messages: list[Any]` → `&mut Vec<Value>` (live transcript tail).

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

// ---------------------------------------------------------------------------
// Constants — mirrors lines 9-11
// ---------------------------------------------------------------------------

/// Fields that describe Hermes' durable record, not provider-visible content.
///
/// Must not influence context-pressure decisions.
///
/// Mirrors `PERSISTENCE_ONLY_MESSAGE_FIELDS = frozenset({"timestamp"})` (line 11).
pub const PERSISTENCE_ONLY_MESSAGE_FIELDS: &[&str] = &["timestamp"];

// Keep underscore-prefixed alias for 1:1 traceability with Python private name.
#[allow(dead_code)]
const _PERSISTENCE_ONLY_MESSAGE_FIELDS: &[&str] = PERSISTENCE_ONLY_MESSAGE_FIELDS;

/// Returns `true` if `field` is persistence-only (i.e. excluded from context-pressure).
///
/// Mirrors `field in PERSISTENCE_ONLY_MESSAGE_FIELDS` (line 11).
#[inline]
pub fn is_persistence_only_field(field: &str) -> bool {
    PERSISTENCE_ONLY_MESSAGE_FIELDS.contains(&field)
}

// ---------------------------------------------------------------------------
// wall_time — mirrors `from time import time as wall_time` (line 5)
// ---------------------------------------------------------------------------

/// Wall-clock seconds since Unix epoch as `f64`.
///
/// Mirrors `wall_time()` / `time.time()` (line 5, used on line 28).
#[inline]
fn wall_time() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[allow(dead_code)]
fn _wall_time() -> f64 {
    wall_time()
}

// ---------------------------------------------------------------------------
// stamp_message_timestamp — mirrors lines 16-29
// ---------------------------------------------------------------------------

/// Attach a creation timestamp without replacing source-provided time.
///
/// Gateway adapters can supply the platform event time. All other callers use
/// the local wall clock at the point the message enters the live transcript.
/// Returning the same mapping keeps the helper convenient at append sites.
///
/// Mirrors `stamp_message_timestamp(message, *, timestamp=None)` (lines 16-29):
/// ```python
/// def stamp_message_timestamp(
///     message: _Message,
///     *,
///     timestamp: Optional[float] = None,
/// ) -> _Message:
///     """Attach a creation timestamp without replacing source-provided time.
///     Gateway adapters can supply the platform event time. All other callers use
///     the local wall clock at the point the message enters the live transcript.
///     Returning the same mapping keeps the helper convenient at append sites.
///     """
///     if message.get("timestamp") is None:
///         message["timestamp"] = wall_time() if timestamp is None else timestamp
///     return message
/// ```
///
/// `message` is `&mut Value` expecting `Value::Object`. Non-object values are
/// left untouched (no dict to stamp). `timestamp` is `Some(f64)` for an
/// explicit platform time, or `None` to use [`wall_time()`].
#[inline]
pub fn stamp_message_timestamp(message: &mut Value, timestamp: Option<f64>) -> &mut Value {
    // Mirrors `if message.get("timestamp") is None:` (line 27)
    // In Python `dict.get` returns `None` for missing; we treat missing OR `Null` as `is None`.
    let needs_stamp = match message {
        Value::Object(map) => match map.get("timestamp") {
            None => true,
            Some(Value::Null) => true,
            _ => false,
        },
        _ => false,
    };
    if needs_stamp {
        // Mirrors `message["timestamp"] = wall_time() if timestamp is None else timestamp` (line 28)
        let ts = timestamp.unwrap_or_else(wall_time);
        // `json!` would coerce; direct `Value::from(f64)` preserves the float shape.
        // `serde_json::Number::from_f64` returns `None` for NaN/inf (not expected from wall_time),
        // fall back to `Value::Null` in that degenerate case.
        let ts_value = serde_json::Number::from_f64(ts)
            .map(Value::Number)
            .unwrap_or(Value::Null);
        if let Value::Object(map) = message {
            map.insert("timestamp".to_string(), ts_value);
        }
    }
    // Mirrors `return message` (line 29) — return same mutable reference.
    message
}

#[allow(dead_code)]
fn _stamp_message_timestamp(message: &mut Value, timestamp: Option<f64>) -> &mut Value {
    stamp_message_timestamp(message, timestamp)
}

/// Map-typed overload for callers holding `serde_json::Map<String, Value>` directly.
///
/// Mirrors same logic as [`stamp_message_timestamp`] (lines 27-28) without the
/// `Value::Object` dispatch.
#[inline]
pub fn stamp_message_timestamp_map(
    message: &mut serde_json::Map<String, Value>,
    timestamp: Option<f64>,
) -> &mut serde_json::Map<String, Value> {
    // Mirrors `if message.get("timestamp") is None:` (line 27)
    let needs_stamp = match message.get("timestamp") {
        None => true,
        Some(Value::Null) => true,
        _ => false,
    };
    if needs_stamp {
        let ts = timestamp.unwrap_or_else(wall_time);
        let ts_value = serde_json::Number::from_f64(ts)
            .map(Value::Number)
            .unwrap_or(Value::Null);
        message.insert("timestamp".to_string(), ts_value);
    }
    message
}

#[allow(dead_code)]
fn _stamp_message_timestamp_map(
    message: &mut serde_json::Map<String, Value>,
    timestamp: Option<f64>,
) -> &mut serde_json::Map<String, Value> {
    stamp_message_timestamp_map(message, timestamp)
}

// ---------------------------------------------------------------------------
// append_message — mirrors lines 32-41
// ---------------------------------------------------------------------------

/// Stamp and append one live transcript message (owned-value variant).
///
/// Mirrors `append_message(messages, message, *, timestamp=None)` (lines 32-41):
/// ```python
/// def append_message(
///     messages: list[Any],
///     message: _Message,
///     *,
///     timestamp: Optional[float] = None,
/// ) -> _Message:
///     """Stamp and append one live transcript message."""
///     stamp_message_timestamp(message, timestamp=timestamp)
///     messages.append(message)
///     return message
/// ```
///
/// Takes `message` by value, stamps it in place, pushes a clone into
/// `messages`, and returns the stamped owned value (mirrors Python returning
/// the same mapping object; `Vec` holds an owned clone).
pub fn append_message(
    messages: &mut Vec<Value>,
    mut message: Value,
    timestamp: Option<f64>,
) -> Value {
    // Mirrors `stamp_message_timestamp(message, timestamp=timestamp)` (line 39)
    stamp_message_timestamp(&mut message, timestamp);
    // Mirrors `messages.append(message)` (line 40) — clone for Vec ownership, return owned stamped value
    messages.push(message.clone());
    // Mirrors `return message` (line 41)
    message
}

#[allow(dead_code)]
fn _append_message(
    messages: &mut Vec<Value>,
    message: Value,
    timestamp: Option<f64>,
) -> Value {
    append_message(messages, message, timestamp)
}

/// Stamp and append one live transcript message (in-place reference variant).
///
/// Mutates `message` in place (matching Python's `MutableMapping` mutation),
/// pushes a clone into `messages`, and returns the same mutable reference for
/// chaining at append sites. This is the closest 1:1 to Python's reference
/// identity (`messages[-1] is message` in Python; `messages.last() == message`
/// clone in Rust).
pub fn append_message_ref<'a>(
    messages: &mut Vec<Value>,
    message: &'a mut Value,
    timestamp: Option<f64>,
) -> &'a mut Value {
    // Mirrors `stamp_message_timestamp(message, timestamp=timestamp)` (line 39)
    stamp_message_timestamp(message, timestamp);
    // Mirrors `messages.append(message)` (line 40)
    messages.push(message.clone());
    // Mirrors `return message` (line 41)
    message
}

#[allow(dead_code)]
fn _append_message_ref<'a>(
    messages: &mut Vec<Value>,
    message: &'a mut Value,
    timestamp: Option<f64>,
) -> &'a mut Value {
    append_message_ref(messages, message, timestamp)
}

/// Map-typed overload for callers holding `Map<String, Value>` messages and a
/// `Vec<Value>` transcript.
pub fn append_message_map(messages: &mut Vec<Value>, mut message: serde_json::Map<String, Value>, timestamp: Option<f64>) -> Value {
    stamp_message_timestamp_map(&mut message, timestamp);
    let stamped = Value::Object(message);
    messages.push(stamped.clone());
    stamped
}
