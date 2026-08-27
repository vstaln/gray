//! Authoritative project -> repo -> lane -> session tree builder — slice 2 (re-export shim).
//!
//! 1:1 port of `tui_gateway/project_tree.py` (793 lines) — supplemental slice for T0388 retry.
//!
//! Primary implementation lives in [`crate::project_tree`] (T0388, commit `5626d60`).
//! This module exists as the fallback generic target described in the T0403
//! remaining-file picker: `tui_gateway/project_tree` was already queued as T0388,
//! `tui_gateway/ws` and `tui_gateway/render` and `tui_gateway/loop_noise` and
//! `tui_gateway/methods_browser_control` were all done, so the queue fell back to
//! a generic `project_tree2.rs` retry. It re-exports the full public surface so
//! `crate::project_tree2::*` is byte-compatible with `crate::project_tree::*`
//! and any `tui_gateway.project_tree2` shim import resolves.
//!
//! ```python
//! # Python — tui_gateway/project_tree.py (shim slice, re-export)
//! # tui_gateway/project_tree2.py — fallback generic (T0388 retry)
//! from .project_tree import *  # noqa: F401,F403
//! # All symbols: TRUNK_BRANCHES, DEFAULT_BRANCH_LABEL, NO_PROJECT_ID,
//! # NO_PROJECT_LABEL, MAX_SIBLING_PROBES, ResolveResult, segments,
//! # is_windows_path, comparison_segments, path_key, lane_key, base_name,
//! # kanban_worktree_dir, is_path_under, with_base_name, parent_dir,
//! # branch_lane_id, kanban_lane_id, probe_sibling_worktree, place_by_heuristic,
//! # place, session_repo_root, session_time, session_cost, FolderIndex,
//! # project_for_session, build_repos, seed_folder_repos, build_tree, etc.
//! ```
//!
//! # Rust mapping
//! * `tui_gateway/project_tree.py` → [`crate::project_tree`] (primary 1:1 port).
//! * `tui_gateway/project_tree2.py` (fallback) → this module, `pub use crate::project_tree::*`.
//!   No logic is duplicated; `cargo` (never run per task) would see identical symbols
//!   via either path.

pub use crate::project_tree::*;

// Re-exported slice marker — distinguishes the retry artifact in `crate::project_tree2`
// without duplicating logic. Mirrors the Python shim's `__all__` passthrough.
pub const PROJECT_TREE2_SLICE: &str = "T0388-retry";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reexport_parity() {
        // Spot-check that the re-export surface matches the primary module.
        assert_eq!(TRUNK_BRANCHES, crate::project_tree::TRUNK_BRANCHES);
        assert_eq!(DEFAULT_BRANCH_LABEL, crate::project_tree::DEFAULT_BRANCH_LABEL);
        assert_eq!(NO_PROJECT_ID, crate::project_tree::NO_PROJECT_ID);
        assert_eq!(NO_PROJECT_LABEL, crate::project_tree::NO_PROJECT_LABEL);
        assert_eq!(MAX_SIBLING_PROBES, crate::project_tree::MAX_SIBLING_PROBES);
        assert_eq!(PROJECT_TREE2_SLICE, "T0388-retry");
        // path helpers round-trip through re-export
        assert_eq!(path_key("/a/b/"), crate::project_tree::path_key("/a/b/"));
        assert_eq!(lane_key("/a/b::branch::Main"), crate::project_tree::lane_key("/a/b::branch::Main"));
        assert_eq!(base_name("/a/b/c"), "c");
        assert_eq!(kanban_worktree_dir("/repo/.worktrees/t_abc123"), Some("/repo/.worktrees".to_string()));
        assert!(is_path_under("/a/b", "/a/b/c"));
        assert_eq!(branch_lane_id("/repo", "main"), "/repo::branch::main");
        assert_eq!(kanban_lane_id("/repo"), "/repo::kanban");
    }

    #[test]
    fn build_tree_via_reexport() {
        let sessions = vec![Session::new("s1").with_cwd("/repo").with_branch("main").with_repo_root("/repo")];
        let out = build_repos(&sessions, None, true);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].session_count, 1);
        let out2 = build_tree_simple(&[], &sessions, &[], None);
        assert!(out2.projects.iter().any(|p| p.session_count == 1 || p.is_auto || !p.is_no_project));
    }
}
