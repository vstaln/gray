//! ACP session manager — maps ACP sessions to Hermes AIAgent instances.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/acp_adapter/session.py`
//! (695 lines, full file — single slice). Sessions are persisted to the shared
//! `SessionDB` (`~/.hermes/state.db`) so they survive process restarts and appear
//! in `session_search`. When the editor reconnects after idle/restart, the
//! `load_session` / `resume_session` calls find the persisted session in the
//! database and restore the full conversation history.
//!
//! Mirrors Python module docstring (lines 1-8):
//! ```text
//! ACP session manager — maps ACP sessions to Hermes AIAgent instances.
//! Sessions are persisted to the shared SessionDB (~/.hermes/state.db) ...
//! ```
//!
//! T0412 — 1:1 port, no cargo (NEVER cargo). All external crates / DB / AIAgent
//! types are stubbed as local structs for traceability; `threading.Lock` is
//! modelled as `std::sync::Mutex`, `uuid` as a std-only pseudo-UUID, `datetime`
//! as `SystemTime` + manual ISO8601, `json` as minimal string helpers.
//! `hermes_constants` WSL helpers are re-implemented inline (std only).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-8
// ---------------------------------------------------------------------------

/// Mirrors `acp_adapter/session.py` top-level docstring (lines 1-8).
pub const MODULE_DOC: &str = "ACP session manager — maps ACP sessions to Hermes AIAgent instances.";

// ---------------------------------------------------------------------------
// Logger — mirrors `logger = logging.getLogger(__name__)` (line 26)
// ---------------------------------------------------------------------------

pub fn logger_name() -> &'static str {
    "acp_adapter.session"
}

// ---------------------------------------------------------------------------
// WSL helpers — mirrors `hermes_constants.{windows_path_to_wsl,
// wsl_unc_path_to_posix, translate_cwd_for_wsl_backend, is_wsl}` (lines 1386-1443)
// ---------------------------------------------------------------------------

/// Mirrors `hermes_constants.is_wsl()` (1386-1401). Checks `/proc/version` for
/// the `microsoft` marker. Result is not cached in this slice (Python caches
/// in global); caller may cache if hot path.
pub fn is_wsl() -> bool {
    // Mirrors `try: with open("/proc/version") ... except: return False`
    std::fs::read_to_string("/proc/version")
        .map(|s| s.to_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

/// Mirrors `hermes_constants.windows_path_to_wsl(path)` (1404-1413).
///
/// ```python
/// match = re.match(r"^([A-Za-z]):[\\/](.*)$", str(path or "").strip())
/// if not match: return None
/// return f"/mnt/{drive}/{tail}"
/// ```
pub fn windows_path_to_wsl(path: &str) -> Option<String> {
    let raw = path.trim();
    if raw.len() < 2 {
        return None;
    }
    let mut chars = raw.chars();
    let drive = chars.next()?;
    if !drive.is_ascii_alphabetic() {
        return None;
    }
    let sep = chars.next()?;
    if sep != ':' {
        return None;
    }
    let rest = &raw[2..];
    // Need at least one separator after colon: either '/' or '\' present at rest start
    // Python regex `:[\\/](.*)` allows empty tail after slash, but requires slash.
    // We replicate: if rest is empty -> not a match (needs slash)
    // If rest starts with '/' or '\', capture tail after it.
    if rest.is_empty() {
        return None;
    }
    let first = rest.chars().next()?;
    if first != '/' && first != '\\' {
        return None;
    }
    // Tail is everything after the first slash
    let tail = &rest[1..];
    let tail = tail.replace('\\', "/");
    Some(format!("/mnt/{}/{}", drive.to_ascii_lowercase(), tail))
}

/// Mirrors `hermes_constants.wsl_unc_path_to_posix(path)` (1416-1426).
pub fn wsl_unc_path_to_posix(path: &str) -> Option<String> {
    // Mirrors `normalized = str(path or "").strip().replace("/", "\\")`
    let normalized = path.trim().replace('/', "\\");
    // Lowercase prefix check for `\\wsl.localhost\` or `\\wsl$\`
    // Python: r"^\\\\wsl(?:\.localhost|\$)\\[^\\]+\\(.*)$" case-insensitive
    let lower = normalized.to_lowercase();
    let prefix1 = "\\\\wsl.localhost\\";
    let prefix2 = "\\\\wsl$\\";
    let after_prefix = if lower.starts_with(prefix1) {
        // Find where the distro segment ends: after prefix, next backslash separates distro and tail
        let distro_start = prefix1.len();
        let rest = &normalized[distro_start..];
        // rest = "<distro>\<tail>"
        if let Some(pos) = rest.find('\\') {
            Some(&rest[pos + 1..])
        } else {
            // No tail separator -> regex requires `\\(.*)` after distro, but Python match
            // would fail if no trailing content after distro? Actually pattern `\\[^\\]+\\(.*)`
            // requires a trailing slash after distro even if tail empty (.* allows empty).
            // So if no slash, no match.
            None
        }
    } else if lower.starts_with(prefix2) {
        let distro_start = prefix2.len();
        let rest = &normalized[distro_start..];
        if let Some(pos) = rest.find('\\') {
            Some(&rest[pos + 1..])
        } else {
            None
        }
    } else {
        None
    };
    let tail = after_prefix?;
    let tail = tail.replace('\\', "/");
    if tail.is_empty() {
        Some("/".to_string())
    } else {
        Some(format!("/{}", tail))
    }
}

/// Mirrors `_translate_acp_cwd(cwd)` (29-40) which delegates to
/// `hermes_constants.translate_cwd_for_wsl_backend` (1429-1443).
///
/// Python:
/// ```python
/// def _translate_acp_cwd(cwd: str) -> str:
///     from hermes_constants import translate_cwd_for_wsl_backend
///     return translate_cwd_for_wsl_backend(str(cwd))
/// ```
pub fn translate_acp_cwd(cwd: &str) -> String {
    // Mirrors `translate_cwd_for_wsl_backend(str(cwd))` -> is_wsl guard + translators
    if !is_wsl() {
        return cwd.to_string();
    }
    if let Some(posix) = wsl_unc_path_to_posix(cwd) {
        return posix;
    }
    if let Some(wsl) = windows_path_to_wsl(cwd) {
        return wsl;
    }
    cwd.to_string()
}

#[allow(dead_code)]
pub fn _translate_acp_cwd(cwd: &str) -> String {
    translate_acp_cwd(cwd)
}

// ---------------------------------------------------------------------------
// _normalize_cwd_for_compare — lines 43-70
// ---------------------------------------------------------------------------

/// Mirrors `_normalize_cwd_for_compare(cwd)` (43-70).
///
/// ```python
/// def _normalize_cwd_for_compare(cwd: str | None) -> str:
///     raw = str(cwd or ".").strip()
///     if not raw: raw = "."
///     expanded = os.path.expanduser(raw)
///     from hermes_constants import windows_path_to_wsl
///     translated = windows_path_to_wsl(expanded)
///     if translated is not None:
///         expanded = translated
///     elif re.match(r"^/mnt/[A-Za-z]/", expanded):
///         expanded = f"/mnt/{expanded[5].lower()}/{expanded[7:]}"
///     try: return os.path.realpath(expanded)
///     except OSError: return os.path.normpath(expanded)
/// ```
pub fn normalize_cwd_for_compare(cwd: Option<&str>) -> String {
    let raw_in = cwd.unwrap_or(".");
    let mut raw = raw_in.trim().to_string();
    if raw.is_empty() {
        raw = ".".to_string();
    }
    // Mirrors `os.path.expanduser(raw)`
    let expanded = expand_user(&raw);

    let mut expanded = {
        if let Some(translated) = windows_path_to_wsl(&expanded) {
            translated
        } else if is_mnt_drive_path(&expanded) {
            // Mirrors `f"/mnt/{expanded[5].lower()}/{expanded[7:]}"`
            // expanded is "/mnt/X/..." -> lower drive letter
            let drive = expanded.chars().nth(5).unwrap_or('a').to_ascii_lowercase();
            let rest = if expanded.len() > 7 { &expanded[7..] } else { "" };
            format!("/mnt/{}/{}", drive, rest)
        } else {
            expanded
        }
    };

    // Mirrors `os.path.realpath(expanded)` with `except OSError: normpath`
    // `realpath` is lexical for missing paths (strict=False), so use canonicalize
    // when file exists, else fall back to lexical normpath.
    match std::fs::canonicalize(&expanded) {
        Ok(p) => {
            // canonicalize may resolve symlinks; return as string
            // If canonicalize succeeds we return that path (mirrors realpath)
            p.to_string_lossy().to_string()
        }
        Err(_) => {
            // OSError or not found -> lexical normpath (preserve WSL drive paths)
            lexical_normpath(&expanded)
        }
    }
}

fn expand_user(raw: &str) -> String {
    // Mirrors `os.path.expanduser` — expands leading `~` using $HOME
    if raw == "~" || raw.starts_with("~/") || raw.starts_with("~\\") {
        if let Ok(home) = std::env::var("HOME") {
            if raw == "~" {
                return home;
            } else {
                // raw is `~/rest` -> join home + rest[1..]
                let rest = &raw[1..];
                // rest starts with / or \
                return format!("{}{}", home, rest);
            }
        }
    }
    // Also handle `~user`? Python expanduser handles it, but rare for ACP cwd.
    raw.to_string()
}

fn is_mnt_drive_path(s: &str) -> bool {
    // Mirrors `re.match(r"^/mnt/[A-Za-z]/", expanded)`
    let b = s.as_bytes();
    if b.len() < 6 {
        return false;
    }
    if &s[0..5] != "/mnt/" {
        return false;
    }
    let c = b[5] as char;
    if !c.is_ascii_alphabetic() {
        return false;
    }
    if b.len() == 6 {
        return true; // exactly "/mnt/X/" prefix? need trailing slash
    }
    b[6] == b'/'
}

fn lexical_normpath(path: &str) -> String {
    // Minimal lexical normpath — mirrors `os.path.normpath` for missing paths.
    // Handles `.`, `..`, duplicate slashes, preserves leading `/`.
    let is_absolute = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        } else if comp == ".." {
            if !parts.is_empty() && *parts.last().unwrap() != ".." {
                parts.pop();
            } else if !is_absolute {
                parts.push("..");
            }
        } else {
            parts.push(comp);
        }
    }
    let mut out = String::new();
    if is_absolute {
        out.push('/');
    }
    out.push_str(&parts.join("/"));
    if out.is_empty() {
        if is_absolute {
            "/".to_string()
        } else {
            ".".to_string()
        }
    } else {
        out
    }
}

