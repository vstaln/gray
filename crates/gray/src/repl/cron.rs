//! Read-only `/cron` dashboard (interactive management deferred).

/// One line per job: name, id-prefix, schedule, next run, last status.
/// Ends with the tick hint. Pure: the store stays in dispatch.
pub(crate) fn format_cron_dashboard(jobs: &[gray_cron::CronJob]) -> String {
    if jobs.is_empty() {
        return "no cron jobs — `gray cron add \"every 1h\" \"prompt\"` to create one".to_string();
    }
    let mut out = String::new();
    for j in jobs {
        out.push_str(&format!(
            "• {} ({}) — {:?} — next {} — last {}\n",
            j.name,
            &j.id[..8.min(j.id.len())],
            j.schedule,
            j.next_run_at
                .map(|t| t.to_string())
                .as_deref()
                .unwrap_or("-"),
            j.last_status
                .map(|s| format!("{s:?}"))
                .as_deref()
                .unwrap_or("-"),
        ));
    }
    out.push_str("due jobs fire automatically in this session");
    out
}

#[cfg(test)]
mod tests {
    // UNRUN (cargo test banned under X): run in TTY/CI.
    use super::*;

    fn job(name: &str, status: Option<gray_cron::RunStatus>) -> gray_cron::CronJob {
        gray_cron::CronJob {
            id: "abc123def456".to_string(),
            name: name.to_string(),
            prompt: "p".to_string(),
            schedule: gray_cron::Schedule::Interval { secs: 3600 },
            enabled: true,
            state: Default::default(),
            created_at: 1,
            next_run_at: Some(1_700_000_000),
            last_run_at: None,
            last_status: status,
            last_error: None,
            last_delivery_error: None,
            deliver: Default::default(),
            origin: None,
            workdir: None,
            fire_claim: None,
            skills: vec![],
            script: None,
        }
    }

    #[test]
    fn dashboard_empty() {
        assert!(format_cron_dashboard(&[]).contains("no cron jobs"));
    }

    #[test]
    fn dashboard_row_shapes() {
        let out = format_cron_dashboard(&[
            job("hourly", Some(gray_cron::RunStatus::Ok)),
            job("nightly", Some(gray_cron::RunStatus::Error)),
        ]);
        assert!(out.contains("hourly"));
        assert!(out.contains("abc123de"));
        assert!(out.contains("Interval"));
        assert!(out.contains("1700000000"));
        assert!(out.contains("Ok"));
        assert!(out.contains("nightly"));
        assert!(out.contains("Error"));
        assert!(out.contains("fire automatically in this session"));
    }
}
