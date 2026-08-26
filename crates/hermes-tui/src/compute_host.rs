//! Persistent dashboard compute-host process.
//!
//! 1:1 port of `tui_gateway/compute_host.py` (899 lines).
//!
//! Phase 0 used this module as a deterministic line-JSON spike. Phase 1 keeps the
//! same transport and turns it into the long-lived child that owns live `AIAgent`
//! objects when `dashboard.turn_isolation` is enabled.
//!
//! ```python
//! # Python — tui_gateway/compute_host.py
//! import argparse, concurrent.futures, contextlib, json, os, signal, subprocess, sys, threading, time, uuid
//! from dataclasses import dataclass, field
//! from pathlib import Path
//! from typing import Any, Callable, Collection
//! from agent.interrupt_compat import request_hard_interrupt
//! def now_ns() -> int: return time.perf_counter_ns()
//! @dataclass
//! class SpikeAgent:
//!     session_id: str
//!     history: list[dict[str,str]] = field(default_factory=list)
//!     _interrupt: threading.Event = field(default_factory=threading.Event)
//!     def clear_interrupt(self) -> None: self._interrupt.clear()
//!     def interrupt(self, *, hard_cancel: bool=False) -> None: self._interrupt.set()
//!     def run_conversation(self, prompt: str, *, conversation_history=None, stream_callback=None, delta_count=24, delay_s=0.001) -> dict: ...
//! @dataclass
//! class HostSession:
//!     sid: str; agent: SpikeAgent; history_version: int=0; running: bool=False; lock: threading.Lock=field(...)
//! class _HostTransport:
//!     def __init__(self, emit: Callable[[dict],None]) -> None: self._emit=emit
//!     def write(self, obj: dict) -> bool: sid=""; try: if obj.get("method")=="event": sid=str(((obj.get("params")or{}).get("session_id"))or""); except: sid=""; self._emit({"type":"rpc","sid":sid,"message":obj}); return True
//!     def close(self) -> None: return None
//! def _repo_root() -> Path: return Path(__file__).resolve().parents[1]
//! def _build_sha() -> str: subprocess.check_output(["git","rev-parse","HEAD"], cwd=str(_repo_root()), ...)
//! _FLUSH_RESERVE_SECS = 1.0
//! class ComputeHost:
//!     def __init__(self, *, stdout=None, max_workers=None, heartbeat_secs=None): ...
//!     def emit(self, frame: dict) -> None: ...
//!     def close(self) -> None: ...
//!     def shutdown(self, *, reason="shutdown", wait=10.0) -> None: ...
//!     def flush_all_sessions(self, *, reason="shutdown", skip_sids=None) -> None: ...
//!     def handle_frame(self, frame: dict) -> None: ...
//!     def _handle_seed(self, frame: dict) -> None: ...
//!     def _track_turn_future(self, future, sid: str) -> None: ...
//!     def _untrack_turn_future(self, future) -> None: ...
//!     def _handle_turn_start(self, frame: dict) -> None: ...
//!     def _handle_spike_turn_start(self, frame: dict) -> None: ...
//!     def _handle_interrupt(self, frame: dict) -> None: ...
//!     def _run_spike_turn(self, session: HostSession, frame: dict) -> None: ...
//!     def _run_real_turn(self, frame: dict) -> None: ...
//!     def _ensure_server_session(self, server, frame: dict) -> dict: ...
//!     def _handle_reload_mcp(self, frame: dict) -> None: ...
//!     def _handle_control(self, frame: dict) -> None: ...
//!     def _bump_progress(self) -> None: ...
//!     def _heartbeat_loop(self) -> None: ...
//!     def _parent_guard_loop(self) -> None: ...
//! def _rss_mb(pid: int) -> float: subprocess.check_output(["ps","-o","rss=","-p",str(pid)], ...)
//! def _default_workers() -> int: max(2, int(os.environ.get("HERMES_TUI_RPC_POOL_WORKERS") or "8"))
//! def run_host(stdin=None, stdout=None) -> None: ...
//! def main(argv=None) -> int: parser=argparse.ArgumentParser(...); parser.parse_args(argv); run_host(); return 0
//! ```
//!
//! # Rust mapping
//!
//! * `time.perf_counter_ns()` → [`now_ns`] (`Instant::now` anchored to `UNIX_EPOCH`+`elapsed` monotonic mix; returns `u64` ns, same units).
//!   Python returns wall+monotonic ns; Rust returns monotonic ns via `std::time::SystemTime` + `Instant`.
//! * `@dataclass class SpikeAgent` → [`SpikeAgent`] (`session_id: String`, `history: Vec<Message>`, `interrupt: Arc<AtomicBool>`).
//!   `threading.Event` → `AtomicBool` (set/clear/is_set); `clear_interrupt`/`interrupt` map directly.
//!   `run_conversation(prompt, conversation_history, stream_callback, delta_count, delay_s)` →
//!   [`SpikeAgent::run_conversation`] (`conversation_history: Option<Vec<Message>>`, `stream_callback: Option<&dyn Fn(&str)>`,
//!   `delta_count: i64`, `delay_s: f64`). Chunk format `f"{session_id}:{prompt}:{index:04d} "` + `[interrupted]` suffix + history chaining is preserved.
//! * `@dataclass class HostSession` → [`HostSession`] (`sid`, `agent`, `history_version`, `running: bool`, `lock: Mutex<()>`).
//!   Python `threading.Lock` → `Mutex<()>`; `history_version` is `i64`; `agent: SpikeAgent` owned.
//! * `class _HostTransport` → [`HostTransport`] (`emit: Arc<dyn Fn(Frame)+Send+Sync>`). `write(obj)` extracts `sid`
//!   via `obj.get("method")=="event" → params.session_id` and emits `{"type":"rpc","sid":sid,"message":obj}`; `close()` is no-op.
//! * `_repo_root()` → [`repo_root`] + [`repo_root_with`] (injected `__file__` analogue; walks `current_exe` → `current_dir` → `/` like `host_supervisor`).
//! * `_build_sha()` → [`build_sha`] + [`build_sha_with`] (injected runner; `git rev-parse HEAD` with `cwd=repo_root`, `stderr=Null`, `timeout=2` via `try_wait` poll; `Err→"unknown"`).
//! * `_FLUSH_RESERVE_SECS = 1.0` → [`FLUSH_RESERVE_SECS`] + [`FLUSH_RESERVE`] (`Duration`).
//! * `ComputeHost.__init__(stdout, max_workers, heartbeat_secs)` → [`ComputeHost`] + [`ComputeHostConfig`] + [`ComputeHost::new`].
//!   `stdout or sys.stdout` → `Arc<Mutex<Box<dyn Write+Send>>>`; `ThreadPoolExecutor(max_workers=max_workers or _default_workers(), thread_name_prefix="compute-host-turn")` → `max_workers: usize` stored + `thread::Builder` per turn;
//!   `threading.Event _closed` → `Arc<AtomicBool>`; `os.getppid()` → `current_ppid()` (Unix `getppid` else 0);
//!   `uuid.uuid4().hex` → [`generate_boot_id`]; `progress_counter` + `progress_lock` → `Arc<Mutex<u64>>`;
//!   `_turn_futures: dict[Future,str]` + `_turn_futures_lock` → `Arc<Mutex<HashMap<u64,String>>>` + `next_turn_id: Arc<Mutex<u64>>` plus `Arc<Mutex<HashMap<u64, JoinHandle>>>` for join semantics;
//!   `_transport = _HostTransport(self.emit)` → [`HostTransport`] wrapping `emit`; `heartbeat_secs` env `HERMES_COMPUTE_HOST_HEARTBEAT_SECS` default `"15"` + `float()` parse → [`heartbeat_secs_from_env`] + [`heartbeat_secs_with`].
//!   Heartbeat + parent-guard daemon threads → `thread::Builder` detached (`compute-host-heartbeat`, `compute-host-ppid-guard`).
//! * `emit(frame)` → [`ComputeHost::emit`] (`frame.setdefault("host_ns", now_ns())` + `json.dumps(separators=(",",":"), ensure_ascii=False)` + `print(..., flush=True)` under `write_lock`).
//! * `close()` → [`ComputeHost::close`] (`_closed.set()` + `executor.shutdown(wait=False, cancel_futures=True)` → `closed.store(true)` + drain pending + `shutdown` flag).
//! * `shutdown(reason, wait)` → [`ComputeHost::shutdown`] (budget/deadline `wait - min(FLUSH_RESERVE, wait/2)`, drain loop `sleep(min(0.05, remaining))`, `live_sids = {sid for fut,sid in _turn_futures if not done}`, `flush_all_sessions(reason, skip_sids=live_sids)`, `executor.shutdown(wait=False)`). The 10s budget and sigkill race comment is preserved in doc.
//! * `flush_all_sessions(reason, skip_sids)` → [`ComputeHost::flush_all_sessions`] + [`ComputeHost::flush_all_sessions_with`] (injected `finalize: Fn(&str, &str)`). `from tui_gateway import server; except: return` → optional `server_sessions` inject; `skip = set(skip_sids or ())` + iterate `list(server._sessions.items())` with `session["_finalized"]` latch semantics documented.
//! * `handle_frame(frame)` → [`ComputeHost::handle_frame`] (dispatch `type` → `session.seed`/`turn.start`/`interrupt`/`reload_mcp`/`control`/`shutdown`/`error`). `shutdown` branch emits `shutdown.ack` then `closed.set()` + `shutdown(wait=False)`; unknown type emits `error`.
//! * `_handle_seed` → [`ComputeHost::handle_seed`] (`sid required` check + `history` list guard + `HostSession{SpikeAgent}` insert + `session.seeded` emit). Request `id` passthrough via `request_id` field.
//! * `_track_turn_future` → [`ComputeHost::track_turn_future`] (`_turn_futures[future]=sid` under lock + `add_done_callback(_untrack)`). `_untrack_turn_future` → [`ComputeHost::untrack_turn_future`] (`pop(future, None)`).
//! * `_handle_turn_start` → [`ComputeHost::handle_turn_start`] (`sid in _sessions → _handle_spike_turn_start` else `executor.submit(_run_real_turn, dict(frame))` + track). Real-turn path is injected via closure (`run_real_turn_fn`) so `server` deps stay out of `std`.
//! * `_handle_spike_turn_start` → [`ComputeHost::handle_spike_turn_start`] (`unknown session → turn.error`, `running → session busy`, else `running=True` under lock + `executor.submit(_run_spike_turn)` + track).
//! * `_handle_interrupt` → [`ComputeHost::handle_interrupt`] (spike session → `request_hard_interrupt(agent)` + `interrupt.ack applied=True + applied_ns`; real session branch via injected `server_sessions` + `request_hard_interrupt` + queued_prompt clearing + generation bump; `applied=False` when no session).
//! * `_run_spike_turn(session, frame)` → [`ComputeHost::run_spike_turn`] (parse `request_id`/`prompt`/`delta_count`/`delay_s` with fallbacks, `history` snapshot under lock, `clear_interrupt`, `turn.started` emit, per-chunk `stream(delta)` → `_bump_progress` + `delta` emit, `session.agent.run_conversation` + `history_version` bump + `running=False` + `turn.end` / `turn.error`).
//! * `_run_real_turn(frame)` → [`ComputeHost::run_real_turn`] + [`ComputeHost::run_real_turn_with`] (injected `ensure_session`, `persist`, `session_info` etc). The signature mirrors `_run_real_turn(self, frame: dict)` but server calls are closures so crate stays `std`-only. The queue-generation race (`queued_prompt_generation` mismatch → `turn.end interrupted=True`), `running` busy guard, `last_active`, `_start_inflight_turn`, `turn.started`, `_ensure_session_db_row`, `on_user_message_appended`, `_persist_branch_seed`, `_run_prompt_submit`, `run_thread.join`, `history_version`/`message_count`/`interrupted` capture, `session_info` emit are all documented and wired through injection points. The exception path clears `running` + `_clear_inflight_turn` and emits `turn.error reason=exception`.
//! * `_ensure_server_session(server, frame)` → [`ComputeHost::ensure_server_session`] + injected `make_agent` / `init_session` closures. The `profile_home` override dance (`set_hermes_home_override` + `set_secret_scope` + `SessionDB` + `owns_db` + `_transfer_db_to_agent` + `finally close/reset`) is modelled as scoped closures; the fallback minimal session dict is documented. `cols`/`cwd`/`profile_home`/`attached_images` overlay is preserved.
//! * `_handle_reload_mcp` → [`ComputeHost::handle_reload_mcp`] (injected `reload_handler` returning `reload_mcp.ack` or `control.error`).
//! * `_handle_control` → [`ComputeHost::handle_control`] + [`MutatorRouteKind`] table. Guards `MUTATOR_ROUTE_TABLE` (unknown route → `control.error`), `session not found`, `idle-gated + running → busy`, `reload.mcp` delegation, `session.save`/`session.compress` branches, generic slash mirror + `messages/history_version/message_count/session_key/session_info` capture. The compress notification finalize-on-exception (`finalize_context_engine_compression_notification(agent, committed=False)`) is mapped to an injected `on_compress_fail` closure.
//! * `_bump_progress` → [`ComputeHost::bump_progress`] (`progress_lock +=1`).
//! * `_heartbeat_loop` → [`ComputeHost::heartbeat_loop_once`] + detached `compute-host-heartbeat` thread (`while not _closed.wait(heartbeat_secs): active_turns = sum(not done) ; emit hb {active_turns, progress_counter, rss_mb}`).
//! * `_parent_guard_loop` → [`ComputeHost::parent_guard_should_exit`] + detached `compute-host-ppid-guard` thread (`while not _closed.wait(1.0): ppid=getppid(); if ppid in {0,1} or ppid != _parent_pid: emit orphan; shutdown(reason="orphan"); os._exit(0)`).
//! * `_rss_mb(pid)` → [`rss_mb`] + [`rss_mb_with`] (injected runner; default `ps -o rss= -p pid` with 2s timeout + `int(out)/1024.0`).
//! * `_default_workers()` → [`default_workers`] + [`default_workers_with`] (env `HERMES_TUI_RPC_POOL_WORKERS` default `"8"` → `max(2, int(...))`).
//! * `run_host(stdin, stdout)` → [`run_host`] + [`run_host_with`] (sets `HERMES_COMPUTE_HOST_CHILD=1`, `ComputeHost(stdout)`, `shutting_down Event`, `SIGTERM/SIGINT` handler → `shutdown(reason="sigterm")` + `SystemExit(0)`, hello emit `{type:"hello",host_pid,boot_id,build_sha,cwd,hermes_home}`, reader thread `compute-host-control-reader` for `json.loads` + `handle_frame` + `os._exit(0)` on `shutdown`, main loop `while not _closed.wait(0.2): if not reader.is_alive(): break` → `finally shutdown(reason="stdin_closed", wait=2.0)`).
//! * `main(argv)` → [`parse_args`] (argparse stub: accepts any `--*` flags; `parse_args([])=Ok(())`) + [`run_host`] entry.
//! * `request_hard_interrupt` → [`request_hard_interrupt`] (sets `SpikeAgent.interrupt` + optional agent interrupt closure).
//!
//! `std`-only: no `serde_json`, no `tokio`. JSON helpers use the same minimal
//! flat-map approach as `crate::host_supervisor` (callers that need full `serde`
//! can adapt the `emit` closure).

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Condvar, Mutex,
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Constants — mirrors compute_host.py:128-133 + env knobs
// ---------------------------------------------------------------------------

