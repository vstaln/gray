use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use gray_session::{JsonlSessionStore, SessionId, SessionSummary, SessionStore};

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

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
    let secs = ts / 1000;
    let days_since_epoch = secs / 86400;
    let mut y = 1970i32;
    let mut d = days_since_epoch as i32;
    loop {
        let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
        let diy = if leap { 366 } else { 365 };
        if d < diy {
            break;
        }
        d -= diy;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let month_lens = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 1;
    for ml in month_lens {
        if d < ml {
            break;
        }
        d -= ml;
        m += 1;
    }
    format!("{y:04}-{m:02}-{:02}", d + 1)
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
        let mut t = short.chars().skip(short.chars().count() - width + 1).collect::<String>();
        t.insert(0, '…');
        t
    }
}

fn preview_text(s: &SessionSummary, width: usize) -> String {
    let raw = s.first_user_text.as_deref().unwrap_or("(no message yet)");
    let one_line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= width {
        one_line
    } else {
        let mut t: String = one_line.chars().take(width - 1).collect();
        t.push('…');
        t
    }
}

fn short_id(id: &SessionId) -> String {
    let s = id.as_str();
    s.split('-').next().unwrap_or(s).to_string()
}

fn paths_match(a: &Path, b: &Path) -> bool {
    let ca = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let cb = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    ca == cb
}

pub fn latest_summary<'a>(summaries: &'a [SessionSummary], cwd_filter: Option<&Path>) -> Option<&'a SessionSummary> {
    let mut filtered: Vec<&SessionSummary> = summaries
        .iter()
        .filter(|s| {
            if let Some(cwd) = cwd_filter {
                paths_match(&s.cwd, cwd)
            } else {
                true
            }
        })
        .collect();
    filtered.sort_by_key(|s| s.started_at);
    filtered.into_iter().last()
}

pub async fn resolve_prefix(store: &JsonlSessionStore, input: &str, all: bool) -> Option<SessionId> {
    let cwd = std::env::current_dir().ok();
    let summaries = store.list().await;
    let lower = input.trim().to_lowercase();
    if lower.is_empty() {
        return None;
    }

    // 1. Exact match across all sessions
    if let Some(s) = summaries.iter().find(|s| s.id.as_str().to_lowercase() == lower) {
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
            let latest = cwd_matches.into_iter().max_by_key(|s| s.started_at);
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
        let latest = all_matches.into_iter().max_by_key(|s| s.started_at);
        if let Some(s) = latest {
            return Some(s.id.clone());
        }
    }

    None
}

pub fn resume_command_hint(id: &SessionId) -> String {
    format!("gray resume {}", id.as_str())
}

pub async fn run_resume_picker(show_all: bool, bg: Option<&crate::setup::BackgroundSnapshot>) -> anyhow::Result<Option<SessionId>> {
    let root = gray_session::default_root().ok_or_else(|| anyhow::anyhow!("cannot resolve home"))?;
    let store = JsonlSessionStore::new(root);
    let mut summaries = store.list().await;
    summaries.sort_by_key(|s| s.started_at);
    summaries.reverse();
    if summaries.is_empty() {
        return Ok(None);
    }
    run_picker_sync(summaries, show_all, bg)
}

