use super::*;

fn line_to_ansi(l: &Line<'_>) -> String {
    let mut line_str = String::new();
    for span in &l.spans {
        let mut style_prefix = String::new();
        if let Some(fg) = span.style.fg {
            let fg = gray_markdown::adapt_ratatui_color(fg);
            if fg != Color::Reset {
                style_prefix.push_str(&crate::tui::sgr_fg(fg));
            }
        }
        if let Some(bg) = span.style.bg {
            let bg = gray_markdown::adapt_ratatui_color(bg);
            if bg != Color::Reset {
                style_prefix.push_str(&crate::tui::sgr_bg(bg));
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
    line_str
}

pub fn format_tool_call_header_plain(
    name: &str,
    args: &serde_json::Value,
    cwd: Option<&Path>,
    preview_path: Option<&str>,
) -> String {
    line_to_ansi(&format_tool_call_header(name, args, cwd, preview_path))
}

pub fn format_tool_call_lines_plain(
    name: &str,
    args: &serde_json::Value,
    cwd: Option<&Path>,
    preview_path: Option<&str>,
) -> String {
    format_tool_call_lines(name, args, cwd, preview_path)
        .iter()
        .map(line_to_ansi)
        .collect::<Vec<_>>()
        .join("\n")
}

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
        out.push_str(&line_to_ansi(&l));
        out.push('\n');
    }
    out
}
