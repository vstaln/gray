//! Sticker description cache for Telegram.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/sticker_cache.py` (124 LOC).
//!
//! Python source docstring (preserved):
//! ```text
//! Sticker description cache for Telegram.
//!
//! When users send stickers, we describe them via the vision tool and cache
//! the descriptions keyed by file_unique_id so we don't re-analyze the same
//! sticker image on every send. Descriptions are concise (1-2 sentences).
//!
//! Cache location: ~/.hermes/sticker_cache.json
//! ```
//!
//! Mapping:
//! - `get_hermes_home()` → [`get_hermes_home`] (mirrors `hermes_cli.config.get_hermes_home`)
//! - `CACHE_PATH = get_hermes_home() / "sticker_cache.json"` → [`cache_path`] / [`CACHE_FILE_NAME`]
//! - `STICKER_VISION_PROMPT` → [`STICKER_VISION_PROMPT`]
//! - `_load_cache()` → [`_load_cache`] / [`_load_cache_from`]
//! - `_save_cache(cache)` → [`_save_cache`] / [`_save_cache_to`] (atomic via tmp + fsync + rename, mirrors `tempfile.mkstemp` + `os.fsync` + `os.replace`)
//! - `get_cached_description(file_unique_id)` → [`get_cached_description`] / [`get_cached_description_from`]
//! - `cache_sticker_description(file_unique_id, description, emoji, set_name)` → [`cache_sticker_description`]
//! - `build_sticker_injection(description, emoji, set_name)` → [`build_sticker_injection`]
//! - `build_animated_sticker_injection(emoji)` → [`build_animated_sticker_injection`]

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants — mirrors Python module-level constants
// ---------------------------------------------------------------------------

/// File name for the cache within `HERMES_HOME`.
pub const CACHE_FILE_NAME: &str = "sticker_cache.json";

/// Vision prompt for describing stickers -- kept concise to save tokens.
/// Mirrors `STICKER_VISION_PROMPT`.
pub const STICKER_VISION_PROMPT: &str =
    "Describe this sticker in 1-2 sentences. Focus on what it depicts -- character, action, emotion. Be concise and objective.";

// ---------------------------------------------------------------------------
// HERMES_HOME — mirrors `hermes_cli.config.get_hermes_home()`
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

/// Path to `sticker_cache.json` under the active `HERMES_HOME`.
/// Mirrors `CACHE_PATH = get_hermes_home() / "sticker_cache.json"`.
pub fn cache_path() -> PathBuf {
    get_hermes_home().join(CACHE_FILE_NAME)
}

/// Testable variant with explicit home.
pub fn cache_path_with_home(home: &Path) -> PathBuf {
    home.join(CACHE_FILE_NAME)
}

// Provide private alias mirroring Python's `CACHE_PATH` for grep-ability.
#[allow(dead_code)]
fn _cache_path() -> PathBuf {
    cache_path()
}

// ---------------------------------------------------------------------------
// Cache entry — mirrors the dict stored per file_unique_id
// ---------------------------------------------------------------------------

/// Cached sticker description entry.
///
/// Mirrors the dict written in `cache_sticker_description`:
/// `{"description": ..., "emoji": ..., "set_name": ..., "cached_at": time.time()}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StickerCacheEntry {
    pub description: String,
    #[serde(default)]
    pub emoji: String,
    #[serde(default)]
    pub set_name: String,
    pub cached_at: f64,
}

// ---------------------------------------------------------------------------
// Helpers — mirrors Python underscore-prefixed helpers
// ---------------------------------------------------------------------------

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

// ---------------------------------------------------------------------------
// _load_cache — mirrors Python `def _load_cache() -> dict:`
// ---------------------------------------------------------------------------

/// Load the sticker cache from disk.
///
/// Mirrors:
/// ```python
/// def _load_cache() -> dict:
///     if CACHE_PATH.exists():
///         try:
///             return json.loads(CACHE_PATH.read_text(encoding="utf-8"))
///         except (json.JSONDecodeError, OSError):
///             return {}
///     return {}
/// ```
pub fn _load_cache() -> HashMap<String, StickerCacheEntry> {
    _load_cache_from(&cache_path())
}

