//! Disk-usage rollup for `/api/status` (NS-656).
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/disk_status.py` (117 LOC).
//!
//! Python source docstring (preserved):
//! ```text
//! Disk-usage rollup for ``/api/status`` (NS-656).
//!
//! Companion to :mod:`gateway.memory_status`, closing the same class of gap
//! for storage: a hosted agent can fill its data volume completely — SQLite
//! writes failing, session persistence dead, config saves lost — while its
//! dashboard and the NAS agent card both look perfectly healthy.  Fleet
//! incidents OOF-2 (unrecoverable disk-full) and OOF-107 (fleet-wide disk
//! exhaustion, remediated by hand) are exactly this failure mode.
//!
//! The readiness endpoint already probes disk (``gateway/readiness.py::
//! _probe_disk``), but readiness is a component verdict, not user-facing
//! telemetry — nothing renders it.  This module produces the public block
//! the dashboard SPA and the NAS availability sweep actually consume.
//!
//! Unlike the memory block (which distills already-persisted heartbeat
//! files), disk is sampled live via :func:`shutil.disk_usage` — a single
//! ``statvfs`` call, the same thing the readiness probe does per request.
//! There is no meaningful "staleness" dimension, so no ``sampled_at``.
//!
//! Public-safety note: ``/api/status`` is an unauthenticated liveness probe
//! (``PUBLIC_API_PATHS``).  This block carries only coarse numbers (MB
//! granularity, whole-percent usage) and an enum — the same disclosure
//! class as the ``memory`` block.
//!
//! Everything is best-effort and read-only: an unreadable filesystem
//! degrades to ``pressure="unknown"`` rather than raising into the status
//! endpoint.
//! ```
//!
//! Mapping:
//! - `_CRITICAL_FREE_MB = 256` → [`_CRITICAL_FREE_MB`] / [`CRITICAL_FREE_MB`]
//! - `_CRITICAL_PERCENT = 95.0` → [`_CRITICAL_PERCENT`] / [`CRITICAL_PERCENT`]
//! - `_CRITICAL_HEADROOM_MB = 1024` → [`_CRITICAL_HEADROOM_MB`] / [`CRITICAL_HEADROOM_MB`]
//! - `_ELEVATED_FREE_MB = 512` → [`_ELEVATED_FREE_MB`] / [`ELEVATED_FREE_MB`]
//! - `_ELEVATED_PERCENT = 85.0` → [`_ELEVATED_PERCENT`] / [`ELEVATED_PERCENT`]
//! - `_ELEVATED_HEADROOM_MB = 4096` → [`_ELEVATED_HEADROOM_MB`] / [`ELEVATED_HEADROOM_MB`]
//! - `_BYTES_PER_MB = 1024*1024` → [`_BYTES_PER_MB`] / [`BYTES_PER_MB`]
//! - `def _coerce_mb(value)` → [`_coerce_mb`] (bool / non-int / negative → `None`)
//! - `def classify_disk_pressure(free_mb, total_mb)` → [`classify_disk_pressure`] / [`classify_disk_pressure_values`] / [`classify_disk_pressure_opt`]
//! - `def collect_disk_status(home)` → [`collect_disk_status`] / [`collect_disk_status_with_home`] / [`collect_disk_status_default`]
//! - `shutil.disk_usage(home)` (`statvfs`) → [`get_disk_usage`] (`df -B1` fallback, same `total/used/free` shape, mirrors `readiness.get_disk_usage`)
//! - `round((used/total)*100, 1)` → `(raw*10.0).round()/10.0`
//! - `get_hermes_home()` → [`get_hermes_home`] (mirrors `hermes_constants.get_hermes_home`)

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

// ---------------------------------------------------------------------------
// Constants — mirrors Python module-level constants
// ---------------------------------------------------------------------------

/// < 256 MB free: critical on any volume. Mirrors `_CRITICAL_FREE_MB = 256`.
pub const _CRITICAL_FREE_MB: i64 = 256;
/// Public alias.
pub const CRITICAL_FREE_MB: i64 = _CRITICAL_FREE_MB;

