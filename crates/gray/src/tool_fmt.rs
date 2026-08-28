//! Tool formatting and rendering matching Codex & Pi (prime-agent).
//!
//! Provides rich, clean terminal display for tool calls (bash, write, edit, read,
//! grep, find, ls) and their indented command outputs.

use std::path::Path;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

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

/// Formats a tool invocation header line matching Codex/Pi styling for Ratatui.
pub fn format_tool_call_header(name: &str, args: &serde_json::Value, cwd: Option<&Path>) -> Line<'static> {
    let bullet = Span::styled("• ", Style::default().fg(Color::Rgb(74, 222, 128)).add_modifier(Modifier::BOLD));
    let peach = Color::Rgb(246, 173, 126);
    let cyan = Color::Rgb(125, 207, 255);
    let dim = Color::Rgb(140, 140, 140);
    let white = Color::White;

    match name {
        "bash" => {
            let cmd = args.get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            Line::from(vec![
                bullet,
                Span::styled("Ran ", Style::default().fg(white).add_modifier(Modifier::BOLD)),
                Span::styled(cmd.to_string(), Style::default().fg(peach).add_modifier(Modifier::BOLD)),
            ])
        }
        "write" => {
            let raw_path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let path = shorten_path(raw_path, cwd);
            let lines_count = args.get("content")
                .and_then(|v| v.as_str())
                .map(|c| c.lines().count())
                .unwrap_or(0);
            Line::from(vec![
                bullet,
                Span::styled("Wrote ", Style::default().fg(white).add_modifier(Modifier::BOLD)),
                Span::styled(path, Style::default().fg(cyan).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" ({lines_count} lines)"), Style::default().fg(dim)),
            ])
        }
        "edit" => {
            let raw_path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let path = shorten_path(raw_path, cwd);
            Line::from(vec![
                bullet,
                Span::styled("Edited ", Style::default().fg(white).add_modifier(Modifier::BOLD)),
                Span::styled(path, Style::default().fg(cyan).add_modifier(Modifier::BOLD)),
            ])
        }
        "read" => {
            let raw_path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let path = shorten_path(raw_path, cwd);
            let offset = args.get("offset").and_then(|v| v.as_u64());
            let limit = args.get("limit").and_then(|v| v.as_u64());
            let span_detail = match (offset, limit) {
                (Some(o), Some(l)) => format!(" (lines {o}-{})", o + l),
                (Some(o), None) => format!(" (line {o}+)"),
                _ => String::new(),
            };
            Line::from(vec![
                bullet,
                Span::styled("Read ", Style::default().fg(white).add_modifier(Modifier::BOLD)),
                Span::styled(path, Style::default().fg(cyan).add_modifier(Modifier::BOLD)),
                Span::styled(span_detail, Style::default().fg(dim)),
            ])
        }
        "grep" => {
            let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let raw_path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let mut spans = vec![
                bullet,
                Span::styled("Grep ", Style::default().fg(white).add_modifier(Modifier::BOLD)),
                Span::styled(format!("\"{pattern}\""), Style::default().fg(peach).add_modifier(Modifier::BOLD)),
            ];
            if !raw_path.is_empty() && raw_path != "." {
                spans.push(Span::styled(" in ", Style::default().fg(dim)));
                spans.push(Span::styled(shorten_path(raw_path, cwd), Style::default().fg(cyan)));
            }
            Line::from(spans)
        }
        "find" => {
            let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let raw_path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let mut spans = vec![
                bullet,
                Span::styled("Find ", Style::default().fg(white).add_modifier(Modifier::BOLD)),
                Span::styled(format!("\"{pattern}\""), Style::default().fg(peach).add_modifier(Modifier::BOLD)),
            ];
            if !raw_path.is_empty() && raw_path != "." {
                spans.push(Span::styled(" in ", Style::default().fg(dim)));
                spans.push(Span::styled(shorten_path(raw_path, cwd), Style::default().fg(cyan)));
            }
            Line::from(spans)
        }
        "ls" => {
            let raw_path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            Line::from(vec![
                bullet,
                Span::styled("List ", Style::default().fg(white).add_modifier(Modifier::BOLD)),
                Span::styled(shorten_path(raw_path, cwd), Style::default().fg(cyan).add_modifier(Modifier::BOLD)),
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
                Span::styled(other.to_string(), Style::default().fg(white).add_modifier(Modifier::BOLD)),
                Span::raw(" "),
                Span::styled(args_preview, Style::default().fg(dim)),
            ])
        }
    }
}

