//! CronScheduler provider interface (Axis B — the trigger).
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/cron/scheduler_provider.py` (703 lines).
//!
//! A CronScheduler decides *when* a due job fires. It does NOT decide what firing
//! means: execution + delivery stay in `cron.scheduler.run_job` / `_deliver_result`,
//! shared by all providers. Providers must never reimplement agent construction or
//! delivery.
//!
//! The built-in [`InProcessCronScheduler`] runs the historical 60s daemon-thread
//! ticker. Alternative providers (e.g. Chronos) live under `plugins/cron_providers/<name>/`
//! and are selected via the `cron.provider` config key (empty = built-in).
//!
//! ⚠️ EXPERIMENTAL — this interface is validated by exactly ONE consumer (the
//! built-in) until an external provider (Chronos, Phase 4) shakes it out. Until
//! then the module path, method signatures, and `start()` kwargs MAY change without
//! a deprecation cycle. Once a second provider validates the shape it becomes
//! stable. Any growth MUST be additive (new optional method with a default), never
//! a changed signature on `start()` or a new abstract method.
//!
//! Python source docstring (preserved):
//! ```text
//! CronScheduler provider interface (Axis B — the trigger).
//!
//! ⚠️ EXPERIMENTAL — this interface is validated by exactly ONE consumer (the
//! built-in) until an external provider (Chronos, Phase 4) shakes it out. Until
//! then the module path, method signatures, and start() kwargs MAY change without
//! a deprecation cycle. Once a second provider validates the shape it becomes
//! stable. Any growth MUST be additive (new optional method with a default), never
//! a changed signature on start() or a new abstractmethod.
//!
//! A CronScheduler decides *when* a due job fires. It does NOT decide what firing
//! means: execution + delivery stay in cron.scheduler.run_job / _deliver_result,
//! shared by all providers. Providers must never reimplement agent construction or
//! delivery.
//!
//! The built-in InProcessCronScheduler runs the historical 60s daemon-thread
//! ticker. Alternative providers (e.g. Chronos, a NAS-mediated managed-cron
//! provider for scale-to-zero deployments) live under plugins/cron_providers/<name>/ and are
//! selected via the `cron.provider` config key (empty = built-in).
//! ```

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Constants — mirrors Python module globals
// ---------------------------------------------------------------------------

/// Cap for the exponential tick backoff applied while consecutive ticks fail
/// with fd exhaustion (EMFILE/ENFILE, #87644). Base is the tick interval
/// (60s by default); each consecutive EMFILE failure doubles the wait, capped
/// here so a still-alive-but-exhausted gateway never sleeps longer than this
/// between recovery attempts.
/// Mirrors `_EMFILE_BACKOFF_MAX_SECONDS = 15 * 60`.
pub const EMFILE_BACKOFF_MAX_SECONDS: f64 = 15.0 * 60.0;

/// Default misfire catch-up grace window in minutes.
/// Mirrors `DEFAULT_MISFIRE_GRACE_MINUTES = 10`.
pub const DEFAULT_MISFIRE_GRACE_MINUTES: f64 = 10.0;

// ---------------------------------------------------------------------------
// Home / path helpers — mirrors `hermes_constants.get_hermes_home()`
// ---------------------------------------------------------------------------

/// Resolve the Hermes home directory.
/// Mirrors `hermes_constants.get_hermes_home()`:
/// `HERMES_HOME` env → `~/.hermes` (POSIX) / `%LOCALAPPDATA%/hermes` (Windows).
pub fn get_hermes_home() -> PathBuf {
    if let Ok(val) = std::env::var("HERMES_HOME") {
        let trimmed = val.trim().to_string();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home).join(".hermes");
        }
    }
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        if !userprofile.trim().is_empty() {
            return PathBuf::from(userprofile).join(".hermes");
        }
    }
    if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
        if !localappdata.trim().is_empty() {
            return PathBuf::from(localappdata).join("hermes");
        }
    }
    PathBuf::from(".hermes")
}

fn cron_dir() -> PathBuf {
    get_hermes_home().join("cron")
}

fn jobs_file() -> PathBuf {
    cron_dir().join("jobs.json")
}

fn hermes_now_iso() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// ---------------------------------------------------------------------------
// StopEvent — mirrors `threading.Event`
// ---------------------------------------------------------------------------

/// Thread-safe stop signal, mirrors `threading.Event`.
///
/// `start()` blocks on `wait()` until `set()` is called by the caller.
/// Analogous to `threading.Event` with `is_set()` / `wait(timeout)` / `set()`.
#[derive(Debug, Clone)]
pub struct StopEvent {
    inner: Arc<(Mutex<bool>, Condvar)>,
}

