use crate::config::HeartbeatConfig;
use anyhow::Context;

pub const JOB_NAME: &str = "heartbeat";

pub fn cron_dir() -> anyhow::Result<std::path::PathBuf> {
    Ok(crate::gray_home()?.join("cron"))
}

pub fn render_prompt(goal: &str) -> String {
    format!(
        "Heartbeat wake-up. Your standing goal:\n\n{goal}\n\n\
You woke on your own; no user message triggered this. Take the next useful step \
toward the goal using your tools, then reply for the user.\n\
If there is nothing to do, nothing changed, or nothing worth reporting, reply \
with exactly [SILENT] and nothing else."
    )
}

pub fn sync_job(cfg: &HeartbeatConfig, goal: &str) -> anyhow::Result<String> {
    let store = gray_cron::CronStore::open(cron_dir()?).context("open cron store")?;
    let _ = store.remove(JOB_NAME);
    if !cfg.enabled {
        return Ok(String::new());
    }
    let id = store.add_full(
        JOB_NAME,
        &cfg.schedule,
        &render_prompt(goal),
        gray_cron::Deliver::Target(cfg.deliver.clone()),
        None,
        None,
    )?;
    Ok(id)
}

pub enum JobStatus {
    Disabled,
    Missing,
    Live { next_run_at: Option<i64> },
}

pub fn job_status(cfg: &HeartbeatConfig) -> anyhow::Result<JobStatus> {
    if !cfg.enabled {
        return Ok(JobStatus::Disabled);
    }
    let store = gray_cron::CronStore::open(cron_dir()?)?;
    match store.get(JOB_NAME)? {
        Some(j) => Ok(JobStatus::Live {
            next_run_at: j.next_run_at,
        }),
        None => Ok(JobStatus::Missing),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ENV_LOCK;
    use crate::config::HeartbeatConfig;

    #[test]
    fn render_prompt_embeds_goal_and_silence_token() {
        let p = render_prompt("Ship it.");
        assert!(p.contains("Ship it."), "{p}");
        assert!(p.contains("[SILENT]"), "{p}");
    }

    #[test]
    fn sync_adds_updates_and_removes_job() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("GRAY_HOME", dir.path()) };
        let mut cfg = HeartbeatConfig {
            enabled: true,
            schedule: "every 30m".into(),
            deliver: "local".into(),
        };
        let id = sync_job(&cfg, "goal one").unwrap();
        assert!(!id.is_empty());
        let store = gray_cron::CronStore::open(cron_dir().unwrap()).unwrap();
        assert!(
            store
                .get(JOB_NAME)
                .unwrap()
                .unwrap()
                .prompt
                .contains("goal one")
        );

        // Re-sync after a goal edit replaces the job (no duplicates).
        sync_job(&cfg, "goal two").unwrap();
        let jobs = store.list().unwrap();
        assert_eq!(jobs.iter().filter(|j| j.name == JOB_NAME).count(), 1);
        assert!(jobs[0].prompt.contains("goal two"));

        cfg.enabled = false;
        assert_eq!(sync_job(&cfg, "goal two").unwrap(), "");
        assert!(store.get(JOB_NAME).unwrap().is_none());
    }

    #[test]
    fn status_reports_disabled_missing_and_live() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("GRAY_HOME", dir.path()) };
        let cfg = HeartbeatConfig::default();
        assert!(matches!(job_status(&cfg).unwrap(), JobStatus::Disabled));
        let cfg = HeartbeatConfig {
            enabled: true,
            schedule: "every 30m".into(),
            deliver: "local".into(),
        };
        assert!(matches!(job_status(&cfg).unwrap(), JobStatus::Missing));
        sync_job(&cfg, "g").unwrap();
        assert!(matches!(job_status(&cfg).unwrap(), JobStatus::Live { .. }));
    }
}
