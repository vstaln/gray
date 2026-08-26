//! Single source of truth for the agent working directory.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/runtime_cwd.py` (100 lines).
//!
//! `TERMINAL_CWD` is the runtime carrier for the configured working directory
//! (design #19214/#19242: `terminal.cwd` is bridged once to `TERMINAL_CWD` at
//! gateway/cron startup). The local-CLI backend deliberately leaves it unset and
//! relies on the launch dir. Reading it in one place keeps the system prompt, the
//! tool surfaces, and context-file discovery agreeing on where the agent lives.
//!
//! Multi-session gateways can pin a logical cwd via the session cwd
//! thread-local; CLI/cron fall through to `TERMINAL_CWD`/launch cwd.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Session cwd — mirrors `ContextVar("HERMES_SESSION_CWD", default=_UNSET)`
// (lines 14-23). Python uses a ContextVar with an _UNSET sentinel; Rust uses
// a thread-local Option where None == _UNSET and Some(value) == set (even if
// empty string after trim, matching clear_session_cwd's "" semantics).
// ---------------------------------------------------------------------------

thread_local! {
    static SESSION_CWD: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Token returned by [`set_session_cwd`] — the previous value of the
/// session cwd. Mirrors `contextvars.Token` (line 44). Pass to
/// [`reset_session_cwd`] to restore, matching `rt._SESSION_CWD.reset(token)`
/// in tests.
pub type SessionCwdToken = Option<String>;

/// Package/source root — mirrors `_PACKAGE_ROOT = Path(__file__).resolve().parent.parent`
/// (lines 25-30). In Python the file lives at `<root>/agent/runtime_cwd.py`; in
/// Rust the manifest lives at `<root>/crates/hermes-core`, so parent.parent
/// of the manifest dir is the workspace root. Compile-time constant via
/// `CARGO_MANIFEST_DIR`.
pub fn package_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is absolute at compile time: /.../crates/hermes-core
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or(manifest)
}

/// Mirrors `_is_install_tree` (lines 33-41).
///
/// True only when p IS the package root or sits inside it. Ancestors of the
/// package root (a user home that happens to contain the checkout) are
/// legitimate workspaces and must not be blocked.
pub fn is_install_tree(p: &Path) -> bool {
    let resolved = match p.canonicalize() {
        Ok(r) => r,
        Err(_) => return false,
    };
    let root = match package_root().canonicalize() {
        Ok(r) => r,
        Err(_) => package_root(),
    };
    if resolved == root {
        return true;
    }
    resolved.starts_with(&root)
}

// Keep underscore-prefixed alias for 1:1 traceability with Python private name
#[allow(dead_code)]
fn _is_install_tree(p: &Path) -> bool {
    is_install_tree(p)
}

/// Mirrors `_PACKAGE_ROOT` constant access for callers that borrowed it
/// directly (e.g. `prompt_builder.py` monkeypatches `rt._PACKAGE_ROOT`).
pub fn package_root_path() -> PathBuf {
    package_root()
}

// ---------------------------------------------------------------------------
// Session cwd helpers — mirrors lines 44-57
// ---------------------------------------------------------------------------

/// Pin the logical cwd for the current context.
/// Mirrors `set_session_cwd(cwd: str | None) -> Token` (lines 44-46):
/// `return _SESSION_CWD.set((cwd or "").strip())`
pub fn set_session_cwd(cwd: Option<&str>) -> SessionCwdToken {
    let normalized = cwd.unwrap_or("").trim().to_string();
    SESSION_CWD.with(|cell| {
        let prev = cell.borrow().clone();
        *cell.borrow_mut() = Some(normalized);
        prev
    })
}

/// Convenience overload accepting `&str` directly (empty handling via trim).
pub fn set_session_cwd_str(cwd: &str) -> SessionCwdToken {
    set_session_cwd(Some(cwd))
}

/// Restore a previous token — mirrors `ContextVar.reset(token)`.
pub fn reset_session_cwd(token: SessionCwdToken) {
    SESSION_CWD.with(|cell| {
        *cell.borrow_mut() = token;
    })
}

/// Mirrors `clear_session_cwd()` (lines 49-50): `_SESSION_CWD.set("")`.
pub fn clear_session_cwd() {
    SESSION_CWD.with(|cell| {
        *cell.borrow_mut() = Some(String::new());
    });
}

