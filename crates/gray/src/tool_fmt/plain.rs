//! Plain ANSI string formatters (split from `tool_fmt`).

use super::*;

/// Converts a [`Line`] to an ANSI string (single line, no trailing newline).
fn line_to_ansi(l: &Line<'_>) -> String {
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
        if let Some(bg) = span.style.bg
            && let Color::Rgb(r, g, b) = bg
        {
            style_prefix.push_str(&format!("\x1b[48;2;{r};{g};{b}m"));
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

/// Plain ANSI string formatting for one-shot / non-TUI output header.
// Thin wrapper over `format_tool_call_header`; kept as `String`
// because print.rs + repl/mod.rs callers (outside this file) need `String`.
pub fn format_tool_call_header_plain(
    name: &str,
    args: &serde_json::Value,
    cwd: Option<&Path>,
) -> String {
    line_to_ansi(&format_tool_call_header(name, args, cwd))
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
        out.push_str(&line_to_ansi(&l));
        out.push('\n');
    }
    out
}

/// Plain ANSI string formatting for tool output lines.
pub fn format_tool_result_plain(tool_name: &str, output: &str, is_error: bool) -> String {
    format_tool_result_plain_with_context(tool_name, None, output, is_error, None)
}
