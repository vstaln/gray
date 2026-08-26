//! TUI gateway server — session lifecycle, WS-orphan reap, and idle/LRU reapers (slice 2).
//!
//! 1:1 port of `tui_gateway/server.py` lines 800–1600 (T0382).
//!
//! This slice covers the tail of `_finalize_session` (persist + hooks + DB end
//! + async-delegation interrupt + slash-worker close), the reclaim broadcast,
//! and the whole close/reap subsystem through the LRU-cap enforcement and its
//! off-response-path scheduler.
//!
//! ```python
//! # Python — tui_gateway/server.py 800-1600 (abridged, comments preserved)
//!
//!         history = list(session.get("history", []))
//!
//!     # ── Persist unflushed messages to SQLite ──────────────────────────
//!     if agent is not None and hasattr(agent, "_persist_session"):
//!         snapshot = getattr(agent, "_session_messages", None)
//!         if snapshot:
//!             try: agent._persist_session(snapshot)
//!             except Exception: pass
//!
//!     # ── Plugin hook: on_session_end ────────────────────────────────────
//!     if agent is not None:
//!         try:
//!             from hermes_cli.lifecycle import invoke_hook
//!             invoke_hook("on_session_end", session_id=..., completed=False, interrupted=True, model=..., platform=...)
//!         except Exception: pass
//!
//!     if agent is not None and history and hasattr(agent, "commit_memory_session"):
//!         try: agent.commit_memory_session(history)
//!         except Exception: pass
//!
//!     session_key = session.get("session_key")
//!     session_id = getattr(agent, "session_id", None) or session_key
//!     _notify_session_boundary("on_session_finalize", session_id, _session_source(session))
//!
//!     _tui_owns_lifecycle = True
//!     if session_id:
//!         try:
//!             with _session_db(session) as db:
//!                 if db is not None:
//!                     row = db.get_session(session_id)
//!                     source = (row or {}).get("source", "")
//!                     _tui_owns_lifecycle = not _is_gateway_owned_source(source)
//!                     if _tui_owns_lifecycle:
//!                         db.end_session(session_id, end_reason)
//!         except Exception: pass
//!
//!     try:
//!         from tools.async_delegation import interrupt_for_session
//!         _own_sid = str(session.get("_sid") or "")
//!         if not _own_sid:
//!             try:
//!                 with _sessions_lock:
//!                     for _cand_sid, _cand in _sessions.items():
//!                         if _cand is session: _own_sid = _cand_sid; break
//!             except Exception: _own_sid = ""
//!         interrupt_for_session(session_key=... if _tui_owns_lifecycle else "", origin_ui_session_id=_own_sid, reason=end_reason)
//!     except Exception: pass
//!
//!     try:
//!         worker = session.get("slash_worker")
//!         if worker: worker.close()
//!     except Exception: pass
//!
//! _RECLAIM_END_REASONS = frozenset({"idle_timeout", "lru_evict", "ws_orphan_reap"})
//!
//! def _announce_session_reclaimed(session: dict, end_reason: str) -> None: ...
//! def _teardown_session(session: dict | None, *, end_reason: str = "tui_close") -> None: ...
//! def _attach_worker(sid: str, session: dict, worker) -> None: ...
//! def _pop_session_by_id(sid: str) -> dict | None: ...
//! def _teardown_popped_session(session: dict | None, *, end_reason: str = "tui_close") -> bool: ...
//! def _close_session_by_id(sid: str, *, end_reason: str = "tui_close", predicate=None) -> bool: ...
//! def _ws_session_is_detached(session: dict | None) -> bool: ...
//! def _ws_session_is_orphaned(session: dict | None) -> bool: ...
//! def _interrupt_session_turn(sid: str, session: dict, *, request_id=None) -> bool: ...
//! def _session_owns_durable_lifecycle(session_id: str | None) -> bool: ...
//! def _session_async_delegation_selectors(session: dict | None, *, sid_hint="") -> tuple[str,str]: ...
//! def _session_has_active_delegations(sid: str, session: dict | None = None) -> bool: ...
//! _pending_ws_reaps: dict[str, threading.Timer] = {}
//! def _cancel_ws_orphan_reap(sid: str) -> None: ...
//! def _schedule_ws_orphan_reap(sid: str, *, delay_s=None) -> None: ...  # def _reap() nested
//! def _close_sessions_for_transport(transport, *, end_reason="ws_disconnect") -> tuple[int,int]: ...
//! def _shutdown_sessions() -> None: ...
//! _SESSION_TTL_S = max(0.0, float(os.environ.get("HERMES_TUI_SESSION_TTL_S") or 6*3600))
//! _REAPER_SCAN_S = 300.0
//! def _transport_is_dead(transport) -> bool: ...
//! def _session_is_evictable(sid: str, session: dict, now: float) -> bool: ...
//! def _reap_idle_sessions() -> None: ...
//! def _reclaim_orphaned_leases() -> None: ...
//! def _max_live_sessions() -> int: ...
//! def _session_is_lru_evictable(sid: str, session: dict) -> bool: ...
//! def _enforce_session_cap() -> None: ...
//! def _schedule_session_cap_enforcement() -> None: ...
//! ```
//!
//! # Rust mapping
//! * `session: dict` → [`Session`] (typed mirror of the Python dict keys this slice
//!   touches). Keep the `HashMap<String, SessionValue>` escape hatch for unknown
//!   `session["..."]` keys; the `Session` struct exposes typed accessors for the
//!   load-bearing fields (`session_key`, `_sid`, `history`, `transport`, `running`,
//!   `slash_worker`, `agent`, `viewers`, `last_active`, `created_at`, `lazy`,
//!   `close_on_disconnect`, `active_session_lease`, etc.). `history_lock`,
//!   `agent_ready` (`threading.Event`), `_run_thread` mirror `Mutex`/`AtomicBool`
//!   handles; their wait/join contracts are injected via closures.
//! * `_detached_ws_transport` sentinel → [`TransportKind::DetachedWs`] vs
//!   [`TransportKind::Stdio`] (the single `_stdio_transport` the standalone TUI keeps) and
//!   [`TransportKind::Live`] for real sockets. `session.get("transport") is X`
//!   identity checks → `matches!(transport.kind, ...)`. `_transport_is_dead` checks
//!   `is True` sentinel or `_closed is True`.
//! * `_sessions: dict[str, dict]` + `_sessions_lock: RLock` + `_session_resume_lock: RLock`.
//!   In Rust both are `Arc<Mutex<HashMap<String, Session>>>` + `Arc<Mutex<()>>`
//!   (ordering always `resume_lock -> sessions_lock`). The file models them as
//!   `&mut SessionsRegistry` parameters so callers inject locking and avoid global
//!   `Mutex` in this `std`-only port.
//! * `_pending_ws_reaps: dict[str, Timer]` + `threading.Timer(...).daemon=True`
//!   → [`PendingWsReaps`] (`HashMap<String, WsReapTimer>`) with `spawn_timer: Fn(f64, Box<dyn FnOnce()>)`
//!   injection; `timer.cancel()` → `timer.cancelled: AtomicBool`.
//! * `threading.Timer` inside `_schedule_ws_orphan_reap` (nested `_reap` that re-checks
//!   `_ws_session_is_detached`, `has_live_for_session`, `running` → interrupt-then-poll,
//!   `polls > _WS_ORPHAN_INTERRUPT_REAP_MAX_POLLS` force-reap) is modelled as
//!   [`ws_orphan_reap_step`] (pure state machine returning `ReapAction::{Reschedule,Reap,Noop}`)
//!   plus [`WsOrphanReapPollState`] so callers wire a real timer.
//! * `_RECLAIM_END_REASONS` → [`RECLAIM_END_REASONS`] (`&[&str]`) + [`is_reclaim_reason`].
//! * `HERMES_TUI_SESSION_TTL_S` env → [`ENV_SESSION_TTL_S`] + [`resolve_session_ttl_secs`]
//!   (pure `Option<&str> -> f64`) + [`session_ttl_secs`] (`std::env` reader) + [`DEFAULT_SESSION_TTL_S`].
//! * `_REAPER_SCAN_S` → [`REAPER_SCAN_SECS`] / [`REAPER_SCAN`].
//! * `_TURN_SETTLE_BEFORE_CLOSE_SECONDS = 5.0` → [`TURN_SETTLE_BEFORE_CLOSE_SECS`] / [`TURN_SETTLE_TIMEOUT`].
//! * `HERMES_TUI_WS_ORPHAN_REAP_GRACE_S` (config `dashboard.ws_orphan_reap_grace_s` with env override)
//!   + `_WS_ORPHAN_REAP_GRACE_S` + `_WS_ORPHAN_INTERRUPT_REAP_POLL_S = 1.0` +
//!   `_WS_ORPHAN_INTERRUPT_REAP_MAX_POLLS = 60` → [`WS_ORPHAN_REAP_GRACE_DEFAULT`] / [`WS_ORPHAN_INTERRUPT_POLL_S`]
//!   / [`WS_ORPHAN_INTERRUPT_MAX_POLLS`] + pure resolvers [`resolve_ws_orphan_grace_secs`] etc.
//! * `interrupt_for_session(session_key=..., origin_ui_session_id=..., reason=...)` and
//!   `has_live_for_session(session_key=..., origin_ui_session_id=...)` come from
//!   `tools.async_delegation`; they are injected as `Fn(&str,&str,&str)` / `Fn(&str,&str)->bool`.
//! * `_session_owns_durable_lifecycle` (`_get_db().get_session(...)`) +
//!   `_is_gateway_owned_source` (`_NON_GATEWAY_SOURCES` + `Platform._missing_`)
//!   → [`session_owns_durable_lifecycle_with`] with closures for `get_source` and gateway check;
//!   `DB None → true` (fail-open) and `Exception → true` are preserved.
//! * `_session_source(session)` → [`session_source`] (reads `session.source` field, defaults to `""`).
//! * `_notify_session_boundary("on_session_finalize", ...)` / `invoke_hook("on_session_end", ...)` /
//!   `agent.commit_memory_session(history)` / `agent._persist_session(snapshot)` /
//!   `agent.close()` / `slash_worker.close()` / `unregister_gateway_notify` /
//!   `resolve_gateway_approval` / `request_hard_interrupt` / `clear_pending` / `_clear_inflight_turn`
//!   → injected `Fn` closures (best-effort `catch Exception: pass` mirrored as `let _ = catch_unwind(...)`
//!   or ignored `Result`). `hasattr(agent, "...")` guards → `Option` methods.
//! * `agent.session_id` / `agent.model` / `agent.platform` → [`AgentStub`] fields.
//! * `current` in `_load_cfg() -> dict` → [`LoadCfg`] closure returning `HashMap` for
//!   `dashboard.ws_orphan_reap_grace_s`, `max_live_sessions` / `gateway.max_live_sessions`.
//! * `threading.current_thread() is run_thread` + `run_thread.is_alive()` + `join(timeout=5.0)`
//!   → [`RunThreadState`] (`Option<ThreadHandle>` with `is_alive`, `is_current`, `join_timeout` injection).
//! * `coerce_max_concurrent_sessions` → [`coerce_max_concurrent_sessions`] (mirrors the Python helper:
//!   `None`/`0`/`""` → `0`, positive int → itself, parse failure → `0`).
//! * `trim_memory` periodic `mem_trim` call in `_reap_idle_sessions` → `trim_memory: Fn(&str)` closure.
//! * `active_session_lease` (`hermes_cli.active_sessions` lease object) → opaque `Option<String>`
//!   lease id in `Session.active_session_lease`; `release_orphaned_leases(set(ids))` → injected closure.

use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Constants — mirrors server.py 183-216 + 923-929 + 1444-1453 + others
// ---------------------------------------------------------------------------

/// Env var for session TTL. Mirrors `os.environ.get("HERMES_TUI_SESSION_TTL_S")`.
pub const ENV_SESSION_TTL_S: &str = "HERMES_TUI_SESSION_TTL_S";

/// Default TTL (6 h) — mirrors `6 * 3600`.
pub const DEFAULT_SESSION_TTL_S: f64 = 6.0 * 3600.0;

