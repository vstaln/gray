//! Detect when the gateway is running stale code after a hot `git pull`.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/code_skew.py` (64 LOC).
//!
//! Python source docstring (preserved):
//! ```text
//! Detect when the gateway is running stale code after a hot ``git pull``.
//!
//! The gateway is a single long-lived process; its ``sys.modules`` is frozen at
//! boot. If the checkout is updated underneath it (a manual ``git pull``, or the
//! window before ``hermes update``'s graceful restart fires), a first-time lazy
//! import on a new code path can resolve a freshly-pulled consumer module against a
//! stale cached dependency -> ImportError (see
//! ``tests/test_stale_utils_module_import.py`` for the exact failure).
//!
//! We snapshot the checkout revision at gateway startup and compare on demand, so
//! risky callers (e.g. ``/model`` switching) can refuse with a clear "restart the
//! gateway" message instead of crashing on a cryptic import error.
//!
//! If the revision can't be read (non-git install, IO error), the boot snapshot
//! stays ``None`` and skew detection no-ops — it never produces a false positive.
//! ```
//!
//! Mapping:
//! - `_PROJECT_ROOT = Path(__file__).resolve().parent.parent` → [`_project_root`] / [`project_root`]
//! - `_boot_fingerprint: str | None = None` → [`_boot_fingerprint`] static via `OnceLock<Mutex<Option<String>>>`
//! - `def _fingerprint() -> str | None` → [`_fingerprint`] (reuses `read_git_revision_fingerprint`)
//! - `def record_boot_fingerprint() -> None` → [`record_boot_fingerprint`] (idempotent)
//! - `def _short(fingerprint: str) -> str` → [`_short`]
//! - `def detect_code_skew() -> tuple[str, str] | None` → [`detect_code_skew`]
//! - `from hermes_cli.main import _read_git_revision_fingerprint` → [`read_git_revision_fingerprint`] (same worktree-aware logic, no subprocess)
//! - `except Exception: return None` → `Option::None` on any `OSError` / missing `.git`

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// _PROJECT_ROOT — mirrors Python module-level constant
// ---------------------------------------------------------------------------

/// Resolve the checkout root (parent of `gateway/`).
///
/// Mirrors ` _PROJECT_ROOT = Path(__file__).resolve().parent.parent`.
///
/// In Rust `__file__` is compile-time; we approximate at runtime:
/// - `$HERMES_REPO_ROOT` if set (explicit override, same as `hermes_cli` bootstrap)
/// - `CARGO_MANIFEST_DIR` parent.parent (crate `crates/hermes-gateway` → repo root) when available
/// - `current_dir()` fallback (matches `hermes_cli/main.py::bootstrap_root` fallback)
pub fn _project_root() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_REPO_ROOT") {
        let t = v.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    // Compile-time crate location → repo root (no cargo at runtime, just env! expansion)
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(parent) = manifest.parent() {
        if let Some(repo) = parent.parent() {
            return repo.to_path_buf();
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Public alias.
pub fn project_root() -> PathBuf {
    _project_root()
}

// ---------------------------------------------------------------------------
// Boot fingerprint — mirrors `_boot_fingerprint: str | None = None`
// ---------------------------------------------------------------------------

static BOOT_FINGERPRINT: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn boot_cell() -> &'static Mutex<Option<String>> {
    BOOT_FINGERPRINT.get_or_init(|| Mutex::new(None))
}

/// Read current boot snapshot (clone).
pub fn _boot_fingerprint() -> Option<String> {
    boot_cell().lock().ok().and_then(|g| g.clone())
}

// ---------------------------------------------------------------------------
// _read_packed_ref / _read_git_revision_fingerprint — mirrors hermes_cli/main.py
// ---------------------------------------------------------------------------

fn _read_packed_ref(common_dir: &Path, target_ref: &str) -> Option<String> {
    // Mirrors `hermes_cli/main.py::_read_packed_ref` (858-874)
    let text = std::fs::read_to_string(common_dir.join("packed-refs")).ok()?;
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        let mut parts = line.splitn(2, ' ');
        let sha = parts.next()?;
        let r = parts.next()?;
        if r.trim() == target_ref {
            return Some(sha.trim().to_string());
        }
    }
    None
}

/// Return a cheap checkout fingerprint without spawning git.
///
/// Mirrors `hermes_cli/main.py::_read_git_revision_fingerprint` (877-919).
/// Format: `git:<ref>:<sha>` or `git:HEAD:<sha>` or `git:<ref>:unresolved`.
pub fn read_git_revision_fingerprint(repo_root: &Path) -> Option<String> {
    let mut git_dir = repo_root.join(".git");
    if git_dir.is_file() {
        if let Ok(text) = std::fs::read_to_string(&git_dir) {
            for line in text.lines() {
                let mut kv = line.splitn(2, ':');
                let k = kv.next()?.trim();
                let v = kv.next()?.trim();
                if k == "gitdir" && !v.is_empty() {
                    // `repo_root / value` then resolve (canonicalize best-effort)
                    let candidate = repo_root.join(v);
                    git_dir = std::fs::canonicalize(&candidate).unwrap_or(candidate);
                    break;
                }
            }
        }
    }
    let mut common_dir = git_dir.clone();
    let commondir_file = git_dir.join("commondir");
    if commondir_file.exists() {
        if let Ok(rel) = std::fs::read_to_string(&commondir_file) {
            let rel = rel.trim();
            if !rel.is_empty() {
                let candidate = git_dir.join(rel);
                common_dir = std::fs::canonicalize(&candidate).unwrap_or(candidate);
            }
        }
    }
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim().to_string();
    if head.starts_with("ref:") {
        let r = head["ref:".len()..].trim().to_string();
        for cand in [&git_dir, &common_dir] {
            let ref_file = cand.join(&r);
            if ref_file.exists() {
                if let Ok(sha) = std::fs::read_to_string(&ref_file) {
                    return Some(format!("git:{}:{}", r, sha.trim()));
                }
            }
        }
        if let Some(sha) = _read_packed_ref(&common_dir, &r) {
            return Some(format!("git:{}:{}", r, sha));
        }
        return Some(format!("git:{}:unresolved", r));
    }
    Some(format!("git:HEAD:{}", head))
}