#[allow(dead_code)]
pub fn _normalize_cwd_for_compare(cwd: Option<&str>) -> String {
    normalize_cwd_for_compare(cwd)
}

// ---------------------------------------------------------------------------
// _build_session_title — lines 73-81
// ---------------------------------------------------------------------------

/// Mirrors `_build_session_title(title, preview, cwd)` (73-81).
///
/// ```python
/// def _build_session_title(title, preview, cwd):
///     explicit = str(title or "").strip()
///     if explicit: return explicit
///     preview_text = str(preview or "").strip()
///     if preview_text: return preview_text
///     leaf = os.path.basename(str(cwd or "").rstrip("/\\"))
///     return leaf or "New thread"
/// ```
pub fn build_session_title(title: Option<&str>, preview: Option<&str>, cwd: Option<&str>) -> String {
    let explicit = title.unwrap_or("").trim().to_string();
    if !explicit.is_empty() {
        return explicit;
    }
    let preview_text = preview.unwrap_or("").trim().to_string();
    if !preview_text.is_empty() {
        return preview_text;
    }
    let cwd_s = cwd.unwrap_or("");
    let stripped = cwd_s.trim_end_matches(|c| c == '/' || c == '\\');
    let leaf = Path::new(stripped)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if !leaf.is_empty() {
        leaf
    } else {
        "New thread".to_string()
    }
}

#[allow(dead_code)]
pub fn _build_session_title(title: Option<&str>, preview: Option<&str>, cwd: Option<&str>) -> String {
    build_session_title(title, preview, cwd)
}

// ---------------------------------------------------------------------------
// _format_updated_at — lines 84-92
// ---------------------------------------------------------------------------

/// Input for `_format_updated_at` — mirrors Python's `value: Any` where
/// `None`, `str`, `float`, `int` are all possible. In Rust we model the
/// three shapes that matter: absent, textual timestamp, or numeric epoch.
#[derive(Debug, Clone)]
pub enum UpdatedAtValue {
    Null,
    Text(String),
    Number(f64),
}

impl UpdatedAtValue {
    pub fn from_str(s: Option<&str>) -> Self {
        match s {
            None => UpdatedAtValue::Null,
            Some(v) => UpdatedAtValue::Text(v.to_string()),
        }
    }
    pub fn from_f64(v: Option<f64>) -> Self {
        match v {
            None => UpdatedAtValue::Null,
            Some(n) => UpdatedAtValue::Number(n),
        }
    }
}

/// Mirrors `_format_updated_at(value)` (84-92).
///
/// ```python
/// def _format_updated_at(value):
///     if value is None: return None
///     if isinstance(value, str) and value.strip(): return value
///     try: return datetime.fromtimestamp(float(value), tz=timezone.utc).isoformat()
///     except Exception: return None
/// ```
pub fn format_updated_at(value: &UpdatedAtValue) -> Option<String> {
    match value {
        UpdatedAtValue::Null => None, // 85-86
        UpdatedAtValue::Text(s) => {
            if !s.trim().is_empty() {
                return Some(s.clone()); // 87-88
            }
            // Empty string falls through to try float path? Python would try float("") -> exception -> None
            None
        }
        UpdatedAtValue::Number(n) => {
            // Mirrors `datetime.fromtimestamp(float(value), tz=timezone.utc).isoformat()`
            // Use SystemTime for std-only isoformat (seconds since epoch)
            let secs = *n;
            if !secs.is_finite() {
                return None;
            }
            // Convert to SystemTime and format as ISO8601
            let ts = if secs >= 0.0 {
                UNIX_EPOCH + std::time::Duration::from_secs_f64(secs)
            } else {
                // Negative epoch (pre-1970) — SystemTime can't go before epoch in std
                // Fallback: return raw float string (Python would produce isoformat with negative)
                return Some(format_iso8601_from_secs(secs));
            };
            Some(system_time_to_iso8601(ts))
        }
    }
}