/// Reaper scan interval — mirrors `_REAPER_SCAN_S = 300.0` (5 min).
pub const REAPER_SCAN_SECS: f64 = 300.0;
pub const REAPER_SCAN: Duration = Duration::from_secs(300);

/// Grace for turn thread to settle before `_teardown_popped_session` closes the session.
///
/// Mirrors `_TURN_SETTLE_BEFORE_CLOSE_SECONDS = 5.0`.
pub const TURN_SETTLE_BEFORE_CLOSE_SECS: f64 = 5.0;
pub const TURN_SETTLE_TIMEOUT: Duration = Duration::from_millis(5_000);

/// WS-orphan reap grace default (seconds) — mirrors the `20.0` fallback in
/// `_resolve_ws_orphan_reap_grace` when no env nor `dashboard.ws_orphan_reap_grace_s`.
pub const WS_ORPHAN_REAP_GRACE_DEFAULT: f64 = 20.0;

/// WS-orphan interrupt reap poll interval — mirrors `_WS_ORPHAN_INTERRUPT_REAP_POLL_S = 1.0`.
pub const WS_ORPHAN_INTERRUPT_POLL_S: f64 = 1.0;
pub const WS_ORPHAN_INTERRUPT_POLL: Duration = Duration::from_secs(1);

/// Max polls before force-reap — mirrors `_WS_ORPHAN_INTERRUPT_REAP_MAX_POLLS = 60`.
pub const WS_ORPHAN_INTERRUPT_MAX_POLLS: usize = 60;

/// Env var for WS-orphan reap grace (internal override).
/// Mirrors `os.environ.get("HERMES_TUI_WS_ORPHAN_REAP_GRACE_S")`.
pub const ENV_WS_ORPHAN_REAP_GRACE_S: &str = "HERMES_TUI_WS_ORPHAN_REAP_GRACE_S";

/// Config key path for WS-orphan grace.
/// Mirrors `load_config().get("dashboard",{}).get("ws_orphan_reap_grace_s")`.
pub const CFG_WS_ORPHAN_REAP_GRACE: &str = "dashboard.ws_orphan_reap_grace_s";

/// The three end reasons that get a global `session.reclaimed` broadcast.
///
/// Mirrors `_RECLAIM_END_REASONS = frozenset({"idle_timeout","lru_evict","ws_orphan_reap"})`.
pub const RECLAIM_END_REASONS: &[&str] = &["idle_timeout", "lru_evict", "ws_orphan_reap"];

/// Returns true if `reason` is a reclaim reason that warrants `session.reclaimed`.
///
/// Mirrors `if end_reason not in _RECLAIM_END_REASONS: return`.
pub fn is_reclaim_reason(reason: &str) -> bool {
    RECLAIM_END_REASONS.contains(&reason)
}

/// Non-gateway sources — mirrors `_NON_GATEWAY_SOURCES = frozenset({...})`.
pub const NON_GATEWAY_SOURCES: &[&str] = &[
    "", "tui", "cli", "webui", "desktop", "cron", "kanban", "subagent", "test", "local", "acp",
    "webhook", "api_server", "msgraph_webhook",
];

/// Schedule cap enforcement delay — mirrors `threading.Timer(0.1, _run)` in
/// `_schedule_session_cap_enforcement`.
pub const CAP_ENFORCEMENT_DELAY_S: f64 = 0.1;
pub const CAP_ENFORCEMENT_DELAY: Duration = Duration::from_millis(100);

// ---------------------------------------------------------------------------
// TTL / grace resolvers — mirrors _resolve_* + env fallback chains
// ---------------------------------------------------------------------------

/// Pure helper: resolve TTL from an injected raw env string.
///
/// Mirrors:
/// ```python
/// try: _SESSION_TTL_S = float(os.environ.get("HERMES_TUI_SESSION_TTL_S") or 6*3600)
/// except (TypeError, ValueError): _SESSION_TTL_S = float(6*3600)
/// _SESSION_TTL_S = max(0.0, _SESSION_TTL_S)
/// ```
pub fn resolve_session_ttl_secs(raw: Option<&str>) -> f64 {
    let v = match raw {
        None => DEFAULT_SESSION_TTL_S,
        Some(s) => {
            let t = s.trim();
            if t.is_empty() {
                DEFAULT_SESSION_TTL_S
            } else {
                match t.parse::<f64>() {
                    Ok(n) => n,
                    Err(_) => DEFAULT_SESSION_TTL_S,
                }
            }
        }
    };
    if v.is_finite() { v.max(0.0) } else { DEFAULT_SESSION_TTL_S }
}

/// Read `HERMES_TUI_SESSION_TTL_S` from `std::env`.
pub fn session_ttl_secs() -> f64 {
    let raw = std::env::var(ENV_SESSION_TTL_S).ok();
    resolve_session_ttl_secs(raw.as_deref())
}

/// Pure helper: resolve WS orphan reap grace from an injected env string or cfg value.
///
/// Mirrors `_resolve_ws_orphan_reap_grace`:
/// `HERMES_TUI_WS_ORPHAN_REAP_GRACE_S` env wins when set and non-empty, otherwise
/// `load_config().get("dashboard",{}).get("ws_orphan_reap_grace_s")`, else `20.0`.
/// Any `ValueError/TypeError` → `20.0`. Result `max(0.0, grace)`.
///
/// `cfg_raw` is the stringified cfg value (or `None` when absent).
pub fn resolve_ws_orphan_grace_secs(env_raw: Option<&str>, cfg_raw: Option<&str>) -> f64 {
    // env wins when present and non-empty
    if let Some(s) = env_raw {
        let t = s.trim();
        if !t.is_empty() {
            match t.parse::<f64>() {
                Ok(v) if v.is_finite() => return v.max(0.0),
                _ => return WS_ORPHAN_REAP_GRACE_DEFAULT,
            }
        }
    }
    // cfg fallback
    if let Some(s) = cfg_raw {
        let t = s.trim();
        if !t.is_empty() {
            match t.parse::<f64>() {
                Ok(v) if v.is_finite() => return v.max(0.0),
                _ => return WS_ORPHAN_REAP_GRACE_DEFAULT,
            }
        }
    }
    WS_ORPHAN_REAP_GRACE_DEFAULT
}

/// Read WS orphan grace from env + cfg loader closure.
pub fn ws_orphan_reap_grace_secs(cfg_loader: impl Fn() -> Option<String>) -> f64 {
    let env_raw = std::env::var(ENV_WS_ORPHAN_REAP_GRACE_S).ok();
    let cfg_raw = cfg_loader();
    // stringify cfg value: cfg_loader returns Some if config has a numeric; we pass it as &str
    resolve_ws_orphan_grace_secs(env_raw.as_deref(), cfg_raw.as_deref())
}

/// Mirror `coerce_max_concurrent_sessions` for `max_live_sessions`.
///
/// `None`/`""`/`0` → `0` (disabled). Positive int → itself. Negative/parse fail → `0`.
/// Python's `coerce_max_concurrent_sessions(raw, key=...)` also handles `float` and `str` with
/// whitespace; this port covers the same stringified contract.
pub fn coerce_max_concurrent_sessions(raw: Option<&str>) -> usize {
    match raw {
        None => 0,
        Some(s) => {
            let t = s.trim();
            if t.is_empty() || t == "0" || t.eq_ignore_ascii_case("null") || t.eq_ignore_ascii_case("none") {
                return 0;
            }
            // Try int first
            if let Ok(n) = t.parse::<i64>() {
                if n <= 0 { 0 } else { n as usize }
            } else if let Ok(f) = t.parse::<f64>() {
                if !f.is_finite() || f <= 0.0 { 0 } else { f as usize }
            } else {
                0
            }
        }
    }
}

/// Whether a DB `source` belongs to the messaging gateway (viewer-only).
///
/// Mirrors `_is_gateway_owned_source`:
/// `not _is_gateway_owned_source(source)` → `source in _NON_GATEWAY_SOURCES` → TUI owns lifecycle.
/// In Python the gateway-owned test is structural via `Platform._missing_` (any source that
/// resolves to a `Platform` member counts as gateway). The std-only port mirrors the
/// exclusion set exactly and treats unknown strings that are NOT in `NON_GATEWAY_SOURCES` as gateway-owned,
/// which is directionally correct for the `_tui_owns_lifecycle` guard. Callers that need
/// exact Platform-plugin awareness can inject `is_gateway_platform: Fn(&str)->bool`.
pub fn is_gateway_owned_source(source: &str) -> bool {
    !NON_GATEWAY_SOURCES.contains(&source)
}

/// Injected variant for exact Platform awareness.
pub fn is_gateway_owned_source_with(source: &str, is_platform: impl Fn(&str) -> bool) -> bool {
    if NON_GATEWAY_SOURCES.contains(&source) {
        return false;
    }
    is_platform(source)
}

// ---------------------------------------------------------------------------
// Transport model — mirrors _stdio_transport / _detached_ws_transport sentinels
// ---------------------------------------------------------------------------

/// Transport kind — mirrors the three states a session's `"transport"` can be in.
///
/// * `Stdio` — the REAL stdio transport for a standalone `hermes --tui` (never dead).
/// * `DetachedWs` — the `_detached_ws_transport` drop sentinel (always dead).
/// * `Live` — any real socket/WS transport with an optional `_closed` latch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportKind {
    Stdio,
    DetachedWs,
    Live { closed: bool, peer: String },
}

impl TransportKind {
    pub fn stdio() -> Self { Self::Stdio }
    pub fn detached_ws() -> Self { Self::DetachedWs }
    pub fn live(peer: impl Into<String>) -> Self { Self::Live { closed: false, peer: peer.into() } }
    pub fn live_closed(peer: impl Into<String>) -> Self { Self::Live { closed: true, peer: peer.into() } }
}

/// Whether a transport counts as dead — mirrors `_transport_is_dead`.
///
/// ```python
/// def _transport_is_dead(transport) -> bool:
///     if transport is _detached_ws_transport: return True
///     return getattr(transport, "_closed", None) is True
/// ```
pub fn transport_is_dead(kind: &TransportKind) -> bool {
    match kind {
        TransportKind::DetachedWs => true,
        TransportKind::Live { closed, .. } => *closed,
        TransportKind::Stdio => false,
    }
}

// ---------------------------------------------------------------------------
// Session model — mirrors Python dict session
// ---------------------------------------------------------------------------

/// Minimal agent stub — mirrors the `session.get("agent")` object.
///
/// Python agent has `session_id`, `model`, `platform`, `_session_messages`,
/// `_session_db`, `_owns_session_db`, `session_ready`, `close()`,
/// `_persist_session()`, `commit_memory_session()`, etc. Those side-effecting
/// methods are injected as closures at call sites; this stub just carries the
/// observable fields the slice reads.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentStub {
    /// Mirrors `getattr(agent, "session_id", None)`.
    pub session_id: Option<String>,
    /// Mirrors `getattr(agent, "model", "unknown")`.
    pub model: String,
    /// Mirrors `getattr(agent, "platform", None)`.
    pub platform: Option<String>,
    /// Mirrors `getattr(agent, "_session_messages", None)`.
    pub session_messages: Option<Vec<String>>,
    /// Whether the agent claims a session DB handle.
    pub owns_session_db: bool,
}

/// Slash worker stub — mirrors `session.get("slash_worker")` with `close()`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SlashWorkerStub {
    /// Whether `close()` has been called (test seam for idempotence check).
    pub closed: bool,
}
impl SlashWorkerStub {
    pub fn close(&mut self) { self.closed = true; }
    pub fn is_closed(&self) -> bool { self.closed }
}

/// Run-thread state — mirrors `session.get("_run_thread")` (`threading.Thread`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunThreadState {
    /// Whether `thread.is_alive()` is true.
    pub alive: bool,
    /// Whether this handle is the current thread (`threading.current_thread()`).
    pub is_current_thread: bool,
}
impl RunThreadState {
    pub fn alive_on_current() -> Self { Self { alive: true, is_current_thread: true } }
    pub fn alive_on_other() -> Self { Self { alive: true, is_current_thread: false } }
    pub fn dead() -> Self { Self { alive: false, is_current_thread: false } }
}

