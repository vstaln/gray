//! Persistent job store with atomic at-most-once claim (Task 2).
//!
//! Gray-minimal port of the hermes `cron/jobs.py` claim logic: one
//! load-modify-save pass under `<cron>/.jobs.lock` (same flock mechanism as
//! `gray-gateway/src/lock.rs`), 300s fire-claim TTL, 120s one-shot grace,
//! fast-forward of stale recurring jobs. Deliberate deltas vs hermes,
//! documented at each site:
//! - owner stamp is `pid:boot-uuid` (no pid-start-time helper in gray; a dead
//!   process never reuses its boot uuid, which is all the TTL needs).
//! - lifecycle guard is a 3-shape substring floor, not the full hermes
//!   detection table (`reject_lifecycle_shape`).
//! - no NL weekday schedules, no per-job model/skill overrides (follow-ups).

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use crate::schedule::{ONESHOT_GRACE_SECS, Schedule, next_run, parse_schedule};

/// Fire-claim TTL: a live claim blocks re-fire; a stale one is reclaimable
/// after a crashed ticker (hermes number).
pub const FIRE_CLAIM_TTL_SECS: i64 = 300;
/// Max job name length (keeps the `cron list` table readable).
pub const MAX_NAME_LEN: usize = 50;
/// Job ids are this many hex chars of a uuid v4.
const JOB_ID_HEX_LEN: usize = 12;

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
    pub fire_claim: Option<Claim>,
}

fn default_enabled() -> bool {
    true
}

/// Whole-file JSON write via tmp-file + rename (atomic on the same fs), mode
/// 0600 on unix. Plan-A fallback home: `gray_gateway::delivery::atomic_write_json`
/// is `pub(crate)` there, and depending on gray-gateway from here would cycle
/// (gateway gains the gray-cron dep in Task 3) — so gray-cron owns this copy.
/// Do NOT add a third copy elsewhere.
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

/// Cross-process mutual exclusion for one load-modify-save pass. Same flock
/// mechanism as `gray-gateway/src/lock.rs`, blocking variant: the critical
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

/// Gray-minimal lifecycle floor (documented as the floor, not the ceiling):
/// hermes runs a full detection table (`cron/lifecycle_guard.py` — anchored
/// regexes, argv tokenizing, referenced-script walks). That table is a
/// follow-up here; this rejects the three shapes that SIGTERM-respawn a
/// supervised gateway, case-insensitive substring on normalized lowercase.
pub fn reject_lifecycle_shape(prompt: &str) -> anyhow::Result<()> {
    let n = prompt.to_lowercase();
    if n.contains("gateway restart") || n.contains("gateway stop") {
        anyhow::bail!("blocked: cron prompt targets the gateway lifecycle (respawn loop)");
    }
    if n.contains("systemctl") && n.contains("gray") {
        anyhow::bail!("blocked: cron prompt targets the gray service lifecycle (respawn loop)");
    }
    if n.contains("pkill") && n.contains("gray") {
        anyhow::bail!("blocked: cron prompt kills the gray process (respawn loop)");
    }
    Ok(())
}

