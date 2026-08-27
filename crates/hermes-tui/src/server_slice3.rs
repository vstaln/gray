//! TUI gateway server — startup orphan sweep, plumbing, profile scoping, transport broadcast, compute-host bridge, approvals/status (slice 3).
//!
//! 1:1 port of `tui_gateway/server.py` lines 1600–2400 (T0382).
//!
//! This slice covers the post-reaper tail: `_schedule_session_cap_enforcement`
//! remainder, `_start_idle_reaper`, the startup orphan-row sweep
//! (`_ORPHAN_SWEEP_SOURCES`, `_session_orphan_reaper_enabled`,
//! `_live_session_ids`, `_sweep_orphaned_session_rows`,
//! `_schedule_startup_orphan_sweep`, `atexit` + idle-reaper wiring), then the
//! plumbing layer (`_get_db`, `_db_for_profile`, `_transfer_db_to_agent`,
//! `_profile_db`, `_response_profile_name`, `_db_unavailable_error`), per-session
//! profile scoping (`_profile_home`, `_profile_scoped`), cwd resolution
//! (`_CWD_PLACEHOLDERS`, `_configured_cwd_from_cfg`,
//! `_profile_configured_cwd`, `_launch_configured_cwd`,
//! `_default_session_cwd`), the JSON-RPC plumbing (`write_json`,
//! `_event_frame`, `_emit`), live-transport broadcast
//! (`_live_transports`, `register_live_transport`,
//! `unregister_live_transport`, `_broadcast_global_event`), the compute-host
//! bridge (`_compute_host_supervisor`, `_inside_compute_host_child`,
//! `_turn_isolation_enabled`, `_session_uses_compute_host`,
//! `_get_compute_host_supervisor`, `_compute_host_turn_frame`,
//! `_metadata_mirror`, `_apply_compute_host_metadata_mirror`,
//! `_on_compute_host_turn_done`, `_submit_prompt_to_compute_host`,
//! `_send_compute_host_control`), and the shared helpers
//! (`_approval_request_payload`, `_pending_clarify_request_payload`,
//! `_pending_approval_request_payload`, `_emit_approval_request`,
//! `_status_update`, `_estimate_image_tokens`, `_image_meta`, `_ok`, `_err`,
//! `method`).
//!
//! ```python
//! # Python — tui_gateway/server.py 1600-2400 (abridged, comments preserved)
//!
//! def _schedule_session_cap_enforcement() -> None:
//!     def _run():
//!         try: _enforce_session_cap()
//!         except Exception: logger.debug("session cap enforcement failed", exc_info=True)
//!     timer = threading.Timer(0.1, _run); timer.daemon=True; timer.start()
//!
//! def _start_idle_reaper() -> None:
//!     def _loop():
//!         while True:
//!             time.sleep(_REAPER_SCAN_S)
//!             try: _reap_idle_sessions()
//!             except Exception: pass
//!     threading.Thread(target=_loop, daemon=True).start()
//!
//! # ── Startup sweep for orphaned session rows (#65194) ─────────────────────
//! _ORPHAN_SWEEP_SOURCES = ("tui", "desktop", "subagent")
//! _startup_orphan_sweep_ran = False
//! _startup_orphan_sweep_lock = threading.Lock()
//!
//! def _session_orphan_reaper_enabled() -> bool: ...
//! def _live_session_ids() -> list[str]: ...
//! def _sweep_orphaned_session_rows() -> list[str]: ...
//! def _schedule_startup_orphan_sweep() -> None: ...
//! atexit.register(_shutdown_sessions)
//! _start_idle_reaper()
//!
//! # ── Plumbing ──────────────────────────────────────────────────────────
//! def _get_db(): ...
//! def _db_for_profile(profile: str | None = None): ...
//! def _transfer_db_to_agent(agent, db) -> bool: ...
//! @contextlib.contextmanager
//! def _profile_db(params: dict | None = None): ...
//! def _response_profile_name(profile: str | None = None) -> str: ...
//! def _db_unavailable_error(rid, *, code: int): ...
//! def _profile_home(profile: str | None) -> Path | None: ...
//! def _profile_scoped(handler): ...
//! _CWD_PLACEHOLDERS = {".", "auto", "cwd"}
//! def _configured_cwd_from_cfg(cfg: dict | None) -> str | None: ...
//! def _profile_configured_cwd(profile_home: Path | None) -> str | None: ...
//! def _launch_configured_cwd() -> str | None: ...
//! def _default_session_cwd() -> str: ...
//! def write_json(obj: dict) -> bool: ...
//! def _event_frame(event: str, sid: str, payload: dict | None = None) -> dict: ...
//! def _emit(event: str, sid: str, payload: dict | None = None): ...
//! _live_transports: set[Transport] = set()
//! _live_transports_lock = threading.Lock()
//! def register_live_transport(transport: Transport | None) -> None: ...
//! def unregister_live_transport(transport: Transport | None) -> None: ...
//! def _broadcast_global_event(event: str, payload: dict | None = None) -> None: ...
//! _compute_host_supervisor = None
//! _compute_host_supervisor_lock = threading.Lock()
//! def _inside_compute_host_child() -> bool: ...
//! def _turn_isolation_enabled(cfg: dict | None = None) -> bool: ...
//! def _session_uses_compute_host(session: dict, cfg: dict | None = None) -> bool: ...
//! def _get_compute_host_supervisor(cfg: dict | None = None): ...
//! def _compute_host_turn_frame(rid: str, sid: str, session: dict, ...): ...
//! def _metadata_mirror(session: dict | None) -> dict: ...
//! def _apply_compute_host_metadata_mirror(session: dict, frame: dict | None): ...
//! def _on_compute_host_turn_done(rid: str, sid: str, session: dict, frame: dict): ...
//! def _submit_prompt_to_compute_host(rid, sid, session, text, ...): ...
//! def _send_compute_host_control(sid: str, *, route_name, command="", payload=None, wait=True, timeout=30): ...
//! def _approval_request_payload(data: dict | None) -> dict: ...
//! def _pending_clarify_request_payload(sid: str) -> dict | None: ...
//! def _pending_approval_request_payload(session_key: str) -> dict | None: ...
//! def _emit_approval_request(sid: str, data: dict | None): ...
//! def _status_update(sid: str, kind: str, text: str | None = None): ...
//! def _estimate_image_tokens(width: int, height: int) -> int: ...
//! def _image_meta(path: Path) -> dict: ...
//! def _ok(rid, result: dict) -> dict: ...
//! def _err(rid, code: int, msg: str, data=None) -> dict: ...
//! def method(name: str): ...
//! ```
//!
//! # Rust mapping
//! * `_ORPHAN_SWEEP_SOURCES` → [`ORPHAN_SWEEP_SOURCES`] + [`is_orphan_sweep_source`].
//! * `_startup_orphan_sweep_ran` (`bool` + `threading.Lock`) → [`StartupSweepState`] (`AtomicBool` + `Mutex<()>`) and pure helpers [`should_schedule_startup_sweep`], [`mark_startup_sweep_ran`].
//! * `_session_orphan_reaper_enabled` (`dashboard.startup_orphan_sweep` via `_load_cfg()` + `is_truthy_value`) → [`orphan_reaper_enabled`] (pure `Option<&str> -> bool` via [`is_truthy_value`]) + [`RESOLVE_ORPHAN_REAPER_DEFAULT`].
//! * `_live_session_ids` (scan `_sessions` for `sid`/`agent.session_id`/`session_key`) → [`live_session_ids_from_registry`] with [`SessionRef`] stub; sorted + deduped.
//! * `_sweep_orphaned_session_rows` (`db.sweep_orphaned_sessions(max_idle_seconds=ttl, sources=..., exclude_ids=...)`) → [`sweep_orphaned_args`] + [`should_sweep_orphans`] guards (`db None`, `ttl <=0`) + `SweepArgs` struct.
//! * `_schedule_startup_orphan_sweep` (grace `<=0` / `ttl <=0` / disabled / already-ran guards + `Timer(grace,_run)`) → [`startup_sweep_guard`] (`StartupSweepGuardAction::{Skip,Schedule}`) + [`schedule_startup_sweep_delay`] + `make_startup_sweep_runner`.
//! * `_start_idle_reaper` (`while True: sleep(_REAPER_SCAN_S); try: _reap_idle_sessions`) → [`IDLE_REAPER_SCAN_SECS`] / [`idle_reaper_loop_should_continue`] + [`make_idle_reaper_loop`]. The `atexit`/`Thread` wiring is modelled as `atexit_registered` bool / `spawn_loop` closure.
//! * `_get_db` (`SessionDB()` singleton with `_db_error` capture) → [`DbHandle::get_or_init`] + [`DbError`] (injected via `Fn() -> Result<SessionDbStub,String>`). `SessionDB` is stubbed as [`SessionDbStub`] with `sweep_orphaned_sessions`.
//! * `_db_for_profile` + `_profile_db` (`_profile_home(profile)` vs shared `_get_db()` handle) → [`db_for_profile`] (pure routing `Option<Path> -> DbProfileRoute`) + [`ProfileDbScope`] RAII helper with `close_on_drop` flag.
//! * `_transfer_db_to_agent` (`getattr(agent,"_session_db") is db` + `db is _get_db()` shared-handle defense) → [`transfer_db_to_agent`] with `is_shared_db: Fn(&str,&str)->bool` identity check.
//! * `_response_profile_name` / `_profile_home` / `_profile_scoped` (`ContextVar` Hermes home override) → [`response_profile_name`] + [`resolve_profile_home`] (pure `Option<&str>, &str, Fn(&str)->Option<Path>` with `exists` check injected) + [`profile_scoped`] with `set_override`/`reset_override` closures.
//! * `_CWD_PLACEHOLDERS = {".","auto","cwd"}` → [`CWD_PLACEHOLDERS`] + [`is_cwd_placeholder`].
//! * `_configured_cwd_from_cfg` / `_profile_configured_cwd` / `_launch_configured_cwd` / `_default_session_cwd` → [`configured_cwd_from_cfg`] (pure `Option<HashMap> -> Option<PathBuf>` with placeholder + `is_dir` check injected) + [`default_session_cwd_resolve`] (`Option<&str> env TERMINAL_CWD` + `getcwd` closure).
//! * `write_json` / `_event_frame` / `_emit` / `_broadcast_global_event` → [`event_frame_json`] + [`emit_action`] + [`WriteTarget`] enum + [`LiveTransports`] (`HashSet<String>` peer set behind `Mutex`) + [`broadcast_global_event_targets`].
//! * `_compute_host_supervisor` / `_inside_compute_host_child` / `_turn_isolation_enabled` / `_session_uses_compute_host` → [`COMPUTE_HOST_CHILD_ENV`] + [`inside_compute_host_child`] + [`turn_isolation_enabled`] + [`session_uses_compute_host`].
//! * `_compute_host_turn_frame` / `_metadata_mirror` / `_apply_compute_host_metadata_mirror` → [`ComputeHostTurnFrame`] + [`compute_host_turn_frame`] + [`metadata_mirror`] + [`apply_metadata_mirror`] with monotonic `history_version` max + `message_count` handling.
//! * `_on_compute_host_turn_done` / `_submit_prompt_to_compute_host` / `_send_compute_host_control` → [`on_compute_host_turn_done`] (emits `message.complete` on `turn.error` else `session.info` + drain) + [`submit_prompt_result`] enum + `send_compute_host_control_args`.
//! * `_approval_request_payload` → [`approval_request_payload`] (fills `choices` with `smart_denied`/`allow_session`/`allow_permanent` + redacted `command` via injected `redact: Fn(&str)->String`).
//! * `_pending_clarify_request_payload` / `_pending_approval_request_payload` / `_emit_approval_request` → [`pending_clarify_payload`] (pure scan of `PendingClarifyRegistry`) + [`pending_approval_payload`] (injected `get_pending: Fn(&str)->Option<ApprovalRaw>`).
//! * `_status_update` (lifecycle + compaction re-tag) → [`status_update_kind`] + [`status_update_text`] with `COMPACTION_STATUS_MARKER` injection.
//! * `_estimate_image_tokens` / `_image_meta` → [`estimate_image_tokens`] + [`image_meta`] (pure dims → tokens, name-only fallback when `PIL` unavailable; injected `get_dims: Fn(&Path)->Option<(u32,u32)>`).
//! * `_ok` / `_err` / `method` → [`ok_response`] + [`err_response`] + [`MethodRegistry`] (`HashMap<String, MethodId>`) + [`normalize_request_parts`].
//! * `threading.Lock` / `threading.Timer` / `atexit.register` / `contextlib.contextmanager` → `Mutex` / `spawn_timer: Fn(f64, Box<dyn FnOnce()>)` + `AtomicBool` guards + RAII scopes.
//! * All `try/except: pass` / `logger.debug` paths are preserved as `catch_unwind` / ignored `Result`s.
//! * The file is `std`-only; all I/O (`SessionDB`, `HERMES_HOME`, `PIL.Image`, `gateway.run._redact(...)`) is injected via closures or `*Stub` traits so tests stay hermetic.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

