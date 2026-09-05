//! Connect-modal render: provider list + API-key entry (split from `setup::connect`).

use super::connect::ConnectColors;
use super::*;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

#[allow(clippy::too_many_arguments)]
// Mechanical split of `run_connect_modal`: params are the modal state the arm renders.
pub(crate) fn render_selecting(
    frame: &mut Frame,
    area: Rect,
    all_items: &[ConnectItem],
    filter: &str,
    sel: usize,
    scroll_top: &mut usize,
    config: &Config,
    auth_keys: &std::collections::BTreeMap<String, String>,
    colors: &ConnectColors,
) {
    let modal_w = 68.min(area.width.saturating_sub(4)).max(42).min(area.width);
    let modal_h = 18
        .min(area.height.saturating_sub(2))
        .max(10)
        .min(area.height);
    let modal_x = (area.width.saturating_sub(modal_w)) / 2;
    let modal_y = (area.height.saturating_sub(modal_h)) / 3;
    let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);
    let filtered: Vec<&ConnectItem> = all_items
        .iter()
        .filter(|item| {
            let f = filter.to_lowercase();
            f.is_empty()
                || item.name.to_lowercase().contains(&f)
                || item.id.to_lowercase().contains(&f)
                || item.sublabel.to_lowercase().contains(&f)
        })
        .collect();

    // Clear popup background
    frame.render_widget(Clear, modal_rect);

    // Container Box (pure colored block matching text box, no border characters)
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

    // 1. Header Line (with esc at top right)
    let title_str = "Connect a provider";
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

    // 2. Search Bar
    let search_line = if filter.is_empty() {
        Line::from(vec![
            Span::styled(
                "Search: ",
                Style::default()
                    .fg(colors.accent_peach)
                    .add_modifier(Modifier::BOLD)
                    .bg(colors.box_bg),
            ),
            Span::styled(
                "Type to filter providers...",
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
                filter,
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

    // 3. Provider List
    let list_y = inner.y + 3;
    let list_h = inner.height.saturating_sub(4) as usize;

    if filtered.is_empty() {
        let empty_msg = Paragraph::new(Line::from(vec![Span::styled(
            "  No matching providers found",
            Style::default().fg(colors.text_dim).bg(colors.box_bg),
        )]));
        frame.render_widget(empty_msg, Rect::new(inner.x, list_y + 1, inner.width, 1));
    } else {
        let safe_sel = sel.min(filtered.len().saturating_sub(1));
        if safe_sel < *scroll_top {
            *scroll_top = safe_sel;
        } else if safe_sel >= *scroll_top + list_h {
            *scroll_top = safe_sel.saturating_sub(list_h.saturating_sub(1));
        }

        for r in 0..list_h {
            let idx = *scroll_top + r;
            if idx >= filtered.len() {
                break;
            }

            let item = filtered[idx];
            let is_selected = idx == safe_sel;

            let is_connected = auth_keys.contains_key(&item.id)
                || (config.base_url == item.base_url && config.api_key.is_some());

            let check_glyph = if is_connected { "✓ " } else { "  " };

            let sub = if item.sublabel.is_empty() {
                String::new()
            } else {
                format!(" {}", item.sublabel)
            };

            let raw_content = format!(" {check_glyph}{}{sub}", item.name);
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
                let check_span = if is_connected {
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
                    &item.name,
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

    // 4. Footer Help Line (no brackets)
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

pub(crate) fn render_entering_key(
    frame: &mut Frame,
    area: Rect,
    item: &ConnectItem,
    key_buf: &str,
    existing_key: &Option<String>,
    status_msg: &Option<String>,
    colors: &ConnectColors,
) {
    let dialog_w = 64.min(area.width.saturating_sub(4)).max(40).min(area.width);
    let dialog_h = 10
        .min(area.height.saturating_sub(2))
        .max(8)
        .min(area.height);
    let dialog_x = (area.width.saturating_sub(dialog_w)) / 2;
    let dialog_y = (area.height.saturating_sub(dialog_h)) / 3;
    let dialog_rect = Rect::new(dialog_x, dialog_y, dialog_w, dialog_h);

    frame.render_widget(Clear, dialog_rect);

    let box_block = Block::default().style(Style::default().bg(colors.box_bg));
    frame.render_widget(box_block, dialog_rect);

    let pad_x = 3u16;
    let inner_w = dialog_w.saturating_sub(pad_x * 2);
    let inner = Rect::new(
        dialog_x + pad_x,
        dialog_y + 1,
        inner_w,
        dialog_h.saturating_sub(2),
    );

    // Header (with esc at top right)
    let title_str = "API Key Configuration";
    let esc_str = "esc";
    let pad_len =
        (inner.width as usize).saturating_sub(title_str.chars().count() + esc_str.chars().count());
    let line0 = Line::from(vec![
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
    let line1 = Line::from(vec![Span::styled(
        format!("Provider: {}", item.name),
        Style::default().fg(colors.text_dim).bg(colors.box_bg),
    )]);
    frame.render_widget(
        Paragraph::new(line0),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    frame.render_widget(
        Paragraph::new(line1),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );

    // Input Box (inset colored block)
    let input_content = if key_buf.is_empty() {
        if let Some(existing) = existing_key.as_ref() {
            let masked = mask_key_pretty(existing);
            Line::from(vec![
                Span::styled(
                    format!(" {masked}"),
                    Style::default()
                        .fg(Color::Rgb(210, 210, 210))
                        .bg(colors.input_bg),
                ),
                Span::styled(
                    "  \u{00b7} Enter to keep, paste to replace",
                    Style::default()
                        .fg(Color::Rgb(110, 110, 110))
                        .bg(colors.input_bg),
                ),
            ])
        } else {
            Line::from(vec![Span::styled(
                " Paste or type API key...",
                Style::default()
                    .fg(Color::Rgb(110, 110, 110))
                    .bg(colors.input_bg),
            )])
        }
    } else {
        let masked = "•".repeat(key_buf.chars().count());
        Line::from(vec![Span::styled(
            format!(" {masked}"),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
                .bg(colors.input_bg),
        )])
    };

    let input_rect = Rect::new(inner.x, inner.y + 3, inner.width, 1);
    frame.render_widget(Clear, input_rect);
    frame.render_widget(
        Paragraph::new(input_content).style(Style::default().bg(colors.input_bg)),
        input_rect,
    );

    // Status or note
    let note_line = if let Some(msg) = status_msg {
        Line::from(Span::styled(
            format!(" \u{2022} {msg}"),
            Style::default()
                .fg(Color::Rgb(239, 68, 68))
                .bg(colors.box_bg),
        ))
    } else {
        Line::from(Span::styled(
            " (Key stored securely in ~/.gray/auth.json)",
            Style::default()
                .fg(Color::Rgb(90, 90, 90))
                .bg(colors.box_bg),
        ))
    };
    frame.render_widget(
        Paragraph::new(note_line),
        Rect::new(inner.x, inner.y + 5, inner.width, 1),
    );

    // Footer buttons (enter update / submit - no brackets)
    let action_label = if existing_key.is_some() {
        "update"
    } else {
        "submit"
    };
    let footer = Line::from(vec![
        Span::styled(
            "enter ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
                .bg(colors.box_bg),
        ),
        Span::styled(
            action_label,
            Style::default().fg(colors.text_dim).bg(colors.box_bg),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(footer),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
}
