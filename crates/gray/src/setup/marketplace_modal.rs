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

use super::tabs::{tab_segments, wrapped_tab};
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

wrapped_tab!(MarketTab, 3, Plugins => 0, Skills => 1, Marketplaces => 2);

/// Pure row split, shared by both search tabs:
/// head `name version [source]`, tail ` - desc` (empty when desc blank).
/// (Mirrors `ops::format_search_hit` without depending on its enum.)
pub(crate) fn split_market_row(
    name: &str,
    version: &str,
    source_label: &str,
    desc: &str,
) -> (String, String) {
    let head = format!("{name} {version} [{source_label}]");
    let desc = desc.trim();
    if desc.is_empty() {
        (head, String::new())
    } else {
        (head, format!(" - {desc}"))
    }
}

/// Short row chip for a plugin source (display only; install specs and
/// the preview pane keep full labels via `format_preview`).
fn source_chip(source: Source) -> &'static str {
    use gray_pkg::sources::Source as S;
    match source {
        S::GrayIndex => "gray",
        S::PiGallery => "pi",
        S::ClawHub => "claw",
        S::ClaudeRepo => "claude",
    }
}

/// Short row chip for a Skills-tab source label (`SkillHit.source` is a
/// plain string): `"ClawHub"`→`"claw"`, `"Claude"`→`"claude"`, else input.
fn skill_chip(source: &str) -> &str {
    match source {
        "ClawHub" => "claw",
        "Claude" => "claude",
        other => other,
    }
}

