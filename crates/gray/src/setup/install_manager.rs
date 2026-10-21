use super::tabs::{Tab, tab_segments};
use super::*;

use gray_pkg::errors::ErrorEntry;

#[derive(Clone)]
pub(crate) struct ManagerItem {
    pub name: String,
    pub row: String,

    pub lit: bool,

    pub enabled: bool,

    pub read_only: bool,

    pub needs_setup: bool,
}

pub(crate) type SetupAction<'a> = &'a dyn Fn(&ManagerItem) -> anyhow::Result<()>;

pub(crate) enum EnterAction {
    Setup,

    Toggle,

    Invoke,

    Ignore,
}

pub enum ManagerOutcome {
    Closed(bool),

    Invoke(String),
}

impl ManagerOutcome {
    pub(crate) fn into_changed(self) -> bool {
        matches!(self, ManagerOutcome::Closed(true))
    }
}

#[derive(Debug, Default)]
pub struct PluginModalOutcome {
    pub changed: bool,
    /// Catalog keys newly committed during this visit, excluding concurrent installs.
    pub installed: Vec<String>,
    /// Post-commit warnings do not undo a successful installation.
    pub warnings: Vec<String>,
}

pub(crate) trait ManagerExtension {
    fn enter(&mut self);
    fn tick(&mut self);
    fn draw(&mut self, frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect);
    fn key(&mut self, key: crossterm::event::KeyEvent) -> bool;
    fn request_close(&mut self) -> bool;
    fn take_back(&mut self) -> bool {
        false
    }
    fn animating(&self) -> bool {
        false
    }
}

pub(crate) fn manager_tab_names(has_errors: bool, has_extension: bool) -> Vec<&'static str> {
    let mut names = vec!["Installed"];
    if has_extension {
        names.push("Marketplace");
    }
    if has_errors {
        names.push("Errors");
    }
    names
}

fn manager_panel_tab(names: &[&str], active: usize) -> Tab {
    if names.get(active) == Some(&"Errors") {
        Tab::Errors
    } else {
        Tab::Installed
    }
}

fn manager_tab_for_key(
    active: usize,
    names: &[&str],
    key: crossterm::event::KeyEvent,
    arrows: bool,
) -> Option<usize> {
    use crossterm::event::{KeyCode, KeyModifiers};
    if names.len() < 2 {
        return None;
    }
    let forward = match key.code {
        KeyCode::Right if arrows => true,
        KeyCode::Left if arrows => false,
        KeyCode::Tab => true,
        KeyCode::BackTab => false,
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => true,
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => false,
        _ => return None,
    };
    Some(if forward {
        super::tabs::next_tab(active, names.len())
    } else {
        super::tabs::prev_tab(active, names.len())
    })
}

pub(crate) fn enter_action(
    item: Option<&ManagerItem>,
    has_setup: bool,
    spec: &ManagerSpec,
) -> EnterAction {
    let Some(item) = item else {
        return EnterAction::Ignore;
    };
    if item.read_only {
        return EnterAction::Ignore;
    }
    if item.needs_setup && has_setup {
        return EnterAction::Setup;
    }
    if spec.supports_toggle {
        return EnterAction::Toggle;
    }
    if spec.enter_invokes {
        return EnterAction::Invoke;
    }
    EnterAction::Ignore
}

pub(crate) fn enter_label(
    item: Option<&ManagerItem>,
    has_setup: bool,
    spec: &ManagerSpec,
) -> &'static str {
    match enter_action(item, has_setup, spec) {
        EnterAction::Setup => "set up \u{b7} ",
        EnterAction::Toggle => "toggle \u{b7} ",
        EnterAction::Invoke => "invoke \u{b7} ",
        EnterAction::Ignore => "",
    }
}

pub(crate) struct ManagerSpec {
    pub title: &'static str,
    pub empty_hint: &'static str,

    pub error_verb: &'static str,
    pub supports_toggle: bool,

    pub enter_invokes: bool,

    pub supports_remove: bool,

    pub errors_tab: bool,

    pub keep_stale_on_relist_error: bool,
}

const SKILLS_SPEC: ManagerSpec = ManagerSpec {
    title: "Skills",
    empty_hint: "no skills discovered",
    error_verb: "remove failed",
    supports_toggle: false,
    enter_invokes: true,
    supports_remove: true,
    errors_tab: true,
    keep_stale_on_relist_error: false,
};

