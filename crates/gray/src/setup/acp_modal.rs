use super::TuiSession;
use super::ui::{BackgroundSnapshot, render_dimmed_background};

fn acp_rows() -> Vec<(String, String, String)> {
    let home = gray_acp::gray_home_dir();
    let mut rows: Vec<(String, String, String)> = Vec::new();
    for spec in gray_acp::all_specs(Some(home.as_path())) {
        let status = if gray_acp::installed(&spec) {
            "installed".to_string()
        } else {
            "not found".to_string()
        };
        let display = if spec.display.is_empty() {
            spec.key.to_string()
        } else {
            spec.display.to_string()
        };
        rows.push((format!("/acp {}", spec.key), display, status));
    }
    rows.push(("__sep".to_string(), String::new(), String::new()));
    rows.push((
        "/acp off".to_string(),
        "gray (native)".to_string(),
        String::new(),
    ));
    rows
}

fn move_sel(rows: &[(String, String, String)], sel: usize, delta: i32) -> usize {
    if rows.is_empty() {
        return 0;
    }
    let max = rows.len() - 1;
    let mut next = (sel as i32 + delta).clamp(0, max as i32) as usize;
    while rows[next].0 == "__sep" {
        let stepped = (next as i32 + delta.signum()).clamp(0, max as i32) as usize;
        if stepped == next {
            break;
        }
        next = stepped;
    }
    next
}

/// Interactive `/acp` picker: installed agents plus a "gray (native)" row.
/// Returns the equivalent command string for the caller to execute.
pub fn run_acp_modal(bg: Option<&BackgroundSnapshot>) -> anyhow::Result<Option<String>> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
    use crossterm::terminal::EnterAlternateScreen;
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use std::time::Duration;

    let _session = TuiSession::acquire()?;
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
    let status_ok = Color::Rgb(74, 222, 128);

    let mut sel = 0usize;
    let rows = acp_rows();
    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(BackgroundSnapshot::default_initial);

    let result = (|| -> anyhow::Result<Option<String>> {
        loop {
            let max_sel = rows.len().saturating_sub(1);
            if sel > max_sel {
                sel = max_sel;
            }
            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 20 || area.height < 6 {
                    return;
                }
                render_dimmed_background(frame, &bg_snapshot);
                let modal_w = 62.min(area.width.saturating_sub(4)).max(40).min(area.width);
                let modal_h = (rows.len() as u16 + 6)
                    .min(area.height.saturating_sub(2))
                    .max(10)
                    .min(area.height);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);
                frame.render_widget(Clear, modal_rect);
                frame.render_widget(
                    Block::default().style(Style::default().bg(box_bg)),
                    modal_rect,
                );
                let pad_x = 3u16;
                let inner_w = modal_w.saturating_sub(pad_x * 2);
                let inner = Rect::new(
                    modal_x + pad_x,
                    modal_y + 1,
                    inner_w,
                    modal_h.saturating_sub(2),
                );

                let title_str = "ACP agents";
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
                let sub_line = Line::from(Span::styled(
                    "~/.gray/acp.json  •  external agents over ACP",
                    Style::default().fg(Color::Rgb(100, 100, 100)).bg(box_bg),
                ));
                frame.render_widget(
                    Paragraph::new(sub_line),
                    Rect::new(inner.x, inner.y + 1, inner.width, 1),
                );
                let list_y = inner.y + 3;
                for (idx, (cmd, label, status)) in rows.iter().enumerate() {
                    let row_y = list_y + idx as u16;
                    if cmd == "__sep" {
                        continue;
                    }
                    let is_selected = idx == sel;
                    let is_agent = !cmd.is_empty() && cmd != "/acp off";
                    let ready = status == "installed";
                    if is_selected {
                        let raw_content = format!(" › {label:<22}  {status}");
                        let fill =
                            (inner.width as usize).saturating_sub(raw_content.chars().count());
                        let full = format!("{raw_content}{}", " ".repeat(fill));
                        frame.render_widget(
                            Paragraph::new(Line::from(Span::styled(
                                full,
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(accent_peach)
                                    .add_modifier(Modifier::BOLD),
                            ))),
                            Rect::new(inner.x, row_y, inner.width, 1),
                        );
                    } else {
                        let name_span = Span::styled(
                            format!("   {label:<22}  "),
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        );
                        let status_span = if is_agent {
                            Span::styled(
                                status.clone(),
                                Style::default()
                                    .fg(if ready { status_ok } else { text_dim })
                                    .bg(box_bg),
                            )
                        } else {
                            Span::styled(status.clone(), Style::default().fg(text_dim).bg(box_bg))
                        };
                        frame.render_widget(
                            Paragraph::new(Line::from(vec![name_span, status_span])),
                            Rect::new(inner.x, row_y, inner.width, 1),
                        );
                    }
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
                    Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
                );
            })?;
            if !poll(Duration::from_millis(100))? {
                continue;
            }
            let ev = read()?;
            match ev {
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
                    KeyCode::Char('p') => sel = move_sel(&rows, sel, -1),
                    KeyCode::Char('n') => sel = move_sel(&rows, sel, 1),
                    _ => {}
                },
                Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Press,
                    ..
                }) => match code {
                    KeyCode::Up => sel = move_sel(&rows, sel, -1),
                    KeyCode::Down => sel = move_sel(&rows, sel, 1),
                    KeyCode::Esc => return Ok(None),
                    KeyCode::Enter => {
                        let (cmd, _, _) = &rows[sel];
                        if cmd == "__sep" {
                            continue;
                        }
                        return Ok(Some(cmd.clone()));
                    }
                    _ => {}
                },
                Event::Resize(_, _) => {
                    let _ = terminal.clear();
                }
                _ => {}
            }
        }
    })();
    let _ = terminal.clear();
    result
}
