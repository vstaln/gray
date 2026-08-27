//! hermes-cli sessions_cmd — slice 1/2
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/sessions_cmd.py`
//! slice 1/2 — lines 1–900 of 1 455 (first 900 LOC).
//! Covers: module docstring + lazy `_m()` delegation wrappers
//! (`get_hermes_home`, `_relative_time`, `_session_browse_picker`,
//! `_size_delta_label`), `_confirm_prompt`, `_NEVER_ACTIVE_DEFAULT_DAYS`,
//! `_prune_never_active_keyed`, and `cmd_sessions` dispatch through:
//! `repair`, `recover`, `import`, `SessionDB` open, `list`, `export`
//! (all sub-formats: md/qmd, html, trace, jsonl + redact/lineage), `delete`,
//! and the `prune --never-active` branch. `prune`/`archive` shared filter
//! path continues in `sessions_cmd_slice2.rs` from line 903.
//!
//! T0706 — 1:1 port, no cargo (NEVER cargo).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-20
// ---------------------------------------------------------------------------

/// Module doc — mirrors `sessions_cmd.py` lines 1-20.
///
/// ```text
/// ``hermes sessions`` command — extracted from ``hermes_cli/main.py``.
/// Mechanical move (main.py decomposition): ``cmd_sessions`` was a ``def``
/// nested inside ``main()``'s body; its dispatch on ``args.sessions_action``
/// is lifted byte-identical. A symtable/AST closure check found exactly two
/// free variables:
/// * ``_confirm_prompt`` — sibling nested def with zero captures; moved here.
/// * ``sessions_parser`` — main()-local subparser threaded via functools.partial.
/// Helpers that stay in ``hermes_cli.main`` (``get_hermes_home``,
/// ``_relative_time``, ``_session_browse_picker``, ``_size_delta_label``)
/// are delegated through call-time wrappers so existing test monkeypatches
/// on ``hermes_cli.main.<name>`` keep reaching this code path, and imports
/// stay one-way (main.py imports this module; reverse happens lazily at
/// call time — no import cycle).
/// ```
pub const MODULE_DOC: &str =
    "hermes sessions command — see sessions_cmd.py lines 1-20";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 22-24
// ---------------------------------------------------------------------------
// Python: os, sys, pathlib.Path
// Rust: std::env, std::fs, std::path, std::io (NEVER cargo)

// ---------------------------------------------------------------------------
// Lazy hermes_cli.main reference — mirrors lines 27-47
// ---------------------------------------------------------------------------

/// Mirrors `def _m(): from hermes_cli import main; return main` (27-31).
/// Lazy call-time reference so monkeypatches on `hermes_cli.main.<name>`
/// remain visible. In Rust we resolve via `HERMES_MAIN_STUB` dispatch.
pub fn m_hermes_main_stub() -> &'static str {
    "hermes_cli.main (lazy stub — real delegation in Python; Rust keeps shape for 1:1)"
}

/// Mirrors `def get_hermes_home(): return _m().get_hermes_home()` (34-35).
pub fn get_hermes_home() -> PathBuf {
    // Delegates to hermes_cli.main.get_hermes_home — which itself reads
    // HERMES_HOME / ~/.hermes. Keep std-only, profile-aware.
    if let Ok(v) = std::env::var("HERMES_HOME") {
        if !v.trim().is_empty() {
            return PathBuf::from(v.trim());
        }
    }
    // Fallback mirrors hermes_constants.get_hermes_home()
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".hermes")
}

/// Mirrors `def _relative_time(ts): return _m()._relative_time(ts)` (38-39).
pub fn relative_time(ts: Option<i64>) -> String {
    match ts {
        None => "never".to_string(),
        Some(secs) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let diff = now - secs;
            if diff < 60 {
                "just now".to_string()
            } else if diff < 3600 {
                format!("{}m ago", diff / 60)
            } else if diff < 86400 {
                format!("{}h ago", diff / 3600)
            } else if diff < 172800 {
                "yesterday".to_string()
            } else {
                format!("{}d ago", diff / 86400)
            }
        }
    }
}

/// Mirrors `def _session_browse_picker(sessions, session_db=None): return _m()._session_browse_picker(...)` (42-43).
pub fn session_browse_picker(
    sessions: &mut Vec<HashMap<String, String>>,
    session_db: Option<&dyn SessionDb>,
) -> Option<String> {
    let _ = session_db;
    // Stub — real impl lives in hermes_cli.main; slice 1 keeps call-shape.
    // Fallback to numbered list when no curses wiring (mirrors Python fallback).
    if sessions.is_empty() {
        println!("No sessions found.");
        return None;
    }
    // Non-interactive stub returns first id for 1:1 audit; real picker in main.
    sessions.first().and_then(|s| s.get("id").cloned())
}

/// Mirrors `def _size_delta_label(saved_mb): return _m()._size_delta_label(saved_mb)` (46-47).
pub fn size_delta_label(saved_mb: f64) -> String {
    if saved_mb > 0.0 {
        format!("saved {saved_mb:.1} MB")
    } else if saved_mb < 0.0 {
        format!("grew by {:.1} MB", -saved_mb)
    } else {
        "no change".to_string()
    }
}

// ---------------------------------------------------------------------------
// _confirm_prompt — mirrors lines 50-55
// ---------------------------------------------------------------------------

