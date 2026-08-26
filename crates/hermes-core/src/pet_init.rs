//! Petdex pet engine — shared core for the CLI, TUI, and desktop surfaces.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/pet/__init__.py` (51 lines).
//!
//! This package is the **single source of truth** for the pet feature so the
//! base CLI (Python) and TUI (Ink, via `tui_gateway`) never duplicate the hard
//! parts:
//!
//! - `agent.pet.constants` — frame geometry + the `PetState` enum.
//! - `agent.pet.state`     — map agent activity → a `PetState`.
//! - `agent.pet.manifest`  — fetch the public petdex manifest.
//! - `agent.pet.store`     — install / list / resolve pets on disk
//!                           (profile-aware via `get_hermes_home()`).
//! - `agent.pet.render`    — decode a spritesheet and encode frames for a
//!                           terminal (kitty / iTerm2 / sixel graphics
//!                           protocols, with a Unicode half-block fallback).
//!
//! Rendering in the Electron desktop is necessarily TypeScript (canvas), but it
//! reuses the same on-disk store and the same state semantics.
//!
//! The whole feature is a *display* concern: it adds no model tool, mutates no
//! system prompt or toolset, and therefore has zero effect on prompt caching.
//!
//! Python source docstring (preserved verbatim):
//! ```text
//! Petdex pet engine — shared core for the CLI, TUI, and desktop surfaces.
//!
//! Petdex (https://github.com/crafter-station/petdex) is a public gallery of
//! animated sprite "pets" for coding agents.  Each pet is a ``pet.json`` plus a
//! ``spritesheet.{webp,png}`` of 192×208 px cells. Current Codex/petdex sheets use
//! an 8-column × 9-row atlas; older Hermes/petdex sheets used an 8-row atlas.
//! Hermes infers the row taxonomy from the sheet and maps agent activity onto
//! idle/run/review/failed/wave/jump.
//!
//! This package is the **single source of truth** for the feature so the base
//! CLI (Python) and TUI (Ink, via ``tui_gateway``) never duplicate the hard
//! parts:
//!
//! - :mod:`agent.pet.constants` — frame geometry + the :class:`PetState` enum.
//! - :mod:`agent.pet.state`     — map agent activity → a :class:`PetState`.
//! - :mod:`agent.pet.manifest`  — fetch the public petdex manifest.
//! - :mod:`agent.pet.store`     — install / list / resolve pets on disk
//!                                (profile-aware via ``get_hermes_home()``).
//! - :mod:`agent.pet.render`    — decode a spritesheet and encode frames for a
//!                                terminal (kitty / iTerm2 / sixel graphics
//!                                protocols, with a Unicode half-block
//!                                fallback).
//!
//! Rendering in the Electron desktop is necessarily TypeScript (canvas), but it
//! reuses the same on-disk store and the same state semantics.
//!
//! The whole feature is a *display* concern: it adds no model tool, mutates no
//! system prompt or toolset, and therefore has zero effect on prompt caching.
//! ```
//!
//! Python re-exports (preserved verbatim):
//! ```python
//! from agent.pet.constants import (
//!     DEFAULT_SCALE,
//!     FRAME_H,
//!     FRAME_W,
//!     FRAMES_PER_STATE,
//!     LOOP_MS,
//!     STATE_ROWS,
//!     PetState,
//! )
//! from agent.pet.state import derive_pet_state
//!
//! __all__ = [
//!     "DEFAULT_SCALE",
//!     "FRAME_H",
//!     "FRAME_W",
//!     "FRAMES_PER_STATE",
//!     "LOOP_MS",
//!     "STATE_ROWS",
//!     "PetState",
//!     "derive_pet_state",
//! ]
//! ```
//!
//! Rust notes:
//! - `agent.pet.constants` geometry values are mirrored here as `pub const`
//!   so this crate surface matches Python `__all__` without requiring a
//!   separate `pet_constants` crate module yet. When `crate::pet_constants`
//!   lands, wire `pub use crate::pet_constants::{FRAME_W, ...}`.
//! - `PetState` and `derive_pet_state` are re-exported from `crate::pet_state`
//!   (1:1 port of `agent/pet/constants.py` + `agent/pet/state.py`).
//!   Until a dedicated `crate::pet_constants` exists, `pet_state::PetState`
//!   is the canonical type.
//!   ponytail: stub re-exports until pet_constants / manifest / store / render land; wire pub use when modules land.
//!
//! Mapping:
//! - `from agent.pet.constants import DEFAULT_SCALE` → [`DEFAULT_SCALE`]
//! - `from agent.pet.constants import FRAME_H` → [`FRAME_H`]
//! - `from agent.pet.constants import FRAME_W` → [`FRAME_W`]
//! - `from agent.pet.constants import FRAMES_PER_STATE` → [`FRAMES_PER_STATE`]
//! - `from agent.pet.constants import LOOP_MS` → [`LOOP_MS`]
//! - `from agent.pet.constants import STATE_ROWS` → [`STATE_ROWS`]
//! - `from agent.pet.constants import PetState` → [`PetState`]
//! - `from agent.pet.state import derive_pet_state` → [`derive_pet_state`]
//! - `__all__` → [`ALL`] / [`__ALL__`]

