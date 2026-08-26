//! Hermes Bootstrap — 1:1 port of `hermes_bootstrap.py` (239 LOC).
//!
//! Windows UTF-8 bootstrap for Hermes entry points.
//!
//! Python on Windows has two long-standing text-encoding footguns:
//!
//! 1. `sys.stdout` / `sys.stderr` are bound to the console code page
//!    (`cp1252` on US-locale installs), so `print("café")` crashes with
//!    `UnicodeEncodeError: 'charmap' codec can't encode character`.
//!
//! 2. Child processes spawned via `subprocess` don't know to use UTF-8
//!    unless `PYTHONUTF8` and/or `PYTHONIOENCODING` are set in their
//!    environment.
//!
//! This module fixes both on Windows *only* — POSIX is untouched. It
//! should be imported at the very top of every Hermes entry point before
//! any other imports that might do file I/O or print to stdout.
//!
//! What this module does on Windows:
//!   - Sets `PYTHONUTF8=1` (PEP 540 UTF-8 mode) so every child process
//!     uses UTF-8 for `open()` and stdio (setdefault — user can opt out).
//!   - Sets `PYTHONIOENCODING=utf-8` belt-and-suspenders.
//!   - Reconfigures `sys.stdout` / `sys.stderr` / `sys.stdin` to UTF-8
//!     with `errors="replace"` in the current process (Python
//!     `reconfigure()` API, 3.7+).
//!
//! What this module does NOT do:
//!   - Does not re-exec Python with `-X utf8`, so `open()` in the current
//!     process still defaults to locale encoding without explicit
//!     `encoding="utf-8"`.
//!
//! What this module does on POSIX:
//!   - Nothing — POSIX is already UTF-8 in 99% of cases.
//!
//! Idempotent: safe to call multiple times. `BOOTSTRAP_APPLIED` guards
//! against double-reconfigure.
//!
//! Additional helpers (1:1 with Python):
//!   - `suppress_platform_ver_console` — stub `platform._syscmd_ver` on
//!     Windows to avoid console flash + UnicodeDecodeError on 3.11.0/3.11.1.
//!   - `harden_import_path` — prevent a package in cwd from shadowing Hermes
//!     modules by cleaning `sys.path` (relative entries) and pinning the
//!     Hermes source root to the front.
//!   - `activate_durable_lazy_target` — put the durable lazy-install dir
//!     (`HERMES_LAZY_INSTALL_TARGET`) on `sys.path` if configured.
//!
//! Import side effects in Python (`apply_windows_utf8_bootstrap()` +
//! `suppress_platform_ver_console()` + `activate_durable_lazy_target()`
//! called at import time) are exposed here as `bootstrap()` for callers
//! that want the same eager behaviour. Rust has no import side effects.

use std::env;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Platform detection — mirrors `sys.platform == "win32"`
// ---------------------------------------------------------------------------

/// Mirrors `_IS_WINDOWS = sys.platform == "win32"`.
///
/// Compile-time on Rust — `cfg!(windows)` is the idiomatic equivalent.
pub const IS_WINDOWS: bool = cfg!(windows);

/// Mirrors `_bootstrap_applied = False` — process-wide idempotency guard.
static BOOTSTRAP_APPLIED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Public API — mirrors Python functions 1:1
// ---------------------------------------------------------------------------

