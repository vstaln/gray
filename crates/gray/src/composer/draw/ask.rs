//! The inline question prompt (`host/ask`): tool approvals from
//! gray-permissions, `request_user_input` questions, MCP consent.
//!
//! Borderless like the shared REPL picker (`repl::picker`): a bright bold
//! title, the subject shown whole behind an accent bar, dim context, then the
//! options with a `›` marker and bold label on the highlighted row.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::composer::AskModal;
use crate::composer::transcript::wrap_styled_line;
use crate::text_width::display_width;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Title,
    Subject,
    Detail,
    Other,
}

/// The prompt's rows wrapped to `width`, at most `max` of them. When the
/// prompt is taller than `max`, the middle of the subject (a long command)
/// folds into a `… +N lines` row first so the options and key hint stay
/// visible; only then is the bottom cut.
pub(crate) fn ask_lines(m: &AskModal, width: usize, max: usize) -> Vec<Line<'static>> {
    let mut rows = tagged_lines(m, width);
    if rows.len() > max {
        fold_subject(&mut rows, max);
    }
    rows.truncate(max);
    rows.into_iter().map(|(_, l)| l).collect()
}

fn tagged_lines(m: &AskModal, width: usize) -> Vec<(Part, Line<'static>)> {
    let th = crate::theme::theme();
    let width = width.max(16);
    let label_w = m
        .rows
        .iter()
        .filter(|(k, ..)| is_option(k))
        .map(|(k, label, _)| display_width(k) + 1 + display_width(label))
        .max()
        .unwrap_or(0);
    let mut out = Vec::new();
    for (ri, (kind, label, text)) in m.rows.iter().enumerate() {
        match kind.as_str() {
            "?" => {
                let style = Style::default()
                    .fg(th.text_bright)
                    .add_modifier(Modifier::BOLD);
                for l in hang(vec![Span::raw(" ")], label, style, width) {
                    out.push((Part::Title, l));
                }
            }
            ">" => {
                let bar = vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{} ", crate::tui::glyph("\u{258e}", "|")),
                        Style::default().fg(th.accent),
                    ),
                ];
                let style = Style::default()
                    .fg(th.text_bright)
                    .add_modifier(Modifier::BOLD);
                for l in hang(bar, text, style, width) {
                    out.push((Part::Subject, l));
                }
            }
            "\u{2502}" => {
                let style = Style::default().fg(th.text_dim);
                for l in hang(vec![Span::raw("    ")], text, style, width) {
                    out.push((Part::Detail, l));
                }
            }
            " " => out.push((Part::Other, Line::default())),
            "\u{b7}" => out.push((Part::Other, hint_line(text, width))),
            _ => {
                for l in option_lines(kind, label, text, ri == m.cursor, label_w, width) {
                    out.push((Part::Other, l));
                }
            }
        }
    }
    out
}

fn is_option(kind: &str) -> bool {
    !matches!(kind, "?" | ">" | "\u{2502}" | " " | "\u{b7}")
}