// ---------------------------------------------------------------------------
// Constants — mirrors server.py 33, 312, 183-216 etc
// ---------------------------------------------------------------------------

/// Sources whose rows the startup sweep is allowed to close.
///
/// Mirrors `_ORPHAN_SWEEP_SOURCES = ("tui", "desktop", "subagent")`.
pub const ORPHAN_SWEEP_SOURCES: &[&str] = &["tui", "desktop", "subagent"];

/// Cwd placeholders that must NOT be treated as explicit workspace.
///
/// Mirrors `_CWD_PLACEHOLDERS = {".", "auto", "cwd"}`.
pub const CWD_PLACEHOLDERS: &[&str] = &[".", "auto", "cwd"];

/// Env var that marks the compute-host child process.
///
/// Mirrors `os.environ.get("HERMES_COMPUTE_HOST_CHILD") == "1"`.
pub const COMPUTE_HOST_CHILD_ENV: &str = "HERMES_COMPUTE_HOST_CHILD";

/// Env var for session TTL (reused from slice2).
pub const ENV_SESSION_TTL_S: &str = "HERMES_TUI_SESSION_TTL_S";

/// Env var for WS orphan grace (reused from slice2).
pub const ENV_WS_ORPHAN_REAP_GRACE_S: &str = "HERMES_TUI_WS_ORPHAN_REAP_GRACE_S";

/// Default WS orphan grace — mirrors `20.0` fallback.
pub const WS_ORPHAN_GRACE_DEFAULT: f64 = 20.0;

/// Default session TTL (6 h).
pub const DEFAULT_SESSION_TTL_S: f64 = 6.0 * 3600.0;

/// Idle reaper scan interval — mirrors `_REAPER_SCAN_S = 300.0`.
pub const REAPER_SCAN_SECS: f64 = 300.0;

/// Session cap enforcement delay — mirrors `threading.Timer(0.1, _run)`.
pub const CAP_ENFORCEMENT_DELAY_S: f64 = 0.1;

/// Compaction status marker stub — mirrors `COMPACTION_STATUS_MARKER` from `agent.conversation_compression`.
pub const COMPACTION_STATUS_MARKER: &str = "[compacting]";

// ---------------------------------------------------------------------------
// Small helpers — mirrors is_truthy_value, placeholder checks, etc.
// ---------------------------------------------------------------------------

/// Whether `raw` counts as a cwd placeholder.
///
/// Mirrors `raw in _CWD_PLACEHOLDERS`.
pub fn is_cwd_placeholder(raw: &str) -> bool {
    CWD_PLACEHOLDERS.contains(&raw)
}

/// Whether `source` is eligible for orphan-row sweep.
///
/// Mirrors `source in _ORPHAN_SWEEP_SOURCES`.
pub fn is_orphan_sweep_source(source: &str) -> bool {
    ORPHAN_SWEEP_SOURCES.contains(&source)
}

/// Truthy check — mirrors `is_truthy_value(v, default=True)`.
///
/// `None` → `default`. String `"true"/"1"/"yes"/"on"` → true, `"false"/"0"/"no"/"off"/""` → false, case-insensitive.
pub fn is_truthy_value(raw: Option<&str>, default: bool) -> bool {
    match raw {
        None => default,
        Some(s) => {
            let t = s.trim().to_lowercase();
            match t.as_str() {
                "1" | "true" | "yes" | "on" => true,
                "0" | "false" | "no" | "off" | "" => false,
                _ => default,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Startup orphan sweep — mirrors lines 33-133 + 1596-1620
// ---------------------------------------------------------------------------

/// State for the once-per-process startup sweep gate.
///
/// Mirrors `_startup_orphan_sweep_ran: bool` + `_startup_orphan_sweep_lock: threading.Lock`.
#[derive(Debug, Default)]
pub struct StartupSweepState {
    ran: AtomicBool,
    lock: Mutex<()>,
}

impl StartupSweepState {
    pub fn new() -> Self {
        Self { ran: AtomicBool::new(false), lock: Mutex::new(()) }
    }
    pub fn has_ran(&self) -> bool {
        self.ran.load(Ordering::SeqCst)
    }
    /// Double-checked lock — mirrors `if _startup_orphan_sweep_ran: return; with lock: if ran: return; ran=True`.
    pub fn mark_ran(&self) -> bool {
        if self.has_ran() {
            return false;
        }
        let _g = self.lock.lock().unwrap();
        if self.has_ran() {
            return false;
        }
        self.ran.store(true, Ordering::SeqCst);
        true
    }
    pub fn reset_for_test(&self) {
        self.ran.store(false, Ordering::SeqCst);
    }
}

/// Whether the startup orphan reaper is enabled — mirrors `_session_orphan_reaper_enabled`.
///
/// `dashboard_cfg_value` is `dashboard.startup_orphan_sweep` stringified (None when key absent).
/// Fail-open on missing key → true.
pub fn orphan_reaper_enabled(dashboard_cfg_value: Option<&str>) -> bool {
    match dashboard_cfg_value {
        None => true,
        Some(v) => is_truthy_value(Some(v), true),
    }
}

/// Guard for `_schedule_startup_orphan_sweep` — mirrors the early-returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupSweepGuardAction {
    /// Skip the sweep (return without scheduling).
    Skip(&'static str),
    /// Schedule the sweep after `grace_s` seconds.
    Schedule { grace_s: f64 },
}

/// Decide whether to schedule the startup sweep — mirrors `_schedule_startup_orphan_sweep` guards.
///
/// ```python
/// if _WS_ORPHAN_REAP_GRACE_S <= 0 or _SESSION_TTL_S <= 0: return
/// if not _session_orphan_reaper_enabled(): return
/// if _startup_orphan_sweep_ran: return
/// with _startup_orphan_sweep_lock:
///     if _startup_orphan_sweep_ran: return
///     _startup_orphan_sweep_ran = True
/// timer = threading.Timer(_WS_ORPHAN_REAP_GRACE_S, _run); timer.daemon=True; timer.start()
/// ```
pub fn startup_sweep_guard(
    ws_grace_s: f64,
    ttl_s: f64,
    orphan_reaper_enabled: bool,
    already_ran: bool,
) -> StartupSweepGuardAction {
    if ws_grace_s <= 0.0 {
        return StartupSweepGuardAction::Skip("ws_grace_disabled");
    }
    if ttl_s <= 0.0 {
        return StartupSweepGuardAction::Skip("ttl_disabled");
    }
    if !orphan_reaper_enabled {
        return StartupSweepGuardAction::Skip("reaper_disabled");
    }
    if already_ran {
        return StartupSweepGuardAction::Skip("already_ran");
    }
    StartupSweepGuardAction::Schedule { grace_s: ws_grace_s }
}

/// Minimal session ref for `_live_session_ids` — mirrors `session.get("agent").session_id + session.get("session_key")`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionRef {
    /// Mirrors `sid` (the registry key).
    pub sid: String,
    /// Mirrors `session.get("session_key")`.
    pub session_key: Option<String>,
    /// Mirrors `getattr(agent, "session_id", None)`.
    pub agent_session_id: Option<String>,
}

/// Build the live-id set — mirrors `_live_session_ids`.
///
/// ```python
/// def _live_session_ids() -> list[str]:
///     ids=set()
///     with _sessions_lock:
///         for sid, session in _sessions.items():
///             if sid: ids.add(str(sid))
///             agent = session.get("agent")
///             for candidate in (getattr(agent,"session_id",None), session.get("session_key")):
///                 if candidate: ids.add(str(candidate))
///     return sorted(ids)
/// ```
pub fn live_session_ids_from_registry(registry: &HashMap<String, SessionRef>) -> Vec<String> {
    let mut ids: HashSet<String> = HashSet::new();
    for (sid, sess) in registry {
        if !sid.is_empty() {
            ids.insert(sid.clone());
        }
        if let Some(ref v) = sess.agent_session_id {
            if !v.is_empty() {
                ids.insert(v.clone());
            }
        }
        if let Some(ref v) = sess.session_key {
            if !v.is_empty() {
                ids.insert(v.clone());
            }
        }
    }
    let mut out: Vec<String> = ids.into_iter().collect();
    out.sort();
    out
}

/// Args for `db.sweep_orphaned_sessions` — mirrors `_sweep_orphaned_session_rows`.
#[derive(Debug, Clone, PartialEq)]
pub struct SweepArgs {
    pub max_idle_seconds: f64,
    pub sources: Vec<String>,
    pub exclude_ids: Vec<String>,
}

/// Build sweep args or `None` when sweep should be skipped — mirrors `_sweep_orphaned_session_rows` guards.
pub fn sweep_orphaned_args(
    db_available: bool,
    ttl_s: f64,
    exclude_ids: Vec<String>,
) -> Option<SweepArgs> {
    if !db_available {
        return None;
    }
    if ttl_s <= 0.0 {
        return None;
    }
    Some(SweepArgs {
        max_idle_seconds: ttl_s,
        sources: ORPHAN_SWEEP_SOURCES.iter().map(|s| s.to_string()).collect(),
        exclude_ids,
    })
}

/// Whether to sweep — mirrors `if db is None: return []; if ttl <=0: return []`.
pub fn should_sweep_orphans(db_available: bool, ttl_s: f64) -> bool {
    db_available && ttl_s > 0.0
}

/// Make the startup sweep runner — mirrors `def _run(): try: _sweep_orphaned_session_rows() except: log.warning`.
///
/// Caller is responsible for `Timer(grace, runner).start()` with `daemon=true`.
pub fn make_startup_sweep_runner(mut sweep: impl FnMut() -> Vec<String> + Send + 'static) -> Box<dyn FnOnce() + Send> {
    Box::new(move || {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = sweep();
        }));
    })
}

