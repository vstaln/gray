//! Async/sync bridging helpers.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/async_utils.py` (84 lines).
//!
//! The codebase has ~30 sites that schedule a coroutine onto an event loop from a
//! worker thread via `asyncio.run_coroutine_threadsafe`. That function can raise
//! `RuntimeError` (e.g. the loop was closed during a shutdown race), and when it
//! does the coroutine object is never awaited and never closed — which triggers a
//! `"coroutine '<name>' was never awaited"` RuntimeWarning and leaks the
//! coroutine's frame until GC.
//!
//! `safe_schedule_threadsafe` wraps the call, closes the coroutine on scheduling
//! failure, and returns `None` (instead of a half-formed future) so callers can
//! branch cleanly:
//!
//! ```python
//! fut = safe_schedule_threadsafe(coro, loop)
//! if fut is None:
//!     return  # or fallback behavior
//! fut.result(timeout=5)
//! ```
//!
//! The helper deliberately does NOT also handle `future.result()` failures —
//! that is a separate concern. Once the loop has accepted the coroutine, its
//! lifecycle belongs to the loop, not the scheduling thread.
//!
//! Python source docstring (preserved):
//! ```text
//! Async/sync bridging helpers.
//!
//! The codebase has ~30 sites that schedule a coroutine onto an event loop from a
//! worker thread via :func:`asyncio.run_coroutine_threadsafe`.  That function can
//! raise :class:`RuntimeError` (e.g. the loop was closed during a shutdown race),
//! and when it does the coroutine object is never awaited and never closed —
//! which triggers a ``"coroutine '<name>' was never awaited"`` RuntimeWarning and
//! leaks the coroutine's frame until GC.
//!
//! :func:`safe_schedule_threadsafe` wraps the call, closes the coroutine on
//! scheduling failure, and returns ``None`` (instead of a half-formed future) so
//! callers can branch cleanly:
//!
//!     fut = safe_schedule_threadsafe(coro, loop)
//!     if fut is None:
//!         return  # or fallback behavior
//!     fut.result(timeout=5)
//!
//! The helper deliberately does NOT also handle ``future.result()`` failures —
//! that is a separate concern.  Once the loop has accepted the coroutine, its
//! lifecycle belongs to the loop, not the scheduling thread.
//! ```

use std::fmt;

// ---------------------------------------------------------------------------
// Defaults — mirrors line 31-40 defaults
// ---------------------------------------------------------------------------

/// Default log message — mirrors `log_message: str = "Failed to schedule coroutine on loop"` (line 39).
pub const DEFAULT_LOG_MESSAGE: &str = "Failed to schedule coroutine on loop";

/// Default log level — mirrors `log_level: int = logging.DEBUG` (line 40).
/// In Rust `log::Level::Debug` is the direct equivalent.
pub const DEFAULT_LOG_LEVEL: log::Level = log::Level::Debug;

// Keep underscore-prefixed aliases for 1:1 traceability with Python private names.
#[allow(dead_code)]
const _DEFAULT_LOG_MESSAGE: &str = DEFAULT_LOG_MESSAGE;
#[allow(dead_code)]
const _DEFAULT_LOGGER: &str = env!("CARGO_PKG_NAME");

// ---------------------------------------------------------------------------
// safe_schedule_threadsafe — mirrors lines 34-68
// ---------------------------------------------------------------------------

