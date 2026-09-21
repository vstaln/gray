// UNRUN (cargo test banned under X): run in TTY/CI.
use super::*;

fn job(name: &str, status: Option<crate::cron::RunStatus>) -> crate::cron::CronJob {
    crate::cron::CronJob {
        id: "abc123def456".to_string(),
        name: name.to_string(),
        prompt: "p".to_string(),
        schedule: crate::cron::Schedule::Interval { secs: 3600 },
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
            job("hourly", Some(crate::cron::RunStatus::Ok)),
            job("nightly", Some(crate::cron::RunStatus::Error)),
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
    let health = crate::cron::CronHealth {
        last_tick: None,
        overdue: vec![],
    };
    let out = format_cron_dashboard(&[job("nightly", None)], Some(&health), 1_700_000_000);
    assert!(out.contains("nightly"));
    assert!(out.contains("no tick has ever run"), "{out}");
    assert!(!out.contains("fire automatically in this session"), "{out}");
}

fn paused(name: &str) -> crate::cron::CronJob {
    crate::cron::CronJob {
        state: crate::cron::store::JobState::Paused,
        ..job(name, None)
    }
}

#[test]
fn picker_rows_carry_the_dashboard_fields() {
    let rows = items(
        &[job("hourly", Some(crate::cron::RunStatus::Ok))],
        None,
        1_700_000_000,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "abc123def456");
    assert!(
        rows[0].row.starts_with("\u{2713} hourly (abc123de)"),
        "{}",
        rows[0].row
    );
    assert!(rows[0].row.contains("next 1700000000"), "{}", rows[0].row);
    assert!(rows[0].row.contains("last Ok"), "{}", rows[0].row);
    assert!(rows[0].enabled);
    assert!(!rows[0].read_only);
}

#[test]
fn picker_marks_paused_jobs_dim_and_unticked() {
    let rows = items(&[paused("nightly")], None, 1_700_000_000);
    assert!(rows[0].row.starts_with("\u{25cb}"), "{}", rows[0].row);
    assert!(rows[0].row.ends_with("[paused]"), "{}", rows[0].row);
    assert!(!rows[0].lit);
    assert!(!rows[0].enabled);
}

#[test]
fn picker_appends_the_ticker_liveness_row() {
    let health = crate::cron::CronHealth {
        last_tick: None,
        overdue: vec![],
    };
    let rows = items(&[job("nightly", None)], Some(&health), 1_700_000_000);
    assert_eq!(rows.len(), 2);
    assert!(
        rows[1].row.contains("no tick has ever run"),
        "{}",
        rows[1].row
    );
    assert!(rows[1].read_only);
}

#[test]
fn cron_spec_toggles_without_removal_or_errors() {
    assert_eq!(CRON_SPEC.title, "Cron");
    const {
        assert!(CRON_SPEC.supports_toggle);
    }
    const {
        assert!(!CRON_SPEC.supports_remove);
    }
    const {
        assert!(!CRON_SPEC.errors_tab);
    }
}
