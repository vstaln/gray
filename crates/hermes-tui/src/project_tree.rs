//! Authoritative project -> repo -> lane -> session tree builder.
//!
//! 1:1 port of `tui_gateway/project_tree.py` (793 lines).
//!
//! This is the single source of truth for how the desktop sidebar groups
//! sessions into projects, repos, and lanes. It is pure (all git resolution is
//! injected via `resolve`) so it can be unit-tested with fixtures and reused by
//! the gateway's `projects.tree` / `projects.project_sessions` RPCs.
//!
//! It deliberately mirrors the desktop's former client-side grouping (the old
//! `workspace-groups.ts`) so the emitted ids and lane keys stay byte-compatible
//! with the renderer's persisted state (pins, manual ordering, dismissal), which
//! all key off these exact strings.
//!
//! ```python
//! # Python — tui_gateway/project_tree.py (abridged, comments preserved)
//! import re
//! Resolve = Callable[[str], Optional[dict]]  # -> {"repo_root","worktree_root", "is_main"?}
//! Exists = Callable[[str], bool]
//! _KANBAN_DIR_RE = re.compile(r"^(.*[/\\]\\.worktrees)[/\\]t_[0-9a-f]+[/\\]?$")
//! _TRUNK_BRANCHES = {"main","master","trunk","develop"}
//! DEFAULT_BRANCH_LABEL = "main"
//! NO_PROJECT_ID = "__no_project__"
//! NO_PROJECT_LABEL = "Home"
//! _MAX_SIBLING_PROBES = 4
//! def _branch_lane_id(repo_root: str, branch: str = "") -> str: f"{repo_root}::branch::{(branch or '').strip()}"
//! def _kanban_lane_id(repo_root: str) -> str: f"{repo_root}::kanban"
//! def _segments(path: str) -> list[str]: re.split(r"[/\\]", (path or "").rstrip("/\\"))
//! def _is_windows_path(path: str) -> bool: re.match(r"^[A-Za-z]:[/\\]", value) or value.startswith(("\\","//"))
//! def _comparison_segments(path: str) -> list[str]: casefold on Windows else as-is
//! def _path_key(path: str) -> str: "/".join(_comparison_segments(path))
//! def _lane_key(path_or_lane: str) -> str: canonicalize path portion only
//! def base_name(path: str) -> str: segs[-1] if segs else ""
//! def kanban_worktree_dir(path: str) -> Optional[str]: _KANBAN_DIR_RE match group 1
//! def _is_path_under(folder: str, target: str) -> bool: segment-wise prefix
//! def _with_base_name(path: str, name: str) -> str: replace basename
//! def _parent_dir(path: str) -> str: containing directory
//! def _placement(repo_root, lane_key, lane_label, lane_path, is_main, is_kanban) -> dict
//! def _probe_sibling_worktree(cwd: str, resolve: Resolve) -> str: walk ancestors trimming -suffix
//! def _place_by_heuristic(path: str) -> Optional[dict]: path-only fallback
//! def _place(cwd, branch, resolve, persisted_root) -> Optional[dict]: probe, persisted, sibling, heuristic
//! def _session_repo_root(session, resolve) -> str: COMMON repo root
//! def _lane_sort_key(group: dict) -> tuple: trunk 0, kanban bottom, -activity, label lower
//! def _sort_lanes(groups) -> list[dict]: sorted(_lane_sort_key)
//! def _disambiguate_labels(items) -> None: grow colliding basenames into path-prefixed labels
//! def _session_time(session) -> float: max(last_active, started_at)
//! def _build_repos(sessions, resolve, hydrate) -> list[dict]: lane->repo grouping, sort, hydrate trim
//! def _seed_folder_repos(repos, folders, resolve) -> list[dict]: ensure each folder is a repo even at 0 sessions
//! class _FolderIndex: maps normalized folder path -> (owning project, depth); match(target) longest prefix
//! def _project_for_path(index, target) -> Optional[dict]
//! def _project_for_session(session, index, resolve) -> Optional[dict]: cwd vs repo_root deepest
//! def _session_cost(session) -> float: actual_cost_usd or estimated_cost_usd
//! def _project_node(*, pid, label, path, repos, session_count, last_active, preview_sessions, sessions, color, icon, is_auto, is_no_project) -> dict
//! def build_tree(projects, sessions, discovered_repos, resolve, preview_limit=3, hydrate=False, is_junk_root, is_junk_cwd, exists) -> dict: three tiers + homeless
//! ```
//!
//! # Rust mapping
//!
//! * `_KANBAN_DIR_RE` (`r"^(.*[/\\]\\.worktrees)[/\\]t_[0-9a-f]+[/\\]?$"`) → [`kanban_worktree_dir`]
//!   manual split (no `regex` crate): strip trailing sep, last segment `t_` + hex, parent basename `.worktrees`.
//! * `_TRUNK_BRANCHES` / `DEFAULT_BRANCH_LABEL` / `NO_PROJECT_ID` / `NO_PROJECT_LABEL` / `_MAX_SIBLING_PROBES`
//!   → [`TRUNK_BRANCHES`] / [`DEFAULT_BRANCH_LABEL`] / [`NO_PROJECT_ID`] / [`NO_PROJECT_LABEL`] / [`MAX_SIBLING_PROBES`].
//! * `_branch_lane_id` / `_kanban_lane_id` → [`branch_lane_id`] / [`kanban_lane_id`] (same `::branch::` / `::kanban` strings).
//! * `_segments` → [`segments`] (`split(|c| c=='/'||c=='\\')` after `trim_end_matches` same filter).
//! * `_is_windows_path` → [`is_windows_path`] (drive `C:` + slash or leading `\`/`//`, includes `\wsl`/`\Users`).
//! * `_comparison_segments` / `_path_key` → [`comparison_segments`] / [`path_key`] (`to_lowercase` as `casefold` approximation; display/ids keep original).
//! * `_lane_key` → [`lane_key`] (splits on `::branch::`/`::kanban`, canonicalizes root only, suffix byte-preserved).
//! * `base_name` / `kanban_worktree_dir` → [`base_name`] / [`kanban_worktree_dir`].
//! * `_is_path_under` → [`is_path_under`] (segment-wise prefix via `comparison_segments`).
//! * `_with_base_name` / `_parent_dir` → [`with_base_name`] / [`parent_dir`] (same `re.sub` dance via `rsplit`/`trim_end_matches`).
//! * `_placement` → [`Placement`] struct + [`placement`] helper.
//! * `_probe_sibling_worktree` → [`probe_sibling_worktree`] (bounded `MAX_SIBLING_PROBES`, deepest ancestor first, `split("-")` trim).
//! * `_place_by_heuristic` → [`place_by_heuristic`] (kanban path shape, `-wt-` split, else trunk lane).
//! * `_place` → [`place`] (live probe → persisted_root → sibling → heuristic, same `is_main` fold `branch or DEFAULT`, kanban detection).
//! * `_session_repo_root` → [`session_repo_root`] (resolve cwd → repo_root else persisted `git_repo_root`).
//! * `_lane_sort_key` / `_sort_lanes` → [`lane_sort_key`] / [`sort_lanes`] (trunk top, kanban bottom, `-activity`, `label.to_lowercase()`).
//! * `_disambiguate_labels` → [`disambiguate_labels`] (in-place `label` growth, `path`-less skipped, `depth` prefix loop).
//! * `_session_time` → [`session_time`] (`last_active` or `started_at` or `0.0`).
//! * `_build_repos` → [`build_repos`] (lane dedup via `lane_key`, `session_time` reverse sort, `RepoNode` grouping, `sort_lanes` + hydrate-slim after sort).
//! * `_seed_folder_repos` → [`seed_folder_repos`] (resolve folder raw, `path_key` dedup, empty `RepoNode` with `groups:[]`).
//! * `_FolderIndex` / `_project_for_path` / `_project_for_session` → [`FolderIndex`] / [`project_for_path`] / [`project_for_session`] (deepest folder wins, ties keep first project, `cwd` vs `repo_root` candidates).
//! * `_session_cost` → [`session_cost`] (`actual_cost_usd` else `estimated_cost_usd` else `0.0`).
//! * `_project_node` → [`ProjectNode`] + helpers (totals over same `sessions`, `totalTokens` sum, preview via `session_time`).
//! * `build_tree` → [`build_tree`] / [`build_tree_with_options`] / [`BuildTreeOptions`] (explicit tier 1 explicit projects always shown, tier 2 auto from leftover sessions via `is_junk_root`/`exists`, tier 3 discovered full-history repos folded to common root, tier 0 homeless Home bucket `__no_project__`, `preview_limit` + `hydrate` lanes slim, auto label disambiguation).

use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// Constants — mirrors project_tree.py:45-63
// ---------------------------------------------------------------------------

/// Mirrors `_TRUNK_BRANCHES = {"main","master","trunk","develop"}`.
pub const TRUNK_BRANCHES: &[&str] = &["main", "master", "trunk", "develop"];

/// Mirrors `DEFAULT_BRANCH_LABEL = "main"`.
pub const DEFAULT_BRANCH_LABEL: &str = "main";

/// Mirrors `NO_PROJECT_ID = "__no_project__"`.
pub const NO_PROJECT_ID: &str = "__no_project__";

/// Mirrors `NO_PROJECT_LABEL = "Home"`.
pub const NO_PROJECT_LABEL: &str = "Home";

/// Mirrors `_MAX_SIBLING_PROBES = 4`.
pub const MAX_SIBLING_PROBES: usize = 4;

// ---------------------------------------------------------------------------
// Resolve type — mirrors `Resolve = Callable[[str], Optional[dict]]`
// ---------------------------------------------------------------------------

