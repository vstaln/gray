use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use gray_core::agent::PickerSpec;

use crate::setup::BackgroundSnapshot;

pub(crate) type Trailing<'a, T> = Option<Box<dyn Fn(&T) -> Vec<Span<'static>> + 'a>>;
pub(crate) type OnHighlight<'a, T> = Option<Box<dyn FnMut(&T) + 'a>>;
pub(crate) type GapBefore<'a, T> = Option<Box<dyn Fn(&T) -> bool + 'a>>;

pub(crate) struct ListPicker<'a, T> {
    pub(crate) title: &'a str,
    pub(crate) items: &'a [T],
    pub(crate) label: Box<dyn Fn(&T) -> &str + 'a>,
    pub(crate) trailing: Trailing<'a, T>,
    pub(crate) footer: Option<Box<dyn Fn() -> Vec<Line<'static>> + 'a>>,
    pub(crate) on_highlight: OnHighlight<'a, T>,
    pub(crate) gap_before: GapBefore<'a, T>,
    pub(crate) selected: usize,
    /// The item in effect now; drawn with a check mark.
    pub(crate) current: Option<usize>,
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
        if it.gap_before {
            out.push(Line::default());
        }
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
    super::pager::plain_lines(&spec_lines(spec))
}

fn match_range(label: &str, filter: &str) -> Option<(usize, usize)> {
    let lower = |c: char| c.to_lowercase().next().unwrap_or(c);
    let needle: Vec<char> = filter.chars().map(lower).collect();
    let chars: Vec<char> = label.chars().map(lower).collect();
    if needle.is_empty() {
        return None;
    }
    chars
        .windows(needle.len())
        .position(|w| w == needle.as_slice())
        .map(|i| (i, i + needle.len()))
}

pub(crate) fn underline_match(label: &str, filter: &str, style: Style) -> Vec<Span<'static>> {
    let Some((i, j)) = match_range(label, filter) else {
        return vec![Span::styled(label.to_string(), style)];
    };
    let chars: Vec<char> = label.chars().collect();
    [
        (0, i, style),
        (i, j, style.add_modifier(Modifier::UNDERLINED)),
        (j, chars.len(), style),
    ]
    .into_iter()
    .filter(|(a, b, _)| a < b)
    .map(|(a, b, st)| Span::styled(chars[a..b].iter().collect::<String>(), st))
    .collect()
}