/// Schedule `future` on `handle` from a sync context, leak-safe.
///
/// Returns `Some(handle)` on success, or `None` if the handle is `None` (mirrors
/// `loop is None`) or `try_schedule` returns `Err` (mirrors
/// `asyncio.run_coroutine_threadsafe` raising, e.g. loop closed during shutdown
/// race). In all failure paths the future is dropped (mirrors `coro.close()`) so
/// it does not trigger `"coroutine was never awaited"` warnings or leak its frame.
///
/// Callers retain full control over what to do with the returned handle
/// (`result(timeout)`, `add_done_callback`, fire-and-forget, etc.).
///
/// Generic over `F` (the future/coroutine), `H` (the event-loop handle), `R`
/// (the returned future handle, e.g. `JoinHandle` / `Future` / `concurrent::Future`),
/// and `E` (scheduling error). The `try_schedule` closure mirrors
/// `asyncio.run_coroutine_threadsafe(coro, loop)` and should return `Err` on
/// any scheduling failure (closed loop, shutdown race, etc.).
///
/// Mirrors `safe_schedule_threadsafe` (lines 34-68):
/// ```python
/// def safe_schedule_threadsafe(coro, loop, *, logger=None, log_message=..., log_level=DEBUG):
///     log = logger if logger is not None else _DEFAULT_LOGGER
///     if loop is None:
///         if asyncio.iscoroutine(coro): coro.close()
///         log.log(log_level, "%s: loop is None", log_message)
///         return None
///     try:
///         return asyncio.run_coroutine_threadsafe(coro, loop)
///     except Exception as exc:
///         if asyncio.iscoroutine(coro): coro.close()
///         log.log(log_level, "%s: %s", log_message, exc)
///         return None
/// ```
///
/// In Rust the `asyncio.iscoroutine` guard is implicit — every `F` is droppable
/// and `drop(future)` is exactly `coro.close()`. Logging uses the `log` crate's
/// global logger at `log_level` (default `Debug`), analogous to the Python
/// `logger` parameter.
pub fn safe_schedule_threadsafe<F, H, R, E>(
    future: F,
    handle: Option<&H>,
    try_schedule: impl FnOnce(F, &H) -> Result<R, E>,
) -> Option<R>
where
    E: fmt::Display,
{
    safe_schedule_threadsafe_with_message(
        future,
        handle,
        try_schedule,
        DEFAULT_LOG_MESSAGE,
        DEFAULT_LOG_LEVEL,
    )
}

/// Variant with explicit `log_message` — mirrors the `log_message` kwarg (line 39).
pub fn safe_schedule_threadsafe_with_message<F, H, R, E>(
    future: F,
    handle: Option<&H>,
    try_schedule: impl FnOnce(F, &H) -> Result<R, E>,
    log_message: &str,
) -> Option<R>
where
    E: fmt::Display,
{
    safe_schedule_threadsafe_with_level(future, handle, try_schedule, log_message, DEFAULT_LOG_LEVEL)
}

/// Full variant with explicit `log_message` and `log_level` — mirrors lines 39-40.
///
/// `log_level` maps directly to `log::Level` (`logging.DEBUG` → `Level::Debug`).
pub fn safe_schedule_threadsafe_with_level<F, H, R, E>(
    future: F,
    handle: Option<&H>,
    try_schedule: impl FnOnce(F, &H) -> Result<R, E>,
    log_message: &str,
    log_level: log::Level,
) -> Option<R>
where
    E: fmt::Display,
{
    // Mirrors lines 54-60: `if loop is None: coro.close(); log; return None`
    let Some(h) = handle else {
        // Mirrors `if asyncio.iscoroutine(coro): coro.close()` — in Rust drop == close
        drop(future);
        log::log!(log_level, "{}: loop is None", log_message);
        return None;
    };

    // Mirrors lines 62-68: `try: return run_coroutine_threadsafe(coro, loop); except Exception: close(); log; return None`
    match try_schedule(future, h) {
        Ok(r) => Some(r),
        Err(exc) => {
            // The future was moved into `try_schedule`; a correct `try_schedule`
            // implementation drops it on `Err` (or returns it for the caller to
            // drop). In the common case the future is already dropped inside
            // `try_schedule`'s error path. If the closure returned `Err` without
            // consuming the future, the closure's own `Err` handling would have
            // dropped it. We log and return None in all cases.
            // For the pure-Rust path where `try_schedule` borrows rather than
            // consumes, the `future` would have been dropped by the `Err` branch
            // below — but that shape is provided by `safe_schedule_threadsafe_borrow`.
            log::log!(log_level, "{}: {}", log_message, exc);
            None
        }
    }
}

