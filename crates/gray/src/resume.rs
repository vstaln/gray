use std::path::{Path, PathBuf};

use crate::session_store::{
    JsonlSessionStore, SessionEnd, SessionId, SessionScanEvent, SessionSummary,
};

use crate::print::now_millis;

fn format_relative(ts: u64) -> String {
    let now = now_millis();
    let diff = now.saturating_sub(ts);
    let secs = diff / 1000;
    if secs < 60 {
        return "just now".to_string();
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins}m ago");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    if days < 7 {
        return format!("{days}d ago");
    }
    let weeks = days / 7;
    if weeks < 5 {
        return format!("{weeks}w ago");
    }
    chrono::DateTime::from_timestamp((ts / 1000) as i64, 0)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn cwd_display(cwd: &Path, width: usize) -> String {
    let s = cwd.display().to_string();
    let home = std::env::var("HOME").unwrap_or_default();
    let short = if !home.is_empty() && s.starts_with(&home) {
        format!("~{}", &s[home.len()..])
    } else {
        s
    };
    if short.chars().count() <= width {
        short
    } else {
        let mut t = short
            .chars()
            .skip(short.chars().count() - width + 1)
            .collect::<String>();
        t.insert(0, '…');
        t
    }
}

fn fit_col(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut t: String = text.chars().take(width.saturating_sub(1)).collect();
    t.push('\u{2026}');
    t
}

fn preview_text(s: &SessionSummary, width: usize) -> String {
    let raw = s
        .title
        .as_deref()
        .or(s.last_user_text.as_deref())
        .or(s.first_user_text.as_deref())
        .unwrap_or("(no message yet)");
    let one_line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= width {
        one_line
    } else {
        let mut t: String = one_line.chars().take(width - 1).collect();
        t.push('…');
        t
    }
}

pub(crate) fn short_id(id: &SessionId) -> String {
    let s = id.as_str();
    if uuid::Uuid::parse_str(s).is_ok() {
        s.split('-').next().unwrap_or(s).to_string()
    } else {
        s.to_string()
    }
}

fn paths_match(a: &Path, b: &Path) -> bool {
    let ca = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let cb = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    ca == cb
}

fn is_empty_session(s: &SessionSummary) -> bool {
    let has = |o: &Option<String>| o.as_deref().is_some_and(|t| !t.trim().is_empty());
    !(has(&s.first_user_text) || has(&s.last_user_text))
}

fn is_auxiliary_session(s: &SessionSummary) -> bool {
    s.origin.is_some() || system_tag(s).is_some()
}

fn system_tag(s: &SessionSummary) -> Option<String> {
    if let Some(o) = &s.origin {
        return Some(o.clone());
    }
    crate::session_store::opener_tag(s.first_user_text.as_deref()?).map(str::to_string)
}

const MIN_INTERRUPTED_SPAN_MS: u64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionState {
    Done,

    Interrupted,

    Running,
}

fn session_state(s: &SessionSummary, running: &std::collections::HashSet<String>) -> SessionState {
    if running.contains(s.id.as_str()) {
        SessionState::Running
    } else if s.last_end != SessionEnd::Done
        && s.dismissed_at != Some(s.last_message_at)
        && s.last_message_at.saturating_sub(s.started_at) >= MIN_INTERRUPTED_SPAN_MS
    {
        SessionState::Interrupted
    } else {
        SessionState::Done
    }
}