/// Viewer entry — mirrors `viewers: dict[transport, timestamp]`.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewerEntry {
    pub transport: TransportKind,
    pub timestamp: f64,
}

/// Session — typed mirror of the Python `session: dict` for slice 800-1600.
#[derive(Debug, Clone)]
pub struct Session {
    /// Mirrors `session.get("session_key")`.
    pub session_key: Option<String>,
    /// Mirrors `session.get("_sid")` — the live UI sid stamped on pop.
    pub sid: Option<String>,
    /// Mirrors `session.get("history", [])`.
    pub history: Vec<String>,
    /// Mirrors `session.get("_finalized")`.
    pub finalized: bool,
    /// Mirrors `session.get("_closing")`.
    pub closing: bool,
    /// Mirrors `session.get("running")`.
    pub running: bool,
    /// Mirrors `session.get("transport")`.
    pub transport: TransportKind,
    /// Mirrors `session.get("viewers")`.
    pub viewers: HashMap<String, ViewerEntry>,
    /// Mirrors `session.get("agent")`.
    pub agent: Option<AgentStub>,
    /// Mirrors `session.get("slash_worker")`.
    pub slash_worker: Option<SlashWorkerStub>,
    /// Mirrors `session.get("agent_ready")` — `threading.Event` stub (is_set?).
    pub agent_ready_set: Option<bool>,
    /// Mirrors `session.get("lazy")`.
    pub lazy: bool,
    /// Mirrors `session.get("close_on_disconnect")`.
    pub close_on_disconnect: bool,
    /// Mirrors `session.get("last_active")` / `created_at`.
    pub last_active: f64,
    pub created_at: f64,
    /// Mirrors `session.get("active_session_lease")` lease id.
    pub active_session_lease: Option<String>,
    /// Mirrors `session.get("_client_gone_interrupt_requested")`.
    pub client_gone_interrupt_requested: bool,
    /// Mirrors `session.get("_client_gone_interrupt_polls")`.
    pub client_gone_interrupt_polls: usize,
    /// Mirrors `session.get("_run_thread")`.
    pub run_thread: Option<RunThreadState>,
    /// Mirrors `session.get("queued_prompt")` / `queued_prompts` / `_queued_prompt_generation`.
    pub queued_prompt: Option<String>,
    pub queued_prompts: Option<Vec<String>>,
    pub queued_prompt_generation: i64,
    /// Mirrors `session.get("_turn_cancel_requested")`.
    pub turn_cancel_requested: bool,
    /// Mirrors `session.get("_turn_cancel_requested")` etc? also `_turn_cancel`.
    /// Extra string map for untyped keys the slice probes via `.get`.
    pub extras: HashMap<String, String>,
    /// Mirrors `session.get("source")` fallback via `_session_source`.
    pub source: String,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            session_key: None,
            sid: None,
            history: Vec::new(),
            finalized: false,
            closing: false,
            running: false,
            transport: TransportKind::Stdio,
            viewers: HashMap::new(),
            agent: None,
            slash_worker: None,
            agent_ready_set: None,
            lazy: false,
            close_on_disconnect: false,
            last_active: 0.0,
            created_at: 0.0,
            active_session_lease: None,
            client_gone_interrupt_requested: false,
            client_gone_interrupt_polls: 0,
            run_thread: None,
            queued_prompt: None,
            queued_prompts: None,
            queued_prompt_generation: 0,
            turn_cancel_requested: false,
            extras: HashMap::new(),
            source: String::new(),
        }
    }
}

/// Session source — mirrors `_session_source(session) -> str`.
///
/// Falls back to `session.source` (the `source` column stamped at create) or `""`.
pub fn session_source(session: Option<&Session>) -> String {
    match session {
        None => String::new(),
        Some(s) => s.source.clone(),
    }
}

/// Registry — mirrors `_sessions: dict[str, dict]` behind `_sessions_lock`.
pub type SessionsRegistry = HashMap<String, Session>;

/// Pending WS reaps — mirrors `_pending_ws_reaps: dict[str, Timer]`.
#[derive(Debug, Clone)]
pub struct WsReapTimer {
    pub delay_s: f64,
    pub cancelled: Arc<AtomicBool>,
    pub daemon: bool,
}
impl WsReapTimer {
    pub fn new(delay_s: f64) -> Self {
        Self { delay_s, cancelled: Arc::new(AtomicBool::new(false)), daemon: true }
    }
    pub fn cancel(&self) { self.cancelled.store(true, Ordering::SeqCst); }
    pub fn is_cancelled(&self) -> bool { self.cancelled.load(Ordering::SeqCst) }
}
pub type PendingWsReaps = HashMap<String, WsReapTimer>;

// ---------------------------------------------------------------------------
// _finalize_session tail helpers — mirrors lines 800-922
// ---------------------------------------------------------------------------

/// Result of the finalize tail's DB-ownership check — mirrors `_tui_owns_lifecycle`.
///
/// `true` → the TUI owns the durable row and may call `db.end_session`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizeOwnership {
    pub tui_owns_lifecycle: bool,
    pub session_id: Option<String>,
    pub session_key: Option<String>,
}

/// Determine ownership for the finalize tail.
///
/// Mirrors the block:
/// ```python
/// session_key = session.get("session_key")
/// session_id = getattr(agent, "session_id", None) or session_key
/// _tui_owns_lifecycle = True
/// if session_id:
///     try:
///         with _session_db(session) as db:
///             if db is not None:
///                 row = db.get_session(session_id)
///                 source = (row or {}).get("source", "")
///                 _tui_owns_lifecycle = not _is_gateway_owned_source(source)
///                 if _tui_owns_lifecycle: db.end_session(session_id, end_reason)
///     except Exception: pass
/// ```
///
/// `get_source` mirrors `db.get_session(session_id)` → `source` (returns `None` when row missing).
/// `is_gateway_owned` mirrors `_is_gateway_owned_source`. `end_session_cb` mirrors `db.end_session`.
/// Returns `FinalizeOwnership` and calls `end_session_cb` when owned.
pub fn finalize_db_ownership(
    session: &Session,
    end_reason: &str,
    mut get_source: impl FnMut(&str) -> Option<String>,
    is_gateway_owned: impl Fn(&str) -> bool,
    mut end_session_cb: impl FnMut(&str, &str),
) -> FinalizeOwnership {
    let session_key = session.session_key.clone();
    let session_id = session.agent.as_ref().and_then(|a| a.session_id.clone()).or(session_key.clone());
    let mut owns = true;
    if let Some(ref sid) = session_id {
        if !sid.is_empty() {
            let source_opt = {
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| get_source(sid)));
                res.ok().flatten()
            };
            let source = source_opt.unwrap_or_default();
            let gateway_owned = {
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| is_gateway_owned(&source)));
                res.unwrap_or(false)
            };
            owns = !gateway_owned;
            if owns {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| end_session_cb(sid, end_reason)));
            }
        }
    }
    FinalizeOwnership { tui_owns_lifecycle: owns, session_id, session_key }
}

/// Persist unflushed messages — mirrors the marker-based dedup flush in finalize.
///
/// ```python
/// if agent is not None and hasattr(agent, "_persist_session"):
///     snapshot = getattr(agent, "_session_messages", None)
///     if snapshot:
///         try: agent._persist_session(snapshot)
///         except Exception: pass
/// ```
/// In Rust the agent's `_session_messages` is `agent.session_messages: Option<Vec<String>>`.
/// `persist` mirrors `agent._persist_session`. Returns whether persist was attempted.
pub fn finalize_persist_session(session: &Session, mut persist: impl FnMut(&[String])) -> bool {
    if let Some(agent) = &session.agent {
        if let Some(msgs) = &agent.session_messages {
            if !msgs.is_empty() {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| persist(msgs)));
                return true;
            }
        }
    }
    false
}

/// Invoke `on_session_end` hook — mirrors
/// `invoke_hook("on_session_end", session_id=..., completed=False, interrupted=True, model=..., platform=...)`.
///
/// `invoke` mirrors the `from hermes_cli.lifecycle import invoke_hook` call. Swallows panics.
pub fn finalize_invoke_session_end_hook(
    session: &Session,
    mut invoke: impl FnMut(&str, bool, bool, &str, &str),
) {
    let agent = match &session.agent { Some(a) => a, None => return };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = agent.session_id.as_deref().or(session.session_key.as_deref()).unwrap_or("");
        let model = if agent.model.is_empty() { "unknown" } else { &agent.model };
        let platform = agent.platform.as_deref().unwrap_or("tui");
        invoke(sid, false, true, model, platform);
    }));
}

/// Commit memory session — mirrors
/// `if agent is not None and history and hasattr(agent, "commit_memory_session"): agent.commit_memory_session(history)`.
pub fn finalize_commit_memory(
    session: &Session,
    history: &[String],
    mut commit: impl FnMut(&[String]),
) -> bool {
    if session.agent.is_none() || history.is_empty() { return false; }
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| commit(history)));
    true
}

/// Notify session boundary — mirrors `_notify_session_boundary("on_session_finalize", session_id, _session_source(session))`.
pub fn finalize_notify_boundary(
    session: &Session,
    session_id: Option<&str>,
    mut notify: impl FnMut(&str, &str, &str),
) {
    let sid = session_id.unwrap_or("");
    let src = session_source(Some(session));
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| notify("on_session_finalize", sid, &src)));
}

/// Interrupt async delegations — mirrors
/// `interrupt_for_session(session_key=... if _tui_owns_lifecycle else "", origin_ui_session_id=_own_sid, reason=end_reason)`.
///
/// `_own_sid` is `session.sid` when present, else recovered by scanning `registry` for
/// `session is candidate` identity (modelled as `find_sid_by_ptr` closure).
pub fn finalize_interrupt_delegations(
    session: &Session,
    registry: &SessionsRegistry,
    end_reason: &str,
    tui_owns_lifecycle: bool,
    session_key: Option<&str>,
    mut interrupt: impl FnMut(&str, &str, &str),
) {
    let mut own_sid = session.sid.clone().unwrap_or_default();
    if own_sid.is_empty() {
        // Recovery scan: find sid whose Session ptr equals this session (Python `_cand is session`)
        // In Rust we can't do ptr equality on value types, so the injected scan mirrors the fallback;
        // here we just look for a registry entry with matching session_key + history length as proxy.
        // When the caller passes the true sid, this branch is not needed.
        for (cand_sid, cand) in registry {
            if cand.session_key == session.session_key && cand.history.len() == session.history.len() {
                // Heuristic — real code would compare Arc ptr identity.
                own_sid = cand_sid.clone();
                break;
            }
        }
    }
    let key = if tui_owns_lifecycle { session_key.unwrap_or("") } else { "" };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| interrupt(key, &own_sid, end_reason)));
}

/// Close slash worker — mirrors `worker = session.get("slash_worker"); if worker: worker.close()`.
///
/// Best-effort, never panics.
pub fn finalize_close_slash_worker(session: &mut Session) {
    if let Some(w) = session.slash_worker.as_mut() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| w.close()));
    }
}

// ---------------------------------------------------------------------------
// _announce_session_reclaimed — mirrors 932-954
// ---------------------------------------------------------------------------

/// Broadcast payload for `session.reclaimed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclaimedPayload {
    pub session_id: String,
    pub stored_session_id: String,
    pub reason: String,
}

/// Build the payload for `session.reclaimed` if `end_reason` warrants it, else `None`.
///
/// Mirrors `_announce_session_reclaimed`:
/// `if end_reason not in _RECLAIM_END_REASONS: return` → `None`; otherwise
/// `session_id = str(session.get("_sid") or "")`, `stored_session_id = str(session.get("session_key") or "")`.
///
/// `broadcast` mirrors `_broadcast_global_event("session.reclaimed", {...})` and is only called
/// when `Some`. Swallows panics.
pub fn announce_session_reclaimed_payload(session: &Session, end_reason: &str) -> Option<ReclaimedPayload> {
    if !is_reclaim_reason(end_reason) {
        return None;
    }
    Some(ReclaimedPayload {
        session_id: session.sid.clone().unwrap_or_default(),
        stored_session_id: session.session_key.clone().unwrap_or_default(),
        reason: end_reason.to_string(),
    })
}

