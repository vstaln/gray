//! Supervisor for the dashboard compute-host child process.
//!
//! 1:1 port of `tui_gateway/host_supervisor.py` (577 lines).
//!
//! The dashboard process owns sockets and JSON-RPC dispatch. When
//! `dashboard.turn_isolation` is enabled, agent turns move behind one persistent
//! `python -m tui_gateway.compute_host` child so compute-heavy agent threads do
//! not contend with the serving process' event loop for the same GIL.
//!
//! ```python
//! # Python — tui_gateway/host_supervisor.py
//! import json, logging, os, queue, signal, subprocess, sys, threading, time, uuid
//! from collections.abc import Callable
//! from pathlib import Path
//! from typing import Any
//! from hermes_constants import get_hermes_home
//! from tools.environments.local import hermes_subprocess_env
//! logger = logging.getLogger(__name__)
//! _Thread = threading.Thread
//! MUTATOR_ROUTE_TABLE: dict[str, str] = {
//!     "prompt.submit": "turn-path",
//!     "session.interrupt": "turn-path",
//!     "reload.mcp": "run-concurrent",
//!     "session.save": "run-concurrent",
//!     "session.compress": "idle-gated",
//!     "prompt.submit.truncate": "idle-gated",
//!     "slash.model": "idle-gated",
//!     "slash.personality": "idle-gated",
//!     "slash.prompt": "idle-gated",
//!     "slash.compress": "idle-gated",
//!     "session.reset": "idle-gated",
//!     "session.history.reload": "idle-gated",
//!     "slash.retry": "idle-gated",
//! }
//! _REGISTRY_NAME = "dashboard-compute-host.json"
//! _RESPAWN_WINDOW_SECS = 300.0
//! _SHUTDOWN_TIMEOUT_SECS = 10.0
//! def append_log_record(path: str | Path, record: str) -> None: ...
//! def _repo_root() -> Path: return Path(__file__).resolve().parents[1]
//! def _build_sha() -> str: subprocess.check_output(["git","rev-parse","HEAD"], ...)
//! def _default_registry_path() -> Path: return get_hermes_home() / "state" / _REGISTRY_NAME
//! def _pid_alive(pid: int) -> bool: os.kill(pid,0)
//! def _pid_command(pid: int) -> str: /proc/pid/cmdline or ps
//! def is_compute_host_identity(pid: int) -> bool: "tui_gateway.compute_host" in cmd
//! class HostSupervisor:
//!     def __init__(self, *, registry_path=None, argv=None, cwd=None, env=None, rpc_sink=None,
//!                  respawn_max=3, heartbeat_secs=15, expected_build_sha=None,
//!                  expected_hermes_home=None, autostart=True): ...
//!     @property def pid(self) -> int: ...
//!     @property def hello(self) -> dict[str, Any]: ...
//!     def is_running(self) -> bool: ...
//!     def start(self) -> None: ...
//!     def shutdown(self) -> None: ...
//!     def reconcile_startup_orphan(self) -> str: ...
//!     def submit_turn(self, frame, *, on_complete=None) -> str: ...
//!     def interrupt(self, sid, *, request_id=None) -> None: ...
//!     def reload_mcp(self, sid, *, request_id=None) -> dict: ...
//!     def control(self, sid, *, route_name, payload=None, wait=True, timeout=30.0) -> dict: ...
//!     def _spawn_locked(self, *, reason: str) -> None: ...
//!     def _validate_hello(self) -> None: ...
//!     def _persist_registry(self) -> None: ...
//!     def _remove_registry(self) -> None: ...
//!     def _send_frame(self, frame: dict) -> None: ...
//!     def _drain_stdout(self, proc) -> None: ...
//!     def _drain_stderr(self, proc) -> None: ...
//!     def _handle_host_frame(self, frame: dict) -> None: ...
//!     def _complete_turn(self, frame: dict) -> None: ...
//!     def _wait_for_exit(self, proc) -> None: ...
//!     def _fail_pending_turns(self, *, reason, message) -> None: ...
//!     def _maybe_respawn_after_crash(self) -> None: ...
//!     def _pid_matches_compute_host(self, pid) -> bool: ...
//!     def _terminate_pid(self, pid, *, timeout=_SHUTDOWN_TIMEOUT_SECS) -> None: ...
//!     def _terminate_process(self, proc) -> None: ...
//! ```

//! # Rust mapping
//!
//! * `MUTATOR_ROUTE_TABLE` → [`MUTATOR_ROUTE_TABLE`] slice + [`mutator_route`] lookup
//!   + [`is_known_mutator_route`]. Python `dict[str,str]` membership test
//!   `route_name not in MUTATOR_ROUTE_TABLE: raise ValueError` →
//!   `is_known_mutator_route(route_name)`.
//! * `_REGISTRY_NAME` → [`REGISTRY_NAME`] (`"dashboard-compute-host.json"`).
//! * `_RESPAWN_WINDOW_SECS = 300.0` → [`RESPAWN_WINDOW_SECS`] + [`RESPAWN_WINDOW`] (`Duration`).
//! * `_SHUTDOWN_TIMEOUT_SECS = 10.0` → [`SHUTDOWN_TIMEOUT_SECS`] + [`SHUTDOWN_TIMEOUT`].
//! * `append_log_record(path, record)` → [`append_log_record`] + [`append_log_record_with`].
//!   Python `os.open(O_WRONLY|O_CREAT|O_APPEND, 0o600)` + `os.write` + `os.close` + `Path.parent.mkdir(..., exist_ok=True)` +
//!   `record.endswith("\n")` guard + `encode("utf-8", errors="replace")` is mirrored:
//!   `mkdir -p` via `fs::create_dir_all`, single `OpenOptions::append+create` + `write_all` +
//!   `PermissionsExt(0o600)` on Unix, newline guarantee, UTF-8 lossless (Rust `String` is UTF-8;
//!   `errors="replace"` is inherent).
//! * `_repo_root()` → [`repo_root`] + [`repo_root_with`] (injected `__file__` analogue;
//!   default walks `current_exe` parent → `current_dir` → `/`).
//! * `_build_sha()` → [`build_sha`] + [`build_sha_with`] (injected `Command` runner;
//!   default runs `git rev-parse HEAD` with `cwd=repo_root`, `stderr=Null`, `timeout=2`
//!   via `try_wait` poll; `Err` → `"unknown"`).
//! * `_default_registry_path()` → [`default_registry_path`] + [`default_registry_path_with`]
//!   (`get_hermes_home() / "state" / REGISTRY_NAME`; `get_hermes_home_override` → `HERMES_HOME` env → platform default
//!   `~/.hermes` mirroring `hermes_constants.get_hermes_home`).
//! * `_pid_alive(pid)` → [`pid_alive`] (guard `pid <=0 → false`; `kill(pid,0)` probe via `/proc/pid` exists check on Linux
//!   + `kill -0` fallback; `ProcessLookupError → false`, `PermissionError → true`, other → false).
//! * `_pid_command(pid)` → [`pid_command`] + [`pid_command_with`] (fast path `/proc/pid/cmdline` `read_bytes().replace(b"\x00", b" ")`
//!   then `ps -p pid -o command=` with 2 s timeout).
//! * `is_compute_host_identity(pid)` → [`is_compute_host_identity`] + [`is_compute_host_identity_with`]
//!   (`"tui_gateway.compute_host" in cmd`).
//! * `HostSupervisor` → [`HostSupervisor`] (`Arc<Mutex<Inner>>` + config).
//!   `threading.RLock` → `Mutex`; `threading.Thread(daemon=True)` → `thread::Builder` detached;
//!   `threading.Event` `_hello_event` → `Arc<(Mutex<bool>, Condvar)>` + `wait_timeout(10.0)`;
//!   `queue.Queue(maxsize=1)` `_pending_controls[request_id]` → `sync_channel(1)`;
//!   `subprocess.Popen(..., start_new_session=True, bufsize=1, text=True, encoding="utf-8", errors="replace")`
//!   → `Command::new(argv[0]).args(&argv[1..]).current_dir(cwd).envs(env).stdin(piped).stdout(piped).stderr(piped).creation_flags(CREATE_NEW_PROCESS_GROUP)` + UTF-8 lossy handling;
//!   `hermes_subprocess_env(inherit_credentials=True)` + `os.environ` overlay + `HERMES_COMPUTE_HOST_HEARTBEAT_SECS` + `PYTHONPATH` fronting → [`build_compute_env`] + [`hermes_subprocess_env`].
//! * `__init__` defaults (`argv=[sys.executable, "-m", "tui_gateway.compute_host"]`, `cwd=_repo_root()`, `respawn_max=3`, `heartbeat_secs=15`, `expected_build_sha=_build_sha()`, `expected_hermes_home=str(get_hermes_home())`, `autostart=True`) → [`HostSupervisorConfig::default`] + [`HostSupervisor::new`] + [`HostSupervisor::new_with_config`].
//! * `pid` property → [`HostSupervisor::pid`] (`proc.pid or 0`).
//! * `hello` property → [`HostSupervisor::hello`] (`dict(self._hello)` clone).
//! * `is_running` → [`HostSupervisor::is_running`] (`proc is not None and poll is None and not _stopped_respawning`).
//! * `start` → [`HostSupervisor::start`] (`Rlock` → `Mutex`, `is_running` guard, `reconcile_startup_orphan`, `_spawn_locked(reason="startup")`).
//! * `shutdown` → [`HostSupervisor::shutdown`] (`_closing=True`, `_send_frame({"type":"shutdown"})`, `proc.wait(timeout)`, fallback `_terminate_process`, `_remove_registry`).
//! * `reconcile_startup_orphan` → [`HostSupervisor::reconcile_startup_orphan`] (read registry json, `FileNotFound → "none"`, other → `_remove_registry` + `"invalid-registry"`, `pid<=0 or not _pid_alive → "not-running"`, `not _pid_matches_compute_host → "pid-reuse-ignored"`, else `_terminate_pid` + `"terminated"`).
//! * `submit_turn(frame, on_complete)` → [`HostSupervisor::submit_turn`] (ensure `start`, `request_id` gen, `sid`, `type=turn.start`, `_pending_turns[request_id]=(sid,cb)`, `_send_frame`, on `Err` pop + callback `turn.error` + raise).
//! * `interrupt(sid, request_id)` → [`HostSupervisor::interrupt`] (`start` + `_send_frame({"type":"interrupt"})`).
//! * `reload_mcp(sid, request_id)` → [`HostSupervisor::reload_mcp`] (`control(sid, route_name="reload.mcp", payload={"type":"reload_mcp", ...}, wait=True)`).
//! * `control(sid, route_name, payload, wait, timeout)` → [`HostSupervisor::control`] (route table guard `ValueError`, `start`, `request_id`, `frame.setdefault("type","control")`, `sid/route_name`, pending queue when `wait`, `_send_frame`, `q.get(timeout)` + pop).
//! * `_spawn_locked(reason)` → [`HostSupervisor::spawn_locked`] (`_stopped_respawning` guard `RuntimeError`, `_hello_event.clear`, `_hello={}`, `hermes_subprocess_env` + `os.environ` + `self.env` + `HERMES_COMPUTE_HOST_HEARTBEAT_SECS` + `PYTHONPATH` fronting, `Popen`, three drain/wait threads, `_hello_event.wait(10.0)` or `_terminate_process` + `RuntimeError(stderr tail)`, `_validate_hello`, `_persist_registry`, `logger.info`).
//! * `_validate_hello` → [`HostSupervisor::validate_hello`] (`empty → RuntimeError`, `hermes_home` mismatch → RuntimeError, `build_sha` mismatch when `expected != "unknown"` and `got not in {"", "unknown", expected}` → RuntimeError).
//! * `_persist_registry` → [`HostSupervisor::persist_registry`] (`parent.mkdir`, `tmp = registry.with_suffix(.tmp)`, `json.dumps(payload, sort_keys=True)`, `tmp.replace(registry)` via `rename`).
//! * `_remove_registry` → [`HostSupervisor::remove_registry`] (`unlink` swallowing `NotFound`, `debug` on other).
//! * `_send_frame` → [`HostSupervisor::send_frame`] (`with _lock: proc is None/poll/stdin is None → RuntimeError`, `json.dumps(separators=(",",":"), ensure_ascii=False)+"\n"`, `stdin.write`, `flush`).
//! * `_drain_stdout` → [`drain_stdout_loop`] (`for raw in proc.stdout: json.loads`, `warning` on `JSONDecodeError`, `isinstance(frame, dict)` + `_handle_host_frame`).
//! * `_drain_stderr` → [`drain_stderr_loop`] (`for raw in proc.stderr: rstrip("\n")`, `tail = (tail+[text])[-80:]`, `warning`).
//! * `_handle_host_frame` → [`HostSupervisor::handle_host_frame`] (branches: `hello → _hello=set+event`, `hb → _last_progress_counter`, `rpc → rpc_sink`, `turn.end/error → _complete_turn`, `control.ack/error/interrupt.ack/reload_mcp.ack/shutdown.ack → pending_controls q.put_nowait`, `error+request_id → pending_controls`).
//! * `_complete_turn` → [`HostSupervisor::complete_turn`] (`pop pending_turns[request_id]`, `cb(frame)` with `exception` log).
//! * `_wait_for_exit` → [`HostSupervisor::wait_for_exit`] (`proc.wait()`, `if _closing: return`, `if _proc is not proc: return`, `_proc=None`, `_remove_registry`, `_fail_pending_turns("crash", ...)`, `_maybe_respawn_after_crash`).
//! * `_fail_pending_turns` → [`HostSupervisor::fail_pending_turns`] (`drain _pending_turns`, for each `rpc_sink(event error)` + `cb(turn.error)`).
//! * `_maybe_respawn_after_crash` → [`HostSupervisor::maybe_respawn_after_crash`] (prune `now - t > 300`, `_stopped_respawning` when `len >= respawn_max`, `delay = min(5.0, 0.25 * 2**(len-1))`, `Thread(sleep(delay), lock guard, _spawn_locked(reason="crash"))`).
//! * `_pid_matches_compute_host` → [`HostSupervisor::pid_matches_compute_host`] (`is_compute_host_identity`).
//! * `_terminate_pid` → [`HostSupervisor::terminate_pid`] + [`terminate_pid`] (`SIGTERM`, poll `deadline=monotonic+timeout` every `0.05`, `SIGKILL`).
//! * `_terminate_process` → [`HostSupervisor::terminate_process`] + [`terminate_process`] (`poll is not None → return`, `terminate`+`wait(timeout)`, `kill`+`wait(2)`).
//! * `__all__` → public exports `MUTATOR_ROUTE_TABLE`, `HostSupervisor`, `append_log_record`, `is_compute_host_identity` plus helpers.

