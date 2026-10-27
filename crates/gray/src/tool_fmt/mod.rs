use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::path::Path;

fn arg_path(args: &serde_json::Value) -> &str {
    args.get("path")
        .or_else(|| args.get("file_path"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

fn arg_content(args: &serde_json::Value) -> &str {
    args.get("content")
        .or_else(|| args.get("contents"))
        .or_else(|| args.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

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

    let chars: Vec<char> = rel.chars().collect();
    let tail_len = 49;
    let head_len = MAX - 1 - tail_len;
    let head: String = chars[..head_len].iter().collect();
    let tail: String = chars[chars.len() - tail_len..].iter().collect();
    format!("{head}…{tail}")
}

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

fn strip_markup(s: &str) -> String {
    let mut out = s.to_string();
    for (open, close) in [("**", "**"), ("__", "__"), ("~~", "~~")] {
        while let Some(a) = out.find(open) {
            let after = a + open.len();
            let Some(rel) = out[after..].find(close) else {
                break;
            };
            let b = after + rel;
            let inner = out[after..b].to_string();
            if inner.trim().is_empty() {
                break;
            }
            out.replace_range(a..b + close.len(), &inner);
        }
    }

    let mut res = String::with_capacity(out.len());
    let mut chars = out.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '`' {
            if chars.peek() == Some(&'`') {
                res.push('`');
                chars.next();
            }
            continue;
        }
        res.push(ch);
    }
    res
}

fn truncate_cmd(cmd: &str) -> &str {
    let line = cmd.lines().next().unwrap_or(cmd);
    if crate::text_width::display_width(line) <= 80 {
        return line;
    }
    let mut width = 0;
    let mut end = line.len();
    for (idx, ch) in line.char_indices() {
        let w = crate::text_width::char_width(ch);
        if width + w > 80 {
            end = idx;
            break;
        }
        width += w;
    }
    &line[..end]
}

pub fn format_live_tool_header(
    name: &str,
    args_so_far: &str,
    cwd: Option<&Path>,
    label: Option<&str>,
    preview: Option<&str>,
) -> Vec<Line<'static>> {
    const RAW_CAP: usize = 9000;
    let trimmed = args_so_far.trim();
    if trimmed.is_empty() {
        return vec![tool_name_line(name, label)];
    }

    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return format_tool_card_lines(name, &v, cwd, label, preview);
    }

    match extract_partial_scalar(name, trimmed, RAW_CAP) {
        Some(partial) => format_tool_card_lines(name, &partial, cwd, label, preview),
        None => vec![tool_name_line(name, label)],
    }
}

fn tool_hue(name: &str) -> Color {
    let t = crate::theme::theme();
    match name {
        "bash" | "shell" => t.hue_green,
        "read" | "view" | "ls" | "glob" | "grep" | "find" | "search" | "file_search" => t.hue_blue,
        "write" | "edit" | "patch" | "apply_patch" | "edit_patch" => t.hue_yellow,
        "web_search" | "web_fetch" => t.hue_magenta,
        "task" | "agent" | "subagent" | "todo" | "todo_write" | "task_create" | "task_list"
        | "task_get" | "task_update" => t.hue_cyan,
        _ => t.tool_accent,
    }
}

fn tool_name_line(name: &str, label: Option<&str>) -> Line<'static> {
    let hue = tool_hue(name);
    let name = match label.map(str::trim).filter(|t| !t.is_empty()) {
        Some(label) => label.to_string(),
        None => humanize_tool_name(name),
    };
    Line::from(vec![
        Span::styled(
            format!("{} ", crate::tui::glyph("\u{2b22}", "#")),
            Style::default().fg(hue).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            name.to_string(),
            Style::default()
                .fg(crate::theme::theme().text_body)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

fn extract_partial_scalar(name: &str, raw: &str, cap: usize) -> Option<serde_json::Value> {
    let key = match name {
        "bash" => "command",
        "skill" => "name",
        _ => scalar_key(name, raw),
    };
    let from = find_key_value_start(raw, key)?;
    let (value, _closed) = decode_partial_string(&raw[from..], cap)?;
    let mut obj = serde_json::Map::with_capacity(1);
    obj.insert(key.to_string(), serde_json::Value::String(value));
    Some(serde_json::Value::Object(obj))
}

fn scalar_key(name: &str, raw: &str) -> &'static str {
    match name {
        "read" | "write" | "edit" | "ls" => "path",
        "grep" | "find" => "pattern",
        "web_search" => "query",
        "web_fetch" => "url",
        _ => {
            let path_pos = raw.find("\"path\"").map(|i| (i, "path"));
            let pattern_pos = raw.find("\"pattern\"").map(|i| (i, "pattern"));
            let command_pos = raw.find("\"command\"").map(|i| (i, "command"));
            [path_pos, pattern_pos, command_pos]
                .into_iter()
                .flatten()
                .min_by_key(|(i, _)| *i)
                .map(|(_, k)| k)
                .unwrap_or("path")
        }
    }
}

fn find_key_value_start(raw: &str, key: &str) -> Option<usize> {
    let quoted = format!("\"{key}\"");
    let mut search = raw;
    let mut base = 0usize;
    loop {
        let rel = search.find(quoted.as_str())?;
        let mut i = base + rel + quoted.len();
        let b = raw.as_bytes();
        while i < b.len() && (b[i] == b' ' || b[i] == b'\t' || b[i] == b'\n' || b[i] == b'\r') {
            i += 1;
        }
        if i < b.len() && b[i] == b':' {
            i += 1;
            while i < b.len() && (b[i] == b' ' || b[i] == b'\t' || b[i] == b'\n' || b[i] == b'\r') {
                i += 1;
            }
            if i < b.len() && b[i] == b'"' {
                return Some(i + 1);
            }
            return None;
        }
        base += rel + quoted.len();
        search = &raw[base..];
    }
}

fn decode_partial_string(raw: &str, cap: usize) -> Option<(String, bool)> {
    let mut out = String::new();
    let mut chars = raw.chars();
    let mut closed = false;
    while let Some(c) = chars.next() {
        if c == '"' {
            closed = true;
            break;
        }
        if c != '\\' {
            out.push(c);
        } else {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some(other) => out.push(other),
                None => break,
            }
        }
        if out.chars().count() > cap {
            return None;
        }
    }
    if out.is_empty() && !closed {
        return None;
    }
    Some((out, closed))
}

/// The part of a bash command the result card shows under its header.
///
/// The header holds only the first line, cut at 80 columns, so anything past
/// that has to appear in the card or it never reaches the transcript at all.
/// A command that is exactly one heredoc (`cat > f <<'EOF'` … `EOF`) shows just
/// its body, highlighted for the target file, because the header already
/// carries the opener and the delimiter adds nothing. Every other command the
/// header can't hold whole is shown in full.
enum CommandBody {
    Heredoc {
        body: String,
        target: Option<String>,
    },
    Script(String),
}

fn command_body(command: &str) -> Option<CommandBody> {
    let full = command.trim();
    if full.is_empty() || gray_tools::shell::label::read_only(full).is_some() {
        return None;
    }
    let core = gray_tools::shell::label::core_command(full).command;
    let header_fits = truncate_cmd(core).len() == core.len();
    if !full.contains('\n') {
        return (!header_fits).then(|| CommandBody::Script(full.to_string()));
    }
    if header_fits && let Some((body, target)) = sole_heredoc(full) {
        return Some(CommandBody::Heredoc { body, target });
    }
    Some(CommandBody::Script(full.to_string()))
}

/// Body and redirect target of a command made of nothing but one heredoc: the
/// opener on the first line, then the body, then the delimiter as the last line.
fn sole_heredoc(command: &str) -> Option<(String, Option<String>)> {
    let mut lines = command.lines();
    let opener = lines.next()?;
    if opener.matches("<<").count() != 1 || opener.contains("<<<") {
        return None;
    }
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
    let body: Vec<&str> = lines.collect();
    let (last, body) = body.split_last()?;
    if last.trim() != delim || body.is_empty() || body.iter().any(|l| l.trim() == delim) {
        return None;
    }
    Some((body.join("\n"), target))
}

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

pub fn preview_at(args: &serde_json::Value, spec: &str) -> Option<String> {
    if !is_template(spec) {
        return dot_path_at(args, spec);
    }
    let (head, detail) = render_template(args, spec);
    let parts: Vec<String> = std::iter::once(head)
        .chain(detail)
        .filter(|s| !s.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join(PREVIEW_SEP))
}

fn dot_path_at(args: &serde_json::Value, path: &str) -> Option<String> {
    let mut cur = args;
    for key in path.split('.').map(str::trim).filter(|k| !k.is_empty()) {
        cur = cur.as_object()?.get(key)?;
    }
    cur.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

const PREVIEW_SEP: &str = " \u{b7} ";

fn is_template(spec: &str) -> bool {
    spec.contains('{')
}

fn render_template(args: &serde_json::Value, spec: &str) -> (String, Option<String>) {
    let (first, second) = split_template_lines(spec);
    let head = render_segments(args, first);
    let detail = second
        .map(|line| render_segments(args, &line))
        .filter(|s| !s.is_empty());
    (head, detail)
}

fn split_template_lines(spec: &str) -> (&str, Option<String>) {
    let real = spec.find('\n').map(|at| (at, 1));
    let escaped = spec.find("\\n").map(|at| (at, 2));
    let cut = match (real, escaped) {
        (Some(r), Some(e)) => Some(if r.0 < e.0 { r } else { e }),
        (r, e) => r.or(e),
    };
    match cut {
        None => (spec, None),
        Some((at, len)) => {
            let rest = spec[at + len..].replace("\\n", " ").replace('\n', " ");
            (&spec[..at], Some(rest))
        }
    }
}

fn render_segments(args: &serde_json::Value, line: &str) -> String {
    line.split(PREVIEW_SEP)
        .filter_map(|segment| render_segment(args, segment))
        .collect::<Vec<_>>()
        .join(PREVIEW_SEP)
}

fn render_segment(args: &serde_json::Value, segment: &str) -> Option<String> {
    let mut out = String::new();
    let mut placeholders = 0usize;
    let mut filled = false;
    let mut rest = segment;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open + 1..].find('}').map(|c| open + 1 + c) else {
            break;
        };
        out.push_str(&rest[..open]);
        let text = placeholder_text(args, &rest[open + 1..close]);
        filled |= !text.is_empty();
        out.push_str(&text);
        placeholders += 1;
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    if placeholders > 0 && !filled {
        return None;
    }
    let out = out.trim();
    (!out.is_empty()).then(|| out.to_string())
}

fn placeholder_text(args: &serde_json::Value, inner: &str) -> String {
    let (path, filter) = match inner.split_once('|') {
        Some((path, filter)) => (path, filter.trim()),
        None => (inner, ""),
    };
    let raw = value_text(args, path);
    match filter {
        "model" => model_display_name(&one_line(&raw)),
        "firstline" => raw
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(ellipsize_cells)
            .unwrap_or_default(),
        _ => one_line(&raw),
    }
}

fn value_text(args: &serde_json::Value, path: &str) -> String {
    let mut cur = args;
    for key in path.split('.').map(str::trim).filter(|k| !k.is_empty()) {
        match cur.as_object().and_then(|obj| obj.get(key)) {
            Some(next) => cur = next,
            None => return String::new(),
        }
    }
    match cur {
        serde_json::Value::String(s) => s.trim().to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(scalar_item_text)
            .collect::<Vec<_>>()
            .join(", "),
        serde_json::Value::Null | serde_json::Value::Object(_) => String::new(),
    }
}

fn scalar_item_text(item: &serde_json::Value) -> Option<String> {
    let text = match item {
        serde_json::Value::String(s) => one_line(s),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        _ => return None,
    };
    (!text.is_empty()).then_some(text)
}

fn one_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn ellipsize_cells(text: &str) -> String {
    let cut = truncate_cmd(text);
    if cut.len() < text.len() {
        format!("{}\u{2026}", cut.trim_end())
    } else {
        cut.to_string()
    }
}

fn model_display_name(id: &str) -> String {
    let id = id.trim();
    let friendly = crate::setup::friendly_model_name(id);
    let short = friendly
        .strip_prefix("Claude ")
        .unwrap_or(friendly.as_str())
        .trim();
    if short.is_empty() {
        id.to_string()
    } else {
        short.to_string()
    }
}

fn template_header_lines(
    bullet: Span<'static>,
    headline: String,
    args: &serde_json::Value,
    template: &str,
    action_style: Style,
    dim_style: Style,
) -> Vec<Line<'static>> {
    let indent = " ".repeat(crate::text_width::display_width(&bullet.content));
    let (head, detail) = render_template(args, template);
    let mut first = vec![bullet, Span::styled(headline, action_style)];
    if !head.is_empty() {
        first.push(Span::raw(" "));
        first.push(Span::styled(ellipsize_cells(&head), dim_style));
    }
    let mut lines = vec![Line::from(first)];
    if let Some(detail) = detail {
        lines.push(Line::from(vec![
            Span::raw(indent),
            Span::styled(ellipsize_cells(&detail), dim_style),
        ]));
    }
    lines
}

pub fn with_tool_label(args: &serde_json::Value, label: Option<&str>) -> serde_json::Value {
    let Some(label) = label.map(str::trim).filter(|t| !t.is_empty()) else {
        return args.clone();
    };
    match args {
        serde_json::Value::Object(map) => {
            let mut map = map.clone();
            map.insert(
                "label".to_string(),
                serde_json::Value::String(label.to_string()),
            );
            serde_json::Value::Object(map)
        }
        _ => args.clone(),
    }
}

#[derive(Clone, Copy, Default)]
pub struct ToolDisplay<'a> {
    pub labels: Option<&'a std::collections::HashMap<String, String>>,
    pub previews: Option<&'a std::collections::HashMap<String, String>>,
}

