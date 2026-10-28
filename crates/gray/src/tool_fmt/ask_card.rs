//! The "Asked" card: every question in full with the user's answer under
//! it. The host draws it for every plugin's `host/ask` (tool approvals,
//! `request_user_input`, git-flow, ...) when the modal closes.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// Indent under the card's `⬢ ` bullet, so the text lines up with "Asked".
const INDENT: &str = "  ";
const ARROW_PAD: &str = "    ";

/// One asked question and what the user picked (`None` = nothing yet).
pub struct AskedRow {
    pub question: String,
    pub answers: Vec<String>,
}

/// Body of the `request_user_input` card when a session is resumed: the
/// same rows the host drew live, rebuilt from the tool call and result.
pub(super) fn ask_body(args: Option<&serde_json::Value>, output: &str) -> Vec<Line<'static>> {
    let qs: Vec<(String, String)> = args
        .and_then(|a| a.get("questions"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|q| {
                    let text = q.get("question")?.as_str()?.trim();
                    let id = q.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    (!text.is_empty()).then(|| (id.to_string(), text.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    if qs.is_empty() {
        return Vec::new();
    }
    let parsed: Option<serde_json::Value> = serde_json::from_str(output.trim()).ok();
    let answers = parsed.as_ref().and_then(|v| v.get("answers"));
    // A non-JSON result is the plugin's "no answer yet" text.
    let fallback = if parsed.is_none() && !output.trim().is_empty() {
        "no answer yet"
    } else {
        "skipped"
    };
    let rows: Vec<AskedRow> = qs
        .into_iter()
        .map(|(id, question)| AskedRow {
            question,
            answers: answers
                .and_then(|m| m.get(&id))
                .and_then(|e| e.get("answers"))
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect();
    asked_lines(&rows, fallback)
}

/// Card header for any ask: `⬢ Asked` or `⬢ Asked N questions`.
pub fn asked_header(n: usize) -> Line<'static> {
    let theme = crate::theme::theme();
    let mut spans = vec![
        Span::styled(
            format!("{} ", crate::tui::glyph("\u{2b22}", "#")),
            Style::default()
                .fg(super::tool_hue("request_user_input"))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "Asked",
            Style::default()
                .fg(theme.text_body)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if n > 1 {
        spans.push(Span::styled(
            format!(" {n} questions"),
            Style::default().fg(theme.tool_dim),
        ));
    }
    Line::from(spans)
}

/// Every question in full with the answer under it. Shared by the host's
/// card for every plugin ask (approvals, request_user_input, ...) and the
/// resumed `request_user_input` tool card.
pub fn asked_lines(rows: &[AskedRow], fallback: &str) -> Vec<Line<'static>> {
    let theme = crate::theme::theme();
    let q_style = Style::default().fg(theme.text_body);
    let arrow_style = Style::default().fg(theme.tool_dim);
    let a_style = Style::default()
        .fg(theme.tool_command)
        .add_modifier(Modifier::BOLD);
    let none_style = Style::default()
        .fg(theme.tool_dim)
        .add_modifier(Modifier::ITALIC);

    let term_w = crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(100)
        .max(40);
    // card gutter (2) + right margin (2) + our indent
    let q_w = term_w.saturating_sub(4 + INDENT.len()).max(20);
    let a_w = term_w.saturating_sub(4 + ARROW_PAD.len()).max(20);

    let mut lines = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            lines.push(Line::from(""));
        }
        for r in wrap(row.question.trim(), q_w) {
            lines.push(Line::from(vec![
                Span::raw(INDENT),
                Span::styled(r, q_style),
            ]));
        }
        let picked: Vec<String> = row
            .answers
            .iter()
            .map(|s| match s.strip_prefix("user_note: ") {
                Some(note) => format!("note: {note}"),
                None => s.clone(),
            })
            .collect();
        let (text, style) = if picked.is_empty() {
            (fallback.to_string(), none_style)
        } else {
            (picked.join(" \u{b7} "), a_style)
        };
        for (j, r) in wrap(&text, a_w).into_iter().enumerate() {
            let lead = if j == 0 {
                Span::styled(
                    format!("{INDENT}{} ", crate::tui::glyph("\u{2192}", ">")),
                    arrow_style,
                )
            } else {
                Span::raw(ARROW_PAD)
            };
            lines.push(Line::from(vec![lead, Span::styled(r, style)]));
        }
    }
    lines
}

/// Greedy word wrap by display width; words wider than `w` are split.
fn wrap(text: &str, w: usize) -> Vec<String> {
    let mut rows = Vec::new();
    for para in text.lines() {
        let mut cur = String::new();
        for word in para.split_whitespace() {
            let sep = usize::from(!cur.is_empty());
            if cur.width() + sep + word.width() <= w {
                if sep == 1 {
                    cur.push(' ');
                }
                cur.push_str(word);
                continue;
            }
            if !cur.is_empty() {
                rows.push(std::mem::take(&mut cur));
            }
            let mut piece = String::new();
            for c in word.chars() {
                if piece.width() + c.to_string().width() > w {
                    rows.push(std::mem::take(&mut piece));
                }
                piece.push(c);
            }
            cur = piece;
        }
        rows.push(cur);
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn args() -> serde_json::Value {
        serde_json::json!({"questions": [
            {"id": "ship", "question": "What should I do with the long question that must not be cut?"},
            {"id": "color", "question": "Pick a color"}
        ]})
    }

    #[test]
    fn shows_every_question_in_full_with_its_answer() {
        let out = r#"{"answers":{"ship":{"answers":["Merge + deploy (Recommended)"]},"color":{"answers":["Red","user_note: darker"]}}}"#;
        let shown = text(&ask_body(Some(&args()), out));
        assert!(
            shown.contains("What should I do with the long question that must not be cut?"),
            "{shown}"
        );
        assert!(shown.contains("Merge + deploy (Recommended)"), "{shown}");
        assert!(shown.contains("Red \u{b7} note: darker"), "{shown}");
        assert!(!shown.contains('\u{2026}'), "{shown}");
    }

    #[test]
    fn missing_answers_read_as_skipped_or_pending() {
        let shown = text(&ask_body(Some(&args()), r#"{"answers":{}}"#));
        assert_eq!(shown.matches("skipped").count(), 2, "{shown}");
        let shown = text(&ask_body(Some(&args()), "no answer yet — keep working"));
        assert!(shown.contains("no answer yet"), "{shown}");
    }

    #[test]
    fn wrap_keeps_words_and_width() {
        let rows = wrap("aaa bbb ccc dddddddddd", 7);
        assert_eq!(rows, ["aaa bbb", "ccc", "ddddddd", "ddd"]);
    }
}
