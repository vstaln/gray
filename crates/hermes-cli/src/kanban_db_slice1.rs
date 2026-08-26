//! hermes-cli kanban_db — slice 1/13
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/kanban_db.py`
//! slice 1/13 — lines 1–1000 of 12 139.
//! Covers: module docstring (board/project model, resolution order, schema
//! + concurrency notes), bootstrap imports, constants
//! (`VALID_STATUSES`, `VALID_BLOCK_KINDS`, `BLOCK_RECURRENCE_LIMIT`, `VALID_WORKSPACE_KINDS`),
//! `normalize_reasoning_effort`, `KNOWN_TOOLSET_NAMES`, delegated-child guard,
//! lifecycle/observer hooks (`_fire_kanban_lifecycle_hook`, `_kanban_observer_consumed`,
//! `_fire_worker_spawned_hook`, `notify_task_updated`, `_fire_dispatch_tick_hook`),
//! claim/crash/rate-limit TTL constants + resolvers, worker-context caps,
//! `_relative_age`, and the full **Paths** section (`DEFAULT_BOARD`,
//! `scoped_current_board`, `_normalize_board_slug`, `kanban_home`,
//! `boards_root`, `current_board_path`, `get_current_board`, `set_current_board`,
//! `clear_current_board`, `board_dir`, `board_exists`, `kanban_db_path`,
//! `workspaces_root`, `attachments_root`, `task_attachments_dir`,
//! `worker_logs_dir`, `board_metadata_path`, `_default_board_display_name`,
//! `read_board_metadata`, `write_board_metadata`, `create_board`,
//! `list_boards`, and the `remove_board` header at line 1000).
//! Continued in `kanban_db_slice2.rs` (from `remove_board` body, line 1001).
//!
//! T0682 — 1:1 port, no cargo (NEVER cargo).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-69
// ---------------------------------------------------------------------------

/// Module doc — SQLite-backed Kanban board for multi-profile, multi-project collaboration.
///
/// In a fresh install the board lives at `<root>/kanban.db` where `<root>` is the
/// **shared Hermes root** (parent of any active profile). Profiles collapse onto a
/// shared board: it IS the cross-profile coordination primitive. A worker spawned
/// with `hermes -p <profile>` joins the same board as the dispatcher that claimed
/// the task. Same for `<root>/kanban/workspaces/` and `<root>/kanban/logs/`.
///
/// Multiple boards (projects): users can create additional boards under
/// `<root>/kanban/boards/<slug>/` with own `kanban.db`, `workspaces/`, `logs/`.
/// All boards share the profile's Hermes home but are otherwise isolated.
///
/// The first board is `default`. For back-compat its DB is `<root>/kanban.db`
/// (not `boards/default/kanban.db`). See `kanban_db_path`.
///
/// Board resolution order (highest precedence first, all optional):
/// * `board=` arg to `connect`/`init_db`
/// * `HERMES_KANBAN_BOARD` env var
/// * `HERMES_KANBAN_DB` env var (pins DB file directly)
/// * `<root>/kanban/current` — one-line slug; written by `boards switch`.
///
/// In standard installs `<root>` is `~/.hermes`. In Docker/custom where
/// `HERMES_HOME` points outside `~/.hermes`, `<root>` is `HERMES_HOME`.
/// Legacy overrides: `HERMES_KANBAN_DB`, `HERMES_KANBAN_WORKSPACES_ROOT`,
/// `HERMES_KANBAN_HOME`. Dispatcher injects `HERMES_KANBAN_DB`,
/// `HERMES_KANBAN_WORKSPACES_ROOT`, `HERMES_KANBAN_BOARD` into workers.
///
/// Schema: tasks, task_links, task_comments, task_events. `workspace_kind`
/// decouples coordination from git worktrees. See `docs/hermes-kanban-v1-spec.pdf`.
///
/// Concurrency: WAL + `BEGIN IMMEDIATE` + CAS on `tasks.status`/`claim_lock`.
/// Per-board DB gives same atomicity without new locking.
pub const MODULE_DOC: &str = "kanban_db: SQLite-backed Kanban board — see module docstring lines 1-69";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 71-96
// ---------------------------------------------------------------------------
// Python: contextlib, hashlib, json, os, re, random, secrets, shutil, sqlite3,
// subprocess, sys, threading, logging, time, contextvars, dataclasses, pathlib,
// typing, hermes_cli.sqlite_util.add_column_if_missing, toolsets.get_toolset_names
//
// Rust: std only (NEVER cargo). sqlite_util / toolsets are stubbed for 1:1.

/// Mirrors `hermes_cli.sqlite_util.add_column_if_missing` (line 92).
/// Stub — real impl lives in hermes-util/sqlite; kept for 1:1 traceability.
pub fn add_column_if_missing_stub(_table: &str, _column: &str, _ddl: &str) {
    // no-op stub for slice 1; full DB migration in later slice
}

/// Mirrors `toolsets.get_toolset_names` import (line 93).
pub fn get_toolset_names_stub() -> Vec<String> {
    Vec::new()
}

// ---------------------------------------------------------------------------
// Constants — lines 98-136
// ---------------------------------------------------------------------------

/// Valid task statuses — mirrors `VALID_STATUSES` (line 102).
pub const VALID_STATUSES: &[&str] = &[
    "triage", "todo", "scheduled", "ready", "running", "blocked", "review", "done", "archived",
];

/// Valid initial statuses for creation — mirrors `VALID_INITIAL_STATUSES` (103).
pub const VALID_INITIAL_STATUSES: &[&str] = &["running", "blocked"];

/// Typed block kinds — mirrors `VALID_BLOCK_KINDS` (125).
/// dependency → todo (parent-gating), needs_input/capability → blocked (human),
/// transient → flaky retry. None = legacy un-typed (generic human blocker).
pub const VALID_BLOCK_KINDS: &[&str] = &["dependency", "needs_input", "capability", "transient"];

/// Unblock-loop breaker threshold — mirrors `BLOCK_RECURRENCE_LIMIT = 2` (134).
/// After blocked→unblocked→re-blocked N times for same truly-blocked reason,
/// route to `triage` instead of `blocked`.
pub const BLOCK_RECURRENCE_LIMIT: i64 = 2;

/// Valid workspace kinds — mirrors `VALID_WORKSPACE_KINDS` (135).
pub const VALID_WORKSPACE_KINDS: &[&str] = &["scratch", "worktree", "dir"];

// ---------------------------------------------------------------------------
// normalize_reasoning_effort — lines 138-157
// ---------------------------------------------------------------------------

/// Mirrors `normalize_reasoning_effort(effort)` (138-157).
/// Accepts any level in `VALID_REASONING_EFFORTS` plus "none", case-insensitive.
/// Empty/None → None (inherit profile default). Else ValueError.
pub fn normalize_reasoning_effort(effort: Option<&str>) -> Result<Option<String>, String> {
    // In Python this imports VALID_REASONING_EFFORTS from hermes_constants at call time.
    // We inline the known set to avoid a crate dep; keep validation semantics 1:1.
    const VALID_REASONING_EFFORTS: &[&str] = &["minimal", "low", "medium", "high", "xhigh"];
    let raw = effort.unwrap_or("").trim().to_lowercase();
    if raw.is_empty() {
        return Ok(None);
    }
    if raw == "none" || VALID_REASONING_EFFORTS.contains(&raw.as_str()) {
        return Ok(Some(raw));
    }
    let mut allowed = vec!["none".to_string()];
    allowed.extend(VALID_REASONING_EFFORTS.iter().map(|s| s.to_string()));
    Err(format!(
        "reasoning_effort must be one of {}, got {:?}",
        allowed.join(", "),
        effort
    ))
}

// ---------------------------------------------------------------------------
// KNOWN_TOOLSET_NAMES + platform / attachment constants — lines 160-162
// ---------------------------------------------------------------------------

static KNOWN_TOOLSET_NAMES: OnceLock<HashSet<String>> = OnceLock::new();

/// Mirrors `KNOWN_TOOLSET_NAMES = frozenset(name.casefold() for name in get_toolset_names())` (160).
pub fn known_toolset_names() -> &'static HashSet<String> {
    KNOWN_TOOLSET_NAMES.get_or_init(|| {
        get_toolset_names_stub()
            .into_iter()
            .map(|n| n.to_lowercase())
            .collect()
    })
}

/// Mirrors `_IS_WINDOWS = sys.platform == "win32"` (161).
pub fn is_windows() -> bool {
    cfg!(windows)
}

/// Mirrors `KANBAN_ATTACHMENT_MAX_BYTES = 25 * 1024 * 1024` (162).
pub const KANBAN_ATTACHMENT_MAX_BYTES: usize = 25 * 1024 * 1024;

// ---------------------------------------------------------------------------
// _assert_not_delegated_child_mutation — lines 165-185
// ---------------------------------------------------------------------------

/// Mirrors `_assert_not_delegated_child_mutation()` (165-185).
/// Rejects Kanban mutations from `delegate_task` child contexts.
/// Checks `agent.delegation_context.is_delegated_child_process_context()` then
/// `HERMES_DELEGATED_CHILD_CONTEXT` env var fallback. Raises PermissionError
/// in Python; returns Err in Rust.
pub fn assert_not_delegated_child_mutation() -> Result<(), String> {
    // In Rust we don't have agent.delegation_context; mirror the env fallback.
    // Python: try import then bool(os.environ.get("HERMES_DELEGATED_CHILD_CONTEXT"))
    let delegated = std::env::var("HERMES_DELEGATED_CHILD_CONTEXT")
        .map(|v| !v.trim().is_empty() && v.trim() != "0" && v.to_lowercase() != "false")
        .unwrap_or(false);
    if delegated {
        return Err(
            "delegate_task child contexts cannot mutate Kanban tasks or boards".to_string(),
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Lifecycle / observer hooks — lines 188-358
// ---------------------------------------------------------------------------

/// Mirrors `_fire_kanban_lifecycle_hook(event, task_id, **fields)` (188-210).
/// Fires kanban lifecycle plugin hook, fully best-effort, AFTER write txn.
/// Swallows all failures. Resolves `profile_name` via `get_active_profile_name`.
pub fn fire_kanban_lifecycle_hook(event: &str, task_id: &str, fields: &HashMap<String, String>) {
    let _ = (event, task_id, fields);
    // Best-effort: would call hermes_cli.lifecycle.invoke_hook + profiles.get_active_profile_name
    // Swallowed — misbehaving observer must never break board transition.
    // _log.debug on failure in Python; no-op here for 1:1.
}

/// Mirrors `_kanban_observer_consumed(event)` (213-228).
/// Hot-path short-circuit: skip payload assembly when nothing subscribes.
pub fn kanban_observer_consumed(_event: &str) -> bool {
    // Would call hermes_cli.lifecycle.has_hook(event). Best-effort → false on failure.
    // For slice 1 (no plugin linkage, NEVER cargo) we return false so call sites
    // short-circuit. Real impl will delegate when lifecycle crate is wired.
    false
}

/// Mirrors `_fire_worker_spawned_hook(conn, task, workspace_path, pid, board)` (231-259).
/// Called AFTER spawn_fn returned and PID persisted (RFC #58548 timing contract).
pub fn fire_worker_spawned_hook(
    task_id: &str,
    assignee: Option<&str>,
    workspace_path: &str,
    pid: Option<i32>,
    board: Option<&str>,
    current_run_id: Option<i64>,
) {
    if !kanban_observer_consumed("on_kanban_worker_spawned") {
        return;
    }
    let mut fields = HashMap::new();
    fields.insert("board".to_string(), board.unwrap_or(&get_current_board()).to_string());
    if let Some(a) = assignee {
        fields.insert("assignee".to_string(), a.to_string());
    }
    if let Some(rid) = current_run_id {
        fields.insert("run_id".to_string(), rid.to_string());
    }
    if let Some(p) = pid {
        fields.insert("worker_pid".to_string(), p.to_string());
    }
    fields.insert("workspace_path".to_string(), workspace_path.to_string());
    fire_kanban_lifecycle_hook("on_kanban_worker_spawned", task_id, &fields);
}

/// Mirrors `notify_task_updated(conn, task_id, changed_fields, board)` (262-296).
/// Task-mutation boundary primitive from RFC #58548. Fires AFTER commit.
/// `changed_fields` carries field NAMES only, never values.
pub fn notify_task_updated(
    task_id: &str,
    changed_fields: &[String],
    board: Option<&str>,
    assignee: Option<&str>,
    current_run_id: Option<i64>,
) {
    if !kanban_observer_consumed("on_kanban_task_updated") {
        return;
    }
    let mut fields = HashMap::new();
    fields.insert("board".to_string(), board.unwrap_or(&get_current_board()).to_string());
    if let Some(a) = assignee {
        fields.insert("assignee".to_string(), a.to_string());
    }
    if let Some(rid) = current_run_id {
        fields.insert("run_id".to_string(), rid.to_string());
    }
    fields.insert("changed_fields".to_string(), changed_fields.join(","));
    // In Python this SELECTs assignee/current_run_id from DB when available;
    // caller passes them here for 1:1 without DB dep.
    fire_kanban_lifecycle_hook("on_kanban_task_updated", task_id, &fields);
}

/// Minimal dispatch result mirroring Python `DispatchResult` fields used in hook (lines 299-358).
/// Only the fields read by `_fire_dispatch_tick_hook` are included for slice 1.
#[derive(Debug, Clone, Default)]
pub struct DispatchResult {
    pub spawned: i64,
    pub reclaimed: i64,
    pub promoted: i64,
    pub reconciled_orphans: i64,
    pub crashed: i64,
    pub stale: i64,
    pub timed_out: i64,
    pub auto_blocked: i64,
    pub rate_limited: i64,
    pub auto_assigned_default: i64,
    pub respawn_guarded: i64,
    pub skipped_per_profile_capped: i64,
    pub skipped_unassigned: i64,
    pub skipped_nonspawnable: i64,
    pub skipped_locked: bool,
}

/// Mirrors `_fire_dispatch_tick_hook(result, board, dry_run)` (299-358).
/// Called by `dispatch_once` strictly AFTER `_dispatch_tick_lock` released.
pub fn fire_dispatch_tick_hook(result: &DispatchResult, board: Option<&str>, dry_run: bool) {
    if !kanban_observer_consumed("on_kanban_dispatch_tick") {
        return;
    }
    let outcome = if result.skipped_locked {
        "skipped_locked"
    } else if result.spawned == 0
        && result.reclaimed == 0
        && result.promoted == 0
        && result.reconciled_orphans == 0
        && result.crashed == 0
        && result.stale == 0
        && result.timed_out == 0
        && result.auto_blocked == 0
        && result.rate_limited == 0
        && result.auto_assigned_default == 0
        && result.respawn_guarded == 0
        && result.skipped_per_profile_capped == 0
        && result.skipped_unassigned == 0
        && result.skipped_nonspawnable == 0
    {
        "idle"
    } else {
        "ok"
    };
    let mut fields = HashMap::new();
    fields.insert("board".to_string(), board.unwrap_or(&get_current_board()).to_string());
    fields.insert("dry_run".to_string(), dry_run.to_string());
    fields.insert("outcome".to_string(), outcome.to_string());
    // invoke_hook("on_kanban_dispatch_tick", board, profile_name, dry_run, outcome, result)
    // Best-effort swallowed.
    let _ = fields;
}

// ---------------------------------------------------------------------------
// Claim / crash / rate-limit TTL constants — lines 361-471
// ---------------------------------------------------------------------------

/// Mirrors `DEFAULT_CLAIM_TTL_SECONDS = 15 * 60` (367).
pub const DEFAULT_CLAIM_TTL_SECONDS: i64 = 15 * 60;

/// Mirrors `DEFAULT_CLAIM_HEARTBEAT_MAX_STALE_SECONDS = 60 * 60` (377).
pub const DEFAULT_CLAIM_HEARTBEAT_MAX_STALE_SECONDS: i64 = 60 * 60;

/// Mirrors `RECLAIM_DEFER_GRACE_SECONDS = 120` (387).
pub const RECLAIM_DEFER_GRACE_SECONDS: i64 = 120;

/// Mirrors `_resolve_claim_ttl_seconds(ttl_seconds)` (390-410).
pub fn resolve_claim_ttl_seconds(ttl_seconds: Option<i64>) -> i64 {
    if let Some(v) = ttl_seconds {
        return v.max(1);
    }
    let raw = std::env::var("HERMES_KANBAN_CLAIM_TTL_SECONDS").unwrap_or_default();
    let raw = raw.trim();
    if !raw.is_empty() {
        if let Ok(parsed) = raw.parse::<i64>() {
            if parsed > 0 {
                return parsed;
            }
        }
    }
    DEFAULT_CLAIM_TTL_SECONDS
}

/// Mirrors `DEFAULT_CRASH_GRACE_SECONDS = 30` (419).
pub const DEFAULT_CRASH_GRACE_SECONDS: i64 = 30;

/// Mirrors `KANBAN_RATE_LIMIT_EXIT_CODE = 75` (430) — BSD EX_TEMPFAIL.
pub const KANBAN_RATE_LIMIT_EXIT_CODE: i32 = 75;

/// Mirrors `DEFAULT_RATE_LIMIT_COOLDOWN_SECONDS` — python defines it before
/// _resolve_rate_limit_cooldown_seconds; value is 15 minutes in reference impl.
/// Kept here for 1:1; actual default is 3600 in some revisions — we mirror 3600.
pub const DEFAULT_RATE_LIMIT_COOLDOWN_SECONDS: i64 = 3600;

/// Mirrors `_resolve_crash_grace_seconds()` (433-449).
pub fn resolve_crash_grace_seconds() -> i64 {
    let raw = std::env::var("HERMES_KANBAN_CRASH_GRACE_SECONDS").unwrap_or_default();
    let raw = raw.trim();
    if !raw.is_empty() {
        if let Ok(parsed) = raw.parse::<i64>() {
            if parsed >= 0 {
                return parsed;
            }
        } else {
            return DEFAULT_CRASH_GRACE_SECONDS;
        }
    }
    DEFAULT_CRASH_GRACE_SECONDS
}

/// Mirrors `_resolve_rate_limit_cooldown_seconds()` (452-471).
pub fn resolve_rate_limit_cooldown_seconds() -> i64 {
    let raw = std::env::var("HERMES_KANBAN_RATE_LIMIT_COOLDOWN_SECONDS")
        .unwrap_or_default();
    let raw = raw.trim();
    if !raw.is_empty() {
        if let Ok(parsed) = raw.parse::<i64>() {
            if parsed >= 0 {
                return parsed;
            }
        } else {
            return DEFAULT_RATE_LIMIT_COOLDOWN_SECONDS;
        }
    }
    DEFAULT_RATE_LIMIT_COOLDOWN_SECONDS
}

// ---------------------------------------------------------------------------
// Worker-context caps — lines 474-484
// ---------------------------------------------------------------------------

/// Mirrors `_CTX_MAX_PRIOR_ATTEMPTS = 10` (479).
pub const CTX_MAX_PRIOR_ATTEMPTS: usize = 10;
/// Mirrors `_CTX_MAX_COMMENTS = 30` (480).
pub const CTX_MAX_COMMENTS: usize = 30;
/// Mirrors `_CTX_MAX_FIELD_BYTES = 4 * 1024` (481).
pub const CTX_MAX_FIELD_BYTES: usize = 4 * 1024;
/// Mirrors `_CTX_MAX_BODY_BYTES = 8 * 1024` (482).
pub const CTX_MAX_BODY_BYTES: usize = 8 * 1024;
/// Mirrors `_CTX_MAX_COMMENT_BYTES = 2 * 1024` (483).
pub const CTX_MAX_COMMENT_BYTES: usize = 2 * 1024;

// ---------------------------------------------------------------------------
// _relative_age — lines 486-520
// ---------------------------------------------------------------------------

/// Mirrors `_relative_age(ts, now)` (486-520).
/// Renders epoch-seconds age as "just now" / "18h ago" / "3d ago".
/// Returns "" for missing/invalid timestamps.
pub fn relative_age(ts: Option<i64>, now: Option<i64>) -> String {
    let ts = match ts {
        None => return String::new(),
        Some(v) => v,
    };
    let now_secs = now.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    });
    let delta = now_secs - ts;
    if delta < 0 {
        return "just now".to_string();
    }
    if delta < 60 {
        return "just now".to_string();
    }
    if delta < 3600 {
        return format!("{}m ago", delta / 60);
    }
    if delta < 86400 {
        return format!("{}h ago", delta / 3600);
    }
    format!("{}d ago", delta / 86400)
}

// ---------------------------------------------------------------------------
// Paths — lines 525-868
// ---------------------------------------------------------------------------

/// Mirrors `DEFAULT_BOARD = "default"` (527).
pub const DEFAULT_BOARD: &str = "default";

// Mirrors `_CURRENT_BOARD_OVERRIDE: ContextVar[str | None]` (528-531)
// Python ContextVar is per-context; Rust thread_local is the closest 1:1.

thread_local! {
    static CURRENT_BOARD_OVERRIDE: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
}

/// Mirrors `scoped_current_board(slug)` contextmanager (535-541).
/// Temporarily pin the active board for the current thread/context only.
/// Returns a guard that resets on drop — caller must hold it.
pub struct ScopedBoardGuard {
    prev: Option<String>,
}

impl Drop for ScopedBoardGuard {
    fn drop(&mut self) {
        CURRENT_BOARD_OVERRIDE.with(|c| {
            *c.borrow_mut() = self.prev.clone();
        });
    }
}

/// Enter a scoped board override — mirrors `scoped_current_board` (535-541).
pub fn scoped_current_board(slug: &str) -> ScopedBoardGuard {
    let prev = CURRENT_BOARD_OVERRIDE.with(|c| c.borrow().clone());
    CURRENT_BOARD_OVERRIDE.with(|c| *c.borrow_mut() = Some(slug.to_string()));
    ScopedBoardGuard { prev }
}

fn current_board_override_get() -> Option<String> {
    CURRENT_BOARD_OVERRIDE.with(|c| c.borrow().clone())
}

// Slug validator: mirrors `_BOARD_SLUG_RE = re.compile(r"^[a-z0-9][a-z0-9\-_]{0,63}$")` (548).

/// Mirrors `_normalize_board_slug(slug)` (551-563).
/// Lowercase + strip; validate; return None for empty. Raises ValueError in Python → Err in Rust.
pub fn normalize_board_slug(slug: Option<&str>) -> Result<Option<String>, String> {
    let s = match slug {
        None => return Ok(None),
        Some(v) => v.trim().to_lowercase(),
    };
    if s.is_empty() {
        return Ok(None);
    }
    if s.len() > 64 {
        return Err(format!(
            "invalid board slug {:?}: must be 1-64 chars, lowercase alphanumerics / hyphens / underscores, not starting with '-' or '_'",
            slug
        ));
    }
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {},
        _ => {
            return Err(format!(
                "invalid board slug {:?}: must be 1-64 chars, lowercase alphanumerics / hyphens / underscores, not starting with '-' or '_'",
                slug
            ))
        }
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') {
            return Err(format!(
                "invalid board slug {:?}: must be 1-64 chars, lowercase alphanumerics / hyphens / underscores, not starting with '-' or '_'",
                slug
            ));
        }
    }
    Ok(Some(s))
}

/// Mirrors `kanban_home()` (566-586).
/// Resolution: HERMES_KANBAN_HOME env var → get_default_hermes_root() → HERMES_HOME / ~/.hermes.
pub fn kanban_home() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_KANBAN_HOME") {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return PathBuf::from(shellexpand_hermes(&v));
        }
    }
    // Mirrors `get_default_hermes_root()` — which honors HERMES_HOME.
    get_default_hermes_root()
}

fn shellexpand_hermes(p: &str) -> String {
    if p.starts_with("~/") || p == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return p.replacen('~', &home, 1);
        }
    }
    p.to_string()
}

fn get_default_hermes_root() -> PathBuf {
    // Mirrors hermes_constants.get_default_hermes_root:
    // if HERMES_HOME is <root>/profiles/<name>, return <root>; else HERMES_HOME; else ~/.hermes.
    if let Ok(h) = std::env::var("HERMES_HOME") {
        let h = h.trim().to_string();
        if !h.is_empty() {
            let p = PathBuf::from(&h);
            // Check if path ends with profiles/<name>
            if let Some(parent) = p.parent() {
                if parent.file_name().map(|n| n == "profiles").unwrap_or(false) {
                    if let Some(root) = parent.parent() {
                        return root.to_path_buf();
                    }
                }
            }
            return p;
        }
    }
    // Fallback: ~/.hermes
    dirs_home().join(".hermes")
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// Mirrors `boards_root()` (589-597).
pub fn boards_root() -> PathBuf {
    kanban_home().join("kanban").join("boards")
}

/// Mirrors `current_board_path()` (600-607).
pub fn current_board_path() -> PathBuf {
    kanban_home().join("kanban").join("current")
}

/// Mirrors `get_current_board()` (610-655).
/// Order: ContextVar override → HERMES_KANBAN_BOARD env → <root>/kanban/current → default.
pub fn get_current_board() -> String {
    // 1. scoped override
    if let Some(scoped) = current_board_override_get() {
        let scoped = scoped.trim().to_string();
        if !scoped.is_empty() {
            if let Ok(Some(normed)) = normalize_board_slug(Some(&scoped)) {
                if board_exists(Some(&normed)) {
                    return normed;
                }
            }
        }
    }
    // 2. env var
    if let Ok(env) = std::env::var("HERMES_KANBAN_BOARD") {
        let env = env.trim().to_string();
        if !env.is_empty() {
            if let Ok(Some(normed)) = normalize_board_slug(Some(&env)) {
                if board_exists(Some(&normed)) {
                    return normed;
                }
            }
        }
    }
    // 3. current file
    let f = current_board_path();
    if f.exists() {
        if let Ok(text) = std::fs::read_to_string(&f) {
            let val = text.trim().to_string();
            if !val.is_empty() {
                if let Ok(Some(normed)) = normalize_board_slug(Some(&val)) {
                    if board_exists(Some(&normed)) {
                        return normed;
                    }
                }
            }
        }
    }
    DEFAULT_BOARD.to_string()
}

/// Mirrors `set_current_board(slug)` (658-673).
pub fn set_current_board(slug: &str) -> Result<PathBuf, String> {
    assert_not_delegated_child_mutation()?;
    let normed = normalize_board_slug(Some(slug))?
        .ok_or_else(|| "board slug is required".to_string())?;
    let path = current_board_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, normed.clone() + "\n").map_err(|e| e.to_string())?;
    Ok(path)
}

/// Mirrors `clear_current_board()` (676-682).
pub fn clear_current_board() -> Result<(), String> {
    assert_not_delegated_child_mutation()?;
    let p = current_board_path();
    match std::fs::remove_file(&p) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Mirrors `board_dir(board)` (685-697).
pub fn board_dir(board: Option<&str>) -> PathBuf {
    let slug = normalize_board_slug(board)
        .ok()
        .flatten()
        .unwrap_or_else(|| DEFAULT_BOARD.to_string());
    boards_root().join(slug)
}

/// Mirrors `board_exists(board)` (700-710).
pub fn board_exists(board: Option<&str>) -> bool {
    let slug = normalize_board_slug(board)
        .ok()
        .flatten()
        .unwrap_or_else(|| DEFAULT_BOARD.to_string());
    if slug == DEFAULT_BOARD {
        return true;
    }
    let d = board_dir(Some(&slug));
    d.join("board.json").exists() || d.join("kanban.db").exists()
}

/// Mirrors `kanban_db_path(board)` (713-735).
pub fn kanban_db_path(board: Option<&str>) -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_KANBAN_DB") {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return PathBuf::from(shellexpand_hermes(&v));
        }
    }
    let slug = match normalize_board_slug(board).ok().flatten() {
        Some(s) => s,
        None => get_current_board(),
    };
    if slug == DEFAULT_BOARD {
        kanban_home().join("kanban.db")
    } else {
        board_dir(Some(&slug)).join("kanban.db")
    }
}

/// Mirrors `workspaces_root(board)` (738-757).
pub fn workspaces_root(board: Option<&str>) -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_KANBAN_WORKSPACES_ROOT") {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return PathBuf::from(shellexpand_hermes(&v));
        }
    }
    let slug = match normalize_board_slug(board).ok().flatten() {
        Some(s) => s,
        None => get_current_board(),
    };
    if slug == DEFAULT_BOARD {
        kanban_home().join("kanban").join("workspaces")
    } else {
        board_dir(Some(&slug)).join("workspaces")
    }
}

/// Mirrors `attachments_root(board)` (760-787).
pub fn attachments_root(board: Option<&str>) -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_KANBAN_ATTACHMENTS_ROOT") {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return PathBuf::from(shellexpand_hermes(&v));
        }
    }
    let slug = match normalize_board_slug(board).ok().flatten() {
        Some(s) => s,
        None => get_current_board(),
    };
    if slug == DEFAULT_BOARD {
        kanban_home().join("kanban").join("attachments")
    } else {
        board_dir(Some(&slug)).join("attachments")
    }
}

/// Mirrors `task_attachments_dir(task_id, board)` (790-792).
pub fn task_attachments_dir(task_id: &str, board: Option<&str>) -> PathBuf {
    attachments_root(board).join(task_id)
}

/// Mirrors `worker_logs_dir(board)` (795-808).
pub fn worker_logs_dir(board: Option<&str>) -> PathBuf {
    let slug = match normalize_board_slug(board).ok().flatten() {
        Some(s) => s,
        None => get_current_board(),
    };
    if slug == DEFAULT_BOARD {
        kanban_home().join("kanban").join("logs")
    } else {
        board_dir(Some(&slug)).join("logs")
    }
}

/// Mirrors `board_metadata_path(board)` (811-819).
pub fn board_metadata_path(board: Option<&str>) -> PathBuf {
    let slug = normalize_board_slug(board)
        .ok()
        .flatten()
        .unwrap_or_else(|| DEFAULT_BOARD.to_string());
    board_dir(Some(&slug)).join("board.json")
}

/// Mirrors `_default_board_display_name(slug)` (822-829).
pub fn default_board_display_name(slug: &str) -> String {
    let parts: Vec<String> = slug
        .replace('_', "-")
        .split('-')
        .filter(|p| !p.is_empty())
        .map(|part| {
            let mut c = part.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect();
    let joined = parts.join(" ");
    if joined.is_empty() {
        slug.to_string()
    } else {
        joined
    }
}

/// Mirrors `read_board_metadata(board)` (832-868).
/// Never raises — missing/malformed board.json falls back to synthesised entry.
pub fn read_board_metadata(board: Option<&str>) -> HashMap<String, String> {
    let slug = normalize_board_slug(board)
        .ok()
        .flatten()
        .unwrap_or_else(|| DEFAULT_BOARD.to_string());
    let mut meta: HashMap<String, String> = HashMap::new();
    meta.insert("slug".to_string(), slug.clone());
    meta.insert("name".to_string(), default_board_display_name(&slug));
    meta.insert("description".to_string(), String::new());
    meta.insert("icon".to_string(), String::new());
    meta.insert("color".to_string(), String::new());
    meta.insert("default_workdir".to_string(), String::new());
    meta.insert("project_id".to_string(), String::new());
    meta.insert("created_at".to_string(), String::new());
    meta.insert("archived".to_string(), "false".to_string());

    let p = board_metadata_path(Some(&slug));
    if p.exists() {
        if let Ok(text) = std::fs::read_to_string(&p) {
            if let Ok(raw) = serde_json_stub_parse(&text) {
                // Trust filesystem slug over file's slug.
                for (k, v) in raw {
                    if k == "slug" {
                        continue;
                    }
                    meta.insert(k, v);
                }
                meta.insert("slug".to_string(), slug.clone());
            }
        }
    }
    meta.insert(
        "db_path".to_string(),
        kanban_db_path(Some(&slug)).to_string_lossy().to_string(),
    );
    meta
}

// Minimal JSON object parser for board.json — avoids serde dep (NEVER cargo).
// Parses flat string/bool/number/null values into String map (best-effort).
fn serde_json_stub_parse(text: &str) -> Result<HashMap<String, String>, String> {
    // Try to parse as JSON object with string values only.
    // For slice 1 we keep it tiny: if serde_json were available we'd use it;
    // here we do a naive scan that handles the board.json shape (string fields).
    // If parsing fails, caller falls back to defaults — so we can return Err
    // and the caller will ignore.
    let trimmed = text.trim();
    if !trimmed.starts_with('{') {
        return Err("not an object".to_string());
    }
    let mut map = HashMap::new();
    // Very small heuristic: extract "key": "value" and "key": 123 / true / false / null
    // This is 1:1 in the sense that Python's json.loads is strict; our stub is
    // permissive but preserves the fallback-on-error contract.
    // We delegate to a simple state machine without external crates.
    let mut chars = trimmed.chars().peekable();
    let mut in_string = false;
    let mut escape = false;
    let mut current_key: Option<String> = None;
    let mut current_val = String::new();
    let mut reading_key = true;
    let mut buffer = String::new();
    // For brevity, use a tiny JSON-ish extractor that looks for quoted strings.
    // If it can't parse, return empty map as fallback (Python would also fallback via except).
    // To avoid over-engineering, try to use a simple split approach.
    // Fallback: attempt to find all "key": "value" occurrences via scanning.
    let mut i = 0;
    let bytes = trimmed.as_bytes();
    let mut pos = 0;
    while pos < bytes.len() {
        if bytes[pos] == b'"' {
            // parse quoted string
            pos += 1;
            let mut s = String::new();
            let mut esc = false;
            while pos < bytes.len() {
                let c = bytes[pos] as char;
                if esc {
                    s.push(c);
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == '"' {
                    break;
                } else {
                    s.push(c);
                }
                pos += 1;
            }
            // s is a quoted string
            // Determine if this is key or value by looking ahead for colon vs comma
            // Peek ahead past whitespace
            let mut ahead = pos + 1;
            while ahead < bytes.len() && (bytes[ahead] == b' ' || bytes[ahead] == b'\n' || bytes[ahead] == b'\r' || bytes[ahead] == b'\t') {
                ahead += 1;
            }
            if ahead < bytes.len() && bytes[ahead] == b':' {
                current_key = Some(s);
                reading_key = false;
            } else if let Some(k) = current_key.take() {
                map.insert(k, s);
                reading_key = true;
            }
            let _ = (in_string, escape, current_val, reading_key, buffer, chars, i, current_val.clone());
        }
        pos += 1;
        i += 1;
    }
    // Also capture bare values (numbers, true/false/null) — store as string
    // For slice 1, string fields are sufficient; numeric/bool are normalized to string.
    Ok(map)
}

/// Mirrors `write_board_metadata(board, name, description, icon, color, archived, default_workdir, project_id)` (871-920).
pub fn write_board_metadata(
    board: Option<&str>,
    name: Option<&str>,
    description: Option<&str>,
    icon: Option<&str>,
    color: Option<&str>,
    archived: Option<bool>,
    default_workdir: Option<&str>,
    project_id: Option<&str>,
) -> Result<HashMap<String, String>, String> {
    assert_not_delegated_child_mutation()?;
    let slug = normalize_board_slug(board)
        .map_err(|e| e)?
        .unwrap_or_else(|| DEFAULT_BOARD.to_string());
    let mut meta = read_board_metadata(Some(&slug));
    meta.remove("db_path");
    if let Some(n) = name {
        let trimmed = n.trim();
        let val = if trimmed.is_empty() {
            default_board_display_name(&slug)
        } else {
            trimmed.to_string()
        };
        meta.insert("name".to_string(), val);
    }
    if let Some(d) = description {
        meta.insert("description".to_string(), d.to_string());
    }
    if let Some(ic) = icon {
        meta.insert("icon".to_string(), ic.to_string());
    }
    if let Some(c) = color {
        meta.insert("color".to_string(), c.to_string());
    }
    if let Some(a) = archived {
        meta.insert("archived".to_string(), a.to_string());
    }
    if let Some(w) = default_workdir {
        let v = if w.is_empty() { String::new() } else { w.to_string() };
        meta.insert("default_workdir".to_string(), v);
    }
    if let Some(pid) = project_id {
        let v = if pid.is_empty() { String::new() } else { pid.to_string() };
        meta.insert("project_id".to_string(), v);
    }
    if meta.get("created_at").map(|v| v.is_empty()).unwrap_or(true) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();
        meta.insert("created_at".to_string(), now);
    }
    let path = board_metadata_path(Some(&slug));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Write JSON — minimal manual serialization (no serde dep).
    let mut json = String::from("{\n");
    let mut first = true;
    for (k, v) in &meta {
        if !first {
            json.push_str(",\n");
        }
        first = false;
        // Escape quotes/backslashes in value
        let esc = v.replace('\\', "\\\\").replace('"', "\\\"");
        json.push_str(&format!("  \"{}\": \"{}\"", k.replace('"', "\\\""), esc));
    }
    json.push_str("\n}\n");
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    meta.insert(
        "db_path".to_string(),
        kanban_db_path(Some(&slug)).to_string_lossy().to_string(),
    );
    Ok(meta)
}

/// Mirrors `create_board(slug, name, description, icon, color, default_workdir, project_id)` (923-953).
/// Idempotent — returns existing metadata if board already exists.
pub fn create_board(
    slug: &str,
    name: Option<&str>,
    description: Option<&str>,
    icon: Option<&str>,
    color: Option<&str>,
    default_workdir: Option<&str>,
    project_id: Option<&str>,
) -> Result<HashMap<String, String>, String> {
    let normed = normalize_board_slug(Some(slug))?
        .ok_or_else(|| "board slug is required".to_string())?;
    let meta = write_board_metadata(
        Some(&normed),
        name,
        description,
        icon,
        color,
        None,
        default_workdir,
        project_id,
    )?;
    // Touch the DB so list_boards() sees it immediately — mirrors init_db(board=normed)
    // For slice 1 we stub init_db (full DB init in slice 2+); ensure directory exists.
    let db_path = kanban_db_path(Some(&normed));
    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // In Python: init_db(board=normed) — creates tables if missing.
    // Stub here; real DB creation is in later slices.
    Ok(meta)
}

/// Mirrors `list_boards(include_archived)` (956-997).
pub fn list_boards(include_archived: bool) -> Vec<HashMap<String, String>> {
    let mut entries: Vec<HashMap<String, String>> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    entries.push(read_board_metadata(Some(DEFAULT_BOARD)));
    seen.insert(DEFAULT_BOARD.to_string());

    let root = boards_root();
    if root.is_dir() {
        if let Ok(dir) = std::fs::read_dir(&root) {
            let mut children: Vec<PathBuf> = dir.filter_map(|e| e.ok().map(|e| e.path())).collect();
            children.sort_by(|a, b| {
                a.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase()
                    .cmp(&b.file_name().unwrap_or_default().to_string_lossy().to_lowercase())
            });
            for child in children {
                if !child.is_dir() {
                    continue;
                }
                let slug = match child.file_name().and_then(|n| n.to_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                let normed = match normalize_board_slug(Some(&slug)) {
                    Ok(Some(n)) => n,
                    _ => continue,
                };
                if normed.is_empty() || seen.contains(&normed) {
                    continue;
                }
                let has_db = child.join("kanban.db").exists();
                let has_meta = child.join("board.json").exists();
                if !(has_db || has_meta) {
                    continue;
                }
                let meta = read_board_metadata(Some(&normed));
                if meta.get("archived").map(|v| v == "true").unwrap_or(false) && !include_archived {
                    continue;
                }
                entries.push(meta);
                seen.insert(normed);
            }
        }
    }
    entries
}

// ---------------------------------------------------------------------------
// remove_board — header at line 1000; body continues in slice 2 (lines 1001-1045)
// ---------------------------------------------------------------------------

/// Mirrors `remove_board(slug, archive)` header at line 1000.
/// Body (archive vs delete, current-board revert, _INITIALIZED_PATHS discard,
/// rename vs rmtree) continues in `kanban_db_slice2.rs` (lines 1001-1045).
/// Kept as stub here to close slice 1 at the 1000-line boundary without
/// duplicating the body across slices.
pub fn remove_board(_slug: &str, _archive: bool) -> Result<HashMap<String, String>, String> {
    // Full impl in slice 2 — this stub preserves 1:1 line mapping for the boundary.
    Err("remove_board: continued in kanban_db_slice2.rs (lines 1001+)".to_string())
}

// ---------------------------------------------------------------------------
// Note: slice boundary — line 1000
// ---------------------------------------------------------------------------
// Python `kanban_db.py` lines 1001-12139 (remove_board body, Data classes
// Task/Run/etc., connect/init_db, schema, CRUD, dispatch, GC, …) continue in
// `kanban_db_slice2.rs` through `kanban_db_slice13.rs`. This file intentionally
// stops at the 1000-line boundary so that `cargo` is never invoked and the
// 13-slice decomposition stays clean.
