//! Tool formatting and rendering matching Grok CLI (GrokNight theme).
//!
//! Provides rich, clean terminal display for tool calls (bash, write, edit, read,
//! grep, find, ls) with Grok-styled diff rendering and syntax highlighting.

use std::path::Path;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

// ── Palette (GrokNight Theme) ──────────────────────────────────────────────
pub const ACCENT_TOOL: Color = Color::Rgb(158, 206, 106); // #9ece6a (green bullet)
pub const TEXT_PRIMARY: Color = Color::Rgb(225, 225, 225); // #e1e1e1 (bold white)
pub const PATH_COLOR: Color = Color::Rgb(255, 158, 100); // #ff9e64 (TokyoNight orange)
pub const COMMAND_COLOR: Color = Color::Rgb(224, 175, 104); // #e0af68 (TokyoNight yellow)
pub const DIM_COLOR: Color = Color::Rgb(108, 108, 108); // #6c6c6c (muted gray)

pub const DIFF_DELETE_BG: Color = Color::Rgb(74, 34, 29); // #4A221D (dark red, Codex matching)
pub const DIFF_DELETE_FG: Color = Color::Rgb(247, 118, 142); // #f7768e (bright red)
pub const DIFF_INSERT_BG: Color = Color::Rgb(33, 58, 43); // #213A2B (dark green, Codex matching)
pub const DIFF_INSERT_FG: Color = Color::Rgb(158, 206, 106); // #9ece6a (bright green)
pub const DIFF_EQUAL_FG: Color = Color::Rgb(225, 225, 225); // #e1e1e1 (code text)
pub const DIFF_GUTTER_FG: Color = Color::Rgb(108, 108, 108); // #6c6c6c (line numbers)