// ---------------------------------------------------------------------------
// Frame geometry — mirrors `agent/pet/constants.py` lines 17-26
// ---------------------------------------------------------------------------

/// Frame width in pixels.
/// Mirrors `FRAME_W = 192` (line 17) in `agent/pet/constants.py`.
pub const FRAME_W: u32 = 192;

/// Frame height in pixels.
/// Mirrors `FRAME_H = 208` (line 18) in `agent/pet/constants.py`.
pub const FRAME_H: u32 = 208;

/// Frames consumed per animation state.
/// Mirrors `FRAMES_PER_STATE = 6` (line 23) in `agent/pet/constants.py`.
pub const FRAMES_PER_STATE: u32 = 6;

/// Full-loop duration for one state, milliseconds.
/// Mirrors `LOOP_MS = 1100` (line 26) in `agent/pet/constants.py`.
pub const LOOP_MS: u32 = 1100;

/// Default on-screen scale relative to native frame size.
/// Mirrors `DEFAULT_SCALE = 0.33` (line 36) in `agent/pet/constants.py`.
pub const DEFAULT_SCALE: f64 = 0.33;

// ---------------------------------------------------------------------------
// Additional constants from `agent/pet/constants.py` (not in `__all__`
// but preserved for 1:1 discoverability / future `pub use` wiring)
// ---------------------------------------------------------------------------

/// User-settable scale floor.
/// Mirrors `MIN_SCALE = 0.1` (line 41) in `agent/pet/constants.py`.
pub const MIN_SCALE: f64 = 0.1;

/// User-settable scale ceiling.
/// Mirrors `MAX_SCALE = 3.0` (line 42) in `agent/pet/constants.py`.
pub const MAX_SCALE: f64 = 3.0;

/// Terminal cells one native frame spans at `scale == 1.0`.
/// Mirrors `BASE_UNICODE_COLS = FRAME_W // 8` (line 52) in `agent/pet/constants.py`.
pub const BASE_UNICODE_COLS: u32 = FRAME_W / 8;

/// Legibility floor for the half-block fallback.
/// Mirrors `UNICODE_MIN_COLS = 16` (line 60) in `agent/pet/constants.py`.
pub const UNICODE_MIN_COLS: u32 = 16;

// ---------------------------------------------------------------------------
// Row taxonomy — mirrors `agent/pet/constants.py` lines 95-124
// ---------------------------------------------------------------------------

/// Legacy Hermes/petdex row order (top -> bottom) for the older 8-row atlas.
/// Mirrors `LEGACY_STATE_ROWS` (lines 97-106) in `agent/pet/constants.py`.
pub const LEGACY_STATE_ROWS: &[&str] = &[
    "idle", "wave", "run", "failed", "review", "jump", "extra1", "extra2",
];

