use std::path::Path;

fn idle_ctx(cwd: &Path, sid: &str) -> gray_core::agent::ToolContext {
    gray_core::agent::ToolContext {
        cwd: cwd.to_path_buf(),
        cancel: tokio_util::sync::CancellationToken::new(),
        session_id: Some(sid.to_string()),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WakeCard {
    Cron(crate::session_mailbox::CronCard),

    Job(JobCard),

    Text(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JobCard {
    pub id: String,

    pub outcome: String,
    pub elapsed: String,
    pub log: String,
}

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

pub(super) fn card_header_spans(
    verb: &str,
    title: String,
    failed: bool,
) -> Vec<ratatui::text::Span<'static>> {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::Span;
    let th = crate::theme::theme();
    let bullet = if failed {
        th.error_soft
    } else {
        th.tool_accent
    };
    vec![
        Span::styled(
            format!("{} ", crate::tui::glyph("\u{2b22}", "#")),
            Style::default().fg(bullet).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            verb.to_string(),
            Style::default()
                .fg(th.text_body)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            title,
            Style::default()
                .fg(th.tool_command)
                .add_modifier(Modifier::BOLD),
        ),
    ]
}

pub(super) fn push_notice(t: &mut crate::composer::Tui, verb: &str, detail: &str, failed: bool) {
    if failed {
        t.push_error(&format!("{verb}: {detail}"));
    } else {
        t.push_action(verb, (!detail.is_empty()).then_some(detail));
    }
}

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
            Style::default()
                .fg(th.error_soft)
                .add_modifier(Modifier::BOLD),
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

pub(crate) fn format_delivery_plain(
    name: &str,
    body: &str,
    reminder: bool,
    failed: bool,
) -> String {
    if reminder && !failed {
        return body.to_string();
    }
    let title = if failed {
        format!("{name} failed")
    } else {
        name.to_string()
    };
    format!("{title}\n\n{}", body.trim())
}

pub(super) fn cron_card_lines(
    card: &crate::session_mailbox::CronCard,
    width: usize,
) -> (
    ratatui::text::Line<'static>,
    Vec<ratatui::text::Line<'static>>,
) {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    let th = crate::theme::theme();

    let subagent = card.name.strip_prefix("subagent/");
    let mut spans = if card.reminder && !card.failed {
        card_header_spans("Reminder", String::new(), false)
    } else if let Some(name) = subagent {
        card_header_spans("Subagent ", name.to_string(), card.failed)
    } else {
        card_header_spans("Cron ", card.name.clone(), card.failed)
    };
    let dim = Style::default().fg(th.tool_dim);
    if card.failed {
        spans.push(Span::styled(" \u{b7} ", dim));
        spans.push(Span::styled(
            "failed",
            Style::default()
                .fg(th.error_soft)
                .add_modifier(Modifier::BOLD),
        ));
    } else if !card.reminder {
        spans.push(Span::styled(" \u{b7} done", dim));
    }
    if card.elapsed_ms > 0 {
        spans.push(Span::styled(
            format!(
                " \u{b7} {}",
                super::format::fmt_duration_ms(card.elapsed_ms)
            ),
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

pub(crate) fn idle_wake(
    agent: Option<&gray_core::agent::Agent>,
    sid: Option<&str>,
    cwd: &Path,
) -> Option<(Vec<WakeCard>, String, Vec<crate::session_send::SendMsg>)> {
    let sid = sid?;
    let mut cards = Vec::new();
    let mut prompts = Vec::new();
    let mut sends = Vec::new();
    if let Ok(home) = crate::setup::gray_home() {
        for d in crate::session_mailbox::drain_session_inbox(&home, sid) {
            cards.push(match d.cron {
                Some(c) => WakeCard::Cron(c),
                None => WakeCard::Text(d.card),
            });
            prompts.push(d.prompt);
        }

        for m in crate::session_send::claim_all(&home, sid) {
            prompts.push(crate::session_send::prompt_for(&m.from, &m.text));
            sends.push(m);
        }
    }
    if let Some(agent) = agent {
        for notice in agent.drain_background_notifications(&idle_ctx(cwd, sid)) {
            cards.push(match parse_job_notice(&notice) {
                Some(job) => WakeCard::Job(job),
                None => WakeCard::Text(notice.lines().next().unwrap_or_default().to_string()),
            });

            prompts.push(format!(
                "{}\n{notice}",
                crate::composer::transcript::BG_NOTICE_MARKER
            ));
        }
    }
    (!prompts.is_empty()).then(|| (cards, prompts.join("\n\n"), sends))
}

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
