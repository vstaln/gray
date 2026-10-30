use super::*;

pub(super) fn row_name(m: &(String, String)) -> &str {
    if m.1.is_empty() { &m.0 } else { &m.1 }
}

pub(super) fn put_line(
    frame: &mut ratatui::Frame,
    at: ratatui::layout::Rect,
    line: ratatui::text::Line<'_>,
) {
    let base = ratatui::style::Style::default().bg(crate::theme::theme().surface_bg);
    frame.render_widget(ratatui::widgets::Paragraph::new(line).style(base), at);
}

pub(super) const PICKER_CHROME: usize = 8;

pub(super) fn lists_fast_variants(models: &[(String, String)]) -> bool {
    models.iter().any(|(id, _)| {
        super::super::variants::row_shape(id).is_some_and(|s| s.variants.iter().any(|v| v.fast))
            || split_fast_variant(id, models).is_some()
    })
}

pub(super) fn listed_slots(models: &[(String, String)]) -> usize {
    models
        .iter()
        .filter_map(|(id, _)| super::super::variants::row_shape(id))
        .map(|s| s.slots.len())
        .max()
        .unwrap_or(0)
}

pub(super) fn detail_rows(models: &[(String, String)]) -> usize {
    let fast = lists_fast_variants(models);
    let priced = models
        .iter()
        .any(|(id, _)| super::super::context::get_model_rate(id).is_some());
    let ctxed = models.iter().any(|(id, _)| known_context(id).is_some());
    let metrics = usize::from(priced) + usize::from(ctxed);
    let head = listed_slots(models) + usize::from(fast);
    let head = if head > 0 {
        head + usize::from(metrics > 0)
    } else {
        0
    };
    match head + metrics {
        0 => 0,
        n => n + 1,
    }
}

pub(super) const ROW_TAIL: usize = 2 + 1 + 1 + 2 + METER_W + 2 + 1 + 7;

pub(super) const RECOMMENDED_HEAD: usize = 5;

pub(super) fn meter_glyphs() -> (&'static str, &'static str) {
    if super::super::icons::has_nerd_font() {
        ("⬢", "⬡")
    } else {
        ("■", "□")
    }
}

pub(super) fn bar_soft() -> ratatui::style::Color {
    use ratatui::style::Color;
    match crate::theme::theme().accent {
        Color::Rgb(r, g, b) => Color::Rgb(
            (r as f32 * 0.45) as u8,
            (g as f32 * 0.45) as u8,
            (b as f32 * 0.45) as u8,
        ),
        c => c,
    }
}

pub(super) struct PickerView<'a> {
    pub(super) provider: &'a str,
    pub(super) refreshing: bool,
    pub(super) filter: &'a str,
    pub(super) rows: &'a [Row],
    pub(super) filtered: &'a [&'a (String, String)],
    pub(super) models: &'a [(String, String)],
    pub(super) config: &'a Config,
    pub(super) saved: &'a SavedConfig,
    pub(super) efforts: &'a RowEfforts,
    pub(super) toggles: PickerToggles,
    pub(super) drop: Option<&'a DropState>,

    pub(super) all_head: bool,

    pub(super) gap: Option<usize>,
}

impl PickerView<'_> {
    pub(super) fn highlighted(&self, sel: usize) -> Option<&str> {
        match self.rows.get(sel) {
            Some(Row::Model(mi)) => self.filtered.get(*mi).map(|(id, _)| id.as_str()),
            Some(Row::Opt(_)) => self.drop.map(|d| d.row.as_str()),
            _ => None,
        }
    }

    pub(super) fn live_parts(&self, model: &str) -> super::super::variants::Parts {
        let parts = self.efforts.parts_of(self.config, self.saved, model);
        if let Some(d) = self.drop.filter(|d| d.row == model)
            && let Some(shape) = super::super::variants::row_shape(model)
            && let Some(opt) = shape.slots.get(d.slot).and_then(|s| s.options.get(d.sel))
            && let Some(p) = option_parts(&shape, &parts, d.slot, &opt.id)
        {
            return p;
        }
        parts
    }

    pub(super) fn fast_variant(&self, model: &str) -> Option<String> {
        if let Some(shape) = super::super::variants::row_shape(model)
            && !shape.variants.is_empty()
        {
            let parts = selection_parts(&shape, &self.live_parts(model));
            if !super::super::variants::has_variant(&shape.variants, true, &parts) {
                return None;
            }
            let effort = self
                .efforts
                .shown_with(self.config, self.saved, model, &parts);
            return super::super::variants::resolve_variant(&shape.variants, effort, true, &parts)
                .map(|v| v.id.clone());
        }
        let effort = self.efforts.shown(self.config, self.saved, model);
        compose_fast_model(model, effort, self.models)
    }
}

