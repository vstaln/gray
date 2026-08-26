//! hermes-cli cli_commands — slice 1/5
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/cli_commands_mixin.py`
//! slice 1/5 — lines 1–900 of 3 919 (first 900 LOC).
//! Covers: module docstring + import discipline, `CLICommandsMixin` class
//! header, `_handle_rollback_command` (list / diff / restore / file-level
//! restore, `safe` vs `--all`, skipped-edits UX, undo_last), `_handle_diff_command`
//! (shlex split, --stat/mode/paths dispatch, session vs working diff,
//! untracked + 400-line truncation), `_print_session_diff` (checkpoint baseline
//! diff via `mgr.session_diff`), `_print_diff_text` (rich console gate),
//! `_handle_snapshot_command` (list/create/restore/rewind/prune with
//! size formatting), `_handle_export_command` (-o handling, active profile
//! default), `_handle_import_command` (--name handling, wrapper creation),
//! `_handle_stop_command` (process_registry + async_delegation dual kill),
//! `_handle_agents_command` (running/finished + delegations with stalling
//! quiet timers + agent idle), `_handle_journey_command` (argparse + register_cli
//! + forced ANSI capture vs interactive), `_handle_paste_command` (Termux gate
//! + clipboard image attach), `_handle_copy_command` (assistant history scan
//! + OSC52 remote fallback), `_handle_image_command` (path resolve + suffix gate),
//! `_handle_tools_command` (capture-via-TTYBuf + disable/enable + session reset),
//! `_handle_profile_command` (slash_exec delegation), and the head of
//! `_handle_handoff_command` through the SessionDB availability gate
//! (platform validation + relay aliasing + home-channel check +
//! `_agent_running` refusal up to the `SessionDB` acquire at line 900).
//! Continued in `cli_commands_slice2.rs` (from `SessionDB` ensure-row tail
//! through `request_handoff` + 60s poll loop + `_handle_resume_command` …).
//!
//! T0691 — 1:1 port, no cargo (NEVER cargo).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-13
// ---------------------------------------------------------------------------

/// Slash-command handlers for the interactive CLI (god-file decomposition Phase 4).
///
/// This module hosts the `_handle_*_command` slash-command handlers lifted out of
/// `cli.py`'s `HermesCLI` class. `HermesCLI` inherits `CLICommandsMixin` so
/// every `self.<handler>` call resolves unchanged via the MRO — behavior-neutral.
///
/// Import discipline (mirrors gateway/slash_commands.py, PR #41886):
///   * Neutral, non-cyclic deps are imported at module top-level below.
///   * cli.py-internal symbols (the `_cprint`/`_ACCENT`/… helpers) are imported
///     LAZILY inside each handler via `from cli import ...` — that resolves at
///     call time when `cli` is fully loaded, so the mixin module never imports
///     `cli` at top level (no cycle).
/// Mirrors `hermes_cli/cli_commands_mixin.py` lines 1-13.
pub const MODULE_DOC: &str = "cli_commands_mixin: slash-command handlers — see lines 1-13";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 15-40
// ---------------------------------------------------------------------------
// Python: json, os, sys, threading, time, uuid, datetime, urllib.parse,
// rich.box, rich.markup.escape, rich.panel.Panel,
// hermes_constants (display_hermes_home, is_termux), agent.turn_context,
// hermes_cli.browser_connect (DEFAULT_BROWSER_CDP_URL …)
//
// Rust: std only (NEVER cargo). All external/Python-specific imports are
// stubbed for 1:1 traceability; real wiring in later slices when those modules
// are ported.

// --- rich stubs — mirrors lines 26-28 ---
pub mod rich_stub {
    pub const BOX_ROUNDED: &str = "rounded";
    pub fn escape(text: &str) -> String {
        // Mirrors `rich.markup.escape` — escape rich markup brackets.
        text.replace('[', "\\[").replace(']', "\\]")
    }
    pub struct Panel {
        pub content: String,
        pub title: String,
    }
    impl Panel {
        pub fn new(content: &str, title: &str) -> Self {
            Self { content: content.to_string(), title: title.to_string() }
        }
    }
}
pub use rich_stub::escape as rich_escape;

// --- hermes_constants stubs — mirrors line 30 ---
pub fn display_hermes_home() -> String {
    // Mirrors `hermes_constants.display_hermes_home()`
    if let Ok(v) = std::env::var("HERMES_HOME") {
        if !v.trim().is_empty() {
            return v;
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    format!("{home}/.hermes")
}
pub fn get_hermes_home() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_HOME") {
        if !v.trim().is_empty() {
            return PathBuf::from(v.trim());
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".hermes")
}
pub fn is_termux_environment() -> bool {
    // Mirrors `hermes_constants.is_termux`
    if std::env::var("TERMUX_VERSION").is_ok() {
        return true;
    }
    if let Ok(prefix) = std::env::var("PREFIX") {
        if prefix.contains("com.termux/files/usr") || prefix.starts_with("/data/data/com.termux/") {
            return true;
        }
    }
    false
}

// --- agent.turn_context stub — mirrors line 31 ---
pub fn extract_api_content_sidecar_stub(_content: &str) -> Option<String> { None }

// --- hermes_cli.browser_connect stubs — mirrors lines 32-40 ---
pub const DEFAULT_BROWSER_CDP_URL: &str = "http://127.0.0.1:9222";
pub fn discover_local_cdp_url() -> Option<String> { None }
pub fn find_free_debug_port() -> Option<u16> { None }
pub fn is_browser_debug_ready(_url: &str) -> bool { false }
pub fn launch_chrome_debug(_port: u16) -> bool { false }
pub fn local_port_in_use(_port: u16) -> bool { false }
pub fn manual_chrome_debug_command(_port: u16) -> String {
    format!("google-chrome --remote-debugging-port={_port}")
}

// --- stdlib import mirrors — lines 17-24 (json, os, sys, threading, time, uuid, datetime, urllib) ---
// `json` -> serde_json stub (stringly), `os` -> std::env, `time` -> std::time,
// `uuid` -> uuid stub, `urllib.parse.urlparse` -> url_parse_stub, etc.
pub fn url_parse_stub(url: &str) -> Option<(String, String)> {
    // Minimal stub: returns (scheme, netloc) for 1:1 traceability.
    let url = url.trim();
    if let Some(idx) = url.find("://") {
        let scheme = url[..idx].to_string();
        let rest = &url[idx + 3..];
        let netloc = rest.split('/').next().unwrap_or("").to_string();
        return Some((scheme, netloc));
    }
    None
}
pub fn generate_uuid_stub() -> String {
    // No uuid crate (NEVER cargo); deterministic stub for 1:1.
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    format!("00000000-0000-4000-8000-{nanos:012x}"[..36].to_string())
}

// ---------------------------------------------------------------------------
// CLI-internal lazy helpers — mirrors `from cli import ...` per handler
// ---------------------------------------------------------------------------
// Python handlers do `from cli import _cprint, _ACCENT, _DIM, _RST, ...` lazily.
// Rust stubs provide the same symbols for 1:1 call-site mapping.

pub const ACCENT: &str = "\x1b[36m";
pub const DIM: &str = "\x1b[2m";
pub const RST: &str = "\x1b[0m";

pub fn cprint(text: &str) {
    // Mirrors `cli._cprint` — print with rich/ANSI handling; here plain println.
    println!("{text}");
}
pub fn cprint_stderr(text: &str) {
    eprintln!("{text}");
}
pub fn rich_text_from_ansi_stub(text: &str) -> String {
    // Mirrors `cli._rich_text_from_ansi` — strip/convert ANSI for rich console.
    text.to_string()
}
pub fn assistant_copy_text_stub(content: Option<&str>) -> String {
    content.unwrap_or("").trim().to_string()
}
pub fn termux_example_image_path_stub(name: Option<&str>) -> String {
    let n = name.unwrap_or("image.png");
    format!("/sdcard/DCIM/{n}")
}
pub const IMAGE_EXTENSIONS: &[&str] = &[".png", ".jpg", ".jpeg", ".webp", ".gif", ".heic", ".heif"];
pub fn split_path_input_stub(raw: &str) -> (String, Option<String>) {
    // Mirrors `cli._split_path_input` — split first path token from remainder, respecting quotes.
    let raw = raw.trim();
    if raw.is_empty() {
        return (String::new(), None);
    }
    // Naïve shell-like split: find first whitespace outside quotes.
    let mut in_q: Option<char> = None;
    let mut end = raw.len();
    for (i, ch) in raw.char_indices() {
        if let Some(q) = in_q {
            if ch == q { in_q = None; }
        } else if ch == '"' || ch == '\'' {
            in_q = Some(ch);
        } else if ch.is_whitespace() {
            end = i;
            break;
        }
    }
    let token = raw[..end].trim_matches(|c| c == '"' || c == '\'').to_string();
    let remainder = raw[end..].trim().to_string();
    let rem_opt = if remainder.is_empty() { None } else { Some(remainder) };
    (token, rem_opt)
}
pub fn resolve_attachment_path_stub(token: &str) -> Option<PathBuf> {
    let p = PathBuf::from(token.trim());
    if p.exists() { Some(p) } else { None }
}

// ---------------------------------------------------------------------------
// Checkpoint / diff / snapshot etc. stubs — mirrors per-handler imports
// ---------------------------------------------------------------------------

// tools.checkpoint_manager — mirrors lines 63, 397
#[derive(Debug, Clone)]
pub struct Checkpoint {
    pub hash: String,
    pub message: String,
}
pub fn format_checkpoint_list(checkpoints: &[Checkpoint], cwd: &str) -> String {
    if checkpoints.is_empty() {
        return format!("  No checkpoints found for {cwd}");
    }
    let mut out = String::new();
    for (i, cp) in checkpoints.iter().enumerate() {
        out.push_str(&format!("  {}  {}  {}\n", i + 1, &cp.hash[..cp.hash.len().min(8)], cp.message));
    }
    out
}

// tools.working_diff — mirrors line 218
#[derive(Debug, Clone, Default)]
pub struct WorkingDiffResult {
    pub success: bool,
    pub stat: String,
    pub diff: String,
    pub untracked: Vec<String>,
    pub empty: bool,
    pub error: String,
}
pub fn collect_working_diff(_cwd: &str, _mode: &str, _paths: Option<Vec<String>>) -> WorkingDiffResult {
    // Stub: no git worktree available in slice 1
    WorkingDiffResult { success: true, empty: true, ..Default::default() }
}

// hermes_cli.backup — mirrors lines 321-324
#[derive(Debug, Clone)]
pub struct SnapshotInfo {
    pub id: String,
    pub file_count: usize,
    pub total_size: u64,
    pub label: Option<String>,
}
pub fn list_quick_snapshots_limit(_limit: Option<usize>) -> Vec<SnapshotInfo> { Vec::new() }
pub fn list_quick_snapshots() -> Vec<SnapshotInfo> { Vec::new() }
pub fn create_quick_snapshot(_label: Option<&str>) -> Option<String> { None }
pub fn restore_quick_snapshot(_id: &str) -> bool { false }
pub fn prune_quick_snapshots(_keep: usize) -> usize { 0 }

// hermes_cli.profiles — mirrors lines 407, 436
pub fn export_profile_stub(_name: &str, _output: &str) -> Result<String, String> { Ok(_output.to_string()) }
pub fn get_active_profile_name_stub() -> Option<String> { None }
pub fn import_profile_stub(_archive: &str, _name: Option<&str>) -> Result<PathBuf, String> {
    Err("not implemented in slice 1".to_string())
}
pub fn check_alias_collision_stub(_name: &str) -> bool { true }
pub fn create_wrapper_script_stub(_name: &str) -> Option<PathBuf> { None }

// tools.process_registry — mirrors lines 480, 509
#[derive(Debug, Clone)]
pub struct ProcessSession {
    pub session_id: String,
    pub status: String,
    pub command: String,
    pub uptime_seconds: u64,
}
pub fn list_process_sessions_stub() -> Vec<ProcessSession> { Vec::new() }
pub fn kill_all_stub() -> usize { 0 }
pub fn format_uptime_short_stub(secs: u64) -> String {
    if secs < 60 { format!("{secs}s") } else if secs < 3600 { format!("{}m", secs/60) } else { format!("{}h", secs/3600) }
}

// tools.async_delegation — mirrors lines 488, 526
pub fn async_active_count_stub() -> usize { 0 }
pub fn async_interrupt_all_stub(_reason: &str) -> usize { 0 }
#[derive(Debug, Clone)]
pub struct DelegationInfo {
    pub delegation_id: String,
    pub status: String,
    pub goal: String,
    pub stalled_after_quiet_seconds: Option<f64>,
    pub seconds_since_progress: Option<f64>,
    pub children_activity: Vec<ChildActivity>,
}
#[derive(Debug, Clone)]
pub struct ChildActivity {
    pub api_calls: String,
    pub current_tool: Option<String>,
    pub seconds_since_activity: Option<f64>,
}
pub fn list_async_delegations_stub() -> Vec<DelegationInfo> { Vec::new() }

// hermes_cli.journey — mirrors lines 586-587
pub fn journey_register_cli_stub(_parser: &mut JourneyParserStub) {}
#[derive(Debug, Default)]
pub struct JourneyParserStub { pub journey_action: Option<String> }
impl JourneyParserStub {
    pub fn parse_args_stub(&mut self, _args: Vec<String>) -> Result<JourneyArgsStub, String> { Ok(JourneyArgsStub::default()) }
}
#[derive(Debug, Default)]
pub struct JourneyArgsStub {
    pub journey_action: Option<String>,
    pub force_color: bool,
    pub func: Option<fn(&JourneyArgsStub)>,
}
impl JourneyArgsStub {
    pub fn call_func(&self) -> Result<(), String> { Ok(()) }
}

// hermes_cli.clipboard — mirrors lines 625, 669
pub fn has_clipboard_image_stub() -> bool { false }
pub fn write_clipboard_text_stub(_text: &str) -> bool { false }
pub fn is_remote_shell_session_stub() -> bool { false }

// hermes_cli.tools_config — mirrors lines 735-798
pub fn tools_disable_enable_command_stub(_ns: &ToolsNamespaceStub) {}
#[derive(Debug, Clone)]
pub struct ToolsNamespaceStub {
    pub tools_action: String,
    pub names: Vec<String>,
    pub platform: String,
}
pub fn get_platform_tools_stub(_config: &HashMap<String, String>, _platform: &str) -> Vec<String> { Vec::new() }

// hermes_cli.slash_exec — mirrors line 803
#[derive(Debug, Clone)]
pub struct CommandContext { pub surface: String }
#[derive(Debug, Clone)]
pub struct SlashReply { pub data: HashMap<String, String> }
pub fn execute_command_stub(_cmd: &str, _ctx: CommandContext) -> SlashReply {
    let mut data = HashMap::new();
    data.insert("profile".to_string(), "default".to_string());
    data.insert("home".to_string(), display_hermes_home());
    SlashReply { data }
}

// hermes_state — mirrors lines 832, 899
pub fn format_session_db_unavailable_stub() -> String { "Session DB unavailable".to_string() }

// gateway.config + gateway.relay — mirrors lines 845, 874
#[derive(Debug, Clone)]
pub struct GatewayPlatformConfig { pub enabled: bool }
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub platforms: HashMap<String, GatewayPlatformConfig>,
}
impl GatewayConfig {
    pub fn get_home_channel(&self, platform: &str) -> Option<HomeChannel> {
        let _ = platform; None
    }
}
#[derive(Debug, Clone)]
pub struct HomeChannel { pub chat_id: String, pub name: String }
pub fn load_gateway_config_stub() -> Result<GatewayConfig, String> { Ok(GatewayConfig { platforms: HashMap::new() }) }
pub fn relay_platform_identities_stub() -> Vec<(String, String)> { Vec::new() }
pub const RELAY_PLATFORM: &str = "relay";

// hermes_cli.config — mirrors save_config flows
pub fn load_config_stub() -> HashMap<String, String> { HashMap::new() }

// ---------------------------------------------------------------------------
// Core state — mirrors HermesCLI / CLICommandsMixin `self` surface
// ---------------------------------------------------------------------------

/// Minimal agent surface needed by slice 1 handlers.
/// Mirrors `self.agent` and `self.agent._checkpoint_mgr` in Python.
#[derive(Debug, Clone)]
pub struct CheckpointMgr {
    pub enabled: bool,
}
impl CheckpointMgr {
    pub fn list_checkpoints(&self, _cwd: &str) -> Vec<Checkpoint> { Vec::new() }
    pub fn list_all_checkpoints(&self) -> Vec<Checkpoint> { Vec::new() }
    pub fn diff(&self, _cwd: &str, _hash: &str) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("success".to_string(), "true".to_string());
        m.insert("stat".to_string(), String::new());
        m.insert("diff".to_string(), String::new());
        m
    }
    pub fn restore(&self, _cwd: &str, _hash: &str, _file: Option<&str>, _safe: bool) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("success".to_string(), "false".to_string());
        m.insert("error".to_string(), "not implemented in slice 1".to_string());
        m
    }
    pub fn session_diff(&self, _cwd: &str) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("success".to_string(), "true".to_string());
        m.insert("empty".to_string(), "true".to_string());
        m
    }
}

#[derive(Debug, Clone)]
pub struct AgentCtx {
    pub checkpoint_mgr: CheckpointMgr,
    pub session_id: String,
}

/// Rich console stub — mirrors `self.console` in Python.
#[derive(Debug, Clone)]
pub struct ConsoleStub;
impl ConsoleStub {
    pub fn print(&self, text: &str) { println!("{text}"); }
}

/// Mirrors `CLICommandsMixin` + `HermesCLI` shared state accessed by handlers.
#[derive(Debug, Clone)]
pub struct CliCommandsMixin {
    pub agent: Option<AgentCtx>,
    pub conversation_history: Vec<HashMap<String, String>>,
    pub console: Option<ConsoleStub>,
    pub attached_images: Vec<PathBuf>,
    pub agent_running: bool,
    pub session_id: String,
    pub session_db: Option<SessionDbStub>,
    pub enabled_toolsets: Vec<String>,
    pub should_exit: bool,
    pub app: Option<String>, // `self._app` presence gate for tools capture
    pub pending_resume_sessions: Option<Vec<HashMap<String, String>>>,
}

#[derive(Debug, Clone, Default)]
pub struct SessionDbStub {
    pub sessions: HashMap<String, HashMap<String, String>>,
}
impl SessionDbStub {
    pub fn get_session(&self, id: &str) -> Option<HashMap<String, String>> { self.sessions.get(id).cloned() }
    pub fn request_handoff(&self, _id: &str, _platform: &str) -> bool { true }
    pub fn get_handoff_state(&self, _id: &str) -> Option<HashMap<String, String>> { None }
    pub fn fail_handoff(&self, _id: &str, _err: &str) {}
    pub fn set_session_title(&self, _id: &str, _title: &str) {}
    pub fn end_session(&self, _id: &str, _reason: &str) {}
    pub fn reopen_session(&self, _id: &str) {}
    pub fn get_resume_conversations(&self, _id: &str) -> (Vec<HashMap<String, String>>, Vec<HashMap<String, String>>) { (Vec::new(), Vec::new()) }
    pub fn resolve_resume_session_id(&self, id: &str) -> Option<String> { Some(id.to_string()) }
}

impl Default for CliCommandsMixin {
    fn default() -> Self {
        Self {
            agent: None,
            conversation_history: Vec::new(),
            console: None,
            attached_images: Vec::new(),
            agent_running: false,
            session_id: "test-session-id".to_string(),
            session_db: None,
            enabled_toolsets: Vec::new(),
            should_exit: false,
            app: None,
            pending_resume_sessions: None,
        }
    }
}

impl CliCommandsMixin {
    pub fn new() -> Self { Self::default() }

    // -----------------------------------------------------------------------
    // Helpers — mirrors private helpers used by handlers
    // -----------------------------------------------------------------------

    fn resolve_checkpoint_ref(&self, reference: &str, checkpoints: &[Checkpoint]) -> Option<String> {
        // Mirrors `self._resolve_checkpoint_ref(ref, checkpoints)` — accepts
        // 1-indexed number or hash prefix.
        if checkpoints.is_empty() { return None; }
        let r = reference.trim();
        if let Ok(n) = r.parse::<usize>() {
            if n >= 1 && n <= checkpoints.len() {
                return Some(checkpoints[n - 1].hash.clone());
            }
            println!("  Invalid checkpoint number. Use 1-{}", checkpoints.len());
            return None;
        }
        // hash prefix match
        for cp in checkpoints {
            if cp.hash.starts_with(r) { return Some(cp.hash.clone()); }
        }
        println!("  Checkpoint not found: {r}");
        None
    }

    fn undo_last(&mut self, _prefill: bool) {
        // Mirrors `self.undo_last(prefill=False)` — pops last turn.
        if !self.conversation_history.is_empty() {
            self.conversation_history.pop();
        }
    }

    fn try_attach_clipboard_image(&mut self) -> bool {
        // Mirrors `self._try_attach_clipboard_image()` — stub.
        false
    }

    fn write_osc52_clipboard(&self, _text: &str) {
        // Mirrors `self._write_osc52_clipboard(text)`
        // Emit OSC 52 sequence via stdout (base64).
        // Stub for slice 1.
    }

    fn show_tools(&self) {
        // Mirrors `self.show_tools()` — delegate to tools_config listing.
        println!("  Available toolsets: web, terminal, file, ... (see /tools list)");
    }

    fn new_session(&mut self) {
        // Mirrors `self.new_session()` — reset conversation.
        self.conversation_history.clear();
    }

    fn list_recent_sessions(&self, _limit: usize) -> Vec<HashMap<String, String>> { Vec::new() }
    fn show_recent_sessions(&self, _reason: &str) -> bool { false }
    fn display_resumed_history(&self) {}
    fn restore_session_cwd(&self, _meta: &HashMap<String, String>) {}
    fn restore_session_yolo(&self, _meta: &HashMap<String, String>) {}
    fn restore_session_model(&self, _meta: &HashMap<String, String>) {}

    // -----------------------------------------------------------------------
    // _handle_rollback_command — mirrors lines 51-177
    // -----------------------------------------------------------------------

    /// Handle /rollback — list, diff, or restore filesystem checkpoints.
    ///
    /// Syntax:
    ///     /rollback                 — list checkpoints
    ///     /rollback <N>             — restore checkpoint N, preserving user
    ///                                 hand-edits (also undoes last chat turn)
    ///     /rollback <N> --all       — classic full restore (may overwrite
    ///                                 files you edited after Hermes did)
    ///     /rollback diff <N>        — preview changes since checkpoint N
    ///     /rollback <N> <file>      — restore a single file from checkpoint N
    /// Mirrors `CLICommandsMixin._handle_rollback_command` (51-177).
    pub fn handle_rollback_command(&mut self, command: &str) {
        // from tools.checkpoint_manager import format_checkpoint_list — lazy
        if self.agent.is_none() {
            println!("  No active agent session.");
            return;
        }
        let agent = self.agent.clone().unwrap();
        let mgr = &agent.checkpoint_mgr;
        if !mgr.enabled {
            println!("  Checkpoints are not enabled.");
            println!("  Enable with: hermes --checkpoints");
            println!("  Or in config.yaml: checkpoints: {{ enabled: true }}");
            return;
        }
        let cwd = std::env::var("TERMINAL_CWD").unwrap_or_else(|_| {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).to_string_lossy().to_string()
        });
        let parts: Vec<String> = command.split_whitespace().map(|s| s.to_string()).collect();
        let mut args: Vec<String> = if parts.len() > 1 { parts[1..].to_vec() } else { Vec::new() };

        // --all / --force: classic full restore, overwriting user edits too.
        let mut restore_all = false;
        let mut filtered: Vec<String> = Vec::new();
        for a in &args {
            if a.to_lowercase() == "--all" || a.to_lowercase() == "--force" {
                restore_all = true;
            } else {
                filtered.push(a.clone());
            }
        }
        args = filtered;

        if args.is_empty() {
            // List checkpoints — fall back to cross-project view when current
            // directory has none (#10505, reapply of PR #10633).
            let checkpoints = mgr.list_checkpoints(&cwd);
            if checkpoints.is_empty() {
                let all = mgr.list_all_checkpoints();
                if !all.is_empty() {
                    println!("  No checkpoints for {cwd} — showing all directories.");
                    println!("{}", format_checkpoint_list(&all, "all directories"));
                    return;
                }
            }
            println!("{}", format_checkpoint_list(&checkpoints, &cwd));
            return;
        }

        // Handle /rollback diff <N>
        if args[0].to_lowercase() == "diff" {
            if args.len() < 2 {
                println!("  Usage: /rollback diff <N>");
                return;
            }
            let checkpoints = mgr.list_checkpoints(&cwd);
            if checkpoints.is_empty() {
                println!("  No checkpoints found for {cwd}");
                return;
            }
            let target_hash = match self.resolve_checkpoint_ref(&args[1], &checkpoints) {
                Some(h) => h,
                None => return,
            };
            let result = mgr.diff(&cwd, &target_hash);
            let success = result.get("success").map(|v| v == "true").unwrap_or(false);
            if success {
                let stat = result.get("stat").cloned().unwrap_or_default();
                let diff = result.get("diff").cloned().unwrap_or_default();
                if stat.is_empty() && diff.is_empty() {
                    println!("  No changes since this checkpoint.");
                } else {
                    if !stat.is_empty() {
                        println!("\n{stat}");
                    }
                    if !diff.is_empty() {
                        let diff_lines: Vec<&str> = diff.lines().collect();
                        if diff_lines.len() > 80 {
                            println!("{}", diff_lines[..80].join("\n"));
                            println!("\n  ... ({} more lines, showing first 80)", diff_lines.len() - 80);
                        } else {
                            println!("\n{diff}");
                        }
                    }
                }
            } else {
                let err = result.get("error").cloned().unwrap_or_else(|| "unknown error".to_string());
                println!("  ❌ {err}");
            }
            return;
        }

        // Resolve checkpoint reference (number or hash)
        let checkpoints = mgr.list_checkpoints(&cwd);
        if checkpoints.is_empty() {
            println!("  No checkpoints found for {cwd}");
            return;
        }
        let target_hash = match self.resolve_checkpoint_ref(&args[0], &checkpoints) {
            Some(h) => h,
            None => return,
        };
        // Check for file-level restore: /rollback <N> <file>
        let file_path: Option<String> = if args.len() > 1 { Some(args[1].clone()) } else { None };
        let safe = !restore_all && file_path.is_none();
        let result = mgr.restore(&cwd, &target_hash, file_path.as_deref(), safe);
        let success = result.get("success").map(|v| v == "true").unwrap_or(false);
        if success {
            let restored_to = result.get("restored_to").cloned().unwrap_or_default();
            let reason = result.get("reason").cloned().unwrap_or_default();
            if let Some(fp) = file_path {
                println!("  ✅ Restored {fp} from checkpoint {restored_to}: {reason}");
            } else {
                println!("  ✅ Restored to checkpoint {restored_to}: {reason}");
            }
            // skipped_user_edits
            if let Some(skipped_str) = result.get("skipped_user_edits") {
                if !skipped_str.is_empty() {
                    let skipped: Vec<&str> = skipped_str.split(',').filter(|s| !s.is_empty()).collect();
                    if !skipped.is_empty() {
                        let shown = skipped.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
                        let more = if skipped.len() > 5 { format!(" (+{} more)", skipped.len() - 5) } else { String::new() };
                        println!("  ↷ Kept your hand-edits: {shown}{more}");
                        println!("  Use /rollback <N> --all to restore those too.");
                    }
                }
            }
            println!("  A pre-rollback snapshot was saved automatically.");
            if !self.conversation_history.is_empty() {
                self.undo_last(false);
                println!("  Chat turn undone to match restored file state.");
            }
        } else {
            let err = result.get("error").cloned().unwrap_or_else(|| "unknown error".to_string());
            println!("  ❌ {err}");
        }
    }

    // -----------------------------------------------------------------------
    // _handle_diff_command — mirrors lines 178-255
    // -----------------------------------------------------------------------

    /// Handle /diff — show git changes in the working directory.
    ///
    /// Syntax:
    ///     /diff                  — unstaged changes + untracked files
    ///     /diff staged           — staged changes (git diff --cached)
    ///     /diff all              — staged + unstaged + untracked (vs HEAD)
    ///     /diff session          — everything Hermes changed (checkpoint baseline)
    ///     /diff [mode] --stat    — summary only (changed files + counts)
    ///     /diff [mode] <path...> — restrict to specific paths
    /// Mirrors `CLICommandsMixin._handle_diff_command` (178-255).
    pub fn handle_diff_command(&self, command: &str) {
        // shlex.split handling — preserves quoted paths
        let parts = shlex_split_stub(command);
        let args = if parts.len() > 1 { parts[1..].to_vec() } else { Vec::new() };
        let mut stat_only = false;
        let mut mode = "working".to_string();
        let mut paths: Vec<String> = Vec::new();
        for arg in &args {
            let low = arg.to_lowercase();
            if low == "--stat" || low == "stat" {
                stat_only = true;
            } else if matches!(low.as_str(), "staged" | "--staged" | "cached" | "--cached") {
                mode = "staged".to_string();
            } else if matches!(low.as_str(), "all" | "--all" | "head") {
                mode = "all".to_string();
            } else if low == "session" {
                mode = "session".to_string();
            } else {
                paths.push(arg.clone());
            }
        }
        let cwd = std::env::var("TERMINAL_CWD").unwrap_or_else(|_| {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).to_string_lossy().to_string()
        });
        if mode == "session" {
            self.print_session_diff(&cwd, stat_only);
            return;
        }
        // from tools.working_diff import collect_working_diff — lazy
        let result = collect_working_diff(&cwd, &mode, if paths.is_empty() { None } else { Some(paths.clone()) });
        if !result.success {
            let err = if result.error.is_empty() { "Could not generate diff".to_string() } else { result.error.clone() };
            println!("  {err}");
            return;
        }
        if result.empty || (result.stat.is_empty() && result.diff.is_empty() && result.untracked.is_empty()) {
            println!("  No changes.");
            return;
        }
        let label = match mode.as_str() {
            "staged" => "Staged",
            "all" => "All (vs HEAD)",
            _ => "Unstaged",
        };
        if !result.stat.is_empty() {
            println!("\n  {label}:");
            self.print_diff_text(&result.stat);
        }
        if !result.untracked.is_empty() && (mode == "working" || mode == "all") {
            println!("\n  Untracked:");
            for rel in result.untracked.iter().take(20) {
                println!("    + {rel}");
            }
            if result.untracked.len() > 20 {
                println!("    ... and {} more", result.untracked.len() - 20);
            }
        }
        if stat_only || result.diff.is_empty() {
            return;
        }
        let diff_lines: Vec<&str> = result.diff.lines().collect();
        println!();
        if diff_lines.len() > 400 {
            self.print_diff_text(&diff_lines[..400].join("\n"));
            println!("\n  ... ({} more lines — run /diff --stat for a summary)", diff_lines.len() - 400);
        } else {
            self.print_diff_text(&result.diff);
        }
    }

    // -----------------------------------------------------------------------
    // _print_session_diff — mirrors lines 256-294
    // -----------------------------------------------------------------------

    /// Print the cumulative checkpoint-baseline diff (/diff session).
    /// Mirrors `_print_session_diff` (256-294).
    pub fn print_session_diff(&self, cwd: &str, stat_only: bool) {
        if self.agent.is_none() {
            println!("  No active agent session.");
            return;
        }
        let agent = self.agent.as_ref().unwrap();
        let mgr = &agent.checkpoint_mgr;
        if !mgr.enabled {
            println!("  Checkpoints are not enabled, so there's no session baseline.");
            println!("  Enable with: hermes --checkpoints");
            println!("  Or in config.yaml: checkpoints: {{ enabled: true }}");
            println!("  (Plain /diff still works — it uses git directly.)");
            return;
        }
        let result = mgr.session_diff(cwd);
        let success = result.get("success").map(|v| v == "true").unwrap_or(false);
        if !success {
            let err = result.get("error").cloned().unwrap_or_else(|| "Could not generate diff".to_string());
            println!("  {err}");
            return;
        }
        let stat = result.get("stat").cloned().unwrap_or_default();
        let diff = result.get("diff").cloned().unwrap_or_default();
        let empty = result.get("empty").map(|v| v == "true").unwrap_or(false);
        if empty || (stat.is_empty() && diff.is_empty()) {
            println!("  No changes — Hermes hasn't edited any files here yet.");
            return;
        }
        if !stat.is_empty() {
            self.print_diff_text(&format!("\n{stat}"));
        }
        if stat_only || diff.is_empty() {
            return;
        }
        let diff_lines: Vec<&str> = diff.lines().collect();
        println!();
        if diff_lines.len() > 400 {
            self.print_diff_text(&diff_lines[..400].join("\n"));
            println!("\n  ... ({} more lines — run /diff session --stat for a summary)", diff_lines.len() - 400);
        } else {
            self.print_diff_text(&diff);
        }
    }

    // -----------------------------------------------------------------------
    // _print_diff_text — mirrors lines 296-310
    // -----------------------------------------------------------------------

    /// Render diff/stat text with color when a rich console is present.
    ///
    /// Falls back to plain print when the console isn't available (e.g. unit
    /// tests instantiating the mixin standalone).
    /// Mirrors `_print_diff_text` (296-310).
    pub fn print_diff_text(&self, text: &str) {
        if let Some(console) = &self.console {
            // Mirrors `from cli import _rich_text_from_ansi; console.print(...)`
            let rendered = rich_text_from_ansi_stub(text);
            console.print(&rendered);
            return;
        }
        println!("{text}");
    }

    // -----------------------------------------------------------------------
    // _handle_snapshot_command — mirrors lines 312-398
    // -----------------------------------------------------------------------

    /// Handle /snapshot — lightweight state snapshots for Hermes config/state.
    ///
    /// Syntax:
    ///     /snapshot                  — list recent snapshots
    ///     /snapshot create [label]   — create a snapshot
    ///     /snapshot restore <id>     — restore state from snapshot
    ///     /snapshot prune [N]        — prune to N snapshots (default 20)
    /// Mirrors `CLICommandsMixin._handle_snapshot_command` (312-398).
    pub fn handle_snapshot_command(&self, command: &str) {
        // from hermes_cli.backup import ... — lazy
        let parts: Vec<&str> = command.split_whitespace().collect();
        let subcmd = if parts.len() > 1 { parts[1].to_lowercase() } else { "list".to_string() };
        match subcmd.as_str() {
            "list" | "ls" => {
                let snaps = list_quick_snapshots();
                if snaps.is_empty() {
                    println!("  No state snapshots yet.");
                    println!("  Create one: /snapshot create [label]");
                    return;
                }
                println!("  State snapshots ({}/state-snapshots/):\n", display_hermes_home());
                println!("  {:>3}  {:<35} {:>5} {:>10} {}", "#", "ID", "Files", "Size", "Label");
                println!("  {:>3}  {:<35} {:>5} {:>10} {}", "─".repeat(3), "─".repeat(35), "─".repeat(5), "─".repeat(10), "─".repeat(20));
                for (i, s) in snaps.iter().enumerate() {
                    let size = s.total_size;
                    let size_str = if size < 1024 {
                        format!("{size} B")
                    } else if size < 1024 * 1024 {
                        format!("{} KB", size / 1024)
                    } else {
                        format!("{:.1} MB", size as f64 / 1024.0 / 1024.0)
                    };
                    let label = s.label.clone().unwrap_or_default();
                    println!("  {:3}  {:<35} {:>5} {:>10} {}", i + 1, s.id, s.file_count, size_str, label);
                }
            }
            "create" => {
                let label = if parts.len() > 2 { Some(parts[2..].join(" ")) } else { None };
                let snap_id = create_quick_snapshot(label.as_deref());
                if let Some(id) = snap_id {
                    println!("  Snapshot created: {id}");
                } else {
                    println!("  No state files found to snapshot.");
                }
            }
            "restore" | "rewind" => {
                if parts.len() < 3 {
                    println!("  Usage: /snapshot restore <snapshot-id>");
                    let recent = list_quick_snapshots_limit(Some(1));
                    if let Some(first) = recent.first() {
                        println!("  Most recent: {}", first.id);
                    }
                    return;
                }
                let mut snap_id = parts[2].to_string();
                // Allow restore by number (1-indexed)
                if let Ok(idx) = snap_id.parse::<usize>() {
                    let snaps = list_quick_snapshots();
                    if idx >= 1 && idx <= snaps.len() {
                        snap_id = snaps[idx - 1].id.clone();
                    } else {
                        println!("  Invalid snapshot number. Use 1-{}.", snaps.len());
                        return;
                    }
                }
                if restore_quick_snapshot(&snap_id) {
                    println!("  Restored state from: {snap_id}");
                    println!("  Restart recommended for state.db changes to take effect.");
                } else {
                    println!("  Snapshot not found: {snap_id}");
                }
            }
            "prune" => {
                let mut keep: usize = 20;
                if parts.len() > 2 {
                    match parts[2].parse::<usize>() {
                        Ok(n) => keep = n,
                        Err(_) => {
                            println!("  Usage: /snapshot prune [keep-count]");
                            return;
                        }
                    }
                }
                let deleted = prune_quick_snapshots(keep);
                println!("  Pruned {deleted} old snapshot(s) (keeping {keep}).");
            }
            _ => {
                println!("  Unknown subcommand: {subcmd}");
                println!("  Usage: /snapshot [list|create [label]|restore <id>|prune [N]]");
            }
        }
    }

    // -----------------------------------------------------------------------
    // _handle_export_command — mirrors lines 399-429
    // -----------------------------------------------------------------------

    /// Handle /export — export a profile to a shareable .tar.gz archive.
    ///
    /// Syntax:
    ///     /export                       — export the active profile
    ///     /export <profile>             — export a named profile
    ///     /export [profile] -o <path>   — choose the output path
    /// Mirrors `CLICommandsMixin._handle_export_command` (399-429).
    pub fn handle_export_command(&self, command: &str) {
        // from hermes_cli.profiles import export_profile, get_active_profile_name — lazy
        let mut parts: Vec<String> = command.split_whitespace().skip(1).map(|s| s.to_string()).collect();
        let mut output: Option<String> = None;
        if let Some(idx) = parts.iter().position(|p| p == "-o") {
            if idx + 1 >= parts.len() {
                println!("  Usage: /export [profile] [-o output.tar.gz]");
                return;
            }
            output = Some(parts[idx + 1].clone());
            parts = [&parts[..idx], &parts[idx + 2..]].concat();
        }
        let name = if !parts.is_empty() {
            parts[0].clone()
        } else {
            get_active_profile_name_stub().unwrap_or_else(|| "default".to_string())
        };
        let out_path = output.unwrap_or_else(|| format!("{name}.tar.gz"));
        match export_profile_stub(&name, &out_path) {
            Ok(result) => {
                println!("  ✓ Exported '{name}' to {result}");
                println!("  Share it: the other user runs /import or `hermes profile import <archive>`.");
            }
            Err(e) => {
                println!("  Error: {e}");
            }
        }
    }

    // -----------------------------------------------------------------------
    // _handle_import_command — mirrors lines 430-472
    // -----------------------------------------------------------------------

    /// Handle /import — import a shared profile archive as a new profile.
    ///
    /// Syntax:
    ///     /import <archive.tar.gz> [--name <name>]
    /// Mirrors `CLICommandsMixin._handle_import_command` (430-472).
    pub fn handle_import_command(&self, command: &str) {
        // from hermes_cli.profiles import check_alias_collision, create_wrapper_script, import_profile — lazy
        let mut parts: Vec<String> = command.split_whitespace().skip(1).map(|s| s.to_string()).collect();
        let mut name: Option<String> = None;
        if let Some(idx) = parts.iter().position(|p| p == "--name") {
            if idx + 1 >= parts.len() {
                println!("  Usage: /import <archive.tar.gz> [--name <name>]");
                return;
            }
            name = Some(parts[idx + 1].clone());
            parts = [&parts[..idx], &parts[idx + 2..]].concat();
        }
        if parts.is_empty() {
            println!("  Usage: /import <archive.tar.gz> [--name <name>]");
            return;
        }
        let archive = parts.join(" "); // paths may contain spaces
        let profile_dir = match import_profile_stub(&archive, name.as_deref()) {
            Ok(p) => p,
            Err(e) => {
                println!("  Error: {e}");
                return;
            }
        };
        let imported = profile_dir.file_name().and_then(|n| n.to_str()).unwrap_or("imported").to_string();
        println!("  ✓ Imported profile '{imported}' at {}", profile_dir.display());
        // Wrapper creation — best-effort
        if !check_alias_collision_stub(&imported) {
            if let Some(wrapper_path) = create_wrapper_script_stub(&imported) {
                println!("  Wrapper created: {}", wrapper_path.display());
            }
        }
        println!("  Use it: hermes -p {imported}");
    }

    // -----------------------------------------------------------------------
    // _handle_stop_command — mirrors lines 473-505
    // -----------------------------------------------------------------------

    /// Handle /stop — kill all running background processes and
    /// background (async) delegations.
    ///
    /// Inspired by OpenAI Codex's separation of interrupt (stop current turn)
    /// from /stop (clean up background processes). See openai/codex#14602.
    /// Mirrors `CLICommandsMixin._handle_stop_command` (473-505).
    pub fn handle_stop_command(&self) {
        // from tools.process_registry import process_registry — lazy
        let processes = list_process_sessions_stub();
        let running: Vec<&ProcessSession> = processes.iter().filter(|p| p.status == "running").collect();
        // Background subagents dispatched via delegate_task(background=true)
        // live in their own registry, not the process registry.
        let n_async = async_active_count_stub();
        if running.is_empty() && n_async == 0 {
            println!("  No running background processes.");
            return;
        }
        if !running.is_empty() {
            println!("  Stopping {} background process(es)...", running.len());
            let killed = kill_all_stub();
            println!("  ✅ Stopped {killed} process(es).");
        }
        if n_async > 0 {
            let stopped = async_interrupt_all_stub("/stop");
            println!("  ✅ Interrupted {stopped} background delegation(s).");
        }
    }

    // -----------------------------------------------------------------------
    // _handle_agents_command — mirrors lines 506-571
    // -----------------------------------------------------------------------

    /// Handle /agents — show background processes and agent status.
    /// Mirrors `CLICommandsMixin._handle_agents_command` (506-571).
    pub fn handle_agents_command(&self) {
        // from cli import _cprint; from tools.process_registry import format_uptime_short, process_registry — lazy
        let processes = list_process_sessions_stub();
        let running: Vec<&ProcessSession> = processes.iter().filter(|p| p.status == "running").collect();
        let finished: Vec<&ProcessSession> = processes.iter().filter(|p| p.status != "running").collect();
        cprint(&format!("  Running processes: {}", running.len()));
        for p in &running {
            let cmd = p.command.chars().take(80).collect::<String>();
            let up = format_uptime_short_stub(p.uptime_seconds);
            cprint(&format!("    {} · {up} · {cmd}", p.session_id));
        }
        if !finished.is_empty() {
            cprint(&format!("  Recently finished: {}", finished.len()));
        }
        // Background (async) delegations — delegate_task(background=true)
        let delegations = list_async_delegations_stub();
        let running_d: Vec<&DelegationInfo> = delegations.iter().filter(|d| matches!(d.status.as_str(), "running" | "stalling")).collect();
        if !delegations.is_empty() {
            cprint(&format!("  Background delegations: {} running", running_d.len()));
            for d in &delegations {
                let goal = d.goal.chars().take(60).collect::<String>();
                let status = &d.status;
                let mut line = format!("    {} · {status} · {goal}", d.delegation_id);
                // Live-status detail for in-flight delegations (#51690).
                if status == "stalling" {
                    if let Some(quiet) = d.stalled_after_quiet_seconds {
                        line.push_str(&format!(" · no progress {quiet:.0}s — interrupting"));
                    }
                } else if status == "running" {
                    if let Some(quiet) = d.seconds_since_progress {
                        if quiet >= 60.0 {
                            line.push_str(&format!(" · quiet {quiet:.0}s"));
                        }
                    }
                }
                cprint(&line);
                for (i, child) in d.children_activity.iter().enumerate() {
                    let doing = if let Some(tool) = &child.current_tool {
                        format!("in {tool}")
                    } else {
                        "between turns".to_string()
                    };
                    let mut part = format!("      └ child {}: {} api calls · {doing}", i + 1, child.api_calls);
                    if let Some(idle) = child.seconds_since_activity {
                        part.push_str(&format!(" · last activity {idle:.0}s ago"));
                    }
                    cprint(&part);
                }
            }
        }
        let agent_running = self.agent_running;
        cprint(&format!("  Agent: {}", if agent_running { "running" } else { "idle" }));
    }

    // -----------------------------------------------------------------------
    // _handle_journey_command — mirrors lines 572-608
    // -----------------------------------------------------------------------

    /// Handle /journey — the learning timeline (see `hermes journey`).
    ///
    /// The read-only views (default + `list`) render Rich color, which
    /// patch_stdout would swallow as raw escapes; capture with forced ANSI and
    /// re-emit through `_cprint`. `delete`/`edit` are interactive
    /// (confirm prompt / `$EDITOR`) so they keep the real stdio.
    /// Mirrors `CLICommandsMixin._handle_journey_command` (572-608).
    pub fn handle_journey_command(&self, cmd_original: &str) {
        // import argparse, io, shlex, contextlib.redirect_stdout, cli._cprint, hermes_cli.journey.register_cli — lazy
        let rest = if let Some(idx) = cmd_original.find(' ') {
            cmd_original[idx + 1..].trim().to_string()
        } else {
            String::new()
        };
        let mut parser = JourneyParserStub::default();
        journey_register_cli_stub(&mut parser);
        let shlex_parts = shlex_split_stub(&rest);
        // Simulate parse_args — exit on error (SystemExit → return)
        let args = match parser.parse_args_stub(shlex_parts) {
            Ok(a) => a,
            Err(_) => return,
        };
        let interactive = matches!(args.journey_action.as_deref(), Some("delete") | Some("edit"));
        if interactive {
            let _ = args.call_func();
            return;
        }
        // Non-interactive: force_color=True, capture stdout, re-emit via _cprint
        let mut buf = String::new();
        // Simulate redirect_stdout capture
        let capture_result: Result<String, String> = (|| {
            // args.func(args) would write to buf
            let _ = args.call_func();
            Ok("journey output (slice 1 stub)".to_string())
        })();
        match capture_result {
            Ok(output) => {
                buf = output;
                cprint(buf.trim_end_matches('\n'));
            }
            Err(exc) => {
                cprint(&format!("  /journey failed: {exc}"));
            }
        }
        let _ = rest;
    }

    // -----------------------------------------------------------------------
    // _handle_paste_command — mirrors lines 609-633
    // -----------------------------------------------------------------------

    /// Handle /paste — explicitly check clipboard for an image.
    ///
    /// This is the reliable fallback for terminals where BracketedPaste
    /// doesn't fire for image-only clipboard content (e.g., VSCode terminal,
    /// Windows Terminal with WSL2).
    /// Mirrors `CLICommandsMixin._handle_paste_command` (609-633).
    pub fn handle_paste_command(&mut self) {
        // from cli import _DIM, _RST, _cprint, _termux_example_image_path — lazy
        if is_termux_environment() {
            cprint(&format!("  {DIM}Clipboard image paste is not available on Termux — use /image <path> or paste a local image path like {}{RST}", termux_example_image_path_stub(None)));
            return;
        }
        // from hermes_cli.clipboard import has_clipboard_image — lazy
        if has_clipboard_image_stub() {
            if self.try_attach_clipboard_image() {
                let n = self.attached_images.len();
                cprint(&format!("  📎 Image #{n} attached from clipboard"));
            } else {
                cprint(&format!("  {DIM}(>_<) Clipboard has an image but extraction failed{RST}"));
            }
        } else {
            cprint(&format!("  {DIM}(._.) No image found in clipboard{RST}"));
        }
    }

    // -----------------------------------------------------------------------
    // _handle_copy_command — mirrors lines 635-694
    // -----------------------------------------------------------------------

    /// Handle /copy [number] — copy assistant output to clipboard.
    /// Mirrors `CLICommandsMixin._handle_copy_command` (635-694).
    pub fn handle_copy_command(&self, cmd_original: &str) {
        // from cli import _assistant_copy_text, _cprint — lazy
        let parts: Vec<&str> = cmd_original.splitn(2, ' ').collect();
        let arg = if parts.len() > 1 { parts[1].trim().to_string() } else { String::new() };
        let assistant: Vec<&HashMap<String, String>> = self.conversation_history.iter().filter(|m| m.get("role").map(|r| r == "assistant").unwrap_or(false)).collect();
        if assistant.is_empty() {
            cprint("  Nothing to copy yet.");
            return;
        }
        let idx: usize;
        if !arg.is_empty() {
            match arg.parse::<usize>() {
                Ok(n) => {
                    if n < 1 || n > assistant.len() {
                        cprint(&format!("  Invalid response number. Use 1-{}.", assistant.len()));
                        return;
                    }
                    idx = n - 1;
                }
                Err(_) => {
                    cprint("  Usage: /copy [number]");
                    return;
                }
            }
        } else {
            // Find last assistant message with copyable text
            let mut found: Option<usize> = None;
            for (i, msg) in assistant.iter().enumerate().rev() {
                let text = assistant_copy_text_stub(msg.get("content").map(|s| s.as_str()));
                if !text.is_empty() {
                    found = Some(i);
                    break;
                }
            }
            match found {
                Some(i) => idx = i,
                None => {
                    cprint("  Nothing to copy in assistant responses yet.");
                    return;
                }
            }
        }
        let text = assistant_copy_text_stub(assistant[idx].get("content").map(|s| s.as_str()));
        if text.is_empty() {
            cprint("  Nothing to copy in that assistant response.");
            return;
        }
        // from hermes_cli.clipboard import is_remote_shell_session, write_clipboard_text — lazy
        if is_remote_shell_session_stub() {
            // Over SSH, native tools would write the REMOTE clipboard — OSC 52 reaches the terminal
            self.write_osc52_clipboard(&text);
            cprint(&format!("  Copied assistant response #{} via OSC 52 (terminal support required)", idx + 1));
            return;
        }
        if write_clipboard_text_stub(&text) {
            cprint(&format!("  Copied assistant response #{} to clipboard", idx + 1));
            return;
        }
        // Native tools unavailable/failed — fall back to OSC 52
        self.write_osc52_clipboard(&text);
        cprint(&format!("  Copied assistant response #{} via OSC 52 (terminal support required)", idx + 1));
    }

    // -----------------------------------------------------------------------
    // _handle_image_command — mirrors lines 696-720
    // -----------------------------------------------------------------------

    /// Handle /image <path> — attach a local image file for the next prompt.
    /// Mirrors `CLICommandsMixin._handle_image_command` (696-720).
    pub fn handle_image_command(&mut self, cmd_original: &str) {
        // from cli import _DIM, _IMAGE_EXTENSIONS, _RST, _cprint, _resolve_attachment_path, _split_path_input, _termux_example_image_path — lazy
        let raw_args = if let Some(idx) = cmd_original.find(' ') {
            cmd_original[idx + 1..].trim().to_string()
        } else {
            String::new()
        };
        if raw_args.is_empty() {
            let hint = if is_termux_environment() { termux_example_image_path_stub(None) } else { "/path/to/image.png".to_string() };
            cprint(&format!("  {DIM}Usage: /image <path>  e.g. /image {hint}{RST}"));
            return;
        }
        let (path_token, remainder) = split_path_input_stub(&raw_args);
        let image_path = match resolve_attachment_path_stub(&path_token) {
            Some(p) => p,
            None => {
                cprint(&format!("  {DIM}(>_<) File not found: {path_token}{RST}"));
                return;
            }
        };
        let suffix = image_path.extension().and_then(|e| e.to_str()).map(|e| format!(".{}", e.to_lowercase())).unwrap_or_default();
        if !IMAGE_EXTENSIONS.contains(&suffix.as_str()) {
            let name = image_path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
            cprint(&format!("  {DIM}(._.) Not a supported image file: {name}{RST}"));
            return;
        }
        let name = image_path.file_name().and_then(|n| n.to_str()).unwrap_or("image").to_string();
        self.attached_images.push(image_path.clone());
        cprint(&format!("  📎 Attached image: {name}"));
        if let Some(rem) = remainder {
            if !rem.is_empty() {
                cprint(&format!("  {DIM}Now type your prompt (or use --image in single-query mode): {rem}{RST}"));
            }
        } else if is_termux_environment() {
            cprint(&format!("  {DIM}Tip: type your next message, or run hermes chat -q --image {} \"What do you see?\"{RST}", termux_example_image_path_stub(Some(&name))));
        }
    }

    // -----------------------------------------------------------------------
    // _handle_tools_command — mirrors lines 721-800
    // -----------------------------------------------------------------------

    /// Handle /tools [list|disable|enable] slash commands.
    ///
    /// /tools (no args) shows the tool list.
    /// /tools list shows enabled/disabled status per toolset.
    /// /tools disable/enable saves the change to config and resets
    /// the session so the new tool set takes effect cleanly (no
    /// prompt-cache breakage mid-conversation).
    /// Mirrors `CLICommandsMixin._handle_tools_command` (721-800).
    pub fn handle_tools_command(&mut self, cmd: &str) {
        // from cli import _ACCENT, _DIM, _RST, _cprint, shlex, argparse.Namespace,
        // contextlib.redirect_stdout, io.StringIO, hermes_cli.tools_config.tools_disable_enable_command — lazy
        // _run_capture helper — mirrors 737-763
        let run_capture = |ns: ToolsNamespaceStub, app: Option<&String>| {
            // Standalone/tests, run as usual
            if app.is_none() {
                tools_disable_enable_command_stub(&ns);
                return;
            }
            // Inside TUI: buffer with isatty()=True so color() still emits ANSI.
            // Rust stub: just call through then re-emit via _cprint.
            // Python buffers via _TTYBuf + redirect_stdout, then _cprint per line.
            // Here we simulate by capturing stub output (no real output in slice 1).
            let mut buf = String::new();
            // Simulate tools_disable_enable_command writing to buf
            {
                // Buffer isatty=True gate preserved for 1:1
                let _ = &ns;
                buf.push_str(&format!("tools {} {}\n", ns.tools_action, ns.names.join(" ")));
            }
            for line in buf.lines() {
                cprint(line);
            }
        };

        let parts = shlex_split_stub(cmd);
        let subcommand = if parts.len() > 1 { parts[1].clone() } else { String::new() };
        if !matches!(subcommand.as_str(), "list" | "disable" | "enable") {
            self.show_tools();
            return;
        }
        if subcommand == "list" {
            run_capture(ToolsNamespaceStub { tools_action: "list".to_string(), names: Vec::new(), platform: "cli".to_string() }, self.app.as_ref());
            return;
        }
        let names: Vec<String> = if parts.len() > 2 { parts[2..].to_vec() } else { Vec::new() };
        if names.is_empty() {
            println!("(._.) Usage: /tools {} <name> [name ...]", subcommand);
            println!("  Built-in toolset:  /tools {} web", subcommand);
            println!("  MCP tool:          /tools {} github:create_issue", subcommand);
            return;
        }
        let verb = if subcommand == "disable" { "Disabling" } else { "Enabling" };
        let label = names.join(", ");
        cprint(&format!("{ACCENT}{verb} {label}...{RST}"));
        run_capture(ToolsNamespaceStub { tools_action: subcommand.clone(), names: names.clone(), platform: "cli".to_string() }, self.app.as_ref());
        // Reset session so new tool config is picked up from clean state
        // from hermes_cli.tools_config import _get_platform_tools; from hermes_cli.config import load_config — lazy
        self.enabled_toolsets = get_platform_tools_stub(&load_config_stub(), "cli");
        self.new_session();
        cprint(&format!("{DIM}Session reset. New tool configuration is active.{RST}"));
    }

    // -----------------------------------------------------------------------
    // _handle_profile_command — mirrors lines 801-813
    // -----------------------------------------------------------------------

    /// Display active profile name and home directory.
    /// Mirrors `CLICommandsMixin._handle_profile_command` (801-813).
    pub fn handle_profile_command(&self) {
        // from hermes_cli.slash_exec import CommandContext, execute_command — lazy
        let reply = execute_command_stub("profile", CommandContext { surface: "cli".to_string() });
        let profile_name = reply.data.get("profile").cloned().unwrap_or_else(|| "default".to_string());
        let display = reply.data.get("home").cloned().unwrap_or_else(display_hermes_home);
        println!();
        println!("  Profile: {profile_name}");
        println!("  Home:    {display}");
        println!();
    }

    // -----------------------------------------------------------------------
    // _handle_handoff_command — mirrors lines 814-900 (head, truncated)
    // -----------------------------------------------------------------------

    /// Handle `/handoff <platform>` — transfer this CLI session to a gateway platform.
    ///
    /// Flow:
    ///   1. Validate platform name + the gateway has a home channel for it.
    ///   2. Reject if the agent is currently running (the in-flight turn
    ///      would race with the gateway's switch_session).
    ///   3. Write `handoff_state='pending'` on this session row.
    ///   4. Block-poll `state.db` for terminal state (timeout 60s).
    ///   5. On `completed` → print resume hint and signal CLI exit by
    ///      returning False (the caller honors that like `/quit`).
    ///   6. On `failed` / timeout → print error and return True so the
    ///      user keeps their CLI session.
    ///
    /// Returns: False to signal CLI exit, True to keep going.
    /// Mirrors `CLICommandsMixin._handle_handoff_command` (814-900 head; tail in slice 2).
    pub fn handle_handoff_command(&mut self, cmd_original: &str) -> bool {
        // from cli import _cprint; from hermes_state import format_session_db_unavailable — lazy
        let parts: Vec<&str> = cmd_original.splitn(2, ' ').collect();
        if parts.len() < 2 || parts[1].trim().is_empty() {
            cprint("  Usage: /handoff <platform>");
            cprint("  Hands the current session off to that platform's home channel.");
            cprint("  The CLI session ends here; resume it later with /resume.");
            return true;
        }
        let platform_name = parts[1].trim().to_lowercase();
        // Validate platform name + home channel via live gateway config.
        // try: from gateway.config import load_gateway_config, Platform — lazy
        // In Rust we stub Platform validation via gateway stub.
        // Platform lookup — mirrors `Platform(platform_name)` (850-854)
        let platform = match validate_platform_stub(&platform_name) {
            Some(p) => p,
            None => {
                cprint(&format!("  Unknown platform '{platform_name}'."));
                return true;
            }
        };
        let gw_config = match load_gateway_config_stub() {
            Ok(c) => c,
            Err(exc) => {
                cprint(&format!("  Could not load gateway config: {exc}"));
                return true;
            }
        };
        let pcfg = gw_config.platforms.get(platform.as_str()).cloned();
        if pcfg.as_ref().map(|c| !c.enabled).unwrap_or(true) {
            // Relay aliasing: a relay-fronted gateway has no per-platform
            // config block for the logical platform ("discord" etc.) — only a
            // RELAY entry — yet /handoff discord is deliverable when the relay
            // fronts it. The fronted set is deploy config
            // (GATEWAY_RELAY_PLATFORMS), readable here without the live
            // adapter; the gateway watcher re-checks against the authenticated
            // transport (resolve_delivery_transport) before dispatch, so this
            // is a UX pre-check, not the security gate.
            // Mirrors lines 864-883.
            let mut relay_fronts = false;
            // try: from gateway.relay import relay_platform_identities — lazy
            let relay_cfg = gw_config.platforms.get(RELAY_PLATFORM).cloned();
            if let Some(rc) = relay_cfg {
                if rc.enabled {
                    let fronted: Vec<String> = relay_platform_identities_stub().into_iter().map(|(p, _)| p).collect();
                    relay_fronts = fronted.contains(&platform_name);
                }
            }
            if !relay_fronts {
                cprint(&format!("  Platform '{platform_name}' is not configured/enabled in the gateway."));
                return true;
            }
        }
        let home = gw_config.get_home_channel(&platform);
        if home.as_ref().map(|h| h.chat_id.is_empty()).unwrap_or(true) {
            cprint(&format!("  No home channel configured for {platform_name}."));
            cprint("  Set one with /sethome on the destination chat first.");
            return true;
        }

        // Refuse mid-turn: an in-flight agent run would race with the
        // gateway's switch_session and the synthetic turn dispatch.
        // Mirrors lines 891-895: `if getattr(self, "_agent_running", False):` 
        if self.agent_running {
            cprint("  Agent is busy. Wait for the current turn to finish, then retry /handoff.");
            return true;
        }

        // Make sure we have a SessionDB handle — mirrors lines 897-906.
        // Slice 1 boundary: we include the `self._session_db` ensure through
        // the `format_session_db_unavailable` gate at line 900 and then
        // explicitly note continuation.
        if self.session_db.is_none() {
            // try: from hermes_state import SessionDB; self._session_db = SessionDB() — lazy
            // Best-effort stub: create empty SessionDbStub for 1:1 mapping.
            // Real impl would open the SQLite handle; stub preserves control flow.
            self.session_db = Some(SessionDbStub::default());
        }
        if self.session_db.is_none() {
            cprint(&format!("  {}", format_session_db_unavailable_stub()));
            return true;
        }

        // NOTE: slice boundary — line 900.
        // The remainder of `_handle_handoff_command` (lines 908-989: session row
        // ensure, placeholder title INSERT OR IGNORE, title resolve,
        // `request_handoff`, 60s poll loop with `get_handoff_state` /
        // `fail_handoff` timeout + `completed`/`failed` branches, `_handed_off_session_ids`
        // bookkeeping and `self._should_exit = True`) continues in
        // `cli_commands_slice2.rs`. This file intentionally stops at the
        // SessionDB availability gate so the 900-line slice stays clean.
        // Caller for now returns true (keep session) since handoff tail not yet driven.
        true
    }
}

// ---------------------------------------------------------------------------
// Helpers — mirrors per-handler utilities
// ---------------------------------------------------------------------------

fn shlex_split_stub(command: &str) -> Vec<String> {
    // Mirrors `shlex.split(command)` (190, 765) with ValueError fallback.
    // Preserve quoted paths: handle single/double quotes naïvely.
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for ch in command.chars() {
        if escaped {
            cur.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && !in_single {
            escaped = true;
            continue;
        }
        if ch == '\'' && !in_double {
            in_single = !in_single;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            continue;
        }
        if ch.is_whitespace() && !in_single && !in_double {
            if !cur.is_empty() {
                out.push(cur.clone());
                cur.clear();
            }
            continue;
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    // If quotes were unbalanced, emulate ValueError → fallback is caller-split.
    // Our loop already handles unbalanced as open; for 1:1 we return out anyway.
    if in_single || in_double {
        // Simulate ValueError path: caller falls back to command.split()
        return command.split_whitespace().map(|s| s.to_string()).collect();
    }
    out
}

fn validate_platform_stub(name: &str) -> Option<String> {
    // Mirrors `Platform(platform_name)` enum validation (851)
    const KNOWN: &[&str] = &["telegram","discord","slack","whatsapp","signal","matrix","mattermost","email","sms","dingtalk","wecom","weixin","feishu","qqbot","bluebubbles","yuanbao","webhook","api_server","irc","relay"];
    if KNOWN.contains(&name) { Some(name.to_string()) } else { None }
}

// ---------------------------------------------------------------------------
// Re-exported state for 1:1 import traceability — mirrors lazy `from cli import ...`
// ---------------------------------------------------------------------------

/// Mirrors `from cli import _cprint` re-emit path for handlers that capture via redirect_stdout.
pub fn cli_cprint_reexport(text: &str) { cprint(text); }

// ---------------------------------------------------------------------------
// Slice boundary — line 900
// ---------------------------------------------------------------------------
// Python `cli_commands_mixin.py` lines 908-3919 (handoff tail through
// `_handle_resume_command`, `_handle_sessions_command`, `_handle_history_command`,
// `_handle_clear_command`, `_handle_compact_command`, `_handle_model_command`,
// `_handle_provider_command`, `_handle_browser_command`, etc.) continue in
// `cli_commands_slice2.rs` (from the `get_session` ensure-row block at 912).
// This file intentionally stops at the SessionDB gate (~900) so that `cargo`
// is never invoked and the 5-slice decomposition stays clean.
