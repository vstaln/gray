//! Permissions picker modal (split from `setup`).

use super::*;

/// Word-wraps `s` to `width` columns (greedy, splits on spaces). Never
/// returns an empty vec so callers can always advance the cursor.
fn wrap_words(s: &str, width: usize) -> Vec<String> {
    let width = width.max(20);
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        if cur.is_empty() {
            cur.push_str(word);
        } else if cur.chars().count() + 1 + word.chars().count() <= width {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur.push_str(word);
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Interactive `/permissions` picker: Ask for approval / Full Access / Read
/// Only, mirroring codex's preset label + description copy. Returns the
/// chosen mode id, or `None` on cancel.
pub fn run_permissions_modal(
    current: &str,
    bg: Option<&BackgroundSnapshot>,
) -> anyhow::Result<Option<String>> {
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

    let modes = gray_core::approvals::permission_modes();
    let mut sel = modes
        .iter()
        .position(|(id, _, _)| *id == current)
        .unwrap_or(0);
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
    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(BackgroundSnapshot::default_initial);

    let result = (|| -> anyhow::Result<Option<String>> {
        loop {
            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 20 || area.height < 6 {
                    return;
                }
                render_dimmed_background(frame, &bg_snapshot);
                let modal_w = (area.width.saturating_sub(4))
                    .clamp(56, 116)
                    .min(area.width);
                let pad_x = 3u16;
                let inner_w = modal_w.saturating_sub(pad_x * 2);
                // Two-line rows: label on line 1, wrapped description below,
                // so the full text is always visible (no truncation).
                let desc_width =
                    (inner_w as usize).saturating_sub(6).max(20);
                let wrapped: Vec<Vec<String>> = modes
                    .iter()
                    .map(|(_, _, desc)| wrap_words(desc, desc_width))
                    .collect();
                let rows_h: u16 = wrapped
                    .iter()
                    .map(|w| 1 + w.len() as u16)
                    .sum();
                let sel_id = modes.get(sel).map(|(id, _, _)| *id).unwrap_or("");
                let detail_text = match sel_id {
                    gray_core::approvals::MODE_FULL => {
                        "Full Access edits anywhere and uses the network without asking. Exercise caution."
                    }
                    gray_core::approvals::MODE_READ_ONLY => {
                        "Read Only disables all mutating tools. Mutating actions are rejected."
                    }
                    _ => {
                        "Ask for approval prompts before running commands or editing outside files."
                    }
                };
                let detail_lines =
                    wrap_words(detail_text, inner_w as usize).len() as u16;
                // header(1) + gap(1) + rows + gap(1) + detail + gap(1) + footer(1)
                // + 2 for modal top/bottom padding.
                let needed_h = rows_h + detail_lines + 8;
                let modal_h = needed_h
                    .clamp(12, 24)
                    .min(area.height.saturating_sub(2).max(10))
                    .min(area.height);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);
                frame.render_widget(Clear, modal_rect);
                frame.render_widget(
                    Block::default().style(Style::default().bg(box_bg)),
                    modal_rect,
                );
                let inner_h = modal_h.saturating_sub(2);
                let inner = Rect::new(
                    modal_x + pad_x,
                    modal_y + 1,
                    inner_w,
                    inner_h,
                );
                let title_str = "Update Model Permissions";
                let esc_str = "esc";
                let pad_len = (inner.width as usize)
                    .saturating_sub(title_str.chars().count() + esc_str.chars().count());
                let header_line = Line::from(vec![
                    Span::styled(
                        title_str,
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                            .bg(box_bg),
                    ),
                    Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                    Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                ]);
                frame.render_widget(
                    Paragraph::new(header_line),
                    Rect::new(inner.x, inner.y, inner.width, 1),
                );
                let mut cur_y = inner.y + 2;
                let bottom = inner.y + inner_h;
                for (idx, (id, label, _)) in modes.iter().enumerate() {
                    if cur_y >= bottom {
                        break;
                    }
                    let is_selected = idx == sel;
                    let is_current = *id == current;
                    let row_bg = if is_selected { accent_peach } else { box_bg };
                    let check_span = if is_current {
                        Span::styled(
                            " ✓ ",
                            Style::default()
                                .fg(if is_selected {
                                    Color::Rgb(20, 80, 30)
                                } else {
                                    Color::Rgb(74, 222, 128)
                                })
                                .add_modifier(Modifier::BOLD)
                                .bg(row_bg),
                        )
                    } else {
                        Span::styled("   ", Style::default().bg(row_bg))
                    };
                    let name_span = Span::styled(
                        label.to_string(),
                        Style::default()
                            .fg(if is_selected {
                                Color::Black
                            } else {
                                Color::White
                            })
                            .add_modifier(Modifier::BOLD)
                            .bg(row_bg),
                    );
                    let used_w = 3 + label.chars().count();
                    let pad_w = (inner.width as usize).saturating_sub(used_w);
                    let pad_span = Span::styled(" ".repeat(pad_w), Style::default().bg(row_bg));
                    frame.render_widget(
                        Paragraph::new(Line::from(vec![check_span, name_span, pad_span])),
                        Rect::new(inner.x, cur_y, inner.width, 1),
                    );
                    cur_y += 1;
                    let desc_fg = if is_selected {
                        Color::Rgb(60, 50, 50)
                    } else {
                        Color::Rgb(130, 130, 130)
                    };
                    for wline in &wrapped[idx] {
                        if cur_y >= bottom {
                            break;
                        }
                        let text = format!("      {wline}");
                        let fill = (inner.width as usize).saturating_sub(text.chars().count());
                        let dline = Line::from(vec![
                            Span::styled(text, Style::default().fg(desc_fg).bg(row_bg)),
                            Span::styled(" ".repeat(fill), Style::default().bg(row_bg)),
                        ]);
                        frame.render_widget(
                            Paragraph::new(dline),
                            Rect::new(inner.x, cur_y, inner.width, 1),
                        );
                        cur_y += 1;
                    }
                }
                let footer_y = (inner.y + inner_h).saturating_sub(1);
                // Wrapped detail block between the list and the footer.
                let detail_style = if sel_id == gray_core::approvals::MODE_FULL {
                    Style::default().fg(Color::Rgb(220, 120, 120)).bg(box_bg)
                } else {
                    Style::default().fg(text_dim).bg(box_bg)
                };
                for (di, wline) in wrap_words(detail_text, inner_w as usize)
                    .iter()
                    .enumerate()
                {
                    let dy = cur_y + 1 + di as u16;
                    if dy >= footer_y {
                        break;
                    }
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled(wline.clone(), detail_style))),
                        Rect::new(inner.x, dy, inner.width, 1),
                    );
                }
                let footer_line = Line::from(vec![
                    Span::styled(
                        "↑↓ ",
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                            .bg(box_bg),
                    ),
                    Span::styled("navigate    ", Style::default().fg(text_dim).bg(box_bg)),
                    Span::styled(
                        "enter ",
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                            .bg(box_bg),
                    ),
                    Span::styled("select", Style::default().fg(text_dim).bg(box_bg)),
                ]);
                frame.render_widget(
                    Paragraph::new(footer_line),
                    Rect::new(inner.x, footer_y, inner.width, 1),
                );
            })?;
            if !poll(Duration::from_millis(100))? {
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
                    kind: KeyEventKind::Press,
                    ..
                }) => match code {
                    KeyCode::Up | KeyCode::Char('k') => sel = sel.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => {
                        sel = (sel + 1).min(modes.len().saturating_sub(1))
                    }
                    KeyCode::Char('1') if !modes.is_empty() => {
                        return Ok(Some(modes[0].0.to_string()));
                    }
                    KeyCode::Char('2') if modes.len() > 1 => {
                        return Ok(Some(modes[1].0.to_string()));
                    }
                    KeyCode::Char('3') if modes.len() > 2 => {
                        return Ok(Some(modes[2].0.to_string()));
                    }
                    KeyCode::Esc => return Ok(None),
                    KeyCode::Enter => return Ok(Some(modes[sel].0.to_string())),
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

#[cfg(test)]
mod tests {
    use super::wrap_words;

    #[test]
    fn wrap_keeps_lines_within_width_and_lossless() {
        for (desc, width) in [
            (
                "Read and edit files in the workspace, run commands. Approval is required to access the internet or edit other files.",
                40,
            ),
            (
                "Edit files outside the workspace and access the internet without asking for approval. Exercise caution.",
                40,
            ),
            (
                "Read files in the workspace. Approval is required to edit files or access the internet.",
                40,
            ),
        ] {
            let lines = wrap_words(desc, width);
            assert!(lines.len() > 1, "long desc should wrap: {desc}");
            for line in &lines {
                assert!(
                    line.chars().count() <= width,
                    "line too wide ({line:?} > {width})"
                );
            }
            let joined = lines.join(" ");
            let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
            assert_eq!(norm(&joined), norm(desc));
        }
    }
}
