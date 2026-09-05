//! Tool formatting and rendering matching Grok CLI (GrokNight theme).
//!
//! Provides rich, clean terminal display for tool calls (bash, write, edit, read,
//! grep, find, ls) with Grok-styled diff rendering and syntax highlighting.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::path::Path;

// ── Palette (GrokNight Theme) ──────────────────────────────────────────────
pub const ACCENT_TOOL: Color = Color::Rgb(158, 206, 106); // #9ece6a (green bullet)
pub const TEXT_PRIMARY: Color = Color::Rgb(225, 225, 225); // #e1e1e1 (bold white)
pub const PATH_COLOR: Color = Color::Rgb(255, 158, 100); // #ff9e64 (TokyoNight orange)
pub const COMMAND_COLOR: Color = Color::Rgb(224, 175, 104); // #e0af68 (TokyoNight yellow)
pub const DIM_COLOR: Color = Color::Rgb(108, 108, 108); // #6c6c6c (muted gray)

pub const DIFF_DELETE_BG: Color = Color::Rgb(55, 25, 28); // #37191c (dark red diff tint)
pub const DIFF_DELETE_FG: Color = Color::Rgb(247, 118, 142); // #f7768e (bright red)
pub const DIFF_INSERT_BG: Color = Color::Rgb(24, 50, 32); // #183220 (dark green diff tint)
pub const DIFF_INSERT_FG: Color = Color::Rgb(158, 206, 106); // #9ece6a (bright green)
pub const DIFF_EQUAL_FG: Color = Color::Rgb(225, 225, 225); // #e1e1e1 (code text)
pub const DIFF_GUTTER_FG: Color = Color::Rgb(108, 108, 108); // #6c6c6c (line numbers)

