//! Gateway session stall notification policy (#72016 item 2).
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/session_stall.py` (121 LOC).
//! Consumes the shared activity observation contract from
//! ``agent.session_activity`` / ``AIAgent.get_activity_summary()``
//! (#72039) as the **single progress source**. This module owns only the
//! notify-once policy for "pending inbound + stale progress"; it does not
//! invent a parallel progress clock from turn-start or inbound event
//! timestamps.
//!
//! Python source docstring (preserved):
//! ```text
//! Gateway session stall notification policy (#72016 item 2).
//!
//! Consumes the shared activity observation contract from
//! ``agent.session_activity`` / ``AIAgent.get_activity_summary()``
//! (#72039) as the **single progress source**. This module owns only the
//! notify-once policy for "pending inbound + stale progress"; it does not
//! invent a parallel progress clock from turn-start or inbound event
//! timestamps.
//!
//! Boundaries (keep separate):
//! - ``gateway/shutdown_watchdog.py`` — process / event-loop liveness
//! - ``gateway/delivery_ledger.py`` — outbound delivery obligations
//! - Pending inbound here is a stall *policy gate* (queued follow-up exists),
//!   not an outbound obligation and not a progress timestamp.
//!
//! Notification / timeout / kill / retry policy stay in their own components;
//! the shared contract remains observation-only (timestamp + bounded
//! description + provenance).
//! ```
//!
//! Mapping:
//! - `should_emit_session_stall_notification` → [`should_emit_session_stall_notification`]
//! - `should_clear_session_stall_notification` → [`should_clear_session_stall_notification`]
//! - `format_session_stall_notification` → [`format_session_stall_notification`]
//! - `resolve_session_idle_seconds_from_activity` → [`resolve_session_idle_seconds_from_activity`]
//! - `activity.get("seconds_since_activity")` / `last_activity_at` / `last_activity_ts` → serde_json Value lookups with `value_to_f64`
//! - `math.isfinite` → `f64::is_finite`
//! - `time.time()` fallback → `SystemTime::now().duration_since(UNIX_EPOCH)`

use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Internal helpers — mirrors Python helpers / float coercion
// ---------------------------------------------------------------------------

/// Coerce a `serde_json::Value` to `f64` mirroring Python's `float(value)`.
///
/// Handles numbers, numeric strings (trimmed), and booleans. Returns `None` on
/// `Null`, non-numeric strings, arrays, objects, or parse failures — mirrors
/// Python's `except (TypeError, ValueError): idle = None`.
fn value_to_f64(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Null => None,
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return None;
            }
            trimmed.parse::<f64>().ok()
        }
        serde_json::Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Public API — mirrors Python top-level functions
// ---------------------------------------------------------------------------

/// Return `true` when a stall warning should be sent for this session.
///
/// Mirrors:
/// ```python
/// def should_emit_session_stall_notification(
///     *,
///     timeout_seconds: float,
///     idle_seconds: Optional[float],
///     has_pending_inbound: bool,
///     already_notified: bool,
/// ) -> bool:
///     if timeout_seconds <= 0:
///         return False
///     if not has_pending_inbound:
///         return False
///     if already_notified:
///         return False
///     if idle_seconds is None:
///         return False
///     return idle_seconds >= timeout_seconds
/// ```
pub fn should_emit_session_stall_notification(
    timeout_seconds: f64,
    idle_seconds: Option<f64>,
    has_pending_inbound: bool,
    already_notified: bool,
) -> bool {
    if timeout_seconds <= 0.0 {
        return false;
    }
    if !has_pending_inbound {
        return false;
    }
    if already_notified {
        return false;
    }
    let Some(idle) = idle_seconds else {
        return false;
    };
    idle >= timeout_seconds
}