pub(super) fn picker_row_line(
    v: &PickerView<'_>,
    m: &(String, String),
    selected: bool,
    name_w: usize,
    w: usize,
) -> ratatui::text::Line<'static> {
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    let t = crate::theme::theme();
    let fg = |c: Color| Style::default().fg(c);

    let (on, on_dim) = (t.on_selection, bar_soft());
    let sel = |s: Style| {
        if selected { s.bg(t.accent) } else { s }
    };
    let (id, name) = (m.0.as_str(), row_name(m));

    let mut spans = vec![
        Span::styled(
            if selected {
                format!("{} ", super::super::icon("arrow"))
            } else {
                "  ".to_string()
            },
            sel(fg(on)),
        ),
        Span::styled(
            fit_chars(name, name_w),
            sel(if selected {
                fg(on).add_modifier(Modifier::BOLD)
            } else {
                fg(Color::White)
            }),
        ),
    ];
    let pad = name_w.saturating_sub(spans[1].content.chars().count()) + 2;
    spans.push(Span::styled(" ".repeat(pad), sel(Style::default())));
    let current = v.config.model.as_deref() == Some(id);
    spans.push(Span::styled(
        if current { "✓" } else { " " },
        sel(fg(if selected { on } else { t.success })),
    ));
    spans.push(Span::styled(" ", sel(Style::default())));
    let mut used = 2 + name_w + 2 + 1 + 1;

    let parts = v.efforts.parts_of(v.config, v.saved, id);
    if let Some(level) = v.efforts.shown_with(v.config, v.saved, id, &parts) {
        let levels = row_levels(id, v.efforts.fast, &parts);
        spans.extend(meter_spans(&levels, level, selected));
        used += 2 + METER_W + 2 + 1 + 7;
    }
    let suffix = sel(fg(if selected { on_dim } else { t.text_dim }));
    let shape = super::super::variants::row_shape(id).filter(|s| s.is_composite());
    if let Some(shape) = shape {
        let summary: Vec<&str> = shape
            .slots
            .iter()
            .filter_map(|slot| {
                let opt = parts.get(&slot.key)?;
                Some(shape.option_name(&slot.key, opt).unwrap_or(opt))
            })
            .collect();
        let room = w.saturating_sub(used + 2);
        if room >= 8 && !summary.is_empty() {
            spans.push(Span::styled(
                format!("  {}", fit_chars(&summary.join(" × "), room)),
                suffix,
            ));
        }
    } else if selected && name != id && v.models.iter().filter(|o| row_name(o) == name).count() > 1
    {
        let room = w.saturating_sub(used + 2);
        if room >= 8 {
            spans.push(Span::styled(format!("  {}", fit_chars(id, room)), suffix));
        }
    }

    if selected {
        let fill = w.saturating_sub(spans.iter().map(|s| s.width()).sum::<usize>());
        spans.push(Span::styled(
            " ".repeat(fill),
            Style::default().bg(t.accent),
        ));
    }
    Line::from(spans)
}

pub(crate) fn meter_spans(
    levels: &[&'static str],
    level: &'static str,
    selected: bool,
) -> Vec<ratatui::text::Span<'static>> {
    use ratatui::style::Style;
    use ratatui::text::Span;
    let t = crate::theme::theme();
    let fg = |c: ratatui::style::Color| Style::default().fg(c);
    let (cell, hollow) = meter_glyphs();
    let fill = meter_fill(levels, level, METER_W);
    let pos = levels.iter().position(|l| *l == level);
    let (on, on_dim) = (t.on_selection, bar_soft());
    let sel = |s: Style| {
        if selected { s.bg(t.accent) } else { s }
    };
    let arrow = |live: bool| {
        sel(fg(if selected {
            if live { on } else { on_dim }
        } else if live {
            t.accent
        } else {
            t.text_faint
        }))
    };
    let (left, right) = if selected {
        ("← ", " →")
    } else {
        ("  ", "  ")
    };
    vec![
        Span::styled(left, arrow(pos.is_some_and(|p| p > 0))),
        Span::styled(
            cell.repeat(fill),
            sel(fg(if selected { on } else { t.text_muted })),
        ),
        Span::styled(
            hollow.repeat(METER_W - fill),
            sel(fg(if selected { on_dim } else { t.text_faint })),
        ),
        Span::styled(right, arrow(pos.is_some_and(|p| p + 1 < levels.len()))),
        Span::styled(" ", sel(Style::default())),
        Span::styled(
            format!("{:<7}", effort_label(level)),
            sel(fg(if selected { on } else { t.text_muted })),
        ),
    ]
}

