//! Diff rendering: hunk parsing, per-line highlight, wrapping (split from `tool_fmt`).

use super::*;

// ── Diff Line and Hunk Data Types ──────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffTag {
    Equal,
    Delete,
    Insert,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub text: String,
    pub lo: usize, // old line number (1-based)
    pub ln: usize, // new line number (1-based)
    pub tag: DiffTag,
}

pub type DiffHunk = Vec<DiffLine>;

/// Parses a hunk header line like `@@ -10,5 +12,6 @@`.
fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    let parts: Vec<&str> = line.split("@@").collect();
    if parts.len() < 2 {
        return None;
    }
    let middle = parts[1].trim();
    let mut old_start = 1;
    let mut new_start = 1;
    for token in middle.split_whitespace() {
        if let Some(s) = token.strip_prefix('-') {
            if let Some(num) = s.split(',').next().and_then(|n| n.parse::<usize>().ok()) {
                old_start = num;
            }
        } else if let Some(s) = token.strip_prefix('+')
            && let Some(num) = s.split(',').next().and_then(|n| n.parse::<usize>().ok())
        {
            new_start = num;
        }
    }
    Some((old_start, new_start))
}

/// Parses unified diff text into structured [`DiffHunk`]s.
pub fn parse_diff_hunks(diff_text: &str) -> Vec<DiffHunk> {
    let mut hunks = Vec::new();
    let mut current_hunk = Vec::new();
    let mut old_line = 1usize;
    let mut new_line = 1usize;
    let mut in_hunk = false;

    for line in diff_text.lines() {
        if line.starts_with("@@ ") {
            if !current_hunk.is_empty() {
                hunks.push(std::mem::take(&mut current_hunk));
            }
            in_hunk = true;
            if let Some((o, n)) = parse_hunk_header(line) {
                old_line = o;
                new_line = n;
            }
            continue;
        }

        if !in_hunk {
            if line.starts_with("--- ") || line.starts_with("+++ ") {
                continue;
            }
            // Fallback for diffs without @@ header
            if line.starts_with('+') || line.starts_with('-') || line.starts_with(' ') {
                in_hunk = true;
            } else {
                continue;
            }
        }

        if line.starts_with('+') && !line.starts_with("+++") {
            let text = line[1..].to_string();
            current_hunk.push(DiffLine {
                text,
                lo: old_line,
                ln: new_line,
                tag: DiffTag::Insert,
            });
            new_line += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            let text = line[1..].to_string();
            current_hunk.push(DiffLine {
                text,
                lo: old_line,
                ln: new_line,
                tag: DiffTag::Delete,
            });
            old_line += 1;
        } else if let Some(stripped) = line.strip_prefix(' ') {
            let text = stripped.to_string();
            current_hunk.push(DiffLine {
                text,
                lo: old_line,
                ln: new_line,
                tag: DiffTag::Equal,
            });
            old_line += 1;
            new_line += 1;
        }
    }

    if !current_hunk.is_empty() {
        hunks.push(current_hunk);
    }
    hunks
}

pub(crate) fn highlight_line_spans(
    line: &str,
    highlighter: &mut Option<gray_markdown::syntect::easy::HighlightLines<'_>>,
    syntect: &gray_markdown::Syntect,
    fallback_fg: Color,
    bg: Option<Color>,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if let Some(hl) = highlighter.as_mut()
        && let Ok(ranges) = hl.highlight_line(&format!("{line}\n"), &syntect.syntax_set)
    {
        let mut wrote = false;
        for (style, segment) in ranges {
            let mut text = segment.to_string();
            while text.ends_with('\n') || text.ends_with('\r') {
                text.pop();
            }
            if text.is_empty() {
                continue;
            }
            let fg = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
            let mut st = Style::default().fg(fg);
            if style
                .font_style
                .contains(gray_markdown::syntect::highlighting::FontStyle::BOLD)
            {
                st = st.add_modifier(Modifier::BOLD);
            }
            if style
                .font_style
                .contains(gray_markdown::syntect::highlighting::FontStyle::ITALIC)
            {
                st = st.add_modifier(Modifier::ITALIC);
            }
            if let Some(bg_c) = bg {
                st = st.bg(bg_c);
            }
            spans.push(Span::styled(text, st));
            wrote = true;
        }
        if wrote {
            return spans;
        }
    }

    let mut st = Style::default().fg(fallback_fg);
    if let Some(bg_c) = bg {
        st = st.bg(bg_c);
    }
    let text = if line.is_empty() { " " } else { line };
    spans.push(Span::styled(text.to_string(), st));
    spans
}

