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

/// Map a config deliver string to its `Deliver` variant. `"local"`/`""` mean
/// stay local; `"origin"` replies in the originating chat; anything else names
/// an explicit target.
fn deliver_from_str(s: &str) -> gray_cron::Deliver {
    match s.trim().to_ascii_lowercase().as_str() {
        "" | "local" => gray_cron::Deliver::Local,
        "origin" => gray_cron::Deliver::Origin,
        _ => gray_cron::Deliver::Target(s.to_string()),
    }
}

pub fn sync_job(cfg: &HeartbeatConfig, goal: &str) -> anyhow::Result<String> {
    let store = gray_cron::CronStore::open(cron_dir()?).context("open cron store")?;
    store.remove(JOB_NAME)?;
    if !cfg.enabled {
        return Ok(String::new());
    }
    let id = store.add_full_unguarded(
        JOB_NAME,
        &cfg.schedule,
        &render_prompt(goal),
        deliver_from_str(&cfg.deliver),
        None,
        None,
    )?;
    Ok(id)
}

/// Apply `on` overrides, persist the config, and (re)create the cron job.
/// Returns the created job id (empty when the job could not be created).
pub fn enable(
    cfg: &mut HeartbeatConfig,
    schedule: Option<String>,
    deliver: Option<String>,
) -> anyhow::Result<String> {
    if let Some(s) = schedule {
        cfg.schedule = s;
    }
    if let Some(d) = deliver {
        cfg.deliver = d;
    }
    cfg.enabled = true;
    // Sync first: a failed sync must not persist `enabled: true`, or `off`
    // could silently leave the job firing.
    let id = sync_job(cfg, &crate::goal::read_goal()?)?;
    crate::config::save_config(cfg)?;
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
    fn enable_applies_overrides_and_persists() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("GRAY_HOME", dir.path()) };
        let mut cfg = HeartbeatConfig::default();
        enable(&mut cfg, Some("every 1h".into()), Some("telegram:9".into())).unwrap();
        let saved = crate::config::load_config().unwrap();
        assert!(saved.enabled);
        assert_eq!(saved.schedule, "every 1h");
        assert_eq!(saved.deliver, "telegram:9");
    }

    #[test]
    fn sync_maps_local_deliver_to_local_variant() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("GRAY_HOME", dir.path()) };
        let cfg = HeartbeatConfig {
            enabled: true,
            schedule: "every 30m".into(),
            deliver: "local".into(),
        };
        sync_job(&cfg, "g").unwrap();
        let store = gray_cron::CronStore::open(cron_dir().unwrap()).unwrap();
        let job = store.get(JOB_NAME).unwrap().unwrap();
        assert_eq!(job.deliver, gray_cron::Deliver::Local);
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
