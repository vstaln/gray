//! Suggested cron jobs — proposed automations the user accepts with one tap.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/cron/suggestions.py` (269 lines).
//!
//! A *suggestion* is a ready-to-run cron job spec that Hermes surfaces to the
//! user, who accepts it (creates the real cron job) or dismisses it (latched so
//! it is never re-offered). Every automation proposal flows through here:
//!
//!   * `catalog`     — curated starter automation
//!   * `blueprint`   — skill carries a `blueprint:` block
//!   * `usage`       — recurring ask noticed by self-improvement review
//!   * `integration` — user connected an account and the obvious automations are offered
//!
//! Accepting a suggestion just calls the existing `cron.jobs.create_job` with
//! the stored `job_spec` — there is NO second job engine. Suggestions never
//! auto-create jobs; acceptance is always explicit (consent-first). Dismissed
//! suggestions latch by a stable `dedup_key` so the same proposal is not
//! re-offered after the user says no.
//!
//! Storage mirrors `cron/jobs.py`: `~/.hermes/cron/suggestions.json`, atomic
//! writes, an in-process lock, and 0600 perms.
//!
//! Python source docstring (preserved):
//! ```text
//! Suggested cron jobs — proposed automations the user accepts with one tap.
//! ...
//! Storage mirrors cron/jobs.py: ~/.hermes/cron/suggestions.json, atomic
//! writes, an in-process lock, and 0600 perms.
//! ```

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Constants — mirrors Python module globals
// ---------------------------------------------------------------------------

/// Cap pending suggestions so the list never becomes a nag wall.
/// Mirrors `MAX_PENDING = 5`.
pub const MAX_PENDING: usize = 5;

/// Valid suggestion sources. Mirrors `VALID_SOURCES = frozenset({...})`.
pub const VALID_SOURCES: &[&str] = &["catalog", "blueprint", "usage", "integration"];

const STATUS_PENDING: &str = "pending";
const STATUS_ACCEPTED: &str = "accepted";
const STATUS_DISMISSED: &str = "dismissed";

// ---------------------------------------------------------------------------
// Global lock — mirrors `_suggestions_lock = threading.Lock()`
// ---------------------------------------------------------------------------

static SUGGESTIONS_LOCK: Mutex<()> = Mutex::new(());

/// Optional test override for the suggestions file path.
/// Mirrors monkeypatched `SUGGESTIONS_FILE` in Python tests.
static SUGGESTIONS_FILE_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum SuggestionError {
    #[error("unknown suggestion source: {0:?}")]
    UnknownSource(String),
    #[error("title and dedup_key are required")]
    MissingField,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("scheduler registration failed: {0}")]
    Scheduler(String),
}

pub type Result<T> = std::result::Result<T, SuggestionError>;

// ---------------------------------------------------------------------------
// Data model — mirrors Python `Dict[str, Any]` suggestion record
// ---------------------------------------------------------------------------

/// One suggestion record. Mirrors Python's `Dict[str, Any]` with keys
/// `id`, `title`, `description`, `source`, `job_spec`, `dedup_key`,
/// `status`, `created_at`, `resolved_at` (optional).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    pub id: String,
    pub title: String,
    pub description: String,
    pub source: String,
    pub job_spec: Value,
    #[serde(default)]
    pub dedup_key: String,
    pub status: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<String>,
}

// Internal file shape: {"suggestions": [...], "updated_at": "..."}
#[derive(Debug, Serialize, Deserialize)]
struct SuggestionsFile {
    suggestions: Vec<Suggestion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at: Option<String>,
}

// ---------------------------------------------------------------------------
// Home / path helpers — mirrors `hermes_constants.get_hermes_home()`
// ---------------------------------------------------------------------------

/// Resolve the Hermes home directory.
/// Mirrors `hermes_constants.get_hermes_home()`:
/// `HERMES_HOME` env → `~/.hermes` (POSIX) / `%LOCALAPPDATA%/hermes` (Windows).
pub fn get_hermes_home() -> PathBuf {
    if let Ok(val) = std::env::var("HERMES_HOME") {
        let trimmed = val.trim().to_string();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home).join(".hermes");
        }
    }
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        if !userprofile.trim().is_empty() {
            return PathBuf::from(userprofile).join(".hermes");
        }
    }
    if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
        if !localappdata.trim().is_empty() {
            return PathBuf::from(localappdata).join("hermes");
        }
    }
    PathBuf::from(".hermes")
}

fn cron_dir() -> PathBuf {
    get_hermes_home().join("cron")
}