fn validate_new_job(name: &str, prompt: &str, workdir: Option<&Path>) -> anyhow::Result<()> {
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

    /// Add a job with the lifecycle-shape guard applied (default for
    /// untrusted prompts).
    pub fn add_full(
        &self,
        name: &str,
        schedule: &str,
        prompt: &str,
        deliver: Deliver,
        origin: Option<Origin>,
        workdir: Option<PathBuf>,
    ) -> anyhow::Result<String> {
        self.add_full_inner(name, schedule, prompt, deliver, origin, workdir, true)
    }

    /// Like [`add_full`], but skips [`reject_lifecycle_shape`]. For callers
    /// that legitimately manage gray's own lifecycle — e.g. the heartbeat,
    /// whose standing goal may mention restarting the gateway. Never feed
    /// this raw end-user input.
    pub fn add_full_unguarded(
        &self,
        name: &str,
        schedule: &str,
        prompt: &str,
        deliver: Deliver,
        origin: Option<Origin>,
        workdir: Option<PathBuf>,
    ) -> anyhow::Result<String> {
        self.add_full_inner(name, schedule, prompt, deliver, origin, workdir, false)
    }

    #[allow(clippy::too_many_arguments)] // mirrors add_full plus the guard flag
    fn add_full_inner(
        &self,
        name: &str,
        schedule: &str,
        prompt: &str,
        deliver: Deliver,
        origin: Option<Origin>,
        workdir: Option<PathBuf>,
        guard: bool,
    ) -> anyhow::Result<String> {
        validate_new_job(name, prompt, workdir.as_deref())?;
        if guard {
            reject_lifecycle_shape(prompt)?;
        }
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

#[cfg(test)]
mod tests {
    // UNRUN (cargo test banned under X): run in TTY/CI.
    use super::*;

    impl CronStore {
        fn add(
            &self,
            name: &str,
            schedule: &str,
            prompt: &str,
            deliver: Deliver,
        ) -> anyhow::Result<String> {
            self.add_full(name, schedule, prompt, deliver, None, None)
        }
    }

    fn test_store() -> (tempfile::TempDir, CronStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = CronStore::open(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn claim_is_at_most_once_across_handles() {
        let (_dir, store) = test_store();
        let id = store
            .add("ping", "every 1h", "say hi", Deliver::Local)
            .unwrap();
        // force due:
        store.set_next_run_for_test(&id, 1).unwrap();
        let cron_dir = store.cron_dir.clone();
        let a = CronStore::open(&cron_dir).unwrap();
        let b = CronStore::open(&cron_dir).unwrap();
        let got_a = a.claim_due(now_secs(), "owner-a").unwrap();
        let got_b = b.claim_due(now_secs(), "owner-b").unwrap();
        assert_eq!(got_a.len() + got_b.len(), 1, "exactly one claimant wins");
    }

    #[test]
    fn live_claim_never_reclaimed_within_ttl() {
        let (_dir, store) = test_store();
        let id = store
            .add("hourly", "every 1h", "say hi", Deliver::Local)
            .unwrap();
        let t0 = 1_700_000_000;
        store.set_next_run_for_test(&id, 1).unwrap();
        assert_eq!(store.claim_due(t0, "owner-a").unwrap().len(), 1);
        // Force the job due again while the claim is still live: the live
        // claim must still block re-fire (at-most-once).
        store.set_next_run_for_test(&id, 1).unwrap();
        let second = store.claim_due(t0 + 10, "owner-b").unwrap();
        assert!(second.is_empty(), "live claim must not re-fire");
    }

    #[test]
    fn stale_claim_is_reclaimed_with_warning_and_refires_once() {
        let (_dir, store) = test_store();
        let id = store
            .add("hourly", "every 1h", "say hi", Deliver::Local)
            .unwrap();
        let t0 = 1_700_000_000;
        store.set_next_run_for_test(&id, 1).unwrap();
        assert_eq!(store.claim_due(t0, "owner-a").unwrap().len(), 1);
        // Crashed worker: the job is due, the claim is past the TTL.
        store.set_next_run_for_test(&id, 1).unwrap();
        let due = store
            .claim_due(t0 + FIRE_CLAIM_TTL_SECS + 1, "owner-b")
            .unwrap();
        assert_eq!(due.len(), 1, "stale claim must be reclaimed past the TTL");
        assert_eq!(due[0].fire_claim.as_ref().unwrap().by, "owner-b");
        // The reclaim is a single winner: the new claim is live for others.
        let third = store
            .claim_due(t0 + FIRE_CLAIM_TTL_SECS + 2, "owner-c")
            .unwrap();
        assert!(third.is_empty(), "reclaimed job is at-most-once again");
    }

    #[test]
    fn corrupt_store_fails_loads_and_mutations() {
        let (dir, store) = test_store();
        std::fs::write(dir.path().join("jobs.json"), "{torn").unwrap();
        assert!(store.list().is_err());
        assert!(store.claim_due(1_700_000_000, "o").is_err());
        assert!(store.add("x", "every 1h", "hi", Deliver::Local).is_err());
        // Missing store still reads empty and accepts the first add.
        std::fs::remove_file(dir.path().join("jobs.json")).unwrap();
        assert!(store.list().unwrap().is_empty());
        store.add("x", "every 1h", "hi", Deliver::Local).unwrap();
    }

    #[test]
    fn add_rejects_bad_specs() {
        let (_dir, store) = test_store();
        assert!(store.add("", "every 1h", "hi", Deliver::Local).is_err());
        assert!(
            store
                .add(
                    &"n".repeat(MAX_NAME_LEN + 1),
                    "every 1h",
                    "hi",
                    Deliver::Local
                )
                .is_err()
        );
        assert!(store.add("x", "every 1h", "   ", Deliver::Local).is_err());
        assert!(store.add("x", "every 30s", "hi", Deliver::Local).is_err());
        assert!(
            store
                .add("x", "not a schedule", "hi", Deliver::Local)
                .is_err()
        );
        assert!(
            store
                .add(
                    "x",
                    "every 1h",
                    "run gateway restart nightly",
                    Deliver::Local
                )
                .is_err()
        );
        assert!(
            store
                .add(
                    "x",
                    "every 1h",
                    "systemctl restart gray-gateway",
                    Deliver::Local
                )
                .is_err()
        );
        assert!(
            store
                .add("x", "every 1h", "pkill -f gray", Deliver::Local)
                .is_err()
        );
        assert!(
            store
                .add_full(
                    "x",
                    "every 1h",
                    "hi",
                    Deliver::Local,
                    None,
                    Some(PathBuf::from("relative/path")),
                )
                .is_err()
        );
        assert!(
            store
                .add_full(
                    "x",
                    "every 1h",
                    "hi",
                    Deliver::Local,
                    None,
                    Some(PathBuf::from("/no/such/dir/gray-cron-test")),
                )
                .is_err()
        );
        // benign prompt mentioning no lifecycle shape passes
        let id = store
            .add("ok", "every 1h", "check CI and report", Deliver::Local)
            .unwrap();
        assert_eq!(store.get(&id).unwrap().unwrap().name, "ok");
    }

    #[test]
    fn add_full_unguarded_bypasses_lifecycle_guard() {
        let (_dir, store) = test_store();
        // Guarded path still rejects a lifecycle-shaped prompt.
        assert!(
            store
                .add("g", "every 1h", "systemctl restart gray", Deliver::Local)
                .is_err()
        );
        // Unguarded path accepts the same prompt (heartbeat standing goal).
        let id = store
            .add_full_unguarded(
                "u",
                "every 1h",
                "systemctl restart gray",
                Deliver::Local,
                None,
                None,
            )
            .unwrap();
        assert_eq!(store.get(&id).unwrap().unwrap().name, "u");
    }

    #[test]
    fn missed_oneshot_retires_without_firing() {
        let (_dir, store) = test_store();
        let at = now_secs() - 1000;
        let raw_job = serde_json::json!({
            "id": "once1",
            "name": "once",
            "prompt": "hi",
            "schedule": {"Once": {"at": at}},
            "enabled": true,
            "created_at": at,
            "next_run_at": at,
        });
        std::fs::write(
            store.jobs_path(),
            serde_json::to_string_pretty(&vec![raw_job]).unwrap(),
        )
        .unwrap();
        let due = store.claim_due(now_secs(), "owner").unwrap();
        assert!(due.is_empty(), "missed one-shot must never fire");
        let job = store.get("once1").unwrap().unwrap();
        assert!(!job.enabled);
        assert_eq!(job.state, JobState::Done);
        assert_eq!(job.last_error.as_deref(), Some("missed one-shot window"));
    }

    #[test]
    fn stale_recurring_fast_forwards_but_fires_once() {
        let (_dir, store) = test_store();
        let id = store
            .add("hourly", "every 1h", "hi", Deliver::Local)
            .unwrap();
        store
            .set_next_run_for_test(&id, now_secs() - 5 * 3600)
            .unwrap();
        let due = store.claim_due(now_secs(), "owner").unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, id);
        let job = store.get(&id).unwrap().unwrap();
        assert!(job.next_run_at.unwrap() > now_secs(), "advanced past now");
        assert!(job.fire_claim.unwrap().by == "owner");
        // second tick finds nothing due (already advanced + claimed)
        assert!(store.claim_due(now_secs(), "owner2").unwrap().is_empty());
    }

    #[test]
    fn mark_done_clears_claim_and_records_status() {
        let (_dir, store) = test_store();
        let id = store
            .add("hourly", "every 1h", "hi", Deliver::Local)
            .unwrap();
        store.set_next_run_for_test(&id, 1).unwrap();
        let due = store.claim_due(now_secs(), "owner").unwrap();
        assert_eq!(due.len(), 1);
        store.mark_done(&id, RunStatus::Ok, None).unwrap();
        let job = store.get(&id).unwrap().unwrap();
        assert!(job.fire_claim.is_none());
        assert_eq!(job.last_status, Some(RunStatus::Ok));
        assert!(job.last_run_at.is_some());
        // one-shot completion goes terminal
        let dir2 = tempfile::tempdir().unwrap();
        let s2 = CronStore::open(dir2.path()).unwrap();
        let oid = s2.add("once", "in 10m", "hi", Deliver::Local).unwrap();
        s2.mark_done(&oid, RunStatus::Error, Some("boom")).unwrap();
        let o = s2.get(&oid).unwrap().unwrap();
        assert_eq!(o.state, JobState::Done);
        assert_eq!(o.last_status, Some(RunStatus::Error));
        assert_eq!(o.last_error.as_deref(), Some("boom"));
    }

    #[test]
    fn malformed_record_does_not_abort_scan() {
        let (_dir, store) = test_store();
        let id = store.add("good", "every 1h", "hi", Deliver::Local).unwrap();
        store.set_next_run_for_test(&id, 1).unwrap();
        let mut raw: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(store.jobs_path()).unwrap()).unwrap();
        raw.push(serde_json::json!({"id": "bad", "name": 42}));
        std::fs::write(
            store.jobs_path(),
            serde_json::to_string_pretty(&raw).unwrap(),
        )
        .unwrap();
        let due = store.claim_due(now_secs(), "owner").unwrap();
        assert_eq!(due.len(), 1, "good record still fires past a bad one");
        assert_eq!(due[0].id, id);
        // bad record survives the save pass
        let raw2: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(store.jobs_path()).unwrap()).unwrap();
        assert!(
            raw2.iter()
                .any(|v| v.get("id").and_then(|i| i.as_str()) == Some("bad"))
        );
    }
}
