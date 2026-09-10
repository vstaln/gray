//! Interactive `/plugins` manager modal: toggle/uninstall installed plugins,
//! plus a read-only Errors tab fed by `gray_pkg::errors`.
//!
//! Sync modal returning whether anything changed, mirroring the
//! `permissions_modal` chrome with the effort-modal toggle-stays-open
//! pattern (Enter/Space flips `enabled` via `gray_pkg::ops` and re-reads).
//! Tab rendering goes through the shared [`super::tabs`] scaffolding so the
//! Task 6 store reuses the same code path.

use super::tabs::{Tab, tab_segments};
use super::*;

use gray_pkg::errors::ErrorEntry;
use gray_pkg::ops::LockEntry;

/// Display label for a lockfile ecosystem: known sources get friendly
/// names, anything else shows the raw ecosystem string.
fn source_label(ecosystem: &str) -> &str {
    match ecosystem {
        "gray-native" => "Gray Index",
        "pi-gallery" => "Pi Gallery (preview)",
        other => other,
    }
}

/// Pure row renderer: `✓ name version (scope) [source]` when enabled,
/// dim `○ … [disabled]` when disabled.
pub(crate) fn format_plugin_row(name: &str, entry: &LockEntry) -> String {
    let base = format!(
        "{} {} ({}) [{}]",
        name,
        entry.version,
        entry.scope,
        source_label(&entry.ecosystem)
    );
    if entry.enabled {
        format!("✓ {base}")
    } else {
        format!("○ {base} [disabled]")
    }
}

/// Pure row renderer for the Errors tab: `<source> <item>: <message>`.
pub(crate) fn format_error_row(entry: &ErrorEntry) -> String {
    format!("{} {}: {}", entry.source, entry.item, entry.message)
}

