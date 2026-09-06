//! Interactive `/plugins` manager modal: toggle installed plugins on/off.
//!
//! Sync modal returning whether anything changed, mirroring the
//! `permissions_modal` chrome with the effort-modal toggle-stays-open
//! pattern (Enter/Space flips `enabled` via `gray_pkg::ops` and re-reads).

use super::*;

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

/// Bare `/plugin` picker: navigate installed plugins, Enter/Space toggles
/// `enabled` and stays open. Returns true when anything was toggled.
pub fn run_plugins_modal(bg: Option<&BackgroundSnapshot>) -> anyhow::Result<bool> {
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

    let mut entries = gray_pkg::ops::list().unwrap_or_default();
    let mut names: Vec<String> = entries.keys().cloned().collect();
    let mut sel = 0usize;
    let mut changed = false;
    let mut toggle_err: Option<String> = None;

    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !was_raw {
        enable_raw_mode()?;
    }
    // Roll back raw mode / alt screen manually: a `?` here would skip the
    // cleanup at the end of the function and leak the terminal state.
    let mut stdout_handle = std::io::stdout();
    if let Err(e) = crossterm::execute!(
        stdout_handle,
        EnterAlternateScreen,
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
        crossterm::cursor::Hide
    ) {
        if !was_raw {
            let _ = disable_raw_mode();
        }
        return Err(e.into());
    }
    let _ = crossterm::terminal::size();
    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = match Terminal::new(backend) {
        Ok(t) => t,
        Err(e) => {
            let _ = crossterm::execute!(
                std::io::stdout(),
                LeaveAlternateScreen,
                crossterm::cursor::Show
            );
            if !was_raw {
                let _ = disable_raw_mode();
            }
            return Err(e.into());
        }
    };

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
                let rows = names.len().max(1) as u16;
                // header(1) + gap(1) + rows + gap(1) + footer(1)
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
                let mut cur_y = inner.y + 2;
                let bottom = inner.y + inner_h;
                let footer_y = (inner.y + inner_h).saturating_sub(1);
                // Reserve the line above the footer for a toggle error, if any.
                let rows_cap = if toggle_err.is_some() {
                    footer_y.saturating_sub(1).max(inner.y + 2)
                } else {
                    bottom
                };
                if names.is_empty() {
                    if cur_y < rows_cap {
                        let text = "no plugins installed — /plugin install <spec>";
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
                    for (idx, name) in names.iter().enumerate() {
                        if cur_y >= rows_cap {
                            break;
                        }
                        let is_selected = idx == sel;
                        let row = entries
                            .get(name)
                            .map(|e| format_plugin_row(name, e))
                            .unwrap_or_else(|| name.clone());
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
                            let enabled = entries.get(name).map(|e| e.enabled).unwrap_or(true);
                            let fg = if enabled { Color::White } else { text_dim };
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
                }
                let footer_line = Line::from(vec![
                    Span::styled(
                        "↑↓ ",
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                            .bg(box_bg),
                    ),
                    Span::styled("navigate · ", Style::default().fg(text_dim).bg(box_bg)),
                    Span::styled(
                        "Enter/Space ",
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                            .bg(box_bg),
                    ),
                    Span::styled("toggle · ", Style::default().fg(text_dim).bg(box_bg)),
                    Span::styled(
                        "Esc ",
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                            .bg(box_bg),
                    ),
                    Span::styled("close", Style::default().fg(text_dim).bg(box_bg)),
                ]);
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
                    kind: KeyEventKind::Press,
                    ..
                }) => match code {
                    KeyCode::Up | KeyCode::Char('k') => sel = sel.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => {
                        sel = (sel + 1).min(names.len().saturating_sub(1))
                    }
                    KeyCode::Esc => return Ok(changed),
                    KeyCode::Enter | KeyCode::Char(' ') => {
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
                    _ => {}
                },
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

#[cfg(test)]
mod tests {
    use super::format_plugin_row;
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
}