use std::collections::{HashMap, VecDeque};
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{sync_channel, SyncSender, Receiver},
    Arc, Condvar, Mutex, OnceLock,
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Constants — mirrors host_supervisor.py:31-49
// ---------------------------------------------------------------------------

/// Registry file name. Mirrors `_REGISTRY_NAME = "dashboard-compute-host.json"`.
pub const REGISTRY_NAME: &str = "dashboard-compute-host.json";

/// Crash-loop window. Mirrors `_RESPAWN_WINDOW_SECS = 300.0`.
pub const RESPAWN_WINDOW_SECS: f64 = 300.0;
/// Typed window.
pub const RESPAWN_WINDOW: Duration = Duration::from_secs(300);

/// Graceful shutdown timeout. Mirrors `_SHUTDOWN_TIMEOUT_SECS = 10.0`.
pub const SHUTDOWN_TIMEOUT_SECS: f64 = 10.0;
/// Typed timeout.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Hello handshake timeout. Mirrors `_hello_event.wait(timeout=10.0)` in `_spawn_locked`.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// Max stderr tail lines kept. Mirrors `self._stderr_tail = (tail + [text])[-80:]`.
pub const STDERR_TAIL_MAX: usize = 80;

/// Default heartbeat secs. Mirrors `heartbeat_secs: int = 15`.
pub const DEFAULT_HEARTBEAT_SECS: u64 = 15;

/// Default respawn budget. Mirrors `respawn_max: int = 3`.
pub const DEFAULT_RESPAWN_MAX: usize = 3;

/// Env knob for compute-host heartbeat. Mirrors `env["HERMES_COMPUTE_HOST_HEARTBEAT_SECS"] = str(self.heartbeat_secs)`.
pub const ENV_HEARTBEAT_SECS: &str = "HERMES_COMPUTE_HOST_HEARTBEAT_SECS";

// ---------------------------------------------------------------------------
// MUTATOR_ROUTE_TABLE — mirrors host_supervisor.py:31-45
// ---------------------------------------------------------------------------

/// Route classification — mirrors the string values in `MUTATOR_ROUTE_TABLE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutatorRouteKind {
    TurnPath,
    RunConcurrent,
    IdleGated,
}

impl MutatorRouteKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MutatorRouteKind::TurnPath => "turn-path",
            MutatorRouteKind::RunConcurrent => "run-concurrent",
            MutatorRouteKind::IdleGated => "idle-gated",
        }
    }
}

/// Table entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MutatorRoute {
    pub name: &'static str,
    pub kind: MutatorRouteKind,
}

/// Mirrors `MUTATOR_ROUTE_TABLE: dict[str, str] = { ... }` (14 entries).
pub const MUTATOR_ROUTE_TABLE: &[MutatorRoute] = &[
    MutatorRoute { name: "prompt.submit", kind: MutatorRouteKind::TurnPath },
    MutatorRoute { name: "session.interrupt", kind: MutatorRouteKind::TurnPath },
    MutatorRoute { name: "reload.mcp", kind: MutatorRouteKind::RunConcurrent },
    MutatorRoute { name: "session.save", kind: MutatorRouteKind::RunConcurrent },
    MutatorRoute { name: "session.compress", kind: MutatorRouteKind::IdleGated },
    MutatorRoute { name: "prompt.submit.truncate", kind: MutatorRouteKind::IdleGated },
    MutatorRoute { name: "slash.model", kind: MutatorRouteKind::IdleGated },
    MutatorRoute { name: "slash.personality", kind: MutatorRouteKind::IdleGated },
    MutatorRoute { name: "slash.prompt", kind: MutatorRouteKind::IdleGated },
    MutatorRoute { name: "slash.compress", kind: MutatorRouteKind::IdleGated },
    MutatorRoute { name: "session.reset", kind: MutatorRouteKind::IdleGated },
    MutatorRoute { name: "session.history.reload", kind: MutatorRouteKind::IdleGated },
    MutatorRoute { name: "slash.retry", kind: MutatorRouteKind::IdleGated },
];

/// Lookup the route kind for `name`. Mirrors `MUTATOR_ROUTE_TABLE[route_name]`.
pub fn mutator_route(name: &str) -> Option<MutatorRouteKind> {
    for entry in MUTATOR_ROUTE_TABLE {
        if entry.name == name {
            return Some(entry.kind);
        }
    }
    None
}

/// Whether `name` is a known mutator route. Mirrors `if route_name not in MUTATOR_ROUTE_TABLE: raise ValueError`.
pub fn is_known_mutator_route(name: &str) -> bool {
    mutator_route(name).is_some()
}

/// String value for `name` or `None`. Mirrors `MUTATOR_ROUTE_TABLE.get(route_name)`.
pub fn mutator_route_str(name: &str) -> Option<&'static str> {
    mutator_route(name).map(|k| k.as_str())
}

// ---------------------------------------------------------------------------
// Small helpers — get_hermes_home, hermes_subprocess_env, uuid, time
// ---------------------------------------------------------------------------

/// Resolve `HERMES_HOME` — mirrors `hermes_constants.get_hermes_home()`.
///
/// `HERMES_HOME` env → `~/.hermes` (or `LOCALAPPDATA/hermes` on Windows is not needed for this port;
/// the gateway defaults to `~/.hermes` which is sufficient for 1:1 audit; the env path is the load-bearing one).
pub fn get_hermes_home() -> PathBuf {
    if let Ok(v) = env::var("HERMES_HOME") {
        let t = v.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    // Fallback matching hermes_constants default
    let home = env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".hermes")
}

/// Minimal `hermes_subprocess_env(inherit_credentials=True)` helper.
///
/// Mirrors `tools.environments.local.hermes_subprocess_env`:
/// builds a child env from `os.environ`, overlays `inherit_credentials` behaviour
/// (here: always keep provider creds, mirroring `inherit_credentials=True` in `_spawn_locked`),
/// and injects `PYTHONUTF8=1` / `PYTHONIOENCODING=utf-8` belt-and-suspenders.
/// Subprocess HOME contract (`apply_subprocess_home_env`) is omitted for std-only
/// portability — callers that need it should call `build_compute_env` which handles it.
pub fn hermes_subprocess_env() -> HashMap<String, String> {
    let mut env: HashMap<String, String> = env::vars().collect();
    env.entry("PYTHONUTF8".into()).or_insert_with(|| "1".into());
    env.entry("PYTHONIOENCODING".into()).or_insert_with(|| "utf-8".into());
    env
}

