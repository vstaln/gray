//! Persistent registry of delivery targets that are confirmed unreachable.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/dead_targets.py` (143 LOC).
//!
//! When a messaging platform reports that a target chat is permanently gone — a
//! deleted group (`Forbidden: the group chat was deleted`), a bot kicked/blocked,
//! or a deactivated user — re-sending to it on every cron tick or every fan-out
//! delivery wastes a send attempt against the platform's flood-control envelope and
//! spams the logs. This registry lets the delivery layer short-circuit a target it
//! has already proven dead, while staying self-healing: any successful send to that
//! target clears the flag, so a user who re-adds the bot (or restores the chat)
//! recovers automatically with no manual cleanup.
//!
//! Scope is deliberately narrow. Only *whole-chat* deaths are recorded — the
//! `forbidden` and chat-level `not_found` (`chat not found`) error kinds.
//! Thread/topic-level `not_found` is NOT recorded here: the adapters already
//! self-heal that by retrying without `reply_to` (see the Telegram adapter's
//! reply-target-deleted path), and a deleted topic does not mean the parent chat is
//! dead.
//!
//! The store is a small JSON file under the active profile's HERMES_HOME so each
//! profile keeps its own dead set. Reads/writes are best-effort: a corrupt or
//! unwritable file degrades to an in-memory-only registry rather than raising on
//! the delivery path.
//!
//! Python source docstring (preserved):
//! ```text
//! Persistent registry of delivery targets that are confirmed unreachable.
//!
//! When a messaging platform reports that a target chat is permanently gone — a
//! deleted group (``Forbidden: the group chat was deleted``), a bot kicked/blocked,
//! or a deactivated user — re-sending to it on every cron tick or every fan-out
//! delivery wastes a send attempt against the platform's flood-control envelope and
//! spams the logs.  This registry lets the delivery layer short-circuit a target it
//! has already proven dead, while staying self-healing: any successful send to that
//! target clears the flag, so a user who re-adds the bot (or restores the chat)
//! recovers automatically with no manual cleanup.
//!
//! Scope is deliberately narrow.  Only *whole-chat* deaths are recorded — the
//! ``forbidden`` and chat-level ``not_found`` (``chat not found``) error kinds.
//! Thread/topic-level ``not_found`` is NOT recorded here: the adapters already
//! self-heal that by retrying without ``reply_to`` (see the Telegram adapter's
//! reply-target-deleted path), and a deleted topic does not mean the parent chat is
//! dead.
//!
//! The store is a small JSON file under the active profile's HERMES_HOME so each
//! profile keeps its own dead set.  Reads/writes are best-effort: a corrupt or
//! unwritable file degrades to an in-memory-only registry rather than raising on
//! the delivery path.
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants — mirrors Python module-level constants
// ---------------------------------------------------------------------------

/// Error kinds (from gateway.platforms.base.classify_send_error) that mean the
/// *whole chat* is unreachable, not a transient or thread-level problem.
/// Mirrors `_DEAD_ERROR_KINDS = frozenset({"forbidden", "not_found"})`.
pub const _DEAD_ERROR_KINDS: &[&str] = &["forbidden", "not_found"];

/// Alias for readability.
pub const DEAD_ERROR_KINDS: &[&str] = _DEAD_ERROR_KINDS;

// ---------------------------------------------------------------------------
// Helpers — mirrors Python underscore-prefixed helpers
// ---------------------------------------------------------------------------

/// Resolve `HERMES_HOME`: `$HERMES_HOME` if set and non-empty, else `~/.hermes`.
/// Mirrors `hermes_cli.config.get_hermes_home`.
fn get_hermes_home() -> PathBuf {
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

/// Canonical key for a (platform, chat_id) pair.
/// Mirrors `_normalize(platform: str, chat_id: str) -> str`.
pub fn _normalize(platform: &str, chat_id: &str) -> String {
    format!("{}:{}", platform.trim().to_lowercase(), chat_id.trim())
}

/// Current wall-clock seconds since UNIX epoch, mirrors `time.time()`.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

// ---------------------------------------------------------------------------
// DeadEntry — mirrors the `Dict[str, object]` value stored per key
// ---------------------------------------------------------------------------

/// Value stored per dead key. Mirrors the dict written in `mark_dead`:
/// `{"platform": ..., "chat_id": ..., "reason": ..., "marked_at": time.time()}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeadEntry {
    pub platform: String,
    pub chat_id: String,
    pub reason: String,
    pub marked_at: f64,
}