/// Testable variant with explicit path.
pub fn _load_cache_from(path: &Path) -> HashMap<String, StickerCacheEntry> {
    if !path.exists() {
        return HashMap::new();
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return HashMap::new(),
    };
    match serde_json::from_str::<HashMap<String, StickerCacheEntry>>(&text) {
        Ok(map) => map,
        Err(_) => HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// _save_cache — mirrors Python `def _save_cache(cache: dict) -> None:`
// ---------------------------------------------------------------------------

/// Save the sticker cache to disk atomically.
///
/// Mirrors:
/// ```python
/// def _save_cache(cache: dict) -> None:
///     CACHE_PATH.parent.mkdir(parents=True, exist_ok=True)
///     fd, tmp_path = tempfile.mkstemp(dir=str(CACHE_PATH.parent), suffix=".tmp")
///     try:
///         with os.fdopen(fd, "w", encoding="utf-8") as f:
///             json.dump(cache, f, indent=2, ensure_ascii=False)
///             f.flush()
///             os.fsync(f.fileno())
///         os.replace(tmp_path, str(CACHE_PATH))
///     except BaseException:
///         try:
///             os.unlink(tmp_path)
///         except OSError:
///             pass
///         raise
/// ```
pub fn _save_cache(cache: &HashMap<String, StickerCacheEntry>) -> std::io::Result<()> {
    _save_cache_to(cache, &cache_path())
}

/// Testable variant with explicit path.
pub fn _save_cache_to(cache: &HashMap<String, StickerCacheEntry>, path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Mirrors `tempfile.mkstemp(dir=str(CACHE_PATH.parent), suffix=".tmp")`
    // but without the `tempfile` crate: use pid + nanos for uniqueness, same dir.
    let tmp_path = {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        PathBuf::from(format!("{}.tmp.{}-{}", path.display(), pid, nanos))
    };
    let result: std::io::Result<()> = (|| {
        let data = serde_json::to_string_pretty(cache)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let mut file = std::fs::File::create(&tmp_path)?;
        file.write_all(data.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp_path, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result
}

// ---------------------------------------------------------------------------
// get_cached_description — mirrors Python
// ---------------------------------------------------------------------------

/// Look up a cached sticker description.
///
/// Returns `Some(entry)` with keys `{description, emoji, set_name, cached_at}` or `None`.
///
/// Mirrors:
/// ```python
/// def get_cached_description(file_unique_id: str) -> Optional[dict]:
///     cache = _load_cache()
///     return cache.get(file_unique_id)
/// ```
pub fn get_cached_description(file_unique_id: &str) -> Option<StickerCacheEntry> {
    get_cached_description_from(file_unique_id, &cache_path())
}

/// Testable variant with explicit path.
pub fn get_cached_description_from(file_unique_id: &str, path: &Path) -> Option<StickerCacheEntry> {
    let cache = _load_cache_from(path);
    cache.get(file_unique_id).cloned()
}

// ---------------------------------------------------------------------------
// cache_sticker_description — mirrors Python
// ---------------------------------------------------------------------------

/// Store a sticker description in the cache.
///
/// Mirrors:
/// ```python
/// def cache_sticker_description(
///     file_unique_id: str,
///     description: str,
///     emoji: str = "",
///     set_name: str = "",
/// ) -> None:
///     cache = _load_cache()
///     cache[file_unique_id] = {
///         "description": description,
///         "emoji": emoji,
///         "set_name": set_name,
///         "cached_at": time.time(),
///     }
///     _save_cache(cache)
/// ```
pub fn cache_sticker_description(
    file_unique_id: &str,
    description: &str,
    emoji: &str,
    set_name: &str,
) -> std::io::Result<()> {
    cache_sticker_description_to(file_unique_id, description, emoji, set_name, &cache_path())
}

/// Testable variant with explicit path.
pub fn cache_sticker_description_to(
    file_unique_id: &str,
    description: &str,
    emoji: &str,
    set_name: &str,
    path: &Path,
) -> std::io::Result<()> {
    let mut cache = _load_cache_from(path);
    cache.insert(
        file_unique_id.to_string(),
        StickerCacheEntry {
            description: description.to_string(),
            emoji: emoji.to_string(),
            set_name: set_name.to_string(),
            cached_at: now_secs(),
        },
    );
    _save_cache_to(&cache, path)
}

// ---------------------------------------------------------------------------
// build_sticker_injection — mirrors Python
// ---------------------------------------------------------------------------

/// Build the warm-style injection text for a sticker description.
///
/// Returns a string like:
/// `[The user sent a sticker 😀 from "MyPack"~ It shows: "A cat waving" (=^.w.^=)]`
///
/// Mirrors:
/// ```python
/// def build_sticker_injection(description: str, emoji: str = "", set_name: str = "") -> str:
///     context = ""
///     if set_name and emoji:
///         context = f" {emoji} from \"{set_name}\""
///     elif emoji:
///         context = f" {emoji}"
///     return f"[The user sent a sticker{context}~ It shows: \"{description}\" (=^.w.^=)]"
/// ```
pub fn build_sticker_injection(description: &str, emoji: &str, set_name: &str) -> String {
    let context = if !set_name.is_empty() && !emoji.is_empty() {
        format!(" {} from \"{}\"", emoji, set_name)
    } else if !emoji.is_empty() {
        format!(" {}", emoji)
    } else {
        String::new()
    };
    format!(
        "[The user sent a sticker{}~ It shows: \"{}\" (=^.w.^=)]",
        context, description
    )
}

// ---------------------------------------------------------------------------
// build_animated_sticker_injection — mirrors Python
// ---------------------------------------------------------------------------

/// Build injection text for animated/video stickers we can't analyze.
///
/// Mirrors:
/// ```python
/// def build_animated_sticker_injection(emoji: str = "") -> str:
///     if emoji:
///         return f"[The user sent an animated sticker {emoji}~ I can't see animated ones yet, but the emoji suggests: {emoji}]"
///     return "[The user sent an animated sticker~ I can't see animated ones yet]"
/// ```
pub fn build_animated_sticker_injection(emoji: &str) -> String {
    if !emoji.is_empty() {
        format!(
            "[The user sent an animated sticker {}~ I can't see animated ones yet, but the emoji suggests: {}]",
            emoji, emoji
        )
    } else {
        "[The user sent an animated sticker~ I can't see animated ones yet]".to_string()
    }
}

// Provide private aliases mirroring Python's underscore-prefixed helpers for traceability
#[allow(dead_code)]
fn _get_hermes_home() -> PathBuf {
    get_hermes_home()
}

#[allow(dead_code)]
fn _now_secs() -> f64 {
    now_secs()
}