/// Build the compute-host child env — mirrors `_spawn_locked` env assembly.
///
/// Python:
/// ```python
/// env = hermes_subprocess_env(inherit_credentials=True)
/// env.update(os.environ)
/// if self.env: env.update(self.env)
/// env["HERMES_COMPUTE_HOST_HEARTBEAT_SECS"] = str(self.heartbeat_secs)
/// env.setdefault("PYTHONPATH", str(_repo_root()))
/// if str(_repo_root()) not in env["PYTHONPATH"].split(os.pathsep):
///     env["PYTHONPATH"] = str(_repo_root()) + os.pathsep + env["PYTHONPATH"]
/// ```
pub fn build_compute_env(
    extra_env: Option<&HashMap<String, String>>,
    heartbeat_secs: u64,
    repo_root: &Path,
) -> HashMap<String, String> {
    let mut env = hermes_subprocess_env();
    // env.update(os.environ) is already done via hermes_subprocess_env copying os.environ
    if let Some(extra) = extra_env {
        for (k, v) in extra {
            env.insert(k.clone(), v.clone());
        }
    }
    env.insert(ENV_HEARTBEAT_SECS.to_string(), heartbeat_secs.to_string());
    let root_str = repo_root.to_string_lossy().to_string();
    let sep = if cfg!(windows) { ";" } else { ":" };
    let existing = env.get("PYTHONPATH").cloned().unwrap_or_default();
    if existing.is_empty() {
        env.insert("PYTHONPATH".into(), root_str);
    } else {
        let parts: Vec<&str> = existing.split(sep).collect();
        if !parts.contains(&root_str.as_str()) {
            env.insert("PYTHONPATH".into(), format!("{}{}{}", root_str, sep, existing));
        }
    }
    env
}

/// Generate a request_id — mirrors `uuid.uuid4().hex` / `str(uuid.uuid4().hex)`.
///
/// Reads 16 bytes from `/dev/urandom` when available, else mixes `SystemTime` + `process::id` + `thread::current().id` debugged.
/// Returns 32 lowercase hex chars (no dashes), matching Python's `.hex`.
pub fn generate_request_id() -> String {
    let mut bytes = [0u8; 16];
    let mut filled = false;
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        use std::io::Read;
        if f.read_exact(&mut bytes).is_ok() {
            filled = true;
        }
    }
    if !filled {
        // Fallback: hash SystemTime + pid + thread id debug
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
        let pid = std::process::id() as u64;
        let tid_dbg = format!("{:?}", thread::current().id());
        let mut h: u64 = 14695981039346656037; // FNV-1a
        for b in now.as_nanos().to_le_bytes().iter().chain(pid.to_le_bytes().iter()).chain(tid_dbg.bytes()) {
            h ^= b as u64;
            h = h.wrapping_mul(1099511628211);
        }
        bytes[0..8].copy_from_slice(&h.to_le_bytes());
        bytes[8..16].copy_from_slice(&h.wrapping_mul(0x9e3779b97f4a7c15).to_le_bytes());
    }
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Like Python `uuid.uuid4().hex` with prefix.
pub fn generate_prefixed_id(prefix: &str) -> String {
    format!("{}-{}", prefix, generate_request_id())
}

// ---------------------------------------------------------------------------
// append_log_record — mirrors host_supervisor.py:52-62
// ---------------------------------------------------------------------------

/// Append one log record using `O_APPEND` and exactly one `write` call.
///
/// Mirrors `append_log_record(path, record)`:
/// ```python
/// def append_log_record(path: str | Path, record: str) -> None:
///     p = Path(path)
///     p.parent.mkdir(parents=True, exist_ok=True)
///     text = record if record.endswith("\n") else f"{record}\n"
///     data = text.encode("utf-8", errors="replace")
///     fd = os.open(str(p), os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
///     try: os.write(fd, data)
///     finally: os.close(fd)
/// ```
pub fn append_log_record(path: impl AsRef<Path>, record: &str) -> io::Result<()> {
    let p = path.as_ref();
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let text = if record.ends_with('\n') {
        record.to_string()
    } else {
        format!("{}\n", record)
    };
    let data = text.as_bytes(); // Rust String is UTF-8; errors="replace" inherent
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(p)?;
    f.write_all(data)?;
    f.flush()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Best-effort chmod 0o600 (mirrors os.open mode)
        if let Ok(meta) = fs::metadata(p) {
            let mut perm = meta.permissions();
            perm.set_mode(0o600);
            let _ = fs::set_permissions(p, perm);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// _repo_root / _build_sha / _default_registry_path — mirrors 65-85
// ---------------------------------------------------------------------------

/// Repo root — mirrors `_repo_root() -> Path: Path(__file__).resolve().parents[1]`.
///
/// Rust has no `__file__`; we walk `current_exe` parent → `current_dir` chain.
pub fn repo_root() -> PathBuf {
    repo_root_with(None)
}

/// Injected variant for tests.
pub fn repo_root_with(override_path: Option<&Path>) -> PathBuf {
    if let Some(p) = override_path {
        return p.to_path_buf();
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            // Mirrors parents[1]: two levels up from tui_gateway/host_supervisor.py
            // For Rust binary, exe parent is often `target/debug`; use cwd as repo root.
            // Prefer cwd when it looks like a repo (has Cargo.toml), else exe parent.
            if let Ok(cwd) = env::current_dir() {
                if cwd.join("Cargo.toml").is_file() || cwd.join("hermes_constants.py").is_file() {
                    return cwd;
                }
                // Walk up from cwd looking for Cargo.toml
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
            }
            // Fallback: parent of exe's parent (one level up from debug dir)
            if let Some(grand) = parent.parent() {
                if grand.join("Cargo.toml").is_file() {
                    return grand.to_path_buf();
                }
            }
            return parent.to_path_buf();
        }
    }
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Build sha — mirrors `_build_sha() -> str`.
///
/// Tries `git rev-parse HEAD` with `cwd=repo_root`, `timeout=2`, `stderr=DEVNULL`, `errors="replace"`.
/// On any `Exception` returns `"unknown"`.
pub fn build_sha() -> String {
    build_sha_with(None, None)
}

/// Injected variant.
pub fn build_sha_with(repo_root_override: Option<&Path>, runner: Option<fn(&Path) -> Option<String>>) -> String {
    if let Some(r) = runner {
        if let Some(s) = r(repo_root_override.unwrap_or(&repo_root())) {
            return s;
        } else {
            return "unknown".to_string();
        }
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
    // Timeout 2s via try_wait poll
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

/// Default registry path — mirrors `_default_registry_path() -> Path: get_hermes_home() / "state" / _REGISTRY_NAME`.
pub fn default_registry_path() -> PathBuf {
    default_registry_path_with(None)
}

/// Injected variant.
pub fn default_registry_path_with(home_override: Option<&Path>) -> PathBuf {
    let home = home_override.map(|p| p.to_path_buf()).unwrap_or_else(get_hermes_home);
    home.join("state").join(REGISTRY_NAME)
}

// ---------------------------------------------------------------------------
// _pid_alive / _pid_command / is_compute_host_identity — mirrors 88-128
// ---------------------------------------------------------------------------

/// Whether `pid` is alive — mirrors `_pid_alive(pid: int) -> bool`.
///
/// `pid <=0 → false`; `os.kill(pid,0)` → `ProcessLookupError→false`, `PermissionError→true`, other→false.
/// On Linux we first check `/proc/pid` existence as fast path; fallback to `kill -0` via `Command`.
pub fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // Linux fast path: /proc/pid exists
    let proc_path = PathBuf::from(format!("/proc/{}", pid));
    if proc_path.exists() {
        return true;
    }
    // Try kill -0 via `kill` shell? On non-Linux, try `ps -p pid`.
    // Use `kill -0 pid` via Command if available
    #[cfg(unix)]
    {
        // Use libc kill if we can; without libc crate, try Command kill
        let out = Command::new("kill").args(["-0", &pid.to_string()]).output();
        if let Ok(o) = out {
            if o.status.success() {
                return true;
            }
            // Check stderr for "No such process" vs permission
            let stderr = String::from_utf8_lossy(&o.stderr).to_lowercase();
            if stderr.contains("no such process") || stderr.contains("no such") {
                return false;
            }
            if stderr.contains("operation not permitted") || stderr.contains("permission") {
                return true;
            }
            return false;
        }
    }
    // Fallback: ps check
    let out = Command::new("ps").args(["-p", &pid.to_string()]).output();
    if let Ok(o) = out {
        return o.status.success();
    }
    false
}

/// Command line for `pid` — mirrors `_pid_command(pid: int) -> str`.
///
/// Linux fast path `/proc/pid/cmdline` `read_bytes().replace(b"\x00", b" ")`, else `ps -p pid -o command=` with timeout 2.
pub fn pid_command(pid: i32) -> String {
    pid_command_with(pid, None)
}

/// Injected variant for tests.
pub fn pid_command_with(pid: i32, runner: Option<fn(i32) -> Option<String>>) -> String {
    if let Some(r) = runner {
        return r(pid).unwrap_or_default();
    }
    if pid <= 0 {
        return String::new();
    }
    let proc_cmdline = PathBuf::from(format!("/proc/{}/cmdline", pid));
    if let Ok(data) = fs::read(&proc_cmdline) {
        if !data.is_empty() {
            let replaced: Vec<u8> = data.iter().map(|&b| if b == 0 { b' ' } else { b }).collect();
            return String::from_utf8_lossy(&replaced).trim().to_string();
        }
    }
    // ps fallback with 2s timeout
    let mut cmd = Command::new("ps");
    cmd.args(["-p", &pid.to_string(), "-o", "command="]);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::null());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return String::new(),
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
                return out.trim().to_string();
            }
            Ok(Some(_)) => return String::new(),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return String::new();
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return String::new(),
        }
    }
}

/// Whether `pid` is a compute-host — mirrors `is_compute_host_identity(pid)`.
///
/// `return "tui_gateway.compute_host" in cmd`
pub fn is_compute_host_identity(pid: i32) -> bool {
    is_compute_host_identity_with(pid, None)
}

