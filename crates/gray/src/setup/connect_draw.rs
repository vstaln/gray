//! Connect-modal render: provider list + API-key entry (split from `setup::connect`).

use super::connect::ConnectColors;
use super::*;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

/// Centered dialog: clears the region, paints the bg block, returns the padded inner rect.
fn centered_dialog(
    frame: &mut Frame,
    area: Rect,
    w: u16,
    h: u16,
    min_w: u16,
    min_h: u16,
    colors: &ConnectColors,
) -> Rect {
    let w = w
        .min(area.width.saturating_sub(4))
        .max(min_w)
        .min(area.width);
    let h = h
        .min(area.height.saturating_sub(2))
        .max(min_h)
        .min(area.height);
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 3;
    let rect = Rect::new(x, y, w, h);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Block::default().style(Style::default().bg(colors.box_bg)),
        rect,
    );
    Rect::new(x + 3, y + 1, w.saturating_sub(6), h.saturating_sub(2))
}

/// Title row with the padded "esc" hint at the right edge.
fn render_header_esc(frame: &mut Frame, inner: Rect, title: &str, colors: &ConnectColors) {
    let pad_len =
        (inner.width as usize).saturating_sub(title.chars().count() + "esc".chars().count());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                title,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
                    .bg(colors.box_bg),
            ),
            Span::styled(" ".repeat(pad_len), Style::default().bg(colors.box_bg)),
            Span::styled(
                "esc",
                Style::default().fg(colors.text_dim).bg(colors.box_bg),
            ),
        ])),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
}

/// Footer of (key, description) pairs: bold keys, dim descriptions.
/// Bold-key / dim-desc spans for a static footer table.
fn footer_key_spans<'a>(keys: &[(&'a str, &'a str)], colors: &ConnectColors) -> Vec<Span<'a>> {
    keys.iter()
        .flat_map(|(k, v)| {
            [
                Span::styled(
                    *k,
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                        .bg(colors.box_bg),
                ),
                Span::styled(*v, Style::default().fg(colors.text_dim).bg(colors.box_bg)),
            ]
        })
        .collect()
}

/// Render a footer line at the bottom of `inner`. Takes pre-built spans so a
/// caller whose footer is conditional (the provider list appends a
/// shift+enter hint only for rows that hold a credential) shares the
/// positioning without the styling being frozen to a static table.
fn render_footer(frame: &mut Frame, inner: Rect, spans: Vec<Span<'_>>) {
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
}

/// Status message (error-styled bullet) or a dim default note, one row at y+5.
fn render_note_or_status(
    frame: &mut Frame,
    inner: Rect,
    status_msg: &Option<String>,
    note: &str,
    colors: &ConnectColors,
) {
    let span = match status_msg {
        Some(msg) => Span::styled(
            format!(" \u{2022} {msg}"),
            Style::default()
                .fg(crate::theme::theme().error)
                .bg(colors.box_bg),
        ),
        None => Span::styled(
            note,
            Style::default()
                .fg(crate::theme::theme().text_dim)
                .bg(colors.box_bg),
        ),
    };
    frame.render_widget(
        Paragraph::new(Line::from(span)),
        Rect::new(inner.x, inner.y + 5, inner.width, 1),
    );
}

