use crate::pulse::config::PulseConfig;
use anyhow::Context;
use gray_gateway::config::gray_home_dir;
use std::path::{Path, PathBuf};

pub const JOB_NAME: &str = "pulse";

pub(crate) fn cron_dir_at(home: &Path) -> PathBuf {
    home.join("cron")
}

pub fn cron_dir() -> anyhow::Result<PathBuf> {
    Ok(cron_dir_at(&gray_home_dir()?))
}

pub fn render_prompt(goal: &str) -> String {
    format!(
        "Pulse wake-up. Your standing goal:\n\n{goal}\n\n\
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

pub(crate) fn sync_job_at(
    cfg: &PulseConfig,
    goal: &str,
    cron_dir: &Path,
) -> anyhow::Result<String> {
    let store = gray_cron::CronStore::open(cron_dir).context("open cron store")?;
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

pub fn sync_job(cfg: &PulseConfig, goal: &str) -> anyhow::Result<String> {
    sync_job_at(cfg, goal, &cron_dir()?)
}

/// Apply `on` overrides, persist the config, and (re)create the cron job.
/// Returns the created job id (empty when the job could not be created).
pub(crate) fn enable_at(
    cfg: &mut PulseConfig,
    schedule: Option<String>,
    deliver: Option<String>,
    home: &Path,
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
    let goal = crate::pulse::goal::read_goal_at(&crate::pulse::goal::goal_path_at(home))?;
    let id = sync_job_at(cfg, &goal, &cron_dir_at(home))?;
    crate::pulse::config::save_config_at(&crate::pulse::config::config_path_at(home), cfg)?;
    Ok(id)
}

pub fn enable(
    cfg: &mut PulseConfig,
    schedule: Option<String>,
    deliver: Option<String>,
) -> anyhow::Result<String> {
    enable_at(cfg, schedule, deliver, &gray_home_dir()?)
}

pub enum JobStatus {
    Disabled,
    Missing,
    Live { next_run_at: Option<i64> },
}

pub(crate) fn job_status_at(cfg: &PulseConfig, cron_dir: &Path) -> anyhow::Result<JobStatus> {
    if !cfg.enabled {
        return Ok(JobStatus::Disabled);
    }
    let store = gray_cron::CronStore::open(cron_dir)?;
    match store.get(JOB_NAME)? {
        Some(j) => Ok(JobStatus::Live {
            next_run_at: j.next_run_at,
        }),
        None => Ok(JobStatus::Missing),
    }
}

pub fn job_status(cfg: &PulseConfig) -> anyhow::Result<JobStatus> {
    job_status_at(cfg, &cron_dir()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pulse::config::PulseConfig;

    #[test]
    fn render_prompt_embeds_goal_and_silence_token() {
        let p = render_prompt("Ship it.");
        assert!(p.contains("Ship it."), "{p}");
        assert!(p.contains("[SILENT]"), "{p}");
    }

    #[test]
    fn sync_adds_updates_and_removes_job() {
        let dir = tempfile::tempdir().unwrap();
        let cron = cron_dir_at(dir.path());
        let mut cfg = PulseConfig {
            enabled: true,
            schedule: "every 30m".into(),
            deliver: "local".into(),
        };
        let id = sync_job_at(&cfg, "goal one", &cron).unwrap();
        assert!(!id.is_empty());
        let store = gray_cron::CronStore::open(&cron).unwrap();
        assert!(
            store
                .get(JOB_NAME)
                .unwrap()
                .unwrap()
                .prompt
                .contains("goal one")
        );

        // Re-sync after a goal edit replaces the job (no duplicates).
        sync_job_at(&cfg, "goal two", &cron).unwrap();
        let jobs = store.list().unwrap();
        assert_eq!(jobs.iter().filter(|j| j.name == JOB_NAME).count(), 1);
        assert!(jobs[0].prompt.contains("goal two"));

        cfg.enabled = false;
        assert_eq!(sync_job_at(&cfg, "goal two", &cron).unwrap(), "");
        assert!(store.get(JOB_NAME).unwrap().is_none());
    }

    #[test]
    fn enable_applies_overrides_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = PulseConfig::default();
        enable_at(
            &mut cfg,
            Some("every 1h".into()),
            Some("telegram:9".into()),
            dir.path(),
        )
        .unwrap();
        let saved =
            crate::pulse::config::load_config_at(&crate::pulse::config::config_path_at(dir.path()))
                .unwrap();
        assert!(saved.enabled);
        assert_eq!(saved.schedule, "every 1h");
        assert_eq!(saved.deliver, "telegram:9");
    }

    #[test]
    fn sync_maps_local_deliver_to_local_variant() {
        let dir = tempfile::tempdir().unwrap();
        let cron = cron_dir_at(dir.path());
        let cfg = PulseConfig {
            enabled: true,
            schedule: "every 30m".into(),
            deliver: "local".into(),
        };
        sync_job_at(&cfg, "g", &cron).unwrap();
        let store = gray_cron::CronStore::open(&cron).unwrap();
        let job = store.get(JOB_NAME).unwrap().unwrap();
        assert_eq!(job.deliver, gray_cron::Deliver::Local);
    }

    #[test]
    fn status_reports_disabled_missing_and_live() {
        let dir = tempfile::tempdir().unwrap();
        let cron = cron_dir_at(dir.path());
        let cfg = PulseConfig::default();
        assert!(matches!(
            job_status_at(&cfg, &cron).unwrap(),
            JobStatus::Disabled
        ));
        let cfg = PulseConfig {
            enabled: true,
            schedule: "every 30m".into(),
            deliver: "local".into(),
        };
        assert!(matches!(
            job_status_at(&cfg, &cron).unwrap(),
            JobStatus::Missing
        ));
        sync_job_at(&cfg, "g", &cron).unwrap();
        assert!(matches!(
            job_status_at(&cfg, &cron).unwrap(),
            JobStatus::Live { .. }
        ));
    }
}