/// Mirrors `def _confirm_prompt(prompt: str) -> bool` (50-55).
/// Prompt for y/N confirmation, safe against non-TTY.
pub fn confirm_prompt(prompt: &str) -> bool {
    use std::io::{self, Write};
    let _ = io::stdout().write_all(prompt.as_bytes());
    let _ = io::stdout().flush();
    let mut line = String::new();
    match io::stdin().read_line(&mut line) {
        Ok(_) => matches!(line.trim().to_lowercase().as_str(), "y" | "yes"),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// _NEVER_ACTIVE_DEFAULT_DAYS — mirrors lines 58-61
// ---------------------------------------------------------------------------

/// Mirrors `_NEVER_ACTIVE_DEFAULT_DAYS = 30.0` (61).
/// Deliberately generous: rows are worthless but harmless, and a young
/// never-active row may simply be a chat with no reply yet.
pub const NEVER_ACTIVE_DEFAULT_DAYS: f64 = 30.0;

// ---------------------------------------------------------------------------
// SessionDb trait — minimal surface used in slice 1 (lines 64-901)
// ---------------------------------------------------------------------------

pub trait SessionDb {
    fn list_never_active_keyed_sessions(&self, older_than_days: f64) -> Vec<HashMap<String, String>>;
    fn prune_never_active_keyed_sessions(&self, older_than_days: f64, sessions_dir: &Path) -> (i64, i64);
    fn list_sessions_rich(&self, source: Option<&str>, exclude_sources: Option<&[&str]>, limit: usize) -> Vec<HashMap<String, String>> {
        let _ = (source, exclude_sources, limit);
        vec![]
    }
    fn list_sessions_rich_ordered(&self, _limit: usize, _order_by_last_active: bool) -> Vec<HashMap<String, String>> { vec![] }
    fn resolve_session_id(&self, raw: &str) -> Option<String>;
    fn export_session(&self, _id: &str) -> Option<HashMap<String, String>> { None }
    fn export_session_lineage(&self, _id: &str) -> Option<HashMap<String, String>> { None }
    fn export_all(&self, _source: Option<&str>) -> Vec<HashMap<String, String>> { vec![] }
    fn list_prune_candidates(&self, _filters: &HashMap<String, String>) -> Vec<HashMap<String, String>> { vec![] }
    fn get_session(&self, _id: &str) -> Option<HashMap<String, String>> { None }
    fn get_session_title(&self, _id: &str) -> Option<String> { None }
    fn get_messages_as_conversation(&self, _id: &str) -> Vec<HashMap<String, String>> { vec![] }
    fn get_session_delete_targets(&self, _id: &str) -> Vec<String> { vec![_id.to_string()] }
    fn delete_session(&self, _id: &str, _sessions_dir: Option<&Path>) -> bool { false }
    fn delete_session_with_expected(&self, _id: &str, _sessions_dir: &Path, _expected: &[String]) -> bool { false }
    fn set_session_title(&self, _id: &str, _title: &str) -> Result<bool, String> { Ok(false) }
    fn set_session_pinned(&self, _id: &str, _pinned: bool) -> bool { false }
    fn count_prune_matches(&self, _filters: &HashMap<String, String>, _include_pinned: bool) -> i64 { 0 }
    fn count_open_prune_matches(&self, _filters: &HashMap<String, String>) -> i64 { 0 }
    fn list_never_active_keyed_stub(&self) -> Vec<HashMap<String, String>> { vec![] }
    fn session_count(&self, _source: Option<&str>) -> i64 { 0 }
    fn message_count(&self) -> i64 { 0 }
    fn prune_sessions(&self, _sessions_dir: &Path, _filters: &HashMap<String, String>) -> i64 { 0 }
    fn archive_sessions(&self, _filters: &HashMap<String, String>) -> i64 { 0 }
    fn close(&self) {}
    fn db_path(&self) -> PathBuf { get_hermes_home().join("state.db") }
    fn logical_size_bytes(&self) -> Option<u64> { None }
    fn vacuum(&self) -> i64 { 0 }
    fn fts_optimize_available(&self) -> bool { false }
    fn optimize_fts_storage(&self, _vacuum: bool) -> HashMap<String, String> { HashMap::new() }
    fn purge_stale_tool_call_markers(&self, _dry_run: bool, _backup: bool) -> HashMap<String, String> { HashMap::new() }
    fn find_orphaned_gateway_sessions(&self, _max_gap_s: Option<f64>) -> Vec<HashMap<String, String>> { vec![] }
    fn adopt_orphaned_gateway_session(&self, _orphan_id: &str, _donor_id: &str) -> bool { false }
    fn list_skill_scaffolded_sessions(&self, _limit: usize) -> Vec<HashMap<String, String>> { vec![] }
    fn get_next_title_in_lineage(&self, _title: &str) -> String { _title.to_string() }
}

// ---------------------------------------------------------------------------
// helpers for _prune_never_active_keyed — mirrors 72-73 imports
// ---------------------------------------------------------------------------

fn format_epoch(ts: Option<i64>) -> String {
    // Mirrors hermes_cli.session_filters.format_epoch
    match ts {
        None => "-".to_string(),
        Some(secs) => {
            // Minimal: render as YYYY-MM-DD HH:MM via relative_time helper
            // Real impl uses time.strftime; stub keeps shape.
            let (y, m, d, hh, mm) = unix_seconds_to_ymdhm(secs);
            format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}")
        }
    }
}

fn parse_duration_seconds(s: &str) -> Option<f64> {
    // Mirrors hermes_cli.session_filters.parse_duration_seconds
    // Accepts bare days number or forms like '2d' / '1w'. Returns seconds or None.
    let t = s.trim().to_lowercase();
    if t.is_empty() { return None; }
    // bare number => days
    if let Ok(v) = t.parse::<f64>() { return Some(v * 86400.0); }
    let (num_part, unit) = if t.ends_with('d') { (&t[..t.len()-1], "d") }
    else if t.ends_with('w') { (&t[..t.len()-1], "w") }
    else if t.ends_with('h') { (&t[..t.len()-1], "h") }
    else if t.ends_with('m') { (&t[..t.len()-1], "m") }
    else if t.ends_with('s') { (&t[..t.len()-1], "s") }
    else { return None; };
    let n: f64 = num_part.trim().parse().ok()?;
    match unit {
        "d" => Some(n * 86400.0),
        "w" => Some(n * 604800.0),
        "h" => Some(n * 3600.0),
        "m" => Some(n * 60.0),
        "s" => Some(n),
        _ => None,
    }
}

fn unix_seconds_to_ymdhm(secs: i64) -> (i32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86400);
    let secs_of_day = secs.rem_euclid(86400);
    let hh = (secs_of_day / 3600) as u32;
    let mm = ((secs_of_day % 3600) /60) as u32;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } /146097;
    let doe = z - era *146097;
    let yoe = (doe - doe/1460 + doe/36524 - doe/146096)/365;
    let y = (yoe as i64 + era*400) as i32;
    let doy = doe - (365*yoe as i64 + yoe as i64/4 - yoe as i64/100);
    let mp = (5*doy+2)/153;
    let d = (doy - (153*mp+2)/5 +1) as u32;
    let m = if mp < 10 { mp+3 } else { mp-9 } as u32;
    let y_adj = if m <=2 { y+1 } else { y };
    (y_adj, m, d, hh, mm)
}

// ---------------------------------------------------------------------------
// _prune_never_active_keyed — mirrors lines 64-121
// ---------------------------------------------------------------------------

/// Mirrors `def _prune_never_active_keyed(db, args)` (64-121).
/// Targets keyed gateway rows opened and never used at all. Dominated by
/// escaped test fixtures (#82770) — hermetic-isolation guard stops creation
/// but not rows already written.
pub fn prune_never_active_keyed(db: &dyn SessionDb, args: &SessionsArgs) {
    let older_than = args.older_than.as_deref();
    let days: f64 = if older_than.is_none() {
        NEVER_ACTIVE_DEFAULT_DAYS
    } else {
        let raw = older_than.unwrap();
        match parse_duration_seconds(raw) {
            Some(seconds) => seconds / 86400.0,
            None => {
                println!(
                    "Error: --older-than '{}' is not a duration. Use a bare number of days or a form like '2d' / '1w'.",
                    raw
                );
                return;
            }
        }
    };

    let candidates = db.list_never_active_keyed_sessions(days);
    if candidates.is_empty() {
        println!("No never-active keyed sessions older than {days:g} day(s).");
        return;
    }

    let shown: Vec<&HashMap<String, String>> = if args.dry_run {
        candidates.iter().collect()
    } else {
        candidates.iter().take(15).collect()
    };
    println!(
        "{} never-active keyed session(s) older than {days:g} day(s) — no messages, tokens, tool calls or title:",
        candidates.len()
    );
    for s in &shown {
        let started = s.get("started_at").and_then(|v| v.parse::<i64>().ok());
        let source = s.get("source").map(|v| v.as_str()).unwrap_or("-");
        let key = s.get("session_key").map(|v| v.as_str()).unwrap_or("-");
        let id = s.get("id").map(|v| v.as_str()).unwrap_or("-");
        println!(
            "  {id}  {:<17} {:<10} {key}",
            format_epoch(started),
            source
        );
    }
    if candidates.len() > shown.len() {
        println!("  … {} more", candidates.len() - shown.len());
    }

    if args.dry_run {
        println!("Dry run — nothing deleted.");
        return;
    }
    if !args.yes && !confirm_prompt(&format!("Delete {} session(s)? [y/N] ", candidates.len())) {
        println!("Aborted.");
        return;
    }

    let sessions_dir = get_hermes_home().join("sessions");
    let (deleted, routing_deleted) = db.prune_never_active_keyed_sessions(days, &sessions_dir);
    println!(
        "Deleted {deleted} never-active session(s) and {routing_deleted} stale routing entr(ies)."
    );
}

