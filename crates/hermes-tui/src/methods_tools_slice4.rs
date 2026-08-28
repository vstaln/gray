//! Tools & system / slash.exec / insights / rollback / browser-plugins-cron-skills JSON-RPC handlers — slice 4 (remainder beyond 2579).
//!
//! 1:1 port of `tui_gateway/methods_tools.py` remainder after line 2579 (T0384 slice 4/2579).
//! The source file is exactly 2579 lines; slices 1 (1-900), 2 (900-1800) and 3 (1800-2579)
//! cover 100% of `@method` handlers and `register(server)`. This module closes
//! T0384 — no new `@method` decorators exist beyond EOF.
//!
//! ```python
//! # Python — tui_gateway/methods_tools.py 2579/2579 (EOF)
//! # (no remaining handlers beyond shell.exec at 2532 + register at 2577)
//! def register(server) -> None:
//!     """Bind this module's handlers onto ``server``'s globals and registry."""
//!     _registry.install(server)
//! # EOF — slices 1-3 already ported all handlers + register; slice4 is empty remainder.
//! ```
//!
//! # Rust mapping
//! * No new `METHOD_*` / `ERR_*` / `handle_*` beyond slice 3 (`shell.exec` at 2532).
//! * This file provides [`COVERAGE_NOTE`] + [`SLICE_LINES`] for traceability and
//!   a unified aggregator [`register_all`] / [`build_registry_default`] that wires
//!   slices 1-3 onto one [`crate::method_ctx::HandlerRegistry`] (ponytail: one call
//!   instead of three at call-site; unwrap when caller needs full surface).
//! * Raw `register` is no-op (no handlers beyond EOF); use `register_all` for the
//!   complete 40-method surface (slice1:10 + slice2:15 + slice3:15).
//! * `_ok(rid, result)` / `_err(rid, code, msg)` → already in slices 1-3
//!   (`ok_response` / `err_response`); not duplicated here.
//! * `@method("...")` + `register(server)` → [`register_all`] / [`build_registry_default`].

use crate::method_ctx::HandlerRegistry;

/// Slice window for this file — EOF remainder (no handlers).
pub const SLICE_LINES: &str = "2579-2579 (EOF — no handlers beyond shell.exec/register)";

/// Human-readable coverage note for T0384.
pub const COVERAGE_NOTE: &str =
    "methods_tools.py 1-2579 fully covered by slices 1-3 (1:1-900, 2:900-1800, 3:1800-2579); slice4 is empty remainder";

// ---------------------------------------------------------------------------
// Unified aggregator — wires the complete 40-method surface
// ---------------------------------------------------------------------------

/// Register the complete `methods_tools` surface (slices 1-3) onto `registry`.
///
/// Slice1: 10 methods (`system.battery` … `command.dispatch`)
/// Slice2: 15 methods (`slash.exec` … `learning.frames`)
/// Slice3: 15 methods (`learning.detail` … `shell.exec`)
/// Slice4: 0 new methods (EOF).
pub fn register_all(registry: &mut HandlerRegistry) {
    crate::methods_tools_slice1::register(registry);
    crate::methods_tools_slice2::register(registry);
    crate::methods_tools_slice3::register(registry);
}

/// Build a fresh [`HandlerRegistry`] with all 40 `methods_tools` methods.
pub fn build_registry_default() -> HandlerRegistry {
    let mut reg = HandlerRegistry::new();
    register_all(&mut reg);
    reg
}

/// No-op `register` for this slice — no new handlers beyond 2579.
///
/// Exists so `pub mod methods_tools_slice4` satisfies the `register(server)`
/// shape uniformly across slices; full wiring is via [`register_all`].
pub fn register(_registry: &mut HandlerRegistry) {}