/// Helper for numeric-string fallback path: Python tries `float(value)` even when
/// value is a numeric string that slipped past the `isinstance(str)` guard? Actually
/// strings return early if non-empty, so numeric strings like "1234567890" are returned
/// verbatim. Only empty strings or numbers reach the float path. In Rust we expose
/// a separate helper for completeness.
pub fn format_updated_at_from_any(value_opt: Option<&str>, numeric: Option<f64>) -> Option<String> {
    if let Some(n) = numeric {
        return format_updated_at(&UpdatedAtValue::Number(n));
    }
    match value_opt {
        None => None,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => Some(s.to_string()),
    }
}

fn system_time_to_iso8601(st: SystemTime) -> String {
    // Minimal ISO8601 formatter using chrono-free arithmetic.
    // Python: `datetime.fromtimestamp(..., tz=timezone.utc).isoformat()` produces like
    // `2026-08-27T12:34:56.123456+00:00`
    // For std-only we compute via UNIX timestamp seconds + fallback to Debug.
    match st.duration_since(UNIX_EPOCH) {
        Ok(dur) => {
            let secs = dur.as_secs() as f64 + dur.subsec_nanos() as f64 / 1e9;
            format_iso8601_from_secs(secs)
        }
        Err(_) => "1970-01-01T00:00:00+00:00".to_string(),
    }
}

fn format_iso8601_from_secs(secs: f64) -> String {
    // Cheap iso8601 stub: delegates to `time` crate would be cargo, so we emit
    // a deterministic UTC string via manual calendar? For slice fidelity we keep
    // a simple second-precision formatter using days arithmetic.
    // If secs is not finite or negative large, fall back to raw.
    if !secs.is_finite() {
        return String::new();
    }
    let total_secs = secs as i64;
    // Use a simple conversion: start from 1970-01-01, compute date via days.
    // For T0412 slice, callers only sort by this value; exact calendar fields
    // are not asserted in Rust tests beyond non-empty. So we provide a
    // stable `YYYY-MM-DDTHH:MM:SS+00:00` shape.
    let days = total_secs.div_euclid(86400);
    let secs_of_day = total_secs.rem_euclid(86400);
    let (y, m, d) = days_to_ymd(days);
    let h = secs_of_day / 3600;
    let min = (secs_of_day % 3600) / 60;
    let s = secs_of_day % 60;
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}+00:00")
}

fn days_to_ymd(days_since_epoch: i64) -> (i32, u32, u32) {
    // Howard Hinnant's civil_from_days algorithm, valid for proleptic Gregorian.
    let z = days_since_epoch + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    y += if m <= 2 { 1 } else { 0 };
    (y as i32, m as u32, d as u32)
}

#[allow(dead_code)]
pub fn _format_updated_at_str(value: Option<&str>) -> Option<String> {
    format_updated_at(&UpdatedAtValue::from_str(value))
}
#[allow(dead_code)]
pub fn _format_updated_at_f64(value: Option<f64>) -> Option<String> {
    format_updated_at(&UpdatedAtValue::from_f64(value))
}

// ---------------------------------------------------------------------------
// _updated_at_sort_key — lines 95-109
// ---------------------------------------------------------------------------

/// Mirrors `_updated_at_sort_key(value)` (95-109).
///
/// ```python
/// def _updated_at_sort_key(value):
///     if value is None: return float("-inf")
///     if isinstance(value, (int, float)): return float(value)
///     raw = str(value).strip()
///     if not raw: return float("-inf")
///     try: return datetime.fromisoformat(raw.replace("Z", "+00:00")).timestamp()
///     except: try: return float(raw) except: return float("-inf")
/// ```
pub fn updated_at_sort_key(value: Option<&str>, numeric: Option<f64>) -> f64 {
    if let Some(n) = numeric {
        if n.is_finite() {
            return n; // 98-99
        } else {
            return f64::NEG_INFINITY;
        }
    }
    let raw_opt = value;
    match raw_opt {
        None => f64::NEG_INFINITY, // 96-97
        Some(s) => {
            let raw = s.trim().to_string();
            if raw.is_empty() {
                return f64::NEG_INFINITY; // 101-102
            }
            // Try ISO8601 parse: `raw.replace("Z", "+00:00")` then fromisoformat
            if let Some(ts) = parse_iso8601_to_timestamp(&raw.replace('Z', "+00:00")) {
                return ts; // 103-104
            }
            // Try float
            if let Ok(f) = raw.parse::<f64>() {
                if f.is_finite() {
                    return f; // 106-107
                }
            }
            f64::NEG_INFINITY
        }
    }
}

fn parse_iso8601_to_timestamp(s: &str) -> Option<f64> {
    // Minimal ISO8601 parser for sort key: handles `YYYY-MM-DDTHH:MM:SS[.frac][+00:00]`
    // Python: datetime.fromisoformat(...).timestamp()
    // For std-only we parse the shape emitted by format_iso8601_from_secs and
    // common DB shapes; return None on failure to exercise float fallback.
    let s = s.trim();
    if s.len() < 10 {
        return None;
    }
    // Quick check: must contain '-' and 'T' or ' '
    if !s.contains('-') {
        return None;
    }
    // Try to split date/time
    let (date_part, rest) = if let Some(t_pos) = s.find('T').or_else(|| s.find(' ')) {
        (&s[..t_pos], &s[t_pos + 1..])
    } else if s.len() == 10 {
        (s, "")
    } else {
        return None;
    };
    let date_comps: Vec<&str> = date_part.split('-').collect();
    if date_comps.len() != 3 {
        return None;
    }
    let y: i32 = date_comps[0].parse().ok()?;
    let m: u32 = date_comps[1].parse().ok()?;
    let d: u32 = date_comps[2].parse().ok()?;
    if rest.is_empty() {
        let days = ymd_to_days(y, m, d);
        return Some(days as f64 * 86400.0);
    }
    // rest: `HH:MM:SS[.frac][+00:00]` or `HH:MM:SS[.frac]Z` already replaced
    // Strip timezone suffix
    let time_core = if let Some(plus) = rest.find('+') {
        &rest[..plus]
    } else if let Some(minus) = rest.rfind('-') {
        // Only treat as tz if after time portion (contains ':')
        if rest[..minus].contains(':') {
            &rest[..minus]
        } else {
            rest
        }
    } else {
        rest
    };
    let time_core = time_core.trim_end_matches('Z');
    let t_comps: Vec<&str> = time_core.split(':').collect();
    if t_comps.len() < 2 {
        return None;
    }
    let hh: i64 = t_comps[0].parse().ok()?;
    let mm: i64 = t_comps[1].parse().ok()?;
    let ss: f64 = if t_comps.len() >= 3 {
        t_comps[2].parse().unwrap_or(0.0)
    } else {
        0.0
    };
    let days = ymd_to_days(y, m, d);
    Some(days as f64 * 86400.0 + hh as f64 * 3600.0 + mm as f64 * 60.0 + ss)
}

fn ymd_to_days(y: i32, m: u32, d: u32) -> i64 {
    // Inverse of days_to_ymd; days since 1970-01-01
    let mut y = y as i64;
    let mut m = m as i64;
    let d = d as i64;
    if m <= 2 {
        y -= 1;
        m += 12;
    }
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m - 3) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    days
}

