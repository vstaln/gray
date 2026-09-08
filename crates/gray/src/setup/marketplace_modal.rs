//! Interactive `/marketplace` store modal: browse+install plugins/skills,
//! plus a read-only Marketplaces tab with source reachability.
//!
//! Tabs `Plugins | Skills | Marketplaces` on the shared [`super::tabs`]
//! scaffolding (`tab_segments` + `next_tab`/`prev_tab` — no reimplemented
//! tab logic). Sync modal returning whether anything was installed,
//! mirroring the `plugins_modal` chrome and init/rollback discipline.
//! Async backends (`ops::search_all`, `skills_ops::search/install`,
//! `sources::status`) run on spawned threads via the current runtime
//! handle (same thread+`block_on` shape as the provider live-fetch path)
//! while the modal shows `* Loading...` / `installing...` / `checking...`.

use super::tabs::{next_tab, prev_tab, tab_segments};
use super::*;

use gray_pkg::ops::{Report, SearchHit, SearchOutput};
use gray_pkg::skills_ops::SkillHit;
use gray_pkg::sources::Source;
use std::sync::mpsc::Receiver;

/// Store tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarketTab {
    Plugins,
    Skills,
    Marketplaces,
}

impl MarketTab {
    const COUNT: usize = 3;

    fn index(self) -> usize {
        match self {
            MarketTab::Plugins => 0,
            MarketTab::Skills => 1,
            MarketTab::Marketplaces => 2,
        }
    }

    fn from_index(i: usize) -> Self {
        match i % Self::COUNT {
            0 => MarketTab::Plugins,
            1 => MarketTab::Skills,
            _ => MarketTab::Marketplaces,
        }
    }

    fn next(self) -> Self {
        Self::from_index(next_tab(self.index(), Self::COUNT))
    }

    fn prev(self) -> Self {
        Self::from_index(prev_tab(self.index(), Self::COUNT))
    }
}

/// Pure row renderer, shared by both search tabs:
/// `name version [source] - desc`, desc suffix omitted when empty
/// (mirrors `ops::format_search_hit` without depending on its enum).
pub(crate) fn format_market_row(
    name: &str,
    version: &str,
    source_label: &str,
    desc: &str,
) -> String {
    let base = format!("{name} {version} [{source_label}]");
    let desc = desc.trim();
    if desc.is_empty() {
        base
    } else {
        format!("{base} - {desc}")
    }
}

/// Pure plugin preview: name/version/source/desc plus files, trust,
/// version detail and required bins/env — each trailing line only when
/// known (non-empty). `requires` is empty in the current `search_all`
/// flow (hits carry no requires); the param keeps the pane honest when
/// the backend learns them.
pub(crate) fn format_preview(hit: &SearchHit, requires: &[String]) -> String {
    let mut lines = vec![format!(
        "{} {} [{}]",
        hit.name,
        hit.version,
        hit.source.label()
    )];
    if !hit.desc.trim().is_empty() {
        lines.push(hit.desc.trim().to_string());
    }
    if !hit.version_detail.trim().is_empty() {
        lines.push(format!("detail: {}", hit.version_detail.trim()));
    }
    if !hit.files.is_empty() {
        lines.push(format!("files: {}", hit.files.join(", ")));
    }
    if !hit.trust.trim().is_empty() {
        lines.push(format!("trust: {}", hit.trust.trim()));
    }
    if !requires.is_empty() {
        lines.push(format!("requires: {}", requires.join(", ")));
    }
    lines.join("\n")
}

/// Pure skill preview: desc, source, trust, origin note — trailing lines
/// only when known.
pub(crate) fn format_skill_preview(hit: &SkillHit, origin_note: &str) -> String {
    let mut lines = vec![format!("{} {} [{}]", hit.name, hit.version, hit.source)];
    if !hit.desc.trim().is_empty() {
        lines.push(hit.desc.trim().to_string());
    }
    if !hit.trust.trim().is_empty() {
        lines.push(format!("trust: {}", hit.trust.trim()));
    }
    if !origin_note.trim().is_empty() {
        lines.push(origin_note.trim().to_string());
    }
    lines.join("\n")
}