/// Borrow-based variant where `try_schedule` borrows the future instead of
/// consuming it. This allows the helper itself to own the `drop(future)` on
/// failure, exactly matching Python's `coro.close()` after `run_coroutine_threadsafe`
/// raises. Useful when the scheduling closure cannot take ownership on failure.
///
/// Mirrors the same Python lines 34-68 but with explicit ownership retained on `Err`.
pub fn safe_schedule_threadsafe_borrow<F, H, R, E>(
    future: F,
    handle: Option<&H>,
    try_schedule: impl FnOnce(&H, &F) -> Result<R, E>,
    log_message: &str,
    log_level: log::Level,
) -> Option<R>
where
    E: fmt::Display,
{
    let Some(h) = handle else {
        drop(future);
        log::log!(log_level, "{}: loop is None", log_message);
        return None;
    };
    match try_schedule(h, &future) {
        Ok(r) => {
            // On success the runtime has logically taken ownership; prevent
            // double-drop of the borrowed future by forgetting it. The caller
            // must ensure the future's resources are now owned by the runtime
            // (e.g. by wrapping in `Box::new` or moving into the closure on
            // success). For simple futures this is a no-op; for owned futures
            // callers should use the owning `safe_schedule_threadsafe` instead.
            std::mem::forget(future);
            Some(r)
        }
        Err(exc) => {
            drop(future);
            log::log!(log_level, "{}: {}", log_message, exc);
            None
        }
    }
}

// ---------------------------------------------------------------------------
// consume_detached_task_result — mirrors lines 71-84
// ---------------------------------------------------------------------------

/// Retrieve a detached task's result without surfacing cancellation.
///
/// Used as an `add_done_callback` on tasks that were cancelled and detached
/// (e.g. an adapter close path that swallows `CancelledError` past its teardown
/// deadline). Observing `task.exception()` prevents "exception was never
/// retrieved" noise on the event loop; cancellation and any terminal error are
/// deliberately swallowed — the task's owner already gave up on it.
///
/// In Rust the equivalent is observing a `JoinHandle` / `Result`. Calling this
/// with the task's `Result` marks the error as observed, preventing
/// Tokio's "task failed" / "exception was never retrieved" warnings. Both
/// cancellation (`JoinError::is_cancelled()` / `CancelledError`) and any other
/// error are swallowed.
///
/// Mirrors `consume_detached_task_result` (lines 71-84):
/// ```python
/// def consume_detached_task_result(task: "asyncio.Future[Any]") -> None:
///     try:
///         task.exception()
///     except (asyncio.CancelledError, Exception):
///         pass
/// ```
pub fn consume_detached_task_result<T, E>(_result: Result<T, E>) {
    // Mirrors `task.exception()` + swallow — observing the Result marks it
    // as retrieved; both CancelledError and generic Exception are swallowed.
    // In Rust the type system already distinguishes cancellation (JoinError)
    // from generic errors; both are ignored here.
}

/// Reference-based variant — mirrors `task.exception()` on a borrowed handle.
pub fn consume_detached_task_result_ref<T, E>(_result: &Result<T, E>) {
    // No-op: borrowing already observed the error without moving it.
}

/// Variant for `Option<Result<T,E>>` (e.g. `JoinHandle` that may be `None` if
/// the task was not spawned). Mirrors the same swallow semantics.
pub fn consume_detached_task_result_opt<T, E>(result: Option<Result<T, E>>) {
    if let Some(r) = result {
        let _ = r;
    }
}

/// Tokio-specific helper that swallows `JoinError` cancellation and panics.
///
/// When `tokio` is available, `JoinError::is_cancelled()` corresponds to
/// `asyncio.CancelledError` and `is_panic()` corresponds to a terminal
/// exception. Both are swallowed. This function is generic over `E: Display`
/// for non-Tokio callers; when `E` is `tokio::task::JoinError` the caller can
/// pass `handle.await`'s result directly.
///
/// Mirrors the `except (CancelledError, Exception): pass` branch (lines 83-84).
pub fn consume_join_result<T, E>(_result: Result<T, E>)
where
    E: fmt::Debug,
{
    // Deliberately empty — the act of receiving the Result proves the
    // exception was observed, which is exactly what `task.exception()` does.
}

// Keep underscore-prefixed alias for 1:1 traceability with Python private name.
#[allow(dead_code)]
fn _consume_detached_task_result<T, E>(result: Result<T, E>) {
    consume_detached_task_result(result)
}
