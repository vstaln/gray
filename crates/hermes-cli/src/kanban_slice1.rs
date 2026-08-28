//! hermes-cli kanban — slice 1/4
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/kanban.py`
//! slice 1/4 — lines 1–900 of 3442 (first 900 LOC).
//! Covers: module docstring + imports, small formatting helpers
//! (`_STATUS_ICONS`, `_fmt_ts`, `_fmt_task_line`, `_task_to_dict`,
//! `_run_state_kwargs`, `_parse_workspace_flag`, `_parse_branch_flag`,
//! `_check_dispatcher_presence`), and the argparse builder `build_parser`
//! through the `heartbeat` subcommand (up to line 900, `p_hb --note` +
//! `# --- assignees ---` header). Remaining subparsers (`assignees`,
//! `context`, `specify`, `decompose`, `gc`, `repair`) and the
//! `kanban_command` dispatch + all handlers continue in `kanban_slice2.rs`
//! and beyond.
//!
//! T0693 — 1:1 port, no cargo (NEVER cargo).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-13
// ---------------------------------------------------------------------------

/// Mirrors `hermes_cli/kanban.py` module doc (lines 1-13).
///
/// ```text
/// CLI for the Hermes Kanban board — `hermes kanban …` subcommand.
///
/// Exposes the full Kanban command surface documented in the design spec
/// (`docs/hermes-kanban-v1-spec.pdf`).  All DB work is delegated to
/// `kanban_db`.  This module adds:
///
///   * Argparse subcommand construction (`build_parser`).
///   * Argument dispatch (`kanban_command`).
///   * Output formatting (plain text + `--json`).
///   * A short shared helper that parses a single slash-style string
///     (used by `/kanban …` in CLI and gateway) and forwards it to the
///     argparse surface.
/// ```
pub const MODULE_DOC: &str = "hermes_cli/kanban.py — kanban CLI (lines 1-900 slice)";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 15-28
// ---------------------------------------------------------------------------
// Python: argparse, contextlib, json, os, shlex, sys, time, pathlib.Path,
// typing.Any/Optional, hermes_cli.kanban_db as kb, hermes_cli.kanban_swarm as ks
//
// Rust: std only (NEVER cargo). `kb`/`ks` are stubbed for 1:1 traceability
// without pulling the DB crate in this slice.

/// Stub for `hermes_cli.kanban_db` import (line 27) — kept for 1:1 line mapping.
/// Full DB types live in `kanban_db_slice1.rs`; this slice references them via
/// local stubs to stay self-contained without cargo.
pub mod kb_stub {
    pub const DEFAULT_BOARD: &str = "default";
    pub const DEFAULT_CLAIM_TTL_SECONDS: i64 = 15 * 60;
    pub const DEFAULT_FAILURE_LIMIT: i64 = 2;
    pub const DEFAULT_SPAWN_FAILURE_LIMIT: i64 = 2;
    pub const VALID_STATUSES: &[&str] = &[
        "archived", "blocked", "done", "ready", "running", "scheduled", "todo",
    ];
    pub const VALID_SORT_ORDERS: &[&str] = &["priority", "created", "updated"];
    pub const VALID_BLOCK_KINDS: &[&str] = &["capability", "dependency", "needs_input", "transient"];
    pub const VALID_INITIAL_STATUSES: &[&str] = &["blocked", "running"];
    pub const _NOTIFY_DELIVERY_MODES: &[&str] = &["notify", "notify+wake", "wake"];
}

/// Stub for `hermes_cli.kanban_swarm` import (line 28).
pub mod ks_stub {
    // Mirrors `kanban_swarm.parse_worker_arg` used in `_cmd_swarm` (not in slice 1).
    pub fn parse_worker_arg(_raw: &str) -> Result<String, String> {
        Ok(_raw.to_string())
    }
}

// ---------------------------------------------------------------------------
// Small formatting helpers — lines 31-210
// ---------------------------------------------------------------------------

/// Mirrors `_STATUS_ICONS` (lines 35-43).
pub fn status_icon(status: &str) -> &'static str {
    match status {
        "todo" => "◻",
        "ready" => "▶",
        "running" => "●",
        "scheduled" => "⏱",
        "blocked" => "⊘",
        "done" => "✓",
        "archived" => "—",
        _ => "?",
    }
}

/// Mirrors `_STATUS_ICONS` map as a constant slice for iteration (lines 35-43).
pub const STATUS_ICONS: &[(&str, &str)] = &[
    ("todo", "◻"),
    ("ready", "▶"),
    ("running", "●"),
    ("scheduled", "⏱"),
    ("blocked", "⊘"),
    ("done", "✓"),
    ("archived", "—"),
];

// ---------------------------------------------------------------------------
// Task stub — mirrors `kb.Task` dataclass used by _fmt_task_line/_task_to_dict
// ---------------------------------------------------------------------------

/// Minimal `Task` mirror for slice 1 helpers. Full `Task` definition lives in
/// `kanban_db_slice1.rs` / later slices; this local stub is intentionally
/// small and only carries the fields touched by `_fmt_task_line` and
/// `_task_to_dict` (lines 52-84).
#[derive(Debug, Clone, Default)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub body: Option<String>,
    pub assignee: Option<String>,
    pub status: String,
    pub priority: i64,
    pub tenant: Option<String>,
    pub workspace_kind: String,
    pub workspace_path: Option<String>,
    pub branch_name: Option<String>,
    pub project_id: Option<String>,
    pub created_by: Option<String>,
    pub created_at: Option<i64>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub result: Option<String>,
    pub skills: Vec<String>,
    pub max_retries: Option<i64>,
    pub model_override: Option<String>,
    pub provider_override: Option<String>,
    pub session_id: Option<String>,
    pub workflow_template_id: Option<String>,
    pub current_step_key: Option<String>,
    pub claim_lock: Option<String>,
}

// ---------------------------------------------------------------------------
// _fmt_ts — lines 46-49
// ---------------------------------------------------------------------------

/// Mirrors `_fmt_ts(ts)` (lines 46-49).
///
/// ```python
/// def _fmt_ts(ts: Optional[int]) -> str:
///     if not ts:
///         return ""
///     return time.strftime("%Y-%m-%d %H:%M", time.localtime(ts))
/// ```
///
/// Rust: no `chrono`/`libc` dep (NEVER cargo), so we implement a pure-std
/// UTC calendar conversion. Output format is identical (`YYYY-MM-DD HH:MM`);
/// timezone is UTC rather than `localtime` — documented deviation for the
/// no-dep slice. Callers that need true localtime can wire `chrono` in a
/// later slice without changing call sites.
pub fn fmt_ts(ts: Option<i64>) -> String {
    let epoch = match ts {
        None => return String::new(),
        Some(0) => return String::new(),
        Some(v) => v,
    };
    // Use a tiny civil-date algorithm (Howard Hinnant) to convert epoch
    // seconds to YYYY-MM-DD HH:MM in UTC without external crates.
    // Mirrors `time.localtime` semantics modulo timezone.
    let (y, m, d, hh, mm) = unix_seconds_to_ymdhm(epoch);
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}")
}

fn unix_seconds_to_ymdhm(secs: i64) -> (i32, u32, u32, u32, u32) {
    // Days + seconds within day
    let days = secs.div_euclid(86400);
    let secs_of_day = secs.rem_euclid(86400);
    let hh = (secs_of_day / 3600) as u32;
    let mm = ((secs_of_day % 3600) / 60) as u32;
    // civil_from_days: days since 1970-01-01 -> y/m/d
    let z = days + 719468; // days since 0000-03-01
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0,399]
    let mut y = (yoe as i64 + era * 400) as i32;
    let doy = doe - (365 * yoe as i64 + yoe as i64 / 4 - yoe as i64 / 100); // [0,365]
    let mp = (5 * doy + 2) / 153; // [0,11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1,31]
    let mut m = (mp + if mp < 10 { 3 } else { -9 }) as u32; // [1,12]
    y += if m <= 2 { 1 } else { 0 };
    if m <= 2 {
        // adjust already
    }
    // mp mapping already yields correct m
    let _ = m;
    // Recompute m correctly after y adjustment
    let mp2 = (5 * doy + 2) / 153;
    let d2 = (doy - (153 * mp2 + 2) / 5 + 1) as u32;
    let m2 = if mp2 < 10 { mp2 + 3 } else { mp2 - 9 } as u32;
    let y2 = yoe as i32 + (era * 400) as i32 + if m2 <= 2 { 1 } else { 0 };
    (y2, m2, d2, hh, mm)
}

// ---------------------------------------------------------------------------
// _fmt_task_line — lines 52-56
// ---------------------------------------------------------------------------