// ---------------------------------------------------------------------------
// DeadTargetRegistry — mirrors `class DeadTargetRegistry`
// ---------------------------------------------------------------------------

/// Thread-safe, persistent set of confirmed-dead delivery targets.
///
/// Keyed on `platform:chat_id`. Stores the reason and a timestamp for
/// observability. Self-healing: `clear` (called on a successful send)
/// removes the flag.
///
/// Mirrors `gateway/dead_targets.py::DeadTargetRegistry`.
pub struct DeadTargetRegistry {
    _path: PathBuf,
    _dead: Mutex<HashMap<String, DeadEntry>>,
}

impl std::fmt::Debug for DeadTargetRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let dead = self._dead.lock().unwrap_or_else(|e| e.into_inner());
        f.debug_struct("DeadTargetRegistry")
            .field("_path", &self._path)
            .field("_dead", &*dead)
            .finish()
    }
}

impl DeadTargetRegistry {
    /// Create a registry at `path` or the default `HERMES_HOME/gateway/dead_targets.json`.
    /// Mirrors `DeadTargetRegistry.__init__(self, path: Optional[Path] = None)`.
    pub fn new(path: Option<PathBuf>) -> Self {
        let resolved = path.unwrap_or_else(|| get_hermes_home().join("gateway").join("dead_targets.json"));
        let reg = Self {
            _path: resolved,
            _dead: Mutex::new(HashMap::new()),
        };
        reg._load();
        reg
    }

    /// Convenience constructor with an explicit path.
    /// Mirrors `DeadTargetRegistry(path=Path(...))`.
    pub fn with_path(path: PathBuf) -> Self {
        Self::new(Some(path))
    }

    /// Return the backing file path.
    pub fn path(&self) -> &Path {
        &self._path
    }

    // -- persistence -------------------------------------------------------