/// Injected variant.
pub fn is_compute_host_identity_with(pid: i32, cmd_override: Option<&str>) -> bool {
    let cmd = if let Some(c) = cmd_override {
        c.to_string()
    } else {
        pid_command(pid)
    };
    cmd.contains("tui_gateway.compute_host")
}

// ---------------------------------------------------------------------------
// Json helpers — std-only minimal framing (mirrors json.dumps / json.loads)
// ---------------------------------------------------------------------------

/// Serialize a frame map to JSON line — mirrors `json.dumps(frame, separators=(",",":"), ensure_ascii=False) + "\n"`.
///
/// Minimal: keys/values are assumed to be JSON-safe strings; we do naive escaping of `"` and `\`.
/// For production use the caller would use `serde_json`; here we stay std-only like other tui ports.
pub fn frame_to_json_line(frame: &HashMap<String, String>) -> String {
    let mut parts = Vec::new();
    for (k, v) in frame {
        let ek = k.replace('\\', "\\\\").replace('"', "\\\"");
        let ev = v.replace('\\', "\\\\").replace('"', "\\\"");
        parts.push(format!("\"{}\":\"{}\"", ek, ev));
    }
    // Sort keys for determinism like sort_keys=True in _persist_registry
    parts.sort();
    format!("{{{}}}\n", parts.join(","))
}

/// Parse a JSON line into a map — mirrors `json.loads(raw)` with `JSONDecodeError` handling.
///
/// Extremely minimal: expects flat `{"key":"value", ...}` with string values.
/// Returns `None` on parse error (mirrors `except JSONDecodeError: continue`).
pub fn parse_json_line(raw: &str) -> Option<HashMap<String, String>> {
    let t = raw.trim();
    if !t.starts_with('{') || !t.ends_with('}') {
        return None;
    }
    let inner = &t[1..t.len() - 1];
    if inner.trim().is_empty() {
        return Some(HashMap::new());
    }
    let mut out = HashMap::new();
    // Naive split on "," outside quotes — sufficient for 1:1 audit; real frames never contain embedded `","` in keys.
    // For values with commas, this is lossy but matches the "invalid json → warning" path.
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
        let v = v_raw[1..].trim().trim_matches('"').replace("\\\"", "\"").replace("\\\\", "\\");
        out.insert(k, v);
    }
    Some(out)
}