fn diff_lines(output: &str, is_error: bool) -> Vec<Line<'static>> {
    let pipe_dim = Color::Rgb(110, 110, 110);
    let text_dim = Color::Rgb(160, 160, 160);
    let add_bg = Color::Rgb(22, 55, 22);
    let add_fg = Color::Rgb(120, 220, 120);
    let del_bg = Color::Rgb(55, 22, 22);
    let del_fg = Color::Rgb(240, 120, 120);
    let hunk_fg = Color::Rgb(125, 207, 255);
    let mut lines = Vec::new();
    let mut in_diff = false;
    for raw in output.lines() {
        if is_error {
            lines.push(Line::from(vec![Span::styled(format!("  {raw}"), Style::default().fg(Color::Rgb(239, 68, 68)))]));
            continue;
        }
        if raw.starts_with("@@ ") {
            in_diff = true;
            lines.push(Line::from(vec![Span::styled(format!("  {raw}"), Style::default().fg(hunk_fg).add_modifier(Modifier::DIM))]));
        } else if in_diff && raw.starts_with('+') && !raw.starts_with("+++") {
            lines.push(Line::from(vec![Span::styled(format!("  {raw}"), Style::default().fg(add_fg).bg(add_bg))]));
        } else if in_diff && raw.starts_with('-') && !raw.starts_with("---") {
            lines.push(Line::from(vec![Span::styled(format!("  {raw}"), Style::default().fg(del_fg).bg(del_bg))]));
        } else if in_diff && (raw.starts_with("--- ") || raw.starts_with("+++ ")) {
            lines.push(Line::from(vec![Span::styled(format!("  {raw}"), Style::default().fg(pipe_dim).add_modifier(Modifier::DIM))]));
        } else if in_diff && raw.starts_with(' ') {
            lines.push(Line::from(vec![Span::styled(format!("  {raw}"), Style::default().fg(text_dim))]));
        } else {
            in_diff = false;
            if raw.trim().is_empty() { continue; }
            lines.push(Line::from(vec![Span::styled(format!("  └ {}", raw), Style::default().fg(text_dim))]));
        }
    }
    lines
}

/// Formats tool output lines with Codex/Pi angle-pipe `  └ ` indentation and ellipsis for Ratatui.
pub fn format_tool_result_lines(tool_name: &str, output: &str, is_error: bool) -> Vec<Line<'static>> {
    if is_error {
        let trimmed = output.trim();
        if trimmed.is_empty() { return Vec::new(); }
        let mut lines = Vec::new();
        for (i, l) in trimmed.lines().take(8).enumerate() {
            let prefix = if i == 0 { "  ✗ " } else { "    " };
            lines.push(Line::from(vec![
                Span::styled(prefix, Style::default().fg(Color::Rgb(239, 68, 68)).add_modifier(Modifier::BOLD)),
                Span::styled((*l).to_string(), Style::default().fg(Color::Rgb(239, 68, 68))),
            ]));
        }
        return lines;
    }
    if output.contains("@@ ") || output.contains("\n+ ") || output.contains("\n- ") {
        return diff_lines(output, false);
    }
    let show_output = matches!(tool_name, "bash" | "grep" | "find" | "ls" | "edit" | "write");
    if !show_output { return Vec::new(); }
    let trimmed = output.trim();
    if trimmed.is_empty() { return Vec::new(); }
    const MAX_HEAD_LINES: usize = 6;
    const MAX_TAIL_LINES: usize = 4;
    const MAX_TOTAL_LINES: usize = MAX_HEAD_LINES + MAX_TAIL_LINES + 2;
    let raw_lines: Vec<&str> = trimmed.lines().collect();
    let total = raw_lines.len();
    let text_dim = Color::Rgb(160, 160, 160);
    let pipe_dim = Color::Rgb(110, 110, 110);
    let mut lines = Vec::new();
    if total <= MAX_TOTAL_LINES {
        for (i, l) in raw_lines.iter().enumerate() {
            let prefix = if i == 0 { "  └ " } else { "    " };
            lines.push(Line::from(vec![Span::styled(prefix, Style::default().fg(pipe_dim)), Span::styled((*l).to_string(), Style::default().fg(text_dim))]));
        }
    } else {
        for (i, l) in raw_lines.iter().take(MAX_HEAD_LINES).enumerate() {
            let prefix = if i == 0 { "  └ " } else { "    " };
            lines.push(Line::from(vec![Span::styled(prefix, Style::default().fg(pipe_dim)), Span::styled((*l).to_string(), Style::default().fg(text_dim))]));
        }
        let omitted = total.saturating_sub(MAX_HEAD_LINES + MAX_TAIL_LINES);
        lines.push(Line::from(vec![Span::styled(format!("    … +{omitted} lines"), Style::default().fg(pipe_dim).add_modifier(Modifier::ITALIC))]));
        for l in raw_lines.iter().skip(total - MAX_TAIL_LINES) {
            lines.push(Line::from(vec![Span::styled("    ", Style::default().fg(pipe_dim)), Span::styled((*l).to_string(), Style::default().fg(text_dim))]));
        }
    }
    lines
}

