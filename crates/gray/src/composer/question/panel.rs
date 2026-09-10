//! Question panel rendering for the inline viewport (split from `question`).

use super::*;
use crate::text_width::{display_width, fit_char_count};

/// Builds the panel lines for the inline viewport (draw side). Rows are
/// capped at `max_rows`; the option window scrolls around the selection.
pub(crate) fn panel_lines(
    q: &QuestionSession,
    draft: &str,
    w: usize,
    max_rows: usize,
) -> Vec<Line<'static>> {
    let bg_style = Style::default().bg(Color::Rgb(22, 22, 22));
    let mut lines: Vec<Line<'static>> = Vec::new();
    // top margin — like 4f8cc65 [WORKING WORKING FINAL ULTRA MEGA SUPREME...] padded card box
    lines.push(Line::from("").style(bg_style));
    if q.confirm_unanswered.is_some() {
        let sel = q.confirm_unanswered.unwrap_or(0);
        lines.push(Line::from(Span::styled(
            format!(" {UNANSWERED_CONFIRM_TITLE}"),
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        )));
        let n = q.unanswered_count();
        let rows = [
            (
                "Submit anyway",
                format!(
                    "Submit with {n} unanswered question{}.",
                    if n == 1 { "" } else { "s" }
                ),
            ),
            (
                "Go back",
                "Return to the first unanswered question.".to_string(),
            ),
        ];
        for (i, (label, desc)) in rows.iter().enumerate() {
            let prefix = if i == sel { icon("arrow") } else { " " };
            lines.extend(option_rows(prefix, i + 1, label, Some(desc), i == sel, w));
        }
        lines.push(Line::from("").style(bg_style));
        return lines;
    }

    let mut countdown = String::new();
    if let Some(text) = &q.last_countdown {
        countdown = format!(" · {text}");
    }
    lines.push(Line::from(Span::styled(
        format!(" {}{countdown}", q.progress_prefix()),
        Style::default().fg(DIM),
    )));

    let raw_q = &q.current_question().question;
    let mut q_lines_vec: Vec<Line<'static>> = Vec::new();
    let q_parts: Vec<&str> = raw_q.split('\n').collect();
    if q_parts.len() <= 1 {
        let wrapped = wrap_plain(raw_q, w.saturating_sub(4).max(10));
        for l in wrapped {
            q_lines_vec.push(Line::from(Span::styled(
                format!(" {l}"),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            )));
        }
    } else {
        // First line is question title (e.g. "Allow bash?")
        let title_wrapped = wrap_plain(q_parts[0], w.saturating_sub(4).max(10));
        for l in title_wrapped {
            q_lines_vec.push(Line::from(Span::styled(
                format!(" {l}"),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            )));
        }
        // Remaining lines are command / details preview with side padding (indented by 2 spaces)
        for part in &q_parts[1..] {
            if part.is_empty() {
                q_lines_vec.push(Line::from(""));
            } else {
                let cmd_wrapped = wrap_plain(part, w.saturating_sub(6).max(10));
                for l in cmd_wrapped {
                    q_lines_vec.push(Line::from(Span::styled(
                        format!("  {l}"),
                        Style::default().fg(Color::Rgb(210, 210, 210)),
                    )));
                }
            }
        }
    }
    let q_lines = q_lines_vec.len();
    lines.extend(q_lines_vec);

    // Budget: top(1) + progress(1) + question + notes(0/1) + tips(1) +
    // bottom margin(1); rest goes to options.
    let overhead = 4 + q_lines + usize::from(q.notes_editor_visible());
    let budget = max_rows.saturating_sub(overhead);
    let len = q.options_len();
    // Cursor position vs committed pick are different things: the cursor row
    // always shows the arrow and gets accent styling from the start, so the
    // preselected first option reads as highlighted with zero moves — a
    // highlight is the cursor, never a submitted answer (preselect stays
    // unanswered until Enter, Space, a digit, or notes confirm it).
    let picked = q.answers[q.current_idx].selected_idx.is_some();
    let cursor = q.answers[q.current_idx].selected_idx.unwrap_or(0);
    let visible = budget.min(len).max(1);
    let start = cursor.saturating_sub(visible.saturating_sub(1)).min(cursor);
    for i in start..len.min(start + visible) {
        let is_cursor = i == cursor;
        let prefix = if is_cursor { icon("arrow") } else { " " };
        let label = q
            .option_label_for_index(q.current_idx, i)
            .unwrap_or_default();
        let desc = if i < q.current_question().options.len() {
            Some(q.current_question().options[i].description.clone())
        } else {
            Some(OTHER_OPTION_DESCRIPTION.to_string())
        };
        for row in option_rows(
            prefix,
            i + 1,
            &label,
            desc.as_deref(),
            is_cursor && picked,
            w,
        ) {
            lines.push(row);
        }
    }

    // Editable notes row (Tab): the composer box is hidden while a question
    // owns the viewport, so without this row typed notes are invisible.
    if q.notes_editor_visible() {
        lines.push(notes_row(draft));
    }
    lines.push(tips_line(q));
    // bottom margin mirrors the top one — without it the footer jams
    // against the tips line.
    lines.push(Line::from("").style(bg_style));
    lines
}