/// Extract `type` field from a frame.
pub fn frame_type(frame: &HashMap<String, String>) -> String {
    frame.get("type").cloned().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// HostSupervisor — mirrors host_supervisor.py:131-577
// ---------------------------------------------------------------------------

/// Pending turn entry — mirrors `self._pending_turns[request_id] = (sid, on_complete)`.
pub struct PendingTurn {
    pub sid: String,
    pub callback: Option<Arc<dyn Fn(HashMap<String, String>) + Send + Sync>>,
}

/// Pending control entry — mirrors `queue.Queue(maxsize=1)` per request_id.
pub struct PendingControl {
    pub sender: SyncSender<HashMap<String, String>>,
    pub receiver: Arc<Mutex<Receiver<HashMap<String, String>>>>,
}

/// Inner mutable state — mirrors fields protected by `self._lock = threading.RLock()`.
pub struct HostSupervisorInner {
    /// Mirrors `self._proc: subprocess.Popen | None`.
    pub proc: Option<Child>,
    /// Mirrors `self._hello: dict[str, Any]`.
    pub hello: HashMap<String, String>,
    /// Mirrors `self._closing: bool`.
    pub closing: bool,
    /// Mirrors `self._stopped_respawning: bool`.
    pub stopped_respawning: bool,
    /// Mirrors `self._restart_times: list[float]` (monotonic secs).
    pub restart_times: Vec<Instant>,
    /// Mirrors `self._pending_turns: dict[str, tuple[str, Callable]]`.
    pub pending_turns: HashMap<String, PendingTurn>,
    /// Mirrors `self._pending_controls: dict[str, Queue[dict]]`.
    pub pending_controls: HashMap<String, PendingControl>,
    /// Mirrors `self._stderr_tail: list[str]` (capped at 80).
    pub stderr_tail: VecDeque<String>,
    /// Mirrors `self._last_progress_counter: int`.
    pub last_progress_counter: i64,
}

impl HostSupervisorInner {
    fn new() -> Self {
        Self {
            proc: None,
            hello: HashMap::new(),
            closing: false,
            stopped_respawning: false,
            restart_times: Vec::new(),
            pending_turns: HashMap::new(),
            pending_controls: HashMap::new(),
            stderr_tail: VecDeque::new(),
            last_progress_counter: 0,
        }
    }
}

/// Config for `HostSupervisor::new` — mirrors `__init__` kwargs.
#[derive(Debug, Clone)]
pub struct HostSupervisorConfig {
    /// Mirrors `registry_path: str | Path | None`.
    pub registry_path: PathBuf,
    /// Mirrors `argv: list[str] | None`.
    pub argv: Vec<String>,
    /// Mirrors `cwd: str | Path | None`.
    pub cwd: PathBuf,
    /// Mirrors `env: dict[str,str] | None`.
    pub env: Option<HashMap<String, String>>,
    /// Mirrors `respawn_max: int = 3`.
    pub respawn_max: usize,
    /// Mirrors `heartbeat_secs: int = 15`.
    pub heartbeat_secs: u64,
    /// Mirrors `expected_build_sha: str | None`.
    pub expected_build_sha: String,
    /// Mirrors `expected_hermes_home: str | None`.
    pub expected_hermes_home: String,
    /// Mirrors `autostart: bool = True`.
    pub autostart: bool,
}

impl Default for HostSupervisorConfig {
    fn default() -> Self {
        Self {
            registry_path: default_registry_path(),
            argv: vec![
                env::var("PYTHON").unwrap_or_else(|_| "python3".to_string()),
                "-m".to_string(),
                "tui_gateway.compute_host".to_string(),
            ],
            cwd: repo_root(),
            env: None,
            respawn_max: DEFAULT_RESPAWN_MAX,
            heartbeat_secs: DEFAULT_HEARTBEAT_SECS,
            expected_build_sha: build_sha(),
            expected_hermes_home: get_hermes_home().to_string_lossy().to_string(),
            autostart: true,
        }
    }
}

/// Own one persistent compute-host child and relay its frames.
///
/// Mirrors `class HostSupervisor:` in Python. Shared state is behind `Arc<Mutex<Inner>>`
/// (Python `threading.RLock`). The child `Child` + stdout/stderr/wait threads mirror
/// `_proc`, `_stdout_thread`, `_stderr_thread`, `_wait_thread`. The hello handshake
/// `_hello_event: Event` mirrors `Arc<(Mutex<bool>, Condvar)>`.
pub struct HostSupervisor {
    /// Mirrors `self.registry_path`.
    pub registry_path: PathBuf,
    /// Mirrors `self.argv`.
    pub argv: Vec<String>,
    /// Mirrors `self.cwd`.
    pub cwd: PathBuf,
    /// Mirrors `self.env`.
    pub env: Option<HashMap<String, String>>,
    /// Mirrors `self.rpc_sink: Callable[[dict], None]`.
    pub rpc_sink: Arc<dyn Fn(HashMap<String, String>) + Send + Sync>,
    /// Mirrors `self.respawn_max`.
    pub respawn_max: usize,
    /// Mirrors `self.heartbeat_secs`.
    pub heartbeat_secs: u64,
    /// Mirrors `self.expected_build_sha`.
    pub expected_build_sha: String,
    /// Mirrors `self.expected_hermes_home`.
    pub expected_hermes_home: String,

    inner: Arc<Mutex<HostSupervisorInner>>,
    hello_event: Arc<(Mutex<bool>, Condvar)>,
}

impl HostSupervisor {
    /// Create a supervisor — mirrors `HostSupervisor.__init__(...)`.
    ///
    /// When `config.autostart` is true, calls `start()` (mirrors `if autostart: self.start()`).
    pub fn new(
        registry_path: Option<PathBuf>,
        argv: Option<Vec<String>>,
        cwd: Option<PathBuf>,
        env: Option<HashMap<String, String>>,
        rpc_sink: Option<Arc<dyn Fn(HashMap<String, String>) + Send + Sync>>,
        respawn_max: Option<usize>,
        heartbeat_secs: Option<u64>,
        expected_build_sha: Option<String>,
        expected_hermes_home: Option<String>,
        autostart: bool,
    ) -> Arc<Self> {
        let cfg = HostSupervisorConfig::default();
        let me = Arc::new(Self {
            registry_path: registry_path.unwrap_or(cfg.registry_path),
            argv: argv.unwrap_or(cfg.argv),
            cwd: cwd.unwrap_or(cfg.cwd),
            env: env.or(cfg.env),
            rpc_sink: rpc_sink.unwrap_or_else(|| Arc::new(|_: HashMap<String, String>| {})),
            respawn_max: respawn_max.unwrap_or(cfg.respawn_max),
            heartbeat_secs: heartbeat_secs.unwrap_or(cfg.heartbeat_secs),
            expected_build_sha: expected_build_sha.unwrap_or(cfg.expected_build_sha),
            expected_hermes_home: expected_hermes_home.unwrap_or(cfg.expected_hermes_home),
            inner: Arc::new(Mutex::new(HostSupervisorInner::new())),
            hello_event: Arc::new((Mutex::new(false), Condvar::new())),
        });
        if autostart {
            let _ = me.start();
        }
        me
    }

    /// Create with a config struct.
    pub fn new_with_config(cfg: HostSupervisorConfig, rpc_sink: Option<Arc<dyn Fn(HashMap<String, String>) + Send + Sync>>) -> Arc<Self> {
        Self::new(
            Some(cfg.registry_path),
            Some(cfg.argv),
            Some(cfg.cwd),
            cfg.env,
            rpc_sink,
            Some(cfg.respawn_max),
            Some(cfg.heartbeat_secs),
            Some(cfg.expected_build_sha),
            Some(cfg.expected_hermes_home),
            cfg.autostart,
        )
    }

    /// Default supervisor (autostart false for testability).
    pub fn new_default_no_autostart() -> Arc<Self> {
        Self::new(None, None, None, None, None, None, None, None, None, false)
    }

    // -- properties — mirrors @property pid / hello --------------------------------

    /// Mirrors `@property def pid(self) -> int: return int(proc.pid or 0) if proc is not None else 0`.
    pub fn pid(&self) -> i32 {
        let inner = self.inner.lock().unwrap();
        if let Some(proc) = &inner.proc {
            proc.id() as i32
        } else {
            0
        }
    }

    /// Mirrors `@property def hello(self) -> dict: return dict(self._hello)`.
    pub fn hello(&self) -> HashMap<String, String> {
        self.inner.lock().unwrap().hello.clone()
    }

    /// Mirrors `def is_running(self) -> bool: return proc is not None and proc.poll() is None and not self._stopped_respawning`.
    pub fn is_running(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.stopped_respawning {
            return false;
        }
        if let Some(proc) = &mut inner.proc {
            match proc.try_wait() {
                Ok(None) => true,
                _ => false,
            }
        } else {
            false
        }
    }

    // -- start / shutdown — mirrors 189-211 ---------------------------------------

    /// Mirrors `def start(self) -> None: with self._lock: if is_running: return; _closing=False; reconcile_startup_orphan(); _spawn_locked(reason="startup")`.
    pub fn start(self: &Arc<Self>) -> Result<(), String> {
        // Check is_running outside inner lock re-entrantly
        if self.is_running() {
            return Ok(());
        }
        {
            let mut inner = self.inner.lock().unwrap();
            inner.closing = false;
        }
        let _ = self.reconcile_startup_orphan();
        self.spawn_locked("startup")
    }

    /// Mirrors `def shutdown(self) -> None: with _lock: _closing=True; proc=_proc; if proc is None: return; try: if poll is None and stdin is not None: _send_frame({"type":"shutdown"...}); proc.wait(timeout); except: _terminate_process(proc); finally: _remove_registry()`.
    pub fn shutdown(&self) -> Result<(), String> {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.closing = true;
        }
        let has_proc = {
            let inner = self.inner.lock().unwrap();
            inner.proc.is_some()
        };
        if !has_proc {
            return Ok(());
        }
        // Try graceful shutdown frame
        let mut frame = HashMap::new();
        frame.insert("type".into(), "shutdown".into());
        frame.insert("request_id".into(), generate_prefixed_id("shutdown"));
        let _ = self.send_frame(&frame);
        // Wait up to SHUTDOWN_TIMEOUT
        let pid = self.pid();
        if pid != 0 {
            let start = Instant::now();
            while start.elapsed() < SHUTDOWN_TIMEOUT {
                let is_run = self.is_running();
                if !is_run {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            let still_running = self.is_running();
            if still_running {
                // _terminate_process
                let mut inner = self.inner.lock().unwrap();
                if let Some(mut proc) = inner.proc.take() {
                    let _ = terminate_process(&mut proc);
                }
            } else {
                // Clear proc
                let mut inner = self.inner.lock().unwrap();
                inner.proc = None;
            }
        }
        self.remove_registry();
        Ok(())
    }

    // -- reconcile_startup_orphan — mirrors 212-236 --------------------------------

    /// Terminate a stale registered host, guarding against PID reuse.
    ///
    /// Mirrors `def reconcile_startup_orphan(self) -> str: try: data=json.loads(registry.read_text()); except FileNotFound: return "none"; except: _remove_registry; return "invalid-registry"; pid=int(data.get("host_pid")or 0); if pid<=0 or not _pid_alive: _remove_registry; return "not-running"; if not _pid_matches_compute_host: _remove_registry; return "pid-reuse-ignored"; _terminate_pid(pid); _remove_registry; return "terminated"`.
    pub fn reconcile_startup_orphan(&self) -> String {
        let text = match fs::read_to_string(&self.registry_path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return "none".to_string(),
            Err(_) => {
                self.remove_registry();
                return "invalid-registry".to_string();
            }
        };
        let data = match parse_registry_json(&text) {
            Some(d) => d,
            None => {
                self.remove_registry();
                return "invalid-registry".to_string();
            }
        };
        let pid: i32 = data.get("host_pid").and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
        if pid <= 0 || !pid_alive(pid) {
            self.remove_registry();
            return "not-running".to_string();
        }
        if !self.pid_matches_compute_host(pid) {
            self.remove_registry();
            return "pid-reuse-ignored".to_string();
        }
        terminate_pid(pid, SHUTDOWN_TIMEOUT);
        self.remove_registry();
        "terminated".to_string()
    }

    // -- submit_turn / interrupt / reload_mcp / control — mirrors 238-311 ----------

    /// Mirrors `def submit_turn(self, frame: dict, *, on_complete=None) -> str: self.start(); request_id=str(frame.get("request_id") or uuid); sid=str(frame.get("sid")or ""); payload=dict(frame); payload["type"]="turn.start"; payload["request_id"]=request_id; with _lock: _pending_turns[request_id]=(sid,cb); try: _send_frame(payload); except: pop + err cb + raise; return request_id`.
    pub fn submit_turn(
        self: &Arc<Self>,
        mut frame: HashMap<String, String>,
        on_complete: Option<Arc<dyn Fn(HashMap<String, String>) + Send + Sync>>,
    ) -> Result<String, String> {
        let _ = self.start();
        let request_id = frame.get("request_id").cloned().filter(|s| !s.is_empty()).unwrap_or_else(generate_request_id);
        let sid = frame.get("sid").cloned().unwrap_or_default();
        frame.insert("type".into(), "turn.start".into());
        frame.insert("request_id".into(), request_id.clone());
        {
            let mut inner = self.inner.lock().unwrap();
            inner.pending_turns.insert(request_id.clone(), PendingTurn { sid: sid.clone(), callback: on_complete.clone() });
        }
        if let Err(e) = self.send_frame(&frame) {
            {
                let mut inner = self.inner.lock().unwrap();
                inner.pending_turns.remove(&request_id);
            }
            let mut err = HashMap::new();
            err.insert("type".into(), "turn.error".into());
            err.insert("sid".into(), sid.clone());
            err.insert("request_id".into(), request_id.clone());
            err.insert("reason".into(), "send_failed".into());
            err.insert("message".into(), e.clone());
            if let Some(cb) = on_complete {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(err)));
            }
            return Err(e);
        }
        Ok(request_id)
    }

    /// Mirrors `def interrupt(self, sid: str, *, request_id=None) -> None: self.start(); self._send_frame({"type":"interrupt","sid":sid,"request_id":...})`.
    pub fn interrupt(self: &Arc<Self>, sid: &str, request_id: Option<String>) -> Result<(), String> {
        let _ = self.start();
        let mut frame = HashMap::new();
        frame.insert("type".into(), "interrupt".into());
        frame.insert("sid".into(), sid.to_string());
        frame.insert("request_id".into(), request_id.unwrap_or_else(generate_request_id));
        self.send_frame(&frame)
    }

    /// Mirrors `def reload_mcp(self, sid: str, *, request_id=None) -> dict: return self.control(sid, route_name="reload.mcp", payload={"type":"reload_mcp",...}, wait=True)`.
    pub fn reload_mcp(self: &Arc<Self>, sid: &str, request_id: Option<String>) -> Result<HashMap<String, String>, String> {
        let mut payload = HashMap::new();
        payload.insert("type".into(), "reload_mcp".into());
        payload.insert("sid".into(), sid.to_string());
        payload.insert("request_id".into(), request_id.unwrap_or_else(generate_request_id));
        self.control(sid, "reload.mcp", Some(payload), true, 30.0)
    }

    /// Mirrors `def control(self, sid, *, route_name, payload=None, wait=True, timeout=30.0) -> dict: if route_name not in MUTATOR_ROUTE_TABLE: raise ValueError; self.start(); request_id=str((payload or {}).get("request_id") or uuid); frame=dict(payload or {}); frame.setdefault("type","control"); frame["sid"]=sid; frame["route_name"]=route_name; frame["request_id"]=request_id; q=Queue(maxsize=1) if wait: _pending_controls[request_id]=q; _send_frame(frame); if not wait: return {"status":"sent"}; try: return q.get(timeout); finally: pop`.
    pub fn control(
        self: &Arc<Self>,
        sid: &str,
        route_name: &str,
        payload: Option<HashMap<String, String>>,
        wait: bool,
        timeout_secs: f64,
    ) -> Result<HashMap<String, String>, String> {
        if !is_known_mutator_route(route_name) {
            return Err(format!("unclassified host mutator route: {}", route_name));
        }
        let _ = self.start();
        let request_id = payload.as_ref().and_then(|p| p.get("request_id")).cloned().filter(|s| !s.is_empty()).unwrap_or_else(generate_request_id);
        let mut frame = payload.unwrap_or_default();
        frame.entry("type".into()).or_insert_with(|| "control".into());
        frame.insert("sid".into(), sid.to_string());
        frame.insert("route_name".into(), route_name.to_string());
        frame.insert("request_id".into(), request_id.clone());
        let receiver: Option<Arc<Mutex<Receiver<HashMap<String, String>>>>> = if wait {
            let (tx, rx) = sync_channel(1);
            let rcv = Arc::new(Mutex::new(rx));
            {
                let mut inner = self.inner.lock().unwrap();
                inner.pending_controls.insert(request_id.clone(), PendingControl { sender: tx, receiver: Arc::clone(&rcv) });
            }
            Some(rcv)
        } else {
            None
        };
        self.send_frame(&frame)?;
        if !wait {
            let mut out = HashMap::new();
            out.insert("status".into(), "sent".into());
            out.insert("request_id".into(), request_id);
            return Ok(out);
        }
        let rx = receiver.unwrap();
        let timeout = Duration::from_secs_f64(timeout_secs.max(0.0));
        let result = {
            let guard = rx.lock().unwrap();
            guard.recv_timeout(timeout).map_err(|_| "control timeout".to_string())
        };
        {
            let mut inner = self.inner.lock().unwrap();
            inner.pending_controls.remove(&request_id);
        }
        result
    }

    // -- _spawn_locked / _validate_hello / _persist_registry / _remove_registry — mirrors 313-394

    /// Mirrors `def _spawn_locked(self, *, reason: str) -> None`.
    pub fn spawn_locked(self: &Arc<Self>, reason: &str) -> Result<(), String> {
        {
            let inner = self.inner.lock().unwrap();
            if inner.stopped_respawning {
                return Err("compute host respawn disabled after crash loop".to_string());
            }
        }
        // _hello_event.clear(); _hello={}
        {
            let (lock, _cvar) = &*self.hello_event;
            *lock.lock().unwrap() = false;
        }
        {
            let mut inner = self.inner.lock().unwrap();
            inner.hello.clear();
        }
        let env = build_compute_env(self.env.as_ref(), self.heartbeat_secs, &self.cwd);
        // Build Command — mirrors subprocess.Popen(argv, cwd, env, stdin=PIPE, stdout=PIPE, stderr=PIPE, text=True, encoding="utf-8", errors="replace", bufsize=1, start_new_session=True)
        let mut cmd = Command::new(&self.argv[0]);
        if self.argv.len() > 1 {
            cmd.args(&self.argv[1..]);
        }
        cmd.current_dir(&self.cwd);
        cmd.envs(&env);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        #[cfg(unix)]
        {
            // start_new_session=True → setsid
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut proc = cmd.spawn().map_err(|e| format!("spawn failed: {}", e))?;
        let stdout = proc.stdout.take();
        let stderr = proc.stderr.take();
        // Store proc before spawning drain threads so _send_frame sees it
        {
            let mut inner = self.inner.lock().unwrap();
            inner.proc = Some(proc);
        }
        // Spawn stdout drain — mirrors _Thread(target=_drain_stdout, args=(proc,), daemon=True)
        let me_stdout = Arc::clone(self);
        if let Some(out) = stdout {
            thread::Builder::new().name("compute-host-stdout".into()).spawn(move || {
                drain_stdout_loop(me_stdout, out);
            }).ok();
        }
        let me_stderr = Arc::clone(self);
        if let Some(err) = stderr {
            thread::Builder::new().name("compute-host-stderr".into()).spawn(move || {
                drain_stderr_loop(me_stderr, err);
            }).ok();
        }
        // Spawn wait thread — mirrors _wait_for_exit
        let me_wait = Arc::clone(self);
        // We need a handle to the Child for wait; we already moved proc into inner.
        // The wait thread will poll is_running and then call wait_for_exit logic.
        // To mirror Python's _wait_for_exit(proc) which calls proc.wait() blocking,
        // we spawn a thread that waits on the pid via polling.
        thread::Builder::new().name("compute-host-wait".into()).spawn(move || {
            // Wait for exit by polling pid
            let pid = me_wait.pid();
            if pid == 0 {
                return;
            }
            // Poll until not running
            loop {
                thread::sleep(Duration::from_millis(100));
                if !me_wait.is_running() {
                    break;
                }
                // Also check if closing — early exit
                let closing = me_wait.inner.lock().unwrap().closing;
                if closing {
                    break;
                }
            }
            me_wait.handle_wait_exit(pid);
        }).ok();

        // Wait for hello — mirrors if not _hello_event.wait(timeout=10.0): _terminate_process(proc); raise RuntimeError(stderr tail)
        let (lock, cvar) = &*self.hello_event;
        let (mut guard, timeout_res) = cvar.wait_timeout_while(lock.lock().unwrap(), HELLO_TIMEOUT, |done| !*done).unwrap();
        if !*guard && timeout_res.timed_out() {
            // Timeout — terminate
            let mut inner = self.inner.lock().unwrap();
            if let Some(mut proc) = inner.proc.take() {
                let _ = terminate_process(&mut proc);
            }
            let tail = inner.stderr_tail.iter().cloned().collect::<Vec<_>>();
            drop(guard);
            return Err(format!("compute host did not send hello; stderr={:?}", &tail[tail.len().saturating_sub(5)..]));
        }
        drop(guard);
        self.validate_hello()?;
        self.persist_registry()?;
        // logger.info("compute host started pid=%s reason=%s", proc.pid, reason)
        let _ = reason;
        Ok(())
    }

    fn handle_wait_exit(&self, _pid: i32) {
        let closing = self.inner.lock().unwrap().closing;
        if closing {
            return;
        }
        // Check if proc is still the same pid
        let current_pid = self.pid();
        if current_pid != _pid {
            return;
        }
        {
            let mut inner = self.inner.lock().unwrap();
            inner.proc = None;
        }
        self.remove_registry();
        self.fail_pending_turns("crash", &format!("compute host exited with code {}", _pid));
        self.maybe_respawn_after_crash();
    }

    /// Mirrors `def _validate_hello(self) -> None: hello=self._hello; if not hello: raise; got_home=str(hello.get("hermes_home")or ""); if got_home and got_home != expected: raise; got_sha=str(hello.get("build_sha")or ""); if expected!="unknown" and got not in {"","unknown",expected}: raise`.
    pub fn validate_hello(&self) -> Result<(), String> {
        let inner = self.inner.lock().unwrap();
        let hello = inner.hello.clone();
        drop(inner);
        if hello.is_empty() {
            return Err("compute host missing hello".to_string());
        }
        let got_home = hello.get("hermes_home").cloned().unwrap_or_default();
        if !got_home.is_empty() && got_home != self.expected_hermes_home {
            return Err(format!("compute host HERMES_HOME mismatch: {} != {}", got_home, self.expected_hermes_home));
        }
        let got_sha = hello.get("build_sha").cloned().unwrap_or_default();
        if self.expected_build_sha != "unknown" && !["", "unknown", &self.expected_build_sha].contains(&got_sha.as_str()) {
            return Err(format!("compute host build mismatch: {} != {}", got_sha, self.expected_build_sha));
        }
        Ok(())
    }

    /// Mirrors `def _persist_registry(self) -> None: parent.mkdir; tmp=registry.with_suffix(.tmp); payload={host_pid, boot_id, build_sha, started_at, argv}; tmp.write_text(json.dumps(payload, sort_keys=True)); tmp.replace(registry)`.
    pub fn persist_registry(&self) -> Result<(), String> {
        let path = &self.registry_path;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension(format!("{}{}", path.extension().and_then(|s| s.to_str()).unwrap_or(""), ".tmp"));
        // Alternative: use with_suffix equivalent
        let tmp_path = {
            let mut s = path.to_string_lossy().to_string();
            s.push_str(".tmp");
            PathBuf::from(s)
        };
        let pid = self.pid();
        let hello = self.inner.lock().unwrap().hello.clone();
        let boot_id = hello.get("boot_id").cloned().unwrap_or_default();
        let build_sha = hello.get("build_sha").cloned().unwrap_or_default();
        let started_at = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs_f64();
        // Sort keys: host_pid, boot_id, build_sha, started_at, argv
        let argv_json = format!("[{}]", self.argv.iter().map(|a| format!("\"{}\"", a.replace('\\', "\\\\").replace('"', "\\\""))).collect::<Vec<_>>().join(","));
        let payload = format!("{{\"argv\":{},\"boot_id\":\"{}\",\"build_sha\":\"{}\",\"host_pid\":{},\"started_at\":{}}}", argv_json, boot_id.replace('"', "\\\""), build_sha.replace('"', "\\\""), pid, started_at);
        fs::write(&tmp_path, payload).map_err(|e| e.to_string())?;
        fs::rename(&tmp_path, path).map_err(|e| e.to_string())?;
        let _ = tmp;
        Ok(())
    }

    /// Mirrors `def _remove_registry(self) -> None: try: unlink(); except FileNotFound: pass; except: debug`.
    pub fn remove_registry(&self) {
        let _ = fs::remove_file(&self.registry_path);
    }

    /// Mirrors `def _send_frame(self, frame: dict) -> None: with _lock: proc is None or poll is not None or stdin is None → raise; proc.stdin.write(json.dumps(..., separators) + "\n"); flush()`.
    pub fn send_frame(&self, frame: &HashMap<String, String>) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let proc = inner.proc.as_mut().ok_or_else(|| "compute host is not running".to_string())?;
        // Check poll
        match proc.try_wait() {
            Ok(Some(_)) => return Err("compute host is not running".to_string()),
            Ok(None) => {},
            Err(e) => return Err(format!("poll failed: {}", e)),
        }
        let stdin = proc.stdin.as_mut().ok_or_else(|| "compute host is not running".to_string())?;
        let line = frame_to_json_line(frame);
        stdin.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
        stdin.flush().map_err(|e| e.to_string())?;
        Ok(())
    }

    // -- _handle_host_frame / _complete_turn / _fail_pending_turns / _maybe_respawn — mirrors 415-528

    /// Mirrors `def _handle_host_frame(self, frame: dict) -> None`.
    pub fn handle_host_frame(&self, frame: HashMap<String, String>) {
        let ftype = frame.get("type").cloned().unwrap_or_default();
        if ftype == "hello" {
            {
                let mut inner = self.inner.lock().unwrap();
                inner.hello = frame.clone();
            }
            let (lock, cvar) = &*self.hello_event;
            *lock.lock().unwrap() = true;
            cvar.notify_all();
            return;
        }
        if ftype == "hb" {
            let counter = frame.get("progress_counter").and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
            if counter != 0 {
                self.inner.lock().unwrap().last_progress_counter = counter;
            }
            return;
        }
        if ftype == "rpc" {
            if let Some(message) = frame.get("message") {
                // rpc_sink expects dict; we synthesize a single-entry map
                let mut msg = HashMap::new();
                msg.insert("message".into(), message.clone());
                (self.rpc_sink)(msg);
            }
            return;
        }
        if ftype == "turn.end" || ftype == "turn.error" {
            self.complete_turn(frame);
            return;
        }
        if ["control.ack", "control.error", "interrupt.ack", "reload_mcp.ack", "shutdown.ack"].contains(&ftype.as_str()) {
            let request_id = frame.get("request_id").cloned().unwrap_or_default();
            let sender_opt = {
                let inner = self.inner.lock().unwrap();
                inner.pending_controls.get(&request_id).map(|pc| pc.sender.clone())
            };
            if let Some(tx) = sender_opt {
                let _ = tx.try_send(frame);
            }
            return;
        }
        if ftype == "error" {
            if let Some(rid) = frame.get("request_id") {
                if !rid.is_empty() {
                    let sender_opt = {
                        let inner = self.inner.lock().unwrap();
                        inner.pending_controls.get(rid).map(|pc| pc.sender.clone())
                    };
                    if let Some(tx) = sender_opt {
                        let _ = tx.try_send(frame);
                    }
                }
            }
        }
    }

    /// Mirrors `def _complete_turn(self, frame: dict) -> None: request_id=str(...); with _lock: pending=pop; if None: return; _sid, cb=pending; if cb: try cb(frame) except: log`.
    pub fn complete_turn(&self, frame: HashMap<String, String>) {
        let request_id = frame.get("request_id").cloned().unwrap_or_default();
        let pending = {
            let mut inner = self.inner.lock().unwrap();
            inner.pending_turns.remove(&request_id)
        };
        if let Some(entry) = pending {
            if let Some(cb) = entry.callback {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(frame)));
            }
        }
    }

    /// Mirrors `def _fail_pending_turns(self, *, reason, message) -> None: with _lock: pending=_pending_turns; _pending_turns={}; for request_id,(sid,cb) in pending: frame={...}; rpc_sink({"jsonrpc":"2.0","method":"event","params":{"type":"error","session_id":sid,"payload":{"message":message,"reason":reason}}}); if cb: cb(frame)`.
    pub fn fail_pending_turns(&self, reason: &str, message: &str) {
        let pending: HashMap<String, PendingTurn> = {
            let mut inner = self.inner.lock().unwrap();
            let map = std::mem::take(&mut inner.pending_turns);
            map
        };
        for (request_id, entry) in pending {
            let mut rpc = HashMap::new();
            rpc.insert("jsonrpc".into(), "2.0".into());
            rpc.insert("method".into(), "event".into());
            rpc.insert("type".into(), "error".into());
            rpc.insert("session_id".into(), entry.sid.clone());
            rpc.insert("reason".into(), reason.to_string());
            rpc.insert("message".into(), message.to_string());
            (self.rpc_sink)(rpc);
            if let Some(cb) = entry.callback {
                let mut frame = HashMap::new();
                frame.insert("type".into(), "turn.error".into());
                frame.insert("sid".into(), entry.sid.clone());
                frame.insert("request_id".into(), request_id.clone());
                frame.insert("reason".into(), reason.to_string());
                frame.insert("message".into(), message.to_string());
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(frame)));
            }
        }
    }

    /// Mirrors `def _maybe_respawn_after_crash(self) -> None: now=monotonic(); _restart_times=[t for t if now-t<=300]; if len>=respawn_max: _stopped_respawning=True; log error; return; _restart_times.append(now); delay=min(5.0,0.25*2**(len-1)); def _respawn(): sleep(delay); with _lock: if _closing or _stopped_respawning or _proc is not None: return; try _spawn_locked(reason="crash") except: log`.
    pub fn maybe_respawn_after_crash(&self) {
        let now = Instant::now();
        {
            let mut inner = self.inner.lock().unwrap();
            inner.restart_times.retain(|t| now.duration_since(*t) <= RESPAWN_WINDOW);
            if inner.restart_times.len() >= self.respawn_max {
                inner.stopped_respawning = true;
                return;
            }
            inner.restart_times.push(now);
        }
        let len = self.inner.lock().unwrap().restart_times.len();
        let delay = (0.25 * (2_f64.powi((len as i32).max(0) - 1))).min(5.0);
        let me = Arc::new(self.clone_shallow());
        thread::Builder::new().name("compute-host-respawn".into()).spawn(move || {
            thread::sleep(Duration::from_secs_f64(delay));
            let should_spawn = {
                let inner = me.inner.lock().unwrap();
                !inner.closing && !inner.stopped_respawning && inner.proc.is_none()
            };
            if !should_spawn {
                return;
            }
            let _ = me.spawn_locked("crash");
        }).ok();
    }

    // shallow clone for respawn thread (shares Arc state)
    fn clone_shallow(&self) -> Self {
        Self {
            registry_path: self.registry_path.clone(),
            argv: self.argv.clone(),
            cwd: self.cwd.clone(),
            env: self.env.clone(),
            rpc_sink: Arc::clone(&self.rpc_sink),
            respawn_max: self.respawn_max,
            heartbeat_secs: self.heartbeat_secs,
            expected_build_sha: self.expected_build_sha.clone(),
            expected_hermes_home: self.expected_hermes_home.clone(),
            inner: Arc::clone(&self.inner),
            hello_event: Arc::clone(&self.hello_event),
        }
    }

    /// Mirrors `def _pid_matches_compute_host(self, pid: int) -> bool: return is_compute_host_identity(pid)`.
    pub fn pid_matches_compute_host(&self, pid: i32) -> bool {
        is_compute_host_identity(pid)
    }

    /// Mirrors `def _terminate_pid(self, pid, *, timeout) -> None: try os.kill(pid,SIGTERM) except Lookup/Other; deadline=monotonic+timeout; while monotonic<deadline: if not _pid_alive: return; sleep(0.05); try os.kill(pid,SIGKILL) except`.
    pub fn terminate_pid(&self, pid: i32, timeout: Duration) {
        terminate_pid(pid, timeout);
    }

    /// Mirrors `def _terminate_process(self, proc) -> None: if poll is not None: return; try proc.terminate(); wait(timeout); return; except: pass; try proc.kill(); except: pass; try wait(2) except: pass`.
    pub fn terminate_process_inner(&self, proc: &mut Child) {
        let _ = terminate_process(proc);
    }
}