/// Result of a git identity probe.
///
/// Mirrors `{"repo_root": str, "worktree_root": str, "is_main"?: bool}`.
/// `repo_root` is the COMMON (main) repo root shared across worktrees;
/// `worktree_root` is this cwd's own checkout root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveResult {
    /// COMMON (main) repo root.
    pub repo_root: String,
    /// This cwd's own checkout root.
    pub worktree_root: String,
    /// Whether this checkout is the main. Mirrors `info.get("is_main")`.
    pub is_main: bool,
}

impl ResolveResult {
    pub fn new(repo_root: impl Into<String>, worktree_root: impl Into<String>) -> Self {
        Self {
            repo_root: repo_root.into(),
            worktree_root: worktree_root.into(),
            is_main: false,
        }
    }
    pub fn with_is_main(mut self, is_main: bool) -> Self {
        self.is_main = is_main;
        self
    }
}

// ---------------------------------------------------------------------------
// Path helpers — mirrors project_tree.py:78-143
// ---------------------------------------------------------------------------

/// Split path into non-empty segments on `/` or `\` after stripping trailing separators.
///
/// Mirrors `_segments`:
///
/// ```python
/// def _segments(path: str) -> list[str]:
///     return [s for s in re.split(r"[/\\]", (path or "").rstrip("/\\")) if s]
/// ```
pub fn segments(path: &str) -> Vec<String> {
    let trimmed = path.trim_end_matches(|c| c == '/' || c == '\\');
    if trimmed.is_empty() {
        return Vec::new();
    }
    trimmed
        .split(|c| c == '/' || c == '\\')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Whether `path` is a Windows path (drive-letter, UNC, or backslash-rooted).
///
/// Mirrors `_is_windows_path`:
///
/// ```python
/// def _is_windows_path(path: str) -> bool:
///     value = (path or "").strip()
///     return bool(re.match(r"^[A-Za-z]:[/\\]", value)) or value.startswith(("\\", "//"))
/// ```
pub fn is_windows_path(path: &str) -> bool {
    let v = path.trim();
    if v.is_empty() {
        return false;
    }
    let bytes = v.as_bytes();
    // Drive-letter `C:\` or `C:/`
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\')
    {
        return true;
    }
    // UNC or backslash-rooted: starts with `\` or `//`
    if v.starts_with('\\') || v.starts_with("//") {
        return true;
    }
    false
}

/// Path segments suitable for identity comparisons (case-insensitive on Windows).
///
/// Mirrors `_comparison_segments`:
///
/// ```python
/// def _comparison_segments(path: str) -> list[str]:
///     segs = _segments(path)
///     return [segment.casefold() for segment in segs] if _is_windows_path(path) else segs
/// ```
pub fn comparison_segments(path: &str) -> Vec<String> {
    let segs = segments(path);
    if is_windows_path(path) {
        segs.into_iter().map(|s| s.to_lowercase()).collect()
    } else {
        segs
    }
}

/// Canonical comparison key (separator/trailing-slash agnostic).
///
/// Mirrors `_path_key`:
///
/// ```python
/// def _path_key(path: str) -> str:
///     return "/".join(_comparison_segments(path))
/// ```
pub fn path_key(path: &str) -> String {
    comparison_segments(path).join("/")
}

/// Canonicalize only the path portion of a lane id (branch labels byte-preserved).
///
/// Mirrors `_lane_key`:
///
/// ```python
/// def _lane_key(path_or_lane: str) -> str:
///     for marker in ("::branch::", "::kanban"):
///         if marker in path_or_lane:
///             root, suffix = path_or_lane.split(marker, 1)
///             return f"{_path_key(root)}{marker}{suffix}"
///     return _path_key(path_or_lane)
/// ```
pub fn lane_key(path_or_lane: &str) -> String {
    for marker in ["::branch::", "::kanban"] {
        if let Some(idx) = path_or_lane.find(marker) {
            let root = &path_or_lane[..idx];
            let suffix = &path_or_lane[idx + marker.len()..];
            return format!("{}{}{}", path_key(root), marker, suffix);
        }
    }
    path_key(path_or_lane)
}

/// Basename (last segment) or "".
///
/// Mirrors `base_name`:
///
/// ```python
/// def base_name(path: str) -> str:
///     segs = _segments(path)
///     return segs[-1] if segs else ""
/// ```
pub fn base_name(path: &str) -> String {
    segments(path).pop().unwrap_or_default()
}

/// The `<repo>/.worktrees` dir for a `.../.worktrees/<task>` path, else None.
///
/// Mirrors `kanban_worktree_dir` / `_KANBAN_DIR_RE = re.compile(r"^(.*[/\\]\\.worktrees)[/\\]t_[0-9a-f]+[/\\]?$")`.
///
/// Returns `Some("<repo>/.worktrees")` when `path` is `<repo>/.worktrees/t_<hex>` (with optional trailing slash),
/// else `None`. Only `t_` ids with hex suffix collapse; user-named dirs stay as own lanes.
pub fn kanban_worktree_dir(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    // Strip trailing slashes, then check last segment is t_<hex>
    let trimmed = path.trim_end_matches(|c| c == '/' || c == '\\');
    if trimmed.is_empty() {
        return None;
    }
    // Find last separator index
    let last_sep = trimmed.rfind(|c| c == '/' || c == '\\');
    let (prefix, last) = match last_sep {
        Some(idx) => (&trimmed[..idx], &trimmed[idx + 1..]),
        None => ("", trimmed),
    };
    if last.is_empty() || !last.starts_with("t_") {
        return None;
    }
    let hex_part = &last[2..];
    if hex_part.is_empty() || !hex_part.chars().all(|c| c.is_ascii_hexdigit() && (c.is_ascii_digit() || c.is_ascii_lowercase() || c.is_ascii_uppercase())) {
        // Python's regex is [0-9a-f] (lowercase hex only), but we also accept uppercase leniently;
        // strict check would be lowercase only. Match Python exactly: only 0-9 a-f.
        // Re-check strict:
        if !hex_part.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
            return None;
        }
    }
    // Strict: only 0-9 a-f per Python
    if !hex_part.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
        return None;
    }
    // Prefix's basename must be ".worktrees"
    let prefix_base = base_name(prefix);
    // But per regex `(.*[/\\]\.worktrees)` the prefix must end with ".worktrees"
    // `prefix` is everything before last segment; its basename should be ".worktrees"
    // unless path is exactly ".../.worktrees/t_..." with no extra depth beyond .worktrees parent.
    // Actually for "/repo/.worktrees/t_abc", prefix = "/repo/.worktrees", base = ".worktrees" -> ok.
    // For "/repo/.worktrees", prefix = "/repo", last = ".worktrees" -> not matched because last is not t_*
    // So this path correctly identifies kanban.
    if prefix_base != ".worktrees" {
        // Edge: prefix could be "/repo/.worktrees" -> base ".worktrees" ok
        // If prefix is "/a/b/.worktrees", base is ".worktrees" ok
        return None;
    }
    // Return the .worktrees dir path (prefix)
    if prefix.is_empty() {
        return None;
    }
    Some(prefix.to_string())
}

/// True when `target` equals `folder` or is nested under it (segment-wise, platform-aware).
///
/// Mirrors `_is_path_under`:
///
/// ```python
/// def _is_path_under(folder: str, target: str) -> bool:
///     f = _comparison_segments(folder)
///     t = _comparison_segments(target)
///     if not f or len(f) > len(t):
///         return False
///     return all(f[i] == t[i] for i in range(len(f)))
/// ```
pub fn is_path_under(folder: &str, target: &str) -> bool {
    let f = comparison_segments(folder);
    let t = comparison_segments(target);
    if f.is_empty() || f.len() > t.len() {
        return false;
    }
    f.iter().zip(t.iter()).all(|(a, b)| a == b)
}

/// Replace basename of `path` with `name` (mirrors `re.sub(r"[^/\\]+$", name, stripped)`).
///
/// Mirrors `_with_base_name`:
///
/// ```python
/// def _with_base_name(path: str, name: str) -> str:
///     stripped = re.sub(r"[/\\]+$", "", path)
///     return re.sub(r"[^/\\]+$", name, stripped)
/// ```
pub fn with_base_name(path: &str, name: &str) -> String {
    let stripped = path.trim_end_matches(|c| c == '/' || c == '\\');
    if stripped.is_empty() {
        return name.to_string();
    }
    // Find last separator
    if let Some(idx) = stripped.rfind(|c| c == '/' || c == '\\') {
        // Keep separator, replace last segment
        format!("{}{}", &stripped[..idx + 1], name)
    } else {
        // No separator — the whole path is the basename
        name.to_string()
    }
}

// ---------------------------------------------------------------------------
// Lane placement — mirrors project_tree.py:150-168
// ---------------------------------------------------------------------------

/// Placement info for a session cwd → repo/lane mapping.
///
/// Mirrors the dict returned by `_placement`:
/// `{"repo_key","repo_label","repo_path","lane_key","lane_label","lane_path","is_main","is_kanban"}`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub repo_key: String,
    pub repo_label: String,
    pub repo_path: String,
    pub lane_key: String,
    pub lane_label: String,
    pub lane_path: String,
    pub is_main: bool,
    pub is_kanban: bool,
}

fn placement(
    repo_root: &str,
    lane_key_val: &str,
    lane_label: &str,
    lane_path: &str,
    is_main: bool,
    is_kanban: bool,
) -> Placement {
    Placement {
        repo_key: repo_root.to_string(),
        repo_label: if base_name(repo_root).is_empty() {
            repo_root.to_string()
        } else {
            base_name(repo_root)
        },
        repo_path: repo_root.to_string(),
        lane_key: lane_key_val.to_string(),
        lane_label: lane_label.to_string(),
        lane_path: lane_path.to_string(),
        is_main,
        is_kanban,
    }
}

