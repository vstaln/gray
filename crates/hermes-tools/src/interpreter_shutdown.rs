//! Shared interpreter-shutdown detection.
//! Port of `tools/interpreter_shutdown.py` (56 lines) — 1:1 behavior.
//!
//! Single home for the "is the Python interpreter finalizing?" predicate used
//! by every subsystem whose background threads can outlive process teardown
//! (cron delivery, concurrent tool submission, the conversation loop's retry
//! path, background review forks).
//!
//! Once finalization starts, `concurrent.futures` refuses new work with
//! `RuntimeError: cannot schedule new futures after interpreter shutdown` and
//! asyncio's default executor is gone — *any* further attempt to schedule work
//! (an API retry, a thread-pool submit, `asyncio.run`) is doomed and only
//! produces noise: stray `❌` prints after the TUI exited, tracebacks in
//! `errors.log`, and futile retry loops that burn iterations against a dying
//! process (#55924, #58720, and the CLI-exit retry spam this module was
//! extracted for).
//!
//! CPython emits two message variants depending on the failing site:
//!
//! - `cannot schedule new futures after interpreter shutdown` — the
//!   module-global finalization flag (asyncio.run_coroutine_threadsafe, a
//!   torn-down default executor, ThreadPoolExecutor.submit during teardown).
//! - `cannot schedule new futures after shutdown` — a plain
//!   `ThreadPoolExecutor` whose `shutdown()` ran.
//!
//! The common short prefix catches both. Matching the second variant is safe
//! for shutdown detection at every current call site: the pools involved are
//! either module-global daemons or `with`-scoped locals that cannot be shut
//! down mid-use by anything except interpreter finalization.
//!
//! Historically this predicate existed at three sites, each fixed
//! independently as its own incident — `cron/scheduler.py` (#55924/#58720),
//! `agent/tool_executor.py`, and nothing at all in the conversation loop's
//! outer retry handler (the CLI-exit spam). One predicate, all sites.
//!
//! Rust mapping:
//! - Python `sys.is_finalizing()` → Rust `AtomicBool` flag `is_finalizing()` / `set_finalizing()`.
//!   In Python the interpreter sets the flag internally during teardown; in Rust there is
//!   no interpreter finalizer, so callers that embed Python (pyo3) or that want process-
//!   shutdown semantics should call `set_finalizing(true)` from their shutdown hook
//!   (atexit, Drop, signal handler). Until set, `is_finalizing()` returns false — identical
//!   to a non-finalizing interpreter.
//! - Python `str(exc).lower()` → Rust `exc_str.to_ascii_lowercase()` (prefix is ASCII).
//! - Python `exc: Optional[BaseException]` → Rust `Option<&str>` (primary) + generic
//!   `Option<&dyn Display>` helper `interpreter_shutting_down_display` for callers holding
//!   an `Error`/`anyhow` without pre-formatting.

use std::fmt::Display;
use std::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Constant — mirrors `_SHUTDOWN_SUBMIT_ERROR_PREFIX` (line 41)
// ---------------------------------------------------------------------------

/// Mirrors `_SHUTDOWN_SUBMIT_ERROR_PREFIX = "cannot schedule new futures"` (line 41).
///
/// The short prefix catches both CPython variants:
/// - `cannot schedule new futures after interpreter shutdown`
/// - `cannot schedule new futures after shutdown`
pub const SHUTDOWN_SUBMIT_ERROR_PREFIX: &str = "cannot schedule new futures";

// ---------------------------------------------------------------------------
// Finalizing flag — mirrors `sys.is_finalizing()` (line 52)
// ---------------------------------------------------------------------------

static FINALIZING: AtomicBool = AtomicBool::new(false);

/// Mirrors `sys.is_finalizing()` — true when the interpreter/process is finalizing.
///
/// In the Python original this is a runtime query into CPython's finalization
/// flag. In Rust the flag is set explicitly via `set_finalizing(true)` by
/// whatever owns shutdown (pyo3 init, atexit hook, Drop impl). Defaults to
/// `false` — matches a non-finalizing interpreter.
pub fn is_finalizing() -> bool {
    FINALIZING.load(Ordering::SeqCst)
}