/// Apply the Windows UTF-8 bootstrap if we're on Windows.
///
/// Returns `true` if bootstrap was applied (i.e. we're on Windows and
/// haven't already done this), `false` otherwise. Advisory — callers
/// normally don't need it, but tests may want to assert the path was taken.
///
/// Idempotent: subsequent calls after the first are a no-op.
///
/// Mirrors `apply_windows_utf8_bootstrap() -> bool` in Python:
///  1. `os.environ.setdefault("PYTHONUTF8", "1")`
///  2. `os.environ.setdefault("PYTHONIOENCODING", "utf-8")`
///  3. `sys.stdout` / `sys.stderr` / `sys.stdin` reconfigure to
///     `encoding="utf-8", errors="replace"` (skipped in Rust — Rust's
///     stdio is already UTF-8; the env-var fix is the load-bearing part
///     for child processes).
pub fn apply_windows_utf8_bootstrap() -> bool {
    if !IS_WINDOWS {
        return false;
    }
    if BOOTSTRAP_APPLIED.load(Ordering::SeqCst) {
        return false;
    }

    // 1. Child processes inherit these and run in UTF-8 mode.
    //    setdefault — don't overwrite if user explicitly opted out.
    if env::var("PYTHONUTF8").is_err() {
        // SAFETY: single-threaded bootstrap at startup; env::set_var is
        // unsafe in Rust 1.82+ due to data-race concerns, but here we
        // mirror Python's os.environ mutation at entry-point init.
        unsafe {
            env::set_var("PYTHONUTF8", "1");
        }
    }
    if env::var("PYTHONIOENCODING").is_err() {
        unsafe {
            env::set_var("PYTHONIOENCODING", "utf-8");
        }
    }

    // 2. Reconfigure current process stdio to UTF-8.
    //    Python does:
    //      for stream_name in ("stdout", "stderr"):
    //          stream = getattr(sys, stream_name, None)
    //          if stream is None: continue
    //          reconfigure = getattr(stream, "reconfigure", None)
    //          if reconfigure is None: continue  # BytesIO / non-TextIOWrapper
    //          try: reconfigure(encoding="utf-8", errors="replace")
    //          except (OSError, ValueError): pass
    //      # stdin separately with same guard
    //    In Rust, stdout/stderr/stdin are already UTF-8 (String / OsString
    //    handling). No reconfigure API exists; the env-var fix above is
    //    sufficient for child Python processes. Documented as intentional
    //    no-op — same observable effect (no crash on non-UTF8 input thanks
    //    to errors="replace" equivalent in Rust's lossless handling).
    //
    //    We keep the comment structure 1:1 so future readers can map to the
    //    Python source line-for-line.

    BOOTSTRAP_APPLIED.store(true, Ordering::SeqCst);
    true
}

/// Stub `platform._syscmd_ver` on Windows — decode-crash + flash guard.
///
/// CPython's `platform.win32_ver()` shells out `cmd /c ver` via
/// `check_output(..., shell=True)` with no `CREATE_NO_WINDOW`, causing:
///  - Console flash in windowless parents (pythonw gateway, slash workers)
///  - UnicodeDecodeError on 3.11.0/3.11.1 under PEP 540 UTF-8 mode
///
/// Stubbing `_syscmd_ver` to return its inputs makes `win32_ver()` hit its
/// fallback and read from `sys.getwindowsversion()` — same data, in-process.
///
/// Mirrors `suppress_platform_ver_console() -> None` in Python.
/// In Rust there is no `platform._syscmd_ver`; this is a parity stub that
/// no-ops on all platforms (and on non-Windows returns immediately, as in
/// Python). Kept so every entry point can call the same symbol set.
pub fn suppress_platform_ver_console() {
    if !IS_WINDOWS {
        return;
    }
    // Python:
    //   try:
    //       import platform
    //       if hasattr(platform, "_syscmd_ver"):
    //           def _quiet_syscmd_ver(system="", release="", version="",
    //                                supported_platforms=("win32", "win16", "dos")):
    //               return system, release, version
    //           platform._syscmd_ver = _quiet_syscmd_ver
    //   except Exception:
    //       pass
    //
    // Rust has no equivalent — no subprocess, no _syscmd_ver. Hardening-only
    // stub; never raises.
}