impl<'a> ToolDisplay<'a> {
    pub fn label(self, name: &str) -> Option<&'a str> {
        self.labels.and_then(|m| m.get(name)).map(String::as_str)
    }

    pub fn preview(self, name: &str) -> Option<&'a str> {
        self.previews.and_then(|m| m.get(name)).map(String::as_str)
    }
}

pub fn format_tool_card_lines(
    name: &str,
    args: &serde_json::Value,
    cwd: Option<&Path>,
    label: Option<&str>,
    preview: Option<&str>,
) -> Vec<Line<'static>> {
    format_tool_call_lines(name, &with_tool_label(args, label), cwd, preview)
}

pub fn format_tool_call_lines(
    name: &str,
    args: &serde_json::Value,
    cwd: Option<&Path>,
    preview_path: Option<&str>,
) -> Vec<Line<'static>> {
    let bullet = Span::styled(
        format!("{} ", crate::tui::glyph("\u{2b22}", "#")),
        Style::default()
            .fg(tool_hue(name))
            .add_modifier(Modifier::BOLD),
    );
    let action_style = Style::default()
        .fg(crate::theme::theme().text_body)
        .add_modifier(Modifier::BOLD);
    let path_style = Style::default()
        .fg(crate::theme::theme().tool_path)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let cmd_style = Style::default()
        .fg(crate::theme::theme().tool_command)
        .add_modifier(Modifier::BOLD);
    let dim_style = Style::default().fg(crate::theme::theme().tool_dim);

    let head = match name {
        "bash" => bash_header(args, cwd, bullet, action_style, cmd_style, dim_style),
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
        "web_search" => {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            let cut = truncate_cmd(query);
            let shown = if cut.len() < query.len() {
                format!("{}{}", cut.trim_end(), "…")
            } else {
                cut.to_string()
            };
            Line::from(vec![
                bullet,
                Span::styled("Searched ", action_style),
                Span::styled(format!("\"{shown}\""), cmd_style),
            ])
        }
        "request_user_input" => {
            let qs: Vec<&str> = args
                .get("questions")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|q| q.get("question").and_then(|v| v.as_str()))
                        .map(str::trim)
                        .filter(|q| !q.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let mut spans = vec![bullet, Span::styled("Asked", action_style)];
            if let Some(first) = qs.first() {
                let cut = truncate_cmd(first);
                let shown = if cut.len() < first.len() {
                    format!("{}\u{2026}", cut.trim_end())
                } else {
                    cut.to_string()
                };
                spans.push(Span::raw(" "));
                spans.push(Span::styled(shown, cmd_style));
            }
            if qs.len() > 1 {
                spans.push(Span::styled(format!(" +{}", qs.len() - 1), dim_style));
            }
            Line::from(spans)
        }
        "web_fetch" => {
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let cut = truncate_cmd(url.trim());
            Line::from(vec![
                bullet,
                Span::styled("Fetched ", action_style),
                Span::styled(cut.to_string(), path_style),
            ])
        }
        "discord_send" => {
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();

            let plain = strip_markup(content);
            let cut = truncate_cmd(&plain);
            let shown = if cut.len() < plain.len() {
                format!("{}{}", cut.trim_end(), "…")
            } else {
                cut.to_string()
            };
            Line::from(vec![
                bullet,
                Span::styled("Sent Discord ", action_style),
                Span::styled(format!("\"{shown}\""), cmd_style),
            ])
        }
        "discord_send_ui" => {
            Line::from(vec![bullet, Span::styled("Sent Discord UI", action_style)])
        }
        "discord_open_modal" => Line::from(vec![
            bullet,
            Span::styled("Opened Discord modal", action_style),
        ]),
        "discord_file" => {
            let desc = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
            let target = args
                .get("path")
                .or_else(|| args.get("file_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let mut spans = vec![bullet, Span::styled("Shared Discord file", action_style)];
            if !target.is_empty() {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(shorten_path(target.trim(), cwd), path_style));
            } else if !desc.is_empty() {
                spans.push(Span::styled(format!(" ({desc})"), dim_style));
            }
            Line::from(spans)
        }
        "discord_ui_schema" => Line::from(vec![
            bullet,
            Span::styled("Read Discord UI schema", action_style),
        ]),
        n if mcp_parts(n).is_some() => {
            let (server, tool) = mcp_parts(n).unwrap_or_default();
            let mut spans = vec![
                bullet,
                Span::styled("MCP ", action_style),
                Span::styled(server.to_string(), cmd_style),
                Span::styled(format!(" {}", tool.replace(['_', '-'], " ")), action_style),
            ];
            let preview = mcp_arg_preview(args);
            if !preview.is_empty() {
                spans.push(Span::styled(format!(" \u{b7} {preview}"), dim_style));
            }
            Line::from(spans)
        }
        other => {
            let headline = args
                .get("label")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| humanize_tool_name(other));

            let declared = preview_path.map(str::trim).filter(|t| !t.is_empty());
            if let Some(template) = declared.filter(|t| is_template(t)) {
                return template_header_lines(
                    bullet,
                    headline,
                    args,
                    template,
                    action_style,
                    dim_style,
                );
            }
            if let Some(preview_path) = declared
                && let Some(text) = preview_at(args, preview_path)
            {
                let cut = truncate_cmd(&text);
                let shown = if cut.len() < text.len() {
                    format!("{}\u{2026}", cut.trim_end())
                } else {
                    cut.to_string()
                };
                return vec![Line::from(vec![
                    bullet,
                    Span::styled(headline.clone(), action_style),
                    Span::raw(" "),
                    Span::styled(format!("\"{shown}\""), cmd_style),
                ])];
            }
            let path = shorten_path(arg_path(args), cwd);
            if !path.is_empty() {
                Line::from(vec![
                    bullet,
                    Span::styled(headline.clone(), action_style),
                    Span::raw(" "),
                    Span::styled(path, path_style),
                ])
            } else {
                let obj_args: Vec<(&String, &serde_json::Value)> = args
                    .as_object()
                    .map(|obj| obj.iter().filter(|(k, _)| *k != "label").collect())
                    .unwrap_or_default();
                let headline_idx = obj_args
                    .iter()
                    .position(|(_, v)| v.as_str().map(str::trim).is_some_and(|s| !s.is_empty()));
                let mut spans = vec![bullet, Span::styled(headline.clone(), action_style)];
                if let Some(i) = headline_idx {
                    let text = obj_args[i].1.as_str().unwrap().trim();
                    let cut = truncate_cmd(text);
                    let shown = if cut.len() < text.len() {
                        format!("{}\u{2026}", cut.trim_end())
                    } else {
                        cut.to_string()
                    };
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(format!("\"{shown}\""), cmd_style));
                }
                let flags: Vec<String> = obj_args
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| Some(*i) != headline_idx)
                    .take(3)
                    .map(|(_, (k, v))| match v {
                        serde_json::Value::Bool(true) => (*k).clone(),
                        serde_json::Value::String(s) => {
                            let s = s.trim();
                            let cut = truncate_cmd(s);
                            if s.contains(char::is_whitespace) {
                                format!("{k}=\"{}\"", cut.trim_end())
                            } else {
                                format!("{k}={cut}")
                            }
                        }
                        serde_json::Value::Array(a) => format!("{k}=[{} items]", a.len()),
                        serde_json::Value::Object(_) => format!("{k}={{...}}"),
                        other => format!("{k}={other}"),
                    })
                    .collect();
                if !flags.is_empty() {
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(flags.join(" "), dim_style));
                }
                Line::from(spans)
            }
        }
    };
    vec![head]
}

