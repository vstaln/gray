//! disk-cleanup plugin — auto-cleanup of ephemeral Hermes session files.
//!
//! 1:1 port of `reference/NousResearch/hermes-agent/plugins/disk-cleanup/__init__.py` (316 LOC).
//!
//! Wires three behaviours (mirrors Python module docstring lines 1-18):
//!
//! 1. `post_tool_call` hook — inspects `write_file` and `terminal` tool results
//!    for newly-created paths matching test/temp patterns under `HERMES_HOME`
//!    and tracks them silently. Zero agent compliance required.
//!
//! 2. `on_session_end` hook — when any test files were auto-tracked during
//!    the just-finished turn, runs `disk_cleanup::quick` and logs a single
//!    line to `$HERMES_HOME/disk-cleanup/cleanup.log`.
//!
//! 3. `/disk-cleanup` slash command — manual `status`, `dry-run`, `quick`,
//!    `deep`, `track`, `forget`.
//!
//! Replaces PR #12212's skill-plus-script design: the agent no longer needs
//! to remember to run commands.
//!
//! Python surface ported line-for-line:
//! - `_recent_test_tracks` + `_lock` (lines 39-40)
//! - `_WRITE_FILE_PATH_KEY`, `_TERMINAL_PATH_REGEX` (lines 44-45)
//! - `_tracker_key`, `_record_track`, `_drain` (lines 52-70)
//! - `_attempt_track`, `_extract_paths_from_write_file`, `_extract_paths_from_patch`,
//!   `_extract_paths_from_terminal` (lines 72-121)
//! - `_on_post_tool_call`, `_on_session_end` (lines 128-189)
//! - `_HELP_TEXT`, `_fmt_summary`, `_handle_slash` (lines 195-302)
//! - `register(ctx)` (lines 309-316)

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

// Use sibling disk_cleanup module as `dg` — mirrors `from . import disk_cleanup as dg` (line 30).
use crate::disk_cleanup as dg;

// ---------------------------------------------------------------------------
// Module-level state — mirrors lines 39-45
// ---------------------------------------------------------------------------

/// Per-task set of "test files newly tracked this turn". Keyed by task_id
/// (or session_id as fallback) so on_session_end can decide whether to run
/// cleanup. Guarded by a lock — post_tool_call can fire concurrently on
/// parallel tool calls. Mirrors `_recent_test_tracks: Dict[str, Set[str]] = {}`
/// + `_lock = threading.Lock()` (lines 39-40).
static RECENT_TEST_TRACKS: OnceLock<Mutex<HashMap<String, HashSet<String>>>> = OnceLock::new();

fn recent_tracks() -> &'static Mutex<HashMap<String, HashSet<String>>> {
    RECENT_TEST_TRACKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Mirrors `_WRITE_FILE_PATH_KEY = "path"` (line 44).
pub const WRITE_FILE_PATH_KEY: &str = "path";

/// Mirrors `_TERMINAL_PATH_REGEX = re.compile(r"(?:^|\s)(/[^\s'\"`]+|\~/[^\s'\"`]+)")` (line 45).
/// Stored as the raw pattern string for documentation; the Rust impl uses
/// `terminal_path_regex_findall` which implements the same semantics without
/// the `regex` crate (keeps `workspace.dependencies` lean, `NEVER cargo`).
pub const TERMINAL_PATH_REGEX: &str = r"(?:^|\s)(/[^\s'\"`]+|\~/[^\s'\"`]+)";

// ---------------------------------------------------------------------------
// Helpers — mirrors lines 52-86
// ---------------------------------------------------------------------------

/// Mirrors `def _tracker_key(task_id: str, session_id: str) -> str: return task_id or session_id or "default"` (lines 52-53).
pub fn tracker_key(task_id: &str, session_id: &str) -> String {
    if !task_id.is_empty() {
        task_id.to_string()
    } else if !session_id.is_empty() {
        session_id.to_string()
    } else {
        "default".to_string()
    }
}