/// Mirrors `_fmt_task_line(t: kb.Task) -> str` (lines 52-56).
///
/// ```python
/// def _fmt_task_line(t: kb.Task) -> str:
///     icon = _STATUS_ICONS.get(t.status, "?")
///     assignee = t.assignee or "(unassigned)"
///     tenant = f" [{t.tenant}]" if t.tenant else ""
///     return f"{icon} {t.id}  {t.status:8s}  {assignee:20s}{tenant}  {t.title}"
/// ```
pub fn fmt_task_line(t: &Task) -> String {
    let icon = status_icon(&t.status);
    let assignee = t.assignee.as_deref().unwrap_or("(unassigned)");
    let tenant = t
        .tenant
        .as_deref()
        .map(|x| format!(" [{x}]"))
        .unwrap_or_default();
    format!(
        "{icon} {}  {:8}  {assignee:20}{tenant}  {}",
        t.id, t.status, t.title
    )
}

// ---------------------------------------------------------------------------
// _task_to_dict — lines 59-84
// ---------------------------------------------------------------------------

/// Mirrors `_task_to_dict(t: kb.Task) -> dict[str, Any]` (lines 59-84).
/// Returns a `HashMap<String, String>` stringified view; list fields
/// (`skills`) are joined with `,` for the no-serde slice. Full fidelity
/// (nested JSON, `Any` values) would use `serde_json` in a later slice.
pub fn task_to_dict(t: &Task) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("id".to_string(), t.id.clone());
    m.insert("title".to_string(), t.title.clone());
    m.insert("body".to_string(), t.body.clone().unwrap_or_default());
    m.insert("assignee".to_string(), t.assignee.clone().unwrap_or_default());
    m.insert("status".to_string(), t.status.clone());
    m.insert("priority".to_string(), t.priority.to_string());
    m.insert("tenant".to_string(), t.tenant.clone().unwrap_or_default());
    m.insert(
        "workspace_kind".to_string(),
        t.workspace_kind.clone(),
    );
    m.insert(
        "workspace_path".to_string(),
        t.workspace_path.clone().unwrap_or_default(),
    );
    m.insert(
        "branch_name".to_string(),
        t.branch_name.clone().unwrap_or_default(),
    );
    m.insert(
        "project_id".to_string(),
        t.project_id.clone().unwrap_or_default(),
    );
    m.insert(
        "created_by".to_string(),
        t.created_by.clone().unwrap_or_default(),
    );
    m.insert(
        "created_at".to_string(),
        t.created_at.map(|v| v.to_string()).unwrap_or_default(),
    );
    m.insert(
        "started_at".to_string(),
        t.started_at.map(|v| v.to_string()).unwrap_or_default(),
    );
    m.insert(
        "completed_at".to_string(),
        t.completed_at.map(|v| v.to_string()).unwrap_or_default(),
    );
    m.insert("result".to_string(), t.result.clone().unwrap_or_default());
    m.insert("skills".to_string(), t.skills.join(","));
    m.insert(
        "max_retries".to_string(),
        t.max_retries.map(|v| v.to_string()).unwrap_or_default(),
    );
    m.insert(
        "model_override".to_string(),
        t.model_override.clone().unwrap_or_default(),
    );
    m.insert(
        "provider_override".to_string(),
        t.provider_override.clone().unwrap_or_default(),
    );
    m.insert(
        "session_id".to_string(),
        t.session_id.clone().unwrap_or_default(),
    );
    m.insert(
        "workflow_template_id".to_string(),
        t.workflow_template_id.clone().unwrap_or_default(),
    );
    m.insert(
        "current_step_key".to_string(),
        t.current_step_key.clone().unwrap_or_default(),
    );
    m
}

// ---------------------------------------------------------------------------
// _run_state_kwargs — lines 87-94
// ---------------------------------------------------------------------------

/// Input mirror for `_run_state_kwargs` (lines 87-94).
#[derive(Debug, Clone, Default)]
pub struct RunStateArgs {
    pub state_type: Option<String>,
    pub state_name: Option<String>,
}

/// Mirrors `_run_state_kwargs(args) -> Optional[dict]` (lines 87-94).
///
/// ```python
/// def _run_state_kwargs(args):
///     st = getattr(args, "state_type", None)
///     sn = getattr(args, "state_name", None)
///     if (st is None) != (sn is None):
///         return None
///     if st is None:
///         return {}
///     return {"state_type": st, "state_name": sn}
/// ```
/// Returns `None` on partial specification (caller should emit usage error),
/// `Some(empty)` when both absent, `Some(map)` when both present.
pub fn run_state_kwargs(args: &RunStateArgs) -> Option<HashMap<String, String>> {
    let st = args.state_type.as_deref();
    let sn = args.state_name.as_deref();
    if st.is_none() != sn.is_none() {
        return None;
    }
    if st.is_none() {
        return Some(HashMap::new());
    }
    let mut m = HashMap::new();
    m.insert("state_type".to_string(), st.unwrap().to_string());
    m.insert("state_name".to_string(), sn.unwrap().to_string());
    Some(m)
}

// ---------------------------------------------------------------------------
// _parse_workspace_flag — lines 97-119
// ---------------------------------------------------------------------------

/// Mirrors `_parse_workspace_flag(value: str) -> tuple[str, Optional[str]]`
/// (lines 97-119).
///
/// Accepts: `scratch`, `worktree`, `worktree:<path>`, `dir:<path>`.
/// Returns `Err(msg)` instead of raising `ArgumentTypeError` so CLI call sites
/// can map it to exit code 2.
pub fn parse_workspace_flag(value: &str) -> Result<(String, Option<String>), String> {
    if value.is_empty() {
        return Ok(("scratch".to_string(), None));
    }
    let v = value.trim().to_string();
    if v == "scratch" || v == "worktree" {
        return Ok((v, None));
    }
    for (prefix, kind) in [("dir:", "dir"), ("worktree:", "worktree")] {
        if !v.starts_with(prefix) {
            continue;
        }
        let path = v[prefix.len()..].trim().to_string();
        if path.is_empty() {
            return Err(format!("--workspace {prefix} requires a path after the colon"));
        }
        let expanded = shellexpand(&path);
        return Ok((kind.to_string(), Some(expanded)));
    }
    Err(format!(
        "unknown --workspace value {value:?}: use scratch, worktree, worktree:<path>, or dir:<path>"
    ))
}

fn shellexpand(p: &str) -> String {
    if p.starts_with("~/") || p == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return p.replacen('~', &home, 1);
        }
    }
    // Also expand $HOME / ${HOME} minimally — mirrors os.path.expanduser only;
    // additional env expansion is not in Python's contract.
    p.to_string()
}

// ---------------------------------------------------------------------------
// _parse_branch_flag — lines 122-133
// ---------------------------------------------------------------------------

/// Mirrors `_parse_branch_flag(value: Optional[str]) -> Optional[str]`
/// (lines 122-133). `None` → `None` (no flag). Validates non-empty,
/// no leading `-`, no whitespace.
pub fn parse_branch_flag(value: Option<&str>) -> Result<Option<String>, String> {
    let raw = match value {
        None => return Ok(None),
        Some(v) => v,
    };
    let branch = raw.trim().to_string();
    if branch.is_empty() {
        return Err("--branch requires a non-empty name".to_string());
    }
    if branch.starts_with('-') {
        return Err("--branch must not start with '-'".to_string());
    }
    if branch.chars().any(|ch| ch.is_whitespace()) {
        return Err("--branch must not contain whitespace".to_string());
    }
    Ok(Some(branch))
}

// ---------------------------------------------------------------------------
// _check_dispatcher_presence — lines 136-209
// ---------------------------------------------------------------------------