pub fn format_tool_call_header(
    name: &str,
    args: &serde_json::Value,
    cwd: Option<&Path>,
    preview_path: Option<&str>,
) -> Line<'static> {
    format_tool_call_lines(name, args, cwd, preview_path)
        .into_iter()
        .next()
        .unwrap_or_default()
}

fn bash_header(
    args: &serde_json::Value,
    cwd: Option<&Path>,
    bullet: Span<'static>,
    action_style: Style,
    cmd_style: Style,
    dim_style: Style,
) -> Line<'static> {
    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("run");
    let job = args
        .get("job_id")
        .and_then(|v| v.as_str())
        .map(job_label)
        .unwrap_or_else(|| "job".to_string());
    let waits = args
        .get("wait_ms")
        .and_then(|v| v.as_u64())
        .filter(|ms| *ms > 0);
    let verb = |v: &'static str| Span::styled(v, action_style);
    match action {
        "output" | "status" => {
            let mut spans = vec![
                bullet,
                verb(if waits.is_some() {
                    "Waited on "
                } else {
                    "Checked "
                }),
                Span::styled(job, cmd_style),
            ];
            if let Some(ms) = waits {
                spans.push(Span::styled(
                    format!(" \u{00b7} up to {}", human_secs(ms.div_ceil(1000))),
                    dim_style,
                ));
            }
            Line::from(spans)
        }
        "cancel" => Line::from(vec![bullet, verb("Stopped "), Span::styled(job, cmd_style)]),
        "list" => Line::from(vec![
            bullet,
            verb("Listed "),
            Span::styled("background jobs", dim_style),
        ]),
        _ => {
            let full = args
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            let core = gray_tools::shell::label::core_command(full);
            let background = args.get("background").and_then(|v| v.as_bool()) == Some(true);

            if !background && let Some(read) = gray_tools::shell::label::read_only(full) {
                return read_header(
                    &read,
                    core.cwd,
                    cwd,
                    bullet,
                    action_style,
                    cmd_style,
                    dim_style,
                );
            }
            let cut = truncate_cmd(core.command);

            let cmd = if cut.len() < core.command.len() || full.lines().nth(1).is_some() {
                format!("{}\u{2026}", cut.trim_end())
            } else {
                cut.to_string()
            };
            let mut spans = vec![
                bullet,
                verb(if background { "Started " } else { "Ran " }),
                Span::styled(cmd, cmd_style),
            ];
            if let Some(dir) = core.cwd.filter(|d| !d.trim().is_empty()) {
                spans.push(Span::styled(format!(" \u{00b7} {dir}"), dim_style));
            }
            if background {
                spans.push(Span::styled(" \u{00b7} in background", dim_style));
            }
            Line::from(spans)
        }
    }
}

