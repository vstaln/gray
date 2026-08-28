//! Best-effort accessors for the single-writer stream fence (#65991).
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/stream_single_writer.py` (70 lines).
//!
//! The fence itself lives on `AIAgent` (`_claim_stream_writer` /
//! `_stream_writer_is_current` in `run_agent.py`), but the streaming code paths
//! that use it live in *other* modules — `chat_completion_helpers` (chat /
//! anthropic / bedrock) and `codex_runtime` (codex responses). Calling the fence
//! directly as `agent._claim_stream_writer()` from those modules makes them
//! hard-depend on the method being present on whatever object is passed in as
//! `agent`.
//!
//! That coupling is a latent crash: a partially-updated checkout (the streaming
//! helper module newer than `run_agent`), a hot-reloaded gateway, a duck-typed
//! agent, or a test double without the method turns an *additive* safety net into a
//! fatal `AttributeError` that aborts the whole turn. A cron job died exactly
//! this way with `'AIAgent' object has no attribute '_claim_stream_writer'`.
//!
//! The fence is only ever allowed to drop a *provably* superseded stream — never
//! the sole legitimate writer. So when the guard is unavailable (or raises), the
//! correct degradation is "no fence": keep streaming. These helpers make the
//! claim/check best-effort to guarantee that.
//!
//! Python source docstring (preserved):
//! ```text
//! Best-effort accessors for the single-writer stream fence (#65991).
//!
//! The fence itself lives on ``AIAgent`` (``_claim_stream_writer`` /
//! ``_stream_writer_is_current`` in ``run_agent.py``), but the streaming code paths
//! that use it live in *other* modules — ``chat_completion_helpers`` (chat /
//! anthropic / bedrock) and ``codex_runtime`` (codex responses). Calling the fence
//! directly as ``agent._claim_stream_writer()`` from those modules makes them
//! hard-depend on the method being present on whatever object is passed in as
//! ``agent``.
//!
//! That coupling is a latent crash: a partially-updated checkout (the streaming
//! helper module newer than ``run_agent``), a hot-reloaded gateway, a duck-typed
//! agent, or a test double without the method turns an *additive* safety net into a
//! fatal ``AttributeError`` that aborts the whole turn. A cron job died exactly
//! this way with ``'AIAgent' object has no attribute '_claim_stream_writer'``.
//!
//! The fence is only ever allowed to drop a *provably* superseded stream — never
//! the sole legitimate writer. So when the guard is unavailable (or raises), the
//! correct degradation is "no fence": keep streaming. These helpers make the
//! claim/check best-effort to guarantee that.
//! ```

// ---------------------------------------------------------------------------
// Constants — mirrors Python `0` sentinel (lines 48, 59-60)
// ---------------------------------------------------------------------------

/// Sentinel token meaning "no fence" — never fences, always current.
/// Mirrors `return 0` in `claim_stream_writer` (line 48) and `if not token: return True` (lines 59-60).
pub const NO_FENCE_TOKEN: i64 = 0;

// Keep underscore-prefixed alias for 1:1 traceability with Python `0` literal.
#[allow(dead_code)]
const _NO_FENCE_TOKEN: i64 = NO_FENCE_TOKEN;

// ---------------------------------------------------------------------------
// Trait — mirrors `AIAgent._claim_stream_writer` / `_stream_writer_is_current`
// (run_agent.py lines 6899-6922). Python uses `getattr(agent, "_claim_...", None)`
// + `callable` check; Rust models the same via `Option<&dyn StreamWriterFence>`:
// `None` = attribute missing / not callable, `Some` = present. The `Result`
// return lets implementations model Python's `except Exception` without panics,
// while the free functions also catch panics to emulate `except Exception` for
// infallible trait impls.
// ---------------------------------------------------------------------------

/// Types that expose the single-writer fence.
///
/// Mirrors `AIAgent._claim_stream_writer() -> int` and
/// `AIAgent._stream_writer_is_current(token: int) -> bool` (run_agent.py).
pub trait StreamWriterFence {
    /// Claim the delta sink, returning a monotonic writer token.
    /// Mirrors `_claim_stream_writer` (run_agent.py:6899).
    fn claim_stream_writer(&self) -> i64;

    /// True when `token` is still the active writer.
    /// Mirrors `_stream_writer_is_current` (run_agent.py:6918).
    fn stream_writer_is_current(&self, token: i64) -> bool;
}

// ---------------------------------------------------------------------------
// Public API — mirrors lines 31-70
// ---------------------------------------------------------------------------

/// Claim the delta sink for the calling stream attempt, best-effort.
///
/// Returns the agent's monotonic writer token when the fence is available, or
/// `0` when the agent doesn't expose it (or the claim raised). A `0` token
/// pairs with [`stream_writer_is_current`] always returning `true`, so a
/// guard-less agent is simply never fenced instead of crashing the turn.
///
/// Mirrors `claim_stream_writer` (lines 31-48):
/// ```python
/// def claim_stream_writer(agent: Any) -> int:
///     claim = getattr(agent, "_claim_stream_writer", None)
///     if callable(claim):
///         try:
///             return int(claim())
///         except Exception:
///             logger.debug(
///                 "stream single-writer: claim failed; proceeding unfenced",
///                 exc_info=True,
///             )
///     return 0
/// ```
pub fn claim_stream_writer(agent: Option<&dyn StreamWriterFence>) -> i64 {
    // Mirrors `claim = getattr(agent, "_claim_stream_writer", None); if callable(claim):`
    // In Rust `None` = getattr returned None / not callable.
    let Some(a) = agent else {
        return NO_FENCE_TOKEN;
    };
    // Mirrors `try: return int(claim()) ; except Exception: logger.debug(...);`
    // Catch both panics (Rust analogue of Python exceptions for infallible impls)
    // and handle the return value directly.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| a.claim_stream_writer()));
    match result {
        Ok(token) => token,
        Err(_) => {
            // Mirrors `logger.debug("stream single-writer: claim failed; proceeding unfenced", exc_info=True)` (lines 44-47)
            log::debug!("stream single-writer: claim failed; proceeding unfenced");
            NO_FENCE_TOKEN
        }
    }
}

