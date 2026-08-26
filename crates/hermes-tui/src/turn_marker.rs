//! Durable interrupted-turn markers for the desktop/TUI auto-continue path.
//!
//! 1:1 port of `tui_gateway/turn_marker.py` (159 lines).
//!
//! A running turn's progress lives only in process memory (the agent flushes to
//! SQLite at turn end, not mid-turn), so an app/backend/machine death mid-turn
//! leaves no durable trace of the interrupted prompt. This sidecar is that
//! trace: a marker is written when a turn starts running and cleared when the
//! turn concludes — success, handled error, or interrupt all clear it, so only
//! a process death leaves one behind. `session.resume` reads the marker to
//! decide whether to auto-continue the interrupted turn (see
//! `_maybe_schedule_auto_continue` in `tui_gateway/server.py`).
//!
//! Markers are stored per `HERMES_HOME` (callers pass the session's home so
//! profile sessions keep their state in their own profile directory) and the
//! file is bounded: writes prune entries older than `_MAX_AGE_SECS` and cap
//! the total count, so an unlucky streak of crashes can't grow it unboundedly.
//!
//! Every function is best-effort by design — marker bookkeeping must never
//! break a turn — so I/O errors degrade to "no marker" instead of raising.
//!
//! ```python
//! # Python — tui_gateway/turn_marker.py
//! _MARKER_DIR = "desktop"
//! _MARKER_FILE = "interrupted_turns.json"
//! _MAX_AGE_SECS = 24 * 3600
//! _MAX_ENTRIES = 32
//! _MAX_PROMPT_CHARS = 64_000
//! _lock = threading.Lock()
//! def _marker_path(home: Path | str) -> Path: ...
//! def _load(path: Path) -> dict[str, dict]: ...
//! def _prune(entries: dict[str, dict], now: float) -> dict[str, dict]: ...
//! def _store(path: Path, entries: dict[str, dict]) -> None: ...
//! def record_turn_start(home, session_key, prompt, *, attempts=0) -> None: ...
//! def clear_turn_marker(home, session_key) -> None: ...
//! def read_turn_marker(home, session_key) -> dict | None: ...
//! ```
//!
//! # Rust mapping
//!
//! * `_MARKER_DIR` / `_MARKER_FILE` / `_MAX_AGE_SECS` / `_MAX_ENTRIES` /
//!   `_MAX_PROMPT_CHARS` → [`MARKER_DIR`] / [`MARKER_FILE`] / [`MAX_AGE_SECS`] /
//!   [`MAX_ENTRIES`] / [`MAX_PROMPT_CHARS`] (identical values).
//! * `_lock = threading.Lock()` → `OnceLock<Mutex<()>>` global (`GLOBAL_LOCK`).
//!   Every public entry (`record_turn_start`, `clear_turn_marker`,
//!   `read_turn_marker`) holds the lock across the `load → prune/mutate → store`
//!   critical section, mirroring `with _lock:`.
//! * `Path(home) / _MARKER_DIR / _MARKER_FILE` → [`marker_path`].
//! * `_load` (`open` + `json.load` + `except FileNotFound` + `except Exception` +
//!   `isinstance(data, dict)` + `{k:v for ... if isinstance(v, dict)}`) →
//!   [`load`] (std-only JSON parser; `FileNotFound` → `{}`, unreadable/invalid
//!   → `{}`, non-dict root → `{}`, non-dict values filtered).
//! * `_prune` (age filter `now - float(entry.get("started_at") or 0) <= _MAX_AGE_SECS`,
//!   then keep newest `_MAX_ENTRIES` by `started_at` descending) → [`prune`].
//! * `_store` (`if not entries: unlink(missing_ok=True); else: mkdir(parents) +
//!   `mkstemp` + `json.dump` + `os.replace` + `unlink(tmp) on exception`) →
//!   [`store`] (create parent, write to `.turn-marker-<pid>-<nanos>` temp in same
//!   dir, `fs::rename` atomic replace, `remove_file(tmp)` on failure, `remove_file`
//!   when empty).
//! * `record_turn_start` (`if not session_key or not prompt: return` + `time.time()` +
//!   `{"attempts": max(0,int(attempts)), "prompt": prompt[:_MAX_PROMPT_CHARS],
//!   "started_at": now}` + `with _lock: entries=_prune(_load(path),now);
//!   entries[session_key]=entry; _store`) → [`record_turn_start`] (chars-truncated
//!   prompt via `chars().take`, `attempts.max(0)`, same lock+prune+insert+store,
//!   best-effort `log::debug` on failure when `log` feature enabled).
//! * `clear_turn_marker` (`if not session_key: return` + `with _lock: entries=_load;
//!   if key not in: return; del; _store`) → [`clear_turn_marker`].
//! * `read_turn_marker` (`if not session_key: return None` + `with _lock: entry=_load(...).get` +
//!   `if not isinstance(entry,dict): None` + `prompt=str(entry.get("prompt") or "")` +
//!   `if not prompt.strip(): None` + `started_at=float(entry.get("started_at") or 0)` +
//!   `attempts=max(0,int(entry.get("attempts") or 0))` + `except (TypeError,ValueError): None`)
//!   → [`read_turn_marker`] (truthiness of `JsonValue` mirrors Python's `or`, strict
//!   `f64`/`i64` parsing for the typed fields, whitespace-trim check for prompt).
//! * `json.dump` / `json.load` → std-only minimal JSON engine (`JsonValue` +
//!   `parse_value` / `encode_entries`) handling the exact marker shape
//!   (`{key: {attempts:number, prompt:string, started_at:number}}`) with
//!   escaped-string support, so the crate stays `std`-only like the other
//!   `hermes-tui` modules (cf. `methods_images::parse_generate_result`).

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Constants — mirrors turn_marker.py:34-40
// ---------------------------------------------------------------------------