pub(super) fn picker_option_line(
    v: &PickerView<'_>,
    drop: &DropState,
    opt: usize,
    selected: bool,
    name_w: usize,
    w: usize,
) -> ratatui::text::Line<'static> {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    let t = crate::theme::theme();
    let fg = |c: ratatui::style::Color| Style::default().fg(c);
    let on = t.on_selection;
    let sel = |s: Style| {
        if selected { s.bg(t.accent) } else { s }
    };
    let Some(shape) = super::super::variants::row_shape(&drop.row) else {
        return Line::default();
    };
    let Some(slot) = shape.slots.get(drop.slot) else {
        return Line::default();
    };
    let Some(option) = slot.options.get(opt) else {
        return Line::default();
    };
    let cur = v.efforts.parts_of(v.config, v.saved, &drop.row);
    let parts = option_parts(&shape, &cur, drop.slot, &option.id);
    let style = sel(match (&parts, selected) {
        (None, _) => fg(t.text_faint),
        (Some(_), true) => fg(on).add_modifier(Modifier::BOLD),
        (Some(_), false) => fg(t.text_soft),
    });
    let inner_w = name_w.saturating_sub(2);
    let name = fit_chars(&option.name, inner_w);
    let pad = inner_w.saturating_sub(name.chars().count()) + 2;
    let badge = if selected {
        on
    } else if parts.is_some() {
        t.success
    } else {
        t.text_faint
    };
    let mut spans = vec![
        Span::styled("  ", sel(Style::default())),
        Span::styled(
            if selected {
                format!("{} ", super::super::icon("arrow"))
            } else {
                "  ".to_string()
            },
            sel(fg(on)),
        ),
        Span::styled(name, style),
        Span::styled(" ".repeat(pad), sel(Style::default())),
        Span::styled(
            if cur.get(&slot.key) == Some(&option.id) {
                "✓"
            } else {
                " "
            },
            sel(fg(badge)),
        ),
        Span::styled(" ", sel(Style::default())),
    ];

    if drop.slot == 0
        && let Some(parts) = &parts
        && let Some(level) = v.efforts.shown_with(v.config, v.saved, &drop.row, parts)
    {
        let levels = row_levels(&drop.row, v.efforts.fast, parts);
        spans.extend(meter_spans(&levels, level, selected));
    }
    if selected {
        let fill = w.saturating_sub(spans.iter().map(|s| s.width()).sum::<usize>());
        spans.push(Span::styled(
            " ".repeat(fill),
            Style::default().bg(t.accent),
        ));
    }
    Line::from(spans)
}

