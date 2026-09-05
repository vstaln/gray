//! Gateway picker modal (split from `setup`).

use super::*;

/// Interactive `/gateway` picker (like `/model`, `/skills`): platforms with
/// enabled state, plus daemon/service actions. Enter on a platform without a
/// saved token opens an inline token input; on a disabled platform with a
/// saved token it re-enables; on an enabled platform it disconnects (token kept).
/// Returns the equivalent command string for the caller to execute.
pub fn run_gateway_modal(
    bg: Option<&BackgroundSnapshot>,
    running: bool,
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
    let status_ok = Color::Rgb(74, 222, 128);

    let mut sel = 0usize;
    // token-input mode: (platform label, buffer)
    let mut input_for: Option<String> = None;
    let mut input_buf = String::new();
    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(BackgroundSnapshot::default_initial);

    let result = (|| -> anyhow::Result<Option<String>> {
        loop {
            let cfg = gray_gateway::config::load_gateway_config();
            let rows = gateway_modal_rows(&cfg, running);
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

                if let Some(plat) = &input_for {
                    // token input mode
                    let title = format!("Gateway — {plat} token");
                    let esc_str = "esc back";
                    let pad_len = (inner.width as usize)
                        .saturating_sub(title.chars().count() + esc_str.chars().count());
                    let header_line = Line::from(vec![
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
                        Paragraph::new(header_line),
                        Rect::new(inner.x, inner.y, inner.width, 1),
                    );
                    let sub_line = Line::from(Span::styled(
                        "paste or type token  •  enter save  •  esc cancel",
                        Style::default().fg(Color::Rgb(100, 100, 100)).bg(box_bg),
                    ));
                    frame.render_widget(
                        Paragraph::new(sub_line),
                        Rect::new(inner.x, inner.y + 1, inner.width, 1),
                    );
                    // Never render the token itself (stream-safe): dots only.
                    let masked = "•".repeat(input_buf.chars().count());
                    let field = format!(" {} {masked}█", icon("arrow"));
                    let field_line = Line::from(Span::styled(
                        field,
                        Style::default().fg(Color::White).bg(box_bg),
                    ));
                    frame.render_widget(
                        Paragraph::new(field_line),
                        Rect::new(inner.x, inner.y + 3, inner.width, 1),
                    );
                } else {
                    let title_str = "Gateway";
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
                        "~/.gray/gateway.yaml  •  messaging gateway",
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
                        let is_platform = cmd.is_empty();
                        let enabled = status == "enabled";
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
                            let status_span = if is_platform {
                                Span::styled(
                                    status.clone(),
                                    Style::default()
                                        .fg(if enabled { status_ok } else { text_dim })
                                        .bg(box_bg),
                                )
                            } else {
                                Span::styled(
                                    status.clone(),
                                    Style::default().fg(text_dim).bg(box_bg),
                                )
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
                }
            })?;
            if !poll(Duration::from_millis(100))? {
                continue;
            }
            let ev = read()?;
            if let Some(plat) = input_for.clone() {
                match ev {
                    Event::Key(KeyEvent {
                        code: KeyCode::Char('c'),
                        modifiers,
                        kind: KeyEventKind::Press,
                        ..
                    }) if modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
                    Event::Paste(pasted) => {
                        // bracketed paste: tokens never contain whitespace —
                        // keep the first run so trailing newlines can't sneak in.
                        if let Some(tok) = pasted.split_whitespace().next() {
                            input_buf.push_str(tok);
                        }
                    }
                    Event::Key(KeyEvent {
                        code,
                        kind: KeyEventKind::Press,
                        ..
                    }) => match code {
                        KeyCode::Esc => {
                            input_for = None;
                            input_buf.clear();
                        }
                        KeyCode::Enter if !input_buf.trim().is_empty() => {
                            let cmd = format!("/gateway connect {plat} {}", input_buf.trim());
                            return Ok(Some(cmd));
                        }
                        KeyCode::Enter => {}
                        KeyCode::Backspace => {
                            input_buf.pop();
                        }
                        KeyCode::Char(c) => input_buf.push(c),
                        _ => {}
                    },
                    _ => {}
                }
                continue;
            }
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
                        let (cmd, label, _) = &rows[sel];
                        if cmd == "__sep" {
                            continue;
                        }
                        if cmd.is_empty() {
                            // platform row: no saved token → token input,
                            // disabled with token → re-enable, enabled → disconnect
                            let Ok(plat) = label.parse::<gray_gateway::config::Platform>() else {
                                continue;
                            };
                            let cfg_now = gray_gateway::config::load_gateway_config();
                            let entry = cfg_now.platforms.get(&plat);
                            let enabled = entry.is_some_and(|p| p.enabled);
                            if enabled {
                                return Ok(Some(format!("/gateway disconnect {plat}")));
                            }
                            if entry
                                .and_then(|p| p.token.as_ref())
                                .is_some_and(|t| !t.is_empty())
                            {
                                return Ok(Some(format!("/gateway enable {plat}")));
                            }
                            input_for = Some(label.clone());
                            input_buf.clear();
                        } else {
                            return Ok(Some(cmd.clone()));
                        }
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
mod gateway_modal_tests {
    use super::*;

    #[test]
    fn gateway_modal_rows_reflect_state() {
        use gray_gateway::config::{GatewayConfig, Platform, PlatformConfig};
        let mut platforms = std::collections::HashMap::new();
        platforms.insert(
            Platform::Telegram,
            PlatformConfig {
                enabled: true,
                token: Some("x".into()),
                ..Default::default()
            },
        );
        let cfg = GatewayConfig {
            platforms,
            ..Default::default()
        };
        let rows = gateway_modal_rows(&cfg, false);
        assert_eq!(
            rows[0],
            (String::new(), "Telegram".into(), "enabled".into())
        );
        assert_eq!(rows[1].2, "disabled — enter token");
        assert!(
            rows.iter()
                .any(|(c, l, _)| c == "/gateway run" && l == "Start gateway")
        );
        let rows = gateway_modal_rows(&cfg, true);
        assert!(
            rows.iter()
                .any(|(c, l, _)| c == "/gateway stop" && l == "Stop gateway")
        );
        assert!(rows.iter().any(|(c, _, _)| c == "/gateway install"));
        assert!(rows.iter().any(|(c, _, _)| c == "/gateway uninstall"));
    }

    #[test]
    fn gateway_modal_rows_show_saved_token() {
        use gray_gateway::config::{GatewayConfig, Platform, PlatformConfig};
        let mut platforms = std::collections::HashMap::new();
        platforms.insert(
            Platform::Discord,
            PlatformConfig {
                enabled: false,
                token: Some("saved".into()),
                ..Default::default()
            },
        );
        let cfg = GatewayConfig {
            platforms,
            ..Default::default()
        };
        let rows = gateway_modal_rows(&cfg, false);
        assert_eq!(
            rows[1],
            (
                String::new(),
                "Discord".into(),
                "disabled — token saved".into()
            )
        );
    }

    #[test]
    fn move_sel_skips_sep() {
        let rows = vec![
            ("".to_string(), "Telegram".to_string(), String::new()),
            ("".to_string(), "Discord".to_string(), String::new()),
            ("".to_string(), "Slack".to_string(), String::new()),
            ("__sep".to_string(), String::new(), String::new()),
            (
                "/gateway run".to_string(),
                "Start gateway".to_string(),
                String::new(),
            ),
        ];
        // Slack -> one Down lands on Start gateway, not the invisible gap.
        assert_eq!(move_sel(&rows, 2, 1), 4);
        // ...and back up in one press.
        assert_eq!(move_sel(&rows, 4, -1), 2);
        assert_eq!(move_sel(&rows, 0, -1), 0);
        assert_eq!(move_sel(&rows, 4, 1), 4);
    }
}