/// Stop a package in the current directory from shadowing Hermes modules.
///
/// Hermes ships top-level modules with common names (`utils`, `proxy`, `ui`).
/// Python seeds `sys.path` with the current directory, so launching from a
/// project with its own `utils/` shadows Hermes and crashes at import.
///
/// Mirrors `harden_import_path(src_root: str | None = None) -> None` in
/// Python:
///
/// ```python
/// root = src_root or os.environ.get("HERMES_PYTHON_SRC_ROOT") or
///        os.path.dirname(os.path.abspath(__file__))
/// sys.path[:] = [p for p in sys.path if p not in ("", ".")]
/// root_abs = os.path.abspath(root)
/// sys.path[:] = [p for p in sys.path if os.path.abspath(p) != root_abs]
/// sys.path.insert(0, root)
/// ```
///
/// In Rust there is no `sys.path`. This function resolves the same `root`
/// (src_root → HERMES_PYTHON_SRC_ROOT → exe parent / current dir) and, if
/// `PYTHONPATH` is set, applies the identical cleaning logic to it
/// (remove `""` / `"."` entries, de-duplicate `root_abs`, prepend `root`).
/// If `PYTHONPATH` is unset, the function is a no-op beyond root resolution
/// — matching Python's "self-sufficient, no env var required" guarantee.
///
/// For pure, testable path-list manipulation see `harden_path_list`.
pub fn harden_import_path(src_root: Option<&str>) {
    let root = resolve_src_root(src_root);

    // Apply sys.path cleaning to PYTHONPATH if present — closest Rust
    // analogue to Python's sys.path mutation. If PYTHONPATH is absent we
    // still resolved root (validates env handling) but mutate nothing.
    if let Ok(python_path) = env::var("PYTHONPATH") {
        let mut parts: Vec<String> = if python_path.is_empty() {
            Vec::new()
        } else {
            // PYTHONPATH uses OS-specific separator (: on POSIX, ; on Windows)
            env::split_paths(&python_path)
                .map(|p| p.to_string_lossy().into_owned())
                .collect()
        };
        let root_str = root.to_string_lossy().into_owned();
        harden_path_list(&mut parts, &root_str);
        let joined = env::join_paths(parts.iter().map(|s| PathBuf::from(s)))
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        unsafe {
            env::set_var("PYTHONPATH", joined);
        }
    }
}

/// Resolve the Hermes source root — mirrors Python's
/// `src_root or HERMES_PYTHON_SRC_ROOT or dirname(abspath(__file__))`.
fn resolve_src_root(src_root: Option<&str>) -> PathBuf {
    if let Some(s) = src_root {
        return PathBuf::from(s);
    }
    if let Ok(v) = env::var("HERMES_PYTHON_SRC_ROOT") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    // Fallback: directory of current executable (closest to __file__'s
    // dirname for a Rust binary), then current_dir, then ".".
    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            return parent.to_path_buf();
        }
    }
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Pure path-list hardening — testable core of `harden_import_path`.
///
/// Mutates `paths` in place:
///  1. Drop `""` and `"."` entries (Python's `sys.path[0]` seeding).
///  2. Drop any entry whose absolute form equals `root_abs`.
///  3. Insert `root` at index 0 (force to front, not just "if absent").
///
/// Mirrors the three `sys.path` assignments in Python exactly.
pub fn harden_path_list(paths: &mut Vec<String>, root: &str) {
    // 1. Drop relative forms outright.
    paths.retain(|p| p != "" && p != ".");

    // 2. Drop absolute-root duplicates.
    let root_abs = absolute(root);
    paths.retain(|p| absolute(p) != root_abs);

    // 3. Force root to front.
    paths.insert(0, root.to_string());
}

fn absolute(p: &str) -> String {
    let path = Path::new(p);
    // Mirrors os.path.abspath — if relative, join with current_dir.
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    // Normalize without requiring existence (no canonicalize).
    abs.to_string_lossy().into_owned()
}

