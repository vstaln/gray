//! Interactive `/skills` manager modal: uninstall installed skills,
//! plus a read-only Errors tab fed by `gray_pkg::errors`.
//!
//! Mirrors the `plugins_modal` chrome and key patterns on the shared
//! [`super::tabs`] scaffolding (`Tab` + `tab_segments` — no forked tab
//! logic). Sync modal returning whether anything changed.

use super::tabs::{Tab, tab_segments};
use super::*;

use gray_pkg::errors::ErrorEntry;
use gray_pkg::skills_ops::InstalledSkill;

/// Pure row renderer: `name version [source]`, version omitted when empty
/// (hand-placed skills without an origin sidecar).
pub(crate) fn format_skill_row(skill: &InstalledSkill) -> String {
    if skill.version.trim().is_empty() {
        format!("{} [{}]", skill.name, skill.source)
    } else {
        format!("{} {} [{}]", skill.name, skill.version, skill.source)
    }
}

/// Pure row renderer for the Errors tab: `<source> <item>: <message>`
/// (same row format as the plugins manager).
pub(crate) fn format_error_row(entry: &ErrorEntry) -> String {
    super::plugins_modal::format_error_row(entry)
}

/// Bare `/skills` manager: navigate installed skills, `u`/`Delete`
/// two-press uninstalls via `skills_ops::remove`. Returns true when
/// anything changed (uninstall or errors cleared).
pub fn run_skills_modal(bg: Option<&BackgroundSnapshot>) -> anyhow::Result<bool> {
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

    let mut skills = gray_pkg::skills_ops::list().unwrap_or_default();
    let mut error_entries = gray_pkg::errors::list();
    let mut tab = Tab::Installed;
    let mut sel = 0usize;
    let mut changed = false;
    let mut op_err: Option<String> = None;
    // Armed uninstall confirm: first `u` arms, second `u` on the same entry
    // removes it.
    let mut pending_remove: Option<String> = None;

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
                // Row count of the active tab (empty states render one line).
                let tab_count = match tab {
                    Tab::Installed => skills.len(),
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
                let title_str = "Skills";
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
                // Reserve the line above the footer for an op error or an
                // armed uninstall confirm, if any.
                let rows_cap = if op_err.is_some() || pending_remove.is_some() {
                    footer_y.saturating_sub(1).max(inner.y + 2)
                } else {
                    bottom
                };
                // Row texts for the active tab; one shared render loop below.
                let tab_rows: Vec<String> = match tab {
                    Tab::Installed => skills.iter().map(format_skill_row).collect(),
                    Tab::Errors => error_entries.iter().map(format_error_row).collect(),
                };
                if tab_rows.is_empty() {
                    if cur_y < rows_cap {
                        let text = match tab {
                            Tab::Installed => "no skills installed — /marketplace to browse",
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
                    for (idx, row) in tab_rows.iter().enumerate() {
                        if cur_y >= rows_cap {
                            break;
                        }
                        let is_selected = idx == sel;
                        // Truncate to the inner width so long rows never wrap.
                        let row: String = row.chars().take(inner.width as usize).collect();
                        let fill = (inner.width as usize).saturating_sub(row.chars().count());
                        let row_line = if is_selected {
                            Line::from(Span::styled(
                                format!("{row}{}", " ".repeat(fill)),
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(accent_peach)
                                    .add_modifier(Modifier::BOLD),
                            ))
                        } else {
                            Line::from(vec![
                                Span::styled(row, Style::default().fg(Color::White).bg(box_bg)),
                                Span::styled(" ".repeat(fill), Style::default().bg(box_bg)),
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
                        let text: String = format!("remove failed: {msg}")
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
                            Tab::Installed => skills.len(),
                            Tab::Errors => error_entries.len(),
                        }
                        .saturating_sub(1);
                        sel = (sel + 1).min(max);
                    }
                    KeyCode::Esc => return Ok(changed),
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        // Manager is remove-only: Enter/Space never run
                        // anything (no toggle like the plugins manager).
                        pending_remove = None;
                    }
                    KeyCode::Char('u') | KeyCode::Delete => {
                        if tab == Tab::Installed && !skills.is_empty() {
                            let name = skills[sel].name.clone();
                            if pending_remove.as_deref() == Some(name.as_str()) {
                                match gray_pkg::skills_ops::remove(&name) {
                                    Ok(()) => {
                                        changed = true;
                                        op_err = None;
                                        pending_remove = None;
                                        skills = gray_pkg::skills_ops::list().unwrap_or_default();
                                    }
                                    Err(e) => {
                                        op_err = Some(format!("{e:#}"));
                                        pending_remove = None;
                                    }
                                }
                                sel = sel.min(skills.len().saturating_sub(1));
                            } else {
                                op_err = None;
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
    use super::format_skill_row;
    use gray_pkg::skills_ops::{InstalledSkill, SkillOrigin};

    fn installed(name: &str, version: &str, source: &str) -> InstalledSkill {
        InstalledSkill {
            name: name.to_string(),
            version: version.to_string(),
            source: source.to_string(),
            origin: None,
        }
    }

    fn with_origin() -> InstalledSkill {
        InstalledSkill {
            name: "demo-skill".to_string(),
            version: "1.2.3".to_string(),
            source: "clawhub".to_string(),
            origin: Some(SkillOrigin {
                version: 1,
                registry: "clawhub".to_string(),
                slug: "demo-skill".to_string(),
                owner: "arein".to_string(),
                installed_version: "1.2.3".to_string(),
                installed_at: "0".to_string(),
                source_url: "https://clawhub.ai/arein/skills/demo-skill".to_string(),
            }),
        }
    }

    #[test]
    fn row_shows_name_version_and_source() {
        assert_eq!(
            format_skill_row(&installed("demo-skill", "1.2.3", "clawhub")),
            "demo-skill 1.2.3 [clawhub]"
        );
    }

    #[test]
    fn row_omits_empty_version_for_hand_placed_skills() {
        assert_eq!(
            format_skill_row(&installed("local-skill", "", "local")),
            "local-skill [local]"
        );
    }

    #[test]
    fn row_shows_origin_version_when_known() {
        // `list()` carries the origin version in `version`; the row shows it.
        let skill = with_origin();
        let row = format_skill_row(&skill);
        assert!(row.contains("demo-skill 1.2.3 [clawhub]"), "row: {row:?}");
        let origin = skill.origin.as_ref().expect("origin");
        assert_eq!(origin.installed_version, "1.2.3");
    }

    #[test]
    fn error_row_matches_plugins_format() {
        let row = format_skill_row(&installed("x", "0.0.0", "local"));
        assert_eq!(row, "x 0.0.0 [local]");
        let err = super::format_error_row(&gray_pkg::errors::ErrorEntry {
            ts_secs: 0,
            source: "skills".to_string(),
            item: "demo".to_string(),
            message: "boom".to_string(),
        });
        assert_eq!(err, "skills demo: boom");
    }
}