/// Editable notes input row: mirrors the composer textarea draft, which is
/// where keystrokes land while the question owns the viewport.
fn notes_row(draft: &str) -> Line<'static> {
    let flat: String = draft.replace('\n', " ");
    if flat.trim().is_empty() {
        Line::from(vec![
            Span::styled("› ".to_string(), Style::default().fg(DIM)),
            Span::styled(
                "add notes…".to_string(),
                Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                "› ".to_string(),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(flat, Style::default().fg(TEXT)),
        ])
    }
}

/// One option as wrapped rows: the head row keeps the picked styling, long
/// descriptions wrap onto dim continuation rows instead of clipping off-screen.
pub(crate) fn option_rows(
    prefix: &str,
    num: usize,
    label: &str,
    desc: Option<&str>,
    selected: bool,
    w: usize,
) -> Vec<Line<'static>> {
    let accent = Style::default()
        .fg(if selected { ACCENT } else { DIM })
        .add_modifier(Modifier::BOLD);
    let label_style = Style::default()
        .fg(if selected { ACCENT } else { TEXT })
        .add_modifier(Modifier::BOLD);
    let dim_style = Style::default().fg(DIM);
    let head = format!(" {prefix} {num}. {label}");
    let Some(d) = desc.filter(|d| !d.is_empty()) else {
        return vec![Line::from(vec![
            Span::styled(format!(" {prefix} {num}. "), accent),
            Span::styled(label.to_string(), label_style),
        ])];
    };
    // wrap_plain reserves 4 for padding; desc starts after "head — ".
    let desc_w = w.saturating_sub(display_width(&head) + 3 + 4).max(10);
    let chunks = wrap_plain(d, desc_w + 4);
    let mut rows = vec![Line::from(vec![
        Span::styled(format!(" {prefix} {num}. "), accent),
        Span::styled(label.to_string(), label_style),
        Span::styled(
            format!(" — {}", chunks.first().cloned().unwrap_or_default()),
            dim_style,
        ),
    ])];
    let indent = " ".repeat(display_width(&head) + 3);
    for c in chunks.iter().skip(1) {
        rows.push(Line::from(Span::styled(format!("{indent}{c}"), dim_style)));
    }
    rows
}

fn tips_line(q: &QuestionSession) -> Line<'static> {
    let notes_visible =
        q.answers[q.current_idx].notes_visible || !q.answers[q.current_idx].draft.trim().is_empty();
    let mut tips: Vec<(String, bool)> = Vec::new();
    let sel = q.answers[q.current_idx].selected_idx.is_some();
    let is_approval = q.questions[q.current_idx].id == "tool-approval";
    if is_approval {
        tips.push(("enter confirm".into(), true));
        tips.push(("esc cancel".into(), false));
    } else {
        if sel && !notes_visible {
            tips.push(("tab notes".into(), true));
        } else if sel && notes_visible {
            tips.push(("esc clear notes".into(), false));
        }
        let is_last = q.current_idx + 1 >= q.questions.len();
        let submit = if q.questions.len() == 1 || is_last {
            "enter submit"
        } else {
            "enter next"
        };
        tips.push((submit.into(), true));
        if q.questions.len() > 1 {
            tips.push(("backspace skip".into(), false));
            tips.push(("←/→ navigate".into(), false));
        }
    }
    let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
    for (i, (text, highlight)) in tips.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(
                TIP_SEPARATOR,
                Style::default().fg(Color::Rgb(80, 80, 80)),
            ));
        }
        spans.push(Span::styled(
            text.clone(),
            Style::default().fg(if *highlight { ACCENT } else { DIM }),
        ));
    }
    Line::from(spans)
}

/// Word-aware wrap matching the transcript's `word_flush_cut`: break at the
/// last space in the window, hard-cut only a single overlong word.
pub(crate) fn wrap_plain(s: &str, w: usize) -> Vec<String> {
    let content_w = w.saturating_sub(4).max(1);
    s.split('\n')
        .flat_map(|line| {
            if line.is_empty() {
                return vec![String::new()];
            }
            let chars: Vec<char> = line.chars().collect();
            let mut rows = Vec::new();
            let mut start = 0usize;
            while start < chars.len() {
                let mut end = start + fit_char_count(&chars[start..], content_w);
                if end < chars.len()
                    && let Some(sp) = chars[start..end].iter().rposition(|c| *c == ' ')
                    && sp > 0
                {
                    end = start + sp + 1;
                }
                rows.push(chars[start..end].iter().collect());
                start = end;
            }
            rows
        })
        .collect()
}