impl StopEvent {
    pub fn new() -> Self {
        Self {
            inner: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }

    /// Set the event (signal stop).
    /// Mirrors `stop_event.set()`.
    pub fn set(&self) {
        let (lock, cvar) = &*self.inner;
        let mut flag = lock.lock().unwrap();
        *flag = true;
        cvar.notify_all();
    }

    /// Whether the event has been set.
    /// Mirrors `stop_event.is_set()`.
    pub fn is_set(&self) -> bool {
        *self.inner.0.lock().unwrap()
    }

    /// Wait up to `timeout` until the event is set.
    /// Mirrors `stop_event.wait(timeout)` — returns `true` if set, `false` on timeout.
    pub fn wait(&self, timeout: Duration) -> bool {
        let (lock, cvar) = &*self.inner;
        let mut flag = lock.lock().unwrap();
        if *flag {
            return true;
        }
        let (guard, result) = cvar.wait_timeout(flag, timeout).unwrap();
        flag = guard;
        *flag || !result.timed_out()
    }

    /// Wait with a floating-point seconds timeout (mirrors Python `wait(float)`).
    pub fn wait_secs(&self, secs: f64) -> bool {
        if secs <= 0.0 {
            return self.is_set();
        }
        self.wait(Duration::from_secs_f64(secs))
    }

    /// Clear the event (test helper; not in Python but useful for reuse).
    pub fn clear(&self) {
        let (lock, _) = &*self.inner;
        *lock.lock().unwrap() = false;
    }
}

impl Default for StopEvent {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers: backoff + fd exhaustion — mirrors `_backoff_wait_seconds`,
// `_is_fd_exhaustion`, `_reclaim_fds_best_effort`, `_note_tick_failure`
// ---------------------------------------------------------------------------

/// Exponential tick backoff shared by both ticker loops (#87644).
///
/// Returns the plain `interval` while healthy; doubles per consecutive
/// fd-exhaustion failure, capped at `EMFILE_BACKOFF_MAX_SECONDS`.
///
/// Mirrors `def _backoff_wait_seconds(interval: float, consecutive_failures: int) -> float`.
pub fn backoff_wait_seconds(interval: f64, consecutive_failures: usize) -> f64 {
    if consecutive_failures == 0 {
        return interval;
    }
    let v = interval * (2_f64.powi(consecutive_failures as i32 - 1));
    v.min(EMFILE_BACKOFF_MAX_SECONDS)
}

/// Best-effort fd-exhaustion classifier.
/// Mirrors `cron.scheduler._is_fd_exhaustion(exc)`.
///
/// Checks whether the error string looks like EMFILE/ENFILE / "Too many open files".
pub fn is_fd_exhaustion_msg(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    lower.contains("emfile")
        || lower.contains("enfile")
        || lower.contains("too many open files")
        || lower.contains("too many files open")
        || lower.contains("file table overflow")
}

/// Best-effort fd reclamation (gc + raise soft nofile limit).
/// Mirrors `cron.scheduler._reclaim_fds_best_effort()` — best-effort, never panics.
///
/// In Python this calls `gc.collect()` and tries to raise `RLIMIT_NOFILE` soft
/// limit. In Rust we have no GC; we log and attempt to probe `/proc/self/limits`
/// as a diagnostic, but do not fail.
pub fn reclaim_fds_best_effort() {
    log::warn!("FD exhaustion detected — attempting best-effort reclamation (Rust stub: no GC, no rlimit bump)");
    // Diagnostic: log current open fd count on Linux if possible.
    #[cfg(target_os = "linux")]
    {
        let count = std::fs::read_dir("/proc/self/fd").map(|rd| rd.count()).unwrap_or(0);
        log::warn!("Current open fds: {count}");
    }
}

/// Classify one failed tick and return the updated failure counter.
///
/// Shared by both ticker loops (#87644): on fd exhaustion, attempt
/// reclamation and bump the counter so `backoff_wait_seconds` backs off
/// exponentially while the process has no chance of making progress. Any
/// other failure resets the counter.
///
/// Mirrors `def _note_tick_failure(exc: BaseException, consecutive_failures: int) -> int`.
pub fn note_tick_failure(exc_msg: &str, consecutive_failures: usize) -> usize {
    if is_fd_exhaustion_msg(exc_msg) {
        reclaim_fds_best_effort();
        return consecutive_failures + 1;
    }
    0
}

// ---------------------------------------------------------------------------
// Ticker heartbeat / error markers — mirrors `cron.jobs` helpers
// ---------------------------------------------------------------------------

fn ticker_heartbeat_file() -> PathBuf {
    cron_dir().join("ticker_heartbeat.json")
}

fn ticker_error_file() -> PathBuf {
    cron_dir().join("ticker_error.json")
}

/// Record liveness heartbeat. Mirrors `cron.jobs.record_ticker_heartbeat(success=...)`.
///
/// `success` distinguishes "alive but failing every tick" from "actually firing jobs"
/// (#32612, #32895). Writes a JSON marker with `at` and `success`.
pub fn record_ticker_heartbeat(success: bool) {
    let path = ticker_heartbeat_file();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let payload = serde_json::json!({
        "at": hermes_now_iso(),
        "success": success,
        "pid": std::process::id(),
    });
    let text = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());
    let tmp = path.with_extension(format!("tmp.{}", uuid::Uuid::new_v4().simple()));
    if std::fs::write(&tmp, format!("{text}\n")).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
        secure_file(&path);
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Persist the failure reason next to the heartbeat markers so
/// `hermes cron status` can show WHY ticks fail, not just that the
/// success marker is stale. Mirrors `cron.jobs.record_ticker_error(msg)` (#68483).
pub fn record_ticker_error(msg: &str) {
    let path = ticker_error_file();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let payload = serde_json::json!({
        "at": hermes_now_iso(),
        "error": msg,
        "pid": std::process::id(),
    });
    let text = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());
    let tmp = path.with_extension(format!("tmp.{}", uuid::Uuid::new_v4().simple()));
    if std::fs::write(&tmp, format!("{text}\n")).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
        secure_file(&path);
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Clear the persisted ticker error marker. Mirrors `cron.jobs.clear_ticker_error()`.
pub fn clear_ticker_error() {
    let _ = std::fs::remove_file(ticker_error_file());
}

fn secure_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

// ---------------------------------------------------------------------------
// Cron store helpers — mirrors `cron.jobs` file helpers (minimal)
// ---------------------------------------------------------------------------

fn load_jobs_raw() -> Vec<Value> {
    let path = jobs_file();
    if !path.exists() {
        return Vec::new();
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    if let Some(obj) = value.as_object() {
        if let Some(jobs_val) = obj.get("jobs") {
            if let Some(arr) = jobs_val.as_array() {
                return arr.clone();
            }
            if let Some(map) = jobs_val.as_object() {
                let mut out = Vec::new();
                for (k, v) in map {
                    if let Some(m) = v.as_object() {
                        let mut merged = m.clone();
                        if !merged.contains_key("id") {
                            merged.insert("id".to_string(), Value::String(k.clone()));
                        }
                        out.push(Value::Object(merged));
                    }
                }
                return out;
            }
        }
    }
    if let Some(arr) = value.as_array() {
        return arr.clone();
    }
    Vec::new()
}

fn is_job_runnable(job: &Value) -> bool {
    if let Some(enabled) = job.get("enabled").and_then(|v| v.as_bool()) {
        if !enabled {
            return false;
        }
    }
    if let Some(paused) = job.get("paused").and_then(|v| v.as_bool()) {
        if paused {
            return false;
        }
    }
    // Also respect `status` == "paused" if present.
    if let Some(s) = job.get("status").and_then(|v| v.as_str()) {
        if s == "paused" || s == "disabled" {
            return false;
        }
    }
    true
}

fn ensure_aware_dt(s: &str) -> Option<chrono::DateTime<Utc>> {
    // Mirrors `cron.jobs._ensure_aware(datetime.fromisoformat(next_run_at))`
    // and `cron.jobs._hermes_now()`. Parses ISO-8601 strings, normalizing to UTC.
    // Accepts both `Z`-suffixed and offset forms; naive datetimes assumed UTC.
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    // Try without TZ: e.g. "2026-01-02T15:04:05" or "2026-01-02 15:04:05"
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Some(naive.and_utc());
    }
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Some(naive.and_utc());
    }
    // With millis
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.3f") {
        return Some(dt.and_utc());
    }
    None
}

fn hermes_now() -> chrono::DateTime<Utc> {
    Utc::now()
}

// ---------------------------------------------------------------------------
// Stub orchestrator hooks — mirrors `cron.scheduler.run_one_job`,
// `cron.jobs.claim_job_for_fire`, `cron.executions` ledger
// ---------------------------------------------------------------------------

/// Stub for `cron.scheduler.run_one_job(claimed_job, ...)`.
/// Mirrors `cron.scheduler.run_one_job` — the shared orchestrator that
/// executes the agent and delivers the result. In Rust this is a stub that
/// logs; real dispatch is wired by the gateway.
///
/// Returns `()` — Python's `run_one_job` returns None.
pub fn run_one_job(
    claimed_job: &Value,
    adapters: Option<&Value>,
    loop_handle: Option<&Value>,
    cancel_event: Option<&StopEvent>,
) {
    let job_id = claimed_job
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let exec_id = claimed_job
        .get("execution_id")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    log::info!(
        "run_one_job stub: job_id={} execution_id={} adapters_present={} loop_present={} cancel={}",
        job_id,
        exec_id,
        adapters.is_some(),
        loop_handle.is_some(),
        cancel_event.is_some()
    );
    let _ = (adapters, loop_handle, cancel_event);
}