/// Return `true` when a prior stall notice may be cleared (episode ended).
///
/// Mirrors:
/// ```python
/// def should_clear_session_stall_notification(
///     *,
///     timeout_seconds: float,
///     idle_seconds: Optional[float],
///     has_pending_inbound: bool,
/// ) -> bool:
///     if not has_pending_inbound:
///         return True
///     if timeout_seconds <= 0:
///         return True
///     # Unknown progress: hold the latch. Do not treat observation gaps as recovery.
///     if idle_seconds is None:
///         return False
///     return idle_seconds < timeout_seconds
/// ```
pub fn should_clear_session_stall_notification(
    timeout_seconds: f64,
    idle_seconds: Option<f64>,
    has_pending_inbound: bool,
) -> bool {
    if !has_pending_inbound {
        return true;
    }
    if timeout_seconds <= 0.0 {
        return true;
    }
    // Unknown progress: hold the latch. Do not treat observation gaps as recovery.
    let Some(idle) = idle_seconds else {
        return false;
    };
    idle < timeout_seconds
}

/// User-facing stall warning (ASCII minutes; matches issue #72016 copy).
///
/// Mirrors:
/// ```python
/// def format_session_stall_notification(idle_seconds: float) -> str:
///     mins = max(1, int(idle_seconds // 60))
///     return (
///         f"⚠️ Agent session appears stalled (last activity {mins} min ago). "
///         f"Try /new to reset."
///     )
/// ```
pub fn format_session_stall_notification(idle_seconds: f64) -> String {
    let mins = std::cmp::max(1, (idle_seconds.div_euclid(60.0)) as i64);
    format!(
        "⚠️ Agent session appears stalled (last activity {} min ago). Try /new to reset.",
        mins
    )
}

/// Idle seconds from a shared activity snapshot only (#72039 contract).
///
/// Prefers `seconds_since_activity` when present and finite; otherwise
/// derives from `last_activity_at` / `last_activity_ts`. Returns `None`
/// when there is no usable progress timestamp — callers must not fall
/// back to turn-start or pending-inbound clocks.
///
/// Mirrors:
/// ```python
/// def resolve_session_idle_seconds_from_activity(
///     activity: Optional[Mapping[str, Any]],
///     *,
///     now: Optional[float] = None,
/// ) -> Optional[float]:
///     if not activity:
///         return None
///     elapsed = activity.get("seconds_since_activity")
///     if elapsed is not None:
///         try:
///             idle = float(elapsed)
///         except (TypeError, ValueError):
///             idle = None
///         else:
///             if math.isfinite(idle):
///                 if idle < 0:
///                     return 0.0
///                 return idle
///             # Non-finite: fall through to last_activity_at / last_activity_ts
///     ts = activity.get("last_activity_at")
///     if ts is None:
///         ts = activity.get("last_activity_ts")
///     if ts is None:
///         return None
///     try:
///         when = float(ts)
///     except (TypeError, ValueError):
///         return None
///     if not math.isfinite(when):
///         return None
///     if now is None:
///         import time as _time
///         clock = float(_time.time())
///     else:
///         clock = float(now)
///     idle = clock - when
///     if idle < 0:
///         return 0.0
///     return idle
/// ```
pub fn resolve_session_idle_seconds_from_activity(
    activity: Option<&serde_json::Value>,
    now: Option<f64>,
) -> Option<f64> {
    let activity = activity?;
    // `if not activity: return None` — None already handled, empty mapping is falsy
    let obj = activity.as_object()?;
    if obj.is_empty() {
        return None;
    }

    // Prefer `seconds_since_activity` when present and finite
    if let Some(elapsed_val) = obj.get("seconds_since_activity") {
        if !elapsed_val.is_null() {
            if let Some(idle) = value_to_f64(elapsed_val) {
                if idle.is_finite() {
                    if idle < 0.0 {
                        return Some(0.0);
                    }
                    return Some(idle);
                }
                // Non-finite: fall through to last_activity_at / last_activity_ts
            }
            // TypeError/ValueError (value_to_f64 == None): fall through as well
        }
    }

    // Derive from `last_activity_at` / `last_activity_ts`
    let ts_val = obj
        .get("last_activity_at")
        .filter(|v| !v.is_null())
        .or_else(|| obj.get("last_activity_ts").filter(|v| !v.is_null()))?;
    let when = value_to_f64(ts_val)?;
    if !when.is_finite() {
        return None;
    }

    let clock = match now {
        Some(n) => n,
        None => SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_secs_f64(),
    };
    let idle = clock - when;
    if idle < 0.0 {
        return Some(0.0);
    }
    Some(idle)
}

