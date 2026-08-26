//! Trajectory saving utilities and static helpers.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/trajectory.py` (56 lines).
//!
//! `_convert_to_trajectory_format` stays as an `AIAgent` method (`batch_runner.py`
//! calls `agent._convert_to_trajectory_format`). Only the static helpers and
//! the file-write logic live here.
//!
//! Python source docstring (preserved):
//! ```text
//! Trajectory saving utilities and static helpers.
//!
//! _convert_to_trajectory_format stays as an AIAgent method (batch_runner.py
//! calls agent._convert_to_trajectory_format). Only the static helpers and
//! the file-write logic live here.
//! ```

use std::fs::OpenOptions;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// convert_scratchpad_to_think — mirrors lines 16-20
// ---------------------------------------------------------------------------

/// Convert `<REASONING_SCRATCHPAD>` tags to `<think>` tags.
///
/// Mirrors `convert_scratchpad_to_think` (lines 16-20):
/// ```python
/// def convert_scratchpad_to_think(content: str) -> str:
///     if not content or "<REASONING_SCRATCHPAD>" not in content:
///         return content
///     return content.replace("<REASONING_SCRATCHPAD>", "<think>").replace("</REASONING_SCRATCHPAD>", "</think>")
/// ```
pub fn convert_scratchpad_to_think(content: &str) -> String {
    // Mirrors `if not content or "<REASONING_SCRATCHPAD>" not in content: return content` (lines 18-19)
    if content.is_empty() || !content.contains("<REASONING_SCRATCHPAD>") {
        return content.to_string();
    }
    // Mirrors `return content.replace(...).replace(...)` (line 20)
    content
        .replace("<REASONING_SCRATCHPAD>", "<think>")
        .replace("</REASONING_SCRATCHPAD>", "</think>")
}

#[allow(dead_code)]
fn _convert_scratchpad_to_think(content: &str) -> String {
    convert_scratchpad_to_think(content)
}

// ---------------------------------------------------------------------------
// has_incomplete_scratchpad — mirrors lines 23-27
// ---------------------------------------------------------------------------

/// Check if content has an opening `<REASONING_SCRATCHPAD>` without a closing tag.
///
/// Mirrors `has_incomplete_scratchpad` (lines 23-27):
/// ```python
/// def has_incomplete_scratchpad(content: str) -> bool:
///     if not content:
///         return False
///     return "<REASONING_SCRATCHPAD>" in content and "</REASONING_SCRATCHPAD>" not in content
/// ```
pub fn has_incomplete_scratchpad(content: &str) -> bool {
    // Mirrors `if not content: return False` (lines 25-26)
    if content.is_empty() {
        return false;
    }
    // Mirrors `return "<REASONING_SCRATCHPAD>" in content and "</REASONING_SCRATCHPAD>" not in content` (line 27)
    content.contains("<REASONING_SCRATCHPAD>") && !content.contains("</REASONING_SCRATCHPAD>")
}

#[allow(dead_code)]
fn _has_incomplete_scratchpad(content: &str) -> bool {
    has_incomplete_scratchpad(content)
}

// ---------------------------------------------------------------------------
// save_trajectory — mirrors lines 30-56
// ---------------------------------------------------------------------------

/// Default filename for successful trajectories.
/// Mirrors `"trajectory_samples.jsonl"` branch on line 42.
pub const DEFAULT_TRAJECTORY_FILENAME: &str = "trajectory_samples.jsonl";

/// Default filename for failed trajectories.
/// Mirrors `"failed_trajectories.jsonl"` branch on line 42.
pub const DEFAULT_FAILED_FILENAME: &str = "failed_trajectories.jsonl";

/// Resolve the default filename for `completed`.
/// Mirrors `filename = "trajectory_samples.jsonl" if completed else "failed_trajectories.jsonl"` (line 42).
pub fn default_trajectory_filename(completed: bool) -> &'static str {
    if completed {
        DEFAULT_TRAJECTORY_FILENAME
    } else {
        DEFAULT_FAILED_FILENAME
    }
}

#[allow(dead_code)]
fn _default_trajectory_filename(completed: bool) -> &'static str {
    default_trajectory_filename(completed)
}