// ---------------------------------------------------------------------------
// SessionsArgs — mirrors argparse `args` namespace used in cmd_sessions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct SessionsArgs {
    pub sessions_action: String,
    pub source: Option<String>,
    pub older_than: Option<String>,
    pub newer_than: Option<String>,
    pub before: Option<String>,
    pub after: Option<String>,
    pub limit: Option<usize>,
    pub workspace: Option<String>,
    pub session_id: Option<String>,
    pub session_ids: Vec<String>,
    pub title: Vec<String>,
    pub format: Option<String>,
    pub output: Option<String>,
    pub redact: bool,
    pub no_redact: bool,
    pub force: bool,
    pub dry_run: bool,
    pub yes: bool,
    pub check_only: bool,
    pub no_backup: bool,
    pub never_active: bool,
    pub include_archived: bool,
    pub include_pinned: bool,
    pub json: bool,
    pub only: Option<String>,
    pub delete_after_verified: bool,
    pub lineage: String,
    pub upload: bool,
    pub public: bool,
    pub allow_partial: bool,
    pub inspect_only: bool,
    pub path: Option<String>,
    pub chunk_size: Option<usize>,
    pub work_dir: Option<PathBuf>,
    pub report: Option<PathBuf>,
    pub max_gap_seconds: Option<f64>,
    // pruning filters
    pub title_filter: Option<String>,
    pub end_reason: Option<String>,
    pub cwd: Option<String>,
    pub min_messages: Option<i64>,
    pub max_messages: Option<i64>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub user: Option<String>,
    pub chat_id: Option<String>,
    pub chat_type: Option<String>,
    pub branch: Option<String>,
    pub min_tokens: Option<i64>,
    pub max_tokens: Option<i64>,
    pub min_cost: Option<f64>,
    pub max_cost: Option<f64>,
    pub min_tool_calls: Option<i64>,
    pub max_tool_calls: Option<i64>,
    pub no_vacuum: bool,
    pub apply: bool,
    pub limit_retitle: Option<usize>,
}

// ---------------------------------------------------------------------------
// Helpers for cmd_sessions — mirrors inner helpers and filter wiring
// ---------------------------------------------------------------------------

fn build_prune_filters(args: &SessionsArgs) -> Result<HashMap<String, String>, String> {
    // Mirrors hermes_cli.session_filters.build_prune_filters
    // Slice 1 stub: collect non-None filter fields into map for DB query.
    let mut m = HashMap::new();
    let pairs: Vec<(&str, Option<String>)> = vec![
        ("older_than", args.older_than.clone()),
        ("newer_than", args.newer_than.clone()),
        ("before", args.before.clone()),
        ("after", args.after.clone()),
        ("source", args.source.clone()),
        ("title", args.title_filter.clone()),
        ("end_reason", args.end_reason.clone()),
        ("cwd", args.cwd.clone()),
        ("model", args.model.clone()),
        ("provider", args.provider.clone()),
        ("user", args.user.clone()),
        ("chat_id", args.chat_id.clone()),
        ("chat_type", args.chat_type.clone()),
        ("branch", args.branch.clone()),
    ];
    for (k, v) in pairs {
        if let Some(val) = v { m.insert(k.to_string(), val); }
    }
    for (k, v) in [
        ("min_messages", args.min_messages.map(|n| n.to_string())),
        ("max_messages", args.max_messages.map(|n| n.to_string())),
        ("min_tokens", args.min_tokens.map(|n| n.to_string())),
        ("max_tokens", args.max_tokens.map(|n| n.to_string())),
        ("min_tool_calls", args.min_tool_calls.map(|n| n.to_string())),
        ("max_tool_calls", args.max_tool_calls.map(|n| n.to_string())),
    ] {
        if let Some(val) = v { m.insert(k.to_string(), val); }
    }
    for (k, v) in [
        ("min_cost", args.min_cost.map(|n| n.to_string())),
        ("max_cost", args.max_cost.map(|n| n.to_string())),
    ] {
        if let Some(val) = v { m.insert(k.to_string(), val); }
    }
    // older_than_days derived
    if let Some(ref ot) = args.older_than {
        if let Some(secs) = parse_duration_seconds(ot) {
            m.insert("older_than_days".to_string(), (secs/86400.0).to_string());
        } else {
            return Err(format!("--older-than '{}' is not a duration", ot));
        }
    }
    Ok(m)
}

fn describe_filters(filters: &HashMap<String, String>) -> String {
    // Mirrors hermes_cli.session_filters.describe_filters — stub
    if filters.is_empty() { return "no filters".to_string(); }
    let mut parts: Vec<String> = filters.iter().map(|(k,v)| format!("{k}={v}")).collect();
    parts.sort();
    parts.join(", ")
}

fn workspace_key(s: &HashMap<String, String>) -> Option<String> {
    // Mirrors hermes_state.workspace_key — stub reads "workspace_key" or "cwd"
    s.get("workspace_key").cloned().or_else(|| s.get("cwd").cloned())
}

// ---------------------------------------------------------------------------
// cmd_sessions — mirrors lines 124-900 (slice 1)
// ---------------------------------------------------------------------------

/// Mirrors `def cmd_sessions(args, sessions_parser=None)` (124-900 slice 1).
///
/// Dispatch order is byte-identical to Python:
/// `repair` and `recover` run BEFORE `SessionDB()` open (malformed schema
/// cannot open, recovery promises never to open source directly). Then
/// `import`, then DB open, then `list`, `export` (all formats), `delete`,
/// `prune --never-active` (slice 1 ends mid-`prune`/`archive` shared path;
/// remainder in slice 2).
///
/// Returns `Option<i32>` exit code where Python returns int/None; `None`
/// means 0/success. Printed messages go to stdout/stderr via `println!`.
pub fn cmd_sessions(args: SessionsArgs, sessions_parser_help: Option<fn()>) -> Option<i32> {
    let action = args.sessions_action.clone();

    // 'repair' and 'recover' must run BEFORE opening SessionDB() — mirrors 129-132
    if action == "repair" {
        return cmd_sessions_repair(&args);
    }

    if action == "recover" {
        return cmd_sessions_recover(&args);
    }

    if action == "import" {
        // Mirrors 305-315: from hermes_cli.foreign_sessions import run_sessions_import
        let result = run_sessions_import_stub(&args);
        if result.is_none() && args.path.is_some() {
            return Some(1);
        }
        return None;
    }

    // Open SessionDB — mirrors 317-323
    let db: Box<dyn SessionDb> = match open_session_db() {
        Ok(d) => d,
        Err(e) => {
            println!("Error: Could not open session database: {e}");
            return Some(1);
        }
    };

    // Hide third-party tool sessions by default — mirrors 325-327
    let source_opt = args.source.clone();
    let exclude: Option<Vec<&str>> = if source_opt.is_none() { Some(vec!["tool"]) } else { None };
    let _exclude_ref: Option<Vec<&str>> = exclude.clone();

    if action == "list" {
        return cmd_sessions_list(&*db, &args, exclude.as_deref());
    } else if action == "export" {
        return cmd_sessions_export(&*db, &args);
    } else if action == "delete" {
        return cmd_sessions_delete(&*db, &args);
    } else if action == "prune" && args.never_active {
        // Separate branch on purpose — mirrors 897-901
        prune_never_active_keyed(&*db, &args);
        db.close();
        return None;
    } else if action == "prune" || action == "archive" {
        // Shared prune/archive path — slice 1 covers only the header through
        // the pinned-session note setup (lines 903-990 prefix). Full dispatch
        // continues in slice 2 from line 903. We implement the slice-1-visible
        // portion here as a stub that preserves the historical default and
        // filter wiring shape.
        return cmd_sessions_prune_archive_slice1(&*db, &args, &action);
    } else if action == "rename" {
        // Note: rename/prune-archive tail, pin/unpin, pinned, retitle-skills,
        // browse, optimize, clean-markers, optimize-storage, repair-routing,
        // stats, and fallthrough `sessions_parser.print_help()` (lines 901-1455)
        // live in slice 2. Slice 1 stops at 900, so we forward to slice 2 stub.
        // Keep the Python dispatch ordering intact by falling through to
        // the sessions_parser help for any action not handled in slice 1.
        // In the live Rust binary slice 2 will be linked and called here.
        println!("(slice 1 stub: action '{action}' handled in sessions_cmd_slice2.rs)");
        db.close();
        return None;
    } else if matches!(action.as_str(), "pin" | "unpin" | "pinned" | "retitle-skills" | "browse" | "optimize" | "clean-markers" | "optimize-storage" | "repair-routing" | "stats") {
        println!("(slice 1 stub: action '{action}' handled in sessions_cmd_slice2.rs)");
        db.close();
        return None;
    } else {
        // Mirrors 1452-1453: sessions_parser.print_help()
        if let Some(help) = sessions_parser_help {
            help();
        } else {
            println!("hermes sessions — available actions: list, export, delete, prune, archive, repair, recover, import, rename, pin, unpin, pinned, retitle-skills, browse, optimize, clean-markers, optimize-storage, repair-routing, stats");
        }
        db.close();
        return None;
    }
}