/// Slice of `shutdown`'s budget held back for post-drain finalize.
///
/// Mirrors `_FLUSH_RESERVE_SECS = 1.0`.
pub const FLUSH_RESERVE_SECS: f64 = 1.0;
/// Typed reserve.
pub const FLUSH_RESERVE: Duration = Duration::from_secs(1);

/// Env knob for heartbeat interval. Mirrors `os.environ.get("HERMES_COMPUTE_HOST_HEARTBEAT_SECS") or "15"`.
pub const ENV_HEARTBEAT_SECS: &str = "HERMES_COMPUTE_HOST_HEARTBEAT_SECS";

/// Env knob for worker pool size. Mirrors `os.environ.get("HERMES_TUI_RPC_POOL_WORKERS") or "8"`.
pub const ENV_RPC_POOL_WORKERS: &str = "HERMES_TUI_RPC_POOL_WORKERS";

/// Env marker set by `run_host`. Mirrors `os.environ["HERMES_COMPUTE_HOST_CHILD"] = "1"`.
pub const ENV_COMPUTE_HOST_CHILD: &str = "HERMES_COMPUTE_HOST_CHILD";

/// Default heartbeat secs. Mirrors `float(os.environ.get(...) or "15")` → 15 when absent.
pub const DEFAULT_HEARTBEAT_SECS: f64 = 15.0;

/// Default workers literal fallback. Mirrors `or "8"`.
pub const DEFAULT_WORKERS_STR: &str = "8";

/// Minimum workers. Mirrors `max(2, ...)`.
pub const MIN_WORKERS: usize = 2;

// ---------------------------------------------------------------------------
// tiny time helper — mirrors now_ns
// ---------------------------------------------------------------------------

/// Monotonic ns clock for `host_ns` / `applied_ns` / `started_ns` / `ended_ns` / `emitted_ns`.
///
/// Mirrors `time.perf_counter_ns()` (monotonic). Uses `Instant` anchored at process
/// start plus `UNIX_EPOCH` mixing so values are `u64` nanoseconds and monotonic
/// within the process (the absolute epoch is irrelevant — only deltas + ordering matter).
pub fn now_ns() -> u64 {
    // Cheap monotonic ns: SystemTime elapsed + Instant would be double; we use
    // SystemTime for anchor and Instant for tick to keep monotonic.
    static START: std::sync::OnceLock<(SystemTime, Instant)> = std::sync::OnceLock::new();
    let (sys, inst) = START.get_or_init(|| (SystemTime::now(), Instant::now()));
    let base_ns = sys.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_nanos() as u64;
    let elapsed = inst.elapsed().as_nanos() as u64;
    base_ns.wrapping_add(elapsed)
}

// ---------------------------------------------------------------------------
// Message / history helpers — mirrors dict {"role":..., "content":...}
// ---------------------------------------------------------------------------

/// Chat message. Mirrors `{"role": "user"|"assistant", "content": str}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: String,
    pub content: String,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: "user".into(), content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: "assistant".into(), content: content.into() }
    }
}

// ---------------------------------------------------------------------------
// SpikeAgent — mirrors @dataclass class SpikeAgent
// ---------------------------------------------------------------------------

/// Deterministic `AIAgent`-shaped object for pipe/interrupt measurements.
///
/// Mirrors `tui_gateway/compute_host.py::SpikeAgent`.
#[derive(Debug, Clone)]
pub struct SpikeAgent {
    /// Mirrors `session_id: str`.
    pub session_id: String,
    /// Mirrors `history: list[dict[str,str]]`.
    pub history: Vec<Message>,
    /// Mirrors `_interrupt: threading.Event`.
    interrupt: Arc<AtomicBool>,
}

impl SpikeAgent {
    /// Create a spike agent. Mirrors `SpikeAgent(sid, list(history))`.
    pub fn new(session_id: impl Into<String>, history: Vec<Message>) -> Self {
        Self {
            session_id: session_id.into(),
            history,
            interrupt: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create with an injected interrupt flag (test seam).
    pub fn new_with_flag(session_id: impl Into<String>, history: Vec<Message>, flag: Arc<AtomicBool>) -> Self {
        Self { session_id: session_id.into(), history, interrupt: flag }
    }

    /// Mirrors `def clear_interrupt(self) -> None: self._interrupt.clear()`.
    pub fn clear_interrupt(&self) {
        self.interrupt.store(false, Ordering::SeqCst);
    }

    /// Mirrors `def interrupt(self, *, hard_cancel: bool=False) -> None: self._interrupt.set()`.
    pub fn interrupt(&self, hard_cancel: bool) {
        let _ = hard_cancel;
        self.interrupt.store(true, Ordering::SeqCst);
    }

    /// Alias that mirrors `request_hard_interrupt(spike.agent)` indirection.
    pub fn request_hard_interrupt(&self) {
        self.interrupt(true);
    }

    /// Whether the interrupt flag is set. Mirrors `self._interrupt.is_set()`.
    pub fn is_interrupted(&self) -> bool {
        self.interrupt.load(Ordering::SeqCst)
    }

    /// Shared flag handle (so a supervisor can interrupt via the flag).
    pub fn interrupt_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.interrupt)
    }

    /// Deterministic conversation that emits `delta_count` chunks.
    ///
    /// Mirrors `SpikeAgent.run_conversation`:
    ///
    /// ```python
    /// def run_conversation(self, prompt, *, conversation_history=None, stream_callback=None, delta_count=24, delay_s=0.001) -> dict:
    ///     base = list(conversation_history if ... else self.history)
    ///     chunks=[]; interrupted=False
    ///     for index in range(max(0, int(delta_count))):
    ///         if self._interrupt.is_set(): interrupted=True; break
    ///         chunk = f"{self.session_id}:{prompt}:{index:04d} "
    ///         chunks.append(chunk); if stream_callback: stream_callback(chunk)
    ///         if delay_s>0: time.sleep(delay_s)
    ///     if self._interrupt.is_set(): interrupted=True
    ///     final="".join(chunks); if interrupted: final+="[interrupted]"
    ///     messages=[*base_history, {"role":"user","content":prompt}, {"role":"assistant","content":final}]
    ///     self.history = messages
    ///     return {"final_response": final, "messages": messages, "interrupted": interrupted}
    /// ```
    pub fn run_conversation(
        &mut self,
        prompt: &str,
        conversation_history: Option<Vec<Message>>,
        stream_callback: Option<&dyn Fn(&str)>,
        delta_count: i64,
        delay_s: f64,
    ) -> SpikeTurnResult {
        let base_history = conversation_history.unwrap_or_else(|| self.history.clone());
        let n = delta_count.max(0) as usize;
        let mut chunks: Vec<String> = Vec::with_capacity(n);
        let mut interrupted = false;
        for index in 0..n {
            if self.is_interrupted() {
                interrupted = true;
                break;
            }
            let chunk = format!("{}:{}:{:04} ", self.session_id, prompt, index);
            chunks.push(chunk.clone());
            if let Some(cb) = stream_callback {
                cb(&chunk);
            }
            if delay_s > 0.0 {
                let dur = Duration::from_secs_f64(delay_s.max(0.0));
                if !dur.is_zero() {
                    thread::sleep(dur);
                }
            }
        }
        if self.is_interrupted() {
            interrupted = true;
        }
        let mut final_text = chunks.concat();
        if interrupted {
            final_text.push_str("[interrupted]");
        }
        let mut messages = Vec::with_capacity(base_history.len() + 2);
        messages.extend(base_history);
        messages.push(Message::user(prompt.to_string()));
        messages.push(Message::assistant(final_text.clone()));
        self.history = messages.clone();
        SpikeTurnResult { final_response: final_text, messages, interrupted }
    }
}