fn arg_path<'a>(args: &'a serde_json::Value) -> &'a str {
    args.get("path")
        .or_else(|| args.get("file_path"))
        .or_else(|| args.get("filePath"))
        .or_else(|| args.get("TargetFile"))
        .or_else(|| args.get("target_file"))
        .or_else(|| args.get("targetFile"))
        .or_else(|| args.get("file"))
        .or_else(|| args.get("filename"))
        .or_else(|| args.get("target"))
        .or_else(|| args.get("destination"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

fn arg_content<'a>(args: &'a serde_json::Value) -> &'a str {
    args.get("content")
        .or_else(|| args.get("contents"))
        .or_else(|| args.get("CodeContent"))
        .or_else(|| args.get("code_content"))
        .or_else(|| args.get("codeContent"))
        .or_else(|| args.get("text"))
        .or_else(|| args.get("code"))
        .or_else(|| args.get("body"))
        .or_else(|| args.get("data"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

/// Shortens a path relative to CWD or HOME for compact terminal display.
pub fn shorten_path(path_str: &str, cwd: Option<&Path>) -> String {
    let path = Path::new(path_str);
    if let Some(cwd) = cwd {
        if let Ok(rel) = path.strip_prefix(cwd) {
            let rel_str = rel.display().to_string();
            if !rel_str.is_empty() {
                return rel_str;
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let home_path = Path::new(&home);
        if let Ok(rel) = path.strip_prefix(home_path) {
            return format!("~/{}", rel.display());
        }
    }
    path_str.to_string()
}

/// Formats a tool invocation header line matching Grok CLI styling for Ratatui.
pub fn format_tool_call_header(name: &str, args: &serde_json::Value, cwd: Option<&Path>) -> Line<'static> {
    let bullet = Span::styled("\u{2b22} ", Style::default().fg(ACCENT_TOOL).add_modifier(Modifier::BOLD));
    let action_style = Style::default().fg(TEXT_PRIMARY).add_modifier(Modifier::BOLD);
    let path_style = Style::default().fg(PATH_COLOR).add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let cmd_style = Style::default().fg(COMMAND_COLOR).add_modifier(Modifier::BOLD);
    let dim_style = Style::default().fg(DIM_COLOR);

    match name {
        "bash" => {
            let cmd = args.get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            Line::from(vec![
                bullet,
                Span::styled("Ran ", action_style),
                Span::styled(cmd.to_string(), cmd_style),
            ])
        }
        "write" => {
            let path = shorten_path(arg_path(args), cwd);
            let content = arg_content(args);
            let lines_count = content.lines().count();
            Line::from(vec![
                bullet,
                Span::styled("Wrote ", action_style),
                Span::styled(path, path_style),
                Span::styled(format!(" ({lines_count} lines)"), dim_style),
            ])
        }
        "edit" => {
            let path = shorten_path(arg_path(args), cwd);
            Line::from(vec![
                bullet,
                Span::styled("Edit ", action_style),
                Span::styled(path, path_style),
            ])
        }
        "read" => {
            let path = shorten_path(arg_path(args), cwd);
            let offset = args.get("offset").and_then(|v| v.as_u64());
            let limit = args.get("limit").and_then(|v| v.as_u64());
            let span_detail = match (offset, limit) {
                (Some(o), Some(l)) => format!(" (lines {o}-{})", o + l),
                (Some(o), None) => format!(" (line {o}+)"),
                _ => String::new(),
            };
            Line::from(vec![
                bullet,
                Span::styled("Read ", action_style),
                Span::styled(path, path_style),
                Span::styled(span_detail, dim_style),
            ])
        }
        "grep" => {
            let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let raw_path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let mut spans = vec![
                bullet,
                Span::styled("Grep ", action_style),
                Span::styled(format!("\"{pattern}\""), cmd_style),
            ];
            if !raw_path.is_empty() && raw_path != "." {
                spans.push(Span::styled(" in ", dim_style));
                spans.push(Span::styled(shorten_path(raw_path, cwd), path_style));
            }
            Line::from(spans)
        }
        "find" => {
            let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let raw_path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let mut spans = vec![
                bullet,
                Span::styled("Find ", action_style),
                Span::styled(format!("\"{pattern}\""), cmd_style),
            ];
            if !raw_path.is_empty() && raw_path != "." {
                spans.push(Span::styled(" in ", dim_style));
                spans.push(Span::styled(shorten_path(raw_path, cwd), path_style));
            }
            Line::from(spans)
        }
        "ls" => {
            let raw_path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            Line::from(vec![
                bullet,
                Span::styled("List ", action_style),
                Span::styled(shorten_path(raw_path, cwd), path_style),
            ])
        }
        other => {
            let args_preview = if let Some(obj) = args.as_object() {
                obj.iter()
                    .take(2)
                    .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or(&v.to_string())))
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                String::new()
            };
            Line::from(vec![
                bullet,
                Span::styled(other.to_string(), action_style),
                Span::raw(" "),
                Span::styled(args_preview, dim_style),
            ])
        }
    }
}

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
        } else if let Some(s) = token.strip_prefix('+') {
            if let Some(num) = s.split(',').next().and_then(|n| n.parse::<usize>().ok()) {
                new_start = num;
            }
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
        } else if line.starts_with(' ') {
            let text = line[1..].to_string();
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

fn render_content_spans(
    content: &str,
    highlighter: &mut Option<gray_markdown::syntect::easy::HighlightLines<'_>>,
    syntect: &gray_markdown::Syntect,
    fallback_fg: Color,
    bg: Option<Color>,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if let Some(hl) = highlighter.as_mut()
        && let Ok(ranges) = hl.highlight_line(&format!("{content}\n"), &syntect.syntax_set)
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
            if style.font_style.contains(gray_markdown::syntect::highlighting::FontStyle::BOLD) {
                st = st.add_modifier(Modifier::BOLD);
            }
            if style.font_style.contains(gray_markdown::syntect::highlighting::FontStyle::ITALIC) {
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
    let text = if content.is_empty() { " " } else { content };
    spans.push(Span::styled(text.to_string(), st));
    spans
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
            Span::styled(p_display, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" with {additions} additions and {deletions} removals"), Style::default().fg(Color::Rgb(160, 160, 160))),
        ]));
    }

    for (i, hunk) in hunks.iter().enumerate() {
        if i > 0 && !lines.is_empty() {
            let prev_last = hunks[i - 1]
                .iter()
                .rev()
                .find(|l| l.tag != DiffTag::Delete)
                .map(|l| l.ln);
            let next_first = hunk
                .iter()
                .find(|l| l.tag != DiffTag::Delete)
                .map(|l| l.ln);

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

            lines.push(Line::from(vec![
                Span::styled(gap_text, Style::default().fg(DIFF_GUTTER_FG)),
            ]));
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

        for line in hunk {
            let mut spans = Vec::new();

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
            spans.push(Span::styled("  ", prefix_style));

            let gutter_style = match line.tag {
                DiffTag::Equal => Style::default().fg(DIFF_GUTTER_FG),
                DiffTag::Delete => Style::default().fg(DIFF_DELETE_FG).bg(DIFF_DELETE_BG),
                DiffTag::Insert => Style::default().fg(DIFF_INSERT_FG).bg(DIFF_INSERT_BG),
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

            let gutter_str = format!("{:>width$} | {sign} ", num, width = gutter_width);
            spans.push(Span::styled(gutter_str, gutter_style));

            let text = &line.text;
            let content_spans = match line.tag {
                DiffTag::Delete => {
                    render_content_spans(text, &mut old_highlighter, syntect, DIFF_DELETE_FG, bg_color)
                }
                DiffTag::Insert => {
                    render_content_spans(text, &mut new_highlighter, syntect, DIFF_INSERT_FG, bg_color)
                }
                DiffTag::Equal => {
                    let s = render_content_spans(text, &mut new_highlighter, syntect, DIFF_EQUAL_FG, None);
                    if let Some(hl) = old_highlighter.as_mut() {
                        let _ = hl.highlight_line(&format!("{text}\n"), &syntect.syntax_set);
                    }
                    s
                }
            };
            spans.extend(content_spans);

            let mut line_obj = Line::from(spans);
            if let Some(bg) = bg_color {
                line_obj.style = Style::default().bg(bg);
            }
            lines.push(line_obj);
        }
    }

    lines
}

/// Renders a newly created / written code block with line numbers and syntax highlighting.
pub fn render_code_block(content: &str, path: Option<&Path>) -> Vec<Line<'static>> {
    let syntect = gray_markdown::get_syntect();
    let mut highlighter = path.and_then(|p| syntect.highlight_lines_by_file_path(p));
    let raw_lines: Vec<&str> = content.lines().collect();
    let total = raw_lines.len();
    if total == 0 {
        return Vec::new();
    }
    let max_lines_to_show = 30usize;
    let gutter_width = total.to_string().len().max(3);
    let mut lines = Vec::new();

    if total <= max_lines_to_show {
        for (idx, line_text) in raw_lines.iter().enumerate() {
            let line_num = idx + 1;
            let mut spans = Vec::new();
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                format!("{:>width$} | ", line_num, width = gutter_width),
                Style::default().fg(DIFF_GUTTER_FG),
            ));
            let content_spans = render_content_spans(line_text, &mut highlighter, syntect, DIFF_EQUAL_FG, None);
            spans.extend(content_spans);
            lines.push(Line::from(spans));
        }
    } else {
        const HEAD: usize = 18;
        const TAIL: usize = 6;
        for (idx, line_text) in raw_lines.iter().take(HEAD).enumerate() {
            let line_num = idx + 1;
            let mut spans = Vec::new();
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                format!("{:>width$} | ", line_num, width = gutter_width),
                Style::default().fg(DIFF_GUTTER_FG),
            ));
            let content_spans = render_content_spans(line_text, &mut highlighter, syntect, DIFF_EQUAL_FG, None);
            spans.extend(content_spans);
            lines.push(Line::from(spans));
        }
        let omitted = total.saturating_sub(HEAD + TAIL);
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("… +{omitted} lines"), Style::default().fg(DIM_COLOR).add_modifier(Modifier::ITALIC)),
        ]));
        for (idx, line_text) in raw_lines.iter().skip(total - TAIL).enumerate() {
            let line_num = total - TAIL + idx + 1;
            let mut spans = Vec::new();
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                format!("{:>width$} | ", line_num, width = gutter_width),
                Style::default().fg(DIFF_GUTTER_FG),
            ));
            let content_spans = render_content_spans(line_text, &mut highlighter, syntect, DIFF_EQUAL_FG, None);
            spans.extend(content_spans);
            lines.push(Line::from(spans));
        }
    }

    lines
}