#[allow(dead_code)]
pub fn _updated_at_sort_key(value: Option<&str>) -> f64 {
    updated_at_sort_key(value, None)
}

// ---------------------------------------------------------------------------
// _acp_stderr_print — lines 112-120
// ---------------------------------------------------------------------------

/// Mirrors `_acp_stderr_print(*args, **kwargs)` (112-120).
///
/// ACP reserves stdout for JSON-RPC frames, so incidental CLI/status output
/// from AIAgent is routed to stderr.
pub fn acp_stderr_print(args: &[&str]) {
    // Mirrors `kwargs.setdefault("file", sys.stderr); print(*args, **kwargs)`
    // In Rust we join with space and emit to stderr.
    eprintln!("{}", args.join(" "));
}

#[allow(dead_code)]
pub fn _acp_stderr_print(args: &[&str]) {
    acp_stderr_print(args)
}

// ---------------------------------------------------------------------------
// _register_task_cwd / _clear_task_cwd — lines 123-137, 158-166
// ---------------------------------------------------------------------------

/// Mirrors `_register_task_cwd(task_id, cwd)` (123-137).
///
/// Binds a task/session id to the editor's working directory for tools.
/// In Python this calls `tools.terminal_tool.register_task_env_overrides`.
/// In Rust (NEVER cargo) we stub as best-effort with debug on failure.
pub fn register_task_cwd(task_id: &str, cwd: &str) {
    if task_id.is_empty() {
        return; // 131-132
    }
    let translated = translate_acp_cwd(cwd); // 135
    // Mirrors `try: register_task_env_overrides(task_id, {"cwd": ...}) except: logger.debug`
    // Stub: in std-only build we have no terminal_tool registry; log at debug level.
    let _ = (task_id, translated);
    // debug: intentionally no-op, matching the `except: logger.debug` swallow
}

#[allow(dead_code)]
pub fn _register_task_cwd(task_id: &str, cwd: &str) {
    register_task_cwd(task_id, cwd)
}

/// Mirrors `_clear_task_cwd(task_id)` (158-166).
pub fn clear_task_cwd(task_id: &str) {
    if task_id.is_empty() {
        return;
    }
    // Mirrors `try: clear_task_env_overrides(task_id) except: logger.debug`
    let _ = task_id;
}

#[allow(dead_code)]
pub fn _clear_task_cwd(task_id: &str) {
    clear_task_cwd(task_id)
}

// ---------------------------------------------------------------------------
// _expand_acp_enabled_toolsets — lines 140-155
// ---------------------------------------------------------------------------

/// Mirrors `_expand_acp_enabled_toolsets(toolsets, mcp_server_names)` (140-155).
pub fn expand_acp_enabled_toolsets(
    toolsets: Option<Vec<String>>,
    mcp_server_names: Option<Vec<String>>,
) -> Vec<String> {
    let mut expanded: Vec<String> = Vec::new();
    let base = toolsets.unwrap_or_else(|| vec!["hermes-acp".to_string()]); // 146 `or ["hermes-acp"]`
    for name in base {
        if !name.is_empty() && !expanded.contains(&name) {
            expanded.push(name); // 147-148
        }
    }
    let mcp_names = mcp_server_names.unwrap_or_default();
    for server_name in mcp_names {
        if server_name.is_empty() {
            continue;
        }
        let toolset_name = format!("mcp-{}", server_name); // 151
        if !expanded.contains(&toolset_name) {
            expanded.push(toolset_name); // 152-153
        }
    }
    expanded
}

#[allow(dead_code)]
pub fn _expand_acp_enabled_toolsets(
    toolsets: Option<Vec<String>>,
    mcp_server_names: Option<Vec<String>>,
) -> Vec<String> {
    expand_acp_enabled_toolsets(toolsets, mcp_server_names)
}

// ---------------------------------------------------------------------------
// SessionState — lines 169-183
// ---------------------------------------------------------------------------

/// Tracks per-session state for an ACP-managed Hermes agent.
/// Mirrors `@dataclass class SessionState:` (169-183).
#[derive(Debug, Clone)]
pub struct SessionState {
    /// Mirrors `session_id: str`
    pub session_id: String,
    /// Mirrors `agent: Any` — the live `AIAgent` (stubbed)
    pub agent: AgentStub,
    /// Mirrors `cwd: str = "."`
    pub cwd: String,
    /// Mirrors `model: str = ""`
    pub model: String,
    /// Mirrors `history: List[Dict[str, Any]] = field(default_factory=list)`
    pub history: Vec<HashMap<String, String>>,
    /// Mirrors `cancel_event: Any = None` — `threading.Event` stub
    pub cancel_event: Option<CancelEvent>,
    /// Mirrors `is_running: bool = False`
    pub is_running: bool,
    /// Mirrors `queued_prompts: List[str] = field(default_factory=list)`
    pub queued_prompts: Vec<String>,
    /// Mirrors `runtime_lock: Any = field(default_factory=Lock)`
    pub runtime_lock: Arc<Mutex<()>>,
    /// Mirrors `current_prompt_text: str = ""`
    pub current_prompt_text: String,
    /// Mirrors `interrupted_prompt_text: str = ""`
    pub interrupted_prompt_text: String,
}

impl SessionState {
    pub fn new(session_id: String, agent: AgentStub, cwd: String, model: String) -> Self {
        Self {
            session_id,
            agent,
            cwd,
            model,
            history: Vec::new(),
            cancel_event: Some(CancelEvent::new()),
            is_running: false,
            queued_prompts: Vec::new(),
            runtime_lock: Arc::new(Mutex::new(())),
            current_prompt_text: String::new(),
            interrupted_prompt_text: String::new(),
        }
    }
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            agent: AgentStub::default(),
            cwd: ".".to_string(),
            model: String::new(),
            history: Vec::new(),
            cancel_event: None,
            is_running: false,
            queued_prompts: Vec::new(),
            runtime_lock: Arc::new(Mutex::new(())),
            current_prompt_text: String::new(),
            interrupted_prompt_text: String::new(),
        }
    }
}

/// Minimal stub for `threading.Event` (lines 170, 179).
#[derive(Debug, Clone)]
pub struct CancelEvent {
    flag: Arc<Mutex<bool>>,
}

impl CancelEvent {
    pub fn new() -> Self {
        Self { flag: Arc::new(Mutex::new(false)) }
    }
    pub fn set(&self) {
        if let Ok(mut g) = self.flag.lock() { *g = true; }
    }
    pub fn clear(&self) {
        if let Ok(mut g) = self.flag.lock() { *g = false; }
    }
    pub fn is_set(&self) -> bool {
        self.flag.lock().map(|g| *g).unwrap_or(false)
    }
}

/// Minimal stub for the live `AIAgent` (lines 174, 228+). Mirrors the subset
/// of `AIAgent` fields accessed by `SessionManager` persistence and
/// `_make_agent`: `model`, `provider`, `base_url`, `api_mode`, `session_id`,
/// `session_cwd`, `_session_db`, `_session_db_created`, `_print_fn`.
#[derive(Debug, Clone)]
pub struct AgentStub {
    pub model: String,
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub api_mode: Option<String>,
    pub session_id: String,
    pub session_cwd: String,
    /// Mirrors `agent._session_db` — pointer to DB for ownership check (424-486)
    pub session_db_ptr: Option<usize>,
    /// Mirrors `agent._session_db_created`
    pub session_db_created: bool,
}

