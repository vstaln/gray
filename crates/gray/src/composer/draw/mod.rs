use std::time::Duration;

use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::{MIN_VIEWPORT_H, PANEL_ROWS, Tui};
use crate::text_width::display_width;

mod widgets;

pub(crate) use widgets::{
    build_input_box, queued_preview_lines, ratchet_seam, shimmer_spans, status_dock_h,
    transcript_ends_blank,
};

/// Exact-fit viewport height for the given content, clamped to
/// `MIN_VIEWPORT_H..=max_h`.
pub(crate) fn desired_viewport_h(
    status_h: u16,
    queued_h: u16,
    live_h: u16,
    box_rows: u16,
    panel_h: u16,
    attach_h: u16,
    max_h: u16,
) -> u16 {
    (status_h + queued_h + live_h + box_rows + panel_h + attach_h + 1).clamp(MIN_VIEWPORT_H, max_h)
}

/// Latched viewport floor for the frame: the estimate above short-cuts
/// dock segments (`status_h` recomputed inside the frame after the seam
/// latch re-arms, live rows re-wrapped at the frame width), and any
/// measured overrun pushed the footer gauge past the viewport bottom —
/// where it was skipped entirely, so the gauge (and the `+1` pad band
/// below it) flickered while tokens streamed. Grow the viewport by the
/// same amount the estimate short-cuts, so the measured frame always
/// fits without moving the viewport's top edge (no input-box bounce).
pub(crate) fn latched_viewport_floor(
    status_h: u16,
    queued_h: u16,
    live_h: u16,
    box_rows: u16,
    attach_h: u16,
    max_h: u16,
) -> u16 {
    // The three frames the seam latch can produce mid-turn, given the
    // estimate short-cut `status_h` to 0: full dock (seam + status +
    // breath), the dock without seam, and no dock at all (box + footer).
    let with_dock = status_h + queued_h + live_h + box_rows + attach_h + 1;
    let no_status = queued_h + live_h + box_rows + attach_h + 1;
    // A short-cut live reserve: measured live rows push the frame past
    // the estimate's box + footer floor.
    let live_overrun = live_h + box_rows + attach_h + 1;
    with_dock.max(no_status).max(live_overrun).min(max_h)
}

/// Upper bound for the inline viewport.
///
/// The input box is content-sized and has to be able to grow past the idle
/// 14-row transcript: capping the viewport at `VIEWPORT_H` clipped a multi-line
/// paste to a couple of visible rows, however tall the terminal was. Bound by
/// the screen instead (leaving the shell prompt row), so the box grows until
/// the terminal is full -- `set_viewport_height` already clamps to the screen
/// and scrolls if a paste ever exceeds it.
pub(crate) fn viewport_cap(rows: u16) -> u16 {
    rows.saturating_sub(2).max(MIN_VIEWPORT_H)
}

/// The row the footer gauge paints on, clamped into the frame.
///
/// The gauge is the viewport's last row, so a one-row shortfall in the
/// pre-computed viewport estimate (`status_h` re-read inside the frame,
/// live rows re-wrapped at the frame width) used to push `footer_y` past
/// the bottom and skip the paint entirely — the footer flickered while
/// tokens streamed. Clamping paints it on the frame's last row instead:
/// one frame riding the screen bottom beats a vanished gauge. Pure for
/// testability (`Tui::new` needs a TTY).
pub(crate) fn footer_paint_row(footer_y: u16, area: Rect) -> Option<u16> {
    (area.height > 0).then(|| footer_y.min(area.y + area.height - 1))
}

/// Rows the band may spend on everything above the text area: the screen
/// minus the rows that must always survive (the status dock, the input box,
/// the attachment row, the context footer).
///
/// The text area and the context line are the two things that are always
/// on screen, so they are never trimmed: a busy turn (a wall of live tool
/// cards plus a widget and a queued follow-up) shrinks the rows above them
/// instead of pushing the prompt and the gauge off the bottom.
pub(crate) fn band_budget(screen_h: u16, reserved_rows: u16) -> u16 {
    screen_h.saturating_sub(reserved_rows)
}

