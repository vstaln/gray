use std::path::{Path, PathBuf};

use crate::session_store::{JsonlSessionStore, SessionEnd, SessionId, SessionSummary};

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

/// Row preview: the latest user message, so a session reads as what it was
/// last about. Falls back to the opener for a session whose only user text is
/// empty, then to the empty-session placeholder.
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

/// Display id: the first 8 hex chars for a legacy UUID, the full string for
/// a three-word name (its first `-` segment would read as just "chiral").
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

/// An "empty" session has no user text on either end — created but never
/// sent a message (the `(no message yet)` row, usually the `just now`
/// session at the top). Noise in every resume listing, so the picker,
/// headless lists, and `--last` all skip them. Explicit `resume <id>`
/// still loads one.
fn is_empty_session(s: &SessionSummary) -> bool {
    let has = |o: &Option<String>| o.as_deref().is_some_and(|t| !t.trim().is_empty());
    !(has(&s.first_user_text) || has(&s.last_user_text))
}

/// An auxiliary session was minted by a non-interactive producer (subagent
/// runs carry `origin: "subagent"` from the supervisor's env): real work,
/// still loadable by id, but noise in pickers and `-c`/`--last` selection.
fn is_auxiliary_session(s: &SessionSummary) -> bool {
    s.origin.is_some() || system_tag(s).is_some()
}

/// Why a session reads as gray-made, for the picker's right-hand tag: the
/// header `origin`, or a recognisable opener from before origins existed
/// (background-task notifications, subagent / worker prompts).
fn system_tag(s: &SessionSummary) -> Option<String> {
    if let Some(o) = &s.origin {
        return Some(o.clone());
    }
    crate::session_store::opener_tag(s.first_user_text.as_deref()?).map(str::to_string)
}

/// Shorter than this between first and last entry reads as a one-shot, not
/// work worth flagging as cut off.
const MIN_INTERRUPTED_SPAN_MS: u64 = 60_000;

/// Where a session stands, for the picker's marker column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionState {
    /// Finished: the last entry is an assistant reply.
    Done,
    /// Cut off mid-task: nobody holds it and the last entry never got its reply.
    Interrupted,
    /// Open in a live process right now.
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

/// How an interrupted session was cut off, for the interrupted view.
fn end_label(e: SessionEnd) -> &'static str {
    match e {
        SessionEnd::ToolUse => "tool call",
        SessionEnd::ToolResult => "tool result",
        SessionEnd::UserUnanswered => "you, unanswered",
        SessionEnd::Done => "",
    }
}

/// Picker status filter, cycled with shift-tab.
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

/// Rows the picker shows: scope + search + status filter, with interrupted
/// then running sessions floated above finished ones (stable, so each group
/// keeps newest-activity order).
fn visible_sessions<'a>(
    summaries: &'a [SessionSummary],
    query: &str,
    cwd_filter: Option<&Path>,
    show_system: bool,
    filter: StateFilter,
    running: &std::collections::HashSet<String>,
) -> Vec<&'a SessionSummary> {
    let mut out: Vec<&SessionSummary> = summaries
        .iter()
        .filter(|s| session_matches_with(s, query, cwd_filter, show_system))
        .filter(|s| {
            let st = session_state(s, running);
            match filter {
                StateFilter::All => true,
                StateFilter::Interrupted => st == SessionState::Interrupted,
                StateFilter::Running => st == SessionState::Running,
            }
        })
        .collect();
    out.sort_by_key(|s| match session_state(s, running) {
        SessionState::Interrupted => 0,
        SessionState::Running => 1,
        SessionState::Done => 2,
    });
    out
}

/// Picker filter: cwd scope plus case-insensitive query over id, cwd, and user
/// text (both ends — the preview shows the latest, a remembered opener still
/// identifies the session). Gray-made sessions are kept only when
/// `show_hidden`. Empty sessions never show.
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

/// The session `--last` resumes: the one touched most recently, scoped to
/// the cwd. Activity, not creation — a session opened last week and used
/// this morning is the one to continue. Empty sessions (never sent a
/// message) are skipped.
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

