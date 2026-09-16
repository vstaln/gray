//! Persistent job store with atomic at-most-once claim (Task 2).
//!
//! Gray-minimal port of the hermes `cron/jobs.py` claim logic: one
//! load-modify-save pass under `<cron>/.jobs.lock` (same flock + 300s-claim
//! shape the daemon used), 300s fire-claim TTL, 120s one-shot grace,
//! fast-forward of stale recurring jobs. Deliberate deltas vs hermes,
//! documented at each site:
//! - owner stamp is `pid:boot-uuid` (no pid-start-time helper in gray; a dead
//!   process never reuses its boot uuid, which is all the TTL needs).
//! - no NL weekday schedules, no per-job model/skill overrides (follow-ups).

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use crate::cron::schedule::{ONESHOT_GRACE_SECS, Schedule, next_run, parse_schedule};

/// Fire-claim TTL: a live claim blocks re-fire; a stale one is reclaimable
/// after a crashed ticker (hermes number).
pub const FIRE_CLAIM_TTL_SECS: i64 = 300;
/// Max job name length (keeps the `cron list` table readable).
pub const MAX_NAME_LEN: usize = 50;
/// Job ids are this many hex chars of a uuid v4.
const JOB_ID_HEX_LEN: usize = 12;

/// Ticker liveness horizon: a heartbeat inside this window means a driver is
/// running. 5x the 60s tick cadence, so one slow pass never reads as dead.
pub const TICKER_STALE_SECS: i64 = 300;

pub fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Who holds a job while it fires: `at` is epoch seconds, `by` is
/// `pid:boot-uuid` of the claimant.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Claim {
    pub at: i64,
    pub by: String,
}

/// Heartbeat left by every tick pass. `kind` names the driver: `serve`, `cli`
/// (one-shot `gray cron tick`), or `repl` (the in-session thread). Without a
/// stamp a store cannot tell "nothing is due" from "nobody is ticking", and a
/// job can sit due forever in silence.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TickStamp {
    pub at: i64,
    pub pid: u32,
    pub kind: String,
}

/// A job that should have fired and did not: `next_run_at` is older than the
/// liveness horizon, so a ticker that ran recently would have claimed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverdueJob {
    pub id: String,
    pub name: String,
    pub next_run_at: i64,
}

/// Store liveness for the reporting surfaces (`cron list`, `cron add`,
/// `/cron`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CronHealth {
    pub last_tick: Option<TickStamp>,
    pub overdue: Vec<OverdueJob>,
}

impl CronHealth {
    /// True when a ticker has been seen inside the liveness horizon.
    pub fn ticker_live(&self, now: i64) -> bool {
        self.last_tick
            .as_ref()
            .is_some_and(|t| now.saturating_sub(t.at) <= TICKER_STALE_SECS)
    }
}

/// Where a finished job's output goes (targets resolve fully in Task 4).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Deliver {
    Origin,
    #[default]
    Local,
    Target(String),
}

/// Chat the job was created from (drives the default `Origin` delivery).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Origin {
    pub platform: String,
    pub chat: String,
    #[serde(default)]
    pub thread: Option<String>,
}

/// Lifecycle: `Paused`/`Done` (terminal) never fire; `claim_due` skips them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    #[default]
    Active,
    Paused,
    Done,
}

/// Outcome of the last fire. `delivery_failed` (send broke, run was fine) is
/// distinct from `error` (the run itself broke) — two columns like hermes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RunStatus {
    #[serde(rename = "ok")]
    Ok,
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "delivery_failed")]
    DeliveryFailed,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub schedule: Schedule,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub state: JobState,
    pub created_at: i64,
    #[serde(default)]
    pub next_run_at: Option<i64>,
    #[serde(default)]
    pub last_run_at: Option<i64>,
    #[serde(default)]
    pub last_status: Option<RunStatus>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub last_delivery_error: Option<String>,
    #[serde(default)]
    pub deliver: Deliver,
    #[serde(default)]
    pub origin: Option<Origin>,
    #[serde(default)]
    pub workdir: Option<PathBuf>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub script: Option<PathBuf>,
    #[serde(default)]
    pub fire_claim: Option<Claim>,
}

fn default_enabled() -> bool {
    true
}