    /// Load persisted state, best-effort.
    /// Mirrors `DeadTargetRegistry._load`.
    fn _load(&self) {
        let path = &self._path;
        if !path.exists() {
            return;
        }
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                log::debug!("dead_targets: could not load {} ({}) — starting empty", path.display(), e);
                return;
            }
        };
        let raw: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                log::debug!("dead_targets: could not load {} ({}) — starting empty", path.display(), e);
                return;
            }
        };
        let Some(obj) = raw.as_object() else {
            return;
        };
        let mut dead = self._dead.lock().unwrap_or_else(|e| e.into_inner());
        dead.clear();
        for (k, v) in obj {
            if !v.is_object() {
                continue;
            }
            // Only keep well-shaped entries (mirrors `if isinstance(v, dict)`).
            // Try typed deserialization first; fall back to lenient field extraction so any
            // dict is kept like Python does.
            if let Ok(entry) = serde_json::from_value::<DeadEntry>(v.clone()) {
                dead.insert(k.clone(), entry);
            } else if let Some(o) = v.as_object() {
                let platform = o.get("platform").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let chat_id = o.get("chat_id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let reason = o.get("reason").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let marked_at = o.get("marked_at").and_then(|x| x.as_f64()).unwrap_or(0.0);
                dead.insert(
                    k.clone(),
                    DeadEntry {
                        platform,
                        chat_id,
                        reason,
                        marked_at,
                    },
                );
            }
        }
    }

    /// Flush in-memory state to disk atomically, best-effort.
    /// Mirrors `DeadTargetRegistry._flush_locked` (called with the lock held).
    fn _flush_locked(&self, dead: &HashMap<String, DeadEntry>) {
        let path = &self._path;
        let res: std::io::Result<()> = (|| {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // Mirrors `path.with_suffix(path.suffix + ".tmp")` — i.e. original + ".tmp".
            // Using string concat avoids `with_extension` edge cases for multi-dot names.
            let tmp = PathBuf::from(format!("{}.tmp", path.display()));
            let data = serde_json::to_string_pretty(dead)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
            std::fs::write(&tmp, data)?;
            std::fs::rename(&tmp, path)?;
            Ok(())
        })();
        if let Err(e) = res {
            log::debug!("dead_targets: could not persist {} ({})", path.display(), e);
        }
    }

    // -- public API --------------------------------------------------------

    /// Return True when `error_kind` denotes a permanent whole-chat death.
    /// Mirrors `DeadTargetRegistry.is_dead_error_kind` (staticmethod).
    pub fn is_dead_error_kind(error_kind: Option<&str>) -> bool {
        is_dead_error_kind(error_kind)
    }

    /// Return true if `(platform, chat_id)` is currently marked dead.
    /// Mirrors `DeadTargetRegistry.is_dead`.
    pub fn is_dead(&self, platform: &str, chat_id: Option<&str>) -> bool {
        let chat_id = match chat_id {
            Some(s) if !s.is_empty() => s,
            _ => return false,
        };
        let key = _normalize(platform, chat_id);
        let dead = self._dead.lock().unwrap_or_else(|e| e.into_inner());
        dead.contains_key(&key)
    }

    /// Record a target as confirmed-dead. Returns True if newly added.
    /// Mirrors `DeadTargetRegistry.mark_dead`.
    pub fn mark_dead(&self, platform: &str, chat_id: Option<&str>, reason: &str) -> bool {
        let chat_id = match chat_id {
            Some(s) if !s.is_empty() => s,
            _ => return false,
        };
        let key = _normalize(platform, chat_id);
        let existed;
        {
            let mut dead = self._dead.lock().unwrap_or_else(|e| e.into_inner());
            existed = dead.contains_key(&key);
            dead.insert(
                key.clone(),
                DeadEntry {
                    platform: platform.trim().to_lowercase(),
                    chat_id: chat_id.to_string(),
                    reason: reason.chars().take(200).collect(),
                    marked_at: now_secs(),
                },
            );
            self._flush_locked(&dead);
        }
        if !existed {
            log::info!(
                "dead_targets: marked {} as unreachable ({}) — future deliveries to this target will be skipped until a send succeeds",
                key,
                if reason.is_empty() { "no reason given" } else { reason }
            );
        }
        !existed
    }

    /// Remove a target's dead flag (self-healing). Returns True if it was set.
    /// Mirrors `DeadTargetRegistry.clear`.
    pub fn clear(&self, platform: &str, chat_id: Option<&str>) -> bool {
        let chat_id = match chat_id {
            Some(s) if !s.is_empty() => s,
            _ => return false,
        };
        let key = _normalize(platform, chat_id);
        let removed;
        {
            let mut dead = self._dead.lock().unwrap_or_else(|e| e.into_inner());
            removed = dead.remove(&key).is_some();
            if removed {
                self._flush_locked(&dead);
            }
        }
        if removed {
            log::info!("dead_targets: cleared {} (delivery succeeded again)", key);
            return true;
        }
        false
    }

    /// Snapshot of the current dead set (for diagnostics / `hermes` CLI).
    /// Mirrors `DeadTargetRegistry.all_dead`.
    pub fn all_dead(&self) -> HashMap<String, DeadEntry> {
        let dead = self._dead.lock().unwrap_or_else(|e| e.into_inner());
        dead.clone()
    }

    /// `serde_json::Value` snapshot variant (mirrors Python's `Dict[str, Dict[str, object]]`).
    pub fn all_dead_value(&self) -> HashMap<String, serde_json::Value> {
        let dead = self._dead.lock().unwrap_or_else(|e| e.into_inner());
        dead.iter()
            .map(|(k, v)| (k.clone(), serde_json::to_value(v).unwrap_or(serde_json::Value::Null)))
            .collect()
    }
}

impl Default for DeadTargetRegistry {
    fn default() -> Self {
        Self::new(None)
    }
}

// ---------------------------------------------------------------------------
// Free functions — mirrors Python module-level helpers
// ---------------------------------------------------------------------------

/// Return True when `error_kind` denotes a permanent whole-chat death.
/// Mirrors `DeadTargetRegistry.is_dead_error_kind` as a free function and
/// the underlying `_DEAD_ERROR_KINDS` check.
pub fn is_dead_error_kind(error_kind: Option<&str>) -> bool {
    match error_kind {
        Some(k) if !k.is_empty() => _DEAD_ERROR_KINDS.contains(&k),
        _ => false,
    }
}

/// Convenience `&str` variant — treats empty string as None (mirrors `bool(error_kind)` guard).
pub fn is_dead_error_kind_str(error_kind: &str) -> bool {
    if error_kind.is_empty() {
        return false;
    }
    _DEAD_ERROR_KINDS.contains(&error_kind)
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