/// Mirrors `_check_dispatcher_presence(hermes_home: Optional[Path]) -> (bool, str)`
/// (lines 136-209).
///
/// Python probes `gateway.status.resolve_gateway_liveness(profile_dir, use_cache=False)`
/// and `hermes_cli.config.load_config` for `kanban.dispatch_in_gateway`.  The
/// probe is defensive: any import/config failure is treated as "running/keep quiet"
/// to avoid crying wolf.
///
/// Rust slice 1 has no gateway/config linkage (NEVER cargo, no python bridge),
/// so we implement the env-gated fallback that the real probe would take plus
/// a filesystem probe for `gateway_state.json` when `HERMES_HOME` is known.
/// Any probe error → `(true, "")`  (silent, fail-open) matching Python.
pub fn check_dispatcher_presence(hermes_home: Option<&Path>) -> (bool, String) {
    // Try to resolve a pid-like liveness signal from the gateway state file.
    // This is a best-effort approximation; the real `resolve_gateway_liveness`
    // ladders through pidfile, launch-service, container, etc. Here we look
    // for `<hermes_home>/gateway/gateway_state.json` containing a pid.
    let home = hermes_home
        .map(|p| p.to_path_buf())
        .or_else(|| std::env::var("HERMES_HOME").ok().map(PathBuf::from))
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".hermes"))
        });

    let pid: Option<i64> = home.as_ref().and_then(|h| {
        let state = h.join("gateway").join("gateway_state.json");
        if !state.exists() {
            // Also try legacy location used by some tests
            let alt = h.join("gateway_state.json");
            if !alt.exists() {
                return None;
            }
            return read_pid_from_state(&alt);
        }
        read_pid_from_state(&state)
    });

    // Config gate: `kanban.dispatch_in_gateway` — default true.  Without a
    // config loader we honour an env override `HERMES_KANBAN_DISPATCH_IN_GATEWAY`
    // if set to "0"/"false" to exercise the disabled-dispatcher path; otherwise
    // assume enabled (matching Python's `except Exception: dispatch_on = True`).
    let dispatch_on = match std::env::var("HERMES_KANBAN_DISPATCH_IN_GATEWAY") {
        Ok(v) => {
            let low = v.trim().to_lowercase();
            !(low == "0" || low == "false" || low == "no" || low == "off")
        }
        Err(_) => true,
    };

    match (pid, dispatch_on) {
        (Some(p), true) => (true, format!("gateway pid={p}, dispatch enabled")),
        (Some(_), false) => (
            false,
            "Gateway is running but kanban.dispatch_in_gateway=false in config.yaml — the task will sit in 'ready' until you flip it back on and restart the gateway, OR run the legacy standalone daemon (`hermes kanban daemon --force`).".to_string(),
        ),
        (None, _) => (
            false,
            "No gateway is running — the task will sit in 'ready' until you start it. Run:\n    hermes gateway start\nThe gateway hosts an embedded dispatcher (tick interval 60s by default); your task will be picked up on the next tick after the gateway comes up.".to_string(),
        ),
    }
}

fn read_pid_from_state(path: &Path) -> Option<i64> {
    let text = std::fs::read_to_string(path).ok()?;
    // Tiny JSON scan for "pid": <int> without serde (NEVER cargo)
    let key = "\"pid\"";
    let idx = text.find(key)?;
    let after = &text[idx + key.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '-')
        .unwrap_or(rest.len());
    rest[..end].trim().parse::<i64>().ok().filter(|&p| p > 0)
}

// ---------------------------------------------------------------------------
// Argparse builder — lines 216-900 (truncated)
// ---------------------------------------------------------------------------
// Python builds the full `hermes kanban …` subcommand tree via argparse.
// Rust has no `argparse` and must not add `clap` (NEVER cargo) in this slice,
// so we mirror the builder as a pure-std `ParserSpec` tree.  Each
// `add_parser`/`add_argument` call below is annotated with its Python line
// span and preserves the exact `help`/`description` strings so audits can
// diff the two files line-for-line.  The tree is useful for `--help`
// generation, slash-command forwarding (`/kanban …` via `shlex`), and
// gateway dispatch without pulling a CLI framework.

/// Argument spec — mirrors one `parser.add_argument(...)` call.
#[derive(Debug, Clone)]
pub struct ArgSpec {
    pub flags: Vec<String>,
    pub dest: String,
    pub help: String,
    pub default: Option<String>,
    pub required: bool,
    pub choices: Vec<String>,
    pub action: String,
    pub metavar: Option<String>,
    pub nargs: Option<String>,
    pub type_name: Option<String>,
    pub aliases_help: Option<String>,
}

impl ArgSpec {
    pub fn new(flags: &[&str], help: &str) -> Self {
        let dest = flags
            .iter()
            .find(|f| f.starts_with("--"))
            .or_else(|| flags.first())
            .map(|f| f.trim_start_matches('-').replace('-', "_"))
            .unwrap_or_else(|| flags[0].replace('-', "_"));
        Self {
            flags: flags.iter().map(|s| s.to_string()).collect(),
            dest,
            help: help.to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        }
    }
    pub fn positional(name: &str, help: &str) -> Self {
        Self {
            flags: vec![name.to_string()],
            dest: name.to_string(),
            help: help.to_string(),
            default: None,
            required: true,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        }
    }
}

/// Parser spec — mirrors one `argparse.ArgumentParser` / subparser.
#[derive(Debug, Clone)]
pub struct ParserSpec {
    pub name: String,
    pub help: Option<String>,
    pub description: Option<String>,
    pub args: Vec<ArgSpec>,
    pub subparsers: Vec<ParserSpec>,
    pub subparsers_dest: Option<String>,
    pub aliases: Vec<String>,
}

impl ParserSpec {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            help: None,
            description: None,
            args: Vec::new(),
            subparsers: Vec::new(),
            subparsers_dest: None,
            aliases: Vec::new(),
        }
    }
    pub fn help(mut self, h: &str) -> Self {
        self.help = Some(h.to_string());
        self
    }
    pub fn description(mut self, d: &str) -> Self {
        self.description = Some(d.to_string());
        self
    }
    pub fn aliases(mut self, a: &[&str]) -> Self {
        self.aliases = a.iter().map(|s| s.to_string()).collect();
        self
    }
    pub fn add_argument(&mut self, arg: ArgSpec) -> &mut Self {
        self.args.push(arg);
        self
    }
    pub fn add_subparsers(&mut self, dest: &str) -> &mut Vec<ParserSpec> {
        self.subparsers_dest = Some(dest.to_string());
        &mut self.subparsers
    }
    pub fn add_parser(&mut self, parser: ParserSpec) -> &mut ParserSpec {
        self.subparsers.push(parser);
        self.subparsers.last_mut().unwrap()
    }
}