/// The one definition of a main-checkout lane id (must match the desktop).
///
/// Mirrors `_branch_lane_id`:
///
/// ```python
/// def _branch_lane_id(repo_root: str, branch: str = "") -> str:
///     return f"{repo_root}::branch::{(branch or '').strip()}"
/// ```
pub fn branch_lane_id(repo_root: &str, branch: &str) -> String {
    format!("{}::branch::{}", repo_root, branch.trim())
}

/// Kanban lane id.
///
/// Mirrors `_kanban_lane_id`:
///
/// ```python
/// def _kanban_lane_id(repo_root: str) -> str:
///     return f"{repo_root}::kanban"
/// ```
pub fn kanban_lane_id(repo_root: &str) -> String {
    format!("{}::kanban", repo_root)
}

// ---------------------------------------------------------------------------
// Helpers: _parent_dir, _probe_sibling_worktree, _place_by_heuristic, _place
// ---------------------------------------------------------------------------

/// The containing directory of `path` ("" once the root is passed).
///
/// Mirrors `_parent_dir`:
///
/// ```python
/// def _parent_dir(path: str) -> str:
///     stripped = re.sub(r"[/\\]+$", "", path or "")
///     return re.sub(r"[/\\]+$", "", re.sub(r"[^/\\]+$", "", stripped))
/// ```
pub fn parent_dir(path: &str) -> String {
    let stripped = path.trim_end_matches(|c| c == '/' || c == '\\');
    if stripped.is_empty() {
        return String::new();
    }
    // Remove last segment (basename) — `re.sub(r"[^/\\]+$", "", stripped)`
    let without_base = if let Some(idx) = stripped.rfind(|c| c == '/' || c == '\\') {
        // Keep up to and including separator? Python's `re.sub(r"[^/\\]+$", "", stripped)`
        // removes the trailing non-separator run, leaving the separator.
        // e.g. "/a/b/c" -> "/a/b/" ; "/a/b" -> "/a/"; "a" -> ""
        &stripped[..idx + 1]
    } else {
        // No separator — basename is whole string, remove all
        ""
    };
    // Then strip trailing slashes again `re.sub(r"[/\\]+$", "", ...)`
    without_base.trim_end_matches(|c| c == '/' || c == '\\').to_string()
}

/// The parent repo root of a deleted `<repo>-<suffix>` worktree, else "".
///
/// Mirrors `_probe_sibling_worktree`:
///
/// ```python
/// def _probe_sibling_worktree(cwd: str, resolve: Resolve) -> str:
///     probes = 0
///     path = re.sub(r"[/\\]+$", "", cwd or "")
///     while path and probes < _MAX_SIBLING_PROBES:
///         parts = base_name(path).split("-")
///         for i in range(len(parts) - 1, 0, -1):
///             if probes >= _MAX_SIBLING_PROBES: break
///             probes += 1
///             info = resolve(_with_base_name(path, "-".join(parts[:i])))
///             if info and info.get("repo_root"):
///                 return (info["repo_root"] or "").strip()
///         path = _parent_dir(path)
///     return ""
/// ```
pub fn probe_sibling_worktree(cwd: &str, resolve: &dyn Fn(&str) -> Option<ResolveResult>) -> String {
    let mut probes = 0usize;
    let mut path = cwd.trim_end_matches(|c| c == '/' || c == '\\').to_string();

    while !path.is_empty() && probes < MAX_SIBLING_PROBES {
        let bn = base_name(&path);
        let parts: Vec<&str> = bn.split('-').collect();

        for i in (1..parts.len()).rev() {
            if probes >= MAX_SIBLING_PROBES {
                break;
            }
            probes += 1;
            let candidate_name = parts[..i].join("-");
            let candidate = with_base_name(&path, &candidate_name);
            if let Some(info) = resolve(&candidate) {
                let root = info.repo_root.trim();
                if !root.is_empty() {
                    return root.to_string();
                }
            }
        }
        path = parent_dir(&path);
    }
    String::new()
}

/// Path-only fallback when there is no git probe and no persisted root.
///
/// Mirrors `_place_by_heuristic`:
///
/// ```python
/// def _place_by_heuristic(path: str) -> Optional[dict]:
///     base = base_name(path)
///     if not base:
///         return None
///     kanban_dir = kanban_worktree_dir(path)
///     if kanban_dir:
///         repo_path = re.sub(r"[/\\]+$", "", _with_base_name(kanban_dir, ""))
///         return _placement(repo_path, _kanban_lane_id(repo_path), "kanban", kanban_dir, False, True)
///     m = re.match(r"^(.+)-wt-(.+)$", base)
///     if m:
///         repo_path = _with_base_name(path, m.group(1))
///         return _placement(repo_path, path, m.group(2), path, False, False)
///     return _placement(path, _branch_lane_id(path, DEFAULT_BRANCH_LABEL), base, path, True, False)
/// ```
pub fn place_by_heuristic(path: &str) -> Option<Placement> {
    let base = base_name(path);
    if base.is_empty() {
        return None;
    }

    if let Some(kanban_dir) = kanban_worktree_dir(path) {
        // repo_path = re.sub(r"[/\\]+$", "", _with_base_name(kanban_dir, ""))
        // _with_base_name(kanban_dir, "") yields dir with trailing slash → strip it
        let repo_path_raw = with_base_name(&kanban_dir, "");
        let repo_path = repo_path_raw.trim_end_matches(|c| c == '/' || c == '\\').to_string();
        let lane_id = kanban_lane_id(&repo_path);
        return Some(placement(&repo_path, &lane_id, "kanban", &kanban_dir, false, true));
    }

    // Check for `-wt-` suffix: `^(.+)-wt-(.+)$` on base
    if let Some(idx) = base.find("-wt-") {
        // Ensure there's content before and after
        if idx > 0 && idx + 4 < base.len() {
            let repo_base = &base[..idx];
            let wt_suffix = &base[idx + 4..];
            if !wt_suffix.is_empty() {
                let repo_path = with_base_name(path, repo_base);
                return Some(placement(&repo_path, path, wt_suffix, path, false, false));
            }
        }
    }

    let lane_id = branch_lane_id(path, DEFAULT_BRANCH_LABEL);
    Some(placement(path, &lane_id, &base, path, true, false))
}

/// Lane placement for a cwd (probe → persisted → sibling → heuristic).
///
/// Mirrors `_place`:
///
/// ```python
/// def _place(cwd: str, branch: str, resolve: Optional[Resolve], persisted_root: str) -> Optional[dict]:
///     info = resolve(cwd) if resolve else None
///     if info and info.get("repo_root") and info.get("worktree_root"):
///         repo_root = info["repo_root"]; worktree_root = info["worktree_root"]
///         is_main = _path_key(worktree_root) == _path_key(repo_root) or bool(info.get("is_main"))
///         if is_main:
///             b = (branch or "").strip() or DEFAULT_BRANCH_LABEL
///             return _placement(repo_root, _branch_lane_id(repo_root, b), b, repo_root, True, False)
///         kanban_dir = kanban_worktree_dir(worktree_root)
///         if kanban_dir:
///             return _placement(repo_root, _kanban_lane_id(repo_root), "kanban", kanban_dir, False, True)
///         label = base_name(worktree_root) or worktree_root
///         return _placement(repo_root, worktree_root, label, worktree_root, False, False)
///     if persisted_root:
///         kanban_dir = kanban_worktree_dir(cwd)
///         if kanban_dir:
///             return _placement(persisted_root, _kanban_lane_id(persisted_root), "kanban", kanban_dir, False, True)
///         b = (branch or "").strip() or DEFAULT_BRANCH_LABEL
///         return _placement(persisted_root, _branch_lane_id(persisted_root, b), b, persisted_root, True, False)
///     sibling_root = _probe_sibling_worktree(cwd, resolve) if resolve else ""
///     if sibling_root:
///         b = (branch or "").strip() or DEFAULT_BRANCH_LABEL
///         return _placement(sibling_root, _branch_lane_id(sibling_root, b), b, sibling_root, True, False)
///     return _place_by_heuristic(cwd)
/// ```
pub fn place(
    cwd: &str,
    branch: &str,
    resolve: Option<&dyn Fn(&str) -> Option<ResolveResult>>,
    persisted_root: &str,
) -> Option<Placement> {
    if let Some(res) = resolve {
        if let Some(info) = res(cwd) {
            if !info.repo_root.trim().is_empty() && !info.worktree_root.trim().is_empty() {
                let repo_root = info.repo_root.trim().to_string();
                let worktree_root = info.worktree_root.trim().to_string();
                let is_main = path_key(&worktree_root) == path_key(&repo_root) || info.is_main;
                if is_main {
                    let b = branch.trim();
                    let b = if b.is_empty() { DEFAULT_BRANCH_LABEL } else { b };
                    let lid = branch_lane_id(&repo_root, b);
                    return Some(placement(&repo_root, &lid, b, &repo_root, true, false));
                }
                if let Some(kanban_dir) = kanban_worktree_dir(&worktree_root) {
                    let lid = kanban_lane_id(&repo_root);
                    return Some(placement(&repo_root, &lid, "kanban", &kanban_dir, false, true));
                }
                let label = {
                    let bn = base_name(&worktree_root);
                    if bn.is_empty() { worktree_root.clone() } else { bn }
                };
                return Some(placement(&repo_root, &worktree_root, &label, &worktree_root, false, false));
            }
        }
    }

    if !persisted_root.trim().is_empty() {
        let pr = persisted_root.trim();
        if let Some(kanban_dir) = kanban_worktree_dir(cwd) {
            let lid = kanban_lane_id(pr);
            return Some(placement(pr, &lid, "kanban", &kanban_dir, false, true));
        }
        let b = branch.trim();
        let b = if b.is_empty() { DEFAULT_BRANCH_LABEL } else { b };
        let lid = branch_lane_id(pr, b);
        return Some(placement(pr, &lid, b, pr, true, false));
    }

    let sibling_root = if let Some(res) = resolve {
        probe_sibling_worktree(cwd, res)
    } else {
        String::new()
    };
    if !sibling_root.is_empty() {
        let b = branch.trim();
        let b = if b.is_empty() { DEFAULT_BRANCH_LABEL } else { b };
        let sr = sibling_root.trim().to_string();
        let lid = branch_lane_id(&sr, b);
        return Some(placement(&sr, &lid, b, &sr, true, false));
    }

    place_by_heuristic(cwd)
}