/// Per-source chip fg for the unselected row head (`gray` reuses the
/// modal peach; selected rows suppress chip colors wholesale).
fn chip_color(chip: &str, accent_peach: ratatui::style::Color) -> ratatui::style::Color {
    use ratatui::style::Color;
    match chip {
        "[gray]" => accent_peach,
        "[pi]" => crate::theme::theme().badge_pi,
        "[claw]" => crate::theme::theme().badge_claw,
        "[claude]" => crate::theme::theme().badge_claude,
        _ => Color::White,
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

/// True when the preview footer already shows the in-flight `installing...`
/// line, so the status row above it must stay quiet (no duplicate).
pub(crate) fn install_status_covered_by_footer(msg: &str, preview_open: bool) -> bool {
    msg == "installing..." && preview_open
}

/// Install spec for a plugin hit: Gray names install bare, Pi via `npm:`,
/// ClawHub via `clawhub:`, Claude via `claude:`.
pub(crate) fn install_spec_for_plugin(hit: &SearchHit) -> String {
    use Source;
    match hit.source {
        Source::GrayIndex => hit.name.clone(),
        Source::PiGallery => format!("npm:{}", hit.name),
        Source::ClawHub => format!("clawhub:{}", hit.name),
        Source::ClaudeRepo => format!("claude:{}", hit.name),
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
    use crossterm::terminal::EnterAlternateScreen;
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
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
    // First visible hit row in the list view (auto-follows `sel`;
    // self-corrects every frame, so no reset sites needed).
    let mut sel_top = 0usize;
    let mut changed = false;
    // Empty-state content (local lock reads, instant, no network).
    // Corrupt locks degrade to "no installed row", never a modal error.
    let installed_plugins: Vec<(String, String)> = gray_pkg::ops::list()
        .map(|m| m.into_iter().map(|(n, e)| (n, e.version.clone())).collect())
        .unwrap_or_default();
    let installed_skills: Vec<(String, String)> = gray_pkg::skills_ops::list()
        .map(|v| {
            v.into_iter()
                .map(|s| (s.name.clone(), s.version.clone()))
                .collect()
        })
        .unwrap_or_default();
    // Pristine receipts; the display vecs are filter+sort views over these.
    let mut plugin_all: Vec<SearchHit> = Vec::new();
    let mut skill_all: Vec<SkillHit> = Vec::new();
    let mut plugin_filter: Option<Source> = None;
    let mut skill_filter: Option<Source> = None;
    let mut sort_mode = SortMode::Relevance;
    let mut skill_booted = false;

    // Runtime handle for spawned `block_on` flights (same shape as the
    // provider live-fetch path: `try_current` here, `block_on` on a fresh
    // OS thread so the modal loop never blocks the runtime worker).
    let rt = tokio::runtime::Handle::try_current().ok();
    // Browse on open: empty-query search in the background. The modal
    // opens instantly over Installed rows; hits land when ready.
    if let Some(handle) = rt.clone() {
        pending_search = Some(spawn_plugin_search(handle, String::new()));
    }
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
            // Poll background flights (non-blocking): search results land
            // the hit list, installs land the inline status, statuses land
            // the Marketplaces rows.
            if let Some(polled) = pending_search.as_mut().map(SearchFlight::poll) {
                match polled {
                    SearchPoll::Pending => {}
                    SearchPoll::PluginReady(res) => {
                        pending_search = None;
                        match res {
                            Ok(out) => {
                                plugin_unreachable = unreachable_lines(&out);
                                plugin_all = out.hits;
                                plugin_hits =
                                    apply_plugin_view(&plugin_all, plugin_filter, sort_mode);
                                plugin_dirty = false;
                                search_err = None;
                                sel = 0;
                            }
                            Err(e) => {
                                search_err = Some(format!("{e:#}"));
                                plugin_hits.clear();
                                plugin_unreachable.clear();
                            }
                        }
                    }
                    SearchPoll::SkillReady(res) => {
                        pending_search = None;
                        match res {
                            Ok(hits) => {
                                skill_all = hits;
                                skill_hits = apply_skill_view(&skill_all, skill_filter, sort_mode);
                                skill_dirty = false;
                                search_err = None;
                                sel = 0;
                            }
                            Err(e) => {
                                search_err = Some(format!("{e:#}"));
                                skill_hits.clear();
                            }
                        }
                    }
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
                // Near-fullscreen: result rows are long and many;
                // the old 116-wide cap truncated every description.
                let modal_w = (area.width.saturating_sub(4))
                    .clamp(56, 220)
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
                    .clamp(10, 40)
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
                // Body rows for the active view: (head, tail, lit) —
                // browse hits split bright-head/dim-tail, every other
                // line passes tail = "".
                let body: Vec<(String, String, bool)> = match tab {
                    MarketTab::Plugins => {
                        if let Some(idx) = preview {
                            preview_lines_plugins(&plugin_hits, Some(idx))
                                .into_iter()
                                .map(|l| (l, String::new(), true))
                                .collect()
                        } else {
                            let mut out = vec![(query_line(&plugin_query), String::new(), true)];
                            if searching {
                                out.push(("* Loading...".to_string(), String::new(), true));
                            } else if let Some(e) = search_err.as_deref() {
                                out.push((format!("search failed: {e}"), String::new(), true));
                            } else if plugin_hits.is_empty() {
                                if !plugin_dirty {
                                    out.push((
                                        "no hits — try another query".to_string(),
                                        String::new(),
                                        true,
                                    ));
                                } else if let Some(line) = installed_summary(&installed_plugins) {
                                    // Unsearched: show what's installed
                                    // instead of an empty pane (dim info
                                    // row, never selectable).
                                    out.push((line, String::new(), false));
                                }
                            } else {
                                for h in &plugin_hits {
                                    let (head, tail) = split_market_row(
                                        &h.name,
                                        &h.version,
                                        source_chip(h.source),
                                        &h.desc,
                                    );
                                    out.push((head, tail, true));
                                }
                                for line in &plugin_unreachable {
                                    out.push((line.clone(), String::new(), true));
                                }
                            }
                            out
                        }
                    }
                    MarketTab::Skills => {
                        if let Some(idx) = preview {
                            preview_lines_skills(&skill_hits, Some(idx))
                                .into_iter()
                                .map(|l| (l, String::new(), true))
                                .collect()
                        } else {
                            let mut out = vec![(query_line(&skill_query), String::new(), true)];
                            if searching {
                                out.push(("* Loading...".to_string(), String::new(), true));
                            } else if let Some(e) = search_err.as_deref() {
                                out.push((format!("search failed: {e}"), String::new(), true));
                            } else if skill_hits.is_empty() {
                                if !skill_dirty {
                                    out.push((
                                        "no hits — try another query".to_string(),
                                        String::new(),
                                        true,
                                    ));
                                } else if let Some(line) = installed_summary(&installed_skills) {
                                    // Unsearched: show what's installed
                                    // instead of an empty pane (dim info
                                    // row, never selectable).
                                    out.push((line, String::new(), false));
                                }
                            } else {
                                for h in &skill_hits {
                                    let (head, tail) = split_market_row(
                                        &h.name,
                                        &h.version,
                                        skill_chip(&h.source),
                                        &h.desc,
                                    );
                                    out.push((head, tail, true));
                                }
                            }
                            out
                        }
                    }
                    MarketTab::Marketplaces => MARKET_SOURCES
                        .iter()
                        .enumerate()
                        .map(|(i, s)| {
                            (
                                format_source_row(s.label(), statuses[i]),
                                String::new(),
                                true,
                            )
                        })
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
                    for (head, tail, lit) in body.iter() {
                        if cur_y >= rows_cap {
                            break;
                        }
                        render_market_row(
                            frame,
                            inner,
                            cur_y,
                            head,
                            tail,
                            false,
                            *lit,
                            box_bg,
                            accent_peach,
                            text_dim,
                        );
                        cur_y += 1;
                    }
                    // Marketplaces rows are status only (Enter does
                    // nothing there): never highlight, like hint rows.
                } else {
                    // Hits length for selection (hint/loading rows are never
                    // highlighted).
                    let hits_len = match tab {
                        MarketTab::Plugins => plugin_hits.len(),
                        MarketTab::Skills => skill_hits.len(),
                        MarketTab::Marketplaces => 0,
                    };
                    // Hit slots on screen past the query row; the
                    // window follows `sel` so long lists scroll.
                    let hit_slots = rows_cap.saturating_sub(cur_y + 1).max(1) as usize;
                    sel_top = scroll_top(sel_top, sel, hit_slots);
                    for (idx, (head, tail, lit)) in body.iter().enumerate() {
                        if cur_y >= rows_cap {
                            break;
                        }
                        if idx >= list_offset && idx - list_offset < hits_len {
                            let vi = idx - list_offset;
                            if vi < sel_top || vi >= sel_top + hit_slots {
                                continue;
                            }
                        }
                        let is_selected = idx >= list_offset
                            && (idx - list_offset) == sel
                            && (idx - list_offset) < hits_len;
                        render_market_row(
                            frame,
                            inner,
                            cur_y,
                            head,
                            tail,
                            is_selected,
                            *lit,
                            box_bg,
                            accent_peach,
                            text_dim,
                        );
                        cur_y += 1;
                    }
                }
                if let Some(msg) = install_msg.as_deref()
                    && !install_status_covered_by_footer(msg, preview.is_some())
                {
                    let msg_y = footer_y.saturating_sub(1);
                    if msg_y > inner.y + 1 {
                        let text: String = msg.chars().take(inner.width as usize).collect();
                        let fill = (inner.width as usize).saturating_sub(text.chars().count());
                        let fg = if msg == "active" || msg == "installing..." {
                            accent_peach
                        } else {
                            crate::theme::theme().error_soft
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
                // Active source filter + sort mode for the footer
                // (`^F` cycles sources, `^S` cycles rel|name|pop).
                let filter_seg = format!(
                    "src:{} · ",
                    filter_short(match tab {
                        MarketTab::Plugins => plugin_filter,
                        MarketTab::Skills => skill_filter,
                        MarketTab::Marketplaces => None,
                    })
                );
                let sort_seg = format!("sort:{} · ", sort_mode.short());
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
                            ("←→ ", true),
                            ("tabs · ", false),
                            ("Enter ", true),
                            ("search/open · ", false),
                            ("^S ", true),
                            (sort_seg.as_str(), false),
                            ("^F ", true),
                            (filter_seg.as_str(), false),
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
                            ("←→ ", true),
                            ("tabs · ", false),
                            ("u ", true),
                            ("re-check · ", false),
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
                    KeyCode::Left | KeyCode::Right if preview.is_none() => {
                        tab = if code == KeyCode::Right {
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
                    // A–Z toggle over the pristine receipts (fresh
                    // searches re-apply it). Ctrl namespace: plain
                    // letters type into the query.
                    KeyCode::Char('s') | KeyCode::Char('S')
                        if modifiers.contains(KeyModifiers::CONTROL) && preview.is_none() =>
                    {
                        sort_mode = sort_mode.next();
                        match tab {
                            MarketTab::Plugins => {
                                plugin_hits =
                                    apply_plugin_view(&plugin_all, plugin_filter, sort_mode);
                            }
                            MarketTab::Skills => {
                                skill_hits = apply_skill_view(&skill_all, skill_filter, sort_mode);
                            }
                            MarketTab::Marketplaces => {}
                        }
                        sel = 0;
                    }
                    // Source filter cycle over the pristine receipts.
                    KeyCode::Char('f') | KeyCode::Char('F')
                        if modifiers.contains(KeyModifiers::CONTROL) && preview.is_none() =>
                    {
                        match tab {
                            MarketTab::Plugins => {
                                plugin_filter = next_plugin_filter(plugin_filter);
                                plugin_hits =
                                    apply_plugin_view(&plugin_all, plugin_filter, sort_mode);
                            }
                            MarketTab::Skills => {
                                skill_filter = next_skill_filter(skill_filter);
                                skill_hits = apply_skill_view(&skill_all, skill_filter, sort_mode);
                            }
                            MarketTab::Marketplaces => {}
                        }
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
                                    // Dirty alone (re)searches: empty query
                                    // browses the full catalog, no typing needed.
                                    if plugin_dirty {
                                        if let Some(handle) = rt.clone() {
                                            let q = plugin_query.trim().to_string();
                                            pending_search = Some(spawn_plugin_search(handle, q));
                                            search_err = None;
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
                                    // Dirty alone (re)searches: empty query
                                    // browses the full catalog, no typing needed.
                                    if skill_dirty {
                                        if let Some(handle) = rt.clone() {
                                            let q = skill_query.trim().to_string();
                                            pending_search = Some(spawn_skill_search(handle, q));
                                            search_err = None;
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
            // Lazy Skills browse: first visit with an empty query fires
            // the background search (Plugins browses at open). Never
            // clobbers a running flight; manual search works regardless.
            if tab == MarketTab::Skills
                && !skill_booted
                && skill_query.trim().is_empty()
                && pending_search.is_none()
            {
                skill_booted = true;
                if let Some(handle) = rt.clone() {
                    pending_search = Some(spawn_skill_search(handle, String::new()));
                    search_err = None;
                }
            }
        }
    })();
    let _ = terminal.clear();
    result
}

enum SearchFlight {
    Plugin(Receiver<anyhow::Result<SearchOutput>>),
    Skill(Receiver<anyhow::Result<Vec<SkillHit>>>),
}

enum SearchPoll {
    Pending,
    PluginReady(anyhow::Result<SearchOutput>),
    SkillReady(anyhow::Result<Vec<SkillHit>>),
}

impl SearchFlight {
    fn poll(&mut self) -> SearchPoll {
        match self {
            SearchFlight::Plugin(rx) => match rx.try_recv() {
                Ok(res) => SearchPoll::PluginReady(res),
                Err(std::sync::mpsc::TryRecvError::Empty) => SearchPoll::Pending,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    SearchPoll::PluginReady(Err(anyhow::anyhow!("background task ended")))
                }
            },
            SearchFlight::Skill(rx) => match rx.try_recv() {
                Ok(res) => SearchPoll::SkillReady(res),
                Err(std::sync::mpsc::TryRecvError::Empty) => SearchPoll::Pending,
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

/// Background browse/search flight for plugins (empty query = browse all).
fn spawn_plugin_search(handle: tokio::runtime::Handle, query: String) -> SearchFlight {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let out = handle.block_on(gray_pkg::ops::search_all(&query));
        let _ = tx.send(out);
    });
    SearchFlight::Plugin(rx)
}

/// Background search flight for skills (empty query = browse all).
fn spawn_skill_search(handle: tokio::runtime::Handle, query: String) -> SearchFlight {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let out = handle.block_on(gray_pkg::skills_ops::search(&query));
        let _ = tx.send(out);
    });
    SearchFlight::Skill(rx)
}

/// `^S` sort cycle over the pristine receipts: relevance (search order),
/// A–Z, then npm popularity descending. Skills-tab hits all report 0.0,
/// so popularity there is a documented name-fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortMode {
    Relevance,
    Name,
    Popularity,
}

impl SortMode {
    fn next(self) -> SortMode {
        match self {
            SortMode::Relevance => SortMode::Name,
            SortMode::Name => SortMode::Popularity,
            SortMode::Popularity => SortMode::Relevance,
        }
    }

    fn short(self) -> &'static str {
        match self {
            SortMode::Relevance => "rel",
            SortMode::Name => "name",
            SortMode::Popularity => "pop",
        }
    }
}

/// Display view over pristine receipts: source filter, then the `^S` sort.
fn apply_plugin_view(all: &[SearchHit], filter: Option<Source>, sort: SortMode) -> Vec<SearchHit> {
    let mut v: Vec<SearchHit> = all
        .iter()
        .filter(|h| filter.is_none_or(|f| h.source == f))
        .cloned()
        .collect();
    match sort {
        SortMode::Relevance => {}
        SortMode::Name => sort_plugins_by_name(&mut v),
        SortMode::Popularity => sort_plugins_by_popularity(&mut v),
    }
    v
}

/// Display view over pristine skill receipts: source filter, then `^S` sort.
fn apply_skill_view(all: &[SkillHit], filter: Option<Source>, sort: SortMode) -> Vec<SkillHit> {
    let mut v: Vec<SkillHit> = all
        .iter()
        .filter(|h| filter.is_none_or(|f| h.source == f.label()))
        .cloned()
        .collect();
    match sort {
        SortMode::Relevance => {}
        SortMode::Name => sort_skills_by_name(&mut v),
        SortMode::Popularity => sort_skills_by_popularity(&mut v),
    }
    v
}

/// `^F` cycles (tab-scoped; `None` = all sources).
fn next_plugin_filter(cur: Option<Source>) -> Option<Source> {
    use gray_pkg::sources::Source as S;
    const ORDER: [Option<S>; 5] = [
        None,
        Some(S::GrayIndex),
        Some(S::PiGallery),
        Some(S::ClaudeRepo),
        Some(S::ClawHub),
    ];
    let pos = ORDER.iter().position(|f| *f == cur).unwrap_or(0);
    ORDER[(pos + 1) % ORDER.len()]
}

/// `^F` cycles on the Skills tab (only skill-bearing sources).
fn next_skill_filter(cur: Option<Source>) -> Option<Source> {
    use gray_pkg::sources::Source as S;
    const ORDER: [Option<S>; 3] = [None, Some(S::ClawHub), Some(S::ClaudeRepo)];
    let pos = ORDER.iter().position(|f| *f == cur).unwrap_or(0);
    ORDER[(pos + 1) % ORDER.len()]
}

/// Short footer label for the active source filter.
fn filter_short(f: Option<Source>) -> &'static str {
    use gray_pkg::sources::Source as S;
    match f {
        None => "all",
        Some(S::GrayIndex) => "gray",
        Some(S::PiGallery) => "pi",
        Some(S::ClaudeRepo) => "claude",
        Some(S::ClawHub) => "clawhub",
    }
}

/// Scroll offset so the selected hit stays visible: pulls the window
/// up when `sel` moves above it, pushes it down when `sel` runs past
/// the bottom. `visible` = hit slots on screen (≥1 by caller).
fn scroll_top(top: usize, sel: usize, visible: usize) -> usize {
    if sel < top {
        sel
    } else if visible == 0 {
        top
    } else if sel >= top + visible {
        sel + 1 - visible
    } else {
        top
    }
}

/// Case-insensitive A–Z over a hit list (`^S` cycle).
fn sort_plugins_by_name(hits: &mut [SearchHit]) {
    hits.sort_by_key(|a| a.name.to_lowercase());
}

/// Case-insensitive A–Z over a skill hit list (`^S` cycle).
fn sort_skills_by_name(hits: &mut [SkillHit]) {
    hits.sort_by_key(|a| a.name.to_lowercase());
}

/// Popularity descending (`total_cmp`, NaN-proof) with a lowercase-name
/// tiebreak (`^S` third stop).
fn sort_plugins_by_popularity(hits: &mut [SearchHit]) {
    hits.sort_by(|a, b| {
        b.popularity
            .total_cmp(&a.popularity)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

/// Popularity descending over a skill hit list (all zeros today, so this
/// reads as A–Z — the documented name-fallback).
fn sort_skills_by_popularity(hits: &mut [SkillHit]) {
    hits.sort_by(|a, b| {
        b.popularity
            .total_cmp(&a.popularity)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

/// Empty-state line naming what's already installed:
/// `Installed (2): foo 1.0.0, bar`. `None` when empty (row stays hidden).
fn installed_summary(items: &[(String, String)]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let list = items
        .iter()
        .map(|(n, v)| {
            if v.trim().is_empty() {
                n.clone()
            } else {
                format!("{n} {}", v.trim())
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!("Installed ({}): {list}", items.len()))
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
fn render_market_row(
    frame: &mut ratatui::Frame,
    inner: ratatui::layout::Rect,
    y: u16,
    head: &str,
    tail: &str,
    is_selected: bool,
    lit: bool,
    box_bg: ratatui::style::Color,
    accent_peach: ratatui::style::Color,
    text_dim: ratatui::style::Color,
) {
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Paragraph;
    // Head-first truncation to the inner width (rows never wrap): the
    // head stays whole while the tail absorbs the cut; a head longer
    // than the row drops the tail entirely.
    let budget = inner.width as usize;
    let (head_vis, tail_vis): (String, String) = if head.chars().count() >= budget {
        (head.chars().take(budget).collect(), String::new())
    } else {
        (
            head.to_string(),
            tail.chars().take(budget - head.chars().count()).collect(),
        )
    };
    let visible = format!("{head_vis}{tail_vis}");
    let fill = budget.saturating_sub(visible.chars().count());
    let row_line = if is_selected {
        Line::from(Span::styled(
            format!("{visible}{}", " ".repeat(fill)),
            Style::default()
                .fg(crate::theme::theme().on_selection)
                .bg(accent_peach)
                .add_modifier(Modifier::BOLD),
        ))
    } else {
        let mut head_style = Style::default()
            .fg(if lit { Color::White } else { text_dim })
            .bg(box_bg);
        if lit {
            head_style = head_style.add_modifier(Modifier::BOLD);
        }
        // The trailing `[chip]` (always emitted by `split_market_row`)
        // paints in its source color, bold off — the color is the signal.
        // Non-hit rows carry no bracket suffix and stay head-styled.
        let chip_at = head_vis.rfind('[').filter(|_| head_vis.ends_with(']'));
        let (head_text, chip_text) = match chip_at {
            Some(i) => head_vis.split_at(i),
            None => (head_vis.as_str(), ""),
        };
        Line::from(vec![
            Span::styled(head_text, head_style),
            Span::styled(
                chip_text,
                Style::default()
                    .fg(chip_color(chip_text, accent_peach))
                    .bg(box_bg),
            ),
            Span::styled(tail_vis, Style::default().fg(text_dim).bg(box_bg)),
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

#[path = "marketplace_modal_tests.rs"]
#[cfg(test)]
mod tests;