fn read_header(
    read: &gray_tools::shell::label::ReadOnly,
    peeled_cd: Option<&str>,
    cwd: Option<&Path>,
    bullet: Span<'static>,
    action_style: Style,
    cmd_style: Style,
    dim_style: Style,
) -> Line<'static> {
    use gray_tools::shell::label::Look;
    let verb = match read.look {
        Look::Viewed => "Viewed ",
        Look::Read => "Read ",
        Look::Listed => "Listed ",
        Look::Searched => "Searched ",
    };
    let target = match read.look {
        Look::Searched => read.targets.join(" "),
        _ => read
            .targets
            .iter()
            .map(|t| shorten_path(t, cwd))
            .collect::<Vec<_>>()
            .join(", "),
    };
    let cut = truncate_cmd(&target);
    let target = if cut.len() < target.len() {
        format!("{}\u{2026}", cut.trim_end())
    } else {
        target
    };
    let mut spans = vec![
        bullet,
        Span::styled(verb, action_style),
        Span::styled(target, cmd_style),
    ];
    if let Some(detail) = &read.detail {
        spans.push(Span::styled(format!(" \u{00b7} {detail}"), dim_style));
    }
    if let Some(dir) = peeled_cd.filter(|d| !d.trim().is_empty()) {
        spans.push(Span::styled(format!(" \u{00b7} {dir}"), dim_style));
    }
    Line::from(spans)
}

