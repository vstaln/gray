//! Tool formatting and rendering matching Grok CLI (GrokNight theme).
//!
//! Provides rich, clean terminal display for tool calls (bash, write, edit, read,
//! grep, find, ls) with Grok-styled diff rendering and syntax highlighting.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::path::Path;

// ── Palette (active theme) ─────────────────────────────────────────────────
// GrokNight/TokyoNight heritage: green bullet, orange paths, yellow commands.
// These read the shared [`UiTheme`] palette (single gray theme).
// The palette seeds today's exact values, so output is
// pixel-identical until the user switches.
pub fn accent_tool() -> Color {
    crate::theme::theme().tool_accent
}
pub fn text_primary() -> Color {
    crate::theme::theme().text_body
}
pub fn path_color() -> Color {
    crate::theme::theme().tool_path
}
pub fn command_color() -> Color {
    crate::theme::theme().tool_command
}
pub fn dim_color() -> Color {
    crate::theme::theme().tool_dim
}

pub fn diff_delete_bg() -> Color {
    crate::theme::theme().diff_del_bg
}
pub fn diff_delete_fg() -> Color {
    crate::theme::theme().diff_del_fg
}
pub fn diff_insert_bg() -> Color {
    crate::theme::theme().diff_add_bg
}
pub fn diff_insert_fg() -> Color {
    crate::theme::theme().diff_add_fg
}
pub fn diff_equal_fg() -> Color {
    crate::theme::theme().text_body
}
pub fn diff_gutter_fg() -> Color {
    crate::theme::theme().diff_gutter
}

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