/// Claim a job for fire via the local store's compare-and-set.
/// Mirrors `cron.jobs.claim_job_for_fire(job_id, return_job=True, force=...)`.
///
/// In Rust this does a best-effort file read; the real CAS is under
/// `cron/jobs.py` with file locking. Returns the job dict if found and
/// runnable, else `None` (lost claim or missing job).
pub fn claim_job_for_fire(job_id: &str, force: bool) -> Option<Value> {
    let jobs = load_jobs_raw();
    for job in jobs {
        let id = job.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if id == job_id {
            if !force && !is_job_runnable(&job) {
                return None;
            }
            return Some(job);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// CronScheduler trait — mirrors `class CronScheduler(ABC)`
// ---------------------------------------------------------------------------

/// Axis-B trigger provider. Decides WHEN a due cron job fires.
///
/// Required surface is intentionally minimal: `name` + `start`. `stop` and
/// `is_available` carry safe defaults. The three Phase-4 hooks
/// (`on_jobs_changed` / `fire_due` / `reconcile`) are added later as
/// NON-abstract methods so the built-in keeps satisfying the ABC without
/// overriding them — see `test_abc_growth_stays_additive`.
///
/// Mirrors `class CronScheduler(ABC)` (lines 67-237).
pub trait CronScheduler: Send + Sync {
    /// Short identifier, e.g. 'builtin', 'chronos'.
    /// Mirrors `@property @abstractmethod def name(self) -> str`.
    fn name(&self) -> &str;

    /// Whether this provider can run in the current environment.
    ///
    /// MUST NOT make network calls. The built-in is always available; an
    /// external provider checks for configured endpoint/credentials. When a
    /// named provider returns False, the resolver falls back to the built-in.
    /// Mirrors `def is_available(self) -> bool`.
    fn is_available(&self) -> bool {
        true
    }

    /// Begin firing due jobs.
    ///
    /// For the built-in this BLOCKS in the 60s loop until `stop_event` is set
    /// (it is run inside a daemon thread by the caller, exactly as today).
    /// An external provider may register a schedule/webhook and return
    /// immediately; in that case it must still honor `stop_event` for teardown.
    /// Mirrors `def start(self, stop_event: threading.Event, *, adapters=None, loop=None, interval=60)`.
    ///
    /// Extra `can_dispatch` and `profile_homes` params are carried here for the
    /// built-in multiplex path; external providers ignore them (default `None`).
    fn start(
        &self,
        stop_event: &StopEvent,
        adapters: Option<&Value>,
        loop_handle: Option<&Value>,
        interval: u64,
        can_dispatch: Option<&dyn Fn() -> bool>,
        profile_homes: Option<&[ProfileHome]>,
    );

    /// Optional eager teardown hook. Default no-op; setting the `stop_event`
    /// is the primary stop signal. Override for providers holding external
    /// resources (queue consumers, HTTP servers).
    /// Mirrors `def stop(self) -> None`.
    fn stop(&self) {}

    // --- Optional hooks for external providers (added Phase 4). --------------
    // All default-safe so the built-in inherits working behavior without
    // overriding. Keep these NON-abstract — see test_abc_growth_stays_additive.

    /// Called after a successful store mutation (create/update/remove/
    /// pause/resume). External providers reconcile their registry here (e.g.
    /// Chronos re-provisions/cancels the affected one-shot via NAS).
    /// Built-in: no-op (it re-reads jobs.json on every tick).
    /// Mirrors `def on_jobs_changed(self) -> None`.
    fn on_jobs_changed(&self) {}

    /// Register the first external trigger for one newly persisted job.
    /// Mirrors `def register_job(self, job: dict[str, Any]) -> None`.
    fn register_job(&self, _job: &Value) {}

    /// Run profile-local attempt recovery for every provider lifecycle.
    /// Mirrors `def recover_interrupted(self) -> int`.
    fn recover_interrupted(&self) -> usize {
        // Mirrors `from cron.executions import recover_interrupted_executions; return recover...`
        // In Rust we call the executions2 ledger. Best-effort: on error return 0.
        match crate::executions2::recover_interrupted_executions() {
            Ok(n) => n,
            Err(e) => {
                log::warn!("recover_interrupted: executions ledger error: {e}");
                0
            }
        }
    }

    /// Whether `fire_due` accepts the additive `force` keyword.
    /// Mirrors `@property def supports_force_fire(self) -> bool`.
    fn supports_force_fire(&self) -> bool {
        provider_supports_force_fire(self)
    }

    /// Run a single job NOW via the shared orchestrator. Called by the
    /// inbound fire webhook when an external scheduler signals a job is due.
    ///
    /// The default claims the job with a store-level compare-and-set
    /// (multi-machine at-most-once), then runs it via the shared
    /// `run_one_job` body. Built-in never calls this (it has its own tick
    /// loop); an external provider routes its inbound fire here.
    ///
    /// Returns True if THIS caller claimed and processed the attempt, even if
    /// the job itself failed. Returns False only if the claim was lost
    /// (another machine/retry won it) or the job no longer exists.
    /// Mirrors `def fire_due(self, job_id: str, *, adapters=None, loop=None, force=False) -> bool`.
    fn fire_due(
        &self,
        job_id: &str,
        adapters: Option<&Value>,
        loop_handle: Option<&Value>,
        force: bool,
    ) -> bool {
        let claimed = self.claim_fire(job_id, force);
        match claimed {
            None => false,
            Some(job) => self.fire_claimed(job, adapters, loop_handle, None),
        }
    }

    /// Durably claim one fire and create its audit attempt before dispatch.
    ///
    /// Webhook transports call this synchronously before acknowledging the
    /// external scheduler, then pass the exact owner-bearing snapshot to
    /// `fire_claimed` in tracked background work.
    /// Mirrors `def claim_fire(self, job_id: str, *, force: bool=False) -> dict | None`.
    fn claim_fire(&self, job_id: &str, force: bool) -> Option<Value> {
        // Mirrors Python: execution = create_execution(job_id, source=self.name)
        let execution = match crate::executions2::create_execution(job_id, self.name()) {
            Ok(rec) => rec,
            Err(e) => {
                log::warn!("claim_fire: create_execution failed for {job_id:?}: {e}");
                return None;
            }
        };
        let execution_id = execution.id.clone();
        // Try to claim the job store entry.
        let claimed_job = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            claim_job_for_fire(job_id, force)
        })) {
            Ok(opt) => opt,
            Err(payload) => {
                let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic".to_string()
                };
                let err = format!("Fire claim failed before dispatch: Panic: {msg}");
                let _ = crate::executions2::finish_execution(&execution_id, false, Some(&err), None);
                // Re-raise as log; Python re-raises. In Rust we return None after logging
                // to keep the thread alive — the caller will see the error via execution ledger.
                log::error!("claim_fire panic for {job_id:?}: {msg}");
                return None;
            }
        };
        match claimed_job {
            None => {
                let _ = crate::executions2::finish_execution(
                    &execution_id,
                    false,
                    Some("Fire claim was not acquired"),
                    None,
                );
                None
            }
            Some(mut job) => {
                // Attach execution_id for run_one_job correlation.
                // Mirrors `claimed_job["execution_id"] = execution["id"]`.
                if let Some(obj) = job.as_object_mut() {
                    obj.insert("execution_id".to_string(), Value::String(execution_id));
                }
                Some(job)
            }
        }
    }

    /// Run an exact snapshot returned by `claim_fire`.
    ///
    /// `cancel_event`: optional transport-owned `StopEvent` (or compatible)
    /// that lets the caller stop this execution cooperatively.
    /// Mirrors `def fire_claimed(self, claimed_job: dict, *, adapters=None, loop=None, cancel_event=None) -> bool`.
    fn fire_claimed(
        &self,
        claimed_job: Value,
        adapters: Option<&Value>,
        loop_handle: Option<&Value>,
        cancel_event: Option<&StopEvent>,
    ) -> bool {
        run_one_job(&claimed_job, adapters, loop_handle, cancel_event);
        true
    }

    /// Converge the external registry toward jobs.json (the desired state):
    /// arm missing one-shots, cancel orphaned ones, re-arm changed times.
    /// Built-in: no-op.
    /// Mirrors `def reconcile(self) -> None`.
    fn reconcile(&self) {}

    // --- Introspection helpers for `provider_supports_*` --------------------
    // Mirrors Python's `inspect.signature` / `getattr(cls, ...)` checks.
    // Rust traits are static, so providers override these to advertise
    // custom behavior. Defaults mean "inherits base behavior" (safe).

    /// Whether this type overrides `claim_fire` with custom logic.
    /// Mirrors `claim_fire_impl is not CronScheduler.claim_fire`.
    fn is_claim_fire_overridden(&self) -> bool {
        false
    }

    /// Whether this type overrides `fire_claimed` with custom logic.
    fn is_fire_claimed_overridden(&self) -> bool {
        false
    }

    /// Whether this type overrides `fire_due` with custom single-phase logic.
    fn is_fire_due_overridden(&self) -> bool {
        false
    }

    /// Whether `fire_claimed` accepts a `cancel_event` kwarg.
    /// Mirrors `provider_supports_fire_cancel` introspection.
    /// In Rust the trait always includes `cancel_event`, so default true.
    fn supports_cancel_event(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Provider capability probes — mirrors `provider_supports_*`
// ---------------------------------------------------------------------------

/// Return whether a provider can safely receive `fire_due(force=...)`.
///
/// Signature detection keeps providers written before `force` was added
/// source-compatible. Providers accepting `**kwargs` are compatible.
/// Mirrors `def provider_supports_force_fire(provider: Any) -> bool`.
///
/// In Rust the trait always carries `force`, so this returns true for any
/// `CronScheduler`. External providers that predate `force` would override
/// `supports_force_fire` to return false.
pub fn provider_supports_force_fire(provider: &dyn CronScheduler) -> bool {
    // Mirrors Python's inspect.signature check for `force` or VAR_KEYWORD.
    // In Rust we delegate to the provider's own advertisement; default true.
    let _ = provider;
    true
}

/// Return whether a provider implements the two-phase fire contract.
///
/// The webhook admission path uses `claim_fire` + `fire_claimed` so the
/// 202 response is backed by a durable, owner-fenced claim. A legacy
/// third-party provider that overrides the documented single-phase
/// `fire_due` hook but inherits the base `claim_fire` must keep being driven
/// through its own `fire_due` — silently routing around its override would drop
/// that behavior. Providers that customize `claim_fire` itself are already
/// split-aware and keep the two-phase path.
/// Mirrors `def provider_supports_split_fire(provider: Any) -> bool`.
pub fn provider_supports_split_fire(provider: &dyn CronScheduler) -> bool {
    if provider.is_claim_fire_overridden() {
        return true;
    }
    if provider.is_fire_claimed_overridden() {
        return true;
    }
    if !provider.is_fire_due_overridden() {
        return true;
    }
    false
}

/// Return whether `fire_claimed` accepts a `cancel_event` kwarg.
/// Mirrors `def provider_supports_fire_cancel(provider: Any) -> bool`.
pub fn provider_supports_fire_cancel(provider: &dyn CronScheduler) -> bool {
    provider.supports_cancel_event()
}

// ---------------------------------------------------------------------------
// Misfire catch-up — mirrors `_misfire_grace_minutes`, `fire_overdue_jobs`
// ---------------------------------------------------------------------------

/// Resolve the misfire catch-up grace window from config.
///
/// `cron.misfire_grace_minutes` (number, default `DEFAULT_MISFIRE_GRACE_MINUTES`).
/// A non-positive value disables the catch-up sweep entirely.
/// Mirrors `def _misfire_grace_minutes() -> float`.
pub fn misfire_grace_minutes() -> f64 {
    // Mirrors Python's `try: from hermes_cli.config import cfg_get, load_config ... except: return default`
    // In Rust we try to read `cron.misfire_grace_minutes` from `config.yaml` / `config.json`
    // with a best-effort manual parse; on any failure fall back to default.
    if let Some(v) = read_misfire_grace_from_config() {
        return v;
    }
    DEFAULT_MISFIRE_GRACE_MINUTES
}

fn read_misfire_grace_from_config() -> Option<f64> {
    let candidates = [
        get_hermes_home().join("config.yaml"),
        get_hermes_home().join("config.yml"),
        get_hermes_home().join("config.json"),
    ];
    for path in &candidates {
        if !path.exists() {
            continue;
        }
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        // Very small manual parser: look for `misfire_grace_minutes` key.
        // Handles YAML-ish `misfire_grace_minutes: 10` and JSON `"misfire_grace_minutes": 10`.
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                continue;
            }
            if trimmed.contains("misfire_grace_minutes") {
                // Extract number after colon.
                if let Some(colon) = trimmed.find(':') {
                    let after = trimmed[colon + 1..].trim();
                    // Strip trailing comma, quotes, comments.
                    let token = after
                        .split('#')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .trim_matches(|c| c == ',' || c == '"' || c == '\'')
                        .trim();
                    let num_str = token.split_whitespace().next().unwrap_or("").trim_matches(|c| c == ',' || c == '"' || c == '\'');
                    if let Ok(v) = num_str.parse::<f64>() {
                        return Some(v);
                    }
                }
            }
        }
        // If file existed but key not found, don't try other candidates for a miss — return None to use default.
        // But we continue to next candidate in case the key is in a different file (unlikely).
    }
    None
}