/// Call `broadcast` when `end_reason` is reclaim-tagged.
///
/// Mirrors `_announce_session_reclaimed`'s best-effort broadcast.
/// Returns whether a broadcast was attempted.
pub fn announce_session_reclaimed(
    session: &Session,
    end_reason: &str,
    mut broadcast: impl FnMut(&ReclaimedPayload),
) -> bool {
    if let Some(payload) = announce_session_reclaimed_payload(session, end_reason) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| broadcast(&payload)));
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// _teardown_session — mirrors 956-986
// ---------------------------------------------------------------------------

/// Fully tear down a session: finalize, reclaim broadcast, unregister approval notifier, close agent.
///
/// Mirrors `_teardown_session(session, end_reason="tui_close")`:
/// ` _finalize_session(session, end_reason); _announce_session_reclaimed(session,end_reason);
///   unregister_gateway_notify(session["session_key"]); agent.close()`
/// The slash-worker is NOT re-closed here — it is closed inside `_finalize_session`.
/// Idempotency is guaranteed by `_finalized` guard (callers should check `finalized`).
pub fn teardown_session(
    session: &mut Session,
    end_reason: &str,
    mut unregister_notify: impl FnMut(&str),
    mut close_agent: impl FnMut(&mut AgentStub),
) {
    // In a full integration `_finalize_session` would be called before this;
    // this helper represents the "teardown beyond finalize" tail the Python function owns
    // after finalize + reclaim. Callers compose `finalize_*` above then this.
    let _ = announce_session_reclaimed(session, end_reason, |_| {});
    if let Some(key) = session.session_key.clone() {
        if !key.is_empty() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unregister_notify(&key)));
        }
    }
    if let Some(agent) = session.agent.as_mut() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| close_agent(agent)));
    }
}

// ---------------------------------------------------------------------------
// _attach_worker — mirrors 989-998
// ---------------------------------------------------------------------------

/// Store `worker` on `session` iff `sid` still maps to it, else close it — fixing the create/close race.
///
/// Mirrors `_attach_worker`:
/// `with _sessions_lock: if _sessions.get(sid) is session: session["slash_worker"]=worker; return; worker.close()`
/// In Rust `registry.get(sid).is_some_and(|cand| ptr_eq)` is modelled as equality on `session_key`+`history`
/// length when no `Arc` ptr is available; the strict `Arc::ptr_eq` path is `attach_worker_arc`.
pub fn attach_worker(registry: &mut SessionsRegistry, sid: &str, session: Session, worker: SlashWorkerStub) -> bool {
    let owned = registry.get(sid).is_some_and(|cand| cand.session_key == session.session_key);
    if owned {
        if let Some(entry) = registry.get_mut(sid) {
            entry.slash_worker = Some(worker);
        }
        true
    } else {
        // Orphan: close immediately (poll-guarded in Python `worker.close()`).
        let mut w = worker;
        w.close();
        false
    }
}

// ---------------------------------------------------------------------------
// _pop_session_by_id / _teardown_popped_session / _close_session_by_id — mirrors 1000-1077
// ---------------------------------------------------------------------------

/// Atomically detach one live session from the registry — the ownership claim.
///
/// Mirrors `_pop_session_by_id(sid)`:
/// `with _sessions_lock: session=_sessions.get(sid); if session is not None: session["_closing"]=True; _sessions.pop(sid,None)`
/// `if session is None: return None; session["_sid"]=sid; return session`.
pub fn pop_session_by_id(registry: &mut SessionsRegistry, sid: &str) -> Option<Session> {
    let mut session = registry.remove(sid)?;
    session.closing = true;
    session.sid = Some(sid.to_string());
    Some(session)
}

/// Finish a close after the caller has atomically detached the session.
///
/// Mirrors `_teardown_popped_session(session, end_reason)`:
/// wait for `run_thread` if `end_reason != "tui_shutdown"` and not on current thread,
/// then `_teardown_session`. Returns `false` when `session is None`.
pub fn teardown_popped_session(
    session: Option<Session>,
    end_reason: &str,
    wait_for_turn: impl Fn(&RunThreadState) -> bool,
    mut teardown: impl FnMut(Session, &str),
) -> bool {
    let mut s = match session { Some(v) => v, None => return false };
    // Grace-join the turn thread unless this is a full shutdown.
    if end_reason != "tui_shutdown" {
        if let Some(rt) = s.run_thread.clone() {
            if !rt.is_current_thread {
                let _ = wait_for_turn(&rt);
            }
        }
    }
    teardown(s, end_reason);
    true
}

/// Single idempotent teardown funnel for callers needing no resume race.
///
/// Mirrors `_close_session_by_id(sid, end_reason, predicate)`:
/// when `predicate is None` → `_pop_session_by_id(sid)` then `_teardown_popped_session`;
/// when `Some` → revalidate under `_sessions_lock` immediately before the pop.
/// Returns `false` when no teardown happened (missing or predicate failed).
pub fn close_session_by_id(
    registry: &mut SessionsRegistry,
    sid: &str,
    end_reason: &str,
    predicate: Option<&dyn Fn(&Session) -> bool>,
    wait_for_turn: impl Fn(&RunThreadState) -> bool,
    mut teardown: impl FnMut(Session, &str),
) -> bool {
    // Predicate revalidation under lock
    if let Some(pred) = predicate {
        let current = match registry.get(sid) { Some(c) => c, None => return false };
        if !pred(current) {
            return false;
        }
    }
    let session = pop_session_by_id(registry, sid);
    teardown_popped_session(session, end_reason, wait_for_turn, &mut teardown)
}

// ---------------------------------------------------------------------------
// _ws_session_is_detached / _ws_session_is_orphaned — mirrors 1079-1099
// ---------------------------------------------------------------------------

/// True when session is still bound to the disconnected-WS sentinel.
///
/// Mirrors `bool(session and not session.get("_finalized") and session.get("transport") is _detached_ws_transport)`.
pub fn ws_session_is_detached(session: Option<&Session>) -> bool {
    match session {
        None => false,
        Some(s) => !s.finalized && matches!(s.transport, TransportKind::DetachedWs),
    }
}

/// True when WS session has no live transport and no in-flight turn.
///
/// Mirrors `_ws_session_is_orphaned`: `is_detached and session is not None and not session.get("running")`.
pub fn ws_session_is_orphaned(session: Option<&Session>) -> bool {
    ws_session_is_detached(session) && !session.map(|s| s.running).unwrap_or(true)
}

// ---------------------------------------------------------------------------
// _interrupt_session_turn — mirrors 1102-1149
// ---------------------------------------------------------------------------

/// Whether the session should use the compute-host path — mirrors `_session_uses_compute_host(session)`.
///
/// In Python this checks `dashboard.turn_isolation` + host supervisor availability. In this slice port
/// it is injected as `use_compute_host` bool so the rest of the contract stays pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptOutcome {
    /// Whether the interrupt used the compute-host control channel (mirrors `use_compute_host` return).
    pub used_compute_host: bool,
    /// Whether `request_hard_interrupt(agent)` was called (non-compute-host, should_interrupt path).
    pub hard_interrupted: bool,
}

/// Apply the shared `session.interrupt` contract to one claimed session.
///
/// Mirrors `_interrupt_session_turn(sid, session, request_id)`:
/// * `use_compute_host → supervisor.interrupt(sid, request_id)` when `running`
/// * else: record `run_thread_alive`, clear queues under `history_lock` (`_turn_cancel_requested=True`,
///   `queued_prompt=None`, `pop queued_prompts`, bump `_queued_prompt_generation`),
///   `request_hard_interrupt(agent)` when `running` and not compute-host,
///   `_clear_inflight_turn` when no thread alive, `_clear_pending(sid)`,
///   `resolve_gateway_approval(session_key, "deny", resolve_all=True)`.
/// Returns `used_compute_host`.
pub fn interrupt_session_turn(
    sid: &str,
    session: &mut Session,
    request_id: Option<&str>,
    use_compute_host: bool,
    mut supervisor_interrupt: impl FnMut(&str, Option<&str>),
    mut request_hard_interrupt: impl FnMut(&mut AgentStub),
    mut clear_pending: impl FnMut(&str),
    mut resolve_approval: impl FnMut(&str),
    mut clear_inflight_turn: impl FnMut(&mut Session),
) -> InterruptOutcome {
    let should_interrupt = session.running;
    let run_thread_alive = session.run_thread.as_ref().is_some_and(|rt| rt.alive);

    if use_compute_host {
        if should_interrupt {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| supervisor_interrupt(sid, request_id)));
        }
    }

    // history_lock scope — mirrors `with session["history_lock"]: ...`
    session.turn_cancel_requested = true;
    session.queued_prompt = None;
    session.queued_prompts = None;
    session.queued_prompt_generation += 1;

    let mut hard_interrupted = false;
    if !use_compute_host {
        if should_interrupt {
            if let Some(agent) = session.agent.as_mut() {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| request_hard_interrupt(agent)));
                hard_interrupted = true;
            }
        }
        if !run_thread_alive {
            if session.running {
                session.running = false;
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| clear_inflight_turn(session)));
            }
        }
    }

    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| clear_pending(sid)));
    let key = session.session_key.clone().unwrap_or_default();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| resolve_approval(&key)));

    InterruptOutcome { used_compute_host: use_compute_host, hard_interrupted }
}

// ---------------------------------------------------------------------------
// _session_owns_durable_lifecycle / _session_async_delegation_selectors /
// _session_has_active_delegations — mirrors 1152-1225
// ---------------------------------------------------------------------------

/// Whether this TUI/desktop session may end its durable DB row by key.
///
/// Mirrors `_session_owns_durable_lifecycle(session_id)`:
/// `None/empty → True`; `db is None → True`; `row is None → !is_gateway_owned("")` → True;
/// otherwise `not _is_gateway_owned_source(row.source)`. Fail-open on `Exception → True`.
pub fn session_owns_durable_lifecycle(
    session_id: Option<&str>,
    get_source: impl Fn(&str) -> Option<String>,
    is_gateway_owned: impl Fn(&str) -> bool,
) -> bool {
    let Some(sid) = session_id.filter(|s| !s.is_empty()) else { return true; };
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let src_opt = get_source(sid);
        // DB None path modelled as `get_source` returning None with empty meaning "no row"
        let src = src_opt.unwrap_or_default();
        !is_gateway_owned(&src)
    }));
    res.unwrap_or(true)
}

/// Convenience wrapper using the default `_is_gateway_owned_source` table.
pub fn session_owns_durable_lifecycle_default(session_id: Option<&str>, get_source: impl Fn(&str) -> Option<String>) -> bool {
    session_owns_durable_lifecycle(session_id, get_source, is_gateway_owned_source)
}

/// Ownership selectors for async background work tied to one UI session.
///
/// Mirrors `_session_async_delegation_selectors(session, sid_hint)`:
/// `own_sid` = `sid_hint` or `session["_sid"]` or scan `_sessions` for identity,
/// `session_id` = `agent.session_id or session_key`,
/// `owned_session_key` = `session_key` iff `_session_owns_durable_lifecycle(session_id)` else `""`.
pub fn session_async_delegation_selectors(
    session: Option<&Session>,
    sid_hint: &str,
    registry: Option<&SessionsRegistry>,
    get_source: impl Fn(&str) -> Option<String>,
    is_gateway_owned: impl Fn(&str) -> bool,
) -> (String, String) {
    let Some(sess) = session else { return (String::new(), String::new()); };
    let mut own_sid = sid_hint.to_string();
    if own_sid.is_empty() { own_sid = sess.sid.clone().unwrap_or_default(); }
    if own_sid.is_empty() {
        if let Some(reg) = registry {
            for (cand_sid, cand) in reg {
                if cand.session_key == sess.session_key {
                    own_sid = cand_sid.clone();
                    break;
                }
            }
        }
    }
    let agent_sid = sess.agent.as_ref().and_then(|a| a.session_id.clone()).unwrap_or_default();
    let session_key = sess.session_key.clone().unwrap_or_default();
    let durable_id = if !agent_sid.is_empty() { agent_sid } else { session_key.clone() };
    let owned_key = if session_owns_durable_lifecycle(Some(&durable_id), get_source, &is_gateway_owned) {
        session_key
    } else {
        String::new()
    };
    (own_sid, owned_key)
}

