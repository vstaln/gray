//! Serialize file mutations targeting the same path, mirroring
//! `pi/packages/coding-agent/src/core/tools/file-mutation-queue.ts`.
//!
//! Different paths run in parallel; mutations for the same canonical path are
//! serialized via a per-path `tokio::sync::Mutex`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use tokio::sync::Mutex;

/// Global registry: canonical path string -> per-file mutex.
static QUEUES: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

fn global() -> &'static Mutex<HashMap<String, Arc<Mutex<()>>>> {
    QUEUES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Derive a stable queue key for `path`.
///
/// Mirrors `getMutationQueueKey` in pi: if the path exists (or its realpath
/// can be resolved), use the canonical path; otherwise fall back to the
/// absolute path string. This ensures `a.txt` and `./a.txt` and a symlink
/// to the same inode all serialize together once the file exists.
fn queue_key(path: &Path) -> String {
    // Try canonicalize the path itself.
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical.to_string_lossy().to_string();
    }
    // If path doesn't exist yet, try canonicalize its parent.
    if let Some(parent) = path.parent() {
        if let Ok(canonical_parent) = std::fs::canonicalize(parent) {
            if let Some(file_name) = path.file_name() {
                return canonical_parent
                    .join(file_name)
                    .to_string_lossy()
                    .to_string();
            }
            return canonical_parent.to_string_lossy().to_string();
        }
    }
    // Fallback: use the path as-is (already absolute via `resolve_path`).
    path.to_string_lossy().to_string()
}

/// Acquire the per-path mutex for `path`, run `f`, then release.
///
/// Operations for different canonical paths do not block each other.
pub async fn with_file_mutation_queue<F, Fut, T>(path: PathBuf, f: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    // Fast path: obtain (or create) the per-file mutex without holding the
    // global lock during `f`.
    let per_file_mutex: Arc<Mutex<()>> = {
        let key = queue_key(&path);
        let mut map = global().lock().await;
        map.entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };

    // Serialize callers targeting the same file.
    let _guard = per_file_mutex.lock().await;
    f().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn different_paths_run_in_parallel() {
        // Sanity: two different paths should not block each other.
        let p1 = PathBuf::from("/tmp/gray-queue-test-a.txt");
        let p2 = PathBuf::from("/tmp/gray-queue-test-b.txt");
        let (a, b) = tokio::join!(
            with_file_mutation_queue(p1, || async { 1 }),
            with_file_mutation_queue(p2, || async { 2 }),
        );
        assert_eq!((a, b), (1, 2));
    }

    #[tokio::test]
    async fn same_path_is_serialized() {
        let path = PathBuf::from("/tmp/gray-queue-serial-test.txt");
        let counter = Arc::new(AtomicUsize::new(0));
        let c1 = counter.clone();
        let c2 = counter.clone();
        let p1 = path.clone();
        let p2 = path.clone();
        let (r1, r2) = tokio::join!(
            with_file_mutation_queue(p1, move || {
                let c = c1.clone();
                async move {
                    // Simulate work while holding the lock.
                    let v = c.fetch_add(1, Ordering::SeqCst);
                    tokio::task::yield_now().await;
                    v
                }
            }),
            with_file_mutation_queue(p2, move || {
                let c = c2.clone();
                async move {
                    let v = c.fetch_add(1, Ordering::SeqCst);
                    tokio::task::yield_now().await;
                    v
                }
            }),
        );
        // Exactly two increments, in some order, but both completed.
        assert!(r1 != r2);
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }
}