/// >= 95% used AND < 1 GB free: critical. Mirrors `_CRITICAL_PERCENT = 95.0`.
pub const _CRITICAL_PERCENT: f64 = 95.0;
/// Public alias.
pub const CRITICAL_PERCENT: f64 = _CRITICAL_PERCENT;

/// Headroom gate for the percent-based critical rule. Mirrors `_CRITICAL_HEADROOM_MB = 1024`.
pub const _CRITICAL_HEADROOM_MB: i64 = 1024;
/// Public alias.
pub const CRITICAL_HEADROOM_MB: i64 = _CRITICAL_HEADROOM_MB;

/// < 512 MB free: elevated on any volume. Mirrors `_ELEVATED_FREE_MB = 512`.
pub const _ELEVATED_FREE_MB: i64 = 512;
/// Public alias.
pub const ELEVATED_FREE_MB: i64 = _ELEVATED_FREE_MB;

/// >= 85% used AND < 4 GB free: elevated. Mirrors `_ELEVATED_PERCENT = 85.0`.
pub const _ELEVATED_PERCENT: f64 = 85.0;
/// Public alias.
pub const ELEVATED_PERCENT: f64 = _ELEVATED_PERCENT;

/// Headroom gate for the percent-based elevated rule. Mirrors `_ELEVATED_HEADROOM_MB = 4096`.
pub const _ELEVATED_HEADROOM_MB: i64 = 4096;
/// Public alias.
pub const ELEVATED_HEADROOM_MB: i64 = _ELEVATED_HEADROOM_MB;

/// Bytes per megabyte. Mirrors `_BYTES_PER_MB = 1024*1024`.
pub const _BYTES_PER_MB: u64 = 1024 * 1024;
/// Public alias.
pub const BYTES_PER_MB: u64 = _BYTES_PER_MB;

// ---------------------------------------------------------------------------
// HERMES_HOME — mirrors `hermes_constants.get_hermes_home()`
// ---------------------------------------------------------------------------

/// Resolve `HERMES_HOME`: `$HERMES_HOME` if set and non-empty, else `~/.hermes`.
///
/// Mirrors `hermes_constants.get_hermes_home()` / `hermes_cli.config.get_hermes_home`.
pub fn get_hermes_home() -> PathBuf {
    if let Ok(val) = std::env::var("HERMES_HOME") {
        let trimmed = val.trim().to_string();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".hermes");
    }
    PathBuf::from(".hermes")
}

// ---------------------------------------------------------------------------
// _coerce_mb — mirrors Python `def _coerce_mb(value: Any) -> Optional[int]:`
// ---------------------------------------------------------------------------

/// Coerce a JSON value to a non-negative MB integer.
///
/// Mirrors:
/// ```python
/// def _coerce_mb(value: Any) -> Optional[int]:
///     if isinstance(value, bool) or not isinstance(value, int) or value < 0:
///         return None
///     return value
/// ```
/// In Rust, `bool` is `Value::Bool`, `int` is `Value::Number` with `as_i64`,
/// and `float`/`string`/etc. map to `None` (same as Python's `not isinstance(int)`).
pub fn _coerce_mb(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => {
            // `bool` is already excluded (Value::Bool). `float` → as_i64 is None.
            if let Some(i) = n.as_i64() {
                if i >= 0 {
                    Some(i)
                } else {
                    None
                }
            } else if let Some(u) = n.as_u64() {
                // Values above i64::MAX that fit in u64 — clamp check; Python int is unbounded,
                // but MB values this large are unrealistic; treat as Some if fits i64, else None.
                if u <= i64::MAX as u64 {
                    Some(u as i64)
                } else {
                    None
                }
            } else {
                // f64 → not an int in Python terms
                None
            }
        }
        _ => None,
    }
}