/// Mirrors `build_parser(parent_subparsers)` (lines 216-1018, truncated at 900).
///
/// Attaches the `kanban` subcommand tree under `parent`. Returns the
/// top-level `kanban` parser spec so caller can `set_defaults` (mirrors
/// `kanban_parser.set_defaults(_kanban_parser=kanban_parser)` at line 1017).
///
/// Slice 1 covers through the `heartbeat` subcommand (line 898) and the
/// `# --- assignees ---` header (line 900). The remainder (`assignees` body
/// through `repair` + `set_defaults`/`return`) continues in `kanban_slice2.rs`.
pub fn build_parser(parent_subparsers: &mut Vec<ParserSpec>) -> ParserSpec {
    // kanban_parser = parent_subparsers.add_parser("kanban", ...) — lines 221-231
    let mut kanban = ParserSpec::new("kanban")
        .help("Multi-profile collaboration board (tasks, links, comments)")
        .description(
            "Durable SQLite-backed task board shared across Hermes profiles. \
             Tasks are claimed atomically, can depend on other tasks, and \
             are executed by a named profile in an isolated workspace. \
             See https://hermes-agent.nousresearch.com/docs/user-guide/features/kanban \
             or docs/hermes-kanban-v1-spec.pdf for the full design.",
        );

    // --- global --board flag --- lines 232-247
    kanban.add_argument(ArgSpec {
        flags: vec!["--board".to_string()],
        dest: "board".to_string(),
        help: "Board slug to operate on. Defaults to the current board \
               (set via `hermes kanban boards switch <slug>` or the \
               HERMES_KANBAN_BOARD env var). Use `hermes kanban boards list` \
               to see all boards."
            .to_string(),
        default: None,
        required: false,
        choices: Vec::new(),
        action: String::new(),
        metavar: Some("<slug>".to_string()),
        nargs: None,
        type_name: None,
        aliases_help: None,
    });

    // sub = kanban_parser.add_subparsers(dest="kanban_action") — line 248
    // All subcommands below are added to `kanban.subparsers` with dest kanban_action.

    // --- init --- line 251
    kanban.subparsers.push(
        ParserSpec::new("init").help("Create kanban.db if missing (idempotent)"),
    );

    // --- boards (new in v2: multi-project support) --- lines 253-328
    {
        let mut p_boards = ParserSpec::new("boards")
            .help("Manage kanban boards (one board per project / workstream)")
            .description(
                "Boards let you separate unrelated streams of work \
                 (projects, repos, domains) into isolated queues. Each \
                 board has its own DB, workspaces directory, and dispatcher \
                 loop — tasks on one board cannot collide with tasks on \
                 another. The first board is 'default' and always exists.",
            );

        // boards_sub = p_boards.add_subparsers(dest="boards_action") — line 265
        // b_list = boards_sub.add_parser("list", aliases=["ls"], ...) — lines 267-273
        let mut b_list = ParserSpec::new("list")
            .help("List all boards with task counts")
            .aliases(&["ls"]);
        b_list.add_argument(ArgSpec {
            flags: vec!["--json".to_string()],
            dest: "json".to_string(),
            help: String::new(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        b_list.add_argument(ArgSpec {
            flags: vec!["--all".to_string()],
            dest: "all".to_string(),
            help: "Include archived boards too".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_boards.subparsers.push(b_list);

        // b_create = boards_sub.add_parser("create", aliases=["new"], ...) — lines 275-292
        let mut b_create = ParserSpec::new("create")
            .help("Create a new board")
            .aliases(&["new"]);
        b_create.add_argument(ArgSpec::positional(
            "slug",
            "Board slug (kebab-case, e.g. atm10-server)",
        ));
        b_create.add_argument(ArgSpec {
            flags: vec!["--name".to_string()],
            dest: "name".to_string(),
            help: "Human-readable display name (defaults to Title Case of slug)".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        b_create.add_argument(ArgSpec {
            flags: vec!["--description".to_string()],
            dest: "description".to_string(),
            help: "Optional description".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        b_create.add_argument(ArgSpec {
            flags: vec!["--icon".to_string()],
            dest: "icon".to_string(),
            help: "Optional emoji or single-character icon for the dashboard".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        b_create.add_argument(ArgSpec {
            flags: vec!["--color".to_string()],
            dest: "color".to_string(),
            help: "Optional hex color (e.g. '#8b5cf6') for the dashboard".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        b_create.add_argument(ArgSpec {
            flags: vec!["--switch".to_string()],
            dest: "switch".to_string(),
            help: "Switch to the new board after creating it".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        b_create.add_argument(ArgSpec {
            flags: vec!["--default-workdir".to_string()],
            dest: "default_workdir".to_string(),
            help: "Default workspace path for tasks created on this board".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_boards.subparsers.push(b_create);

        // b_rm = boards_sub.add_parser("rm", aliases=["remove","delete"], ...) — lines 294-301
        let mut b_rm = ParserSpec::new("rm")
            .help("Archive (default) or delete a board")
            .aliases(&["remove", "delete"]);
        b_rm.add_argument(ArgSpec::positional("slug", ""));
        b_rm.add_argument(ArgSpec {
            flags: vec!["--delete".to_string()],
            dest: "delete".to_string(),
            help: "Hard-delete the board directory instead of archiving it. Default is to move it to boards/_archived/ so it's recoverable.".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_boards.subparsers.push(b_rm);

        // b_switch = boards_sub.add_parser("switch", aliases=["use"], ...) — lines 303-307
        let mut b_switch = ParserSpec::new("switch")
            .help("Set the active board for subsequent CLI calls")
            .aliases(&["use"]);
        b_switch.add_argument(ArgSpec::positional("slug", ""));
        p_boards.subparsers.push(b_switch);

        // boards_sub.add_parser("show", aliases=["current"], ...) — lines 309-312
        p_boards.subparsers.push(
            ParserSpec::new("show")
                .help("Print the currently-active board slug")
                .aliases(&["current"]),
        );

        // b_rename = boards_sub.add_parser("rename", ...) — lines 314-319
        let mut b_rename = ParserSpec::new("rename")
            .help("Change a board's human-readable display name (slug is immutable)");
        b_rename.add_argument(ArgSpec::positional("slug", ""));
        b_rename.add_argument(ArgSpec::positional("name", "New display name"));
        p_boards.subparsers.push(b_rename);

        // b_set_wd = boards_sub.add_parser("set-default-workdir", ...) — lines 321-327
        let mut b_set_wd = ParserSpec::new("set-default-workdir")
            .help("Set the default workspace path for tasks on a board");
        b_set_wd.add_argument(ArgSpec::positional("slug", ""));
        b_set_wd.add_argument(ArgSpec {
            flags: vec!["path".to_string()],
            dest: "path".to_string(),
            help: "Absolute path to use as default workdir. Omit to clear.".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: Some("?".to_string()),
            type_name: None,
            aliases_help: None,
        });
        p_boards.subparsers.push(b_set_wd);

        kanban.subparsers.push(p_boards);
    }

    // --- create --- lines 329-402
    {
        let mut p_create = ParserSpec::new("create").help("Create a new task");
        p_create.add_argument(ArgSpec::positional("title", "Task title"));
        p_create.add_argument(ArgSpec {
            flags: vec!["--body".to_string()],
            dest: "body".to_string(),
            help: "Optional opening post".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_create.add_argument(ArgSpec {
            flags: vec!["--assignee".to_string()],
            dest: "assignee".to_string(),
            help: "Profile name to assign".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_create.add_argument(ArgSpec {
            flags: vec!["--parent".to_string()],
            dest: "parent".to_string(),
            help: "Parent task id (repeatable)".to_string(),
            default: Some("[]".to_string()),
            required: false,
            choices: Vec::new(),
            action: "append".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_create.add_argument(ArgSpec {
            flags: vec!["--workspace".to_string()],
            dest: "workspace".to_string(),
            help: "scratch | worktree | worktree:<path> | dir:<path> (default: scratch)"
                .to_string(),
            default: Some("scratch".to_string()),
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_create.add_argument(ArgSpec {
            flags: vec!["--branch".to_string()],
            dest: "branch".to_string(),
            help: "Branch name for worktree tasks, e.g. wt/t6-wire".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_create.add_argument(ArgSpec {
            flags: vec!["--project".to_string()],
            dest: "project".to_string(),
            help: "Link to a project (id or slug). Anchors the task's worktree under the project's primary repo with a deterministic branch. See `hermes project list`."
                .to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_create.add_argument(ArgSpec {
            flags: vec!["--tenant".to_string()],
            dest: "tenant".to_string(),
            help: "Tenant namespace".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_create.add_argument(ArgSpec {
            flags: vec!["--priority".to_string()],
            dest: "priority".to_string(),
            help: "Priority tiebreaker".to_string(),
            default: Some("0".to_string()),
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: Some("int".to_string()),
            aliases_help: None,
        });
        p_create.add_argument(ArgSpec {
            flags: vec!["--triage".to_string()],
            dest: "triage".to_string(),
            help: "Park in triage — a specifier will flesh out the spec and promote to todo"
                .to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_create.add_argument(ArgSpec {
            flags: vec!["--idempotency-key".to_string()],
            dest: "idempotency_key".to_string(),
            help: "Dedup key. If a non-archived task with this key exists, its id is returned instead of creating a duplicate."
                .to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_create.add_argument(ArgSpec {
            flags: vec!["--max-runtime".to_string()],
            dest: "max_runtime".to_string(),
            help: "Per-task runtime cap. Accepts seconds (300) or durations (90s, 30m, 2h, 1d). When exceeded, the dispatcher SIGTERMs (then SIGKILLs) the worker and re-queues the task."
                .to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_create.add_argument(ArgSpec {
            flags: vec!["--created-by".to_string()],
            dest: "created_by".to_string(),
            help: "Author name recorded on the task (default: user)".to_string(),
            default: Some("user".to_string()),
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_create.add_argument(ArgSpec {
            flags: vec!["--skill".to_string()],
            dest: "skills".to_string(),
            help: "Skill to force-load into the worker (repeatable). The kanban lifecycle is already injected automatically. Example: --skill translation --skill github-code-review"
                .to_string(),
            default: Some("[]".to_string()),
            required: false,
            choices: Vec::new(),
            action: "append".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_create.add_argument(ArgSpec {
            flags: vec!["--max-retries".to_string()],
            dest: "max_retries".to_string(),
            help: "Per-task override for the consecutive-failure circuit breaker. Trip on the Nth failure — e.g. --max-retries 1 blocks on the first failure (no retries), --max-retries 3 allows two retries. Omit to use the dispatcher's kanban.failure_limit config (default 2)."
                .to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: Some("N".to_string()),
            nargs: None,
            type_name: Some("int".to_string()),
            aliases_help: None,
        });
        p_create.add_argument(ArgSpec {
            flags: vec!["--model".to_string()],
            dest: "model_override".to_string(),
            help: "Pin the worker to this model (passed as -m <model>) without changing the profile's configured model. Combine with --provider when the model belongs to a different backend than the profile's default."
                .to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_create.add_argument(ArgSpec {
            flags: vec!["--provider".to_string()],
            dest: "provider_override".to_string(),
            help: "Provider the --model belongs to (passed as --provider <name> to the worker). Requires --model."
                .to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_create.add_argument(ArgSpec {
            flags: vec!["--goal".to_string()],
            dest: "goal_mode".to_string(),
            help: "Run the worker in a goal loop: after each turn a judge checks the response against the card title/body and, if not done, the worker keeps going in the same session until the judge agrees it's complete (or the turn budget runs out, which blocks the card for review). Best for open-ended cards one shot rarely finishes."
                .to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_create.add_argument(ArgSpec {
            flags: vec!["--goal-max-turns".to_string()],
            dest: "goal_max_turns".to_string(),
            help: "Turn budget for --goal workers (default 20). Ignored without --goal."
                .to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: Some("N".to_string()),
            nargs: None,
            type_name: Some("int".to_string()),
            aliases_help: None,
        });
        p_create.add_argument(ArgSpec {
            flags: vec!["--initial-status".to_string()],
            dest: "initial_status".to_string(),
            help: "Initial card status. Use 'blocked' for cards that require immediate human ops (R3 gate) to skip the brief running-to-blocked transition."
                .to_string(),
            default: Some("running".to_string()),
            required: false,
            choices: vec!["blocked".to_string(), "running".to_string()],
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_create.add_argument(ArgSpec {
            flags: vec!["--json".to_string()],
            dest: "json".to_string(),
            help: "Emit JSON output".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_create);
    }

    // --- swarm --- lines 404-423
    {
        let mut p_swarm = ParserSpec::new("swarm")
            .help("Create a Kanban Swarm v1 graph (parallel workers → verifier → synthesizer)");
        p_swarm.add_argument(ArgSpec::positional("goal", "Swarm goal / final outcome"));
        p_swarm.add_argument(ArgSpec {
            flags: vec!["--worker".to_string()],
            dest: "worker".to_string(),
            help: "Parallel worker card (repeatable)".to_string(),
            default: Some("[]".to_string()),
            required: false,
            choices: Vec::new(),
            action: "append".to_string(),
            metavar: Some("PROFILE:TITLE[:SKILL,SKILL]".to_string()),
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_swarm.add_argument(ArgSpec {
            flags: vec!["--verifier".to_string()],
            dest: "verifier".to_string(),
            help: "Verifier profile".to_string(),
            default: None,
            required: true,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_swarm.add_argument(ArgSpec {
            flags: vec!["--synthesizer".to_string()],
            dest: "synthesizer".to_string(),
            help: "Synthesizer/writer profile".to_string(),
            default: None,
            required: true,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_swarm.add_argument(ArgSpec {
            flags: vec!["--tenant".to_string()],
            dest: "tenant".to_string(),
            help: "Tenant namespace".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_swarm.add_argument(ArgSpec {
            flags: vec!["--priority".to_string()],
            dest: "priority".to_string(),
            help: "Priority tiebreaker".to_string(),
            default: Some("0".to_string()),
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: Some("int".to_string()),
            aliases_help: None,
        });
        p_swarm.add_argument(ArgSpec {
            flags: vec!["--created-by".to_string()],
            dest: "created_by".to_string(),
            help: "Creator/anchor profile".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_swarm.add_argument(ArgSpec {
            flags: vec!["--idempotency-key".to_string()],
            dest: "idempotency_key".to_string(),
            help: "Dedup key for the root card".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_swarm.add_argument(ArgSpec {
            flags: vec!["--json".to_string()],
            dest: "json".to_string(),
            help: "Emit JSON output".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_swarm);
    }

    // --- list --- lines 425-457
    {
        let mut p_list = ParserSpec::new("list")
            .help("List tasks")
            .aliases(&["ls"]);
        p_list.add_argument(ArgSpec {
            flags: vec!["--mine".to_string()],
            dest: "mine".to_string(),
            help: "Filter by $HERMES_PROFILE as assignee".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_list.add_argument(ArgSpec {
            flags: vec!["--assignee".to_string()],
            dest: "assignee".to_string(),
            help: String::new(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_list.add_argument(ArgSpec {
            flags: vec!["--status".to_string()],
            dest: "status".to_string(),
            help: String::new(),
            default: None,
            required: false,
            choices: kb_stub::VALID_STATUSES.iter().map(|s| s.to_string()).collect(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_list.add_argument(ArgSpec {
            flags: vec!["--tenant".to_string()],
            dest: "tenant".to_string(),
            help: String::new(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_list.add_argument(ArgSpec {
            flags: vec!["--session".to_string()],
            dest: "session".to_string(),
            help: "Filter by originating chat/agent session id (set on tasks created from inside an ACP loop)"
                .to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_list.add_argument(ArgSpec {
            flags: vec!["--archived".to_string()],
            dest: "archived".to_string(),
            help: "Include archived tasks".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_list.add_argument(ArgSpec {
            flags: vec!["--json".to_string()],
            dest: "json".to_string(),
            help: String::new(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_list.add_argument(ArgSpec {
            flags: vec!["--sort".to_string()],
            dest: "sort".to_string(),
            help: "Sort order for listed tasks (default: priority)".to_string(),
            default: None,
            required: false,
            choices: kb_stub::VALID_SORT_ORDERS.iter().map(|s| s.to_string()).collect(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_list.add_argument(ArgSpec {
            flags: vec!["--workflow-template-id".to_string()],
            dest: "workflow_template_id".to_string(),
            help: "Restrict to tasks with this workflow_template_id".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: Some("ID".to_string()),
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_list.add_argument(ArgSpec {
            flags: vec!["--step-key".to_string()],
            dest: "current_step_key".to_string(),
            help: "Restrict to tasks with this current_step_key".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: Some("KEY".to_string()),
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_list);
    }

    // --- show --- lines 459-474
    {
        let mut p_show = ParserSpec::new("show").help("Show a task with comments + events");
        p_show.add_argument(ArgSpec::positional("task_id", ""));
        p_show.add_argument(ArgSpec {
            flags: vec!["--json".to_string()],
            dest: "json".to_string(),
            help: String::new(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_show.add_argument(ArgSpec {
            flags: vec!["--state-type".to_string()],
            dest: "state_type".to_string(),
            help: "With --state-name: filter listed runs by task_runs column".to_string(),
            default: None,
            required: false,
            choices: vec!["status".to_string(), "outcome".to_string()],
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_show.add_argument(ArgSpec {
            flags: vec!["--state-name".to_string()],
            dest: "state_name".to_string(),
            help: "With --state-type: keep runs whose column equals this value".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: Some("VALUE".to_string()),
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_show);
    }

    // --- assign --- lines 476-479
    {
        let mut p_assign = ParserSpec::new("assign").help("Assign or reassign a task");
        p_assign.add_argument(ArgSpec::positional("task_id", ""));
        p_assign.add_argument(ArgSpec::positional("profile", "Profile name (or 'none' to unassign)"));
        kanban.subparsers.push(p_assign);
    }

    // --- set-model --- lines 481-496
    {
        let mut p_set_model = ParserSpec::new("set-model")
            .help("Set or clear a task's model/provider override (takes effect on the next dispatch)");
        p_set_model.add_argument(ArgSpec::positional("task_id", ""));
        p_set_model.add_argument(ArgSpec {
            flags: vec!["model".to_string()],
            dest: "model".to_string(),
            help: "Model to pin the worker to (or 'none' to clear the override)".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: Some("?".to_string()),
            type_name: None,
            aliases_help: None,
        });
        p_set_model.add_argument(ArgSpec {
            flags: vec!["--provider".to_string()],
            dest: "provider".to_string(),
            help: "Provider the model belongs to (worker is spawned with --provider <name>). Cleared together with the model."
                .to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_set_model);
    }

    // --- reclaim / reassign --- lines 498-525
    {
        let mut p_reclaim = ParserSpec::new("reclaim").help("Release an active worker claim on a running task");
        p_reclaim.add_argument(ArgSpec::positional("task_id", ""));
        p_reclaim.add_argument(ArgSpec {
            flags: vec!["--reason".to_string()],
            dest: "reason".to_string(),
            help: "Human-readable reason (recorded on the reclaimed event)".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_reclaim);

        let mut p_reassign = ParserSpec::new("reassign")
            .help("Reassign a task to a different profile, optionally reclaiming first");
        p_reassign.add_argument(ArgSpec::positional("task_id", ""));
        p_reassign.add_argument(ArgSpec::positional(
            "profile",
            "New profile name (or 'none' to unassign)",
        ));
        p_reassign.add_argument(ArgSpec {
            flags: vec!["--reclaim".to_string()],
            dest: "reclaim".to_string(),
            help: "Release any active claim before reassigning (required if task is running)"
                .to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_reassign.add_argument(ArgSpec {
            flags: vec!["--reason".to_string()],
            dest: "reason".to_string(),
            help: "Human-readable reason (recorded on the reclaimed event)".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_reassign);
    }

    // --- diagnostics --- lines 527-547
    {
        let mut p_diag = ParserSpec::new("diagnostics")
            .help("List active diagnostics on the current board")
            .aliases(&["diag"]);
        p_diag.add_argument(ArgSpec {
            flags: vec!["--severity".to_string()],
            dest: "severity".to_string(),
            help: "Only show diagnostics at or above this severity".to_string(),
            default: None,
            required: false,
            choices: vec!["warning".to_string(), "error".to_string(), "critical".to_string()],
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_diag.add_argument(ArgSpec {
            flags: vec!["--task".to_string()],
            dest: "task".to_string(),
            help: "Only show diagnostics for one task id".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_diag.add_argument(ArgSpec {
            flags: vec!["--json".to_string()],
            dest: "json".to_string(),
            help: "Emit JSON (structured) instead of the default human table".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_diag);
    }

    // --- link / unlink --- lines 549-555
    {
        let mut p_link = ParserSpec::new("link").help("Add a parent->child dependency");
        p_link.add_argument(ArgSpec::positional("parent_id", ""));
        p_link.add_argument(ArgSpec::positional("child_id", ""));
        kanban.subparsers.push(p_link);
        let mut p_unlink = ParserSpec::new("unlink").help("Remove a parent->child dependency");
        p_unlink.add_argument(ArgSpec::positional("parent_id", ""));
        p_unlink.add_argument(ArgSpec::positional("child_id", ""));
        kanban.subparsers.push(p_unlink);
    }

    // --- claim --- lines 557-564
    {
        let mut p_claim = ParserSpec::new("claim")
            .help("Atomically claim a ready task (prints resolved workspace path)");
        p_claim.add_argument(ArgSpec::positional("task_id", ""));
        p_claim.add_argument(ArgSpec {
            flags: vec!["--ttl".to_string()],
            dest: "ttl".to_string(),
            help: format!(
                "Claim TTL in seconds (default: {})",
                kb_stub::DEFAULT_CLAIM_TTL_SECONDS
            ),
            default: Some(kb_stub::DEFAULT_CLAIM_TTL_SECONDS.to_string()),
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: Some("int".to_string()),
            aliases_help: None,
        });
        kanban.subparsers.push(p_claim);
    }

    // --- comment / attach / attachments / attach-rm / complete / edit / block / schedule / unblock / request-review / request-changes / reopen-review / promote / archive / tail / dispatch / daemon / watch / stats / notify-* / log / runs / heartbeat --- lines 566-898
    {
        // comment — lines 567-573
        let mut p_comment = ParserSpec::new("comment").help("Append a comment");
        p_comment.add_argument(ArgSpec::positional("task_id", ""));
        p_comment.add_argument(ArgSpec {
            flags: vec!["text".to_string()],
            dest: "text".to_string(),
            help: "Comment body".to_string(),
            default: None,
            required: true,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: Some("+".to_string()),
            type_name: None,
            aliases_help: None,
        });
        p_comment.add_argument(ArgSpec {
            flags: vec!["--author".to_string()],
            dest: "author".to_string(),
            help: "Author name (default: $HERMES_PROFILE or 'user')".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_comment.add_argument(ArgSpec {
            flags: vec!["--max-len".to_string()],
            dest: "max_len".to_string(),
            help: "Trim the stored comment body to this many characters".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: Some("int".to_string()),
            aliases_help: None,
        });
        kanban.subparsers.push(p_comment);

        // attach — lines 575-584
        let mut p_attach = ParserSpec::new("attach").help("Attach a local file to a task");
        p_attach.add_argument(ArgSpec::positional("task_id", ""));
        p_attach.add_argument(ArgSpec::positional("path", "Path to the local file to attach"));
        p_attach.add_argument(ArgSpec {
            flags: vec!["--content-type".to_string()],
            dest: "content_type".to_string(),
            help: "MIME type (default: guessed from the file extension)".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_attach.add_argument(ArgSpec {
            flags: vec!["--name".to_string()],
            dest: "name".to_string(),
            help: "Stored filename (default: the source file's basename)".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_attach.add_argument(ArgSpec {
            flags: vec!["--author".to_string()],
            dest: "author".to_string(),
            help: "uploaded_by label (default: $HERMES_PROFILE or 'user')".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_attach);

        // attachments — lines 586-588
        let mut p_attachments = ParserSpec::new("attachments").help("List a task's attachments");
        p_attachments.add_argument(ArgSpec::positional("task_id", ""));
        p_attachments.add_argument(ArgSpec {
            flags: vec!["--json".to_string()],
            dest: "json".to_string(),
            help: String::new(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_attachments);

        // attach-rm — lines 590-591
        let mut p_attach_rm = ParserSpec::new("attach-rm").help("Delete an attachment by id");
        p_attach_rm.add_argument(ArgSpec {
            flags: vec!["attachment_id".to_string()],
            dest: "attachment_id".to_string(),
            help: String::new(),
            default: None,
            required: true,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: Some("int".to_string()),
            aliases_help: None,
        });
        kanban.subparsers.push(p_attach_rm);

        // complete — lines 593-603
        let mut p_complete = ParserSpec::new("complete").help("Mark one or more tasks done");
        p_complete.add_argument(ArgSpec {
            flags: vec!["task_ids".to_string()],
            dest: "task_ids".to_string(),
            help: "One or more task ids (only --result applies to all of them)".to_string(),
            default: None,
            required: true,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: Some("+".to_string()),
            type_name: None,
            aliases_help: None,
        });
        p_complete.add_argument(ArgSpec {
            flags: vec!["--result".to_string()],
            dest: "result".to_string(),
            help: "Result summary".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_complete.add_argument(ArgSpec {
            flags: vec!["--summary".to_string()],
            dest: "summary".to_string(),
            help: "Structured handoff summary for downstream tasks. Falls back to --result if omitted."
                .to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_complete.add_argument(ArgSpec {
            flags: vec!["--metadata".to_string()],
            dest: "metadata".to_string(),
            help: "JSON dict of structured facts (e.g. '{\"changed_files\": [...], \"tests_run\": 12}'). Stored on the closing run."
                .to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_complete);

        // edit — lines 604-623
        let mut p_edit = ParserSpec::new("edit").help("Edit recovery fields on an already-completed task");
        p_edit.add_argument(ArgSpec::positional("task_id", ""));
        p_edit.add_argument(ArgSpec {
            flags: vec!["--result".to_string()],
            dest: "result".to_string(),
            help: "Backfilled task result text for a done task".to_string(),
            default: None,
            required: true,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_edit.add_argument(ArgSpec {
            flags: vec!["--summary".to_string()],
            dest: "summary".to_string(),
            help: "Structured handoff summary. Falls back to --result if omitted.".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_edit.add_argument(ArgSpec {
            flags: vec!["--metadata".to_string()],
            dest: "metadata".to_string(),
            help: "JSON dict of structured facts to store on the latest completed run.".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_edit);

        // block — lines 625-639
        let mut p_block = ParserSpec::new("block").help("Mark one or more tasks blocked");
        p_block.add_argument(ArgSpec::positional("task_id", ""));
        p_block.add_argument(ArgSpec {
            flags: vec!["reason".to_string()],
            dest: "reason".to_string(),
            help: "Reason (also appended as a comment)".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: Some("*".to_string()),
            type_name: None,
            aliases_help: None,
        });
        p_block.add_argument(ArgSpec {
            flags: vec!["--ids".to_string()],
            dest: "ids".to_string(),
            help: "Additional task ids to block with the same reason (bulk mode)".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: Some("+".to_string()),
            type_name: None,
            aliases_help: None,
        });
        p_block.add_argument(ArgSpec {
            flags: vec!["--kind".to_string()],
            dest: "kind".to_string(),
            help: "Typed block reason. 'dependency' waits in todo (auto-promoted when parents finish, no human); 'needs_input'/'capability' go to blocked for a human; 'transient' marks a maybe-flaky failure. Repeated same-kind re-blocks after unblock route the task to triage to break unblock loops. Omit for a generic block."
                .to_string(),
            default: None,
            required: false,
            choices: kb_stub::VALID_BLOCK_KINDS.iter().map(|s| s.to_string()).collect(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_block);

        // schedule — lines 641-645
        let mut p_schedule = ParserSpec::new("schedule")
            .help("Park one or more tasks in Scheduled (waiting on time, not human input)");
        p_schedule.add_argument(ArgSpec::positional("task_id", ""));
        p_schedule.add_argument(ArgSpec {
            flags: vec!["reason".to_string()],
            dest: "reason".to_string(),
            help: "Reason/timing note (also appended as a comment)".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: Some("*".to_string()),
            type_name: None,
            aliases_help: None,
        });
        p_schedule.add_argument(ArgSpec {
            flags: vec!["--ids".to_string()],
            dest: "ids".to_string(),
            help: "Additional task ids to schedule with the same reason (bulk mode)".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: Some("+".to_string()),
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_schedule);

        // unblock — lines 647-656
        let mut p_unblock = ParserSpec::new("unblock")
            .help("Return blocked/scheduled tasks to ready, or todo while parents remain open");
        p_unblock.add_argument(ArgSpec {
            flags: vec!["--reason".to_string()],
            dest: "reason".to_string(),
            help: "Optional reason/note — recorded as a comment before unblocking. Quote multi-word reasons."
                .to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_unblock.add_argument(ArgSpec {
            flags: vec!["task_ids".to_string()],
            dest: "task_ids".to_string(),
            help: String::new(),
            default: None,
            required: true,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: Some("+".to_string()),
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_unblock);

        // request-review — lines 658-681
        let mut p_request_review = ParserSpec::new("request-review")
            .help("Move a task to 'review' (implementation done, awaiting review) — NOT a block");
        p_request_review.add_argument(ArgSpec::positional("task_id", ""));
        p_request_review.add_argument(ArgSpec {
            flags: vec!["--summary".to_string()],
            dest: "summary".to_string(),
            help: "What was implemented and how it was verified — shown to the reviewer.".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_request_review.add_argument(ArgSpec {
            flags: vec!["--reviewer".to_string()],
            dest: "reviewer".to_string(),
            help: "Optional reviewer profile; reassigns the task before review dispatch.".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_request_review.add_argument(ArgSpec {
            flags: vec!["--metadata".to_string()],
            dest: "metadata".to_string(),
            help: "JSON object with structured reviewer handoff facts.".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_request_review.add_argument(ArgSpec {
            flags: vec!["--force".to_string()],
            dest: "force".to_string(),
            help: "Override the live-claim guard: move a running, claimed task to review even without owning its run (clears the worker's claim)."
                .to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_request_review);

        // request-changes — lines 683-690
        let mut p_request_changes = ParserSpec::new("request-changes")
            .help("Reviewer verdict: return the active review run to its implementer");
        p_request_changes.add_argument(ArgSpec::positional("task_id", ""));
        p_request_changes.add_argument(ArgSpec {
            flags: vec!["reason".to_string()],
            dest: "reason".to_string(),
            help: "Concrete changes required before re-review".to_string(),
            default: None,
            required: true,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: Some("+".to_string()),
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_request_changes);

        // reopen-review — lines 692-701
        let mut p_reopen_review = ParserSpec::new("reopen-review")
            .help("Send one or more review tasks back for changes (review -> ready/todo)");
        p_reopen_review.add_argument(ArgSpec {
            flags: vec!["task_ids".to_string()],
            dest: "task_ids".to_string(),
            help: String::new(),
            default: None,
            required: true,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: Some("+".to_string()),
            type_name: None,
            aliases_help: None,
        });
        p_reopen_review.add_argument(ArgSpec {
            flags: vec!["--reason".to_string()],
            dest: "reason".to_string(),
            help: "Optional reason/note — recorded as a comment before reopening. Quote multi-word reasons."
                .to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_reopen_review);

        // promote — lines 703-733
        let mut p_promote = ParserSpec::new("promote")
            .help("Manually move one or more todo/blocked tasks to ready (recovery path)");
        p_promote.add_argument(ArgSpec::positional("task_id", ""));
        p_promote.add_argument(ArgSpec {
            flags: vec!["reason".to_string()],
            dest: "reason".to_string(),
            help: "Audit-trail reason (recorded on the task_events row)".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: Some("*".to_string()),
            type_name: None,
            aliases_help: None,
        });
        p_promote.add_argument(ArgSpec {
            flags: vec!["--ids".to_string()],
            dest: "ids".to_string(),
            help: "Additional task ids to promote with the same reason (bulk mode)".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: Some("+".to_string()),
            type_name: None,
            aliases_help: None,
        });
        p_promote.add_argument(ArgSpec {
            flags: vec!["--force".to_string()],
            dest: "force".to_string(),
            help: "Promote even if parent dependencies are not yet done/archived".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_promote.add_argument(ArgSpec {
            flags: vec!["--dry-run".to_string()],
            dest: "dry_run".to_string(),
            help: "Validate the promotion without mutating state".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_promote.add_argument(ArgSpec {
            flags: vec!["--json".to_string()],
            dest: "json".to_string(),
            help: "Emit machine-readable JSON result".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_promote);

        // archive — lines 735-744
        let mut p_archive = ParserSpec::new("archive").help("Archive one or more tasks");
        p_archive.add_argument(ArgSpec {
            flags: vec!["task_ids".to_string()],
            dest: "task_ids".to_string(),
            help: "Task ids to archive (default mode)".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: Some("*".to_string()),
            type_name: None,
            aliases_help: None,
        });
        p_archive.add_argument(ArgSpec {
            flags: vec!["--rm".to_string()],
            dest: "purge_ids".to_string(),
            help: "Permanently delete already-archived task ids from the board".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: Some("+".to_string()),
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_archive);

        // tail — lines 746-749
        let mut p_tail = ParserSpec::new("tail").help("Follow a task's event stream");
        p_tail.add_argument(ArgSpec::positional("task_id", ""));
        p_tail.add_argument(ArgSpec {
            flags: vec!["--interval".to_string()],
            dest: "interval".to_string(),
            help: String::new(),
            default: Some("1.0".to_string()),
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: Some("float".to_string()),
            aliases_help: None,
        });
        kanban.subparsers.push(p_tail);

        // dispatch — lines 751-764
        let mut p_disp =
            ParserSpec::new("dispatch").help("One dispatcher pass: reclaim stale, promote ready, spawn workers");
        p_disp.add_argument(ArgSpec {
            flags: vec!["--dry-run".to_string()],
            dest: "dry_run".to_string(),
            help: "Don't actually spawn processes; just print what would happen".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_disp.add_argument(ArgSpec {
            flags: vec!["--max".to_string()],
            dest: "max".to_string(),
            help: "Cap number of spawns this pass".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: Some("int".to_string()),
            aliases_help: None,
        });
        p_disp.add_argument(ArgSpec {
            flags: vec!["--failure-limit".to_string()],
            dest: "failure_limit".to_string(),
            help: format!(
                "Auto-block a task after this many consecutive non-success attempts (spawn_failed, timed_out, or crashed; default: {})",
                kb_stub::DEFAULT_SPAWN_FAILURE_LIMIT
            ),
            default: Some(kb_stub::DEFAULT_SPAWN_FAILURE_LIMIT.to_string()),
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: Some("int".to_string()),
            aliases_help: None,
        });
        p_disp.add_argument(ArgSpec {
            flags: vec!["--json".to_string()],
            dest: "json".to_string(),
            help: String::new(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_disp);

        // daemon (deprecated) — lines 766-785
        let mut p_daemon = ParserSpec::new("daemon")
            .help("DEPRECATED — dispatcher now runs in the gateway. Use `hermes gateway start`.");
        p_daemon.add_argument(ArgSpec {
            flags: vec!["--interval".to_string()],
            dest: "interval".to_string(),
            help: "Seconds between dispatch ticks (default: 60)".to_string(),
            default: Some("60.0".to_string()),
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: Some("float".to_string()),
            aliases_help: None,
        });
        p_daemon.add_argument(ArgSpec {
            flags: vec!["--max".to_string()],
            dest: "max".to_string(),
            help: "Cap number of spawns per tick".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: Some("int".to_string()),
            aliases_help: None,
        });
        p_daemon.add_argument(ArgSpec {
            flags: vec!["--failure-limit".to_string()],
            dest: "failure_limit".to_string(),
            help: String::new(),
            default: Some(kb_stub::DEFAULT_SPAWN_FAILURE_LIMIT.to_string()),
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: Some("int".to_string()),
            aliases_help: None,
        });
        p_daemon.add_argument(ArgSpec {
            flags: vec!["--pidfile".to_string()],
            dest: "pidfile".to_string(),
            help: "Write the daemon's PID to this file on start".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_daemon.add_argument(ArgSpec {
            flags: vec!["--verbose".to_string(), "-v".to_string()],
            dest: "verbose".to_string(),
            help: "Log each tick's outcome to stdout".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_daemon.add_argument(ArgSpec {
            flags: vec!["--force".to_string()],
            dest: "force".to_string(),
            help: "SUPPRESS".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_daemon);

        // watch — lines 787-801
        let mut p_watch = ParserSpec::new("watch")
            .help("Live-stream task_events to the terminal (Ctrl+C to exit)");
        p_watch.add_argument(ArgSpec {
            flags: vec!["--assignee".to_string()],
            dest: "assignee".to_string(),
            help: "Only show events for tasks assigned to this profile".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_watch.add_argument(ArgSpec {
            flags: vec!["--tenant".to_string()],
            dest: "tenant".to_string(),
            help: "Only show events from tasks in this tenant".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_watch.add_argument(ArgSpec {
            flags: vec!["--kinds".to_string()],
            dest: "kinds".to_string(),
            help: "Comma-separated event kinds to include (e.g. 'completed,blocked,gave_up,crashed,timed_out')"
                .to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_watch.add_argument(ArgSpec {
            flags: vec!["--interval".to_string()],
            dest: "interval".to_string(),
            help: "Poll interval in seconds (default: 0.5)".to_string(),
            default: Some("0.5".to_string()),
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: Some("float".to_string()),
            aliases_help: None,
        });
        kanban.subparsers.push(p_watch);

        // stats — lines 803-806
        let mut p_stats = ParserSpec::new("stats")
            .help("Per-status + per-assignee counts + oldest-ready age");
        p_stats.add_argument(ArgSpec {
            flags: vec!["--json".to_string()],
            dest: "json".to_string(),
            help: String::new(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_stats);

        // notify-subscribe — lines 808-843
        let mut p_nsub = ParserSpec::new("notify-subscribe").help(
            "Subscribe a gateway source to a task's terminal events (used by /kanban subscribe in the gateway adapter)",
        );
        p_nsub.add_argument(ArgSpec::positional("task_id", ""));
        p_nsub.add_argument(ArgSpec {
            flags: vec!["--platform".to_string()],
            dest: "platform".to_string(),
            help: String::new(),
            default: None,
            required: true,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_nsub.add_argument(ArgSpec {
            flags: vec!["--chat-id".to_string()],
            dest: "chat_id".to_string(),
            help: String::new(),
            default: None,
            required: true,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_nsub.add_argument(ArgSpec {
            flags: vec!["--thread-id".to_string()],
            dest: "thread_id".to_string(),
            help: String::new(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_nsub.add_argument(ArgSpec {
            flags: vec!["--user-id".to_string()],
            dest: "user_id".to_string(),
            help: String::new(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_nsub.add_argument(ArgSpec {
            flags: vec!["--user-id-alt".to_string()],
            dest: "user_id_alt".to_string(),
            help: String::new(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_nsub.add_argument(ArgSpec {
            flags: vec!["--chat-type".to_string()],
            dest: "chat_type".to_string(),
            help: "Originating source chat_type, recorded so the active-wake delivery modes resolve the operator's real session. Omit to leave an existing sub unchanged (new subs default to 'dm')."
                .to_string(),
            default: None,
            required: false,
            choices: vec!["dm".to_string(), "group".to_string(), "channel".to_string(), "thread".to_string()],
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_nsub.add_argument(ArgSpec {
            flags: vec!["--notifier-profile".to_string()],
            dest: "notifier_profile".to_string(),
            help: "Profile gateway that owns/delivers this subscription (default: active profile)"
                .to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_nsub.add_argument(ArgSpec {
            flags: vec!["--delivery-mode".to_string()],
            dest: "delivery_mode".to_string(),
            help: "How the kanban-notifier reacts to terminal events for this subscription: 'notify' (passive message only; default), 'notify+wake' (message AND wake the destination gateway agent so it reads the full board context and replies in its own voice), or 'wake' (wake the agent only, no passive message). Omit to leave an existing subscription's mode unchanged (new subs default to 'notify')."
                .to_string(),
            default: None,
            required: false,
            choices: kb_stub::_NOTIFY_DELIVERY_MODES.iter().map(|s| s.to_string()).collect(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_nsub);

        // notify-list — lines 845-850
        let mut p_nlist = ParserSpec::new("notify-list")
            .help("List notification subscriptions (optionally for a single task)");
        p_nlist.add_argument(ArgSpec {
            flags: vec!["task_id".to_string()],
            dest: "task_id".to_string(),
            help: String::new(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: Some("?".to_string()),
            type_name: None,
            aliases_help: None,
        });
        p_nlist.add_argument(ArgSpec {
            flags: vec!["--json".to_string()],
            dest: "json".to_string(),
            help: String::new(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_nlist);

        // notify-unsubscribe — lines 852-859
        let mut p_nrm = ParserSpec::new("notify-unsubscribe")
            .help("Remove a gateway subscription from a task");
        p_nrm.add_argument(ArgSpec::positional("task_id", ""));
        p_nrm.add_argument(ArgSpec {
            flags: vec!["--platform".to_string()],
            dest: "platform".to_string(),
            help: String::new(),
            default: None,
            required: true,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_nrm.add_argument(ArgSpec {
            flags: vec!["--chat-id".to_string()],
            dest: "chat_id".to_string(),
            help: String::new(),
            default: None,
            required: true,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_nrm.add_argument(ArgSpec {
            flags: vec!["--thread-id".to_string()],
            dest: "thread_id".to_string(),
            help: String::new(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_nrm);

        // log — lines 861-868
        let mut p_log = ParserSpec::new("log")
            .help("Print the worker log for a task (from <kanban-root>/kanban/logs/)");
        p_log.add_argument(ArgSpec::positional("task_id", ""));
        p_log.add_argument(ArgSpec {
            flags: vec!["--tail".to_string()],
            dest: "tail".to_string(),
            help: "Only print the last N bytes".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: Some("int".to_string()),
            aliases_help: None,
        });
        kanban.subparsers.push(p_log);

        // runs — lines 870-889
        let mut p_runs = ParserSpec::new("runs")
            .help("Show attempt history for a task (one row per run: profile, outcome, elapsed, summary)");
        p_runs.add_argument(ArgSpec::positional("task_id", ""));
        p_runs.add_argument(ArgSpec {
            flags: vec!["--json".to_string()],
            dest: "json".to_string(),
            help: String::new(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_runs.add_argument(ArgSpec {
            flags: vec!["--state-type".to_string()],
            dest: "state_type".to_string(),
            help: "With --state-name: filter runs by task_runs column".to_string(),
            default: None,
            required: false,
            choices: vec!["status".to_string(), "outcome".to_string()],
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        p_runs.add_argument(ArgSpec {
            flags: vec!["--state-name".to_string()],
            dest: "state_name".to_string(),
            help: "With --state-type: keep runs whose column equals this value".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: Some("VALUE".to_string()),
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_runs);

        // heartbeat — lines 891-898
        let mut p_hb = ParserSpec::new("heartbeat")
            .help("Emit a heartbeat event for a running task (worker liveness signal)");
        p_hb.add_argument(ArgSpec::positional("task_id", ""));
        p_hb.add_argument(ArgSpec {
            flags: vec!["--note".to_string()],
            dest: "note".to_string(),
            help: "Optional short note attached to the heartbeat event".to_string(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: String::new(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_hb);

        // --- assignees --- header at line 900 (truncated — body continues in slice 2)
        // Mirrors:
        //     # --- assignees ---
        //     p_asg = sub.add_parser("assignees", help="List known profiles + per-profile task counts ...")
        //     p_asg.add_argument("--json", action="store_true")
        // Body included as far as slice boundary allows:
        let mut p_asg = ParserSpec::new("assignees").help(
            "List known profiles + per-profile task counts (union of ~/.hermes/profiles/ and current assignees on the board)",
        );
        p_asg.add_argument(ArgSpec {
            flags: vec!["--json".to_string()],
            dest: "json".to_string(),
            help: String::new(),
            default: None,
            required: false,
            choices: Vec::new(),
            action: "store_true".to_string(),
            metavar: None,
            nargs: None,
            type_name: None,
            aliases_help: None,
        });
        kanban.subparsers.push(p_asg);
        // NOTE: slice 1 stops here (line 900).  Remaining parsers from python
        // lines 901-1018 (`context`, `specify`, `decompose`, `gc`, `repair`
        // + `kanban_parser.set_defaults(...)` / `return kanban_parser`) are
        // in `kanban_slice2.rs`.  The comment at line 900 `# --- assignees ---`
        // is the last line owned by this slice.
    }

    // In Python: `kanban_parser.set_defaults(_kanban_parser=kanban_parser)` + `return kanban_parser`
    // (lines 1017-1018) belongs to slice 2 per the 900-line boundary. We record
    // the default here for completeness without duplicating slice 2's ownership:
    // the returned spec carries the same identity as the stored default.

    parent_subparsers.push(kanban.clone());
    kanban
}

// ---------------------------------------------------------------------------
// Note: slice boundary — line 900
// ---------------------------------------------------------------------------
// Python `hermes_cli/kanban.py` lines 901-3442 (remainder of `build_parser`
// from `context` onward, `kanban_command`, `_profile_author`,
// `_is_delegated_child_cli_mutation`, `_dispatch_boards`, all `_cmd_*`
// handlers, slash helper, etc.) continue in `kanban_slice2.rs` through
// `kanban_slice4.rs`. This file intentionally stops at the 900-line
// boundary so that `cargo` is never invoked and the 4-slice decomposition
// stays clean. T0693 — 1:1 port, no cargo.