impl Default for AgentStub {
    fn default() -> Self {
        Self {
            model: String::new(),
            provider: None,
            base_url: None,
            api_mode: None,
            session_id: String::new(),
            session_cwd: ".".to_string(),
            session_db_ptr: None,
            session_db_created: false,
        }
    }
}

impl AgentStub {
    pub fn new(model: String, session_id: String) -> Self {
        Self { model, session_id, ..Default::default() }
    }
}

// ---------------------------------------------------------------------------
// SessionDB stubs — mirrors `hermes_state.SessionDB` (lines 289-597)
// ---------------------------------------------------------------------------

/// Row shape for `SessionDB.get_session` / `list_sessions_rich` / `search_sessions`.
/// Mirrors the dict returned by `hermes_state.SessionDB` (lines 287-352, 509-584).
#[derive(Debug, Clone, Default)]
pub struct PersistedRow {
    pub id: String,
    pub source: String,
    pub model: Option<String>,
    /// `model_config` JSON string containing at least `{"cwd": "...", "provider": "..."}`
    pub model_config: Option<String>,
    pub preview: Option<String>,
    pub title: Option<String>,
    pub last_active: Option<String>,
    pub started_at: Option<String>,
    pub message_count: usize,
    pub billing_provider: Option<String>,
    pub billing_base_url: Option<String>,
}

/// In-memory stub for `SessionDB` — mirrors the persistence surface used by
/// `SessionManager` (lines 289-597). Real `SessionDB` is SQLite-backed; this
/// stub is `Mutex`-guarded for thread-safe access via `SessionManager._lock`.
#[derive(Debug, Default)]
pub struct SessionDbStub {
    pub sessions: HashMap<String, PersistedRow>,
    pub messages: HashMap<String, Vec<HashMap<String, String>>>,
    /// `active` / `compacted` archiving is modelled as a separate set of ids
    /// that carry archived rows (for the `active_only` guard, lines 464-505).
    pub archived_ids: HashSet<String>,
}

impl SessionDbStub {
    pub fn new() -> Self { Self::default() }

    pub fn get_session(&self, session_id: &str) -> Option<PersistedRow> {
        self.sessions.get(session_id).cloned()
    }

    pub fn create_session(&mut self, session_id: &str, source: &str, model: Option<String>, model_config: HashMap<String, String>) {
        let cwd = model_config.get("cwd").cloned().unwrap_or_else(|| ".".to_string());
        let model_config_json = json_dumps_model_config(&model_config);
        self.sessions.insert(session_id.to_string(), PersistedRow {
            id: session_id.to_string(),
            source: source.to_string(),
            model,
            model_config: Some(model_config_json),
            preview: None,
            title: None,
            last_active: Some(now_iso8601()),
            started_at: Some(now_iso8601()),
            message_count: 0,
            billing_provider: model_config.get("provider").cloned(),
            billing_base_url: model_config.get("base_url").cloned(),
        });
        let _ = cwd;
    }

    pub fn update_session_meta(&mut self, session_id: &str, cwd_json: String, model: Option<String>) -> Result<(), String> {
        if let Some(row) = self.sessions.get_mut(session_id) {
            row.model_config = Some(cwd_json);
            if model.is_some() { row.model = model; }
            row.last_active = Some(now_iso8601());
            Ok(())
        } else {
            Err("session not found".to_string())
        }
    }