pub(crate) fn live_bash_verb(done: &str) -> Option<&'static str> {
    Some(match done {
        "Ran " => "Running ",
        "Skipped " => "Skipping ",
        "Started " => "Starting ",
        "Waited on " => "Waiting on ",
        "Checked " => "Checking ",
        "Stopped " => "Stopping ",
        "Listed " => "Listing ",
        "Viewed " => "Viewing ",
        "Read " => "Reading ",
        "Searched " => "Searching ",
        _ => return None,
    })
}

fn job_label(id: &str) -> String {
    match id.strip_prefix("bash-") {
        Some(hex) if hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit()) => {
            format!("job {}", &hex[..6])
        }
        _ => id.to_string(),
    }
}

fn human_secs(secs: u64) -> String {
    match (secs / 3600, (secs % 3600) / 60, secs % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, 0) => format!("{m}m"),
        (0, m, s) => format!("{m}m {s}s"),
        (h, 0, _) => format!("{h}h"),
        (h, m, _) => format!("{h}h {m}m"),
    }
}

fn humanize_bash_notices(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let open = lines
        .iter()
        .position(|l| l.trim_start().starts_with("<untrusted-output"));
    let close = lines.iter().rposition(|l| {
        let t = l.trim();
        t == "</untrusted-output>" || t == "<\\/untrusted-output>"
    });
    let in_body = |i: usize| match (open, close) {
        (Some(o), Some(c)) => i > o && i < c,
        (Some(o), None) => i > o,
        _ => false,
    };
    let more_after = |i: usize| lines.len() > i + 1;
    let mut out: Vec<String> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if in_body(i) || Some(i) == open || Some(i) == close {
            out.push(line.to_string());
            continue;
        }
        let t = line.trim();

        if i == 0 && more_after(i) && t.starts_with("job ") && t.split_whitespace().count() == 2 {
            continue;
        }
        if MODEL_HINTS.iter().any(|h| t.starts_with(h)) {
            continue;
        }
        if let Some(rest) = t.strip_prefix("still running \u{00b7} job ") {
            out.push(running_line(rest));
        } else if let Some(rest) = t.strip_prefix("Liveness: ") {
            out.push(rest.split(" \u{2014} ").next().unwrap_or(rest).to_string());
        } else if let Some(rest) = t.strip_prefix("cancellation requested for job ") {
            out.push(format!(
                "stopping {}",
                rest.split(';').next().unwrap_or(rest)
            ));
        } else if let Some(cut) = line.find("; rerun without the pipe to check") {
            let tail = &line[cut + "; rerun without the pipe to check".len()..];
            out.push(format!("{}{tail}", &line[..cut]));
        } else if t == "Partial output (snapshot):" {
            out.push("output so far:".to_string());
        } else if let Some(rest) = t.strip_prefix("job ")
            && rest.contains(" \u{00b7} ")
        {
            out.push(drop_log_field(&rest.replace("elapsed ", "")));
        } else {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

const MODEL_HINTS: &[&str] = &[
    "Continue other work",
    "Moved to a background job",
    "Not killed",
];

fn running_line(fields: &str) -> String {
    let mut parts = fields.split(" \u{00b7} ");
    let id = job_label(parts.next().unwrap_or("job"));
    let mut out = format!("running in background as {id}");
    for field in parts {
        if let Some(after) = field.strip_prefix("yielded after ") {
            let after = after.split(" (").next().unwrap_or(after);
            out.push_str(&format!(" \u{00b7} after {after}"));
        } else if let Some(quiet) = field.strip_prefix("silent: no new output for ") {
            out.push_str(&format!(" \u{00b7} quiet for {quiet}"));
        } else if let Some(limit) = field
            .strip_prefix("timeout ")
            .and_then(|t| t.strip_suffix('s'))
            .and_then(|t| t.parse::<u64>().ok())
        {
            out.push_str(&format!(" \u{00b7} limit {}", human_secs(limit)));
        }
    }
    out
}

/// `mcp__<server>__<tool>` -> `(server, tool)`.
pub(crate) fn mcp_parts(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    (!server.is_empty() && !tool.is_empty()).then_some((server, tool))
}

/// One-line preview of an MCP call's arguments: the first non-empty
/// string value (query, url, id, ...), else compact JSON.
fn mcp_arg_preview(args: &serde_json::Value) -> String {
    let first = args.as_object().and_then(|m| {
        m.values()
            .filter_map(|v| v.as_str())
            .map(str::trim)
            .find(|s| !s.is_empty())
            .map(|s| s.lines().next().unwrap_or(s).to_string())
    });
    let text = match first {
        Some(t) => t,
        None => match args.as_object() {
            Some(m) if m.is_empty() => return String::new(),
            None => return String::new(),
            _ => serde_json::to_string(args).unwrap_or_default(),
        },
    };
    let cut = truncate_cmd(&text);
    if cut.len() < text.len() {
        format!("{}\u{2026}", cut.trim_end())
    } else {
        cut.to_string()
    }
}

pub(crate) fn humanize_tool_name(name: &str) -> String {
    if let Some((server, tool)) = mcp_parts(name) {
        return format!("{server} \u{b7} {}", tool.replace(['_', '-'], " "));
    }
    if !name.contains('_') && !name.contains('-') {
        return name.to_string();
    }
    let mut out = String::with_capacity(name.len());
    let mut capitalize = true;
    for ch in name.chars() {
        if ch == '_' || ch == '-' {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
            capitalize = true;
        } else if capitalize {
            out.extend(ch.to_uppercase());
            capitalize = false;
        } else {
            out.push(ch);
        }
    }
    if out.is_empty() {
        name.to_string()
    } else {
        out
    }
}
mod diff;

pub use diff::{DiffHunk, DiffLine, DiffTag, parse_diff_hunks, render_diff_hunks};
pub(crate) use diff::{highlight_line_spans, wrap_styled_spans};

pub fn render_code_block(content: &str, path: Option<&Path>) -> Vec<Line<'static>> {
    let syntect = gray_markdown::get_syntect();
    let mut highlighter = path.and_then(|p| syntect.highlight_lines_by_file_path(p));
    render_numbered_lines(&content.lines().collect::<Vec<_>>(), &mut highlighter)
}

fn prettify_output(trimmed: &str) -> (String, Option<&'static str>) {
    let head = trimmed
        .trim_start()
        .get(..9)
        .unwrap_or(trimmed.trim_start())
        .to_ascii_lowercase();
    if head.starts_with("<!doctype") || head.starts_with("<html") {
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
                Style::default().fg(crate::theme::theme().diff_gutter),
            ));
        } else {
            spans.push(Span::styled(
                cont_gutter_str.clone(),
                Style::default().fg(crate::theme::theme().diff_gutter),
            ));
            if cont_indent_len > 0 {
                spans.push(Span::raw(cont_indent_str.clone()));
            }
        }
        spans.extend(chunk_spans);
        lines.push(Line::from(spans));
    }
}

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
                    .fg(crate::theme::theme().tool_dim)
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