/// Whole-file JSON write via tmp-file + rename (atomic on the same fs), mode
/// 0600 on unix. Owned here so the store has no cross-crate edge for its
/// write path — so gray-cron owns this copy.
/// Do NOT add another copy elsewhere.
pub fn atomic_write_json(path: &Path, value: &impl serde::Serialize) -> anyhow::Result<()> {
    use std::io::Write as _;
    let body = serde_json::to_string_pretty(value)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    // Unique private tmp: no predictable-name collisions, mode set at
    // creation (not after a world-readable window), dir synced with data.
    let tmp = parent.join(format!(".gray-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> anyhow::Result<()> {
        let mut file = options.open(&tmp)?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)?;
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Cross-process mutual exclusion for one load-modify-save pass (same
/// flock/claim shape the daemon used). Blocking variant: the critical
/// section is milliseconds long, and the 30s deadline matches hermes
/// `_JOBS_LOCK_TIMEOUT_SECONDS`. Lock open/timeout/unsupported errors abort
/// the pass instead of running it unguarded: at-most-once claims require
/// actual mutual exclusion, and a disabled scheduler is safer than an
/// approximate one.
fn with_jobs_lock<T>(lock_path: &Path, f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => {
                anyhow::ensure!(
                    std::time::Instant::now() < deadline,
                    "cron lock timeout on {}",
                    lock_path.display()
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
    }
    // Closing the file releases the lock, including on error/unwind.
    f()
}

fn validate_new_job(
    name: &str,
    prompt: &str,
    workdir: Option<&Path>,
    skills: &[String],
    script: Option<&Path>,
) -> anyhow::Result<()> {
    if name.trim().is_empty() {
        anyhow::bail!("job name is empty");
    }
    if name.chars().count() > MAX_NAME_LEN {
        anyhow::bail!("job name exceeds {MAX_NAME_LEN} chars");
    }
    if prompt.trim().is_empty() {
        anyhow::bail!("job prompt is empty");
    }
    if let Some(dir) = workdir {
        if !dir.is_absolute() {
            anyhow::bail!("workdir must be absolute: {}", dir.display());
        }
        if !dir.is_dir() {
            anyhow::bail!("workdir is not an existing dir: {}", dir.display());
        }
    }
    for s in skills {
        anyhow::ensure!(!s.trim().is_empty(), "job skill is empty");
        anyhow::ensure!(
            s.chars().count() <= MAX_NAME_LEN,
            "job skill exceeds {MAX_NAME_LEN} chars"
        );
    }
    if let Some(path) = script {
        anyhow::ensure!(
            path.is_absolute(),
            "script must be absolute: {}",
            path.display()
        );
        anyhow::ensure!(
            path.is_file(),
            "script is not an existing file: {}",
            path.display()
        );
    }
    Ok(())
}

fn new_job_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..JOB_ID_HEX_LEN].to_owned()
}

/// `dir` is the cron dir itself (holds `jobs.json` + `.jobs.lock`), e.g.
/// `CronStore::open(home.join("cron"))`.
#[derive(Debug, Clone)]
pub struct CronStore {
    cron_dir: PathBuf,
}

impl CronStore {
    pub fn open(dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let cron_dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&cron_dir)?;
        Ok(Self { cron_dir })
    }

    fn jobs_path(&self) -> PathBuf {
        self.cron_dir.join("jobs.json")
    }

    fn lock_path(&self) -> PathBuf {
        self.cron_dir.join(".jobs.lock")
    }

    fn tick_stamp_path(&self) -> PathBuf {
        self.cron_dir.join(".last_tick")
    }

    /// Record a tick heartbeat. Every driver writes one per pass — including
    /// passes that fire nothing, which is precisely the signal that separates
    /// "nothing due" from "nothing ticking". Callers treat failure as
    /// best-effort: a missing heartbeat must never stop a job from firing.
    pub fn record_tick(&self, kind: &str) -> anyhow::Result<()> {
        atomic_write_json(
            &self.tick_stamp_path(),
            &TickStamp {
                at: now_secs(),
                pid: std::process::id(),
                kind: kind.to_string(),
            },
        )
    }

    /// Last heartbeat, if any. Missing or corrupt reads as `None` (warned):
    /// liveness reporting must never fail a `list`.
    pub fn last_tick(&self) -> anyhow::Result<Option<TickStamp>> {
        let body = match std::fs::read_to_string(self.tick_stamp_path()) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        match serde_json::from_str(&body) {
            Ok(stamp) => Ok(Some(stamp)),
            Err(e) => {
                log::warn!(
                    "cron: unreadable tick stamp {} ({e}), treating as never ticked",
                    self.tick_stamp_path().display()
                );
                Ok(None)
            }
        }
    }

    /// Liveness + overdue jobs for the reporting surfaces. Read-only and
    /// deliberately not under the jobs lock: a stale read only mislabels a
    /// line of text, while `list` must not queue behind a job that is busy
    /// firing.
    pub fn health(&self, now: i64) -> anyhow::Result<CronHealth> {
        let last_tick = self.last_tick()?;
        let mut overdue = Vec::new();
        for job in self.list()? {
            if !job.enabled || job.state != JobState::Active {
                continue;
            }
            let Some(next) = job.next_run_at else {
                continue;
            };
            if next + TICKER_STALE_SECS >= now {
                continue;
            }
            if let Some(claim) = &job.fire_claim
                && now.saturating_sub(claim.at) <= FIRE_CLAIM_TTL_SECS
            {
                continue; // a driver is on it right now
            }
            overdue.push(OverdueJob {
                id: job.id.clone(),
                name: job.name.clone(),
                next_run_at: next,
            });
        }
        Ok(CronHealth { last_tick, overdue })
    }

    /// Raw records; supports the bare array we write plus the `{"jobs": [...]}`
    /// envelope. Missing/empty reads as empty; anything malformed or
    /// unreadable is an error — a later save must never silently overwrite a
    /// corrupt store with just the new record.
    fn load_raw(&self) -> anyhow::Result<Vec<serde_json::Value>> {
        let data = match std::fs::read_to_string(self.jobs_path()) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
            Ok(d) => d,
        };
        if data.trim().is_empty() {
            return Ok(Vec::new());
        }
        match serde_json::from_str::<serde_json::Value>(data.trim()) {
            Ok(serde_json::Value::Array(items)) => Ok(items),
            Ok(serde_json::Value::Object(mut obj)) => match obj.remove("jobs") {
                Some(serde_json::Value::Array(items)) => Ok(items),
                _ => anyhow::bail!("cron store has no jobs array"),
            },
            Ok(_) => anyhow::bail!("cron store is not a JSON array"),
            Err(e) => Err(anyhow::anyhow!("cron store is not JSON ({e})")),
        }
    }

    /// Parsed jobs; one malformed record warns and skips, never aborts the scan.
    fn parse_jobs(raw: &[serde_json::Value]) -> Vec<CronJob> {
        let mut jobs = Vec::new();
        for v in raw {
            match serde_json::from_value::<CronJob>(v.clone()) {
                Ok(j) => jobs.push(j),
                Err(e) => log::warn!("cron store: skipping malformed record: {e}"),
            }
        }
        jobs
    }

    /// Write back, substituting updated jobs by id so malformed/unknown records
    /// survive the pass (poison-proof save to match the poison-proof load).
    fn save_jobs(&self, raw: &mut Vec<serde_json::Value>, jobs: &[CronJob]) -> anyhow::Result<()> {
        for job in jobs {
            let v = serde_json::to_value(job)?;
            match raw
                .iter_mut()
                .find(|x| x.get("id").and_then(|i| i.as_str()) == Some(job.id.as_str()))
            {
                Some(slot) => *slot = v,
                None => raw.push(v),
            }
        }
        atomic_write_json(&self.jobs_path(), raw)
    }

    /// Nine args is the store's creation seam (name/schedule/prompt/deliver/
    /// origin/workdir/skills/script): one constructor, validated once. Splitting
    /// it would scatter the atomic insert across call sites.
    #[allow(clippy::too_many_arguments)]
    pub fn add_full(
        &self,
        name: &str,
        schedule: &str,
        prompt: &str,
        deliver: Deliver,
        origin: Option<Origin>,
        workdir: Option<PathBuf>,
        skills: Vec<String>,
        script: Option<PathBuf>,
    ) -> anyhow::Result<String> {
        validate_new_job(name, prompt, workdir.as_deref(), &skills, script.as_deref())?;
        let sched = parse_schedule(schedule)?;
        let now = now_secs();
        if let Schedule::Once { at } = &sched
            && *at < now - ONESHOT_GRACE_SECS
        {
            anyhow::bail!("one-shot time is more than {ONESHOT_GRACE_SECS}s in the past");
        }
        let job = CronJob {
            id: new_job_id(),
            name: name.trim().to_string(),
            prompt: prompt.to_string(),
            schedule: sched,
            enabled: true,
            state: JobState::Active,
            created_at: now,
            next_run_at: next_run(now, &parse_schedule(schedule)?),
            last_run_at: None,
            last_status: None,
            last_error: None,
            last_delivery_error: None,
            deliver,
            origin,
            workdir,
            skills: skills.into_iter().map(|s| s.trim().to_string()).collect(),
            script,
            fire_claim: None,
        };
        let id = job.id.clone();
        with_jobs_lock(&self.lock_path(), || {
            let mut raw = self.load_raw()?;
            self.save_jobs(&mut raw, &[job])
        })?;
        Ok(id)
    }

    pub fn list(&self) -> anyhow::Result<Vec<CronJob>> {
        let lock = self.lock_path();
        with_jobs_lock(&lock, || {
            let mut jobs = Self::parse_jobs(&self.load_raw()?);
            jobs.sort_by_key(|j| j.next_run_at.unwrap_or(i64::MAX));
            Ok(jobs)
        })
    }

    /// Lookup by id or name (CLI convenience, mirrors the old sidecar).
    pub fn get(&self, id_or_name: &str) -> anyhow::Result<Option<CronJob>> {
        let lock = self.lock_path();
        with_jobs_lock(&lock, || {
            Ok(Self::parse_jobs(&self.load_raw()?)
                .into_iter()
                .find(|j| j.id == id_or_name || j.name == id_or_name))
        })
    }

    pub fn remove(&self, id_or_name: &str) -> anyhow::Result<bool> {
        with_jobs_lock(&self.lock_path(), || {
            let mut raw = self.load_raw()?;
            let before = raw.len();
            raw.retain(|x| {
                let id_match = x.get("id").and_then(|i| i.as_str()) == Some(id_or_name);
                let name_match = x.get("name").and_then(|n| n.as_str()) == Some(id_or_name);
                !(id_match || name_match)
            });
            if raw.len() == before {
                return Ok(false);
            }
            atomic_write_json(&self.jobs_path(), &raw)?;
            Ok(true)
        })
    }

    /// Claim all due jobs for `owner` in one pass (poison-proof): skip
    /// disabled/paused/done, skip live claims (reclaim claims older than
    /// [`FIRE_CLAIM_TTL_SECS`]), skip the not-yet-due, retire missed
    /// one-shots (never fire), and stamp + advance everything else.
    /// Stale recurring jobs fast-forward (`next_run_at` jumps past now) but
    /// STILL fire once now — no grace branch is needed because stale and fresh
    /// recurring jobs behave identically. Single save at end.
    pub fn claim_due(&self, now: i64, owner: &str) -> anyhow::Result<Vec<CronJob>> {
        with_jobs_lock(&self.lock_path(), || {
            let mut raw = self.load_raw()?;
            let mut jobs = Self::parse_jobs(&raw);
            let mut due = Vec::new();
            let mut dirty = false;
            for job in jobs.iter_mut() {
                if !job.enabled || job.state != JobState::Active {
                    continue;
                }
                // A live claim blocks re-claim. A claim older than the TTL is a
                // crashed worker (mark_done always clears on completion): reclaim
                // it once, loudly, for this owner. At-most-once is preserved for
                // any claimant whose claim is younger than the TTL.
                if let Some(claim) = &job.fire_claim {
                    if now.saturating_sub(claim.at) <= FIRE_CLAIM_TTL_SECS {
                        continue;
                    }
                    log::warn!(
                        "cron job {}: reclaiming stale fire claim by {} ({}s old)",
                        job.id,
                        claim.by,
                        now.saturating_sub(claim.at)
                    );
                    job.fire_claim = None;
                    dirty = true;
                }
                if matches!(job.schedule, Schedule::Once { .. }) && job.last_run_at.is_some() {
                    continue; // completed one-shot, retained for inspection
                }
                let next = match job.next_run_at {
                    Some(n) => n,
                    None => match next_run(now, &job.schedule) {
                        Some(n) => {
                            job.next_run_at = Some(n);
                            dirty = true;
                            n
                        }
                        None => {
                            log::warn!(
                                "cron job {}: schedule no longer resolves; skipping",
                                job.id
                            );
                            continue;
                        }
                    },
                };
                if next > now {
                    continue;
                }
                if matches!(job.schedule, Schedule::Once { .. }) {
                    if next < now - ONESHOT_GRACE_SECS {
                        job.enabled = false;
                        job.state = JobState::Done;
                        job.last_error = Some("missed one-shot window".to_string());
                        dirty = true;
                        continue; // retire, never fire
                    }
                } else {
                    match next_run(now, &job.schedule) {
                        Some(advanced) => {
                            job.next_run_at = Some(advanced);
                        }
                        None => {
                            log::warn!("cron job {}: cannot advance schedule; skipping", job.id);
                            continue;
                        }
                    }
                }
                job.fire_claim = Some(Claim {
                    at: now,
                    by: owner.to_string(),
                });
                dirty = true;
                due.push(job.clone());
            }
            if dirty {
                self.save_jobs(&mut raw, &jobs)?;
            }
            Ok(due)
        })
    }

    /// Record a fire outcome: clears the claim, stamps status + timestamps. A
    /// finished one-shot goes terminal (`Done`) so it is retained but never
    /// re-fires. `DeliveryFailed` writes the delivery column, anything else the
    /// run column (hermes parity).
    /// Pause (`paused=true` → `Paused`) or resume (`false` → `Active`).
    /// Resume recomputes `next_run_at` from now so a long-paused job is not
    /// instantly stale. Returns false when the job is unknown.
    /// `enabled` is left untouched: `claim_due` already requires both.
    pub fn set_paused(&self, id_or_name: &str, paused: bool) -> anyhow::Result<bool> {
        with_jobs_lock(&self.lock_path(), || {
            let mut raw = self.load_raw()?;
            let mut jobs = Self::parse_jobs(&raw);
            let Some(job) = jobs
                .iter_mut()
                .find(|j| j.id == id_or_name || j.name == id_or_name)
            else {
                return Ok(false);
            };
            if paused {
                job.state = JobState::Paused;
            } else {
                job.state = JobState::Active;
                job.next_run_at = next_run(now_secs(), &job.schedule);
            }
            self.save_jobs(&mut raw, &jobs)?;
            Ok(true)
        })
    }

    /// Claim one job by id/name for `owner`, bypassing due-ness (implements
    /// `gray cron run`). Same guards as `claim_due`: skip disabled/paused/
    /// done, skip live claims (reclaim past [`FIRE_CLAIM_TTL_SECS`]), retire
    /// missed one-shots without firing, advance recurring schedules.
    pub fn claim_one(
        &self,
        now: i64,
        owner: &str,
        id_or_name: &str,
    ) -> anyhow::Result<Option<CronJob>> {
        with_jobs_lock(&self.lock_path(), || {
            let mut raw = self.load_raw()?;
            let mut jobs = Self::parse_jobs(&raw);
            let Some(job) = jobs
                .iter_mut()
                .find(|j| j.id == id_or_name || j.name == id_or_name)
            else {
                return Ok(None);
            };
            if !job.enabled || job.state != JobState::Active {
                return Ok(None);
            }
            if let Some(claim) = &job.fire_claim
                && now.saturating_sub(claim.at) <= FIRE_CLAIM_TTL_SECS
            {
                return Ok(None);
            }
            job.fire_claim = None;
            if matches!(job.schedule, Schedule::Once { .. }) && job.last_run_at.is_some() {
                return Ok(None);
            }
            if matches!(job.schedule, Schedule::Once { at } if at < now - ONESHOT_GRACE_SECS) {
                job.enabled = false;
                job.state = JobState::Done;
                job.last_error = Some("missed one-shot window".to_string());
                self.save_jobs(&mut raw, &jobs)?;
                return Ok(None);
            }
            if !matches!(job.schedule, Schedule::Once { .. }) {
                match next_run(now, &job.schedule) {
                    Some(advanced) => job.next_run_at = Some(advanced),
                    None => return Ok(None),
                }
            }
            job.fire_claim = Some(Claim {
                at: now,
                by: owner.to_string(),
            });
            let out = job.clone();
            self.save_jobs(&mut raw, &jobs)?;
            Ok(Some(out))
        })
    }

    pub fn mark_done(
        &self,
        id_or_name: &str,
        status: RunStatus,
        err: Option<&str>,
    ) -> anyhow::Result<()> {
        with_jobs_lock(&self.lock_path(), || {
            let mut raw = self.load_raw()?;
            let mut jobs = Self::parse_jobs(&raw);
            let Some(job) = jobs
                .iter_mut()
                .find(|j| j.id == id_or_name || j.name == id_or_name)
            else {
                anyhow::bail!("unknown cron job {id_or_name:?}");
            };
            job.fire_claim = None;
            job.last_status = Some(status);
            job.last_run_at = Some(now_secs());
            match status {
                RunStatus::DeliveryFailed => job.last_delivery_error = err.map(str::to_string),
                _ => job.last_error = err.map(str::to_string),
            }
            if matches!(job.schedule, Schedule::Once { .. }) {
                job.state = JobState::Done;
            }
            self.save_jobs(&mut raw, &jobs)
        })
    }

    #[cfg(test)]
    fn set_next_run_for_test(&self, id: &str, ts: i64) -> anyhow::Result<()> {
        with_jobs_lock(&self.lock_path(), || {
            let mut raw = self.load_raw()?;
            let mut jobs = Self::parse_jobs(&raw);
            let Some(job) = jobs.iter_mut().find(|j| j.id == id) else {
                anyhow::bail!("unknown cron job {id:?}");
            };
            job.next_run_at = Some(ts);
            self.save_jobs(&mut raw, &jobs)
        })
    }
}

#[path = "store_tests.rs"]
#[cfg(test)]
mod tests;
