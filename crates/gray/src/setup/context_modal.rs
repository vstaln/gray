use super::*;

/// How the /context modal closed.
pub enum ContextModalExit {
    /// Closed; `Some` summarizes what changed.
    Closed(Option<String>),
    /// Enter on the Compaction row: the caller runs the model picker (a
    /// separate modal session) and reopens /context.
    PickCompaction(Option<String>),
}

/// The Compaction row's value: the configured model, or the chat model.
pub fn compaction_label(config: &Config) -> String {
    config
        .compaction_model
        .clone()
        .unwrap_or_else(|| "same as chat model".to_string())
}

pub fn run_context_modal(
    config: &mut Config,
    parts: &ContextParts,
    model: &str,
    bg: Option<&BackgroundSnapshot>,
) -> anyhow::Result<ContextModalExit> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
    use ratatui::layout::Rect;
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use std::time::Duration;

    fn persist(cfg: &Config) {
        if let Ok(path) = saved_config_path() {
            let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
            let mut saved = load_saved_config_at(&path);
            saved.context_window = cfg.context_window;
            saved.context_reserve = cfg.context_reserve;
            saved.context_keep = cfg.context_keep;
            let _ = save_saved_config_at(&path, &saved);
        }
    }

    fn compute_diffs(
        cfg: &Config,
        init_win: Option<usize>,
        init_res: Option<usize>,
        init_kp: Option<usize>,
    ) -> Option<String> {
        let mut diffs = Vec::new();
        if cfg.context_window != init_win {
            let s = cfg
                .context_window
                .map(format_context_length)
                .unwrap_or_else(|| "auto".to_string());
            diffs.push(format!("window → {s}"));
        }
        if cfg.context_reserve != init_res {
            let s = cfg
                .context_reserve
                .map(format_context_length)
                .unwrap_or_else(|| "auto".to_string());
            diffs.push(format!("reserve → {s}"));
        }
        if cfg.context_keep != init_kp {
            let s = cfg
                .context_keep
                .map(|k| {
                    if k == 0 {
                        "off".to_string()
                    } else {
                        format_context_length(k)
                    }
                })
                .unwrap_or_else(|| "auto".to_string());
            diffs.push(format!("keep → {s}"));
        }
        if diffs.is_empty() {
            None
        } else {
            Some(diffs.join(" · "))
        }
    }

    let (_session, mut terminal) = super::open_modal()?;

    let box_bg = crate::theme::theme().surface_bg;
    let accent_peach = crate::theme::theme().accent_soft;
    let text_dim = crate::theme::theme().text_muted;

    let c_sys = crate::theme::theme().ctx_system;
    let c_ctx = crate::theme::theme().ctx_context;
    let c_tools = crate::theme::theme().ctx_tools;
    let c_skills = crate::theme::theme().ctx_skills;
    let c_msgs = crate::theme::theme().ctx_messages;
    let c_free = crate::theme::theme().ctx_free;
    let c_reserve = crate::theme::theme().rose;

    let initial_window = config.context_window;
    let initial_reserve = config.context_reserve;
    let initial_keep = config.context_keep;

    let mut sel = 0usize;
    let mut editing: Option<usize> = None;
    let mut buf = String::new();
    let mut status: String = String::new();
    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(BackgroundSnapshot::default_initial);
    let setting_labels = ["Window", "Reserve", "Keep"];

    let result = (|| -> anyhow::Result<ContextModalExit> {
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

            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 20 || area.height < 6 {
                    return;
                }
                render_dimmed_background(frame, &bg_snapshot);
                let modal_w = 76.min(area.width.saturating_sub(2)).max(60).min(area.width);

                let modal_h = 22
                    .min(area.height.saturating_sub(1))
                    .max(20)
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
                                .fg(crate::theme::theme().text_bright)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled(" ".repeat(pad), Style::default().bg(box_bg)),
                        Span::styled(esc, Style::default().fg(text_dim).bg(box_bg)),
                    ])),
                    Rect::new(inner.x, inner.y, inner.width, 1),
                );
                let head = format!(
                    "{} · {} / {} tokens · {pct}% used",
                    if model.is_empty() { "no model" } else { model },
                    format_context_length(used),
                    format_context_length(window),
                );
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        head,
                        Style::default()
                            .fg(crate::theme::theme().text_bright)
                            .bg(box_bg),
                    ))),
                    Rect::new(inner.x, inner.y + 1, inner.width, 1),
                );

                let pct_of = |n: usize| {
                    n.checked_mul(100)
                        .and_then(|v| v.checked_div(window))
                        .unwrap_or(0)
                };

                // Stacked bar: one segment per category, scaled to the modal width.
                let bar_y = inner.y + 3;
                let bar_w = inner.width as usize;
                let colors = [c_sys, c_ctx, c_tools, c_skills, c_msgs, c_free, c_reserve];
                let total: usize = cells.iter().sum::<usize>().max(1);
                let mut spans = Vec::with_capacity(7);
                let (mut cum, mut drawn) = (0usize, 0usize);
                for (kind, n) in cells.iter().enumerate() {
                    cum += n;
                    let end = (cum * bar_w + total / 2) / total;
                    let w = end.saturating_sub(drawn);
                    drawn = end.max(drawn);
                    if w == 0 {
                        continue;
                    }
                    let glyph = match kind {
                        5 => "░",
                        6 => "▒",
                        _ => "█",
                    };
                    spans.push(Span::styled(
                        glyph.repeat(w),
                        Style::default().fg(colors[kind]).bg(box_bg),
                    ));
                }
                let bar = Line::from(spans);
                for dy in 0..2 {
                    frame.render_widget(
                        Paragraph::new(bar.clone()),
                        Rect::new(inner.x, bar_y + dy, inner.width, 1),
                    );
                }

                // Legend: swatch, label, tokens and share in aligned columns.
                let legend = [
                    ("■", c_sys, "System prompt", parts.system_prompt, false),
                    ("■", c_ctx, "Project context", parts.project_context, false),
                    ("■", c_tools, "Tools", parts.tools, false),
                    ("■", c_skills, "Skills", parts.skills, false),
                    ("■", c_msgs, "Messages", parts.messages, false),
                    ("░", c_free, "Free", free, true),
                    ("▒", c_reserve, "Compact buffer", reserve, true),
                ];
                let legend_y = bar_y + 3;
                for (i, (glyph, color, label, n, dim)) in legend.iter().enumerate() {
                    let fg = if *dim {
                        text_dim
                    } else {
                        crate::theme::theme().text_bright
                    };
                    let line = Line::from(vec![
                        Span::styled(format!("{glyph} "), Style::default().fg(*color).bg(box_bg)),
                        Span::styled(format!("{label:<16}"), Style::default().fg(fg).bg(box_bg)),
                        Span::styled(
                            format!("{:>8}", format_context_length(*n)),
                            Style::default().fg(fg).bg(box_bg),
                        ),
                        Span::styled(
                            format!("{:>6}", format!("{}%", pct_of(*n))),
                            Style::default().fg(text_dim).bg(box_bg),
                        ),
                    ]);
                    frame.render_widget(
                        Paragraph::new(line),
                        Rect::new(inner.x, legend_y + i as u16, inner.width, 1),
                    );
                }

                let set_y = legend_y + 8;
                let win_src = context_source(model);
                let set_rows = [
                    format!(
                        "{:<16}{:>8}   {win_src} · max {}",
                        "Window",
                        format_context_length(window),
                        format_context_length(max)
                    ),
                    format!("{:<16}{:>8}", "Reserve", format_context_length(reserve)),
                    format!("{:<16}{:>8}", "Keep", format_context_length(keep)),
                    format!("{:<16}{}", "Compaction", compaction_label(config)),
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
                                    .fg(crate::theme::theme().on_selection)
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
                                    .fg(if i == sel {
                                        crate::theme::theme().text_bright
                                    } else {
                                        text_dim
                                    })
                                    .bg(box_bg),
                            )])),
                            Rect::new(inner.x, y, inner.width, 1),
                        );
                    }
                }

                let foot_y = inner.y + inner.height - 1;
                if let Some(idx) = editing {
                    let line = format!("{} {}: {buf}█", icon("arrow"), setting_labels[idx]);
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled(
                            line,
                            Style::default()
                                .fg(crate::theme::theme().text_bright)
                                .bg(box_bg),
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
            let ev = read()?;
            if editing.is_none()
                && let Event::Key(key) = ev
                && key.kind == KeyEventKind::Press
                && let Some(up) = crate::keymap::select_move(&key)
            {
                sel = if up {
                    sel.saturating_sub(1)
                } else {
                    (sel + 1).min(3)
                };
                continue;
            }
            match ev {
                Event::Key(KeyEvent {
                    code: KeyCode::Char('c'),
                    modifiers,
                    kind: KeyEventKind::Press,
                    ..
                }) if modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(ContextModalExit::Closed(compute_diffs(
                        config,
                        initial_window,
                        initial_reserve,
                        initial_keep,
                    )));
                }
                Event::Key(KeyEvent {
                    modifiers,
                    kind: KeyEventKind::Press,
                    ..
                }) if modifiers.contains(KeyModifiers::CONTROL) => {}
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
                        KeyCode::Esc | KeyCode::Char('q') => {
                            return Ok(ContextModalExit::Closed(compute_diffs(
                                config,
                                initial_window,
                                initial_reserve,
                                initial_keep,
                            )));
                        }
                        KeyCode::Enter if sel == 3 => {
                            return Ok(ContextModalExit::PickCompaction(compute_diffs(
                                config,
                                initial_window,
                                initial_reserve,
                                initial_keep,
                            )));
                        }
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
    result
}
