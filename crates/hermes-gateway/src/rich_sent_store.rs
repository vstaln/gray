//! Local index of text we've sent via `sendRichMessage` (Bot API 10.1).
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/rich_sent_store.py` (83 LOC).
//!
//! Python source docstring (preserved):
//! ```text
//! Local index of text we've sent via ``sendRichMessage`` (Bot API 10.1).
//!
//! Telegram does NOT echo a rich message's content back in ``reply_to_message``
//! when a user replies to it (verified: ``.text``/``.caption`` empty,
//! ``.api_kwargs`` None). So replies to the launchd briefings / any rich send
//! arrive with no quotable text and the agent is blind to what was referenced.
//!
//! Fix: remember ``message_id -> text`` at send time, look it up by
//! ``reply_to_id`` on inbound. This module is the single source of truth for that
//! index.
//!
//! Best-effort and dependency-free: every operation swallows errors and degrades
//! to a no-op / ``None`` so it can never break a send or an inbound message.
//! ```
//!
//! Mapping:
//! - `_MAX_ENTRIES = 1000` → [`_MAX_ENTRIES`] / [`MAX_ENTRIES`]
//! - `_MAX_TEXT_CHARS = 2000` → [`_MAX_TEXT_CHARS`] / [`MAX_TEXT_CHARS`]
//! - `def _store_path() -> str` → [`_store_path`] / [`store_path`] / [`store_path_with_home`]
//! - `def _key(chat_id, message_id) -> str` → [`_key`] / [`key_for`] / [`key_for_display`]
//! - `def record(chat_id, message_id, text)` → [`record`] / [`record_opt`] / [`record_to`] / [`record_ids`]
//! - `def lookup(chat_id, message_id) -> Optional[str]` → [`lookup`] / [`lookup_opt`] / [`lookup_from`] / [`lookup_ids`]
//! - `get_hermes_home()` → [`get_hermes_home`] (mirrors `hermes_constants.get_hermes_home` / `hermes_cli.config.get_hermes_home`)
//! - `os.path.join(str(home), "state", "rich_sent_index.json")` → [`store_path_with_home`] / [`INDEX_FILE_NAME`]
//! - `os.makedirs(..., exist_ok=True)` → [`std::fs::create_dir_all`]
//! - `json.load` / `isinstance(data, dict)` guard → [`_load_map`] / [`_load_map_from`] (non-dict → empty)
//! - `text[:_MAX_TEXT_CHARS]` → chars().take(MAX) truncation (Unicode-correct)
//! - `int(time.time())` → [`now_ts`] (`SystemTime` → `i64` seconds)
//! - trim oldest by `ts` when `len > _MAX_ENTRIES` → sorted by `ts` ascending, drain oldest `len - MAX`
//! - `tmp = f"{path}.tmp.{os.getpid()}"` → `format!("{}.tmp.{}", path.display(), pid)` (plus nanos for uniqueness)
//! - `os.replace(tmp, path)` → [`std::fs::rename`] (atomic on POSIX; tolerates concurrent writers racing)
//! - swallow `Exception` / `FileNotFoundError, ValueError, AttributeError` → `let _ =` / return `None` / no-op
//! - `entry.get("t") or None` → `entry.t.is_empty() → None` else `Some(t)` (empty string degrades to None like Python falsy)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants — mirrors Python module-level constants
// ---------------------------------------------------------------------------

/// Max number of entries to keep. Mirrors `_MAX_ENTRIES = 1000`.
pub const _MAX_ENTRIES: usize = 1000;
/// Public alias.
pub const MAX_ENTRIES: usize = _MAX_ENTRIES;

/// Max chars stored per entry. Mirrors `_MAX_TEXT_CHARS = 2000`.
pub const _MAX_TEXT_CHARS: usize = 2000;
/// Public alias.
pub const MAX_TEXT_CHARS: usize = _MAX_TEXT_CHARS;

/// File name for the index within `HERMES_HOME/state`.
pub const INDEX_FILE_NAME: &str = "rich_sent_index.json";

/// Subdirectory under `HERMES_HOME` where the index lives. Mirrors `"state"`.
pub const INDEX_DIR_NAME: &str = "state";

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

// Provide private alias mirroring Python's `get_hermes_home` import inside `_store_path`.
#[allow(dead_code)]
fn _get_hermes_home() -> PathBuf {
    get_hermes_home()
}

// ---------------------------------------------------------------------------
// Store path — mirrors Python `def _store_path() -> str:`
// ---------------------------------------------------------------------------

/// Resolve the index path via `get_hermes_home()` so the active profile override
/// is honored.
///
/// Mirrors:
/// ```python
/// def _store_path() -> str:
///     from hermes_constants import get_hermes_home
///     home = get_hermes_home()
///     return os.path.join(str(home), "state", "rich_sent_index.json")
/// ```
pub fn _store_path() -> PathBuf {
    store_path()
}

