//! Port of hermes-rs/crates/hermes-cron/src/scheduler.rs — trimmed for gray
//! - InflightGuard with Pending/Live + sweep_stale_inflight max(2*interval,30min)
//! - Scheduler::scan_due_jobs with grace, fast-forward, missing next_run recovery

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::store::{CronJob, CronStorePaths, with_jobs_lock};

// ---------------------------------------------------------------------------
// InflightGuard — hermes/scheduler.rs:148-422 verbatim trimmed
// ---------------------------------------------------------------------------

pub const INFLIGHT_MIN_ALLOWANCE_MINUTES: f64 = 30.0;

pub fn inflight_min_allowance_minutes() -> f64 {
    if let Ok(content) = std::fs::read_to_string(
        std::env::var("GRAY_HOME")
            .map(|h| format!("{h}/config.yaml"))
            .unwrap_or_else(|_| {
                std::env::var("HOME")
                    .map(|h| format!("{h}/.gray/config.yaml"))
                    .unwrap_or_else(|_| ".gray/config.yaml".into())
            }),
    ) {
        if let Ok(cfg) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
            if let Some(v) = cfg
                .get("cron")
                .and_then(|c| c.get("inflight_max_minutes"))
                .and_then(|v| v.as_f64())
            {
                if v > 0.0 {
                    return v;
                }
            }
        }
    }
    if let Ok(raw) = std::env::var("GRAY_CRON_INFLIGHT_MAX_MINUTES") {
        if let Ok(v) = raw.trim().parse::<f64>() {
            if v > 0.0 {
                return v;
            }
        }
    }
    INFLIGHT_MIN_ALLOWANCE_MINUTES
}

const FORCED_RELEASE_HISTORY: usize = 20;

enum Slot {
    Pending,
    Live(Arc<dyn Fn() -> bool + Send + Sync>),
}

