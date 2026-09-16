// UNRUN (cargo test banned under X): run in TTY/CI.
use super::*;

#[test]
fn parse_all_kinds() {
    assert!(matches!(
        parse_schedule("in 10m").unwrap(),
        Schedule::Once { .. }
    ));
    assert!(matches!(
        parse_schedule("every 1h").unwrap(),
        Schedule::Interval { secs: 3600 }
    ));
    assert!(matches!(
        parse_schedule("30m").unwrap(),
        Schedule::Interval { secs: 1800 }
    ));
    assert!(matches!(
        parse_schedule("0 9 * * *").unwrap(),
        Schedule::Cron { .. }
    ));
    assert!(
        parse_schedule("every 30s").is_err(),
        "below ticker resolution"
    );
}

#[test]
fn catchup_window_math() {
    assert_eq!(catchup_grace_secs(3600), 1800); // half period
    assert_eq!(catchup_grace_secs(60), 120); // clamped floor
    assert_eq!(catchup_grace_secs(86400 * 30), 7200); // clamped ceiling
}

#[test]
fn interval_below_resolution_has_no_next_run() {
    assert_eq!(next_run(0, &Schedule::Interval { secs: 0 }), None);
    assert_eq!(next_run(1, &Schedule::Interval { secs: 59 }), None);
    assert_eq!(next_run(1, &Schedule::Interval { secs: 60 }), Some(61));
}