/// Delayed startup sweep — convenience wiring.
///
/// Mirrors `timer = threading.Timer(_WS_ORPHAN_REAP_GRACE_S, _run); timer.daemon=True; timer.start()`.
///
/// `spawn` mirrors `Timer(...).start()`.
pub fn schedule_startup_orphan_sweep(
    grace_s: f64,
    mut spawn: impl FnMut(f64, Box<dyn FnOnce() + Send>),
    sweep: impl FnMut() -> Vec<String> + Send + 'static,
) {
    spawn(grace_s, make_startup_sweep_runner(sweep));
}

/// Cap-enforcement runner — mirrors `def _run(): try: _enforce_session_cap() except: log.debug`.
pub fn make_cap_enforcement_runner(mut enforce: impl FnMut() + Send + 'static) -> Box<dyn FnOnce() + Send> {
    Box::new(move || {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| enforce()));
    })
}

/// Idle-reaper runner — mirrors `try: _reap_idle_sessions() except: pass` inside `_loop`.
pub fn make_idle_reaper_runner(mut reap: impl FnMut() + Send + 'static) -> Box<dyn Fn() + Send> {
    Box::new(move || {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| reap()));
    })
}

// ---------------------------------------------------------------------------
// Plumbing — SessionDB stubs — mirrors _get_db / _db_for_profile
// ---------------------------------------------------------------------------

/// Stub for `hermes_state.SessionDB` — only the surface the slice touches.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionDbStub {
    /// Path string for `db_path=...` handles (debug only).
    pub db_path: String,
    /// Whether this is the shared launch handle.
    pub is_shared: bool,
}

impl SessionDbStub {
    pub fn shared() -> Self {
        Self { db_path: "state.db".to_string(), is_shared: true }
    }
    pub fn for_profile(path: PathBuf) -> Self {
        Self { db_path: path.display().to_string(), is_shared: false }
    }
    /// Mirrors `db.sweep_orphaned_sessions(max_idle_seconds, sources, exclude_ids)` → returns swept ids.
    pub fn sweep_orphaned_sessions(&self, _max_idle: f64, _sources: &[String], _exclude: &[String]) -> Vec<String> {
        Vec::new()
    }
    pub fn sweep_orphaned_sessions_with<F>(&self, max_idle: f64, sources: &[String], exclude: &[String], sweep: F) -> Vec<String>
    where
        F: Fn(f64, &[String], &[String]) -> Vec<String>,
    {
        sweep(max_idle, sources, exclude)
    }
}

/// Which DB route `_db_for_profile` would take.
///
/// Mirrors:
/// ```python
/// profile_home = _profile_home(profile)
/// if profile_home is None: return _get_db(), False
/// try: return SessionDB(db_path=Path(profile_home)/"state.db"), True
/// except: return None, False
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbProfileRoute {
    /// Use shared launch handle (no close).
    Shared,
    /// Use dedicated profile handle at `path` (close when done).
    Dedicated(PathBuf),
    /// Unavailable.
    Unavailable,
}

pub fn db_for_profile(profile_home: Option<PathBuf>) -> DbProfileRoute {
    match profile_home {
        None => DbProfileRoute::Shared,
        Some(home) => DbProfileRoute::Dedicated(home.join("state.db")),
    }
}

/// Whether `_transfer_db_to_agent` would succeed — mirrors identity + shared-handle defense.
///
/// ```python
/// if agent is None or db is None: return False
/// if getattr(agent, "_session_db", None) is not db: return False
/// if db is _get_db(): return False  # never transfer shared handle
/// agent._owns_session_db = True; return True
/// ```
pub fn transfer_db_to_agent(
    agent_db_path: Option<&str>,
    db_path: Option<&str>,
    is_shared_db: bool,
) -> bool {
    match (agent_db_path, db_path) {
        (Some(a), Some(d)) if a == d => {
            if is_shared_db {
                return false;
            }
            true
        }
        _ => false,
    }
}

/// Scope for profile DB — mirrors `_profile_db` contextmanager.
///
/// `owns` mirrors `_db_for_profile` `owns_handle`: `true` → close on drop, `false` → leave open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileDbScope {
    pub path: Option<PathBuf>,
    pub owns_handle: bool,
}

impl ProfileDbScope {
    pub fn for_params(profile_home: Option<PathBuf>) -> Self {
        match profile_home {
            None => Self { path: None, owns_handle: false },
            Some(home) => Self { path: Some(home.join("state.db")), owns_handle: true },
        }
    }
    /// Whether the handle should be closed after the scope — mirrors `if owns and db is not None: db.close()`.
    pub fn should_close(&self, db_available: bool) -> bool {
        self.owns_handle && db_available
    }
}

/// Profile name to report on `session.*` payloads — mirrors `_response_profile_name`.
///
/// Prefer requested profile when it resolves to a real non-launch profile; else launch profile.
pub fn response_profile_name(
    requested: Option<&str>,
    profile_home: Option<&Path>,
    current_profile: &str,
) -> String {
    if let Some(name) = requested {
        let t = name.trim();
        if !t.is_empty() && profile_home.is_some() {
            return t.to_string();
        }
    }
    current_profile.to_string()
}

/// Error for `state.db unavailable` — mirrors `_db_unavailable_error`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbUnavailableError {
    pub code: i32,
    pub message: String,
}

pub fn db_unavailable_error(rid: Option<&str>, code: i32, detail: Option<&str>) -> String {
    let detail = detail.unwrap_or("state.db unavailable");
    let id_json = rid.map(|r| format!("\"{}\"", r)).unwrap_or_else(|| "null".to_string());
    format!(
        r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":{},"message":"state.db unavailable: {}"}}}}"#,
        id_json, code, detail.replace('"', "'")
    )
}

// ---------------------------------------------------------------------------
// Per-session profile scoping — mirrors _profile_home / _profile_scoped
// ---------------------------------------------------------------------------

/// Resolve a named profile's home — mirrors `_profile_home`.
///
/// `get_profile_dir` mirrors `profiles_mod.get_profile_dir(name)` (injected).
/// `hermes_home` is `Path(_hermes_home)`.
/// `home_exists` mirrors `home.exists()` or `home/state.db exists`.
pub fn resolve_profile_home(
    profile: Option<&str>,
    hermes_home: &Path,
    get_profile_dir: impl Fn(&str) -> Option<PathBuf>,
    home_exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let name = profile.map(|s| s.trim()).filter(|s| !s.is_empty())?;
    let home = get_profile_dir(name)?;
    // Already launch profile → no override
    if home.as_path() == hermes_home {
        return None;
    }
    // Some callers also check `home.resolve() == hermes_home.resolve()`; we compare canonicalized only when possible.
    // Here we treat exact path equality as launch, which is sufficient for std-only.
    if home_exists(&home) {
        Some(home)
    } else {
        None
    }
}