/// Whether a tool's result text gets a numbered body under its card.
///
/// Every tool does (core, plugin and MCP alike) except the few whose output is
/// either already on screen or too bulky to be useful: file contents from
/// `read`, a skill's prompt text, the user's own `request_user_input` answer,
/// and the Discord UI schema reference.
pub fn tool_may_render_body(tool_name: &str) -> bool {
    !matches!(
        tool_name,
        "read" | "skill" | "request_user_input" | "discord_ui_schema"
    )
}

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

    let mut owned: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    for l in owned.iter_mut().take(2) {
        *l = drop_log_field(l);
    }
    owned
        .join("\n")
        .replace("<\\/untrusted-output>", "</untrusted-output>")
}

fn drop_log_field(header: &str) -> String {
    const SEP: &str = " \u{00b7} ";
    let Some(start) = header.find(" \u{00b7} log ") else {
        return header.to_string();
    };
    let rest = &header[start + SEP.len()..];
    match rest.find(SEP) {
        Some(end) => format!("{}{}", &header[..start], &rest[end..]),
        None => header[..start].to_string(),
    }
}

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
            let prefix = if i == 0 {
                "    \u{2717}   "
            } else {
                "        "
            };
            lines.push(Line::from(vec![
                Span::styled(
                    prefix,
                    Style::default()
                        .fg(crate::theme::theme().diff_del_fg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    (*l).to_string(),
                    Style::default().fg(crate::theme::theme().diff_del_fg),
                ),
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

    if tool_name == "edit" || output.starts_with("--- ") || output.contains("@@ ") {
        let hunks = parse_diff_hunks(output);
        if !hunks.is_empty() {
            return render_diff_hunks(&hunks, file_path, cwd);
        }
        return Vec::new();
    }

    if tool_name == "write" {
        let content = args.map(arg_content).unwrap_or("");
        if !content.is_empty() {
            return render_code_block(content, file_path);
        }
        return Vec::new();
    }

    if !tool_may_render_body(tool_name) {
        return Vec::new();
    }

    let trimmed = if tool_name == "bash" {
        let output = drop_view_echo(args, output.trim());
        strip_shell_fence(&humanize_bash_notices(output))
    } else {
        strip_shell_fence(output.trim())
    };
    let mut rows = if trimmed.is_empty() {
        Vec::new()
    } else if let Some(text) = web::web_body_text(tool_name, &trimmed) {
        render_numbered_lines(&text.lines().collect::<Vec<_>>(), &mut None)
    } else {
        let (pretty, token) = prettify_output(&trimmed);
        let syntect = gray_markdown::get_syntect();
        let mut highlighter = token.and_then(|t| syntect.highlight_lines_for_token(t));
        render_numbered_lines(&pretty.lines().collect::<Vec<_>>(), &mut highlighter)
    };

    if tool_name == "bash"
        && let Some(cmd) = args.and_then(|a| a.get("command")).and_then(|v| v.as_str())
        && let Some(shown) = command_body(cmd)
    {
        let mut code = match shown {
            CommandBody::Heredoc { body, target } => {
                let hp = target.map(|t| match cwd {
                    Some(c) => c.join(&t),
                    None => std::path::PathBuf::from(&t),
                });
                render_code_block(&body, hp.as_deref())
            }
            CommandBody::Script(script) => {
                let syntect = gray_markdown::get_syntect();
                let mut highlighter = syntect.highlight_lines_for_token("bash");
                render_numbered_lines(&script.lines().collect::<Vec<_>>(), &mut highlighter)
            }
        };
        code.append(&mut rows);
        rows = code;
    }
    rows
}

fn drop_view_echo<'a>(args: Option<&serde_json::Value>, output: &'a str) -> &'a str {
    let viewed = args
        .and_then(|a| a.get("command"))
        .and_then(|v| v.as_str())
        .and_then(gray_tools::shell::label::read_only)
        .is_some_and(|r| r.look == gray_tools::shell::label::Look::Viewed);
    let (first, rest) = output.split_once('\n').unwrap_or((output, ""));
    if viewed
        && first.starts_with("Shown: ")
        && !first.contains("; skipped:")
        && !first.contains("(showing first")
    {
        rest.trim_start()
    } else {
        output
    }
}

mod plain;
mod web;

pub use plain::{
    format_tool_call_header_plain, format_tool_call_lines_plain,
    format_tool_result_plain_with_context,
};

#[cfg(test)]
mod body_tests;