/// Public alias for `_store_path`.
pub fn store_path() -> PathBuf {
    store_path_with_home(&get_hermes_home())
}

/// Testable variant with explicit home.
pub fn store_path_with_home(home: &Path) -> PathBuf {
    home.join(INDEX_DIR_NAME).join(INDEX_FILE_NAME)
}

// ---------------------------------------------------------------------------
// Key — mirrors Python `def _key(chat_id, message_id) -> str:`
// ---------------------------------------------------------------------------

/// Build the map key `"{chat_id}:{message_id}"`.
///
/// Mirrors:
/// ```python
/// def _key(chat_id, message_id) -> str:
///     return f"{chat_id}:{message_id}"
/// ```
pub fn _key(chat_id: &str, message_id: &str) -> String {
    format!("{}:{}", chat_id, message_id)
}

/// Alias for `_key` (ergonomic name without leading underscore).
pub fn key_for(chat_id: &str, message_id: &str) -> String {
    _key(chat_id, message_id)
}

/// Generic `Display` variant — mirrors Python's f-string coercion of any int/str.
///
/// `chat_id`/`message_id` in Python are often `int` (Telegram chat id); this
/// helper stringifies them the same as `f"{chat_id}:{message_id}"`.
pub fn key_for_display(
    chat_id: impl std::fmt::Display,
    message_id: impl std::fmt::Display,
) -> String {
    format!("{}:{}", chat_id, message_id)
}

#[allow(dead_code)]
fn _key_display(
    chat_id: impl std::fmt::Display,
    message_id: impl std::fmt::Display,
) -> String {
    key_for_display(chat_id, message_id)
}

// ---------------------------------------------------------------------------
// Entry — mirrors the dict stored per key: `{"t": ..., "ts": ...}`
// ---------------------------------------------------------------------------

/// Stored entry value.
///
/// Mirrors:
/// ```python
/// data[_key(chat_id, message_id)] = {
///     "t": text[:_MAX_TEXT_CHARS],
///     "ts": int(time.time()),
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RichSentEntry {
    /// Truncated text — mirrors `"t"`.
    pub t: String,
    /// Unix seconds — mirrors `"ts": int(time.time())`.
    pub ts: i64,
}

// ---------------------------------------------------------------------------
// Helpers — mirrors Python helpers
// ---------------------------------------------------------------------------

/// Current wall-clock seconds since UNIX epoch as `i64`, mirrors `int(time.time())`.
fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[allow(dead_code)]
fn _now_ts() -> i64 {
    now_ts()
}

/// Truncate to `_MAX_TEXT_CHARS` chars (Unicode-correct).
///
/// Python `text[:_MAX_TEXT_CHARS]` slices by Unicode code points; Rust `chars().take(...)` mirrors that.
fn truncate_text(text: &str) -> String {
    if text.chars().count() <= _MAX_TEXT_CHARS {
        text.to_string()
    } else {
        text.chars().take(_MAX_TEXT_CHARS).collect()
    }
}

/// Load the on-disk map best-effort. Non-existent / corrupt / non-dict → empty.
///
/// Mirrors:
/// ```python
/// try:
///     with open(path, "r", encoding="utf-8") as fh:
///         data = json.load(fh)
///     if not isinstance(data, dict):
///         data = {}
/// except (FileNotFoundError, ValueError):
///     data = {}
/// ```
fn _load_map(path: &Path) -> HashMap<String, RichSentEntry> {
    _load_map_from(path)
}

/// Testable variant with explicit path.
pub fn _load_map_from(path: &Path) -> HashMap<String, RichSentEntry> {
    if !path.exists() {
        return HashMap::new();
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return HashMap::new(),
    };
    // First check it parses at all — ValueError → empty.
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return HashMap::new(),
    };
    // `isinstance(data, dict)` guard — non-object → empty.
    let obj = match value.as_object() {
        Some(o) => o,
        None => return HashMap::new(),
    };
    // Try typed parse; fall back to lenient per-entry extraction so partially
    // corrupt entries are skipped rather than discarding the whole file.
    let mut map: HashMap<String, RichSentEntry> = HashMap::new();
    for (k, v) in obj {
        if let Ok(entry) = serde_json::from_value::<RichSentEntry>(v.clone()) {
            map.insert(k.clone(), entry);
        } else if let Some(o) = v.as_object() {
            // Lenient fallback — mirrors `data` dict with `t`/`ts` keys.
            // Only keep entries where `t` is a string; `ts` defaults to 0 like `kv[1].get("ts", 0)`.
            if let Some(t) = o.get("t").and_then(|x| x.as_str()) {
                let ts = o
                    .get("ts")
                    .and_then(|x| x.as_i64())
                    .or_else(|| o.get("ts").and_then(|x| x.as_u64()).map(|u| u as i64))
                    .or_else(|| o.get("ts").and_then(|x| x.as_f64()).map(|f| f as i64))
                    .unwrap_or(0);
                map.insert(
                    k.clone(),
                    RichSentEntry {
                        t: t.to_string(),
                        ts,
                    },
                );
            }
        }
    }
    map
}