fn end_label(e: SessionEnd) -> &'static str {
    match e {
        SessionEnd::ToolUse => "tool call",
        SessionEnd::ToolResult => "tool result",
        SessionEnd::UserUnanswered => "you, unanswered",
        SessionEnd::Done => "",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StateFilter {
    All,
    Interrupted,
    Running,
}

impl StateFilter {
    fn next(self) -> Self {
        match self {
            Self::All => Self::Interrupted,
            Self::Interrupted => Self::Running,
            Self::Running => Self::All,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Interrupted => "Interrupted",
            Self::Running => "Running",
        }
    }
}

fn visible_indices(
    summaries: &[SessionSummary],
    query: &str,
    cwd_filter: Option<&Path>,
    show_system: bool,
    filter: StateFilter,
    running: &std::collections::HashSet<String>,
) -> Vec<usize> {
    let rank = |s: &SessionSummary| match session_state(s, running) {
        SessionState::Interrupted => 0,
        SessionState::Running => 1,
        SessionState::Done => 2,
    };
    let mut out: Vec<usize> = summaries
        .iter()
        .enumerate()
        .filter(|(_, s)| session_matches_with(s, query, cwd_filter, show_system))
        .filter(|(_, s)| {
            let st = session_state(s, running);
            match filter {
                StateFilter::All => true,
                StateFilter::Interrupted => st == SessionState::Interrupted,
                StateFilter::Running => st == SessionState::Running,
            }
        })
        .map(|(i, _)| i)
        .collect();
    out.sort_by(|&a, &b| {
        rank(&summaries[a])
            .cmp(&rank(&summaries[b]))
            .then_with(|| {
                summaries[b]
                    .last_message_at
                    .cmp(&summaries[a].last_message_at)
            })
            .then_with(|| summaries[b].id.as_str().cmp(summaries[a].id.as_str()))
    });
    out
}

#[cfg(test)]
fn visible_sessions<'a>(
    summaries: &'a [SessionSummary],
    query: &str,
    cwd_filter: Option<&Path>,
    show_system: bool,
    filter: StateFilter,
    running: &std::collections::HashSet<String>,
) -> Vec<&'a SessionSummary> {
    visible_indices(summaries, query, cwd_filter, show_system, filter, running)
        .into_iter()
        .map(|i| &summaries[i])
        .collect()
}

fn session_matches_with(
    s: &SessionSummary,
    query: &str,
    cwd_filter: Option<&Path>,
    show_hidden: bool,
) -> bool {
    if let Some(f) = cwd_filter
        && !paths_match(&s.cwd, f)
    {
        return false;
    }
    if is_empty_session(s) || (!show_hidden && is_auxiliary_session(s)) {
        return false;
    }
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    s.id.as_str().to_lowercase().contains(&q)
        || s.title.as_deref().unwrap_or("").to_lowercase().contains(&q)
        || s.cwd.display().to_string().to_lowercase().contains(&q)
        || s.first_user_text
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .contains(&q)
        || s.last_user_text
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .contains(&q)
}

pub fn latest_summary<'a>(
    summaries: &'a [SessionSummary],
    cwd_filter: Option<&Path>,
) -> Option<&'a SessionSummary> {
    summaries
        .iter()
        .filter(|s| cwd_filter.is_none_or(|cwd| paths_match(&s.cwd, cwd)))
        .filter(|s| !is_empty_session(s) && !is_auxiliary_session(s))
        .max_by_key(|s| s.last_message_at)
}

pub async fn recent_summaries(store: &JsonlSessionStore, all: bool) -> Vec<SessionSummary> {
    let cwd = std::env::current_dir().ok();
    let filt = if all { None } else { cwd.as_deref() };
    let mut out: Vec<SessionSummary> = store
        .list()
        .await
        .into_iter()
        .filter(|s| filt.is_none_or(|c| paths_match(&s.cwd, c)))
        .filter(|s| !is_empty_session(s) && !is_auxiliary_session(s))
        .collect();

    out.sort_by_key(|s| s.last_message_at);
    out.reverse();
    out
}

pub fn format_summary_row(s: &SessionSummary) -> String {
    format!(
        "{} — {} ({})",
        short_id(&s.id),
        preview_text(s, 80),
        format_relative(s.last_message_at)
    )
}

pub async fn resolve_prefix(
    store: &JsonlSessionStore,
    input: &str,
    all: bool,
) -> Option<SessionId> {
    let cwd = std::env::current_dir().ok();
    let summaries = store.list().await;
    let lower = input.trim().to_lowercase();
    if lower.is_empty() {
        return None;
    }

    if let Some(s) = summaries
        .iter()
        .find(|s| s.id.as_str().to_lowercase() == lower)
    {
        return Some(s.id.clone());
    }

    if !all && let Some(c) = cwd.as_deref() {
        let cwd_matches: Vec<&SessionSummary> = summaries
            .iter()
            .filter(|s| paths_match(&s.cwd, c) && s.id.as_str().to_lowercase().starts_with(&lower))
            .collect();
        if cwd_matches.len() == 1 {
            return Some(cwd_matches[0].id.clone());
        }
        if cwd_matches.len() > 1 {
            let latest = cwd_matches.into_iter().max_by_key(|s| s.last_message_at);
            if let Some(s) = latest {
                return Some(s.id.clone());
            }
        }
    }

    let all_matches: Vec<&SessionSummary> = summaries
        .iter()
        .filter(|s| s.id.as_str().to_lowercase().starts_with(&lower))
        .collect();
    if all_matches.len() == 1 {
        return Some(all_matches[0].id.clone());
    }
    if all_matches.len() > 1 {
        let latest = all_matches.into_iter().max_by_key(|s| s.last_message_at);
        if let Some(s) = latest {
            return Some(s.id.clone());
        }
    }

    None
}