/// Typed helper: coerce an `Option<i64>` (already-typed) with the same negative guard.
pub fn _coerce_mb_opt(value: Option<i64>) -> Option<i64> {
    match value {
        Some(v) if v >= 0 => Some(v),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// classify_disk_pressure — mirrors Python
// ---------------------------------------------------------------------------

/// Map `free/total MB` to `ok`/`elevated`/`critical`/`unknown`.
///
/// Mirrors:
/// ```python
/// def classify_disk_pressure(free_mb: Any, total_mb: Any) -> str:
///     free = _coerce_mb(free_mb)
///     total = _coerce_mb(total_mb)
///     if free is None or total is None or total <= 0:
///         return "unknown"
///     used_percent = (1 - free / total) * 100.0
///     if free < _CRITICAL_FREE_MB or (
///         used_percent >= _CRITICAL_PERCENT and free < _CRITICAL_HEADROOM_MB
///     ):
///         return "critical"
///     if free < _ELEVATED_FREE_MB or (
///         used_percent >= _ELEVATED_PERCENT and free < _ELEVATED_HEADROOM_MB
///     ):
///         return "elevated"
///     return "ok"
/// ```
pub fn classify_disk_pressure(free_mb: &Value, total_mb: &Value) -> String {
    classify_disk_pressure_values(free_mb, total_mb).to_string()
}

/// Value-based variant returning `&'static str` for ergonomics.
pub fn classify_disk_pressure_values(free_mb: &Value, total_mb: &Value) -> &'static str {
    let free = _coerce_mb(free_mb);
    let total = _coerce_mb(total_mb);
    classify_disk_pressure_opt(free, total)
}

/// Typed core: the actual threshold logic, operating on `Option<i64>`.
///
/// This is what `collect_disk_status` calls after `// BYTES_PER_MB` integer division.
pub fn classify_disk_pressure_opt(free_mb: Option<i64>, total_mb: Option<i64>) -> &'static str {
    let free = match free_mb {
        Some(v) if v >= 0 => v,
        _ => return "unknown",
    };
    let total = match total_mb {
        Some(v) if v > 0 => v,
        _ => return "unknown",
    };
    let used_percent = (1.0 - free as f64 / total as f64) * 100.0;
    if free < _CRITICAL_FREE_MB
        || (used_percent >= _CRITICAL_PERCENT && free < _CRITICAL_HEADROOM_MB)
    {
        return "critical";
    }
    if free < _ELEVATED_FREE_MB
        || (used_percent >= _ELEVATED_PERCENT && free < _ELEVATED_HEADROOM_MB)
    {
        return "elevated";
    }
    "ok"
}

// Backwards-compatible alias without underscore (some callers may expect it).
pub fn classify_disk_pressure_opt_typed(free_mb: Option<i64>, total_mb: Option<i64>) -> &'static str {
    classify_disk_pressure_opt(free_mb, total_mb)
}

// ---------------------------------------------------------------------------
// Disk usage — mirrors `shutil.disk_usage(home)` (statvfs)
// ---------------------------------------------------------------------------

fn error_type_name_io(e: &std::io::Error) -> &'static str {
    use std::io::ErrorKind;
    match e.kind() {
        ErrorKind::NotFound => "FileNotFoundError",
        ErrorKind::PermissionDenied => "PermissionError",
        ErrorKind::AlreadyExists => "FileExistsError",
        ErrorKind::WouldBlock => "BlockingIOError",
        _ => "OSError",
    }
}

fn try_df_usage(path: &Path) -> Result<(u64, u64, u64), String> {
    let out = std::process::Command::new("df")
        .arg("-B1")
        .arg(path)
        .output()
        .map_err(|e| error_type_name_io(&e).to_string())?;
    if !out.status.success() {
        return Err("OSError".to_string());
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }
        let total: u64 = parts[1].parse().map_err(|_| "OSError".to_string())?;
        let used: u64 = parts[2].parse().map_err(|_| "OSError".to_string())?;
        let avail: u64 = parts[3].parse().map_err(|_| "OSError".to_string())?;
        let free = avail;
        return Ok((total, used, free));
    }
    Err("OSError".to_string())
}

