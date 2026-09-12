//! Interactive install-manager modal backing `/skills` and `/plugins`:
//! navigate items, two-press `u`/`Delete` uninstalls, plus a read-only
//! Errors tab fed by `gray_pkg::errors`. The skills manager lists all
//! *discovered* skills (global + project dirs); only `~/.gray/skills`
//! entries are removable — `u` on an externally-managed skill says where
//! it lives instead.
//!
//! The two managers were ~85% identical (same chrome, tabs, keys, confirm
//! flow), so they share one parameterized loop ([`run_install_manager`]).
//! Only what differs lives in [`ManagerSpec`] plus the per-manager
//! list/remove/toggle closures: title, empty hint, error verb, whether
//! Enter/Space toggles, and whether a failed re-list after an op keeps the
//! stale list (plugins) or clears it (skills). Sync modal returning whether
//! anything changed.

use super::tabs::{Tab, tab_segments};
use super::*;

use gray_pkg::errors::ErrorEntry;
use gray_pkg::ops::LockEntry;

/// One installed row in the manager's own display terms.
pub(crate) struct ManagerItem {
    pub name: String,
    pub row: String,
    /// Lit rows render white; dim rows (disabled plugins) render dim.
    pub lit: bool,
    /// Current toggle state; only read when the spec supports toggling.
    pub enabled: bool,
}

/// The only axes the skills and plugins managers differ on.
pub(crate) struct ManagerSpec {
    pub title: &'static str,
    pub empty_hint: &'static str,
    /// Verb prefix for the op-error line: "remove failed" / "toggle failed".
    pub error_verb: &'static str,
    pub supports_toggle: bool,
    /// After a successful op, when re-listing fails: keep the stale list
    /// (plugins) or clear it (skills).
    pub keep_stale_on_relist_error: bool,
}

const SKILLS_SPEC: ManagerSpec = ManagerSpec {
    title: "Skills",
    empty_hint: "no skills discovered — /marketplace to browse",
    error_verb: "remove failed",
    supports_toggle: false,
    keep_stale_on_relist_error: false,
};

const PLUGINS_SPEC: ManagerSpec = ManagerSpec {
    title: "Plugins",
    empty_hint: "no plugins installed — /plugin install <spec>",
    error_verb: "toggle failed",
    supports_toggle: true,
    keep_stale_on_relist_error: true,
};

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

/// Bare `/skills` manager: navigate discovered skills, `u`/`Delete`
/// two-press uninstalls `~/.gray/skills` entries via `skills_ops::remove`.
/// Returns true when anything changed (uninstall or errors cleared).
pub fn run_skills_modal(
    bg: Option<&BackgroundSnapshot>,
    cwd: &std::path::Path,
) -> anyhow::Result<bool> {
    run_install_manager(
        bg,
        &SKILLS_SPEC,
        || {
            Some(
                crate::skills::discover_skills(cwd)
                    .skills
                    .iter()
                    .map(|skill| ManagerItem {
                        name: skill.name.clone(),
                        row: crate::skills::format_discovered_skill_row(skill),
                        lit: true,
                        enabled: true,
                    })
                    .collect(),
            )
        },
        |name| {
            match gray_pkg::skills_ops::remove(name) {
                Ok(()) => Ok(()),
                Err(e) => {
                    // Discovered-but-external skills (opencode/agents/claude
                    // dirs) aren't managed here — point at the directory
                    // instead of reporting "not installed".
                    if let Some(hit) = crate::skills::discover_skills(cwd)
                        .skills
                        .iter()
                        .find(|s| s.name == name)
                    {
                        anyhow::bail!(
                            "'{}' lives outside ~/.gray/skills ({}) — delete it manually",
                            name,
                            hit.file_path.display()
                        );
                    }
                    Err(e)
                }
            }
        },
        // Never called: the skills manager is remove-only.
        |_, _| Ok(()),
    )
}

