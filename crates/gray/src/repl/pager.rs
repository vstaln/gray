use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use gray_core::agent::{CommandOutcome, PanelSpan, PanelSpec};

use crate::setup::BackgroundSnapshot;

pub(crate) struct PagerPage {
    pub(crate) tab: String,
    pub(crate) lines: Vec<Line<'static>>,
}

fn span_from_spec(span: &PanelSpan) -> Span<'static> {
    let th = crate::theme::theme();
    let mut style = Style::default().fg(th.text_body);
    if let Some(spec) = &span.style {
        if let Some(role) = spec.fg.as_deref().and_then(|name| th.role(name)) {
            style = style.fg(role);
        }
        if spec.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if spec.italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if spec.dim {
            style = style.add_modifier(Modifier::DIM);
        }
    }
    Span::styled(span.text.clone(), style)
}

pub(crate) fn pages_from_spec(spec: &PanelSpec) -> Vec<PagerPage> {
    spec.pages
        .iter()
        .map(|p| PagerPage {
            tab: p.tab.clone(),
            lines: p
                .lines
                .iter()
                .map(|spans| Line::from(spans.iter().map(span_from_spec).collect::<Vec<_>>()))
                .collect(),
        })
        .collect()
}

pub(crate) fn pages_from_outcome(outcome: Option<CommandOutcome>) -> Vec<PagerPage> {
    match outcome {
        Some(CommandOutcome::Panel(spec)) => pages_from_spec(&spec),
        Some(CommandOutcome::Say(text)) => vec![PagerPage {
            tab: "Result".into(),
            lines: text
                .lines()
                .map(|l| Line::from(Span::raw(l.to_string())))
                .collect(),
        }],
        _ => Vec::new(),
    }
}

pub(crate) fn spec_lines(spec: &PanelSpec) -> Vec<Line<'static>> {
    let th = crate::theme::theme();
    let mut out = Vec::new();
    if !spec.title.is_empty() {
        out.push(Line::from(Span::styled(
            spec.title.clone(),
            Style::default()
                .fg(th.text_bright)
                .add_modifier(Modifier::BOLD),
        )));
    }
    for p in &spec.pages {
        if !p.title.is_empty() && p.title != spec.title {
            out.push(Line::from(Span::styled(
                p.title.clone(),
                Style::default().fg(th.text_muted),
            )));
        }
        out.extend(
            p.lines
                .iter()
                .map(|spans| Line::from(spans.iter().map(span_from_spec).collect::<Vec<_>>())),
        );
    }
    out
}

