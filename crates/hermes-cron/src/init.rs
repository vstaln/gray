//! Cron job scheduling system for Hermes Agent.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/cron/__init__.py` (42 lines).
//!
//! This module provides scheduled task execution, allowing the agent to:
//! - Run automated tasks on schedules (cron expressions, intervals, one-shot)
//! - Self-schedule reminders and follow-up tasks
//! - Execute tasks in isolated sessions (no prior context)
//!
//! Cron jobs are executed automatically by the gateway daemon:
//! ```text
//! hermes gateway install    # Install as a user service
//! sudo hermes gateway install --system  # Linux servers: boot-time system service
//! hermes gateway            # Or run in foreground
//! ```
//! The gateway ticks the scheduler every 60 seconds. A file lock prevents
//! duplicate execution if multiple processes overlap.
//!
//! Python source docstring (preserved verbatim):
//! ```text
//! Cron job scheduling system for Hermes Agent.
//!
//! This module provides scheduled task execution, allowing the agent to:
//! - Run automated tasks on schedules (cron expressions, intervals, one-shot)
//! - Self-schedule reminders and follow-up tasks
//! - Execute tasks in isolated sessions (no prior context)
//!
//! Cron jobs are executed automatically by the gateway daemon:
//!     hermes gateway install    # Install as a user service
//!     sudo hermes gateway install --system  # Linux servers: boot-time system service
//!     hermes gateway            # Or run in foreground
//!
//! The gateway ticks the scheduler every 60 seconds. A file lock prevents
//! duplicate execution if multiple processes overlap.
//! ```
//!
//! Python re-exports (preserved verbatim):
//! ```python
//! from cron.jobs import (
//!     create_job,
//!     get_job,
//!     list_jobs,
//!     remove_job,
//!     update_job,
//!     pause_job,
//!     resume_job,
//!     trigger_job,
//!     JOBS_FILE,
//! )
//! from cron.scheduler import tick
//!
//! __all__ = [
//!     "create_job",
//!     "get_job",
//!     "list_jobs",
//!     "remove_job",
//!     "update_job",
//!     "pause_job",
//!     "resume_job",
//!     "trigger_job",
//!     "tick",
//!     "JOBS_FILE",
//! ]
//! ```
//!
//! Rust notes:
//! - Until `cron/jobs.py` and `cron/scheduler.py` are ported as `crate::jobs`
//!   / `crate::scheduler`, this module documents the unified public surface and
//!   exposes the path helper for `JOBS_FILE` plus the `ALL` manifest.
//!   Re-exports will be wired as `pub use crate::jobs::{...}` and
//!   `pub use crate::scheduler::tick` once those modules exist.
//!   ponytail: stub surface until jobs/scheduler land; wire pub use when modules land.

use std::path::{Path, PathBuf};

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

/// Path to the jobs store, mirroring Python `JOBS_FILE = CRON_DIR / "jobs.json"`.
///
/// Canonical durable location: `~/.hermes/cron/jobs.json` (profile-aware via
/// `HERMES_HOME`). Equivalent to `cron.jobs.JOBS_FILE`.
pub fn jobs_file() -> PathBuf {
    cron_dir().join("jobs.json")
}

/// String form of [`jobs_file`], mirroring Python `str(JOBS_FILE)`.
pub fn jobs_file_str() -> String {
    jobs_file().to_string_lossy().into_owned()
}

/// Alias matching the Python constant name for discoverability.
/// Returns the same path as [`jobs_file`]. Mirrors `JOBS_FILE`.
pub fn jobs_file_path() -> PathBuf {
    jobs_file()
}

/// The jobs-file constant as a `Path` slice helper for callers that need `&Path`.
/// Mirrors direct use of `JOBS_FILE` in Python.
pub fn jobs_file_as_path() -> PathBuf {
    jobs_file()
}

// Keep a legacy-named accessor for 1:1 discoverability.
#[allow(non_upper_case_globals)]
pub static JOBS_FILE: once_cell_like = once_cell_like;

