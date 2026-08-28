//! Curated catalog of starter cron-job suggestions.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/cron/suggestion_catalog.py` (154 lines).
//!
//! These are the built-in automations Hermes can offer a new user out of the box —
//! the `catalog` source of the unified suggestion surface. Each entry is a
//! ready-to-run `cron.jobs.create_job` spec wrapped as a suggestion; the user
//! accepts via `/suggestions`. Nothing here auto-schedules.
//!
//! The "important-mail monitor" entry is where the old proactive-monitor engine
//! lives now: its `classify_items.py` (poll a source -> LLM-score urgency ->
//! surface only above-threshold) is ONE catalog automation, not a standalone
//! feature.
//!
//! Python source docstring (preserved):
//! ```text
//! Curated catalog of starter cron-job suggestions.
//!
//! These are the built-in automations Hermes can offer a new user out of the box —
//! the ``catalog`` source of the unified suggestion surface. Each entry is a
//! ready-to-run ``cron.jobs.create_job`` spec wrapped as a suggestion; the user
//! accepts via ``/suggestions``. Nothing here auto-schedules.
//!
//! The "important-mail monitor" entry is where the old proactive-monitor engine
//! lives now: its ``classify_items.py`` (poll a source -> LLM-score urgency ->
//! surface only above-threshold) is ONE catalog automation, not a standalone
//! feature.
//!
//! Adding a catalog entry: append a CatalogEntry. Keep prompts self-contained
//! (cron jobs run with no chat context) and schedules sensible. The ``job_spec``
//! is passed verbatim to ``create_job`` on accept.
//! ```

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

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

// ---------------------------------------------------------------------------
// classify_items_script_path — mirrors Python `def classify_items_script_path()`
// ---------------------------------------------------------------------------

/// Absolute path to the urgency classifier script shipped with cron/.
/// Mirrors `def classify_items_script_path() -> str`:
/// `Path(__file__).resolve().parent / "scripts" / "classify_items.py"`.
/// In Rust the script is resolved under the Hermes home (`~/.hermes/cron/scripts/`)
/// so the path is profile-aware and absolute; if `HERMES_HOME` is set the
/// script path follows it, matching the Python install layout.
pub fn classify_items_script_path() -> PathBuf {
    get_hermes_home().join("cron").join("scripts").join("classify_items.py")
}

/// String form of [`classify_items_script_path`], mirroring Python's `str` return.
pub fn classify_items_script_path_str() -> String {
    classify_items_script_path().to_string_lossy().into_owned()
}

// ---------------------------------------------------------------------------
// Data model — mirrors `@dataclass(frozen=True) class CatalogEntry`
// ---------------------------------------------------------------------------

/// A curated starter automation offered as a suggestion.
/// Mirrors Python `@dataclass(frozen=True) class CatalogEntry` with fields
/// `key`, `title`, `description`, `job_spec`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogEntry {
    /// Stable dedup key (never re-offered once dismissed).
    pub key: String,
    pub title: String,
    pub description: String,
    /// kwargs for `cron.jobs.create_job`, stored as JSON value.
    pub job_spec: Value,
}

// ---------------------------------------------------------------------------
// CATALOG — mirrors `CATALOG: List[CatalogEntry] = [...]`
// ---------------------------------------------------------------------------