fn suggestions_file() -> PathBuf {
    if let Ok(guard) = SUGGESTIONS_FILE_OVERRIDE.lock() {
        if let Some(p) = guard.clone() {
            return p;
        }
    }
    cron_dir().join("suggestions.json")
}

/// Test-only override for the suggestions file path.
/// Mirrors monkeypatching `cron.suggestions.SUGGESTIONS_FILE` in Python tests.
pub fn set_suggestions_file_override(path: Option<PathBuf>) {
    if let Ok(mut guard) = SUGGESTIONS_FILE_OVERRIDE.lock() {
        *guard = path;
    }
}

fn hermes_now_iso() -> String {
    // Mirrors `hermes_time.now().isoformat()` — timezone-aware.
    // Python resolves HERMES_TIMEZONE / config.yaml; Rust uses UTC as
    // canonical durable timestamp (ISO-8601 / RFC3339).
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// ---------------------------------------------------------------------------
// File helpers — mirrors `_secure_file`, `_ensure_dir`, `atomic_replace`
// ---------------------------------------------------------------------------

fn _secure_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = fs::set_permissions(path, perms);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

fn _ensure_dir() -> std::io::Result<()> {
    fs::create_dir_all(cron_dir())
}

/// Atomic replace — mirrors `utils.atomic_replace`.
///
/// Resolves symlink targets via `read_link` (preserves symlink — GitHub #16743),
/// then `rename`; on EXDEV (cross-device) falls back to copy+fsync+unlink.
/// Windows contended cases (winerror 5/32/33) not needed on Linux.
fn atomic_replace(tmp: &Path, target: &Path) -> std::io::Result<()> {
    // Resolve symlink like Python's `os.path.realpath` for the target.
    let real_target = if target.is_symlink() {
        match fs::read_link(target) {
            Ok(link) => {
                if link.is_absolute() {
                    link
                } else if let Some(parent) = target.parent() {
                    parent.join(link)
                } else {
                    link
                }
            }
            Err(_) => target.to_path_buf(),
        }
    } else {
        target.to_path_buf()
    };

    match fs::rename(tmp, &real_target) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            // EXDEV / EBUSY fallback — copy + fsync + unlink
            fs::copy(tmp, &real_target)?;
            // preserve mode best-effort is handled by caller via _secure_file
            if let Ok(f) = fs::File::open(&real_target) {
                let _ = f.sync_all();
            }
            let _ = fs::remove_file(tmp);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Load / save — mirrors `_load_raw`, `_save_raw`
// ---------------------------------------------------------------------------

fn _load_raw_vec() -> Vec<Suggestion> {
    let path = suggestions_file();
    if !path.exists() {
        return Vec::new();
    }
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("suggestions.json unreadable ({e}); starting empty");
            return Vec::new();
        }
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("suggestions.json unreadable ({e}); starting empty");
            return Vec::new();
        }
    };
    // Shape 1: {"suggestions": [...]}
    if let Some(obj) = value.as_object() {
        if let Some(arr) = obj.get("suggestions") {
            if let Some(list) = arr.as_array() {
                let mut out = Vec::with_capacity(list.len());
                for item in list {
                    match serde_json::from_value::<Suggestion>(item.clone()) {
                        Ok(s) => out.push(s),
                        Err(e) => {
                            log::warn!("suggestions.json entry unreadable ({e}); skipping");
                        }
                    }
                }
                return out;
            } else {
                log::warn!("suggestions.json malformed; starting empty");
                return Vec::new();
            }
        }
    }
    // Shape 2: legacy list [...]
    if let Some(list) = value.as_array() {
        let mut out = Vec::with_capacity(list.len());
        for item in list {
            match serde_json::from_value::<Suggestion>(item.clone()) {
                Ok(s) => out.push(s),
                Err(e) => {
                    log::warn!("suggestions.json entry unreadable ({e}); skipping");
                }
            }
        }
        return out;
    }
    log::warn!("suggestions.json malformed; starting empty");
    Vec::new()
}

fn _save_raw(suggestions: &[Suggestion]) -> Result<()> {
    _ensure_dir()?;
    let path = suggestions_file();
    let parent = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    fs::create_dir_all(&parent)?;

    // mkstemp equivalent: unique tmp file in parent dir with prefix ".sugg_"
    let tmp_name = format!(".sugg_{}.tmp", Uuid::new_v4().simple());
    let tmp_path = parent.join(tmp_name);

    // Use File::create to mimic mkstemp; ensure we clean up on failure.
    let payload = SuggestionsFile {
        suggestions: suggestions.to_vec(),
        updated_at: Some(hermes_now_iso()),
    };
    let mut created = false;
    let save_result: Result<()> = (|| {
        let mut f = fs::File::create(&tmp_path)?;
        created = true;
        let json = serde_json::to_string_pretty(&payload)?;
        f.write_all(json.as_bytes())?;
        f.write_all(b"\n")?;
        f.flush()?;
        f.sync_all()?;
        drop(f);
        atomic_replace(&tmp_path, &path)?;
        _secure_file(&path);
        Ok(())
    })();

    if save_result.is_err() && created {
        let _ = fs::remove_file(&tmp_path);
    }
    save_result
}