/// The `/models` key-hint row: `key label` pairs, dimmed when not live,
/// dropping keys in `drop_order` until the row fits `width`.
pub(crate) fn key_hints(
    mut keys: Vec<(&'static str, &'static str, bool)>,
    drop_order: &[&str],
    width: usize,
) -> Line<'static> {
    let th = crate::theme::theme();
    let keys_w = |keys: &[(&str, &str, bool)]| -> usize {
        keys.iter()
            .map(|(k, l, _)| k.chars().count() + l.chars().count() + 4)
            .sum::<usize>()
            + 1
    };
    for drop in drop_order {
        if keys_w(&keys) <= width {
            break;
        }
        keys.retain(|(k, _, _)| k != drop);
    }
    let mut spans = vec![Span::raw(" ")];
    spans.extend(keys.into_iter().flat_map(|(k, label, live)| {
        [
            Span::styled(
                k,
                Style::default().fg(if live { th.text_soft } else { th.text_faint }),
            ),
            Span::styled(
                format!(" {label}   "),
                Style::default().fg(if live { th.text_dim } else { th.text_faint }),
            ),
        ]
    }));
    Line::from(spans)
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
    use ratatui::layout::{Alignment, Rect};
    use ratatui::widgets::{Clear, Paragraph};

    let th = crate::theme::theme();
    let fg = |c| Style::default().fg(c);
    let bold = |c| Style::default().fg(c).add_modifier(Modifier::BOLD);
    let area = frame.area();
    if area.width < 24 || area.height < 8 {
        return;
    }
    crate::setup::render_dimmed_background(frame, bg);
    let footer_lines = picker.footer.as_ref().map(|f| f()).unwrap_or_default();
    // header, gap, list, gap, [detail lines, gap], keys
    let detail_h = if footer_lines.is_empty() {
        0
    } else {
        footer_lines.len() + 1
    };
    let gap_at = |vi: usize, idx: usize| {
        vi > 0
            && query.is_empty()
            && filtered.len() <= list_h
            && picker
                .gap_before
                .as_ref()
                .is_some_and(|g| g(&picker.items[idx]))
    };
    let gaps = filtered
        .iter()
        .enumerate()
        .filter(|&(vi, &idx)| gap_at(vi, idx))
        .count();
    let inner_h = (list_h + gaps + 4 + detail_h) as u16;
    let trailing_w_of = |item: &T| -> usize {
        picker
            .trailing
            .as_ref()
            .map(|f| f(item))
            .unwrap_or_default()
            .iter()
            .map(Span::width)
            .sum()
    };
    let trailing_col = picker.items.iter().map(trailing_w_of).max().unwrap_or(0);
    let label_max = picker
        .items
        .iter()
        .map(|item| (picker.label)(item).chars().count())
        .max()
        .unwrap_or(0);
    let widest = std::iter::once(3 + label_max + 2 + trailing_col)
        .chain(footer_lines.iter().map(|l| l.width() + 1))
        .chain(std::iter::once(picker.title.chars().count() + 20))
        .max()
        .unwrap_or(0);
    let modal_w = 48u16
        .max(widest as u16 + 3)
        .min(area.width.saturating_sub(4));
    let modal_h = (inner_h + 2).min(area.height.saturating_sub(2));
    let modal = Rect::new(
        (area.width.saturating_sub(modal_w)) / 2,
        (area.height.saturating_sub(modal_h)) / 2,
        modal_w,
        modal_h,
    );
    frame.render_widget(Clear, modal);
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(th.raised_bg)),
        modal,
    );
    let inner = Rect::new(
        modal.x + 1,
        modal.y + 1,
        modal.width.saturating_sub(2),
        modal.height.saturating_sub(2),
    );
    let bottom = inner.y + inner.height;
    let row = |y: u16| Rect::new(inner.x, y, inner.width, 1);

    let mut filter = if query.is_empty() {
        vec![Span::styled("type to filter ", fg(th.text_faint))]
    } else {
        vec![
            Span::styled("filter ", fg(th.text_faint)),
            Span::styled(
                query.to_string(),
                fg(th.text_bright).add_modifier(Modifier::UNDERLINED),
            ),
            Span::raw(" "),
        ]
    };
    if filtered.len() > list_h {
        filter.insert(
            0,
            Span::styled(
                format!("{}/{}  ", sel + 1, filtered.len()),
                fg(th.text_faint),
            ),
        );
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(picker.title.to_string(), bold(th.text_bright)),
        ])),
        row(inner.y),
    );
    frame.render_widget(
        Paragraph::new(Line::from(filter)).alignment(Alignment::Right),
        row(inner.y),
    );

    let list_y = inner.y + 2;
    let start = sel
        .saturating_sub(list_h.saturating_sub(1))
        .min(filtered.len().saturating_sub(list_h));
    if filtered.is_empty() && list_y < bottom {
        frame.render_widget(
            Paragraph::new(Span::styled("   no match", fg(th.text_faint))),
            row(list_y),
        );
    }
    let mut gaps_above = 0u16;
    for (vi, &idx) in filtered.iter().enumerate().skip(start).take(list_h) {
        if gap_at(vi, idx) {
            gaps_above += 1;
        }
        let y = list_y + (vi - start) as u16 + gaps_above;
        if y >= bottom {
            break;
        }
        let item = &picker.items[idx];
        let is_sel = vi == sel;
        let current = picker.current == Some(idx);
        let base = if is_sel {
            Style::default().bg(th.accent)
        } else {
            Style::default()
        };
        let text = base.patch(if is_sel {
            bold(th.on_selection)
        } else if current {
            fg(th.text_bright)
        } else {
            fg(th.text_body)
        });
        let mut trailing = picker
            .trailing
            .as_ref()
            .map(|f| f(item))
            .unwrap_or_default();
        if is_sel {
            // Text recolors on the accent bar; swatches (a set bg) keep theirs.
            for s in &mut trailing {
                if s.style.bg.is_none() {
                    s.style = s.style.fg(th.on_selection);
                }
            }
        }
        let trailing_w: usize = trailing.iter().map(Span::width).sum();
        let col = trailing_col.min((inner.width as usize).saturating_sub(3 + 8 + 1));
        let col = col.max(trailing_w);
        let name_w = (inner.width as usize).saturating_sub(3 + col + 1);
        let label = crate::text_width::fit_chars((picker.label)(item), name_w);
        let mut spans = vec![Span::styled(
            if current { " ✓ " } else { "   " },
            fg(if is_sel { th.on_selection } else { th.success }),
        )];
        spans.extend(underline_match(&label, query, text));
        spans.push(Span::raw(
            " ".repeat(name_w.saturating_sub(label.chars().count())),
        ));
        spans.extend(trailing);
        spans.push(Span::raw(" ".repeat(col - trailing_w)));
        frame.render_widget(Paragraph::new(Line::from(spans)).style(base), row(y));
    }

    for (y, line) in (list_y + list_h as u16 + 1..).zip(footer_lines) {
        if y + 1 >= bottom {
            break;
        }
        let mut spans = vec![Span::raw(" ")];
        spans.extend(line.spans);
        frame.render_widget(Paragraph::new(Line::from(spans)), row(y));
    }
    frame.render_widget(
        Paragraph::new(key_hints(
            vec![
                ("↑↓", "move", true),
                ("enter", "select", true),
                ("esc", "close", true),
            ],
            &[],
            inner.width as usize,
        )),
        row(bottom.saturating_sub(1)),
    );
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

