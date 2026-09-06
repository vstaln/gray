//! Permissions picker modal (split from `setup`).

use super::*;

/// Interactive `/permissions` picker: Ask for approval / Full Access / Read
/// Only, mirroring codex's preset label + description copy. Returns the
/// chosen mode id, or `None` on cancel.
pub fn run_permissions_modal(
    current: &str,
    bg: Option<&BackgroundSnapshot>,
) -> anyhow::Result<Option<String>> {
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

    let modes = gray_core::approvals::permission_modes();
    let mut sel = modes.iter().position(|(id, _, _)| *id == current).unwrap_or(0);
    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !was_raw {
        enable_raw_mode()?;
    }
    let mut stdout_handle = std::io::stdout();
    crossterm::execute!(stdout_handle, EnterAlternateScreen, crossterm::terminal::Clear(crossterm::terminal::ClearType::All), crossterm::cursor::Hide)?;
    let _ = crossterm::terminal::size();
    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = Terminal::new(backend)?;

    let box_bg = Color::Rgb(22, 22, 22);
    let accent_peach = Color::Rgb(246, 173, 126);
    let text_dim = Color::Rgb(120, 120, 120);
    let bg_snapshot = bg.cloned().unwrap_or_else(BackgroundSnapshot::default_initial);

    let result = (|| -> anyhow::Result<Option<String>> {
        loop {
            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 20 || area.height < 6 {
                    return;
                }
                render_dimmed_background(frame, &bg_snapshot);
                let modal_w = 72.min(area.width.saturating_sub(4)).max(44).min(area.width);
                let modal_h = (modes.len() as u16 + 7).min(area.height.saturating_sub(2)).max(10).min(area.height);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);
                frame.render_widget(Clear, modal_rect);
                frame.render_widget(Block::default().style(Style::default().bg(box_bg)), modal_rect);
                let pad_x = 3u16;
                let inner_w = modal_w.saturating_sub(pad_x * 2);
                let inner = Rect::new(modal_x + pad_x, modal_y + 1, inner_w, modal_h.saturating_sub(2));
                let title_str = "Update Model Permissions";
                let esc_str = "esc";
                let pad_len = (inner.width as usize).saturating_sub(title_str.chars().count() + esc_str.chars().count());
                let header_line = Line::from(vec![
                    Span::styled(title_str, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                    Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                ]);
                frame.render_widget(Paragraph::new(header_line), Rect::new(inner.x, inner.y, inner.width, 1));
                let list_y = inner.y + 2;
                for (idx, (id, label, desc)) in modes.iter().enumerate() {
                    let is_selected = idx == sel;
                    let is_current = *id == current;
                    let check = if is_current { "✓ " } else { "  " };
                    let raw_content = format!(" {check}{label}");
                    let fill = (inner.width as usize).saturating_sub(raw_content.chars().count());
                    let full_row_str = format!("{}{}", raw_content, " ".repeat(fill));
                    let row_line = if is_selected {
                        Line::from(Span::styled(full_row_str, Style::default().fg(Color::Black).bg(accent_peach).add_modifier(Modifier::BOLD)))
                    } else {
                        let check_span = if is_current {
                            Span::styled(" ✓ ", Style::default().fg(Color::Rgb(74, 222, 128)).add_modifier(Modifier::BOLD).bg(box_bg))
                        } else {
                            Span::styled("   ", Style::default().bg(box_bg))
                        };
                        let name_span = Span::styled(format!("{label}  "), Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg));
                        let desc_span = Span::styled(*desc, Style::default().fg(Color::Rgb(130, 130, 130)).bg(box_bg));
                        let pad_span = Span::styled(" ".repeat(fill), Style::default().bg(box_bg));
                        Line::from(vec![check_span, name_span, desc_span, pad_span])
                    };
                    frame.render_widget(Paragraph::new(row_line), Rect::new(inner.x, list_y + idx as u16, inner.width, 1));
                }
                let list_end = list_y + modes.len() as u16;
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "Full Access edits anywhere and uses the network without asking.",
                        Style::default().fg(Color::Rgb(200, 120, 120)).bg(box_bg),
                    ))),
                    Rect::new(inner.x, list_end, inner.width, 1),
                );
                let footer_line = Line::from(vec![
                    Span::styled("↑↓ ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled("navigate    ", Style::default().fg(text_dim).bg(box_bg)),
                    Span::styled("enter ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled("select", Style::default().fg(text_dim).bg(box_bg)),
                ]);
                frame.render_widget(Paragraph::new(footer_line), Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1));
            })?;
            if !poll(Duration::from_millis(100))? {
                continue;
            }
            match read()? {
                Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. }) if modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
                Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) => match code {
                    KeyCode::Up => sel = sel.saturating_sub(1),
                    KeyCode::Down => sel = (sel + 1).min(modes.len().saturating_sub(1)),
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
    let _ = crossterm::execute!(std::io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
    if !was_raw {
        let _ = disable_raw_mode();
    } else {
        let _ = enable_raw_mode();
    }
    let _ = std::io::stdout().flush();
    result
}