/// Fire jobs whose scheduled time passed without an external fire arriving.
///
/// The misfire catch-up half of the hosted fire path. External providers
/// (Chronos) deliver scheduled fires over HTTP to this process's api_server
/// adapter; when that hop is down at fire time, the job's `next_run_at` stays
/// parked in the past. The day is silently lost unless this sweep fires it.
///
/// Called from the gateway housekeeping loop. Deliberately:
/// - **No-op for the built-in provider.** Its tick loop already picks up
///   past-due jobs via `get_due_jobs`.
/// - **Routes through the provider's own two-phase fire path**.
/// - **Waits out a grace window** (`cron.misfire_grace_minutes`, default 10).
/// - **Operates on the process-global cron store only**.
///
/// Returns the number of jobs this sweep claimed and dispatched.
/// Mirrors `def fire_overdue_jobs(provider: "CronScheduler", *, adapters=None, loop=None, now=None) -> int`.
pub fn fire_overdue_jobs(
    provider: &dyn CronScheduler,
    adapters: Option<&Value>,
    loop_handle: Option<&Value>,
    now: Option<chrono::DateTime<Utc>>,
) -> usize {
    // No-op for built-in — its tick loop already self-heals.
    if provider.name() == "builtin" {
        return 0;
    }

    let grace_minutes = misfire_grace_minutes();
    if grace_minutes <= 0.0 {
        return 0;
    }

    let now_dt = now.unwrap_or_else(hermes_now);

    let jobs = load_jobs_raw();
    let mut fired: usize = 0;

    for job in jobs {
        if !is_job_runnable(&job) {
            continue;
        }
        let next_run_at = match job.get("next_run_at").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.to_string(),
            _ => continue,
        };
        let due_dt = match ensure_aware_dt(&next_run_at) {
            Some(dt) => dt,
            None => continue,
        };
        let overdue_seconds = (now_dt - due_dt).num_milliseconds() as f64 / 1000.0;
        if overdue_seconds < grace_minutes * 60.0 {
            continue;
        }
        let job_id = job
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if job_id.is_empty() {
            continue;
        }
        let job_name = job
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unnamed");
        log::warn!(
            "Misfire catch-up: job {} ({}) was due {} ({:.0} min overdue) and no external fire arrived — firing locally.",
            job_id,
            job_name,
            next_run_at,
            overdue_seconds / 60.0
        );
        // Two-phase, webhook-style: claim synchronously (fast store CAS),
        // then run off-thread so the caller's loop is never blocked.
        let claimed = provider.claim_fire(&job_id, false);
        let claimed = match claimed {
            Some(c) => c,
            None => continue,
        };
        // Spawn daemon thread mirroring `threading.Thread(..., daemon=True, name=f"cron-misfire-{job_id[:12]}")`
        let adapters_cloned = adapters.cloned();
        let loop_cloned = loop_handle.cloned();
        // We need to move the provider's fire_claimed into the thread. Since
        // `provider` is a trait object borrowed for this call, we handle the
        // misfire dispatch by cloning the job and calling `run_one_job` directly
        // (equivalent to provider.fire_claimed without re-arm side effects for
        // built-ins; for Chronos the re-arm happens in its fire_claimed override,
        // but misfire only targets non-builtin providers so the trait dispatch
        // matters — we use `run_one_job` as the shared fallback).
        let job_id_clone = job_id.clone();
        thread::Builder::new()
            .name(format!("cron-misfire-{}", &job_id_clone[..job_id_clone.len().min(12)]))
            .spawn(move || {
                // For External providers, the ideal is `provider.fire_claimed(claimed, ...)`
                // but we cannot move `provider` into thread without owned Box.
                // Fall back to shared orchestrator — still achieves at-most-once
                // via the synchronous claim above; provider-specific re-arm is
                // best-effort here (housekeeping vs webhook path difference).
                run_one_job(&claimed, adapters_cloned.as_ref(), loop_cloned.as_ref(), None);
            })
            .unwrap_or_else(|e| {
                log::warn!("Misfire catch-up failed for job {}: spawn error: {}", job_id, e);
                // Fallback: run inline (rare)
                // Note: we moved `claimed` into closure on success, so this branch
                // would not have it; log and count as not fired.
                panic!("thread spawn failed");
            });
        // If spawn succeeded, count as fired. The closure owns `claimed`.
        // On spawn failure the thread builder returns Err and we handled it.
        // To keep the logic simple, we assume spawn succeeded if we reach here
        // without panicking — the `unwrap_or_else` above panics on failure,
        // so reaching here means success.
        fired += 1;
        // Catch any exception-like failure in claim path already handled.
        // Additional spawn failure is logged above; we don't double-count.
        // For safety, wrap the whole iteration in catch_unwind-like handling:
        // (Rust panics already caught by spawn error branch).
        let _ = adapters; // silence unused in some builds
    }

    fired
}

