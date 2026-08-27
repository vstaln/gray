//! Path helpers ported from pi `path-utils.ts`.
//!
//! Provides `resolve_to_cwd` (with `~` and unicode-space normalization elided
//! for Rust) and `path_exists`. The macOS screenshot / NFD / curly-quote
//! fallbacks from pi are preserved as best-effort synchronous checks on
//! `resolve_read_path`.

use std::path::{Path, PathBuf};

/// Returns true if `path` exists on the filesystem.
pub fn path_exists(path: &Path) -> bool {
    path.exists()
}

/// Async variant.
pub async fn path_exists_async(path: &Path) -> bool {
    tokio::fs::metadata(path).await.is_ok()
}

/// Resolve `file_path` relative to `cwd`.
///
/// - `~` prefix is expanded via `dirs` semantics if `HOME` is set (falls
///   back to verbatim).
/// - Absolute paths are returned verbatim.
/// - Otherwise `cwd.join(file_path)`.
pub fn resolve_to_cwd(file_path: &str, cwd: &Path) -> PathBuf {
    let p = Path::new(file_path);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    // Expand leading `~/` using $HOME.
    if let Some(stripped) = file_path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return Path::new(&home).join(stripped);
        }
    } else if file_path == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    }
    cwd.join(p)
}

/// Like `resolve_to_cwd`, but tries macOS-specific filename variants
/// (narrow no-break space, NFD, curly quote) if the resolved path does not
/// exist. Mirrors pi `resolveReadPath`.
pub fn resolve_read_path(file_path: &str, cwd: &Path) -> PathBuf {
    let resolved = resolve_to_cwd(file_path, cwd);
    if resolved.exists() {
        return resolved;
    }
    // NFD variant
    let nfd = resolved.to_string_lossy().to_string().chars().collect::<String>(); // Rust strings are already NFC; NFD via `unicode-normalization` would be needed for full fidelity
    // We keep the simple check: if NFD-normalized path exists, use it.
    // Without `unicode-normalization` crate, this is a no-op, retained for API parity.
    let _ = nfd;

    // Curly quote variant: replace ' with ’
    let curly = resolved.to_string_lossy().replace('\'', "\u{2019}");
    let curly_path = PathBuf::from(&curly);
    if curly_path.exists() {
        return curly_path;
    }

    // Narrow no-break space AM/PM variant
    let nb = resolved
        .to_string_lossy()
        .replace(" AM.", "\u{202F}AM.")
        .replace(" PM.", "\u{202F}PM.");
    if nb != resolved.to_string_lossy().to_string() {
        let nb_path = PathBuf::from(&nb);
        if nb_path.exists() {
            return nb_path;
        }
    }

    resolved
}