pub(crate) fn spec_plain(spec: &PanelSpec) -> String {
    spec_lines(spec)
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn draw_pager(
    frame: &mut ratatui::Frame,
    bg: &BackgroundSnapshot,
    pages: &[PagerPage],
    page: usize,
    scroll: &mut usize,
    refreshable: bool,
) {
    use ratatui::layout::Rect;
    use ratatui::widgets::{Block, Clear, Paragraph};

    if pages.is_empty() {
        return;
    }
    let t = crate::theme::theme();
    let (box_bg, accent, text_dim, on_sel) = (t.surface_bg, t.accent, t.text_dim, t.on_selection);
    let area = frame.area();
    if area.width < 30 || area.height < 8 {
        return;
    }
    crate::setup::render_dimmed_background(frame, bg);
    let modal_w = 88.min(area.width.saturating_sub(2)).max(40).min(area.width);
    let modal_h = ((pages[page].lines.len() as u16) + 4)
        .min(area.height.saturating_sub(2))
        .max(8)
        .min(area.height);
    let modal_rect = Rect::new(
        (area.width.saturating_sub(modal_w)) / 2,
        (area.height.saturating_sub(modal_h)) / 4,
        modal_w,
        modal_h,
    );
    frame.render_widget(Clear, modal_rect);
    frame.render_widget(
        Block::default().style(Style::default().bg(box_bg)),
        modal_rect,
    );
    let inner = Rect::new(
        modal_rect.x + 2,
        modal_rect.y + 1,
        modal_w.saturating_sub(4),
        modal_h.saturating_sub(2),
    );
    let mut put = |y: u16, line: Line| {
        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(box_bg)),
            Rect::new(inner.x, inner.y + y, inner.width, 1),
        );
    };

    let mut tabs: Vec<Span> = Vec::new();
    for (i, p) in pages.iter().enumerate() {
        if i > 0 {
            tabs.push(Span::styled("  ", Style::default().bg(box_bg)));
        }
        tabs.push(Span::styled(
            format!(" {} ", p.tab),
            if i == page {
                Style::default()
                    .fg(on_sel)
                    .bg(accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(text_dim).bg(box_bg)
            },
        ));
    }
    let tabs_w: usize = tabs.iter().map(|s| s.content.chars().count()).sum();
    tabs.push(Span::styled(
        " ".repeat((inner.width as usize).saturating_sub(tabs_w + 3)),
        Style::default().bg(box_bg),
    ));
    tabs.push(Span::styled(
        "esc",
        Style::default().fg(text_dim).bg(box_bg),
    ));
    put(0, Line::from(tabs));

    let body_h = inner.height.saturating_sub(3) as usize;
    *scroll = (*scroll).min(pages[page].lines.len().saturating_sub(body_h));
    for (i, line) in pages[page]
        .lines
        .iter()
        .skip(*scroll)
        .take(body_h)
        .enumerate()
    {
        put(2 + i as u16, line.clone());
    }

    let footer = match (pages.len() > 1, refreshable) {
        (true, true) => " ←/→ page · ↑↓ scroll · r refresh · esc close",
        (true, false) => " ←/→ page · ↑↓ scroll · esc close",
        (false, true) => " ↑↓ scroll · r refresh · esc close",
        (false, false) => " ↑↓ scroll · esc close",
    };
    put(
        inner.height.saturating_sub(1),
        Line::from(Span::styled(
            footer,
            Style::default().fg(text_dim).bg(box_bg),
        )),
    );
}

pub(crate) fn run_pager<F>(
    initial: Vec<PagerPage>,
    refresh: Option<&mut F>,
    bg: &BackgroundSnapshot,
) -> anyhow::Result<()>
where
    F: FnMut() -> Vec<PagerPage> + ?Sized,
{
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
    use std::time::Duration;

    let (_session, mut terminal) = crate::setup::open_modal()?;
    let mut pages = initial;
    let mut refresh = refresh;
    let mut page = 0usize;
    let mut scroll = 0usize;

    if pages.is_empty() {
        return Ok(());
    }
    loop {
        page = page.min(pages.len().saturating_sub(1));
        terminal
            .draw(|frame| draw_pager(frame, bg, &pages, page, &mut scroll, refresh.is_some()))?;

        if crate::composer::input::stdin_gone() {
            return Ok(());
        }
        if !poll(Duration::from_millis(250))? {
            continue;
        }
        let Event::Key(KeyEvent {
            code,
            modifiers,
            kind,
            ..
        }) = read()?
        else {
            continue;
        };
        if kind == KeyEventKind::Release {
            continue;
        }
        match code {
            KeyCode::Esc | KeyCode::Char('q') => return Ok(()),
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(());
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                page = (page + 1) % pages.len();
                scroll = 0;
            }
            KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => {
                page = (page + pages.len() - 1) % pages.len();
                scroll = 0;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                scroll = scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                scroll += 1;
            }
            KeyCode::Char('r') => {
                if let Some(f) = refresh.as_mut() {
                    let next = (**f)();
                    if !next.is_empty() {
                        pages = next;
                    }
                }
            }
            _ => {}
        }
    }
}

#[path = "pager_tests.rs"]
#[cfg(test)]
mod tests;