fn quarantined_file_name(root: &Path, raw: &str) -> Option<String> {
    let needle = raw.trim();
    if needle.is_empty() {
        return None;
    }
    let exact = format!("{needle}.corrupt-");
    let mut prefix_hit = None;
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(&exact) {
            return Some(name);
        }
        if prefix_hit.is_none()
            && let Some((stem, _)) = name.split_once(".corrupt-")
            && stem.starts_with(needle)
        {
            prefix_hit = Some(name);
        }
    }
    prefix_hit
}

pub async fn resolve_session_strict(
    store: &JsonlSessionStore,
    raw: &str,
    all: bool,
) -> anyhow::Result<SessionId> {
    if let Some(resolved) = resolve_prefix(store, raw, all).await {
        return Ok(resolved);
    }
    match store.load(&SessionId::new(raw)).await {
        Ok(_) => Ok(SessionId::new(raw)),
        Err(e) if matches!(e, crate::session_store::SessionError::Corrupt { .. }) => {
            if let Some(q) = quarantined_file_name(store.root_dir(), raw) {
                anyhow::bail!("session '{raw}' is corrupt (moved to {q}): {e}");
            }
            anyhow::bail!("session '{raw}' is corrupt: {e}");
        }
        Err(e) => {
            if let Some(q) = quarantined_file_name(store.root_dir(), raw) {
                anyhow::bail!("session '{raw}' is corrupt (moved to {q})");
            }
            anyhow::bail!("no session matching '{raw}': {e}");
        }
    }
}

pub async fn latest_session_anywhere(store: &JsonlSessionStore) -> Option<SessionId> {
    let cwd = std::env::current_dir().ok();
    if let Some(c) = cwd.as_deref()
        && let Some(id) = store.recall_validated(c).await
    {
        return Some(id);
    }
    let summaries = store.list().await;
    latest_summary(&summaries, cwd.as_deref())
        .or_else(|| latest_summary(&summaries, None))
        .map(|s| s.id.clone())
}

pub fn effective_session_model(config_model: Option<&str>, meta_model: &str) -> Option<String> {
    let nonempty = |s: &str| !s.trim().is_empty();
    if let Some(m) = config_model
        && nonempty(m)
    {
        return Some(m.to_string());
    }
    if nonempty(meta_model) {
        return Some(meta_model.to_string());
    }
    None
}

pub async fn resumed_session_line(
    store: &JsonlSessionStore,
    id: &SessionId,
) -> anyhow::Result<String> {
    let (_, entries) = store
        .load(id)
        .await
        .map_err(|e| anyhow::anyhow!("could not resume session {}: {e}", id.as_str()))?;
    Ok(format!(
        "{} Resumed session {} ({} messages)",
        crate::tui::glyph("\u{2b22}", "#"),
        id.as_str(),
        entries.len()
    ))
}

pub fn locked_session_card(id: &SessionId, pid: Option<u32>) -> String {
    use std::io::IsTerminal as _;
    let notice = crate::session_store::SessionError::locked_notice(id, pid);
    let mut lines = notice.lines();
    let head = lines.next().unwrap_or_default();
    let mut out = format!("{} {head}", crate::tui::glyph("\u{2b22}", "#"));
    let dim = std::io::stderr().is_terminal();
    for line in lines {
        if dim {
            out.push_str(&format!("\n  \x1b[2m\u{2514} {line}\x1b[0m"));
        } else {
            out.push_str(&format!("\n  \u{2514} {line}"));
        }
    }
    out
}

fn interrupted_sessions<'a>(
    summaries: &'a [SessionSummary],
    running: &std::collections::HashSet<String>,
    cwd: Option<&Path>,
) -> Vec<&'a SessionSummary> {
    summaries
        .iter()
        .filter(|s| cwd.is_none_or(|c| s.cwd == c))
        .filter(|s| !is_empty_session(s) && system_tag(s).is_none())
        .filter(|s| session_state(s, running) == SessionState::Interrupted)
        .collect()
}

