//! `/cron`: the read-only dashboard (headless) and the job picker (TTY),
//! both rendered from one row builder.

use std::path::Path;

use crate::setup::{ManagerItem, ManagerSpec, run_install_manager};

/// One line per job: name, id-prefix, schedule, next run, last status.
/// Ends with the ticker's liveness (has anything driven this store?) instead
/// of a bare promise that due jobs fire. Pure: the store stays in dispatch,
/// so the caller hands in the health snapshot it already read.
pub(crate) fn format_cron_dashboard(
    jobs: &[crate::cron::CronJob],
    health: Option<&crate::cron::CronHealth>,
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

const CRON_SPEC: ManagerSpec = ManagerSpec {
    title: "Cron",
    empty_hint: "no cron jobs \u{2014} gray cron add \"every 1h\" \"prompt\"",
    error_verb: "toggle failed",
    supports_toggle: true,
    supports_remove: false,
    errors_tab: false,
    keep_stale_on_relist_error: false,
};

/// One picker row per job: the dashboard's fields in the manager's shape,
/// plus the ticker's liveness line (the answer to "is anything driving this
/// store?") as a trailing read-only row.
///
/// A row carries a switch only when flipping it means something: `claim_due`
/// fires a job only when it is `Active` *and* `enabled`, so a paused job
/// toggles, while a disabled job (`enabled: false`) and a finished one-shot
/// (`Done`) render read-only and tagged — resuming either would change state
/// without changing whether the job runs.
pub(crate) fn items(
    jobs: &[crate::cron::CronJob],
    health: Option<&crate::cron::CronHealth>,
    now: i64,
) -> Vec<ManagerItem> {
    let mut out: Vec<ManagerItem> = jobs
        .iter()
        .map(|j| {
            let (glyph, tag, lit, toggleable) = match j.state {
                crate::cron::store::JobState::Active if j.enabled => ("\u{2713}", "", true, true),
                crate::cron::store::JobState::Active => ("\u{25cb}", " [disabled]", false, false),
                crate::cron::store::JobState::Paused => ("\u{25cb}", " [paused]", false, true),
                crate::cron::store::JobState::Done => ("\u{b7}", " [done]", false, false),
            };
            let next = j
                .next_run_at
                .map(|t| t.to_string())
                .unwrap_or_else(|| "-".to_string());
            let last = j
                .last_status
                .map(|s| format!("{s:?}"))
                .unwrap_or_else(|| "-".to_string());
            let row = format!(
                "{glyph} {} ({}) \u{2014} {:?} \u{2014} next {} \u{2014} last {}{tag}",
                j.name,
                &j.id[..8.min(j.id.len())],
                j.schedule,
                next,
                last,
            );
            ManagerItem {
                name: j.id.clone(),
                row,
                lit,
                enabled: toggleable && j.state == crate::cron::store::JobState::Active,
                read_only: !toggleable,
                needs_setup: false,
            }
        })
        .collect();
    // The ticker row only rides along when there are jobs to tick: on an empty
    // store it would crowd out the add-a-job hint.
    if !jobs.is_empty()
        && let Some(h) = health
    {
        out.push(ManagerItem {
            name: String::new(),
            row: crate::cron_status::ticker_line(h, now),
            lit: true,
            enabled: false,
            read_only: true,
            needs_setup: false,
        });
    }
    out
}

/// The job picker: `space` pauses/resumes via the store's own toggle, which
/// recomputes `next_run_at` on resume. Adding and removing stay on the CLI.
pub(crate) fn run_cron_modal(
    bg: Option<&crate::setup::BackgroundSnapshot>,
    home: &Path,
) -> anyhow::Result<bool> {
    run_install_manager(
        bg,
        None,
        &CRON_SPEC,
        || {
            let store = crate::cron::CronStore::open(home.join("cron")).ok()?;
            let jobs = store.list().ok()?;
            let now = crate::cron::now_secs();
            let health = store.health(now).ok();
            Some(items(&jobs, health.as_ref(), now))
        },
        |_| anyhow::bail!("remove jobs with gray cron remove <id>"),
        |id, on| {
            let store = crate::cron::CronStore::open(home.join("cron"))?;
            anyhow::ensure!(
                store.set_paused(id, !on)?,
                "unknown cron job {id:?} \u{2014} it may have been removed"
            );
            Ok(())
        },
    )
}

fn idle_ctx(cwd: &Path, sid: &str) -> gray_core::agent::ToolContext {
    gray_core::agent::ToolContext {
        cwd: cwd.to_path_buf(),
        cancel: tokio_util::sync::CancellationToken::new(),
        session_id: Some(sid.to_string()),
    }
}

/// What the idle REPL owes the model, as `(cards, prompt)`: cron deliveries
/// waiting in this session's inbox, then background-job notices nobody has
/// drained since the last turn. The cards paint first; the prompt (user-role,
/// persisted like a typed one) starts a turn so the model reacts in chat.
pub(super) fn idle_wake(
    agent: Option<&gray_core::agent::Agent>,
    sid: Option<&str>,
    cwd: &Path,
) -> Option<(Vec<String>, String)> {
    let sid = sid?;
    let mut cards = Vec::new();
    let mut prompts = Vec::new();
    if let Ok(home) = crate::setup::gray_home() {
        for (card, prompt) in crate::cron_serve::drain_session_inbox(&home, sid) {
            cards.push(card);
            prompts.push(prompt);
        }
    }
    if let Some(agent) = agent {
        for notice in agent.drain_background_notifications(&idle_ctx(cwd, sid)) {
            cards.push(format!(
                "\u{2699} {}",
                notice
                    .split_once(". ")
                    .map_or(notice.as_str(), |(head, _)| head)
            ));
            // Same framing the agent loop gives a notice it drains mid-run.
            prompts.push(format!("[Background task notification]\n{notice}"));
        }
    }
    (!prompts.is_empty()).then(|| (cards, prompts.join("\n\n")))
}

/// Arms the idle wake for background jobs: while the session has unfinished
/// jobs, a task waits for the first to settle and wakes the prompt. Re-armed
/// at every idle point (the caller aborts the previous one), so jobs started
/// during the last turn are covered.
pub(super) fn arm_background_wake(
    agent: Option<&gray_core::agent::Agent>,
    sid: Option<&str>,
    cwd: &Path,
) -> Option<tokio::task::JoinHandle<()>> {
    // ponytail: one day, then the wait lapses until the next idle point
    // re-arms it; a job outliving that needs a re-arm loop.
    let wait =
        agent?.background_wake(&idle_ctx(cwd, sid?), std::time::Duration::from_secs(86_400))?;
    Some(tokio::spawn(async move {
        if wait.await.is_some() {
            crate::host::request_wake();
        }
    }))
}

#[path = "cron_tests.rs"]
#[cfg(test)]
mod tests;