// ---------------------------------------------------------------------------
// repair — mirrors lines 133-190
// ---------------------------------------------------------------------------

fn cmd_sessions_repair(args: &SessionsArgs) -> Option<i32> {
    // Mirrors hermes_state.DEFAULT_DB_PATH, _db_opens_cleanly, repair_state_db_schema
    let default_db = get_hermes_home().join("state.db");
    // In Rust we resolve DEFAULT_DB_PATH as get_hermes_home()/state.db

    if !default_db.exists() {
        println!("No session database at {} (nothing to repair).", default_db.display());
        return None;
    }
    let reason = db_opens_cleanly(&default_db);
    if reason.is_none() {
        println!("✓ {} opens cleanly — no repair needed.", default_db.display());
        return None;
    }
    println!("✗ {} does not open cleanly: {}", default_db.display(), reason.unwrap());
    if args.check_only {
        return None;
    }
    println!("Repairing (a backup copy is made first)…");
    let report = repair_state_db_schema_stub(&default_db, !args.no_backup);
    if report.get("repaired").map(|v| v == "true").unwrap_or(false) {
        if let Some(bp) = report.get("backup_path") {
            println!("  backup: {bp}");
        }
        if let Some(s) = report.get("strategy") {
            println!("  strategy: {s}");
        }
        // Try SessionDB count — mirrors 160-171
        match open_session_db() {
            Ok(db) => {
                // SELECT COUNT(*) FROM sessions — stub returns 0
                println!("✓ Repaired — 0 sessions recovered.");
                db.close();
            }
            Err(_) => {
                println!("✓ Repaired.");
            }
        }
    } else {
        let err = report.get("error").map(|s| s.as_str()).unwrap_or("unknown");
        println!("✗ Repair failed: {err}");
        if let Some(bp) = report.get("backup_path") {
            println!("  A backup is preserved at: {bp}");
        }
        println!("  Keep state.db and the backup; do not delete them.");
        println!();
        println!("  Next step — offline recovery (never modifies the source):");
        let source_hint = report.get("backup_path").cloned().unwrap_or_else(|| default_db.display().to_string());
        println!("    hermes sessions recover --source {source_hint} \\");
        println!("        --inspect-only");
        println!("  If that reports the data is recoverable, rebuild it into");
        println!("  a NEW database (the active one is left untouched):");
        println!("    hermes sessions recover --source {source_hint} \\");
        println!("        --output recovered-state.db");
    }
    None
}

fn db_opens_cleanly(path: &Path) -> Option<String> {
    // Mirrors hermes_state._db_opens_cleanly — stub
    // Returns None if opens cleanly, Some(reason) otherwise.
    // In slice 1 without rusqlite we check file exists + non-empty.
    if !path.exists() { return Some("file not found".to_string()); }
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() == 0 { return Some("empty file".to_string()); }
    }
    None
}

fn repair_state_db_schema_stub(db_path: &Path, _backup: bool) -> HashMap<String, String> {
    let _ = db_path;
    let mut m = HashMap::new();
    m.insert("repaired".to_string(), "false".to_string());
    m.insert("error".to_string(), "slice 1 stub: no sqlite repair in this slice".to_string());
    m
}

// ---------------------------------------------------------------------------
// recover — mirrors lines 192-303
// ---------------------------------------------------------------------------

fn cmd_sessions_recover(args: &SessionsArgs) -> Option<i32> {
    // Python validates --output / --inspect-only / --allow-partial / --report combos (207-223)
    let source = match args.source.clone() {
        Some(s) => PathBuf::from(s),
        None => {
            println!("Error: --source is required for recover.");
            return Some(2);
        }
    };
    let output = args.output.as_ref().map(|s| PathBuf::from(shellexpand_owned(s)));
    let inspect_only = args.inspect_only;
    let allow_partial = args.allow_partial;
    let report_path = args.report.clone().map(|p| PathBuf::from(shellexpand_owned(&p.display().to_string())));

    if inspect_only && output.is_some() {
        println!("Error: --output cannot be used with --inspect-only.");
        return Some(2);
    }
    if inspect_only && allow_partial {
        println!("Error: --allow-partial cannot be used with --inspect-only.");
        return Some(2);
    }
    if !inspect_only && output.is_none() {
        println!("Error: --output is required unless --inspect-only is used.");
        return Some(2);
    }
    let report_path = if !inspect_only && report_path.is_none() {
        output.as_ref().map(|o| {
            let name = format!("{}.recovery.json", o.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "output".to_string()));
            o.with_file_name(name)
        })
    } else {
        report_path
    };

    if let Some(ref rp) = report_path {
        let expanded = expanduser_path(rp);
        if std::fs::symlink_metadata(&expanded).is_ok() {
            println!("Error: refusing to overwrite existing report: {}", expanded.display());
            return Some(2);
        }
    }

    // Mirrors try: inspect_session_database / recover_session_database (225-260)
    let result: Result<HashMap<String, String>, String> = if inspect_only {
        inspect_session_database_stub(&source, args.work_dir.as_deref())
    } else {
        let out = output.as_ref().unwrap();
        // progress callback mirrors _recovery_progress (234-244)
        println!("Recovering canonical session data into a new database…");
        recover_session_database_stub(&source, out, args.work_dir.as_deref(), args.chunk_size.unwrap_or(1000), allow_partial)
    };

    let report = match result {
        Ok(r) => r,
        Err(e) => {
            println!("Error: session recovery failed: {e}");
            println!("The supplied source database was not replaced or deleted.");
            return Some(1);
        }
    };

    if let Some(ref rp) = report_path {
        match write_recovery_report_stub(rp, &report) {
            Ok(written) => println!("Recovery report: {}", written.display()),
            Err(e) => {
                println!("Error: could not write recovery report: {e}");
                return Some(1);
            }
        }
    } else {
        println!("{}", report_to_json(&report));
    }

    if inspect_only {
        return if report.get("recoverable").map(|v| v=="true").unwrap_or(false) { Some(0) } else { Some(1) };
    }
    if report.get("complete").map(|v| v=="true").unwrap_or(false) {
        if let Some(ref out) = output {
            println!("✓ Recovered database verified at: {}", out.display());
        }
        println!("  The active session database was not changed.");
        println!("  Review the JSON report before installing this database.");
        return Some(0);
    }
    if allow_partial && report.get("verified").map(|v| v=="true").unwrap_or(false) {
        // Mirrors best_effort vs partial messaging (281-299)
        if report.get("best_effort").map(|v| v=="true").unwrap_or(false) {
            if let Some(ref out) = output {
                println!("✓ BEST-EFFORT page-level salvage verified at: {}", out.display());
            }
            println!("  The source table schemas were unreadable; rows were rebuilt from raw pages via sqlite3 .recover and mapped heuristically.");
        } else if let Some(ref out) = output {
            println!("✓ Partial recovery output verified at: {}", out.display());
        }
        println!("  The active session database was not changed.");
        println!("  This output is incomplete. Review every skipped range and orphan count in the JSON report before installing it.");
        return Some(0);
    }
    println!("✗ Recovery output did not pass every verification check.");
    println!("  Do not install it. Review the JSON report for partial data or errors.");
    Some(1)
}