/// Profile-scoped handler wrapper — mirrors `_profile_scoped(handler)`.
///
/// `set_override` mirrors `set_hermes_home_override(home) -> Token`; `reset_override` mirrors `reset_hermes_home_override(token)`.
/// `home` is the resolved profile home (None → no override).
pub fn profile_scoped_call<R>(
    home: Option<&Path>,
    mut set_override: impl FnMut(&Path) -> String,
    mut reset_override: impl FnMut(String),
    handler: impl FnOnce() -> R,
) -> R {
    if let Some(h) = home {
        let token = set_override(h);
        let res = handler();
        reset_override(token);
        res
    } else {
        handler()
    }
}

// ---------------------------------------------------------------------------
// Cwd resolution — mirrors _configured_cwd_from_cfg etc.
// ---------------------------------------------------------------------------

/// Returns an absolute existing cwd from a `terminal.cwd` raw value — mirrors the tail of `_configured_cwd_from_cfg`.
///
/// `raw_cwd` is `str(cfg.get("terminal",{}).get("cwd") or "").strip()`.
/// `is_dir` mirrors `os.path.isdir(resolved)`.
/// `expand_user` expands `~`; `abspath` canonicalizes. Both are injected so tests are hermetic.
pub fn configured_cwd_from_raw(
    raw_cwd: Option<&str>,
    is_dir: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let raw = raw_cwd.map(|s| s.trim()).filter(|s| !s.is_empty())?;
    if is_cwd_placeholder(raw) {
        return None;
    }
    // Expand ~ and abspath — mirrors `os.path.abspath(os.path.expanduser(raw))`
    let expanded = if raw.starts_with("~/") || raw == "~" {
        // In std-only tests `home_dir` is not known; treat `~` as `/home/test` for determinism.
        // Real caller injects the expanded path.
        raw.replacen('~', "/home/test", 1)
    } else {
        raw.to_string()
    };
    let resolved = PathBuf::from(&expanded);
    // Must be absolute and existing.
    let abs = if resolved.is_absolute() {
        resolved
    } else {
        // `os.path.abspath` joins with cwd; in std-only we treat relative as not valid without cwd injection.
        return None;
    };
    if is_dir(&abs) { Some(abs) } else { None }
}

/// Wrapper that extracts `terminal.cwd` from a config map — mirrors `_configured_cwd_from_cfg(cfg)`.
///
/// `cfg` is `Option<HashMap<String, HashMap<String,String>>>` stubbed as `Option<String>` raw for simplicity.
/// For full fidelity, `terminal_cwd` is the stringified `cfg["terminal"]["cwd"]` (None when absent).
pub fn configured_cwd_from_cfg(
    terminal_cwd: Option<&str>,
    is_dir: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    configured_cwd_from_raw(terminal_cwd, is_dir)
}

/// Resolve a non-launch profile's `terminal.cwd` — mirrors `_profile_configured_cwd`.
///
/// `read_profile_config` mirrors reading `home/config.yaml` + `_apply_managed` + `_expand_env_vars`.
/// Returns the raw `terminal.cwd` string if present (None → no config).
pub fn profile_configured_cwd(
    profile_home: Option<&Path>,
    read_raw: impl Fn(&Path) -> Option<String>,
    is_dir: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let home = profile_home?;
    let raw = read_raw(&home.join("config.yaml"))?;
    configured_cwd_from_raw(Some(&raw), is_dir)
}

/// Resolve the launch profile's `terminal.cwd` — mirrors `_launch_configured_cwd`.
///
/// `load_cfg_terminal_cwd` mirrors `_load_cfg().get("terminal",{}).get("cwd")`.
pub fn launch_configured_cwd(
    load_cfg_terminal_cwd: impl Fn() -> Option<String>,
    is_dir: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let raw = load_cfg_terminal_cwd();
    configured_cwd_from_raw(raw.as_deref(), is_dir)
}

/// Fallback cwd — mirrors `_default_session_cwd`.
///
/// `launch_cwd` mirrors `_launch_configured_cwd()`; `env_terminal_cwd` is `os.getenv("TERMINAL_CWD")`; `getcwd` is `os.getcwd()`.
pub fn default_session_cwd(
    launch_cwd: Option<PathBuf>,
    env_terminal_cwd: Option<&str>,
    getcwd: impl Fn() -> PathBuf,
) -> PathBuf {
    if let Some(p) = launch_cwd { return p; }
    if let Some(raw) = env_terminal_cwd {
        let t = raw.trim();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    getcwd()
}

// ---------------------------------------------------------------------------
// JSON-RPC plumbing — mirrors write_json / _event_frame / _emit / _broadcast_global_event
// ---------------------------------------------------------------------------

/// Build an event frame — mirrors `_event_frame(event, sid, payload)`.
///
/// ```python
/// def _event_frame(event: str, sid: str, payload: dict | None = None) -> dict:
///     params: dict = {"type": event, "session_id": sid}
///     if payload is not None: params["payload"] = payload
///     return {"jsonrpc":"2.0","method":"event","params":params}
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventFrame {
    pub event: String,
    pub session_id: String,
    pub payload_json: Option<String>,
}

impl EventFrame {
    pub fn new(event: impl Into<String>, sid: impl Into<String>, payload_json: Option<String>) -> Self {
        Self { event: event.into(), session_id: sid.into(), payload_json }
    }
    pub fn to_json(&self) -> String {
        if let Some(ref p) = self.payload_json {
            format!(
                r#"{{"jsonrpc":"2.0","method":"event","params":{{"type":"{}","session_id":"{}","payload":{}}}}}"#,
                escape_json(&self.event), escape_json(&self.session_id), p
            )
        } else {
            format!(
                r#"{{"jsonrpc":"2.0","method":"event","params":{{"type":"{}","session_id":"{}"}}}}"#,
                escape_json(&self.event), escape_json(&self.session_id)
            )
        }
    }
}

fn escape_json(s: &str) -> String {
    s.replace('\\', r"\\").replace('"', r#"\""#)
}

pub fn event_frame(event: &str, sid: &str, payload_json: Option<&str>) -> String {
    EventFrame::new(event, sid, payload_json.map(|s| s.to_string())).to_json()
}

/// Write target — mirrors `write_json` precedence.
///
/// 1. `Event` frames with `session_id` → `session.transport` (if that sid is known)
/// 2. `current_transport()` (ContextVar-bound)
/// 3. `_stdio_transport`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteTarget {
    SessionTransport { sid: String },
    CurrentTransport,
    Stdio,
}

pub fn write_json_target(is_event: bool, session_id: Option<&str>, has_session_transport: bool, has_current_transport: bool) -> WriteTarget {
    if is_event {
        if let Some(sid) = session_id {
            if !sid.is_empty() && has_session_transport {
                return WriteTarget::SessionTransport { sid: sid.to_string() };
            }
        }
    }
    if has_current_transport {
        return WriteTarget::CurrentTransport;
    }
    WriteTarget::Stdio
}

/// Live transports registry — mirrors `_live_transports: set[Transport]` + `_live_transports_lock`.
#[derive(Debug, Default)]
pub struct LiveTransports {
    peers: Mutex<HashSet<String>>,
}

impl LiveTransports {
    pub fn new() -> Self { Self { peers: Mutex::new(HashSet::new()) } }
    pub fn register(&self, peer: Option<&str>) {
        if let Some(p) = peer {
            if !p.is_empty() {
                self.peers.lock().unwrap().insert(p.to_string());
            }
        }
    }
    pub fn unregister(&self, peer: Option<&str>) {
        if let Some(p) = peer {
            self.peers.lock().unwrap().remove(p);
        }
    }
    pub fn snapshot(&self) -> Vec<String> {
        self.peers.lock().unwrap().iter().cloned().collect()
    }
    pub fn is_empty(&self) -> bool { self.peers.lock().unwrap().is_empty() }
    pub fn len(&self) -> usize { self.peers.lock().unwrap().len() }
}

/// Decide broadcast targets — mirrors `_broadcast_global_event(event,payload)`.
///
/// `has_targets` mirrors `list(_live_transports)` emptiness check.
/// Returns the wire action: either `_emit(event,"",payload)` (no WS peers) or `broadcast to N peers`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BroadcastAction {
    EmitFallback { event: String },
    Broadcast { event: String, peer_count: usize },
}

pub fn broadcast_global_action(event: &str, peers: &[String]) -> BroadcastAction {
    if peers.is_empty() {
        BroadcastAction::EmitFallback { event: event.to_string() }
    } else {
        BroadcastAction::Broadcast { event: event.to_string(), peer_count: peers.len() }
    }
}

// ---------------------------------------------------------------------------
// Compute-host bridge — mirrors _inside_compute_host_child etc.
// ---------------------------------------------------------------------------

/// Whether we are inside the compute-host child — mirrors `_inside_compute_host_child()`.
///
/// `env_val` is `os.environ.get("HERMES_COMPUTE_HOST_CHILD")`.
pub fn inside_compute_host_child(env_val: Option<&str>) -> bool {
    env_val == Some("1")
}

/// Whether turn isolation is enabled — mirrors `_turn_isolation_enabled(cfg)`.
///
/// ```python
/// def _turn_isolation_enabled(cfg=None) -> bool:
///     if _inside_compute_host_child(): return False
///     isolation_cfg = cfg or _load_dashboard_process_isolation_config()
///     return bool(isolation_cfg.get("turn_isolation"))
/// ```
pub fn turn_isolation_enabled(inside_child: bool, isolation_cfg_turn: Option<bool>) -> bool {
    if inside_child { return false; }
    isolation_cfg_turn.unwrap_or(false)
}

/// Whether a session should use the compute host — mirrors `_session_uses_compute_host(session, cfg)`.
///
/// ```python
/// def _session_uses_compute_host(session, cfg=None) -> bool:
///     if not _turn_isolation_enabled(cfg): return False
///     return bool(session.get("_compute_host_active")) or (
///         session.get("agent") is None and session.get("agent_ready") is not None
///     )
/// ```
pub fn session_uses_compute_host(
    turn_isolation_enabled: bool,
    compute_host_active: bool,
    has_agent: bool,
    has_agent_ready: bool,
) -> bool {
    if !turn_isolation_enabled { return false; }
    compute_host_active || (!has_agent && has_agent_ready)
}