/// Plain ANSI string formatting for one-shot / non-TUI output.
pub fn format_tool_call_header_plain(name: &str, args: &serde_json::Value, cwd: Option<&Path>) -> String {
    let bullet = "\x1b[32m\x1b[1m•\x1b[0m";
    let peach = "\x1b[38;2;246;173;126m\x1b[1m";
    let cyan = "\x1b[38;2;125;207;255m\x1b[1m";
    let dim = "\x1b[2m";
    let bold = "\x1b[1m";
    let reset = "\x1b[0m";

    match name {
        "bash" => {
            let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("").trim();
            format!("{bullet} {bold}Ran{reset} {peach}{cmd}{reset}")
        }
        "write" => {
            let raw_path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let path = shorten_path(raw_path, cwd);
            let lines_count = args.get("content").and_then(|v| v.as_str()).map(|c| c.lines().count()).unwrap_or(0);
            format!("{bullet} {bold}Wrote{reset} {cyan}{path}{reset} {dim}({lines_count} lines){reset}")
        }
        "edit" => {
            let raw_path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let path = shorten_path(raw_path, cwd);
            format!("{bullet} {bold}Edited{reset} {cyan}{path}{reset}")
        }
        "read" => {
            let raw_path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let path = shorten_path(raw_path, cwd);
            format!("{bullet} {bold}Read{reset} {cyan}{path}{reset}")
        }
        "grep" => {
            let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let raw_path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let in_clause = if !raw_path.is_empty() && raw_path != "." {
                format!(" {dim}in{reset} {cyan}{}{reset}", shorten_path(raw_path, cwd))
            } else {
                String::new()
            };
            format!("{bullet} {bold}Grep{reset} {peach}\"{pattern}\"{reset}{in_clause}")
        }
        "find" => {
            let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let raw_path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let in_clause = if !raw_path.is_empty() && raw_path != "." {
                format!(" {dim}in{reset} {cyan}{}{reset}", shorten_path(raw_path, cwd))
            } else {
                String::new()
            };
            format!("{bullet} {bold}Find{reset} {peach}\"{pattern}\"{reset}{in_clause}")
        }
        "ls" => {
            let raw_path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            format!("{bullet} {bold}List{reset} {cyan}{}{reset}", shorten_path(raw_path, cwd))
        }
        other => {
            format!("{bullet} {bold}{other}{reset}")
        }
    }
}

/// Plain ANSI string formatting for tool output lines.
pub fn format_tool_result_plain(tool_name: &str, output: &str, is_error: bool) -> String {
    let lines = format_tool_result_lines(tool_name, output, is_error);
    if lines.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for l in lines {
        let mut line_str = String::new();
        for span in &l.spans {
            let fg = span.style.fg.map(|c| match c {
                Color::Rgb(r, g, b) => format!("\x1b[38;2;{r};{g};{b}m"),
                Color::Red => "\x1b[31m".to_string(),
                _ => String::new(),
            }).unwrap_or_default();
            let bg = span.style.bg.map(|c| match c {
                Color::Rgb(r, g, b) => format!("\x1b[48;2;{r};{g};{b}m"),
                _ => String::new(),
            }).unwrap_or_default();
            line_str.push_str(&format!("{fg}{bg}{}\x1b[0m", span.content));
        }
        out.push_str(&format!("{line_str}\n"));
    }
    out
}
