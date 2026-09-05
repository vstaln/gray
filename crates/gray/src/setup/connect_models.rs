//! Connect-modal render: model picker (split from `setup::connect`).

use super::connect::ConnectColors;
use super::*;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

#[allow(clippy::too_many_arguments)]
// Mechanical split of `run_connect_modal`: params are the modal state the arm renders.
pub(crate) fn render_selecting_model(
    frame: &mut Frame,
    area: Rect,
    item: &ConnectItem,
    models: &[(String, String)],
    m_filter: &str,
    m_sel: usize,
    m_scroll_top: &mut usize,
    config: &Config,
    colors: &ConnectColors,
) {
    let filtered_models: Vec<&(String, String)> = models
        .iter()
        .filter(|(m_id, m_name)| {
            let f = m_filter.to_lowercase();
            f.is_empty() || m_id.to_lowercase().contains(&f) || m_name.to_lowercase().contains(&f)
        })
        .collect();

    let modal_w = 68.min(area.width.saturating_sub(4)).max(42).min(area.width);
    let modal_h = 20
        .min(area.height.saturating_sub(2))
        .max(12)
        .min(area.height);
    let modal_x = (area.width.saturating_sub(modal_w)) / 2;
    let modal_y = (area.height.saturating_sub(modal_h)) / 3;
    let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);

    frame.render_widget(Clear, modal_rect);

    let box_block = Block::default().style(Style::default().bg(colors.box_bg));
    frame.render_widget(box_block, modal_rect);

    let pad_x = 3u16;
    let inner_w = modal_w.saturating_sub(pad_x * 2);
    let inner = Rect::new(
        modal_x + pad_x,
        modal_y + 1,
        inner_w,
        modal_h.saturating_sub(2),
    );

    // Header: Select Model — Provider ... esc
    let title_str = format!("Select model \u{2014} {}", item.name);
    let esc_str = "esc";
    let pad_len =
        (inner.width as usize).saturating_sub(title_str.chars().count() + esc_str.chars().count());
    let header_line = Line::from(vec![
        Span::styled(
            title_str,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
                .bg(colors.box_bg),
        ),
        Span::styled(" ".repeat(pad_len), Style::default().bg(colors.box_bg)),
        Span::styled(
            esc_str,
            Style::default().fg(colors.text_dim).bg(colors.box_bg),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(header_line),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    // Search Bar
    let search_line = if m_filter.is_empty() {
        Line::from(vec![
            Span::styled(
                "Search: ",
                Style::default()
                    .fg(colors.accent_peach)
                    .add_modifier(Modifier::BOLD)
                    .bg(colors.box_bg),
            ),
            Span::styled(
                "Type to filter models...",
                Style::default()
                    .fg(Color::Rgb(90, 90, 90))
                    .bg(colors.box_bg),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                "Search: ",
                Style::default()
                    .fg(colors.accent_peach)
                    .add_modifier(Modifier::BOLD)
                    .bg(colors.box_bg),
            ),
            Span::styled(
                m_filter,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
                    .bg(colors.box_bg),
            ),
            Span::styled(
                "▎",
                Style::default().fg(colors.accent_peach).bg(colors.box_bg),
            ),
        ])
    };
    frame.render_widget(
        Paragraph::new(search_line),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );

    // Model list
    let list_y = inner.y + 3;
    let list_h = inner.height.saturating_sub(4) as usize;

    if filtered_models.is_empty() {
        let empty_msg = if m_filter.is_empty() {
            Paragraph::new(Line::from(vec![Span::styled(
                "  No models listed — press Enter to continue",
                Style::default().fg(colors.text_dim).bg(colors.box_bg),
            )]))
        } else {
            Paragraph::new(Line::from(vec![
                Span::styled(
                    "  Use custom model: ",
                    Style::default().fg(colors.text_dim).bg(colors.box_bg),
                ),
                Span::styled(
                    m_filter,
                    Style::default()
                        .fg(colors.accent_peach)
                        .add_modifier(Modifier::BOLD)
                        .bg(colors.box_bg),
                ),
            ]))
        };
        frame.render_widget(empty_msg, Rect::new(inner.x, list_y + 1, inner.width, 1));
    } else {
        let safe_sel = m_sel.min(filtered_models.len().saturating_sub(1));
        if safe_sel < *m_scroll_top {
            *m_scroll_top = safe_sel;
        } else if safe_sel >= *m_scroll_top + list_h {
            *m_scroll_top = safe_sel.saturating_sub(list_h.saturating_sub(1));
        }

        for r in 0..list_h {
            let idx = *m_scroll_top + r;
            if idx >= filtered_models.len() {
                break;
            }

            let (m_id, m_name) = filtered_models[idx];
            let is_selected = idx == safe_sel;
            let is_current = config.model.as_deref() == Some(m_id.as_str());

            let check_glyph = if is_current { "✓ " } else { "  " };

            let display_name = if m_name.is_empty() {
                m_id.as_str()
            } else {
                m_name.as_str()
            };
            let sub = if m_name.is_empty() || m_name == m_id {
                String::new()
            } else {
                format!(" {}", m_id)
            };

            let raw_content = format!(" {check_glyph}{}{sub}", display_name);
            let fill = (inner.width as usize).saturating_sub(raw_content.chars().count());
            let full_row_str = format!("{}{}", raw_content, " ".repeat(fill));

            let row_line = if is_selected {
                Line::from(Span::styled(
                    full_row_str,
                    Style::default()
                        .fg(Color::Black)
                        .bg(colors.accent_peach)
                        .add_modifier(Modifier::BOLD),
                ))
            } else {
                let check_span = if is_current {
                    Span::styled(
                        " ✓ ",
                        Style::default()
                            .fg(Color::Rgb(74, 222, 128))
                            .add_modifier(Modifier::BOLD)
                            .bg(colors.box_bg),
                    )
                } else {
                    Span::styled("   ", Style::default().bg(colors.box_bg))
                };
                let name_span = Span::styled(
                    display_name,
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                        .bg(colors.box_bg),
                );
                let sub_span = Span::styled(
                    sub,
                    Style::default()
                        .fg(Color::Rgb(130, 130, 130))
                        .bg(colors.box_bg),
                );
                let pad_span = Span::styled(" ".repeat(fill), Style::default().bg(colors.box_bg));
                Line::from(vec![check_span, name_span, sub_span, pad_span])
            };

            frame.render_widget(
                Paragraph::new(row_line),
                Rect::new(inner.x, list_y + r as u16, inner.width, 1),
            );
        }
    }

    // Footer
    let footer_line = Line::from(vec![
        Span::styled(
            "↑↓ ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
                .bg(colors.box_bg),
        ),
        Span::styled(
            "navigate    ",
            Style::default().fg(colors.text_dim).bg(colors.box_bg),
        ),
        Span::styled(
            "enter ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
                .bg(colors.box_bg),
        ),
        Span::styled(
            "select",
            Style::default().fg(colors.text_dim).bg(colors.box_bg),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(footer_line),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
}
