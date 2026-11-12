use super::*;

#[derive(Clone)]
pub(crate) struct ManagerItem {
    pub name: String,
    pub row: String,
    pub lit: bool,
    pub read_only: bool,
}

pub(crate) struct ManagerSpec {
    pub title: &'static str,
    pub empty_hint: &'static str,
    pub error_verb: &'static str,
}

pub(crate) fn run_install_manager(
    bg: Option<&BackgroundSnapshot>,
    spec: &ManagerSpec,
    load: impl Fn() -> Vec<ManagerItem>,
    remove: impl Fn(&str) -> anyhow::Result<()>,
) -> anyhow::Result<bool> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Paragraph;
    use std::time::Duration;

    let mut items = load();
    let mut sel = 0usize;
    let mut changed = false;
    let mut op_err: Option<String> = None;
    let mut pending_remove: Option<String> = None;

    let (_session, mut terminal) = super::open_modal()?;

    let box_bg = crate::theme::theme().surface_bg;
    let accent_peach = crate::theme::theme().accent;
    let text_dim = crate::theme::theme().text_dim;
    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(BackgroundSnapshot::default_initial);

    let result = (|| -> anyhow::Result<bool> {
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

                let needed_h = items.len().max(1) as u16 + 5;
                let modal_h = needed_h
                    .clamp(10, 24)
                    .min(area.height.saturating_sub(2).max(10))
                    .min(area.height);
                let modal_rect = centered_modal(frame, area, modal_w, modal_h, box_bg);
                let inner_h = modal_h.saturating_sub(2);
                let inner = Rect::new(modal_rect.x + pad_x, modal_rect.y + 1, inner_w, inner_h);
                let title_str = spec.title;
                let esc_str = "esc";
                let pad_len = (inner.width as usize)
                    .saturating_sub(title_str.chars().count() + esc_str.chars().count());
                let header_line = Line::from(vec![
                    Span::styled(
                        title_str,
                        Style::default()
                            .fg(crate::theme::theme().text_bright)
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

                let mut cur_y = inner.y + 1;
                let bottom = inner.y + inner_h;
                let footer_y = (inner.y + inner_h).saturating_sub(1);

                let rows_cap = if op_err.is_some() || pending_remove.is_some() {
                    footer_y.saturating_sub(1).max(inner.y + 2)
                } else {
                    bottom
                };

                if items.is_empty() {
                    if cur_y < rows_cap {
                        let text = spec.empty_hint;
                        let fill = (inner.width as usize).saturating_sub(text.chars().count());
                        frame.render_widget(
                            Paragraph::new(Line::from(vec![
                                Span::styled(text, Style::default().fg(text_dim).bg(box_bg)),
                                Span::styled(" ".repeat(fill), Style::default().bg(box_bg)),
                            ])),
                            Rect::new(inner.x, cur_y, inner.width, 1),
                        );
                    }
                } else {
                    for (idx, item) in items.iter().enumerate() {
                        if cur_y >= rows_cap {
                            break;
                        }
                        let is_selected = idx == sel;

                        let row: String = item.row.chars().take(inner.width as usize).collect();
                        let fill = (inner.width as usize).saturating_sub(row.chars().count());
                        let row_bg = if is_selected { accent_peach } else { box_bg };
                        let row_line = if is_selected {
                            Line::from(Span::styled(
                                format!("{row}{}", " ".repeat(fill)),
                                Style::default()
                                    .fg(crate::theme::theme().on_selection)
                                    .bg(accent_peach)
                                    .add_modifier(Modifier::BOLD),
                            ))
                        } else {
                            let fg = if item.lit {
                                crate::theme::theme().text_bright
                            } else {
                                text_dim
                            };
                            Line::from(vec![
                                Span::styled(row, Style::default().fg(fg).bg(row_bg)),
                                Span::styled(" ".repeat(fill), Style::default().bg(row_bg)),
                            ])
                        };
                        frame.render_widget(
                            Paragraph::new(row_line),
                            Rect::new(inner.x, cur_y, inner.width, 1),
                        );
                        cur_y += 1;
                    }
                }
                if let Some(msg) = op_err.as_deref() {
                    let err_y = footer_y.saturating_sub(1);
                    if err_y > inner.y + 1 {
                        let text: String = format!("{}: {msg}", spec.error_verb)
                            .chars()
                            .take(inner.width as usize)
                            .collect();
                        let fill = (inner.width as usize).saturating_sub(text.chars().count());
                        frame.render_widget(
                            Paragraph::new(Line::from(vec![
                                Span::styled(
                                    text,
                                    Style::default()
                                        .fg(crate::theme::theme().error_soft)
                                        .add_modifier(Modifier::BOLD)
                                        .bg(box_bg),
                                ),
                                Span::styled(" ".repeat(fill), Style::default().bg(box_bg)),
                            ])),
                            Rect::new(inner.x, err_y, inner.width, 1),
                        );
                    }
                } else if let Some(name) = pending_remove.as_deref() {
                    let confirm_y = footer_y.saturating_sub(1);
                    if confirm_y > inner.y + 1 {
                        let text: String = format!("really remove {name}? (u again)")
                            .chars()
                            .take(inner.width as usize)
                            .collect();
                        let fill = (inner.width as usize).saturating_sub(text.chars().count());
                        frame.render_widget(
                            Paragraph::new(Line::from(vec![
                                Span::styled(
                                    text,
                                    Style::default()
                                        .fg(accent_peach)
                                        .add_modifier(Modifier::BOLD)
                                        .bg(box_bg),
                                ),
                                Span::styled(" ".repeat(fill), Style::default().bg(box_bg)),
                            ])),
                            Rect::new(inner.x, confirm_y, inner.width, 1),
                        );
                    }
                }
                fn kv<'a>(k: &'a str, v: &'a str, box_bg: Color, text_dim: Color) -> [Span<'a>; 2] {
                    [
                        Span::styled(
                            k,
                            Style::default()
                                .fg(crate::theme::theme().text_bright)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled(v, Style::default().fg(text_dim).bg(box_bg)),
                    ]
                }

                let mut footer_spans = vec![Span::styled(
                    "↑↓ ",
                    Style::default()
                        .fg(crate::theme::theme().text_bright)
                        .add_modifier(Modifier::BOLD)
                        .bg(box_bg),
                )];
                footer_spans.extend(kv("nav · ", "", box_bg, text_dim));
                footer_spans.extend(kv("u ", "remove · ", box_bg, text_dim));
                footer_spans.extend(kv("Esc ", "close", box_bg, text_dim));
                frame.render_widget(
                    Paragraph::new(Line::from(footer_spans)),
                    Rect::new(inner.x, footer_y, inner.width, 1),
                );
            })?;
            if !poll(Duration::from_millis(100))? {
                continue;
            }
            let Event::Key(key) = read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let up = match key.code {
                KeyCode::Char('k') => Some(true),
                KeyCode::Char('j') => Some(false),
                _ => crate::keymap::select_move(&key),
            };
            if let Some(up) = up {
                pending_remove = None;
                sel = if up {
                    sel.saturating_sub(1)
                } else {
                    (sel + 1).min(items.len().saturating_sub(1))
                };
                continue;
            }
            match key {
                KeyEvent {
                    code: KeyCode::Char('c'),
                    modifiers,
                    ..
                } if modifiers.contains(KeyModifiers::CONTROL) => return Ok(changed),
                KeyEvent {
                    code: KeyCode::Esc, ..
                } => return Ok(changed),
                KeyEvent {
                    code: KeyCode::Char('u') | KeyCode::Delete,
                    ..
                } if items.get(sel).is_some_and(|item| !item.read_only) => {
                    let name = items[sel].name.clone();
                    if pending_remove.as_deref() == Some(name.as_str()) {
                        pending_remove = None;
                        match remove(&name) {
                            Ok(()) => {
                                changed = true;
                                op_err = None;
                                items = load();
                            }
                            Err(e) => op_err = Some(format!("{e:#}")),
                        }
                        sel = sel.min(items.len().saturating_sub(1));
                    } else {
                        op_err = None;
                        pending_remove = Some(name);
                    }
                }
                _ => {}
            }
        }
    })();
    let _ = terminal.clear();
    result
}