/// Persist `data` atomically via tmp + rename. Mirrors the atomic write block.
///
/// ```python
/// tmp = f"{path}.tmp.{os.getpid()}"
/// with open(tmp, "w", encoding="utf-8") as fh:
///     json.dump(data, fh, ensure_ascii=False)
/// os.replace(tmp, path)
/// ```
fn _save_map_atomic(path: &Path, data: &HashMap<String, RichSentEntry>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Use pid + nanos for uniqueness — mirrors `os.getpid()` but avoids collisions
    // in tight loops (same pid); still in same dir for atomic rename.
    let tmp = {
        let pid = std::process::id();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        PathBuf::from(format!("{}.tmp.{}-{}", path.display(), pid, nanos))
    };
    let result: std::io::Result<()> = (|| {
        let data_str = serde_json::to_string(data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        // Mirrors `json.dump(..., ensure_ascii=False)` — serde_json already preserves unicode.
        let mut file = std::fs::File::create(&tmp)?;
        use std::io::Write as _;
        file.write_all(data_str.as_bytes())?;
        file.flush()?;
        // `os.fsync` equivalent — best-effort, explicit sync before rename for durability.
        let _ = file.sync_all();
        drop(file);
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

// ---------------------------------------------------------------------------
// record — mirrors Python `def record(chat_id, message_id, text) -> None:`
// ---------------------------------------------------------------------------

/// Persist `text` for `(chat_id, message_id)`. No-op on any failure.
///
/// Best-effort and swallows all errors so it can never break a send.
///
/// Mirrors:
/// ```python
/// def record(chat_id, message_id, text: Optional[str]) -> None:
///     if not text or message_id is None or chat_id is None:
///         return
///     path = _store_path()
///     try:
///         os.makedirs(os.path.dirname(path), exist_ok=True)
///         try:
///             with open(path, "r", encoding="utf-8") as fh:
///                 data = json.load(fh)
///             if not isinstance(data, dict):
///                 data = {}
///         except (FileNotFoundError, ValueError):
///             data = {}
///         data[_key(chat_id, message_id)] = {
///             "t": text[:_MAX_TEXT_CHARS],
///             "ts": int(time.time()),
///         }
///         if len(data) > _MAX_ENTRIES:
///             for k, _ in sorted(
///                 data.items(), key=lambda kv: kv[1].get("ts", 0)
///             )[: len(data) - _MAX_ENTRIES]:
///                 data.pop(k, None)
///         tmp = f"{path}.tmp.{os.getpid()}"
///         with open(tmp, "w", encoding="utf-8") as fh:
///             json.dump(data, fh, ensure_ascii=False)
///         os.replace(tmp, path)
///     except Exception:
///         return
/// ```
pub fn record(chat_id: &str, message_id: &str, text: &str) {
    // `if not text` — empty text is no-op; `&str` cannot be None so only empty check.
    if text.is_empty() {
        return;
    }
    record_to(&store_path(), chat_id, message_id, text);
}

/// `Option` variant that mirrors the `is None` guards exactly.
///
/// `None` for any of `chat_id`/`message_id`/`text` → no-op. Empty `text` → no-op
/// (mirrors `if not text`). Empty string ids are treated as `Some("")` — Python would
/// consider `chat_id=""` not None and would still create a key; we preserve that but
/// callers should avoid empty ids.
pub fn record_opt(
    chat_id: Option<&str>,
    message_id: Option<&str>,
    text: Option<&str>,
) {
    let Some(text) = text else { return };
    if text.is_empty() {
        return;
    }
    let Some(chat_id) = chat_id else { return };
    let Some(message_id) = message_id else { return };
    record(chat_id, message_id, text);
}

/// Numeric-id convenience — mirrors `f"{chat_id}:{message_id}"` with ints.
///
/// Telegram `chat_id`/`message_id` are often `i64`; this stringifies them like
/// Python's f-string coercion.
pub fn record_ids(chat_id: impl std::fmt::Display, message_id: impl std::fmt::Display, text: &str) {
    if text.is_empty() {
        return;
    }
    let key_chat = format!("{}", chat_id);
    let key_msg = format!("{}", message_id);
    record_to(&store_path(), &key_chat, &key_msg, text);
}

/// Testable variant with explicit path.
///
/// Same logic as [`record`] but writes to `path` directly — mirrors the inner
/// body of `record` for hermetic tests without touching `HERMES_HOME`.
pub fn record_to(path: &Path, chat_id: &str, message_id: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    // Entire body is best-effort — swallow any error and degrade to no-op.
    let _ = (|| -> std::io::Result<()> {
        let mut data = _load_map_from(path);
        let key = _key(chat_id, message_id);
        data.insert(
            key,
            RichSentEntry {
                t: truncate_text(text),
                ts: now_ts(),
            },
        );
        // Trim oldest by timestamp when over cap.
        if data.len() > _MAX_ENTRIES {
            let to_remove = data.len() - _MAX_ENTRIES;
            let mut sorted: Vec<(String, i64)> = data
                .iter()
                .map(|(k, v)| (k.clone(), v.ts))
                .collect();
            sorted.sort_by_key(|(_, ts)| *ts);
            for (k, _) in sorted.into_iter().take(to_remove) {
                data.remove(&k);
            }
        }
        _save_map_atomic(path, &data)?;
        Ok(())
    })();
}

/// Testable variant with explicit `HERMES_HOME` dir (resolves `state/rich_sent_index.json` under it).
pub fn record_with_home(home: &Path, chat_id: &str, message_id: &str, text: &str) {
    record_to(&store_path_with_home(home), chat_id, message_id, text);
}

// ---------------------------------------------------------------------------
// lookup — mirrors Python `def lookup(chat_id, message_id) -> Optional[str]:`
// ---------------------------------------------------------------------------

/// Return stored text for `(chat_id, message_id)` or `None`.
///
/// Best-effort; any I/O or parse error degrades to `None`.
///
/// Mirrors:
/// ```python
/// def lookup(chat_id, message_id) -> Optional[str]:
///     if message_id is None or chat_id is None:
///         return None
///     try:
///         with open(_store_path(), "r", encoding="utf-8") as fh:
///             data = json.load(fh)
///         entry = data.get(_key(chat_id, message_id))
///         if isinstance(entry, dict):
///             return entry.get("t") or None
///     except (FileNotFoundError, ValueError, AttributeError):
///         return None
///     return None
/// ```
pub fn lookup(chat_id: &str, message_id: &str) -> Option<String> {
    lookup_from(&store_path(), chat_id, message_id)
}

/// `Option` variant that mirrors the `is None` guards.
///
/// `None` for `chat_id` or `message_id` → `None` (mirrors Python).
pub fn lookup_opt(chat_id: Option<&str>, message_id: Option<&str>) -> Option<String> {
    let chat_id = chat_id?;
    let message_id = message_id?;
    lookup(chat_id, message_id)
}

/// Numeric-id convenience — mirrors `f"{chat_id}:{message_id}"` with ints.
pub fn lookup_ids(
    chat_id: impl std::fmt::Display,
    message_id: impl std::fmt::Display,
) -> Option<String> {
    let key_chat = format!("{}", chat_id);
    let key_msg = format!("{}", message_id);
    lookup(&key_chat, &key_msg)
}

/// Testable variant with explicit path.
///
/// Mirrors the inner body of `lookup` but reads from `path` directly.
pub fn lookup_from(path: &Path, chat_id: &str, message_id: &str) -> Option<String> {
    // Best-effort; swallow FileNotFoundError / ValueError / AttributeError → None.
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return None,
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return None,
    };
    let obj = match value.as_object() {
        Some(o) => o,
        None => return None,
    };
    // `data.get(_key(...))` + `isinstance(entry, dict)` guard.
    let key = _key(chat_id, message_id);
    let entry = obj.get(&key)?;
    let entry_obj = entry.as_object()?;
    // `entry.get("t") or None` — None or empty/falsy string → None.
    let t = entry_obj.get("t")?.as_str()?;
    if t.is_empty() {
        return None;
    }
    Some(t.to_string())
}

/// Typed variant returning the full entry (including `ts`) for diagnostics.
///
/// Not in Python but useful for callers that need the timestamp; `None` when
/// missing or malformed — same best-effort contract as [`lookup`].
pub fn lookup_entry_from(path: &Path, chat_id: &str, message_id: &str) -> Option<RichSentEntry> {
    let map = _load_map_from(path);
    map.get(&_key(chat_id, message_id)).cloned()
}

/// Testable variant with explicit `HERMES_HOME` dir.
pub fn lookup_with_home(home: &Path, chat_id: &str, message_id: &str) -> Option<String> {
    lookup_from(&store_path_with_home(home), chat_id, message_id)
}
