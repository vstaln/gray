use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use gray_core::agent::PickerSpec;

use crate::setup::BackgroundSnapshot;

pub(crate) type Trailing<'a, T> = Option<Box<dyn Fn(&T) -> Vec<Span<'static>> + 'a>>;
pub(crate) type OnHighlight<'a, T> = Option<Box<dyn FnMut(&T) + 'a>>;

pub(crate) struct ListPicker<'a, T> {
    pub(crate) title: &'a str,
    pub(crate) items: &'a [T],
    pub(crate) label: Box<dyn Fn(&T) -> &str + 'a>,
    pub(crate) trailing: Trailing<'a, T>,
    pub(crate) footer: Option<Box<dyn Fn() -> Vec<Line<'static>> + 'a>>,
    pub(crate) on_highlight: OnHighlight<'a, T>,
    pub(crate) selected: usize,
}

pub(crate) fn list_rows_for(height: u16, n: usize, footer_rows: usize) -> usize {
    let room = usize::from(height).saturating_sub(9 + footer_rows).max(1);
    n.clamp(1, 12).min(room)
}

pub(crate) fn pick_argv(argv: &[String], id: &str) -> Vec<String> {
    let mut out = argv.to_vec();
    out.push("--pick".to_string());
    out.push(id.to_string());
    out
}

pub(crate) fn spec_lines(spec: &PickerSpec) -> Vec<Line<'static>> {
    let th = crate::theme::theme();
    let mut out = vec![Line::from(Span::styled(
        spec.title.clone(),
        Style::default()
            .fg(th.text_bright)
            .add_modifier(Modifier::BOLD),
    ))];
    for it in &spec.items {
        let mut spans = vec![Span::styled(
            it.label.clone(),
            Style::default().fg(th.text_body),
        )];
        if let Some(desc) = it.description.as_deref().filter(|d| !d.is_empty()) {
            spans.push(Span::styled(
                format!("  {desc}"),
                Style::default().fg(th.text_dim),
            ));
        }
        out.push(Line::from(spans));
    }
    out
}

pub(crate) fn spec_plain(spec: &PickerSpec) -> String {
    spec_lines(spec)
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn draw_list_picker<T>(
    frame: &mut ratatui::Frame,
    bg: &BackgroundSnapshot,
    picker: &ListPicker<'_, T>,
    filtered: &[usize],
    sel: usize,
    query: &str,
    list_h: usize,
) {
    use ratatui::layout::Rect;
    use ratatui::widgets::{Block, Clear, Paragraph};

    let th = crate::theme::theme();
    let area = frame.area();
    if area.width < 24 || area.height < 8 {
        return;
    }
    crate::setup::render_dimmed_background(frame, bg);
    let footer_lines = picker.footer.as_ref().map(|f| f()).unwrap_or_default();
    let inner_h = (list_h + 5 + footer_lines.len()) as u16;
    let modal_w = 48.min(area.width.saturating_sub(2));
    let modal_h = (inner_h + 2).min(area.height.saturating_sub(2));
    let modal_x = (area.width.saturating_sub(modal_w)) / 2;
    let modal_y = (area.height.saturating_sub(modal_h)) / 3;
    let modal = Rect::new(modal_x, modal_y, modal_w, modal_h);
    frame.render_widget(Clear, modal);
    let counter = if filtered.is_empty() {
        "0/0".to_string()
    } else {
        format!("{}/{}", sel + 1, filtered.len())
    };

    frame.render_widget(
        Block::default().style(Style::default().bg(th.surface_bg)),
        modal,
    );
    let inner = Rect::new(
        modal.x + 3,
        modal.y + 1,
        modal.width.saturating_sub(6),
        modal.height.saturating_sub(2),
    );
    let inner_bottom = inner.y + inner.height;

    let y = inner.y;
    if y < inner_bottom {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                picker.title,
                Style::default()
                    .fg(th.text_bright)
                    .add_modifier(Modifier::BOLD),
            ))),
            Rect::new(inner.x, y, inner.width, 1),
        );
    }
    let mut y = inner.y + 2;
    if y < inner_bottom {
        let counter_w = (counter.chars().count() + 1) as u16;
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(query.to_string(), Style::default().fg(th.text_body)),
                Span::styled("█", Style::default().fg(th.text_body)),
            ])),
            Rect::new(inner.x, y, inner.width.saturating_sub(counter_w), 1),
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                counter,
                Style::default().fg(th.text_muted),
            )]))
            .alignment(ratatui::layout::Alignment::Right),
            Rect::new(inner.x, y, inner.width, 1),
        );
    }

    let list_y = inner.y + 4;
    let start = sel
        .saturating_sub(list_h.saturating_sub(1))
        .min(filtered.len().saturating_sub(list_h));
    if filtered.is_empty() {
        if list_y < inner_bottom {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "  no match",
                    Style::default().fg(th.text_dim),
                ))),
                Rect::new(inner.x, list_y, inner.width, 1),
            );
        }
    } else {
        for (vi, &idx) in filtered.iter().enumerate().skip(start).take(list_h) {
            let item = &picker.items[idx];
            y = list_y + (vi - start) as u16;
            if y >= inner_bottom {
                break;
            }
            let is_sel = vi == sel;
            let trailing = picker
                .trailing
                .as_ref()
                .map(|f| f(item))
                .unwrap_or_default();
            let trailing_w: usize = trailing.iter().map(|s| s.content.chars().count()).sum();
            let marker = if is_sel { "› " } else { "  " };
            let label = (picker.label)(item);
            let left_w = 2 + label.chars().count();
            let pad = (inner.width as usize).saturating_sub(left_w + trailing_w + 1);
            let mut spans = vec![
                Span::styled(
                    marker,
                    Style::default().fg(if is_sel { th.accent } else { th.text_dim }),
                ),
                Span::styled(
                    label.to_string(),
                    if is_sel {
                        Style::default()
                            .fg(th.text_bright)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(th.text_dim)
                    },
                ),
                Span::raw(" ".repeat(pad)),
            ];
            spans.extend(trailing);
            frame.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect::new(inner.x, y, inner.width, 1),
            );
        }
    }

    let footer_y = list_y + list_h as u16 + 1;
    for (i, line) in footer_lines.into_iter().enumerate() {
        let y = footer_y + i as u16;
        if y >= inner_bottom {
            break;
        }
        frame.render_widget(Paragraph::new(line), Rect::new(inner.x, y, inner.width, 1));
    }
}

