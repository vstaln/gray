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

/// One thing the idle wake paints before its turn.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum WakeCard {
    /// A cron delivery: painted as a box (see [`cron_card_lines`]).
    Cron(crate::cron_serve::CronCard),
    /// A finished background job: painted as a box (see [`job_card_lines`]).
    Job(JobCard),
    /// A one-line notice (legacy inbox entries, unparsed notices).
    Text(String),
}

/// The parts of a background-job notice worth showing the user.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct JobCard {
    pub id: String,
    /// `exit 0`, `exit 2`, `killed`, ...
    pub outcome: String,
    pub elapsed: String,
    pub log: String,
}

/// Parses `Background job {id} finished ({outcome}) after {t} · log {path}`
/// (see `gray-tools` `bash/jobs.rs`). The trailing "Read its output…" line
/// is for the model only, so it is dropped.
pub(super) fn parse_job_notice(notice: &str) -> Option<JobCard> {
    let head = notice.lines().next()?;
    let rest = head.strip_prefix("Background job ")?;
    let (id, rest) = rest.split_once(" finished (")?;
    let (outcome, rest) = rest.split_once(") after ")?;
    let (elapsed, log) = rest.split_once(" \u{b7} log ")?;
    Some(JobCard {
        id: id.to_string(),
        outcome: outcome.to_string(),
        elapsed: elapsed.to_string(),
        log: log.trim().to_string(),
    })
}

/// `⬢ <verb><title>` in the same type as tool-call headers.
fn card_header_spans(
    verb: &str,
    title: String,
    failed: bool,
) -> Vec<ratatui::text::Span<'static>> {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::Span;
    let th = crate::theme::theme();
    let bullet = if failed { th.error_soft } else { th.tool_accent };
    vec![
        Span::styled(
            "\u{2b22} ",
            Style::default().fg(bullet).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            verb.to_string(),
            Style::default().fg(th.text_body).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            title,
            Style::default().fg(th.tool_command).add_modifier(Modifier::BOLD),
        ),
    ]
}

/// Header + body rows of a background-job box:
/// `⬢ Background job bwrap-5 · exit 0 · 4m26s`, then the log path.
pub(super) fn job_card_lines(
    card: &JobCard,
) -> (
    ratatui::text::Line<'static>,
    Vec<ratatui::text::Line<'static>>,
) {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    let th = crate::theme::theme();
    let ok = card.outcome == "exit 0";
    let dim = Style::default().fg(th.tool_dim);
    let mut spans = card_header_spans("Background job ", card.id.clone(), !ok);
    spans.push(Span::styled(" \u{b7} ", dim));
    spans.push(if ok {
        Span::styled(card.outcome.clone(), dim)
    } else {
        Span::styled(
            card.outcome.clone(),
            Style::default().fg(th.error_soft).add_modifier(Modifier::BOLD),
        )
    });
    spans.push(Span::styled(format!(" \u{b7} {}", card.elapsed), dim));
    let log = match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => card
            .log
            .strip_prefix(home.as_str())
            .map_or_else(|| card.log.clone(), |rest| format!("~{rest}")),
        _ => card.log.clone(),
    };
    let body = vec![Line::from(vec![
        Span::styled("  log ", dim),
        Span::styled(log, Style::default().fg(th.tool_path)),
    ])];
    (Line::from(spans), body)
}

/// Header + body rows of a cron delivery box: `⬢ Cron name · done · 18s`, then
/// the final answer as markdown. A reminder is the user's own words, so its
/// header is just "Reminder" and its body is verbatim.
pub(super) fn cron_card_lines(
    card: &crate::cron_serve::CronCard,
    width: usize,
) -> (
    ratatui::text::Line<'static>,
    Vec<ratatui::text::Line<'static>>,
) {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    let th = crate::theme::theme();
    let mut spans = if card.reminder && !card.failed {
        card_header_spans("Reminder", String::new(), false)
    } else {
        card_header_spans("Cron ", card.name.clone(), card.failed)
    };
    let dim = Style::default().fg(th.tool_dim);
    if card.failed {
        spans.push(Span::styled(" \u{b7} ", dim));
        spans.push(Span::styled(
            "failed",
            Style::default().fg(th.error_soft).add_modifier(Modifier::BOLD),
        ));
    } else if !card.reminder {
        spans.push(Span::styled(" \u{b7} done", dim));
    }
    if card.elapsed_ms > 0 {
        spans.push(Span::styled(
            format!(" \u{b7} {}", super::format::fmt_duration_ms(card.elapsed_ms)),
            dim,
        ));
    }
    let header = Line::from(spans);
    let text = card.body.trim();
    let body = if card.reminder || card.failed {
        text.lines()
            .map(|l| Line::from(vec![Span::raw("  "), Span::raw(l.to_string())]))
            .collect()
    } else {
        let (rows, _) = crate::composer::transcript::render_markdown_lines(
            text,
            Some(width.saturating_sub(4).max(10)),
        );
        rows.into_iter()
            .map(|mut l| {
                l.spans.insert(0, Span::raw("  "));
                l
            })
            .collect()
    };
    (header, body)
}

/// What the idle REPL owes the model, as `(cards, prompt)`: cron deliveries
/// waiting in this session's inbox, then background-job notices nobody has
/// drained since the last turn. The cards paint first; the prompt (user-role,
/// persisted like a typed one) starts a turn so the model reacts in chat.
pub(super) fn idle_wake(
    agent: Option<&gray_core::agent::Agent>,
    sid: Option<&str>,
    cwd: &Path,
) -> Option<(Vec<WakeCard>, String)> {
    let sid = sid?;
    let mut cards = Vec::new();
    let mut prompts = Vec::new();
    if let Ok(home) = crate::setup::gray_home() {
        for d in crate::cron_serve::drain_session_inbox(&home, sid) {
            cards.push(match d.cron {
                Some(c) => WakeCard::Cron(c),
                None => WakeCard::Text(d.card),
            });
            prompts.push(d.prompt);
        }
    }
    if let Some(agent) = agent {
        for notice in agent.drain_background_notifications(&idle_ctx(cwd, sid)) {
            cards.push(match parse_job_notice(&notice) {
                Some(job) => WakeCard::Job(job),
                None => WakeCard::Text(notice.lines().next().unwrap_or_default().to_string()),
            });
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
    let ctx = idle_ctx(cwd, sid?);
    let exec = agent?.executor_handle();
    if !exec.has_pending_background(&ctx) {
        return None;
    }
    Some(tokio::spawn(wait_then_wake(exec, ctx, WAKE_WAIT_SLICE)))
}

/// One bounded wait on the executor; the waiter re-waits while jobs are
/// still pending, so a job of any length wakes the prompt when it settles.
const WAKE_WAIT_SLICE: std::time::Duration = std::time::Duration::from_secs(3600);

async fn wait_then_wake(
    exec: std::sync::Arc<dyn gray_core::agent::ToolExecutor>,
    ctx: gray_core::agent::ToolContext,
    slice: std::time::Duration,
) {
    loop {
        if exec.wait_for_notification(&ctx, slice).await.is_some() {
            crate::host::request_wake();
            return;
        }
        if !exec.has_pending_background(&ctx) {
            return;
        }
    }
}

#[path = "cron_tests.rs"]
#[cfg(test)]
mod tests;
