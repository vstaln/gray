// UNRUN (cargo test banned under X): run in TTY/CI.
use super::*;
use crate::cron::OverdueJob;

fn stamp(at: i64, kind: &str) -> TickStamp {
    TickStamp {
        at,
        pid: 1,
        kind: kind.to_string(),
    }
}

fn overdue(id: &str, next: i64) -> OverdueJob {
    OverdueJob {
        id: id.to_string(),
        name: id.to_string(),
        next_run_at: next,
    }
}

#[test]
fn ago_short_units() {
    assert_eq!(ago_short(0), "0s");
    assert_eq!(ago_short(59), "59s");
    assert_eq!(ago_short(60), "1m");
    assert_eq!(ago_short(3 * 3600), "3h");
    assert_eq!(ago_short(16 * 86_400), "16d");
    assert_eq!(
        ago_short(-5),
        "0s",
        "clock skew never prints a negative age"
    );
}

#[test]
fn live_ticker_says_so_and_names_the_driver() {
    let health = CronHealth {
        last_tick: Some(stamp(1000, "repl")),
        overdue: vec![],
    };
    let line = ticker_line(&health, 1012);
    assert!(line.contains("live"), "{line}");
    assert!(line.contains("12s ago"), "{line}");
    assert!(line.contains("(repl)"), "{line}");
    assert!(!line.contains("NOT fire"), "{line}");
}

#[test]
fn never_ticked_warns_loudly_with_the_drivers() {
    let health = CronHealth {
        last_tick: None,
        overdue: vec![],
    };
    let line = ticker_line(&health, 1000);
    assert!(line.contains("no tick has ever run"), "{line}");
    assert!(
        line.contains("gray cron serve") && line.contains("gray gateway install"),
        "{line}"
    );
}

#[test]
fn stale_ticker_warns_and_overdue_line_joins_it() {
    let health = CronHealth {
        last_tick: Some(stamp(1000, "serve")),
        overdue: vec![overdue("a", 1000), overdue("b", 900)],
    };
    let line = ticker_line(&health, 1000 + 14 * 86_400);
    assert!(line.contains("stale"), "{line}");
    assert!(line.contains("2 job(s) overdue"), "{line}");
    assert!(line.contains("worst 14d behind"), "{line}");
}

#[test]
fn add_warning_only_when_nothing_is_ticking() {
    let now = 10_000;
    assert!(add_warning(Some(&stamp(now - 5, "repl")), now).is_none());
    let stale = add_warning(Some(&stamp(now - 3600, "cli")), now).expect("stale warns");
    assert!(stale.contains("1h ago"), "{stale}");
    let never = add_warning(None, now).expect("never warns");
    assert!(never.contains("will not fire"), "{never}");
}
