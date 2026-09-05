use chrono::Utc;
use crate::scheduler::Scheduler;

/// Simple ticker loop with grace + fast-forward via Scheduler::scan_due_jobs.
/// Due jobs run inline, sequentially — no concurrency, so no dedup guard.
pub async fn run_ticker<F, Fut>(mut on_due: F)
where
    F: FnMut(crate::store::CronJob) -> Fut + Send,
    Fut: std::future::Future<Output = ()> + Send,
{
    let scheduler = Scheduler::from_active();
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
            // Update next_run before dispatch (fast-forward already persisted)
            let now = Utc::now();
            let _ = crate::store::update_job_run(&job.id, now);
            on_due(job).await;
        }
    }
}

/// One-shot scan helper for `gray cron run` headless mode — returns due jobs via Scheduler.
pub fn scan_due() -> Vec<crate::store::CronJob> {
    Scheduler::from_active()
        .scan_due_jobs()
        .unwrap_or_default()
}