/// Subdirectory inside `HERMES_HOME`. Mirrors `_MARKER_DIR = "desktop"`.
pub const MARKER_DIR: &str = "desktop";

/// File name. Mirrors `_MARKER_FILE = "interrupted_turns.json"`.
pub const MARKER_FILE: &str = "interrupted_turns.json";

/// Max age in seconds. Mirrors `_MAX_AGE_SECS = 24 * 3600`.
pub const MAX_AGE_SECS: u64 = 24 * 3600;

/// Max number of entries. Mirrors `_MAX_ENTRIES = 32`.
pub const MAX_ENTRIES: usize = 32;

/// Max prompt length in chars. Mirrors `_MAX_PROMPT_CHARS = 64_000`.
pub const MAX_PROMPT_CHARS: usize = 64_000;

// ---------------------------------------------------------------------------
// Global lock — mirrors `_lock = threading.Lock()`
// ---------------------------------------------------------------------------

static GLOBAL_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn global_lock() -> &'static Mutex<()> {
    GLOBAL_LOCK.get_or_init(|| Mutex::new(()))
}

// ---------------------------------------------------------------------------
// TurnMarker — mirrors the dict returned by read_turn_marker
// ---------------------------------------------------------------------------

/// Marker left by a turn that never concluded.
///
/// Mirrors `{"attempts": int, "prompt": str, "started_at": float}`.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnMarker {
    /// Mirrors `attempts: max(0, int(entry.get("attempts") or 0))`.
    pub attempts: i64,
    /// Mirrors `prompt: str(entry.get("prompt") or "")` (non-empty after trim).
    pub prompt: String,
    /// Mirrors `started_at: float(entry.get("started_at") or 0)`.
    pub started_at: f64,
}

// ---------------------------------------------------------------------------
// JsonValue — minimal JSON engine (std-only)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum JsonValue {
    Null,
    Bool(bool),
    Number(String),
    Str(String),
    Object(HashMap<String, JsonValue>),
    Array(Vec<JsonValue>),
}

fn is_truthy_value(v: &JsonValue) -> bool {
    match v {
        JsonValue::Null => false,
        JsonValue::Bool(b) => *b,
        JsonValue::Number(s) => s.parse::<f64>().map(|n| n != 0.0).unwrap_or(true),
        JsonValue::Str(s) => !s.is_empty(),
        JsonValue::Array(a) => !a.is_empty(),
        JsonValue::Object(m) => !m.is_empty(),
    }
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push(ch),
        }
    }
    out
}

fn encode_json_value(v: &JsonValue) -> String {
    match v {
        JsonValue::Null => "null".to_string(),
        JsonValue::Bool(true) => "true".to_string(),
        JsonValue::Bool(false) => "false".to_string(),
        JsonValue::Number(s) => s.clone(),
        JsonValue::Str(s) => format!("\"{}\"", json_escape(s)),
        JsonValue::Object(m) => {
            let mut out = String::from("{");
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let mut first = true;
            for k in keys {
                if !first {
                    out.push(',');
                }
                first = false;
                out.push('"');
                out.push_str(&json_escape(k));
                out.push_str("\":");
                out.push_str(&encode_json_value(&m[k]));
            }
            out.push('}');
            out
        }
        JsonValue::Array(arr) => {
            let mut out = String::from("[");
            let mut first = true;
            for e in arr {
                if !first {
                    out.push(',');
                }
                first = false;
                out.push_str(&encode_json_value(e));
            }
            out.push(']');
            out
        }
    }
}

fn skip_ws(s: &str, pos: &mut usize) {
    while *pos < s.len() {
        let ch = s[*pos..].chars().next().unwrap();
        if ch.is_whitespace() {
            *pos += ch.len_utf8();
        } else {
            break;
        }
    }
}