pub(crate) fn wrap_styled_spans(
    spans: Vec<Span<'static>>,
    max_w: usize,
    cont_indent_len: usize,
) -> Vec<Vec<Span<'static>>> {
    if spans.is_empty() {
        return vec![Vec::new()];
    }
    let total_width: usize = spans.iter().map(|s| s.width()).sum();
    if total_width <= max_w {
        return vec![spans];
    }

    let mut result: Vec<Vec<Span<'static>>> = Vec::new();
    let mut cur_line: Vec<Span<'static>> = Vec::new();
    let mut cur_w: usize = 0;
    let mut is_first_line = true;

    let get_avail = |is_first: bool| -> usize {
        if is_first {
            max_w
        } else {
            max_w.saturating_sub(cont_indent_len).max(10)
        }
    };

    for span in spans {
        let style = span.style;
        let text = span.content;
        let mut chars = text.chars().peekable();

        while chars.peek().is_some() {
            let is_space = chars.peek().map(|&c| c == ' ').unwrap_or(false);
            let mut word = String::new();
            if is_space {
                while let Some(&c) = chars.peek() {
                    if c == ' ' {
                        word.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
            } else {
                while let Some(&c) = chars.peek() {
                    if c != ' ' {
                        word.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
            }

            let word_w = word.chars().count();
            let avail = get_avail(is_first_line);

            if cur_w + word_w <= avail {
                cur_line.push(Span::styled(word, style));
                cur_w += word_w;
            } else if is_space && cur_line.is_empty() {
                continue;
            } else if word_w > avail {
                if cur_w > 0 {
                    result.push(std::mem::take(&mut cur_line));
                    is_first_line = false;
                    cur_w = 0;
                }
                let line_cap = get_avail(is_first_line);
                let wchars: Vec<char> = word.chars().collect();
                for chunk in wchars.chunks(line_cap) {
                    let chunk_str: String = chunk.iter().collect();
                    let chunk_len = chunk_str.chars().count();
                    if cur_w + chunk_len > get_avail(is_first_line) && cur_w > 0 {
                        result.push(std::mem::take(&mut cur_line));
                        is_first_line = false;
                        cur_w = 0;
                    }
                    cur_line.push(Span::styled(chunk_str, style));
                    cur_w += chunk_len;
                    if cur_w >= get_avail(is_first_line) {
                        result.push(std::mem::take(&mut cur_line));
                        is_first_line = false;
                        cur_w = 0;
                    }
                }
            } else {
                if !cur_line.is_empty() {
                    result.push(std::mem::take(&mut cur_line));
                    is_first_line = false;
                    cur_w = 0;
                }
                if !is_space {
                    cur_line.push(Span::styled(word, style));
                    cur_w += word_w;
                }
            }
        }
    }

    if !cur_line.is_empty() || result.is_empty() {
        result.push(cur_line);
    }
    result
}

/// Renders unified diff hunks with Codex/Grok-style gutter line numbers,
/// additions/removals summary, background colors, and Syntect syntax highlighting.
pub fn render_diff_hunks(
    hunks: &[DiffHunk],
    path: Option<&Path>,
    cwd: Option<&Path>,
) -> Vec<Line<'static>> {
    let syntect = gray_markdown::get_syntect();
    let mut lines = Vec::new();

    let mut additions = 0usize;
    let mut deletions = 0usize;
    for hunk in hunks {
        for line in hunk {
            match line.tag {
                DiffTag::Insert => additions += 1,
                DiffTag::Delete => deletions += 1,
                DiffTag::Equal => {}
            }
        }
    }

    if let Some(p) = path {
        let p_display = shorten_path(&p.display().to_string(), cwd);
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("Updated ", Style::default().fg(Color::Rgb(160, 160, 160))),
            Span::styled(
                p_display,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" with {additions} additions and {deletions} removals"),
                Style::default().fg(Color::Rgb(160, 160, 160)),
            ),
        ]));
    }

    for (i, hunk) in hunks.iter().enumerate() {
        if i > 0 && !lines.is_empty() {
            let prev_last = hunks[i - 1]
                .iter()
                .rev()
                .find(|l| l.tag != DiffTag::Delete)
                .map(|l| l.ln);
            let next_first = hunk.iter().find(|l| l.tag != DiffTag::Delete).map(|l| l.ln);

            let gap_text = if let (Some(p), Some(n)) = (prev_last, next_first) {
                if n > p + 1 {
                    let count = n - p - 1;
                    if count == 1 {
                        "  … 1 unchanged line".to_string()
                    } else {
                        format!("  … {count} unchanged lines")
                    }
                } else {
                    "  …".to_string()
                }
            } else {
                "  …".to_string()
            };

            lines.push(Line::from(vec![Span::styled(
                gap_text,
                Style::default().fg(DIFF_GUTTER_FG),
            )]));
        }

        if hunk.is_empty() {
            continue;
        }

        let mut max_num = 1usize;
        for line in hunk {
            max_num = max_num.max(line.lo).max(line.ln);
        }
        let gutter_width = max_num.to_string().len().max(3);

        let mut old_highlighter = path.and_then(|p| syntect.highlight_lines_by_file_path(p));
        let mut new_highlighter = path.and_then(|p| syntect.highlight_lines_by_file_path(p));
        let term_w = crossterm::terminal::size()
            .map(|(w, _)| w as usize)
            .unwrap_or(120)
            .max(60);
        let overhead = 2 + gutter_width + 5 + 2;
        let content_w = term_w.saturating_sub(overhead).max(20);

        for line in hunk {
            let bg_color = match line.tag {
                DiffTag::Equal => None,
                DiffTag::Delete => Some(DIFF_DELETE_BG),
                DiffTag::Insert => Some(DIFF_INSERT_BG),
            };
            let prefix_style = if let Some(bg) = bg_color {
                Style::default().bg(bg)
            } else {
                Style::default()
            };
            let num = match line.tag {
                DiffTag::Equal => line.ln,
                DiffTag::Delete => line.lo,
                DiffTag::Insert => line.ln,
            };
            let sign = match line.tag {
                DiffTag::Equal => " ",
                DiffTag::Delete => "-",
                DiffTag::Insert => "+",
            };

            let gutter_num_style = if let Some(bg) = bg_color {
                Style::default().fg(DIFF_GUTTER_FG).bg(bg)
            } else {
                Style::default().fg(DIFF_GUTTER_FG)
            };
            let gutter_pipe_style = gutter_num_style;
            let gutter_sign_style = match line.tag {
                DiffTag::Equal => gutter_num_style,
                DiffTag::Delete => Style::default()
                    .fg(DIFF_DELETE_FG)
                    .bg(DIFF_DELETE_BG)
                    .add_modifier(Modifier::BOLD),
                DiffTag::Insert => Style::default()
                    .fg(DIFF_INSERT_FG)
                    .bg(DIFF_INSERT_BG)
                    .add_modifier(Modifier::BOLD),
            };

            let num_str = format!("{:>width$} ", num, width = gutter_width);
            let pipe_str = "| ";
            let sign_str = format!("{sign} ");
            let cont_num_str = format!("{:>width$} ", "", width = gutter_width);
            let cont_sign_str = "  ";

            let expanded = expand_tabs(&line.text, 4);
            let indent_count = expanded.chars().take_while(|c| *c == ' ').count();
            let cont_indent_len = indent_count.min(content_w / 2);
            let cont_indent_str = " ".repeat(cont_indent_len);

            let row_spans = match line.tag {
                DiffTag::Delete => highlight_line_spans(
                    &expanded,
                    &mut old_highlighter,
                    syntect,
                    DIFF_EQUAL_FG,
                    bg_color,
                ),
                DiffTag::Insert => highlight_line_spans(
                    &expanded,
                    &mut new_highlighter,
                    syntect,
                    DIFF_EQUAL_FG,
                    bg_color,
                ),
                DiffTag::Equal => {
                    let s = highlight_line_spans(
                        &expanded,
                        &mut new_highlighter,
                        syntect,
                        DIFF_EQUAL_FG,
                        None,
                    );
                    if let Some(hl) = old_highlighter.as_mut() {
                        let _ = hl.highlight_line(&format!("{expanded}\n"), &syntect.syntax_set);
                    }
                    s
                }
            };

            let wrapped_rows = wrap_styled_spans(row_spans, content_w, cont_indent_len);
            for (ci, chunk_spans) in wrapped_rows.into_iter().enumerate() {
                let mut spans = Vec::new();
                spans.push(Span::styled("  ", prefix_style));
                if ci == 0 {
                    spans.push(Span::styled(num_str.clone(), gutter_num_style));
                    spans.push(Span::styled(pipe_str, gutter_pipe_style));
                    spans.push(Span::styled(sign_str.clone(), gutter_sign_style));
                } else {
                    spans.push(Span::styled(cont_num_str.clone(), gutter_num_style));
                    spans.push(Span::styled(pipe_str, gutter_pipe_style));
                    spans.push(Span::styled(cont_sign_str, gutter_num_style));
                    if cont_indent_len > 0 {
                        spans.push(Span::styled(cont_indent_str.clone(), prefix_style));
                    }
                }
                spans.extend(chunk_spans);
                let base_line = Line::from(spans);
                let line_obj = if let Some(bg) = bg_color {
                    base_line.style(Style::default().bg(bg))
                } else {
                    base_line
                };
                lines.push(line_obj);
            }
        }
    }

    lines
}