pub(crate) fn interrupted_hint(
    summaries: &[SessionSummary],
    running: &std::collections::HashSet<String>,
    cwd: &std::path::Path,
    home: Option<&std::path::Path>,
) -> Option<String> {
    let n = interrupted_sessions(summaries, running, Some(cwd)).len();
    if n == 0 {
        return None;
    }
    let shown = match home.and_then(|h| cwd.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => cwd.display().to_string(),
    };
    let noun = if n == 1 { "session" } else { "sessions" };
    Some(format!(
        "\u{26a0} {n} interrupted {noun} in {shown} \u{b7} /sessions to pick up \u{b7} /sessions dismiss to clear"
    ))
}

pub(crate) async fn dismiss_interrupted(cwd: Option<&Path>) -> usize {
    let Some(root) = crate::session_store::default_root() else {
        return 0;
    };
    let store = JsonlSessionStore::new(root.clone());
    let summaries = store.list().await;
    let running = store
        .held_open_locks(summaries.iter().map(|s| s.id.clone()).collect())
        .await;
    interrupted_sessions(&summaries, &running, cwd)
        .into_iter()
        .filter(|s| crate::session_store::write_dismissal(&root, &s.id, s.last_message_at).is_ok())
        .count()
}

pub(crate) async fn startup_interrupted_hint(cwd: &std::path::Path) -> Option<String> {
    let store = JsonlSessionStore::new(crate::session_store::default_root()?);
    let summaries = store.list_in(cwd).await;
    let running = store
        .held_open_locks(
            summaries
                .iter()
                .filter(|s| s.cwd == cwd)
                .map(|s| s.id.clone())
                .collect(),
        )
        .await;
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    interrupted_hint(&summaries, &running, cwd, home.as_deref())
}

pub async fn run_resume_picker(
    show_all: bool,
    bg: Option<&crate::setup::BackgroundSnapshot>,
    current: Option<SessionId>,
) -> anyhow::Result<Option<SessionId>> {
    let root = crate::session_store::default_root()
        .ok_or_else(|| anyhow::anyhow!("cannot resolve home"))?;
    let store = JsonlSessionStore::new(root);

    let scan = store.scan(None, true);
    run_picker_sync(show_all, scan, &store, bg, current)
}

