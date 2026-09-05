//! Scheduler — scan due jobs with grace, fast-forward, missing next_run recovery.
//!
//! NOTE: an earlier revision carried an `InflightGuard` (Pending/Live slots +
//! stale-claim sweep). Every caller runs due jobs inline in a single loop, so a
//! claim was always released before the next scan — the guard could never
//! dedup. Deleted; if concurrent dispatch ever arrives, add it back then.

use chrono::Utc;

use crate::store::{CronJob, CronStorePaths, with_jobs_lock};

// ---------------------------------------------------------------------------
// Helpers
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
// Scheduler
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct Scheduler {
    pub store: CronStorePaths,
}

impl Scheduler {
    pub fn new(store: CronStorePaths) -> Self {
        Self { store }
    }

    pub fn from_active() -> Self {
        Self::new(CronStorePaths::active())
    }

    /// Scan due jobs. Load/parse and repair-persist go through `store` so the
    /// envelope/bare-array/map compat lives in exactly one place.
    pub fn scan_due_jobs(&self) -> anyhow::Result<Vec<CronJob>> {
        let mut due = Vec::new();
        with_jobs_lock(&self.store, || -> anyhow::Result<()> {
            let mut jobs = crate::store::load_jobs_inner(&self.store);

            let now = Utc::now();
            let mut changed = false;

            for job in jobs.iter_mut() {
                if !job.enabled {
                    continue;
                }
                // Missing next_run recovery
                if job.next_run.is_none() {
                    // For Once, if already run (last_run Some), don't recover
                    let is_once = crate::schedule::parse_schedule(&job.schedule)
                        .map(|s| s.is_once())
                        .unwrap_or(false);
                    if is_once && job.last_run.is_some() {
                        continue;
                    }
                    if let Some(recovered) = crate::schedule::compute_next_run(&job.schedule, now) {
                        log::info!(
                            "Job '{}' had no next_run; recovering to {}",
                            job.name,
                            recovered
                        );
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
                    // Stale recurring fast-forward but fire once
                    let is_recurring = !crate::schedule::parse_schedule(&job.schedule)
                        .map(|s| s.is_once())
                        .unwrap_or(true);
                    if is_recurring {
                        let grace = compute_grace_seconds(job);
                        let age = (now - next).num_seconds() as f64;
                        if age > grace
                            && let Some(new_next) =
                                crate::schedule::compute_next_run(&job.schedule, now)
                        {
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
                    due.push(job.clone());
                }
            }

            if changed {
                // Persist repairs — inner save avoids double lock (we already hold it)
                crate::store::save_jobs_inner(&self.store, jobs, &[], false)?;
            }
            Ok(())
        })?;
        Ok(due)
    }
}