/// Current Petdex row order (top -> bottom) for 1536×1872 atlases (8×9).
/// Mirrors `CODEX_STATE_ROWS` (lines 110-120) in `agent/pet/constants.py`.
pub const CODEX_STATE_ROWS: &[&str] = &[
    "idle",
    "running-right",
    "running-left",
    "waving",
    "jumping",
    "failed",
    "waiting",
    "running",
    "review",
];

/// Default/fallback for callers without a sheet. Prefer the current 9-row Codex format.
/// Mirrors `STATE_ROWS: list[str] = CODEX_STATE_ROWS` (line 124) in `agent/pet/constants.py`.
pub const STATE_ROWS: &[&str] = CODEX_STATE_ROWS;

// ---------------------------------------------------------------------------
// Re-exports — mirrors `from agent.pet.constants import PetState`
//              and `from agent.pet.state import derive_pet_state`
// ---------------------------------------------------------------------------

/// Animation state a pet can be shown in.
///
/// Mirrors `class PetState(str, Enum)` in `agent/pet/constants.py` lines 78-92.
/// Re-exported from `crate::pet_state` (canonical port of `agent/pet/state.py`).
pub use crate::pet_state::PetState;

/// Coarse activity signals that resolve to a [`PetState`].
/// Re-exported from `crate::pet_state`.
pub use crate::pet_state::DerivePetStateParams;

/// Resolve the animation state from coarse activity signals.
///
/// Priority mirrors `derive_pet_state` in `agent/pet/state.py` lines 41-81.
/// Re-exported from `crate::pet_state`.
pub use crate::pet_state::derive_pet_state;

/// Convenience helper mirroring Python keyword call `derive_pet_state(...)`.
pub use crate::pet_state::derive_pet_state_from;

// ---------------------------------------------------------------------------
// Public surface — mirrors `__all__` (8 entries)
// ---------------------------------------------------------------------------

/// Unified public surface, mirroring Python `__all__` (lines 42-51).
///
/// ```python
/// __all__ = [
///     "DEFAULT_SCALE",
///     "FRAME_H",
///     "FRAME_W",
///     "FRAMES_PER_STATE",
///     "LOOP_MS",
///     "STATE_ROWS",
///     "PetState",
///     "derive_pet_state",
/// ]
/// ```
pub const ALL: &[&str] = &[
    "DEFAULT_SCALE",
    "FRAME_H",
    "FRAME_W",
    "FRAMES_PER_STATE",
    "LOOP_MS",
    "STATE_ROWS",
    "PetState",
    "derive_pet_state",
];

/// Alias matching Python `__all__` name for grep discoverability.
pub const __ALL__: &[&str] = ALL;

// Re-exports (future):
// Once `crate::pet_constants`, `crate::pet_manifest`, `crate::pet_store`,
// `crate::pet_render` are ported, wire:
//   pub use crate::pet_constants::{DEFAULT_SCALE, FRAME_H, FRAME_W, FRAMES_PER_STATE, LOOP_MS, STATE_ROWS, PetState, MIN_SCALE, MAX_SCALE, BASE_UNICODE_COLS, UNICODE_MIN_COLS, LEGACY_STATE_ROWS, CODEX_STATE_ROWS, ...};
//   pub use crate::pet_manifest::{...};
//   pub use crate::pet_store::{...};
//   pub use crate::pet_render::{...};
// Until then this module documents the unified public surface and exposes
// `ALL` + geometry constants + re-exports from `crate::pet_state`.

// ---------------------------------------------------------------------------
// Helpers mirroring `agent/pet/constants.py` pure functions (optional,
// not in `__all__` but preserved for 1:1 completeness)
// ---------------------------------------------------------------------------