fn highlight<T>(picker: &mut ListPicker<'_, T>, filtered: &[usize], sel: usize) {
    if let Some(&idx) = filtered.get(sel)
        && let Some(cb) = picker.on_highlight.as_deref_mut()
    {
        cb(&picker.items[idx]);
    }
}

fn highlight_first<T>(picker: &mut ListPicker<'_, T>) {
    if let Some(cb) = picker.on_highlight.as_deref_mut()
        && let Some(item) = picker.items.first()
    {
        cb(item);
    }
}

fn highlight_match<T>(picker: &mut ListPicker<'_, T>, query: &str) {
    let q = query.to_lowercase();
    if let Some(cb) = picker.on_highlight.as_deref_mut()
        && let Some(item) = picker
            .items
            .iter()
            .find(|it| q.is_empty() || (picker.label)(it).to_lowercase().contains(&q))
    {
        cb(item);
    }
}

pub(crate) fn run_list_picker<T>(
    picker: &mut ListPicker<'_, T>,
    bg: &BackgroundSnapshot,
) -> anyhow::Result<Option<usize>> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
    use std::time::Duration;

    let (_session, mut terminal) = crate::setup::open_modal()?;
    let mut sel = picker.selected;
    let mut query = String::new();

    loop {
        let q = query.to_lowercase();
        let filtered: Vec<usize> = picker
            .items
            .iter()
            .enumerate()
            .filter(|(_, it)| q.is_empty() || (picker.label)(it).to_lowercase().contains(&q))
            .map(|(i, _)| i)
            .collect();
        sel = sel.min(filtered.len().saturating_sub(1));
        let footer_n = picker.footer.as_ref().map(|f| f().len()).unwrap_or(0);
        let list_h = list_rows_for(terminal.size()?.height, filtered.len(), footer_n);

        terminal
            .draw(|frame| draw_list_picker(frame, bg, picker, &filtered, sel, &query, list_h))?;

        if !poll(Duration::from_millis(100))? {
            continue;
        }
        let ev = read()?;
        if let Event::Key(key) = ev
            && key.kind == KeyEventKind::Press
            && let Some(up) = crate::keymap::select_move(&key)
        {
            let next = if up {
                sel.saturating_sub(1)
            } else {
                (sel + 1).min(filtered.len().saturating_sub(1))
            };
            if next != sel {
                sel = next;
                highlight(picker, &filtered, sel);
            }
            continue;
        }
        match ev {
            Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers,
                kind: KeyEventKind::Press,
                ..
            }) if modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(None);
            }
            Event::Key(KeyEvent {
                code,
                modifiers,
                kind: KeyEventKind::Press,
                ..
            }) => {
                let last = filtered.len().saturating_sub(1);
                let next = match code {
                    KeyCode::PageUp => Some(sel.saturating_sub(list_h)),
                    KeyCode::PageDown => Some((sel + list_h).min(last)),
                    KeyCode::Home => Some(0),
                    KeyCode::End => Some(last),
                    _ => None,
                };
                if let Some(n) = next {
                    if n != sel {
                        sel = n;
                        highlight(picker, &filtered, sel);
                    }
                    continue;
                }
                match code {
                    KeyCode::Enter => {
                        if let Some(&idx) = filtered.get(sel) {
                            return Ok(Some(idx));
                        }
                    }
                    KeyCode::Esc => {
                        if query.is_empty() {
                            return Ok(None);
                        }
                        query.clear();
                        sel = 0;
                        highlight_first(picker);
                    }
                    KeyCode::Backspace => {
                        if query.pop().is_some() {
                            sel = 0;
                            highlight_match(picker, &query);
                        }
                    }
                    KeyCode::Char(c) if !modifiers.contains(KeyModifiers::ALT) => {
                        query.push(c);
                        sel = 0;
                        highlight_match(picker, &query);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}