pub(super) fn picker_detail_lines(
    v: &PickerView<'_>,
    id: &str,
    w: usize,
) -> Vec<ratatui::text::Line<'static>> {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    let t = crate::theme::theme();
    let fg = |c: ratatui::style::Color| Style::default().fg(c);
    let mut lines: Vec<Line<'static>> = Vec::new();

    if let Some(shape) = super::super::variants::row_shape(id).filter(|s| s.is_composite()) {
        let parts = v.live_parts(id);
        let effort = v.efforts.shown_with(v.config, v.saved, id, &parts);
        for (i, slot) in shape.slots.iter().enumerate() {
            let mut name = parts
                .get(&slot.key)
                .map(|o| shape.option_name(&slot.key, o).unwrap_or(o).to_string())
                .unwrap_or_default();
            if i == 0
                && let Some(e) = effort.filter(|e| *e != "off")
            {
                name = format!("{name} {}", effort_label(e));
            }
            let open = v.drop.is_some_and(|d| d.row == id && d.slot == i);
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("{:<10}", slot.label), fg(t.text_soft)),
                Span::styled(
                    fit_chars(&name, w.saturating_sub(16)),
                    if open {
                        fg(t.accent).add_modifier(Modifier::BOLD)
                    } else {
                        fg(t.text_body)
                    },
                ),
                Span::styled(
                    if open { " ▾" } else { " ▸" },
                    fg(if open { t.accent } else { t.text_dim }),
                ),
            ]));
        }
    }

    if let Some(wire) = v.fast_variant(id) {
        let mut spans = vec![Span::raw("  "), Span::styled("Fast Mode ", fg(t.text_soft))];
        if v.toggles.fast {
            spans.push(Span::styled(
                "On",
                fg(t.accent).add_modifier(Modifier::BOLD),
            ));
            let room = w.saturating_sub(2 + "Fast Mode On".len() + 4);
            if room > 8 {
                spans.push(Span::styled(
                    format!("  → {}", fit_chars(&wire, room)),
                    fg(t.text_dim),
                ));
            }
        } else {
            spans.push(Span::styled("Off", fg(t.text_dim)));
        }
        lines.push(Line::from(spans));
    } else if lists_fast_variants(v.models) {
        lines.push(Line::default());
    }

    let rate = super::super::context::get_model_rate(id);
    let ctx = known_context(id);
    let priced = v
        .models
        .iter()
        .any(|(mid, _)| super::super::context::get_model_rate(mid).is_some());
    let ctxed = v.models.iter().any(|(mid, _)| known_context(mid).is_some());
    if !priced && !ctxed {
        return lines;
    }
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    let label = |l: &'static str| {
        vec![
            Span::raw("  "),
            Span::styled(format!("{l:<10}"), fg(t.text_soft)),
        ]
    };
    if priced {
        let mut spans = label("Price");
        match &rate {
            None => spans.push(Span::styled("—", fg(t.text_faint))),
            Some(r) => {
                let level = (w >= 2 + 10 + PRICE_DOTS + 2 + 12)
                    .then(|| price_level((r.input + r.output) * 1_000_000.0));
                if let Some(level) = level {
                    for i in 0..PRICE_DOTS {
                        spans.push(Span::styled(
                            if i < level { "●" } else { "○" },
                            if i < level {
                                fg(price_gradient(i, PRICE_DOTS))
                            } else {
                                fg(t.text_faint)
                            },
                        ));
                    }
                    spans.push(Span::raw("  "));
                }
                let room = w
                    .saturating_sub(2 + 10)
                    .saturating_sub(usize::from(level.is_some()) * (PRICE_DOTS + 2));
                spans.push(Span::styled(rate_text(r, room), fg(t.text_body)));
            }
        }
        lines.push(Line::from(spans));
    }
    if ctxed {
        let mut spans = label("Context");
        spans.push(match ctx {
            Some(c) => Span::styled(
                super::super::context::format_context_length(c),
                fg(t.text_body),
            ),
            None => Span::styled("—", fg(t.text_faint)),
        });
        lines.push(Line::from(spans));
    }
    lines
}

pub(super) fn footer_tiers<'a>(
    has_fast: bool,
    has_effort: bool,
    enter: &'a str,
    esc: &'a str,
    reasoning: &'a str,
) -> [Vec<(&'static str, &'a str, bool)>; 4] {
    [
        vec![
            ("↑↓", "select", true),
            ("tab", "fast mode", has_fast),
            ("←→", "effort", has_effort),
            ("↵", enter, true),
            ("esc", esc, true),
            ("ctrl+r", reasoning, true),
        ],
        vec![
            ("↑↓", "select", true),
            ("tab", "fast", has_fast),
            ("←→", "effort", has_effort),
            ("↵", enter, true),
            ("esc", "", true),
            ("ctrl+r", "reasoning", true),
        ],
        vec![
            ("↑↓", "", true),
            ("tab", "fast", has_fast),
            ("←→", "effort", has_effort),
            ("↵", "", true),
            ("esc", "", true),
            ("^r", "reasoning", true),
        ],
        vec![
            ("↑↓", "", true),
            ("tab", "fast", has_fast),
            ("←→", "effort", has_effort),
            ("↵", "", true),
            ("esc", "", true),
        ],
    ]
}

pub(super) fn footer_width(segs: &[(&str, &str, bool)]) -> usize {
    segs.iter()
        .map(|(k, d, _)| k.chars().count() + usize::from(!d.is_empty()) + d.chars().count())
        .sum::<usize>()
        + 3 * segs.len().saturating_sub(1)
}

