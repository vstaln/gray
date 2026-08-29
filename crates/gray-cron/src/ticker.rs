use chrono::Utc;

/// Check which jobs are due (next_run <= now and enabled).
pub fn due_jobs(jobs: &[crate::store::CronJob]) -> Vec<crate::store::CronJob> {
    let now = Utc::now();
    jobs.iter()
        .filter(|j| j.enabled && j.next_run.map(|t| t <= now).unwrap_or(false))
        .cloned()
        .collect()
}

/// Simple ticker loop — call from a background task.
/// Sleeps 60s between checks, invokes `on_due` for each due job.
pub async fn run_ticker<F, Fut>(mut on_due: F)
where
    F: FnMut(crate::store::CronJob) -> Fut + Send,
    Fut: std::future::Future<Output = ()> + Send,
{
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
    loop {
        interval.tick().await;
        let jobs = crate::store::list_jobs();
        for job in due_jobs(&jobs) {
            let now = Utc::now();
            let _ = crate::store::update_job_run(&job.id, now);
            on_due(job).await;
        }
    }
}
