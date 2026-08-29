use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub schedule: String,
    pub prompt: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub last_run: Option<DateTime<Utc>>,
    pub next_run: Option<DateTime<Utc>>,
}

impl CronJob {
    pub fn new(name: String, schedule: String, prompt: String) -> Self {
        let now = Utc::now();
        let next_run = crate::schedule::compute_next_run(&schedule, now);
        Self {
            id: uuid::Uuid::new_v4().to_string()[..8].to_string(),
            name,
            schedule,
            prompt,
            enabled: true,
            created_at: now,
            last_run: None,
            next_run,
        }
    }
}

fn cron_home() -> PathBuf {
    std::env::var("GRAY_HOME")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".gray"))
                .unwrap_or_else(|_| PathBuf::from(".gray"))
        })
}

fn jobs_path() -> PathBuf {
    cron_home().join("cron").join("jobs.json")
}

pub fn load_jobs() -> Vec<CronJob> {
    let path = jobs_path();
    let data = std::fs::read_to_string(&path).unwrap_or_default();
    if data.trim().is_empty() {
        return Vec::new();
    }
    serde_json::from_str(&data).unwrap_or_default()
}

pub fn save_jobs(jobs: &[CronJob]) -> anyhow::Result<()> {
    let path = jobs_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Simple atomic write via temp file
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(jobs)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn list_jobs() -> Vec<CronJob> {
    let mut jobs = load_jobs();
    jobs.sort_by_key(|j| j.next_run);
    jobs
}

pub fn create_job(name: String, schedule: String, prompt: String) -> anyhow::Result<CronJob> {
    // Validate schedule
    crate::schedule::parse_schedule(&schedule)?;
    let mut jobs = load_jobs();
    let job = CronJob::new(name, schedule, prompt);
    jobs.push(job.clone());
    save_jobs(&jobs)?;
    Ok(job)
}

pub fn remove_job(id: &str) -> anyhow::Result<bool> {
    let mut jobs = load_jobs();
    let before = jobs.len();
    jobs.retain(|j| j.id != id && j.name != id);
    if jobs.len() == before {
        return Ok(false);
    }
    save_jobs(&jobs)?;
    Ok(true)
}

pub fn find_job(id: &str) -> Option<CronJob> {
    load_jobs().into_iter().find(|j| j.id == id || j.name == id)
}

pub fn update_job_run(id: &str, now: DateTime<Utc>) -> anyhow::Result<()> {
    let mut jobs = load_jobs();
    if let Some(job) = jobs.iter_mut().find(|j| j.id == id) {
        job.last_run = Some(now);
        job.next_run = crate::schedule::compute_next_run(&job.schedule, now);
        save_jobs(&jobs)?;
    }
    Ok(())
}
