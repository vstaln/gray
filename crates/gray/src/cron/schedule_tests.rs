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
fn interval_below_resolution_has_no_next_run() {
    assert_eq!(next_run(0, &Schedule::Interval { secs: 0 }), None);
    assert_eq!(next_run(1, &Schedule::Interval { secs: 59 }), None);
    assert_eq!(next_run(1, &Schedule::Interval { secs: 60 }), Some(61));
}