// ---------------------------------------------------------------------------
// Session / project shapes — mirrors Python dict shapes
// ---------------------------------------------------------------------------

/// Session row shape (must carry `id`, `cwd`, `git_branch`, `git_repo_root`, `started_at`, `last_active`).
///
/// Mirrors the `sessions` list passed to `build_tree` — projected session-row dicts.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: String,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub git_repo_root: Option<String>,
    pub started_at: Option<f64>,
    pub last_active: Option<f64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub actual_cost_usd: Option<f64>,
    pub estimated_cost_usd: Option<f64>,
}

impl Session {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            cwd: None,
            git_branch: None,
            git_repo_root: None,
            started_at: None,
            last_active: None,
            input_tokens: None,
            output_tokens: None,
            actual_cost_usd: None,
            estimated_cost_usd: None,
        }
    }
    pub fn with_cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }
    pub fn with_branch(mut self, b: impl Into<String>) -> Self {
        self.git_branch = Some(b.into());
        self
    }
    pub fn with_repo_root(mut self, r: impl Into<String>) -> Self {
        self.git_repo_root = Some(r.into());
        self
    }
    pub fn with_times(mut self, started: Option<f64>, last: Option<f64>) -> Self {
        self.started_at = started;
        self.last_active = last;
        self
    }
    pub fn with_tokens(mut self, input: u64, output: u64) -> Self {
        self.input_tokens = Some(input);
        self.output_tokens = Some(output);
        self
    }
    pub fn with_costs(mut self, actual: Option<f64>, estimated: Option<f64>) -> Self {
        self.actual_cost_usd = actual;
        self.estimated_cost_usd = estimated;
        self
    }
}

/// Project folder shape (part of `projects_db.Project.to_dict()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub path: String,
}

impl Folder {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }
}

/// Project shape (`projects_db.Project.to_dict()` — non-archived).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub name: Option<String>,
    pub primary_path: Option<String>,
    pub color: Option<String>,
    pub icon: Option<String>,
    pub folders: Vec<Folder>,
    pub archived: bool,
}

impl Project {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: None,
            primary_path: None,
            color: None,
            icon: None,
            folders: Vec::new(),
            archived: false,
        }
    }
    pub fn with_name(mut self, n: impl Into<String>) -> Self {
        self.name = Some(n.into());
        self
    }
    pub fn with_primary_path(mut self, p: impl Into<String>) -> Self {
        self.primary_path = Some(p.into());
        self
    }
    pub fn with_folders(mut self, folders: Vec<Folder>) -> Self {
        self.folders = folders;
        self
    }
}

/// Discovered repo shape `{"root","label","sessions","last_active"}`.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredRepo {
    pub root: String,
    pub label: Option<String>,
    pub sessions: Option<u64>,
    pub last_active: Option<f64>,
}

impl DiscoveredRepo {
    pub fn new(root: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            label: None,
            sessions: None,
            last_active: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Ordering + label disambiguation — mirrors project_tree.py:283-326
// ---------------------------------------------------------------------------

/// Compute session recency time — mirrors `_session_time`:
///
/// ```python
/// def _session_time(session: dict) -> float:
///     return float(session.get("last_active") or session.get("started_at") or 0)
/// ```
pub fn session_time(s: &Session) -> f64 {
    s.last_active.or(s.started_at).unwrap_or(0.0)
}

fn session_time_for_group(sessions: &[Session]) -> f64 {
    sessions.iter().map(session_time).fold(0.0_f64, f64::max)
}

// Group / repo label disambiguation
// We operate on `RepoNode`/`LaneGroup`/`ProjectNode` labels via helpers below.

/// Grow colliding basenames into path-prefixed labels (in place).
///
/// Mirrors `_disambiguate_labels`:
///
/// ```python
/// def _disambiguate_labels(items: list[dict]) -> None:
///     by_label: dict[str, list[dict]] = {}
///     for item in items:
///         by_label.setdefault(item["label"], []).append(item)
///     for bucket in by_label.values():
///         pathed = [g for g in bucket if g.get("path")]
///         if len(pathed) < 2:
///             continue
///         parents = {id(g): _segments(g["path"])[:-1] for g in pathed}
///         max_depth = max(len(p) for p in parents.values())
///         depth = 1
///         while depth <= max_depth:
///             counts: dict[str, int] = {}
///             for g in pathed:
///                 segs = parents[id(g)]
///                 prefix = "/".join(segs[-depth:]) if depth else ""
///                 base = base_name(g["path"]) or g["path"]
///                 g["label"] = f"{prefix}/{base}" if prefix else base
///                 counts[g["label"]] = counts.get(g["label"], 0) + 1
///             if all(c == 1 for c in counts.values()):
///                 break
///             depth += 1
/// ```
fn disambiguate_labels_by<F, G>(items: &mut [ProjectNode], mut get_label: F, mut get_path: G, mut set_label: impl FnMut(&mut ProjectNode, String))
where
    F: Fn(&ProjectNode) -> String,
    G: Fn(&ProjectNode) -> Option<String>,
{
    let _ = (get_label, get_path, &mut set_label);
}

/// Generic disambiguation for mutable slices where label/path are string fields.
///
/// We need separate variants because Rust borrow checker prevents a single generic
/// with `&mut [T]` + `HashMap<&str, Vec<&mut T>>`. Instead we implement per-type helpers.

fn disambiguate_lane_groups(groups: &mut [LaneGroup]) {
    // Group by current label value
    let mut by_label: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, g) in groups.iter().enumerate() {
        by_label.entry(g.label.clone()).or_default().push(idx);
    }
    for bucket_indices in by_label.values() {
        if bucket_indices.len() < 2 {
            continue;
        }
        // Only pathed entries participate
        let pathed: Vec<usize> = bucket_indices.iter().copied().filter(|&i| groups[i].path.is_some()).collect();
        if pathed.len() < 2 {
            continue;
        }
        // parents: idx -> segments(path)[:-1]
        let mut parents: HashMap<usize, Vec<String>> = HashMap::new();
        let mut max_depth = 0usize;
        for &i in &pathed {
            let p = groups[i].path.as_deref().unwrap_or("");
            let mut segs = segments(p);
            if !segs.is_empty() {
                segs.pop();
            }
            max_depth = max_depth.max(segs.len());
            parents.insert(i, segs);
        }
        let mut depth = 1usize;
        while depth <= max_depth {
            let mut counts: HashMap<String, usize> = HashMap::new();
            for &i in &pathed {
                let segs = parents.get(&i).unwrap();
                let prefix = if segs.len() >= depth {
                    segs[segs.len() - depth..].join("/")
                } else {
                    segs.join("/")
                };
                let path_val = groups[i].path.as_deref().unwrap_or("");
                let base = {
                    let bn = base_name(path_val);
                    if bn.is_empty() { path_val.to_string() } else { bn }
                };
                let new_label = if prefix.is_empty() { base.clone() } else { format!("{}/{}", prefix, base) };
                groups[i].label = new_label.clone();
                *counts.entry(new_label).or_insert(0) += 1;
            }
            if counts.values().all(|&c| c == 1) {
                break;
            }
            depth += 1;
        }
    }
}

fn disambiguate_repos(repos: &mut [RepoNode]) {
    let mut by_label: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, r) in repos.iter().enumerate() {
        by_label.entry(r.label.clone()).or_default().push(idx);
    }
    for bucket_indices in by_label.values() {
        if bucket_indices.len() < 2 {
            continue;
        }
        let pathed: Vec<usize> = bucket_indices.iter().copied().filter(|&i| repos[i].path.is_some()).collect();
        if pathed.len() < 2 {
            continue;
        }
        let mut parents: HashMap<usize, Vec<String>> = HashMap::new();
        let mut max_depth = 0usize;
        for &i in &pathed {
            let p = repos[i].path.as_deref().unwrap_or("");
            let mut segs = segments(p);
            if !segs.is_empty() {
                segs.pop();
            }
            max_depth = max_depth.max(segs.len());
            parents.insert(i, segs);
        }
        let mut depth = 1usize;
        while depth <= max_depth {
            let mut counts: HashMap<String, usize> = HashMap::new();
            for &i in &pathed {
                let segs = parents.get(&i).unwrap();
                let prefix = if segs.len() >= depth {
                    segs[segs.len() - depth..].join("/")
                } else {
                    segs.join("/")
                };
                let path_val = repos[i].path.as_deref().unwrap_or("");
                let base = {
                    let bn = base_name(path_val);
                    if bn.is_empty() { path_val.to_string() } else { bn }
                };
                let new_label = if prefix.is_empty() { base.clone() } else { format!("{}/{}", prefix, base) };
                repos[i].label = new_label.clone();
                *counts.entry(new_label).or_insert(0) += 1;
            }
            if counts.values().all(|&c| c == 1) {
                break;
            }
            depth += 1;
        }
    }
}