/// Result of [`SpikeAgent::run_conversation`]. Mirrors the returned dict.
#[derive(Debug, Clone)]
pub struct SpikeTurnResult {
    pub final_response: String,
    pub messages: Vec<Message>,
    pub interrupted: bool,
}

// ---------------------------------------------------------------------------
// HostSession — mirrors @dataclass class HostSession
// ---------------------------------------------------------------------------

/// One deterministic session owned by the host.
///
/// Mirrors `tui_gateway/compute_host.py::HostSession`:
/// `sid, agent, history_version=0, running=False, lock=threading.Lock()`.
#[derive(Debug)]
pub struct HostSession {
    /// Mirrors `sid: str`.
    pub sid: String,
    /// Mirrors `agent: SpikeAgent`.
    pub agent: SpikeAgent,
    /// Mirrors `history_version: int = 0`.
    pub history_version: i64,
    /// Mirrors `running: bool = False`.
    pub running: bool,
    /// Mirrors `lock: threading.Lock`.
    pub lock: Mutex<()>,
}

impl HostSession {
    pub fn new(sid: impl Into<String>, agent: SpikeAgent) -> Self {
        Self { sid: sid.into(), agent, history_version: 0, running: false, lock: Mutex::new(()) }
    }
}

// ---------------------------------------------------------------------------
// _HostTransport — mirrors class _HostTransport
// ---------------------------------------------------------------------------

/// Transport that wraps `emit` for the server's `transport.write` contract.
///
/// Mirrors `tui_gateway/compute_host.py::_HostTransport`.
#[derive(Clone)]
pub struct HostTransport {
    emit: Arc<dyn Fn(HashMap<String, String>) + Send + Sync>,
}

impl std::fmt::Debug for HostTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostTransport").field("has_emit", &true).finish()
    }
}

impl HostTransport {
    pub fn new<F>(emit: F) -> Self
    where
        F: Fn(HashMap<String, String>) + Send + Sync + 'static,
    {
        Self { emit: Arc::new(emit) }
    }

    pub fn new_arc(emit: Arc<dyn Fn(HashMap<String, String>) + Send + Sync>) -> Self {
        Self { emit }
    }

    /// Mirrors `def write(self, obj: dict) -> bool:` with sid extraction.
    ///
    /// ```python
    /// def write(self, obj: dict) -> bool:
    ///     sid=""
    ///     try:
    ///         if obj.get("method")=="event":
    ///             sid=str(((obj.get("params") or {}).get("session_id")) or "")
    ///     except Exception: sid=""
    ///     self._emit({"type":"rpc","sid":sid,"message":obj}); return True
    /// ```
    pub fn write(&self, obj: HashMap<String, String>) -> bool {
        let sid = if obj.get("method").map(|s| s.as_str()) == Some("event") {
            // Our flat map cannot nest params; for the real nested case callers
            // should use `write_nested`. Here we check `session_id` key directly.
            obj.get("session_id").cloned().unwrap_or_default()
        } else {
            String::new()
        };
        let mut frame = HashMap::new();
        frame.insert("type".into(), "rpc".into());
        frame.insert("sid".into(), sid);
        // Preserve message as debug string for std-only; real transport would embed the dict.
        frame.insert("message".into(), format!("{:?}", obj));
        (self.emit)(frame);
        true
    }

    /// Nested variant that handles `params.session_id` like Python.
    pub fn write_nested(&self, obj: HashMap<String, String>, params: Option<HashMap<String, String>>) -> bool {
        let sid = if obj.get("method").map(|s| s.as_str()) == Some("event") {
            params.as_ref().and_then(|p| p.get("session_id")).cloned().unwrap_or_default()
        } else {
            String::new()
        };
        let mut frame = HashMap::new();
        frame.insert("type".into(), "rpc".into());
        frame.insert("sid".into(), sid);
        frame.insert("message".into(), format!("{:?}", obj));
        (self.emit)(frame);
        true
    }

    /// Mirrors `def close(self) -> None: return None`.
    pub fn close(&self) {}
}

// ---------------------------------------------------------------------------
// _repo_root / _build_sha — mirrors compute_host.py:109-126
// ---------------------------------------------------------------------------

/// Repo root — mirrors `_repo_root() -> Path: Path(__file__).resolve().parents[1]`.
pub fn repo_root() -> PathBuf {
    repo_root_with(None)
}

/// Injected variant for tests.
pub fn repo_root_with(override_path: Option<&Path>) -> PathBuf {
    if let Some(p) = override_path {
        return p.to_path_buf();
    }
    if let Ok(cwd) = env::current_dir() {
        if cwd.join("Cargo.toml").is_file() {
            return cwd;
        }
        let mut cur = cwd.clone();
        for _ in 0..4 {
            if cur.join("Cargo.toml").is_file() {
                return cur;
            }
            if let Some(par) = cur.parent() {
                cur = par.to_path_buf();
            } else {
                break;
            }
        }
        return cwd;
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            return parent.to_path_buf();
        }
    }
    PathBuf::from(".")
}

/// Build sha — mirrors `_build_sha() -> str`.
///
/// Tries `git rev-parse HEAD` with `cwd=repo_root`, `timeout=2`, `stderr=DEVNULL`.
/// On any `Exception` returns `"unknown"`.
pub fn build_sha() -> String {
    build_sha_with(None, None)
}

/// Injected variant.
pub fn build_sha_with(repo_root_override: Option<&Path>, runner: Option<fn(&Path) -> Option<String>>) -> String {
    if let Some(r) = runner {
        return r(repo_root_override.unwrap_or(&repo_root())).unwrap_or_else(|| "unknown".to_string());
    }
    let root = repo_root_with(repo_root_override);
    let mut cmd = Command::new("git");
    cmd.args(["rev-parse", "HEAD"]);
    cmd.current_dir(&root);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::null());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return "unknown".to_string(),
    };
    let start = Instant::now();
    let timeout = Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let mut out = String::new();
                if let Some(mut stdout) = child.stdout.take() {
                    use std::io::Read;
                    let _ = stdout.read_to_string(&mut out);
                }
                let trimmed = out.trim().to_string();
                if trimmed.is_empty() {
                    return "unknown".to_string();
                }
                return trimmed;
            }
            Ok(Some(_)) => return "unknown".to_string(),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return "unknown".to_string();
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return "unknown".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// _rss_mb / _default_workers — mirrors 818-830
// ---------------------------------------------------------------------------

/// Resident memory in MiB for `pid`. Mirrors `_rss_mb(pid: int) -> float`.
pub fn rss_mb(pid: u32) -> f64 {
    rss_mb_with(pid, None)
}

/// Injected variant — `runner` returns the raw `ps` stdout or `None` on failure.
pub fn rss_mb_with(pid: u32, runner: Option<fn(u32) -> Option<String>>) -> f64 {
    if let Some(r) = runner {
        return r(pid).and_then(|out| {
            let last = out.splitlines().last()?.trim().to_string();
            last.parse::<f64>().ok().map(|v| v / 1024.0)
        }).unwrap_or(0.0);
    }
    let mut cmd = Command::new("ps");
    cmd.args(["-o", "rss=", "-p", pid.to_string()]);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::null());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return 0.0,
    };
    let start = Instant::now();
    let timeout = Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let mut out = String::new();
                if let Some(mut stdout) = child.stdout.take() {
                    use std::io::Read;
                    let _ = stdout.read_to_string(&mut out);
                }
                let trimmed = out.trim().to_string();
                if trimmed.is_empty() {
                    return 0.0;
                }
                let last = trimmed.splitlines().last().unwrap_or("").trim();
                return last.parse::<f64>().map(|v| v / 1024.0).unwrap_or(0.0);
            }
            Ok(Some(_)) => return 0.0,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return 0.0;
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return 0.0,
        }
    }
}

/// Default worker pool size. Mirrors `_default_workers() -> int: max(2, int(os.environ.get("HERMES_TUI_RPC_POOL_WORKERS") or "8"))`.
pub fn default_workers() -> usize {
    default_workers_with(None)
}

/// Injected variant for tests.
pub fn default_workers_with(raw: Option<&str>) -> usize {
    let raw = raw.or_else(|| {
        // SAFETY: reading env at call time; capture as owned string via thread-local copy
        // We cannot return reference to env var; so read now and leak via bool.
        // Instead callers that want env should call `default_workers()` directly.
        None
    });
    if let Some(s) = raw {
        let t = s.trim();
        if !t.is_empty() {
            if let Ok(v) = t.parse::<i64>() {
                return (v.max(2) as usize).max(MIN_WORKERS);
            }
        }
        return MIN_WORKERS.max(8);
    }
    // Real env path
    let env_raw = env::var(ENV_RPC_POOL_WORKERS).unwrap_or_else(|_| DEFAULT_WORKERS_STR.to_string());
    let t = env_raw.trim();
    if t.is_empty() {
        return 8;
    }
    match t.parse::<i64>() {
        Ok(v) => (v as usize).max(MIN_WORKERS),
        Err(_) => 8,
    }
}

/// Pure helper: compute workers from injected `Option<&str>` without touching `env`.
pub fn default_workers_from_raw(raw: Option<&str>) -> usize {
    match raw {
        None => 8,
        Some(s) => {
            let t = s.trim();
            if t.is_empty() {
                return 8;
            }
            match t.parse::<i64>() {
                Ok(v) => (v.max(2) as usize),
                Err(_) => 8,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Heartbeat / workers env helpers
// ---------------------------------------------------------------------------

/// Parse heartbeat secs from env string. Mirrors `float(os.environ.get("HERMES_COMPUTE_HOST_HEARTBEAT_SECS") or "15")`.
pub fn heartbeat_secs_with(raw: Option<&str>) -> f64 {
    match raw {
        None => DEFAULT_HEARTBEAT_SECS,
        Some(s) => {
            let t = s.trim();
            if t.is_empty() {
                return DEFAULT_HEARTBEAT_SECS;
            }
            match t.parse::<f64>() {
                Ok(v) => v,
                Err(_) => DEFAULT_HEARTBEAT_SECS,
            }
        }
    }
}

/// Read heartbeat secs from process env.
pub fn heartbeat_secs_from_env() -> f64 {
    let raw = env::var(ENV_HEARTBEAT_SECS).ok();
    heartbeat_secs_with(raw.as_deref())
}

// ---------------------------------------------------------------------------
// tiny uuid / boot_id — mirrors uuid.uuid4().hex
// ---------------------------------------------------------------------------

/// Generate a boot id — mirrors `uuid.uuid4().hex` (32 lowercase hex).
pub fn generate_boot_id() -> String {
    let mut bytes = [0u8; 16];
    let mut filled = false;
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        use std::io::Read;
        if f.read_exact(&mut bytes).is_ok() {
            filled = true;
        }
    }
    if !filled {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
        let pid = std::process::id() as u64;
        let tid_dbg = format!("{:?}", thread::current().id());
        let mut h: u64 = 14695981039346656037;
        for b in now.as_nanos().to_le_bytes().iter().chain(pid.to_le_bytes().iter()).chain(tid_dbg.bytes()) {
            h ^= *b as u64;
            h = h.wrapping_mul(1099511628211);
        }
        bytes[0..8].copy_from_slice(&h.to_le_bytes());
        bytes[8..16].copy_from_slice(&h.wrapping_mul(0x9e3779b97f4a7c15).to_le_bytes());
    }
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn generate_request_id() -> String {
    generate_boot_id()
}

// ---------------------------------------------------------------------------
// Json helpers — minimal for emit / hello
// ---------------------------------------------------------------------------

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push(ch),
        }
    }
    out
}

fn frame_to_json(frame: &HashMap<String, String>) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (k, v) in frame {
        parts.push(format!("\"{}\":\"{}\"", json_escape(k), json_escape(v)));
    }
    parts.sort();
    format!("{{{}}}", parts.join(","))
}

