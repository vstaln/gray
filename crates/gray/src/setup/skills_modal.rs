//! Skills picker modal (split from `setup`).

use super::*;

/// Interactive skills picker — searchable list like /resume, peach highlight.
/// Returns the selected skill plus optional args (text after first space in query).
/// Bare `/skills` opens this; `/skills:name` bypasses it via expand_skill_command.
pub fn run_skills_modal(
    cwd: &std::path::Path,
    bg: Option<&BackgroundSnapshot>,
) -> anyhow::Result<Option<(crate::skills::Skill, String)>> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
    use crossterm::terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    };
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use std::io::Write as _;
    use std::time::Duration;

    let discovered = crate::skills::discover_skills(cwd);
    let skills = discovered.skills;
    if skills.is_empty() {
        return Ok(None);
    }

    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !was_raw {
        enable_raw_mode()?;
    }
    let mut stdout_handle = std::io::stdout();
    crossterm::execute!(
        stdout_handle,
        EnterAlternateScreen,
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
        crossterm::cursor::Hide
    )?;
    let _ = crossterm::terminal::size();
    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = Terminal::new(backend)?;

    let box_bg = Color::Rgb(22, 22, 22);
    let accent_peach = Color::Rgb(246, 173, 126);
    let text_dim = Color::Rgb(120, 120, 120);

    let mut query = String::new();
    let mut sel = 0usize;
    let mut scroll_top = 0usize;
    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(BackgroundSnapshot::default_initial);

    let result: anyhow::Result<Option<(crate::skills::Skill, String)>> = (|| {
        loop {
            // split query into filter part (before first space) and args part (after)
            let (filter_part, args_part) = if let Some(pos) = query.find(char::is_whitespace) {
                (
                    query[..pos].trim().to_string(),
                    query[pos..].trim().to_string(),
                )
            } else {
                (query.trim().to_string(), String::new())
            };
            let filter_lower = filter_part.to_lowercase();
            let filtered: Vec<&crate::skills::Skill> = skills
                .iter()
                .filter(|s| {
                    if filter_lower.is_empty() {
                        true
                    } else {
                        s.name.to_lowercase().contains(&filter_lower)
                            || s.description.to_lowercase().contains(&filter_lower)
                    }
                })
                .collect();

            if sel >= filtered.len() && !filtered.is_empty() {
                sel = filtered.len() - 1;
            }
            if filtered.is_empty() {
                sel = 0;
            }
            if sel < scroll_top {
                scroll_top = sel;
            }

            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 30 || area.height < 8 {
                    return;
                }
                render_dimmed_background(frame, &bg_snapshot);
                let modal_w = 78.min(area.width.saturating_sub(2)).max(44).min(area.width);
                let modal_h = 20
                    .min(area.height.saturating_sub(2))
                    .max(12)
                    .min(area.height);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);
                frame.render_widget(Clear, modal_rect);
                let box_block = Block::default().style(Style::default().bg(box_bg));
                frame.render_widget(box_block, modal_rect);
                let pad_x = 3u16;
                let inner_w = modal_w.saturating_sub(pad_x * 2);
                let inner = Rect::new(
                    modal_x + pad_x,
                    modal_y + 1,
                    inner_w,
                    modal_h.saturating_sub(2),
                );
                let title = format!("Skills — {} available", skills.len());
                let esc_str = "esc";
                let pad_len = (inner.width as usize)
                    .saturating_sub(title.chars().count() + esc_str.chars().count());
                let header = Line::from(vec![
                    Span::styled(
                        title,
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                            .bg(box_bg),
                    ),
                    Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                    Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                ]);
                frame.render_widget(
                    Paragraph::new(header),
                    Rect::new(inner.x, inner.y, inner.width, 1),
                );
                let search_line = if query.is_empty() {
                    Line::from(vec![
                        Span::styled(
                            "Search: ",
                            Style::default()
                                .fg(accent_peach)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled(
                            "Type to filter — select to run (args after space)",
                            Style::default().fg(Color::Rgb(90, 90, 90)).bg(box_bg),
                        ),
                    ])
                } else {
                    let args_hint = if !args_part.is_empty() {
                        format!("  · args: {args_part}")
                    } else {
                        String::new()
                    };
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
                        Span::styled(
                            args_hint,
                            Style::default().fg(Color::Rgb(100, 100, 100)).bg(box_bg),
                        ),
                    ])
                };
                frame.render_widget(
                    Paragraph::new(search_line),
                    Rect::new(inner.x, inner.y + 1, inner.width, 1),
                );
                let list_y = inner.y + 3;
                let list_h = inner.height.saturating_sub(4) as usize;
                if filtered.is_empty() {
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled(
                            "  No matching skills",
                            Style::default().fg(text_dim).bg(box_bg),
                        ))),
                        Rect::new(inner.x, list_y, inner.width, 1),
                    );
                } else {
                    let visible = list_h.max(1);
                    if sel >= scroll_top + visible {
                        scroll_top = sel + 1 - visible;
                    }
                    for r in 0..visible {
                        let idx = scroll_top + r;
                        if idx >= filtered.len() {
                            break;
                        }
                        let s = filtered[idx];
                        let is_sel = idx == sel;
                        // truncate description to fit
                        let name_w = 22usize;
                        let desc_avail = (inner.width as usize)
                            .saturating_sub(4 + name_w + 3)
                            .max(10);
                        let desc = if s.description.chars().count() > desc_avail {
                            let mut t: String =
                                s.description.chars().take(desc_avail - 1).collect();
                            t.push('…');
                            t
                        } else {
                            s.description.clone()
                        };
                        let raw = format!(
                            " {name:<name_w$}  {desc}",
                            name = s.name,
                            desc = desc,
                            name_w = name_w
                        );
                        let fill = (inner.width as usize).saturating_sub(raw.chars().count());
                        let row_str = format!("{}{}", raw, " ".repeat(fill));
                        let line = if is_sel {
                            Line::from(Span::styled(
                                row_str,
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(accent_peach)
                                    .add_modifier(Modifier::BOLD),
                            ))
                        } else {
                            let name_span = Span::styled(
                                format!(" {name:<name_w$}  ", name = s.name, name_w = name_w),
                                Style::default()
                                    .fg(Color::White)
                                    .add_modifier(Modifier::BOLD)
                                    .bg(box_bg),
                            );
                            let desc_span = Span::styled(
                                desc,
                                Style::default().fg(Color::Rgb(140, 140, 140)).bg(box_bg),
                            );
                            let pad_span =
                                Span::styled(" ".repeat(fill), Style::default().bg(box_bg));
                            Line::from(vec![name_span, desc_span, pad_span])
                        };
                        frame.render_widget(
                            Paragraph::new(line),
                            Rect::new(inner.x, list_y + r as u16, inner.width, 1),
                        );
                    }
                }
                let footer = Line::from(vec![
                    Span::styled(
                        "↑↓ ",
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                            .bg(box_bg),
                    ),
                    Span::styled("navigate  ", Style::default().fg(text_dim).bg(box_bg)),
                    Span::styled(
                        "enter ",
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                            .bg(box_bg),
                    ),
                    Span::styled("run  ", Style::default().fg(text_dim).bg(box_bg)),
                    Span::styled(
                        "esc ",
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                            .bg(box_bg),
                    ),
                    Span::styled("cancel", Style::default().fg(text_dim).bg(box_bg)),
                ]);
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
                }) if modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
                Event::Key(KeyEvent {
                    code,
                    modifiers,
                    kind: KeyEventKind::Press,
                    ..
                }) if modifiers.contains(KeyModifiers::CONTROL) => match code {
                    KeyCode::Char('p') => sel = sel.saturating_sub(1),
                    KeyCode::Char('n') => {
                        let count = filtered.len();
                        if count > 0 {
                            sel = (sel + 1).min(count - 1);
                        }
                    }
                    _ => {}
                },
                Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Press,
                    ..
                }) => match code {
                    KeyCode::Esc => return Ok(None),
                    KeyCode::Up => sel = sel.saturating_sub(1),
                    KeyCode::Down => {
                        let c = filtered.len();
                        if c > 0 {
                            sel = (sel + 1).min(c - 1);
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(s) = filtered.get(sel) {
                            return Ok(Some(((*s).clone(), args_part.clone())));
                        }
                    }
                    KeyCode::Backspace => {
                        query.pop();
                        sel = 0;
                    }
                    KeyCode::Char(ch) => {
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
    let _ = crossterm::execute!(
        std::io::stdout(),
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );
    if !was_raw {
        let _ = disable_raw_mode();
    } else {
        let _ = enable_raw_mode();
    }
    let _ = std::io::stdout().flush();
    result
}