pub(super) fn footer_floor_width() -> usize {
    footer_width(&footer_tiers(true, true, "confirm", "cancel", "show reasoning")[1])
}

pub(super) fn picker_footer(
    v: &PickerView<'_>,
    sel: usize,
    w: usize,
) -> ratatui::text::Line<'static> {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    let t = crate::theme::theme();
    let highlighted = v.highlighted(sel);
    let has_fast = highlighted.is_some_and(|id| v.fast_variant(id).is_some());

    let has_effort = highlighted.is_some_and(|id| {
        let parts = v.live_parts(id);
        v.drop.is_none_or(|d| d.slot == 0)
            && v.efforts
                .shown_with(v.config, v.saved, id, &parts)
                .is_some()
    });

    let shape = highlighted
        .and_then(super::super::variants::row_shape)
        .filter(|s| s.is_composite());
    let (enter, esc) = match (&shape, v.drop) {
        (Some(shape), Some(d)) => match shape.slots.get(d.slot + 1) {
            Some(next) => (next.label.to_lowercase(), "back"),
            None => ("apply".to_string(), "back"),
        },
        (Some(shape), None) => (
            shape
                .slots
                .first()
                .map_or("confirm".to_string(), |s| s.label.to_lowercase()),
            "cancel",
        ),
        (None, _) => ("confirm".to_string(), "cancel"),
    };
    let (enter, esc) = (enter.as_str(), esc);
    let reasoning = if v.toggles.show_reasoning {
        "hide reasoning"
    } else {
        "show reasoning"
    };
    let [first, second, third, last] = footer_tiers(has_fast, has_effort, enter, esc, reasoning);
    let segs = [first, second, third]
        .into_iter()
        .find(|segs| footer_width(segs) <= w)
        .unwrap_or(last);
    let mut spans = Vec::new();
    for (i, (key, does, live)) in segs.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(t.text_faint)));
        }

        let (kc, dc) = if live {
            (ratatui::style::Color::White, t.text_dim)
        } else {
            (t.text_faint, t.text_faint)
        };
        spans.push(Span::styled(
            key,
            Style::default().fg(kc).add_modifier(Modifier::BOLD),
        ));
        if !does.is_empty() {
            spans.push(Span::styled(format!(" {does}"), Style::default().fg(dc)));
        }
    }
    Line::from(spans)
}

