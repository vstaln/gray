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
    assert!(format_cron_dashboard(&[], None, 1_700_000_000).contains("no cron jobs"));
}

#[test]
fn dashboard_row_shapes() {
    let out = format_cron_dashboard(
        &[
            job("hourly", Some(gray_cron::RunStatus::Ok)),
            job("nightly", Some(gray_cron::RunStatus::Error)),
        ],
        None,
        1_700_000_000,
    );
    assert!(out.contains("hourly"));
    assert!(out.contains("abc123de"));
    assert!(out.contains("Interval"));
    assert!(out.contains("1700000000"));
    assert!(out.contains("Ok"));
    assert!(out.contains("nightly"));
    assert!(out.contains("Error"));
    assert!(out.contains("fire automatically in this session"));
}

#[test]
fn dashboard_reports_ticker_liveness() {
    let health = gray_cron::CronHealth {
        last_tick: None,
        overdue: vec![],
    };
    let out = format_cron_dashboard(&[job("nightly", None)], Some(&health), 1_700_000_000);
    assert!(out.contains("nightly"));
    assert!(out.contains("no tick has ever run"), "{out}");
    assert!(!out.contains("fire automatically in this session"), "{out}");
}