/// Get disk usage for `path`: `(total_bytes, used_bytes, free_bytes)`.
///
/// Mirrors `shutil.disk_usage(home)` (which is a single `statvfs` call).
/// Primary path is `df -B1` (byte granularity); falls back to `OSError` on
/// failure so the caller degrades to `pressure="unknown"` rather than raising.
pub fn get_disk_usage(path: &Path) -> Result<(u64, u64, u64), String> {
    if let Ok(out) = try_df_usage(path) {
        return Ok(out);
    }
    Err("OSError".to_string())
}

// ---------------------------------------------------------------------------
// collect_disk_status — mirrors Python
// ---------------------------------------------------------------------------

/// Build the `disk` block for `/api/status` with an explicit home.
///
/// Mirrors:
/// ```python
/// def collect_disk_status(home: Optional[Path] = None) -> Dict[str, Any]:
///     status: Dict[str, Any] = {
///         "pressure": "unknown",
///         "total_mb": None,
///         "free_mb": None,
///         "used_percent": None,
///     }
///     try:
///         if home is None:
///             from hermes_constants import get_hermes_home
///             home = get_hermes_home()
///         usage = shutil.disk_usage(home)
///     except Exception:
///         return status
///     if usage.total <= 0:
///         return status
///     total_mb = usage.total // _BYTES_PER_MB
///     free_mb = usage.free // _BYTES_PER_MB
///     status["total_mb"] = total_mb
///     status["free_mb"] = free_mb
///     status["used_percent"] = round((usage.used / usage.total) * 100, 1)
///     status["pressure"] = classify_disk_pressure(free_mb, total_mb)
///     return status
/// ```
pub fn collect_disk_status_with_home(home: &Path) -> Value {
    let mut map = Map::new();
    map.insert("pressure".to_string(), json!("unknown"));
    map.insert("total_mb".to_string(), Value::Null);
    map.insert("free_mb".to_string(), Value::Null);
    map.insert("used_percent".to_string(), Value::Null);

    let usage = match get_disk_usage(home) {
        Ok(u) => u,
        Err(_) => return Value::Object(map),
    };
    let (total, used, free) = usage;
    if total == 0 {
        return Value::Object(map);
    }
    let total_mb = (total / _BYTES_PER_MB) as i64;
    let free_mb = (free / _BYTES_PER_MB) as i64;
    let used_percent = {
        let raw = (used as f64 / total as f64) * 100.0;
        (raw * 10.0).round() / 10.0
    };

    map.insert("total_mb".to_string(), json!(total_mb));
    map.insert("free_mb".to_string(), json!(free_mb));
    map.insert("used_percent".to_string(), json!(used_percent));
    let pressure = classify_disk_pressure_opt(Some(free_mb), Some(total_mb));
    map.insert("pressure".to_string(), json!(pressure));
    Value::Object(map)
}

/// Build the `disk` block, resolving `home` via `get_hermes_home()` when `None`.
///
/// Mirrors the `home: Optional[Path] = None` default of the Python function.
pub fn collect_disk_status(home: Option<&Path>) -> Value {
    match home {
        Some(p) => collect_disk_status_with_home(p),
        None => collect_disk_status_with_home(&get_hermes_home()),
    }
}

/// Convenience alias that always uses `get_hermes_home()` (mirrors `home=None` call).
pub fn collect_disk_status_default() -> Value {
    collect_disk_status(None)
}

// Provide private aliases mirroring Python's underscore-prefixed helpers for traceability.
#[allow(dead_code)]
fn _get_hermes_home() -> PathBuf {
    get_hermes_home()
}

#[allow(dead_code)]
fn _classify_disk_pressure(free_mb: &Value, total_mb: &Value) -> String {
    classify_disk_pressure(free_mb, total_mb)
}