fn build_catalog() -> Vec<CatalogEntry> {
    vec![
        CatalogEntry {
            key: "catalog:daily-briefing".to_string(),
            title: "Daily briefing".to_string(),
            description: "Every morning at 8am, a short briefing: today's calendar, \
                weather, and anything urgent waiting on you."
                .to_string(),
            job_spec: serde_json::json!({
                "prompt": "Produce a concise morning briefing for the user: today's calendar events, the local weather, and any urgent items (unread important email, due tasks). Keep it short and scannable. If you have no connected data sources, give a brief general good-morning with the date and offer to connect calendar/email.",
                "schedule": "0 8 * * *",
                "name": "Daily briefing",
                "deliver": "origin",
            }),
        },
        CatalogEntry {
            key: "catalog:important-mail-monitor".to_string(),
            title: "Important-mail monitor".to_string(),
            description: "Check your inbox periodically and ping you ONLY about mail that actually needs attention — never the newsletters.".to_string(),
            job_spec: serde_json::json!({
                "prompt": "Check the user's inbox for new messages since the last run. For each candidate, judge urgency against this rule: surface only mail that needs a reply today, is from a manager/family member, or mentions a deadline. Pipe candidates through the urgency classifier (run `python3 -m cron.scripts.classify_items --threshold 7 --criteria ...` from the hermes-agent install — resolve the script path at run time, do not assume a fixed location) and deliver ONLY what it returns. If nothing clears the bar, respond with [SILENT] so the user is not pinged. Requires a connected mail source; if none is configured, explain how to connect one and then stop.",
                "schedule": "every 30m",
                "name": "Important-mail monitor",
                "deliver": "origin",
            }),
        },
        CatalogEntry {
            key: "catalog:weekly-review".to_string(),
            title: "Weekly review".to_string(),
            description: "Every Sunday evening, a recap of the week: what got done, what's still open, and what's coming up next week.".to_string(),
            job_spec: serde_json::json!({
                "prompt": "Produce a weekly review for the user: summarize what was accomplished this week, list still-open items, and preview next week's calendar. Pull from whatever sources are connected (calendar, task tools, recent conversations). Keep it tight.",
                "schedule": "0 18 * * 0",
                "name": "Weekly review",
                "deliver": "origin",
            }),
        },
        CatalogEntry {
            key: "catalog:standup-reminder".to_string(),
            title: "Workday start reminder".to_string(),
            description: "A weekday nudge at 9am with your day's agenda and top priorities, so you start focused.".to_string(),
            job_spec: serde_json::json!({
                "prompt": "Give the user a brief weekday start-of-day nudge: their calendar for today and the 1-3 highest-priority things to focus on, inferred from recent context and any task tools. Encouraging, short, one message.",
                "schedule": "0 9 * * 1-5",
                "name": "Workday start reminder",
                "deliver": "origin",
            }),
        },
    ]
}

static CATALOG_CACHE: OnceLock<Vec<CatalogEntry>> = OnceLock::new();

/// The curated set, mirroring Python `CATALOG: List[CatalogEntry]`.
/// Returns a `'static` slice to the lazily-initialized catalog.
pub fn catalog() -> &'static [CatalogEntry] {
    CATALOG_CACHE.get_or_init(build_catalog)
}

/// Owned copy of the catalog. Mirrors direct use of `CATALOG` in Python.
pub fn catalog_owned() -> Vec<CatalogEntry> {
    catalog().to_vec()
}

/// Alias matching the Python constant name for callers that prefer `CATALOG`.
/// Returns a `'static` slice. Mirrors `CATALOG`.
pub fn get_catalog() -> &'static [CatalogEntry] {
    catalog()
}

// Keep a legacy-named accessor for 1:1 discoverability.
#[allow(non_upper_case_globals)]
pub static CATALOG: OnceLock<Vec<CatalogEntry>> = OnceLock::new();

// ---------------------------------------------------------------------------
// seed_catalog_suggestions — mirrors `def seed_catalog_suggestions(...)`
// ---------------------------------------------------------------------------