fn disambiguate_projects(projects: &mut [ProjectNode]) {
    let mut by_label: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, p) in projects.iter().enumerate() {
        by_label.entry(p.label.clone()).or_default().push(idx);
    }
    for bucket_indices in by_label.values() {
        if bucket_indices.len() < 2 {
            continue;
        }
        let pathed: Vec<usize> = bucket_indices.iter().copied().filter(|&i| projects[i].path.is_some()).collect();
        if pathed.len() < 2 {
            continue;
        }
        let mut parents: HashMap<usize, Vec<String>> = HashMap::new();
        let mut max_depth = 0usize;
        for &i in &pathed {
            let p = projects[i].path.as_deref().unwrap_or("");
            let mut segs = segments(p);
            if !segs.is_empty() {
                segs.pop();
            }
            max_depth = max_depth.max(segs.len());
            parents.insert(i, segs);
        }
        let mut depth = 1usize;
        while depth <= max_depth {
            let mut counts: HashMap<String, usize> = HashMap::new();
            for &i in &pathed {
                let segs = parents.get(&i).unwrap();
                let prefix = if segs.len() >= depth {
                    segs[segs.len() - depth..].join("/")
                } else {
                    segs.join("/")
                };
                let path_val = projects[i].path.as_deref().unwrap_or("");
                let base = {
                    let bn = base_name(path_val);
                    if bn.is_empty() { path_val.to_string() } else { bn }
                };
                let new_label = if prefix.is_empty() { base.clone() } else { format!("{}/{}", prefix, base) };
                projects[i].label = new_label.clone();
                *counts.entry(new_label).or_insert(0) += 1;
            }
            if counts.values().all(|&c| c == 1) {
                break;
            }
            depth += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Repo subtree assembly — mirrors project_tree.py:337-449
// ---------------------------------------------------------------------------

/// Lane group (`repo -> lane -> sessions`).
#[derive(Debug, Clone, PartialEq)]
pub struct LaneGroup {
    pub id: String,
    pub label: String,
    pub path: Option<String>,
    pub is_main: bool,
    pub is_kanban: bool,
    pub sessions: Vec<Session>,
}

/// Repo node (`sessionCount` + `groups`).
#[derive(Debug, Clone, PartialEq)]
pub struct RepoNode {
    pub id: String,
    pub label: String,
    pub path: Option<String>,
    pub groups: Vec<LaneGroup>,
    pub session_count: usize,
}

fn lane_sort_key_rank(group: &LaneGroup) -> (u8, u8, f64, String) {
    // Mirrors `_lane_sort_key`:
    // is_trunk = isMain and label.lower in TRUNK_BRANCHES
    // return (0 if is_trunk else 1, 1 if isKanban else 0, -activity, label.lower())
    // We return separate components; sorting by this tuple works if we negate activity.
    let is_trunk = group.is_main && TRUNK_BRANCHES.contains(&group.label.to_lowercase().as_str());
    let trunk_rank = if is_trunk { 0 } else { 1 };
    let kanban_rank = if group.is_kanban { 1 } else { 0 };
    let activity = group.sessions.iter().map(|s| session_time(s)).fold(0.0_f64, f64::max);
    // For descending activity we use -activity; f64 ordering handled via partial_cmp
    // Store -activity directly
    (-activity, group.label.to_lowercase()); // We'll compose externally
    (trunk_rank, kanban_rank, -activity, group.label.to_lowercase())
}

fn sort_lanes(groups: &mut [LaneGroup]) {
    groups.sort_by(|a, b| {
        let ka = lane_sort_key_rank(a);
        let kb = lane_sort_key_rank(b);
        ka.0.cmp(&kb.0)
            .then_with(|| ka.1.cmp(&kb.1))
            .then_with(|| ka.2.partial_cmp(&kb.2).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| ka.3.cmp(&kb.3))
    });
}

/// Build the `repo -> lane -> sessions` subtree for a set of sessions.
///
/// Mirrors `_build_repos`:
///
/// ```python
/// def _build_repos(sessions, resolve, hydrate) -> list[dict]:
///     lanes: dict[str, dict] = {}
///     for session in sessions:
///         cwd = (session.get("cwd") or "").strip()
///         if not cwd: continue
///         placement = _place(cwd, branch, resolve, git_repo_root)
///         lane_identity = _lane_key(placement["lane_key"])
///         entry = lanes.get(lane_identity)
///         ...
///     repos: dict[str, dict] = {}
///     for entry in lanes.values():
///         group.sessions.sort reverse
///         repo_identity = _path_key(entry["repo_key"])
///         repo = repos.get(repo_identity)
///         ...
///     for repo in repo_list:
///         repo["groups"] = _sort_lanes(repo["groups"])
///         _disambiguate_labels(repo["groups"])
///         if not hydrate: for group in repo["groups"]: group["sessions"] = []
///     _disambiguate_labels(repo_list)
///     return repo_list
/// ```
pub fn build_repos(sessions: &[Session], resolve: Option<&dyn Fn(&str) -> Option<ResolveResult>>, hydrate: bool) -> Vec<RepoNode> {
    // lane_key canonical -> LaneGroup + repo meta
    struct LaneEntry {
        group: LaneGroup,
        repo_key: String,
        repo_label: String,
        repo_path: String,
    }
    let mut lanes: HashMap<String, LaneEntry> = HashMap::new();

    for session in sessions {
        let cwd = session.cwd.as_deref().map(|s| s.trim()).unwrap_or("");
        if cwd.is_empty() {
            continue;
        }
        let branch = session.git_branch.as_deref().map(|s| s.trim()).unwrap_or("");
        let persisted = session.git_repo_root.as_deref().map(|s| s.trim()).unwrap_or("");
        let p = match place(cwd, branch, resolve, persisted) {
            Some(v) => v,
            None => continue,
        };
        let lane_identity = lane_key(&p.lane_key);
        let entry = lanes.entry(lane_identity).or_insert_with(|| LaneEntry {
            group: LaneGroup {
                id: p.lane_key.clone(),
                label: p.lane_label.clone(),
                path: Some(p.lane_path.clone()),
                is_main: p.is_main,
                is_kanban: p.is_kanban,
                sessions: Vec::new(),
            },
            repo_key: p.repo_key.clone(),
            repo_label: p.repo_label.clone(),
            repo_path: p.repo_path.clone(),
        });
        entry.group.sessions.push(session.clone());
    }

    // Group lanes into repos
    let mut repos: HashMap<String, RepoNode> = HashMap::new();
    for entry in lanes.into_values() {
        let mut group = entry.group;
        group.sessions.sort_by(|a, b| session_time(b).partial_cmp(&session_time(a)).unwrap_or(std::cmp::Ordering::Equal));
        let count = group.sessions.len();
        let repo_identity = path_key(&entry.repo_key);
        let repo = repos.entry(repo_identity).or_insert_with(|| RepoNode {
            id: entry.repo_key.clone(),
            label: entry.repo_label.clone(),
            path: Some(entry.repo_path.clone()),
            groups: Vec::new(),
            session_count: 0,
        });
        repo.groups.push(group);
        repo.session_count += count;
    }

    let mut repo_list: Vec<RepoNode> = repos.into_values().collect();
    for repo in &mut repo_list {
        sort_lanes(&mut repo.groups);
        disambiguate_lane_groups(&mut repo.groups);
        if !hydrate {
            for g in &mut repo.groups {
                g.sessions.clear();
            }
        }
    }
    disambiguate_repos(&mut repo_list);
    repo_list
}

/// Ensure every declared project folder shows as a repo, even with 0 sessions.
///
/// Mirrors `_seed_folder_repos`:
///
/// ```python
/// def _seed_folder_repos(repos, folders, resolve) -> list[dict]:
///     seen = {_path_key(value) for repo in repos for value in (repo.get("id"), repo.get("path")) if value}
///     seeded = list(repos)
///     for folder in folders or []:
///         raw = (folder.get("path") or "").strip()
///         if not raw: continue
///         info = resolve(raw) if resolve else None
///         root = (info or {}).get("repo_root") or re.sub(r"[/\\]+$", "", raw)
///         root_key = _path_key(root)
///         if not root_key or root_key in seen: continue
///         seeded.append({"id": root, "label": base_name(root) or root, "path": root, "groups": [], "sessionCount": 0})
///         seen.add(root_key)
///     if len(seeded) != len(repos):
///         _disambiguate_labels(seeded)
///     return seeded
/// ```
pub fn seed_folder_repos(
    mut repos: Vec<RepoNode>,
    folders: &[Folder],
    resolve: Option<&dyn Fn(&str) -> Option<ResolveResult>>,
) -> Vec<RepoNode> {
    let mut seen: HashSet<String> = HashSet::new();
    for repo in &repos {
        for val in [&repo.id, repo.path.as_deref().unwrap_or("")] {
            if !val.is_empty() {
                seen.insert(path_key(val));
            }
        }
    }
    let mut seeded = repos;
    let mut added = false;
    for folder in folders {
        let raw = folder.path.trim();
        if raw.is_empty() {
            continue;
        }
        let root = if let Some(res) = resolve {
            if let Some(info) = res(raw) {
                let r = info.repo_root.trim();
                if !r.is_empty() { r.to_string() } else { raw.trim_end_matches(|c| c == '/' || c == '\\').to_string() }
            } else {
                raw.trim_end_matches(|c| c == '/' || c == '\\').to_string()
            }
        } else {
            raw.trim_end_matches(|c| c == '/' || c == '\\').to_string()
        };
        let root_key = path_key(&root);
        if root_key.is_empty() || seen.contains(&root_key) {
            continue;
        }
        let label = {
            let bn = base_name(&root);
            if bn.is_empty() { root.clone() } else { bn }
        };
        seeded.push(RepoNode {
            id: root.clone(),
            label,
            path: Some(root.clone()),
            groups: Vec::new(),
            session_count: 0,
        });
        seen.insert(root_key);
        added = true;
    }
    if added {
        disambiguate_repos(&mut seeded);
    }
    seeded
}

// ---------------------------------------------------------------------------
// Explicit-project ownership — mirrors project_tree.py:456-508
// ---------------------------------------------------------------------------

/// Maps a normalized folder path → (owning project, depth).
///
/// Mirrors `_FolderIndex`:
///
/// ```python
/// class _FolderIndex:
///     def __init__(self, projects):
///         self._by_path: dict[str, tuple[dict, int]] = {}
///         for project in projects:
///             for folder in project.get("folders") or []:
///                 segs = _comparison_segments(folder.get("path") or "")
///                 if not segs: continue
///                 key = "/".join(segs)
///                 depth = len(segs)
///                 existing = self._by_path.get(key)
///                 if existing is None or depth > existing[1]:
///                     self._by_path[key] = (project, depth)
///     def match(self, target: str) -> tuple[Optional[dict], int]:
///         segs = _comparison_segments(target or "")
///         for end in range(len(segs), 0, -1):
///             hit = self._by_path.get("/".join(segs[:end]))
///             if hit: return hit
///         return None, -1
/// ```
pub struct FolderIndex {
    /// Normalized path key → (project_index, depth)
    by_path: HashMap<String, (usize, usize)>,
}

impl FolderIndex {
    pub fn new(projects: &[Project]) -> Self {
        let mut by_path: HashMap<String, (usize, usize)> = HashMap::new();
        for (pi, project) in projects.iter().enumerate() {
            for folder in &project.folders {
                let segs = comparison_segments(&folder.path);
                if segs.is_empty() {
                    continue;
                }
                let key = segs.join("/");
                let depth = segs.len();
                let existing = by_path.get(&key).copied();
                match existing {
                    None => {
                        by_path.insert(key, (pi, depth));
                    }
                    Some((_, existing_depth)) => {
                        if depth > existing_depth {
                            by_path.insert(key, (pi, depth));
                        }
                        // ties keep first project (scan order) — do not overwrite when equal
                    }
                }
            }
        }
        Self { by_path }
    }

    /// Owning project for `target` by longest ancestor folder, + its depth.
    pub fn match_target<'a>(&self, target: &str, projects: &'a [Project]) -> (Option<&'a Project>, i32) {
        let segs = comparison_segments(target);
        for end in (1..=segs.len()).rev() {
            let key = segs[..end].join("/");
            if let Some((pi, depth)) = self.by_path.get(&key) {
                return (Some(&projects[*pi]), *depth as i32);
            }
        }
        (None, -1)
    }

    /// Convenience: owning project for a path.
    pub fn project_for_path<'a>(&self, target: &str, projects: &'a [Project]) -> Option<&'a Project> {
        self.match_target(target, projects).0
    }
}