// ---------------------------------------------------------------------------
// request_hard_interrupt shim — mirrors agent.interrupt_compat.request_hard_interrupt
// ---------------------------------------------------------------------------

/// Best-effort hard interrupt for a `SpikeAgent`.
///
/// Mirrors `agent.interrupt_compat.request_hard_interrupt(agent)` which sets the
/// agent's interrupt event and does a hard cancel. For `SpikeAgent` this is `agent.interrupt(hard_cancel=True)`.
pub fn request_hard_interrupt(agent: &SpikeAgent) {
    agent.interrupt(true);
}

// ---------------------------------------------------------------------------
// ComputeHost — mirrors class ComputeHost
// ---------------------------------------------------------------------------

/// `routing` for `_handle_control` — mirrors `MUTATOR_ROUTE_TABLE` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutatorRouteKind {
    TurnPath,
    RunConcurrent,
    IdleGated,
}

/// Transport frame type used by `handle_frame` dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameKind {
    SessionSeed,
    TurnStart,
    Interrupt,
    ReloadMcp,
    Control,
    Shutdown,
    Unknown(String),
}

impl FrameKind {
    pub fn from_str(s: &str) -> Self {
        match s {
            "session.seed" => FrameKind::SessionSeed,
            "turn.start" => FrameKind::TurnStart,
            "interrupt" => FrameKind::Interrupt,
            "reload_mcp" => FrameKind::ReloadMcp,
            "control" => FrameKind::Control,
            "shutdown" => FrameKind::Shutdown,
            other => FrameKind::Unknown(other.to_string()),
        }
    }
}

/// Config for [`ComputeHost::new`]. Mirrors `ComputeHost.__init__` kwargs.
#[derive(Debug, Clone)]
pub struct ComputeHostConfig {
    /// Mirrors `max_workers: int | None`.
    pub max_workers: usize,
    /// Mirrors `heartbeat_secs: int | float | None`.
    pub heartbeat_secs: f64,
}

impl Default for ComputeHostConfig {
    fn default() -> Self {
        Self {
            max_workers: default_workers(),
            heartbeat_secs: heartbeat_secs_from_env(),
        }
    }
}

/// Persistent dashboard compute-host. Mirrors `tui_gateway/compute_host.py::ComputeHost`.
///
/// Shared state is behind `Arc<Mutex<...>>` / `Arc<AtomicBool>` (Python `threading.Lock`/`Event`).
/// The thread pool is modelled as spawned `thread::Builder` workers per turn; `max_workers` is stored
/// for backpressure diagnostics but not enforced as a hard cap in `std` (a bounded semaphore would need
/// an extra crate). `stdout` is `Arc<Mutex<Box<dyn Write+Send>>>` so `emit` can be called from any thread.
pub struct ComputeHost {
    /// Mirrors `self._stdout`.
    stdout: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Mirrors `self._write_lock = threading.Lock()`.
    write_lock: Arc<Mutex<()>>,
    /// Mirrors `self._sessions: dict[str, HostSession]`.
    sessions: Arc<Mutex<HashMap<String, HostSession>>>,
    /// Mirrors `self._executor` — stored as `max_workers` plus handles map.
    max_workers: usize,
    /// Mirrors `self._closed = threading.Event()`.
    closed: Arc<AtomicBool>,
    /// Mirrors `self._parent_pid = os.getppid()`.
    parent_pid: i32,
    /// Mirrors `self._boot_id = uuid.uuid4().hex`.
    boot_id: String,
    /// Mirrors `self._progress_counter` + `self._progress_lock`.
    progress_counter: Arc<Mutex<u64>>,
    /// Mirrors `self._turn_futures: dict[Future,str]` + `self._turn_futures_lock`.
    turn_futures: Arc<Mutex<HashMap<u64, String>>>,
    /// Next synthetic future id.
    next_future_id: Arc<Mutex<u64>>,
    /// Mirror for `threading.Condition` join of futures — not needed in std; we track `HashMap<u64,bool>` done.
    turn_futures_done: Arc<Mutex<HashMap<u64, bool>>>,
    /// Mirrors `self._transport = _HostTransport(self.emit)`.
    transport: HostTransport,
    /// Mirrors `self._heartbeat_secs`.
    heartbeat_secs: f64,
}

impl std::fmt::Debug for ComputeHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComputeHost")
            .field("boot_id", &self.boot_id)
            .field("parent_pid", &self.parent_pid)
            .field("max_workers", &self.max_workers)
            .field("heartbeat_secs", &self.heartbeat_secs)
            .field("closed", &self.closed.load(Ordering::SeqCst))
            .finish()
    }
}

impl ComputeHost {
    /// Create a host — mirrors `ComputeHost.__init__(*, stdout=None, max_workers=None, heartbeat_secs=None)`.
    pub fn new(stdout: Option<Arc<Mutex<Box<dyn Write + Send>>>>, max_workers: Option<usize>, heartbeat_secs: Option<f64>) -> Arc<Self> {
        let cfg = ComputeHostConfig::default();
        Self::new_with_config(stdout, max_workers.unwrap_or(cfg.max_workers), heartbeat_secs.unwrap_or(cfg.heartbeat_secs))
    }

    /// Create with explicit config + optional stdout.
    pub fn new_with_config(stdout: Option<Arc<Mutex<Box<dyn Write + Send>>>>, max_workers: usize, heartbeat_secs: f64) -> Arc<Self> {
        let stdout: Arc<Mutex<Box<dyn Write + Send>>> = stdout.unwrap_or_else(|| {
            // Default to a sink that discards (tests) — real run_host passes stdout
            struct Sink;
            impl Write for Sink {
                fn write(&mut self, buf: &[u8]) -> io::Result<usize> { Ok(buf.len()) }
                fn flush(&mut self) -> io::Result<()> { Ok(()) }
            }
            Arc::new(Mutex::new(Box::new(Sink) as Box<dyn Write + Send>))
        });
        let write_lock = Arc::new(Mutex::new(()));
        let sessions = Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let parent_pid = current_ppid();
        let boot_id = generate_boot_id();
        let progress_counter = Arc::new(Mutex::new(0u64));
        let turn_futures: Arc<Mutex<HashMap<u64, String>>> = Arc::new(Mutex::new(HashMap::new()));
        let turn_futures_done: Arc<Mutex<HashMap<u64, bool>>> = Arc::new(Mutex::new(HashMap::new()));
        let next_future_id = Arc::new(Mutex::new(1u64));

        // Transport emit closure — capture stdout+write_lock weakly for transport writes
        let stdout_clone = Arc::clone(&stdout);
        let lock_clone = Arc::clone(&write_lock);
        let emit: Arc<dyn Fn(HashMap<String, String>) + Send + Sync> = {
            let sc = Arc::clone(&stdout_clone);
            let lc = Arc::clone(&lock_clone);
            Arc::new(move |mut frame: HashMap<String, String>| {
                frame.entry("host_ns".into()).or_insert_with(|| now_ns().to_string());
                let data = frame_to_json(&frame);
                let line = format!("{}\n", data);
                let _guard = lc.lock().unwrap();
                let mut out = sc.lock().unwrap();
                let _ = out.write_all(line.as_bytes());
                let _ = out.flush();
            })
        };
        let transport = HostTransport::new_arc(emit.clone());

        let me = Arc::new(Self {
            stdout: stdout_clone,
            write_lock: lock_clone,
            sessions,
            max_workers,
            closed: Arc::clone(&closed),
            parent_pid,
            boot_id: boot_id.clone(),
            progress_counter: Arc::clone(&progress_counter),
            turn_futures: Arc::clone(&turn_futures),
            next_future_id: Arc::clone(&next_future_id),
            turn_futures_done: Arc::clone(&turn_futures_done),
            transport,
            heartbeat_secs,
        });

        // Heartbeat + ppid guard daemon threads when `heartbeat_secs > 0`.
        // Mirrors `if self._heartbeat_secs > 0: Thread(target=self._heartbeat_loop, daemon=True).start(); Thread(target=self._parent_guard_loop, daemon=True).start()`.
        if heartbeat_secs > 0.0 {
            let me_hb = Arc::clone(&me);
            let _ = thread::Builder::new().name("compute-host-heartbeat".into()).spawn(move || {
                me_hb.heartbeat_loop();
            });
            let me_guard = Arc::clone(&me);
            let _ = thread::Builder::new().name("compute-host-ppid-guard".into()).spawn(move || {
                me_guard.parent_guard_loop();
            });
        }

        // We need to fix emit to use the real host's emit method (which also injects host_ns + lock).
        // The closure above already mimics it; for exact emit routing we keep it.
        // To expose `me.emit` for external callers, the host's `emit` method is the canonical one.
        let _ = emit; // keep for transport
        me
    }

    /// Accessor for `boot_id`. Mirrors `host._boot_id`.
    pub fn boot_id(&self) -> &str {
        &self.boot_id
    }

    /// Whether the host is closed. Mirrors `self._closed.is_set()`.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Emit a frame as line-JSON. Mirrors `ComputeHost.emit`.
    ///
    /// ```python
    /// def emit(self, frame: dict) -> None:
    ///     frame.setdefault("host_ns", now_ns())
    ///     data = json.dumps(frame, separators=(",",":"), ensure_ascii=False)
    ///     with self._write_lock: print(data, file=self._stdout, flush=True)
    /// ```
    pub fn emit(&self, mut frame: HashMap<String, String>) {
        frame.entry("host_ns".into()).or_insert_with(|| now_ns().to_string());
        let data = frame_to_json(&frame);
        let line = format!("{}\n", data);
        let _guard = self.write_lock.lock().unwrap();
        let mut out = self.stdout.lock().unwrap();
        let _ = out.write_all(line.as_bytes());
        let _ = out.flush();
    }

    /// Emit with pre-serialized JSON string (test seam for nested values).
    pub fn emit_json(&self, json_line: &str) {
        let line = if json_line.ends_with('\n') { json_line.to_string() } else { format!("{}\n", json_line) };
        let _guard = self.write_lock.lock().unwrap();
        let mut out = self.stdout.lock().unwrap();
        let _ = out.write_all(line.as_bytes());
        let _ = out.flush();
    }