/// Bare `/plugin` picker: navigate installed plugins, Enter/Space toggles
/// `enabled` and stays open. Returns true when anything changed (toggle,
/// uninstall, or errors cleared).
pub fn run_plugins_modal(bg: Option<&BackgroundSnapshot>) -> anyhow::Result<bool> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
    use crossterm::terminal::EnterAlternateScreen;
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use std::time::Duration;

    let mut entries = gray_pkg::ops::list().unwrap_or_default();
    let mut names: Vec<String> = entries.keys().cloned().collect();
    let mut error_entries = gray_pkg::errors::list();
    let mut tab = Tab::Installed;
    let mut sel = 0usize;
    let mut changed = false;
    let mut toggle_err: Option<String> = None;
    // Armed uninstall confirm: first `u` arms, second `u` on the same entry
    // removes it.
    let mut pending_remove: Option<String> = None;

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
                // Row count of the active tab (empty states render one line).
                let tab_count = match tab {
                    Tab::Installed => names.len(),
                    Tab::Errors => error_entries.len(),
                };
                let rows = tab_count.max(1) as u16;
                // header(1) + tabs(1) + rows + gap(1) + footer(1)
                // + 2 for modal top/bottom padding.
                let needed_h = rows + 6;
                let modal_h = needed_h
                    .clamp(10, 24)
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
                let inner = Rect::new(modal_x + pad_x, modal_y + 1, inner_w, inner_h);
                let title_str = "Plugins";
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
                // Tab bar on the line below the header (shared scaffolding;
                // same code path renders both tabs).
                {
                    let tabs = [
                        ("Installed", None),
                        (
                            "Errors",
                            if error_entries.is_empty() {
                                None
                            } else {
                                Some(error_entries.len())
                            },
                        ),
                    ];
                    let segs = tab_segments(&tabs, tab.index());
                    let mut spans = Vec::with_capacity(segs.len() * 2 + 1);
                    let mut used = 0usize;
                    for (i, seg) in segs.iter().enumerate() {
                        if i > 0 {
                            spans.push(Span::styled(
                                " | ",
                                Style::default().fg(text_dim).bg(box_bg),
                            ));
                            used += 3;
                        }
                        let style = if seg.active {
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg)
                        } else {
                            Style::default().fg(text_dim).bg(box_bg)
                        };
                        used += seg.text.chars().count();
                        spans.push(Span::styled(seg.text.clone(), style));
                    }
                    let fill = (inner.width as usize).saturating_sub(used);
                    spans.push(Span::styled(" ".repeat(fill), Style::default().bg(box_bg)));
                    frame.render_widget(
                        Paragraph::new(Line::from(spans)),
                        Rect::new(inner.x, inner.y + 1, inner.width, 1),
                    );
                }
                let mut cur_y = inner.y + 2;
                let bottom = inner.y + inner_h;
                let footer_y = (inner.y + inner_h).saturating_sub(1);
                // Reserve the line above the footer for a toggle error or an
                // armed uninstall confirm, if any.
                let rows_cap = if toggle_err.is_some() || pending_remove.is_some() {
                    footer_y.saturating_sub(1).max(inner.y + 2)
                } else {
                    bottom
                };
                // Row texts + lit flag (installed rows dim when disabled)
                // for the active tab; one shared render loop below.
                let tab_rows: Vec<(String, bool)> = match tab {
                    Tab::Installed => names
                        .iter()
                        .map(|name| {
                            let text = entries
                                .get(name)
                                .map(|e| format_plugin_row(name, e))
                                .unwrap_or_else(|| name.clone());
                            let lit = entries.get(name).map(|e| e.enabled).unwrap_or(true);
                            (text, lit)
                        })
                        .collect(),
                    Tab::Errors => error_entries
                        .iter()
                        .map(|e| (format_error_row(e), true))
                        .collect(),
                };
                if tab_rows.is_empty() {
                    if cur_y < rows_cap {
                        let text = match tab {
                            Tab::Installed => "no plugins installed — /plugin install <spec>",
                            Tab::Errors => "no errors recorded",
                        };
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
                    for (idx, (row, lit)) in tab_rows.iter().enumerate() {
                        if cur_y >= rows_cap {
                            break;
                        }
                        let is_selected = idx == sel;
                        // Truncate to the inner width so long rows never wrap.
                        let row: String = row.chars().take(inner.width as usize).collect();
                        let fill = (inner.width as usize).saturating_sub(row.chars().count());
                        let row_bg = if is_selected { accent_peach } else { box_bg };
                        let row_line = if is_selected {
                            Line::from(Span::styled(
                                format!("{row}{}", " ".repeat(fill)),
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(accent_peach)
                                    .add_modifier(Modifier::BOLD),
                            ))
                        } else {
                            let fg = if *lit { Color::White } else { text_dim };
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
                if let Some(msg) = toggle_err.as_deref() {
                    let err_y = footer_y.saturating_sub(1);
                    if err_y > inner.y + 1 {
                        let text: String = format!("toggle failed: {msg}")
                            .chars()
                            .take(inner.width as usize)
                            .collect();
                        let fill = (inner.width as usize).saturating_sub(text.chars().count());
                        frame.render_widget(
                            Paragraph::new(Line::from(vec![
                                Span::styled(
                                    text,
                                    Style::default()
                                        .fg(Color::Rgb(220, 120, 120))
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
                let footer_line = match tab {
                    Tab::Installed => Line::from(vec![
                        Span::styled(
                            "↑↓ ",
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled("nav · ", Style::default().fg(text_dim).bg(box_bg)),
                        Span::styled(
                            "Enter ",
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled("toggle · ", Style::default().fg(text_dim).bg(box_bg)),
                        Span::styled(
                            "u ",
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled("remove · ", Style::default().fg(text_dim).bg(box_bg)),
                        Span::styled(
                            "Tab ",
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled("· ", Style::default().fg(text_dim).bg(box_bg)),
                        Span::styled(
                            "Esc ",
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled("close", Style::default().fg(text_dim).bg(box_bg)),
                    ]),
                    Tab::Errors => Line::from(vec![
                        Span::styled(
                            "↑↓ ",
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled("nav · ", Style::default().fg(text_dim).bg(box_bg)),
                        Span::styled(
                            "c ",
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled("clear · ", Style::default().fg(text_dim).bg(box_bg)),
                        Span::styled(
                            "Tab ",
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled("· ", Style::default().fg(text_dim).bg(box_bg)),
                        Span::styled(
                            "Esc ",
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled("close", Style::default().fg(text_dim).bg(box_bg)),
                    ]),
                };
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
                }) if modifiers.contains(KeyModifiers::CONTROL) => return Ok(changed),
                Event::Key(KeyEvent {
                    code,
                    modifiers,
                    kind: KeyEventKind::Press,
                    ..
                }) => match code {
                    KeyCode::Tab => {
                        tab = tab.next();
                        error_entries = gray_pkg::errors::list();
                        sel = 0;
                        pending_remove = None;
                    }
                    KeyCode::BackTab => {
                        tab = tab.prev();
                        error_entries = gray_pkg::errors::list();
                        sel = 0;
                        pending_remove = None;
                    }
                    KeyCode::Char('n') | KeyCode::Char('p')
                        if modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        tab = if code == KeyCode::Char('n') {
                            tab.next()
                        } else {
                            tab.prev()
                        };
                        error_entries = gray_pkg::errors::list();
                        sel = 0;
                        pending_remove = None;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        pending_remove = None;
                        sel = sel.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        pending_remove = None;
                        let max = match tab {
                            Tab::Installed => names.len(),
                            Tab::Errors => error_entries.len(),
                        }
                        .saturating_sub(1);
                        sel = (sel + 1).min(max);
                    }
                    KeyCode::Esc => return Ok(changed),
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        pending_remove = None;
                        if tab != Tab::Installed {
                            // Errors tab is read-only.
                        } else {
                            if names.is_empty() {
                                return Ok(changed);
                            }
                            let name = names[sel].clone();
                            let enabled = entries.get(&name).map(|e| e.enabled).unwrap_or(true);
                            match gray_pkg::ops::set_enabled(&name, !enabled) {
                                Ok(()) => {
                                    changed = true;
                                    toggle_err = None;
                                    // Re-read and stay open (toggle-stays-open pattern);
                                    // entries only ever come from a fresh list().
                                    if let Ok(fresh) = gray_pkg::ops::list() {
                                        entries = fresh;
                                        names = entries.keys().cloned().collect();
                                    }
                                }
                                Err(e) => {
                                    toggle_err = Some(format!("{e:#}"));
                                }
                            }
                            sel = sel.min(names.len().saturating_sub(1));
                        }
                    }
                    KeyCode::Char('u') | KeyCode::Delete => {
                        if tab == Tab::Installed && !names.is_empty() {
                            let name = names[sel].clone();
                            if pending_remove.as_deref() == Some(name.as_str()) {
                                match gray_pkg::ops::remove(&name) {
                                    Ok(()) => {
                                        changed = true;
                                        toggle_err = None;
                                        pending_remove = None;
                                        if let Ok(fresh) = gray_pkg::ops::list() {
                                            entries = fresh;
                                            names = entries.keys().cloned().collect();
                                        }
                                    }
                                    Err(e) => {
                                        toggle_err = Some(format!("{e:#}"));
                                        pending_remove = None;
                                    }
                                }
                                sel = sel.min(names.len().saturating_sub(1));
                            } else {
                                toggle_err = None;
                                pending_remove = Some(name);
                            }
                        }
                    }
                    KeyCode::Char('c') if tab == Tab::Errors => {
                        gray_pkg::errors::clear();
                        error_entries = gray_pkg::errors::list();
                        sel = 0;
                        changed = true;
                    }
                    _ => {}
                },
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    })();
    let _ = terminal.clear();
    result
}

#[cfg(test)]
mod tests {
    use super::{format_error_row, format_plugin_row};
    use gray_pkg::errors::ErrorEntry;
    use gray_pkg::ops::LockEntry;

    fn entry(ecosystem: &str, enabled: bool) -> LockEntry {
        LockEntry {
            ecosystem: ecosystem.to_string(),
            version: "1.2.3".to_string(),
            scope: "user".to_string(),
            enabled,
            ..LockEntry::default()
        }
    }

    #[test]
    fn enabled_row_shows_check_and_source_label() {
        let row = format_plugin_row("demo", &entry("gray-native", true));
        assert!(row.starts_with("✓ "), "enabled marker: {row:?}");
        assert!(row.contains("demo 1.2.3 (user)"), "body: {row:?}");
        assert!(row.contains("[Gray Index]"), "source label: {row:?}");
        assert!(!row.contains("[disabled]"), "no dim marker: {row:?}");
    }

    #[test]
    fn disabled_row_shows_circle_and_disabled_marker() {
        let row = format_plugin_row("demo", &entry("pi-gallery", false));
        assert!(row.starts_with("○ "), "disabled marker: {row:?}");
        assert!(row.contains("[disabled]"), "dim marker text: {row:?}");
        assert!(
            row.contains("[Pi Gallery (preview)]"),
            "source label: {row:?}"
        );
    }

    #[test]
    fn unknown_ecosystem_uses_raw_string() {
        let row = format_plugin_row("demo", &entry("url", true));
        assert!(row.contains("[url]"), "raw ecosystem: {row:?}");
    }

    #[test]
    fn error_row_shows_source_item_and_message() {
        let row = format_error_row(&ErrorEntry {
            ts_secs: 0,
            source: "index".to_string(),
            item: "demo".to_string(),
            message: "boom".to_string(),
        });
        assert_eq!(row, "index demo: boom");
    }
}