fn parse_string(s: &str, pos: &mut usize) -> Option<String> {
    if *pos >= s.len() || s.as_bytes()[*pos] != b'"' {
        return None;
    }
    *pos += 1; // opening "
    let mut out = String::new();
    while *pos < s.len() {
        let ch = s[*pos..].chars().next().unwrap();
        let len = ch.len_utf8();
        if ch == '\\' {
            *pos += len;
            if *pos >= s.len() {
                return None;
            }
            let esc = s[*pos..].chars().next().unwrap();
            let elen = esc.len_utf8();
            match esc {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'b' => out.push('\u{0008}'),
                'f' => out.push('\u{000C}'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    *pos += elen;
                    if *pos + 4 > s.len() {
                        return None;
                    }
                    let hex = &s[*pos..*pos + 4];
                    if hex.len() != 4 {
                        return None;
                    }
                    let code = u32::from_str_radix(hex, 16).ok()?;
                    let c = char::from_u32(code)?;
                    out.push(c);
                    *pos += 4;
                    continue;
                }
                _ => out.push(esc),
            }
            *pos += elen;
        } else if ch == '"' {
            *pos += len;
            return Some(out);
        } else {
            out.push(ch);
            *pos += len;
        }
    }
    None
}

fn parse_number(s: &str, pos: &mut usize) -> Option<String> {
    let start = *pos;
    if *pos < s.len() && s.as_bytes()[*pos] == b'-' {
        *pos += 1;
    }
    let mut has_digits = false;
    while *pos < s.len() && s.as_bytes()[*pos].is_ascii_digit() {
        *pos += 1;
        has_digits = true;
    }
    if *pos < s.len() && s.as_bytes()[*pos] == b'.' {
        *pos += 1;
        while *pos < s.len() && s.as_bytes()[*pos].is_ascii_digit() {
            *pos += 1;
            has_digits = true;
        }
    }
    if *pos < s.len() && (s.as_bytes()[*pos] == b'e' || s.as_bytes()[*pos] == b'E') {
        *pos += 1;
        if *pos < s.len() && (s.as_bytes()[*pos] == b'+' || s.as_bytes()[*pos] == b'-') {
            *pos += 1;
        }
        let mut exp_digits = false;
        while *pos < s.len() && s.as_bytes()[*pos].is_ascii_digit() {
            *pos += 1;
            exp_digits = true;
        }
        if !exp_digits {
            return None;
        }
    }
    if !has_digits {
        return None;
    }
    Some(s[start..*pos].to_string())
}

fn parse_value(s: &str, pos: &mut usize) -> Option<JsonValue> {
    skip_ws(s, pos);
    if *pos >= s.len() {
        return None;
    }
    let b = s.as_bytes()[*pos];
    if b == b'"' {
        let st = parse_string(s, pos)?;
        return Some(JsonValue::Str(st));
    }
    if b == b'{' {
        return parse_object(s, pos);
    }
    if b == b'[' {
        return parse_array(s, pos);
    }
    if s[*pos..].starts_with("true") {
        *pos += 4;
        return Some(JsonValue::Bool(true));
    }
    if s[*pos..].starts_with("false") {
        *pos += 5;
        return Some(JsonValue::Bool(false));
    }
    if s[*pos..].starts_with("null") {
        *pos += 4;
        return Some(JsonValue::Null);
    }
    if b == b'-' || b.is_ascii_digit() {
        let num = parse_number(s, pos)?;
        return Some(JsonValue::Number(num));
    }
    None
}

fn parse_object(s: &str, pos: &mut usize) -> Option<JsonValue> {
    if *pos >= s.len() || s.as_bytes()[*pos] != b'{' {
        return None;
    }
    *pos += 1; // {
    skip_ws(s, pos);
    let mut map = HashMap::new();
    if *pos < s.len() && s.as_bytes()[*pos] == b'}' {
        *pos += 1;
        return Some(JsonValue::Object(map));
    }
    loop {
        skip_ws(s, pos);
        let key = parse_string(s, pos)?;
        skip_ws(s, pos);
        if *pos >= s.len() || s.as_bytes()[*pos] != b':' {
            return None;
        }
        *pos += 1; // :
        let val = parse_value(s, pos)?;
        map.insert(key, val);
        skip_ws(s, pos);
        if *pos >= s.len() {
            return None;
        }
        let c = s.as_bytes()[*pos];
        if c == b',' {
            *pos += 1;
            continue;
        }
        if c == b'}' {
            *pos += 1;
            return Some(JsonValue::Object(map));
        }
        return None;
    }
}

fn parse_array(s: &str, pos: &mut usize) -> Option<JsonValue> {
    if *pos >= s.len() || s.as_bytes()[*pos] != b'[' {
        return None;
    }
    *pos += 1; // [
    skip_ws(s, pos);
    let mut arr = Vec::new();
    if *pos < s.len() && s.as_bytes()[*pos] == b']' {
        *pos += 1;
        return Some(JsonValue::Array(arr));
    }
    loop {
        let val = parse_value(s, pos)?;
        arr.push(val);
        skip_ws(s, pos);
        if *pos >= s.len() {
            return None;
        }
        let c = s.as_bytes()[*pos];
        if c == b',' {
            *pos += 1;
            continue;
        }
        if c == b']' {
            *pos += 1;
            return Some(JsonValue::Array(arr));
        }
        return None;
    }
}