/// Generic variant for concrete `T: StreamWriterFence` — avoids trait-object boxing.
/// Mirrors same Python lines 31-48.
pub fn claim_stream_writer_for<T: StreamWriterFence>(agent: Option<&T>) -> i64 {
    match agent {
        None => NO_FENCE_TOKEN,
        Some(a) => {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| a.claim_stream_writer()));
            match result {
                Ok(token) => token,
                Err(_) => {
                    log::debug!("stream single-writer: claim failed; proceeding unfenced");
                    NO_FENCE_TOKEN
                }
            }
        }
    }
}

/// Closure-based variant that directly mirrors the Python `getattr` + `callable` + `try` shape.
///
/// `try_claim` is `None` when `getattr` returned `None` / non-callable (lines 39-40).
/// When `Some`, the closure is invoked inside a `try`/`except` analogue: `Ok` returns
/// the token (via `int()` conversion in Python), `Err` logs and returns `0`.
///
/// This is the closest 1:1 to the Python control flow for callers that already have
/// a dynamically-resolved callable.
pub fn claim_stream_writer_with<F, E>(try_claim: Option<F>) -> i64
where
    F: FnOnce() -> Result<i64, E>,
    E: std::fmt::Display,
{
    let Some(f) = try_claim else {
        return NO_FENCE_TOKEN;
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f()));
    match result {
        Ok(Ok(token)) => token,
        Ok(Err(e)) => {
            log::debug!("stream single-writer: claim failed; proceeding unfenced: {}", e);
            NO_FENCE_TOKEN
        }
        Err(_) => {
            log::debug!("stream single-writer: claim failed; proceeding unfenced");
            NO_FENCE_TOKEN
        }
    }
}

/// True when `token` is still the active writer, best-effort.
///
/// A falsy token (from a claim that no-oped) or an agent without the fence
/// means we cannot prove supersession, so the stream is treated as current and
/// never fenced. This preserves the single-writer invariant's one-way promise:
/// only a demonstrably stale writer is ever stopped.
///
/// Mirrors `stream_writer_is_current` (lines 51-70):
/// ```python
/// def stream_writer_is_current(agent: Any, token: int) -> bool:
///     if not token:
///         return True
///     is_current = getattr(agent, "_stream_writer_is_current", None)
///     if callable(is_current):
///         try:
///             return bool(is_current(token))
///         except Exception:
///             logger.debug(
///                 "stream single-writer: is_current check failed; treating as current",
///                 exc_info=True,
///             )
///     return True
/// ```
pub fn stream_writer_is_current(agent: Option<&dyn StreamWriterFence>, token: i64) -> bool {
    // Mirrors `if not token: return True` (lines 59-60)
    if token == NO_FENCE_TOKEN {
        return true;
    }
    // Mirrors `is_current = getattr(agent, "_stream_writer_is_current", None); if callable(is_current):`
    let Some(a) = agent else {
        return true;
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| a.stream_writer_is_current(token)));
    match result {
        Ok(v) => v,
        Err(_) => {
            // Mirrors `logger.debug("stream single-writer: is_current check failed; treating as current", exc_info=True)` (lines 66-69)
            log::debug!("stream single-writer: is_current check failed; treating as current");
            true
        }
    }
}

/// Generic variant for concrete `T: StreamWriterFence`.
/// Mirrors same Python lines 51-70.
pub fn stream_writer_is_current_for<T: StreamWriterFence>(agent: Option<&T>, token: i64) -> bool {
    if token == NO_FENCE_TOKEN {
        return true;
    }
    let Some(a) = agent else {
        return true;
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| a.stream_writer_is_current(token)));
    match result {
        Ok(v) => v,
        Err(_) => {
            log::debug!("stream single-writer: is_current check failed; treating as current");
            true
        }
    }
}

/// Closure-based variant mirroring `getattr` + `callable` + `try` for `is_current`.
///
/// `try_is_current` is `None` when `getattr` returned `None` / non-callable (lines 61-62).
/// When `Some`, invoked as `bool(is_current(token))` with `except Exception` → `True`.
pub fn stream_writer_is_current_with<F, E>(token: i64, try_is_current: Option<F>) -> bool
where
    F: FnOnce(i64) -> Result<bool, E>,
    E: std::fmt::Display,
{
    if token == NO_FENCE_TOKEN {
        return true;
    }
    let Some(f) = try_is_current else {
        return true;
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(token)));
    match result {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            log::debug!("stream single-writer: is_current check failed; treating as current: {}", e);
            true
        }
        Err(_) => {
            log::debug!("stream single-writer: is_current check failed; treating as current");
            true
        }
    }
}

// Keep underscore-prefixed aliases for 1:1 traceability with Python private names
// (there are no private helpers in the original; these alias the public API for
// line-level traceability).
#[allow(dead_code)]
fn _claim_stream_writer(agent: Option<&dyn StreamWriterFence>) -> i64 {
    claim_stream_writer(agent)
}

#[allow(dead_code)]
fn _stream_writer_is_current(agent: Option<&dyn StreamWriterFence>, token: i64) -> bool {
    stream_writer_is_current(agent, token)
}