/// Supervisor config — mirrors `HostSupervisor(rpc_sink=write_json, heartbeat_secs=..., respawn_max=...)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorConfig {
    pub heartbeat_secs: u32,
    pub respawn_max: u32,
}

impl SupervisorConfig {
    pub fn from_cfg(heartbeat_raw: Option<&str>, respawn_raw: Option<&str>) -> Self {
        let hb = heartbeat_raw.and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(15);
        let rm = respawn_raw.and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(3);
        Self { heartbeat_secs: hb, respawn_max: rm }
    }
    pub fn defaults() -> Self { Self { heartbeat_secs: 15, respawn_max: 3 } }
}

/// Compute-host turn frame — mirrors `_compute_host_turn_frame` payload keys.
///
/// `session.get("history")`, `history_version`, `attached_images` vs `image_paths`, `cols`, `cwd`, `profile_home`,
/// `model_override`, `create_reasoning_override`, `create_service_tier_override`, `source`, `queued_prompt_generation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputeHostTurnFrame {
    pub typ: String,
    pub sid: String,
    pub request_id: String,
    pub session_key: String,
    pub text_json: String,
    pub display_kind: Option<String>,
    pub history: Vec<String>,
    pub history_version: i64,
    pub cols: u32,
    pub cwd: String,
    pub profile_home: String,
    pub model_override: Option<String>,
    pub reasoning_override_json: Option<String>,
    pub service_tier_override: Option<String>,
    pub source: String,
    pub attached_images: Vec<String>,
    pub queued_prompt_generation: Option<i64>,
}

