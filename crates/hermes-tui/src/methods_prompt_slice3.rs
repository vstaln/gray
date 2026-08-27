//! Prompt / attachment / respond JSON-RPC handlers — slice 3 (lines 1626-1626 / unified).
//!
//! 1:1 port of `tui_gateway/methods_prompt.py` lines 1626–1626 (T0385 slice 3/1626 — empty tail + aggregator).
//!
//! Slices 1 (1–900) and 2 (900–1626) already cover the full 1626-line module.
//! Python's `methods_prompt.py` ends at `def register(server): ...` (1597-1626),
//! which slice 2 already documents and ports. This slice introduces no new
//! `@method` handlers; it exists to close `T0385` as a 3-slice set and to
//! provide the unified re-export / combined registry so callers can
//! `crate::methods_prompt_slice3::register(&mut reg)` once instead of
//! wiring both slices manually.
//!
//! ```python
//! # Python — tui_gateway/methods_prompt.py 1597-1626 (abridged, already ported in slice 2)
//! def register(server) -> None:
//!     """Bind this module's handlers onto ``server``'s globals and registry."""
//!     _registry.install(server)
//!     g = vars(server)
//!     for helper in (
//!         _history_user_indices, _message_row_id, _mem_db_pair_agrees,
//!         _find_user_turn_by_row_id, _load_durable_truncation_history,
//!         _resolve_truncate_row_id, _coerce_truncate_int,
//!         _reconcile_client_ordinal, _pending_reaction_notes,
//!         _approval_respond_session_fallback,
//!     ):
//!         setattr(server, helper.__name__, types.FunctionType(helper.__code__, g, ...))
//! ```
//!
//! # Rust mapping
//! * `methods_prompt.py` 1-1626 → [`crate::methods_prompt_slice1`] (1-900) +
//!   [`crate::methods_prompt_slice2`] (900-1626). No lines remain beyond 1626.
//! * This slice's `register` / `register_combined` / `build_combined_registry`
//!   mirror the single Python `register(server)` that installs one `_registry`.
//!   Here the one registry is split across two Rust registries; the combined
//!   helpers install both into the same [`crate::method_ctx::HandlerRegistry`]
//!   so the full 22-method set is available via one call.
//! * `is_truthy_value` / `_ok` / `_err` are re-exported by slices 1-2 and not
//!   duplicated here.
//! * `HandlerRegistry` → [`crate::method_ctx::HandlerRegistry`] (same deferred
//!   `@method` + `install` shape).
//! * `@method("...")` + `register(server)` → [`register`] / [`register_combined`]
//!   / [`build_combined_registry`] / [`build_registry_default`] (aggregated).
//!

use crate::method_ctx::HandlerRegistry;

// ---------------------------------------------------------------------------
// Coverage note — mirrors file split
// ---------------------------------------------------------------------------

/// Human-readable coverage note — `T0385` is slices 1-3 covering 1-1626.
pub const T0385_COVERAGE: &str = "1-1626 via slice1(1-900)+slice2(900-1626)+slice3(1626-1626 empty tail)";

/// This slice's own line range (empty tail — no new handlers).
pub const SLICE3_LINES: &str = "1626-1626 (empty tail; unified aggregator)";

/// Total `@method` count aggregated by this slice's combined registry.
///
/// slice 1: 3 (`prompt.submit`, `clipboard.paste`, `image.attach`)
/// slice 2: 19 (`image.attach_bytes`, `pdf.attach`, `file.attach`, `image.detach`,
/// `input.detect_drop`, `prompt.background`, `preview.restart`, 9× `*.respond`,
/// `approval.pending`, `approval.received`, `approval.respond`)
/// total: 22
pub const COMBINED_METHOD_COUNT: usize = 22;

// ---------------------------------------------------------------------------
// Combined registry wiring — aggregates slice 1 + slice 2
// ---------------------------------------------------------------------------

/// Install the full `methods_prompt` method set (slices 1+2) onto `registry`.
///
/// Mirrors the single Python `register(server)` that installs one `_registry`.
/// Here that registry is split across two Rust modules, so we install both
/// into the same [`HandlerRegistry`].
pub fn register_combined(registry: &mut HandlerRegistry) {
    crate::methods_prompt_slice1::register(registry);
    crate::methods_prompt_slice2::register(registry);
}

/// Alias for [`register_combined`] — `register(&mut reg)` mirrors Python `register(server)`.
pub fn register(registry: &mut HandlerRegistry) {
    register_combined(registry);
}

/// Build a fresh [`HandlerRegistry`] with the full 22-method set.
pub fn build_combined_registry() -> HandlerRegistry {
    let mut reg = HandlerRegistry::new();
    register_combined(&mut reg);
    reg
}

/// Alias that matches the `build_registry_default` naming in slices 1-2.
pub fn build_registry_default() -> HandlerRegistry {
    build_combined_registry()
}

// ---------------------------------------------------------------------------
// Tests — aggregation only (std-only, no I/O)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice3_is_empty_tail_note() {
        assert!(SLICE3_LINES.contains("empty"), "{}", SLICE3_LINES);
        assert!(T0385_COVERAGE.contains("1-1626"), "{}", T0385_COVERAGE);
        assert_eq!(COMBINED_METHOD_COUNT, 22);
    }

    #[test]
    fn combined_registry_has_all_methods() {
        let reg = build_combined_registry();
        assert_eq!(reg.len(), COMBINED_METHOD_COUNT, "combined len mismatch");
        let mut names: Vec<_> = reg.pending_names().collect();
        names.sort();
        // spot-check cross-slice
        assert!(names.contains(&"prompt.submit"), "{:?}", names);
        assert!(names.contains(&"clipboard.paste"), "{:?}", names);
        assert!(names.contains(&"image.attach"), "{:?}", names);
        assert!(names.contains(&"image.attach_bytes"), "{:?}", names);
        assert!(names.contains(&"approval.respond"), "{:?}", names);
        assert!(names.contains(&"clarify.respond"), "{:?}", names);
    }

    #[test]
    fn register_aggregates_both_slices() {
        let mut reg = HandlerRegistry::new();
        register(&mut reg);
        assert_eq!(reg.len(), 22);
        let mut m = std::collections::HashMap::new();
        reg.install_into(&mut m);
        assert_eq!(m.len(), 22);
        // 4015 still enforced by slice1's image.attach handler
        let out = m.get("image.attach").unwrap()("1".to_string(), "{}".to_string());
        assert!(out.contains("4015"), "{}", out);
        // 4015 still enforced by slice2's image.attach_bytes
        let out2 = m.get("image.attach_bytes").unwrap()("1".to_string(), "{}".to_string());
        assert!(out2.contains("4015"), "{}", out2);
    }

    #[test]
    fn combined_build_default_alias_equivalent() {
        let a = build_combined_registry();
        let b = build_registry_default();
        assert_eq!(a.len(), b.len());
    }
}