/// Convenience wrapper when activity is borrowed as a map.
///
/// Mirrors the same contract but accepts `serde_json::Map` directly for
/// callers that already hold the object without the outer `Value` wrapper.
pub fn resolve_session_idle_seconds_from_activity_map(
    activity: Option<&serde_json::Map<String, serde_json::Value>>,
    now: Option<f64>,
) -> Option<f64> {
    let map = activity?;
    if map.is_empty() {
        return None;
    }
    // Wrap as Value to reuse the main path without duplicating logic
    let value = serde_json::Value::Object(map.clone());
    resolve_session_idle_seconds_from_activity(Some(&value), now)
}

// Provide private aliases mirroring Python's underscore-prefixed helpers for traceability
#[allow(dead_code)]
fn _value_to_f64(v: &serde_json::Value) -> Option<f64> {
    value_to_f64(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn emit_true_when_stale_and_pending() {
        assert!(should_emit_session_stall_notification(60.0, Some(60.0), true, false));
        assert!(should_emit_session_stall_notification(60.0, Some(120.0), true, false));
    }

    #[test]
    fn emit_false_cases() {
        // timeout <=0
        assert!(!should_emit_session_stall_notification(0.0, Some(100.0), true, false));
        assert!(!should_emit_session_stall_notification(-1.0, Some(100.0), true, false));
        // no pending
        assert!(!should_emit_session_stall_notification(60.0, Some(100.0), false, false));
        // already notified
        assert!(!should_emit_session_stall_notification(60.0, Some(100.0), true, true));
        // idle None
        assert!(!should_emit_session_stall_notification(60.0, None, true, false));
        // idle < timeout
        assert!(!should_emit_session_stall_notification(60.0, Some(59.9), true, false));
    }

    #[test]
    fn clear_when_no_pending_or_timeout_zero() {
        assert!(should_clear_session_stall_notification(60.0, Some(100.0), false));
        assert!(should_clear_session_stall_notification(60.0, None, false));
        assert!(should_clear_session_stall_notification(0.0, Some(100.0), true));
        assert!(should_clear_session_stall_notification(-5.0, None, true));
    }

    #[test]
    fn clear_holds_on_unknown_progress() {
        // Unknown progress: hold the latch
        assert!(!should_clear_session_stall_notification(60.0, None, true));
    }

    #[test]
    fn clear_when_recovered() {
        assert!(should_clear_session_stall_notification(60.0, Some(30.0), true));
        assert!(!should_clear_session_stall_notification(60.0, Some(60.0), true));
        assert!(!should_clear_session_stall_notification(60.0, Some(100.0), true));
    }

    #[test]
    fn format_mins() {
        assert_eq!(
            format_session_stall_notification(30.0),
            "⚠️ Agent session appears stalled (last activity 1 min ago). Try /new to reset."
        );
        assert_eq!(
            format_session_stall_notification(60.0),
            "⚠️ Agent session appears stalled (last activity 1 min ago). Try /new to reset."
        );
        assert_eq!(
            format_session_stall_notification(119.0),
            "⚠️ Agent session appears stalled (last activity 1 min ago). Try /new to reset."
        );
        assert_eq!(
            format_session_stall_notification(120.0),
            "⚠️ Agent session appears stalled (last activity 2 min ago). Try /new to reset."
        );
        assert_eq!(
            format_session_stall_notification(3600.0),
            "⚠️ Agent session appears stalled (last activity 60 min ago). Try /new to reset."
        );
    }

    #[test]
    fn resolve_prefers_seconds_since_activity() {
        let activity = json!({"seconds_since_activity": 42.5, "last_activity_at": 1000.0});
        assert_eq!(
            resolve_session_idle_seconds_from_activity(Some(&activity), Some(2000.0)),
            Some(42.5)
        );
    }

    #[test]
    fn resolve_negative_elapsed_clamps_to_zero() {
        let activity = json!({"seconds_since_activity": -5.0});
        assert_eq!(
            resolve_session_idle_seconds_from_activity(Some(&activity), Some(2000.0)),
            Some(0.0)
        );
        // string negative
        let activity2 = json!({"seconds_since_activity": "-10"});
        assert_eq!(
            resolve_session_idle_seconds_from_activity(Some(&activity2), Some(2000.0)),
            Some(0.0)
        );
    }

    #[test]
    fn resolve_non_finite_elapsed_falls_through() {
        // inf should fall through to last_activity_at
        let activity = json!({"seconds_since_activity": "inf", "last_activity_at": 1000.0});
        // now 2000 -> idle 1000
        assert_eq!(
            resolve_session_idle_seconds_from_activity(Some(&activity), Some(2000.0)),
            Some(1000.0)
        );
        // nan
        let activity2 = json!({"seconds_since_activity": "NaN", "last_activity_at": 1000.0});
        assert_eq!(
            resolve_session_idle_seconds_from_activity(Some(&activity2), Some(2000.0)),
            Some(1000.0)
        );
    }

    #[test]
    fn resolve_from_last_activity_at() {
        let activity = json!({"last_activity_at": 1000.0});
        assert_eq!(
            resolve_session_idle_seconds_from_activity(Some(&activity), Some(1500.0)),
            Some(500.0)
        );
    }

    #[test]
    fn resolve_fallback_last_activity_ts() {
        let activity = json!({"last_activity_ts": 1000.0});
        assert_eq!(
            resolve_session_idle_seconds_from_activity(Some(&activity), Some(1500.0)),
            Some(500.0)
        );
        // last_activity_at takes precedence
        let activity2 = json!({"last_activity_at": 1000.0, "last_activity_ts": 500.0});
        assert_eq!(
            resolve_session_idle_seconds_from_activity(Some(&activity2), Some(1500.0)),
            Some(500.0)
        );
    }

    #[test]
    fn resolve_negative_idle_clamps() {
        let activity = json!({"last_activity_at": 2000.0});
        // clock 1500 < when 2000 => 0.0
        assert_eq!(
            resolve_session_idle_seconds_from_activity(Some(&activity), Some(1500.0)),
            Some(0.0)
        );
    }

    #[test]
    fn resolve_none_cases() {
        assert_eq!(resolve_session_idle_seconds_from_activity(None, Some(1000.0)), None);
        assert_eq!(
            resolve_session_idle_seconds_from_activity(Some(&json!({})), Some(1000.0)),
            None
        );
        assert_eq!(
            resolve_session_idle_seconds_from_activity(Some(&json!({"other": 1})), Some(1000.0)),
            None
        );
        // non-finite ts
        assert_eq!(
            resolve_session_idle_seconds_from_activity(Some(&json!({"last_activity_at": "inf"})), Some(1000.0)),
            None
        );
        // unparseable ts
        assert_eq!(
            resolve_session_idle_seconds_from_activity(Some(&json!({"last_activity_at": "bad"})), Some(1000.0)),
            None
        );
        // null ts value treated as missing
        assert_eq!(
            resolve_session_idle_seconds_from_activity(Some(&json!({"last_activity_at": null})), Some(1000.0)),
            None
        );
    }

    #[test]
    fn resolve_string_numeric_elapsed() {
        let activity = json!({"seconds_since_activity": "42.5"});
        assert_eq!(
            resolve_session_idle_seconds_from_activity(Some(&activity), Some(2000.0)),
            Some(42.5)
        );
    }

    #[test]
    fn resolve_map_variant() {
        let mut map = serde_json::Map::new();
        map.insert("seconds_since_activity".to_string(), json!(10.0));
        assert_eq!(
            resolve_session_idle_seconds_from_activity_map(Some(&map), Some(100.0)),
            Some(10.0)
        );
    }
}