/// Formats tool output lines with Codex/Grok-style rendering.
pub fn format_tool_result_lines_with_context(
    tool_name: &str,
    args: Option<&serde_json::Value>,
    output: &str,
    is_error: bool,
    cwd: Option<&Path>,
) -> Vec<Line<'static>> {
    if is_error {
        let trimmed = output.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        let mut lines = Vec::new();
        for (i, l) in trimmed.lines().take(8).enumerate() {
            let prefix = if i == 0 { "  ✗ " } else { "    " };
            lines.push(Line::from(vec![
                Span::styled(prefix, Style::default().fg(DIFF_DELETE_FG).add_modifier(Modifier::BOLD)),
                Span::styled((*l).to_string(), Style::default().fg(DIFF_DELETE_FG)),
            ]));
        }
        return lines;
    }

    let raw_path = args.map(arg_path).unwrap_or("");
    let path_buf = if !raw_path.is_empty() {
        Some(if let Some(c) = cwd {
            c.join(raw_path)
        } else {
            Path::new(raw_path).to_path_buf()
        })
    } else {
        None
    };
    let file_path = path_buf.as_deref();

    // Check if this output is a diff or from edit/write tool
    if tool_name == "edit" || output.starts_with("--- ") || output.contains("@@ ") {
        let hunks = parse_diff_hunks(output);
        if !hunks.is_empty() {
            return render_diff_hunks(&hunks, file_path, cwd);
        }
        return Vec::new();
    }

    if tool_name == "write" {
        // If write produced a diff (from overwriting an existing file), render it
        if output.starts_with("--- ") || output.contains("@@ ") {
            let hunks = parse_diff_hunks(output);
            if !hunks.is_empty() {
                return render_diff_hunks(&hunks, file_path, cwd);
            }
        }
        // If a file was written/created, display the written code block with line numbers & syntax highlighting
        let content = args.map(arg_content).unwrap_or("");
        if !content.is_empty() {
            return render_code_block(content, file_path);
        }
        return Vec::new();
    }

    if tool_name == "read" {
        return Vec::new();
    }

    let show_output = matches!(tool_name, "bash" | "grep" | "find" | "ls");
    if !show_output {
        return Vec::new();
    }

    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    const MAX_HEAD_LINES: usize = 6;
    const MAX_TAIL_LINES: usize = 4;
    const MAX_TOTAL_LINES: usize = MAX_HEAD_LINES + MAX_TAIL_LINES + 2;
    let raw_lines: Vec<&str> = trimmed.lines().collect();
    let total = raw_lines.len();
    let text_dim = Color::Rgb(160, 160, 160);
    let mut lines = Vec::new();

    if total <= MAX_TOTAL_LINES {
        for l in raw_lines {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(l.to_string(), Style::default().fg(text_dim)),
            ]));
        }
    } else {
        for l in raw_lines.iter().take(MAX_HEAD_LINES) {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled((*l).to_string(), Style::default().fg(text_dim)),
            ]));
        }
        let omitted = total.saturating_sub(MAX_HEAD_LINES + MAX_TAIL_LINES);
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("… +{omitted} lines"), Style::default().fg(DIM_COLOR).add_modifier(Modifier::ITALIC)),
        ]));
        for l in raw_lines.iter().skip(total - MAX_TAIL_LINES) {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled((*l).to_string(), Style::default().fg(text_dim)),
            ]));
        }
    }
    lines
}

