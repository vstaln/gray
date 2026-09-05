//! Question panel rendering for the inline viewport (split from `question`).

use super::*;

/// Builds the panel lines for the inline viewport (draw side). Rows are
/// capped at `max_rows`; the option window scrolls around the selection.
pub(crate) fn panel_lines(q: &QuestionSession, w: usize, max_rows: usize) -> Vec<Line<'static>> {
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
            lines.push(option_row(prefix, i + 1, label, Some(desc), i == sel));
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

    let question_text = format!(" {}", q.current_question().question);
    let wrapped = wrap_plain(&question_text, w);
    let q_lines = wrapped.len();
    lines.extend(wrapped.into_iter().map(|l| {
        Line::from(Span::styled(
            l,
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        ))
    }));

    // Budget: top(1) + progress(1) + question + tips(1) + bottom margin(1);
    // rest goes to options.
    // Min 3 options so long questions don't squeeze to 1.
    let budget = max_rows.saturating_sub(4 + q_lines.min(max_rows.saturating_sub(4)));
    let len = q.options_len();
    // Cursor position vs committed pick are different things: the cursor row
    // always shows the arrow and gets accent styling from the start, so the
    // preselected first option reads as highlighted with zero moves — a
    // highlight is the cursor, never a submitted answer (preselect stays
    // unanswered until Enter, Space, a digit, or notes confirm it).
    let picked = q.answers[q.current_idx].selected_idx.is_some();
    let cursor = q.answers[q.current_idx].selected_idx.unwrap_or(0);
    let visible = budget.min(len).max(3.min(len));
    let start = cursor.saturating_sub(visible.saturating_sub(1)).min(cursor);
    for i in start..len.min(start + visible) {
        let is_cursor = i == cursor;
        let prefix = if is_cursor { icon("arrow") } else { " " };
        let label = q.option_label_for_index(i).unwrap_or_default();
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

    lines.push(tips_line(q));
    // bottom margin mirrors the top one — without it the footer jams
    // against "enter to submit".
    lines.push(Line::from("").style(bg_style));
    lines
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
    let desc_w = w.saturating_sub(head.chars().count() + 3 + 4).max(10);
    let chunks = wrap_plain(d, desc_w + 4);
    let mut rows = vec![Line::from(vec![
        Span::styled(format!(" {prefix} {num}. "), accent),
        Span::styled(label.to_string(), label_style),
        Span::styled(
            format!(" — {}", chunks.first().cloned().unwrap_or_default()),
            dim_style,
        ),
    ])];
    let indent = " ".repeat(head.chars().count() + 3);
    for c in chunks.iter().skip(1) {
        rows.push(Line::from(Span::styled(format!("{indent}{c}"), dim_style)));
    }
    rows
}

fn option_row(
    prefix: &str,
    num: usize,
    label: &str,
    desc: Option<&str>,
    selected: bool,
) -> Line<'static> {
    let mut spans = vec![
        Span::styled(
            format!(" {prefix} {num}. "),
            Style::default()
                .fg(if selected { ACCENT } else { DIM })
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            label.to_string(),
            Style::default()
                .fg(if selected { ACCENT } else { TEXT })
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(d) = desc {
        spans.push(Span::styled(format!(" — {d}"), Style::default().fg(DIM)));
    }
    Line::from(spans)
}

fn tips_line(q: &QuestionSession) -> Line<'static> {
    let notes_visible =
        q.answers[q.current_idx].notes_visible || !q.answers[q.current_idx].draft.trim().is_empty();
    let mut tips: Vec<(String, bool)> = Vec::new();
    let sel = q.answers[q.current_idx].selected_idx.is_some();
    if sel && !notes_visible {
        tips.push(("tab to add notes".into(), true));
    } else if sel && notes_visible {
        tips.push(("tab or esc to clear notes".into(), false));
    }
    let is_last = q.current_idx + 1 >= q.questions.len();
    let submit = if q.questions.len() == 1 || is_last {
        "enter picks + submits"
    } else {
        "enter picks + next"
    };
    tips.push(("backspace skips question".into(), false));
    tips.push((submit.into(), true));
    if q.questions.len() > 1 {
        tips.push(("←/→ to change question".into(), false));
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
                let mut end = (start + content_w).min(chars.len());
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