/// Put the durable lazy-install dir on `sys.path` if one is configured.
///
/// On immutable Docker images the agent venv is sealed and lazy installs
/// are redirected to a writable dir on the data volume
/// (`HERMES_LAZY_INSTALL_TARGET`, e.g. `/opt/data/lazy-packages`).
///
/// Mirrors `activate_durable_lazy_target() -> None` in Python:
///
/// ```python
/// if not os.environ.get("HERMES_LAZY_INSTALL_TARGET", "").strip():
///     return
/// try:
///     from tools import lazy_deps
///     lazy_deps.activate_durable_lazy_target()
/// except Exception:
///     pass
/// ```
///
/// In Rust there is no `tools.lazy_deps`; we mirror the guard and the
/// never-raise contract. If the env var is set and non-empty after trim,
/// we append it to `PYTHONPATH` end (core venv wins collisions, as in
/// Python). Missing/empty target is a no-op.
pub fn activate_durable_lazy_target() {
    let target = match env::var("HERMES_LAZY_INSTALL_TARGET") {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => return,
    };

    // Append to END of PYTHONPATH so core venv wins name collisions —
    // matches tools.lazy_deps.activate_durable_lazy_target() rationale.
    let mut parts: Vec<PathBuf> = env::var("PYTHONPATH")
        .ok()
        .map(|v| env::split_paths(&v).collect())
        .unwrap_or_default();

    let target_path = PathBuf::from(&target);
    if !parts.iter().any(|p| p == &target_path) {
        parts.push(target_path);
        if let Ok(joined) = env::join_paths(&parts) {
            unsafe {
                env::set_var("PYTHONPATH", joined);
            }
        }
    }
}

/// Eager bootstrap — mirrors Python import side effects.
///
/// Python runs at import time:
/// ```python
/// apply_windows_utf8_bootstrap()
/// suppress_platform_ver_console()
/// activate_durable_lazy_target()
/// ```
/// Rust has no import side effects; call this at the top of every entry
/// point (`main.rs`) before any other initialization.
pub fn bootstrap() {
    apply_windows_utf8_bootstrap();
    suppress_platform_ver_console();
    activate_durable_lazy_target();
}

