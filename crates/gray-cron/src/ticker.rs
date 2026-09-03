use chrono::Utc;
use crate::scheduler::{InflightGuard, Scheduler};
use std::sync::Arc;

/// Simple ticker loop — hermes-style with InflightGuard dedup and grace.
/// Uses Scheduler::scan_due_jobs under the hood.
pub async fn run_ticker<F, Fut>(mut on_due: F)
where
    F: FnMut(crate::store::CronJob) -> Fut + Send,
    Fut: std::future::Future<Output = ()> + Send,
{
    let scheduler = Scheduler::from_active();
    let guard = Arc::new(InflightGuard::new());
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
    loop {
        interval.tick().await;
        let due = match scheduler.scan_due_jobs() {
            Ok(d) => d,
            Err(e) => {
                log::warn!("cron scan failed: {e}");
                continue;
            }
        };
        for job in due {
            if !guard.try_register_running_job(&job.id) {
                continue;
            }
            let guard_clone = guard.clone();
            let job_clone = job.clone();
            // Update next_run before dispatch (hermes fast-forward already persisted)
            let now = Utc::now();
            let _ = crate::store::update_job_run(&job.id, now);
            // Install live probe
            let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flag2 = flag.clone();
            guard.install_dispatch_probe(&job.id, Arc::new(move || flag2.load(std::sync::atomic::Ordering::SeqCst)));
            on_due(job_clone).await;
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            guard_clone.release_running_job(&job.id);
        }
    }
}

/// One-shot scan helper for `gray cron run` headless mode — returns due jobs via Scheduler.
pub fn scan_due() -> Vec<crate::store::CronJob> {
    Scheduler::from_active()
        .scan_due_jobs()
        .unwrap_or_default()
}