/// Trim `counts` to `allowance` rows in total, in the order they are
/// given: the ask modal (it blocks the turn) first, then the user's queued
/// follow-up, then the plugin widget (decoration), and the live tool cards
/// last because they are the elastic block — their results land in the
/// scrollback anyway, so they absorb whatever is left. The pre-frame
/// estimate and the frame itself run the same order, so the band's height
/// matches what renders and the footer keeps the viewport's last row.
/// Pure for testability.
pub(crate) fn trim_to_allowance(counts: &mut [u16], allowance: u16) {
    let mut left = allowance;
    for count in counts.iter_mut() {
        let keep = left.min(*count);
        *count = keep;
        left = left.saturating_sub(keep);
    }
}

/// The bare-Enter resume ghost, shown only while the composer is idle.
///
/// `allow_empty_submit` is armed by the REPL loop right before it blocks on
/// input — i.e. *before* the turn that a bare Enter would continue is
/// submitted — so mid-turn it is stale: the resume it advertises is already
/// streaming, and the box kept painting "Please continue…" over the live
/// turn. Pure for testability (`Tui::new` needs a TTY).
pub(crate) fn continue_ghost(
    allow_empty_submit: bool,
    is_task_running: bool,
    text: &str,
) -> Option<&'static str> {
    (allow_empty_submit && !is_task_running && text.trim().is_empty())
        .then_some(crate::repl::CONTINUE_GHOST)
}

/// Whether the footer paints the reasoning-effort badge.
///
/// Provider-driven (opencode parity): no badge when the provider says this
/// model doesn't reason; unknown → show, as before. `snapshot` is the turn's
/// frozen answer (see `Tui::turn_show_effort`): while a turn streams, the
/// flag underneath is an async cache shared with background discovery
/// (`repl/mod.rs` spawns the provider `/models` and models.dev fetches), so
/// resolving it per frame lets the badge appear or vanish at any moment.
/// The badge is part of the right-anchored footer segment — a width change
/// walks the whole right segment across the bar mid-stream.
pub(crate) fn footer_badge_visible(model: &str, snapshot: Option<bool>) -> bool {
    snapshot
        .unwrap_or_else(|| crate::setup::context::model_supports_reasoning(model) != Some(false))
}

pub(crate) fn draw(tui: &mut Tui) -> anyhow::Result<()> {
    // Inside `Tui::atomic` the repaint is coalesced: the outermost batch
    // draws once, in the same synchronized update as its scrollback inserts.
    if tui.batch_depth > 0 {
        return Ok(());
    }
    frame(tui, true)
}

/// Gives the band its current height without painting it. Every scrollback
/// insert runs this first (see `Tui::insert_paragraph`), so rows the band
/// gives up (a committed live card, a dropped seam, a cleared status) are
/// vacated *before* the insert, which fills them. Shrunk after the insert,
/// the band strands those rows as blank margin between the transcript and
/// the dock, and the only defence was a hint estimating the shrink, which
/// drifted from what the band really drew. This keeps every margin at one
/// row by construction.
pub(crate) fn settle_band(tui: &mut Tui) -> anyhow::Result<()> {
    frame(tui, false)
}