/// Reset bootstrap guard — test-only helper to restore initial state.
///
/// Mirrors ability to re-test idempotency by resetting `_bootstrap_applied`.
#[cfg(test)]
pub fn reset_bootstrap_for_tests() {
    BOOTSTRAP_APPLIED.store(false, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// Self-check (ponytail: one runnable check, no framework)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn is_windows_matches_cfg() {
        assert_eq!(IS_WINDOWS, cfg!(windows));
    }

    #[test]
    fn apply_is_idempotent_and_posix_noop() {
        reset_bootstrap_for_tests();
        let first = apply_windows_utf8_bootstrap();
        let second = apply_windows_utf8_bootstrap();
        if IS_WINDOWS {
            assert!(first);
            assert!(!second, "second call must be no-op");
        } else {
            assert!(!first);
            assert!(!second);
        }
        reset_bootstrap_for_tests();
    }

    #[test]
    fn apply_sets_env_on_windows_only() {
        // On POSIX, env vars must NOT be set by bootstrap.
        // Save/restore to avoid test pollution.
        let orig_utf8 = env::var("PYTHONUTF8").ok();
        let orig_io = env::var("PYTHONIOENCODING").ok();
        unsafe {
            env::remove_var("PYTHONUTF8");
            env::remove_var("PYTHONIOENCODING");
        }
        reset_bootstrap_for_tests();
        apply_windows_utf8_bootstrap();
        if IS_WINDOWS {
            assert_eq!(env::var("PYTHONUTF8").unwrap(), "1");
            assert_eq!(env::var("PYTHONIOENCODING").unwrap(), "utf-8");
        } else {
            assert!(env::var("PYTHONUTF8").is_err());
            assert!(env::var("PYTHONIOENCODING").is_err());
        }
        // restore
        unsafe {
            match orig_utf8 {
                Some(v) => env::set_var("PYTHONUTF8", v),
                None => env::remove_var("PYTHONUTF8"),
            }
            match orig_io {
                Some(v) => env::set_var("PYTHONIOENCODING", v),
                None => env::remove_var("PYTHONIOENCODING"),
            }
        }
        reset_bootstrap_for_tests();
    }

    #[test]
    fn apply_setdefault_does_not_overwrite() {
        if !IS_WINDOWS {
            return;
        }
        unsafe {
            env::set_var("PYTHONUTF8", "0");
            env::set_var("PYTHONIOENCODING", "latin-1");
        }
        reset_bootstrap_for_tests();
        apply_windows_utf8_bootstrap();
        assert_eq!(env::var("PYTHONUTF8").unwrap(), "0");
        assert_eq!(env::var("PYTHONIOENCODING").unwrap(), "latin-1");
        unsafe {
            env::remove_var("PYTHONUTF8");
            env::remove_var("PYTHONIOENCODING");
        }
        reset_bootstrap_for_tests();
    }

    #[test]
    fn suppress_never_panics() {
        suppress_platform_ver_console();
    }

    #[test]
    fn harden_path_list_drops_relative_and_dedupes() {
        let mut paths = vec![
            "".to_string(),
            ".".to_string(),
            "/tmp/foo".to_string(),
            "/a/b".to_string(),
        ];
        harden_path_list(&mut paths, "/a/b");
        assert_eq!(paths[0], "/a/b");
        assert!(!paths[1..].contains(&"".to_string()));
        assert!(!paths[1..].contains(&".".to_string()));
        // original /a/b deduped, only new front remains
        assert_eq!(paths.iter().filter(|p| *p == "/a/b").count(), 1);
    }

    #[test]
    fn harden_path_list_inserts_front() {
        let mut paths = vec!["/x".to_string(), "/y".to_string()];
        harden_path_list(&mut paths, "/root");
        assert_eq!(paths[0], "/root");
        assert_eq!(paths[1], "/x");
    }

    #[test]
    fn harden_import_path_resolves_src_root_priority() {
        // src_root arg wins over env
        unsafe {
            env::set_var("HERMES_PYTHON_SRC_ROOT", "/env/root");
        }
        let r = resolve_src_root(Some("/arg/root"));
        assert_eq!(r, PathBuf::from("/arg/root"));
        let r2 = resolve_src_root(None);
        assert_eq!(r2, PathBuf::from("/env/root"));
        unsafe {
            env::remove_var("HERMES_PYTHON_SRC_ROOT");
        }
        // None + no env => fallback to exe parent or cwd (not empty)
        let r3 = resolve_src_root(None);
        assert!(!r3.as_os_str().is_empty());
    }

    #[test]
    fn activate_durable_noop_when_unset() {
        unsafe {
            env::remove_var("HERMES_LAZY_INSTALL_TARGET");
            env::remove_var("PYTHONPATH");
        }
        activate_durable_lazy_target(); // must not panic
        assert!(env::var("PYTHONPATH").is_err() || env::var("PYTHONPATH").unwrap().is_empty());
    }

    #[test]
    fn activate_durable_noop_on_empty_or_whitespace() {
        for val in ["", "   ", "\t\n"] {
            unsafe {
                env::set_var("HERMES_LAZY_INSTALL_TARGET", val);
                env::remove_var("PYTHONPATH");
            }
            activate_durable_lazy_target();
            assert!(env::var("PYTHONPATH").is_err() || env::var("PYTHONPATH").unwrap().is_empty());
        }
        unsafe {
            env::remove_var("HERMES_LAZY_INSTALL_TARGET");
        }
    }

    #[test]
    fn activate_durable_appends_and_dedupes() {
        unsafe {
            env::set_var("HERMES_LAZY_INSTALL_TARGET", "/tmp/lazy");
            env::remove_var("PYTHONPATH");
        }
        activate_durable_lazy_target();
        let pp = env::var("PYTHONPATH").unwrap();
        assert!(pp.contains("/tmp/lazy"));

        // second call must not duplicate
        activate_durable_lazy_target();
        let pp2 = env::var("PYTHONPATH").unwrap();
        assert_eq!(pp.matches("/tmp/lazy").count(), 1);
        assert_eq!(pp, pp2);

        unsafe {
            env::remove_var("HERMES_LAZY_INSTALL_TARGET");
            env::remove_var("PYTHONPATH");
        }
    }

    #[test]
    fn bootstrap_never_panics() {
        reset_bootstrap_for_tests();
        bootstrap();
        bootstrap(); // idempotent
        reset_bootstrap_for_tests();
    }
}