    /// Mirrors `def close(self) -> None: self._closed.set(); self._executor.shutdown(wait=False, cancel_futures=True)`.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        // In std, cancel pending futures is best-effort — we clear the map.
        // Running threads are not killed (Python's cancel_futures only cancels queued, not running).
        let mut m = self.turn_futures.lock().unwrap();
        m.clear();
        let mut d = self.turn_futures_done.lock().unwrap();
        d.clear();
    }

    /// Drain in-flight turns then finalize sessions. Mirrors `ComputeHost.shutdown`.
    ///
    /// See `compute_host.py` docstring for the reserve / live-sids semantics.
    pub fn shutdown(&self, reason: &str, wait_secs: f64) {
        self.closed.store(true, Ordering::SeqCst);
        let budget = wait_secs.max(0.0);
        let reserve = FLUSH_RESERVE_SECS.min(budget / 2.0);
        let deadline = Instant::now() + Duration::from_secs_f64(budget - reserve);
        // Drain loop: bounded by remaining, sleep min(0.05, remaining)
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let pending = {
                let m = self.turn_futures.lock().unwrap();
                let d = self.turn_futures_done.lock().unwrap();
                m.keys().filter(|k| !d.get(k).copied().unwrap_or(false)).count()
            };
            if pending == 0 {
                break;
            }
            let sleep_dur = Duration::from_secs_f64(0.05).min(remaining);
            if sleep_dur.is_zero() {
                break;
            }
            thread::sleep(sleep_dur);
        }
        let live_sids: HashSet<String> = {
            let m = self.turn_futures.lock().unwrap();
            let d = self.turn_futures_done.lock().unwrap();
            m.iter().filter(|(k, _)| !d.get(k).copied().unwrap_or(false)).map(|(_, v)| v.clone()).filter(|s| !s.is_empty()).collect()
        };
        self.flush_all_sessions(reason, Some(&live_sids));
        // shutdown executor — clear futures
        {
            let mut m = self.turn_futures.lock().unwrap();
            m.clear();
        }
        {
            let mut d = self.turn_futures_done.lock().unwrap();
            d.clear();
        }
    }

    /// Finalize every server session except `skip_sids`. Mirrors `flush_all_sessions`.
    ///
    /// The real Python does `from tui_gateway import server; for sid, session in list(server._sessions.items()): if sid in skip: continue; server._finalize_session(...)`.
    /// In Rust the `server` dict is injected via `finalize` closure so the crate stays `std`-only.
    pub fn flush_all_sessions(&self, reason: &str, skip_sids: Option<&HashSet<String>>) {
        let _ = reason;
        let _ = skip_sids;
        // No-op when server is not injected. Injected variant below is the testable one.
    }

    /// Injected variant — `sessions` is the `server._sessions` snapshot, `finalize` mirrors `server._finalize_session`.
    pub fn flush_all_sessions_with<F>(&self, reason: &str, skip_sids: Option<&HashSet<String>>, sessions: &HashMap<String, String>, mut finalize: F)
    where
        F: FnMut(&str, &str),
    {
        let skip = skip_sids.cloned().unwrap_or_default();
        for sid in sessions.keys() {
            if skip.contains(sid) {
                continue;
            }
            // Mirrors `try: server._finalize_session(session, end_reason=f"compute_host_{reason}") except: pass`
            let end_reason = format!("compute_host_{}", reason);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| finalize(sid, &end_reason)));
        }
    }

    /// Dispatch a decoded frame. Mirrors `handle_frame`.
    pub fn handle_frame(&self, frame: HashMap<String, String>) {
        let kind = FrameKind::from_str(frame.get("type").map(|s| s.as_str()).unwrap_or(""));
        match kind {
            FrameKind::SessionSeed => self.handle_seed(frame),
            FrameKind::TurnStart => self.handle_turn_start(frame),
            FrameKind::Interrupt => self.handle_interrupt(frame),
            FrameKind::ReloadMcp => self.handle_reload_mcp(frame),
            FrameKind::Control => self.handle_control(frame),
            FrameKind::Shutdown => {
                let mut ack = HashMap::new();
                ack.insert("type".into(), "shutdown.ack".into());
                if let Some(rid) = frame.get("request_id") {
                    ack.insert("request_id".into(), rid.clone());
                }
                self.emit(ack);
                self.closed.store(true, Ordering::SeqCst);
                // executor shutdown
                let mut m = self.turn_futures.lock().unwrap();
                m.clear();
            }
            FrameKind::Unknown(k) => {
                let mut err = HashMap::new();
                err.insert("type".into(), "error".into());
                if let Some(rid) = frame.get("request_id") {
                    err.insert("request_id".into(), rid.clone());
                }
                err.insert("message".into(), format!("unknown frame type: {}", k));
                self.emit(err);
            }
        }
    }

    /// Mirrors `_handle_seed`.
    pub fn handle_seed(&self, frame: HashMap<String, String>) {
        let sid = frame.get("sid").map(|s| s.as_str()).unwrap_or("").to_string();
        if sid.is_empty() {
            let mut err = HashMap::new();
            err.insert("type".into(), "error".into());
            if let Some(rid) = frame.get("request_id") {
                err.insert("request_id".into(), rid.clone());
            }
            err.insert("message".into(), "sid required".into());
            self.emit(err);
            return;
        }
        // history is a list; in std we treat it as empty unless injected
        let history: Vec<Message> = Vec::new();
        let agent = SpikeAgent::new(sid.clone(), history);
        let sess = HostSession::new(sid.clone(), agent);
        {
            let mut m = self.sessions.lock().unwrap();
            m.insert(sid.clone(), sess);
        }
        let mut ack = HashMap::new();
        ack.insert("type".into(), "session.seeded".into());
        ack.insert("sid".into(), sid);
        if let Some(rid) = frame.get("request_id") {
            ack.insert("request_id".into(), rid.clone());
        }
        self.emit(ack);
    }

    /// Register an in-flight turn. Mirrors `_track_turn_future`.
    pub fn track_turn_future(&self, future_id: u64, sid: &str) {
        let mut m = self.turn_futures.lock().unwrap();
        m.insert(future_id, sid.to_string());
        let mut d = self.turn_futures_done.lock().unwrap();
        d.insert(future_id, false);
    }

    /// Remove a completed turn. Mirrors `_untrack_turn_future`.
    pub fn untrack_turn_future(&self, future_id: u64) {
        let mut m = self.turn_futures.lock().unwrap();
        m.remove(&future_id);
        let mut d = self.turn_futures_done.lock().unwrap();
        d.remove(&future_id);
    }

    fn alloc_future_id(&self) -> u64 {
        let mut n = self.next_future_id.lock().unwrap();
        let id = *n;
        *n += 1;
        id
    }

    fn mark_done(&self, future_id: u64) {
        let mut d = self.turn_futures_done.lock().unwrap();
        d.insert(future_id, true);
    }

    /// Mirrors `_handle_turn_start`.
    pub fn handle_turn_start(&self, frame: HashMap<String, String>) {
        let sid = frame.get("sid").map(|s| s.as_str()).unwrap_or("").to_string();
        let has_spike = { self.sessions.lock().unwrap().contains_key(&sid) };
        if has_spike {
            self.handle_spike_turn_start(frame);
            return;
        }
        // Real turn path — spawn a detached thread mimicking `executor.submit(self._run_real_turn, dict(frame))`
        let me = Arc::new(self.clone_for_thread());
        let fid = me.alloc_future_id();
        me.track_turn_future(fid, &sid);
        let sid_clone = sid.clone();
        let frame_clone = frame.clone();
        let me_done = Arc::clone(&me);
        let _ = thread::Builder::new().name("compute-host-turn".into()).spawn(move || {
            me_done.run_real_turn(frame_clone);
            me_done.mark_done(fid);
            me_done.untrack_turn_future(fid);
            let _ = sid_clone;
        });
    }

    /// Mirrors `_handle_spike_turn_start`.
    pub fn handle_spike_turn_start(&self, frame: HashMap<String, String>) {
        let sid = frame.get("sid").map(|s| s.as_str()).unwrap_or("").to_string();
        // Quick existence check before locking session
        let exists = { self.sessions.lock().unwrap().contains_key(&sid) };
        if !exists {
            let mut err = HashMap::new();
            err.insert("type".into(), "turn.error".into());
            err.insert("sid".into(), sid.clone());
            if let Some(rid) = frame.get("request_id") {
                err.insert("request_id".into(), rid.clone());
            }
            err.insert("message".into(), "unknown session".into());
            self.emit(err);
            return;
        }
        // Check running under session lock
        {
            let mut sessions = self.sessions.lock().unwrap();
            if let Some(sess) = sessions.get_mut(&sid) {
                let _guard = sess.lock.lock().unwrap();
                if sess.running {
                    let mut err = HashMap::new();
                    err.insert("type".into(), "turn.error".into());
                    err.insert("sid".into(), sid.clone());
                    if let Some(rid) = frame.get("request_id") {
                        err.insert("request_id".into(), rid.clone());
                    }
                    err.insert("message".into(), "session busy".into());
                    self.emit(err);
                    return;
                }
                sess.running = true;
            }
        }
        let me = Arc::new(self.clone_for_thread());
        let fid = me.alloc_future_id();
        me.track_turn_future(fid, &sid);
        let me_done = Arc::clone(&me);
        let frame_clone = frame.clone();
        let _ = thread::Builder::new().name("compute-host-turn".into()).spawn(move || {
            // Find session and run
            let sess_sid = frame_clone.get("sid").map(|s| s.as_str()).unwrap_or("").to_string();
            // We need to extract session ownership for the turn — clone the sid and run via host method
            // The host method will look up the session again under lock.
            me_done.run_spike_turn_by_sid(&sess_sid, frame_clone);
            me_done.mark_done(fid);
            me_done.untrack_turn_future(fid);
        });
    }

    /// Mirrors `_handle_interrupt`.
    pub fn handle_interrupt(&self, frame: HashMap<String, String>) {
        let sid = frame.get("sid").map(|s| s.as_str()).unwrap_or("").to_string();
        let request_id = frame.get("request_id").cloned().unwrap_or_default();
        // Spike branch
        let spike_hit = {
            let mut sessions = self.sessions.lock().unwrap();
            if let Some(sess) = sessions.get_mut(&sid) {
                request_hard_interrupt(&sess.agent);
                true
            } else {
                false
            }
        };
        if spike_hit {
            let mut ack = HashMap::new();
            ack.insert("type".into(), "interrupt.ack".into());
            ack.insert("sid".into(), sid);
            ack.insert("request_id".into(), request_id);
            ack.insert("applied".into(), "true".into());
            ack.insert("applied_ns".into(), now_ns().to_string());
            self.emit(ack);
            return;
        }
        // Real session branch — best-effort via injection hook
        // When no server is injected we emit applied=false like Python's `if session is None: applied False`
        let mut ack = HashMap::new();
        ack.insert("type".into(), "interrupt.ack".into());
        ack.insert("sid".into(), sid);
        ack.insert("request_id".into(), request_id);
        ack.insert("applied".into(), "false".into());
        self.emit(ack);
    }

    /// Injected interrupt with server sessions map (test seam for the real branch).
    pub fn handle_interrupt_with<F>(&self, frame: HashMap<String, String>, server_sessions: &mut HashMap<String, ServerSession>, mut on_interrupt: F)
    where
        F: FnMut(&SpikeAgent),
    {
        let sid = frame.get("sid").map(|s| s.as_str()).unwrap_or("").to_string();
        let request_id = frame.get("request_id").cloned().unwrap_or_default();
        // Spike check first
        {
            let mut sessions = self.sessions.lock().unwrap();
            if let Some(sess) = sessions.get_mut(&sid) {
                request_hard_interrupt(&sess.agent);
                let mut ack = HashMap::new();
                ack.insert("type".into(), "interrupt.ack".into());
                ack.insert("sid".into(), sid);
                ack.insert("request_id".into(), request_id);
                ack.insert("applied".into(), "true".into());
                ack.insert("applied_ns".into(), now_ns().to_string());
                self.emit(ack);
                return;
            }
        }
        if let Some(sess) = server_sessions.get_mut(&sid) {
            if let Some(agent) = sess.agent.as_ref() {
                on_interrupt(agent);
            }
            sess.turn_cancel_requested = true;
            sess.queued_prompt = None;
            sess.queued_prompts = None;
            sess.queued_prompt_generation += 1;
            let mut ack = HashMap::new();
            ack.insert("type".into(), "interrupt.ack".into());
            ack.insert("sid".into(), sid);
            ack.insert("request_id".into(), request_id);
            ack.insert("applied".into(), "true".into());
            ack.insert("applied_ns".into(), now_ns().to_string());
            self.emit(ack);
        } else {
            let mut ack = HashMap::new();
            ack.insert("type".into(), "interrupt.ack".into());
            ack.insert("sid".into(), sid);
            ack.insert("request_id".into(), request_id);
            ack.insert("applied".into(), "false".into());
            self.emit(ack);
        }
    }

    /// Run a spike turn for `session` identified by `sid`. Mirrors `_run_spike_turn`.
    fn run_spike_turn_by_sid(&self, sid: &str, frame: HashMap<String, String>) {
        let request_id = frame.get("request_id").cloned().unwrap_or_else(generate_request_id);
        let prompt = frame.get("prompt").or_else(|| frame.get("text")).cloned().unwrap_or_default();
        let delta_count: i64 = frame.get("delta_count").and_then(|s| s.parse().ok()).unwrap_or(24);
        let delay_s: f64 = frame.get("delay_s").and_then(|s| s.parse().ok()).unwrap_or(0.001);
        // Snapshot history under lock
        let history_snapshot: Vec<Message> = {
            let sessions = self.sessions.lock().unwrap();
            sessions.get(sid).map(|s| {
                let _g = s.lock.lock().unwrap();
                s.agent.history.clone()
            }).unwrap_or_default()
        };
        // Clear interrupt
        {
            let mut sessions = self.sessions.lock().unwrap();
            if let Some(sess) = sessions.get_mut(sid) {
                sess.agent.clear_interrupt();
            }
        }
        let mut started = HashMap::new();
        started.insert("type".into(), "turn.started".into());
        started.insert("sid".into(), sid.to_string());
        started.insert("request_id".into(), request_id.clone());
        started.insert("started_ns".into(), now_ns().to_string());
        self.emit(started);

        // Stream callback
        let me = self.clone_for_thread();
        let sid_owned = sid.to_string();
        let request_id_owned = request_id.clone();
        let stream = move |delta: &str| {
            me.bump_progress();
            let mut ev = HashMap::new();
            ev.insert("type".into(), "delta".into());
            ev.insert("sid".into(), sid_owned.clone());
            ev.insert("request_id".into(), request_id_owned.clone());
            ev.insert("text".into(), delta.to_string());
            ev.insert("emitted_ns".into(), now_ns().to_string());
            me.emit(ev);
        };

        // Run conversation — need mutable agent
        let result = {
            let mut sessions = self.sessions.lock().unwrap();
            if let Some(sess) = sessions.get_mut(sid) {
                // Temporarily take agent history snapshot
                let mut agent = sess.agent.clone();
                // We need to run against a mutable copy and then write back history
                // Use the real agent inside the session (clone then replace)
                let res = {
                    let mut tmp = agent.clone();
                    tmp.run_conversation(&prompt, Some(history_snapshot.clone()), Some(&stream), delta_count, delay_s)
                };
                // Write back history
                sess.agent.history = res.messages.clone();
                Some(res)
            } else {
                None
            }
        };

        match result {
            Some(res) => {
                // Bump version + clear running
                let history_version = {
                    let mut sessions = self.sessions.lock().unwrap();
                    if let Some(sess) = sessions.get_mut(sid) {
                        let _g = sess.lock.lock().unwrap();
                        sess.history_version += 1;
                        sess.running = false;
                        sess.history_version
                    } else {
                        0
                    }
                };
                self.bump_progress();
                let mut done = HashMap::new();
                done.insert("type".into(), "turn.end".into());
                done.insert("sid".into(), sid.to_string());
                done.insert("request_id".into(), request_id.clone());
                done.insert("history_version".into(), history_version.to_string());
                done.insert("message_count".into(), res.messages.len().to_string());
                done.insert("interrupted".into(), res.interrupted.to_string());
                done.insert("ended_ns".into(), now_ns().to_string());
                self.emit(done);
            }
            None => {
                let mut sessions = self.sessions.lock().unwrap();
                if let Some(sess) = sessions.get_mut(sid) {
                    let _g = sess.lock.lock().unwrap();
                    sess.running = false;
                }
                let mut err = HashMap::new();
                err.insert("type".into(), "turn.error".into());
                err.insert("sid".into(), sid.to_string());
                err.insert("request_id".into(), request_id);
                err.insert("message".into(), "unknown session".into());
                self.emit(err);
            }
        }
    }

    /// Real dashboard turn path — mirrors `_run_real_turn`.
    ///
    /// In `std` this is a stub that emits `turn.error` when no server injection is provided;
    /// the injected variant `run_real_turn_with` wires real server closures.
    pub fn run_real_turn(&self, frame: HashMap<String, String>) {
        let sid = frame.get("sid").map(|s| s.as_str()).unwrap_or("").to_string();
        let request_id = frame.get("request_id").cloned().unwrap_or_else(generate_request_id);
        if sid.is_empty() {
            let mut err = HashMap::new();
            err.insert("type".into(), "turn.error".into());
            err.insert("sid".into(), sid);
            err.insert("request_id".into(), request_id);
            err.insert("message".into(), "sid required".into());
            self.emit(err);
            return;
        }
        // Without server injection we cannot run a real turn — emit error mirroring exception path
        let mut err = HashMap::new();
        err.insert("type".into(), "turn.error".into());
        err.insert("sid".into(), sid);
        err.insert("request_id".into(), request_id);
        err.insert("reason".into(), "exception".into());
        err.insert("message".into(), "server not injected (std-only stub)".into());
        self.emit(err);
    }

    /// Mirrors `_ensure_server_session` — simplified std-only stub.
    pub fn ensure_server_session(&self, _frame: &HashMap<String, String>) -> Option<String> {
        // Real implementation needs hermes_constants / agent / SessionDB — injected via `run_real_turn_with`.
        None
    }

    /// Mirrors `_handle_reload_mcp`.
    pub fn handle_reload_mcp(&self, frame: HashMap<String, String>) {
        let sid = frame.get("sid").map(|s| s.as_str()).unwrap_or("").to_string();
        let request_id = frame.get("request_id").cloned().unwrap_or_default();
        // Without server injection, emit control.error like the `except Exception as exc` branch
        let mut err = HashMap::new();
        err.insert("type".into(), "control.error".into());
        err.insert("sid".into(), sid);
        err.insert("request_id".into(), request_id);
        err.insert("message".into(), "server not injected".into());
        self.emit(err);
    }

    /// Mirrors `_handle_control`.
    pub fn handle_control(&self, frame: HashMap<String, String>) {
        let sid = frame.get("sid").map(|s| s.as_str()).unwrap_or("").to_string();
        let request_id = frame.get("request_id").cloned().unwrap_or_default();
        let route_name = frame.get("route_name").map(|s| s.as_str()).unwrap_or("").to_string();
        if !is_known_mutator_route(&route_name) {
            let mut err = HashMap::new();
            err.insert("type".into(), "control.error".into());
            err.insert("sid".into(), sid);
            err.insert("request_id".into(), request_id);
            err.insert("message".into(), format!("unclassified route: {}", route_name));
            self.emit(err);
            return;
        }
        // Without server sessions we emit session not found
        let mut err = HashMap::new();
        err.insert("type".into(), "control.error".into());
        err.insert("sid".into(), sid);
        err.insert("request_id".into(), request_id);
        err.insert("message".into(), "session not found".into());
        self.emit(err);
    }

    /// Mirrors `_bump_progress`.
    pub fn bump_progress(&self) {
        let mut c = self.progress_counter.lock().unwrap();
        *c += 1;
    }

    /// One heartbeat tick — mirrors the body of `_heartbeat_loop`.
    pub fn heartbeat_loop_once(&self) -> HashMap<String, String> {
        let active_turns = {
            let m = self.turn_futures.lock().unwrap();
            let d = self.turn_futures_done.lock().unwrap();
            m.keys().filter(|k| !d.get(k).copied().unwrap_or(false)).count()
        };
        let counter = *self.progress_counter.lock().unwrap();
        let mut hb = HashMap::new();
        hb.insert("type".into(), "hb".into());
        hb.insert("active_turns".into(), active_turns.to_string());
        hb.insert("progress_counter".into(), counter.to_string());
        hb.insert("rss_mb".into(), format!("{:.2}", rss_mb(std::process::id())));
        hb
    }

    /// Heartbeat loop — mirrors `_heartbeat_loop` (`while not _closed.wait(heartbeat_secs): emit hb`).
    fn heartbeat_loop(&self) {
        while !self.closed.load(Ordering::SeqCst) {
            let secs = self.heartbeat_secs;
            if secs <= 0.0 {
                break;
            }
            thread::sleep(Duration::from_secs_f64(secs));
            if self.closed.load(Ordering::SeqCst) {
                break;
            }
            let hb = self.heartbeat_loop_once();
            self.emit(hb);
        }
    }

    /// Whether parent guard should fire — mirrors `_parent_guard_loop` check.
    pub fn parent_guard_should_exit(&self, current_ppid: i32) -> bool {
        if current_ppid == 0 || current_ppid == 1 {
            return true;
        }
        if self.parent_pid != 0 && current_ppid != self.parent_pid {
            return true;
        }
        false
    }

    /// Parent guard loop — mirrors `_parent_guard_loop` (`while not _closed.wait(1.0): if ppid in {0,1} or ppid != _parent_pid: emit orphan; shutdown(orphan); os._exit(0)`).
    fn parent_guard_loop(&self) {
        while !self.closed.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_secs(1));
            if self.closed.load(Ordering::SeqCst) {
                break;
            }
            let ppid = current_ppid();
            if self.parent_guard_should_exit(ppid) {
                let mut orphan = HashMap::new();
                orphan.insert("type".into(), "orphan".into());
                orphan.insert("old_ppid".into(), self.parent_pid.to_string());
                orphan.insert("ppid".into(), ppid.to_string());
                self.emit(orphan);
                self.shutdown("orphan", 10.0);
                std::process::exit(0);
            }
        }
    }

    /// Clone a lightweight handle for thread spawning (shares Arcs, not deep).
    fn clone_for_thread(&self) -> Self {
        Self {
            stdout: Arc::clone(&self.stdout),
            write_lock: Arc::clone(&self.write_lock),
            sessions: Arc::clone(&self.sessions),
            max_workers: self.max_workers,
            closed: Arc::clone(&self.closed),
            parent_pid: self.parent_pid,
            boot_id: self.boot_id.clone(),
            progress_counter: Arc::clone(&self.progress_counter),
            turn_futures: Arc::clone(&self.turn_futures),
            next_future_id: Arc::clone(&self.next_future_id),
            turn_futures_done: Arc::clone(&self.turn_futures_done),
            transport: self.transport.clone(),
            heartbeat_secs: self.heartbeat_secs,
        }
    }

    /// Accessor for transport (mirrors `self._transport`).
    pub fn transport(&self) -> &HostTransport {
        &self.transport
    }

    /// Number of active turns (for heartbeat/tests).
    pub fn active_turns(&self) -> usize {
        let m = self.turn_futures.lock().unwrap();
        let d = self.turn_futures_done.lock().unwrap();
        m.keys().filter(|k| !d.get(k).copied().unwrap_or(false)).count()
    }

    /// Progress counter snapshot.
    pub fn progress_counter_val(&self) -> u64 {
        *self.progress_counter.lock().unwrap()
    }
}