// ---------------------------------------------------------------------------
// Public API — mirrors Python top-level functions 1:1
// ---------------------------------------------------------------------------

/// Return all suggestion records (any status).
/// Mirrors `def load_suggestions() -> List[Dict[str, Any]]`.
pub fn load_suggestions() -> Vec<Suggestion> {
    _load_raw_vec()
}

/// Return pending suggestions in creation order (oldest first).
/// Mirrors `def list_pending() -> List[Dict[str, Any]]`.
pub fn list_pending() -> Vec<Suggestion> {
    load_suggestions()
        .into_iter()
        .filter(|s| s.status == STATUS_PENDING)
        .collect()
}

/// Register a pending suggestion. Returns the record, or None if skipped.
///
/// Skipped when: the same `dedup_key` was already dismissed or accepted
/// (never re-offer), an identical pending suggestion exists, or the pending
/// list is full (`MAX_PENDING`).
///
/// `job_spec` is a JSON value of kwargs for `cron.jobs.create_job` — accepting
/// the suggestion passes it straight through.
///
/// Mirrors `def add_suggestion(*, title, description, source, job_spec, dedup_key)`.
pub fn add_suggestion(
    title: &str,
    description: &str,
    source: &str,
    job_spec: Value,
    dedup_key: &str,
) -> Result<Option<Suggestion>> {
    if !VALID_SOURCES.contains(&source) {
        return Err(SuggestionError::UnknownSource(source.to_string()));
    }
    if title.trim().is_empty() || dedup_key.trim().is_empty() {
        return Err(SuggestionError::MissingField);
    }

    let _guard = SUGGESTIONS_LOCK.lock().unwrap();
    let mut suggestions = _load_raw_vec();

    // Never re-offer something the user already saw and decided on, and
    // never duplicate a still-pending proposal.
    for existing in &suggestions {
        if existing.dedup_key == dedup_key.trim() {
            if existing.status == STATUS_DISMISSED || existing.status == STATUS_ACCEPTED {
                return Ok(None);
            }
            if existing.status == STATUS_PENDING {
                return Ok(None);
            }
        }
    }

    let pending_count = suggestions
        .iter()
        .filter(|s| s.status == STATUS_PENDING)
        .count();
    if pending_count >= MAX_PENDING {
        log::info!("Suggestion backlog full ({MAX_PENDING}); dropping {title:?}");
        return Ok(None);
    }

    let record = Suggestion {
        id: Uuid::new_v4().simple().to_string()[..12].to_string(),
        title: title.trim().to_string(),
        description: description.trim().to_string(),
        source: source.to_string(),
        job_spec,
        dedup_key: dedup_key.trim().to_string(),
        status: STATUS_PENDING.to_string(),
        created_at: hermes_now_iso(),
        resolved_at: None,
    };
    suggestions.push(record.clone());
    _save_raw(&suggestions)?;
    Ok(Some(record))
}

/// Resolve a suggestion by id, 1-based pending index, or title (exact, case-insensitive).
/// Mirrors `def get_suggestion(ref: str) -> Optional[Dict[str, Any]]`.
pub fn get_suggestion(ref_: &str) -> Option<Suggestion> {
    let suggestions = load_suggestions();
    // By id.
    for s in &suggestions {
        if s.id == ref_ {
            return Some(s.clone());
        }
    }
    // By 1-based pending index.
    if !ref_.is_empty() && ref_.chars().all(|c| c.is_ascii_digit()) {
        if let Ok(idx) = ref_.parse::<usize>() {
            if idx >= 1 {
                let pending: Vec<&Suggestion> = suggestions
                    .iter()
                    .filter(|s| s.status == STATUS_PENDING)
                    .collect();
                if idx - 1 < pending.len() {
                    return Some((*pending[idx - 1]).clone());
                }
            }
        }
    }
    // By exact title (case-insensitive).
    for s in &suggestions {
        if s.title.to_lowercase() == ref_.to_lowercase() {
            return Some(s.clone());
        }
    }
    None
}