fn frame(tui: &mut Tui, paint: bool) -> anyhow::Result<()> {
    if tui.modal_open {
        return Ok(());
    }
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let screen_size = ratatui::layout::Size::new(cols, rows);
    let w = cols as usize;

    let text = tui.textarea.text().to_string();
    let cursor = tui.textarea.cursor().min(text.len());
    // Ghost resume hint in the empty box while bare Enter would continue.
    let ghost = continue_ghost(tui.allow_empty_submit, tui.is_task_running, &text);
    let ibox = build_input_box(&text, cursor, w, ghost);
    let box_h = ibox.lines.len().max(1) as u16;
    // Attachments row.
    let attach_h: u16 = u16::from(!tui.attachments.is_empty());
    // Seam (only if scrollback didn't already end blank) + shimmer status
    // text + one bare breathing row below it. Latched: a per-frame seam
    // resized the viewport under the input box on every streamed chunk.
    tui.dock_seam = ratchet_seam(
        tui.dock_seam,
        tui.status.is_some(),
        !transcript_ends_blank(&tui.transcript),
    );
    let needs_seam = tui.dock_seam;
    let status_h: u16 = status_dock_h(tui.status.is_some(), needs_seam);
    // Row offset of the status text inside its dock: below the seam when
    // one was reserved, else the very top of the viewport.
    let seam_h: u16 = if status_h > 0 {
        u16::from(needs_seam)
    } else {
        0
    };

    // Exact-fit viewport with in-place resizing (codex parity):
    // Grow and shrink are applied directly to the terminal's viewport area without
    // recreating the terminal or re-probing cursor position via CPR, preventing
    // prompt doubling and cursor drift.
    let n = tui.queued_inputs.len();
    let queued_est: u16 = if n == 0 {
        0
    } else {
        1 + n.min(3) as u16 + u16::from(n > 3)
    };
    let panel_est: u16 = tui.matches.len().min(PANEL_ROWS) as u16;
    // Inline `host/ask` modal (sidecar plugin questions): measured rows.
    let ask_est: u16 = tui
        .ask_modal
        .as_ref()
        .map(|m| m.rows.len().min(12) as u16)
        .unwrap_or(0);
    let box_rows_est: u16 = box_h;
    // Live tool cards above the input box (pi pending cards): measured,
    // not estimated — the rows are already wrapped for `w`.
    let live_est: u16 = tui
        .live_tool_rows()
        .iter()
        .map(|l| {
            crate::composer::transcript::wrap_styled_line(l.clone(), w.saturating_sub(4).max(1))
                .len()
                .max(1) as u16
        })
        .sum::<u16>()
        .saturating_add(u16::from(tui.live_tool_overflow() > 0));
    let widget_budget = rows
        .saturating_sub(box_h + status_h + queued_est + live_est + panel_est + attach_h + 2)
        .min(12);
    let mut widget_rows = tui.plugin_widget.rows(widget_budget as usize);
    let max_viewport_h = viewport_cap(rows);

    // The text area and the context footer always keep their rows, so the
    // band above them is capped before the height is reserved, and the
    // frame trims the measured rows with the same allowance: what the band
    // reserved is what it renders, so the footer keeps the last row. The
    // cap is measured against the band cap (not the screen) because that is
    // all the viewport can ever be.
    let mut band_allowance = band_budget(max_viewport_h, status_h + box_h + attach_h + 1);
    // A cut card set still needs its "… +N more" row.
    let reserve_overflow_row = tui.live_tool_overflow() > 0 && band_allowance > 0;
    if reserve_overflow_row {
        band_allowance -= 1;
    }
    let mut reserved = [ask_est, queued_est, widget_rows.len() as u16, live_est];
    trim_to_allowance(&mut reserved, band_allowance);
    let (ask_est, queued_est, widget_h, live_est) =
        (reserved[0], reserved[1], reserved[2], reserved[3]);
    let desired = desired_viewport_h(
        status_h,
        queued_est,
        live_est + widget_h + ask_est,
        box_rows_est,
        panel_est,
        attach_h,
        max_viewport_h,
    );
    // Footer paint row (the viewport's last row). A mid-stream text wrap can
    // measure one row taller than this estimate (`status_h` re-read inside
    // the frame, live rows re-wrapped at the frame width); the shortfall
    // would push `footer_y` past the bottom and silently drop the gauge for
    // that frame — the footer flickered while tokens streamed. Grow the
    // viewport by the same amount the estimate short-cuts the dock, so the
    // measured frame fits without moving the top edge (no bounce).
    let desired = desired.max(latched_viewport_floor(
        status_h,
        queued_est,
        live_est + widget_h + ask_est,
        box_rows_est,
        attach_h,
        max_viewport_h,
    ));

    // frankentui lesson: synchronized-output bracketing (DEC2026) — one atomic
    // present per frame so the compositor never shows a torn frame.
    // Terminals without support ignore the sequence.
    tui.begin_sync();
    // Viewport geometry is a precondition for the frame: a swallowed failure
    // would commit a frame against stale geometry (audit 24.01). Close the
    // synchronized-update bracket before bailing out.
    if let Err(e) = tui.terminal.set_viewport_height(desired, screen_size) {
        tui.end_sync();
        return Err(e.into());
    }
    tui.viewport_h = desired;

    // The status dock's seam row (if any) sits on top of the band: tell the
    // terminal, so a scrollback commit that ends blank overwrites it instead
    // of stacking a second gap row (see `CustomTerminal::top_slack`).
    tui.terminal.set_top_slack(seam_h);
    if !paint {
        tui.end_sync();
        return Ok(());
    }

    // Hoisted for the draw closure (borrows `tui` immutably inside).
    // Live tool headers hoisted as owned rows — `live_tool_rows` borrows
    // all of `tui`, which would collide with `terminal.draw`'s mutable
    // borrow; wrapping happens inside at the frame width.
    let live_headers: Vec<Line<'static>> = tui.live_tool_rows();
    let live_overflow = tui.live_tool_overflow();
    let ask_modal: Option<super::AskModal> = tui.ask_modal.clone();
    let compaction_elapsed = tui.compaction_elapsed();
    let turn_started = tui.turn_started;
    let is_task_running = tui.is_task_running;
    // Live output total for the top pill (turn-level completed outputs +
    // streamed estimate, ticks per chunk, exact on every report) — empty
    // only before the first streamed byte. Converges to the `Thought`
    // line; the footer below ticks the context base with the same delta.
    // Live TPS numerator is turn-level (completed StepUsage outputs +
    // streamed estimate) so it converges to the final Thought-line rate.
    let pill_turn_output = tui.turn_output_accum;
    let pill_streamed = tui.streamed_bytes;
    let pill_tok_suffix = super::live_pill_suffix(pill_turn_output, pill_streamed);
    // Warmth countdown for the prompt cache: how long the last request's
    // cache stays warm before an idle gap re-bills the whole prompt —
    // ◷ 4m, fading in the last minute. Hidden when the cache is cold or
    // the provider never reports cache activity, so the footer never
    // claims warmth it cannot see. One stamp per frame.
    let cache_timer = tui.cache_remaining().map(|left| {
        (
            format!("\u{25f7} {}", crate::cache::format_remaining(left)),
            if left < Duration::from_secs(60) {
                crate::theme::theme().text_dim
            } else {
                crate::theme::theme().cache_hit
            },
        )
    });

    let res = tui.terminal.draw(|frame| {
        let area = frame.area();
        let w = area.width as usize;

        let status_y = area.y;
        // Queued preview sits between status and input.
        let mut queued_preview: Vec<Line<'static>> = queued_preview_lines(&tui.queued_inputs, w);
        // Live tool cards (pi pending cards): wrapped at the frame width
        // like every other viewport row; `live_h` reserves their space.
        let mut live_rows: Vec<Line<'static>> = live_headers
            .into_iter()
            .flat_map(|l| {
                crate::composer::transcript::wrap_styled_line(l, w.saturating_sub(4).max(1))
            })
            .collect();
        // Ask modal rows (pre-wrapped at frame width, capped like live rows).
        let mut ask_rows: Vec<Line<'static>> = ask_modal
            .as_ref()
            .map(|m| {
                m.rows
                    .iter()
                    .take(12)
                    .enumerate()
                    .flat_map(|(ri, (glyph, label, desc))| {
                        let raw = if ri == 0 {
                            format!("❓ {label} — {desc}")
                        } else {
                            let marker = if ri == m.cursor { "❯" } else { " " };
                            format!("  {marker} {glyph} {label} — {desc}")
                        };
                        crate::composer::transcript::wrap_styled_line(
                            Line::from(vec![Span::styled(
                                raw,
                                if ri == m.cursor {
                                    Style::default()
                                        .fg(crate::theme::theme().accent)
                                        .add_modifier(Modifier::BOLD)
                                } else if ri == 0 {
                                    Style::default()
                                        .fg(Color::White)
                                        .add_modifier(Modifier::BOLD)
                                } else {
                                    Style::default().fg(crate::theme::theme().text_muted)
                                },
                            )]),
                            w.saturating_sub(4).max(1),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Same trim as the height estimate, same order, same allowance.
        let mut rows_h = [
            ask_rows.len() as u16,
            queued_preview.len() as u16,
            widget_rows.len() as u16,
            live_rows.len() as u16,
        ];
        trim_to_allowance(&mut rows_h, band_allowance);
        ask_rows.truncate(rows_h[0] as usize);
        queued_preview.truncate(rows_h[1] as usize);
        widget_rows.truncate(rows_h[2] as usize);
        live_rows.truncate(rows_h[3] as usize);
        let show_live_overflow = reserve_overflow_row;
        let live_h = rows_h[3] + u16::from(show_live_overflow);
        let queued_h = rows_h[1];
        let ask_h = rows_h[0];
        let widget_h = rows_h[2];
        // Space left for the completion panel once the fixed rows
        // (status, queued, input, attachments, footer) are placed.
        let avail = area
            .height
            .saturating_sub(status_h + queued_h + live_h + widget_h + box_h + attach_h + 1);
        let need = PANEL_ROWS as u16;
        let panel_cap = need.min(avail).max((PANEL_ROWS as u16).min(avail));
        let visible_count = if tui.matches.is_empty() {
            0
        } else {
            tui.matches.len().min(panel_cap as usize)
        };
        let panel_h = visible_count as u16;
        let box_rows = box_h;
        let ask_y = status_y + status_h + queued_h + live_h;
        let widget_y = ask_y + ask_h;
        let box_y = widget_y + widget_h;
        let panel_y = box_y + box_rows;
        let attach_y = panel_y + panel_h;
        let footer_y = attach_y + attach_h;

        if let Some((started, label)) = &tui.status {
            let label_text = format!(" ⬡ {label}\u{2026}");
            // Sweep phase rides the turn clock (or the compaction clock),
            // never the status stamp: `set_status` re-stamps it on every
            // label change, and `Preparing tool: …` changes per streamed
            // argument token — the highlight band snapped back to the
            // start each time (flicker on every tool call).
            let elapsed = match compaction_elapsed {
                Some(d) => d,
                None => super::pill_elapsed(turn_started, *started, is_task_running),
            };
            let mut spans = shimmer_spans(&label_text, elapsed);
            // Turn-anchored clock (tool re-stamps never restart it) plus
            // the live output counter: turn-level outputs plus the streamed
            // per-chunk estimate, exact on every report. Live TPS rides
            // between them (turn-level numerator, same denominator), so the
            // rate is always visible — not just on the end-of-turn line.
            // Exact turn bills stay on the Thought line and `/usage`.
            // Codex parity: while compacting, the pill runs on the separate
            // compaction clock — the turn clock is preserved underneath and
            // restored after (`compaction_status_survives_follow_up`).
            // Live TPS needs the turn clock too, so it hides while
            // compacting (that clock isn't turn time).
            let tps_suffix = match compaction_elapsed {
                Some(_) => String::new(),
                None => super::live_tps_suffix(pill_turn_output, pill_streamed, tui.turn_stream_ms),
            };
            let elapsed_str = format!("{:.1}s", elapsed.as_secs_f64());
            let suffix = format!(" {elapsed_str}{pill_tok_suffix}{tps_suffix} (esc to interrupt)");
            spans.push(Span::styled(
                suffix,
                Style::default().fg(crate::theme::theme().tool_dim),
            ));
            frame.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect::new(area.x, status_y + seam_h, area.width, 1),
            );
        }

        for (i, line) in queued_preview.iter().enumerate() {
            let y = status_y + status_h + i as u16;
            if y < area.y || y >= area.y + area.height {
                continue;
            }
            frame.render_widget(
                chrome_row(line.clone()),
                Rect::new(area.x, y, area.width, 1),
            );
        }
        let live_y = status_y + status_h + queued_h;
        for (i, line) in live_rows.iter().enumerate() {
            let y = live_y + i as u16;
            if y < area.y || y >= area.y + area.height {
                continue;
            }
            frame.render_widget(
                live_tool_row(line.clone()),
                Rect::new(area.x, y, area.width, 1),
            );
        }
        if show_live_overflow {
            let y = live_y + live_rows.len() as u16;
            if y >= area.y && y < area.y + area.height {
                frame.render_widget(
                    Paragraph::new(Line::from(vec![Span::styled(
                        format!("    … +{live_overflow} more"),
                        Style::default()
                            .fg(crate::theme::theme().text_muted)
                            .add_modifier(Modifier::DIM)
                            .add_modifier(Modifier::ITALIC),
                    )])),
                    Rect::new(area.x, y, area.width, 1),
                );
            }
        }
        for (i, line) in ask_rows.iter().enumerate() {
            let y = ask_y + i as u16;
            if y < area.y || y >= area.y + area.height {
                continue;
            }
            frame.render_widget(
                chrome_row(line.clone()),
                Rect::new(area.x, y, area.width, 1),
            );
        }
        for (i, line) in widget_rows.iter().enumerate() {
            let y = widget_y + i as u16;
            if y < area.bottom() {
                frame.render_widget(
                    chrome_row(line.clone()),
                    Rect::new(area.x, y, area.width, 1),
                );
            }
        }
        let rendered_box_h = box_h.min(area.bottom().saturating_sub(box_y));
        if rendered_box_h > 0 {
            let box_block =
                Block::default().style(Style::default().bg(crate::theme::theme().surface_bg));
            frame.render_widget(
                Paragraph::new(ibox.lines.clone()).block(box_block),
                Rect::new(area.x, box_y, area.width, rendered_box_h),
            );
        }

        if visible_count > 0 {
            let start = tui
                .sel
                .saturating_sub(visible_count.saturating_sub(1))
                .min(tui.sel);
            // Scroll indicator: the window clips when the list is longer
            // than the cap — arrows at the clipped edge(s).
            let total = tui.matches.len();
            let end = (start + visible_count).min(total);
            let hidden_above = start;
            let hidden_below = total.saturating_sub(end);
            for (i, (name, desc)) in tui
                .matches
                .iter()
                .enumerate()
                .skip(start)
                .take(visible_count)
            {
                let y = (i - start) as u16;
                let item_y = panel_y + y;
                if item_y < area.y || item_y >= area.y + area.height {
                    continue;
                }
                let is_sel = i == tui.sel;
                let cmd_str = format!(" /{name} ");
                let desc_str = format!(" {desc} ");
                let used_len = display_width(&cmd_str) + display_width(&desc_str);
                // ponytail: single-char edge arrows, no extra row or layout.
                let marker = match (
                    i == start && hidden_above > 0,
                    i + 1 == end && hidden_below > 0,
                ) {
                    (true, true) => "↕",
                    (true, false) => "↑",
                    (false, true) => "↓",
                    (false, false) => "",
                };
                let pad_len = w.saturating_sub(used_len + display_width(marker));
                let line_bg = if is_sel {
                    crate::theme::theme().accent
                } else {
                    crate::theme::theme().raised_bg
                };
                let marker_style = if is_sel {
                    Style::default()
                        .fg(crate::theme::theme().on_selection)
                        .bg(line_bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(Color::White)
                        .bg(line_bg)
                        .add_modifier(Modifier::BOLD)
                };
                let line = if is_sel {
                    Line::from(vec![
                        Span::styled(
                            cmd_str,
                            Style::default()
                                .fg(crate::theme::theme().on_selection)
                                .bg(line_bg)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            desc_str,
                            Style::default()
                                .fg(crate::theme::theme().text_faint)
                                .bg(line_bg),
                        ),
                        Span::styled(" ".repeat(pad_len), Style::default().bg(line_bg)),
                        Span::styled(marker.to_string(), marker_style),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled(
                            cmd_str,
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD)
                                .bg(line_bg),
                        ),
                        Span::styled(
                            desc_str,
                            Style::default()
                                .fg(crate::theme::theme().text_muted)
                                .bg(line_bg),
                        ),
                        Span::styled(" ".repeat(pad_len), Style::default().bg(line_bg)),
                        Span::styled(marker.to_string(), marker_style),
                    ])
                };
                frame.render_widget(
                    Paragraph::new(line)
                        .block(Block::default().style(Style::default().bg(line_bg))),
                    Rect::new(area.x, item_y, area.width, 1),
                );
            }
        }

        if attach_h > 0 && attach_y < area.y + area.height {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (_ph, p) in &tui.attachments {
                let fname = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("clipboard")
                    .to_string();
                // blue File badge like opencode
                spans.push(Span::styled(
                    " File ",
                    Style::default()
                        .fg(Color::White)
                        .bg(crate::theme::theme().info)
                        .add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    fname,
                    Style::default()
                        .fg(crate::theme::theme().text_soft)
                        .bg(crate::theme::theme().input_bg),
                ));
                spans.push(Span::raw("  "));
            }
            frame.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect::new(area.x, attach_y, area.width, 1),
            );
        }

        let (_, max_label) = crate::setup::model_context_info(&tui.model_name);
        // Live gauge (bottom): latest report plus the shared streamed
        // estimate, so the `223.8k/300k` footer ticks per chunk with the
        // same delta as the top pill (different base: context vs output;
        // exact on every report). Hit rate stays report-only: streamed
        // output doesn't change the cache ratio until the next report lands.
        let (used_tokens, hit_rate) = if let Some(u) = tui.latest_usage.or(tui.cumulative_usage) {
            (
                super::live_context_total(Some(u), tui.streamed_bytes),
                u.cache_hit_rate() * 100.0,
            )
        } else if tui.streamed_bytes > 0 {
            // No report yet (or gauge cleared): the bare streamed estimate is
            // the only signal — same rule as the pill's empty-before-first-byte.
            (super::live_context_total(None, tui.streamed_bytes), 0.0)
        } else {
            (0, 0.0)
        };
        let ctx_display = format!(
            "{}/{}",
            crate::setup::format_context_length(used_tokens),
            max_label
        );
        let cache_display = format!("{hit_rate:.1}% cache");

        let model_display = crate::setup::friendly_model_name(&tui.model_name);
        // Unknown/`None` (between turns) resolves the flag live, so the
        // provider's own answer still lands once discovery finishes.
        let show_effort = footer_badge_visible(&tui.model_name, tui.turn_show_effort);
        // `off` already implies hidden — don't render "off · hidden".
        // Non-reasoning models (show_effort false) render no badge; the
        // separator is omitted with it so the footer never trails " · ".
        let effort_display = if !show_effort {
            String::new()
        } else if tui.thinking_effort == "off" {
            "off".to_string()
        } else if tui.hide_thinking {
            if tui.thinking_effort.is_empty() {
                "hidden".to_string()
            } else {
                format!("{} · hidden", tui.thinking_effort)
            }
        } else {
            tui.thinking_effort.clone()
        };
        let right_parts = if model_display.is_empty() {
            if effort_display.is_empty() {
                Vec::new()
            } else {
                vec![Span::styled(
                    effort_display.clone(),
                    Style::default().fg(crate::theme::theme().tool_dim),
                )]
            }
        } else if effort_display.is_empty() {
            vec![Span::styled(
                model_display.clone(),
                Style::default().fg(crate::theme::theme().text_muted),
            )]
        } else {
            vec![
                Span::styled(
                    model_display.clone(),
                    Style::default().fg(crate::theme::theme().text_muted),
                ),
                Span::styled(
                    " \u{b7} ",
                    Style::default().fg(crate::theme::theme().text_faint),
                ),
                Span::styled(
                    effort_display.clone(),
                    Style::default().fg(crate::theme::theme().tool_dim),
                ),
            ]
        };
        let right_len = if model_display.is_empty() {
            display_width(&effort_display)
        } else if effort_display.is_empty() {
            display_width(&model_display)
        } else {
            display_width(&model_display) + 3 + display_width(&effort_display)
        };
        let timer_len = cache_timer
            .as_ref()
            .map(|(text, _)| 3 + display_width(text))
            .unwrap_or(0);
        let left_len =
            1 + display_width(&ctx_display) + 3 + display_width(&cache_display) + timer_len;
        let pad_len = w.saturating_sub(left_len + right_len);

        let cache_color = if hit_rate > 0.0 {
            crate::theme::theme().cache_hit
        } else {
            crate::theme::theme().text_faint
        };

        let mut footer_spans = vec![
            Span::raw(" "),
            Span::styled(
                ctx_display,
                Style::default().fg(crate::theme::theme().tool_dim),
            ),
            Span::styled(
                " \u{b7} ",
                Style::default().fg(crate::theme::theme().text_faint),
            ),
            Span::styled(cache_display, Style::default().fg(cache_color)),
        ];
        if let Some((timer_text, timer_color)) = cache_timer {
            footer_spans.push(Span::styled(
                " \u{b7} ",
                Style::default().fg(crate::theme::theme().text_faint),
            ));
            footer_spans.push(Span::styled(timer_text, Style::default().fg(timer_color)));
        }
        footer_spans.push(Span::raw(" ".repeat(pad_len)));
        footer_spans.extend(right_parts);
        // Transparent footer: text only, no full-bleed band. Painted on the
        // row `footer_paint_row` clamps into the frame — never skipped.
        if let Some(footer_y) = footer_paint_row(footer_y, area) {
            frame.render_widget(
                Paragraph::new(Line::from(footer_spans)),
                Rect::new(area.x, footer_y, area.width, 1),
            );
        }

        let used_bottom = footer_y + 1;
        if used_bottom < area.y + area.height {
            frame.render_widget(
                ratatui::widgets::Clear,
                Rect::new(
                    area.x,
                    used_bottom,
                    area.width,
                    (area.y + area.height) - used_bottom,
                ),
            );
        }

        // The caret lives in the input box on every frame. Follow-up input
        // stays editable mid-turn (typed into `queued_inputs`), so gating on
        // idle hides the caret exactly while the user is typing blind. Modals
        // own the screen and return before this point, so no other consumer
        // competes for the cursor.
        let cur_x = (area.x + 3 + ibox.cur_col as u16).min(area.x + area.width.saturating_sub(1));
        let cur_y = (box_y + ibox.cur_row as u16).min(area.y + area.height.saturating_sub(1));
        frame.set_cursor_position(Position::new(cur_x, cur_y));
    });
    let background_result = if res.is_ok() {
        if let Some(bg) = &mut tui.background {
            bg.draw(&mut std::io::stdout().lock(), cols, rows)
        } else {
            Ok(())
        }
    } else {
        Ok(())
    };
    tui.end_sync();
    res?;
    background_result?;
    Ok(())
}

#[path = "mod_tests.rs"]
#[cfg(test)]
mod tests;

/// Shared live-card background and inset, including wrapped continuation rows.
fn live_tool_row(line: Line<'static>) -> Paragraph<'static> {
    Paragraph::new(line).block(
        Block::default()
            .padding(ratatui::widgets::Padding::left(1))
            .style(Style::default().bg(crate::theme::theme().surface_bg)),
    )
}

/// A composer-band row: plugin widget, queued follow-up and ask-modal rows
/// sit between the live cards and the input box but were painted
/// transparent, so on a terminal whose default background differs from the
/// theme they showed as a foreign stripe straight through the middle of the
/// composer (a streaming turn's tool call then walked the whole band).
/// Paint them like the rest of the band.
fn chrome_row(line: Line<'static>) -> Paragraph<'static> {
    Paragraph::new(line)
        .block(Block::default().style(Style::default().bg(crate::theme::theme().surface_bg)))
}
