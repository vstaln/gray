//! Context-window modal (split from `setup`).

use super::*;

/// Interactive `/context` visual: Codex-style 10×10 usage grid + per-category
/// breakdown + editable window/reserve/keep. Mutates `config` (and the global
/// override cells + saved config) on save. Returns true if anything changed.
pub fn run_context_modal(
    config: &mut Config,
    parts: &ContextParts,
    model: &str,
    bg: Option<&BackgroundSnapshot>,
) -> anyhow::Result<bool> {
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

    fn persist(cfg: &Config) {
        if let Ok(path) = saved_config_path() {
            let mut saved = load_saved_config_at(&path);
            saved.context_window = cfg.context_window;
            saved.context_reserve = cfg.context_reserve;
            saved.context_keep = cfg.context_keep;
            let _ = save_saved_config_at(&path, &saved);
        }
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
    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = Terminal::new(backend)?;

    let box_bg = Color::Rgb(22, 22, 22);
    let accent_peach = Color::Rgb(246, 173, 126);
    let text_dim = Color::Rgb(120, 120, 120);
    // jewel-tone category colors (deliberately not Claude's purple/pink)
    let c_sys = Color::Rgb(148, 163, 184); // slate
    let c_ctx = Color::Rgb(45, 212, 191); // teal
    let c_tools = Color::Rgb(56, 189, 248); // sky
    let c_skills = Color::Rgb(163, 230, 53); // lime
    let c_msgs = Color::Rgb(251, 191, 36); // amber
    let c_free = Color::Rgb(90, 90, 90);
    let c_reserve = Color::Rgb(251, 113, 133); // rose

    let mut sel = 0usize;
    let mut editing: Option<usize> = None;
    let mut buf = String::new();
    let mut status: String = String::new();
    let mut changed = false;
    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(BackgroundSnapshot::default_initial);
    let setting_labels = ["Window", "Reserve", "Keep"];

    let result = (|| -> anyhow::Result<bool> {
        loop {
            let window = resolve_model_context_length(model);
            let max = model_max_context(model);
            let reserve = user_reserve_tokens_for(window);
            let keep = user_keep_for(window);
            let used = parts.used();
            let free = parts.free(window, reserve);
            let pct = used
                .checked_mul(100)
                .and_then(|v| v.checked_div(window))
                .unwrap_or(0);
            let cells = parts.grid_cells(window, reserve);
            // cell kind per grid position: 0-4 categories, 5 free, 6 buffer
            let mut flat: Vec<usize> = Vec::with_capacity(100);
            for (kind, n) in cells.iter().enumerate() {
                flat.extend(std::iter::repeat_n(kind, *n));
            }
            while flat.len() < 100 {
                flat.push(5);
            }

            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 40 || area.height < 20 {
                    return;
                }
                render_dimmed_background(frame, &bg_snapshot);
                let modal_w = 76.min(area.width.saturating_sub(2)).max(60).min(area.width);
                let modal_h = 24
                    .min(area.height.saturating_sub(1))
                    .max(22)
                    .min(area.height);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);
                frame.render_widget(Clear, modal_rect);
                frame.render_widget(
                    Block::default().style(Style::default().bg(box_bg)),
                    modal_rect,
                );
                let inner = Rect::new(
                    modal_x + 3,
                    modal_y + 1,
                    modal_w.saturating_sub(6),
                    modal_h.saturating_sub(2),
                );

                let title = "Context Usage";
                let esc = "esc";
                let pad = (inner.width as usize)
                    .saturating_sub(title.chars().count() + esc.chars().count());
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(
                            title,
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled(" ".repeat(pad), Style::default().bg(box_bg)),
                        Span::styled(esc, Style::default().fg(text_dim).bg(box_bg)),
                    ])),
                    Rect::new(inner.x, inner.y, inner.width, 1),
                );
                let head = format!(
                    "{} · {}/{} tokens ({pct}%)",
                    if model.is_empty() { "no model" } else { model },
                    format_context_length(used),
                    format_context_length(window),
                );
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        head,
                        Style::default().fg(Color::White).bg(box_bg),
                    ))),
                    Rect::new(inner.x, inner.y + 1, inner.width, 1),
                );

                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "Estimated usage by category",
                        Style::default()
                            .fg(text_dim)
                            .add_modifier(Modifier::ITALIC)
                            .bg(box_bg),
                    ))),
                    Rect::new(inner.x, inner.y + 2, inner.width, 1),
                );

                let grid_y = inner.y + 3;
                let detail_x = inner.x + 24;
                let detail_w = inner.width.saturating_sub(24);
                let pct_of = |n: usize| {
                    n.checked_mul(100)
                        .and_then(|v| v.checked_div(window))
                        .unwrap_or(0)
                };
                let row = |glyph: &str, color: Color, label: &str, n: usize| {
                    Line::from(vec![
                        Span::styled(format!("{glyph} "), Style::default().fg(color).bg(box_bg)),
                        Span::styled(
                            format!(
                                "{label}: {} tokens ({}%)",
                                format_context_length(n),
                                pct_of(n)
                            ),
                            Style::default().fg(Color::White).bg(box_bg),
                        ),
                    ])
                };
                let details = [
                    row(icon("cell"), c_sys, "System prompt", parts.system_prompt),
                    row(
                        icon("cell"),
                        c_ctx,
                        "Project context",
                        parts.project_context,
                    ),
                    row(icon("cell"), c_tools, "System tools", parts.tools),
                    row(icon("cell"), c_skills, "Skills", parts.skills),
                    row(icon("cell"), c_msgs, "Messages", parts.messages),
                    Line::from(vec![
                        Span::styled(
                            format!("{} ", icon("cell_free")),
                            Style::default().fg(c_free).bg(box_bg),
                        ),
                        Span::styled(
                            format!(
                                "Free space: {} ({}%)",
                                format_context_length(free),
                                pct_of(free)
                            ),
                            Style::default().fg(text_dim).bg(box_bg),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled(
                            format!("{} ", icon("cell_buffer")),
                            Style::default().fg(c_reserve).bg(box_bg),
                        ),
                        Span::styled(
                            format!(
                                "Autocompact buffer: {} tokens ({}%)",
                                format_context_length(reserve),
                                pct_of(reserve)
                            ),
                            Style::default().fg(text_dim).bg(box_bg),
                        ),
                    ]),
                ];
                for r in 0..10u16 {
                    let mut spans = Vec::with_capacity(10);
                    for c in 0..10usize {
                        let kind = flat[(r as usize) * 10 + c];
                        let (g, col) = match kind {
                            0 => (icon("cell"), c_sys),
                            1 => (icon("cell"), c_ctx),
                            2 => (icon("cell"), c_tools),
                            3 => (icon("cell"), c_skills),
                            4 => (icon("cell"), c_msgs),
                            5 => (icon("cell_free"), c_free),
                            _ => (icon("cell_buffer"), c_reserve),
                        };
                        spans.push(Span::styled(
                            format!("{g} "),
                            Style::default().fg(col).bg(box_bg),
                        ));
                    }
                    frame.render_widget(
                        Paragraph::new(Line::from(spans)),
                        Rect::new(inner.x, grid_y + r, 20, 1),
                    );
                    if (r as usize) < details.len() && detail_w > 4 {
                        frame.render_widget(
                            Paragraph::new(details[r as usize].clone()),
                            Rect::new(detail_x, grid_y + r, detail_w, 1),
                        );
                    }
                }

                // settings rows
                let set_y = grid_y + 11;
                let win_src = context_source(model);
                let set_rows = [
                    format!(
                        "Window: {} ({}) [{win_src}, max {}]",
                        format_context_length(window),
                        window,
                        format_context_length(max)
                    ),
                    format!("Reserve: {} ({})", format_context_length(reserve), reserve),
                    format!("Keep: {} ({})", format_context_length(keep), keep),
                ];
                for (i, text) in set_rows.iter().enumerate() {
                    if set_y + i as u16 >= inner.y + inner.height - 2 {
                        break;
                    }
                    let y = set_y + i as u16;
                    if i == sel && editing.is_none() {
                        let raw = format!("{} {text}", icon("arrow"));
                        let fill = (inner.width as usize).saturating_sub(raw.chars().count());
                        frame.render_widget(
                            Paragraph::new(Line::from(Span::styled(
                                format!("{raw}{}", " ".repeat(fill)),
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(accent_peach)
                                    .add_modifier(Modifier::BOLD),
                            ))),
                            Rect::new(inner.x, y, inner.width, 1),
                        );
                    } else {
                        frame.render_widget(
                            Paragraph::new(Line::from(vec![Span::styled(
                                format!("  {text}"),
                                Style::default()
                                    .fg(if i == sel { Color::White } else { text_dim })
                                    .bg(box_bg),
                            )])),
                            Rect::new(inner.x, y, inner.width, 1),
                        );
                    }
                }
                // input / footer
                let foot_y = inner.y + inner.height - 1;
                if let Some(idx) = editing {
                    let line = format!("{} {}: {buf}█", icon("arrow"), setting_labels[idx]);
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled(
                            line,
                            Style::default().fg(Color::White).bg(box_bg),
                        ))),
                        Rect::new(inner.x, foot_y - 1, inner.width, 1),
                    );
                    if !status.is_empty() {
                        frame.render_widget(
                            Paragraph::new(Line::from(Span::styled(
                                status.clone(),
                                Style::default().fg(c_reserve).bg(box_bg),
                            ))),
                            Rect::new(inner.x, foot_y, inner.width, 1),
                        );
                    }
                } else {
                    let foot = if status.is_empty() {
                        "↑↓ navigate · enter edit · esc close".to_string()
                    } else {
                        status.clone()
                    };
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled(
                            foot,
                            Style::default().fg(text_dim).bg(box_bg),
                        ))),
                        Rect::new(inner.x, foot_y, inner.width, 1),
                    );
                }
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
                }) if modifiers.contains(KeyModifiers::CONTROL) => return Ok(changed),
                Event::Key(KeyEvent {
                    code,
                    modifiers,
                    kind: KeyEventKind::Press,
                    ..
                }) if modifiers.contains(KeyModifiers::CONTROL) => match code {
                    KeyCode::Char('p') if editing.is_none() => sel = sel.saturating_sub(1),
                    KeyCode::Char('n') if editing.is_none() => sel = (sel + 1).min(2),
                    _ => {}
                },
                Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Press,
                    ..
                }) => {
                    if let Some(idx) = editing {
                        match code {
                            KeyCode::Esc => {
                                editing = None;
                                buf.clear();
                                status.clear();
                            }
                            KeyCode::Enter => {
                                let v = buf.trim().to_lowercase();
                                let saved: Option<usize> = match idx {
                                    // window: auto clears, else parse + clamp to model max
                                    0 => {
                                        if ["auto", "clear", "reset", "0"].contains(&v.as_str()) {
                                            config.context_window = None;
                                            set_user_context_window(None);
                                            Some(0)
                                        } else {
                                            parse_context_window(&v).map(|n| {
                                                let m = model_max_context(model);
                                                let f = n.min(m);
                                                config.context_window = Some(f);
                                                set_user_context_window(Some(f));
                                                f
                                            })
                                        }
                                    }
                                    // reserve: auto clears to default, else parse
                                    1 => {
                                        if ["auto", "clear", "reset", "default"]
                                            .contains(&v.as_str())
                                        {
                                            config.context_reserve = None;
                                            set_user_reserve_tokens(None);
                                            Some(user_reserve_tokens_for(model_max_context(model)))
                                        } else {
                                            parse_context_window(&v).inspect(|&n| {
                                                config.context_reserve = Some(n);
                                                set_user_reserve_tokens(Some(n));
                                            })
                                        }
                                    }
                                    // keep: auto clears, off/0 = summary only, else parse
                                    _ => {
                                        if ["auto", "clear", "reset", "default"]
                                            .contains(&v.as_str())
                                        {
                                            config.context_keep = None;
                                            set_user_keep_recent_tokens(None);
                                            Some(user_keep_for(model_max_context(model)))
                                        } else if v == "off" || v == "0" {
                                            config.context_keep = Some(0);
                                            set_user_keep_recent_tokens(Some(0));
                                            Some(0)
                                        } else {
                                            parse_context_window(&v).inspect(|&n| {
                                                config.context_keep = Some(n);
                                                set_user_keep_recent_tokens(Some(n));
                                            })
                                        }
                                    }
                                };
                                match saved {
                                    Some(_) => {
                                        persist(config);
                                        changed = true;
                                        status = format!("{} updated", setting_labels[idx]);
                                        editing = None;
                                        buf.clear();
                                    }
                                    None => {
                                        status = format!("invalid '{v}' — e.g. 128k, 1m, auto");
                                    }
                                }
                            }
                            KeyCode::Backspace => {
                                buf.pop();
                            }
                            KeyCode::Char(c) if buf.len() < 24 => {
                                buf.push(c);
                            }
                            _ => {}
                        }
                        continue;
                    }
                    match code {
                        KeyCode::Up => sel = sel.saturating_sub(1),
                        KeyCode::Down => sel = (sel + 1).min(2),
                        KeyCode::Esc | KeyCode::Char('q') => return Ok(changed),
                        KeyCode::Enter => {
                            buf = match sel {
                                0 => get_user_context_window()
                                    .map(format_context_length)
                                    .unwrap_or_else(|| "auto".to_string()),
                                1 => format_context_length(user_reserve_tokens_for(window)),
                                _ => {
                                    if user_keep_for(window) == 0 {
                                        "off".to_string()
                                    } else {
                                        format_context_length(user_keep_for(window))
                                    }
                                }
                            };
                            status.clear();
                            editing = Some(sel);
                        }
                        _ => {}
                    }
                }
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