impl std::fmt::Debug for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Slot::Pending => f.write_str("Pending"),
            Slot::Live(_) => f.write_str("Live(_)"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ForcedReleaseEntry {
    pub job_id: String,
    pub name: String,
    pub age_seconds: f64,
    pub allowance_seconds: f64,
    pub at: String,
}

#[derive(Debug, Default)]
struct GuardInner {
    running: HashSet<String>,
    since: HashMap<String, std::time::Instant>,
    slots: HashMap<String, Slot>,
    forced_release_count: u64,
    recent_forced_releases: VecDeque<ForcedReleaseEntry>,
}

#[derive(Debug, Default)]
pub struct InflightGuard {
    inner: Mutex<GuardInner>,
}

#[derive(Debug, Clone)]
pub struct InflightGuardStats {
    pub running: Vec<String>,
    pub running_ages_seconds: HashMap<String, f64>,
    pub forced_releases: u64,
    pub recent_forced_releases: Vec<ForcedReleaseEntry>,
}

impl InflightGuard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn try_register_running_job(&self, job_id: &str) -> bool {
        let mut g = self.inner.lock().unwrap();
        if g.running.contains(job_id) {
            return false;
        }
        g.running.insert(job_id.to_string());
        g.since.insert(job_id.to_string(), std::time::Instant::now());
        g.slots.insert(job_id.to_string(), Slot::Pending);
        true
    }

    pub fn install_dispatch_probe(
        &self,
        job_id: &str,
        is_finished: Arc<dyn Fn() -> bool + Send + Sync>,
    ) {
        let mut g = self.inner.lock().unwrap();
        if g.running.contains(job_id) {
            g.slots.insert(job_id.to_string(), Slot::Live(is_finished));
        }
    }

    pub fn release_running_job(&self, job_id: &str) {
        let mut g = self.inner.lock().unwrap();
        g.running.remove(job_id);
        g.since.remove(job_id);
        g.slots.remove(job_id);
    }

    pub fn get_running_job_ids(&self) -> HashSet<String> {
        self.inner.lock().unwrap().running.clone()
    }

    pub fn get_inflight_guard_stats(&self) -> InflightGuardStats {
        let now = std::time::Instant::now();
        let g = self.inner.lock().unwrap();
        InflightGuardStats {
            running: {
                let mut v: Vec<String> = g.running.iter().cloned().collect();
                v.sort();
                v
            },
            running_ages_seconds: g
                .since
                .iter()
                .map(|(k, t)| (k.clone(), now.duration_since(*t).as_secs_f64()))
                .collect(),
            forced_releases: g.forced_release_count,
            recent_forced_releases: g.recent_forced_releases.iter().cloned().collect(),
        }
    }

    pub fn sweep_stale_inflight(&self, store: &CronStorePaths, due_jobs: &[CronJob]) -> Vec<String> {
        let floor_seconds = inflight_min_allowance_minutes() * 60.0;
        self.sweep_with_floor(store, due_jobs, floor_seconds)
    }

    fn sweep_with_floor(
        &self,
        store: &CronStorePaths,
        due_jobs: &[CronJob],
        floor_seconds: f64,
    ) -> Vec<String> {
        let by_id: HashMap<&str, &CronJob> = due_jobs.iter().map(|j| (j.id.as_str(), j)).collect();
        let intervals: HashMap<&str, Option<f64>> = by_id
            .iter()
            .map(|(id, job)| (*id, job_interval_minutes(job)))
            .collect();

        let now = std::time::Instant::now();
        let mut stale: Vec<(String, f64, f64)> = Vec::new();
        {
            let mut g = self.inner.lock().unwrap();
            for job_id in g.running.clone() {
                let started = match g.since.get(&job_id) {
                    None => {
                        g.since.insert(job_id.clone(), now);
                        continue;
                    }
                    Some(t) => *t,
                };
                let age = now.duration_since(started).as_secs_f64();
                let allowance = match intervals.get(job_id.as_str()).copied().flatten() {
                    Some(minutes) => floor_seconds.max(2.0 * minutes * 60.0),
                    None => floor_seconds,
                };
                let releasable = match g.slots.get(&job_id) {
                    None => true,
                    Some(Slot::Pending) => true,
                    Some(Slot::Live(probe)) => probe(),
                };
                if !releasable || age < allowance {
                    continue;
                }
                g.running.remove(&job_id);
                g.since.remove(&job_id);
                g.slots.remove(&job_id);
                g.forced_release_count += 1;
                let name = by_id
                    .get(job_id.as_str())
                    .map(|j| j.name.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&job_id)
                    .to_string();
                let entry = ForcedReleaseEntry {
                    job_id: job_id.clone(),
                    name,
                    age_seconds: (age * 10.0).round() / 10.0,
                    allowance_seconds: (allowance * 10.0).round() / 10.0,
                    at: Utc::now().to_rfc3339(),
                };
                g.recent_forced_releases.push_back(entry);
                while g.recent_forced_releases.len() > FORCED_RELEASE_HISTORY {
                    g.recent_forced_releases.pop_front();
                }
                stale.push((job_id, age, allowance));
            }
        }
        for (job_id, age, allowance) in &stale {
            let name = by_id
                .get(job_id.as_str())
                .map(|j| j.name.as_str())
                .unwrap_or(job_id);
            log::warn!(
                "cron.inflight.forced_release job='{name}' id={job_id} age={:.0}s allowance={:.0}s — stale claim released",
                age, allowance
            );
            let path = store.cron_dir.join("inflight_forced_releases.jsonl");
            let _ = std::fs::create_dir_all(&store.cron_dir);
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                use std::io::Write;
                let entry = ForcedReleaseEntry {
                    job_id: job_id.clone(),
                    name: name.to_string(),
                    age_seconds: (*age * 10.0).round() / 10.0,
                    allowance_seconds: (*allowance * 10.0).round() / 10.0,
                    at: Utc::now().to_rfc3339(),
                };
                if let Ok(line) = serde_json::to_string(&entry) {
                    let _ = writeln!(f, "{line}");
                }
            }
        }
        stale.into_iter().map(|(id, _, _)| id).collect()
    }
}

// ---------------------------------------------------------------------------
// Helpers — stole from hermes/jobs.rs:648-677
// ---------------------------------------------------------------------------

fn job_interval_minutes(job: &CronJob) -> Option<f64> {
    let sched = crate::schedule::parse_schedule(&job.schedule).ok()?;
    match sched {
        crate::schedule::Schedule::Interval(d) => Some(d.as_secs_f64() / 60.0),
        crate::schedule::Schedule::Cron(s) => {
            // gap between next two fires
            let base = Utc::now();
            let mut it = s.after(&base);
            let first = it.next()?;
            let second = it.next()?;
            let gap = (second - first).num_seconds() as f64 / 60.0;
            if gap > 0.0 { Some(gap) } else { None }
        }
        crate::schedule::Schedule::Once(_) => None,
    }
}