fn session_cwd_override() -> String {
    // Mirrors `_session_cwd_override()` (lines 53-57):
    //   value = _SESSION_CWD.get()
    //   if value is _UNSET: return ""
    //   return str(value).strip()
    SESSION_CWD.with(|cell| match &*cell.borrow() {
        None => String::new(),
        Some(v) => v.trim().to_string(),
    })
}

#[allow(dead_code)]
fn _session_cwd_override() -> String {
    session_cwd_override()
}

// ---------------------------------------------------------------------------
// expanduser + helpers — mirrors `Path(...).expanduser()` (lines 63,69,87,95)
// ---------------------------------------------------------------------------

fn expanduser(p: &str) -> PathBuf {
    let trimmed = p.trim();
    if trimmed == "~" || trimmed.starts_with("~/") || trimmed.starts_with("~\\") {
        // Resolve HOME (Unix) or USERPROFILE (Windows) — mirrors os.path.expanduser
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default();
        if home.is_empty() {
            return PathBuf::from(trimmed);
        }
        if trimmed == "~" {
            return PathBuf::from(home);
        }
        // "~/..." -> "<home>/..."
        let suffix = &trimmed[2..];
        // Handle both "/" and "\" separators after ~
        let suffix = suffix.trim_start_matches(['/', '\\']);
        if suffix.is_empty() {
            return PathBuf::from(home);
        }
        return PathBuf::from(home).join(suffix);
    }
    PathBuf::from(trimmed)
}

// ---------------------------------------------------------------------------
// resolve_agent_cwd — mirrors lines 60-73
// ---------------------------------------------------------------------------

/// Resolve the agent working directory.
///
/// Mirrors `resolve_agent_cwd() -> Path` (lines 60-73):
/// 1. session cwd override if non-empty and is_dir
/// 2. `TERMINAL_CWD` env if non-empty and is_dir
/// 3. `os.getcwd()` fallback (propagates error if cwd gone — caller owns the
///    try/except, matching `prompt_builder.py:805`)
///
/// Returns `PathBuf`. If the current directory cannot be determined
/// (deleted cwd) this panics with the underlying IO error, mirroring Python's
/// `OSError` propagation. Use [`try_resolve_agent_cwd`] for a `Result` variant.
pub fn resolve_agent_cwd() -> PathBuf {
    try_resolve_agent_cwd().unwrap_or_else(|e| panic!("resolve_agent_cwd: current dir unavailable: {e}"))
}

/// Fallible variant — returns `Err` if `current_dir` fails (deleted cwd).
/// Mirrors the `os.getcwd()` OSError propagation noted in tests (line 35-38).
pub fn try_resolve_agent_cwd() -> std::io::Result<PathBuf> {
    let ov = session_cwd_override();
    if !ov.is_empty() {
        let p = expanduser(&ov);
        if p.is_dir() {
            return Ok(p);
        }
        log::warn!("configured working directory does not exist: {}", ov);
    }
    let raw = std::env::var("TERMINAL_CWD").unwrap_or_default();
    let raw = raw.trim().to_string();
    if !raw.is_empty() {
        let p = expanduser(&raw);
        if p.is_dir() {
            return Ok(p);
        }
        log::warn!("TERMINAL_CWD does not exist: {}", raw);
    }
    std::env::current_dir()
}

// ---------------------------------------------------------------------------
// resolve_context_cwd — mirrors lines 76-100
// ---------------------------------------------------------------------------

/// Resolve the context cwd.
///
/// Mirrors `resolve_context_cwd() -> Path | None` (lines 76-100):
/// - None means "no configured cwd": caller falls back to launch dir
///   (`os.getcwd()`), correct for a local CLI launched inside a real project.
/// - A configured path is validated (previously passed through unchecked).
/// - An explicitly configured path is otherwise honored verbatim — including
///   the Hermes source tree itself when developing Hermes.
pub fn resolve_context_cwd() -> Option<PathBuf> {
    let ov = session_cwd_override();
    if !ov.is_empty() {
        let p = expanduser(&ov);
        if !p.is_dir() {
            log::warn!("configured working directory does not exist: {}", ov);
        } else {
            return Some(p);
        }
        return None;
    }
    let raw = std::env::var("TERMINAL_CWD").unwrap_or_default();
    let raw = raw.trim().to_string();
    if !raw.is_empty() {
        let p = expanduser(&raw);
        if !p.is_dir() {
            log::warn!("TERMINAL_CWD does not exist: {}", raw);
        } else {
            return Some(p);
        }
    }
    None
}