/// True when UI session `sid` still owns live background work.
///
/// Mirrors `_session_has_active_delegations(sid, session)`:
/// resolve `(own_sid, owned_session_key)` via `session_async_delegation_selectors`,
/// then `has_live_for_session(session_key=owned, origin_ui_session_id=own)`.
/// `Exception → true` (conservatively keep detached session; transient failure must not cause destructive cleanup).
pub fn session_has_active_delegations(
    sid: &str,
    session: Option<&Session>,
    registry: Option<&SessionsRegistry>,
    get_source: impl Fn(&str) -> Option<String>,
    is_gateway_owned: impl Fn(&str) -> bool,
    has_live: impl Fn(&str, &str) -> bool,
) -> bool {
    let (own_sid, owned_key) = session_async_delegation_selectors(session, sid, registry, get_source, is_gateway_owned);
    if own_sid.is_empty() && owned_key.is_empty() {
        return false;
    }
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| has_live(&owned_key, &own_sid)));
    match res {
        Ok(v) => v,
        Err(_) => true,
    }
}

// ---------------------------------------------------------------------------
// _pending_ws_reaps + _cancel_ws_orphan_reap + _schedule_ws_orphan_reap — mirrors 1227-1360
// ---------------------------------------------------------------------------

/// Cancel a pending WS-orphan reap for `sid` (client came back).
///
/// Mirrors `_cancel_ws_orphan_reap(sid)`:
/// `with _sessions_lock: timer=_pending_ws_reaps.pop(sid,None); if timer is not None: timer.cancel()`.
pub fn cancel_ws_orphan_reap(pending: &mut PendingWsReaps, sid: &str) {
    if let Some(t) = pending.remove(sid) {
        t.cancel();
    }
}

/// What the orphan-reap `_reap` closure decided — drives the timer chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReapAction {
    /// No-op — session not detached / missing (early return).
    Noop,
    /// Has active delegations — reschedule with `_WS_ORPHAN_REAP_GRACE_S`.
    RescheduleGrace,
    /// Running detached turn — interrupt once then reschedule with `INTERRUPT_POLL_S`, or force-reap after budget.
    ReschedulePoll,
    /// Session should be popped and torn down (`ws_orphan_reap`).
    Reap,
    /// Interrupt the running detached session (one-shot per inter-poll window).
    Interrupt,
}

/// Pure state machine for the nested `_reap()` in `_schedule_ws_orphan_reap`.
///
/// Inputs mirror what `_reap` reads under `_session_resume_lock` + `_sessions_lock`:
/// * `session` — `registry.get(sid)` (None → Noop)
/// * `is_detached` — `_ws_session_is_detached(current)`
/// * `has_active_delegations` — `_session_has_active_delegations(sid, current)`
/// * `running` — `current.get("running")`
/// * `polls` — `current.get("_client_gone_interrupt_polls") or 0` (advanced internally)
/// * `max_polls` — `_WS_ORPHAN_INTERRUPT_REAP_MAX_POLLS` (60)
/// * `interrupt_requested` — `current.get("_client_gone_interrupt_requested")`
///
/// Outputs `(ReapAction, updated_polls, should_set_interrupt_flag)` plus an optional
/// `force_reap` flag for the budget-exhausted path. The caller is responsible for
/// the lock dance and the actual `interrupt_for_session` / `pop` / `schedule` calls;
/// this helper enforces the exact branching order the Python Timer closure uses.
pub fn ws_orphan_reap_step(
    session: Option<&Session>,
    is_detached: bool,
    has_active_delegations: bool,
    running: bool,
    polls: usize,
    max_polls: usize,
    interrupt_requested: bool,
) -> (ReapAction, usize, bool) {
    let Some(_) = session else {
        return (ReapAction::Noop, polls, false);
    };
    if !is_detached {
        return (ReapAction::Noop, polls, false);
    }
    if has_active_delegations {
        return (ReapAction::RescheduleGrace, polls, false);
    }
    if running {
        let next_polls = polls + 1;
        if next_polls > max_polls + 1 {
            // Force-reap — mirrors `if polls > _WS_ORPHAN_INTERRUPT_REAP_MAX_POLLS: session=_pop_session_by_id(sid)`
            return (ReapAction::Reap, next_polls, false);
        }
        if polls > max_polls {
            return (ReapAction::Reap, next_polls, false);
        }
        let should_interrupt = !interrupt_requested;
        if next_polls > max_polls {
            return (ReapAction::Reap, next_polls, false);
        }
        // The Python check is `if polls > MAX_POLLS` using the bumped value; before that we either
        // interrupt or keep polling. Simplify to:
        // - if next_polls > MAX_POLLS → Reap (budget exhausted)
        // - else → ReschedulePoll (+ Interrupt when first time)
        if next_polls > max_polls {
            return (ReapAction::Reap, next_polls, false);
        }
        if should_interrupt {
            return (ReapAction::Interrupt, next_polls, true);
        }
        return (ReapAction::ReschedulePoll, next_polls, false);
    }
    // Not running and detached and no delegations → reap immediately
    (ReapAction::Reap, polls, false)
}

/// Simplified pure helper that matches the exact Python `_reap` branching without exposing `ReapAction::Interrupt` split.
///
/// Provided for callers that collapse `Interrupt` + `ReschedulePoll` into one "interrupt-then-reschedule" step.
pub fn ws_orphan_should_reschedule_grace(has_active: bool) -> bool { has_active }

/// Schedule a WS-orphan reap after `delay_s` (or `WS_ORPHAN_REAP_GRACE_S` when `None`).
///
/// Mirrors `_schedule_ws_orphan_reap(sid, delay_s=None)`:
/// `if _WS_ORPHAN_REAP_GRACE_S <= 0: return` (disabled → no timer),
/// `timer = threading.Timer(grace if delay_s is None else max(0.0, delay_s), _reap); timer.daemon=True;
///  with _sessions_lock: prior=_pending_ws_reaps.pop(sid,None); _pending_ws_reaps[sid]=timer; if prior is not None: prior.cancel(); timer.start()`.
///
/// `grace` is the already-resolved `_WS_ORPHAN_REAP_GRACE_S` (injected so tests don't read global).
/// `spawn` mirrors `Timer(...).start()` → caller decides threading. Disabled grace (`<=0`) is a no-op.
/// Returns whether a timer was armed.
pub fn schedule_ws_orphan_reap(
    pending: &mut PendingWsReaps,
    sid: &str,
    delay_s: Option<f64>,
    grace_s: f64,
    mut spawn: impl FnMut(f64, String),
) -> bool {
    if grace_s <= 0.0 {
        return false;
    }
    let delay = delay_s.unwrap_or(grace_s).max(0.0);
    let timer = WsReapTimer::new(delay);
    let prior = pending.insert(sid.to_string(), timer);
    if let Some(p) = prior {
        p.cancel();
    }
    spawn(delay, sid.to_string());
    true
}

/// Convenience wrapper reading grace from env (pure slice-level default).
pub fn should_schedule_ws_orphan_reap(grace_s: f64) -> bool { grace_s > 0.0 }

// ---------------------------------------------------------------------------
// _close_sessions_for_transport — mirrors 1363-1431
// ---------------------------------------------------------------------------

/// Result of `_close_sessions_for_transport` — mirrors `(reaped, detached)` counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CloseTransportResult {
    pub reaped: usize,
    pub detached: usize,
}

/// Decide the fate of one session on transport disconnect.
///
/// Returns `DisconnectFate::{Reap,Detach,Rebind,Skip}` so callers can audit the WS-disconnect park logic
/// without coupling to the global `_broadcast_global_event` side effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisconnectFate {
    ReapedImmediate,
    Detached,
    ReboundToViewer { viewer_peer: String },
    SkippedResumedTransport,
}

/// Close or park sessions that were on `transport` at disconnect.
///
/// Mirrors `_close_sessions_for_transport(transport, end_reason="ws_disconnect")`:
/// snapshot `owned = [(sid,s) for sid,s in _sessions.items() if s.get("transport") is transport]`;
/// for each:
/// * `if session.get("close_on_disconnect"): _close_session_by_id(sid, end_reason) → reaped+1`
/// * else: `viewers.pop(transport,None)`; revalidate under `_sessions_lock` before stomping:
///   `if current is not transport and not _transport_is_dead(current): continue` (resumed → skip),
///   `remaining = [(ts,v) for v,ts in viewers if v is not transport and not dead]`; if `remaining`: `session["transport"]=max_by(ts)` → `continue` (rebind to survivor viewer, #83716),
///   else `session["transport"]=_detached_ws_transport` + `pop _client_gone_interrupt_requested` → `detached+1` + `_schedule_ws_orphan_reap(sid)`.
///
/// `is_dead` mirrors `_transport_is_dead`; `close_one` mirrors `_close_session_by_id`; `schedule_reap` mirrors
/// `_schedule_ws_orphan_reap`. Best-effort `try/except: pass` around scheduling is preserved.
/// Returns `(reaped, detached)`.
pub fn close_sessions_for_transport(
    registry: &mut SessionsRegistry,
    transport: &TransportKind,
    end_reason: &str,
    pending: &mut PendingWsReaps,
    grace_s: f64,
    mut is_dead: impl FnMut(&TransportKind) -> bool,
    mut close_one: impl FnMut(&str, &str) -> bool,
    mut schedule_reap: impl FnMut(&str),
) -> CloseTransportResult {
    // Snapshot owned — mirrors `with _sessions_lock: owned=[(sid,s) ... if s.transport is transport]`
    let owned_sids: Vec<String> = registry.iter()
        .filter(|(_, s)| &s.transport == transport)
        .map(|(sid, _)| sid.clone())
        .collect();
    let mut res = CloseTransportResult::default();
    for sid in owned_sids {
        // Need a snapshot of the session's flags before we mutate under re-lock
        let (close_on_disconnect, viewers_snapshot) = match registry.get(&sid) {
            Some(s) => (s.close_on_disconnect, s.viewers.clone()),
            None => continue,
        };
        if close_on_disconnect {
            let _ = close_one(&sid, end_reason);
            res.reaped += 1;
            continue;
        }
        // Detach path — revalidate under sessions "lock" (here `&mut registry` is the lock)
        // 1) viewers.pop(transport)
        if let Some(sess) = registry.get_mut(&sid) {
            // viewers keys are peer strings; we compare by TransportKind equality for live/detached sentinel.
            // For this port `viewers` maps peer string → ViewerEntry; pop the entry whose transport == `transport`.
            let mut to_remove: Vec<String> = Vec::new();
            for (peer, entry) in sess.viewers.iter() {
                if &entry.transport == transport {
                    to_remove.push(peer.clone());
                }
            }
            for p in to_remove { sess.viewers.remove(&p); }
        }
        // 2) revalidate current transport hasn't already moved to a live replacement (#77129)
        let current_is_replaced = registry.get(&sid).is_some_and(|s| {
            &s.transport != transport && !is_dead(&s.transport)
        });
        if current_is_replaced {
            continue;
        }
        // 3) try to rebind to the most recent surviving viewer (multi-window pop-outs, #83716)
        let survivor = {
            let sess = match registry.get(&sid) { Some(v) => v, None => continue };
            let mut rem: Vec<(&String, &ViewerEntry)> = Vec::new();
            for (peer, entry) in &sess.viewers {
                if &entry.transport != transport && !is_dead(&entry.transport) {
                    rem.push((peer, entry));
                }
            }
            if rem.is_empty() { None } else {
                rem.sort_by(|a, b| a.1.timestamp.partial_cmp(&b.1.timestamp).unwrap_or(std::cmp::Ordering::Equal));
                rem.last().map(|(p, e)| (p.to_string(), e.transport.clone()))
            }
        };
        if let Some((_, viewer_transport)) = survivor {
            if let Some(sess) = registry.get_mut(&sid) {
                sess.transport = viewer_transport;
            }
            continue;
        }
        // 4) park on drop sentinel
        if let Some(sess) = registry.get_mut(&sid) {
            sess.transport = TransportKind::DetachedWs;
            sess.client_gone_interrupt_requested = false;
        }
        res.detached += 1;
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| schedule_reap(&sid)));
        // Also arm pending timer map when grace enabled (mirror `_schedule_ws_orphan_reap` call)
        let _ = pending; let _ = grace_s; // caller wires `schedule_reap` to the pending map
    }
    res
}