/// Bare `/plugin` picker: navigate installed plugins, Enter/Space toggles
/// `enabled` and stays open. Returns true when anything changed (toggle,
/// uninstall, or errors cleared).
pub fn run_plugins_modal(bg: Option<&BackgroundSnapshot>) -> anyhow::Result<bool> {
    run_install_manager(
        bg,
        &PLUGINS_SPEC,
        || {
            gray_pkg::ops::list().ok().map(|entries| {
                entries
                    .keys()
                    .map(|name| {
                        let entry = entries.get(name);
                        ManagerItem {
                            row: entry
                                .map(|entry| format_plugin_row(name, entry))
                                .unwrap_or_else(|| name.clone()),
                            lit: entry.map(|entry| entry.enabled).unwrap_or(true),
                            enabled: entry.map(|entry| entry.enabled).unwrap_or(true),
                            name: name.clone(),
                        }
                    })
                    .collect()
            })
        },
        gray_pkg::ops::remove,
        gray_pkg::ops::set_enabled,
    )
}

pub(crate) fn run_install_manager(
    bg: Option<&BackgroundSnapshot>,
    spec: &ManagerSpec,
    load: impl Fn() -> Option<Vec<ManagerItem>>,
    remove: impl Fn(&str) -> anyhow::Result<()>,
    set_enabled: impl Fn(&str, bool) -> anyhow::Result<()>,
) -> anyhow::Result<bool> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
    use crossterm::terminal::EnterAlternateScreen;
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use std::time::Duration;

    let mut items = load().unwrap_or_default();
    let mut error_entries = gray_pkg::errors::list();
    let mut tab = Tab::Installed;
    let mut sel = 0usize;
    let mut changed = false;
    let mut op_err: Option<String> = None;
    // Armed uninstall confirm: first `u` arms, second `u` on the same entry
    // removes it.
    let mut pending_remove: Option<String> = None;

    // Refresh after a successful op; a failed re-list keeps the stale list
    // or clears it depending on the spec.
    let relist = |items: &mut Vec<ManagerItem>| {
        if let Some(fresh) = load() {
            *items = fresh;
        } else if !spec.keep_stale_on_relist_error {
            *items = Vec::new();
        }
    };

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
                // Row count of the active tab (empty states render one line).
                let tab_count = match tab {
                    Tab::Installed => items.len(),
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
                let title_str = spec.title;
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
                // Row texts + lit flag (installed rows dim when disabled)
                // for the active tab; one shared render loop below.
                let tab_rows: Vec<(String, bool)> = match tab {
                    Tab::Installed => items
                        .iter()
                        .map(|item| (item.row.clone(), item.lit))
                        .collect(),
                    Tab::Errors => error_entries
                        .iter()
                        .map(|entry| (format_error_row(entry), true))
                        .collect(),
                };
                if tab_rows.is_empty() {
                    if cur_y < rows_cap {
                        let text = match tab {
                            Tab::Installed => spec.empty_hint,
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
                                    .fg(crate::theme::theme().on_selection)
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
                let footer_line = match tab {
                    Tab::Installed if spec.supports_toggle => Line::from(vec![
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
                            Tab::Installed => items.len(),
                            Tab::Errors => error_entries.len(),
                        }
                        .saturating_sub(1);
                        sel = (sel + 1).min(max);
                    }
                    KeyCode::Esc => return Ok(changed),
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        if !spec.supports_toggle {
                            // Manager is remove-only: Enter/Space never run
                            // anything (no toggle like the plugins manager).
                            pending_remove = None;
                        } else {
                            pending_remove = None;
                            if tab != Tab::Installed {
                                // Errors tab is read-only.
                            } else {
                                if items.is_empty() {
                                    return Ok(changed);
                                }
                                let name = items[sel].name.clone();
                                let enabled =
                                    items.get(sel).map(|item| item.enabled).unwrap_or(true);
                                match set_enabled(&name, !enabled) {
                                    Ok(()) => {
                                        changed = true;
                                        op_err = None;
                                        // Re-read and stay open (toggle-stays-open pattern);
                                        // items only ever come from a fresh list().
                                        relist(&mut items);
                                    }
                                    Err(e) => {
                                        op_err = Some(format!("{e:#}"));
                                    }
                                }
                                sel = sel.min(items.len().saturating_sub(1));
                            }
                        }
                    }
                    KeyCode::Char('u') | KeyCode::Delete => {
                        if tab == Tab::Installed && !items.is_empty() {
                            let name = items[sel].name.clone();
                            if pending_remove.as_deref() == Some(name.as_str()) {
                                match remove(&name) {
                                    Ok(()) => {
                                        changed = true;
                                        op_err = None;
                                        pending_remove = None;
                                        relist(&mut items);
                                    }
                                    Err(e) => {
                                        op_err = Some(format!("{e:#}"));
                                        pending_remove = None;
                                    }
                                }
                                sel = sel.min(items.len().saturating_sub(1));
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
    result
}

#[cfg(test)]
mod tests {
    use super::{format_error_row, format_plugin_row};
    use crate::skills::Skill;
    use gray_pkg::errors::ErrorEntry;
    use gray_pkg::ops::LockEntry;

    fn discovered(name: &str, description: &str) -> Skill {
        Skill {
            name: name.to_string(),
            description: description.to_string(),
            file_path: std::path::PathBuf::from("/tmp/skills")
                .join(name)
                .join("SKILL.md"),
            base_dir: std::path::PathBuf::from("/tmp/skills").join(name),
            disable_model_invocation: false,
            source: "user".to_string(),
            args: Vec::new(),
        }
    }

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
    fn manager_specs_differ_only_where_expected() {
        // Contract pin: the two managers share one loop; any new divergence
        // must update this test deliberately.
        let skills = &super::SKILLS_SPEC;
        assert_eq!(skills.title, "Skills");
        assert_eq!(
            skills.empty_hint,
            "no skills discovered — /marketplace to browse"
        );
        assert_eq!(skills.error_verb, "remove failed");
        assert!(!skills.supports_toggle);
        assert!(!skills.keep_stale_on_relist_error);

        let plugins = &super::PLUGINS_SPEC;
        assert_eq!(plugins.title, "Plugins");
        assert_eq!(
            plugins.empty_hint,
            "no plugins installed — /plugin install <spec>"
        );
        assert_eq!(plugins.error_verb, "toggle failed");
        assert!(plugins.supports_toggle);
        assert!(plugins.keep_stale_on_relist_error);
    }

    #[test]
    fn row_shows_name_and_description() {
        assert_eq!(
            crate::skills::format_discovered_skill_row(&discovered("demo-skill", "Do demo things")),
            "demo-skill — Do demo things"
        );
    }

    #[test]
    fn row_falls_back_to_name_without_description() {
        assert_eq!(
            crate::skills::format_discovered_skill_row(&discovered("plain", "")),
            "plain"
        );
    }

    #[test]
    fn row_caps_long_descriptions() {
        let long = "x".repeat(500);
        let row = crate::skills::format_discovered_skill_row(&discovered("big", &long));
        assert!(row.starts_with("big — "), "row: {row:?}");
        assert!(row.chars().count() <= "big — ".len() + 100, "row: {row:?}");
    }

    #[test]
    fn error_row_matches_plugins_format() {
        let row =
            crate::skills::format_discovered_skill_row(&discovered("x", "Does x things"));
        assert_eq!(row, "x — Does x things");
        let err = format_error_row(&ErrorEntry {
            ts_secs: 0,
            source: "skills".to_string(),
            item: "demo".to_string(),
            message: "boom".to_string(),
        });
        assert_eq!(err, "skills demo: boom");
    }

    #[test]
    fn error_row_holds_for_varied_entries() {
        // The old skills modal delegated to the plugins formatter; the
        // single shared renderer must keep parity for any input.
        for (source, item, message) in [
            ("skills", "demo", "boom"),
            ("index", "some-plugin", "fetch failed: 404"),
            ("registry", "", ""),
        ] {
            let row = format_error_row(&ErrorEntry {
                ts_secs: 0,
                source: source.to_string(),
                item: item.to_string(),
                message: message.to_string(),
            });
            assert_eq!(row, format!("{source} {item}: {message}"));
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

    #[test]
    fn enabled_row_exact_string() {
        assert_eq!(
            format_plugin_row("demo", &entry("gray-native", true)),
            "✓ demo 1.2.3 (user) [Gray Index]"
        );
    }

    #[test]
    fn disabled_row_exact_string() {
        assert_eq!(
            format_plugin_row("demo", &entry("pi-gallery", false)),
            "○ demo 1.2.3 (user) [Pi Gallery (preview)] [disabled]"
        );
    }
}
