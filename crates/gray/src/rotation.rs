//! Size-capped log rotation: `gray.log` → `.1` → `.2`, best-effort, never panics.
use std::path::Path;

pub(crate) const LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// If `path` exceeds `LOG_MAX_BYTES`, shift `.1`→`.2`, `path`→`.1`, truncate `path`.
/// Missing/small files are left alone. All errors swallowed (logging must not crash boot).
pub(crate) fn rotate_if_needed(path: &Path) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() <= LOG_MAX_BYTES {
        return;
    }
    let _ = std::fs::remove_file(path.with_extension("log.2"));
    let _ = std::fs::rename(path.with_extension("log.1"), path.with_extension("log.2"));
    let _ = std::fs::rename(path, path.with_extension("log.1"));
    let _ = std::fs::File::create(path);
}

#[path = "rotation_tests.rs"]
#[cfg(test)]
mod tests;