/// Expands tabs to 4-space stops so all characters are explicit printable
/// spaces and background styling covers every cell.
pub fn expand_tabs(s: &str) -> String {
    const TAB_SIZE: usize = 4;
    if !s.contains('\t') {
        return s.to_string();
    }
    let mut result = String::with_capacity(s.len() + 16);
    let mut col = 0;
    for ch in s.chars() {
        if ch == '\t' {
            let count = TAB_SIZE.saturating_sub(col % TAB_SIZE).max(1);
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

/// Heredoc hidden in a bash command (`cat <<'EOF' > path` … `EOF`):
/// returns the body plus the redirect target, if any, so replay shows the
/// code gray wrote or ran (bash-only harness: the `write`-tool arm never
/// fires, and the one-line header plus trivial stdout would hide it).
/// First heredoc wins; unclosed or empty bodies yield None.
fn heredoc_body(command: &str) -> Option<(String, Option<String>)> {
    let mut lines = command.lines();
    let mut opener = None;
    for line in lines.by_ref() {
        if line.contains("<<") {
            opener = Some(line.to_string());
            break;
        }
    }
    let opener = opener?;
    let after = opener.split_once("<<")?.1.trim();
    let after = after.strip_prefix('-').unwrap_or(after).trim();
    let delim = after
        .split_whitespace()
        .next()
        .map(|t| t.trim_matches(|c| c == '\'' || c == '"'))
        .filter(|d| !d.is_empty())?;
    let target = opener
        .split('>')
        .skip(1)
        .filter_map(|t| {
            t.trim_start_matches('>')
                .split_whitespace()
                .next()
                .map(|s| s.trim_matches(|c| c == '\'' || c == '"').to_string())
        })
        .find(|t| !t.is_empty() && !t.starts_with('&'));
    let mut body: Vec<&str> = Vec::new();
    for line in lines {
        if line.trim() == delim {
            return if body.is_empty() {
                None
            } else {
                Some((body.join("\n"), target))
            };
        }
        body.push(line);
    }
    None
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
            .fg(accent_tool())
            .add_modifier(Modifier::BOLD),
    );
    let action_style = Style::default()
        .fg(text_primary())
        .add_modifier(Modifier::BOLD);
    let path_style = Style::default()
        .fg(path_color())
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let cmd_style = Style::default()
        .fg(command_color())
        .add_modifier(Modifier::BOLD);
    let dim_style = Style::default().fg(dim_color());

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
        "skill" => {
            let skill_name = skill_display_name(args);
            Line::from(vec![
                bullet,
                Span::styled("Skill ", action_style),
                Span::styled(format!("\"{skill_name}\""), cmd_style),
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
    render_numbered_lines(&content.lines().collect::<Vec<_>>(), &mut highlighter)
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
    let expanded = expand_tabs(text);
    let indent_count = expanded.chars().take_while(|c| *c == ' ').count();
    let cont_indent_len = indent_count.min(content_w / 2);
    let cont_indent_str = " ".repeat(cont_indent_len);

    let row_spans = highlight_line_spans(&expanded, highlighter, syntect, None);
    let wrapped_rows = wrap_styled_spans(row_spans, content_w, cont_indent_len);

    let gutter_str = format!("{:>width$} | ", line_num, width = gutter_width);
    let cont_gutter_str = format!("{:>width$} | ", "", width = gutter_width);

    for (ci, chunk_spans) in wrapped_rows.into_iter().enumerate() {
        let mut spans = Vec::new();
        spans.push(Span::raw("  "));
        if ci == 0 {
            spans.push(Span::styled(
                gutter_str.clone(),
                Style::default().fg(diff_gutter_fg()),
            ));
        } else {
            spans.push(Span::styled(
                cont_gutter_str.clone(),
                Style::default().fg(diff_gutter_fg()),
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
/// Above the cap, keeps head/tail with an omission marker.
fn render_numbered_lines(
    raw_lines: &[&str],
    highlighter: &mut Option<gray_markdown::syntect::easy::HighlightLines<'_>>,
) -> Vec<Line<'static>> {
    const MAX_LINES_TO_SHOW: usize = 40;
    const HEAD: usize = 18;
    const TAIL: usize = 6;
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

    if total > MAX_LINES_TO_SHOW {
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
        let gutter_pad = " ".repeat(gutter_width + 3);
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::raw(gutter_pad),
            Span::styled(
                format!("… +{omitted} lines"),
                Style::default()
                    .fg(dim_color())
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

/// Tools whose success results can render a body in the TUI (diffs, code,
/// command output). Everything else (skill, read, …) renders header-only,
/// so the REPL keeps the styled result card instead of a naked live line.
pub fn tool_may_render_body(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "bash" | "grep" | "find" | "ls" | "edit" | "write"
    )
}

/// Strip the `<untrusted-output>` shell fence for *display* only.
///
/// `bash` wraps process output in the fence so the model can
/// tell tool output from user text (prompt-injection boundary). The tags are
/// harness plumbing: the transcript keeps them, but rendering them as
/// numbered output lines confuses humans.
///
/// Real shell output is `header + fence(body)` — e.g. `exit 0 …` on line 1,
/// `<untrusted-output …>` on line 2, body, `</untrusted-output>`, then an
/// optional trailer (`…more available…`, `for the rest`, wake text, …).
/// Strip the first opener near the top and the last closer near the bottom,
/// independently — a budget-truncated body may carry only one half. Body
/// closers are escaped as `<\\/…`, so an exact `</untrusted-output>` match
/// is the real fence; opener matches are position-guarded so a body line
/// that happens to look like a tag is left alone.
fn strip_shell_fence(trimmed: &str) -> String {
    let mut lines: Vec<&str> = trimmed.lines().collect();
    if let Some(idx) = lines
        .iter()
        .position(|l| l.trim_start().starts_with("<untrusted-output"))
        && idx <= 3
    {
        lines.remove(idx);
    }
    if let Some(idx) = lines.iter().rposition(|l| {
        let t = l.trim();
        t == "</untrusted-output>" || t == "<\\/untrusted-output>"
    }) && lines.len().saturating_sub(idx) <= 5
    {
        lines.remove(idx);
    }
    lines
        .join("\n")
        .replace("<\\/untrusted-output>", "</untrusted-output>")
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
                        .fg(diff_delete_fg())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled((*l).to_string(), Style::default().fg(diff_delete_fg())),
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
        // Diff-like output was already rendered (or discarded) above; a write
        // reaching here displays the written code block with line numbers.
        let content = args.map(arg_content).unwrap_or("");
        if !content.is_empty() {
            return render_code_block(content, file_path);
        }
        return Vec::new();
    }

    if !tool_may_render_body(tool_name) {
        return Vec::new();
    }

    let trimmed = strip_shell_fence(output.trim());
    let mut rows = if trimmed.is_empty() {
        Vec::new()
    } else {
        // Cap display like code blocks (40-line threshold → 18 head + 6 tail):
        // full output stays in model context, TUI only renders a window.
        // ponytail: reuse render_code_block cap, no new collapsing system.
        let (pretty, token) = prettify_output(&trimmed);
        let syntect = gray_markdown::get_syntect();
        let mut highlighter = token.and_then(|t| syntect.highlight_lines_for_token(t));
        render_numbered_lines(&pretty.lines().collect::<Vec<_>>(), &mut highlighter)
    };
    // Bash-only harness: file writes arrive as heredocs, whose bodies the
    // one-line header never shows — render them like the `write` arm does
    // (same cap), ahead of the command output.
    if tool_name == "bash"
        && let Some(cmd) = args.and_then(|a| a.get("command")).and_then(|v| v.as_str())
        && let Some((body, target)) = heredoc_body(cmd)
    {
        let hp = target.map(|t| match cwd {
            Some(c) => c.join(&t),
            None => std::path::PathBuf::from(&t),
        });
        let mut code = render_code_block(&body, hp.as_deref());
        code.append(&mut rows);
        rows = code;
    }
    rows
}

mod plain;

pub use plain::{format_tool_call_header_plain, format_tool_result_plain_with_context};

#[path = "mod_tests.rs"]
#[cfg(test)]
mod tests;