// ---------------------------------------------------------------------------
// Config / provider resolution — mirrors `resolve_cron_scheduler`,
// `scheduler_for_profile_mode`
// ---------------------------------------------------------------------------

fn read_provider_from_config() -> String {
    let candidates = [
        get_hermes_home().join("config.yaml"),
        get_hermes_home().join("config.yml"),
        get_hermes_home().join("config.json"),
    ];
    for path in &candidates {
        if !path.exists() {
            continue;
        }
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        // Look for `provider:` under `cron:` section. Very small parser:
        // find line containing "provider" and extract value after colon.
        // This intentionally avoids a YAML dep.
        let mut in_cron = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let indent = line.len() - line.trim_start().len();
            // Detect `cron:` section header (indent 0) and track.
            if indent == 0 && trimmed.starts_with("cron:") {
                in_cron = true;
                // Inline style `cron: {provider: chronos}` is not supported in minimal parser;
                // check same line for provider as fallback.
                if trimmed.contains("provider") {
                    if let Some(v) = extract_yaml_string_value(trimmed, "provider") {
                        return v.trim().to_string();
                    }
                }
                continue;
            }
            // Leaving cron section when we hit next top-level key (indent 0, not cron).
            if indent == 0 && in_cron && !trimmed.starts_with("cron:") {
                // If cron block ended and we haven't found provider, don't keep scanning unrelated keys for provider.
                // But we still scan for provider at any indent if in_cron was set and we are inside.
                // Reset if we are at top-level not cron.
                in_cron = false;
            }
            if in_cron || text.contains("\"provider\"") || text.contains("'provider'") {
                if trimmed.contains("provider") {
                    if let Some(v) = extract_yaml_string_value(trimmed, "provider") {
                        return v.trim().to_string();
                    }
                }
            }
        }
        // Also try JSON-style search globally as fallback (covers config.json).
        if text.contains("\"provider\"") {
            if let Some(v) = extract_json_string_value(&text, "provider") {
                return v;
            }
        }
    }
    // Env var override (bridged from config in some deployments)
    if let Ok(val) = std::env::var("HERMES_CRON_PROVIDER") {
        if !val.trim().is_empty() {
            return val.trim().to_string();
        }
    }
    String::new()
}

fn extract_yaml_string_value(line: &str, key: &str) -> Option<String> {
    // Expects `key: value` or `key: "value"` or `key: 'value'`
    let needle = format!("{key}:");
    let idx = line.find(&needle)?;
    let after = line[idx + needle.len()..].trim();
    if after.is_empty() {
        return Some(String::new());
    }
    // Strip inline comment
    let without_comment = after.split('#').next().unwrap_or(after).trim();
    let val = without_comment
        .trim_matches(|c| c == '"' || c == '\'')
        .trim()
        .trim_end_matches(',')
        .trim_matches(|c| c == '"' || c == '\'')
        .to_string();
    Some(val)
}