fn run_picker_sync(
    mut show_all: bool,
    mut scan: tokio::sync::mpsc::UnboundedReceiver<SessionScanEvent>,
    store: &JsonlSessionStore,
    bg: Option<&crate::setup::BackgroundSnapshot>,
    current: Option<SessionId>,
) -> anyhow::Result<Option<SessionId>> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
    use ratatui::layout::Rect;
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use std::time::Duration;

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(crate::setup::BackgroundSnapshot::default_initial);

    let (_session, mut terminal) = crate::setup::open_modal()?;

    let box_bg = crate::theme::theme().surface_bg;
    let accent_peach = crate::theme::theme().accent;
    let text_dim = crate::theme::theme().text_dim;

    let mut summaries: Vec<SessionSummary> = Vec::new();
    let mut running = std::collections::HashSet::new();
    let mut filtered: Vec<usize> = Vec::new();
    let mut dirty = true;
    let mut scan_done = false;

    let mut query = String::new();
    let mut show_hidden = false;
    let mut state_filter = StateFilter::All;
    let mut sel: usize = 0;
    let mut scroll_top: usize = 0;

    let mut rename: Option<(SessionId, String)> = None;

    let mut takeover: Option<(SessionId, Option<u32>)> = None;

    let mut notice: Option<String> = None;

    let result: anyhow::Result<Option<SessionId>> = (|| {
        loop {
            while let Ok(event) = scan.try_recv() {
                match event {
                    SessionScanEvent::Running(set) => running = set,
                    SessionScanEvent::Summary(s) => {
                        if !is_empty_session(&s) {
                            summaries.push(s);
                        }
                    }
                    SessionScanEvent::Done => scan_done = true,
                }
                dirty = true;
            }
            if scan.is_closed() {
                scan_done = true;
            }
            if scan_done && summaries.is_empty() {
                return Err(anyhow::anyhow!("no saved sessions"));
            }
            if dirty {
                dirty = false;
                let cwd_filter: Option<&Path> = if show_all { None } else { Some(&cwd) };
                filtered = visible_indices(
                    &summaries,
                    &query,
                    cwd_filter,
                    show_hidden,
                    state_filter,
                    &running,
                );
                if sel >= filtered.len() && !filtered.is_empty() {
                    sel = filtered.len() - 1;
                }
                if filtered.is_empty() {
                    sel = 0;
                }
            }

            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 30 || area.height < 8 {
                    return;
                }
                crate::setup::render_dimmed_background(frame, &bg_snapshot);

                let modal_w = 84.min(area.width.saturating_sub(2)).max(40).min(area.width);

                let modal_h = (filtered.len().clamp(1, 12) as u16 + 8)
                    .min(20)
                    .min(area.height.saturating_sub(2))
                    .max(9)
                    .min(area.height);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);

                frame.render_widget(Clear, modal_rect);
                let box_block = Block::default().style(Style::default().bg(box_bg));
                frame.render_widget(box_block, modal_rect);

                let pad_x = 2u16;
                let inner_w = modal_w.saturating_sub(pad_x * 2);
                let inner = Rect::new(
                    modal_x + pad_x,
                    modal_y + 1,
                    inner_w,
                    modal_h.saturating_sub(2),
                );

                let title = if show_all {
                    "Sessions — all"
                } else {
                    "Sessions"
                };
                let esc_str = "esc";
                let hints_str = "tab: all/cwd  shift-tab: status  ctrl-g: system  ";
                let cur_name = current.as_ref().map(|c| c.as_str());
                let name_len = cur_name.map_or(0, |n| n.chars().count() + 8);
                let show_hints = cur_name.is_none()
                    || title.chars().count()
                        + name_len
                        + hints_str.chars().count()
                        + esc_str.chars().count()
                        <= inner.width as usize;
                let pad_len = if cur_name.is_some() {
                    (inner.width as usize).saturating_sub(
                        title.chars().count()
                            + name_len
                            + esc_str.chars().count()
                            + if show_hints {
                                hints_str.chars().count()
                            } else {
                                0
                            },
                    )
                } else {
                    (inner.width as usize)
                        .saturating_sub(title.chars().count() + esc_str.chars().count() + 8)
                };
                let mut header_spans = vec![
                    Span::styled(
                        title,
                        Style::default()
                            .fg(crate::theme::theme().text_bright)
                            .add_modifier(Modifier::BOLD)
                            .bg(box_bg),
                    ),
                    Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                ];
                if let Some(name) = cur_name {
                    header_spans.push(Span::styled(
                        "this: ",
                        Style::default().fg(text_dim).bg(box_bg),
                    ));
                    header_spans.push(Span::styled(
                        name.to_string(),
                        Style::default().fg(accent_peach).bg(box_bg),
                    ));
                    header_spans.push(Span::styled("  ", Style::default().fg(text_dim).bg(box_bg)));
                }
                if show_hints {
                    header_spans.push(Span::styled(
                        hints_str,
                        Style::default().fg(text_dim).bg(box_bg),
                    ));
                }
                header_spans.push(Span::styled(
                    esc_str,
                    Style::default().fg(text_dim).bg(box_bg),
                ));
                let header = Line::from(header_spans);
                frame.render_widget(
                    Paragraph::new(header),
                    Rect::new(inner.x, inner.y, inner.width, 1),
                );

                let search_line = if let Some((id, pid)) = &takeover {
                    let who = match pid {
                        Some(p) => format!("pid {p}"),
                        None => "another process".to_string(),
                    };
                    Line::from(vec![
                        Span::styled(
                            "Take over: ",
                            Style::default()
                                .fg(accent_peach)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled(
                            format!(
                                "'{}' is open in {who} — enter ends it, esc keeps it",
                                short_id(id)
                            ),
                            Style::default()
                                .fg(crate::theme::theme().text_bright)
                                .bg(box_bg),
                        ),
                    ])
                } else if let Some(n) = &notice {
                    Line::from(Span::styled(
                        n.clone(),
                        Style::default().fg(accent_peach).bg(box_bg),
                    ))
                } else if let Some((_, text)) = &rename {
                    Line::from(vec![
                        Span::styled(
                            "Rename: ",
                            Style::default()
                                .fg(accent_peach)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled(
                            text.clone(),
                            Style::default()
                                .fg(crate::theme::theme().text_bright)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled("▎", Style::default().fg(accent_peach).bg(box_bg)),
                    ])
                } else if query.is_empty() {
                    Line::from(vec![
                        Span::styled(
                            "Search: ",
                            Style::default()
                                .fg(accent_peach)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled(
                            "Type to filter…",
                            Style::default()
                                .fg(crate::theme::theme().text_dim)
                                .bg(box_bg),
                        ),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled(
                            "Search: ",
                            Style::default()
                                .fg(accent_peach)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled(
                            query.clone(),
                            Style::default()
                                .fg(crate::theme::theme().text_bright)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled("▎", Style::default().fg(accent_peach).bg(box_bg)),
                    ])
                };
                frame.render_widget(
                    Paragraph::new(search_line),
                    Rect::new(inner.x, inner.y + 1, inner.width, 1),
                );

                let mut filter_spans = if show_all {
                    vec![Span::styled(
                        "Showing all sessions",
                        Style::default().fg(text_dim).bg(box_bg),
                    )]
                } else {
                    vec![
                        Span::styled("Filtered to ", Style::default().fg(text_dim).bg(box_bg)),
                        Span::styled(
                            cwd_display(&cwd, 40),
                            Style::default()
                                .fg(crate::theme::theme().text_bright)
                                .bg(box_bg),
                        ),
                    ]
                };
                if state_filter != StateFilter::All {
                    filter_spans.push(Span::styled(
                        format!("  ·  {} ({})", state_filter.label(), filtered.len()),
                        Style::default().fg(accent_peach).bg(box_bg),
                    ));
                }
                if show_hidden {
                    filter_spans.push(Span::styled(
                        "  ·  + system",
                        Style::default().fg(text_dim).bg(box_bg),
                    ));
                }
                let filter_line = Line::from(filter_spans);
                frame.render_widget(
                    Paragraph::new(filter_line),
                    Rect::new(inner.x, inner.y + 2, inner.width, 1),
                );

                let date_w = 9usize;
                let cwd_w = 14usize;
                let id_w = if state_filter == StateFilter::Interrupted {
                    15usize
                } else {
                    10usize
                };

                let prev_w = (inner.width as usize)
                    .saturating_sub(1 + 2 + date_w + 2 + cwd_w + 2 + id_w + 2)
                    .max(12);

                let list_y = inner.y + 4;
                let list_h = inner.height.saturating_sub(6) as usize;

                if filtered.is_empty() {
                    let msg = if summaries.is_empty() && !scan_done {
                        "Scanning sessions…"
                    } else if summaries.is_empty() {
                        "No saved sessions yet"
                    } else if state_filter == StateFilter::Interrupted {
                        "No interrupted sessions — shift-tab to change the filter"
                    } else if state_filter == StateFilter::Running {
                        "No sessions running right now — shift-tab to change the filter"
                    } else if query.is_empty() {
                        "No sessions in this directory — press Tab to show all"
                    } else {
                        "No matching sessions"
                    };
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled(
                            format!("  {msg}"),
                            Style::default().fg(text_dim).bg(box_bg),
                        ))),
                        Rect::new(inner.x, list_y, inner.width, 1),
                    );
                } else {
                    let visible = list_h.max(1);
                    if sel < scroll_top {
                        scroll_top = sel;
                    } else if sel >= scroll_top + visible {
                        scroll_top = sel + 1 - visible;
                    }
                    for r in 0..visible {
                        let idx = scroll_top + r;
                        if idx >= filtered.len() {
                            break;
                        }
                        let s = &summaries[filtered[idx]];
                        let is_sel = idx == sel;

                        let date = format_relative(s.last_message_at);
                        let cwd_s = cwd_display(&s.cwd, cwd_w);
                        let prev = preview_text(s, prev_w);
                        let st = session_state(s, &running);
                        let (marker, marker_fg) = match st {
                            SessionState::Interrupted => ("\u{26a0}", accent_peach),
                            SessionState::Running => ("\u{25cf}", crate::theme::theme().success),
                            SessionState::Done => (" ", crate::theme::theme().text_bright),
                        };
                        let right = match system_tag(s) {
                            Some(tag) if show_hidden => tag,
                            _ if state_filter == StateFilter::Interrupted => {
                                end_label(s.last_end).to_string()
                            }
                            _ => short_id(&s.id),
                        };
                        let right = fit_col(&right, id_w);
                        let rest = format!(
                            " {:>date_w$}  {:cwd_w$}  {:prev_w$}  {:>id_w$}",
                            date,
                            cwd_s,
                            prev,
                            right,
                            date_w = date_w,
                            cwd_w = cwd_w,
                            prev_w = prev_w,
                            id_w = id_w
                        );
                        let used = 1 + rest.chars().count();
                        let fill = (inner.width as usize).saturating_sub(used);
                        let line = if is_sel {
                            Line::from(Span::styled(
                                format!(" {marker}{rest}{}", " ".repeat(fill)),
                                Style::default()
                                    .fg(crate::theme::theme().on_selection)
                                    .bg(accent_peach)
                                    .add_modifier(Modifier::BOLD),
                            ))
                        } else {
                            let base = Style::default()
                                .fg(crate::theme::theme().text_bright)
                                .bg(box_bg);
                            Line::from(vec![
                                Span::styled(" ", base),
                                Span::styled(marker, Style::default().fg(marker_fg).bg(box_bg)),
                                Span::styled(format!("{rest}{}", " ".repeat(fill)), base),
                            ])
                        };
                        frame.render_widget(
                            Paragraph::new(line),
                            Rect::new(inner.x, list_y + r as u16, inner.width, 1),
                        );
                    }
                }

                let sel_running = filtered.get(sel).is_some_and(|&i| {
                    session_state(&summaries[i], &running) == SessionState::Running
                });
                let hints: &[(&str, &str)] = if takeover.is_some() {
                    &[("enter", "take over"), ("esc", "back")]
                } else if rename.is_some() {
                    &[("enter", "save"), ("esc", "cancel")]
                } else if sel_running {
                    &[
                        ("↑↓", "navigate"),
                        ("enter", "take over"),
                        ("ctrl-r", "rename"),
                        ("ctrl-d", "dismiss"),
                        ("esc", "cancel"),
                    ]
                } else {
                    &[
                        ("↑↓", "navigate"),
                        ("enter", "resume"),
                        ("ctrl-r", "rename"),
                        ("ctrl-d", "dismiss"),
                        ("esc", "cancel"),
                    ]
                };
                let mut footer_spans = Vec::new();
                for (n, (key, label)) in hints.iter().enumerate() {
                    let gap = if n + 1 < hints.len() { "  " } else { "" };
                    footer_spans.push(Span::styled(
                        format!("{key} "),
                        Style::default()
                            .fg(crate::theme::theme().text_bright)
                            .add_modifier(Modifier::BOLD)
                            .bg(box_bg),
                    ));
                    footer_spans.push(Span::styled(
                        format!("{label}{gap}"),
                        Style::default().fg(text_dim).bg(box_bg),
                    ));
                }
                let footer = Line::from(footer_spans);
                frame.render_widget(
                    Paragraph::new(footer),
                    Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
                );
            })?;

            if !poll(Duration::from_millis(80))? {
                continue;
            }
            let event = read()?;

            if matches!(
                event,
                Event::Key(KeyEvent {
                    kind: KeyEventKind::Press,
                    ..
                })
            ) {
                dirty = true;
            }
            if takeover.is_none()
                && rename.is_none()
                && let Event::Key(key) = event
                && key.kind == KeyEventKind::Press
                && let Some(up) = crate::keymap::select_move(&key)
            {
                if !filtered.is_empty() {
                    sel = if up {
                        sel.saturating_sub(1)
                    } else {
                        (sel + 1).min(filtered.len() - 1)
                    };
                }
                continue;
            }
            match event {
                Event::Key(KeyEvent {
                    code: KeyCode::Char('c'),
                    modifiers,
                    kind: KeyEventKind::Press,
                    ..
                }) if modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(None);
                }
                Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Press,
                    ..
                }) if takeover.is_some() => match code {
                    KeyCode::Esc => takeover = None,
                    KeyCode::Enter => {
                        if let Some((id, pid)) = takeover.take() {
                            match try_take_over(store, &id, pid) {
                                Ok(()) => return Ok(Some(id)),
                                Err(n) => notice = Some(n),
                            }
                        }
                    }
                    _ => {}
                },
                Event::Key(KeyEvent {
                    code,
                    modifiers,
                    kind: KeyEventKind::Press,
                    ..
                }) if rename.is_some() => match code {
                    KeyCode::Esc => rename = None,
                    KeyCode::Enter => {
                        if let Some((id, text)) = rename.take()
                            && let Some(root) = crate::session_store::default_root()
                            && let Ok(stored) = crate::session_store::write_title(&root, &id, &text)
                            && let Some(s) = summaries.iter_mut().find(|s| s.id == id)
                        {
                            s.title = stored;
                        }
                    }
                    KeyCode::Backspace => {
                        if let Some((_, text)) = rename.as_mut() {
                            text.pop();
                        }
                    }
                    KeyCode::Char(ch)
                        if !modifiers.contains(KeyModifiers::CONTROL)
                            && !modifiers.contains(KeyModifiers::ALT) =>
                    {
                        if let Some((_, text)) = rename.as_mut() {
                            text.push(ch);
                        }
                    }
                    _ => {}
                },
                Event::Key(KeyEvent {
                    code,
                    modifiers,
                    kind: KeyEventKind::Press,
                    ..
                }) => match code {
                    KeyCode::Esc => {
                        notice = None;
                        if !query.is_empty() {
                            query.clear();
                            sel = 0;
                        } else {
                            return Ok(None);
                        }
                    }
                    KeyCode::Tab => {
                        show_all = !show_all;
                        sel = 0;
                        scroll_top = 0;
                    }
                    KeyCode::BackTab => {
                        state_filter = state_filter.next();
                        sel = 0;
                        scroll_top = 0;
                    }
                    KeyCode::Char('g') if modifiers.contains(KeyModifiers::CONTROL) => {
                        show_hidden = !show_hidden;
                        sel = 0;
                        scroll_top = 0;
                    }

                    KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {
                        rename = filtered.get(sel).map(|&i| {
                            (
                                summaries[i].id.clone(),
                                summaries[i].title.clone().unwrap_or_default(),
                            )
                        });
                    }

                    KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {
                        let target = filtered
                            .get(sel)
                            .map(|&i| &summaries[i])
                            .filter(|s| session_state(s, &running) == SessionState::Interrupted)
                            .map(|s| (s.id.clone(), s.last_message_at));
                        if let (Some((id, at)), Some(root)) =
                            (target, crate::session_store::default_root())
                            && crate::session_store::write_dismissal(&root, &id, at).is_ok()
                            && let Some(s) = summaries.iter_mut().find(|s| s.id == id)
                        {
                            s.dismissed_at = Some(at);
                        }
                    }
                    KeyCode::Enter => {
                        notice = None;
                        if let Some(&i) = filtered.get(sel) {
                            let s = &summaries[i];
                            let pid = store.open_holder_pid(&s.id);
                            if session_state(s, &running) == SessionState::Running
                                && pid != Some(std::process::id())
                            {
                                takeover = Some((s.id.clone(), pid));
                            } else {
                                return Ok(Some(s.id.clone()));
                            }
                        }
                    }
                    KeyCode::Backspace => {
                        query.pop();
                        sel = 0;
                    }
                    KeyCode::Char(ch)
                        if !modifiers.contains(KeyModifiers::CONTROL)
                            && !modifiers.contains(KeyModifiers::ALT) =>
                    {
                        query.push(ch);
                        sel = 0;
                    }
                    _ => {}
                },
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    })();

    let _ = terminal.clear();
    result
}