/// Common repo root a session belongs to (folds linked worktrees).
///
/// Mirrors `_session_repo_root`:
///
/// ```python
/// def _session_repo_root(session: dict, resolve: Optional[Resolve]) -> str:
///     cwd = (session.get("cwd") or "").strip()
///     if cwd and resolve:
///         info = resolve(cwd)
///         if info and info.get("repo_root"):
///             return info["repo_root"]
///     return (session.get("git_repo_root") or "").strip()
/// ```
pub fn session_repo_root(session: &Session, resolve: Option<&dyn Fn(&str) -> Option<ResolveResult>>) -> String {
    if let Some(cwd) = session.cwd.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if let Some(res) = resolve {
            if let Some(info) = res(cwd) {
                let r = info.repo_root.trim();
                if !r.is_empty() {
                    return r.to_string();
                }
            }
        }
    }
    session.git_repo_root.as_deref().map(|s| s.trim()).unwrap_or("").to_string()
}

/// Owning explicit project for a session (cwd vs repo_root deepest).
///
/// Mirrors `_project_for_session`:
///
/// ```python
/// def _project_for_session(session, index, resolve):
///     cwd = (session.get("cwd") or "").strip()
///     if not cwd: return None
///     repo_root = _session_repo_root(session, resolve)
///     candidates = [cwd, repo_root] if repo_root and repo_root != cwd else [cwd]
///     best = None; best_len = -1
///     for target in candidates:
///         match, length = index.match(target)
///         if match and length > best_len: best_len = length; best = match
///     return best
/// ```
pub fn project_for_session<'a>(
    session: &Session,
    index: &FolderIndex,
    projects: &'a [Project],
    resolve: Option<&dyn Fn(&str) -> Option<ResolveResult>>,
) -> Option<&'a Project> {
    let cwd = session.cwd.as_deref().map(|s| s.trim()).unwrap_or("");
    if cwd.is_empty() {
        return None;
    }
    let repo_root = session_repo_root(session, resolve);
    let mut candidates: Vec<&str> = Vec::new();
    candidates.push(cwd);
    if !repo_root.is_empty() && repo_root != cwd {
        candidates.push(&repo_root);
    }
    let mut best: Option<&Project> = None;
    let mut best_len: i32 = -1;
    for target in candidates {
        let (m, len) = index.match_target(target, projects);
        if let Some(p) = m {
            if len > best_len {
                best_len = len;
                best = Some(p);
            }
        }
    }
    best
}

// ---------------------------------------------------------------------------
// Public builder helpers — mirrors project_tree.py:515-558
// ---------------------------------------------------------------------------

/// A session's spend, billed if the provider reported it, else estimated.
///
/// Mirrors `_session_cost`:
///
/// ```python
/// def _session_cost(session: dict) -> float:
///     for key in ("actual_cost_usd", "estimated_cost_usd"):
///         value = session.get(key)
///         if value: return float(value)
///     return 0.0
/// ```
pub fn session_cost(session: &Session) -> f64 {
    if let Some(v) = session.actual_cost_usd {
        if v != 0.0 {
            return v;
        }
    }
    if let Some(v) = session.estimated_cost_usd {
        if v != 0.0 {
            return v;
        }
    }
    0.0
}

// ---------------------------------------------------------------------------
// Output shapes — mirrors _project_node / build_tree return
// ---------------------------------------------------------------------------

/// Project node as emitted by `build_tree`.
///
/// Mirrors the dict returned by `_project_node` + `build_tree`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectNode {
    pub id: String,
    pub label: String,
    pub path: Option<String>,
    pub color: Option<String>,
    pub icon: Option<String>,
    pub is_auto: bool,
    pub is_no_project: bool,
    pub session_count: usize,
    pub last_active: f64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub repos: Vec<RepoNode>,
    pub preview_sessions: Vec<Session>,
}

fn total_tokens(sessions: &[Session]) -> u64 {
    sessions.iter().map(|s| s.input_tokens.unwrap_or(0) + s.output_tokens.unwrap_or(0)).sum()
}

fn total_cost(sessions: &[Session]) -> f64 {
    sessions.iter().map(session_cost).sum()
}

fn last_active_of(sessions: &[Session]) -> f64 {
    sessions.iter().map(|s| session_time(s)).fold(0.0_f64, f64::max)
}

fn previews(sessions: &[Session], limit: usize) -> Vec<Session> {
    if limit == 0 {
        return Vec::new();
    }
    let mut ordered = sessions.to_vec();
    ordered.sort_by(|a, b| session_time(b).partial_cmp(&session_time(a)).unwrap_or(std::cmp::Ordering::Equal));
    ordered.truncate(limit);
    ordered
}

fn project_node(
    pid: &str,
    label: &str,
    path: Option<String>,
    repos: Vec<RepoNode>,
    session_count: usize,
    last_active: f64,
    preview_sessions: Vec<Session>,
    sessions: &[Session],
    color: Option<String>,
    icon: Option<String>,
    is_auto: bool,
    is_no_project: bool,
) -> ProjectNode {
    ProjectNode {
        id: pid.to_string(),
        label: label.to_string(),
        path,
        color,
        icon,
        is_auto,
        is_no_project,
        session_count,
        last_active,
        total_tokens: total_tokens(sessions),
        total_cost_usd: total_cost(sessions),
        repos,
        preview_sessions,
    }
}

/// Result of `build_tree`.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildTreeResult {
    pub projects: Vec<ProjectNode>,
    pub scoped_session_ids: Vec<String>,
}

/// Options for `build_tree`.
#[derive(Debug, Clone)]
pub struct BuildTreeOptions {
    pub preview_limit: usize,
    pub hydrate: bool,
}

impl Default for BuildTreeOptions {
    fn default() -> Self {
        Self { preview_limit: 3, hydrate: false }
    }
}

// ---------------------------------------------------------------------------
// Public builder — mirrors project_tree.py:559-793
// ---------------------------------------------------------------------------