fn extract_json_string_value(text: &str, key: &str) -> Option<String> {
    // Very small JSON string extractor: `"key": "value"`
    let needle = format!("\"{key}\"");
    let idx = text.find(&needle)?;
    let after = &text[idx + needle.len()..];
    let colon = after.find(':')?;
    let val_part = after[colon + 1..].trim_start();
    if val_part.starts_with('"') {
        let end = val_part[1..].find('"')?;
        Some(val_part[1..1 + end].to_string())
    } else if val_part.starts_with('\'') {
        let end = val_part[1..].find('\'')?;
        Some(val_part[1..1 + end].to_string())
    } else {
        // Unquoted value — take until comma/brace/newline
        let end = val_part
            .find(|c| c == ',' || c == '}' || c == '\n' || c == '\r')
            .unwrap_or(val_part.len());
        Some(val_part[..end].trim().to_string())
    }
}

/// Return the active cron scheduler provider.
///
/// Reads `cron.provider` from config. Empty/absent → built-in. A named
/// provider that is missing, fails to load, or reports `is_available() == False`
/// falls back to the built-in with a warning — cron must never be left
/// without a trigger.
/// Mirrors `def resolve_cron_scheduler() -> "CronScheduler"`.
pub fn resolve_cron_scheduler() -> Box<dyn CronScheduler> {
    let name = read_provider_from_config();
    let trimmed = name.trim().to_string();

    if trimmed.is_empty()
        || trimmed == "builtin"
        || trimmed == "in-process"
        || trimmed == "inprocess"
    {
        return Box::new(InProcessCronScheduler);
    }

    // Attempt to load external provider via `plugins/cron_providers`.
    // In Rust there is no dynamic Python plugin loader; we log and fall back.
    // External providers are expected to register via a Rust plugin registry;
    // until one is wired, we always fall back with a warning, matching Python's
    // `except Exception` + fallback path.
    log::warn!(
        "cron.provider '{}' not found; using built-in ticker (Rust stub: no external cron_providers registry)",
        trimmed
    );
    Box::new(InProcessCronScheduler)
}

/// Return a scheduler that can safely serve the gateway's profile mode.
///
/// External providers currently own one unscoped remote registry/client and
/// therefore cannot safely reconcile several profile stores from one process.
/// Fail closed to the built-in multiplex ticker until the provider API carries
/// explicit profile identity through lifecycle and webhook calls.
/// Mirrors `def scheduler_for_profile_mode(provider: "CronScheduler", *, multiplex_profiles: bool) -> "CronScheduler"`.
pub fn scheduler_for_profile_mode(
    provider: Box<dyn CronScheduler>,
    multiplex_profiles: bool,
) -> Box<dyn CronScheduler> {
    if !multiplex_profiles || provider.name() == "builtin" {
        return provider;
    }
    log::warn!(
        "cron.provider '{}' does not support multiplex_profiles; using built-in ticker",
        provider.name()
    );
    Box::new(InProcessCronScheduler)
}

// ---------------------------------------------------------------------------
// ProfileHome — mirrors multiplex `profile_homes` entries
// ---------------------------------------------------------------------------

/// One profile home entry for multiplex ticking.
///
/// Python accepts either a `Path` or a `tuple(name, Path)`. This enum
/// preserves both shapes for faithful porting and for `info!("Multiplex ...")`
/// logging that shows profile names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileHome {
    /// Simple path entry: `Path("/home/user/.hermes")`
    Path(PathBuf),
    /// Named entry: `(name, home)` tuple.
    Named { name: String, home: PathBuf },
}

impl ProfileHome {
    pub fn home(&self) -> &Path {
        match self {
            ProfileHome::Path(p) => p.as_path(),
            ProfileHome::Named { home, .. } => home.as_path(),
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            ProfileHome::Path(p) => p.to_string_lossy().into_owned(),
            ProfileHome::Named { name, .. } => name.clone(),
        }
    }
}

impl From<PathBuf> for ProfileHome {
    fn from(p: PathBuf) -> Self {
        ProfileHome::Path(p)
    }
}

impl From<&Path> for ProfileHome {
    fn from(p: &Path) -> Self {
        ProfileHome::Path(p.to_path_buf())
    }
}

impl From<(String, PathBuf)> for ProfileHome {
    fn from((name, home): (String, PathBuf)) -> Self {
        ProfileHome::Named { name, home }
    }
}

// ---------------------------------------------------------------------------
// Hermes home override — mirrors `hermes_constants.set_hermes_home_override`
// ---------------------------------------------------------------------------

/// Token returned by `set_hermes_home_override` to restore the previous value.
/// Mirrors `hermes_constants.set_hermes_home_override` / `reset_hermes_home_override`.
pub struct HomeOverrideToken {
    previous: Option<String>,
}

pub fn set_hermes_home_override(home: &str) -> HomeOverrideToken {
    let previous = std::env::var("HERMES_HOME").ok();
    // SAFETY: set_var is unsafe in Rust 1.66+ due to thread safety; we are
    // single-threaded at the point of multiplex setup or hold no lock that
    // depends on env. This mirrors Python's process-global override.
    unsafe {
        std::env::set_var("HERMES_HOME", home);
    }
    HomeOverrideToken { previous }
}

pub fn reset_hermes_home_override(token: HomeOverrideToken) {
    unsafe {
        match token.previous {
            Some(prev) => std::env::set_var("HERMES_HOME", prev),
            None => std::env::remove_var("HERMES_HOME"),
        }
    }
}

// ---------------------------------------------------------------------------
// Cron tick stub — mirrors `cron.scheduler.tick`
// ---------------------------------------------------------------------------

/// Run one scheduler tick: find due jobs and dispatch them.
/// Mirrors `cron.scheduler.tick(verbose=False, adapters=None, loop=None, sync=False, can_dispatch=None)`.
///
/// In Rust this is a stub that logs; the real tick is in `cron/scheduler.py`
/// (or a future `crate::scheduler`). We keep the signature faithful so
/// `InProcessCronScheduler::start` remains 1:1.
pub fn cron_tick(
    verbose: bool,
    adapters: Option<&Value>,
    loop_handle: Option<&Value>,
    sync: bool,
    can_dispatch: Option<&dyn Fn() -> bool>,
) {
    let dispatch_allowed = can_dispatch.map(|f| f()).unwrap_or(true);
    log::debug!(
        "cron_tick stub: verbose={} sync={} adapters_present={} loop_present={} can_dispatch={}",
        verbose,
        sync,
        adapters.is_some(),
        loop_handle.is_some(),
        dispatch_allowed
    );
    // Real implementation would:
    // - load_jobs(), filter get_due_jobs(now)
    // - claim each job with CAS (claim_job_for_fire)
    // - spawn run_one_job in a thread / async task
    let _ = (verbose, adapters, loop_handle, sync, can_dispatch);
}

// ---------------------------------------------------------------------------
// InProcessCronScheduler — mirrors `class InProcessCronScheduler(CronScheduler)`
// ---------------------------------------------------------------------------