fn arg_path(args: &serde_json::Value) -> &str {
    // Schemas emit only `path` + `file_path` (write.rs); dropped
    // filePath/TargetFile/targetFile/file/filename/target/destination probes.
    args.get("path")
        .or_else(|| args.get("file_path"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

fn arg_content(args: &serde_json::Value) -> &str {
    // write.rs schema emits content/contents/text; dropped
    // CodeContent/code_content/codeContent/code/body/data probes.
    args.get("content")
        .or_else(|| args.get("contents"))
        .or_else(|| args.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

/// Shortens a path relative to CWD or HOME for compact terminal display.
/// Long paths are middle-truncated (`head…tail`, tail kept longer since the
/// filename matters most) so tool headers stay on one line instead of
/// wrapping across three — fixed 80 cols, not terminal width.
pub fn shorten_path(path_str: &str, cwd: Option<&Path>) -> String {
    let path = Path::new(path_str);
    let mut rel = path_str.to_string();
    if let Some(cwd) = cwd
        && let Ok(stripped) = path.strip_prefix(cwd)
    {
        let s = stripped.display().to_string();
        if !s.is_empty() {
            rel = s;
        }
    }
    if rel == path_str
        && let Ok(home) = std::env::var("HOME")
    {
        let home_path = Path::new(&home);
        if let Ok(stripped) = path.strip_prefix(home_path) {
            rel = format!("~/{}", stripped.display());
        }
    }
    const MAX: usize = 80;
    if rel.chars().count() <= MAX {
        return rel;
    }
    // Char-based split, byte-safe via char indices
    let chars: Vec<char> = rel.chars().collect();
    let tail_len = 49;
    let head_len = MAX - 1 - tail_len;
    let head: String = chars[..head_len].iter().collect();
    let tail: String = chars[chars.len() - tail_len..].iter().collect();
    format!("{head}…{tail}")
}

/// Expands tabs to spaces with a given tab size (default 4) to ensure
/// all characters are explicit printable spaces and background styling covers every cell.
pub fn expand_tabs(s: &str, tab_size: usize) -> String {
    if !s.contains('\t') {
        return s.to_string();
    }
    let mut result = String::with_capacity(s.len() + 16);
    let mut col = 0;
    for ch in s.chars() {
        if ch == '\t' {
            let count = tab_size.saturating_sub(col % tab_size).max(1);
            for _ in 0..count {
                result.push(' ');
            }
            col += count;
        } else {
            result.push(ch);
            col += 1;
        }
    }
    result
}

fn truncate_cmd(cmd: &str) -> &str {
    let line = cmd.lines().next().unwrap_or(cmd);
    if line.len() > 80 { &line[..80] } else { line }
}

/// Resolves the display name for a `skill` tool call, matching opencode's
/// `Skill "name"` header. Prefers explicit `name`/`skill` args, then derives
/// from `path`/`location` (parent dir for SKILL.md, else file stem).
fn skill_display_name(args: &serde_json::Value) -> String {
    if let Some(n) = args
        .get("name")
        .or_else(|| args.get("skill"))
        .and_then(|v| v.as_str())
    {
        let t = n.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    if let Some(p) = args
        .get("path")
        .or_else(|| args.get("location"))
        .and_then(|v| v.as_str())
    {
        let t = p.trim();
        if !t.is_empty() {
            let path = Path::new(t);
            if let Some(fname) = path.file_name().and_then(|n| n.to_str()) {
                if fname == "SKILL.md" {
                    if let Some(parent) = path
                        .parent()
                        .and_then(|p| p.file_name())
                        .and_then(|n| n.to_str())
                    {
                        return parent.to_string();
                    }
                } else if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    return stem.to_string();
                } else {
                    return fname.to_string();
                }
            }
            return t.to_string();
        }
    }
    "skill".to_string()
}

/// Formats a tool invocation header line matching Grok CLI styling for Ratatui.
pub fn format_tool_call_header(
    name: &str,
    args: &serde_json::Value,
    cwd: Option<&Path>,
) -> Line<'static> {
    let bullet = Span::styled(
        "\u{2b22} ",
        Style::default()
            .fg(ACCENT_TOOL)
            .add_modifier(Modifier::BOLD),
    );
    let action_style = Style::default()
        .fg(TEXT_PRIMARY)
        .add_modifier(Modifier::BOLD);
    let path_style = Style::default()
        .fg(PATH_COLOR)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let cmd_style = Style::default()
        .fg(COMMAND_COLOR)
        .add_modifier(Modifier::BOLD);
    let dim_style = Style::default().fg(DIM_COLOR);

    match name {
        "bash" => {
            let cmd = truncate_cmd(
                args.get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim(),
            );
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
        "request_user_input" => {
            let q_summary = args
                .get("questions")
                .and_then(|q| q.as_array())
                .and_then(|arr| {
                    if arr.len() == 1 {
                        arr[0]
                            .get("question")
                            .and_then(|v| v.as_str())
                            .map(|s| format!("\"{s}\""))
                    } else if arr.len() > 1 {
                        Some(format!("{} questions", arr.len()))
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    args.get("question")
                        .and_then(|v| v.as_str())
                        .map(|s| format!("\"{s}\""))
                })
                .unwrap_or_else(|| "question".to_string());
            let summary = truncate_cmd(&q_summary);
            Line::from(vec![
                bullet,
                Span::styled("Asked ", action_style),
                Span::styled(summary.to_string(), cmd_style),
            ])
        }
        "skill" => {
            let skill_name = skill_display_name(args);
            Line::from(vec![
                bullet,
                Span::styled("Skill ", action_style),
                Span::styled(format!("\"{skill_name}\""), cmd_style),
            ])
        }
        "cron" => {
            let sched = args
                .get("cron")
                .or_else(|| args.get("schedule"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Line::from(vec![
                bullet,
                Span::styled("Cron ", action_style),
                Span::styled(sched.to_string(), cmd_style),
            ])
        }
        other => {
            let path = shorten_path(arg_path(args), cwd);
            if !path.is_empty() {
                Line::from(vec![
                    bullet,
                    Span::styled(other.to_string(), action_style),
                    Span::raw(" "),
                    Span::styled(path, path_style),
                ])
            } else {
                let args_preview = if let Some(obj) = args.as_object() {
                    obj.iter()
                        .take(2)
                        .map(|(k, v)| {
                            let val_str = if let Some(s) = v.as_str() {
                                truncate_cmd(s).to_string()
                            } else if let Some(arr) = v.as_array() {
                                format!("[{} items]", arr.len())
                            } else if v.is_object() {
                                "{...}".to_string()
                            } else {
                                v.to_string()
                            };
                            format!("{k}={val_str}")
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                } else {
                    String::new()
                };
                let preview_truncated = truncate_cmd(&args_preview);
                Line::from(vec![
                    bullet,
                    Span::styled(other.to_string(), action_style),
                    Span::raw(" "),
                    Span::styled(preview_truncated.to_string(), dim_style),
                ])
            }
        }
    }
}
mod diff;

pub use diff::{DiffHunk, DiffLine, DiffTag, parse_diff_hunks, render_diff_hunks};
pub(crate) use diff::{highlight_line_spans, wrap_styled_spans};

/// Renders a newly created / written code block with line numbers and syntax highlighting.
pub fn render_code_block(content: &str, path: Option<&Path>) -> Vec<Line<'static>> {
    let syntect = gray_markdown::get_syntect();
    let mut highlighter = path.and_then(|p| syntect.highlight_lines_by_file_path(p));
    render_numbered_lines(
        &content.lines().collect::<Vec<_>>(),
        &mut highlighter,
        Some(40),
    )
}

/// Guesses a syntect language token for command output and lightly
/// pretty-prints it: JSON is reflowed, minified HTML/XML is split one tag
/// per line. Returns (text, token); token is None for plain output.
fn prettify_output(trimmed: &str) -> (String, Option<&'static str>) {
    let head = trimmed
        .trim_start()
        .get(..9)
        .unwrap_or(trimmed.trim_start())
        .to_ascii_lowercase();
    if head.starts_with("<!doctype") || head.starts_with("<html") {
        // Tag-boundary split only (`><`), never touches text content.
        return (trimmed.replace("><", ">\n<"), Some("html"));
    }
    if head.starts_with("<?xml") {
        return (trimmed.replace("><", ">\n<"), Some("xml"));
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed)
        && let Ok(pretty) = serde_json::to_string_pretty(&v)
    {
        return (pretty, Some("json"));
    }
    (trimmed.to_string(), None)
}

fn push_numbered_wrapped(
    lines: &mut Vec<Line<'static>>,
    line_num: usize,
    text: &str,
    highlighter: &mut Option<gray_markdown::syntect::easy::HighlightLines<'_>>,
    gutter_width: usize,
    content_w: usize,
) {
    let syntect = gray_markdown::get_syntect();
    let expanded = expand_tabs(text, 4);
    let indent_count = expanded.chars().take_while(|c| *c == ' ').count();
    let cont_indent_len = indent_count.min(content_w / 2);
    let cont_indent_str = " ".repeat(cont_indent_len);

    let row_spans = highlight_line_spans(&expanded, highlighter, syntect, DIFF_EQUAL_FG, None);
    let wrapped_rows = wrap_styled_spans(row_spans, content_w, cont_indent_len);

    let gutter_str = format!("{:>width$} | ", line_num, width = gutter_width);
    let cont_gutter_str = format!("{:>width$} | ", "", width = gutter_width);

    for (ci, chunk_spans) in wrapped_rows.into_iter().enumerate() {
        let mut spans = Vec::new();
        spans.push(Span::raw("  "));
        if ci == 0 {
            spans.push(Span::styled(
                gutter_str.clone(),
                Style::default().fg(DIFF_GUTTER_FG),
            ));
        } else {
            spans.push(Span::styled(
                cont_gutter_str.clone(),
                Style::default().fg(DIFF_GUTTER_FG),
            ));
            if cont_indent_len > 0 {
                spans.push(Span::raw(cont_indent_str.clone()));
            }
        }
        spans.extend(chunk_spans);
        lines.push(Line::from(spans));
    }
}

/// Numbered, highlighted, indent-wrapped rendering shared by
/// [`render_code_block`] (capped) and command output (uncapped).
/// `max_lines_to_show`: Some(n) keeps head/tail with an omission marker,
/// None shows every line.
fn render_numbered_lines(
    raw_lines: &[&str],
    highlighter: &mut Option<gray_markdown::syntect::easy::HighlightLines<'_>>,
    max_lines_to_show: Option<usize>,
) -> Vec<Line<'static>> {
    let total = raw_lines.len();
    if total == 0 {
        return Vec::new();
    }
    let gutter_width = total.to_string().len().max(3);
    let mut lines = Vec::new();

    let term_w = crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(120)
        .max(60);
    let overhead = 2 + gutter_width + 3 + 2;
    let content_w = term_w.saturating_sub(overhead).max(20);

    if let Some(max) = max_lines_to_show
        && total > max
    {
        const HEAD: usize = 18;
        const TAIL: usize = 6;
        for (idx, line_text) in raw_lines.iter().take(HEAD).enumerate() {
            push_numbered_wrapped(
                &mut lines,
                idx + 1,
                line_text,
                highlighter,
                gutter_width,
                content_w,
            );
        }
        let omitted = total.saturating_sub(HEAD + TAIL);
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("… +{omitted} lines"),
                Style::default()
                    .fg(DIM_COLOR)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]));
        for (idx, line_text) in raw_lines.iter().skip(total - TAIL).enumerate() {
            push_numbered_wrapped(
                &mut lines,
                total - TAIL + idx + 1,
                line_text,
                highlighter,
                gutter_width,
                content_w,
            );
        }
        return lines;
    }
    for (idx, line_text) in raw_lines.iter().enumerate() {
        push_numbered_wrapped(
            &mut lines,
            idx + 1,
            line_text,
            highlighter,
            gutter_width,
            content_w,
        );
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
            let prefix = if i == 0 { " ✗ " } else { "   " };
            lines.push(Line::from(vec![
                Span::styled(
                    prefix,
                    Style::default()
                        .fg(DIFF_DELETE_FG)
                        .add_modifier(Modifier::BOLD),
                ),
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

    if tool_name == "read" || tool_name == "request_user_input" {
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

    // Show the whole output as a code block: numbered gutter, syntax
    // highlighting, indent-aware wrapping. HTML/XML is split one tag per
    // line and JSON is reflowed so minified bodies stay readable.
    let (pretty, token) = prettify_output(trimmed);
    let syntect = gray_markdown::get_syntect();
    let mut highlighter = token.and_then(|t| syntect.highlight_lines_for_token(t));
    render_numbered_lines(&pretty.lines().collect::<Vec<_>>(), &mut highlighter, None)
}

/// Formats tool output lines (convenience wrapper).
pub fn format_tool_result_lines(
    tool_name: &str,
    output: &str,
    is_error: bool,
) -> Vec<Line<'static>> {
    format_tool_result_lines_with_context(tool_name, None, output, is_error, None)
}

mod plain;

pub use plain::{
    format_tool_call_header_plain, format_tool_result_plain, format_tool_result_plain_with_context,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn row_text(l: &Line<'_>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn bash_plain_output_is_numbered() {
        let lines = format_tool_result_lines("bash", "hello\nworld", false);
        assert_eq!(lines.len(), 2);
        assert!(
            row_text(&lines[0]).contains("1 | "),
            "got {:?}",
            row_text(&lines[0])
        );
        assert!(row_text(&lines[0]).contains("hello"));
        assert!(row_text(&lines[1]).contains("2 | "));
    }

    #[test]
    fn bash_empty_output_returns_nothing() {
        assert!(format_tool_result_lines("bash", "   \n  ", false).is_empty());
    }

    #[test]
    fn bash_shows_every_line_uncapped() {
        let out: String = (1..=60)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = format_tool_result_lines("bash", &out, false);
        let first_rows: Vec<String> = lines.iter().map(row_text).collect();
        assert!(
            first_rows.iter().any(|r| r.contains("60 | ")),
            "last gutter missing"
        );
        assert!(
            !first_rows.iter().any(|r| r.contains("… +")),
            "must not truncate"
        );
    }

    #[test]
    fn bash_json_is_pretty_printed() {
        let lines = format_tool_result_lines("bash", r#"{"a":1,"b":[1,2]}"#, false);
        let text: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
        assert!(lines.len() > 1);
        assert!(text.contains("\"a\": 1"), "got {text:?}");
    }

    #[test]
    fn bash_html_is_split_one_tag_per_line() {
        let html = "<!DOCTYPE html><html><head><title>Vercel Security</title></head><body><p>hi</p></body></html>";
        let lines = format_tool_result_lines("bash", html, false);
        assert!(lines.len() > 1);
        for l in &lines {
            assert!(
                !row_text(l).contains("><"),
                "still minified: {:?}",
                row_text(l)
            );
        }
        let text: String = lines.iter().map(row_text).collect::<Vec<_>>().join("\n");
        assert!(text.contains("<title>Vercel Security</title>"));
    }

    #[test]
    fn render_code_block_cap_unchanged() {
        let content: String = (1..=50)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = render_code_block(&content, None);
        // 18 head + 1 omission marker + 6 tail
        assert_eq!(lines.len(), 25);
        assert!(row_text(&lines[18]).contains("… +26 lines"));
    }
}