/// Formats tool output lines (convenience wrapper).
pub fn format_tool_result_lines(tool_name: &str, output: &str, is_error: bool) -> Vec<Line<'static>> {
    format_tool_result_lines_with_context(tool_name, None, output, is_error, None)
}

/// Plain ANSI string formatting for one-shot / non-TUI output header.
pub fn format_tool_call_header_plain(name: &str, args: &serde_json::Value, cwd: Option<&Path>) -> String {
    let bullet = "\x1b[38;2;158;206;106m\x1b[1m\u{2b22}\x1b[0m";
    let bold = "\x1b[1m";
    let orange = "\x1b[38;2;255;158;100m\x1b[1m";
    let yellow = "\x1b[38;2;224;175;104m\x1b[1m";
    let dim = "\x1b[38;2;108;108;108m";
    let reset = "\x1b[0m";

    match name {
        "bash" => {
            let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("").trim();
            format!("{bullet} {bold}Ran{reset} {yellow}{cmd}{reset}")
        }
        "write" => {
            let path = shorten_path(arg_path(args), cwd);
            let content = arg_content(args);
            let lines_count = content.lines().count();
            format!("{bullet} {bold}Wrote{reset} {orange}{path}{reset} {dim}({lines_count} lines){reset}")
        }
        "edit" => {
            let path = shorten_path(arg_path(args), cwd);
            format!("{bullet} {bold}Edit{reset} {orange}{path}{reset}")
        }
        "read" => {
            let path = shorten_path(arg_path(args), cwd);
            let offset = args.get("offset").and_then(|v| v.as_u64());
            let limit = args.get("limit").and_then(|v| v.as_u64());
            let span_detail = match (offset, limit) {
                (Some(o), Some(l)) => format!(" {dim}(lines {o}-{}){reset}", o + l),
                (Some(o), None) => format!(" {dim}(line {o}+){reset}"),
                _ => String::new(),
            };
            format!("{bullet} {bold}Read{reset} {orange}{path}{reset}{span_detail}")
        }
        "grep" => {
            let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let raw_path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let in_clause = if !raw_path.is_empty() && raw_path != "." {
                format!(" {dim}in{reset} {orange}{}{reset}", shorten_path(raw_path, cwd))
            } else {
                String::new()
            };
            format!("{bullet} {bold}Grep{reset} {yellow}\"{pattern}\"{reset}{in_clause}")
        }
        "find" => {
            let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let raw_path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let in_clause = if !raw_path.is_empty() && raw_path != "." {
                format!(" {dim}in{reset} {orange}{}{reset}", shorten_path(raw_path, cwd))
            } else {
                String::new()
            };
            format!("{bullet} {bold}Find{reset} {yellow}\"{pattern}\"{reset}{in_clause}")
        }
        "ls" => {
            let raw_path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            format!("{bullet} {bold}List{reset} {orange}{}{reset}", shorten_path(raw_path, cwd))
        }
        other => {
            format!("{bullet} {bold}{other}{reset}")
        }
    }
}