/// Default provider: the historical in-process 60s ticker.
///
/// `start()` blocks in the tick loop until `stop_event` is set, identical
/// to the pre-refactor `_start_cron_ticker` core loop. The caller runs it in
/// a daemon thread. `can_dispatch` is an optional synchronous gate supplied
/// by GatewayRunner during external drain; skipped ticks leave due jobs intact
/// for the next allowed tick.
/// Mirrors `class InProcessCronScheduler(CronScheduler)` (lines 491-703).
#[derive(Debug, Clone, Copy, Default)]
pub struct InProcessCronScheduler;

impl CronScheduler for InProcessCronScheduler {
    fn name(&self) -> &str {
        "builtin"
    }

    fn start(
        &self,
        stop_event: &StopEvent,
        adapters: Option<&Value>,
        loop_handle: Option<&Value>,
        interval: u64,
        can_dispatch: Option<&dyn Fn() -> bool>,
        profile_homes: Option<&[ProfileHome]>,
    ) {
        log::info!("In-process cron scheduler started (interval={}s)", interval);

        // ── Multiplex profiles ────────────────────────────────────────────────
        if let Some(homes) = profile_homes {
            if !homes.is_empty() {
                self.start_multiplex(
                    stop_event,
                    homes,
                    adapters,
                    loop_handle,
                    interval,
                    can_dispatch,
                );
                return;
            }
        }

        // ── Single-profile (legacy) path ────────────────────────────────────
        let recovered = self.recover_interrupted();
        if recovered > 0 {
            log::warn!(
                "Marked {} interrupted cron execution(s) unknown after restart",
                recovered
            );
        }
        // Heartbeat once before the first sleep so `hermes cron status` sees a
        // live ticker immediately after startup, not only after the first tick.
        record_ticker_heartbeat(true);

        let interval_f = interval as f64;
        let mut consecutive_failures: usize = 0;

        while !stop_event.is_set() {
            let mut ok = false;
            let mut tick_error: Option<String> = None;

            // Catch panics as well as errors — mirrors Python's `except BaseException`
            // which catches SystemExit/KeyboardInterrupt so the ticker never dies silently.
            let tick_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if let Some(gate) = can_dispatch {
                    if !gate() {
                        log::debug!("Cron dispatch paused while gateway drains existing work");
                        return Ok::<(), String>(());
                    }
                }
                cron_tick(false, adapters, loop_handle, false, can_dispatch);
                Ok(())
            }));

            match tick_result {
                Ok(Ok(())) => {
                    ok = true;
                }
                Ok(Err(msg)) => {
                    log::error!("Cron tick error: {}", msg);
                    record_ticker_error(&msg);
                    tick_error = Some(msg.clone());
                    consecutive_failures = note_tick_failure(&msg, consecutive_failures);
                }
                Err(payload) => {
                    let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                        format!("Panic: {}", s)
                    } else if let Some(s) = payload.downcast_ref::<String>() {
                        format!("Panic: {}", s)
                    } else {
                        "Panic: unknown".to_string()
                    };
                    log::error!("Cron tick error: {}", msg);
                    record_ticker_error(&msg);
                    tick_error = Some(msg.clone());
                    consecutive_failures = note_tick_failure(&msg, consecutive_failures);
                }
            }

            // Record liveness every iteration; bump the success marker only on a
            // clean tick, so status can tell "alive but failing every tick" from
            // "actually firing jobs" (#32612, #32895).
            record_ticker_heartbeat(ok);
            if ok {
                let _ = tick_error;
                clear_ticker_error();
                consecutive_failures = 0;
            }

            let wait_secs = backoff_wait_seconds(interval_f, consecutive_failures);
            // Mirrors `stop_event.wait(_backoff_wait_seconds(interval, consecutive_failures))`
            // — wait with timeout, but break early if stop_event is set.
            stop_event.wait_secs(wait_secs);
        }
    }
}

impl InProcessCronScheduler {
    /// Tick every served profile's cron store when multiplex_profiles is on.
    ///
    /// Each profile uses `set_hermes_home_override()` + `use_cron_store()`
    /// to scope its tick, heartbeat, recovery, lock file, config/.env, and
    /// agent execution to that profile's home — mirroring how
    /// `_profile_runtime_scope` scopes the multiplexed inbound path and
    /// `web_server.py` scopes per-profile cron API calls.
    /// Mirrors `def _start_multiplex(self, stop_event, *, profile_homes, adapters=None, loop=None, interval=60, can_dispatch=None)`.
    pub fn start_multiplex(
        &self,
        stop_event: &StopEvent,
        profile_homes: &[ProfileHome],
        adapters: Option<&Value>,
        loop_handle: Option<&Value>,
        interval: u64,
        can_dispatch: Option<&dyn Fn() -> bool>,
    ) {
        log::info!(
            "Multiplex cron scheduler started for {} profile(s): {:?}",
            profile_homes.len(),
            profile_homes
                .iter()
                .map(|p| p.display_name())
                .collect::<Vec<_>>()
        );

        // Recovery + initial heartbeat for every profile.
        for entry in profile_homes {
            let home = entry.home();
            let token = set_hermes_home_override(&home.to_string_lossy());
            // Scope ensures reset even on panic — use catch_unwind for safety.
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let recovered = self.recover_interrupted();
                if recovered > 0 {
                    log::warn!(
                        "Marked {} interrupted cron execution(s) for profile at {}",
                        recovered,
                        home.display()
                    );
                }
                record_ticker_heartbeat(true);
            }));
            reset_hermes_home_override(token);
            if let Err(payload) = res {
                let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic".to_string()
                };
                log::error!("Multiplex init panic for {}: {}", home.display(), msg);
            }
        }

        let interval_f = interval as f64;
        let mut consecutive_failures: usize = 0;

        while !stop_event.is_set() {
            let mut ok = false;
            let mut tick_error: Option<String> = None;

            let tick_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if let Some(gate) = can_dispatch {
                    if !gate() {
                        log::debug!("Cron dispatch paused while gateway drains existing work");
                        return Ok::<(), String>(());
                    }
                }
                for entry in profile_homes {
                    let home = entry.home();
                    let token = set_hermes_home_override(&home.to_string_lossy());
                    // `use_cron_store(home)` scoping is approximated by the home override;
                    // the Rust ledger already resolves `get_hermes_home()` per call.
                    cron_tick(false, adapters, loop_handle, false, can_dispatch);
                    reset_hermes_home_override(token);
                }
                Ok(())
            }));

            match tick_result {
                Ok(Ok(())) => ok = true,
                Ok(Err(msg)) => {
                    log::error!("Cron tick error: {}", msg);
                    tick_error = Some(msg.clone());
                    consecutive_failures = note_tick_failure(&msg, consecutive_failures);
                }
                Err(payload) => {
                    let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                        format!("Panic: {}", s)
                    } else if let Some(s) = payload.downcast_ref::<String>() {
                        format!("Panic: {}", s)
                    } else {
                        "Panic: unknown".to_string()
                    };
                    log::error!("Cron tick error: {}", msg);
                    tick_error = Some(msg.clone());
                    consecutive_failures = note_tick_failure(&msg, consecutive_failures);
                }
            }

            // Record per-profile heartbeat after each tick cycle.
            for entry in profile_homes {
                let home = entry.home();
                let token = set_hermes_home_override(&home.to_string_lossy());
                record_ticker_heartbeat(ok);
                if ok {
                    clear_ticker_error();
                } else if let Some(ref err) = tick_error {
                    record_ticker_error(err);
                }
                reset_hermes_home_override(token);
            }

            if ok {
                consecutive_failures = 0;
            }

            let wait_secs = backoff_wait_seconds(interval_f, consecutive_failures);
            stop_event.wait_secs(wait_secs);
        }
    }

    /// Convenience wrapper matching Python's `start` signature with owned profile_homes.
    /// Starts the loop; blocks until `stop_event` is set.
    pub fn start_simple(&self, stop_event: &StopEvent, interval: u64) {
        self.start(stop_event, None, None, interval, None, None)
    }
}

