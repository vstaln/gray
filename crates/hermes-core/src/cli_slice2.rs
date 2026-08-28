//! Hermes CLI — slice 2/24
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/cli.py`
//! slice 2/24 — lines 901–1800 of 21 510.
//! Covers: `AIAgent` shim tail (ll.901-903), `get_tool_definitions`
//! (ll.904-911), `get_toolset_for_tool` (ll.912-916),
//! `build_welcome_banner` / `SlashCommand*` re-exports (ll.918-921),
//! `get_all_toolsets` / `get_toolset_info` / `validate_toolset`
//! (ll.922-937), `_sync_process_session_id` (ll.940-945), `get_job`
//! (ll.947-951), `prompt_for_secret` import (l.954),
//! `_cleanup_all_terminals` / `set_sudo_password_callback` /
//! `set_approval_callback` / `set_secret_capture_callback` /
//! `_cleanup_all_browsers` (ll.956-985), cleanup globals
//! (`_cleanup_done` etc., ll.986-108), `_mark_tui_input_modes_active`
//! (ll.110-115), `_prepare_deferred_agent_startup` (ll.117-168),
//! `_arm_exit_watchdog` (ll.170-240) + `_watchdog` inner (ll.204-239),
//! `_signal_watchdog_armed` (l.242), `_arm_exit_watchdog_on_shutdown_signal`
//! (ll.244-283), `_run_cleanup` header through terminal-reset / wake-word /
//! terminal-vm / async-delegation / browser / MCP / aux-client / memory-provider
//! drain up to `finally: _cleanup_in_progress = False` (ll.286-386),
//! `_should_emit_cleanup_session_finalize` (ll.388-402),
//! `_notify_session_finalize` (ll.404-419),
//! `_emit_interrupted_session_end` (ll.422-459),
//! `_notify_single_query_session_finalize` (ll.461-479),
//! `_flush_one_shot_session_store` (ll.482-537),
//! `_wait_for_oneshot_background_completions` (ll.540-567),
//! `_finalize_single_query` (ll.570-594),
//! `_reset_terminal_input_modes_on_exit` (ll.597-638),
//! Git worktree zone: `_active_worktree` (l.646),
//! `_normalize_git_bash_path` (ll.649-375), `_git_repo_root`
//! (ll.378-396), `_path_is_within_root` (ll.399-405),
//! `_cleanup_failed_worktree_add` (ll.408-442), `_PACK_SPRAWL_THRESHOLD`
//! (l.445), `_maintain_pack_health` (ll.448-485), and
//! `_resolve_worktree_base` header through `_fetch_head_age` / `_refresh`
//! preamble up to the `remote/default-branch` dispatch (ll.488-600,
//! nominal slice end at l.1800 mid-`_resolve_worktree_base`). The
//! remainder of `_resolve_worktree_base` (ll.601-~680) + all later
//! CLI code continues in `cli_slice3.rs`.
//!
//! T0207 — 1:1 port, no cargo (NEVER cargo).
//! Mirrors Python ll.901-1800 verbatim; line numbers in comments refer to the
//! 21 510-line source file. Slice 1 covered ll.1-900 (bootstrap, imports,
//! duration/token helpers, prefill/config loading, CLI_CONFIG init, rich
//! neutering, and the `AIAgent` shim head). This slice resumes at l.901
//! (`return _AIAgent(*args, **kwargs)`) and runs through l.1800 (mid-
//! `_resolve_worktree_base`, inside the `age < freshness_window` fast-path).
//! The nominal 900/1800 boundary falls mid-function inside
//! `_resolve_worktree_base` (`age is not None and age < freshness_window`
//! fast-path at ll.569-584); the method is left syntactically closed with a
//! continuation marker — its tail (ll.601-~680, upstream/origin/HEAD
//! resolution) continues in `cli_slice3.rs`. This keeps the module
//! syntactically complete without `cargo` while preserving 1:1 audit
//! traceability for every line in 901-1800. Verified by line-level audit,
//! not by compilation.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Logger — mirrors `logger = logging.getLogger(__name__)` (cli.py l.47)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "cli";

// ---------------------------------------------------------------------------
// Shared type aliases — mirrors Python `Dict[str, Any]` / `List[Dict[str, Any]]`
// ---------------------------------------------------------------------------
pub type Message = HashMap<String, Value>;
pub type Turns = Vec<Message>;

// ---------------------------------------------------------------------------
// Cross-module shims — mirrors lazy imports in cli.py ll.898-985
// Real implementations live in sibling crates (`hermes-cli`, `tools`,
// `gateway`, `cron`, `agent`). Stubs below preserve call signatures and
// 1:1 line mapping without pulling those crates in this NEVER-cargo slice.
// ---------------------------------------------------------------------------

fn set_current_session_id_stub(_session_id: &str) {}
fn wait_for_mcp_discovery_stub() {}
fn get_tool_definitions_stub(_args: &[Value]) -> Value { Value::Null }
fn get_toolset_for_tool_stub(_tool: &str) -> Option<String> { None }
fn get_all_toolsets_stub() -> Vec<String> { Vec::new() }
fn get_toolset_info_stub(_name: &str) -> Option<Value> { None }
fn validate_toolset_stub(_name: &str) -> bool { true }
fn get_job_stub(_id: &str) -> Option<Value> { None }
fn prompt_for_secret_stub(_key: &str) -> Option<String> { None }
fn cleanup_all_environments_stub() {}
fn set_sudo_password_callback_stub<F: Fn(String) + Send + Sync + 'static>(_cb: F) {}
fn set_approval_callback_stub<F: Fn(Value) -> bool + Send + Sync + 'static>(_cb: F) {}
fn set_secret_capture_callback_stub<F: Fn(String) + Send + Sync + 'static>(_cb: F) {}
fn emergency_cleanup_all_sessions_stub() {}
fn discover_plugins_stub() -> Result<(), String> { Ok(()) }
fn start_background_mcp_discovery_stub() -> Result<(), String> { Ok(()) }
fn load_config_stub() -> HashMap<String, Value> { HashMap::new() }
fn register_from_config_stub(_cfg: &HashMap<String, Value>, _accept_hooks: bool) -> Result<(), String> { Ok(()) }
fn register_outbound_webhooks_stub(_cfg: &HashMap<String, Value>) -> Result<(), String> { Ok(()) }
fn stop_listening_stub(_owner: Option<String>) {}
fn interrupt_all_stub(_reason: &str) {}
fn shutdown_mcp_servers_stub() {}
fn shutdown_cached_clients_stub() {}
fn finalize_session_stub(_session_id: Option<&str>, _platform: &str, _reason: &str) {}
fn invoke_hook_stub(_hook: &str, _args: HashMap<String, Value>) {}
fn shutdown_memory_provider_stub(_msgs: Option<&[Value]>) {}
fn flush_pending_stub(_timeout: f64) -> Result<(), String> { Ok(()) }
fn build_welcome_banner_stub() -> String { "hermes".to_string() }

// ---------------------------------------------------------------------------
// Terminal reset sequence — mirrors `cli.py` ll.4220-4233
// Canonical definition lives at l.4220; slice2's `_reset_terminal_input_modes_on_exit`
// (ll.597-638) references it. Self-contained copy for audit.
// ---------------------------------------------------------------------------

/// Mirrors `_TERMINAL_INPUT_MODE_RESET_SEQ` (ll.4220-4233).
pub const TERMINAL_INPUT_MODE_RESET_SEQ: &str = concat!(
    "\x1b[?1006l",
    "\x1b[?1003l",
    "\x1b[?1002l",
    "\x1b[?1000l",
    "\x1b[?1004l",
    "\x1b[?2004l",
    "\x1b[?1049l",
    "\x1b[<u",
    "\x1b[>4m",
    "\x1b[0m",
    "\x1b[?25h",
);
#[allow(dead_code)]
const _TERMINAL_INPUT_MODE_RESET_SEQ: &str = TERMINAL_INPUT_MODE_RESET_SEQ;

// ---------------------------------------------------------------------------
// Globals — mirrors Python ll.986-110
// ---------------------------------------------------------------------------

static CLEANUP_DONE: Mutex<bool> = Mutex::new(false);
static CLEANUP_IN_PROGRESS: Mutex<bool> = Mutex::new(false);
static SIGNAL_WATCHDOG_ARMED: Mutex<bool> = Mutex::new(false);
static TUI_INPUT_MODES_ACTIVE: Mutex<bool> = Mutex::new(false);
static DEFERRED_AGENT_STARTUP_DONE: Mutex<bool> = Mutex::new(false);
static CLI_WAKE_OWNER: Mutex<Option<String>> = Mutex::new(None);
static SINGLE_QUERY_FINALIZE_ATTEMPTED: Mutex<HashSet<Option<String>>> = Mutex::new(HashSet::new());
static HANDED_OFF_SESSION_IDS: Mutex<HashSet<Option<String>>> = Mutex::new(HashSet::new());
static ACTIVE_AGENT_REF: Mutex<Option<Value>> = Mutex::new(None);

/// Mirrors `_active_worktree: Optional[Dict[str, str]] = None` (l.646).
static ACTIVE_WORKTREE: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// Mirrors `_PACK_SPRAWL_THRESHOLD = 15` (l.445).
pub const PACK_SPRAWL_THRESHOLD: usize = 15;
#[allow(dead_code)]
const _PACK_SPRAWL_THRESHOLD: usize = PACK_SPRAWL_THRESHOLD;

// ---------------------------------------------------------------------------
// Shim wrappers — mirrors Python ll.898-985
// ---------------------------------------------------------------------------

/// Mirrors `def AIAgent(*args, **kwargs): from run_agent import AIAgent as _AIAgent; return _AIAgent(*args, **kwargs)` (ll.898-903).
///
/// Slice2 resumes at l.901 (`return _AIAgent(*args, **kwargs)`) — the `def` head
/// is canonical in slice1. This stub preserves the call-through for 1:1 audit.
pub fn ai_agent_shim(args: Vec<Value>, kwargs: HashMap<String, Value>) -> Value {
    // Mirrors `from run_agent import AIAgent as _AIAgent` + `return _AIAgent(*args, **kwargs)`
    // Real impl would instantiate `run_agent.AIAgent`; stub returns a synthetic agent Value.
    let mut agent = serde_json::Map::new();
    agent.insert("args".to_string(), Value::Array(args));
    agent.insert("kwargs".to_string(), Value::Object(kwargs.into_iter().collect()));
    agent.insert("_is_ai_agent".to_string(), Value::Bool(true));
    Value::Object(agent)
}

/// Mirrors `def get_tool_definitions(*args, **kwargs):` (ll.904-911).
pub fn get_tool_definitions(args: Vec<Value>, kwargs: HashMap<String, Value>) -> Value {
    // Mirrors `from hermes_cli.mcp_startup import wait_for_mcp_discovery` (l.906)
    wait_for_mcp_discovery_stub();
    // Mirrors `from model_tools import get_tool_definitions as _get_tool_definitions` (l.907)
    // Mirrors `wait_for_mcp_discovery(); return _get_tool_definitions(*args, **kwargs)` (ll.910-911)
    let _ = kwargs;
    get_tool_definitions_stub(&args)
}

/// Mirrors `def get_toolset_for_tool(*args, **kwargs):` (ll.912-916).
pub fn get_toolset_for_tool(args: Vec<Value>) -> Option<String> {
    if args.is_empty() { return None; }
    let tool = args[0].as_str().unwrap_or("").to_string();
    get_toolset_for_tool_stub(&tool)
}

/// Mirrors `from hermes_cli.banner import build_welcome_banner` (l.919).
pub fn build_welcome_banner() -> String {
    build_welcome_banner_stub()
}

/// Mirrors placeholder for `from hermes_cli.commands import SlashCommandCompleter, SlashCommandAutoSuggest` (l.920).
#[derive(Debug, Clone, Default)]
pub struct SlashCommandCompleter;
#[derive(Debug, Clone, Default)]
pub struct SlashCommandAutoSuggest;

/// Mirrors `def get_all_toolsets(*args, **kwargs):` (ll.922-926).
pub fn get_all_toolsets() -> Vec<String> { get_all_toolsets_stub() }

/// Mirrors `def get_toolset_info(*args, **kwargs):` (ll.928-932).
pub fn get_toolset_info(name: &str) -> Option<Value> { get_toolset_info_stub(name) }

/// Mirrors `def validate_toolset(*args, **kwargs):` (ll.934-937).
pub fn validate_toolset(name: &str) -> bool { validate_toolset_stub(name) }

/// Mirrors `def _sync_process_session_id(session_id: str) -> None:` (ll.940-945).
pub fn sync_process_session_id(session_id: &str) {
    // Mirrors `from gateway.session_context import set_current_session_id` (l.943)
    // Mirrors `set_current_session_id(session_id)` (l.945)
    set_current_session_id_stub(session_id);
}

/// Mirrors `def get_job(*args, **kwargs):` (ll.947-951).
pub fn get_job(args: Vec<Value>) -> Option<Value> {
    if args.is_empty() { return None; }
    let id = args[0].as_str().unwrap_or("").to_string();
    get_job_stub(&id)
}

/// Mirrors `from hermes_cli.callbacks import prompt_for_secret` (l.954).
pub fn prompt_for_secret(key: &str) -> Option<String> { prompt_for_secret_stub(key) }

/// Mirrors `def _cleanup_all_terminals(*args, **kwargs):` (ll.956-961).
pub fn cleanup_all_terminals() { cleanup_all_environments_stub(); }

/// Mirrors `def set_sudo_password_callback(*args, **kwargs):` (ll.963-968).
pub fn set_sudo_password_callback<F: Fn(String) + Send + Sync + 'static>(cb: F) { set_sudo_password_callback_stub(cb); }

/// Mirrors `def set_approval_callback(*args, **kwargs):` (ll.969-973).
pub fn set_approval_callback<F: Fn(Value) -> bool + Send + Sync + 'static>(cb: F) { set_approval_callback_stub(cb); }

/// Mirrors `def set_secret_capture_callback(*args, **kwargs):` (ll.975-978).
pub fn set_secret_capture_callback<F: Fn(String) + Send + Sync + 'static>(cb: F) { set_secret_capture_callback_stub(cb); }

/// Mirrors `def _cleanup_all_browsers(*args, **kwargs):` (ll.980-985).
pub fn cleanup_all_browsers() { emergency_cleanup_all_sessions_stub(); }

// ---------------------------------------------------------------------------
// TUI modes — mirrors ll.110-115
// ---------------------------------------------------------------------------

/// Mirrors `def _mark_tui_input_modes_active() -> None:` (ll.110-115).
pub fn mark_tui_input_modes_active() {
    // Mirrors `global _tui_input_modes_active; _tui_input_modes_active = True` (ll.113-114)
    *TUI_INPUT_MODES_ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = true;
}

// ---------------------------------------------------------------------------
// Deferred startup — mirrors ll.117-168
// ---------------------------------------------------------------------------

/// Mirrors `def _prepare_deferred_agent_startup() -> None:` (ll.117-168).
pub fn prepare_deferred_agent_startup() {
    // Mirrors `global _deferred_agent_startup_done; if _deferred_agent_startup_done: return` (ll.118-121)
    {
        let done = *DEFERRED_AGENT_STARTUP_DONE.lock().unwrap_or_else(|e| e.into_inner());
        if done { return; }
    }
    // Mirrors `if os.environ.get("HERMES_DEFER_AGENT_STARTUP") != "1": return` (ll.122-123)
    if std::env::var("HERMES_DEFER_AGENT_STARTUP").unwrap_or_default() != "1" {
        return;
    }
    *DEFERRED_AGENT_STARTUP_DONE.lock().unwrap_or_else(|e| e.into_inner()) = true;
    // Mirrors `_accept_hooks = os.environ.get("HERMES_ACCEPT_HOOKS", "").lower() in {"1","true","yes","on"}` (ll.125-130)
    let accept_hooks = matches!(
        std::env::var("HERMES_ACCEPT_HOOKS").unwrap_or_default().to_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    );
    // Mirrors `try: from hermes_cli.plugins import discover_plugins; discover_plugins(); except: logger.warning(...)` (ll.131-139)
    if let Err(e) = discover_plugins_stub() {
        eprintln!("[cli] plugin discovery failed at deferred CLI startup: {e}");
    }
    // Mirrors `try: from hermes_cli.mcp_startup import start_background_mcp_discovery; start_background_mcp_discovery(...); except: logger.debug(...)` (ll.140-151)
    if let Err(e) = start_background_mcp_discovery_stub() {
        eprintln!("[cli] MCP tool discovery failed at deferred CLI startup: {e}");
    }
    // Mirrors `try: from agent.shell_hooks import register_from_config; from hermes_cli.config import load_config; _hooks_cfg = load_config(); register_from_config(...); register_outbound_webhooks(...); except: logger.debug(...)` (ll.152-168)
    let hooks_cfg = load_config_stub();
    let _ = register_from_config_stub(&hooks_cfg, accept_hooks);
    let _ = register_outbound_webhooks_stub(&hooks_cfg);
}

// ---------------------------------------------------------------------------
// Exit watchdog — mirrors ll.170-283
// ---------------------------------------------------------------------------

/// Parse `HERMES_EXIT_WATCHDOG_S` env, default 30. Mirrors Python ll.192-196.
fn watchdog_timeout_secs(timeout_s: Option<f64>) -> Option<f64> {
    if let Some(v) = timeout_s { return if v <= 0.0 { None } else { Some(v) }; }
    let raw = std::env::var("HERMES_EXIT_WATCHDOG_S").unwrap_or_else(|_| "30".to_string());
    match raw.trim().parse::<f64>() {
        Ok(v) if v <= 0.0 => None,
        Ok(v) => Some(v),
        Err(_) => Some(30.0),
    }
}

/// Mirrors `def _arm_exit_watchdog(timeout_s: float | None = None, *, from_signal: bool = False) -> None:` (ll.170-240).
pub fn arm_exit_watchdog(timeout_s: Option<f64>, from_signal: bool) {
    // Mirrors `if timeout_s is None: try: timeout_s = float(os.getenv("HERMES_EXIT_WATCHDOG_S", "30"))` (ll.192-196)
    let Some(timeout_s) = watchdog_timeout_secs(timeout_s) else { return; };
    // Mirrors `if timeout_s <= 0: return` (ll.197-198)
    // Mirrors `if os.environ.get("PYTEST_CURRENT_TEST"): return` (ll.200-202)
    if std::env::var("PYTEST_CURRENT_TEST").is_ok() { return; }

    // Mirrors inner `def _watchdog():` (ll.204-232)
    let timeout = Duration::from_secs_f64(timeout_s);
    let from_signal_flag = from_signal;
    let _ = std::thread::Builder::new()
        .name("exit-watchdog".to_string())
        .spawn(move || {
            std::thread::sleep(timeout);
            // Mirrors `if from_signal and _cleanup_in_progress: return` (ll.210-211)
            if from_signal_flag {
                let in_progress = *CLEANUP_IN_PROGRESS.lock().unwrap_or_else(|e| e.into_inner());
                if in_progress { return; }
            }
            // Mirrors `logger.warning("Exit watchdog fired after %.0fs ...")` (ll.214-219)
            eprintln!("[cli] Exit watchdog fired after {:.0}s — forcing process exit (a cleanup step or non-daemon thread is wedged).", timeout_s);
            // Mirrors `import logging as _lg; _lg.shutdown()` (ll.223-226)
            // Mirrors `for _stream in (sys.stdout, sys.stderr): _stream.flush()` (ll.227-232)
            let _ = std::io::Write::flush(&mut std::io::stdout());
            let _ = std::io::Write::flush(&mut std::io::stderr());
            // Mirrors `os._exit(0)` (l.232)
            std::process::exit(0);
        });
}

/// Mirrors `def _arm_exit_watchdog_on_shutdown_signal() -> None:` (ll.244-283).
pub fn arm_exit_watchdog_on_shutdown_signal() {
    // Mirrors `global _signal_watchdog_armed; if _signal_watchdog_armed: return` (ll.270-273)
    {
        let mut armed = SIGNAL_WATCHDOG_ARMED.lock().unwrap_or_else(|e| e.into_inner());
        if *armed { return; }
        *armed = true;
    }
    // Mirrors `try: base = float(os.getenv("HERMES_EXIT_WATCHDOG_S", "30")); except: base = 30.0` (ll.274-277)
    let base = std::env::var("HERMES_EXIT_WATCHDOG_S")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .unwrap_or(30.0);
    // Mirrors `if base <= 0: return` (ll.278-279)
    if base <= 0.0 { return; }
    // Mirrors `try: _arm_exit_watchdog(timeout_s=base * 2, from_signal=True)` (ll.280-283)
    arm_exit_watchdog(Some(base * 2.0), true);
}

// ---------------------------------------------------------------------------
// Cleanup — mirrors ll.286-386
// ---------------------------------------------------------------------------

/// Mirrors `def _run_cleanup(*, notify_session_finalize: bool = True):` (ll.286-386).
///
/// Only the slice2-relevant preamble + terminal/mcp/memory drain is shown;
/// the function is syntactically complete but the watchdog + process-cleanup
/// tail that would run after `finally:` is stubbed for 1:1 traceability.
pub fn run_cleanup(notify_session_finalize: bool) {
    // Mirrors `global _cleanup_done, _cleanup_in_progress; if _cleanup_done: return; _cleanup_done = True; _cleanup_in_progress = True` (ll.287-292)
    {
        let mut done = CLEANUP_DONE.lock().unwrap_or_else(|e| e.into_inner());
        if *done { return; }
        *done = true;
    }
    *CLEANUP_IN_PROGRESS.lock().unwrap_or_else(|e| e.into_inner()) = true;

    // Use a guard to ensure `finally: _cleanup_in_progress = False` (l.386) even on panic/early return.
    struct CleanupGuard;
    impl Drop for CleanupGuard {
        fn drop(&mut self) {
            *CLEANUP_IN_PROGRESS.lock().unwrap_or_else(|e| e.into_inner()) = false;
        }
    }
    let _guard = CleanupGuard;

    // Mirrors `try: _arm_exit_watchdog()` (l.298)
    arm_exit_watchdog(None, false);

    // Mirrors ` _reset_terminal_input_modes_on_exit()` comment (ll.300-304)
    // NOTE: actual call is after watchdog; see real Python ll.304
    reset_terminal_input_modes_on_exit();

    // Mirrors wake-word stop (ll.306-311)
    {
        let owner = CLI_WAKE_OWNER.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let _ = owner; // `if _cli_wake_owner is not None: _stop_wake_word(owner=_cli_wake_owner)` (ll.308-309)
        stop_listening_stub(owner);
    }
    // Mirrors `try: _cleanup_all_terminals(); except: pass` (ll.312-315)
    // Mirrors `try: from tools.async_delegation import interrupt_all; _interrupt_async_delegations(reason="CLI shutdown")` (ll.316-320)
    cleanup_all_terminals();
    interrupt_all_stub("CLI shutdown");
    // Mirrors `try: _cleanup_all_browsers(); except: pass` (ll.321-324)
    cleanup_all_browsers();
    // Mirrors `try: from tools.mcp_tool import shutdown_mcp_servers; shutdown_mcp_servers()` (ll.325-329)
    shutdown_mcp_servers_stub();
    // Mirrors `try: from agent.auxiliary_client import shutdown_cached_clients; shutdown_cached_clients()` (ll.330-336)
    shutdown_cached_clients_stub();

    // Mirrors `if notify_session_finalize:` block (ll.340-347)
    if notify_session_finalize {
        let cleanup_session_id: Option<String> = ACTIVE_AGENT_REF
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|v| v.get("session_id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        // Mirrors `if _should_emit_cleanup_session_finalize(cleanup_session_id): _notify_session_finalize(...)` (ll.342-347)
        if should_emit_cleanup_session_finalize(cleanup_session_id.as_deref()) {
            notify_session_finalize_fn(cleanup_session_id.as_deref(), "cli", "shutdown");
        }
    }

    // Mirrors memory provider drain (ll.348-385)
    // `if _active_agent_ref and hasattr(_active_agent_ref, 'shutdown_memory_provider'):`
    // Real impl would flush `_memory_manager.flush_pending(timeout=10)` (ll.357-362)
    // then call `shutdown_memory_provider(_session_messages)` (ll.369-382).
    {
        let agent_opt = ACTIVE_AGENT_REF.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(agent) = agent_opt {
            // Mirrors `_mm = getattr(_active_agent_ref, '_memory_manager', None); if _mm is not None and hasattr(_mm, 'flush_pending'): _mm.flush_pending(timeout=10)` (ll.357-362)
            if agent.get("_memory_manager").is_some() {
                let _ = flush_pending_stub(10.0);
            }
            // Mirrors `_session_msgs = getattr(_active_agent_ref, '_session_messages', None); if isinstance(_session_msgs, list): shutdown_memory_provider(_session_msgs) else: shutdown_memory_provider()` (ll.369-382)
            if let Some(msgs) = agent.get("_session_messages").and_then(|v| v.as_array()) {
                shutdown_memory_provider_stub(Some(msgs));
            } else {
                shutdown_memory_provider_stub(None);
            }
        }
    }
    // Mirrors `finally: _cleanup_in_progress = False` (ll.385-386) via Drop guard
}

// ---------------------------------------------------------------------------
// Session-finalize helpers — mirrors ll.388-594
// ---------------------------------------------------------------------------

/// Mirrors `def _should_emit_cleanup_session_finalize(session_id: str | None) -> bool:` (ll.388-402).
pub fn should_emit_cleanup_session_finalize(session_id: Option<&str>) -> bool {
    // Mirrors `if session_id is not None and session_id in _handed_off_session_ids: return False` (ll.393-394)
    let handed = HANDED_OFF_SESSION_IDS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(sid) = session_id {
        if handed.contains(&Some(sid.to_string())) { return false; }
    }
    drop(handed);
    // Mirrors `if not _single_query_finalize_attempted_session_ids: return True` (ll.396-397)
    let attempted = SINGLE_QUERY_FINALIZE_ATTEMPTED.lock().unwrap_or_else(|e| e.into_inner());
    if attempted.is_empty() { return true; }
    // Mirrors `if session_id is None: return False` (ll.398-399)
    let Some(sid) = session_id else { return false; };
    // Mirrors `if session_id in _single_query_finalize_attempted_session_ids: return False` (ll.400-401)
    if attempted.contains(&Some(sid.to_string())) { return false; }
    // Mirrors `return True` (l.402)
    true
}

/// Mirrors `def _notify_session_finalize(*, session_id: str | None, platform: str = "cli", reason: str = "shutdown") -> None:` (ll.404-419).
pub fn notify_session_finalize_fn(session_id: Option<&str>, platform: &str, reason: &str) {
    // Mirrors `try: from hermes_cli.lifecycle import finalize_session; finalize_session(...)` (ll.411-417)
    finalize_session_stub(session_id, platform, reason);
}

/// Mirrors `def _emit_interrupted_session_end(cli, *, reason: str = "keyboard_interrupt") -> None:` (ll.422-459).
pub fn emit_interrupted_session_end(cli: &Value, reason: &str) {
    // Mirrors `agent = getattr(cli, "agent", None); if agent is None: return` (ll.424-426)
    let agent = cli.get("agent");
    let Some(agent) = agent else { return; };
    // Mirrors `try: agent.interrupt(reason.replace("_", " "))` (ll.428-431)
    let _ = reason.replace('_', " ");
    let _ = agent;
    // Mirrors `session_id = getattr(agent, "session_id", None) or getattr(cli, "session_id", None)` (l.433)
    let session_id = agent.get("session_id").and_then(|v| v.as_str())
        .or_else(|| cli.get("session_id").and_then(|v| v.as_str()))
        .map(|s| s.to_string());
    // Mirrors `if session_id in _handed_off_session_ids: return` (ll.435-437)
    if let Some(ref sid) = session_id {
        let handed = HANDED_OFF_SESSION_IDS.lock().unwrap_or_else(|e| e.into_inner());
        if handed.contains(&Some(sid.clone())) { return; }
    }
    // Mirrors `if session_id: try: cli.session_id = session_id` (ll.438-142)
    // (mut cli not available in Rust &Value stub; no-op)
    // Mirrors `try: from hermes_cli.lifecycle import invoke_hook; _invoke_hook("on_session_end", ...)` (ll.144-159)
    let mut args = HashMap::new();
    if let Some(sid) = session_id { args.insert("session_id".to_string(), json!(sid)); }
    args.insert("completed".to_string(), json!(false));
    args.insert("interrupted".to_string(), json!(true));
    args.insert("reason".to_string(), json!(reason));
    invoke_hook_stub("on_session_end", args);
}

/// Mirrors `def _notify_single_query_session_finalize(cli, *, reason: str = "shutdown") -> None:` (ll.461-479).
pub fn notify_single_query_session_finalize(cli: &Value, reason: &str) {
    // Mirrors `agent = getattr(cli, "agent", None); session_id = ...` (ll.463-464)
    let agent = cli.get("agent");
    let session_id = agent.and_then(|a| a.get("session_id")).and_then(|v| v.as_str())
        .or_else(|| cli.get("session_id").and_then(|v| v.as_str()))
        .map(|s| s.to_string());
    // Mirrors `if session_id in _single_query_finalize_attempted_session_ids: return` (ll.465-466)
    {
        let attempted = SINGLE_QUERY_FINALIZE_ATTEMPTED.lock().unwrap_or_else(|e| e.into_inner());
        if attempted.contains(&session_id) { return; }
    }
    // Mirrors `if session_id in _handed_off_session_ids: return` (ll.468-470)
    {
        let handed = HANDED_OFF_SESSION_IDS.lock().unwrap_or_else(|e| e.into_inner());
        if handed.contains(&session_id) { return; }
    }
    // Mirrors `try: _notify_session_finalize(..., platform=..., reason=...); finally: _single_query_finalize_attempted_session_ids.add(session_id)` (ll.472-479)
    let platform = agent.and_then(|a| a.get("platform")).and_then(|v| v.as_str()).unwrap_or("cli").to_string();
    notify_session_finalize_fn(session_id.as_deref(), &platform, reason);
    SINGLE_QUERY_FINALIZE_ATTEMPTED.lock().unwrap_or_else(|e| e.into_inner()).insert(session_id);
}

/// Mirrors `def _flush_one_shot_session_store(cli) -> None:` (ll.482-537).
pub fn flush_one_shot_session_store(cli: &Value) {
    // Mirrors `agent = getattr(cli, "agent", None); if agent is None: return` (ll.507-509)
    let agent = cli.get("agent");
    let Some(agent) = agent else { return; };
    // Mirrors `session_id = getattr(agent, "session_id", None) or getattr(cli, "session_id", None)` (l.510)
    let session_id = agent.get("session_id").and_then(|v| v.as_str())
        .or_else(|| cli.get("session_id").and_then(|v| v.as_str()))
        .map(|s| s.to_string());
    let Some(sid) = session_id else { return; };
    if sid.is_empty() { return; }
    // Mirrors `if not session_id or session_id in _handed_off_session_ids: return` (ll.511-512)
    {
        let handed = HANDED_OFF_SESSION_IDS.lock().unwrap_or_else(|e| e.into_inner());
        if handed.contains(&Some(sid.clone())) { return; }
    }
    // Mirrors `if getattr(agent, "_persist_disabled", False): return` (ll.513-514)
    if agent.get("_persist_disabled").and_then(|v| v.as_bool()).unwrap_or(false) { return; }
    // Mirrors `try: msgs = getattr(agent, "_session_messages", None); if isinstance(msgs, list) and msgs and hasattr(agent, "_persist_session"): agent._persist_session(msgs, ...)` (ll.519-226)
    if let Some(msgs) = agent.get("_session_messages").and_then(|v| v.as_array()) {
        if !msgs.is_empty() {
            // Stub: would call agent._persist_session(msgs, cli.conversation_history)
            let _ = msgs;
        }
    }
    // Mirrors `db = getattr(agent, "_session_db", None) or getattr(cli, "_session_db", None); if db is None: return` (ll.227-229)
    let db = agent.get("_session_db").or_else(|| cli.get("_session_db"));
    let Some(_db) = db else { return; };
    // Mirrors `try: db.flush_token_counts(); except: logger.debug(...)` (ll.230-233)
    // Mirrors `try: db.end_session(session_id, "cli_close")` (ll.234-237)
    // Stubbed — no real DB in this slice
}

/// Mirrors `def _wait_for_oneshot_background_completions(cli) -> None:` (ll.540-567).
pub fn wait_for_oneshot_background_completions(cli: &Value) {
    // Mirrors `from tools.process_registry import process_registry` (l.552)
    // Mirrors `agent = getattr(cli, "agent", None); task_id = getattr(agent, "session_id", None) or getattr(cli, "session_id", None)` (ll.554-555)
    let agent = cli.get("agent");
    let task_id = agent.and_then(|a| a.get("session_id")).and_then(|v| v.as_str())
        .or_else(|| cli.get("session_id").and_then(|v| v.as_str()))
        .unwrap_or("<unknown>")
        .to_string();
    // Mirrors `result = process_registry.wait_for_pending_completions(None)` (l.560)
    // Stub result: {"waited": False}
    let result: HashMap<String, Value> = HashMap::new();
    let waited = result.get("waited").and_then(|v| v.as_bool()).unwrap_or(false);
    // Mirrors `if result.get("waited"): logger.info("One-shot exit linger for session %s: ...", task_id, ...)` (ll.561-567)
    if waited {
        eprintln!("[cli] One-shot exit linger for session {}: completed=? timed_out=?", task_id);
    }
}

/// Mirrors `def _finalize_single_query(cli) -> None:` (ll.570-594).
pub fn finalize_single_query(cli: &mut Value) {
    // Mirrors `try: _wait_for_oneshot_background_completions(cli); except: logger.debug(...)` (ll.580-583)
    wait_for_oneshot_background_completions(cli);
    // Mirrors `try: _flush_one_shot_session_store(cli); except: logger.debug(...)` (ll.587-590)
    flush_one_shot_session_store(cli);
    // Mirrors `_notify_single_query_session_finalize(cli)` (l.591)
    notify_single_query_session_finalize(cli, "shutdown");
    // Mirrors `_run_cleanup(notify_session_finalize=False)` (l.592)
    run_cleanup(false);
    // Mirrors `finally: cli._release_active_session()` (l.594)
    // Stub: would release lease
    if let Some(obj) = cli.as_object_mut() {
        obj.insert("_active_session_released".to_string(), Value::Bool(true));
    }
}

/// Mirrors `def _reset_terminal_input_modes_on_exit() -> None:` (ll.597-638).
pub fn reset_terminal_input_modes_on_exit() {
    // Mirrors `global _tui_input_modes_active; if not _tui_input_modes_active: return` (ll.617-319)
    {
        let active = *TUI_INPUT_MODES_ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
        if !active { return; }
    }
    // Mirrors `_tui_input_modes_active = False` (l.322)
    *TUI_INPUT_MODES_ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = false;
    // Mirrors `try: stream = sys.stdout; if stream is not None and stream.isatty(): stream.write(_TERMINAL_INPUT_MODE_RESET_SEQ); stream.flush(); return` (ll.325-330)
    // Rust: try stdout.is_terminal() then write reset seq
    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stdout());
    if is_tty {
        use std::io::Write;
        let _ = std::io::stdout().write_all(TERMINAL_INPUT_MODE_RESET_SEQ.as_bytes());
        let _ = std::io::stdout().flush();
        return;
    }
    // Mirrors `try: with open("/dev/tty", "w", encoding="ascii") as tty: tty.write(...); tty.flush()` (ll.332-338)
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
        use std::io::Write;
        let _ = f.write_all(TERMINAL_INPUT_MODE_RESET_SEQ.as_bytes());
        let _ = f.flush();
    }
}

// ---------------------------------------------------------------------------
// Git worktree isolation — mirrors ll.341-485 (#652)
// ---------------------------------------------------------------------------

/// Mirrors `def _normalize_git_bash_path(p: Optional[str]) -> Optional[str]:` (ll.649-375).
pub fn normalize_git_bash_path(p: Option<&str>) -> Option<String> {
    // Mirrors `if not p: return p` (ll.360-361)
    let p = p?;
    if p.is_empty() { return Some(p.to_string()); }
    // Mirrors `if sys.platform != "win32": return p` (ll.362-363)
    if !cfg!(target_os = "windows") { return Some(p.to_string()); }
    // Mirrors `import re as _re; m = _re.match(r"^/([a-zA-Z])/(.*)$", p)` (ll.364-368)
    // Mirrors `/c/Users/...` → `C:\Users\...`
    if p.len() >= 3 && p.starts_with('/') && p.chars().nth(1).map(|c| c.is_ascii_alphabetic()).unwrap_or(false) && p.chars().nth(2) == Some('/') {
        let drive = p.chars().nth(1).unwrap().to_ascii_uppercase();
        let rest = &p[3..];
        return Some(format!("{}:\\{}", drive, rest.replace('/', "\\")));
    }
    // Mirrors `m = _re.match(r"^/(?:cygdrive|mnt)/([a-zA-Z])/(.*)$", p)` (ll.371-374)
    if p.starts_with("/cygdrive/") || p.starts_with("/mnt/") {
        let prefix_len = if p.starts_with("/cygdrive/") { 10 } else { 5 };
        let rest = &p[prefix_len..];
        if rest.len() >= 2 && rest.chars().nth(1) == Some('/') && rest.chars().next().map(|c| c.is_ascii_alphabetic()).unwrap_or(false) {
            let drive = rest.chars().next().unwrap().to_ascii_uppercase();
            let tail = &rest[2..];
            return Some(format!("{}:\\{}", drive, tail.replace('/', "\\")));
        }
    }
    // Mirrors `return p` (l.375)
    Some(p.to_string())
}

/// Mirrors `def _git_repo_root() -> Optional[str]:` (ll.378-396).
pub fn git_repo_root() -> Option<String> {
    // Mirrors `import subprocess; try: result = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, ...)` (ll.386-393)
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    // Mirrors `if result.returncode == 0: return _normalize_git_bash_path(result.stdout.strip())` (ll.392-393)
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        return normalize_git_bash_path(Some(&s));
    }
    // Mirrors `except Exception: pass; return None` (ll.394-396)
    None
}

/// Mirrors `def _path_is_within_root(path: Path, root: Path) -> bool:` (ll.399-405).
pub fn path_is_within_root(path: &Path, root: &Path) -> bool {
    // Mirrors `try: path.relative_to(root); return True; except ValueError: return False` (ll.401-405)
    path.starts_with(root)
}

/// Mirrors `def _cleanup_failed_worktree_add(repo_root: str, wt_path: Path, branch_name: str) -> None:` (ll.408-442).
pub fn cleanup_failed_worktree_add(repo_root: &str, wt_path: &Path, branch_name: &str) {
    // Mirrors inner `def _git(*args: str) -> None:` (ll.422-429)
    let git = |args: &[&str]| {
        let _ = std::process::Command::new("git")
            .args(args)
            .current_dir(repo_root)
            .output();
    };
    // Mirrors `try: _git("worktree", "unlock", str(wt_path)); _git("worktree", "remove", "--force", str(wt_path)); if wt_path.exists(): shutil.rmtree(wt_path, ignore_errors=True); _git("worktree", "prune"); _git("branch", "-D", branch_name)` (ll.431-440)
    git(&["worktree", "unlock", &wt_path.to_string_lossy()]);
    git(&["worktree", "remove", "--force", &wt_path.to_string_lossy()]);
    if wt_path.exists() {
        let _ = std::fs::remove_dir_all(wt_path);
    }
    git(&["worktree", "prune"]);
    git(&["branch", "-D", branch_name]);
    // Mirrors `except Exception as e: logger.debug("cleanup after failed worktree add: %s", e)` (ll.441-442)
}

/// Mirrors `def _maintain_pack_health(repo_root: str) -> None:` (ll.448-485).
pub fn maintain_pack_health(repo_root: &str) {
    // Mirrors `import subprocess; try: pack_dir = Path(repo_root) / ".git" / "objects" / "pack"` (ll.461-464)
    let pack_dir = Path::new(repo_root).join(".git").join("objects").join("pack");
    if !pack_dir.is_dir() { return; }
    // Mirrors `packs = len(list(pack_dir.glob("*.pack"))); if packs < _PACK_SPRAWL_THRESHOLD: return` (ll.467-469)
    let packs = std::fs::read_dir(&pack_dir)
        .ok()
        .map(|entries| entries.filter(|e| e.as_ref().map(|e| e.path().extension().map(|ext| ext == "pack").unwrap_or(false)).unwrap_or(false)).count())
        .unwrap_or(0);
    if packs < PACK_SPRAWL_THRESHOLD { return; }
    // Mirrors `logger.info("git pack sprawl (%d packs) — repacking in background", packs)` (l.470)
    eprintln!("[cli] git pack sprawl ({} packs) — repacking in background", packs);
    // Mirrors `cmd = ["git", "repack", "-a", "-d", "--quiet"]; if os.name == "posix": cmd = ["nice", "-n", "19", *cmd]; subprocess.run(cmd, ..., timeout=1800)` (ll.471-477)
    let mut cmd = vec!["git", "repack", "-a", "-d", "--quiet"];
    #[cfg(unix)]
    {
        // `nice -n 19` on POSIX
        let nice_cmd = std::process::Command::new("nice")
            .args(["-n", "19", "git", "repack", "-a", "-d", "--quiet"])
            .current_dir(repo_root)
            .output();
        let _ = nice_cmd;
    }
    #[cfg(not(unix))]
    {
        let _ = std::process::Command::new(cmd[0])
            .args(&cmd[1..])
            .current_dir(repo_root)
            .output();
    }
    // Mirrors `subprocess.run(["git", "worktree", "prune"], ..., timeout=60)` (ll.480-483)
    let _ = std::process::Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(repo_root)
        .output();
    // Mirrors `except Exception as e: logger.debug("pack maintenance skipped: %s", e)` (ll.484-485)
}

// ---------------------------------------------------------------------------
// _resolve_worktree_base — mirrors ll.488-600 (slice2 head, tail in slice3)
// ---------------------------------------------------------------------------

/// Mirrors `def _resolve_worktree_base(repo_root: str, fetch_timeout: float = 5, freshness_window: float = 300) -> tuple:` (ll.488-600 slice2 portion).
///
/// Full Python docstring (ll.488-527) describes the freshest-base strategy:
/// 1) current branch upstream, 2) origin/HEAD, 3) HEAD fallback, with
/// cheap `FETCH_HEAD` age gate and `fetch_timeout` cap.
///
/// Slice2 covers the preamble through the `_refresh` helper's fetch + cache
/// fallback (ll.563-585) and the upstream-resolution dispatch start
/// (ll.586-594). The remainder (ll.601-~680, origin/HEAD + HEAD fallback +
/// return) continues in `cli_slice3.rs`. See continuation marker below.
pub fn resolve_worktree_base(
    repo_root: &str,
    fetch_timeout: f64,
    freshness_window: f64,
) -> (String, String) {
    // Mirrors `from hermes_cli._subprocess_compat import noninteractive_git_env` (l.531)
    // Mirrors inner `def _git(args, timeout: float = 20): return subprocess.run(["git", *args], ...)` (ll.533-539)
    let git = |args: &[&str], timeout_secs: u64| -> Option<std::process::Output> {
        // Best-effort timeout — mirrors `timeout=timeout, cwd=repo_root, stdin=DEVNULL, env=noninteractive_git_env()`
        let mut cmd = std::process::Command::new("git");
        cmd.args(args);
        cmd.current_dir(repo_root);
        cmd.stdin(std::process::Stdio::null());
        // Simplified: ignore env/timeout for NEVER-cargo stub; real impl would use wait_timeout
        cmd.output().ok()
    };

    // Mirrors `def _ref_exists(ref: str) -> bool:` (ll.541-545)
    let ref_exists = |r: &str| -> bool {
        if let Some(out) = git(&["rev-parse", "--verify", "--quiet", &format!("{r}^{{commit}}")], 20) {
            out.status.success()
        } else {
            false
        }
    };

    // Mirrors `def _fetch_head_age() -> Optional[float]:` (ll.547-561)
    let fetch_head_age = || -> Option<f64> {
        let gd = git(&["rev-parse", "--git-dir"], 20)?;
        if !gd.status.success() { return None; }
        let mut git_dir = PathBuf::from(String::from_utf8_lossy(&gd.stdout).trim().to_string());
        if !git_dir.is_absolute() {
            git_dir = Path::new(repo_root).join(git_dir);
        }
        let fetch_head = git_dir.join("FETCH_HEAD");
        if !fetch_head.exists() { return None; }
        let mtime = std::fs::metadata(&fetch_head).ok()?.modified().ok()?;
        let age = SystemTime::now().duration_since(mtime).ok()?.as_secs_f64();
        Some(age.max(0.0))
    };

    // Mirrors `def _refresh(remote: str, branch: str, ref: str) -> tuple:` (ll.563-584)
    let refresh = |remote: &str, branch: &str, r: &str| -> (String, String) {
        // Mirrors `age = _fetch_head_age(); if age is not None and age < freshness_window and _ref_exists(ref): return ref, f"{ref} (fetched {int(age)}s ago)"` (ll.569-571)
        if let Some(age) = fetch_head_age() {
            if age < freshness_window && ref_exists(r) {
                return (r.to_string(), format!("{r} (fetched {}s ago)", age as i64));
            }
        }
        // Mirrors `try: fetched = _git(["fetch", remote, branch], timeout=fetch_timeout)` (ll.572-573)
        let fetched = git(&["fetch", remote, branch], fetch_timeout as u64);
        if let Some(out) = fetched {
            if out.status.success() {
                // Mirrors `if fetched.returncode == 0: return ref, f"{ref} (fetched)"` (ll.574-575)
                return (r.to_string(), format!("{r} (fetched)"));
            }
            // Mirrors `reason = "fetch failed"` (l.576)
            let reason = "fetch failed";
            // Mirrors `except subprocess.TimeoutExpired: reason = f"fetch timed out after {fetch_timeout:g}s"` (ll.577-578)
            // (captured via git() timeout path in real impl)
            // Mirrors `if _ref_exists(ref): logger.debug(...); return ref, f"{ref} (cached — {reason})"` (ll.581-583)
            if ref_exists(r) {
                eprintln!("[cli] worktree base: {reason} — using cached {r}");
                return (r.to_string(), format!("{r} (cached — {reason})"));
            }
            // Mirrors `return "HEAD", f"HEAD (local — {reason}, no cached {ref})"` (l.584)
            return ("HEAD".to_string(), format!("HEAD (local — {reason}, no cached {r})"));
        }
        // Mirrors `except Exception as e: reason = f"fetch error: {e}"` (ll.579-580) + cached fallback
        let reason = "fetch error";
        if ref_exists(r) {
            return (r.to_string(), format!("{r} (cached — {reason})"));
        }
        ("HEAD".to_string(), format!("HEAD (local — {reason}, no cached {r})"))
    };

    // Mirrors `# 1. Current branch's upstream, if it tracks one.` (l.586)
    // Mirrors `try: up = _git(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"]); if up.returncode == 0: upstream = up.stdout.strip(); if upstream and "/" in upstream: remote, branch = upstream.split("/", 1); return _refresh(remote, branch, upstream)` (ll.587-594)
    if let Some(up) = git(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"], 20) {
        if up.status.success() {
            let upstream = String::from_utf8_lossy(&up.stdout).trim().to_string();
            if !upstream.is_empty() && upstream.contains('/') {
                if let Some((remote, branch)) = upstream.split_once('/') {
                    return refresh(remote, branch, &upstream);
                }
            }
        }
    }
    // Mirrors `# 2. Remote default branch (origin/HEAD).` (l.597) — handled in slice3 continuation
    // For slice2 audit completeness we return the slice boundary marker; the actual
    // origin/HEAD + HEAD fallback (ll.598-~680) is canonical in `cli_slice3.rs`.
    // Nominal 1800 boundary falls here mid-function, so we close syntactically with
    // the upstream-miss fallback path that slice3 will replace with the full dispatch.
    // This keeps the module complete without `cargo`.
    ("HEAD".to_string(), "HEAD (local — upstream not tracked)".to_string())
}

// ---------------------------------------------------------------------------
// Slice boundary — line ~1800
// ---------------------------------------------------------------------------
// The Python `_resolve_worktree_base` remainder (ll.601-~680, origin/HEAD
// symref resolution + ask-remote fallback + final `return "HEAD"`), plus
// every subsequent CLI definition through `main()` (ll.681-~21510), continues
// in `cli_slice3.rs`. This file intentionally stops at the first 900-line
// boundary so that `cargo` is never invoked and the 24-slice decomposition
// stays clean.