/// Mirrors `def _record_track(task_id: str, session_id: str, path: Path, category: str)` (lines 56-62).
/// Only records `test` categories — mirrors early return `if category != "test": return`.
pub fn record_track(task_id: &str, session_id: &str, path: &Path, category: &str) {
    if category != "test" {
        return;
    }
    let key = tracker_key(task_id, session_id);
    if let Ok(mut map) = recent_tracks().lock() {
        map.entry(key).or_default().insert(path.to_string_lossy().to_string());
    }
}

/// Mirrors `def _drain(task_id: str, session_id: str) -> Set[str]` (lines 65-70).
/// Pops the set for the key, returning empty if absent.
pub fn drain(task_id: &str, session_id: &str) -> HashSet<String> {
    let key = tracker_key(task_id, session_id);
    if let Ok(mut map) = recent_tracks().lock() {
        map.remove(&key).unwrap_or_default()
    } else {
        HashSet::new()
    }
}

/// Snapshot all current keys — used by `on_session_end` to sweep task buckets
/// (lines 169-173). Returns a clone of the key set outside the lock.
fn snapshot_keys() -> Vec<String> {
    recent_tracks()
        .lock()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// Pop a single key if it exists and is not the given session_id / empty.
/// Mirrors the loop at lines 171-173.
fn pop_key_if_task_bucket(key: &str, session_id: &str) {
    if key.is_empty() || key == session_id {
        return;
    }
    if let Ok(mut map) = recent_tracks().lock() {
        map.remove(key);
    }
}

fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    } else if s.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(&s[2..]);
        }
    }
    path.to_path_buf()
}

/// Best-effort audit log writer — mirrors `dg._log` / `disk_cleanup::_log` (disk_cleanup.py:88-98).
/// Never lets the audit log break the agent loop (catches OSError in Python).
fn dg_log(message: &str) {
    // Reuse the same log file path as `disk_cleanup::get_log_file()` to keep
    // AUTO_QUICK and other messages in the single `cleanup.log`.
    let log_file = dg::get_log_file();
    if let Some(parent) = log_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Timestamp "%Y-%m-%d %H:%M:%S" UTC — best-effort without `chrono`.
    // Falls back to secs since epoch if clock fails, matching
    // `disk_cleanup::log_message`'s std fallback.
    let ts = {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // Try to format as readable UTC if `chrono` were linked; otherwise use secs.
        // To keep `NEVER cargo` and zero deps, emit secs string here — still
        // searchable and never breaks.
        format!("{}", now_secs)
    };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log_file) {
        use std::io::Write;
        let _ = writeln!(f, "[{}] {}", ts, message);
    }
}

/// Mirrors `def _attempt_track(path_str: str, task_id: str, session_id: str)` (lines 72-85).
/// Best-effort auto-track. Never raises.
pub fn attempt_track(path_str: &str, task_id: &str, session_id: &str) {
    let expanded = expand_tilde(Path::new(path_str));
    if !expanded.exists() {
        return;
    }
    let category = match dg::guess_category(&expanded) {
        Some(c) => c,
        None => return,
    };
    // `dg.track(str(p), category, silent=True)` — mirrors line 83
    let newly = dg::track(&expanded.to_string_lossy(), &category, true);
    if newly {
        record_track(task_id, session_id, &expanded, &category);
    }
}

// ---------------------------------------------------------------------------
// Path extractors — mirrors lines 88-121
// ---------------------------------------------------------------------------

/// Mirrors `def _extract_paths_from_write_file(args: Dict[str, Any]) -> Set[str]` (lines 88-90).
pub fn extract_paths_from_write_file(args: &Value) -> HashSet<String> {
    let mut out = HashSet::new();
    if let Some(map) = args.as_object() {
        if let Some(Value::String(path)) = map.get(WRITE_FILE_PATH_KEY) {
            if !path.is_empty() {
                out.insert(path.clone());
            }
        }
    }
    out
}

/// Mirrors `def _extract_paths_from_patch(args: Dict[str, Any]) -> Set[str]` (lines 93-100).
pub fn extract_paths_from_patch(args: &Value) -> HashSet<String> {
    let mut out = HashSet::new();
    if let Some(map) = args.as_object() {
        if let Some(Value::String(path)) = map.get("path") {
            if !path.is_empty() {
                out.insert(path.clone());
            }
        }
    }
    out
}