pub(super) fn draw_picker(
    frame: &mut ratatui::Frame,
    bg: &BackgroundSnapshot,
    v: &PickerView<'_>,
    sel: usize,
    scroll_top: &mut usize,
) {
    use ratatui::layout::Rect;
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear};

    let area = frame.area();
    if area.width < 24 || area.height < 10 {
        return;
    }
    render_dimmed_background(frame, bg);
    let t = crate::theme::theme();
    let fg = |c: ratatui::style::Color| Style::default().fg(c);

    let avail = area.height.saturating_sub(2) as usize;

    let want = (v.models.len() + 1 + usize::from(gap_possible(v.models))).clamp(4, 12);
    let detail_need = detail_rows(v.models);
    let detail_h = if avail >= PICKER_CHROME + detail_need + 6 {
        detail_need
    } else {
        0
    };
    let list_h = want
        .min(avail.saturating_sub(PICKER_CHROME + detail_h))
        .max(1);
    let modal_h = ((PICKER_CHROME + list_h + detail_h) as u16).min(area.height);

    let longest = v
        .models
        .iter()
        .map(|m| row_name(m).chars().count())
        .max()
        .unwrap_or(0);
    let inner_want = (2 + longest + ROW_TAIL).max(footer_floor_width()).max(24);
    let modal_w = ((inner_want + 6) as u16)
        .min(92)
        .min(area.width.saturating_sub(4))
        .max(40)
        .min(area.width);
    let pad_x: u16 = if modal_w >= 60 { 3 } else { 2 };
    let x0 = area.x + (area.width - modal_w) / 2;
    let y0 = area.y + (area.height - modal_h) / 3;
    let rect = Rect::new(x0, y0, modal_w, modal_h);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Block::default().style(Style::default().bg(t.surface_bg)),
        rect,
    );
    let (ix, iw) = (x0 + pad_x, modal_w - pad_x * 2);
    let w = iw as usize;
    let at = |y: u16| Rect::new(ix, y, iw, 1);

    let title = "Select model";
    let gap = w.saturating_sub(title.chars().count() + 3);
    put_line(
        frame,
        at(y0 + 1),
        Line::from(vec![
            Span::styled(title, fg(t.text_bright).add_modifier(Modifier::BOLD)),
            Span::raw(" ".repeat(gap)),
            Span::styled("esc", fg(t.text_dim)),
        ]),
    );

    let mut sub = vec![Span::styled(
        fit_chars(v.provider, w.saturating_sub(14)),
        fg(t.text_dim),
    )];
    if v.refreshing {
        sub.push(Span::styled(" · refreshing…", fg(t.text_faint)));
    }
    put_line(frame, at(y0 + 2), Line::from(sub));

    let label = Span::styled("Search: ", fg(t.accent).add_modifier(Modifier::BOLD));
    let search = if v.filter.is_empty() {
        Line::from(vec![label, Span::styled("type to filter", fg(t.text_dim))])
    } else {
        Line::from(vec![
            label,
            Span::styled(
                fit_chars(v.filter, w.saturating_sub(9)),
                fg(t.text_bright).add_modifier(Modifier::BOLD),
            ),
            Span::styled("▎", fg(t.accent)),
        ])
    };
    put_line(frame, at(y0 + 3), search);

    let list_y = y0 + 5;
    let name_w = longest.clamp(8, w.saturating_sub(2 + ROW_TAIL).max(8));
    let row_w = (2 + name_w + ROW_TAIL).min(w);
    let mut safe_sel = 0;
    if v.filtered.is_empty() {
        let line = if v.filter.is_empty() {
            Line::from(Span::styled(
                "  No models listed — type a model id, then Enter",
                fg(t.text_dim),
            ))
        } else {
            Line::from(vec![
                Span::styled("  Use custom model: ", fg(t.text_dim)),
                Span::styled(
                    fit_chars(v.filter, w.saturating_sub(22)),
                    fg(t.accent).add_modifier(Modifier::BOLD),
                ),
            ])
        };
        put_line(frame, at(list_y), line);
    } else {
        safe_sel = skip_divider(v.rows, sel.min(v.rows.len() - 1), false);

        let slots = v.rows.len() + usize::from(v.gap.is_some());
        let (top, shown) = list_window(slots, slot_of(safe_sel, v.gap), *scroll_top, list_h);
        *scroll_top = top;
        let end = top + shown;
        let models_in = |rows: &[Row]| rows.iter().filter(|r| matches!(r, Row::Model(_))).count();
        let hint = |text: String| Line::from(Span::styled(text, fg(t.text_dim)));
        let mut y = list_y;
        if top > 0 {
            let above = models_in(&v.rows[..rows_before(top, v.gap)]);
            put_line(frame, at(y), hint(format!("  ↑ {above} more above")));
            y += 1;
        }
        for slot in top..end {
            let line = match row_at(slot, v.gap) {
                None => Line::default(),
                Some(ri) => match v.rows[ri] {
                    Row::Divider if v.all_head => Line::from(Span::styled(
                        format!("  ─ all models {}", "─".repeat(row_w.saturating_sub(15))),
                        fg(t.text_faint),
                    )),
                    Row::Divider => Line::from(Span::styled(
                        format!("  {}", "─".repeat(row_w.saturating_sub(2))),
                        fg(t.text_faint),
                    )),
                    Row::Model(mi) => picker_row_line(v, v.filtered[mi], ri == safe_sel, name_w, w),
                    Row::Opt(oi) => match v.drop {
                        Some(d) => picker_option_line(v, d, oi, ri == safe_sel, name_w, w),
                        None => Line::default(),
                    },
                },
            };
            put_line(frame, at(y), line);
            y += 1;
        }
        let below = models_in(&v.rows[rows_before(end, v.gap)..]);
        if below > 0 {
            put_line(frame, at(y), hint(format!("  ↓ {below} more below")));
        }
    }

    if detail_h > 0
        && let Some(id) = v.highlighted(safe_sel)
    {
        let detail_y = list_y + list_h as u16 + 1;
        for (i, line) in picker_detail_lines(v, id, w)
            .into_iter()
            .take(detail_h - 1)
            .enumerate()
        {
            put_line(frame, at(detail_y + i as u16), line);
        }
    }

    put_line(frame, at(y0 + modal_h - 2), picker_footer(v, safe_sel, w));
}