fn _set_status(suggestion_id: &str, status: &str) -> Result<bool> {
    let _guard = SUGGESTIONS_LOCK.lock().unwrap();
    let mut suggestions = _load_raw_vec();
    let mut changed = false;
    for s in &mut suggestions {
        if s.id == suggestion_id {
            s.status = status.to_string();
            s.resolved_at = Some(hermes_now_iso());
            changed = true;
            break;
        }
    }
    if changed {
        _save_raw(&suggestions)?;
    }
    Ok(changed)
}

/// Dismiss a suggestion (latched — never re-offered for its dedup_key).
/// Mirrors `def dismiss_suggestion(ref: str) -> bool`.
pub fn dismiss_suggestion(ref_: &str) -> Result<bool> {
    let s = get_suggestion(ref_);
    match s {
        None => Ok(false),
        Some(rec) => _set_status(&rec.id, STATUS_DISMISSED),
    }
}

/// Accept a suggestion: mark accepted and return its job_spec with origin merged.
///
/// Returns the job_spec (with `origin` merged if provided and not already present),
/// or None if the suggestion isn't found / not pending.
///
/// This is the scheduler-free variant: it does not call `cron.jobs.create_job`.
/// For the full 1:1 that also creates the cron job, use `accept_suggestion_with`.
///
/// Mirrors `def accept_suggestion(ref: str, *, origin: Optional[Dict]=None)`.
pub fn accept_suggestion(ref_: &str, origin: Option<Value>) -> Result<Option<Value>> {
    let s = get_suggestion(ref_);
    let Some(s) = s else {
        return Ok(None);
    };
    if s.status != STATUS_PENDING {
        return Ok(None);
    }
    let mut spec = match s.job_spec.clone() {
        Value::Object(map) => Value::Object(map),
        other => other,
    };
    if let Some(o) = origin {
        if let Value::Object(ref mut map) = spec {
            if !map.contains_key("origin") {
                map.insert("origin".to_string(), o);
            }
        }
    }
    _set_status(&s.id, STATUS_ACCEPTED)?;
    Ok(Some(spec))
}

/// Accept a suggestion and create the real cron job via `create_job`.
///
/// `create_job` is called with the stored `job_spec` (with `origin` merged).
/// On `SchedulerError` the suggestion is still resolved to `accepted` before
/// the error is propagated — mirroring Python's `except CronSchedulerRegistrationError`
/// branch which marks accepted then re-raises so retrying cannot create another
/// local copy.
///
/// Mirrors the full `def accept_suggestion` with `cron.scheduler` integration.
pub fn accept_suggestion_with<F>(
    ref_: &str,
    origin: Option<Value>,
    create_job: F,
) -> std::result::Result<Option<Value>, SuggestionError>
where
    F: FnOnce(Value) -> std::result::Result<Value, SuggestionError>,
{
    let s = get_suggestion(ref_);
    let Some(s) = s else {
        return Ok(None);
    };
    if s.status != STATUS_PENDING {
        return Ok(None);
    }
    let mut spec = s.job_spec.clone();
    if let Some(o) = origin {
        if let Value::Object(ref mut map) = spec {
            if !map.contains_key("origin") {
                map.insert("origin".to_string(), o);
            }
        } else if spec.is_null() {
            let mut map = serde_json::Map::new();
            map.insert("origin".to_string(), o);
            spec = Value::Object(map);
        }
    }
    match create_job(spec.clone()) {
        Ok(job) => {
            _set_status(&s.id, STATUS_ACCEPTED)?;
            Ok(Some(job))
        }
        Err(e) => {
            // The job is already durable. Resolve the suggestion so retrying the
            // same acceptance cannot create another local copy.
            // Only latch on scheduler errors; other errors still latch to avoid double-create.
            let _ = _set_status(&s.id, STATUS_ACCEPTED);
            Err(e)
        }
    }
}

/// Drop accepted records from disk. Returns the count removed.
///
/// Dismissed records are RETAINED for their dedup_key (so they aren't re-offered).
/// This only prunes ACCEPTED records, which have served their purpose once the job exists.
///
/// Mirrors `def clear_resolved() -> int`.
pub fn clear_resolved() -> Result<usize> {
    let _guard = SUGGESTIONS_LOCK.lock().unwrap();
    let suggestions = _load_raw_vec();
    let kept: Vec<Suggestion> = suggestions
        .iter()
        .filter(|s| s.status != STATUS_ACCEPTED)
        .cloned()
        .collect();
    let removed = suggestions.len() - kept.len();
    if removed > 0 {
        _save_raw(&kept)?;
    }
    Ok(removed)
}