fn compute_grace_seconds(job: &CronJob) -> f64 {
    const MIN_GRACE: f64 = 120.0;
    const MAX_GRACE: f64 = 7200.0;
    match job_interval_minutes(job) {
        Some(minutes) => ((minutes * 60.0) / 2.0).clamp(MIN_GRACE, MAX_GRACE),
        None => MIN_GRACE,
    }
}

// ---------------------------------------------------------------------------
// Scheduler — hermes/scheduler.rs:468-668 trimmed
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct Scheduler {
    pub store: CronStorePaths,
    pub inflight: InflightGuard,
}

impl Scheduler {
    pub fn new(store: CronStorePaths) -> Self {
        Self {
            store,
            inflight: InflightGuard::new(),
        }
    }

    pub fn from_active() -> Self {
        Self::new(CronStorePaths::active())
    }

    /// Scan due jobs — hermes Scheduler::scan_due_jobs core, gray-trimmed
    pub fn scan_due_jobs(&self) -> anyhow::Result<Vec<CronJob>> {
        let mut due = Vec::new();
        with_jobs_lock(&self.store, || -> anyhow::Result<()> {
            // Load raw via store inner (avoid double lock)
            let path = self.store.jobs_file.clone();
            let data = std::fs::read_to_string(&path).unwrap_or_default();
            let mut jobs: Vec<CronJob> = if data.trim().is_empty() {
                Vec::new()
            } else {
                // use store's peek but we already hold lock, so inline parse
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                    if let Some(arr) = v.get("jobs").and_then(|j| j.as_array()) {
                        arr.iter().filter_map(|x| serde_json::from_value(x.clone()).ok()).collect()
                    } else if let Some(arr) = v.as_array() {
                        arr.iter().filter_map(|x| serde_json::from_value(x.clone()).ok()).collect()
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                }
            };
            // Also try fallback bare Vec parse
            if jobs.is_empty() && !data.trim().is_empty() {
                if let Ok(v) = serde_json::from_str::<Vec<CronJob>>(&data) {
                    jobs = v;
                }
            }

            let now = Utc::now();
            let mut changed = false;

            for job in jobs.iter_mut() {
                if !job.enabled {
                    continue;
                }
                // Missing next_run recovery — hermes scheduler.rs:528
                if job.next_run.is_none() {
                    // For Once, if already run (last_run Some), don't recover
                    let is_once = crate::schedule::parse_schedule(&job.schedule)
                        .map(|s| s.is_once())
                        .unwrap_or(false);
                    if is_once && job.last_run.is_some() {
                        continue;
                    }
                    if let Some(recovered) = crate::schedule::compute_next_run(&job.schedule, now) {
                        log::info!("Job '{}' had no next_run; recovering to {}", job.name, recovered);
                        job.next_run = Some(recovered);
                        changed = true;
                    } else {
                        continue;
                    }
                }
                let next = match job.next_run {
                    Some(n) => n,
                    None => continue,
                };
                if next <= now {
                    // Stale recurring fast-forward but fire once — hermes scheduler.rs:564
                    let is_recurring = !crate::schedule::parse_schedule(&job.schedule)
                        .map(|s| s.is_once())
                        .unwrap_or(true);
                    if is_recurring {
                        let grace = compute_grace_seconds(job);
                        let age = (now - next).num_seconds() as f64;
                        if age > grace {
                            if let Some(new_next) = crate::schedule::compute_next_run(&job.schedule, now) {
                                log::info!(
                                    "Job '{}' missed schedule ({} grace {}s). Fire now, next -> {}",
                                    job.name,
                                    next,
                                    grace as i64,
                                    new_next
                                );
                                job.next_run = Some(new_next);
                                changed = true;
                            }
                        }
                    }
                    due.push(job.clone());
                }
            }

            if changed {
                // Persist repairs — use inner save to avoid double lock (we already hold it)
                let body = serde_json::to_string_pretty(&jobs)?;
                let tmp = tempfile::Builder::new()
                    .prefix(".jobs_")
                    .suffix(".tmp")
                    .tempfile_in(&self.store.cron_dir)?;
                {
                    use std::io::Write;
                    let mut f = tmp.as_file();
                    f.write_all(body.as_bytes())?;
                    f.flush()?;
                    f.sync_all()?;
                }
                tmp.persist(&self.store.jobs_file)?;
            }
            Ok(())
        })?;
        // Sweep stale inflight outside lock
        let _ = self.inflight.sweep_stale_inflight(&self.store, &due);
        Ok(due)
    }
}