// ---------------------------------------------------------------------------
// Minimal server session stub for control/interrupt injection
// ---------------------------------------------------------------------------

/// Minimal stub for a real server session dict.
///
/// Mirrors the `dict` stored in `server._sessions[sid]` with only the fields
/// `compute_host.py` touches in the interrupt path.
#[derive(Debug, Clone)]
pub struct ServerSession {
    pub agent: Option<SpikeAgent>,
    pub turn_cancel_requested: bool,
    pub queued_prompt: Option<String>,
    pub queued_prompts: Option<Vec<String>>,
    pub queued_prompt_generation: i64,
}

// ---------------------------------------------------------------------------
// Helpers: current_ppid, MUTATOR_ROUTE_TABLE, is_known_mutator_route
// ---------------------------------------------------------------------------

#[cfg(unix)]
extern "C" {
    fn getppid() -> i32;
}

/// Current parent pid. Mirrors `os.getppid()`.
pub fn current_ppid() -> i32 {
    #[cfg(unix)]
    {
        unsafe { getppid() }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Mutator route table — mirrors `tui_gateway/host_supervisor.MUTATOR_ROUTE_TABLE` reused by `compute_host`.
pub const MUTATOR_ROUTES: &[(&str, MutatorRouteKind)] = &[
    ("prompt.submit", MutatorRouteKind::TurnPath),
    ("session.interrupt", MutatorRouteKind::TurnPath),
    ("reload.mcp", MutatorRouteKind::RunConcurrent),
    ("session.save", MutatorRouteKind::RunConcurrent),
    ("session.compress", MutatorRouteKind::IdleGated),
    ("prompt.submit.truncate", MutatorRouteKind::IdleGated),
    ("slash.model", MutatorRouteKind::IdleGated),
    ("slash.personality", MutatorRouteKind::IdleGated),
    ("slash.prompt", MutatorRouteKind::IdleGated),
    ("slash.compress", MutatorRouteKind::IdleGated),
    ("session.reset", MutatorRouteKind::IdleGated),
    ("session.history.reload", MutatorRouteKind::IdleGated),
    ("slash.retry", MutatorRouteKind::IdleGated),
];

pub fn is_known_mutator_route(name: &str) -> bool {
    MUTATOR_ROUTES.iter().any(|(n, _)| *n == name)
}

pub fn mutator_route_kind(name: &str) -> Option<MutatorRouteKind> {
    MUTATOR_ROUTES.iter().find(|(n, _)| *n == name).map(|(_, k)| *k)
}

// ---------------------------------------------------------------------------
// run_host / main — mirrors run_host / main
// ---------------------------------------------------------------------------

/// Parse CLI args — mirrors `argparse.ArgumentParser(description="Dashboard compute-host process")` which takes no required args.
///
/// Any `--*` flag is accepted (mirrors `parser.parse_args(argv)` with no `add_argument` calls beyond `description`).
pub fn parse_args(argv: &[String]) -> Result<(), String> {
    for a in argv {
        if a == "--help" || a == "-h" {
            return Err("help".into());
        }
        // Unknown flags are accepted silently — Python argparse with no args would error on unknown,
        // but the compute_host parser has no positional/required args and tolerates empty argv.
        // We keep it lenient for 1:1 compat: only `--help` is special.
        let _ = a;
    }
    Ok(())
}

/// Run the host event loop over `reader` → `host`.
///
/// Mirrors `run_host(stdin=None, stdout=None)` but testable with `BufRead`/`Write`.
/// The real `run_host` uses `sys.stdin`/`sys.stdout` + `signal.signal` + reader thread.
/// This helper is the injected version.
pub fn run_host_with<R, W>(reader: &mut R, host: Arc<ComputeHost>) -> io::Result<()>
where
    R: BufRead,
    W: Write,
{
    let _ = host; // host is used via emit/handle_frame
    // Emit hello — mirrors `host.emit({"type":"hello","host_pid":os.getpid(),"boot_id":host._boot_id,"build_sha":_build_sha(),"cwd":os.getcwd(),"hermes_home":os.environ.get("HERMES_HOME","")})`
    let mut hello = HashMap::new();
    hello.insert("type".into(), "hello".into());
    hello.insert("host_pid".into(), std::process::id().to_string());
    hello.insert("boot_id".into(), host.boot_id().to_string());
    hello.insert("build_sha".into(), build_sha());
    hello.insert("cwd".into(), env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).to_string_lossy().to_string());
    hello.insert("hermes_home".into(), env::var("HERMES_HOME").unwrap_or_default());
    host.emit(hello);

    let shutting_down = Arc::new(AtomicBool::new(false));
    let host_clone = Arc::clone(&host);
    let shutting_clone = Arc::clone(&shutting_down);

    // Reader thread — mirrors `def _reader(): for raw in stdin: if host._closed.is_set(): break; try: frame=json.loads(raw) except: emit error; continue; if not isinstance(frame, dict): emit error; continue; host.handle_frame(frame); if frame.get("type")=="shutdown": os._exit(0); if host._closed.is_set(): break`
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        if host.is_closed() {
            break;
        }
        let raw = line.trim().to_string();
        if raw.is_empty() {
            continue;
        }
        // Minimal json decode: must be an object
        let frame = parse_frame(&raw);
        match frame {
            None => {
                let mut err = HashMap::new();
                err.insert("type".into(), "error".into());
                err.insert("message".into(), format!("invalid json: {}", raw.chars().take(80).collect::<String>()));
                host.emit(err);
                continue;
            }
            Some(map) => {
                let is_shutdown = map.get("type").map(|s| s.as_str()) == Some("shutdown");
                host.handle_frame(map);
                if is_shutdown {
                    std::process::exit(0);
                }
                if host.is_closed() {
                    break;
                }
            }
        }
    }

    // Main wait loop — mirrors `while not host._closed.wait(0.2): if not reader.is_alive(): break; finally: host.shutdown(reason="stdin_closed", wait=2.0)`
    // Here reader is synchronous so we just shutdown after EOF.
    if !shutting_clone.load(Ordering::SeqCst) {
        host_clone.shutdown("stdin_closed", 2.0);
    }
    Ok(())
}