/// Pure marketplace status row: `Label: ok` / `Label: unreachable`,
/// `checking...` while the background re-check runs.
pub(crate) fn format_source_row(label: &str, ok: Option<bool>) -> String {
    match ok {
        Some(true) => format!("{label}: ok"),
        Some(false) => format!("{label}: unreachable"),
        None => format!("{label}: checking..."),
    }
}

/// Pure install outcome line: `active` on success, `failed: {e}` inline.
pub(crate) fn format_install_status(err: Option<&str>) -> String {
    match err {
        None => "active".to_string(),
        Some(e) => format!("failed: {e}"),
    }
}

/// Install spec for a plugin hit: Gray names install bare, Pi via `npm:`,
/// ClawHub via `clawhub:`, Claude via `claude:`.
pub(crate) fn install_spec_for_plugin(hit: &SearchHit) -> String {
    use gray_pkg::ops::SearchSource;
    match hit.source {
        SearchSource::Gray => hit.name.clone(),
        SearchSource::Pi => format!("npm:{}", hit.name),
        SearchSource::ClawHub => format!("clawhub:{}", hit.name),
        SearchSource::Claude => format!("claude:{}", hit.name),
    }
}

/// Install spec for a skill hit (`skills_ops::install`): ClawHub via
/// `clawhub:`, Claude via `claude:` (may fail inline — reported, never a
/// modal crash — until the backend learns Claude skill bundles).
pub(crate) fn install_spec_for_skill(hit: &SkillHit) -> String {
    if hit.source == "ClawHub" {
        format!("clawhub:{}", hit.name)
    } else {
        format!("claude:{}", hit.name)
    }
}

const MARKET_SOURCES: [Source; 4] = [
    Source::GrayIndex,
    Source::PiGallery,
    Source::ClawHub,
    Source::ClaudeRepo,
];