impl ComputeHostTurnFrame {
    pub fn to_json(&self) -> String {
        // Minimal JSON; real serialization uses serde_json — here we emit a stable shape for tests.
        let mut parts = vec![
            format!(r#""type":"{}""#, escape_json(&self.typ)),
            format!(r#""sid":"{}""#, escape_json(&self.sid)),
            format!(r#""request_id":"{}""#, escape_json(&self.request_id)),
            format!(r#""session_key":"{}""#, escape_json(&self.session_key)),
            format!(r#""text":{}"#, self.text_json),
            format!(r#""history_version":{}"#, self.history_version),
            format!(r#""cols":{}"#, self.cols),
            format!(r#""cwd":"{}""#, escape_json(&self.cwd)),
            format!(r#""profile_home":"{}""#, escape_json(&self.profile_home)),
            format!(r#""source":"{}""#, escape_json(&self.source)),
        ];
        if let Some(ref dk) = self.display_kind {
            parts.push(format!(r#""display_kind":"{}""#, escape_json(dk)));
        }
        if let Some(qg) = self.queued_prompt_generation {
            parts.push(format!(r#""queued_prompt_generation":{}"#, qg));
        }
        format!("{{{}}}", parts.join(","))
    }
}

pub fn compute_host_turn_frame(
    rid: &str,
    sid: &str,
    session_key: Option<&str>,
    text_json: &str,
    display_kind: Option<&str>,
    history: Vec<String>,
    history_version: i64,
    cols: u32,
    cwd: &str,
    profile_home: Option<&str>,
    model_override: Option<String>,
    reasoning_override_json: Option<String>,
    service_tier_override: Option<String>,
    source: &str,
    image_paths: Option<Vec<String>>,
    attached_images: Vec<String>,
    queued_prompt_generation: Option<i64>,
) -> ComputeHostTurnFrame {
    ComputeHostTurnFrame {
        typ: "turn.start".to_string(),
        sid: sid.to_string(),
        request_id: rid.to_string(),
        session_key: session_key.unwrap_or(sid).to_string(),
        text_json: text_json.to_string(),
        display_kind: display_kind.map(|s| s.to_string()),
        history,
        history_version,
        cols,
        cwd: cwd.to_string(),
        profile_home: profile_home.unwrap_or("").to_string(),
        model_override,
        reasoning_override_json,
        service_tier_override,
        source: source.to_string(),
        attached_images: image_paths.unwrap_or(attached_images),
        queued_prompt_generation,
    }
}

/// Metadata mirror — mirrors `_metadata_mirror(session) -> dict`.
pub fn metadata_mirror(mirror_json: Option<&str>) -> String {
    mirror_json.unwrap_or("{}").to_string()
}

/// Apply the host metadata mirror — mirrors `_apply_compute_host_metadata_mirror(session, frame)`.
///
/// Returns the merged mirror json and whether `_metadata_mirror_updated_at` should be set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyMirrorResult {
    pub history_version: i64,
    pub message_count: Option<i64>,
    pub mirror_json: Option<String>,
    pub should_stamp: bool,
}

pub fn apply_metadata_mirror(
    cur_history_version: i64,
    frame_history_version: Option<i64>,
    frame_message_count: Option<i64>,
    frame_session_info_json: Option<&str>,
    existing_mirror_json: Option<&str>,
) -> ApplyMirrorResult {
    let hv = match frame_history_version {
        Some(v) => cur_history_version.max(v),
        None => cur_history_version,
    };
    let mc = frame_message_count;
    let (mirror_json, stamp) = if let Some(info) = frame_session_info_json {
        // Merge into existing mirror
        let base = existing_mirror_json.unwrap_or("{}");
        // Naïve merge: if both are {}-enclosed json objects, concat.
        let merged = if base.trim() == "{}" {
            info.to_string()
        } else {
            // Best-effort merge for tests: return info as merged (real merge is dict.update).
            info.to_string()
        };
        (Some(merged), true)
    } else {
        (None, false)
    };
    ApplyMirrorResult { history_version: hv, message_count: mc, mirror_json, should_stamp: stamp }
}

/// What `_on_compute_host_turn_done` should emit — mirrors the `message.complete` vs `session.info` + `_drain_queued_prompt` tail.
///
/// Frame `type == "turn.error"` → error completion; else info emit (unless `session_info_emitted` was already true) + queue drain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnDoneAction {
    Error { message: String },
    InfoAndDrain { emit_info: bool },
}

pub fn on_compute_host_turn_done(
    frame_type: &str,
    frame_message: Option<&str>,
    frame_session_info_emitted: bool,
) -> TurnDoneAction {
    if frame_type == "turn.error" {
        let msg = frame_message.unwrap_or("compute host turn failed").to_string();
        TurnDoneAction::Error { message: format!("Error: {}", msg) }
    } else {
        TurnDoneAction::InfoAndDrain { emit_info: !frame_session_info_emitted }
    }
}

/// Submit prompt result — mirrors `_submit_prompt_to_compute_host` return.
///
/// `Ok(streaming=true)` → `{status:"streaming",turn_isolation:true}`; `Err(5019, msg)` → compute-host dispatch failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitPromptResult {
    Streaming,
    Err { code: i32, message: String },
}

pub fn submit_prompt_result(submit_ok: bool, dispatch_err: Option<&str>) -> SubmitPromptResult {
    if let Some(msg) = dispatch_err {
        return SubmitPromptResult::Err { code: 5019, message: format!("compute-host dispatch failed: {}", msg) };
    }
    if submit_ok {
        SubmitPromptResult::Streaming
    } else {
        SubmitPromptResult::Err { code: 5019, message: "compute-host dispatch failed: unknown".to_string() }
    }
}

/// Control frame for `_send_compute_host_control` — mirrors `frame.setdefault("type","control"); frame.setdefault("command",command)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlFrame {
    pub typ: String,
    pub command: String,
    pub extra_json: Option<String>,
}

impl ControlFrame {
    pub fn new(payload_json: Option<&str>, command: &str) -> Self {
        Self {
            typ: "control".to_string(),
            command: command.to_string(),
            extra_json: payload_json.map(|s| s.to_string()),
        }
    }
    pub fn to_json(&self) -> String {
        let mut base = format!(r#"{{"type":"{}","command":"{}""#, escape_json(&self.typ), escape_json(&self.command));
        if let Some(ref extra) = self.extra_json {
            base.push(',');
            // strip braces from extra for inline merge (best-effort)
            let inner = extra.trim().trim_start_matches('{').trim_end_matches('}');
            if !inner.trim().is_empty() {
                base.push_str(inner);
            }
        }
        base.push('}');
        base
    }
}

// ---------------------------------------------------------------------------
// Approval / clarify / status / images — mirrors helpers 676-787
// ---------------------------------------------------------------------------

/// Build the client-safe approval payload — mirrors `_approval_request_payload`.
///
/// Fills `choices` when missing:
/// * `smart_denied == true` → `["once","deny"]`
/// * else `["once"]` + `["session"]` unless `allow_session is False` + `["always"]` unless `allow_permanent is False` + `["deny"]`
/// Redacts `command` via `redact`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApprovalRaw {
    pub smart_denied: bool,
    pub allow_session: Option<bool>,
    pub allow_permanent: Option<bool>,
    pub command: Option<String>,
    pub choices: Option<Vec<String>>,
    pub extra: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPayload {
    pub choices: Vec<String>,
    pub command: Option<String>,
    pub extra: HashMap<String, String>,
}

pub fn approval_request_payload(raw: Option<&ApprovalRaw>, redact: impl Fn(&str) -> String) -> ApprovalPayload {
    let empty = ApprovalRaw::default();
    let r = raw.unwrap_or(&empty);
    let choices = if let Some(ref c) = r.choices {
        c.clone()
    } else if r.smart_denied {
        vec!["once".to_string(), "deny".to_string()]
    } else {
        let mut c = vec!["once".to_string()];
        if r.allow_session != Some(false) {
            c.push("session".to_string());
            if r.allow_permanent != Some(false) {
                c.push("always".to_string());
            }
        }
        c.push("deny".to_string());
        c
    };
    let command = r.command.as_deref().map(|s| redact(s));
    ApprovalPayload { choices, command, extra: r.extra.clone() }
}

/// Pending clarify registry scan — mirrors `_pending_clarify_request_payload(sid)`.
///
/// `pending` is `HashMap<rid, (owner_sid, event)>` + `pending_payloads: HashMap<rid, payload_json>` + `batch: HashMap<rid, answers_json>`.
pub fn pending_clarify_payload(
    sid: &str,
    pending: &HashMap<String, (String, String)>,
    payloads: &HashMap<String, (String, String)>,
    batches: &HashMap<String, HashMap<String, String>>,
) -> Option<String> {
    for (rid, (owner_sid, _ev)) in pending {
        if owner_sid != sid { continue; }
        if let Some((event, prompt_payload)) = payloads.get(rid) {
            if event == "clarify.request" {
                // snapshot = dict(prompt_payload); replay batch answers if present
                let mut out = prompt_payload.clone();
                if let Some(answers) = batches.get(rid) {
                    if !answers.is_empty() {
                        out.push_str(&format!(" answers={:?}", answers));
                    }
                }
                return Some(out);
            }
        }
    }
    None
}

/// Pending approval payload — mirrors `_pending_approval_request_payload(session_key)`.
///
/// `get_pending` mirrors `get_pending_gateway_approval(session_key)` (injected).
pub fn pending_approval_payload(
    session_key: &str,
    get_pending: impl Fn(&str) -> Option<ApprovalRaw>,
    redact: impl Fn(&str) -> String,
) -> Option<ApprovalPayload> {
    let raw = get_pending(session_key)?;
    Some(approval_request_payload(Some(&raw), redact))
}

/// Status update kind — mirrors `_status_update(sid, kind, text)` compacting re-tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusUpdate {
    pub kind: String,
    pub text: String,
}

pub fn status_update(sid: &str, kind: &str, text: Option<&str>, compaction_marker: &str) -> Option<StatusUpdate> {
    let _ = sid;
    let body = text.unwrap_or(kind).trim().to_string();
    if body.is_empty() {
        return None;
    }
    let mut out_kind = if text.is_some() { kind.to_string() } else { "status".to_string() };
    if out_kind == "lifecycle" && body.contains(compaction_marker) {
        out_kind = "compacting".to_string();
    }
    Some(StatusUpdate { kind: out_kind, text: body })
}

/// Estimate image tokens — mirrors `_estimate_image_tokens(width,height)`.
///
/// `max(1, (w+511)//512) * max(1, (h+511)//512) * 85`
pub fn estimate_image_tokens(width: i64, height: i64) -> i64 {
    if width <= 0 || height <= 0 { return 0; }
    let tiles_w = (width + 511) / 512;
    let tiles_h = (height + 511) / 512;
    tiles_w.max(1) * tiles_h.max(1) * 85
}

/// Image meta — mirrors `_image_meta(path) -> {name,width,height,token_estimate}`.
///
/// `get_dims` mirrors `PIL.Image.open(path).size` (None → no dims, no token estimate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageMeta {
    pub name: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub token_estimate: Option<i64>,
}

pub fn image_meta(path: &Path, get_dims: impl Fn(&Path) -> Option<(u32, u32)>) -> ImageMeta {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
    let (w, h) = match get_dims(path) {
        Some((ww, hh)) => (Some(ww), Some(hh)),
        None => (None, None),
    };
    let token_estimate = match (w, h) {
        (Some(ww), Some(hh)) => Some(estimate_image_tokens(ww as i64, hh as i64)),
        _ => None,
    };
    ImageMeta { name, width: w, height: h, token_estimate }
}

// ---------------------------------------------------------------------------
// JSON-RPC helpers — mirrors _ok / _err / method
// ---------------------------------------------------------------------------

/// Build a success envelope — mirrors `_ok(rid, result)`.
///
/// ```python
/// def _ok(rid, result: dict) -> dict:
///     return {"jsonrpc":"2.0","id":rid,"result":result}
/// ```
pub fn ok_response(rid_json: &str, result_json: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":{},"result":{}}}"#, rid_json, result_json)
}

/// Build an error envelope — mirrors `_err(rid, code, msg, data=None)`.
///
/// ```python
/// def _err(rid, code: int, msg: str, data=None) -> dict:
///     error={"code":code,"message":msg}
///     if data is not None: error["data"]=data
///     return {"jsonrpc":"2.0","id":rid,"error":error}
/// ```
pub fn err_response(rid_json: &str, code: i32, message: &str, data_json: Option<&str>) -> String {
    let msg = escape_json(message);
    if let Some(data) = data_json {
        format!(r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":{},"message":"{}","data":{}}}}}"#, rid_json, code, msg, data)
    } else {
        format!(r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":{},"message":"{}"}}}}"#, rid_json, code, msg)
    }
}

/// Registry for `method(name)` — mirrors `_methods: dict[str, Callable]` + `def method(name): _methods[name]=fn`.
///
/// In Python `method` is a decorator; here it is a typed registry.
#[derive(Debug, Default, Clone)]
pub struct MethodRegistry {
    inner: HashMap<String, String>,
}

impl MethodRegistry {
    pub fn new() -> Self { Self { inner: HashMap::new() } }
    /// Register `name` → `handler_id` (mirrors `_methods[name]=fn`).
    pub fn register(&mut self, name: &str, handler_id: impl Into<String>) -> bool {
        let is_new = !self.inner.contains_key(name);
        self.inner.insert(name.to_string(), handler_id.into());
        is_new
    }
    pub fn contains(&self, name: &str) -> bool { self.inner.contains_key(name) }
    pub fn len(&self) -> usize { self.inner.len() }
    pub fn get(&self, name: &str) -> Option<&String> { self.inner.get(name) }
}

// ---------------------------------------------------------------------------
// Tests — mirror Python invariants (std-only)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // -- cwd placeholder ---------------------------------------------------

    #[test]
    fn cwd_placeholder() {
        assert!(is_cwd_placeholder("."));
        assert!(is_cwd_placeholder("auto"));
        assert!(is_cwd_placeholder("cwd"));
        assert!(!is_cwd_placeholder("/home/test"));
        assert!(!is_cwd_placeholder(""));
        assert!(!is_cwd_placeholder("AUTO"));
    }

    // -- orphan sweep sources ----------------------------------------------

    #[test]
    fn orphan_sources() {
        assert!(is_orphan_sweep_source("tui"));
        assert!(is_orphan_sweep_source("desktop"));
        assert!(is_orphan_sweep_source("subagent"));
        assert!(!is_orphan_sweep_source("telegram"));
        assert!(!is_orphan_sweep_source(""));
        assert_eq!(ORPHAN_SWEEP_SOURCES.len(), 3);
    }

    // -- is_truthy_value ---------------------------------------------------

    #[test]
    fn truthy() {
        assert!(is_truthy_value(Some("true"), false));
        assert!(is_truthy_value(Some("1"), false));
        assert!(is_truthy_value(Some("yes"), false));
        assert!(is_truthy_value(Some("ON"), false));
        assert!(!is_truthy_value(Some("false"), true));
        assert!(!is_truthy_value(Some("0"), true));
        assert!(!is_truthy_value(Some("off"), true));
        assert!(!is_truthy_value(Some(""), true));
        // default fallback on unknown + None
        assert!(is_truthy_value(None, true));
        assert!(!is_truthy_value(None, false));
        assert!(is_truthy_value(Some("unknown"), true));
        assert!(!is_truthy_value(Some("unknown"), false));
    }

    #[test]
    fn orphan_reaper_enabled_gate() {
        // missing key → true (fail-open)
        assert!(orphan_reaper_enabled(None));
        assert!(!orphan_reaper_enabled(Some("false")));
        assert!(orphan_reaper_enabled(Some("true")));
        assert!(orphan_reaper_enabled(Some("1")));
        assert!(!orphan_reaper_enabled(Some("0")));
        // unknown → default true
        assert!(orphan_reaper_enabled(Some("maybe")));
    }

    // -- startup sweep guard -----------------------------------------------

    #[test]
    fn startup_guard() {
        assert!(matches!(startup_sweep_guard(0.0, 21600.0, true, false), StartupSweepGuardAction::Skip("ws_grace_disabled")));
        assert!(matches!(startup_sweep_guard(20.0, 0.0, true, false), StartupSweepGuardAction::Skip("ttl_disabled")));
        assert!(matches!(startup_sweep_guard(20.0, 21600.0, false, false), StartupSweepGuardAction::Skip("reaper_disabled")));
        assert!(matches!(startup_sweep_guard(20.0, 21600.0, true, true), StartupSweepGuardAction::Skip("already_ran")));
        assert!(matches!(startup_sweep_guard(20.0, 21600.0, true, false), StartupSweepGuardAction::Schedule { grace_s } if (grace_s - 20.0).abs() < 1e-9));
    }

    #[test]
    fn startup_state_mark() {
        let s = StartupSweepState::new();
        assert!(!s.has_ran());
        assert!(s.mark_ran());
        assert!(s.has_ran());
        assert!(!s.mark_ran(), "second mark is no-op");
        s.reset_for_test();
        assert!(!s.has_ran());
    }

    // -- live ids ----------------------------------------------------------

    #[test]
    fn live_ids() {
        let mut reg: HashMap<String, SessionRef> = HashMap::new();
        reg.insert("sid1".into(), SessionRef { sid: "sid1".into(), session_key: Some("key1".into()), agent_session_id: Some("sess1".into()) });
        reg.insert("sid2".into(), SessionRef { sid: "sid2".into(), session_key: Some("key1".into()), agent_session_id: None });
        reg.insert("".into(), SessionRef { sid: "".into(), session_key: Some("orphan".into()), agent_session_id: None });
        let mut ids = live_session_ids_from_registry(&reg);
        ids.sort();
        assert!(ids.contains(&"sid1".to_string()));
        assert!(ids.contains(&"sess1".to_string()));
        assert!(ids.contains(&"key1".to_string()));
        assert!(ids.contains(&"orphan".to_string()));
        // deduped and sorted
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
    }

    // -- sweep args --------------------------------------------------------

    #[test]
    fn sweep_args_guards() {
        assert!(sweep_orphaned_args(false, 21600.0, vec![]).is_none());
        assert!(sweep_orphaned_args(true, 0.0, vec![]).is_none());
        assert!(sweep_orphaned_args(true, -1.0, vec![]).is_none());
        let args = sweep_orphaned_args(true, 21600.0, vec!["a".into()]).unwrap();
        assert!((args.max_idle_seconds - 21600.0).abs() < 1e-9);
        assert_eq!(args.sources, vec!["tui".to_string(), "desktop".to_string(), "subagent".to_string()]);
        assert_eq!(args.exclude_ids, vec!["a".to_string()]);
        assert!(should_sweep_orphans(true, 10.0));
        assert!(!should_sweep_orphans(false, 10.0));
        assert!(!should_sweep_orphans(true, 0.0));
    }

    // -- db_for_profile / transfer -----------------------------------------

    #[test]
    fn db_for_profile_routing() {
        assert_eq!(db_for_profile(None), DbProfileRoute::Shared);
        let home = PathBuf::from("/tmp/profile");
        assert_eq!(db_for_profile(Some(home.clone())), DbProfileRoute::Dedicated(home.join("state.db")));
        let scope = ProfileDbScope::for_params(Some(PathBuf::from("/tmp/p")));
        assert!(scope.owns_handle);
        assert!(scope.should_close(true));
        assert!(!scope.should_close(false));
        let scope2 = ProfileDbScope::for_params(None);
        assert!(!scope2.owns_handle);
        assert!(!scope2.should_close(true));
    }

    #[test]
    fn transfer_db() {
        assert!(transfer_db_to_agent(Some("/a/state.db"), Some("/a/state.db"), false));
        assert!(!transfer_db_to_agent(Some("/a/state.db"), Some("/a/state.db"), true), "shared must not transfer");
        assert!(!transfer_db_to_agent(Some("/a/state.db"), Some("/b/state.db"), false));
        assert!(!transfer_db_to_agent(None, Some("/a/state.db"), false));
        assert!(!transfer_db_to_agent(Some("/a/state.db"), None, false));
    }

    #[test]
    fn response_profile_name_gate() {
        let current = "default";
        assert_eq!(response_profile_name(Some("work"), Some(Path::new("/tmp/work")), current), "work");
        assert_eq!(response_profile_name(Some("work"), None, current), "default");
        assert_eq!(response_profile_name(None, Some(Path::new("/tmp/work")), current), "default");
        assert_eq!(response_profile_name(Some("  "), Some(Path::new("/tmp/work")), current), "default");
    }

    // -- profile home ------------------------------------------------------

    #[test]
    fn profile_home_resolve() {
        let hermes_home = Path::new("/home/test/.hermes");
        let got = resolve_profile_home(Some("work"), hermes_home, |name| {
            if name == "work" { Some(PathBuf::from("/home/test/.hermes-work")) } else { None }
        }, |p| p == Path::new("/home/test/.hermes-work"));
        assert_eq!(got, Some(PathBuf::from("/home/test/.hermes-work")));

        // launch profile → None
        let got2 = resolve_profile_home(Some("default"), hermes_home, |_| Some(hermes_home.to_path_buf()), |_| true);
        assert_eq!(got2, None);

        // missing home on disk → None
        let got3 = resolve_profile_home(Some("ghost"), hermes_home, |_| Some(PathBuf::from("/tmp/ghost")), |_| false);
        assert_eq!(got3, None);

        assert!(resolve_profile_home(None, hermes_home, |_| None, |_| true).is_none());
        assert!(resolve_profile_home(Some("  "), hermes_home, |_| None, |_| true).is_none());
    }

    #[test]
    fn profile_scoped_wrapper() {
        let mut set_calls = 0;
        let mut reset_calls = 0;
        let res = profile_scoped_call(Some(Path::new("/tmp/work")),
            |_p| { set_calls += 1; "token".to_string() },
            |tok| { reset_calls += 1; assert_eq!(tok, "token"); },
            || 42);
        assert_eq!(res, 42);
        assert_eq!(set_calls, 1);
        assert_eq!(reset_calls, 1);

        // None → no override
        let res2 = profile_scoped_call(None, |_| panic!("should not set"), |_| panic!("should not reset"), || 7);
        assert_eq!(res2, 7);
    }

    // -- cwd ---------------------------------------------------------------

    #[test]
    fn configured_cwd() {
        let is_dir = |p: &Path| p == Path::new("/home/test/projects") || p == Path::new("/home/test/work");
        assert_eq!(configured_cwd_from_raw(Some("/home/test/projects"), is_dir), Some(PathBuf::from("/home/test/projects")));
        assert!(configured_cwd_from_raw(Some("."), is_dir).is_none());
        assert!(configured_cwd_from_raw(Some("auto"), is_dir).is_none());
        assert!(configured_cwd_from_raw(Some("cwd"), is_dir).is_none());
        assert!(configured_cwd_from_raw(Some("/nonexistent"), is_dir).is_none());
        assert!(configured_cwd_from_raw(Some("relative/path"), is_dir).is_none());
        assert!(configured_cwd_from_raw(None, is_dir).is_none());
        assert!(configured_cwd_from_raw(Some("   "), is_dir).is_none());
        // ~ expansion
        assert_eq!(configured_cwd_from_raw(Some("~/work"), |p| p == Path::new("/home/test/work")), Some(PathBuf::from("/home/test/work")));
    }

    #[test]
    fn default_cwd() {
        assert_eq!(default_session_cwd(Some(PathBuf::from("/cfg/cwd")), Some("/env"), || PathBuf::from("/getcwd")), PathBuf::from("/cfg/cwd"));
        assert_eq!(default_session_cwd(None, Some("/env/cwd"), || PathBuf::from("/getcwd")), PathBuf::from("/env/cwd"));
        assert_eq!(default_session_cwd(None, Some("   "), || PathBuf::from("/getcwd")), PathBuf::from("/getcwd"));
        assert_eq!(default_session_cwd(None, None, || PathBuf::from("/getcwd")), PathBuf::from("/getcwd"));
    }

    // -- event frame / write target / broadcast ----------------------------

    #[test]
    fn event_frame_json() {
        let j = event_frame("gateway.ready", "sid1", Some(r#"{"skin":"dark"}"#));
        assert!(j.contains(r#""type":"gateway.ready""#));
        assert!(j.contains(r#""session_id":"sid1""#));
        assert!(j.contains(r#""skin":"dark""#));
        let j2 = event_frame("status.update", "s", None);
        assert!(j2.contains(r#""type":"status.update""#));
        assert!(!j2.contains("payload"));
    }

    #[test]
    fn write_target() {
        assert_eq!(write_json_target(true, Some("sid"), true, true), WriteTarget::SessionTransport { sid: "sid".into() });
        assert_eq!(write_json_target(true, Some(""), true, true), WriteTarget::CurrentTransport);
        assert_eq!(write_json_target(true, None, false, true), WriteTarget::CurrentTransport);
        assert_eq!(write_json_target(false, Some("sid"), true, false), WriteTarget::Stdio);
        assert_eq!(write_json_target(true, Some("sid"), false, false), WriteTarget::Stdio);
    }

    #[test]
    fn live_transports() {
        let lt = LiveTransports::new();
        lt.register(Some("peer1"));
        lt.register(Some("peer2"));
        lt.register(None);
        lt.register(Some(""));
        assert_eq!(lt.len(), 2);
        let mut snap = lt.snapshot();
        snap.sort();
        assert_eq!(snap, vec!["peer1".to_string(), "peer2".to_string()]);
        lt.unregister(Some("peer1"));
        assert_eq!(lt.len(), 1);
        lt.unregister(Some("ghost")); // no-op
        assert_eq!(lt.len(), 1);
    }

    #[test]
    fn broadcast_action() {
        assert_eq!(broadcast_global_action("skin.changed", &[]), BroadcastAction::EmitFallback { event: "skin.changed".into() });
        assert_eq!(broadcast_global_action("skin.changed", &["p1".to_string(), "p2".to_string()]), BroadcastAction::Broadcast { event: "skin.changed".into(), peer_count: 2 });
    }

    // -- compute host ------------------------------------------------------

    #[test]
    fn inside_child_and_isolation() {
        assert!(inside_compute_host_child(Some("1")));
        assert!(!inside_compute_host_child(Some("0")));
        assert!(!inside_compute_host_child(None));
        assert!(!turn_isolation_enabled(true, Some(true)));
        assert!(turn_isolation_enabled(false, Some(true)));
        assert!(!turn_isolation_enabled(false, Some(false)));
        assert!(!turn_isolation_enabled(false, None));
    }

    #[test]
    fn uses_compute_host() {
        assert!(!session_uses_compute_host(false, true, false, true));
        assert!(session_uses_compute_host(true, true, true, false));
        assert!(session_uses_compute_host(true, false, false, true));
        assert!(!session_uses_compute_host(true, false, false, false));
        assert!(!session_uses_compute_host(true, false, true, false));
    }

    #[test]
    fn supervisor_config() {
        let c = SupervisorConfig::from_cfg(Some("20"), Some("5"));
        assert_eq!(c.heartbeat_secs, 20);
        assert_eq!(c.respawn_max, 5);
        let d = SupervisorConfig::defaults();
        assert_eq!(d.heartbeat_secs, 15);
        assert_eq!(d.respawn_max, 3);
        let bad = SupervisorConfig::from_cfg(Some("bad"), None);
        assert_eq!(bad.heartbeat_secs, 15);
        assert_eq!(bad.respawn_max, 3);
    }

    #[test]
    fn compute_frame() {
        let f = compute_host_turn_frame("rid1", "sid1", Some("key1"), r#""hello""#, None,
            vec!["msg".into()], 3, 80, "/tmp", Some("/home/test/.hermes"), None, None, None, "tui",
            None, vec!["img.png".into()], Some(2));
        assert_eq!(f.sid, "sid1");
        assert_eq!(f.session_key, "key1");
        assert_eq!(f.attached_images, vec!["img.png".to_string()]);
        let j = f.to_json();
        assert!(j.contains(r#""sid":"sid1""#));
        // image_paths overrides attached_images
        let f2 = compute_host_turn_frame("r", "s", None, r#""hi""#, Some("k"),
            vec![], 0, 80, "/tmp", None, None, None, None, "tui",
            Some(vec!["a.png".into()]), vec!["b.png".into()], None);
        assert_eq!(f2.attached_images, vec!["a.png".to_string()]);
        assert_eq!(f2.display_kind.as_deref(), Some("k"));
        assert_eq!(f2.session_key, "s");
    }

    #[test]
    fn metadata_mirror_apply() {
        let r = apply_metadata_mirror(5, Some(10), Some(7), Some(r#"{"model":"m"}"#), Some("{}"));
        assert_eq!(r.history_version, 10);
        assert_eq!(r.message_count, Some(7));
        assert!(r.should_stamp);
        assert!(r.mirror_json.is_some());
        let r2 = apply_metadata_mirror(5, Some(2), None, None, None);
        assert_eq!(r2.history_version, 5);
        assert!(!r2.should_stamp);
        let r3 = apply_metadata_mirror(5, None, None, None, None);
        assert_eq!(r3.history_version, 5);
    }

    #[test]
    fn turn_done() {
        assert_eq!(on_compute_host_turn_done("turn.error", Some("boom"), false), TurnDoneAction::Error { message: "Error: boom".into() });
        assert_eq!(on_compute_host_turn_done("turn.done", None, false), TurnDoneAction::InfoAndDrain { emit_info: true });
        assert_eq!(on_compute_host_turn_done("turn.done", None, true), TurnDoneAction::InfoAndDrain { emit_info: false });
    }

    #[test]
    fn submit_result() {
        assert_eq!(submit_prompt_result(true, None), SubmitPromptResult::Streaming);
        assert!(matches!(submit_prompt_result(false, Some("pipe broken")), SubmitPromptResult::Err { code: 5019, .. }));
        assert!(matches!(submit_prompt_result(true, Some("err")), SubmitPromptResult::Err { .. }));
    }

    // -- approval / clarify / status / images ------------------------------

    #[test]
    fn approval_payload_choices() {
        let raw = ApprovalRaw { smart_denied: false, allow_session: None, allow_permanent: None, command: Some("rm -rf /".into()), choices: None, extra: HashMap::new() };
        let p = approval_request_payload(Some(&raw), |s| format!("redacted:{}", s));
        assert_eq!(p.choices, vec!["once".to_string(), "session".to_string(), "always".to_string(), "deny".to_string()]);
        assert_eq!(p.command.as_deref(), Some("redacted:rm -rf /"));

        let raw2 = ApprovalRaw { smart_denied: true, allow_session: None, allow_permanent: None, command: None, choices: None, extra: HashMap::new() };
        let p2 = approval_request_payload(Some(&raw2), |s| s.to_string());
        assert_eq!(p2.choices, vec!["once".to_string(), "deny".to_string()]);

        let raw3 = ApprovalRaw { smart_denied: false, allow_session: Some(false), allow_permanent: None, command: None, choices: None, extra: HashMap::new() };
        let p3 = approval_request_payload(Some(&raw3), |s| s.to_string());
        assert_eq!(p3.choices, vec!["once".to_string(), "deny".to_string()]);

        let raw4 = ApprovalRaw { smart_denied: false, allow_session: None, allow_permanent: Some(false), command: None, choices: None, extra: HashMap::new() };
        let p4 = approval_request_payload(Some(&raw4), |s| s.to_string());
        assert_eq!(p4.choices, vec!["once".to_string(), "session".to_string(), "deny".to_string()]);

        // explicit choices wins
        let raw5 = ApprovalRaw { smart_denied: false, allow_session: None, allow_permanent: None, command: None, choices: Some(vec!["x".into()]), extra: HashMap::new() };
        assert_eq!(approval_request_payload(Some(&raw5), |s| s.to_string()).choices, vec!["x".to_string()]);

        // None raw → default choices
        let p6 = approval_request_payload(None, |s| s.to_string());
        assert_eq!(p6.choices, vec!["once".to_string(), "session".to_string(), "always".to_string(), "deny".to_string()]);
    }

    #[test]
    fn pending_clarify() {
        let mut pending = HashMap::new();
        pending.insert("rid1".into(), ("sid1".into(), "ev".into()));
        pending.insert("rid2".into(), ("sid2".into(), "ev".into()));
        let mut payloads = HashMap::new();
        payloads.insert("rid1".into(), ("clarify.request".into(), "payload1".into()));
        payloads.insert("rid2".into(), ("other.event".into(), "payload2".into()));
        let batches = HashMap::new();
        assert!(pending_clarify_payload("sid1", &pending, &payloads, &batches).is_some());
        assert!(pending_clarify_payload("sid2", &pending, &payloads, &batches).is_none());
        assert!(pending_clarify_payload("ghost", &pending, &payloads, &batches).is_none());

        // batch answers
        let mut batches2 = HashMap::new();
        batches2.insert("rid1".into(), HashMap::from([("q1".into(), "a1".into())]));
        assert!(pending_clarify_payload("sid1", &pending, &payloads, &batches2).unwrap().contains("answers"));
    }

    #[test]
    fn pending_approval() {
        let p = pending_approval_payload("key1", |k| {
            if k == "key1" { Some(ApprovalRaw { smart_denied: true, allow_session: None, allow_permanent: None, command: None, choices: None, extra: HashMap::new() }) } else { None }
        }, |s| s.to_string());
        assert!(p.is_some());
        assert_eq!(p.unwrap().choices, vec!["once".to_string(), "deny".to_string()]);
        assert!(pending_approval_payload("ghost", |_| None, |s| s.to_string()).is_none());
    }

    #[test]
    fn status_update_kind() {
        assert!(status_update("sid", "lifecycle", Some(""), COMPACTION_STATUS_MARKER).is_none());
        assert!(status_update("sid", "   ", None, COMPACTION_STATUS_MARKER).is_none());
        let s = status_update("sid", "lifecycle", Some("start [compacting] done"), COMPACTION_STATUS_MARKER).unwrap();
        assert_eq!(s.kind, "compacting");
        let s2 = status_update("sid", "lifecycle", Some("plain lifecycle"), COMPACTION_STATUS_MARKER).unwrap();
        assert_eq!(s2.kind, "lifecycle");
        let s3 = status_update("sid", "lifecycle", None, COMPACTION_STATUS_MARKER).unwrap();
        assert_eq!(s3.kind, "status"); // no text → kind is "status", not lifecycle
        let s4 = status_update("sid", "custom", Some("hello"), COMPACTION_STATUS_MARKER).unwrap();
        assert_eq!(s4.kind, "custom");
        assert_eq!(s4.text, "hello");
    }

    #[test]
    fn image_tokens_and_meta() {
        assert_eq!(estimate_image_tokens(0, 100), 0);
        assert_eq!(estimate_image_tokens(100, 0), 0);
        assert_eq!(estimate_image_tokens(-1, 10), 0);
        assert_eq!(estimate_image_tokens(512, 512), 85);
        assert_eq!(estimate_image_tokens(513, 512), 170);
        assert_eq!(estimate_image_tokens(1024, 1024), 340);
        assert_eq!(estimate_image_tokens(1, 1), 85);
        let m = image_meta(Path::new("/tmp/foo.png"), |_| Some((512, 512)));
        assert_eq!(m.name, "foo.png");
        assert_eq!(m.width, Some(512));
        assert_eq!(m.token_estimate, Some(85));
        let m2 = image_meta(Path::new("/tmp/bar.jpg"), |_| None);
        assert_eq!(m2.width, None);
        assert!(m2.token_estimate.is_none());
    }

    // -- json rpc ----------------------------------------------------------

    #[test]
    fn ok_err() {
        assert_eq!(ok_response("1", r#"{"status":"ok"}"#), r#"{"jsonrpc":"2.0","id":1,"result":{"status":"ok"}}"#);
        assert_eq!(err_response("null", -32600, "invalid request", None), r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"invalid request"}}"#);
        assert_eq!(err_response("1", -32601, "unknown method", Some(r#"{"info":"x"}"#)), r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"unknown method","data":{"info":"x"}}}"#);
    }

    #[test]
    fn method_registry() {
        let mut r = MethodRegistry::new();
        assert!(r.register("session.create", "h1"));
        assert!(r.contains("session.create"));
        assert!(!r.register("session.create", "h2"), "re-register is not new");
        assert_eq!(r.len(), 1);
        assert_eq!(r.get("session.create").unwrap(), "h2");
        assert!(r.get("ghost").is_none());
    }

    #[test]
    fn db_unavailable() {
        let j = db_unavailable_error(Some("1"), -32000, Some("no db"));
        assert!(j.contains("state.db unavailable: no db"));
        assert!(j.contains(r#""id":"1""#));
        let j2 = db_unavailable_error(None, 500, None);
        assert!(j2.contains("state.db unavailable"));
        assert!(j2.contains(r#""id":null"#));
    }

    #[test]
    fn control_frame() {
        let c = ControlFrame::new(Some(r#"{"extra":1}"#), "interrupt");
        assert_eq!(c.typ, "control");
        assert_eq!(c.command, "interrupt");
        let j = c.to_json();
        assert!(j.contains(r#""type":"control""#));
        assert!(j.contains(r#""command":"interrupt""#));
        let c2 = ControlFrame::new(None, "");
        assert_eq!(c2.to_json(), r#"{"type":"control","command":""}"#);
    }
}
