//! shell/tools/sleep.rs — deliberate wait that ends early on wake events (brief 3B).
//!
//! Lets the model wait without holding a process or spending turns. Wakes
//! early on this session's task exits / pattern matches, on any user input,
//! or on cancel. Never `is_error`. (Registration in the plugin builder,
//! the TUI countdown, and keystroke → `notify_user_input` are follow-ups
//! outside this brief — see P3B-report.md.)

use std::time::{Duration, Instant};

use async_trait::async_trait;
use gray_core::agent::{Tool, ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;

use crate::shell::contract::{MAX_TIMEOUT_SECS, TaskId, WakeEvent};
use crate::shell::registry::registry;
use crate::shell::view::format_elapsed;
use crate::{fail, get_opt_u64};

pub const SLEEP_SNIPPET: &str =
    "Wait without polling: sleep(seconds) ends early when a task exits or the user types.";

/// Longest single sleep (brief 3B non-goal: beyond 600 s is `schedule_task`).
pub const MAX_SLEEP_SECS: u64 = MAX_TIMEOUT_SECS;

pub struct SleepTool;

#[async_trait]
impl Tool for SleepTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "sleep",
            "Wait up to `seconds` for background tasks, waking early when one exits, \
             matches notify_on, or the user types. Use instead of polling with shell_output.",
            json!({
                "type": "object",
                "properties": {
                    "seconds": {"type": "integer", "description": "Seconds to wait (1..=600, required)"},
                    "reason": {"type": "string", "description": "Why you are waiting (shown in the UI)"}
                },
                "required": ["seconds"]
            }),
        )
    }

    fn prompt_snippet(&self) -> Option<&'static str> {
        Some(SLEEP_SNIPPET)
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let (secs, _reason) = match parse_sleep_args(&args) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let session = ctx.session_id.clone().unwrap_or_else(|| "nosession".into());
        // Subscribe BEFORE observing any state, or an exit in between is lost.
        let mut rx = registry().wake_tx().subscribe();
        let start = Instant::now();
        let total = Duration::from_secs(secs);
        loop {
            let remaining = total.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                return ToolOutput::ok(format!("slept {secs}s · no events"));
            }
            let el = || format_elapsed(start.elapsed());
            tokio::select! {
                _ = tokio::time::sleep(remaining) => {
                    return ToolOutput::ok(format!("slept {secs}s · no events"));
                }
                ev = rx.recv() => match ev {
                    Ok(WakeEvent::Exited { id, report }) if ours(&session, id) => {
                        return ToolOutput::ok(format!(
                            "slept {} of {secs}s · woken early: {id} {}",
                            el(), report.label
                        ));
                    }
                    Ok(WakeEvent::PatternMatched { id, line }) if ours(&session, id) => {
                        return ToolOutput::ok(format!(
                            "slept {} of {secs}s · woken early: {id} matched \"{line}\"",
                            el()
                        ));
                    }
                    Ok(WakeEvent::UserInput) => {
                        return ToolOutput::ok(format!(
                            "slept {} of {secs}s · woken early: user typed",
                            el()
                        ));
                    }
                    Ok(_) => {} // another session's task — keep sleeping
                    Err(RecvError::Lagged(n)) => {
                        return ToolOutput::ok(format!(
                            "slept {} of {secs}s · woken early: {n} task events were dropped; shell_output() to list",
                            el()
                        ));
                    }
                    // The registry holds a sender forever; Closed means teardown.
                    Err(RecvError::Closed) => {
                        return ToolOutput::ok(format!(
                            "slept {} of {secs}s · woken early: event channel closed",
                            el()
                        ));
                    }
                },
                _ = ctx.cancel.cancelled() => {
                    return ToolOutput::ok(format!("sleep cancelled after {}", el()));
                }
            }
        }
    }
}

/// `WakeEvent` carries no session, and ids are per-session (`t1` exists in
/// every session), so an event is "ours" when the id is known in this
/// session. A gc'd task reads as foreign — the sleep just isn't cut short.
fn ours(session: &str, id: TaskId) -> bool {
    registry().get(session, id).is_some()
}

/// Pure arg parsing (kept separate so tests don't need a runtime).
/// Local `reason` reader until 4A centralizes arg parsing (same as 2C's).
fn parse_sleep_args(args: &Value) -> Result<(u64, Option<String>), ToolOutput> {
    let secs = match get_opt_u64(args, "seconds") {
        Ok(Some(s)) => s,
        Ok(None) => {
            return Err(fail(
                "missing required argument 'seconds' (1..=600)".to_string(),
            ));
        }
        Err(e) => return Err(e),
    };
    if !(1..=MAX_SLEEP_SECS).contains(&secs) {
        return Err(fail(format!(
            "invalid argument 'seconds': expected 1..={MAX_SLEEP_SECS}, got {secs}"
        )));
    }
    let reason = match args.get("reason") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => {
            return Err(fail(
                "invalid argument 'reason': expected string".to_string(),
            ));
        }
    };
    Ok((secs, reason))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn seconds_required_in_range_reason_optional() {
        assert!(parse_sleep_args(&json!({})).is_err()); // missing
        assert!(parse_sleep_args(&json!({"seconds": 0})).is_err());
        assert!(parse_sleep_args(&json!({"seconds": 601})).is_err());
        assert!(parse_sleep_args(&json!({"seconds": "60"})).is_err()); // wrong type, no coercion here
        assert!(parse_sleep_args(&json!({"seconds": 1.5})).is_err());
        let (s, r) = parse_sleep_args(&json!({"seconds": 60})).unwrap();
        assert_eq!((s, r), (60, None));
        let (s, r) = parse_sleep_args(&json!({"seconds": 600, "reason": "build"})).unwrap();
        assert_eq!((s, r.as_deref()), (600, Some("build")));
        assert!(parse_sleep_args(&json!({"seconds": 5, "reason": 7})).is_err());
    }

    #[test]
    fn bash_guidelines_stay_short() {
        // 3D partial gate (full token test needs all four snippets incl.
        // shell_output's, which this brief doesn't own): ≤ 6 bullets.
        assert!(
            super::super::bash::BASH_GUIDELINES.len() <= 6,
            "guidelines ship on every request — cut, don't add"
        );
    }
}