fn parse_root_object(s: &str) -> Option<HashMap<String, JsonValue>> {
    let mut pos = 0;
    skip_ws(s, &mut pos);
    if pos >= s.len() {
        return None;
    }
    let val = parse_value(s, &mut pos)?;
    skip_ws(s, &mut pos);
    if pos != s.len() {
        return None;
    }
    if let JsonValue::Object(m) = val {
        Some(m)
    } else {
        None
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs_f64()
}

// ---------------------------------------------------------------------------
// Public path helper — mirrors _marker_path
// ---------------------------------------------------------------------------

/// Marker file path for `home`. Mirrors `_marker_path`.
///
/// ```python
/// def _marker_path(home: Path | str) -> Path:
///     return Path(home) / _MARKER_DIR / _MARKER_FILE
/// ```
pub fn marker_path(home: impl AsRef<Path>) -> PathBuf {
    home.as_ref().join(MARKER_DIR).join(MARKER_FILE)
}

// ---------------------------------------------------------------------------
// _load / _prune / _store — mirrors Python helpers
// ---------------------------------------------------------------------------

/// Load marker entries. Mirrors `_load`.
///
/// * `FileNotFound` → `{}`
/// * unreadable / invalid JSON / non-dict root → `{}`
/// * non-dict values filtered (`isinstance(v, dict)`).
pub fn load(path: &Path) -> HashMap<String, HashMap<String, JsonValue>> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return HashMap::new(),
        Err(_) => {
            #[cfg(feature = "log")]
            log::debug!("unreadable turn-marker file {}; starting fresh", path.display());
            return HashMap::new();
        }
    };
    let map = match parse_root_object(&content) {
        Some(m) => m,
        None => {
            #[cfg(feature = "log")]
            log::debug!("unreadable turn-marker file {}; starting fresh", path.display());
            return HashMap::new();
        }
    };
    let mut out = HashMap::new();
    for (k, v) in map {
        if let JsonValue::Object(inner) = v {
            out.insert(k, inner);
        }
    }
    out
}

fn get_started_at_raw(inner: &HashMap<String, JsonValue>) -> f64 {
    match inner.get("started_at") {
        Some(JsonValue::Number(s)) => s.parse::<f64>().unwrap_or(0.0),
        Some(JsonValue::Str(s)) => s.parse::<f64>().unwrap_or(0.0),
        Some(JsonValue::Bool(true)) => 1.0,
        Some(JsonValue::Bool(false)) => 0.0,
        _ => 0.0,
    }
}

/// Prune stale and excess entries. Mirrors `_prune`.
///
/// ```python
/// def _prune(entries: dict[str, dict], now: float) -> dict[str, dict]:
///     fresh = {key: entry for key, entry in entries.items()
///              if now - float(entry.get("started_at") or 0) <= _MAX_AGE_SECS}
///     if len(fresh) <= _MAX_ENTRIES: return fresh
///     newest = sorted(fresh.items(), key=lambda item: float(item[1].get("started_at") or 0), reverse=True)[:_MAX_ENTRIES]
///     return dict(newest)
/// ```
pub fn prune(
    entries: HashMap<String, HashMap<String, JsonValue>>,
    now: f64,
) -> HashMap<String, HashMap<String, JsonValue>> {
    let mut fresh = HashMap::new();
    for (k, v) in entries {
        let started = get_started_at_raw(&v);
        if now - started <= MAX_AGE_SECS as f64 {
            fresh.insert(k, v);
        }
    }
    if fresh.len() <= MAX_ENTRIES {
        return fresh;
    }
    let mut vec: Vec<(String, HashMap<String, JsonValue>, f64)> = fresh
        .into_iter()
        .map(|(k, v)| {
            let s = get_started_at_raw(&v);
            (k, v, s)
        })
        .collect();
    vec.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    vec.truncate(MAX_ENTRIES);
    vec.into_iter().map(|(k, v, _)| (k, v)).collect()
}

fn encode_entries(entries: &HashMap<String, HashMap<String, JsonValue>>) -> String {
    let mut out = String::from("{");
    let mut keys: Vec<&String> = entries.keys().collect();
    keys.sort();
    let mut first = true;
    for k in keys {
        if !first {
            out.push(',');
        }
        first = false;
        out.push('"');
        out.push_str(&json_escape(k));
        out.push_str("\":");
        let inner = &entries[k];
        out.push('{');
        let mut inner_keys: Vec<&String> = inner.keys().collect();
        inner_keys.sort();
        let mut first_inner = true;
        for ik in inner_keys {
            if !first_inner {
                out.push(',');
            }
            first_inner = false;
            out.push('"');
            out.push_str(&json_escape(ik));
            out.push_str("\":");
            out.push_str(&encode_json_value(&inner[ik]));
        }
        out.push('}');
    }
    out.push('}');
    out
}