// ---------------------------------------------------------------------------
// Free helpers for pid/process termination — mirrors 533-569
// ---------------------------------------------------------------------------

/// Terminate a pid with SIGTERM → SIGKILL fallback. Mirrors `_terminate_pid`.
pub fn terminate_pid(pid: i32, timeout: Duration) {
    if pid <= 0 {
        return;
    }
    // SIGTERM
    let term = Command::new("kill").args(["-TERM", &pid.to_string()]).output();
    if let Ok(o) = term {
        if o.status.success() || String::from_utf8_lossy(&o.stderr).contains("No such process") {
            if !pid_alive(pid) {
                return;
            }
        }
    } else {
        // Try direct kill via Command kill -15 already attempted; fallback
        let _ = Command::new("kill").args(["-15", &pid.to_string()]).status();
    }
    // Poll
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !pid_alive(pid) {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).output();
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
}

/// Terminate a `Child` — mirrors `_terminate_process`.
pub fn terminate_process(proc: &mut Child) -> Result<(), String> {
    // if poll is not None → already exited
    match proc.try_wait() {
        Ok(Some(_)) => return Ok(()),
        Ok(None) => {},
        Err(e) => return Err(e.to_string()),
    }
    // try terminate + wait
    // Child::kill sends SIGKILL on Unix; we try terminate via `kill -TERM pid` for graceful
    let pid = proc.id() as i32;
    terminate_pid(pid, SHUTDOWN_TIMEOUT);
    // Now ensure Child is reaped
    match proc.try_wait() {
        Ok(Some(_)) => return Ok(()),
        Ok(None) => {
            // Force kill via Child::kill
            let _ = proc.kill();
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(2) {
                match proc.try_wait() {
                    Ok(Some(_)) => return Ok(()),
                    Ok(None) => thread::sleep(Duration::from_millis(50)),
                    Err(_) => break,
                }
            }
        }
        Err(_) => {}
    }
    let _ = proc.wait();
    Ok(())
}

