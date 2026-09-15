//! Human-facing cron liveness lines (pure: no store, no I/O).
//!
//! Why this module exists: a job store can be perfectly consistent while
//! nothing ever drives it. `next=<timestamp>` then prints on schedule and no
//! job ever fires — silent exactly where a user is looking. Every surface
//! that reports on cron (`cron list`, `cron add`, the `/cron` dashboard)
//! renders through here instead of implying a schedule is armed when no
//! ticker runs.

use gray_cron::{CronHealth, TICKER_STALE_SECS, TickStamp};

/// Driver recipe shared by every warning, so the fix is one copy-paste.
const DRIVERS: &str = "run `gray cron serve`, host `gray cron tick` (cron/runit), or keep a REPL open";

/// Compact age: `42s`, `9m`, `3h`, `16d`.
pub fn ago_short(secs: i64) -> String {
    let s = secs.max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86_400)
    }
}

/// One line answering "is anything ticking this store?", plus an overdue
/// line when jobs should have fired and did not.
pub fn ticker_line(health: &CronHealth, now: i64) -> String {
    let mut out = match &health.last_tick {
        Some(t) if health.ticker_live(now) => format!(
            "ticker: live — last tick {} ago ({})",
            ago_short(now.saturating_sub(t.at)),
            t.kind
        ),
        Some(t) => format!(
            "⚠ ticker: stale — last tick {} ago ({}) — jobs will NOT fire; {DRIVERS}",
            ago_short(now.saturating_sub(t.at)),
            t.kind
        ),
        None => format!("⚠ ticker: no tick has ever run — jobs will NOT fire; {DRIVERS}"),
    };
    if let Some(worst) = health
        .overdue
        .iter()
        .map(|j| now.saturating_sub(j.next_run_at))
        .max()
    {
        // Terse: the ticker line above already names the drivers whenever
        // the missing driver is what made these jobs late.
        out.push_str(&format!(
            "\n⚠ {} job(s) overdue (worst {} behind) — not firing",
            health.overdue.len(),
            ago_short(worst)
        ));
    }
    out
}

/// Warning for `cron add`: the job is stored, but will anything ever fire it?
/// `None` when a ticker is live (a REPL session or `serve` is running).
pub fn add_warning(last_tick: Option<&TickStamp>, now: i64) -> Option<String> {
    let detail = match last_tick {
        None => "no cron ticker has ever run".to_string(),
        Some(t) => {
            let age = now.saturating_sub(t.at);
            if age <= TICKER_STALE_SECS {
                return None;
            }
            format!("last cron tick was {} ago", ago_short(age))
        }
    };
    Some(format!(
        "⚠ {detail} — this job will not fire until one does; {DRIVERS}"
    ))
}

#[cfg(test)]
mod tests {
    // UNRUN (cargo test banned under X): run in TTY/CI.
    use super::*;
    use gray_cron::OverdueJob;

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
        assert_eq!(ago_short(-5), "0s", "clock skew never prints a negative age");
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
            line.contains("gray cron serve") && line.contains("cron/runit"),
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
}