/// Clears and renders one input row with the input background.
fn render_input_row(frame: &mut Frame, inner: Rect, content: Line, colors: &ConnectColors) {
    let rect = Rect::new(inner.x, inner.y + 3, inner.width, 1);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(content).style(Style::default().bg(colors.input_bg)),
        rect,
    );
}

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
    auth: &std::collections::BTreeMap<String, catalog::AuthEntry>,
    colors: &ConnectColors,
) {
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
    let inner = centered_dialog(frame, area, 68, 18, 42, 10, colors);

    // 1. Header Line (with esc at top right)
    render_header_esc(frame, inner, "Connect a provider", colors);

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
                    .fg(crate::theme::theme().text_dim)
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

    // The separator follows Custom even when connected providers precede it.
    let list_y = inner.y + 3;
    let list_h = inner.height.saturating_sub(4) as usize;
    let mut rows = Vec::new();
    for (idx, item) in filtered.iter().enumerate() {
        rows.push(Some(idx));
        if item.id == "custom" && idx + 1 < filtered.len() && list_h > 1 {
            rows.push(None);
        }
    }

    if filtered.is_empty() {
        let empty_msg = Paragraph::new(Line::from(vec![Span::styled(
            "  No matching providers found",
            Style::default().fg(colors.text_dim).bg(colors.box_bg),
        )]));
        frame.render_widget(empty_msg, Rect::new(inner.x, list_y + 1, inner.width, 1));
    } else {
        let safe_sel = sel.min(filtered.len().saturating_sub(1));
        let selected_row = rows
            .iter()
            .position(|row| *row == Some(safe_sel))
            .unwrap_or(0);
        if selected_row < *scroll_top {
            *scroll_top = selected_row;
        } else if selected_row >= *scroll_top + list_h {
            *scroll_top = selected_row.saturating_sub(list_h.saturating_sub(1));
        }

        for (r, row) in rows.iter().skip(*scroll_top).take(list_h).enumerate() {
            let Some(idx) = *row else {
                let rule = "─".repeat(inner.width as usize);
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        rule,
                        Style::default().fg(colors.text_dim).bg(colors.box_bg),
                    ))),
                    Rect::new(inner.x, list_y + r as u16, inner.width, 1),
                );
                continue;
            };

            let item = filtered[idx];
            let is_selected = idx == safe_sel;

            let is_connected = item.is_connected(config, auth);

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
                        .fg(crate::theme::theme().on_selection)
                        .bg(colors.accent_peach)
                        .add_modifier(Modifier::BOLD),
                ))
            } else {
                let check_span = if is_connected {
                    Span::styled(
                        " ✓ ",
                        Style::default()
                            .fg(crate::theme::theme().success)
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
                        .fg(crate::theme::theme().text_dim)
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
    let mut footer_spans =
        footer_key_spans(&[("↑↓ ", "navigate    "), ("enter ", "select")], colors);
    // Removal is only offered for a row that actually holds a credential.
    if filtered
        .get(sel.min(filtered.len().saturating_sub(1)))
        .is_some_and(|item| super::connect::is_removable(item, config, auth))
    {
        footer_spans.extend([
            Span::styled(
                "    ",
                Style::default().fg(colors.text_dim).bg(colors.box_bg),
            ),
            Span::styled(
                "shift+enter ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
                    .bg(colors.box_bg),
            ),
            Span::styled(
                "remove",
                Style::default().fg(colors.text_dim).bg(colors.box_bg),
            ),
        ]);
    }
    render_footer(frame, inner, footer_spans);
}

/// Shift+Enter confirmation: names the provider (and its endpoint, which is
/// what a custom entry is really keyed by) and states the one consequence.
pub(crate) fn render_confirm_remove(
    frame: &mut Frame,
    area: Rect,
    item: &ConnectItem,
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
    frame.render_widget(
        Block::default().style(Style::default().bg(colors.box_bg)),
        dialog_rect,
    );

    let pad_x = 3u16;
    let inner_w = dialog_w.saturating_sub(pad_x * 2);
    let inner = Rect::new(
        dialog_x + pad_x,
        dialog_y + 1,
        inner_w,
        dialog_h.saturating_sub(2),
    );

    let title_str = "Remove provider";
    let esc_str = "esc";
    let pad_len =
        (inner.width as usize).saturating_sub(title_str.chars().count() + esc_str.chars().count());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
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
        ])),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let rows = [
        Line::from(vec![Span::styled(
            format!("Remove {}?", item.name),
            Style::default()
                .fg(crate::theme::theme().accent)
                .add_modifier(Modifier::BOLD)
                .bg(colors.box_bg),
        )]),
        Line::from(vec![Span::styled(
            format!(" {}", item.base_url),
            Style::default()
                .fg(crate::theme::theme().text_dim)
                .bg(colors.box_bg),
        )]),
        Line::from(vec![Span::styled(
            "The stored API key will be deleted.",
            Style::default().fg(colors.text_dim).bg(colors.box_bg),
        )]),
    ];
    for (i, row) in rows.iter().enumerate() {
        frame.render_widget(
            Paragraph::new(row.clone()),
            Rect::new(inner.x, inner.y + 2 + i as u16, inner.width, 1),
        );
    }

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "enter ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
                    .bg(colors.box_bg),
            ),
            Span::styled(
                "confirm    ",
                Style::default().fg(colors.text_dim).bg(colors.box_bg),
            ),
            Span::styled(
                "esc ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
                    .bg(colors.box_bg),
            ),
            Span::styled(
                "cancel",
                Style::default().fg(colors.text_dim).bg(colors.box_bg),
            ),
        ])),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
}