/// `prefix` then `text` wrapped to the room left; continuation rows indent
/// under the text, so a wrapped command reads as one block.
fn hang(prefix: Vec<Span<'static>>, text: &str, style: Style, width: usize) -> Vec<Line<'static>> {
    let pw: usize = prefix.iter().map(|s| display_width(&s.content)).sum();
    let room = width.saturating_sub(pw + 1).max(8);
    let cont: Vec<Span<'static>> = prefix
        .iter()
        .map(|s| {
            if s.content.trim().is_empty() {
                s.clone()
            } else {
                // Keep bars, blank out markers and numbers.
                match s.content.trim() {
                    "\u{258e}" | "|" => s.clone(),
                    _ => Span::raw(" ".repeat(display_width(&s.content))),
                }
            }
        })
        .collect();
    let body = Line::from(Span::styled(text.to_string(), style));
    wrap_styled_line(body, room)
        .into_iter()
        .enumerate()
        .map(|(i, row)| {
            let mut spans = if i == 0 { prefix.clone() } else { cont.clone() };
            spans.extend(row.spans);
            Line::from(spans)
        })
        .collect()
}

fn option_lines(
    kind: &str,
    label: &str,
    text: &str,
    here: bool,
    label_w: usize,
    width: usize,
) -> Vec<Line<'static>> {
    let th = crate::theme::theme();
    let marker = if here {
        Span::styled(
            format!("  {} ", crate::tui::glyph("\u{203a}", ">")),
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::raw("    ")
    };
    let (key_style, label_style, text_style) = if here {
        (
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
            Style::default()
                .fg(th.text_bright)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(th.text_muted),
        )
    } else if kind == "~" {
        let dim = Style::default().fg(th.text_dim);
        (dim, dim, dim)
    } else {
        (
            Style::default().fg(th.text_dim),
            Style::default().fg(th.text_muted),
            Style::default().fg(th.text_dim),
        )
    };
    let mut prefix = vec![marker];
    let shown_key = match kind {
        "" | "~" | "*" => String::new(),
        k => format!("{k} "),
    };
    let key_style = if kind == "[x]" {
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
    } else {
        key_style
    };
    if !shown_key.is_empty() {
        prefix.push(Span::styled(shown_key.clone(), key_style));
    }
    if kind == "*" {
        // A free-form note being typed: "Other: the note█".
        prefix.push(Span::styled(format!("{label}: "), label_style));
        let note = format!("{text}\u{2588}");
        return hang(prefix, &note, Style::default().fg(th.text_body), width);
    }
    if text.is_empty() {
        return hang(prefix, label, label_style, width);
    }
    let used = display_width(&shown_key) + display_width(label);
    let pad = label_w.saturating_sub(used) + 3;
    prefix.push(Span::styled(label.to_string(), label_style));
    prefix.push(Span::raw(" ".repeat(pad)));
    let pw: usize = prefix.iter().map(|s| display_width(&s.content)).sum();
    if width.saturating_sub(pw) < 16 {
        // Too narrow to align: description on its own indented row.
        prefix.truncate(prefix.len() - 1);
        let mut lines = vec![Line::from(prefix)];
        lines.extend(hang(vec![Span::raw("      ")], text, text_style, width));
        return lines;
    }
    hang(prefix, text, text_style, width)
}

/// "↑↓ select · 1-3 choose · enter confirm": keys a step brighter than
/// the action words.
fn hint_line(hint: &str, width: usize) -> Line<'static> {
    let th = crate::theme::theme();
    let key = Style::default().fg(th.text_muted);
    let word = Style::default().fg(th.text_dim);
    let mut spans = vec![Span::raw("    ")];
    for (i, seg) in hint.split(" \u{b7} ").enumerate() {
        if i > 0 {
            spans.push(Span::styled(" \u{b7} ", word));
        }
        match seg.rsplit_once(' ') {
            Some((k, w)) => {
                spans.push(Span::styled(k.to_string(), key));
                spans.push(Span::styled(format!(" {w}"), word));
            }
            None => spans.push(Span::styled(seg.to_string(), word)),
        }
    }
    let line = Line::from(spans);
    wrap_styled_line(line.clone(), width.saturating_sub(1).max(8))
        .into_iter()
        .next()
        .unwrap_or(line)
}

/// Fold the middle of the subject block into one `… +N lines` row until the
/// prompt fits in `max` rows (or the subject is down to head + tail).
fn fold_subject(rows: &mut Vec<(Part, Line<'static>)>, max: usize) {
    let Some(start) = rows.iter().position(|(p, _)| *p == Part::Subject) else {
        return;
    };
    let end = rows[start..]
        .iter()
        .position(|(p, _)| *p != Part::Subject)
        .map_or(rows.len(), |n| start + n);
    let len = end - start;
    // Keep at least 2 head rows and 1 tail row around the fold marker.
    let foldable = len.saturating_sub(3);
    let excess = rows.len() + 1 - max.min(rows.len());
    if foldable < 2 {
        return;
    }
    let cut = excess.min(foldable);
    let head = start + 2 + (foldable - cut) / 2;
    let th = crate::theme::theme();
    let marker = Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{} ", crate::tui::glyph("\u{258e}", "|")),
            Style::default().fg(th.accent),
        ),
        Span::styled(
            format!("\u{2026} +{cut} lines"),
            Style::default()
                .fg(th.text_dim)
                .add_modifier(Modifier::ITALIC),
        ),
    ]);
    rows.splice(head..head + cut, [(Part::Subject, marker)]);
}

#[cfg(test)]
#[path = "ask_tests.rs"]
mod tests;
