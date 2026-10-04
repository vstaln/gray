//! Tool formatting and rendering matching Grok CLI (GrokNight theme).
//!
//! Provides rich, clean terminal display for tool calls (bash, write, edit, read,
//! grep, find, ls) with Grok-styled diff rendering and syntax highlighting.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::path::Path;

// ── Palette (active theme) ─────────────────────────────────────────────────
// GrokNight/TokyoNight heritage: green bullet, orange paths, yellow commands.
// Call sites read the shared [`crate::theme::theme()`] palette directly.

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

/// First line of a command or arg preview, capped at 80 display cells.
///
/// The cap is counted in cells and cut on a char boundary, never in raw
/// bytes: `line.len()` is a byte count and `&line[..80]` panics the moment
/// a multi-byte char straddles offset 80 — which is exactly what a pasted
/// review's `\u{1f7e0}` marker did to a live REPL (byte index 80 inside the
/// 4-byte emoji). ASCII text cuts at exactly 80, as before; wide text now
/// fills the same 80 cells instead of being cut a quarter of the way in.
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

/// Live header for a still-streaming tool call (pi `renderCall` on partial
/// args): renders the same header as [`format_tool_call_header`] once
/// `args_so_far` parses as JSON, else streams the in-progress scalar
/// (`command` / `path` / `pattern` / …) raw. Char-safe throughout: partial
/// tails decode `char`-wise (a multibyte split mid-chunk can never panic)
/// and the 9000-char cap falls back to the name-only line, so `truncate_cmd`
/// never byte-slices attacker-shaped partial text (it cuts on a char
/// boundary at 80 cells — see its doc comment).
pub fn format_live_tool_header(name: &str, args_so_far: &str, cwd: Option<&Path>) -> Line<'static> {
    const RAW_CAP: usize = 9000;
    let trimmed = args_so_far.trim();
    if trimmed.is_empty() {
        return tool_name_line(name);
    }
    // Fast path: complete JSON renders exactly like the final header.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return format_tool_call_header(name, &v, cwd);
    }
    // Slow path: stream the in-progress scalar, tolerating unclosed quotes
    // and trailing escapes. Stays a dumb string scan (no new deps).
    match extract_partial_scalar(name, trimmed, RAW_CAP) {
        Some(partial) => format_tool_call_header(name, &partial, cwd),
        None => tool_name_line(name),
    }
}