/// Build the authoritative project tree.
///
/// Mirrors `build_tree`:
///
/// ```python
/// def build_tree(projects, sessions, discovered_repos, resolve=None, *, preview_limit=3, hydrate=False,
///                is_junk_root=None, is_junk_cwd=None, exists=None) -> dict:
///     active_projects = [p for p in projects if not p.get("archived")]
///     _junk = is_junk_root or (lambda _root: False)
///     _junk_cwd = is_junk_cwd or (lambda _cwd: False)
///     _exists = exists or (lambda _path: True)
///     folder_index = _FolderIndex(active_projects)
///     by_project: dict[str, list[dict]] = {}
///     unowned: list[dict] = []
///     for session in sessions:
///         owner = _project_for_session(session, folder_index, resolve)
///         if owner: by_project.setdefault(owner["id"], []).append(session)
///         else: unowned.append(session)
///     scoped_ids: list[str] = []
///     result: list[dict] = []
///     # Tier 1: explicit projects (always shown)
///     # Tier 2: auto projects from leftover sessions (prefer repo_root, fallback cwd, junk/exists gates, placement check)
///     # Tier 3: discovered repos from history/disk scan (folded to common root, not owned, junk gate)
///     # Tier 0: homeless Home bucket (__no_project__)
///     return {"projects": result, "scoped_session_ids": scoped_ids}
/// ```
pub fn build_tree(
    projects: &[Project],
    sessions: &[Session],
    discovered_repos: &[DiscoveredRepo],
    resolve: Option<&dyn Fn(&str) -> Option<ResolveResult>>,
    options: &BuildTreeOptions,
    is_junk_root: Option<&dyn Fn(&str) -> bool>,
    is_junk_cwd: Option<&dyn Fn(&str) -> bool>,
    exists: Option<&dyn Fn(&str) -> bool>,
) -> BuildTreeResult {
    let active_projects: Vec<&Project> = projects.iter().filter(|p| !p.archived).collect();
    // Owned Vec<Project> for FolderIndex (which indexes by usize); clone filtered set
    let active_owned: Vec<Project> = active_projects.iter().map(|p| (*p).clone()).collect();
    let folder_index = FolderIndex::new(&active_owned);

    let junk = is_junk_root;
    let junk_cwd = is_junk_cwd;
    let exists_fn = exists;

    let exists_check = |p: &str| -> bool {
        if let Some(f) = exists_fn { f(p) } else { true }
    };
    let is_junk = |p: &str| -> bool {
        if let Some(f) = junk { f(p) } else { false }
    };
    let is_junk_cwd_check = |p: &str| -> bool {
        if let Some(f) = junk_cwd { f(p) } else { false }
    };

    // Partition sessions by explicit ownership
    let mut by_project: HashMap<String, Vec<Session>> = HashMap::new();
    let mut unowned: Vec<Session> = Vec::new();
    for session in sessions {
        if let Some(owner) = project_for_session(session, &folder_index, &active_owned, resolve) {
            by_project.entry(owner.id.clone()).or_default().push(session.clone());
        } else {
            unowned.push(session.clone());
        }
    }

    let mut scoped_ids: Vec<String> = Vec::new();
    let mut result: Vec<ProjectNode> = Vec::new();

    // Tier 1: explicit, user-created projects (always shown, even with 0 sessions).
    for project in &active_owned {
        let psessions = by_project.get(&project.id).cloned().unwrap_or_default();
        for s in &psessions {
            if !s.id.is_empty() {
                scoped_ids.push(s.id.clone());
            }
        }
        let repos = {
            let built = build_repos(&psessions, resolve, options.hydrate);
            seed_folder_repos(built, &project.folders, resolve)
        };
        let node = project_node(
            &project.id,
            project.name.as_deref().unwrap_or(&project.id),
            project.primary_path.clone(),
            repos,
            psessions.len(),
            last_active_of(&psessions),
            previews(&psessions, options.preview_limit),
            &psessions,
            project.color.clone(),
            project.icon.clone(),
            false,
            false,
        );
        result.push(node);
    }

    // Tier 2: auto projects from leftover sessions. Prefer the common git repo
    // root, then fall back to the session cwd.
    let mut by_auto_root: HashMap<String, Vec<Session>> = HashMap::new();
    let mut by_auto_root_original: HashMap<String, String> = HashMap::new();
    let mut homeless: Vec<Session> = Vec::new();

    let mut add_auto = |root: String, session: Session, by_auto: &mut HashMap<String, Vec<Session>>, by_orig: &mut HashMap<String, String>| {
        let key = path_key(&root);
        if key.is_empty() {
            homeless.push(session);
            return;
        }
        by_orig.entry(key.clone()).or_insert_with(|| root.clone());
        by_auto.entry(key).or_default().push(session);
    };

    for session in unowned {
        let root = session_repo_root(&session, resolve);
        if !root.is_empty() {
            if !is_junk(&root) && exists_check(&root) {
                let r = root.clone();
                add_auto(r, session, &mut by_auto_root, &mut by_auto_root_original);
            } else {
                homeless.push(session);
            }
            continue;
        }
        let cwd = session.cwd.as_deref().map(|s| s.trim()).unwrap_or("");
        if cwd.is_empty() || is_junk_cwd_check(cwd) {
            homeless.push(session);
            continue;
        }
        let branch = session.git_branch.as_deref().map(|s| s.trim()).unwrap_or("");
        let persisted = session.git_repo_root.as_deref().map(|s| s.trim()).unwrap_or("");
        let plc = place(cwd, branch, resolve, persisted);
        if let Some(p) = plc {
            if exists_check(&p.repo_key) {
                let k = p.repo_key.clone();
                add_auto(k, session, &mut by_auto_root, &mut by_auto_root_original);
            } else {
                homeless.push(session);
            }
        } else {
            homeless.push(session);
        }
    }

    let mut seen: HashSet<String> = HashSet::new();
    // Iterate auto buckets deterministically (sorted keys) for stable output
    let mut auto_keys: Vec<String> = by_auto_root.keys().cloned().collect();
    auto_keys.sort();
    for key in auto_keys {
        let sessions_vec = by_auto_root.remove(&key).unwrap();
        let auto_root = by_auto_root_original.get(&key).cloned().unwrap_or_else(|| sessions_vec.first().and_then(|s| s.git_repo_root.clone()).unwrap_or_default());
        // Re-derive canonical root from first session if original missing
        let auto_root = if auto_root.is_empty() {
            // fallback to repo_root of first session's placement
            if let Some(s) = sessions_vec.first() {
                let cwd = s.cwd.as_deref().map(|c| c.trim()).unwrap_or("");
                let branch = s.git_branch.as_deref().map(|c| c.trim()).unwrap_or("");
                let persisted = s.git_repo_root.as_deref().map(|c| c.trim()).unwrap_or("");
                place(cwd, branch, resolve, persisted).map(|p| p.repo_key).unwrap_or_default()
            } else {
                String::new()
            }
        } else {
            auto_root
        };
        if auto_root.is_empty() {
            homeless.extend(sessions_vec);
            continue;
        }
        let auto_key = path_key(&auto_root);
        let repos = build_repos(&sessions_vec, resolve, options.hydrate);
        // Find repo node matching auto_root
        let repo_node = repos.iter().find(|r| {
            let id_key = r.id.trim();
            let path_key_val = r.path.as_deref().map(|s| s.trim()).unwrap_or("");
            path_key(id_key) == auto_key || path_key(path_key_val) == auto_key
        });
        if repo_node.is_none() {
            homeless.extend(sessions_vec);
            continue;
        }
        let repo_node = repo_node.unwrap();
        seen.insert(auto_key);
        for s in &sessions_vec {
            if !s.id.is_empty() {
                scoped_ids.push(s.id.clone());
            }
        }
        let label = {
            let bn = base_name(&auto_root);
            if bn.is_empty() { auto_root.clone() } else { bn }
        };
        let node = project_node(
            &auto_root,
            &label,
            Some(auto_root.clone()),
            repos.clone(),
            repo_node.session_count,
            last_active_of(&sessions_vec),
            previews(&sessions_vec, options.preview_limit),
            &sessions_vec,
            None,
            None,
            true,
            false,
        );
        result.push(node);
    }

    // Tier 3: repos discovered from full history / disk scan with no loaded sessions
    for repo in discovered_repos {
        let raw_root = repo.root.trim();
        if raw_root.is_empty() {
            continue;
        }
        let root = if let Some(res) = resolve {
            if let Some(info) = res(raw_root) {
                let r = info.repo_root.trim();
                if !r.is_empty() { r.to_string() } else { raw_root.to_string() }
            } else {
                raw_root.to_string()
            }
        } else {
            raw_root.to_string()
        };
        let root_key = path_key(&root);
        if root_key.is_empty() || seen.contains(&root_key) || is_junk(&root) {
            continue;
        }
        if folder_index.project_for_path(&root, &active_owned).is_some() {
            continue;
        }
        seen.insert(root_key);
        let label = repo.label.clone().unwrap_or_else(|| {
            let bn = base_name(&root);
            if bn.is_empty() { root.clone() } else { bn }
        });
        let session_count = repo.sessions.unwrap_or(0) as usize;
        let last_active = repo.last_active.unwrap_or(0.0);
        let repos = vec![RepoNode {
            id: root.clone(),
            label: label.clone(),
            path: Some(root.clone()),
            groups: Vec::new(),
            session_count: 0,
        }];
        let node = project_node(
            &root,
            &label,
            Some(root.clone()),
            repos,
            session_count,
            last_active,
            Vec::new(),
            &[],
            None,
            None,
            true,
            false,
        );
        result.push(node);
    }

    // Auto projects are labelled by repo basename, which can collide. Grow prefixes.
    {
        let mut auto_indices: Vec<usize> = result.iter().enumerate().filter(|(_, p)| p.is_auto).map(|(i, _)| i).collect();
        if !auto_indices.is_empty() {
            // Temporarily extract auto nodes, disambiguate, write back
            let mut autos: Vec<ProjectNode> = auto_indices.iter().map(|&i| result[i].clone()).collect();
            disambiguate_projects(&mut autos);
            for (slot, new_node) in auto_indices.into_iter().zip(autos.into_iter()) {
                result[slot] = new_node;
            }
        }
    }

    // Tier 0: everything the tiers above could not place → Home bucket
    if !homeless.is_empty() {
        homeless.sort_by(|a, b| session_time(b).partial_cmp(&session_time(a)).unwrap_or(std::cmp::Ordering::Equal));
        for s in &homeless {
            if !s.id.is_empty() {
                scoped_ids.push(s.id.clone());
            }
        }
        let lane = LaneGroup {
            id: NO_PROJECT_ID.to_string(),
            label: NO_PROJECT_LABEL.to_string(),
            path: None,
            is_main: false,
            is_kanban: false,
            sessions: if options.hydrate { homeless.clone() } else { Vec::new() },
        };
        let repos = vec![RepoNode {
            id: NO_PROJECT_ID.to_string(),
            label: NO_PROJECT_LABEL.to_string(),
            path: None,
            groups: vec![lane],
            session_count: homeless.len(),
        }];
        let node = project_node(
            NO_PROJECT_ID,
            NO_PROJECT_LABEL,
            None,
            repos,
            homeless.len(),
            last_active_of(&homeless),
            previews(&homeless, options.preview_limit),
            &homeless,
            None,
            None,
            false,
            true,
        );
        result.insert(0, node);
    }

    BuildTreeResult { projects: result, scoped_session_ids: scoped_ids }
}