// ---------------------------------------------------------------------------
// Self-check (ponytail: one runnable check, no framework)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;

    fn with_temp_home<F: FnOnce()>(f: F) {
        let dir = env::temp_dir().join(format!("hermes-test-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let orig = env::var("HERMES_HOME").ok();
        unsafe { env::set_var("HERMES_HOME", &dir) };
        set_suggestions_file_override(None);
        f();
        // cleanup
        let _ = fs::remove_dir_all(&dir);
        unsafe {
            match orig {
                Some(v) => env::set_var("HERMES_HOME", v),
                None => env::remove_var("HERMES_HOME"),
            }
        }
        set_suggestions_file_override(None);
    }

    #[test]
    fn max_pending_is_5() {
        assert_eq!(MAX_PENDING, 5);
    }

    #[test]
    fn valid_sources_contains_expected() {
        assert!(VALID_SOURCES.contains(&"catalog"));
        assert!(VALID_SOURCES.contains(&"blueprint"));
        assert!(VALID_SOURCES.contains(&"usage"));
        assert!(VALID_SOURCES.contains(&"integration"));
    }

    #[test]
    fn add_and_list_pending_roundtrip() {
        with_temp_home(|| {
            let spec = serde_json::json!({"prompt": "hello", "schedule": "0 9 * * *"});
            let rec = add_suggestion("Daily briefing", "desc", "catalog", spec, "dedup1")
                .unwrap()
                .unwrap();
            assert_eq!(rec.title, "Daily briefing");
            assert_eq!(rec.status, STATUS_PENDING);
            let pending = list_pending();
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].id, rec.id);
        });
    }

    #[test]
    fn dedup_latches_dismissed() {
        with_temp_home(|| {
            let spec = serde_json::json!({"prompt": "hi"});
            add_suggestion("T", "d", "catalog", spec.clone(), "k1")
                .unwrap()
                .unwrap();
            let s = get_suggestion("1").unwrap();
            dismiss_suggestion(&s.id).unwrap();
            // re-add same dedup_key must be dropped
            let second = add_suggestion("T2", "d2", "catalog", spec, "k1").unwrap();
            assert!(second.is_none());
            // dismissed retained after clear_resolved (only accepted pruned)
            assert_eq!(clear_resolved().unwrap(), 0);
            assert_eq!(load_suggestions().len(), 1);
        });
    }

    #[test]
    fn backlog_full_drops() {
        with_temp_home(|| {
            for i in 0..MAX_PENDING {
                add_suggestion(
                    &format!("T{i}"),
                    "d",
                    "catalog",
                    serde_json::json!({}),
                    &format!("k{i}"),
                )
                .unwrap()
                .unwrap();
            }
            let overflow = add_suggestion("extra", "d", "catalog", serde_json::json!({}), "k_extra")
                .unwrap();
            assert!(overflow.is_none());
        });
    }

    #[test]
    fn get_suggestion_by_id_index_title() {
        with_temp_home(|| {
            add_suggestion("Alpha", "d", "catalog", serde_json::json!({}), "ka")
                .unwrap()
                .unwrap();
            add_suggestion("Beta", "d", "usage", serde_json::json!({}), "kb")
                .unwrap()
                .unwrap();
            let by_id = get_suggestion(&load_suggestions()[0].id).unwrap();
            assert_eq!(by_id.title, "Alpha");
            let by_idx = get_suggestion("2").unwrap();
            assert_eq!(by_idx.title, "Beta");
            let by_title = get_suggestion("beta").unwrap();
            assert_eq!(by_title.title, "Beta");
        });
    }

    #[test]
    fn accept_merges_origin_and_clears() {
        with_temp_home(|| {
            add_suggestion(
                "T",
                "d",
                "catalog",
                serde_json::json!({"prompt": "hi"}),
                "k1",
            )
            .unwrap()
            .unwrap();
            let spec = accept_suggestion("1", Some(serde_json::json!({"platform": "cli"})))
                .unwrap()
                .unwrap();
            assert_eq!(spec["origin"]["platform"], "cli");
            assert_eq!(list_pending().len(), 0);
            assert_eq!(clear_resolved().unwrap(), 1);
            assert_eq!(load_suggestions().len(), 0);
        });
    }

    #[test]
    fn unknown_source_errors() {
        with_temp_home(|| {
            let e = add_suggestion("T", "d", "nope", serde_json::json!({}), "k").unwrap_err();
            assert!(matches!(e, SuggestionError::UnknownSource(_)));
        });
    }
}