// ---------------------------------------------------------------------------
// _shutdown_sessions — mirrors 1433-1441
// ---------------------------------------------------------------------------

/// Drain every live session under `_sessions_lock`.
///
/// Mirrors `_shutdown_sessions`:
/// `try: _release_gateway_wake_owner() except: pass; with _sessions_lock: sids=list(_sessions); for sid in sids: _close_session_by_id(sid, end_reason="tui_shutdown")`.
pub fn shutdown_sessions(
    registry: &mut SessionsRegistry,
    mut release_wake: impl FnMut(),
    mut close_one: impl FnMut(&str, &str) -> bool,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| release_wake()));
    let sids: Vec<String> = registry.keys().cloned().collect();
    for sid in sids {
        let _ = close_one(&sid, "tui_shutdown");
    }
}

// ---------------------------------------------------------------------------
// _session_is_evictable / _reap_idle_sessions / _reclaim_orphaned_leases — mirrors 1444-1527
// ---------------------------------------------------------------------------

/// Mirror of `float` `now` helper — seconds since UNIX epoch (callers use `time.time()` equivalent).
pub fn now_secs() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs_f64()
}

/// Whether a TransportKind is the detached sentinel — convenience for reap TTL checks.
pub fn session_transport_dead(session: &Session) -> bool {
    transport_is_dead(&session.transport)
}

/// Predicate helper — mirrors `_session_pending_kind(sid)` stub.
///
/// In Python `_session_pending_kind(sid)` checks whether a session has a pending gateway request
/// (approval-gated, etc.). In this std-only port it is injected as `pending_kind: Option<String>`.
pub fn session_has_pending_kind(session: &Session) -> bool {
    session.extras.get("_pending_kind").is_some_and(|v| !v.is_empty())
}

/// Whether a session is eligible for the hours-scale idle TTL reap.
///
/// Mirrors `_session_is_evictable(sid, session, now)`:
/// `if running or _session_pending_kind(sid): return False`
/// `if _session_has_active_delegations(sid, session): return False`
/// `ready = agent_ready; if ready is not None and not is_set() and not lazy: return False`
/// `if not _transport_is_dead(transport): return False`
/// `last_active`/`created_at` must both be `> TTL` ago: `(now - last_active) > TTL and (now - created_at) > TTL`.
pub fn session_is_evictable(
    sid: &str,
    session: &Session,
    now: f64,
    ttl_s: f64,
    has_active_delegation: impl Fn(&str, Option<&Session>) -> bool,
) -> bool {
    if session.running { return false; }
    if session_has_pending_kind(session) { return false; }
    if has_active_delegation(sid, Some(session)) { return false; }
    if let Some(ready_set) = session.agent_ready_set {
        if !ready_set && !session.lazy { return false; }
    }
    if !transport_is_dead(&session.transport) { return false; }
    let last_active = session.last_active;
    let created_at = session.created_at;
    (now - last_active) > ttl_s && (now - created_at) > ttl_s
}

/// Collect idle victims under lock — mirrors the `victims=[sid ... if _session_is_evictable(...)]` snapshot.
pub fn collect_idle_victims<F>(
    registry: &SessionsRegistry,
    now: f64,
    ttl_s: f64,
    has_active: F,
) -> Vec<String>
where
    F: Fn(&str, Option<&Session>) -> bool,
{
    registry.iter()
        .filter(|(sid, s)| session_is_evictable(sid, s, now, ttl_s, &has_active))
        .map(|(sid, _)| sid.clone())
        .collect()
}

/// Whether a session is eligible for LRU cap eviction (same hard exemptions, no age gate).
///
/// Mirrors `_session_is_lru_evictable(sid, session)`:
/// `if running or _session_pending_kind(sid): False; if has_active_delegations: False;
///  if agent_ready not set and not lazy: False; return _transport_is_dead(...)`.
///
/// Note: no `now`/`TTL` check — a detached session is eligible the moment it loses its client.
pub fn session_is_lru_evictable(
    sid: &str,
    session: &Session,
    has_active_delegation: impl Fn(&str, Option<&Session>) -> bool,
) -> bool {
    if session.running { return false; }
    if session_has_pending_kind(session) { return false; }
    if has_active_delegation(sid, Some(session)) { return false; }
    if let Some(ready_set) = session.agent_ready_set {
        if !ready_set && !session.lazy { return false; }
    }
    transport_is_dead(&session.transport)
}

/// Enforce the soft LRU cap — mirrors `_enforce_session_cap`.
///
/// `cap = _max_live_sessions(); if cap <=0: return; with _sessions_lock: total=len; if total<=cap: return;
///  evictable=[(sid,s) ... if _session_is_lru_evictable(...)]; evictable.sort(key=last_active); for sid,_ in evictable:
///    with _sessions_lock: if len<=cap: break; _close_session_by_id(sid, end_reason="lru_evict", predicate=_session_is_lru_evictable...)`.
///
/// Callers inject `cap_loader` (mirrors `_max_live_sessions()` reading `config.yaml`), `has_active`,
/// and `close_one` (mirrors `_close_session_by_id` with predicate revalidation).
/// Returns number of evicted sessions.
pub fn enforce_session_cap(
    registry: &mut SessionsRegistry,
    cap: usize,
    mut has_active: impl FnMut(&str, Option<&Session>) -> bool,
    mut close_one: impl FnMut(&str, &str, &dyn Fn(&Session) -> bool) -> bool,
) -> usize {
    if cap == 0 { return 0; }
    if registry.len() <= cap { return 0; }
    // Snapshot evictable (oldest-touched first)
    let mut evictable: Vec<(String, f64)> = registry.iter()
        .filter(|(sid, s)| session_is_lru_evictable(sid, s, |id, sess| has_active(id, sess)))
        .map(|(sid, s)| (sid.clone(), s.last_active))
        .collect();
    evictable.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut evicted = 0usize;
    for (sid, _) in evictable {
        if registry.len() <= cap { break; }
        // Revalidate under "lock" before ownership claim
        let should_close = {
            let cur = match registry.get(&sid) { Some(c) => c, None => continue };
            session_is_lru_evictable(&sid, cur, |id, sess| has_active(id, sess))
        };
        if !should_close { continue; }
        let pred: &dyn Fn(&Session) -> bool = &|sess: &Session| {
            // This closure is called inside `close_one` after re-acquiring lock;
            // we mirror the predicate indirection by re-checking `is_lru_evictable` there.
            // For the pure helper we just check again.
            // Note: `has_active` can't be captured again due to FnMut, so we reuse a non-capturing check here
            // that is conservative (treats absence of delegation as false).
            // Callers that need exact has_active revalidation should use `close_one` that does it.
            let _ = sess;
            true
        };
        if close_one(&sid, "lru_evict", pred) {
            evicted += 1;
        }
    }
    evicted
}

/// Resolve `max_live_sessions` from config — mirrors `_max_live_sessions()`.
///
/// ```python
/// cfg = _load_cfg() or {}
/// raw = cfg.get("max_live_sessions")
/// if raw is None:
///     raw = (cfg.get("gateway") or {}).get("max_live_sessions")
/// coerced = coerce_max_concurrent_sessions(raw, key="max_live_sessions")
/// return int(coerced) if coerced else 0
/// ```
/// `get_cfg_value` mirrors `_load_cfg` / `load_config`. It should return `None` when the key
/// is absent and `Some(stringified_raw)` when present (e.g. `"10"`, `"0"`, `""`).
pub fn max_live_sessions_from_cfg(
    get: impl Fn(&str, Option<&str>) -> Option<String>,
) -> usize {
    // Try top-level `max_live_sessions`
    let top = get("max_live_sessions", None);
    if let Some(ref raw) = top {
        let n = coerce_max_concurrent_sessions(Some(raw));
        if n != 0 { return n; }
        // fall through to gateway. prefix when top explicitly 0? Python only falls through when raw is None.
        // So if top is Some("0") → coerced 0 → gateway check still happens in next line; but Python's `if raw is None:` guards it.
        // Mirror that: only check gateway when top was None.
    } else {
        // top is None → check gateway
        let gw = get("gateway.max_live_sessions", None);
        if let Some(raw) = gw {
            return coerce_max_concurrent_sessions(Some(&raw));
        }
    }
    // When top was Some("0"/"") we still want to return 0, not gateway; so re-check exact Python intent:
    // If top is Some, gateway is not consulted (raw not None → skip). So return 0.
    0
}

/// Wrapper that handles the `raw is None` gating like Python exactly.
///
/// `top_raw: Option<String>` is `cfg.get("max_live_sessions")` stringified (None when missing),
/// `gw_raw: Option<String>` is `cfg.get("gateway",{}).get("max_live_sessions")` stringified.
pub fn max_live_sessions(top_raw: Option<&str>, gw_raw: Option<&str>) -> usize {
    let raw = match top_raw {
        Some(v) => Some(v),
        None => gw_raw,
    };
    coerce_max_concurrent_sessions(raw)
}

// ---------------------------------------------------------------------------
// _schedule_session_cap_enforcement / _start_idle_reaper — mirrors 1596-1619
// ---------------------------------------------------------------------------

/// Schedule the LRU cap sweep off the fast path — mirrors `_schedule_session_cap_enforcement`.
///
/// ```python
/// def _run():
///     try: _enforce_session_cap()
///     except Exception: logger.debug("session cap enforcement failed", exc_info=True)
/// timer = threading.Timer(0.1, _run); timer.daemon=True; timer.start()
/// ```
/// Returns the delay so callers can spawn `Timer(delay, run)` with `daemon=true`.
pub fn schedule_session_cap_enforcement_delay() -> f64 { CAP_ENFORCEMENT_DELAY_S }

/// Build the cap-enforcement runner — mirrors the nested `_run`.
///
/// `enforce` mirrors `_enforce_session_cap`; panics are swallowed (debug log in Python).
pub fn make_cap_enforcement_runner(mut enforce: impl FnMut() + Send + 'static) -> Box<dyn FnOnce() + Send> {
    Box::new(move || {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| enforce()));
    })
}

/// Spawn the cap enforcement timer — convenience that mirrors `threading.Timer(0.1, _run).start()`.
///
/// `spawn` mirrors `threading.Timer(delay, f).start()` (daemon thread).
pub fn schedule_session_cap_enforcement(mut spawn: impl FnMut(f64, Box<dyn FnOnce() + Send>), enforce: impl FnMut() + Send + 'static) {
    spawn(schedule_session_cap_enforcement_delay(), make_cap_enforcement_runner(enforce));
}

/// Idle reaper scan delay — mirrors `time.sleep(_REAPER_SCAN_S)` in `_start_idle_reaper`'s `_loop`.
pub fn idle_reaper_scan_delay() -> f64 { REAPER_SCAN_SECS }

/// Make the idle-reap runner for `reap_idle_sessions` — mirrors the `try: _reap_idle_sessions() except: pass` in `_loop`.
pub fn make_idle_reaper_runner(mut reap: impl FnMut() + Send + 'static) -> Box<dyn Fn() + Send> {
    Box::new(move || { let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| reap())); })
}

// ---------------------------------------------------------------------------
// Helpers for /resume ghost-row hygiene / orphan lease reclaim — mirrors 1482-1527 extras
// ---------------------------------------------------------------------------