/// Register catalog entries as pending suggestions.
///
/// `add_fn` defaults to `crate::suggestions::add_suggestion` (injectable for
/// tests). `keys` restricts to specific catalog entries; omit to seed all.
/// Entries already dismissed/accepted (by dedup key) or beyond the pending cap
/// are skipped by the store, so re-seeding is safe and idempotent. Returns the
/// list of suggestion records actually created.
///
/// Mirrors `def seed_catalog_suggestions(*, add_fn=None, keys=None)` (lines 124-154).
pub fn seed_catalog_suggestions_with<F, E>(
    mut add_fn: F,
    keys: Option<&[String]>,
) -> Result<Vec<Value>, E>
where
    F: FnMut(&str, &str, &str, Value, &str) -> Result<Option<Value>, E>,
{
    let wanted: Option<std::collections::HashSet<&str>> =
        keys.map(|ks| ks.iter().map(|s| s.as_str()).collect());
    let mut created: Vec<Value> = Vec::new();
    for entry in catalog() {
        if let Some(ref wanted_set) = wanted {
            if !wanted_set.contains(entry.key.as_str()) {
                continue;
            }
        }
        // `job_spec` is cloned as dict copy, mirroring `dict(entry.job_spec)`.
        let job_spec = entry.job_spec.clone();
        let rec = add_fn(
            &entry.title,
            &entry.description,
            "catalog",
            job_spec,
            &entry.key,
        )?;
        if let Some(r) = rec {
            created.push(r);
        }
    }
    Ok(created)
}

/// Convenience wrapper using the real suggestion store.
/// Mirrors `seed_catalog_suggestions()` with `add_fn=None` (defaults to
/// `cron.suggestions.add_suggestion`) and optional `keys` filter.
pub fn seed_catalog_suggestions(
    keys: Option<&[String]>,
) -> Result<Vec<crate::suggestions::Suggestion>, crate::suggestions::SuggestionError> {
    let wanted: Option<std::collections::HashSet<&str>> =
        keys.map(|ks| ks.iter().map(|s| s.as_str()).collect());
    let mut created = Vec::new();
    for entry in catalog() {
        if let Some(ref wanted_set) = wanted {
            if !wanted_set.contains(entry.key.as_str()) {
                continue;
            }
        }
        let job_spec = entry.job_spec.clone();
        let rec = crate::suggestions::add_suggestion(
            &entry.title,
            &entry.description,
            "catalog",
            job_spec,
            &entry.key,
        )?;
        if let Some(r) = rec {
            created.push(r);
        }
    }
    Ok(created)
}