/// Bare `⬡ name` line for empty/garbage partial args (pi `renderCall` with
/// `undefined` args: `formatShellCall` shows the prompt + `...`).
/// Wire-encoded names decode here too (`web_search` renders `Web Search`);
/// single tokens stay raw (see `humanize_tool_name`).
fn tool_name_line(name: &str) -> Line<'static> {
    let name = humanize_tool_name(name);
    Line::from(vec![
        Span::styled(
            "\u{2b22} ",
            Style::default()
                .fg(crate::theme::theme().tool_accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            name.to_string(),
            Style::default()
                .fg(crate::theme::theme().text_body)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

/// Best-effort scalar for the tool's headline arg straight from the raw
/// partial-JSON buffer. Finds the key's opening quote, then decodes
/// `\"`-aware up to the buffer end; trailing backslash (split escape) is
/// dropped. `None` when the key/value isn't started yet. Bounded: values
/// past RAW_CAP chars fall back to the name-only line, and only the key
/// region is scanned, so per-delta cost is O(key + value), never O(buffer).
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

/// Headline key per tool: `path` for file tools, `pattern` for search
/// tools, `skill`'s `name` handled by the caller. Unknown tools reuse the
/// same guess so `format_tool_call_header`'s `other` arm renders `k=v`.
fn scalar_key(name: &str, raw: &str) -> &'static str {
    match name {
        "read" | "write" | "edit" | "ls" => "path",
        "grep" | "find" => "pattern",
        "web_search" => "query",
        "web_fetch" => "url",
        _ => {
            // key order decides, not a schema table. `path` wins
            // on ties (the final header's `other` arm prefers it too).
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

/// Byte offset of the string value's first content char after
/// `"key" : "`, or `None` if the key/opening quote hasn't streamed yet.
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

/// Decodes a partial JSON string body (opening quote already consumed):
/// `\"` stays a literal quote, `\\` a backslash, `\n`/`\t` their
/// control chars; unknown escapes keep the escaped char. A trailing lone
/// `\\` is a split escape — dropped. Over-cap values return `None`
/// (caller falls back to the name-only line).
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

/// First non-empty string at a plugin-declared dot path (`"document.title"`).
/// Core walks object keys only: no array indices, no wildcards, no schema
/// knowledge. Anything missing, non-string, or whitespace-only is `None`,
/// so a surface that declares a path for a shape it never sends degrades
/// to the built-in preview instead of an error.
pub fn preview_at(args: &serde_json::Value, path: &str) -> Option<String> {
    let mut cur = args;
    for key in path.split('.').map(str::trim).filter(|k| !k.is_empty()) {
        cur = cur.as_object()?.get(key)?;
    }
    cur.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Args with a display-only `preview` path injected (plugin `preview`
/// support). Same contract as [`with_tool_label`]: renderers read `preview`
/// in the `other` arm, sub-renderers ignore it, callers pass wire args.
pub fn with_tool_preview(args: &serde_json::Value, preview: Option<&str>) -> serde_json::Value {
    let Some(preview) = preview.map(str::trim).filter(|t| !t.is_empty()) else {
        return args.clone();
    };
    match args {
        serde_json::Value::Object(map) => {
            let mut map = map.clone();
            map.entry("preview".to_string())
                .or_insert(serde_json::Value::String(preview.to_string()));
            serde_json::Value::Object(map)
        }
        _ => args.clone(),
    }
}

/// Args with a display-only `label` injected (plugin `label` support).
/// Renderers read `label` in the `other` arm; sub-renderers
/// (`arg_path`, previews) ignore unknown keys, so injection is safe.
/// Callers pass the wire args; this stays a pure view helper.
pub fn with_tool_label(args: &serde_json::Value, label: Option<&str>) -> serde_json::Value {
    let Some(label) = label.map(str::trim).filter(|t| !t.is_empty()) else {
        return args.clone();
    };
    match args {
        serde_json::Value::Object(map) => {
            let mut map = map.clone();
            map.entry("label".to_string())
                .or_insert(serde_json::Value::String(label.to_string()));
            serde_json::Value::Object(map)
        }
        _ => args.clone(),
    }
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
            .fg(crate::theme::theme().tool_accent)
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

    match name {
        "bash" => bash_header(args, bullet, action_style, cmd_style, dim_style),
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
        "web_fetch" => {
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let cut = truncate_cmd(url.trim());
            Line::from(vec![
                bullet,
                Span::styled("Fetched ", action_style),
                Span::styled(cut.to_string(), path_style),
            ])
        }
        other => {
            // `label` is display-only: a plugin may pass one inside args
            // (see `Agent::with_tool_labels`) so transcripts name the tool
            // while the wire name stays the executor key. Multi-word wire
            // names humanize (`my_tool` renders `My Tool`); single tokens
            // stay raw.
            let headline = args
                .get("label")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| humanize_tool_name(other));
            if let Some(preview_path) = args
                .get("preview")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|t| !t.is_empty())
                && let Some(text) = preview_at(args, preview_path)
            {
                let cut = truncate_cmd(&text);
                let shown = if cut.len() < text.len() {
                    format!("{}\u{2026}", cut.trim_end())
                } else {
                    cut.to_string()
                };
                return Line::from(vec![
                    bullet,
                    Span::styled(headline.clone(), action_style),
                    Span::raw(" "),
                    Span::styled(format!("\"{shown}\""), cmd_style),
                ]);
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
                let args_preview = if let Some(obj) = args.as_object() {
                    obj.iter()
                        .filter(|(k, _)| *k != "label" && *k != "preview")
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
                    Span::styled(headline, action_style),
                    Span::raw(" "),
                    Span::styled(preview_truncated.to_string(), dim_style),
                ])
            }
        }
    }
}

/// The `bash` header names what the call did, not just "Ran": one tool runs
/// commands and manages their background jobs (`action`), so a job check
/// reads `Waited on cargo-check`, never an empty `Ran`. Commands show
/// without their setup (`cd … && nice … flock …`, see
/// [`gray_tools::shell::label`]); a peeled `cd` stays visible, dimmed.
fn bash_header(
    args: &serde_json::Value,
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
            let cut = truncate_cmd(core.command);
            // A cut command says so: `… | sort |` alone reads as a broken pipe.
            let cmd = if cut.len() < core.command.len() || full.lines().nth(1).is_some() {
                format!("{}\u{2026}", cut.trim_end())
            } else {
                cut.to_string()
            };
            let background = args.get("background").and_then(|v| v.as_bool()) == Some(true);
            let mut spans = vec![
                bullet,
                verb(if background { "Started " } else { "Ran " }),
                Span::styled(cmd, cmd_style),
            ];
            if let Some(dir) = core.cwd {
                spans.push(Span::styled(format!(" \u{00b7} in {dir}"), dim_style));
            }
            if background {
                spans.push(Span::styled(" \u{00b7} in background", dim_style));
            }
            Line::from(spans)
        }
    }
}

/// The verb a running `bash` header shimmers instead of its finished one.
pub(crate) fn live_bash_verb(done: &str) -> Option<&'static str> {
    Some(match done {
        "Ran " => "Running ",
        "Started " => "Starting ",
        "Waited on " => "Waiting on ",
        "Checked " => "Checking ",
        "Stopped " => "Stopping ",
        "Listed " => "Listing ",
        _ => return None,
    })
}

/// A job as people read it: its name (`cargo-check`). Ids from before jobs
/// had names (`bash-<32 hex>`) shrink to `job bbc7b0`.
fn job_label(id: &str) -> String {
    match id.strip_prefix("bash-") {
        Some(hex) if hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit()) => {
            format!("job {}", &hex[..6])
        }
        _ => id.to_string(),
    }
}

/// `900` → `15m`, `75` → `1m 15s`, `30` → `30s`.
fn human_secs(secs: u64) -> String {
    match (secs / 3600, (secs % 3600) / 60, secs % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, 0) => format!("{m}m"),
        (0, m, s) => format!("{m}m {s}s"),
        (h, 0, _) => format!("{h}h"),
        (h, m, _) => format!("{h}h {m}m"),
    }
}