/// Clamp `scale` to `[MIN_SCALE, MAX_SCALE]`.
/// Mirrors `clamp_scale` (lines 45-47) in `agent/pet/constants.py`.
pub fn clamp_scale(scale: f64) -> f64 {
    scale.max(MIN_SCALE).min(MAX_SCALE)
}

/// Half-block width implied by `scale`, clamped to the legibility floor.
/// Mirrors `cols_for_scale` (lines 63-70) in `agent/pet/constants.py`.
pub fn cols_for_scale(scale: f64) -> u32 {
    let s = if scale == 0.0 { DEFAULT_SCALE } else { scale };
    let cols = (BASE_UNICODE_COLS as f64 * s).round() as i64;
    let floored = cols.max(UNICODE_MIN_COLS as i64);
    floored as u32
}

/// Resolve terminal width: explicit `unicode_cols` override, else from `scale`.
/// Mirrors `resolve_cols` (lines 73-75) in `agent/pet/constants.py`.
pub fn resolve_cols(scale: f64, unicode_cols: u32) -> u32 {
    if unicode_cols > 0 {
        unicode_cols
    } else {
        cols_for_scale(scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_matches_python() {
        assert_eq!(
            ALL,
            [
                "DEFAULT_SCALE",
                "FRAME_H",
                "FRAME_W",
                "FRAMES_PER_STATE",
                "LOOP_MS",
                "STATE_ROWS",
                "PetState",
                "derive_pet_state",
            ]
        );
        assert_eq!(__ALL__, ALL);
        assert_eq!(ALL.len(), 8);
    }

    #[test]
    fn geometry_matches_constants_py() {
        assert_eq!(FRAME_W, 192);
        assert_eq!(FRAME_H, 208);
        assert_eq!(FRAMES_PER_STATE, 6);
        assert_eq!(LOOP_MS, 1100);
        assert!((DEFAULT_SCALE - 0.33).abs() < f64::EPSILON);
        assert_eq!(MIN_SCALE, 0.1);
        assert_eq!(MAX_SCALE, 3.0);
        assert_eq!(BASE_UNICODE_COLS, 24);
        assert_eq!(UNICODE_MIN_COLS, 16);
    }

    #[test]
    fn state_rows_match_codex() {
        assert_eq!(
            STATE_ROWS,
            [
                "idle",
                "running-right",
                "running-left",
                "waving",
                "jumping",
                "failed",
                "waiting",
                "running",
                "review"
            ]
        );
        assert_eq!(CODEX_STATE_ROWS, STATE_ROWS);
        assert_eq!(
            LEGACY_STATE_ROWS,
            ["idle", "wave", "run", "failed", "review", "jump", "extra1", "extra2"]
        );
    }

    #[test]
    fn reexports_work() {
        // PetState re-export resolves
        let s = PetState::Idle;
        assert_eq!(s.as_str(), "idle");
        // derive_pet_state re-export resolves
        let st = derive_pet_state(DerivePetStateParams {
            busy: true,
            ..Default::default()
        });
        assert_eq!(st, PetState::Run);
        let st2 = derive_pet_state_from(false, false, true, false, false, false, false);
        assert_eq!(st2, PetState::Failed);
    }

    #[test]
    fn helpers_match_constants_py() {
        assert!((clamp_scale(0.05) - MIN_SCALE).abs() < f64::EPSILON);
        assert!((clamp_scale(5.0) - MAX_SCALE).abs() < f64::EPSILON);
        assert!((clamp_scale(0.5) - 0.5).abs() < f64::EPSILON);
        // cols_for_scale at default scale should hit floor
        assert_eq!(cols_for_scale(DEFAULT_SCALE), UNICODE_MIN_COLS);
        // cols_for_scale at scale 1.0 should be BASE_UNICODE_COLS
        assert_eq!(cols_for_scale(1.0), BASE_UNICODE_COLS);
        // resolve_cols respects override
        assert_eq!(resolve_cols(0.33, 40), 40);
        assert_eq!(resolve_cols(0.33, 0), cols_for_scale(0.33));
    }
}