/// Set the finalizing flag — mirrors the interpreter entering teardown.
///
/// Call with `true` from shutdown hooks; call with `false` to reset (e.g. in
/// tests). Not present in the Python original (there the interpreter sets it);
/// exposed here so Rust hosts can drive the same predicate.
pub fn set_finalizing(v: bool) {
    FINALIZING.store(v, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// Predicate — mirrors `def interpreter_shutting_down(...)` (lines 44-56)
// ---------------------------------------------------------------------------

/// Return true when the interpreter/process is finalizing.
///
/// Mirrors Python `def interpreter_shutting_down(exc: Optional[BaseException] = None) -> bool:` (lines 44-56):
/// ```python
/// if sys.is_finalizing():
///     return True
/// if exc is not None:
///     return _SHUTDOWN_SUBMIT_ERROR_PREFIX in str(exc).lower()
/// return False
/// ```
///
/// `exc` lets a caller also treat an already-raised scheduling error as a
/// shutdown signal: the `concurrent.futures` module-global flag can be set
/// a hair before `sys.is_finalizing()` flips, so matching the error text
/// is a safe fallback for that race.
///
/// In Rust `exc` is the already-formatted error string (`format!("{}", err)` or
/// `err.to_string()`). Use `interpreter_shutting_down_display` if you hold a
/// `Display` error directly.
pub fn interpreter_shutting_down(exc: Option<&str>) -> bool {
    if is_finalizing() {
        return true;
    }
    if let Some(msg) = exc {
        return is_shutdown_submit_error(msg);
    }
    false
}

/// Convenience wrapper for callers holding a `Display` error (e.g. `anyhow::Error`, `std::io::Error`).
///
/// Mirrors the same predicate but formats via `Display` — equivalent to Python's
/// `str(exc).lower()` applied to the exception's display string.
pub fn interpreter_shutting_down_display<E: Display>(exc: Option<&E>) -> bool {
    if is_finalizing() {
        return true;
    }
    if let Some(e) = exc {
        // format Display then lower — mirrors `str(exc).lower()` exactly
        let s = format!("{e}");
        return is_shutdown_submit_error(&s);
    }
    false
}

/// True when `msg` contains the shutdown-submit prefix (case-insensitive).
///
/// Extracted helper for the `str(exc).lower()` containment check. Mirrors
/// `_SHUTDOWN_SUBMIT_ERROR_PREFIX in str(exc).lower()` (line 55).
pub fn is_shutdown_submit_error(msg: &str) -> bool {
    msg.to_ascii_lowercase()
        .contains(SHUTDOWN_SUBMIT_ERROR_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset() {
        set_finalizing(false);
    }

    #[test]
    fn constant_matches_python() {
        assert_eq!(SHUTDOWN_SUBMIT_ERROR_PREFIX, "cannot schedule new futures");
    }

    #[test]
    fn not_shutting_down_when_no_exc_and_not_finalizing() {
        reset();
        assert!(!interpreter_shutting_down(None));
        assert!(!interpreter_shutting_down(Some("some other error")));
        assert!(!interpreter_shutting_down(Some("")));
    }

    #[test]
    fn shutting_down_when_finalizing_regardless_of_exc() {
        reset();
        set_finalizing(true);
        assert!(interpreter_shutting_down(None));
        assert!(interpreter_shutting_down(Some("irrelevant")));
        assert!(interpreter_shutting_down(Some("")));
        assert!(interpreter_shutting_down_display::<String>(None));
        reset();
        // after reset back to false
        assert!(!interpreter_shutting_down(None));
    }

    #[test]
    fn exc_match_catches_both_cpython_variants() {
        reset();
        // variant 1: module-global finalization flag
        assert!(interpreter_shutting_down(Some(
            "cannot schedule new futures after interpreter shutdown"
        )));
        // variant 2: plain ThreadPoolExecutor shutdown
        assert!(interpreter_shutting_down(Some(
            "cannot schedule new futures after shutdown"
        )));
        // with RuntimeError prefix wrapping (as CPython actually raises)
        assert!(interpreter_shutting_down(Some(
            "RuntimeError: cannot schedule new futures after interpreter shutdown"
        )));
    }

    #[test]
    fn exc_match_is_case_insensitive() {
        reset();
        assert!(interpreter_shutting_down(Some(
            "Cannot Schedule New Futures After Interpreter Shutdown"
        )));
        assert!(interpreter_shutting_down(Some(
            "CANNOT SCHEDULE NEW FUTURES AFTER SHUTDOWN"
        )));
        assert!(is_shutdown_submit_error(
            "Cannot schedule new Futures after Shutdown"
        ));
    }

    #[test]
    fn exc_match_requires_prefix_substring() {
        reset();
        assert!(!interpreter_shutting_down(Some("cannot schedule")));
        assert!(!interpreter_shutting_down(Some("schedule new futures")));
        assert!(!interpreter_shutting_down(Some("interpreter shutdown")));
        assert!(!interpreter_shutting_down(Some("")));
        assert!(!is_shutdown_submit_error("some unrelated RuntimeError"));
    }

    #[test]
    fn display_wrapper_matches_str_version() {
        reset();
        let err = std::io::Error::new(
            std::io::ErrorKind::Other,
            "cannot schedule new futures after interpreter shutdown",
        );
        assert!(interpreter_shutting_down_display(Some(&err)));
        let other = std::io::Error::new(std::io::ErrorKind::Other, "other");
        assert!(!interpreter_shutting_down_display(Some(&other)));
        assert!(!interpreter_shutting_down_display::<std::io::Error>(None));
    }

    #[test]
    fn is_shutdown_submit_error_helper() {
        assert!(is_shutdown_submit_error(
            "cannot schedule new futures after interpreter shutdown"
        ));
        assert!(is_shutdown_submit_error(
            "cannot schedule new futures after shutdown"
        ));
        // prefix as substring → true (short prefix catches both variants)
        assert!(is_shutdown_submit_error("cannot schedule new futuresX"));
        assert!(!is_shutdown_submit_error(""));
    }
}
