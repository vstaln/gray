//! Map agent activity → a [`PetState`].
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/pet/state.py` (81 lines).
//!
//! This is the one place the "what is the agent doing right now?" → "which
//! animation row?" decision lives. Each surface feeds it the signals it already
//! tracks:
//!
//! - CLI    — `KawaiiSpinner` waiting/thinking state + tool outcomes.
//! - TUI    — gateway `tool.start/complete` + `message.delta/complete` events.
//! - Desktop — the `$busy`/`$awaitingResponse`/tool-event nanostores
//!             (re-implemented in TS, but mirroring this priority order).
//!
//! Keeping the priority order here (and documenting it) lets the TypeScript
//! mirror stay faithful without a second design.
//!
//! Python source docstring (preserved):
//! ```text
//! Map agent activity → a :class:`PetState`.
//!
//! This is the one place the "what is the agent doing right now?" → "which
//! animation row?" decision lives.  Each surface feeds it the signals it already
//! tracks:
//!
//! - CLI    — ``KawaiiSpinner`` waiting/thinking state + tool outcomes.
//! - TUI    — gateway ``tool.start/complete`` + ``message.delta/complete`` events.
//! - Desktop — the ``$busy``/``$awaitingResponse``/tool-event nanostores
//!             (re-implemented in TS, but mirroring this priority order).
//!
//! Keeping the priority order here (and documenting it) lets the TypeScript
//! mirror stay faithful without a second design.
//! ```

// ---------------------------------------------------------------------------
// PetState — mirrors `from agent.pet.constants import PetState` (line 21)
// ---------------------------------------------------------------------------
// Copied from `agent/pet/constants.py` lines 78-92 so this module is
// self-contained; the constants file is not ported as a separate crate
// module yet. Values are the Hermes activity state names (`idle`/`wave`/
// `run`/…); aliases (`waving`/`jumping`/…) are resolved at the renderer,
// not here.

/// Animation state a pet can be shown in.
///
/// Mirrors `class PetState(str, Enum)` in `agent/pet/constants.py` (lines 78-92).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PetState {
    /// Idle — default, no activity.
    Idle,
    /// Wave — turn finished cleanly / greeting.
    Wave,
    /// Run — tool executing or turn in flight.
    Run,
    /// Failed — a tool/turn just failed.
    Failed,
    /// Review — model is thinking / reading.
    Review,
    /// Jump — explicit success beat (todos done).
    Jump,
    /// Waiting — blocked on user input.
    Waiting,
}

impl PetState {
    /// Canonical string value — mirrors `PetState.*.value` in Python.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Wave => "wave",
            Self::Run => "run",
            Self::Failed => "failed",
            Self::Review => "review",
            Self::Jump => "jump",
            Self::Waiting => "waiting",
        }
    }

    /// Parse from canonical string — inverse of [`Self::as_str`].
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "idle" => Some(Self::Idle),
            "wave" => Some(Self::Wave),
            "run" => Some(Self::Run),
            "failed" => Some(Self::Failed),
            "review" => Some(Self::Review),
            "jump" => Some(Self::Jump),
            "waiting" => Some(Self::Waiting),
            _ => None,
        }
    }
}

impl std::fmt::Display for PetState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for PetState {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

// ---------------------------------------------------------------------------
// todos_all_done — mirrors lines 24-38
// ---------------------------------------------------------------------------

/// Minimal todo item — covers both Python shapes:
///
/// - `dict` with `{"status": ...}` — `t.get("status")` (line 36)
/// - object with `.status` attr — `getattr(t, "status", None)` (line 36)
///
/// In Rust both collapse to an optional status string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Todo {
    /// Todo status, e.g. `"completed"`, `"cancelled"`, `"pending"`, `"in_progress"`.
    pub status: Option<String>,
}

impl Todo {
    /// Create a todo with optional status.
    pub fn new(status: Option<&str>) -> Self {
        Self {
            status: status.map(|s| s.to_string()),
        }
    }

    /// Convenience for a todo with no status.
    pub fn without_status() -> Self {
        Self { status: None }
    }

    /// Mirrors `_status(t)` helper (lines 35-36) — returns status str if present.
    pub fn status_str(&self) -> Option<&str> {
        self.status.as_deref()
    }
}

/// Trait for anything that can provide a todo status — enables generic `todos_all_done`.
pub trait HasStatus {
    fn status(&self) -> Option<&str>;
}

impl HasStatus for Todo {
    fn status(&self) -> Option<&str> {
        self.status_str()
    }
}

impl HasStatus for Option<String> {
    fn status(&self) -> Option<&str> {
        self.as_deref()
    }
}

// Keep underscore-prefixed alias for 1:1 traceability with Python private name.
#[allow(dead_code)]
fn _status<T: HasStatus>(t: &T) -> Option<&str> {
    t.status()
}

