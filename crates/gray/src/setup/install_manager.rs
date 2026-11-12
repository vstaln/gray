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
    /// What `u` does to the selected row, shown in the key hints.
    pub action: &'static str,
}

pub(crate) fn run_install_manager(
    bg: Option<&BackgroundSnapshot>,
    spec: &ManagerSpec,
    load: impl Fn() -> Vec<ManagerItem>,
    remove: impl Fn(&str) -> anyhow::Result<()>,
) -> anyhow::Result<bool> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
    use ratatui::layout::Rect;
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Paragraph;
    use std::time::Duration;

    let mut items = load();
    let mut sel = 0usize;
    let mut changed = false;
    let mut op_err: Option<String> = None;
    let mut pending_remove: Option<String> = None;

    let (_session, mut terminal) = super::open_modal()?;

    let box_bg = crate::theme::theme().raised_bg;
    let accent_peach = crate::theme::theme().accent;
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
                let th = crate::theme::theme();
                let fg = |c| Style::default().fg(c);
                let widest = items
                    .iter()
                    .map(|i| i.row.chars().count())
                    .max()
                    .unwrap_or(0);
                let modal_w = (widest as u16 + 8)
                    .clamp(56, 116)
                    .min(area.width.saturating_sub(4))
                    .min(area.width);
                // header, gap, rows, gap, [notice], keys
                let notice = op_err.is_some() || pending_remove.is_some();
                let needed_h = items.len().max(1) as u16 + 6 + u16::from(notice);
                let modal_h = needed_h.min(area.height.saturating_sub(2)).min(area.height);
                let modal_rect = centered_modal(frame, area, modal_w, modal_h, box_bg);
                let inner = Rect::new(
                    modal_rect.x + 1,
                    modal_rect.y + 1,
                    modal_w.saturating_sub(2),
                    modal_h.saturating_sub(2),
                );
                let row = |y: u16| Rect::new(inner.x, y, inner.width, 1);
                let footer_y = (inner.y + inner.height).saturating_sub(1);

                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::raw(" "),
                        Span::styled(spec.title, fg(th.text_bright).add_modifier(Modifier::BOLD)),
                    ])),
                    row(inner.y),
                );
                if !items.is_empty() {
                    frame.render_widget(
                        Paragraph::new(Span::styled(
                            format!("{} ", items.len()),
                            fg(th.text_faint),
                        ))
                        .alignment(ratatui::layout::Alignment::Right),
                        row(inner.y),
                    );
                }

                let list_y = inner.y + 2;
                let rows_end = footer_y.saturating_sub(1 + u16::from(notice));
                if items.is_empty() && list_y < rows_end {
                    frame.render_widget(
                        Paragraph::new(Span::styled(
                            format!("   {}", spec.empty_hint),
                            fg(th.text_faint),
                        )),
                        row(list_y),
                    );
                }
                let visible = rows_end.saturating_sub(list_y) as usize;
                let start = sel.saturating_sub(visible.saturating_sub(1));
                for (vi, item) in items.iter().enumerate().skip(start).take(visible) {
                    let is_sel = vi == sel;
                    let base = if is_sel {
                        Style::default().bg(accent_peach)
                    } else {
                        Style::default()
                    };
                    let text = base.patch(if is_sel {
                        fg(th.on_selection).add_modifier(Modifier::BOLD)
                    } else if item.lit {
                        fg(th.text_body)
                    } else {
                        fg(th.text_faint)
                    });
                    let label = crate::text_width::fit_chars(
                        &item.row,
                        (inner.width as usize).saturating_sub(4),
                    );
                    frame.render_widget(
                        Paragraph::new(Line::from(vec![
                            Span::styled(
                                if item.lit { " ● " } else { " ○ " },
                                fg(if is_sel {
                                    th.on_selection
                                } else if item.lit {
                                    th.success
                                } else {
                                    th.text_faint
                                }),
                            ),
                            Span::styled(label, text),
                        ]))
                        .style(base),
                        row(list_y + (vi - start) as u16),
                    );
                }

                let notice_line = if let Some(msg) = op_err.as_deref() {
                    Some(Span::styled(
                        format!(" {}: {msg}", spec.error_verb),
                        fg(th.error_soft),
                    ))
                } else {
                    pending_remove.as_deref().map(|name| {
                        let what = items
                            .iter()
                            .find(|i| i.name == name)
                            .map_or(name, |i| i.row.as_str());
                        Span::styled(
                            format!(" press u again to {} {what}", spec.action),
                            fg(accent_peach),
                        )
                    })
                };
                if let Some(span) = notice_line {
                    frame.render_widget(Paragraph::new(span), row(footer_y.saturating_sub(1)));
                }
                frame.render_widget(
                    Paragraph::new(crate::repl::picker::key_hints(
                        vec![
                            ("↑↓", "move", !items.is_empty()),
                            (
                                "u",
                                spec.action,
                                items.get(sel).is_some_and(|i| !i.read_only),
                            ),
                            ("esc", "close", true),
                        ],
                        &[],
                        inner.width as usize,
                    )),
                    row(footer_y),
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
