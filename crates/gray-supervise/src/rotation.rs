//! Size-capped log rotation: `gray.log` → `.1` → `.2`, best-effort, never panics.
use std::path::Path;

pub const LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// If `path` exceeds `LOG_MAX_BYTES`, shift `.1`→`.2`, `path`→`.1`, truncate `path`.
/// Missing/small files are left alone. All errors swallowed (logging must not crash boot).
pub fn rotate_if_needed(path: &Path) {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_log_rotates_and_caps() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("gray.log");
        std::fs::write(&log, vec![b'x'; (LOG_MAX_BYTES + 1) as usize]).unwrap();
        std::fs::write(dir.path().join("gray.log.1"), b"old1").unwrap();
        std::fs::write(dir.path().join("gray.log.2"), b"old2").unwrap();
        rotate_if_needed(&log);
        assert!(std::fs::metadata(&log).unwrap().len() < LOG_MAX_BYTES);
        assert!(!dir.path().join("gray.log.3").exists(), "must cap at .2");
    }
    #[test]
    fn small_log_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("gray.log");
        std::fs::write(&log, b"tiny").unwrap();
        rotate_if_needed(&log);
        assert_eq!(std::fs::read(&log).unwrap(), b"tiny");
    }
}