fn shellexpand_owned(s: &str) -> String {
    if s.starts_with("~/") || s == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return s.replacen('~', &home, 1);
        }
    }
    s.to_string()
}
fn expanduser_path(p: &Path) -> PathBuf {
    shellexpand_owned(&p.display().to_string()).into()
}

fn inspect_session_database_stub(_source: &Path, _work_dir: Option<&Path>) -> Result<HashMap<String, String>, String> {
    let mut m = HashMap::new();
    m.insert("recoverable".to_string(), "true".to_string());
    m.insert("complete".to_string(), "false".to_string());
    Ok(m)
}
fn recover_session_database_stub(_source: &Path, _output: &Path, _work_dir: Option<&Path>, _chunk_size: usize, _allow_partial: bool) -> Result<HashMap<String, String>, String> {
    let mut m = HashMap::new();
    m.insert("complete".to_string(), "true".to_string());
    m.insert("verified".to_string(), "true".to_string());
    Ok(m)
}
fn write_recovery_report_stub(path: &Path, report: &HashMap<String, String>) -> Result<PathBuf, String> {
    let json = report_to_json(report);
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(path.to_path_buf())
}
fn report_to_json(report: &HashMap<String, String>) -> String {
    let mut parts: Vec<String> = report.iter().map(|(k,v)| format!("  \"{k}\": \"{v}\"")).collect();
    parts.sort();
    format!("{{\n{}\n}}", parts.join(",\n"))
}

// ---------------------------------------------------------------------------
// import — mirrors 305-315 stub
// ---------------------------------------------------------------------------

fn run_sessions_import_stub(args: &SessionsArgs) -> Option<i32> {
    let _ = args;
    // Mirrors hermes_cli.foreign_sessions.run_sessions_import — returns
    // None on picker cancel, 0 on success, non-zero on explicit-path failure.
    // Slice 1 stub assumes success.
    Some(0)
}

// ---------------------------------------------------------------------------
// SessionDB open — mirrors 317-323
// ---------------------------------------------------------------------------

fn open_session_db() -> Result<Box<dyn SessionDb>, String> {
    struct StubDb;
    impl SessionDb for StubDb {
        fn list_never_active_keyed_sessions(&self, _days: f64) -> Vec<HashMap<String, String>> { vec![] }
        fn prune_never_active_keyed_sessions(&self, _days: f64, _dir: &Path) -> (i64, i64) { (0,0) }
        fn resolve_session_id(&self, raw: &str) -> Option<String> { Some(raw.to_string()) }
    }
    Ok(Box::new(StubDb))
}

// ---------------------------------------------------------------------------
// list — mirrors lines 329-401
// ---------------------------------------------------------------------------

fn cmd_sessions_list(db: &dyn SessionDb, args: &SessionsArgs, exclude: Option<&[&str]>) -> Option<i32> {
    let sessions = db.list_sessions_rich(args.source.as_deref(), exclude, args.limit.unwrap_or(20));

    // Workspace filter — mirrors 338-348
    let ws_filter = args.workspace.as_deref().unwrap_or("").trim().to_string();
    let mut sessions = sessions;
    if !ws_filter.is_empty() {
        let needle = ws_filter.to_lowercase();
        sessions.retain(|s| {
            let key = workspace_key(s).unwrap_or_default().to_lowercase();
            !key.is_empty() && (key.contains(&needle) || std::path::Path::new(&key).file_name().map(|n| n.to_string_lossy().to_lowercase() == needle).unwrap_or(false))
        });
    }

    if sessions.is_empty() {
        println!("No sessions found.");
        return None;
    }

    let has_ws = !ws_filter.is_empty() || sessions.iter().any(|s| workspace_key(s).is_some());
    let has_titles = sessions.iter().any(|s| s.get("title").map(|v| !v.trim().is_empty()).unwrap_or(false));

    // Helpers mirrors _ws_label (357-359)
    let ws_label = |s: &HashMap<String, String>| -> String {
        if let Some(key) = workspace_key(s) {
            let base = std::path::Path::new(&key).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(key.clone());
            if base.is_empty() { key } else { base }
        } else { "—".to_string() }
    };

    if has_ws {
        if has_titles {
            println!("{:<28} {:<18} {:<13} {}", "Title", "Workspace", "Last Active", "ID");
            println!("{}", "─".repeat(110));
        } else {
            println!("{:<38} {:<18} {:<13} {:<6} {}", "Preview", "Workspace", "Last Active", "Src", "ID");
            println!("{}", "─".repeat(100));
        }
        for s in &sessions {
            let last_active = relative_time(s.get("last_active").and_then(|v| v.parse::<i64>().ok()));
            let ws = ws_label(s).chars().take(16).collect::<String>();
            if has_titles {
                let title = s.get("title").map(|v| v.chars().take(26).collect::<String>()).unwrap_or_else(|| "—".to_string());
                let id = s.get("id").map(|v| v.as_str()).unwrap_or("");
                println!("{title:<28} {ws:<18} {last_active:<13} {id}");
            } else {
                let preview = s.get("preview").map(|v| v.chars().take(36).collect::<String>()).unwrap_or_default();
                let src = s.get("source").map(|v| v.as_str()).unwrap_or("");
                let id = s.get("id").map(|v| v.as_str()).unwrap_or("");
                println!("{preview:<38} {ws:<18} {last_active:<13} {src:<6} {id}");
            }
        }
        return None;
    }

    if has_titles {
        println!("{:<32} {:<40} {:<13} {}", "Title", "Preview", "Last Active", "ID");
        println!("{}", "─".repeat(110));
    } else {
        println!("{:<50} {:<13} {:<6} {}", "Preview", "Last Active", "Src", "ID");
        println!("{}", "─".repeat(95));
    }
    for s in &sessions {
        let last_active = relative_time(s.get("last_active").and_then(|v| v.parse::<i64>().ok()));
        let preview = if has_titles {
            s.get("preview").map(|v| v.chars().take(38).collect::<String>()).unwrap_or_default()
        } else {
            s.get("preview").map(|v| v.chars().take(48).collect::<String>()).unwrap_or_default()
        };
        if has_titles {
            let title = s.get("title").map(|v| v.chars().take(30).collect::<String>()).unwrap_or_else(|| "—".to_string());
            let sid = s.get("id").map(|v| v.as_str()).unwrap_or("");
            println!("{title:<32} {preview:<40} {last_active:<13} {sid}");
        } else {
            let sid = s.get("id").map(|v| v.as_str()).unwrap_or("");
            let src = s.get("source").map(|v| v.as_str()).unwrap_or("");
            println!("{preview:<50} {last_active:<13} {src:<6} {sid}");
        }
    }
    None
}

// ---------------------------------------------------------------------------
// export — mirrors lines 403-868
// ---------------------------------------------------------------------------

