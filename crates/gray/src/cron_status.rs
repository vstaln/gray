//! Human-facing cron liveness lines (pure: no store, no I/O).
//!
//! Why this module exists: a job store can be perfectly consistent while
//! nothing ever drives it. `next=<timestamp>` then prints on schedule and no
//! job ever fires — silent exactly where a user is looking. Every surface
//! that reports on cron (`cron list`, `cron add`, the `/cron` dashboard)
//! renders through here instead of implying a schedule is armed when no
//! ticker runs.

use crate::cron::{CronHealth, TICKER_STALE_SECS, TickStamp};

/// Driver recipe shared by every warning, so the fix is one copy-paste.
const DRIVERS: &str = "install the gateway (`gray gateway install`), or run `gray cron serve`, `gray cron tick`, or a REPL";

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

#[path = "cron_status_tests.rs"]
#[cfg(test)]
mod tests;