/// Persist entries. Mirrors `_store`.
///
/// * empty → `unlink(missing_ok=True)`
/// * otherwise `mkdir(parents)` + `mkstemp` + `json.dump` + `os.replace`
///   (atomic via `fs::rename`), `unlink(tmp)` on failure.
pub fn store(path: &Path, entries: &HashMap<String, HashMap<String, JsonValue>>) -> io::Result<()> {
    if entries.is_empty() {
        match fs::remove_file(path) {
            Ok(_) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = encode_entries(entries);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp_name = format!(
        ".turn-marker-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos()
    );
    let tmp_path = parent.join(tmp_name);
    let write_res = (|| -> io::Result<()> {
        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        Ok(())
    })();
    if write_res.is_err() {
        let _ = fs::remove_file(&tmp_path);
        return write_res;
    }
    match fs::rename(&tmp_path, path) {
        Ok(_) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            Err(e)
        }
    }
}

// ---------------------------------------------------------------------------
// Public API — mirrors record_turn_start / clear_turn_marker / read_turn_marker
// ---------------------------------------------------------------------------

/// Persist the marker for a turn that is about to run.
///
/// Mirrors `record_turn_start`:
///
/// ```python
/// def record_turn_start(home, session_key, prompt, *, attempts=0) -> None:
///     if not session_key or not prompt: return
///     now = time.time()
///     entry = {"attempts": max(0,int(attempts)), "prompt": prompt[:_MAX_PROMPT_CHARS], "started_at": now}
///     try:
///         with _lock:
///             path = _marker_path(home)
///             entries = _prune(_load(path), now)
///             entries[session_key] = entry
///             _store(path, entries)
///     except Exception:
///         logger.debug("failed to record turn marker for %s", session_key, exc_info=True)
/// ```
///
/// Best-effort: I/O errors are swallowed (debug-logged when `log` feature is enabled).
pub fn record_turn_start(
    home: impl AsRef<Path>,
    session_key: &str,
    prompt: &str,
    attempts: i64,
) {
    if session_key.is_empty() || prompt.is_empty() {
        return;
    }
    let guard = global_lock().lock().unwrap();
    let now = now_secs();
    let truncated: String = prompt.chars().take(MAX_PROMPT_CHARS).collect();
    let attempts_clamped = attempts.max(0);
    let mut entry = HashMap::new();
    entry.insert(
        "attempts".to_string(),
        JsonValue::Number(attempts_clamped.to_string()),
    );
    entry.insert("prompt".to_string(), JsonValue::Str(truncated));
    entry.insert(
        "started_at".to_string(),
        JsonValue::Number(now.to_string()),
    );
    let path = marker_path(home.as_ref());
    let mut entries = load(&path);
    entries = prune(entries, now);
    entries.insert(session_key.to_string(), entry);
    let res = store(&path, &entries);
    if let Err(e) = res {
        #[cfg(feature = "log")]
        log::debug!("failed to record turn marker for {}: {}", session_key, e);
        let _ = e;
    }
    drop(guard);
}

/// Convenience wrapper with `attempts = 0`.
///
/// Mirrors the default-arg call `record_turn_start(home, key, prompt)`.
pub fn record_turn_start_simple(
    home: impl AsRef<Path>,
    session_key: &str,
    prompt: &str,
) {
    record_turn_start(home, session_key, prompt, 0);
}

/// Remove the marker once its turn concluded.
///
/// Mirrors `clear_turn_marker`:
///
/// ```python
/// def clear_turn_marker(home, session_key) -> None:
///     if not session_key: return
///     try:
///         with _lock:
///             path = _marker_path(home)
///             entries = _load(path)
///             if session_key not in entries: return
///             del entries[session_key]
///             _store(path, entries)
///     except Exception:
///         logger.debug("failed to clear turn marker for %s", session_key, exc_info=True)
/// ```
pub fn clear_turn_marker(home: impl AsRef<Path>, session_key: &str) {
    if session_key.is_empty() {
        return;
    }
    let guard = global_lock().lock().unwrap();
    let path = marker_path(home.as_ref());
    let mut entries = load(&path);
    if !entries.contains_key(session_key) {
        return;
    }
    entries.remove(session_key);
    let res = store(&path, &entries);
    if let Err(e) = res {
        #[cfg(feature = "log")]
        log::debug!("failed to clear turn marker for {}: {}", session_key, e);
        let _ = e;
    }
    drop(guard);
}

fn get_prompt_string(inner: &HashMap<String, JsonValue>) -> Option<String> {
    let v = inner.get("prompt")?;
    if !is_truthy_value(v) {
        return Some(String::new());
    }
    let s = match v {
        JsonValue::Str(s) => s.clone(),
        JsonValue::Number(s) => s.clone(),
        JsonValue::Bool(true) => "True".to_string(),
        JsonValue::Bool(false) => "False".to_string(),
        JsonValue::Null => String::new(),
        JsonValue::Object(_) | JsonValue::Array(_) => encode_json_value(v),
    };
    Some(s)
}

fn parse_started_at_strict(inner: &HashMap<String, JsonValue>) -> Result<f64, ()> {
    let v = inner.get("started_at");
    let raw = match v {
        None => return Ok(0.0),
        Some(val) if !is_truthy_value(val) => return Ok(0.0),
        Some(val) => val,
    };
    match raw {
        JsonValue::Number(s) => s.parse::<f64>().map_err(|_| ()),
        JsonValue::Str(s) => s.parse::<f64>().map_err(|_| ()),
        JsonValue::Bool(true) => Ok(1.0),
        JsonValue::Bool(false) => Ok(0.0),
        _ => Err(()),
    }
}

fn parse_attempts_strict(inner: &HashMap<String, JsonValue>) -> Result<i64, ()> {
    let v = inner.get("attempts");
    let raw = match v {
        None => return Ok(0),
        Some(val) if !is_truthy_value(val) => return Ok(0),
        Some(val) => val,
    };
    match raw {
        JsonValue::Number(s) => {
            let f: f64 = s.parse().map_err(|_| ())?;
            Ok((f as i64).max(0))
        }
        JsonValue::Str(s) => {
            let t = s.trim();
            if t.is_empty() {
                return Ok(0);
            }
            let n: i64 = t.parse().map_err(|_| ())?;
            Ok(n.max(0))
        }
        JsonValue::Bool(b) => Ok(if *b { 1 } else { 0 }),
        _ => Err(()),
    }
}

/// The marker left by a turn that never concluded, or `None`.
///
/// Mirrors `read_turn_marker`:
///
/// ```python
/// def read_turn_marker(home, session_key) -> dict | None:
///     if not session_key: return None
///     try:
///         with _lock:
///             entry = _load(_marker_path(home)).get(session_key)
///     except Exception: return None
///     if not isinstance(entry, dict): return None
///     prompt = str(entry.get("prompt") or "")
///     if not prompt.strip(): return None
///     try:
///         started_at = float(entry.get("started_at") or 0)
///         attempts = max(0, int(entry.get("attempts") or 0))
///     except (TypeError, ValueError): return None
///     return {"attempts": attempts, "prompt": prompt, "started_at": started_at}
/// ```
pub fn read_turn_marker(home: impl AsRef<Path>, session_key: &str) -> Option<TurnMarker> {
    if session_key.is_empty() {
        return None;
    }
    let guard = global_lock().lock().unwrap();
    let path = marker_path(home.as_ref());
    let entries = load(&path);
    let inner = entries.get(session_key)?;
    let prompt = get_prompt_string(inner)?;
    if prompt.trim().is_empty() {
        return None;
    }
    let started_at = parse_started_at_strict(inner).ok()?;
    let attempts = parse_attempts_strict(inner).ok()?;
    drop(guard);
    Some(TurnMarker {
        attempts,
        prompt,
        started_at,
    })
}

// ---------------------------------------------------------------------------
// Tests — mirror Python invariants
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_dir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "hermes-tui-turn-marker-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn constants_match_python() {
        assert_eq!(MARKER_DIR, "desktop");
        assert_eq!(MARKER_FILE, "interrupted_turns.json");
        assert_eq!(MAX_AGE_SECS, 24 * 3600);
        assert_eq!(MAX_ENTRIES, 32);
        assert_eq!(MAX_PROMPT_CHARS, 64_000);
    }

    #[test]
    fn marker_path_joins() {
        let p = marker_path("/tmp/home");
        assert_eq!(p, PathBuf::from("/tmp/home/desktop/interrupted_turns.json"));
        let p2 = marker_path(PathBuf::from("/a/b"));
        assert_eq!(p2, PathBuf::from("/a/b/desktop/interrupted_turns.json"));
    }

    #[test]
    fn load_missing_is_empty() {
        let dir = test_dir();
        let path = marker_path(&dir);
        let m = load(&path);
        assert!(m.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_invalid_and_non_dict_filtered() {
        let dir = test_dir();
        let path = marker_path(&dir);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        // invalid json
        fs::write(&path, b"not json").unwrap();
        assert!(load(&path).is_empty());
        // non-dict root (array)
        fs::write(&path, b"[1,2,3]").unwrap();
        assert!(load(&path).is_empty());
        // dict with non-dict values filtered
        fs::write(&path, br#"{"a": 123, "b": {"prompt":"hi","started_at":1,"attempts":0}}"#).unwrap();
        let m = load(&path);
        assert_eq!(m.len(), 1);
        assert!(m.contains_key("b"));
        assert!(!m.contains_key("a"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_empty_unlinks() {
        let dir = test_dir();
        let path = marker_path(&dir);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{}").unwrap();
        assert!(path.exists());
        store(&path, &HashMap::new()).unwrap();
        assert!(!path.exists());
        // second unlink is no-op
        store(&path, &HashMap::new()).unwrap();
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_age_and_cap() {
        let now = 1_000_000.0;
        let mut entries: HashMap<String, HashMap<String, JsonValue>> = HashMap::new();
        // fresh entry
        let mut fresh = HashMap::new();
        fresh.insert("prompt".to_string(), JsonValue::Str("hi".into()));
        fresh.insert("started_at".to_string(), JsonValue::Number(now.to_string()));
        fresh.insert("attempts".to_string(), JsonValue::Number("0".into()));
        entries.insert("fresh".into(), fresh);
        // stale entry (>24h)
        let mut stale = HashMap::new();
        stale.insert("prompt".to_string(), JsonValue::Str("old".into()));
        stale.insert(
            "started_at".to_string(),
            JsonValue::Number((now - MAX_AGE_SECS as f64 - 1.0).to_string()),
        );
        stale.insert("attempts".to_string(), JsonValue::Number("0".into()));
        entries.insert("stale".into(), stale);
        // entry missing started_at => 0 => stale
        let mut missing = HashMap::new();
        missing.insert("prompt".to_string(), JsonValue::Str("x".into()));
        missing.insert("attempts".to_string(), JsonValue::Number("0".into()));
        entries.insert("missing".into(), missing);

        let pruned = prune(entries, now);
        assert!(pruned.contains_key("fresh"));
        assert!(!pruned.contains_key("stale"));
        assert!(!pruned.contains_key("missing"));

        // cap: create 40 fresh entries with distinct started_at, keep newest 32
        let mut many = HashMap::new();
        for i in 0..40 {
            let mut e = HashMap::new();
            e.insert("prompt".to_string(), JsonValue::Str(format!("p{}", i)));
            e.insert(
                "started_at".to_string(),
                JsonValue::Number((now - i as f64).to_string()),
            );
            e.insert("attempts".to_string(), JsonValue::Number("0".into()));
            many.insert(format!("k{}", i), e);
        }
        let pruned2 = prune(many, now);
        assert_eq!(pruned2.len(), MAX_ENTRIES);
        // newest should be k0..k31 remain, k32..k39 evicted (since k0 has highest started_at)
        assert!(pruned2.contains_key("k0"));
        assert!(pruned2.contains_key("k31"));
        assert!(!pruned2.contains_key("k32"));
        assert!(!pruned2.contains_key("k39"));
    }

    #[test]
    fn record_and_read_roundtrip() {
        let dir = test_dir();
        record_turn_start(&dir, "sess1", "hello world", 0);
        let m = read_turn_marker(&dir, "sess1").unwrap();
        assert_eq!(m.prompt, "hello world");
        assert_eq!(m.attempts, 0);
        assert!(m.started_at > 0.0);
        // overwrite same key with new prompt and attempts
        record_turn_start(&dir, "sess1", "second", 3);
        let m2 = read_turn_marker(&dir, "sess1").unwrap();
        assert_eq!(m2.prompt, "second");
        assert_eq!(m2.attempts, 3);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_empty_key_or_prompt_is_noop() {
        let dir = test_dir();
        record_turn_start(&dir, "", "hi", 0);
        record_turn_start(&dir, "k", "", 0);
        let path = marker_path(&dir);
        assert!(!path.exists());
        // also record with attempts negative clamped
        record_turn_start(&dir, "k2", "hi", -5);
        let m = read_turn_marker(&dir, "k2").unwrap();
        assert_eq!(m.attempts, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prompt_truncation_and_attempts_clamp() {
        let dir = test_dir();
        let long = "a".repeat(MAX_PROMPT_CHARS + 100);
        record_turn_start(&dir, "k", &long, 5);
        let m = read_turn_marker(&dir, "k").unwrap();
        assert_eq!(m.prompt.chars().count(), MAX_PROMPT_CHARS);
        assert_eq!(m.attempts, 5);
        // negative attempts
        record_turn_start(&dir, "k2", "hi", -10);
        assert_eq!(read_turn_marker(&dir, "k2").unwrap().attempts, 0);
        // unicode truncation (chars not bytes)
        let unicode = "é".repeat(MAX_PROMPT_CHARS + 10);
        record_turn_start(&dir, "k3", &unicode, 0);
        assert_eq!(read_turn_marker(&dir, "k3").unwrap().prompt.chars().count(), MAX_PROMPT_CHARS);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_removes_and_noop() {
        let dir = test_dir();
        record_turn_start(&dir, "a", "hi", 0);
        record_turn_start(&dir, "b", "hi2", 0);
        assert!(read_turn_marker(&dir, "a").is_some());
        clear_turn_marker(&dir, "a");
        assert!(read_turn_marker(&dir, "a").is_none());
        assert!(read_turn_marker(&dir, "b").is_some());
        // clear non-existent is no-op
        clear_turn_marker(&dir, "nonexistent");
        assert!(read_turn_marker(&dir, "b").is_some());
        // clear last entry unlinks file
        clear_turn_marker(&dir, "b");
        assert!(read_turn_marker(&dir, "b").is_none());
        let path = marker_path(&dir);
        assert!(!path.exists());
        // clear with empty key is no-op
        clear_turn_marker(&dir, "");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_validates_prompt_and_fields() {
        let dir = test_dir();
        let path = marker_path(&dir);
        fs::create_dir_all(path.parent().unwrap()).unwrap();

        // prompt missing -> None
        let mut e1 = HashMap::new();
        e1.insert("started_at".to_string(), JsonValue::Number("1000".into()));
        e1.insert("attempts".to_string(), JsonValue::Number("0".into()));
        let mut entries = HashMap::new();
        entries.insert("k".into(), e1);
        store(&path, &entries).unwrap();
        assert!(read_turn_marker(&dir, "k").is_none());

        // prompt empty after trim -> None
        let mut e2 = HashMap::new();
        e2.insert("prompt".to_string(), JsonValue::Str("   ".into()));
        e2.insert("started_at".to_string(), JsonValue::Number("1000".into()));
        e2.insert("attempts".to_string(), JsonValue::Number("0".into()));
        let mut entries2 = HashMap::new();
        entries2.insert("k".into(), e2);
        store(&path, &entries2).unwrap();
        assert!(read_turn_marker(&dir, "k").is_none());

        // invalid started_at -> None
        let mut e3 = HashMap::new();
        e3.insert("prompt".to_string(), JsonValue::Str("hi".into()));
        e3.insert("started_at".to_string(), JsonValue::Str("not-a-number".into()));
        e3.insert("attempts".to_string(), JsonValue::Number("0".into()));
        let mut entries3 = HashMap::new();
        entries3.insert("k".into(), e3);
        store(&path, &entries3).unwrap();
        assert!(read_turn_marker(&dir, "k").is_none());

        // invalid attempts -> None
        let mut e4 = HashMap::new();
        e4.insert("prompt".to_string(), JsonValue::Str("hi".into()));
        e4.insert("started_at".to_string(), JsonValue::Number("1000".into()));
        e4.insert("attempts".to_string(), JsonValue::Str("bad".into()));
        let mut entries4 = HashMap::new();
        entries4.insert("k".into(), e4);
        store(&path, &entries4).unwrap();
        assert!(read_turn_marker(&dir, "k").is_none());

        // valid with string numbers
        let mut e5 = HashMap::new();
        e5.insert("prompt".to_string(), JsonValue::Str("hello".into()));
        e5.insert("started_at".to_string(), JsonValue::Str("1234.5".into()));
        e5.insert("attempts".to_string(), JsonValue::Str("2".into()));
        let mut entries5 = HashMap::new();
        entries5.insert("k".into(), e5);
        store(&path, &entries5).unwrap();
        let m = read_turn_marker(&dir, "k").unwrap();
        assert_eq!(m.prompt, "hello");
        assert_eq!(m.started_at, 1234.5);
        assert_eq!(m.attempts, 2);

        // prompt numeric -> stringified
        let mut e6 = HashMap::new();
        e6.insert("prompt".to_string(), JsonValue::Number("123".into()));
        e6.insert("started_at".to_string(), JsonValue::Number("1000".into()));
        e6.insert("attempts".to_string(), JsonValue::Number("0".into()));
        let mut entries6 = HashMap::new();
        entries6.insert("k".into(), e6);
        store(&path, &entries6).unwrap();
        let m2 = read_turn_marker(&dir, "k").unwrap();
        assert_eq!(m2.prompt, "123");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_empty_session_key_is_none() {
        let dir = test_dir();
        record_turn_start(&dir, "k", "hi", 0);
        assert!(read_turn_marker(&dir, "").is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_prunes_old_entries() {
        let dir = test_dir();
        let now = now_secs();
        // Manually store an old entry and a fresh entry
        let path = marker_path(&dir);
        let mut old = HashMap::new();
        old.insert("prompt".to_string(), JsonValue::Str("old".into()));
        old.insert(
            "started_at".to_string(),
            JsonValue::Number((now - MAX_AGE_SECS as f64 - 100.0).to_string()),
        );
        old.insert("attempts".to_string(), JsonValue::Number("0".into()));
        let mut fresh = HashMap::new();
        fresh.insert("prompt".to_string(), JsonValue::Str("fresh".into()));
        fresh.insert("started_at".to_string(), JsonValue::Number(now.to_string()));
        fresh.insert("attempts".to_string(), JsonValue::Number("0".into()));
        let mut entries = HashMap::new();
        entries.insert("old".into(), old);
        entries.insert("fresh".into(), fresh);
        store(&path, &entries).unwrap();
        // record new turn should prune old
        record_turn_start(&dir, "new", "hi", 0);
        assert!(read_turn_marker(&dir, "old").is_none());
        assert!(read_turn_marker(&dir, "fresh").is_some());
        assert!(read_turn_marker(&dir, "new").is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn json_escape_roundtrip() {
        let dir = test_dir();
        let prompt = "hello \"world\"\n new\\line \t tab";
        record_turn_start(&dir, "k", prompt, 0);
        let m = read_turn_marker(&dir, "k").unwrap();
        assert_eq!(m.prompt, prompt);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_store_and_load_preserves_all() {
        let dir = test_dir();
        for i in 0..5 {
            record_turn_start(&dir, &format!("k{}", i), &format!("prompt {}", i), i as i64);
        }
        for i in 0..5 {
            let m = read_turn_marker(&dir, &format!("k{}", i)).unwrap();
            assert_eq!(m.prompt, format!("prompt {}", i));
            assert_eq!(m.attempts, i as i64);
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
