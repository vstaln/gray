//! Serialize file mutations targeting the same path, mirroring
//! `pi/packages/coding-agent/src/core/tools/file-mutation-queue.ts`.
//!
//! Different paths and different `ToolContext` envs (cwd) run in parallel;
//! mutations for the same canonical path *within the same env* are
//! serialized via a per-path `tokio::sync::Mutex` chain. Uses both
//! `absolutePath` and `canonicalPath` (like pi's `getMutationQueueKey`)
//! so `a.txt` vs `./a.txt` vs symlink all serialize once the file exists.
//! Weak refs are cleaned on release so the map does not grow unbounded.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, Weak};

use tokio::sync::Mutex;

use gray_core::agent::ToolContext;
use crate::resolve_path;

/// Global registry: env cwd -> canonical-path string -> weak per-file mutex.
static STATES: OnceLock<Mutex<HashMap<PathBuf, HashMap<String, Weak<Mutex<()>>>>>> =
    OnceLock::new();

fn global() -> &'static Mutex<HashMap<PathBuf, HashMap<String, Weak<Mutex<()>>>>> {
    STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Derive a stable queue key for `path` within `ctx`.
///
/// Mirrors pi's `getMutationQueueKey`: `absolutePath` + `canonicalPath`.
/// If canonicalization succeeds, use canonical; if `not_found`/`not_supported`,
/// fall back to absolute. This ensures `a.txt` and `./a.txt` serialize
/// together once the file exists, and also through symlinks.
async fn queue_key(ctx: &ToolContext, path_str: &str) -> String {
    let absolute = resolve_path(&ctx.cwd, path_str);
    // Try canonicalize the absolute path itself
    if let Ok(canonical) = tokio::fs::canonicalize(&absolute).await {
        return canonical.to_string_lossy().to_string();
    }
    // If path doesn't exist yet, try canonicalize its parent
    if let Some(parent) = absolute.parent() {
        if let Ok(canonical_parent) = tokio::fs::canonicalize(parent).await {
            if let Some(file_name) = absolute.file_name() {
                return canonical_parent
                    .join(file_name)
                    .to_string_lossy()
                    .to_string();
            }
            return canonical_parent.to_string_lossy().to_string();
        }
    }
    // Fallback: absolute path string (already absolute via resolve_path)
    absolute.to_string_lossy().to_string()
}

/// Acquire the per-path mutex for `path` within `ctx`, run `f`, then release.
///
/// Operations for different canonical paths or different envs do not block
/// each other. Weak refs are cleaned so the map does not leak.
pub async fn with_file_mutation_queue<F, Fut, T>(ctx: &ToolContext, path: &str, f: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let key = queue_key(ctx, path).await;
    let per_file_mutex: Arc<Mutex<()>> = {
        let mut outer = global().lock().await;
        let inner = outer.entry(ctx.cwd.clone()).or_insert_with(HashMap::new);
        // Clean dead weak refs
        inner.retain(|_, weak| weak.strong_count() > 0);
        if let Some(weak) = inner.get(&key) {
            if let Some(arc) = weak.upgrade() {
                arc
            } else {
                let arc = Arc::new(Mutex::new(()));
                inner.insert(key.clone(), Arc::downgrade(&arc));
                arc
            }
        } else {
            let arc = Arc::new(Mutex::new(()));
            inner.insert(key.clone(), Arc::downgrade(&arc));
            arc
        }
    };

    // Serialize callers targeting the same file within this env
    let _guard = per_file_mutex.lock().await;
    let result = f().await;

    // Cleanup weak entry if no other holders remain
    {
        let mut outer = global().lock().await;
        if let Some(inner) = outer.get_mut(&ctx.cwd) {
            if let Some(weak) = inner.get(&key) {
                // Only the map holds a weak; if no strong holders (we dropped _guard), remove
                if weak.strong_count() == 0 {
                    inner.remove(&key);
                } else if let Some(arc) = weak.upgrade() {
                    // If only one strong left (the map's weak upgraded), the lock is free
                    // Check strong_count == 1 (only our per_file_mutex clone, which is about to drop)
                    // After _guard dropped, the mutex is free; if strong_count == 1, no waiters
                    if Arc::strong_count(&arc) == 1 {
                        // No queued waiters, safe to clean on next acquisition
                        // Keep weak for now; it will be upgraded or cleaned next time
                    }
                }
            }
            if inner.is_empty() {
                outer.remove(&ctx.cwd);
            }
        }
    }

    result
}