/// A plugin's picker as a modal over the transcript: the picked item's id,
/// `None` on esc, an error when no modal can open (no tty).
pub(crate) fn run_spec_picker(
    shared: &crate::composer::SharedTui,
    spec: &PickerSpec,
) -> anyhow::Result<Option<String>> {
    let bg = shared.lock().expect("tui lock").snapshot();
    let dim = Style::default().fg(crate::theme::theme().text_dim);
    let selected = spec
        .selected_id
        .as_deref()
        .and_then(|id| spec.items.iter().position(|i| i.id == id))
        .unwrap_or(0);
    let mut picker = ListPicker {
        title: spec.title.as_str(),
        items: &spec.items,
        label: Box::new(|i: &gray_core::agent::PickerItem| i.label.as_str()),
        trailing: Some(Box::new(move |i: &gray_core::agent::PickerItem| {
            i.description
                .as_deref()
                .filter(|d| !d.is_empty())
                .map(|d| vec![Span::styled(d.to_string(), dim)])
                .unwrap_or_default()
        })),
        footer: spec.footer.as_deref().map(|f| {
            Box::new(move || {
                f.lines()
                    .map(|l| Line::styled(l.to_string(), dim))
                    .collect()
            }) as Box<dyn Fn() -> Vec<Line<'static>>>
        }),
        on_highlight: None,
        gap_before: Some(Box::new(|i: &gray_core::agent::PickerItem| i.gap_before)),
        selected,
        current: spec.selected_id.is_some().then_some(selected),
    };
    super::with_modal_sync(Some(shared), || {
        let Some(idx) = run_list_picker(&mut picker, &bg)? else {
            return Ok(None);
        };
        let item = &spec.items[idx];
        let Some(prompt) = item.capture_key.as_deref() else {
            return Ok(Some(item.id.clone()));
        };
        Ok(capture_key(prompt, &bg)?.map(|k| format!("{}:{k}", item.id)))
    })
}

/// Wait for one key press and return it as a keymap chord (`ctrl+space`).
/// Esc cancels.
fn capture_key(prompt: &str, bg: &BackgroundSnapshot) -> anyhow::Result<Option<String>> {
    use crossterm::event::{Event, KeyCode, KeyEventKind, read};
    let (_session, mut terminal) = crate::setup::open_modal()?;
    let items = ["Press a key, or Esc to cancel".to_string()];
    let picker = ListPicker {
        title: prompt,
        items: &items,
        label: Box::new(|s: &String| s.as_str()),
        trailing: None,
        footer: None,
        on_highlight: None,
        gap_before: None,
        selected: 0,
        current: None,
    };
    terminal.draw(|frame| draw_list_picker(frame, bg, &picker, &[0], 0, "", 1))?;
    loop {
        if let Event::Key(key) = read()?
            && key.kind == KeyEventKind::Press
        {
            if key.code == KeyCode::Esc {
                return Ok(None);
            }
            if let Some(chord) = crate::keymap::Chord::from_event(&key) {
                return Ok(Some(chord.display()));
            }
        }
    }
}