const PLUGINS_SPEC: ManagerSpec = ManagerSpec {
    title: "Plugins",
    empty_hint: "no plugins installed — /plugin install <spec>",
    error_verb: "toggle failed",
    supports_toggle: true,
    enter_invokes: false,
    supports_remove: true,
    errors_tab: true,
    keep_stale_on_relist_error: true,
};

fn source_label(ecosystem: &str) -> &str {
    match ecosystem {
        "gray-native" => "Gray Index",
        "gray-cli" => "Plugin command",
        other => other,
    }
}

pub(crate) fn format_plugin_row_parts(
    name: &str,
    version: &str,
    scope: &str,
    ecosystem: &str,
    on: bool,
) -> String {
    let base = format!("{name} {version} ({scope}) [{}]", source_label(ecosystem));
    if on {
        format!("✓ {base}")
    } else {
        format!("○ {base} [disabled]")
    }
}

pub(crate) fn format_error_row(entry: &ErrorEntry) -> String {
    format!("{} {}: {}", entry.source, entry.item, entry.message)
}

pub fn run_skills_modal(
    bg: Option<&BackgroundSnapshot>,
    cwd: &std::path::Path,
) -> anyhow::Result<ManagerOutcome> {
    run_install_manager(
        bg,
        None,
        &SKILLS_SPEC,
        || {
            Some(
                crate::skills::discover_skills(cwd)
                    .skills
                    .iter()
                    .map(|skill| {
                        let auto = crate::setup::skills_auto_enabled();
                        let mut row = crate::skills::format_discovered_skill_row(skill);
                        if !auto {
                            row.push_str(" [auto-off]");
                        } else if crate::setup::disabled_skill_names().contains(&skill.name) {
                            row.push_str(" [disabled]");
                        }
                        ManagerItem {
                            name: skill.name.clone(),
                            row,
                            lit: true,
                            enabled: true,
                            read_only: false,
                            needs_setup: false,
                        }
                    })
                    .collect(),
            )
        },
        |name| match gray_pkg::skills_ops::remove(name) {
            Ok(()) => Ok(()),
            Err(e) => {
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
        },
        |_, _| Ok(()),
    )
}

pub fn run_plugins_modal(bg: Option<&BackgroundSnapshot>) -> anyhow::Result<PluginModalOutcome> {
    // The synchronous modal must not occupy a Tokio executor thread while it
    // waits for its owned worker, including on terminal errors.
    tokio::task::block_in_place(|| {
        let mut marketplace = super::marketplace::Marketplace::new(crate::plugin_cli::home()?);
        let result = run_install_manager_extended(
            bg,
            None,
            &PLUGINS_SPEC,
            || {
                crate::plugin_cli::list_rows().ok().map(|rows| {
                    rows.into_iter()
                        .map(|r| ManagerItem {
                            row: format_plugin_row_parts(
                                &r.name,
                                &r.version,
                                &r.scope,
                                &r.ecosystem,
                                r.on,
                            ),
                            lit: r.on,
                            enabled: r.on,
                            read_only: false,
                            needs_setup: false,
                            name: r.name,
                        })
                        .collect()
                })
            },
            crate::plugin_cli::remove_managed,
            crate::plugin_cli::set_managed_enabled,
            Some(&mut marketplace),
        );
        marketplace.shutdown();
        let changed = result?.into_changed();
        let mut outcome = marketplace.into_outcome();
        outcome.changed |= changed;
        Ok(outcome)
    })
}

pub(crate) fn run_install_manager(
    bg: Option<&BackgroundSnapshot>,
    setup: Option<SetupAction<'_>>,
    spec: &ManagerSpec,
    load: impl Fn() -> Option<Vec<ManagerItem>>,
    remove: impl Fn(&str) -> anyhow::Result<()>,
    set_enabled: impl Fn(&str, bool) -> anyhow::Result<()>,
) -> anyhow::Result<ManagerOutcome> {
    run_install_manager_extended(bg, setup, spec, load, remove, set_enabled, None)
}

fn run_install_manager_extended(
    bg: Option<&BackgroundSnapshot>,
    setup: Option<SetupAction<'_>>,
    spec: &ManagerSpec,
    load: impl Fn() -> Option<Vec<ManagerItem>>,
    remove: impl Fn(&str) -> anyhow::Result<()>,
    set_enabled: impl Fn(&str, bool) -> anyhow::Result<()>,
    mut extension: Option<&mut dyn ManagerExtension>,
) -> anyhow::Result<ManagerOutcome> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
    use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use std::time::Duration;

    let mut items = load().unwrap_or_default();
    let mut error_entries = gray_pkg::errors::list();
    let mut tab = Tab::Installed;
    let tab_names = manager_tab_names(spec.errors_tab, extension.is_some());
    let extension_index = tab_names.iter().position(|name| *name == "Marketplace");
    let mut active_tab = 0;
    let mut closing = false;
    let mut sel = 0usize;
    let mut changed = false;
    let mut op_err: Option<String> = None;

    let mut pending_remove: Option<String> = None;

    let relist = |items: &mut Vec<ManagerItem>| {
        if let Some(fresh) = load() {
            *items = fresh;
        } else if !spec.keep_stale_on_relist_error {
            *items = Vec::new();
        }
    };

    let (_session, mut terminal) = super::open_modal()?;

    let box_bg = crate::theme::theme().surface_bg;
    let accent_peach = crate::theme::theme().accent;
    let text_dim = crate::theme::theme().text_dim;
    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(BackgroundSnapshot::default_initial);

    let result = (|| -> anyhow::Result<ManagerOutcome> {
        loop {
            if let Some(extra) = extension.as_deref_mut() {
                extra.tick();
                if closing && extra.request_close() {
                    return Ok(ManagerOutcome::Closed(changed));
                }
            }
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

                let tab_count = match tab {
                    Tab::Installed => items.len(),
                    Tab::Errors => error_entries.len(),
                };
                let is_extension = Some(active_tab) == extension_index;
                let rows = if is_extension {
                    18
                } else {
                    tab_count.max(1).min(u16::MAX as usize) as u16
                };

                let needed_h = rows.saturating_add(if tab_names.len() > 1 { 6 } else { 5 });
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

                if tab_names.len() > 1 {
                    let tabs = tab_names
                        .iter()
                        .map(|name| {
                            (
                                *name,
                                if *name == "Errors" && !error_entries.is_empty() {
                                    Some(error_entries.len())
                                } else {
                                    None
                                },
                            )
                        })
                        .collect::<Vec<_>>();
                    let segs = tab_segments(&tabs, active_tab);
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
                                .fg(crate::theme::theme().text_bright)
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
                let mut cur_y = inner.y + if tab_names.len() > 1 { 2 } else { 1 };
                let bottom = inner.y + inner_h;
                let footer_y = (inner.y + inner_h).saturating_sub(1);
                if is_extension {
                    if let Some(extra) = extension.as_deref_mut() {
                        extra.draw(
                            frame,
                            Rect::new(inner.x, cur_y, inner.width, bottom.saturating_sub(cur_y)),
                        );
                    }
                    return;
                }

                let rows_cap = if op_err.is_some() || pending_remove.is_some() {
                    footer_y.saturating_sub(1).max(inner.y + 2)
                } else {
                    bottom
                };

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
                            let fg = if *lit {
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

                let enter = enter_label(items.get(sel), setup.is_some(), spec);
                let middle: Vec<(&str, &str)> = match tab {
                    Tab::Errors => vec![("c ", "clear · ")],
                    Tab::Installed => {
                        let mut arms = Vec::new();
                        if !enter.is_empty() {
                            arms.push(("Enter ", enter));
                        }
                        if spec.supports_remove {
                            arms.push(("u ", "remove · "));
                        }
                        arms
                    }
                };
                let mut footer_spans = vec![Span::styled(
                    "↑↓ ",
                    Style::default()
                        .fg(crate::theme::theme().text_bright)
                        .add_modifier(Modifier::BOLD)
                        .bg(box_bg),
                )];
                footer_spans.extend(kv("nav · ", "", box_bg, text_dim));
                for (k, v) in middle {
                    footer_spans.extend(kv(k, v, box_bg, text_dim));
                }
                if tab_names.len() > 1 {
                    footer_spans.extend(kv("Tab ", "· ", box_bg, text_dim));
                }
                footer_spans.extend(kv("Esc ", "close", box_bg, text_dim));
                let footer_line = Line::from(footer_spans);
                frame.render_widget(
                    Paragraph::new(footer_line),
                    Rect::new(inner.x, footer_y, inner.width, 1),
                );
            })?;
            let animating = Some(active_tab) == extension_index
                && extension.as_deref().is_some_and(|extra| extra.animating());
            if !poll(Duration::from_millis(if animating { 33 } else { 100 }))? {
                continue;
            }
            let event = read()?;
            if closing {
                continue;
            }
            if Some(active_tab) == extension_index
                && let Event::Key(key) = event
                && key.kind == KeyEventKind::Press
                && let Some(extra) = extension.as_deref_mut()
            {
                let consumed = extra.key(key);
                if extra.take_back() {
                    active_tab = 0;
                    tab = Tab::Installed;
                    relist(&mut items);
                    error_entries = gray_pkg::errors::list();
                    sel = 0;
                    continue;
                }
                if consumed {
                    continue;
                }
            }
            if let Event::Key(key) = event
                && key.kind == KeyEventKind::Press
                && let Some(next) =
                    manager_tab_for_key(active_tab, &tab_names, key, extension.is_some())
            {
                active_tab = next;
                tab = manager_panel_tab(&tab_names, active_tab);
                if Some(active_tab) == extension_index {
                    if let Some(extra) = extension.as_deref_mut() {
                        extra.enter();
                    }
                } else if extension.is_some() {
                    relist(&mut items);
                }
                error_entries = gray_pkg::errors::list();
                sel = 0;
                pending_remove = None;
                continue;
            }
            match event {
                Event::Key(KeyEvent {
                    code: KeyCode::Char('c'),
                    modifiers,
                    kind: KeyEventKind::Press,
                    ..
                }) if modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some(extra) = extension.as_deref_mut()
                        && !extra.request_close()
                    {
                        closing = true;
                        continue;
                    }
                    return Ok(ManagerOutcome::Closed(changed));
                }
                Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Press,
                    ..
                }) => match code {
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
                    KeyCode::Esc => {
                        if let Some(extra) = extension.as_deref_mut()
                            && !extra.request_close()
                        {
                            closing = true;
                            continue;
                        }
                        return Ok(ManagerOutcome::Closed(changed));
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        pending_remove = None;
                        match enter_action(items.get(sel), setup.is_some(), spec) {
                            EnterAction::Setup => {
                                let item = items[sel].clone();

                                let _ = crossterm::execute!(
                                    std::io::stdout(),
                                    LeaveAlternateScreen,
                                    crossterm::cursor::Show
                                );
                                let outcome = (setup.unwrap())(&item);
                                let _ = crossterm::execute!(
                                    std::io::stdout(),
                                    EnterAlternateScreen,
                                    crossterm::cursor::Hide
                                );
                                match outcome {
                                    Ok(()) => {
                                        changed = true;
                                        op_err = None;

                                        relist(&mut items);
                                    }
                                    Err(e) => {
                                        op_err = Some(format!("{e:#}"));
                                    }
                                }
                                sel = sel.min(items.len().saturating_sub(1));
                            }
                            EnterAction::Toggle => {
                                let name = items[sel].name.clone();
                                let enabled = items[sel].enabled;
                                match set_enabled(&name, !enabled) {
                                    Ok(()) => {
                                        changed = true;
                                        op_err = None;

                                        relist(&mut items);
                                    }
                                    Err(e) => {
                                        op_err = Some(format!("{e:#}"));
                                    }
                                }
                                sel = sel.min(items.len().saturating_sub(1));
                            }
                            EnterAction::Invoke => {
                                if tab == Tab::Installed {
                                    let name = items[sel].name.clone();
                                    return Ok(ManagerOutcome::Invoke(name));
                                }
                            }
                            EnterAction::Ignore => {}
                        }
                    }
                    KeyCode::Char('u') | KeyCode::Delete => {
                        if tab == Tab::Installed
                            && spec.supports_remove
                            && !items.is_empty()
                            && !items[sel].read_only
                        {
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
    if let Some(extra) = extension {
        while !extra.request_close() {
            extra.tick();
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let _ = terminal.clear();
    result
}

#[path = "install_manager_tests.rs"]
#[cfg(test)]
mod tests;