fn try_take_over(
    store: &JsonlSessionStore,
    id: &SessionId,
    pid: Option<u32>,
) -> Result<(), String> {
    if !store.open_lock_held_sync(id) {
        return Ok(());
    }
    let Some(pid) = pid else {
        return Err(format!(
            "'{}' is locked but the holder pid is unreadable — close the other instance yourself",
            short_id(id)
        ));
    };
    if pid == std::process::id() {
        return Ok(());
    }
    terminate_holder(pid)?;

    for _ in 0..30 {
        if !store.open_lock_held_sync(id) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err(format!(
        "pid {pid} still holds '{}' — it may be ignoring SIGTERM; close it there or pick another",
        short_id(id)
    ))
}

#[cfg(unix)]
fn terminate_holder(pid: u32) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let mut exe = String::new();
        for _ in 0..5 {
            if let Ok(cmdline) = std::fs::read_to_string(format!("/proc/{pid}/cmdline")) {
                exe = cmdline.split('\0').next().unwrap_or_default().to_string();
                if !exe.is_empty() {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        match exe.is_empty() {
            false => {
                let is_gray = exe.rsplit('/').next().unwrap_or(&exe).starts_with("gray");
                if !is_gray {
                    return Err(format!(
                        "pid {pid} doesn't look like a gray process ({exe}) — refusing to signal it"
                    ));
                }
            }
            true => {
                let alive = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
                if !alive {
                    return Err(format!("pid {pid} is already gone — pick it again"));
                }
                return Err(format!(
                    "pid {pid} is alive but its cmdline is unreadable — refusing to signal it"
                ));
            }
        }
    }
    if unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } == 0 {
        Ok(())
    } else {
        Err(format!("couldn't signal pid {pid} — close it yourself"))
    }
}

#[cfg(not(unix))]
fn terminate_holder(pid: u32) -> Result<(), String> {
    let _ = pid;
    Err("take over isn't supported on this platform — close the other instance yourself".into())
}

#[path = "resume_tests.rs"]
#[cfg(test)]
mod tests;