/// Append a trajectory entry to a JSONL file.
///
/// Mirrors `save_trajectory` (lines 30-56):
/// ```python
/// def save_trajectory(trajectory: List[Dict[str, Any]], model: str,
///                     completed: bool, filename: str = None):
///     if filename is None:
///         filename = "trajectory_samples.jsonl" if completed else "failed_trajectories.jsonl"
///     entry = {
///         "conversations": trajectory,
///         "timestamp": datetime.now().isoformat(),
///         "model": model,
///         "completed": completed,
///     }
///     try:
///         with open(filename, "a", encoding="utf-8") as f:
///             f.write(json.dumps(entry, ensure_ascii=False) + "\n")
///         logger.info("Trajectory saved to %s", filename)
///     except Exception as e:
///         logger.warning("Failed to save trajectory: %s", e)
/// ```
///
/// `trajectory` is the ShareGPT-format conversation list (`List[Dict[str, Any]]`
/// in Python). In Rust it is any `Serialize` value — typically `&[Value]` or
/// `&Value` where `Value` is `serde_json::Value`. Using `impl Serialize`
/// preserves the 1:1 call shape while staying idiomatic for callers holding
/// either a typed slice or a dynamic JSON array.
///
/// `filename` overrides the output file. `None` mirrors Python's `filename=None`
/// default branch (line 41-42).
pub fn save_trajectory<S: Serialize>(
    trajectory: &S,
    model: &str,
    completed: bool,
    filename: Option<&str>,
) {
    // Mirrors `if filename is None: filename = "trajectory_samples.jsonl" if completed else "failed_trajectories.jsonl"` (lines 41-42)
    let filename = filename
        .map(|s| s.to_string())
        .unwrap_or_else(|| default_trajectory_filename(completed).to_string());

    // Mirrors entry construction (lines 44-49)
    let entry = json!({
        "conversations": trajectory,
        "timestamp": now_iso(),
        "model": model,
        "completed": completed,
    });

    // Mirrors try/except around file write (lines 51-56)
    let serialized = match serde_json::to_string(&entry) {
        Ok(s) => s,
        Err(e) => {
            // Mirrors `logger.warning("Failed to save trajectory: %s", e)` (line 56)
            log::warn!("Failed to save trajectory: {}", e);
            return;
        }
    };

    // Mirrors `with open(filename, "a", encoding="utf-8") as f: f.write(json.dumps(entry, ensure_ascii=False) + "\n")`
    // `ensure_ascii=False` in Python writes UTF-8 as-is; `serde_json::to_string` does the same
    // (only control chars are escaped, non-ASCII is emitted as UTF-8).
    match OpenOptions::new().create(true).append(true).open(&filename) {
        Ok(mut file) => {
            // Mirrors `f.write(json.dumps(entry, ensure_ascii=False) + "\n")` (line 53)
            if let Err(e) = writeln!(file, "{}", serialized) {
                log::warn!("Failed to save trajectory: {}", e);
            } else {
                // Mirrors `logger.info("Trajectory saved to %s", filename)` (line 54)
                log::info!("Trajectory saved to {}", filename);
            }
        }
        Err(e) => {
            log::warn!("Failed to save trajectory: {}", e);
        }
    }
}

/// Convenience overload accepting `serde_json::Value` directly (the most
/// literal 1:1 for `List[Dict[str, Any]]` encoded as `Value::Array`).
/// Delegates to the generic [`save_trajectory`].
pub fn save_trajectory_value(trajectory: &Value, model: &str, completed: bool, filename: Option<&str>) {
    save_trajectory(trajectory, model, completed, filename)
}

/// Convenience overload accepting a slice of `Value` (`&[Value]`),
/// matching `List[Dict[str, Any]]` most idiomatically.
/// Delegates to the generic [`save_trajectory`].
pub fn save_trajectory_slice(trajectory: &[Value], model: &str, completed: bool, filename: Option<&str>) {
    save_trajectory(trajectory, model, completed, filename)
}

#[allow(dead_code)]
fn _save_trajectory<S: Serialize>(trajectory: &S, model: &str, completed: bool, filename: Option<&str>) {
    save_trajectory(trajectory, model, completed, filename)
}

// ---------------------------------------------------------------------------
// now_iso + civil_from_days — mirrors `datetime.now().isoformat()` (line 46)
// Dependency-free RFC3339-style via SystemTime + civil_from_days (no chrono),
// matching the pattern in `verify_environment::utc_now_iso`.
// Python's `datetime.now().isoformat()` is naive local time without timezone;
// we emit UTC naive with microsecond precision (`YYYY-MM-DDTHH:MM:SS.mmmmmm`)
// which preserves the ISO8601 shape and round-trips through parsers expecting
// either naive or RFC3339. The millisecond vs microsecond difference is not
// load-bearing for trajectory consumers.
// ---------------------------------------------------------------------------

fn now_iso() -> String {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    let micros = dur.subsec_micros();
    let days = (secs / 86_400) as i64;
    let secs_of_day = secs % 86_400;
    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let second = secs_of_day % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{micros:06}")
}

/// Convert days since Unix epoch (1970-01-01) to civil date (year, month, day).
/// Howard Hinnant's civil_from_days algorithm (public domain).
/// Mirrors helper in `verify_environment.rs` (lines 382-394).
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    y += if m <= 2 { 1 } else { 0 };
    (y as i32, m as u32, d as u32)
}

#[allow(dead_code)]
fn _civil_from_days(z: i64) -> (i32, u32, u32) {
    civil_from_days(z)
}

#[allow(dead_code)]
fn _now_iso() -> String {
    now_iso()
}
