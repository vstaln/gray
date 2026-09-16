//! Read-only `/cron` dashboard (interactive management deferred).

/// One line per job: name, id-prefix, schedule, next run, last status.
/// Ends with the ticker's liveness (has anything driven this store?) instead
/// of a bare promise that due jobs fire. Pure: the store stays in dispatch,
/// so the caller hands in the health snapshot it already read.
pub(crate) fn format_cron_dashboard(
    jobs: &[gray_cron::CronJob],
    health: Option<&gray_cron::CronHealth>,
    now: i64,
) -> String {
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
    match health {
        Some(h) => out.push_str(&crate::cron_status::ticker_line(h, now)),
        None => out.push_str("due jobs fire automatically in this session"),
    }
    out
}

#[path = "cron_tests.rs"]
#[cfg(test)]
mod tests;