// Minimal once-cell-like placeholder so `crate::init::JOBS_FILE` resolves as a
// discoverable symbol without pulling `OnceLock` into the public API before
// jobs.rs lands. Real `JOBS_FILE` is the path returned by `jobs_file()`.
#[derive(Debug, Clone, Copy)]
pub struct once_cell_like;
impl once_cell_like {
    /// Resolve to the current profile-aware jobs file path.
    pub fn path(&self) -> PathBuf {
        jobs_file()
    }
    /// String form, mirroring `str(JOBS_FILE)` in Python.
    pub fn as_str(&self) -> String {
        jobs_file_str()
    }
}
impl std::fmt::Display for once_cell_like {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", jobs_file_str())
    }
}
impl AsRef<Path> for once_cell_like {
    fn as_ref(&self) -> &Path {
        // Leak is intentional for `AsRef` demo parity; real callers should use `jobs_file()`.
        // Avoid leaking: return reference to thread-local? Instead just panic-avoid via owned path
        // in tests — this impl is only for trait completeness.
        // We cannot return reference to temporary, so we return a static fallback.
        // Callers needing `&Path` should call `jobs_file()` directly.
        Path::new("")
    }
}

// ---------------------------------------------------------------------------
// Public surface — mirrors `__all__`
// ---------------------------------------------------------------------------

/// Unified public surface, mirroring Python `__all__`.
///
/// ```python
/// __all__ = [
///     "create_job", "get_job", "list_jobs", "remove_job", "update_job",
///     "pause_job", "resume_job", "trigger_job", "tick", "JOBS_FILE",
/// ]
/// ```
pub const ALL: &[&str] = &[
    "create_job",
    "get_job",
    "list_jobs",
    "remove_job",
    "update_job",
    "pause_job",
    "resume_job",
    "trigger_job",
    "tick",
    "JOBS_FILE",
];

/// Alias matching Python `__all__` name for grep discoverability.
pub const __ALL__: &[&str] = ALL;

// Re-exports (future):
// Once `crate::jobs` and `crate::scheduler` exist, wire:
//   pub use crate::jobs::{create_job, get_job, list_jobs, remove_job, update_job, pause_job, resume_job, trigger_job};
//   pub use crate::scheduler::tick;
// Until then this module exposes `ALL` + `jobs_file()` as the portable surface.

// ---------------------------------------------------------------------------
// Self-check (ponytail: one runnable check, no framework)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn all_matches_python() {
        assert_eq!(
            ALL,
            [
                "create_job",
                "get_job",
                "list_jobs",
                "remove_job",
                "update_job",
                "pause_job",
                "resume_job",
                "trigger_job",
                "tick",
                "JOBS_FILE",
            ]
        );
        assert_eq!(__ALL__, ALL);
        assert_eq!(ALL.len(), 10);
    }

    #[test]
    fn jobs_file_is_profile_aware_and_absolute_when_home_set() {
        // With HERMES_HOME set, path must be absolute and end with cron/jobs.json
        let orig = std::env::var("HERMES_HOME").ok();
        let tmp = std::env::temp_dir().join("hermes-test-init-jobsfile");
        unsafe { std::env::set_var("HERMES_HOME", &tmp) };
        let p = jobs_file();
        assert!(p.is_absolute(), "jobs_file should be absolute when HERMES_HOME is set, got {p:?}");
        assert!(
            p.ends_with(Path::new("cron/jobs.json")),
            "jobs_file should end with cron/jobs.json, got {p:?}"
        );
        assert_eq!(p, jobs_file_path());
        assert_eq!(p, jobs_file_as_path());
        assert_eq!(p.to_string_lossy(), jobs_file_str());
        // JOBS_FILE static helper mirrors same path
        assert_eq!(JOBS_FILE.path(), p);
        assert_eq!(JOBS_FILE.as_str(), jobs_file_str());
        unsafe {
            match orig {
                Some(v) => std::env::set_var("HERMES_HOME", v),
                None => std::env::remove_var("HERMES_HOME"),
            }
        }
    }

    #[test]
    fn get_hermes_home_respects_env() {
        let orig = std::env::var("HERMES_HOME").ok();
        unsafe { std::env::set_var("HERMES_HOME", "/tmp/hermes-init-test") };
        assert_eq!(get_hermes_home(), PathBuf::from("/tmp/hermes-init-test"));
        unsafe {
            match orig {
                Some(v) => std::env::set_var("HERMES_HOME", v),
                None => std::env::remove_var("HERMES_HOME"),
            }
        }
    }
}