    pub fn replace_messages(&mut self, session_id: &str, history: &[HashMap<String, String>], active_only: bool) -> Result<(), String> {
        // Mirrors `db.replace_messages(session_id, history, active_only=True)` (503-505)
        // When `active_only=True`, archived rows survive. In stub we model this by
        // keeping `archived_ids` untouched and only replacing `messages`.
        let _ = active_only;
        self.messages.insert(session_id.to_string(), history.to_vec());
        if let Some(row) = self.sessions.get_mut(session_id) {
            row.message_count = history.len();
            row.last_active = Some(now_iso8601());
            if !history.is_empty() {
                // Update preview from first user message
                for msg in history {
                    if msg.get("role").map(|s| s.as_str()) == Some("user") {
                        if let Some(content) = msg.get("content") {
                            if !content.trim().is_empty() {
                                row.preview = Some(content.trim().to_string());
                                break;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn delete_session(&mut self, session_id: &str) -> bool {
        let existed = self.sessions.remove(session_id).is_some();
        self.messages.remove(session_id);
        self.archived_ids.remove(session_id);
        existed
    }

    pub fn list_sessions_rich(&self, source: &str, limit: usize) -> Vec<PersistedRow> {
        let mut out: Vec<PersistedRow> = self.sessions.values()
            .filter(|r| r.source == source)
            .cloned()
            .collect();
        out.truncate(limit);
        out
    }

    pub fn search_sessions(&self, source: &str, limit: usize) -> Vec<PersistedRow> {
        self.list_sessions_rich(source, limit)
    }

    pub fn get_messages_as_conversation(&self, session_id: &str, repair_alternation: bool) -> Vec<HashMap<String, String>> {
        let _ = repair_alternation; // Mirrors line 555-556
        self.messages.get(session_id).cloned().unwrap_or_default()
    }

    pub fn ptr_id(&self) -> usize {
        self as *const Self as usize
    }
}

fn now_iso8601() -> String {
    system_time_to_iso8601(SystemTime::now())
}

fn json_dumps_model_config(map: &HashMap<String, String>) -> String {
    // Minimal JSON dump for `{"cwd": "...", "provider": "...", "base_url": "...", "api_mode": "..."}`
    // Mirrors `json.dumps(session_meta)` (445). In std-only we emit a simple object
    // with quoted keys/values and no pretty print.
    let mut parts: Vec<String> = Vec::new();
    for (k, v) in map {
        let ek = json_escape(k);
        let ev = json_escape(v);
        parts.push(format!("\"{ek}\":\"{ev}\""));
    }
    format!("{{{}}}", parts.join(","))
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r")
}

fn json_loads_get(map_json: &str, key: &str) -> Option<String> {
    // Minimal JSON loader for `model_config` extraction (339-341, 538-543).
    // Looks for `"key":"value"` substring; not a full parser, but sufficient
    // for the flat `{"cwd": "...", "provider": "..."}` shape emitted above.
    let pat = format!("\"{key}\"");
    let pos = map_json.find(&pat)?;
    let after = &map_json[pos + pat.len()..];
    let colon = after.find(':')?;
    let after_colon = after[colon + 1..].trim_start();
    if !after_colon.starts_with('"') {
        return None;
    }
    let end = after_colon[1..].find('"')?;
    let val = &after_colon[1..1 + end];
    Some(val.replace("\\\"", "\"").replace("\\\\", "\\"))
}

// ---------------------------------------------------------------------------
// SessionManager — lines 186-695
// ---------------------------------------------------------------------------

/// Thread-safe manager for ACP sessions backed by Hermes AIAgent instances.
/// Mirrors `class SessionManager:` (186-695). Sessions are held in-memory for
/// fast access **and** persisted to the shared SessionDB.
pub struct SessionManager {
    sessions: Mutex<HashMap<String, SessionState>>,
    lock: Mutex<()>, // Mirrors `self._lock = Lock()` (204)
    agent_factory: Option<Box<dyn Fn() -> AgentStub + Send + Sync>>,
    db: Arc<Mutex<SessionDbStub>>,
}

impl SessionManager {
    /// Mirrors `def __init__(self, agent_factory=None, db=None)` (194-206).
    pub fn new(
        agent_factory: Option<Box<dyn Fn() -> AgentStub + Send + Sync>>,
        db: Option<Arc<Mutex<SessionDbStub>>>,
    ) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            lock: Mutex::new(()),
            agent_factory,
            db: db.unwrap_or_else(|| Arc::new(Mutex::new(SessionDbStub::new()))),
        }
    }

    pub fn with_db(db: Arc<Mutex<SessionDbStub>>) -> Self {
        Self::new(None, Some(db))
    }

    // ---- public API ---------------------------------------------------------

    /// Mirrors `def create_session(self, cwd: str = ".") -> SessionState` (210-229).
    pub fn create_session(&self, cwd: &str) -> SessionState {
        let cwd = translate_acp_cwd(cwd); // 214
        let session_id = new_uuid4(); // 215
        let agent = self.make_agent(&session_id, &cwd, None, None, None, None); // 216
        let model = agent.model.clone(); // 220 `getattr(agent, "model", "") or ""`
        let state = SessionState::new(session_id.clone(), agent, cwd.clone(), model);
        // Mirrors `with self._lock: self._sessions[session_id] = state` (224-225)
        {
            let _guard = self.lock.lock().unwrap();
            self.sessions.lock().unwrap().insert(session_id.clone(), state.clone());
        }
        register_task_cwd(&session_id, &cwd); // 226
        self.persist(&state); // 227
        eprintln!("[{}] Created ACP session {} (cwd={})", logger_name(), session_id, cwd); // 228
        state
    }

    /// Mirrors `def get_session(self, session_id: str) -> Optional[SessionState]` (231-242).
    pub fn get_session(&self, session_id: &str) -> Option<SessionState> {
        // Mirrors `with self._lock: state = self._sessions.get(session_id)` (237-238)
        let state = {
            let _guard = self.lock.lock().unwrap();
            self.sessions.lock().unwrap().get(session_id).cloned()
        };
        if state.is_some() {
            return state; // 239-240
        }
        // Attempt to restore from database (242)
        self.restore(session_id)
    }

    /// Mirrors `def remove_session(self, session_id: str) -> bool` (244-251).
    pub fn remove_session(&self, session_id: &str) -> bool {
        let existed = {
            let _guard = self.lock.lock().unwrap();
            self.sessions.lock().unwrap().remove(session_id).is_some() // 246-247
        };
        let db_existed = self.delete_persisted(session_id); // 248
        if existed || db_existed {
            clear_task_cwd(session_id); // 250
        }
        existed || db_existed // 251
    }

    /// Mirrors `def fork_session(self, session_id: str, cwd: str = ".") -> Optional[SessionState]` (253-281).
    pub fn fork_session(&self, session_id: &str, cwd: &str) -> Option<SessionState> {
        let cwd = translate_acp_cwd(cwd); // 257
        let original = self.get_session(session_id)?; // 258-260
        let new_id = new_uuid4(); // 262
        let agent = self.make_agent(&new_id, &cwd, if original.model.is_empty() { None } else { Some(original.model.as_str()) }, None, None, None); // 263-267
        let model = if !agent.model.is_empty() { agent.model.clone() } else { original.model.clone() }; // 272
        let mut state = SessionState::new(new_id.clone(), agent, cwd.clone(), model);
        state.history = original.history.clone(); // 273 `copy.deepcopy(original.history)`
        {
            let _guard = self.lock.lock().unwrap();
            self.sessions.lock().unwrap().insert(new_id.clone(), state.clone()); // 276-277
        }
        register_task_cwd(&new_id, &cwd); // 278
        self.persist(&state); // 279
        eprintln!("[{}] Forked ACP session {} -> {}", logger_name(), session_id, new_id); // 280
        Some(state)
    }

    /// Mirrors `def list_sessions(self, cwd: str | None = None) -> List[Dict[str, Any]]` (283-355).
    pub fn list_sessions(&self, cwd: Option<&str>) -> Vec<ListSessionInfo> {
        let normalized_cwd = cwd.map(|c| normalize_cwd_for_compare(Some(c))); // 285
        let db = self.get_db_arc(); // 286
        let persisted_rows: HashMap<String, PersistedRow> = {
            let guard = db.lock().unwrap();
            let mut map: HashMap<String, PersistedRow> = HashMap::new();
            // Mirrors `for row in db.list_sessions_rich(source="acp", limit=1000)` (291)
            for row in guard.list_sessions_rich("acp", 1000) {
                map.insert(row.id.clone(), row);
            }
            map
        };

        let mut results: Vec<ListSessionInfo> = Vec::new();
        let mut seen_ids: HashSet<String> = HashSet::new();

        // Collect in-memory sessions first (296-326)
        {
            let _guard = self.lock.lock().unwrap();
            let sessions = self.sessions.lock().unwrap();
            for s in sessions.values() {
                seen_ids.insert(s.session_id.clone());
                let history_len = s.history.len();
                if history_len == 0 {
                    continue; // 302-303
                }
                if let Some(ref norm) = normalized_cwd {
                    if normalize_cwd_for_compare(Some(&s.cwd)) != *norm {
                        continue; // 304-305
                    }
                }
                let persisted = persisted_rows.get(&s.session_id);
                // Mirrors preview extraction (307-314): first user message content
                let preview_from_history = s.history.iter()
                    .find(|msg| msg.get("role").map(|r| r.as_str()) == Some("user"))
                    .and_then(|msg| msg.get("content"))
                    .map(|c| c.trim().to_string())
                    .filter(|c| !c.is_empty())
                    .unwrap_or_default();
                let preview = if !preview_from_history.is_empty() {
                    preview_from_history
                } else {
                    persisted.and_then(|r| r.preview.clone()).unwrap_or_default()
                };
                let persisted_title = persisted.and_then(|r| r.title.clone());
                let title = build_session_title(persisted_title.as_deref(), if preview.is_empty() { None } else { Some(preview.as_str()) }, Some(&s.cwd)); // 321
                // Mirrors `_format_updated_at(persisted.get("last_active") or persisted.get("started_at") or time.time())`
                let updated_at_raw = persisted.and_then(|r| r.last_active.clone())
                    .or_else(|| persisted.and_then(|r| r.started_at.clone()));
                let updated_at = match updated_at_raw {
                    Some(ts) => Some(ts), // already iso string from DB
                    None => Some(now_iso8601()), // mirrors `or time.time()` -> iso
                };
                results.push(ListSessionInfo {
                    session_id: s.session_id.clone(),
                    cwd: s.cwd.clone(),
                    model: s.model.clone(),
                    history_len,
                    title,
                    updated_at,
                });
            }
        }

        // Merge any persisted sessions not currently in memory (328-352)
        for (sid, row) in &persisted_rows {
            if seen_ids.contains(sid) {
                continue; // 330-331
            }
            let message_count = row.message_count; // 332
            if message_count == 0 {
                continue; // 333-334
            }
            // Extract cwd from model_config JSON (336-342)
            let session_cwd = row.model_config.as_deref()
                .and_then(|mc| json_loads_get(mc, "cwd"))
                .unwrap_or_else(|| ".".to_string());
            if let Some(ref norm) = normalized_cwd {
                if normalize_cwd_for_compare(Some(&session_cwd)) != *norm {
                    continue; // 343-344
                }
            }
            let title = build_session_title(row.title.as_deref(), row.preview.as_deref(), Some(&session_cwd)); // 350
            let updated_at = row.last_active.clone().or_else(|| row.started_at.clone());
            results.push(ListSessionInfo {
                session_id: sid.clone(),
                cwd: session_cwd,
                model: row.model.clone().unwrap_or_default(),
                history_len: message_count,
                title,
                updated_at,
            });
        }

        // Mirrors `results.sort(key=lambda item: _updated_at_sort_key(item.get("updated_at")), reverse=True)` (354)
        results.sort_by(|a, b| {
            let ka = updated_at_sort_key(a.updated_at.as_deref(), None);
            let kb = updated_at_sort_key(b.updated_at.as_deref(), None);
            // reverse=True
            kb.partial_cmp(&ka).unwrap_or(std::cmp::Ordering::Equal)
        });
        results
    }

    /// Mirrors `def update_cwd(self, session_id: str, cwd: str) -> Optional[SessionState]` (357-366).
    pub fn update_cwd(&self, session_id: &str, cwd: &str) -> Option<SessionState> {
        let cwd = translate_acp_cwd(cwd); // 359
        // Need to get session and mutate in place. Use clone + reinsert for simplicity
        // (Python mutates `state.cwd` directly under lock-free get).
        let mut state = self.get_session(session_id)?; // 360-362
        state.cwd = cwd.clone(); // 363
        register_task_cwd(session_id, &cwd); // 364
        // Need to update in map before persist so persisted state has new cwd
        {
            let _guard = self.lock.lock().unwrap();
            self.sessions.lock().unwrap().insert(session_id.to_string(), state.clone());
        }
        self.persist(&state); // 365
        Some(state)
    }

    /// Mirrors `def cleanup(self) -> None` (368-386).
    pub fn cleanup(&self) {
        let session_ids: Vec<String> = {
            let _guard = self.lock.lock().unwrap();
            let mut sessions = self.sessions.lock().unwrap();
            let ids = sessions.keys().cloned().collect::<Vec<_>>();
            sessions.clear(); // 372
            ids
        };
        for session_id in session_ids {
            clear_task_cwd(&session_id); // 374
            self.delete_persisted(&session_id); // 375
        }
        // Also remove any DB-only ACP sessions not currently in memory (376-386)
        let db = self.get_db_arc();
        let mut guard = db.lock().unwrap();
        let rows = guard.search_sessions("acp", 10000); // 380
        let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
        for sid in ids {
            clear_task_cwd(&sid); // 383
            guard.delete_session(&sid); // 384
        }
    }

    /// Mirrors `def save_session(self, session_id: str) -> None` (388-397).
    pub fn save_session(&self, session_id: &str) {
        let state = {
            let _guard = self.lock.lock().unwrap();
            self.sessions.lock().unwrap().get(session_id).cloned() // 394-395
        };
        if let Some(state) = state {
            self.persist(&state); // 397
        }
    }

    // ---- persistence via SessionDB ------------------------------------------

    /// Mirrors `def _get_db(self)` (401-421).
    fn get_db_arc(&self) -> Arc<Mutex<SessionDbStub>> {
        // Mirrors lazy init via `get_hermes_home()` / `SessionDB(db_path=...)`
        // In Rust we always have a `SessionDbStub` (NEVER cargo SQLite), so we
        // return the existing arc directly, matching the `if self._db_instance is not None: return` path (412-413)
        // and the `try: SessionDB(...) except: return None` fallback is modelled as
        // always-succeeding stub.
        Arc::clone(&self.db)
    }

    fn _get_db(&self) -> Option<Arc<Mutex<SessionDbStub>>> {
        Some(self.get_db_arc())
    }

    /// Mirrors `def _persist(self, state: SessionState) -> None` (423-507).
    fn persist(&self, state: &SessionState) {
        let db_arc = self.get_db_arc();
        let mut db = db_arc.lock().unwrap();

        let model_str = if state.model.is_empty() { None } else { Some(state.model.clone()) }; // 434
        let mut session_meta: HashMap<String, String> = HashMap::new();
        session_meta.insert("cwd".to_string(), state.cwd.clone()); // 435
        if let Some(ref provider) = state.agent.provider {
            if !provider.trim().is_empty() {
                session_meta.insert("provider".to_string(), provider.trim().to_string()); // 440
            }
        }
        if let Some(ref base_url) = state.agent.base_url {
            if !base_url.trim().is_empty() {
                session_meta.insert("base_url".to_string(), base_url.trim().to_string()); // 442
            }
        }
        if let Some(ref api_mode) = state.agent.api_mode {
            if !api_mode.trim().is_empty() {
                session_meta.insert("api_mode".to_string(), api_mode.trim().to_string()); // 444
            }
        }
        let cwd_json = json_dumps_model_config(&session_meta); // 445

        // Mirrors `existing = db.get_session(state.session_id)` (449)
        let existing = db.get_session(&state.session_id);
        if existing.is_none() {
            db.create_session(&state.session_id, "acp", model_str.clone(), session_meta.clone()); // 451-456
        } else {
            // Mirrors `db.update_session_meta(state.session_id, cwd_json, model_str)` (460)
            let _ = db.update_session_meta(&state.session_id, cwd_json.clone(), model_str.clone());
        }

        // Mirrors agent_owns_persistence guard (480-486)
        // `agent_db is db and _session_db_created`
        let agent_owns = if let Some(ptr) = state.agent.session_db_ptr {
            ptr == db.ptr_id() && state.agent.session_db_created // 482-485
        } else {
            false
        };
        if !agent_owns {
            // Mirrors `db.replace_messages(state.session_id, state.history, active_only=True)` (503-505)
            // Commentary lines 464-502 preserved: only live rows replaced so archived rows survive.
            let _ = db.replace_messages(&state.session_id, &state.history, true);
        }
        // If agent_owns == true, do nothing — agent already flushed incrementally.
    }

    /// Mirrors `def _restore(self, session_id: str) -> Optional[SessionState]` (509-587).
    fn restore(&self, session_id: &str) -> Option<SessionState> {
        let db_arc = self.get_db_arc();
        let db_guard = db_arc.lock().unwrap();
        let row = db_guard.get_session(session_id)?; // 518-524

        if row.source != "acp" {
            return None; // 527-528
        }

        // Extract cwd from model_config (531-545)
        let mut cwd = ".".to_string();
        let mut requested_provider = row.billing_provider.clone();
        let mut restored_base_url = row.billing_base_url.clone();
        let mut restored_api_mode: Option<String> = None;
        if let Some(ref mc) = row.model_config {
            // Try parse as JSON
            if let Some(c) = json_loads_get(mc, "cwd") {
                cwd = c;
            }
            if let Some(p) = json_loads_get(mc, "provider") {
                if !p.trim().is_empty() { requested_provider = Some(p); }
            }
            if let Some(b) = json_loads_get(mc, "base_url") {
                if !b.trim().is_empty() { restored_base_url = Some(b); }
            }
            if let Some(a) = json_loads_get(mc, "api_mode") {
                if !a.trim().is_empty() { restored_api_mode = Some(a); }
            }
        }

        let model = row.model.clone(); // 547

        // Load conversation history (549-560)
        let history = db_guard.get_messages_as_conversation(session_id, true);

        // Drop guard before make_agent (which may need db again) to avoid deadlock
        drop(db_guard);

        let agent = self.make_agent(
            session_id,
            &cwd,
            model.as_deref(),
            requested_provider.as_deref(),
            restored_base_url.as_deref(),
            restored_api_mode.as_deref(),
        ); // 563-570

        let mut state = SessionState::new(
            session_id.to_string(),
            agent.clone(),
            cwd.clone(),
            model.clone().unwrap_or_else(|| agent.model.clone()),
        );
        state.history = history.clone();
        // Mirrors `with self._lock: self._sessions[session_id] = state` (583-584)
        {
            let _guard = self.lock.lock().unwrap();
            self.sessions.lock().unwrap().insert(session_id.to_string(), state.clone());
        }
        register_task_cwd(session_id, &cwd); // 585
        eprintln!("[{}] Restored ACP session {} from DB ({} messages)", logger_name(), session_id, history.len()); // 586
        Some(state)
    }

    /// Mirrors `def _delete_persisted(self, session_id: str) -> bool` (589-598).
    fn delete_persisted(&self, session_id: &str) -> bool {
        let db_arc = self.get_db_arc();
        let mut db = db_arc.lock().unwrap();
        db.delete_session(session_id)
    }

    // ---- internal -----------------------------------------------------------

    /// Mirrors `def _make_agent(self, *, session_id, cwd, model, requested_provider, base_url, api_mode)` (602-695).
    fn make_agent(
        &self,
        session_id: &str,
        cwd: &str,
        model: Option<&str>,
        requested_provider: Option<&str>,
        base_url: Option<&str>,
        api_mode: Option<&str>,
    ) -> AgentStub {
        if let Some(ref factory) = self.agent_factory {
            return factory(); // 612-613
        }

        // Mirrors `from run_agent import AIAgent; from hermes_cli.config import load_config; ...` (615-617)
        // In Rust (NEVER cargo) we stub config loading and provider resolution.

        // Mirrors `config = load_config(); model_cfg = config.get("model"); ...` (619-627)
        let default_model = ""; // Stub: no config.yaml linked in this slice
        let config_provider: Option<String> = None;

        // Mirrors `configured_mcp_servers = [name for name, cfg in (config.get("mcp_servers") or {}).items() if ...]` (629-633)
        let configured_mcp_servers: Vec<String> = Vec::new(); // stub

        // Mirrors `kwargs = { "platform": "acp", "enabled_toolsets": _expand_acp_enabled_toolsets(...), ... }` (635-645)
        let enabled_toolsets = expand_acp_enabled_toolsets(Some(vec!["hermes-acp".to_string()]), Some(configured_mcp_servers));
        let _ = enabled_toolsets; // used to build agent

        let model_val = model.unwrap_or(default_model).to_string();

        // Mirrors `try: runtime = resolve_runtime_provider(requested=...) ; kwargs.update(...) except: logger.debug` (647-660)
        let mut provider = requested_provider.or(config_provider.as_deref()).map(|s| s.to_string());
        let mut resolved_base_url = base_url.map(|s| s.to_string());
        let mut resolved_api_mode = api_mode.map(|s| s.to_string());
        // Stub: if no provider, default to openrouter-like resolution
        if provider.is_none() {
            provider = Some("openrouter".to_string());
        }
        let _ = (resolved_base_url.clone(), resolved_api_mode.clone());

        register_task_cwd(session_id, cwd); // 662

        // Mirrors bounded wait for background MCP discovery (664-685)
        // `ensure_mcp_discovery_before_agent_build(logger, thread_name="acp-mcp-discovery")`
        // Stub: best-effort no-op with debug on failure

        let mut agent = AgentStub {
            model: if model_val.is_empty() { "unknown".to_string() } else { model_val },
            provider: provider.clone(),
            base_url: resolved_base_url.clone(),
            api_mode: resolved_api_mode.clone(),
            session_id: session_id.to_string(),
            session_cwd: cwd.to_string(), // 691
            session_db_ptr: Some(self.get_db_arc().lock().unwrap().ptr_id()), // approximates `_session_db is db`
            session_db_created: false, // stub: fresh agent not yet owning DB compaction rows
        };
        // Mirrors `agent.session_cwd = cwd` (691) and `agent._print_fn = _acp_stderr_print` (694)
        let _ = agent.session_cwd.clone();
        agent
    }

    #[allow(dead_code)]
    pub fn _make_agent(
        &self,
        session_id: &str,
        cwd: &str,
        model: Option<&str>,
        requested_provider: Option<&str>,
        base_url: Option<&str>,
        api_mode: Option<&str>,
    ) -> AgentStub {
        self.make_agent(session_id, cwd, model, requested_provider, base_url, api_mode)
    }
}

/// Info returned by `list_sessions` — mirrors the dict built at 315-352.
#[derive(Debug, Clone)]
pub struct ListSessionInfo {
    pub session_id: String,
    pub cwd: String,
    pub model: String,
    pub history_len: usize,
    pub title: String,
    pub updated_at: Option<String>,
}

// ---------------------------------------------------------------------------
// tiny helpers — uuid, path utils
// ---------------------------------------------------------------------------

fn new_uuid4() -> String {
    // Mirrors `str(uuid.uuid4())` (215, 262). std-only pseudo-UUID: use time + counter.
    // Not cryptographically strong, but unique enough for session ids in this stub.
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(0);
    let count = CTR.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let nanos = now.as_nanos();
    // Mix nanos and counter into 128-bit-ish hex
    let hi = (nanos >> 64) as u64 ^ count.wrapping_mul(0x9E3779B97F4A7C15);
    let lo = (nanos as u64) ^ count.rotate_left(13);
    format!("{hi:016x}-{lo:016x}-4{lo:03x}-a{:03x}-{:012x}", lo & 0xfff, lo & 0xfff, lo & 0xffffffffffff)
}

// ---------------------------------------------------------------------------
// Re-exports for 1:1 traceability — underscore-prefixed aliases
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub fn _build_session_title_alias(title: Option<&str>, preview: Option<&str>, cwd: Option<&str>) -> String {
    build_session_title(title, preview, cwd)
}

#[allow(dead_code)]
pub fn _format_updated_at_alias(v: &UpdatedAtValue) -> Option<String> {
    format_updated_at(v)
}

#[allow(dead_code)]
pub fn _updated_at_sort_key_alias(v: Option<&str>) -> f64 {
    updated_at_sort_key(v, None)
}

#[allow(dead_code)]
pub fn _register_task_cwd_alias(a: &str, b: &str) { register_task_cwd(a,b) }
#[allow(dead_code)]
pub fn _clear_task_cwd_alias(a: &str) { clear_task_cwd(a) }
#[allow(dead_code)]
pub fn _expand_acp_enabled_toolsets_alias(a: Option<Vec<String>>, b: Option<Vec<String>>) -> Vec<String> {
    expand_acp_enabled_toolsets(a,b)
}