/// Bare `/marketplace` store: search Plugins/Skills, preview hits,
/// install inline, check marketplace reachability. Returns true when
/// anything was installed.
pub fn run_marketplace_modal(bg: Option<&BackgroundSnapshot>) -> anyhow::Result<bool> {
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
    use std::sync::mpsc::channel;
    use std::time::Duration;

    // Pending async work: at most one search/install/status flight at a
    // time; tab switches drop the receiver (stale results are ignored).
    let mut pending_search: Option<SearchFlight> = None;
    let mut pending_install: Option<Receiver<anyhow::Result<Report>>> = None;
    let mut pending_status: Option<Receiver<[bool; 4]>> = None;

    let mut tab = MarketTab::Plugins;
    let mut plugin_query = String::new();
    let mut skill_query = String::new();
    let mut plugin_hits: Vec<SearchHit> = Vec::new();
    let mut skill_hits: Vec<SkillHit> = Vec::new();
    let mut plugin_dirty = true;
    let mut skill_dirty = true;
    let mut plugin_unreachable: Vec<String> = Vec::new();
    let mut preview: Option<usize> = None;
    let mut install_msg: Option<String> = None;
    let mut search_err: Option<String> = None;
    let mut statuses: [Option<bool>; 4] = [None, None, None, None];
    let mut sel = 0usize;
    let mut changed = false;

    // Runtime handle for spawned `block_on` flights (same shape as the
    // provider live-fetch path: `try_current` here, `block_on` on a fresh
    // OS thread so the modal loop never blocks the runtime worker).
    let rt = tokio::runtime::Handle::try_current().ok();
    // Kick off the first marketplace reachability check immediately so
    // the Marketplaces tab rarely shows `checking...`.
    if let Some(handle) = rt.clone() {
        let (tx, rx) = channel();
        pending_status = Some(rx);
        std::thread::spawn(move || {
            let out = handle.block_on(async {
                tokio::join!(
                    gray_pkg::sources::status(Source::GrayIndex),
                    gray_pkg::sources::status(Source::PiGallery),
                    gray_pkg::sources::status(Source::ClawHub),
                    gray_pkg::sources::status(Source::ClaudeRepo),
                )
            });
            let _ = tx.send([out.0, out.1, out.2, out.3]);
        });
    }

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
            // Poll background flights (non-blocking): search results land
            // the hit list, installs land the inline status, statuses land
            // the Marketplaces rows.
            if let Some(flight) = pending_search.take() {
                match flight.try_recv() {
                    SearchPoll::Pending(rx) => {
                        pending_search = Some(match rx {
                            SearchRx::Plugin(r) => SearchFlight::Plugin(r),
                            SearchRx::Skill(r) => SearchFlight::Skill(r),
                        })
                    }
                    SearchPoll::PluginReady(res) => match res {
                        Ok(out) => {
                            plugin_unreachable = unreachable_lines(&out);
                            plugin_hits = out.hits;
                            plugin_dirty = false;
                            search_err = None;
                            sel = 0;
                        }
                        Err(e) => {
                            search_err = Some(format!("{e:#}"));
                            plugin_hits.clear();
                            plugin_unreachable.clear();
                        }
                    },
                    SearchPoll::SkillReady(res) => match res {
                        Ok(hits) => {
                            skill_hits = hits;
                            skill_dirty = false;
                            search_err = None;
                            sel = 0;
                        }
                        Err(e) => {
                            search_err = Some(format!("{e:#}"));
                            skill_hits.clear();
                        }
                    },
                }
            }
            if let Some(rx) = pending_install.take() {
                match rx.try_recv() {
                    Ok(res) => match res {
                        Ok(_) => {
                            changed = true;
                            install_msg = Some(format_install_status(None));
                        }
                        Err(e) => {
                            install_msg = Some(format_install_status(Some(&format!("{e:#}"))));
                        }
                    },
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        pending_install = Some(rx);
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        install_msg = Some(format_install_status(Some("background task ended")));
                    }
                }
            }
            if let Some(rx) = pending_status.take() {
                match rx.try_recv() {
                    Ok(arr) => {
                        statuses = [Some(arr[0]), Some(arr[1]), Some(arr[2]), Some(arr[3])];
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        pending_status = Some(rx);
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
                }
            }
            let searching = pending_search.is_some();
            let installing = pending_install.is_some();

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
                // Row count of the active view (preview lines count as rows).
                let tab_count = match tab {
                    MarketTab::Plugins => {
                        if preview.is_some() {
                            preview_lines_plugins(&plugin_hits, preview).len()
                        } else if searching || !plugin_hits.is_empty() || search_err.is_some() {
                            plugin_hits.len().max(1) + 2
                        } else {
                            2
                        }
                    }
                    MarketTab::Skills => {
                        if preview.is_some() {
                            preview_lines_skills(&skill_hits, preview).len()
                        } else if searching || !skill_hits.is_empty() || search_err.is_some() {
                            skill_hits.len().max(1) + 2
                        } else {
                            2
                        }
                    }
                    MarketTab::Marketplaces => MARKET_SOURCES.len(),
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
                let title_str = "Marketplace";
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
                // Tab bar on the line below the header (shared scaffolding).
                {
                    let tabs = [("Plugins", None), ("Skills", None), ("Marketplaces", None)];
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
                // Reserve the line above the footer for the install status.
                let rows_cap = if install_msg.is_some() {
                    footer_y.saturating_sub(1).max(inner.y + 2)
                } else {
                    bottom
                };
                // Body rows for the active view.
                let body: Vec<(String, bool)> = match tab {
                    MarketTab::Plugins => {
                        if let Some(idx) = preview {
                            preview_lines_plugins(&plugin_hits, Some(idx))
                                .into_iter()
                                .map(|l| (l, true))
                                .collect()
                        } else {
                            let mut out = vec![(query_line(&plugin_query), true)];
                            if searching {
                                out.push(("* Loading...".to_string(), true));
                            } else if let Some(e) = search_err.as_deref() {
                                out.push((format!("search failed: {e}"), true));
                            } else if plugin_hits.is_empty() {
                                out.push((
                                    if plugin_dirty {
                                        "type a query + Enter to search".to_string()
                                    } else {
                                        "no hits — try another query".to_string()
                                    },
                                    true,
                                ));
                            } else {
                                for h in &plugin_hits {
                                    out.push((
                                        format_market_row(
                                            &h.name,
                                            &h.version,
                                            h.source.label(),
                                            &h.desc,
                                        ),
                                        true,
                                    ));
                                }
                                for line in &plugin_unreachable {
                                    out.push((line.clone(), true));
                                }
                            }
                            out
                        }
                    }
                    MarketTab::Skills => {
                        if let Some(idx) = preview {
                            preview_lines_skills(&skill_hits, Some(idx))
                                .into_iter()
                                .map(|l| (l, true))
                                .collect()
                        } else {
                            let mut out = vec![(query_line(&skill_query), true)];
                            if searching {
                                out.push(("* Loading...".to_string(), true));
                            } else if let Some(e) = search_err.as_deref() {
                                out.push((format!("search failed: {e}"), true));
                            } else if skill_hits.is_empty() {
                                out.push((
                                    if skill_dirty {
                                        "type a query + Enter to search".to_string()
                                    } else {
                                        "no hits — try another query".to_string()
                                    },
                                    true,
                                ));
                            } else {
                                for h in &skill_hits {
                                    out.push((
                                        format_market_row(&h.name, &h.version, &h.source, &h.desc),
                                        true,
                                    ));
                                }
                            }
                            out
                        }
                    }
                    MarketTab::Marketplaces => MARKET_SOURCES
                        .iter()
                        .enumerate()
                        .map(|(i, s)| (format_source_row(s.label(), statuses[i]), true))
                        .collect(),
                };
                // In list views the first row is the query line (never
                // highlighted); selection starts at row 1. In preview and
                // marketplaces views every row is content.
                let list_offset = match tab {
                    MarketTab::Plugins | MarketTab::Skills if preview.is_none() => 1,
                    _ => 0,
                };
                if body.is_empty() {
                    // Unreachable: every branch emits at least one row.
                } else if preview.is_some() || tab == MarketTab::Marketplaces {
                    for (text, lit) in body.iter() {
                        if cur_y >= rows_cap {
                            break;
                        }
                        render_row(
                            frame,
                            inner,
                            cur_y,
                            text,
                            false,
                            *lit,
                            box_bg,
                            accent_peach,
                            text_dim,
                        );
                        cur_y += 1;
                    }
                    // Marketplaces selection highlight.
                    if tab == MarketTab::Marketplaces && MARKET_SOURCES.len() > 1 {
                        let row_y = inner.y + 2 + (sel as u16);
                        if row_y < rows_cap
                            && sel < MARKET_SOURCES.len()
                            && let Some(text) = body.get(sel).map(|(t, _)| t.clone())
                        {
                            render_row(
                                frame,
                                inner,
                                row_y,
                                &text,
                                true,
                                true,
                                box_bg,
                                accent_peach,
                                text_dim,
                            );
                        }
                    }
                } else {
                    // Hits length for selection (hint/loading rows are never
                    // highlighted).
                    let hits_len = match tab {
                        MarketTab::Plugins => plugin_hits.len(),
                        MarketTab::Skills => skill_hits.len(),
                        MarketTab::Marketplaces => 0,
                    };
                    for (idx, (text, lit)) in body.iter().enumerate() {
                        if cur_y >= rows_cap {
                            break;
                        }
                        let is_selected = idx >= list_offset
                            && (idx - list_offset) == sel
                            && (idx - list_offset) < hits_len;
                        render_row(
                            frame,
                            inner,
                            cur_y,
                            text,
                            is_selected,
                            *lit,
                            box_bg,
                            accent_peach,
                            text_dim,
                        );
                        cur_y += 1;
                    }
                }
                if let Some(msg) = install_msg.as_deref() {
                    let msg_y = footer_y.saturating_sub(1);
                    if msg_y > inner.y + 1 {
                        let text: String = msg.chars().take(inner.width as usize).collect();
                        let fill = (inner.width as usize).saturating_sub(text.chars().count());
                        let fg = if msg == "active" || msg == "installing..." {
                            accent_peach
                        } else {
                            Color::Rgb(220, 120, 120)
                        };
                        let style = Style::default()
                            .fg(fg)
                            .add_modifier(Modifier::BOLD)
                            .bg(box_bg);
                        frame.render_widget(
                            Paragraph::new(Line::from(vec![
                                Span::styled(text, style),
                                Span::styled(" ".repeat(fill), Style::default().bg(box_bg)),
                            ])),
                            Rect::new(inner.x, msg_y, inner.width, 1),
                        );
                    }
                }
                let footer_line = match tab {
                    MarketTab::Plugins | MarketTab::Skills if preview.is_some() => {
                        if installing {
                            footer_spans(box_bg, text_dim, vec![("installing... ", true)])
                        } else {
                            footer_spans(
                                box_bg,
                                text_dim,
                                vec![
                                    ("i ", true),
                                    ("install · ", false),
                                    ("Esc ", true),
                                    ("back", false),
                                ],
                            )
                        }
                    }
                    MarketTab::Plugins | MarketTab::Skills => footer_spans(
                        box_bg,
                        text_dim,
                        vec![
                            ("↑↓ ", true),
                            ("nav · ", false),
                            ("Enter ", true),
                            ("search/open · ", false),
                            ("Tab ", true),
                            ("· ", false),
                            ("Esc ", true),
                            ("close", false),
                        ],
                    ),
                    MarketTab::Marketplaces => footer_spans(
                        box_bg,
                        text_dim,
                        vec![
                            ("↑↓ ", true),
                            ("nav · ", false),
                            ("u ", true),
                            ("re-check · ", false),
                            ("Tab ", true),
                            ("· ", false),
                            ("Esc ", true),
                            ("close", false),
                        ],
                    ),
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
                        pending_search = None;
                        preview = None;
                        install_msg = None;
                        search_err = None;
                        sel = 0;
                    }
                    KeyCode::BackTab => {
                        tab = tab.prev();
                        pending_search = None;
                        preview = None;
                        install_msg = None;
                        search_err = None;
                        sel = 0;
                    }
                    KeyCode::Char('n') | KeyCode::Char('p')
                        if modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        tab = if code == KeyCode::Char('n') {
                            tab.next()
                        } else {
                            tab.prev()
                        };
                        pending_search = None;
                        preview = None;
                        install_msg = None;
                        search_err = None;
                        sel = 0;
                    }
                    KeyCode::Up if preview.is_none() => {
                        sel = sel.saturating_sub(1);
                    }
                    KeyCode::Down if preview.is_none() => {
                        let max = match tab {
                            MarketTab::Plugins => plugin_hits.len(),
                            MarketTab::Skills => skill_hits.len(),
                            MarketTab::Marketplaces => MARKET_SOURCES.len(),
                        }
                        .saturating_sub(1);
                        sel = (sel + 1).min(max);
                    }
                    KeyCode::Esc => {
                        if let Some(idx) = preview.take() {
                            install_msg = None;
                            pending_install = None;
                            sel = idx.min(
                                match tab {
                                    MarketTab::Plugins => plugin_hits.len(),
                                    MarketTab::Skills => skill_hits.len(),
                                    MarketTab::Marketplaces => 0,
                                }
                                .saturating_sub(1),
                            );
                        } else {
                            return Ok(changed);
                        }
                    }
                    KeyCode::Enter => {
                        if preview.is_some() || installing || searching {
                            // Preview installs via `i`; Enter never
                            // double-fires a flight.
                        } else {
                            match tab {
                                MarketTab::Plugins => {
                                    if plugin_dirty && !plugin_query.trim().is_empty() {
                                        if let Some(handle) = rt.clone() {
                                            let q = plugin_query.trim().to_string();
                                            let (tx, rx) = channel();
                                            pending_search = Some(SearchFlight::Plugin(rx));
                                            search_err = None;
                                            std::thread::spawn(move || {
                                                let out =
                                                    handle.block_on(gray_pkg::ops::search_all(&q));
                                                let _ = tx.send(out);
                                            });
                                        } else {
                                            search_err = Some("no runtime".to_string());
                                        }
                                    } else if !plugin_hits.is_empty() {
                                        preview =
                                            Some(sel.min(plugin_hits.len().saturating_sub(1)));
                                        install_msg = None;
                                    }
                                }
                                MarketTab::Skills => {
                                    if skill_dirty && !skill_query.trim().is_empty() {
                                        if let Some(handle) = rt.clone() {
                                            let q = skill_query.trim().to_string();
                                            let (tx, rx) = channel();
                                            pending_search = Some(SearchFlight::Skill(rx));
                                            search_err = None;
                                            std::thread::spawn(move || {
                                                let out = handle
                                                    .block_on(gray_pkg::skills_ops::search(&q));
                                                let _ = tx.send(out);
                                            });
                                        } else {
                                            search_err = Some("no runtime".to_string());
                                        }
                                    } else if !skill_hits.is_empty() {
                                        preview = Some(sel.min(skill_hits.len().saturating_sub(1)));
                                        install_msg = None;
                                    }
                                }
                                MarketTab::Marketplaces => {}
                            }
                        }
                    }
                    KeyCode::Char('i') if preview.is_some() && !installing => {
                        let spec = match tab {
                            MarketTab::Plugins => plugin_hits
                                .get(preview.unwrap_or(0))
                                .map(install_spec_for_plugin),
                            MarketTab::Skills => skill_hits
                                .get(preview.unwrap_or(0))
                                .map(install_spec_for_skill),
                            MarketTab::Marketplaces => None,
                        };
                        if let Some(spec) = spec {
                            if let Some(handle) = rt.clone() {
                                let (tx, rx) = channel();
                                pending_install = Some(rx);
                                install_msg = Some("installing...".to_string());
                                let is_skill = tab == MarketTab::Skills;
                                std::thread::spawn(move || {
                                    let out: anyhow::Result<Report> = handle.block_on(async {
                                        if is_skill {
                                            gray_pkg::skills_ops::install(&spec).await
                                        } else {
                                            gray_pkg::ops::install(
                                                gray_pkg::ops::parse_spec(&spec),
                                                gray_pkg::ops::InstallOpts::default(),
                                            )
                                            .await
                                        }
                                    });
                                    let _ = tx.send(out);
                                });
                            } else {
                                install_msg = Some(format_install_status(Some("no runtime")));
                            }
                        }
                    }
                    KeyCode::Char('u') if tab == MarketTab::Marketplaces => {
                        if let Some(handle) = rt.clone() {
                            statuses = [None, None, None, None];
                            let (tx, rx) = channel();
                            pending_status = Some(rx);
                            std::thread::spawn(move || {
                                let out = handle.block_on(async {
                                    tokio::join!(
                                        gray_pkg::sources::status(Source::GrayIndex),
                                        gray_pkg::sources::status(Source::PiGallery),
                                        gray_pkg::sources::status(Source::ClawHub),
                                        gray_pkg::sources::status(Source::ClaudeRepo),
                                    )
                                });
                                let _ = tx.send([out.0, out.1, out.2, out.3]);
                            });
                        }
                    }
                    KeyCode::Backspace if preview.is_none() && tab != MarketTab::Marketplaces => {
                        match tab {
                            MarketTab::Plugins => {
                                plugin_query.pop();
                                plugin_dirty = true;
                                sel = 0;
                            }
                            MarketTab::Skills => {
                                skill_query.pop();
                                skill_dirty = true;
                                sel = 0;
                            }
                            MarketTab::Marketplaces => {}
                        }
                    }
                    KeyCode::Char(ch)
                        if preview.is_none()
                            && tab != MarketTab::Marketplaces
                            && !modifiers.contains(KeyModifiers::CONTROL)
                            && !modifiers.contains(KeyModifiers::ALT)
                            && !modifiers.contains(KeyModifiers::SUPER)
                            && !modifiers.contains(KeyModifiers::HYPER)
                            && !modifiers.contains(KeyModifiers::META) =>
                    {
                        match tab {
                            MarketTab::Plugins => {
                                plugin_query.push(ch);
                                plugin_dirty = true;
                                sel = 0;
                            }
                            MarketTab::Skills => {
                                skill_query.push(ch);
                                skill_dirty = true;
                                sel = 0;
                            }
                            MarketTab::Marketplaces => {}
                        }
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

enum SearchFlight {
    Plugin(Receiver<anyhow::Result<SearchOutput>>),
    Skill(Receiver<anyhow::Result<Vec<SkillHit>>>),
}

enum SearchRx {
    Plugin(Receiver<anyhow::Result<SearchOutput>>),
    Skill(Receiver<anyhow::Result<Vec<SkillHit>>>),
}

enum SearchPoll {
    Pending(SearchRx),
    PluginReady(anyhow::Result<SearchOutput>),
    SkillReady(anyhow::Result<Vec<SkillHit>>),
}

impl SearchFlight {
    fn try_recv(self) -> SearchPoll {
        match self {
            SearchFlight::Plugin(rx) => match rx.try_recv() {
                Ok(res) => SearchPoll::PluginReady(res),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    SearchPoll::Pending(SearchRx::Plugin(rx))
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    SearchPoll::PluginReady(Err(anyhow::anyhow!("background task ended")))
                }
            },
            SearchFlight::Skill(rx) => match rx.try_recv() {
                Ok(res) => SearchPoll::SkillReady(res),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    SearchPoll::Pending(SearchRx::Skill(rx))
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    SearchPoll::SkillReady(Err(anyhow::anyhow!("background task ended")))
                }
            },
        }
    }
}

fn unreachable_lines(out: &SearchOutput) -> Vec<String> {
    let mut lines = Vec::new();
    if out.gray_unreachable {
        lines.push(gray_pkg::ops::GRAY_UNREACHABLE_LINE.to_string());
    }
    if out.pi_unreachable {
        lines.push(gray_pkg::ops::PI_UNREACHABLE_LINE.to_string());
    }
    if out.clawhub_unreachable {
        lines.push(gray_pkg::ops::CLAWHUB_UNREACHABLE_LINE.to_string());
    }
    if out.claude_unreachable {
        lines.push(gray_pkg::ops::CLAUDE_UNREACHABLE_LINE.to_string());
    }
    lines
}

fn query_line(query: &str) -> String {
    if query.is_empty() {
        "Search: type + Enter".to_string()
    } else {
        format!("Search: {query}▎")
    }
}

fn preview_lines_plugins(hits: &[SearchHit], preview: Option<usize>) -> Vec<String> {
    let Some(idx) = preview else {
        return Vec::new();
    };
    let Some(hit) = hits.get(idx) else {
        return Vec::new();
    };
    // No requires in the current flow (see `format_preview`).
    format_preview(hit, &[])
        .split('\n')
        .map(str::to_string)
        .collect()
}

fn preview_lines_skills(hits: &[SkillHit], preview: Option<usize>) -> Vec<String> {
    let Some(idx) = preview else {
        return Vec::new();
    };
    let Some(hit) = hits.get(idx) else {
        return Vec::new();
    };
    let origin_note = format!("install: {}", install_spec_for_skill(hit));
    format_skill_preview(hit, &origin_note)
        .split('\n')
        .map(str::to_string)
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn render_row(
    frame: &mut ratatui::Frame,
    inner: ratatui::layout::Rect,
    y: u16,
    text: &str,
    is_selected: bool,
    lit: bool,
    box_bg: ratatui::style::Color,
    accent_peach: ratatui::style::Color,
    text_dim: ratatui::style::Color,
) {
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Paragraph;
    // Truncate to the inner width so long rows never wrap.
    let row: String = text.chars().take(inner.width as usize).collect();
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
        let fg = if lit { Color::White } else { text_dim };
        Line::from(vec![
            Span::styled(row, Style::default().fg(fg).bg(box_bg)),
            Span::styled(" ".repeat(fill), Style::default().bg(box_bg)),
        ])
    };
    frame.render_widget(
        Paragraph::new(row_line),
        ratatui::layout::Rect::new(inner.x, y, inner.width, 1),
    );
}

fn footer_spans(
    box_bg: ratatui::style::Color,
    text_dim: ratatui::style::Color,
    parts: Vec<(&str, bool)>,
) -> ratatui::text::Line<'static> {
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    let spans: Vec<Span> = parts
        .into_iter()
        .map(|(t, lit)| {
            let style = if lit {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
                    .bg(box_bg)
            } else {
                Style::default().fg(text_dim).bg(box_bg)
            };
            Span::styled(t.to_string(), style)
        })
        .collect();
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::{
        MarketTab, format_install_status, format_market_row, format_preview, format_skill_preview,
        format_source_row, install_spec_for_plugin, install_spec_for_skill,
    };
    use gray_pkg::ops::{SearchHit, SearchSource};
    use gray_pkg::skills_ops::SkillHit;

    fn plugin_hit() -> SearchHit {
        SearchHit {
            name: "demo".to_string(),
            version: "1.2.3".to_string(),
            desc: "does things".to_string(),
            source: SearchSource::Gray,
            version_detail: String::new(),
            files: Vec::new(),
            trust: String::new(),
        }
    }

    fn skill_hit() -> SkillHit {
        SkillHit {
            name: "arein/gifgrep".to_string(),
            version: "1.2.3".to_string(),
            desc: "grep gifs".to_string(),
            source: "ClawHub".to_string(),
            trust: "community".to_string(),
        }
    }

    #[test]
    fn market_row_shows_name_version_source_and_desc() {
        let row = format_market_row("demo", "1.2.3", "Gray Index", "does things");
        assert_eq!(row, "demo 1.2.3 [Gray Index] - does things");
        let bare = format_market_row("demo", "1.2.3", "Gray Index", "");
        assert_eq!(bare, "demo 1.2.3 [Gray Index]");
    }

    #[test]
    fn preview_shows_detail_files_trust_and_requires_when_known() {
        let mut hit = plugin_hit();
        hit.version_detail = "github:o/r@main".to_string();
        hit.files = vec!["SKILL.md".to_string()];
        hit.trust = "official + scan:clean".to_string();
        let out = format_preview(&hit, &["git".to_string(), "rg".to_string()]);
        assert!(out.contains("demo 1.2.3 [Gray Index]"), "head: {out:?}");
        assert!(out.contains("does things"), "desc: {out:?}");
        assert!(out.contains("detail: github:o/r@main"), "detail: {out:?}");
        assert!(out.contains("files: SKILL.md"), "files: {out:?}");
        assert!(
            out.contains("trust: official + scan:clean"),
            "trust: {out:?}"
        );
        assert!(out.contains("requires: git, rg"), "requires: {out:?}");
        // Unknown fields stay omitted, never blank lines.
        let bare = format_preview(&plugin_hit(), &[]);
        assert_eq!(bare, "demo 1.2.3 [Gray Index]\ndoes things");
    }

    #[test]
    fn skill_preview_is_skill_shaped_with_origin_note() {
        let out = format_skill_preview(&skill_hit(), "install: clawhub:arein/gifgrep");
        assert!(
            out.contains("arein/gifgrep 1.2.3 [ClawHub]"),
            "head: {out:?}"
        );
        assert!(out.contains("grep gifs"), "desc: {out:?}");
        assert!(out.contains("trust: community"), "trust: {out:?}");
        assert!(
            out.contains("install: clawhub:arein/gifgrep"),
            "origin: {out:?}"
        );
    }

    #[test]
    fn source_row_covers_ok_unreachable_and_checking() {
        assert_eq!(
            format_source_row("Gray Index", Some(true)),
            "Gray Index: ok"
        );
        assert_eq!(
            format_source_row("ClawHub", Some(false)),
            "ClawHub: unreachable"
        );
        assert_eq!(format_source_row("Claude", None), "Claude: checking...");
    }

    #[test]
    fn install_status_reports_active_or_failed_inline() {
        assert_eq!(format_install_status(None), "active");
        assert_eq!(format_install_status(Some("boom")), "failed: boom");
    }

    #[test]
    fn install_specs_derive_from_source() {
        assert_eq!(install_spec_for_plugin(&plugin_hit()), "demo");
        let mut pi = plugin_hit();
        pi.source = SearchSource::Pi;
        pi.name = "@scope/bar".to_string();
        assert_eq!(install_spec_for_plugin(&pi), "npm:@scope/bar");
        let mut ch = plugin_hit();
        ch.source = SearchSource::ClawHub;
        ch.name = "arein/test".to_string();
        assert_eq!(install_spec_for_plugin(&ch), "clawhub:arein/test");
        let mut cl = plugin_hit();
        cl.source = SearchSource::Claude;
        cl.name = "grep-skills".to_string();
        assert_eq!(install_spec_for_plugin(&cl), "claude:grep-skills");
        assert_eq!(
            install_spec_for_skill(&skill_hit()),
            "clawhub:arein/gifgrep"
        );
        let mut cs = skill_hit();
        cs.source = "Claude".to_string();
        cs.name = "grep-skills".to_string();
        assert_eq!(install_spec_for_skill(&cs), "claude:grep-skills");
    }

    #[test]
    fn market_tabs_wrap_at_both_ends() {
        assert_eq!(MarketTab::from_index(0), MarketTab::Plugins);
        assert_eq!(MarketTab::from_index(1), MarketTab::Skills);
        assert_eq!(MarketTab::from_index(2), MarketTab::Marketplaces);
        assert_eq!(MarketTab::Plugins.next(), MarketTab::Skills);
        assert_eq!(MarketTab::Skills.next(), MarketTab::Marketplaces);
        assert_eq!(MarketTab::Marketplaces.next(), MarketTab::Plugins);
        assert_eq!(MarketTab::Plugins.prev(), MarketTab::Marketplaces);
        assert_eq!(MarketTab::Marketplaces.prev(), MarketTab::Skills);
    }
}