/// True iff there's ≥1 todo and every one is completed/cancelled.
///
/// Mirrors `todos_all_done` (lines 24-38):
/// ```python
/// def todos_all_done(todos: Iterable[Any] | None) -> bool:
///     items = list(todos or [])
///     if not items:
///         return False
///     def _status(t: Any) -> Any:
///         return t.get("status") if isinstance(t, dict) else getattr(t, "status", None)
///     return all(_status(t) in ("completed", "cancelled") for t in items)
/// ```
///
/// Accepts `None` (null) or a slice; empty slice returns `false`.
pub fn todos_all_done(todos: Option<&[Todo]>) -> bool {
    let items = match todos {
        None => return false,
        Some(v) => v,
    };
    if items.is_empty() {
        return false;
    }
    items.iter().all(|t| {
        matches!(t.status_str(), Some("completed") | Some("cancelled"))
    })
}

/// Generic variant for any `HasStatus` slice — mirrors `Iterable[Any]` flexibility.
pub fn todos_all_done_generic<T: HasStatus>(todos: Option<&[T]>) -> bool {
    let items = match todos {
        None => return false,
        Some(v) => v,
    };
    if items.is_empty() {
        return false;
    }
    items
        .iter()
        .all(|t| matches!(t.status(), Some("completed") | Some("cancelled")))
}

// ---------------------------------------------------------------------------
// derive_pet_state — mirrors lines 41-81
// ---------------------------------------------------------------------------

/// Coarse activity signals that resolve to a [`PetState`].
///
/// Mirrors `derive_pet_state` kwargs (lines 42-50):
/// ```python
/// def derive_pet_state(
///     *,
///     busy: bool = False,
///     awaiting_input: bool = False,
///     error: bool = False,
///     celebrate: bool = False,
///     just_completed: bool = False,
///     tool_running: bool = False,
///     reasoning: bool = False,
/// ) -> PetState:
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DerivePetStateParams {
    /// Turn in flight, unspecified work — maps to `RUN` if no stronger signal.
    /// Mirrors `busy: bool = False` (line 43).
    pub busy: bool,
    /// Blocked on user (clarify/approval prompt open).
    /// Mirrors `awaiting_input: bool = False` (line 44).
    pub awaiting_input: bool,
    /// A tool/turn just failed.
    /// Mirrors `error: bool = False` (line 45).
    pub error: bool,
    /// Explicit success beat (e.g. todos done).
    /// Mirrors `celebrate: bool = False` (line 46).
    pub celebrate: bool,
    /// Turn finished cleanly / greeting.
    /// Mirrors `just_completed: bool = False` (line 47).
    pub just_completed: bool,
    /// A tool is executing.
    /// Mirrors `tool_running: bool = False` (line 48).
    pub tool_running: bool,
    /// Model is thinking / reading.
    /// Mirrors `reasoning: bool = False` (line 49).
    pub reasoning: bool,
}

/// Resolve the animation state from coarse activity signals.
///
/// Priority (highest first) — only one row can show at a time, so the most
/// salient signal wins (lines 51-66):
///
/// 1. `error`          → `FAILED`
/// 2. `celebrate`      → `JUMP`
/// 3. `just_completed` → `WAVE`
/// 4. `awaiting_input` → `WAITING` (outranks in-flight signals — turn paused on you)
/// 5. `tool_running`   → `RUN`
/// 6. `reasoning`      → `REVIEW`
/// 7. `busy`           → `RUN`
/// 8. otherwise        → `IDLE`
///
/// Mirrors `derive_pet_state` (lines 41-81).
pub fn derive_pet_state(params: DerivePetStateParams) -> PetState {
    if params.error {
        return PetState::Failed;
    }
    if params.celebrate {
        return PetState::Jump;
    }
    if params.just_completed {
        return PetState::Wave;
    }
    if params.awaiting_input {
        return PetState::Waiting;
    }
    if params.tool_running {
        return PetState::Run;
    }
    if params.reasoning {
        return PetState::Review;
    }
    if params.busy {
        return PetState::Run;
    }
    PetState::Idle
}

/// Convenience: `derive_pet_state` with explicit bool args — mirrors the Python
/// keyword call `derive_pet_state(busy=..., awaiting_input=..., ...)`.
#[allow(clippy::too_many_arguments)]
pub fn derive_pet_state_from(
    busy: bool,
    awaiting_input: bool,
    error: bool,
    celebrate: bool,
    just_completed: bool,
    tool_running: bool,
    reasoning: bool,
) -> PetState {
    derive_pet_state(DerivePetStateParams {
        busy,
        awaiting_input,
        error,
        celebrate,
        just_completed,
        tool_running,
        reasoning,
    })
}