/// Rewrite the `bash` tool's job notices for people, display only. They are
/// written for the model (`still running · job … · yielded after 10s`, then
/// how to await it); the card says what is happening instead. Only lines
/// outside the `<untrusted-output>` fence are touched: command output is
/// shown exactly as it came.
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
        // `job cargo-check` above a finished result: the header names it.
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
            // `exit 0 (\`head\` masks earlier stages' exit; rerun …)`: keep
            // the fact, drop the instruction.
            let tail = &line[cut + "; rerun without the pipe to check".len()..];
            out.push(format!("{}{tail}", &line[..cut]));
        } else if t == "Partial output (snapshot):" {
            out.push("output so far:".to_string());
        } else if let Some(rest) = t.strip_prefix("job ")
            && rest.contains(" \u{00b7} ")
        {
            // A status row: `job X · running · elapsed 1m 3s · log …`.
            out.push(drop_log_field(&rest.replace("elapsed ", "")));
        } else {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

/// Lines that tell the model what to do next; people never need them.
const MODEL_HINTS: &[&str] = &[
    "Continue other work",
    "Moved to a background job",
    "Not killed",
];

/// `cargo-check · yielded after 10s (duration limit, not a stall) ·
/// timeout 900s · log …` → `running in background as cargo-check · after
/// 10s · limit 15m`.
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

/// Humanizes a wire name for the transcript (`web_search` renders `Web Search`).
/// Known tools get verb arms above; this is the fallback so a new plugin
/// never renders as raw snake_case. Only wire-encoded names (with `_`/`-`)
/// are decoded — a single token (`bash`, `custom`) is already displayable
/// and stays byte-identical, so live and final headers agree with history.
/// Single pass, no allocs beyond output.
pub(crate) fn humanize_tool_name(name: &str) -> String {
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

/// Tools whose success results can render a body in the TUI (diffs, code,
/// command output). Everything else (skill, read, …) renders header-only,
/// so the REPL keeps the styled result card instead of a naked live line.
pub fn tool_may_render_body(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "bash" | "grep" | "find" | "ls" | "edit" | "write" | "web_search" | "web_fetch"
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
    // The header's `· log <path>` names the raw log for the model (it can
    // page it back); on a card it is a long path on every run. Drop that
    // field from the display only, keeping any field after it.
    // The header is line 1, or line 2 under a `cancelled by user` row.
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
        // Same margins as a numbered output row (`  ` + `  1 | `): the
        // mark sits in the number column, the text where output text starts.
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

    let trimmed = if tool_name == "bash" {
        strip_shell_fence(&humanize_bash_notices(output.trim()))
    } else {
        strip_shell_fence(output.trim())
    };
    let mut rows = if trimmed.is_empty() {
        Vec::new()
    } else {
        // Cap display like code blocks (40-line threshold → 18 head + 6 tail):
        // full output stays in model context, TUI only renders a window.
        // reuse render_code_block cap, no new collapsing system.
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