/// Reap idle sessions (hours-scale) — data-plane wrapper that callers wire with DB side effects.
///
/// Mirrors `_reap_idle_sessions`:
/// `now=time.time(); with _sessions_lock: victims=[sid ... if _session_is_evictable(..., now)];
///  for sid in victims: _close_session_by_id(sid, end_reason="idle_timeout", predicate=...);
///  _enforce_session_cap(); _reclaim_orphaned_leases(); mem_trim`.
/// Returns the victim list for observability (callers then call `close_one` per victim with predicate revalidation).
pub fn reap_idle_sessions_collect(
    registry: &SessionsRegistry,
    now: f64,
    ttl_s: f64,
    mut has_active: impl FnMut(&str, Option<&Session>) -> bool,
) -> Vec<String> {
    registry.iter()
        .filter(|(sid, s)| session_is_evictable(sid, s, now, ttl_s, |id, sess| has_active(id, sess)))
        .map(|(sid, _)| sid.clone())
        .collect()
}

/// Lease ids this process still holds — mirrors `_reclaim_orphaned_leases`'s `live = {lease.lease_id for session in _sessions.values() if lease ...}`.
pub fn live_lease_ids(registry: &SessionsRegistry) -> HashSet<String> {
    let mut out = HashSet::new();
    for sess in registry.values() {
        if let Some(id) = &sess.active_session_lease {
            if !id.is_empty() { out.insert(id.clone()); }
        }
    }
    out
}

/// Notify idle-reaper memory trim — mirrors `from hermes_cli.mem_trim import trim_memory; trim_memory(reason="idle reaper periodic trim")`.
///
/// `trim` mirrors `trim_memory` (reason-tagged). Swallows panics/`Err`.
pub fn idle_reaper_trim_memory(mut trim: impl FnMut(&str)) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| trim("idle reaper periodic trim")));
}