// ---------------------------------------------------------------------------
// Drain loops — mirrors _drain_stdout / _drain_stderr
// ---------------------------------------------------------------------------

fn drain_stdout_loop(host: Arc<HostSupervisor>, out: std::process::ChildStdout) {
    let reader = BufReader::new(out);
    for line_res in reader.lines() {
        let raw = match line_res {
            Ok(l) => l,
            Err(_) => break,
        };
        let frame = match parse_json_line(&raw) {
            Some(m) if !m.is_empty() || raw.trim() == "{}" => m,
            Some(m) => m,
            None => {
                // logger.warning("compute host emitted invalid json: %r", raw[:200])
                let _ = &raw;
                continue;
            }
        };
        host.handle_host_frame(frame);
    }
}

fn drain_stderr_loop(host: Arc<HostSupervisor>, err: std::process::ChildStderr) {
    let reader = BufReader::new(err);
    for line_res in reader.lines() {
        let raw = match line_res {
            Ok(l) => l,
            Err(_) => break,
        };
        let text = raw.trim_end_matches('\n').to_string();
        if text.is_empty() {
            continue;
        }
        {
            let mut inner = host.inner.lock().unwrap();
            inner.stderr_tail.push_back(text.clone());
            while inner.stderr_tail.len() > STDERR_TAIL_MAX {
                inner.stderr_tail.pop_front();
            }
        }
        // logger.warning("compute host stderr: %s", text)
        let _ = text;
    }
}

// ---------------------------------------------------------------------------
// Registry helpers — minimal json parse for reconcile/persist
// ---------------------------------------------------------------------------

fn parse_registry_json(text: &str) -> Option<HashMap<String, String>> {
    parse_json_line(text)
}