/// Minimal `shlex.split` (POSIX, `errors='replace'`-style). Mirrors
/// `shlex.split(cmd, posix=True)` at line 112. Returns `Err` on unclosed
/// quotes (mapped to Python's `ValueError`), which the caller ignores.
fn shlex_split(cmd: &str) -> Result<Vec<String>, String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut in_token = false;

    for ch in cmd.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            in_token = true;
            continue;
        }
        if ch == '\\' && !in_single {
            escaped = true;
            continue;
        }
        if ch == '\'' && !in_double {
            in_single = !in_single;
            in_token = true;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            in_token = true;
            continue;
        }
        if !in_single && !in_double && ch.is_whitespace() {
            if in_token {
                tokens.push(current.clone());
                current.clear();
                in_token = false;
            }
            continue;
        }
        current.push(ch);
        in_token = true;
    }
    if in_single || in_double {
        return Err("No closing quotation".to_string());
    }
    if escaped {
        // Trailing backslash — treat as literal
        current.push('\\');
    }
    if in_token || !current.is_empty() {
        tokens.push(current);
    }
    Ok(tokens)
}

/// Implements the semantics of `_TERMINAL_PATH_REGEX.findall(result)` (line 119).
/// Pattern: `(?:^|\s)(/[^\s'\"`]+|\~/[^\s'\"`]+)` — captures absolute paths
/// and `~/` paths delimited by whitespace or start, terminated by whitespace
/// or quotes/backticks. Implemented without `regex` crate.
fn terminal_path_regex_findall(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        let is_start = i == 0 || chars[i - 1].is_whitespace();
        if is_start {
            if chars[i] == '/' {
                let mut j = i + 1;
                while j < n
                    && !chars[j].is_whitespace()
                    && chars[j] != '\''
                    && chars[j] != '"'
                    && chars[j] != '`'
                {
                    j += 1;
                }
                if j > i + 1 {
                    // Strip trailing punctuation that is not part of a real path
                    // in many terminal outputs (e.g. "/tmp/foo,").
                    // Python's regex eats until whitespace/quote, so ',' is eaten;
                    // we keep the same length. The downstream `is_safe_path` /
                    // `guess_category` filters will reject non-existent paths anyway.
                    let path: String = chars[i..j].iter().collect();
                    out.insert(path);
                }
                i = j;
                continue;
            } else if i + 1 < n && chars[i] == '~' && chars[i + 1] == '/' {
                let mut j = i + 2;
                while j < n
                    && !chars[j].is_whitespace()
                    && chars[j] != '\''
                    && chars[j] != '"'
                    && chars[j] != '`'
                {
                    j += 1;
                }
                if j > i + 2 {
                    let path: String = chars[i..j].iter().collect();
                    out.insert(path);
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Mirrors `def _extract_paths_from_terminal(args: Dict[str, Any], result: str)` (lines 103-121).
pub fn extract_paths_from_terminal(args: &Value, result: &str) -> HashSet<String> {
    let mut paths: HashSet<String> = HashSet::new();

    // Tokenise the command — catches `touch /tmp/hermes-x/test_foo.py` (lines 108-116)
    if let Some(map) = args.as_object() {
        if let Some(Value::String(cmd)) = map.get("command") {
            if !cmd.is_empty() {
                if let Ok(tokens) = shlex_split(cmd) {
                    for tok in tokens {
                        if tok.starts_with('/') || tok.starts_with('~') {
                            // Python checks `tok.startswith(("/", "~"))` — any ~, not just ~/
                            // We preserve that; downstream `guess_category` / `is_safe_path`
                            // will filter non-`~/` paths like `~user` conservatively.
                            paths.insert(tok);
                        }
                    }
                }
                // ValueError is silently ignored — mirrors `except ValueError: pass`
            }
        }
    }

    // Only scan the result text if it's a reasonable size (avoid 50KB dumps) (lines 117-120)
    if result.len() < 4096 {
        for m in terminal_path_regex_findall(result) {
            paths.insert(m);
        }
    }
    paths
}

// ---------------------------------------------------------------------------
// Hooks — mirrors lines 128-189
// ---------------------------------------------------------------------------

/// Mirrors `def _on_post_tool_call(tool_name: str = "", args: Optional[Dict[str, Any]] = None, result: Any = None, task_id: str = "", session_id: str = "", tool_call_id: str = "", **_: Any)` (lines 128-152).
pub fn on_post_tool_call(
    tool_name: &str,
    args: Option<&Value>,
    result: Option<&Value>,
    task_id: &str,
    session_id: &str,
    _tool_call_id: &str,
) {
    let args_val = match args {
        Some(v) if v.is_object() => v,
        _ => return,
    };

    let mut candidates: HashSet<String> = HashSet::new();
    if tool_name == "write_file" {
        candidates = extract_paths_from_write_file(args_val);
    } else if tool_name == "patch" {
        candidates = extract_paths_from_patch(args_val);
    } else if tool_name == "terminal" {
        let result_str = result.and_then(|v| v.as_str()).unwrap_or("");
        candidates = extract_paths_from_terminal(args_val, result_str);
    } else {
        return;
    }

    for path_str in candidates {
        attempt_track(&path_str, task_id, session_id);
    }
}

/// Mirrors `def _on_session_end(session_id: str = "", completed: bool = True, interrupted: bool = False, **_: Any)` (lines 155-189).
pub fn on_session_end(session_id: &str, _completed: bool, _interrupted: bool) {
    // Drain both task-level and session-level buckets. In practice only one
    // is populated per turn; the other is empty. Mirrors lines 163-176.
    let drained_session = drain("", session_id);

    // Also drain any task-scoped buckets that happen to exist. This is a
    // cheap sweep: if an agent spawned subagents (each with their own
    // task_id) they'll have recorded into separate buckets; we want to
    // cleanup them all at session end. Mirrors lines 169-173.
    let task_keys = snapshot_keys();
    for key in &task_keys {
        if !key.is_empty() && key != session_id {
            if let Ok(mut map) = recent_tracks().lock() {
                map.remove(key);
            }
        }
    }

    if drained_session.is_empty() && task_keys.is_empty() {
        return;
    }
    // Also need to handle case where task_keys contained only session_id or was empty
    // but drained_session was non-empty — the early return already checks that.
    // The Python check `if not drained_session and not task_buckets: return` uses
    // the pre-pop task_buckets snapshot (which may contain session_id key if it
    // existed before drain? but drain already removed it, so snapshot after
    // is correct). We preserve exact semantics: if both empty, no cleanup.

    let summary = dg::quick();
    // Python wraps in try/except Exception and logs debug on failure
    // (dg::quick in Rust returns a value and never throws; errors are inside
    // the QuickResult.errors vec). We treat the call as infallible.

    if summary.deleted > 0 || summary.empty_dirs > 0 {
        dg_log(&format!(
            "AUTO_QUICK (session_end): deleted={} dirs={} freed={}",
            summary.deleted,
            summary.empty_dirs,
            dg::fmt_size(summary.freed as f64)
        ));
    }
}

// ---------------------------------------------------------------------------
// Slash command — mirrors lines 195-302
// ---------------------------------------------------------------------------

/// Mirrors `_HELP_TEXT` (lines 195-210).
pub const HELP_TEXT: &str = "\
/disk-cleanup — ephemeral-file cleanup

Subcommands:
  status                     Per-category breakdown + top-10 largest
  dry-run                    Preview what quick/deep would delete
  quick                      Run safe cleanup now (no prompts)
  deep                       Run quick, then list items that need prompts
  track <path> <category>    Manually add a path to tracking
  forget <path>              Stop tracking a path (does not delete)

Categories: temp | test | research | download | chrome-profile | cron-output | other

All operations are scoped to HERMES_HOME and /tmp/hermes-*.
Test files are auto-tracked on write_file / terminal and auto-cleaned at session end.
";

/// Mirrors `def _fmt_summary(summary: Dict[str, Any]) -> str` (lines 213-220).
pub fn fmt_summary(summary: &dg::QuickResult) -> String {
    let mut base = format!(
        "[disk-cleanup] Cleaned {} files + {} empty dirs, freed {}.",
        summary.deleted,
        summary.empty_dirs,
        dg::fmt_size(summary.freed as f64)
    );
    if !summary.errors.is_empty() {
        base.push_str(&format!("\n  {} error(s); see cleanup.log.", summary.errors.len()));
    }
    base
}

/// Mirrors `def _handle_slash(raw_args: str) -> Optional[str]` (lines 223-302).
pub fn handle_slash(raw_args: &str) -> Option<String> {
    let argv: Vec<String> = raw_args
        .trim()
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();

    if argv.is_empty() || matches!(argv[0].as_str(), "help" | "-h" | "--help") {
        return Some(HELP_TEXT.to_string());
    }

    let sub = argv[0].as_str();

    if sub == "status" {
        return Some(dg::format_status(&dg::status()));
    }

    if sub == "dry-run" {
        let (auto, prompt) = dg::dry_run();
        let auto_size: u64 = auto.iter().map(|i| i.size).sum();
        let prompt_size: u64 = prompt.iter().map(|i| i.size).sum();
        let mut lines: Vec<String> = Vec::new();
        lines.push("Dry-run preview (nothing deleted):".to_string());
        lines.push(format!(
            "  Auto-delete : {} files ({})",
            auto.len(),
            dg::fmt_size(auto_size as f64)
        ));
        for item in &auto {
            lines.push(format!("    [{}] {}", item.category, item.path));
        }
        lines.push(format!(
            "  Needs prompt: {} files ({})",
            prompt.len(),
            dg::fmt_size(prompt_size as f64)
        ));
        for item in &prompt {
            lines.push(format!("    [{}] {}", item.category, item.path));
        }
        lines.push(format!(
            "\n  Total potential: {}",
            dg::fmt_size((auto_size + prompt_size) as f64)
        ));
        return Some(lines.join("\n"));
    }

    if sub == "quick" {
        return Some(fmt_summary(&dg::quick()));
    }

    if sub == "deep" {
        // In-session deep can't prompt the user interactively — show what
        // quick cleaned plus the items that WOULD need confirmation.
        // Mirrors lines 256-274.
        let quick_summary = dg::quick();
        let (_auto, prompt_items) = dg::dry_run();
        let mut lines: Vec<String> = Vec::new();
        lines.push(fmt_summary(&quick_summary));
        if !prompt_items.is_empty() {
            let size: u64 = prompt_items.iter().map(|i| i.size).sum();
            lines.push(format!(
                "\n{} item(s) need confirmation ({}):",
                prompt_items.len(),
                dg::fmt_size(size as f64)
            ));
            for item in &prompt_items {
                lines.push(format!("  [{}] {}", item.category, item.path));
            }
            lines.push(
                "\nRun `/disk-cleanup forget <path>` to skip, or delete manually via terminal."
                    .to_string(),
            );
        }
        return Some(lines.join("\n"));
    }

    if sub == "track" {
        if argv.len() < 3 {
            return Some("Usage: /disk-cleanup track <path> <category>".to_string());
        }
        let path_arg = &argv[1];
        let category = &argv[2];
        if !dg::ALLOWED_CATEGORIES.contains(&category.as_str()) {
            let mut allowed: Vec<String> = dg::ALLOWED_CATEGORIES.iter().map(|s| s.to_string()).collect();
            allowed.sort();
            return Some(format!(
                "Unknown category '{}'. Allowed: {:?}",
                category, allowed
            ));
        }
        if dg::track(path_arg, category, true) {
            return Some(format!("Tracked {} as '{}'.", path_arg, category));
        }
        return Some(format!(
            "Not tracked (already present, missing, or outside HERMES_HOME): {}",
            path_arg
        ));
    }

    if sub == "forget" {
        if argv.len() < 2 {
            return Some("Usage: /disk-cleanup forget <path>".to_string());
        }
        let n = dg::forget(&argv[1]);
        if n > 0 {
            let word = if n == 1 { "entry" } else { "entries" };
            return Some(format!("Removed {} tracking {} for {}.", n, word, argv[1]));
        } else {
            return Some(format!("Not found in tracking: {}", argv[1]));
        }
    }

    Some(format!("Unknown subcommand: {}\n\n{}", sub, HELP_TEXT))
}

// ---------------------------------------------------------------------------
// Plugin registration — mirrors lines 309-316
// ---------------------------------------------------------------------------

/// Minimal `ctx` trait for plugin registration — mirrors `hermes_cli.plugins.PluginContext`
/// with `register_hook` and `register_command`. The real gateway provides richer
/// signatures (`register_hook(name, handler)` / `register_command(name, handler, description)`),
/// but the Rust port keeps the trait minimal so the `register` flow is testable
/// without importing the full plugin runtime.
pub trait PluginContext {
    fn register_hook(&mut self, hook: &str);
    fn register_command(&mut self, name: &str, description: &str);
}

/// Mirrors `def register(ctx) -> None` (lines 309-316).
///
/// ```python
/// def register(ctx) -> None:
///     ctx.register_hook("post_tool_call", _on_post_tool_call)
///     ctx.register_hook("on_session_end", _on_session_end)
///     ctx.register_command("disk-cleanup", handler=_handle_slash, description="...")
/// ```
pub fn register(ctx: &mut dyn PluginContext) {
    ctx.register_hook("post_tool_call");
    ctx.register_hook("on_session_end");
    ctx.register_command(
        "disk-cleanup",
        "Track and clean up ephemeral Hermes session files.",
    );
    // Handler bindings (`_on_post_tool_call`, `_on_session_end`, `_handle_slash`)
    // are the Rust equivalents `on_post_tool_call`, `on_session_end`, `handle_slash`.
    // The trait records names; the runtime wires the actual function pointers
    // when the broader plugin registry supports them.
    let _ = on_post_tool_call as fn(&str, Option<&Value>, Option<&Value>, &str, &str, &str);
    let _ = on_session_end as fn(&str, bool, bool);
    let _ = handle_slash as fn(&str) -> Option<String>;
}

// ---------------------------------------------------------------------------
// Tests — mirrors Python contract invariants (no cargo network needed)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Helpers to isolate HERMES_HOME per test
    fn with_temp_hermes_home<F: FnOnce()>(f: F) {
        let tmp = std::env::temp_dir().join(format!(
            "hermes-disk-cleanup-init-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&tmp);
        let prev = std::env::var("HERMES_HOME").ok();
        unsafe { std::env::set_var("HERMES_HOME", &tmp); }
        // Also clear recent tracks between tests
        if let Ok(mut map) = recent_tracks().lock() {
            map.clear();
        }
        f();
        if let Some(v) = prev {
            unsafe { std::env::set_var("HERMES_HOME", v); }
        } else {
            unsafe { std::env::remove_var("HERMES_HOME"); }
        }
        let _ = std::fs::remove_dir_all(&tmp);
        if let Ok(mut map) = recent_tracks().lock() {
            map.clear();
        }
    }

    #[test]
    fn tracker_key_prefers_task_id() {
        assert_eq!(tracker_key("task-1", "sess-1"), "task-1");
        assert_eq!(tracker_key("", "sess-1"), "sess-1");
        assert_eq!(tracker_key("", ""), "default");
        assert_eq!(tracker_key("t", ""), "t");
    }

    #[test]
    fn record_and_drain_roundtrip() {
        with_temp_hermes_home(|| {
            let p = Path::new("/tmp/hermes-test-a/test_foo.py");
            record_track("task-1", "sess-1", p, "test");
            record_track("task-1", "sess-1", p, "test");
            // non-test category not recorded
            record_track("task-1", "sess-1", p, "temp");
            let drained = drain("task-1", "sess-1");
            assert!(drained.contains(&p.to_string_lossy().to_string()));
            assert_eq!(drained.len(), 1);
            // second drain empty
            assert!(drain("task-1", "sess-1").is_empty());
        });
    }

    #[test]
    fn record_track_ignores_non_test() {
        with_temp_hermes_home(|| {
            let p = Path::new("/tmp/hermes-x/file.txt");
            record_track("", "sess-2", p, "temp");
            assert!(drain("", "sess-2").is_empty());
            record_track("", "sess-2", p, "research");
            assert!(drain("", "sess-2").is_empty());
        });
    }

    #[test]
    fn extract_paths_from_write_file_ok() {
        let args = json!({"path": "/tmp/hermes-x/test_foo.py"});
        let set = extract_paths_from_write_file(&args);
        assert!(set.contains("/tmp/hermes-x/test_foo.py"));
        let empty = json!({});
        assert!(extract_paths_from_write_file(&empty).is_empty());
        let non_string = json!({"path": 123});
        assert!(extract_paths_from_write_file(&non_string).is_empty());
    }

    #[test]
    fn extract_paths_from_patch_ok() {
        let args = json!({"path": "/tmp/hermes-x/tmp_foo.py"});
        assert!(extract_paths_from_patch(&args).contains("/tmp/hermes-x/tmp_foo.py"));
        assert!(extract_paths_from_patch(&json!({})).is_empty());
    }

    #[test]
    fn shlex_split_basic_and_error() {
        let toks = shlex_split("touch /tmp/hermes-x/test_foo.py").unwrap();
        assert_eq!(toks, vec!["touch", "/tmp/hermes-x/test_foo.py"]);
        let toks2 = shlex_split("echo 'hello world' /tmp/a").unwrap();
        assert_eq!(toks2, vec!["echo", "hello world", "/tmp/a"]);
        assert!(shlex_split("echo 'unclosed").is_err());
        assert!(shlex_split("cmd \"quoted arg\" /tmp/x").unwrap().contains(&"/tmp/x".to_string()));
    }

    #[test]
    fn terminal_path_regex_findall_basic() {
        let t = "created /tmp/hermes-x/test_foo.py and ~/docs/file.txt";
        let set = terminal_path_regex_findall(t);
        assert!(set.contains("/tmp/hermes-x/test_foo.py"));
        assert!(set.contains("~/docs/file.txt"));
        // quoted paths still captured up to quote
        let t2 = "output: '/tmp/hermes-x/a.py' and \"/tmp/b.py\"";
        let set2 = terminal_path_regex_findall(t2);
        assert!(set2.contains("/tmp/hermes-x/a.py"));
        assert!(set2.contains("/tmp/b.py"));
        // no false positive on non-path
        assert!(!terminal_path_regex_findall("no paths here").contains("/tmp"));
    }

    #[test]
    fn extract_paths_from_terminal_command_and_result() {
        let args = json!({"command": "touch /tmp/hermes-x/test_foo.py ~/myfile.txt"});
        let result = "wrote /tmp/hermes-x/other.txt";
        let set = extract_paths_from_terminal(&args, result);
        assert!(set.contains("/tmp/hermes-x/test_foo.py"));
        assert!(set.contains("~/myfile.txt"));
        assert!(set.contains("/tmp/hermes-x/other.txt"));
        // result too large is ignored
        let big = "x".repeat(5000);
        let set2 = extract_paths_from_terminal(&json!({"command": ""}), &big);
        assert!(set2.is_empty());
        // shlex error is ignored
        let bad_args = json!({"command": "echo 'unclosed"});
        let set3 = extract_paths_from_terminal(&bad_args, "/tmp/hermes-x/a.txt");
        // command paths not added due to shlex error, but result paths still added
        assert!(set3.contains("/tmp/hermes-x/a.txt"));
        assert!(!set3.contains("echo"));
    }

    #[test]
    fn on_post_tool_call_dispatch() {
        with_temp_hermes_home(|| {
            // Need a real file under HERMES_HOME to be tracked
            let home = dg::get_hermes_home();
            let file_path = home.join("test_hello.py");
            let _ = std::fs::create_dir_all(home.clone());
            std::fs::write(&file_path, "hello").unwrap();
            let args = json!({"path": file_path.to_string_lossy().to_string()});
            on_post_tool_call("write_file", Some(&args), None, "task-1", "sess-1", "");
            let drained = drain("task-1", "sess-1");
            // guess_category for test_ file under HERMES_HOME should be test, so recorded
            assert!(drained.contains(&file_path.to_string_lossy().to_string()));
            let _ = std::fs::remove_file(&file_path);
        });
    }

    #[test]
    fn on_post_tool_call_ignores_unknown_tool() {
        with_temp_hermes_home(|| {
            let home = dg::get_hermes_home();
            let file_path = home.join("test_ignore.py");
            let _ = std::fs::create_dir_all(home.clone());
            std::fs::write(&file_path, "x").unwrap();
            let args = json!({"path": file_path.to_string_lossy().to_string()});
            on_post_tool_call("unknown_tool", Some(&args), None, "task-1", "sess-1", "");
            assert!(drain("task-1", "sess-1").is_empty());
            let _ = std::fs::remove_file(&file_path);
        });
    }

    #[test]
    fn handle_slash_help_and_unknown() {
        with_temp_hermes_home(|| {
            let help = handle_slash("").unwrap();
            assert!(help.contains("/disk-cleanup"));
            assert!(help.contains("Subcommands"));
            let help2 = handle_slash("help").unwrap();
            assert_eq!(help, help2);
            let help3 = handle_slash("--help").unwrap();
            assert_eq!(help, help3);
            let unknown = handle_slash("bogus").unwrap();
            assert!(unknown.contains("Unknown subcommand"));
            assert!(unknown.contains("/disk-cleanup"));
        });
    }

    #[test]
    fn handle_slash_track_and_forget() {
        with_temp_hermes_home(|| {
            let home = dg::get_hermes_home();
            let _ = std::fs::create_dir_all(&home);
            let file_path = home.join("test_tracked.py");
            std::fs::write(&file_path, "data").unwrap();
            // unknown category
            let r = handle_slash(&format!("track {} unknowncat", file_path.display())).unwrap();
            assert!(r.contains("Unknown category"));
            // missing args
            assert!(handle_slash("track").unwrap().contains("Usage"));
            assert!(handle_slash("forget").unwrap().contains("Usage"));
            // valid track
            let ok = handle_slash(&format!("track {} test", file_path.display())).unwrap();
            assert!(ok.contains("Tracked"));
            // duplicate track
            let dup = handle_slash(&format!("track {} test", file_path.display())).unwrap();
            assert!(dup.contains("Not tracked"));
            // forget
            let forgot = handle_slash(&format!("forget {}", file_path.display())).unwrap();
            assert!(forgot.contains("Removed 1"));
            assert!(forgot.contains("entry"));
            let notfound = handle_slash(&format!("forget {}", file_path.display())).unwrap();
            assert!(notfound.contains("Not found"));
            let _ = std::fs::remove_file(&file_path);
        });
    }

    #[test]
    fn handle_slash_status_and_dry_run_and_quick_and_deep() {
        with_temp_hermes_home(|| {
            // status on empty should not panic
            let status = handle_slash("status").unwrap();
            assert!(status.contains("Category") || status.contains("nothing tracked"));
            let dry = handle_slash("dry-run").unwrap();
            assert!(dry.contains("Dry-run preview"));
            assert!(dry.contains("Auto-delete"));
            assert!(dry.contains("Needs prompt"));
            let quick = handle_slash("quick").unwrap();
            assert!(quick.contains("[disk-cleanup] Cleaned"));
            let deep = handle_slash("deep").unwrap();
            assert!(deep.contains("[disk-cleanup] Cleaned"));
        });
    }

    #[test]
    fn fmt_summary_shows_errors() {
        let r = dg::QuickResult { deleted: 2, empty_dirs: 1, freed: 1024, errors: vec!["oops".to_string()] };
        let s = fmt_summary(&r);
        assert!(s.contains("2 files"));
        assert!(s.contains("1 empty dirs"));
        assert!(s.contains("error(s)"));
        let r2 = dg::QuickResult { deleted: 0, empty_dirs: 0, freed: 0, errors: vec![] };
        assert!(!fmt_summary(&r2).contains("error"));
    }

    #[test]
    fn plugin_register_hooks_and_commands() {
        struct Collector { hooks: Vec<String>, commands: Vec<String> }
        impl PluginContext for Collector {
            fn register_hook(&mut self, hook: &str) { self.hooks.push(hook.to_string()); }
            fn register_command(&mut self, name: &str, _desc: &str) { self.commands.push(name.to_string()); }
        }
        let mut c = Collector { hooks: vec![], commands: vec![] };
        register(&mut c);
        assert!(c.hooks.contains(&"post_tool_call".to_string()));
        assert!(c.hooks.contains(&"on_session_end".to_string()));
        assert!(c.commands.contains(&"disk-cleanup".to_string()));
    }
}
