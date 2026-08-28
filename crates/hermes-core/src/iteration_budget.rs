//! Per-agent iteration budget — thread-safe consume/refund counter.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/iteration_budget.py` (62 lines).
//!
//! Each `AIAgent` instance (parent or subagent) holds an [`IterationBudget`];
//! the parent's cap comes from `max_iterations` (default 500), each subagent's
//! cap comes from `delegation.max_iterations` (default 50).
//!
//! `execute_code` (programmatic tool calling) iterations are refunded via
//! [`IterationBudget::refund`] so they don't eat into the budget.
//!
//! Python source docstring (preserved):
//! ```text
//! Per-agent iteration budget — thread-safe consume/refund counter.
//!
//! Extracted from ``run_agent.py``.  Each ``AIAgent`` instance (parent or
//! subagent) holds an :class:`IterationBudget`; the parent's cap comes from
//! ``max_iterations`` (default 500), each subagent's cap comes from
//! ``delegation.max_iterations`` (default 50).
//!
//! ``run_agent`` re-exports ``IterationBudget`` so existing
//! ``from run_agent import IterationBudget`` imports keep working unchanged.
//! ```

use std::sync::Mutex;

// ---------------------------------------------------------------------------
// IterationBudget — mirrors `class IterationBudget` (lines 17-59)
// ---------------------------------------------------------------------------

/// Thread-safe iteration counter for an agent.
///
/// Each agent (parent or subagent) gets its own [`IterationBudget`].
/// The parent's budget is capped at `max_iterations` (default 500).
/// Each subagent gets an independent budget capped at
/// `delegation.max_iterations` (default 50) — this means total
/// iterations across parent + subagents can exceed the parent's cap.
/// Users control the per-subagent limit via `delegation.max_iterations`
/// in config.yaml.
///
/// `execute_code` (programmatic tool calling) iterations are refunded via
/// [`IterationBudget::refund`] so they don't eat into the budget.
///
/// Mirrors `class IterationBudget` (lines 17-59).
pub struct IterationBudget {
    /// Maximum iterations allowed. Mirrors `self.max_total` (line 33).
    pub max_total: usize,
    /// Consumed count, guarded by a mutex. Mirrors `self._used` + `self._lock` (lines 34-35).
    used: Mutex<usize>,
}

impl IterationBudget {
    /// Create a new budget with the given cap.
    /// Mirrors `__init__(self, max_total: int)` (lines 32-35).
    pub fn new(max_total: usize) -> Self {
        Self {
            max_total,
            used: Mutex::new(0),
        }
    }

    /// Try to consume one iteration. Returns `true` if allowed.
    /// Mirrors `consume` (lines 37-43):
    /// ```python
    /// def consume(self) -> bool:
    ///     with self._lock:
    ///         if self._used >= self.max_total:
    ///             return False
    ///         self._used += 1
    ///         return True
    /// ```
    pub fn consume(&self) -> bool {
        let mut guard = self.used.lock().unwrap_or_else(|e| e.into_inner());
        if *guard >= self.max_total {
            return false;
        }
        *guard += 1;
        true
    }

    /// Give back one iteration (e.g. for execute_code turns).
    /// Mirrors `refund` (lines 45-49):
    /// ```python
    /// def refund(self) -> None:
    ///     with self._lock:
    ///         if self._used > 0:
    ///             self._used -= 1
    /// ```
    pub fn refund(&self) {
        let mut guard = self.used.lock().unwrap_or_else(|e| e.into_inner());
        if *guard > 0 {
            *guard -= 1;
        }
    }

    /// Consumed count. Mirrors `used` property (lines 51-54).
    pub fn used(&self) -> usize {
        *self.used.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Remaining iterations, clamped to 0. Mirrors `remaining` property (lines 56-59):
    /// ```python
    /// @property
    /// def remaining(self) -> int:
    ///     with self._lock:
    ///         return max(0, self.max_total - self._used)
    /// ```
    pub fn remaining(&self) -> usize {
        let used = *self.used.lock().unwrap_or_else(|e| e.into_inner());
        self.max_total.saturating_sub(used)
    }
}

impl std::fmt::Debug for IterationBudget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IterationBudget")
            .field("max_total", &self.max_total)
            .field("used", &self.used())
            .field("remaining", &self.remaining())
            .finish()
    }
}

// Keep underscore-prefixed alias for 1:1 traceability if needed (no private helpers to alias).