fn run_picker_sync(
    summaries: Vec<SessionSummary>,
    mut show_all: bool,
    bg: Option<&crate::setup::BackgroundSnapshot>,
) -> anyhow::Result<Option<SessionId>> {
    use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use ratatui::Terminal;
    use std::io::Write as _;
    use std::time::Duration;

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let bg_snapshot = bg.cloned().unwrap_or_else(crate::setup::BackgroundSnapshot::default_initial);

    enable_raw_mode()?;
    let mut stdout_handle = std::io::stdout();
    crossterm::execute!(stdout_handle, EnterAlternateScreen, crossterm::cursor::Hide)?;
    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = Terminal::new(backend)?;

    let box_bg = Color::Rgb(22, 22, 22);
    let accent_peach = Color::Rgb(246, 173, 126);
    let text_dim = Color::Rgb(120, 120, 120);

    let mut query = String::new();
    let mut sel: usize = 0;
    let mut scroll_top: usize = 0;

    let result: anyhow::Result<Option<SessionId>> = (|| {
        loop {
            let cwd_filter: Option<&Path> = if show_all { None } else { Some(&cwd) };
            let filtered: Vec<&SessionSummary> = summaries
                .iter()
                .filter(|s| {
                    if let Some(f) = cwd_filter {
                        paths_match(&s.cwd, f)
                    } else {
                        true
                    }
                })
                .filter(|s| {
                    if query.is_empty() {
                        true
                    } else {
                        let q = query.to_lowercase();
                        s.id.as_str().to_lowercase().contains(&q)
                            || s.cwd.display().to_string().to_lowercase().contains(&q)
                            || s.first_user_text
                                .as_deref()
                                .unwrap_or("")
                                .to_lowercase()
                                .contains(&q)
                    }
                })
                .collect();

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
                let modal_h = 20.min(area.height.saturating_sub(2)).max(12).min(area.height);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);

                frame.render_widget(Clear, modal_rect);
                let box_block = Block::default().style(Style::default().bg(box_bg));
                frame.render_widget(box_block, modal_rect);

                let pad_x = 2u16;
                let inner_w = modal_w.saturating_sub(pad_x * 2);
                let inner = Rect::new(modal_x + pad_x, modal_y + 1, inner_w, modal_h.saturating_sub(2));

                let title = if show_all { "Resume session — all" } else { "Resume session" };
                let esc_str = "esc";
                let pad_len = (inner.width as usize).saturating_sub(title.chars().count() + esc_str.chars().count() + 8);
                let header = Line::from(vec![
                    Span::styled(title, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                    Span::styled("tab: all/cwd  ", Style::default().fg(text_dim).bg(box_bg)),
                    Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                ]);
                frame.render_widget(Paragraph::new(header), Rect::new(inner.x, inner.y, inner.width, 1));

                let search_line = if query.is_empty() {
                    Line::from(vec![
                        Span::styled("Search: ", Style::default().fg(accent_peach).add_modifier(Modifier::BOLD).bg(box_bg)),
                        Span::styled("Type to filter…", Style::default().fg(Color::Rgb(90, 90, 90)).bg(box_bg)),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled("Search: ", Style::default().fg(accent_peach).add_modifier(Modifier::BOLD).bg(box_bg)),
                        Span::styled(query.clone(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                        Span::styled("▎", Style::default().fg(accent_peach).bg(box_bg)),
                    ])
                };
                frame.render_widget(Paragraph::new(search_line), Rect::new(inner.x, inner.y + 1, inner.width, 1));

                let filter_line = if show_all {
                    Line::from(Span::styled("Showing all sessions", Style::default().fg(text_dim).bg(box_bg)))
                } else {
                    Line::from(vec![
                        Span::styled("Filtered to ", Style::default().fg(text_dim).bg(box_bg)),
                        Span::styled(cwd_display(&cwd, 40), Style::default().fg(Color::White).bg(box_bg)),
                        Span::styled("  (tab to show all)", Style::default().fg(text_dim).bg(box_bg)),
                    ])
                };
                frame.render_widget(Paragraph::new(filter_line), Rect::new(inner.x, inner.y + 2, inner.width, 1));

                let date_w = 9usize;
                let cwd_w = 14usize;
                let id_w = 8usize;
                let prev_w = (inner.width as usize).saturating_sub(1 + date_w + 2 + cwd_w + 2 + id_w + 2).max(12);

                let list_y = inner.y + 4;
                let list_h = inner.height.saturating_sub(6) as usize;

                if filtered.is_empty() {
                    let msg = if summaries.is_empty() {
                        "No saved sessions yet"
                    } else if query.is_empty() {
                        "No sessions in this directory — press Tab to show all"
                    } else {
                        "No matching sessions"
                    };
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled(format!("  {msg}"), Style::default().fg(text_dim).bg(box_bg)))),
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
                        let date = format_relative(s.started_at);
                        let cwd_s = cwd_display(&s.cwd, cwd_w);
                        let prev = preview_text(s, prev_w);
                        let sid = short_id(&s.id);
                        let content = format!(
                            " {:>date_w$}  {:cwd_w$}  {:prev_w$}  {:>id_w$}",
                            date,
                            cwd_s,
                            prev,
                            sid,
                            date_w = date_w,
                            cwd_w = cwd_w,
                            prev_w = prev_w,
                            id_w = id_w
                        );
                        let fill = (inner.width as usize).saturating_sub(content.chars().count());
                        let row_str = format!("{}{}", content, " ".repeat(fill));
                        let line = if is_sel {
                            Line::from(Span::styled(row_str, Style::default().fg(Color::Black).bg(accent_peach).add_modifier(Modifier::BOLD)))
                        } else {
                            Line::from(Span::styled(row_str, Style::default().fg(Color::White).bg(box_bg)))
                        };
                        frame.render_widget(Paragraph::new(line), Rect::new(inner.x, list_y + r as u16, inner.width, 1));
                    }
                }

                let footer = Line::from(vec![
                    Span::styled("↑↓ ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled("navigate  ", Style::default().fg(text_dim).bg(box_bg)),
                    Span::styled("enter ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled("resume  ", Style::default().fg(text_dim).bg(box_bg)),
                    Span::styled("tab ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled("toggle  ", Style::default().fg(text_dim).bg(box_bg)),
                    Span::styled("esc ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled("cancel", Style::default().fg(text_dim).bg(box_bg)),
                ]);
                frame.render_widget(Paragraph::new(footer), Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1));
            })?;

            if !poll(Duration::from_millis(80))? {
                continue;
            }
            match read()? {
                Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. })
                    if modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    return Ok(None);
                }
                Event::Key(KeyEvent { code, modifiers, kind: KeyEventKind::Press, .. }) => match code {
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
                    KeyCode::Up => sel = sel.saturating_sub(1),
                    KeyCode::Down => {
                        let cwd_filter: Option<&Path> = if show_all { None } else { Some(&cwd) };
                        let count = summaries
                            .iter()
                            .filter(|s| if let Some(f) = cwd_filter { paths_match(&s.cwd, f) } else { true })
                            .filter(|s| {
                                if query.is_empty() {
                                    true
                                } else {
                                    let q = query.to_lowercase();
                                    s.id.as_str().to_lowercase().contains(&q)
                                        || s.cwd.display().to_string().to_lowercase().contains(&q)
                                        || s.first_user_text.as_deref().unwrap_or("").to_lowercase().contains(&q)
                                }
                            })
                            .count();
                        if count > 0 {
                            sel = (sel + 1).min(count - 1);
                        }
                    }
                    KeyCode::Enter => {
                        let cwd_filter: Option<&Path> = if show_all { None } else { Some(&cwd) };
                        let filtered: Vec<&SessionSummary> = summaries
                            .iter()
                            .filter(|s| if let Some(f) = cwd_filter { paths_match(&s.cwd, f) } else { true })
                            .filter(|s| {
                                if query.is_empty() {
                                    true
                                } else {
                                    let q = query.to_lowercase();
                                    s.id.as_str().to_lowercase().contains(&q)
                                        || s.cwd.display().to_string().to_lowercase().contains(&q)
                                        || s.first_user_text.as_deref().unwrap_or("").to_lowercase().contains(&q)
                                }
                            })
                            .collect();
                        if let Some(s) = filtered.get(sel) {
                            return Ok(Some(s.id.clone()));
                        }
                    }
                    KeyCode::Backspace => {
                        query.pop();
                        sel = 0;
                    }
                    KeyCode::Char(ch) if !modifiers.contains(KeyModifiers::CONTROL) && !modifiers.contains(KeyModifiers::ALT) => {
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
    disable_raw_mode()?;
    crossterm::execute!(
        std::io::stdout(),
        LeaveAlternateScreen,
        crossterm::cursor::Show,
    )?;
    let _ = std::io::stdout().flush();
    result
}