pub(crate) fn render_entering_url(
    frame: &mut Frame,
    area: Rect,
    url_buf: &str,
    status_msg: &Option<String>,
    colors: &ConnectColors,
) {
    let inner = centered_dialog(frame, area, 64, 10, 40, 8, colors);

    render_header_esc(frame, inner, "Custom Provider", colors);
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "Provider: Custom (OpenAI/Anthropic compatible)",
            Style::default().fg(colors.text_dim).bg(colors.box_bg),
        )])),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );

    let input_content = if url_buf.is_empty() {
        Line::from(vec![Span::styled(
            " Paste or type base URL (https://…/v1)...",
            Style::default()
                .fg(crate::theme::theme().text_dim)
                .bg(colors.input_bg),
        )])
    } else {
        Line::from(vec![
            Span::styled(
                format!(" {url_buf}"),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
                    .bg(colors.input_bg),
            ),
            Span::styled(
                "▎",
                Style::default().fg(colors.accent_peach).bg(colors.input_bg),
            ),
        ])
    };

    render_input_row(frame, inner, input_content, colors);
    render_note_or_status(
        frame,
        inner,
        status_msg,
        " (Route suffixes like /chat/completions are trimmed)",
        colors,
    );
    render_footer(
        frame,
        inner,
        footer_key_spans(&[("enter ", "continue")], colors),
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
    let inner = centered_dialog(frame, area, 64, 10, 40, 8, colors);

    // Header (with esc at top right)
    render_header_esc(frame, inner, "API Key Configuration", colors);
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            format!("Provider: {}", item.name),
            Style::default().fg(colors.text_dim).bg(colors.box_bg),
        )])),
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
                        .fg(crate::theme::theme().text_soft)
                        .bg(colors.input_bg),
                ),
                Span::styled(
                    "  \u{00b7} Enter to keep, paste to replace",
                    Style::default()
                        .fg(crate::theme::theme().text_dim)
                        .bg(colors.input_bg),
                ),
            ])
        } else if item.id == "custom" {
            Line::from(vec![Span::styled(
                " Paste or type API key (Enter to skip)...",
                Style::default()
                    .fg(crate::theme::theme().text_dim)
                    .bg(colors.input_bg),
            )])
        } else {
            Line::from(vec![Span::styled(
                " Paste or type API key...",
                Style::default()
                    .fg(crate::theme::theme().text_dim)
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

    render_input_row(frame, inner, input_content, colors);

    // Status or note
    render_note_or_status(
        frame,
        inner,
        status_msg,
        " (Key stored securely in ~/.gray/auth.json)",
        colors,
    );

    // Footer buttons (enter update / submit - no brackets)
    let action_label = if existing_key.is_some() {
        "update"
    } else {
        "submit"
    };
    render_footer(
        frame,
        inner,
        footer_key_spans(&[("enter ", action_label)], colors),
    );
}

#[cfg(test)]
#[path = "connect_draw_tests.rs"]
mod tests;