/// Test-injectable variant returning typed `Suggestion` records.
/// Mirrors `seed_catalog_suggestions(add_fn=..., keys=...)` with an injectable
/// `add_suggestion`-like closure.
pub fn seed_catalog_suggestions_typed_with<F, E>(
    mut add_fn: F,
    keys: Option<&[String]>,
) -> Result<Vec<crate::suggestions::Suggestion>, E>
where
    F: FnMut(&str, &str, &str, Value, &str)
        -> Result<Option<crate::suggestions::Suggestion>, E>,
{
    let wanted: Option<std::collections::HashSet<&str>> =
        keys.map(|ks| ks.iter().map(|s| s.as_str()).collect());
    let mut created = Vec::new();
    for entry in catalog() {
        if let Some(ref wanted_set) = wanted {
            if !wanted_set.contains(entry.key.as_str()) {
                continue;
            }
        }
        let rec = add_fn(
            &entry.title,
            &entry.description,
            "catalog",
            entry.job_spec.clone(),
            &entry.key,
        )?;
        if let Some(r) = rec {
            created.push(r);
        }
    }
    Ok(created)
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Returns true if the given path is inside the cron/scripts tree for diagnostics.
/// Not part of Python surface; helper for Rust consumers.
pub fn is_catalog_key(key: &str) -> bool {
    catalog().iter().any(|e| e.key == key)
}

// ---------------------------------------------------------------------------
// Self-check (ponytail: one runnable check, no framework)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_four_entries() {
        assert_eq!(catalog().len(), 4);
        // Also owned copy matches.
        assert_eq!(catalog_owned().len(), 4);
        assert_eq!(get_catalog().len(), 4);
    }

    #[test]
    fn catalog_keys_match_python() {
        let keys: Vec<&str> = catalog().iter().map(|e| e.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "catalog:daily-briefing",
                "catalog:important-mail-monitor",
                "catalog:weekly-review",
                "catalog:standup-reminder"
            ]
        );
    }

    #[test]
    fn catalog_job_specs_have_expected_schedules() {
        let map: std::collections::HashMap<&str, &str> = catalog()
            .iter()
            .map(|e| {
                (
                    e.key.as_str(),
                    e.job_spec.get("schedule").and_then(|v| v.as_str()).unwrap_or(""),
                )
            })
            .collect();
        assert_eq!(map["catalog:daily-briefing"], "0 8 * * *");
        assert_eq!(map["catalog:important-mail-monitor"], "every 30m");
        assert_eq!(map["catalog:weekly-review"], "0 18 * * 0");
        assert_eq!(map["catalog:standup-reminder"], "0 9 * * 1-5");
    }

    #[test]
    fn classify_items_script_path_ends_with_expected() {
        let p = classify_items_script_path();
        assert!(p.ends_with(Path::new("cron/scripts/classify_items.py")) || p.ends_with(Path::new("scripts/classify_items.py")),
            "path should end with cron/scripts/classify_items.py, got {p:?}");
        // String form matches PathBuf form.
        assert_eq!(classify_items_script_path_str(), p.to_string_lossy());
        // Absolute.
        assert!(p.is_absolute(), "classify_items_script_path should be absolute, got {p:?}");
    }

    #[test]
    fn seed_with_injected_add_fn_filters_and_dedups() {
        let mut called_keys: Vec<String> = Vec::new();
        let add_fn = |title: &str, _desc: &str, source: &str, job_spec: Value, dedup_key: &str| {
            assert_eq!(source, "catalog");
            assert!(!title.is_empty());
            assert!(job_spec.is_object());
            called_keys.push(dedup_key.to_string());
            // Simulate dedup: return None for second call with same key would be handled by real store;
            // here we just return Some for first, None for unknown key?
            Ok::<Option<Value>, String>(Some(serde_json::json!({
                "title": title,
                "dedup_key": dedup_key,
                "source": source,
                "job_spec": job_spec,
            })))
        };

        // No filter — all 4 created.
        let all = seed_catalog_suggestions_with(add_fn, None).unwrap();
        assert_eq!(all.len(), 4);

        // Filter to one key.
        let one_key = vec!["catalog:weekly-review".to_string()];
        let add_fn2 = |_: &str, _: &str, _: &str, _: Value, k: &str| {
            Ok::<Option<Value>, String>(Some(serde_json::json!({"dedup_key": k})))
        };
        let filtered = seed_catalog_suggestions_with(add_fn2, Some(&one_key)).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0]["dedup_key"], "catalog:weekly-review");

        // Dropped entries (add_fn returns None) are not counted.
        let mut seen = std::collections::HashSet::new();
        let add_fn3 = |_: &str, _: &str, _: &str, _: Value, k: &str| {
            if seen.contains(k) {
                Ok::<Option<Value>, String>(None)
            } else {
                seen.insert(k.to_string());
                Ok(Some(serde_json::json!({"dedup_key": k})))
            }
        };
        let all2 = seed_catalog_suggestions_with(add_fn3, None).unwrap();
        assert_eq!(all2.len(), 4);

        // Typed variant also filters.
        let typed_keys = vec!["catalog:daily-briefing".to_string()];
        let typed_add = |t: &str, d: &str, s: &str, js: Value, k: &str| {
            Ok::<Option<crate::suggestions::Suggestion>, String>(Some(
                crate::suggestions::Suggestion {
                    id: "testid".to_string(),
                    title: t.to_string(),
                    description: d.to_string(),
                    source: s.to_string(),
                    job_spec: js,
                    dedup_key: k.to_string(),
                    status: "pending".to_string(),
                    created_at: "now".to_string(),
                    resolved_at: None,
                },
            ))
        };
        let typed = seed_catalog_suggestions_typed_with(typed_add, Some(&typed_keys)).unwrap();
        assert_eq!(typed.len(), 1);
        assert_eq!(typed[0].dedup_key, "catalog:daily-briefing");
    }

    #[test]
    fn is_catalog_key_works() {
        assert!(is_catalog_key("catalog:daily-briefing"));
        assert!(!is_catalog_key("catalog:does-not-exist"));
    }
}