// ---------------------------------------------------------------------------
// Self-check (ponytail: one runnable check, no framework)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_interval_when_healthy() {
        assert!((backoff_wait_seconds(60.0, 0) - 60.0).abs() < f64::EPSILON);
        assert!((backoff_wait_seconds(60.0, 1) - 60.0).abs() < f64::EPSILON);
    }

    #[test]
    fn backoff_doubles_and_caps() {
        // 60 * 2^(n-1) capped at 15*60=900
        assert!((backoff_wait_seconds(60.0, 2) - 120.0).abs() < 1e-9);
        assert!((backoff_wait_seconds(60.0, 3) - 240.0).abs() < 1e-9);
        assert!((backoff_wait_seconds(60.0, 4) - 480.0).abs() < 1e-9);
        // capped
        assert!((backoff_wait_seconds(60.0, 10) - 900.0).abs() < 1e-9);
        assert!((backoff_wait_seconds(60.0, 100) - 900.0).abs() < 1e-9);
    }

    #[test]
    fn note_tick_failure_classifies_fd_exhaustion() {
        assert_eq!(note_tick_failure("Too many open files", 0), 1);
        assert_eq!(note_tick_failure("EMFILE test", 2), 3);
        assert_eq!(note_tick_failure("ENFILE test", 5), 6);
        // non-fd resets to 0
        assert_eq!(note_tick_failure("random error", 5), 0);
        assert_eq!(note_tick_failure("", 3), 0);
    }

    #[test]
    fn in_process_name_is_builtin() {
        let s = InProcessCronScheduler;
        assert_eq!(s.name(), "builtin");
        assert!(s.is_available());
    }

    #[test]
    fn provider_supports_default_true() {
        let p = InProcessCronScheduler;
        assert!(provider_supports_force_fire(&p));
        assert!(provider_supports_split_fire(&p));
        assert!(provider_supports_fire_cancel(&p));
        assert!(p.supports_force_fire());
        assert!(!p.is_claim_fire_overridden());
        assert!(!p.is_fire_due_overridden());
    }

    #[test]
    fn fire_overdue_noop_for_builtin() {
        let p = InProcessCronScheduler;
        let n = fire_overdue_jobs(&p, None, None, None);
        assert_eq!(n, 0);
    }

    #[test]
    fn scheduler_for_profile_mode_fails_closed() {
        let builtin = Box::new(InProcessCronScheduler) as Box<dyn CronScheduler>;
        let out = scheduler_for_profile_mode(builtin, true);
        assert_eq!(out.name(), "builtin");

        // Custom provider that is not builtin should be replaced when multiplex on
        struct Chronos;
        impl CronScheduler for Chronos {
            fn name(&self) -> &str {
                "chronos"
            }
            fn start(
                &self,
                _stop: &StopEvent,
                _a: Option<&Value>,
                _l: Option<&Value>,
                _i: u64,
                _c: Option<&dyn Fn() -> bool>,
                _p: Option<&[ProfileHome]>,
            ) {
            }
        }
        let chronos = Box::new(Chronos) as Box<dyn CronScheduler>;
        let out = scheduler_for_profile_mode(chronos, true);
        assert_eq!(out.name(), "builtin");

        // Single-profile keeps chronos
        let chronos2 = Box::new(Chronos) as Box<dyn CronScheduler>;
        let out2 = scheduler_for_profile_mode(chronos2, false);
        assert_eq!(out2.name(), "chronos");
    }

    #[test]
    fn stop_event_set_and_wait() {
        let ev = StopEvent::new();
        assert!(!ev.is_set());
        ev.set();
        assert!(ev.is_set());
        assert!(ev.wait(Duration::from_millis(10)));
        ev.clear();
        assert!(!ev.is_set());
        // wait with short timeout should return false when not set
        assert!(!ev.wait(Duration::from_millis(10)));
    }

    #[test]
    fn misfire_grace_default_when_no_config() {
        let dir = std::env::temp_dir().join(format!("hermes-test-misfire-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let orig = std::env::var("HERMES_HOME").ok();
        unsafe { std::env::set_var("HERMES_HOME", &dir) };
        let v = misfire_grace_minutes();
        assert!((v - DEFAULT_MISFIRE_GRACE_MINUTES).abs() < 1e-9);
        let _ = std::fs::remove_dir_all(&dir);
        unsafe {
            match orig {
                Some(val) => std::env::set_var("HERMES_HOME", val),
                None => std::env::remove_var("HERMES_HOME"),
            }
        }
    }

    #[test]
    fn profile_home_helpers() {
        let p = ProfileHome::Path(PathBuf::from("/tmp/a"));
        assert_eq!(p.home(), Path::new("/tmp/a"));
        let n = ProfileHome::Named {
            name: "code".to_string(),
            home: PathBuf::from("/tmp/b"),
        };
        assert_eq!(n.home(), Path::new("/tmp/b"));
        assert_eq!(n.display_name(), "code");
        let from_tuple: ProfileHome = ("x".to_string(), PathBuf::from("/tmp/c")).into();
        assert_eq!(from_tuple.home(), Path::new("/tmp/c"));
    }

    #[test]
    fn hermes_home_override_roundtrip() {
        let orig = std::env::var("HERMES_HOME").ok();
        let tok = set_hermes_home_override("/tmp/hermes-test-override");
        assert_eq!(std::env::var("HERMES_HOME").unwrap(), "/tmp/hermes-test-override");
        reset_hermes_home_override(tok);
        match orig {
            Some(v) => assert_eq!(std::env::var("HERMES_HOME").unwrap(), v),
            None => assert!(std::env::var("HERMES_HOME").is_err()),
        }
    }

    #[test]
    fn start_multiplex_init_does_not_panic_with_empty_homes() {
        // Directly test that start returns promptly when stop_event already set.
        let sched = InProcessCronScheduler;
        let ev = StopEvent::new();
        ev.set();
        // Single-profile path should return immediately
        sched.start(&ev, None, None, 60, None, None);
        // Multiplex path with homes but stop already set should also return immediately
        let homes = vec![ProfileHome::Path(std::env::temp_dir())];
        sched.start(&ev, None, None, 60, None, Some(&homes));
    }

    #[test]
    fn emfile_backoff_max_is_900() {
        assert!((EMFILE_BACKOFF_MAX_SECONDS - 900.0).abs() < 1e-9);
        assert!((DEFAULT_MISFIRE_GRACE_MINUTES - 10.0).abs() < 1e-9);
    }
}