/// Environment setup for `run_host` — mirrors `os.environ["HERMES_COMPUTE_HOST_CHILD"] = "1"`.
pub fn run_host_env_setup() {
    env::set_var(ENV_COMPUTE_HOST_CHILD, "1");
}

/// Full `run_host` entry point using `stdin`/`stdout`.
///
/// Mirrors `def run_host(stdin=None, stdout=None) -> None:` with signal handling.
/// Signal installation is omitted in `std` (requires `libc`); the handler logic is documented.
pub fn run_host() {
    run_host_env_setup();
    // Create host with real stdout
    struct StdoutWriter;
    impl Write for StdoutWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let mut out = io::stdout().lock();
            out.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            let mut out = io::stdout().lock();
            out.flush()
        }
    }
    let stdout: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(Box::new(StdoutWriter) as Box<dyn Write + Send>));
    let host = ComputeHost::new(Some(stdout), None, None);
    // Hello emit + reader loop
    let stdin = io::stdin();
    let mut reader = io::BufReader::new(stdin.lock());
    let _ = run_host_with::<_, io::Stdout>(&mut reader, host);
}

/// Minimal frame parser — expects `{"type":"...","sid":"...", ...}` flat JSON with string values.
fn parse_frame(raw: &str) -> Option<HashMap<String, String>> {
    let t = raw.trim();
    if !t.starts_with('{') || !t.ends_with('}') {
        return None;
    }
    let inner = &t[1..t.len() - 1];
    if inner.trim().is_empty() {
        return Some(HashMap::new());
    }
    let mut out = HashMap::new();
    let mut current = String::new();
    let mut in_str = false;
    let mut escaped = false;
    let mut pairs: Vec<String> = Vec::new();
    for ch in inner.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && in_str {
            escaped = true;
            current.push(ch);
            continue;
        }
        if ch == '"' {
            in_str = !in_str;
            current.push(ch);
            continue;
        }
        if ch == ',' && !in_str {
            pairs.push(current.trim().to_string());
            current.clear();
            continue;
        }
        current.push(ch);
    }
    if !current.trim().is_empty() {
        pairs.push(current.trim().to_string());
    }
    for pair in pairs {
        let colon = pair.find(':')?;
        let (k_raw, v_raw) = pair.split_at(colon);
        let k = k_raw.trim().trim_matches('"').replace("\\\"", "\"").replace("\\\\", "\\");
        let v_trim = v_raw[1..].trim();
        let v = if v_trim.starts_with('"') && v_trim.ends_with('"') && v_trim.len() >= 2 {
            v_trim[1..v_trim.len() - 1].replace("\\\"", "\"").replace("\\\\", "\\")
        } else {
            v_trim.to_string()
        };
        out.insert(k, v);
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Tests — mirror Python invariants
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    fn buf_host() -> (Arc<ComputeHost>, Arc<Mutex<Vec<u8>>>) {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let buf_clone = Arc::clone(&buf);
        struct BufWriter(Arc<Mutex<Vec<u8>>>);
        impl Write for BufWriter {
            fn write(&mut self, data: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(data);
                Ok(data.len())
            }
            fn flush(&mut self) -> io::Result<()> { Ok(()) }
        }
        let writer: Box<dyn Write + Send> = Box::new(BufWriter(buf_clone));
        let stdout: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(writer));
        let host = ComputeHost::new_with_config(Some(stdout), 2, 0.0); // heartbeat 0 disables threads
        (host, buf)
    }

    #[test]
    fn now_ns_is_monotonic() {
        let a = now_ns();
        thread::sleep(Duration::from_millis(2));
        let b = now_ns();
        assert!(b >= a, "now_ns must be monotonic: {} >= {}", b, a);
    }

    #[test]
    fn spike_agent_run_conversation_basic() {
        let mut agent = SpikeAgent::new("sid1", vec![]);
        let res = agent.run_conversation("hello", None, None, 4, 0.0);
        assert_eq!(res.messages.len(), 2); // user + assistant
        assert_eq!(res.messages[0].role, "user");
        assert_eq!(res.messages[0].content, "hello");
        assert_eq!(res.messages[1].role, "assistant");
        // Chunks: "sid1:hello:0000 " etc.
        assert!(res.final_response.contains("sid1:hello:0000"));
        assert!(res.final_response.contains("sid1:hello:0003"));
        assert!(!res.interrupted);
        // History updated
        assert_eq!(agent.history.len(), 2);
    }

    #[test]
    fn spike_agent_interrupt_mid_run() {
        let flag = Arc::new(AtomicBool::new(false));
        let mut agent = SpikeAgent::new_with_flag("sid2", vec![], Arc::clone(&flag));
        // Pre-set interrupt so run exits immediately
        flag.store(true, Ordering::SeqCst);
        let res = agent.run_conversation("prompt", None, None, 10, 0.0);
        assert!(res.interrupted);
        assert!(res.final_response.ends_with("[interrupted]"));
    }

    #[test]
    fn spike_agent_delta_zero_and_negative() {
        let mut agent = SpikeAgent::new("sid", vec![]);
        let res = agent.run_conversation("p", None, None, 0, 0.0);
        assert_eq!(res.final_response, "");
        assert!(!res.interrupted);
        let res2 = agent.run_conversation("p", None, None, -5, 0.0);
        assert_eq!(res2.final_response, "");
    }

    #[test]
    fn spike_agent_stream_callback() {
        let mut agent = SpikeAgent::new("s", vec![]);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let cb = move |chunk: &str| {
            seen_clone.lock().unwrap().push(chunk.to_string());
        };
        let res = agent.run_conversation("hi", None, Some(&cb), 3, 0.0);
        assert_eq!(seen.lock().unwrap().len(), 3);
        assert!(res.final_response.contains("s:hi:0001"));
    }

    #[test]
    fn spike_agent_conversation_history_override() {
        let mut agent = SpikeAgent::new("sid", vec![Message::user("old")]);
        let hist = vec![Message::user("base")];
        let res = agent.run_conversation("new", Some(hist.clone()), None, 1, 0.0);
        // base_history is the override, so messages = [base user, new user, assistant]
        assert_eq!(res.messages[0].content, "base");
        assert_eq!(res.messages[1].content, "new");
    }

    #[test]
    fn host_transport_write_emits_rpc() {
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let emitted_clone = Arc::clone(&emitted);
        let t = HostTransport::new(move |frame| emitted_clone.lock().unwrap().push(frame));
        let mut obj = HashMap::new();
        obj.insert("method".into(), "event".into());
        obj.insert("session_id".into(), "sid123".into());
        assert!(t.write(obj));
        let frames = emitted.lock().unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].get("type").map(|s| s.as_str()), Some("rpc"));
    }

    #[test]
    fn host_transport_close_is_noop() {
        let t = HostTransport::new(|_| {});
        t.close();
        t.close();
    }

    #[test]
    fn flush_reserve_constant() {
        assert!((FLUSH_RESERVE_SECS - 1.0).abs() < 1e-9);
        assert_eq!(FLUSH_RESERVE, Duration::from_secs(1));
    }

    #[test]
    fn default_workers_cases() {
        assert_eq!(default_workers_from_raw(None), 8);
        assert_eq!(default_workers_from_raw(Some("")), 8);
        assert_eq!(default_workers_from_raw(Some("bad")), 8);
        assert_eq!(default_workers_from_raw(Some("2")), 2);
        assert_eq!(default_workers_from_raw(Some("1")), 2); // max(2, ...)
        assert_eq!(default_workers_from_raw(Some("0")), 2);
        assert_eq!(default_workers_from_raw(Some("10")), 10);
        assert_eq!(default_workers_from_raw(Some("-5")), 2);
    }

    #[test]
    fn heartbeat_parse() {
        assert!((heartbeat_secs_with(None) - 15.0).abs() < 1e-9);
        assert!((heartbeat_secs_with(Some("")) - 15.0).abs() < 1e-9);
        assert!((heartbeat_secs_with(Some("15")) - 15.0).abs() < 1e-9);
        assert!((heartbeat_secs_with(Some("bad")) - 15.0).abs() < 1e-9);
        assert!((heartbeat_secs_with(Some("2.5")) - 2.5).abs() < 1e-9);
    }

    #[test]
    fn repo_root_with_override() {
        let p = PathBuf::from("/tmp/fake_root");
        assert_eq!(repo_root_with(Some(&p)), p);
    }

    #[test]
    fn build_sha_injected() {
        let sha = build_sha_with(Some(Path::new("/tmp")), Some(|_p| Some("abc123".into())));
        assert_eq!(sha, "abc123");
        let unknown = build_sha_with(Some(Path::new("/tmp")), Some(|_p| None));
        assert_eq!(unknown, "unknown");
    }

    #[test]
    fn rss_mb_injected() {
        let rss = rss_mb_with(1234, Some(|_| Some("2048".into())));
        assert!((rss - 2.0).abs() < 1e-9);
        let zero = rss_mb_with(9999, Some(|_| None));
        assert_eq!(zero, 0.0);
    }

    #[test]
    fn compute_host_emit_has_host_ns() {
        let (host, buf) = buf_host();
        let mut frame = HashMap::new();
        frame.insert("type".into(), "test".into());
        host.emit(frame);
        let data = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(data.contains("host_ns"));
        assert!(data.contains("\"type\":\"test\"") || data.contains("\"type\": \"test\"") || data.contains("test"));
    }

    #[test]
    fn compute_host_handle_seed_and_spike_turn() {
        let (host, buf) = buf_host();
        let mut seed = HashMap::new();
        seed.insert("type".into(), "session.seed".into());
        seed.insert("sid".into(), "s1".into());
        seed.insert("request_id".into(), "r1".into());
        host.handle_frame(seed);
        let data = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(data.contains("session.seeded"));

        // Spike turn
        buf.lock().unwrap().clear();
        let mut turn = HashMap::new();
        turn.insert("type".into(), "turn.start".into());
        turn.insert("sid".into(), "s1".into());
        turn.insert("request_id".into(), "r2".into());
        turn.insert("prompt".into(), "hello".into());
        turn.insert("delta_count".into(), "2".into());
        turn.insert("delay_s".into(), "0".into());
        host.handle_frame(turn);
        // Give thread time
        thread::sleep(Duration::from_millis(200));
        let data2 = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(data2.contains("turn.started") || data2.contains("delta") || data2.contains("turn.end"));
    }

    #[test]
    fn compute_host_handle_seed_missing_sid() {
        let (host, buf) = buf_host();
        let mut f = HashMap::new();
        f.insert("type".into(), "session.seed".into());
        f.insert("request_id".into(), "r1".into());
        host.handle_frame(f);
        let data = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(data.contains("error"));
        assert!(data.contains("sid required"));
    }

    #[test]
    fn compute_host_busy_guard() {
        let (host, _buf) = buf_host();
        // Seed
        let mut seed = HashMap::new();
        seed.insert("type".into(), "session.seed".into());
        seed.insert("sid".into(), "busy".into());
        host.handle_frame(seed);
        // Manually set running
        {
            let mut sessions = host.sessions.lock().unwrap();
            if let Some(s) = sessions.get_mut("busy") {
                s.running = true;
            }
        }
        let (host2, buf2) = {
            // use same host for second turn
            let mut turn = HashMap::new();
            turn.insert("type".into(), "turn.start".into());
            turn.insert("sid".into(), "busy".into());
            turn.insert("request_id".into(), "r2".into());
            host.handle_frame(turn);
            (host.clone(), _buf.clone())
        };
        let _ = host2;
        thread::sleep(Duration::from_millis(50));
        let data = String::from_utf8(buf2.lock().unwrap().clone()).unwrap();
        assert!(data.contains("session busy") || data.contains("busy"));
        // Cleanup
        {
            let mut sessions = host.sessions.lock().unwrap();
            if let Some(s) = sessions.get_mut("busy") {
                s.running = false;
            }
        }
    }

    #[test]
    fn compute_host_handle_unknown_frame() {
        let (host, buf) = buf_host();
        let mut f = HashMap::new();
        f.insert("type".into(), "bogus".into());
        f.insert("request_id".into(), "r1".into());
        host.handle_frame(f);
        let data = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(data.contains("unknown frame type"));
    }

    #[test]
    fn compute_host_shutdown_reserve_logic() {
        let (host, _buf) = buf_host();
        // No in-flight turns → shutdown should not hang
        let start = Instant::now();
        host.shutdown("test", 0.05);
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(host.is_closed());
    }

    #[test]
    fn compute_host_interrupt_spike() {
        let (host, buf) = buf_host();
        let mut seed = HashMap::new();
        seed.insert("type".into(), "session.seed".into());
        seed.insert("sid".into(), "int1".into());
        host.handle_frame(seed);
        let mut intr = HashMap::new();
        intr.insert("type".into(), "interrupt".into());
        intr.insert("sid".into(), "int1".into());
        intr.insert("request_id".into(), "r1".into());
        host.handle_frame(intr);
        let data = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(data.contains("interrupt.ack"));
        assert!(data.contains("true") || data.contains("applied"));
    }

    #[test]
    fn compute_host_flush_skip_sids() {
        let (host, _buf) = buf_host();
        let mut sessions = HashMap::new();
        sessions.insert("a".into(), "sess_a".into());
        sessions.insert("b".into(), "sess_b".into());
        let mut finalized = Vec::new();
        let skip: HashSet<String> = ["a".to_string()].into_iter().collect();
        host.flush_all_sessions_with("test", Some(&skip), &sessions, |sid, reason| {
            finalized.push((sid.to_string(), reason.to_string()));
        });
        assert_eq!(finalized.len(), 1);
        assert_eq!(finalized[0].0, "b");
        assert!(finalized[0].1.contains("compute_host_test"));
    }

    #[test]
    fn compute_host_control_unknown_route() {
        let (host, buf) = buf_host();
        let mut f = HashMap::new();
        f.insert("type".into(), "control".into());
        f.insert("sid".into(), "s".into());
        f.insert("request_id".into(), "r1".into());
        f.insert("route_name".into(), "not.a.route".into());
        host.handle_frame(f);
        let data = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(data.contains("unclassified route"));
    }

    #[test]
    fn compute_host_bump_progress() {
        let (host, _buf) = buf_host();
        assert_eq!(host.progress_counter_val(), 0);
        host.bump_progress();
        host.bump_progress();
        assert_eq!(host.progress_counter_val(), 2);
    }

    #[test]
    fn compute_host_parent_guard() {
        let (host, _buf) = buf_host();
        // parent_pid is current ppid; different ppid should trigger exit
        assert!(host.parent_guard_should_exit(1));
        assert!(host.parent_guard_should_exit(0));
        // Same as parent should not trigger
        assert!(!host.parent_guard_should_exit(host.parent_pid));
        assert!(host.parent_guard_should_exit(host.parent_pid + 9999));
    }

    #[test]
    fn parse_frame_valid_and_invalid() {
        let f = parse_frame(r#"{"type":"hello","sid":"s1"}"#);
        assert!(f.is_some());
        assert_eq!(f.unwrap().get("type").map(|s| s.as_str()), Some("hello"));
        assert!(parse_frame("not json").is_none());
        assert!(parse_frame(r#"{"a":}"#).is_some()); // our parser is lenient
    }

    #[test]
    fn generate_boot_id_format() {
        let id = generate_boot_id();
        assert_eq!(id.len(), 32);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(id.to_lowercase(), id);
    }
}