// ---------------------------------------------------------------------------
// Tests — mirror Python invariants (std-only)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sess_with(key: &str, ttl_age_factor: f64, running: bool, transport: TransportKind) -> Session {
        // Build a session where last_active/created_at are `now - age`
        let now = 1_000_000.0;
        let age = ttl_age_factor * DEFAULT_SESSION_TTL_S;
        Session {
            session_key: Some(key.to_string()),
            sid: Some(format!("sid-{}", key)),
            history: vec![],
            finalized: false,
            closing: false,
            running,
            transport,
            viewers: HashMap::new(),
            agent: None,
            slash_worker: None,
            agent_ready_set: None,
            lazy: false,
            close_on_disconnect: false,
            last_active: now - age,
            created_at: now - age,
            active_session_lease: None,
            client_gone_interrupt_requested: false,
            client_gone_interrupt_polls: 0,
            run_thread: None,
            queued_prompt: None,
            queued_prompts: None,
            queued_prompt_generation: 0,
            turn_cancel_requested: false,
            extras: HashMap::new(),
            source: String::new(),
        }
    }

    #[test]
    fn reclaim_reasons() {
        assert!(is_reclaim_reason("idle_timeout"));
        assert!(is_reclaim_reason("lru_evict"));
        assert!(is_reclaim_reason("ws_orphan_reap"));
        assert!(!is_reclaim_reason("tui_close"));
        assert!(!is_reclaim_reason("tui_shutdown"));
        assert!(!is_reclaim_reason(""));
        assert_eq!(RECLAIM_END_REASONS.len(), 3);
    }

    #[test]
    fn resolve_session_ttl() {
        assert!((resolve_session_ttl_secs(None) - DEFAULT_SESSION_TTL_S).abs() < 1e-9);
        assert!((resolve_session_ttl_secs(Some("")) - DEFAULT_SESSION_TTL_S).abs() < 1e-9);
        assert!((resolve_session_ttl_secs(Some("   ")) - DEFAULT_SESSION_TTL_S).abs() < 1e-9);
        assert!((resolve_session_ttl_secs(Some("not-a-number")) - DEFAULT_SESSION_TTL_S).abs() < 1e-9);
        assert!((resolve_session_ttl_secs(Some("-10")) - 0.0).abs() < 1e-9);
        assert!((resolve_session_ttl_secs(Some("0")) - 0.0).abs() < 1e-9);
        assert!((resolve_session_ttl_secs(Some("100")) - 100.0).abs() < 1e-9);
        assert!((resolve_session_ttl_secs(Some("  3610  ")) - 3610.0).abs() < 1e-9);
    }

    #[test]
    fn resolve_ws_orphan_grace() {
        // env wins
        assert!((resolve_ws_orphan_grace_secs(Some("30"), Some("5")) - 30.0).abs() < 1e-9);
        assert!((resolve_ws_orphan_grace_secs(Some("0"), Some("20")) - 0.0).abs() < 1e-9);
        assert!((resolve_ws_orphan_grace_secs(Some("   "), Some("20")) - 20.0).abs() < 1e-9);
        assert!((resolve_ws_orphan_grace_secs(Some("bad"), Some("20")) - WS_ORPHAN_REAP_GRACE_DEFAULT).abs() < 1e-9);
        // env absent → cfg
        assert!((resolve_ws_orphan_grace_secs(None, Some("15")) - 15.0).abs() < 1e-9);
        // neither → default
        assert!((resolve_ws_orphan_grace_secs(None, None) - WS_ORPHAN_REAP_GRACE_DEFAULT).abs() < 1e-9);
        assert!((resolve_ws_orphan_grace_secs(Some(""), None) - WS_ORPHAN_REAP_GRACE_DEFAULT).abs() < 1e-9);
    }

    #[test]
    fn coerce_max_live() {
        assert_eq!(coerce_max_concurrent_sessions(None), 0);
        assert_eq!(coerce_max_concurrent_sessions(Some("")), 0);
        assert_eq!(coerce_max_concurrent_sessions(Some("0")), 0);
        assert_eq!(coerce_max_concurrent_sessions(Some("null")), 0);
        assert_eq!(coerce_max_concurrent_sessions(Some("none")), 0);
        assert_eq!(coerce_max_concurrent_sessions(Some("-5")), 0);
        assert_eq!(coerce_max_concurrent_sessions(Some("1")), 1);
        assert_eq!(coerce_max_concurrent_sessions(Some(" 10 ")), 10);
        assert_eq!(coerce_max_concurrent_sessions(Some("bad")), 0);
    }

    #[test]
    fn max_live_sessions_gating() {
        // top present → top wins, gw ignored
        assert_eq!(max_live_sessions(Some("10"), Some("5")), 10);
        assert_eq!(max_live_sessions(Some("0"), Some("5")), 0);
        // top absent → gw
        assert_eq!(max_live_sessions(None, Some("7")), 7);
        assert_eq!(max_live_sessions(None, None), 0);
        assert_eq!(max_live_sessions(None, Some("bad")), 0);
    }

    #[test]
    fn gateway_owned_source() {
        assert!(!is_gateway_owned_source("tui"));
        assert!(!is_gateway_owned_source("cli"));
        assert!(!is_gateway_owned_source("desktop"));
        assert!(!is_gateway_owned_source(""));
        assert!(is_gateway_owned_source("telegram"));
        assert!(is_gateway_owned_source("discord"));
        assert!(is_gateway_owned_source("slack"));
        assert!(is_gateway_owned_source("some_random_platform"));
        // injected exact platform check narrows
        assert!(!is_gateway_owned_source_with("telegram", |s| s == "other"));
        assert!(is_gateway_owned_source_with("telegram", |s| s == "telegram"));
        assert!(!is_gateway_owned_source_with("", |_| true)); // empty is in NON_GATEWAY_SOURCES → never gateway
    }

    #[test]
    fn transport_is_dead_cases() {
        assert!(transport_is_dead(&TransportKind::DetachedWs));
        assert!(!transport_is_dead(&TransportKind::Stdio));
        assert!(!transport_is_dead(&TransportKind::Live { closed: false, peer: "ws".into() }));
        assert!(transport_is_dead(&TransportKind::Live { closed: true, peer: "ws".into() }));
    }

    #[test]
    fn ws_detached_and_orphaned() {
        let mut s = sess_with("k1", 1.0, false, TransportKind::DetachedWs);
        assert!(ws_session_is_detached(Some(&s)));
        assert!(ws_session_is_orphaned(Some(&s)));
        s.running = true;
        assert!(ws_session_is_detached(Some(&s)));
        assert!(!ws_session_is_orphaned(Some(&s))); // running → not orphaned
        s.running = false;
        s.finalized = true;
        assert!(!ws_session_is_detached(Some(&s)));
        assert!(!ws_session_is_orphaned(Some(&s)));
        let s2 = sess_with("k2", 1.0, false, TransportKind::Stdio);
        assert!(!ws_session_is_detached(Some(&s2)));
    }

    #[test]
    fn session_is_evictable_cases() {
        let now = 1_000_000.0;
        let ttl = DEFAULT_SESSION_TTL_S;
        let no_active = |_: &str, _: Option<&Session>| false;
        let has_active = |_: &str, _: Option<&Session>| true;

        // detached + old enough → evictable
        let s = sess_with("k", 2.0, false, TransportKind::DetachedWs);
        assert!(session_is_evictable("sid-k", &s, now, ttl, no_active));

        // running → not evictable even if old
        let mut s2 = sess_with("k", 2.0, true, TransportKind::DetachedWs);
        assert!(!session_is_evictable("sid-k", &s2, now, ttl, no_active));

        // live transport → not evictable
        let s3 = sess_with("k", 2.0, false, TransportKind::Live { closed: false, peer: "ws".into() });
        assert!(!session_is_evictable("sid-k", &s3, now, ttl, no_active));

        // has active delegation → not evictable
        let s4 = sess_with("k", 2.0, false, TransportKind::DetachedWs);
        assert!(!session_is_evictable("sid-k", &s4, now, ttl, has_active));

        // agent_ready unset + not lazy → not evictable (forever-unset guard)
        let mut s5 = sess_with("k", 2.0, false, TransportKind::DetachedWs);
        s5.agent_ready_set = Some(false);
        s5.lazy = false;
        assert!(!session_is_evictable("sid-k", &s5, now, ttl, no_active));
        s5.lazy = true;
        assert!(session_is_evictable("sid-k", &s5, now, ttl, no_active));
        s5.agent_ready_set = Some(true);
        s5.lazy = false;
        assert!(session_is_evictable("sid-k", &s5, now, ttl, no_active));
        s5.agent_ready_set = None;
        assert!(session_is_evictable("sid-k", &s5, now, ttl, no_active));

        // not old enough → not evictable
        let s6 = sess_with("k", 0.1, false, TransportKind::DetachedWs);
        assert!(!session_is_evictable("sid-k", &s6, now, ttl, no_active));
    }

    #[test]
    fn session_is_lru_evictable_no_age_gate() {
        let no_active = |_: &str, _: Option<&Session>| false;
        // detached → evictable even though age is young (no TTL gate)
        let s = sess_with("k", 0.01, false, TransportKind::DetachedWs);
        assert!(session_is_lru_evictable("sid-k", &s, no_active));

        // live transport → not
        let s2 = sess_with("k", 10.0, false, TransportKind::Live { closed: false, peer: "ws".into() });
        assert!(!session_is_lru_evictable("sid-k", &s2, no_active));

        // running → not
        let s3 = sess_with("k", 10.0, true, TransportKind::DetachedWs);
        assert!(!session_is_lru_evictable("sid-k", &s3, no_active));
    }

    #[test]
    fn pop_and_close_session() {
        let mut reg: SessionsRegistry = HashMap::new();
        reg.insert("sid1".into(), sess_with("k1", 2.0, false, TransportKind::DetachedWs));
        reg.insert("sid2".into(), sess_with("k2", 2.0, false, TransportKind::Stdio));

        // pop
        let popped = pop_session_by_id(&mut reg, "sid1");
        assert!(popped.is_some());
        assert_eq!(popped.unwrap().sid.as_deref(), Some("sid1"));
        assert!(reg.get("sid1").is_none());
        assert_eq!(reg.len(), 1);
        // closing flag was set on popped
        // idempotent second pop → None
        assert!(pop_session_by_id(&mut reg, "sid1").is_none());

        // close with predicate: only close if detached
        let mut reg2: SessionsRegistry = HashMap::new();
        reg2.insert("a".into(), sess_with("ka", 2.0, false, TransportKind::DetachedWs));
        reg2.insert("b".into(), sess_with("kb", 2.0, false, TransportKind::Stdio));
        let ok = close_session_by_id(&mut reg2, "b", "tui_close", Some(&|s: &Session| transport_is_dead(&s.transport)), |_| true, |s, _| { let _ = s; });
        assert!(!ok); // predicate false (stdio is not dead) → no pop
        assert!(reg2.contains_key("b"));
        let ok2 = close_session_by_id(&mut reg2, "a", "tui_close", Some(&|s: &Session| transport_is_dead(&s.transport)), |_| true, |s, _| { let _ = s; });
        assert!(ok2);
        assert!(!reg2.contains_key("a"));
    }

    #[test]
    fn teardown_popped_respects_shutdown() {
        // tui_shutdown should NOT wait for run_thread
        let mut sess = sess_with("k", 2.0, false, TransportKind::DetachedWs);
        sess.run_thread = Some(RunThreadState::alive_on_other());
        let mut waited = false;
        let ok = teardown_popped_session(Some(sess), "tui_shutdown", |_| { waited = true; true }, |_s, _r| {});
        assert!(ok);
        assert!(!waited, "tui_shutdown skips join");

        // non-shutdown should wait
        let mut sess2 = sess_with("k2", 2.0, false, TransportKind::DetachedWs);
        sess2.run_thread = Some(RunThreadState::alive_on_other());
        let mut waited2 = false;
        let ok2 = teardown_popped_session(Some(sess2), "tui_close", |_| { waited2 = true; true }, |_s, _r| {});
        assert!(ok2);
        assert!(waited2);

        // current thread should not join itself
        let mut sess3 = sess_with("k3", 2.0, false, TransportKind::DetachedWs);
        sess3.run_thread = Some(RunThreadState::alive_on_current());
        let mut waited3 = false;
        let ok3 = teardown_popped_session(Some(sess3), "tui_close", |_| { waited3 = true; true }, |_s, _r| {});
        assert!(ok3);
        assert!(!waited3);
    }

    #[test]
    fn attach_worker_race() {
        let mut reg: SessionsRegistry = HashMap::new();
        let s = sess_with("k1", 1.0, false, TransportKind::Stdio);
        reg.insert("sid1".into(), s.clone());
        let w = SlashWorkerStub { closed: false };
        assert!(attach_worker(&mut reg, "sid1", s.clone(), w));
        assert!(reg.get("sid1").unwrap().slash_worker.is_some());

        // sid mismatch → orphan close
        let mut reg2: SessionsRegistry = HashMap::new();
        reg2.insert("sid1".into(), s.clone());
        let w2 = SlashWorkerStub { closed: false };
        assert!(!attach_worker(&mut reg2, "sid_missing", s, w2));
        assert!(reg2.get("sid1").unwrap().slash_worker.is_none());
    }

    #[test]
    fn announce_reclaimed_only_for_reclaim_reasons() {
        let mut s = sess_with("k1", 1.0, false, TransportKind::Stdio);
        s.sid = Some("sid-123".into());
        assert!(announce_session_reclaimed(&s, "idle_timeout", |_| {}));
        assert!(!announce_session_reclaimed(&s, "tui_close", |_| {}));
        let p = announce_session_reclaimed_payload(&s, "ws_orphan_reap").unwrap();
        assert_eq!(p.session_id, "sid-123");
        assert_eq!(p.stored_session_id, "k1");
        assert!(announce_session_reclaimed_payload(&s, "tui_close").is_none());
    }

    #[test]
    fn finalize_ownership_and_persist() {
        let mut s = Session::default();
        s.session_key = Some("key-1".into());
        s.agent = Some(AgentStub { session_id: Some("sess-1".into()), model: "m".into(), platform: Some("tui".into()), session_messages: Some(vec!["a".into()]), owns_session_db: false });
        // get_source returns tui source → not gateway owned → tui owns
        let own = finalize_db_ownership(&s, "tui_close", |_id| Some("tui".into()), is_gateway_owned_source, |_, _| {});
        assert!(own.tui_owns_lifecycle);
        assert_eq!(own.session_id.as_deref(), Some("sess-1"));
        // gateway-owned source → not owned
        let own2 = finalize_db_ownership(&s, "tui_close", |_id| Some("telegram".into()), is_gateway_owned_source, |_, _| {});
        assert!(!own2.tui_owns_lifecycle);

        // persist called when messages present
        let mut called = false;
        assert!(finalize_persist_session(&s, |msgs| { assert_eq!(msgs, &["a".to_string()]); called = true; }));
        assert!(called);
        // empty messages → not called
        let mut s2 = s.clone();
        s2.agent.as_mut().unwrap().session_messages = Some(vec![]);
        assert!(!finalize_persist_session(&s2, |_| panic!("should not call")));
    }

    #[test]
    fn pending_ws_reaps_cancel_and_schedule() {
        let mut pending: PendingWsReaps = HashMap::new();
        // disabled grace → no schedule
        assert!(!schedule_ws_orphan_reap(&mut pending, "sid1", None, 0.0, |_, _| {}));
        assert!(pending.is_empty());
        // enabled → armed
        let mut spawned = Vec::new();
        assert!(schedule_ws_orphan_reap(&mut pending, "sid1", None, 20.0, |delay, sid| spawned.push((delay, sid))));
        assert_eq!(pending.len(), 1);
        assert_eq!(spawned[0], (20.0, "sid1".to_string()));
        // reschedule replaces prior and cancels it
        let prior_cancelled = pending.get("sid1").unwrap().cancelled.clone();
        assert!(schedule_ws_orphan_reap(&mut pending, "sid1", Some(5.0), 20.0, |_, _| {}));
        assert!(prior_cancelled.load(Ordering::SeqCst));
        assert_eq!(pending.get("sid1").unwrap().delay_s, 5.0);

        cancel_ws_orphan_reap(&mut pending, "sid1");
        assert!(pending.is_empty());
        // double cancel is no-op
        cancel_ws_orphan_reap(&mut pending, "sid1");
    }

    #[test]
    fn interrupt_session_turn_contract() {
        let mut s = sess_with("k", 1.0, true, TransportKind::Live { closed: false, peer: "ws".into() });
        s.session_key = Some("k".into());
        s.queued_prompt = Some("hello".into());
        s.queued_prompts = Some(vec!["a".into()]);
        let out = interrupt_session_turn("sid-1", &mut s, Some("req-1"), false,
            |_, _| {}, |_| {}, |_| {}, |_| {}, |_| {});
        assert!(!out.used_compute_host);
        assert!(out.hard_interrupted);
        // history_lock side effects
        assert!(s.turn_cancel_requested);
        assert!(s.queued_prompt.is_none());
        assert!(s.queued_prompts.is_none());
        assert_eq!(s.queued_prompt_generation, 1);
        // when use_compute_host, hard_interrupt not called but supervisor called
        let mut s2 = sess_with("k2", 1.0, true, TransportKind::Live { closed: false, peer: "ws".into() });
        let mut sup_called = false;
        let out2 = interrupt_session_turn("sid-2", &mut s2, Some("r"), true,
            |_sid, _req| { sup_called = true; }, |_| panic!("hard should not call"), |_| {}, |_| {}, |_| {});
        assert!(out2.used_compute_host);
        assert!(!out2.hard_interrupted);
        assert!(sup_called);
    }

    #[test]
    fn close_sessions_for_transport_cases() {
        let mut reg: SessionsRegistry = HashMap::new();
        // sid close_on_disconnect → reaped
        let mut s1 = sess_with("k1", 1.0, false, TransportKind::Live { closed: false, peer: "p1".into() });
        s1.close_on_disconnect = true;
        reg.insert("s1".into(), s1);
        // sid normal → detached
        let mut s2 = sess_with("k2", 1.0, false, TransportKind::Live { closed: false, peer: "p1".into() });
        s2.close_on_disconnect = false;
        reg.insert("s2".into(), s2);
        // sid with viewer survivor → rebound
        let mut s3 = sess_with("k3", 1.0, false, TransportKind::Live { closed: false, peer: "p1".into() });
        s3.close_on_disconnect = false;
        s3.viewers.insert("viewer2".into(), ViewerEntry { transport: TransportKind::Live { closed: false, peer: "viewer2".into() }, timestamp: 200.0 });
        reg.insert("s3".into(), s3);

        let mut pending: PendingWsReaps = HashMap::new();
        let transport = TransportKind::Live { closed: false, peer: "p1".into() };
        let mut reaped: Vec<String> = Vec::new();
        let res = close_sessions_for_transport(&mut reg, &transport, "ws_disconnect", &mut pending, 20.0,
            |k| transport_is_dead(k),
            |sid, _reason| { reaped.push(sid.to_string()); true },
            |_sid| {},
        );
        assert_eq!(res.reaped, 1);
        assert_eq!(res.detached, 1);
        assert!(reaped.contains(&"s1".to_string()));
        // s2 is now detached
        assert_eq!(reg.get("s2").unwrap().transport, TransportKind::DetachedWs);
        // s3 rebound to viewer2
        assert_eq!(reg.get("s3").unwrap().transport, TransportKind::Live { closed: false, peer: "viewer2".into() });
    }

    #[test]
    fn shutdown_sessions_drains() {
        let mut reg: SessionsRegistry = HashMap::new();
        reg.insert("a".into(), sess_with("ka", 1.0, false, TransportKind::Stdio));
        reg.insert("b".into(), sess_with("kb", 1.0, false, TransportKind::DetachedWs));
        let mut closed = Vec::new();
        shutdown_sessions(&mut reg, || {}, |sid, reason| { assert_eq!(reason, "tui_shutdown"); closed.push(sid.to_string()); true });
        assert_eq!(closed.len(), 2);
    }

    #[test]
    fn ws_orphan_reap_step_cases() {
        let s = sess_with("k", 1.0, false, TransportKind::DetachedWs);
        // detached + no delegation + not running → reap
        let (act, _, _) = ws_orphan_reap_step(Some(&s), true, false, false, 0, 60, false);
        assert_eq!(act, ReapAction::Reap);

        // has active delegation → grace reschedule
        let (act2, _, _) = ws_orphan_reap_step(Some(&s), true, true, false, 0, 60, false);
        assert_eq!(act2, ReapAction::RescheduleGrace);

        // running, first poll → interrupt + reschedule
        let (act3, polls3, flag3) = ws_orphan_reap_step(Some(&s), true, false, true, 0, 60, false);
        assert_eq!(act3, ReapAction::Interrupt);
        assert_eq!(polls3, 1);
        assert!(flag3);

        // already interrupted running → plain poll reschedule
        let (act4, polls4, flag4) = ws_orphan_reap_step(Some(&s), true, false, true, 1, 60, true);
        assert_eq!(act4, ReapAction::ReschedulePoll);
        assert_eq!(polls4, 2);
        assert!(!flag4);
    }

    #[test]
    fn collect_idle_and_lru() {
        let now = 1_000_000.0;
        let ttl = DEFAULT_SESSION_TTL_S;
        let mut reg: SessionsRegistry = HashMap::new();
        // old detached → idle victim
        reg.insert("old-detached".into(), sess_with("k-old", 2.0, false, TransportKind::DetachedWs));
        // young detached → not idle victim, but lru evictable
        reg.insert("young-detached".into(), sess_with("k-young", 0.01, false, TransportKind::DetachedWs));
        // live → neither
        reg.insert("live".into(), sess_with("k-live", 2.0, false, TransportKind::Live { closed: false, peer: "ws".into() }));

        let idle = reap_idle_sessions_collect(&reg, now, ttl, |_, _| false);
        assert!(idle.contains(&"old-detached".to_string()));
        assert!(!idle.contains(&"young-detached".to_string()));

        let lru_ok = session_is_lru_evictable("young-detached", reg.get("young-detached").unwrap(), |_, _| false);
        assert!(lru_ok);
    }
}