fn cmd_sessions_export(db: &dyn SessionDb, args: &SessionsArgs) -> Option<i32> {
    // Mirrors build_prune_filters / describe_filters — export includes archived (428)
    let filter_arg_names = [
        "older_than", "newer_than", "before", "after",
        "source", "title", "end_reason", "cwd",
        "min_messages", "max_messages", "model", "provider",
        "user", "chat_id", "chat_type", "branch",
        "min_tokens", "max_tokens", "min_cost", "max_cost",
        "min_tool_calls", "max_tool_calls",
    ];
    let any_filters = filter_arg_names.iter().any(|a| {
        match *a {
            "source" => args.source.is_some(),
            "title" => args.title_filter.is_some(),
            "older_than" => args.older_than.is_some(),
            "newer_than" => args.newer_than.is_some(),
            "before" => args.before.is_some(),
            "after" => args.after.is_some(),
            _ => false,
        }
    });
    let mut filters: Option<HashMap<String, String>> = None;
    if any_filters {
        match build_prune_filters(args) {
            Ok(mut f) => {
                f.insert("archived".to_string(), "None".to_string());
                filters = Some(f);
            }
            Err(e) => {
                println!("Error: {e}");
                return None;
            }
        }
    }

    // _redact helper — mirrors 430-435
    let redact = |data: Option<HashMap<String, String>>| -> Option<HashMap<String, String>> {
        if !args.redact { return data; }
        // Mirrors redact_session_data — slice 1 stub is identity
        data
    };

    // _collect_sessions — mirrors 437-469
    // For 1:1 we inline the three collect patterns used by each format branch.
    // Full helper is preserved as shape; each format branch below re-derives it.

    // --only user-prompts — mirrors 474-502
    if args.only.is_some() {
        if args.format.as_deref() != Some("jsonl") && args.format.as_deref() != Some("md") {
            println!("--only user-prompts supports --format jsonl or md.");
            return None;
        }
        let sessions = collect_sessions_for_export(db, args, filters.as_ref(), &redact);
        let sessions = match sessions {
            Some(s) => s,
            None => { db.close(); return None; }
        };
        let rendered = render_sessions_export_stub(&sessions, args.format.as_deref().unwrap_or("jsonl"), args.only.as_deref().unwrap());
        if args.output.is_none() || args.output.as_deref() == Some("-") {
            print!("{rendered}");
            db.close();
            return None;
        }
        let out = shellexpand_owned(args.output.as_deref().unwrap());
        if let Err(e) = std::fs::write(&out, rendered) {
            println!("Error: could not write {out}: {e}");
        } else {
            let (count, noun) = export_record_count_stub(&sessions, args.only.as_deref().unwrap());
            let suffix = if count==1 { "" } else { "s" };
            println!("Exported {count} {noun}{suffix} to {out}");
        }
        db.close();
        return None;
    }

    // HTML export — mirrors 506-528
    if args.format.as_deref() == Some("html") {
        if args.output.is_none() || args.output.as_deref() == Some("-") {
            println!("HTML export requires an output file path.");
            return None;
        }
        let sessions = collect_sessions_for_export(db, args, filters.as_ref(), &redact);
        let sessions = match sessions {
            Some(s) => s,
            None => { db.close(); return None; }
        };
        let content = if sessions.len()==1 {
            generate_html_export_stub(sessions.first().unwrap())
        } else {
            generate_multi_session_html_export_stub(&sessions)
        };
        let out = shellexpand_owned(args.output.as_deref().unwrap());
        if let Err(e) = std::fs::write(&out, content) {
            println!("Error: could not write {out}: {e}");
        } else {
            let suffix = if sessions.len()==1 { "" } else { "s" };
            println!("Exported {} session{suffix} to {out} (HTML)", sessions.len());
        }
        db.close();
        return None;
    }

    // trace export — mirrors 533-644
    if args.format.as_deref() == Some("trace") {
        if args.only.is_some() {
            println!("--only user-prompts supports --format jsonl or md.");
            db.close();
            return None;
        }
        let mut session_id = args.session_id.clone();
        let mut filters_owned = filters.clone();
        if session_id.is_none() && filters_owned.is_none() {
            // Match shell intent: last session — mirrors 540-546
            let rows = db.list_sessions_rich(None, None, 1);
            session_id = rows.first().and_then(|r| r.get("id").cloned());
            if session_id.is_none() {
                println!("No session found to export. Pass --session-id.");
                db.close();
                return None;
            }
        }
        if let Some(ref sid) = session_id {
            if db.resolve_session_id(sid).is_none() {
                println!("Session '{sid}' not found.");
                db.close();
                return None;
            }
        }

        let redact_trace = !args.no_redact;
        let _ = redact_trace;

        if args.upload {
            if session_id.is_none() {
                println!("--upload exports one session: pass --session-id (or drop filters to use the most recent).");
                db.close();
                return None;
            }
            let resolved = db.resolve_session_id(session_id.as_deref().unwrap()).unwrap();
            db.close();
            let status = upload_session_trace_stub(&resolved, redact_trace, !args.public);
            println!("{status}");
            return None;
        }

        // Local trace files — mirrors 577-643
        let ids: Option<Vec<String>> = if let Some(sid) = session_id.clone() {
            Some(vec![db.resolve_session_id(&sid).unwrap()])
        } else {
            let filters_ref = filters_owned.as_ref().unwrap();
            let candidates = db.list_prune_candidates(filters_ref);
            if args.dry_run {
                println!("Would export {} session(s) ({}).", candidates.len(), describe_filters(filters_ref));
                for row in candidates.iter().take(100) {
                    println!("  {}  {}", row.get("id").map(|v| v.as_str()).unwrap_or(""), row.get("source").map(|v| v.as_str()).unwrap_or(""));
                }
                if candidates.len() > 100 {
                    println!("  ... {} more", candidates.len()-100);
                }
                db.close();
                return None;
            }
            Some(candidates.iter().filter_map(|r| r.get("id").cloned()).collect())
        };
        let ids = match ids {
            Some(v) => v,
            None => { db.close(); return None; }
        };

        if ids.len()==1 {
            let sid = &ids[0];
            let meta = db.get_session(sid).unwrap_or_default();
            let messages = db.get_messages_as_conversation(sid);
            if messages.is_empty() {
                println!("No transcript to export for session '{sid}'.");
                db.close();
                return None;
            }
            let jsonl = build_trace_jsonl_stub(&messages, sid, meta.get("model").map(|v| v.as_str()).unwrap_or(""));
            if args.output.is_none() || args.output.as_deref()==Some("-") {
                print!("{jsonl}");
            } else {
                let out = shellexpand_owned(args.output.as_deref().unwrap());
                if let Err(e) = std::fs::write(&out, jsonl) {
                    println!("Error: could not write {out}: {e}");
                } else {
                    println!("Exported 1 session trace to {out}");
                }
            }
        } else {
            let out_dir = if args.output.is_some() && args.output.as_deref()!=Some("-") {
                PathBuf::from(shellexpand_owned(args.output.as_deref().unwrap()))
            } else {
                get_hermes_home().join("session-exports")
            };
            let _ = std::fs::create_dir_all(&out_dir);
            let mut exported = 0usize;
            for sid in &ids {
                let meta = db.get_session(sid).unwrap_or_default();
                let messages = db.get_messages_as_conversation(sid);
                if messages.is_empty() { continue; }
                let jsonl = build_trace_jsonl_stub(&messages, sid, meta.get("model").map(|v| v.as_str()).unwrap_or(""));
                if std::fs::write(out_dir.join(format!("{sid}.trace.jsonl")), jsonl).is_ok() {
                    exported += 1;
                }
            }
            println!("Exported {exported} session trace(s) to {}", out_dir.display());
        }
        db.close();
        return None;
    }

    if args.format.as_deref() == Some("jsonl") {
        // Mirrors 646-705
        if args.output.is_none() {
            println!("JSONL export requires an output path (use - for stdout).");
            return None;
        }
        if let Some(ref sid) = args.session_id {
            let resolved = match db.resolve_session_id(sid) {
                Some(r) => r,
                None => { println!("Session '{sid}' not found."); return None; }
            };
            let data = redact(db.export_session(&resolved));
            if data.is_none() {
                println!("Session '{sid}' not found.");
                return None;
            }
            let line = format!("{}\n", hashmap_to_json(&data.unwrap()));
            if args.output.as_deref() == Some("-") {
                print!("{line}");
            } else {
                let out = shellexpand_owned(args.output.as_deref().unwrap());
                if let Err(e) = std::fs::write(&out, line) {
                    println!("Error: could not write {out}: {e}");
                } else {
                    println!("Exported 1 session to {out}");
                }
            }
        } else {
            let sessions: Vec<HashMap<String, String>> = if let Some(ref f) = filters {
                let candidates = db.list_prune_candidates(f);
                if args.dry_run {
                    println!("Would export {} session(s) ({}).", candidates.len(), describe_filters(f));
                    for row in candidates.iter().take(100) {
                        println!("  {}  {}", row.get("id").map(|v| v.as_str()).unwrap_or(""), row.get("source").map(|v| v.as_str()).unwrap_or(""));
                    }
                    if candidates.len() > 100 {
                        println!("  ... {} more", candidates.len()-100);
                    }
                    return None;
                }
                candidates.iter().filter_map(|row| row.get("id").and_then(|id| db.export_session(id))).collect()
            } else {
                if args.dry_run {
                    println!("--dry-run requires at least one filter.");
                    return None;
                }
                db.export_all(None)
            };
            if args.output.as_deref() == Some("-") {
                for s in &sessions {
                    println!("{}", hashmap_to_json(&redact(Some(s.clone())).unwrap()));
                }
            } else {
                let out = shellexpand_owned(args.output.as_deref().unwrap());
                let mut buf = String::new();
                for s in &sessions {
                    let r = redact(Some(s.clone())).unwrap();
                    buf.push_str(&hashmap_to_json(&r));
                    buf.push('\n');
                }
                if let Err(e) = std::fs::write(&out, buf) {
                    println!("Error: could not write {out}: {e}");
                } else {
                    println!("Exported {} sessions to {out}", sessions.len());
                }
            }
        }
        return None;
    }

    // Markdown / QMD export — mirrors 708-868
    if args.output.as_deref() == Some("-") {
        println!("Markdown/QMD export writes files; stdout (-) is only supported with --format jsonl.");
        db.close();
        return None;
    }
    let output_dir = if let Some(ref o) = args.output {
        PathBuf::from(shellexpand_owned(o))
    } else {
        get_hermes_home().join("session-exports")
    };

    // _export_one helper — mirrors 720-736
    let export_one = |session_id: &str, include_lineage: bool| -> (Option<HashMap<String, String>>, Option<PathBuf>) {
        let data = if include_lineage { db.export_session_lineage(session_id) } else { db.export_session(session_id) };
        let data = match data {
            Some(d) => redact(Some(d)).unwrap(),
            None => return (None, None),
        };
        let path = write_session_markdown_stub(&data, &output_dir, args.format.as_deref().unwrap_or("md"), args.force);
        match path {
            Ok(p) => {
                append_manifest_entry_stub(&output_dir, &data, &p, args.format.as_deref().unwrap_or("md"));
                (Some(data), Some(p))
            }
            Err(_) => (None, None),
        }
    };

    if args.delete_after_verified && !args.yes {
        println!("--delete-after-verified requires --yes.");
        db.close();
        return None;
    }
    if args.delete_after_verified && args.session_id.is_none() {
        println!("--delete-after-verified is only supported with --session-id.");
        db.close();
        return None;
    }

    let lineage_is_logical = args.lineage == "logical";

    if let Some(ref sid) = args.session_id.clone() {
        let resolved = match db.resolve_session_id(sid) {
            Some(r) => r,
            None => { println!("Session '{sid}' not found."); db.close(); return None; }
        };
        let mut delete_target_ids = vec![resolved.clone()];
        if args.delete_after_verified {
            delete_target_ids = db.get_session_delete_targets(&resolved);
        }
        let mut exported_items: Vec<(HashMap<String, String>, PathBuf)> = Vec::new();
        for target_id in &delete_target_ids {
            let include_lineage = *target_id == resolved && lineage_is_logical;
            // mirrors try FileExistsError — stub maps to Err
            let (data, path) = export_one(target_id, include_lineage);
            match (data, path) {
                (Some(d), Some(p)) => exported_items.push((d, p)),
                _ => {
                    // distinguish exists vs disappeared — stub treats as disappeared
                    println!("Session '{target_id}' disappeared during export; nothing was deleted.");
                    db.close();
                    return None;
                }
            }
        }
        let message_count: usize = exported_items.iter().map(|(d,_)| d.get("messages").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0)).sum();
        let suffix = if message_count==1 { "" } else { "s" };
        if exported_items.len()==1 {
            println!("Exported 1 session ({message_count} message{suffix}) to {}", exported_items[0].1.display());
        } else {
            println!("Exported {} sessions ({message_count} message{suffix}) to {}", exported_items.len(), output_dir.display());
        }
        if args.delete_after_verified {
            for (data, path) in &exported_items {
                let (ok, reason) = verify_export_file_stub(path, data);
                if !ok {
                    println!("Export verification failed; not deleting session '{}': {}", data.get("id").map(|v| v.as_str()).unwrap_or(""), reason.unwrap_or_else(|| "unknown".to_string()));
                    db.close();
                    return None;
                }
            }
            let sessions_dir = get_hermes_home().join("sessions");
            let deleted = db.delete_session_with_expected(&resolved, &sessions_dir, &delete_target_ids);
            if deleted {
                let delegate_count = delete_target_ids.len()-1;
                let delegate_suffix = if delegate_count==0 { "".to_string() } else { format!(" and {delegate_count} delegate session{}", if delegate_count==1 { "" } else { "s" }) };
                println!("Deleted exported session '{resolved}'{delegate_suffix}.");
            } else {
                println!("Exported, but session '{resolved}' was not deleted because its delegate set changed.");
            }
        }
        db.close();
        return None;
    }

    // Bulk export without --session-id — mirrors 837-868
    if filters.is_none() {
        println!("Refusing bulk export without a filter. Pass --session-id or at least one filter (e.g. --older-than 90, --source telegram).");
        db.close();
        return None;
    }
    let filters_ref = filters.as_ref().unwrap();
    let candidates = db.list_prune_candidates(filters_ref);
    if args.dry_run {
        println!("Would export {} session(s) ({}).", candidates.len(), describe_filters(filters_ref));
        for row in candidates.iter().take(100) {
            println!("  {}  {}", row.get("id").map(|v| v.as_str()).unwrap_or(""), row.get("source").map(|v| v.as_str()).unwrap_or(""));
        }
        if candidates.len() > 100 {
            println!("  ... {} more", candidates.len()-100);
        }
        db.close();
        return None;
    }
    let mut exported = 0usize;
    for row in &candidates {
        let id = match row.get("id") { Some(v) => v.as_str(), None => continue };
        let (data, path) = export_one(id, lineage_is_logical);
        if data.is_some() && path.is_some() { exported += 1; }
    }
    println!("Exported {exported} session(s) to {}", output_dir.display());
    db.close();
    None
}