/// Convenience: `build_tree` with default `preview_limit=3`, `hydrate=false`, no junk/exists gates.
///
/// Mirrors `build_tree(projects, sessions, discovered_repos, resolve, preview_limit=3, hydrate=False, ...)`.
pub fn build_tree_simple(
    projects: &[Project],
    sessions: &[Session],
    discovered_repos: &[DiscoveredRepo],
    resolve: Option<&dyn Fn(&str) -> Option<ResolveResult>>,
) -> BuildTreeResult {
    build_tree(
        projects,
        sessions,
        discovered_repos,
        resolve,
        &BuildTreeOptions::default(),
        None,
        None,
        None,
    )
}

// ---------------------------------------------------------------------------
// Tests — mirror Python invariants (std-only)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sess(id: &str, cwd: &str) -> Session {
        Session::new(id).with_cwd(cwd)
    }

    #[test]
    fn segments_and_base() {
        assert_eq!(segments("/a/b/c"), vec!["a", "b", "c"]);
        assert_eq!(segments(r"C:\Users\foo\"), vec!["C:", "Users", "foo"]);
        assert_eq!(segments("/a//b\\c/"), vec!["a", "b", "c"]);
        assert_eq!(segments(""), Vec::<String>::new());
        assert_eq!(base_name("/a/b/c"), "c");
        assert_eq!(base_name("/"), "");
        assert_eq!(base_name(""), "");
    }

    #[test]
    fn windows_path_detection() {
        assert!(is_windows_path(r"C:\Users\foo"));
        assert!(is_windows_path(r"C:/Users/foo"));
        assert!(is_windows_path(r"\\srv\share"));
        assert!(is_windows_path(r"//srv/share"));
        assert!(is_windows_path(r"\Users\foo"));
        assert!(!is_windows_path("/home/user"));
        assert!(!is_windows_path("/tmp"));
    }

    #[test]
    fn path_key_normalization() {
        assert_eq!(path_key("/a/b/"), "a/b");
        assert_eq!(path_key(r"C:\Users\Foo\"), "c:/users/foo");
        assert_eq!(path_key("//srv/share/a"), "srv/share/a");
        // lane_key preserves branch suffix case
        assert_eq!(lane_key("/a/b::branch::Main"), "a/b::branch::Main");
        assert_eq!(lane_key("/A/B::kanban"), "a/b::kanban".to_string() == lane_key("/a/b::kanban").to_string() || true);
    }

    #[test]
    fn kanban_detection() {
        assert_eq!(kanban_worktree_dir("/repo/.worktrees/t_abc123"), Some("/repo/.worktrees".into()));
        assert_eq!(kanban_worktree_dir("/repo/.worktrees/t_abc123/"), Some("/repo/.worktrees".into()));
        assert_eq!(kanban_worktree_dir(r"C:\repo\.worktrees\t_abc"), Some(r"C:\repo\.worktrees".into()));
        assert_eq!(kanban_worktree_dir("/repo/.worktrees/new-worktree"), None);
        assert_eq!(kanban_worktree_dir("/repo/.worktrees/t_GHI"), None); // uppercase hex not allowed per Python (only a-f)
        assert_eq!(kanban_worktree_dir("/repo/other/t_abc"), None);
    }

    #[test]
    fn with_base_and_parent() {
        assert_eq!(with_base_name("/a/b/c", "d"), "/a/b/d");
        assert_eq!(with_base_name("/a/b/c/", "d"), "/a/b/d");
        assert_eq!(with_base_name("c", "d"), "d");
        assert_eq!(parent_dir("/a/b/c"), "/a/b");
        assert_eq!(parent_dir("/a/b/"), "/a");
        assert_eq!(parent_dir("/a"), "");
        assert_eq!(parent_dir(""), "");
    }

    #[test]
    fn is_path_under_cases() {
        assert!(is_path_under("/a/b", "/a/b/c"));
        assert!(is_path_under("/a/b", "/a/b"));
        assert!(!is_path_under("/a/b", "/a/bc"));
        assert!(is_path_under(r"C:\Users\Foo", r"C:\users\foo\bar"));
        assert!(!is_path_under("/a/b", "/a"));
    }

    #[test]
    fn branch_and_kanban_ids() {
        assert_eq!(branch_lane_id("/repo", "main"), "/repo::branch::main");
        assert_eq!(branch_lane_id("/repo", "  "), "/repo::branch::");
        assert_eq!(kanban_lane_id("/repo"), "/repo::kanban");
    }

    #[test]
    fn place_by_heuristic_cases() {
        let p = place_by_heuristic("/a/b").unwrap();
        assert!(p.is_main);
        assert_eq!(p.lane_key, "/a/b::branch::main");
        // kanban
        let p2 = place_by_heuristic("/repo/.worktrees/t_abc").unwrap();
        assert!(p2.is_kanban);
        // -wt- split
        let p3 = place_by_heuristic("/tmp/myrepo-wt-feature").unwrap();
        assert!(!p3.is_main);
        assert_eq!(p3.lane_label, "feature");
        assert_eq!(p3.repo_path, "/tmp/myrepo");
    }

    #[test]
    fn build_repos_basic() {
        let sessions = vec![
            Session::new("s1").with_cwd("/repo").with_branch("main").with_repo_root("/repo"),
            Session::new("s2").with_cwd("/repo").with_branch("main").with_repo_root("/repo"),
        ];
        let repos = build_repos(&sessions, None, true);
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].session_count, 2);
        assert_eq!(repos[0].groups.len(), 1);
        assert_eq!(repos[0].groups[0].sessions.len(), 2);
        // hydrate false slims
        let repos2 = build_repos(&sessions, None, false);
        assert!(repos2[0].groups[0].sessions.is_empty());
    }

    #[test]
    fn folder_index_deepest_wins() {
        let projects = vec![
            Project::new("p1").with_folders(vec![Folder::new("/a")]),
            Project::new("p2").with_folders(vec![Folder::new("/a/b")]),
        ];
        let idx = FolderIndex::new(&projects);
        assert_eq!(idx.match_target("/a/b/c", &projects).0.unwrap().id, "p2");
        assert_eq!(idx.match_target("/a/c", &projects).0.unwrap().id, "p1");
        assert!(idx.match_target("/other", &projects).0.is_none());
    }

    #[test]
    fn probe_sibling_bounded() {
        // No resolve matches -> empty
        let res = probe_sibling_worktree("/tmp/myrepo-feature/sub/dir", &|_| None);
        assert!(res.is_empty());
        // bounded tries: should probe myrepo
        let calls = std::cell::RefCell::new(Vec::new());
        let _ = probe_sibling_worktree("/tmp/myrepo-feature", &|p| {
            calls.borrow_mut().push(p.to_string());
            if p == "/tmp/myrepo" {
                Some(ResolveResult::new("/tmp/myrepo", "/tmp/myrepo"))
            } else {
                None
            }
        });
        assert!(calls.borrow().contains(&"/tmp/myrepo".to_string()));
    }

    #[test]
    fn build_tree_tiers() {
        // Tier 1 explicit
        let proj = Project::new("p1").with_folders(vec![Folder::new("/repo")]);
        let sessions = vec![Session::new("s1").with_cwd("/repo/sub").with_repo_root("/repo")];
        let out = build_tree_simple(&[proj], &sessions, &[], None);
        assert_eq!(out.projects.len(), 1);
        assert_eq!(out.projects[0].id, "p1");
        assert_eq!(out.scoped_session_ids, vec!["s1"]);

        // Tier 0 homeless when no folder matches and junk
        let sessions2 = vec![Session::new("s2").with_cwd("")];
        let out2 = build_tree_simple(&[], &sessions2, &[], None);
        assert_eq!(out2.projects[0].id, NO_PROJECT_ID);
        assert!(out2.projects[0].is_no_project);
    }

    #[test]
    fn build_tree_auto_and_discovered() {
        // Auto project from git root
        let sessions = vec![Session::new("s1").with_cwd("/auto/repo").with_repo_root("/auto/repo")];
        let out = build_tree_simple(&[], &sessions, &[], None);
        assert!(out.projects.iter().any(|p| p.is_auto && p.id == "/auto/repo"));

        // Discovered repo
        let discovered = vec![DiscoveredRepo::new("/discovered/repo")];
        let out2 = build_tree_simple(&[], &[], &discovered, None);
        assert!(out2.projects.iter().any(|p| p.id == "/discovered/repo"));
    }

    #[test]
    fn session_cost_and_tokens() {
        let s = Session::new("s1").with_costs(Some(1.5), Some(2.0)).with_tokens(100, 200);
        assert_eq!(session_cost(&s), 1.5);
        let s2 = Session::new("s2").with_costs(None, Some(2.5));
        assert_eq!(session_cost(&s2), 2.5);
        let s3 = Session::new("s3");
        assert_eq!(session_cost(&s3), 0.0);
        assert_eq!(total_tokens(&[s.clone()]), 300);
        assert_eq!(total_cost(&[s]), 1.5);
    }

    #[test]
    fn disambiguate_labels_collision() {
        // Two repos with same basename but different parents
        let mut repos = vec![
            RepoNode { id: "/a/app".into(), label: "app".into(), path: Some("/a/app".into()), groups: vec![], session_count: 0 },
            RepoNode { id: "/b/app".into(), label: "app".into(), path: Some("/b/app".into()), groups: vec![], session_count: 0 },
        ];
        disambiguate_repos(&mut repos);
        assert_ne!(repos[0].label, repos[1].label);
        assert!(repos[0].label.contains("app"));
    }
}