/// Plain ANSI string formatting for tool output lines with context.
pub fn format_tool_result_plain_with_context(
    tool_name: &str,
    args: Option<&serde_json::Value>,
    output: &str,
    is_error: bool,
    cwd: Option<&Path>,
) -> String {
    let lines = format_tool_result_lines_with_context(tool_name, args, output, is_error, cwd);
    if lines.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for l in lines {
        let mut line_str = String::new();
        for span in &l.spans {
            let mut style_prefix = String::new();
            if let Some(fg) = span.style.fg {
                match fg {
                    Color::Rgb(r, g, b) => style_prefix.push_str(&format!("\x1b[38;2;{r};{g};{b}m")),
                    Color::Red => style_prefix.push_str("\x1b[31m"),
                    Color::Green => style_prefix.push_str("\x1b[32m"),
                    _ => {}
                }
            }
            if let Some(bg) = span.style.bg {
                if let Color::Rgb(r, g, b) = bg {
                    style_prefix.push_str(&format!("\x1b[48;2;{r};{g};{b}m"));
                }
            }
            if span.style.add_modifier.contains(Modifier::BOLD) {
                style_prefix.push_str("\x1b[1m");
            }
            if span.style.add_modifier.contains(Modifier::DIM) {
                style_prefix.push_str("\x1b[2m");
            }
            if span.style.add_modifier.contains(Modifier::ITALIC) {
                style_prefix.push_str("\x1b[3m");
            }
            if style_prefix.is_empty() {
                line_str.push_str(&span.content);
            } else {
                line_str.push_str(&format!("{style_prefix}{}\x1b[0m", span.content));
            }
        }
        out.push_str(&format!("{line_str}\n"));
    }
    out
}

/// Plain ANSI string formatting for tool output lines.
pub fn format_tool_result_plain(tool_name: &str, output: &str, is_error: bool) -> String {
    format_tool_result_plain_with_context(tool_name, None, output, is_error, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_render_unified_diff() {
        let diff = r#"--- test.rs
+++ test.rs
@@ -10,4 +10,4 @@
  let x = 1;
- let y = 2;
+ let y = 3;
  let z = 4;
"#;
        let hunks = parse_diff_hunks(diff);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].len(), 4);
        assert_eq!(hunks[0][0].lo, 10);
        assert_eq!(hunks[0][0].ln, 10);
        assert_eq!(hunks[0][1].tag, DiffTag::Delete);
        assert_eq!(hunks[0][1].lo, 11);
        assert_eq!(hunks[0][2].tag, DiffTag::Insert);
        assert_eq!(hunks[0][2].ln, 11);

        let lines = render_diff_hunks(&hunks, Some(Path::new("test.rs")));
        assert_eq!(lines.len(), 4);
    }
}