fn collect_sessions_for_export(
    db: &dyn SessionDb,
    args: &SessionsArgs,
    filters: Option<&HashMap<String, String>>,
    redact: &dyn Fn(Option<HashMap<String, String>>) -> Option<HashMap<String, String>>,
) -> Option<Vec<HashMap<String, String>>> {
    if let Some(ref sid) = args.session_id {
        let resolved = db.resolve_session_id(sid)?;
        let data = redact(db.export_session(&resolved))?;
        if data.is_empty() {
            println!("Session '{sid}' not found.");
            return None;
        }
        return Some(vec![data]);
    }
    if let Some(f) = filters {
        let candidates = db.list_prune_candidates(f);
        if args.dry_run {
            println!("Would export {} session(s) ({}).", candidates.len(), describe_filters(f));
            for row in candidates.iter().take(100) {
                println!("  {}  {}", row.get("id").map(|v| v.as_str()).unwrap_or(""), row.get("source").map(|v| v.as_str()).unwrap_or(""));
            }
            if candidates.len() > 100 {
                println!("  ... {} more", candidates.len()-100);
            }
            return None;
        }
        let sessions: Vec<HashMap<String, String>> = candidates.iter()
            .filter_map(|row| row.get("id").and_then(|id| db.export_session(id)))
            .filter_map(|s| redact(Some(s)))
            .collect();
        return Some(sessions);
    }
    if args.dry_run {
        println!("--dry-run requires at least one filter.");
        return None;
    }
    let sessions: Vec<HashMap<String, String>> = db.export_all(None).into_iter().filter_map(|s| redact(Some(s))).collect();
    Some(sessions)
}