// Provide private alias mirroring Python's import path for grep-ability.
#[allow(dead_code)]
fn _read_git_revision_fingerprint(repo_root: &Path) -> Option<String> {
    read_git_revision_fingerprint(repo_root)
}

// ---------------------------------------------------------------------------
// _fingerprint — mirrors Python `def _fingerprint() -> str | None`
// ---------------------------------------------------------------------------

/// Current checkout fingerprint, reusing the CLI's git-rev reader.
///
/// Mirrors:
/// ```python
/// def _fingerprint() -> str | None:
///     try:
///         from hermes_cli.main import _read_git_revision_fingerprint
///         return _read_git_revision_fingerprint(_PROJECT_ROOT)
///     except Exception:
///         return None
/// ```
pub fn _fingerprint() -> Option<String> {
    // Never panics — any OSError → None (mirrors `except Exception: return None`)
    let root = _project_root();
    // catch_unwind mirrors the Python `except Exception` (IO, parse, missing)
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        read_git_revision_fingerprint(&root)
    }));
    match result {
        Ok(v) => v,
        Err(_) => None,
    }
}

/// Testable variant with explicit root (for unit tests without touching global `_PROJECT_ROOT`).
pub fn _fingerprint_with_root(root: &Path) -> Option<String> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        read_git_revision_fingerprint(root)
    }));
    match result {
        Ok(v) => v,
        Err(_) => None,
    }
}

// ---------------------------------------------------------------------------
// record_boot_fingerprint — mirrors Python `def record_boot_fingerprint()`
// ---------------------------------------------------------------------------

/// Snapshot the checkout revision at gateway startup (idempotent).
///
/// Mirrors:
/// ```python
/// def record_boot_fingerprint() -> None:
///     global _boot_fingerprint
///     if _boot_fingerprint is None:
///         _boot_fingerprint = _fingerprint()
/// ```
pub fn record_boot_fingerprint() {
    let cell = boot_cell();
    if let Ok(mut guard) = cell.lock() {
        if guard.is_none() {
            *guard = _fingerprint();
        }
    }
}

/// Test helper: set/clear the boot fingerprint (not in Python, for Rust test isolation).
#[cfg(test)]
pub fn _set_boot_fingerprint_for_test(value: Option<String>) {
    if let Ok(mut guard) = boot_cell().lock() {
        *guard = value;
    }
}

// ---------------------------------------------------------------------------
// _short — mirrors Python `def _short(fingerprint: str) -> str`
// ---------------------------------------------------------------------------

/// Render a `git:<ref>:<sha>` fingerprint as a compact label.
///
/// Mirrors:
/// ```python
/// def _short(fingerprint: str) -> str:
///     sha = fingerprint.rsplit(":", 1)[-1]
///     if sha and sha != "unresolved" and len(sha) > 10:
///         return sha[:10]
///     return sha or fingerprint
/// ```
pub fn _short(fingerprint: &str) -> String {
    let sha = fingerprint.rsplit(':').next().unwrap_or(fingerprint);
    if !sha.is_empty() && sha != "unresolved" && sha.len() > 10 {
        return sha[..10].to_string();
    }
    if sha.is_empty() {
        fingerprint.to_string()
    } else {
        sha.to_string()
    }
}

// ---------------------------------------------------------------------------
// detect_code_skew — mirrors Python `def detect_code_skew()`
// ---------------------------------------------------------------------------

/// Return `(boot_rev, disk_rev)` short labels if the checkout drifted since boot, else `None`.
///
/// Mirrors:
/// ```python
/// def detect_code_skew() -> tuple[str, str] | None:
///     if _boot_fingerprint is None:
///         return None
///     current = _fingerprint()
///     if current is None or current == _boot_fingerprint:
///         return None
///     return _short(_boot_fingerprint), _short(current)
/// ```
pub fn detect_code_skew() -> Option<(String, String)> {
    let boot = boot_cell().lock().ok().and_then(|g| g.clone())?;
    let current = _fingerprint()?;
    if current == boot {
        return None;
    }
    Some((_short(&boot), _short(&current)))
}

/// Testable variant that compares an explicit `boot` against `current` derived from `root`.
/// Mirrors `detect_code_skew` but without touching the global (useful for hermetic tests).
pub fn _detect_code_skew_with(boot: Option<&str>, root: &Path) -> Option<(String, String)> {
    let boot = boot?;
    let current = _fingerprint_with_root(root)?;
    if current == boot {
        return None;
    }
    Some((_short(boot), _short(&current)))
}