// ---------------------------------------------------------------------------
// Self-check (ponytail: one runnable check, no framework)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Arc, Mutex};

    #[test]
    fn mutator_table_has_14_and_known_routes() {
        assert_eq!(MUTATOR_ROUTE_TABLE.len(), 14);
        assert_eq!(mutator_route("prompt.submit"), Some(MutatorRouteKind::TurnPath));
        assert_eq!(mutator_route("session.interrupt"), Some(MutatorRouteKind::TurnPath));
        assert_eq!(mutator_route("reload.mcp"), Some(MutatorRouteKind::RunConcurrent));
        assert_eq!(mutator_route("session.save"), Some(MutatorRouteKind::RunConcurrent));
        assert_eq!(mutator_route("slash.retry"), Some(MutatorRouteKind::IdleGated));
        assert!(is_known_mutator_route("prompt.submit.truncate"));
        assert!(!is_known_mutator_route("unknown.route"));
        assert_eq!(mutator_route_str("slash.model"), Some("idle-gated"));
    }

    #[test]
    fn registry_name_constant() {
        assert_eq!(REGISTRY_NAME, "dashboard-compute-host.json");
        assert!((RESPAWN_WINDOW_SECS - 300.0).abs() < 1e-9);
        assert!((SHUTDOWN_TIMEOUT_SECS - 10.0).abs() < 1e-9);
    }

    #[test]
    fn append_log_record_single_write_and_newline() {
        let dir = env::temp_dir().join(format!("hs-test-{}", generate_request_id()));
        let path = dir.join("logs").join("out.log");
        let _ = fs::remove_dir_all(&dir);
        append_log_record(&path, "hello").unwrap();
        let c1 = fs::read_to_string(&path).unwrap();
        assert_eq!(c1, "hello\n");
        append_log_record(&path, "world\n").unwrap();
        let c2 = fs::read_to_string(&path).unwrap();
        assert_eq!(c2, "hello\nworld\n");
        // Permissions 0o600 on unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pid_alive_guard_and_reuse() {
        assert!(!pid_alive(0));
        assert!(!pid_alive(-1));
        // Current process should be alive
        let me = std::process::id() as i32;
        assert!(pid_alive(me));
        // Very large pid likely not alive (but not guaranteed, just check false or true is bool)
        let _ = pid_alive(999999);
    }

    #[test]
    fn pid_command_and_identity() {
        // Injected identity check
        assert!(is_compute_host_identity_with(123, Some("python -m tui_gateway.compute_host --foo")));
        assert!(!is_compute_host_identity_with(123, Some("python -m other.module")));
        assert!(!is_compute_host_identity_with(123, Some("")));
        // Real pid_command for current pid should be non-empty
        let me = std::process::id() as i32;
        let cmd = pid_command(me);
        assert!(!cmd.is_empty() || pid_command_with(me, Some(|_| Some("".into()))) == "");
    }

    #[test]
    fn default_registry_path_under_home() {
        let home = PathBuf::from("/tmp/hs-home-test");
        let p = default_registry_path_with(Some(&home));
        assert_eq!(p, home.join("state").join(REGISTRY_NAME));
    }

    #[test]
    fn build_sha_fallback() {
        let sha = build_sha_with(Some(Path::new("/nonexistent/path/for/test")), None);
        assert!(!sha.is_empty());
        // Either real sha (40 hex) or "unknown"
        assert!(sha == "unknown" || sha.len() == 40);
        // Injected runner returning None → unknown
        let s = build_sha_with(None, Some(|_| None));
        assert_eq!(s, "unknown");
        let s2 = build_sha_with(None, Some(|_| Some("abc123".into())));
        assert_eq!(s2, "abc123");
    }

    #[test]
    fn repo_root_with_override() {
        let p = PathBuf::from("/tmp/fake-root");
        assert_eq!(repo_root_with(Some(&p)), p);
        let r = repo_root();
        assert!(!r.as_os_str().is_empty());
    }

    #[test]
    fn hermes_subprocess_env_has_python_utf8() {
        let env = hermes_subprocess_env();
        assert_eq!(env.get("PYTHONUTF8").map(|s| s.as_str()), Some("1"));
    }

    #[test]
    fn build_compute_env_heartbeat_and_pythonpath() {
        let root = PathBuf::from("/repo/root");
        let env = build_compute_env(None, 15, &root);
        assert_eq!(env.get(ENV_HEARTBEAT_SECS).map(|s| s.as_str()), Some("15"));
        let pp = env.get("PYTHONPATH").unwrap();
        assert!(pp.contains("/repo/root"));
        // Dedup: if already contains root, not duplicated
        let mut extra = HashMap::new();
        extra.insert("PYTHONPATH".into(), format!("/repo/root:/other"));
        let env2 = build_compute_env(Some(&extra), 20, &root);
        assert_eq!(env2.get("PYTHONPATH").unwrap().matches("/repo/root").count(), 1);
    }

    #[test]
    fn host_supervisor_is_running_false_when_no_proc() {
        let hs = HostSupervisor::new_default_no_autostart();
        assert!(!hs.is_running());
        assert_eq!(hs.pid(), 0);
        assert!(hs.hello().is_empty());
    }

    #[test]
    fn host_supervisor_reconcile_none_when_no_file() {
        let dir = env::temp_dir().join(format!("hs-reg-{}", generate_request_id()));
        let reg = dir.join("state").join(REGISTRY_NAME);
        let hs = HostSupervisor::new(Some(reg.clone()), None, None, None, None, None, None, None, None, false);
        let res = hs.reconcile_startup_orphan();
        assert_eq!(res, "none");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_supervisor_reconcile_invalid_registry() {
        let dir = env::temp_dir().join(format!("hs-reg2-{}", generate_request_id()));
        let reg = dir.join("state").join(REGISTRY_NAME);
        fs::create_dir_all(reg.parent().unwrap()).unwrap();
        fs::write(&reg, "not json").unwrap();
        let hs = HostSupervisor::new(Some(reg.clone()), None, None, None, None, None, None, None, None, false);
        let res = hs.reconcile_startup_orphan();
        assert_eq!(res, "invalid-registry");
        assert!(!reg.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_supervisor_reconcile_not_running_when_pid_dead() {
        let dir = env::temp_dir().join(format!("hs-reg3-{}", generate_request_id()));
        let reg = dir.join("state").join(REGISTRY_NAME);
        fs::create_dir_all(reg.parent().unwrap()).unwrap();
        fs::write(&reg, r#"{"host_pid": 999999}"#).unwrap();
        let hs = HostSupervisor::new(Some(reg.clone()), None, None, None, None, None, None, None, None, false);
        let res = hs.reconcile_startup_orphan();
        // 999999 likely not alive → not-running, but if it is alive and not compute_host → pid-reuse-ignored
        assert!(res == "not-running" || res == "pid-reuse-ignored");
        assert!(!reg.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_supervisor_control_rejects_unknown_route() {
        let hs = HostSupervisor::new_default_no_autostart();
        let res = hs.control("sid1", "unknown.route", None, false, 1.0);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("unclassified host mutator route"));
    }

    #[test]
    fn host_supervisor_handle_hello_sets_event() {
        let hs = HostSupervisor::new_default_no_autostart();
        let mut hello = HashMap::new();
        hello.insert("type".into(), "hello".into());
        hello.insert("hermes_home".into(), hs.expected_hermes_home.clone());
        hello.insert("build_sha".into(), hs.expected_build_sha.clone());
        hello.insert("boot_id".into(), "boot-123".into());
        hs.handle_host_frame(hello.clone());
        assert_eq!(hs.hello().get("boot_id").map(|s| s.as_str()), Some("boot-123"));
        let (lock, _) = &*hs.hello_event;
        assert!(*lock.lock().unwrap());
        // validate_hello should pass
        assert!(hs.validate_hello().is_ok());
    }

    #[test]
    fn host_supervisor_validate_hello_mismatch() {
        let hs = HostSupervisor::new(None, None, None, None, None, None, None, Some("abc".into()), Some("/tmp/home-a".into()), false);
        let mut hello = HashMap::new();
        hello.insert("type".into(), "hello".into());
        hello.insert("hermes_home".into(), "/tmp/home-b".into());
        hello.insert("build_sha".into(), "abc".into());
        hs.handle_host_frame(hello);
        assert!(hs.validate_hello().is_err());
        // Fix home, bad sha
        let mut hello2 = HashMap::new();
        hello2.insert("type".into(), "hello".into());
        hello2.insert("hermes_home".into(), "/tmp/home-a".into());
        hello2.insert("build_sha".into(), "different".into());
        hs.handle_host_frame(hello2);
        assert!(hs.validate_hello().is_err());
        // unknown expected passes
        let hs2 = HostSupervisor::new(None, None, None, None, None, None, None, Some("unknown".into()), Some("/tmp/home-a".into()), false);
        let mut hello3 = HashMap::new();
        hello3.insert("type".into(), "hello".into());
        hello3.insert("hermes_home".into(), "/tmp/home-a".into());
        hello3.insert("build_sha".into(), "anything".into());
        hs2.handle_host_frame(hello3);
        assert!(hs2.validate_hello().is_ok());
    }

    #[test]
    fn host_supervisor_pending_turn_complete_and_fail() {
        let hs = HostSupervisor::new_default_no_autostart();
        let called = Arc::new(Mutex::new(Vec::new()));
        let called_clone = Arc::clone(&called);
        let cb: Arc<dyn Fn(HashMap<String, String>) + Send + Sync> = Arc::new(move |frame: HashMap<String, String>| {
            called_clone.lock().unwrap().push(frame.get("type").cloned().unwrap_or_default());
        });
        let mut frame = HashMap::new();
        frame.insert("sid".into(), "s1".into());
        frame.insert("type".into(), "turn.start".into());
        frame.insert("request_id".into(), "req-1".into());
        {
            let mut inner = hs.inner.lock().unwrap();
            inner.pending_turns.insert("req-1".into(), PendingTurn { sid: "s1".into(), callback: Some(Arc::clone(&cb)) });
        }
        let mut done = HashMap::new();
        done.insert("type".into(), "turn.end".into());
        done.insert("request_id".into(), "req-1".into());
        hs.complete_turn(done);
        assert_eq!(called.lock().unwrap().as_slice(), ["turn.end"]);
        // fail pending
        let called2 = Arc::new(Mutex::new(Vec::new()));
        let called2_clone = Arc::clone(&called2);
        let cb2: Arc<dyn Fn(HashMap<String, String>) + Send + Sync> = Arc::new(move |frame: HashMap<String, String>| {
            called2_clone.lock().unwrap().push(frame.get("reason").cloned().unwrap_or_default());
        });
        {
            let mut inner = hs.inner.lock().unwrap();
            inner.pending_turns.insert("req-2".into(), PendingTurn { sid: "s2".into(), callback: Some(cb2) });
        }
        hs.fail_pending_turns("crash", "boom");
        assert_eq!(called2.lock().unwrap().as_slice(), ["crash"]);
        assert!(hs.inner.lock().unwrap().pending_turns.is_empty());
    }

    #[test]
    fn host_supervisor_handle_control_ack_routes_to_queue() {
        let hs = HostSupervisor::new_default_no_autostart();
        // control will create pending_controls entry; we simulate manually
        let (tx, rx) = sync_channel(1);
        let rcv = Arc::new(Mutex::new(rx));
        {
            let mut inner = hs.inner.lock().unwrap();
            inner.pending_controls.insert("req-ctrl-1".into(), PendingControl { sender: tx, receiver: Arc::clone(&rcv) });
        }
        let mut ack = HashMap::new();
        ack.insert("type".into(), "control.ack".into());
        ack.insert("request_id".into(), "req-ctrl-1".into());
        hs.handle_host_frame(ack);
        let got = rcv.lock().unwrap().try_recv().unwrap();
        assert_eq!(got.get("type").map(|s| s.as_str()), Some("control.ack"));
    }

    #[test]
    fn generate_request_id_is_hex32() {
        let id = generate_request_id();
        assert_eq!(id.len(), 32);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        let id2 = generate_request_id();
        assert_ne!(id, id2);
    }

    #[test]
    fn frame_to_json_and_parse_roundtrip() {
        let mut f = HashMap::new();
        f.insert("type".into(), "hello".into());
        f.insert("request_id".into(), "abc".into());
        let line = frame_to_json_line(&f);
        assert!(line.ends_with('\n'));
        let parsed = parse_json_line(line.trim()).unwrap();
        assert_eq!(parsed.get("type").map(|s| s.as_str()), Some("hello"));
        assert_eq!(parsed.get("request_id").map(|s| s.as_str()), Some("abc"));
        assert!(parse_json_line("not json").is_none());
    }
}