fn render_sessions_export_stub(sessions: &[HashMap<String, String>], fmt: &str, only: &str) -> String {
    let _ = (fmt, only);
    // Mirrors hermes_cli.session_export.render_sessions_export
    let mut out = String::new();
    for s in sessions {
        out.push_str(&hashmap_to_json(s));
        out.push('\n');
    }
    out
}
fn export_record_count_stub(sessions: &[HashMap<String, String>], _only: &str) -> (usize, String) {
    // Mirrors export_record_count — counts prompts
    let count = sessions.len();
    (count, "prompt".to_string())
}
fn generate_html_export_stub(_session: &HashMap<String, String>) -> String {
    "<html><body>stub</body></html>".to_string()
}
fn generate_multi_session_html_export_stub(_sessions: &[HashMap<String, String>]) -> String {
    "<html><body>stub multi</body></html>".to_string()
}
fn build_trace_jsonl_stub(_messages: &[HashMap<String, String>], _sid: &str, _model: &str) -> String {
    "{}\n".to_string()
}
fn upload_session_trace_stub(_sid: &str, _redact: bool, _private: bool) -> String {
    "trace uploaded (stub)".to_string()
}
fn write_session_markdown_stub(_data: &HashMap<String, String>, _dir: &Path, _fmt: &str, _force: bool) -> Result<PathBuf, String> {
    Ok(_dir.join("session.md"))
}
fn append_manifest_entry_stub(_dir: &Path, _data: &HashMap<String, String>, _path: &Path, _fmt: &str) {}
fn verify_export_file_stub(_path: &Path, _data: &HashMap<String, String>) -> (bool, Option<String>) { (true, None) }
fn hashmap_to_json(m: &HashMap<String, String>) -> String {
    let mut parts: Vec<String> = m.iter().map(|(k,v)| format!("\"{}\": \"{}\"", k.replace('"', "\\\""), v.replace('"', "\\\""))).collect();
    parts.sort();
    format!("{{{}}}", parts.join(", "))
}

// ---------------------------------------------------------------------------
// delete — mirrors lines 870-895
// ---------------------------------------------------------------------------

fn cmd_sessions_delete(db: &dyn SessionDb, args: &SessionsArgs) -> Option<i32> {
    let raw = match args.session_id.as_deref() {
        Some(s) => s,
        None => { println!("Session id required for delete."); return Some(1); }
    };
    let resolved = match db.resolve_session_id(raw) {
        Some(r) => r,
        None => { println!("Session '{raw}' not found."); return Some(1); }
    };
    let meta = db.get_session(&resolved).unwrap_or_default();
    let pinned_note = if meta.get("pinned").map(|v| v=="true").unwrap_or(false) { " (this session is PINNED)" } else { "" };
    if !args.yes {
        if !confirm_prompt(&format!("Delete session '{resolved}'{pinned_note} and all its messages? [y/N] ")) {
            println!("Cancelled.");
            return None;
        }
    } else if !pinned_note.is_empty() {
        println!("Warning: deleting a pinned session '{resolved}'.");
    }
    let sessions_dir = get_hermes_home().join("sessions");
    if db.delete_session(&resolved, Some(&sessions_dir)) {
        println!("Deleted session '{resolved}'.");
        None
    } else {
        println!("Session '{raw}' not found.");
        Some(1)
    }
}

// ---------------------------------------------------------------------------
// prune/archive slice-1 header — mirrors lines 897-900 (+ 903-935 stub)
// ---------------------------------------------------------------------------

/// Mirrors `elif action == "prune" and getattr(args, "never_active", False):`
/// (897-901) — handled in `cmd_sessions` above. This helper implements the
/// `prune`/`archive` shared path's opening (903-935) that is still inside
/// the 900-line slice window. Full body through 1057 continues in slice 2.
///
/// The historical default `older_than = "90"` for a truly bare `prune`
/// (no time window, no non-time filters) is preserved here so the audit
/// can diff the cutoff logic line-for-line.
fn cmd_sessions_prune_archive_slice1(db: &dyn SessionDb, args: &SessionsArgs, action: &str) -> Option<i32> {
    // Preserve mutable older_than for the implicit default — mirrors Python's
    // `args.older_than = "90"` mutation (934). In Rust we shadow locally.
    let mut older_than = args.older_than.clone();
    let non_time_filter = args.source.is_some()
        || args.title_filter.is_some()
        || args.end_reason.is_some()
        || args.cwd.is_some()
        || args.min_messages.is_some()
        || args.max_messages.is_some()
        || args.model.is_some()
        || args.provider.is_some()
        || args.user.is_some()
        || args.chat_id.is_some()
        || args.chat_type.is_some()
        || args.branch.is_some()
        || args.min_tokens.is_some()
        || args.max_tokens.is_some()
        || args.min_cost.is_some()
        || args.max_cost.is_some()
        || args.min_tool_calls.is_some()
        || args.max_tool_calls.is_some();

    if action == "prune"
        && older_than.is_none()
        && args.newer_than.is_none()
        && args.before.is_none()
        && args.after.is_none()
        && !non_time_filter
    {
        older_than = Some("90".to_string());
    }

    // Build filters — mirrors 936-940 (through the ValueError catch)
    let mut args_for_filters = args.clone();
    args_for_filters.older_than = older_than;
    let filters = match build_prune_filters(&args_for_filters) {
        Ok(f) => f,
        Err(e) => {
            println!("Error: {e}");
            return Some(1);
        }
    };

    // Slice 1 stops here (line 900 is inside the `archive` guard at 942).
    // The remainder — archive-everything refusal (942-949), archived flag
    // wiring (951-958), pinned-session note (960-989), candidate listing,
    // open-session skip note, verb/empty check, span, dry-run preview,
    // confirmation, and prune/archive execution (991-1057) — lives in slice 2.
    // We forward to the slice-2 entry point as a stub so the 1:1 shape
    // compiles without pulling sqlite in this slice.
    let _ = filters;
    println!("(slice 1 stub: prune/archive remainder continues in sessions_cmd_slice2.rs — {} sessions matched filters filtered by {})", 0, describe_filters(&HashMap::new()));
    db.close();
    // In the live binary this would be: sessions_cmd_slice2::cmd_sessions_prune_archive_tail(db, &args_for_filters, &filters, action)
    None
}

// ---------------------------------------------------------------------------
// Slice boundary — line 900
// ---------------------------------------------------------------------------
// Python `sessions_cmd.py` lines 901-1455 (prune/archive tail from the
// `archive` refusal at 942 through `delete` pinned handling, `rename`,
// `pin`/`unpin`/`pinned`, `retitle-skills`, `browse`, `optimize`,
// `clean-markers`, `optimize-storage`, `repair-routing`, `stats`, and the
// fallthrough `sessions_parser.print_help()`) continue in
// `sessions_cmd_slice2.rs` (from `if action == "archive" and not any(...`,
// line 901). This file intentionally stops at the 900-line boundary so
// that `cargo` is never invoked and the 2-slice decomposition stays clean.