/// Session summaries for headless list output, newest activity first
/// (`/resume` with piped stdout, `gray resume` without a TTY): the picker
/// needs a real terminal, so these print as text instead. Same cwd filter
/// as [`latest_summary`] (`--all` disables it). Empty sessions never show.
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
    // Newest activity first, like the picker: the printed age then reads
    // monotonically instead of jumping around.
    out.sort_by_key(|s| s.last_message_at);
    out.reverse();
    out
}

/// One text row for headless session lists: short id, preview, age. Both the
/// preview and the age describe the session's latest message, not its opener.
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

    // 1. Exact match across all sessions
    if let Some(s) = summaries
        .iter()
        .find(|s| s.id.as_str().to_lowercase() == lower)
    {
        return Some(s.id.clone());
    }

    // 2. Prefix match within CWD first (if not all)
    if !all && let Some(c) = cwd.as_deref() {
        let cwd_matches: Vec<&SessionSummary> = summaries
            .iter()
            .filter(|s| paths_match(&s.cwd, c) && s.id.as_str().to_lowercase().starts_with(&lower))
            .collect();
        if cwd_matches.len() == 1 {
            return Some(cwd_matches[0].id.clone());
        }
        if cwd_matches.len() > 1 {
            // Pick most recent in CWD
            let latest = cwd_matches.into_iter().max_by_key(|s| s.last_message_at);
            if let Some(s) = latest {
                return Some(s.id.clone());
            }
        }
    }

    // 3. Prefix match across ALL sessions in store
    let all_matches: Vec<&SessionSummary> = summaries
        .iter()
        .filter(|s| s.id.as_str().to_lowercase().starts_with(&lower))
        .collect();
    if all_matches.len() == 1 {
        return Some(all_matches[0].id.clone());
    }
    if all_matches.len() > 1 {
        // Pick the most recent session matching this prefix
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

/// Strict explicit-id resolution shared by `gray resume <id>` and `gray -p
/// --session <id>`: prefix/exact match wins, else an exact load is attempted,
/// else the same `no session matching` error both paths report (exit 1).
/// A file `list()` already quarantined surfaces as the quarantined truth
/// (`session corrupt, moved to .corrupt-N`), never masked as NotFound.
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

/// Latest session for `gray -p -c`: most recent in this directory, falling
/// back to the global latest (mirrors the REPL `-c` path, which has no
/// `--all` flag). `Ok(None)` when the store is empty (fresh print run).
/// Recall-first: the remembered-session pointer answers in one file read;
/// the full list scan is the fallback (cold start, pruned pointer).
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

/// Single model priority for every resume path (explicit/config wins over the
/// session's recorded model), so `resume`, `--session`, and `/resume` resolve
/// the same model — and therefore the same context window — for one session.
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

/// Human-readable confirmation for non-TTY `gray resume <id>`: loads the
/// session so the announcement states what was resumed (id + message count).
/// `Err` when the session cannot be loaded (caller exits 1, like `--last`
/// with no sessions — never a silent exit 0).
pub async fn resumed_session_line(
    store: &JsonlSessionStore,
    id: &SessionId,
) -> anyhow::Result<String> {
    let (_, entries) = store
        .load(id)
        .await
        .map_err(|e| anyhow::anyhow!("could not resume session {}: {e}", id.as_str()))?;
    Ok(format!(
        "\u{2b22} Resumed session {} ({} messages)",
        id.as_str(),
        entries.len()
    ))
}

/// Styled stderr card for a `session_locked` refusal — the refusal sibling
/// of [`resumed_session_line`]: same `⬢` sigil, `└` continuation, and the
/// action line dimmed on a real terminal. `main` prints this instead of
/// anyhow's `Error:` dump; contention is routine, not a crash.
pub fn locked_session_card(id: &SessionId, pid: Option<u32>) -> String {
    use std::io::IsTerminal as _;
    let notice = crate::session_store::SessionError::locked_notice(id, pid);
    let mut lines = notice.lines();
    let head = lines.next().unwrap_or_default();
    let mut out = format!("\u{2b22} {head}");
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

/// User sessions in `cwd` (any directory when `None`) that read as cut off.
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

/// Startup nudge: how many user sessions in `cwd` were cut off, and how to
/// clear them. `None` when there are none, so a clean directory starts with
/// no extra line.
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

/// `/sessions dismiss`: marks every interrupted user session in `cwd` (all
/// directories when `None`) as left alone on purpose. Returns how many.
pub(crate) async fn dismiss_interrupted(cwd: Option<&Path>) -> usize {
    let Some(root) = crate::session_store::default_root() else {
        return 0;
    };
    let store = JsonlSessionStore::new(root.clone());
    let summaries = store.list().await;
    let mut running = std::collections::HashSet::new();
    for s in &summaries {
        if store.open_lock_held(&s.id).await {
            running.insert(s.id.as_str().to_string());
        }
    }
    interrupted_sessions(&summaries, &running, cwd)
        .into_iter()
        .filter(|s| crate::session_store::write_dismissal(&root, &s.id, s.last_message_at).is_ok())
        .count()
}

/// Reads the store and builds [`interrupted_hint`] for the startup screen.
pub(crate) async fn startup_interrupted_hint(cwd: &std::path::Path) -> Option<String> {
    let store = JsonlSessionStore::new(crate::session_store::default_root()?);
    let summaries = store.list().await;
    let mut running = std::collections::HashSet::new();
    for s in summaries.iter().filter(|s| s.cwd == cwd) {
        if store.open_lock_held(&s.id).await {
            running.insert(s.id.as_str().to_string());
        }
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    interrupted_hint(&summaries, &running, cwd, home.as_deref())
}

pub async fn run_resume_picker(
    show_all: bool,
    bg: Option<&crate::setup::BackgroundSnapshot>,
) -> anyhow::Result<Option<SessionId>> {
    let root = crate::session_store::default_root()
        .ok_or_else(|| anyhow::anyhow!("cannot resolve home"))?;
    let store = JsonlSessionStore::new(root);
    // `list()` already orders by last activity; sort again so the picker's
    // order does not depend on that internal detail.
    let mut summaries = store.list().await;
    summaries.retain(|s| !is_empty_session(s));
    summaries.sort_by_key(|s| s.last_message_at);
    summaries.reverse();
    if summaries.is_empty() {
        anyhow::bail!("no saved sessions");
    }
    // Which sessions a live process holds right now (`<id>.open` flock).
    let mut running = std::collections::HashSet::new();
    for s in &summaries {
        if store.open_lock_held(&s.id).await {
            running.insert(s.id.as_str().to_string());
        }
    }
    run_picker_sync(summaries, show_all, running, bg)
}

fn run_picker_sync(
    mut summaries: Vec<SessionSummary>,
    mut show_all: bool,
    running: std::collections::HashSet<String>,
    bg: Option<&crate::setup::BackgroundSnapshot>,
) -> anyhow::Result<Option<SessionId>> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
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

    let mut query = String::new();
    let mut show_hidden = false;
    let mut state_filter = StateFilter::All;
    let mut sel: usize = 0;
    let mut scroll_top: usize = 0;
    // `ctrl-r`: the row being renamed and the title typed so far.
    let mut rename: Option<(SessionId, String)> = None;

    let result: anyhow::Result<Option<SessionId>> = (|| {
        loop {
            let cwd_filter: Option<&Path> = if show_all { None } else { Some(&cwd) };
            let filtered = visible_sessions(
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

            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 30 || area.height < 8 {
                    return;
                }
                crate::setup::render_dimmed_background(frame, &bg_snapshot);

                let modal_w = 84.min(area.width.saturating_sub(2)).max(40).min(area.width);
                // Height hugs the filtered list: 8 rows of chrome (title,
                // search, filter, gap, spacer, footer) + one row per session.
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
                let pad_len = (inner.width as usize)
                    .saturating_sub(title.chars().count() + esc_str.chars().count() + 8);
                let header = Line::from(vec![
                    Span::styled(
                        title,
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                            .bg(box_bg),
                    ),
                    Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                    Span::styled(
                        "tab: all/cwd  shift-tab: status  ctrl-g: system  ",
                        Style::default().fg(text_dim).bg(box_bg),
                    ),
                    Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                ]);
                frame.render_widget(
                    Paragraph::new(header),
                    Rect::new(inner.x, inner.y, inner.width, 1),
                );

                let search_line = if let Some((_, text)) = &rename {
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
                                .fg(Color::White)
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
                                .fg(Color::White)
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
                            Style::default().fg(Color::White).bg(box_bg),
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
                // marker column: 1 glyph + 1 space
                let prev_w = (inner.width as usize)
                    .saturating_sub(1 + 2 + date_w + 2 + cwd_w + 2 + id_w + 2)
                    .max(12);

                let list_y = inner.y + 4;
                let list_h = inner.height.saturating_sub(6) as usize;

                if filtered.is_empty() {
                    let msg = if summaries.is_empty() {
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
                        let s = filtered[idx];
                        let is_sel = idx == sel;
                        // When the latest message was sent, matching the
                        // preview beside it — not when the session opened.
                        let date = format_relative(s.last_message_at);
                        let cwd_s = cwd_display(&s.cwd, cwd_w);
                        let prev = preview_text(s, prev_w);
                        let st = session_state(s, &running);
                        let (marker, marker_fg) = match st {
                            SessionState::Interrupted => ("\u{26a0}", accent_peach),
                            SessionState::Running => ("\u{25cf}", Color::Green),
                            SessionState::Done => (" ", Color::White),
                        };
                        let right = match system_tag(s) {
                            Some(tag) if show_hidden => tag,
                            _ if state_filter == StateFilter::Interrupted => {
                                end_label(s.last_end).to_string()
                            }
                            _ => short_id(&s.id),
                        };
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
                            let base = Style::default().fg(Color::White).bg(box_bg);
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

                let hints: &[(&str, &str)] = if rename.is_some() {
                    &[("enter", "save"), ("esc", "cancel")]
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
                            .fg(Color::White)
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
            match read()? {
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
                    // Rename the highlighted session (prefilled with its title).
                    KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {
                        let cwd_filter: Option<&Path> = if show_all { None } else { Some(&cwd) };
                        rename = visible_sessions(
                            &summaries,
                            &query,
                            cwd_filter,
                            show_hidden,
                            state_filter,
                            &running,
                        )
                        .get(sel)
                        .map(|s| (s.id.clone(), s.title.clone().unwrap_or_default()));
                    }
                    // Leave an interrupted session as it is on purpose: stop
                    // flagging it (until it sees new activity).
                    KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {
                        let cwd_filter: Option<&Path> = if show_all { None } else { Some(&cwd) };
                        let target = visible_sessions(
                            &summaries,
                            &query,
                            cwd_filter,
                            show_hidden,
                            state_filter,
                            &running,
                        )
                        .get(sel)
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
                    KeyCode::Up => sel = sel.saturating_sub(1),
                    KeyCode::Down => {
                        let cwd_filter: Option<&Path> = if show_all { None } else { Some(&cwd) };
                        let count = visible_sessions(
                            &summaries,
                            &query,
                            cwd_filter,
                            show_hidden,
                            state_filter,
                            &running,
                        )
                        .len();
                        if count > 0 {
                            sel = (sel + 1).min(count - 1);
                        }
                    }
                    KeyCode::Enter => {
                        let cwd_filter: Option<&Path> = if show_all { None } else { Some(&cwd) };
                        let filtered = visible_sessions(
                            &summaries,
                            &query,
                            cwd_filter,
                            show_hidden,
                            state_filter,
                            &running,
                        );
                        if let Some(s) = filtered.get(sel) {
                            return Ok(Some(s.id.clone()));
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

#[path = "resume_tests.rs"]
#[cfg(test)]
mod tests;
