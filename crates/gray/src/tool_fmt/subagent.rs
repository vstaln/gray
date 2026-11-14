use ratatui::style::Style;
use ratatui::text::{Line, Span};

const INDENT: &str = "  ";
const DETAIL_INDENT: &str = "    ";
const MODEL_SEP: &str = " \u{b7} ";
const ROW_GAP: &str = "   ";

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChildState {
    Running,
    Queued,
    Completed,
    Failed,
    Stopped,
    Unknown,
}

impl ChildState {
    fn parse(status: &str) -> Self {
        match status
            .split(':')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "running" => ChildState::Running,
            "queued" => ChildState::Queued,
            "completed" => ChildState::Completed,
            "failed" => ChildState::Failed,
            "stopped" | "cancelled" | "canceled" => ChildState::Stopped,
            _ => ChildState::Unknown,
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            ChildState::Running => crate::tui::glyph("\u{25d0}", "o"),
            ChildState::Queued => crate::tui::glyph("\u{25e6}", "."),
            ChildState::Completed => crate::tui::glyph("\u{2713}", "v"),
            ChildState::Failed => crate::tui::glyph("\u{2717}", "x"),
            ChildState::Stopped => crate::tui::glyph("\u{25a0}", "-"),
            ChildState::Unknown => "",
        }
    }

    fn label(self) -> &'static str {
        match self {
            ChildState::Running => "running",
            ChildState::Queued => "queued",
            ChildState::Completed => "completed",
            ChildState::Failed => "failed",
            ChildState::Stopped => "stopped",
            ChildState::Unknown => "",
        }
    }

    fn style(self) -> Style {
        let theme = crate::theme::theme();
        match self {
            ChildState::Running => Style::default().fg(theme.accent),
            ChildState::Queued => Style::default().fg(theme.tool_dim),
            ChildState::Completed => Style::default().fg(theme.success),
            ChildState::Failed => Style::default().fg(theme.error),
            ChildState::Stopped => Style::default().fg(theme.tool_dim),
            ChildState::Unknown => Style::default().fg(theme.tool_dim),
        }
    }
}

pub(super) fn subagent_body(tool_name: &str, output: &str) -> Option<Vec<Line<'static>>> {
    let text = super::strip_shell_fence(output.trim());
    match tool_name {
        "subagent_check" | "rlm_collect" => check_body(&text),
        "subagent_spawn" | "rlm_spawn" => spawn_body(&text),
        "subagent_stop" | "rlm_delete" => Some(Vec::new()),
        _ => None,
    }
}

fn check_body(text: &str) -> Option<Vec<Line<'static>>> {
    let snaps: Vec<serde_json::Value> = serde_json::from_str(text).ok()?;
    let mut lines = Vec::with_capacity(snaps.len() * 2);
    for snap in &snaps {
        let name = snap.get("name").and_then(|v| v.as_str())?.trim();
        if name.is_empty() {
            return None;
        }
        let status = snap
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let preview = snap.get("preview").and_then(|v| v.as_str()).unwrap_or("");
        let model = model_from_preview(preview);
        lines.push(child_row(name, status, model.as_deref()));
        if let Some(detail) = detail_text(snap, preview) {
            lines.push(detail_row(&detail));
        }
    }
    Some(lines)
}

fn child_row(name: &str, status: &str, model: Option<&str>) -> Line<'static> {
    let theme = crate::theme::theme();
    let state = ChildState::parse(status);
    let label = match state {
        ChildState::Unknown => status,
        _ => state.label(),
    };
    let mut spans = vec![Span::raw(INDENT)];
    if let Some(glyph) = Some(state.glyph()).filter(|g| !g.is_empty()) {
        spans.push(Span::styled(format!("{glyph} "), state.style()));
    }
    spans.push(Span::styled(
        name.to_string(),
        Style::default().fg(theme.text_body),
    ));
    if !label.is_empty() {
        spans.push(Span::raw(ROW_GAP));
        spans.push(Span::styled(
            label.to_string(),
            Style::default().fg(theme.tool_dim),
        ));
        if let Some(model) = model {
            spans.push(Span::styled(
                format!("{MODEL_SEP}{model}"),
                Style::default().fg(theme.tool_dim),
            ));
        }
    }
    Line::from(spans)
}

fn detail_row(text: &str) -> Line<'static> {
    let budget = body_cells().saturating_sub(crate::text_width::display_width(DETAIL_INDENT));
    Line::from(Span::styled(
        format!("{DETAIL_INDENT}{}", ellipsize(text, budget)),
        Style::default().fg(crate::theme::theme().tool_dim),
    ))
}

fn detail_text(snap: &serde_json::Value, preview: &str) -> Option<String> {
    let activity = snap
        .get("activity")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("");
    if !activity.is_empty() {
        return Some(activity.to_string());
    }
    preview
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty() && !l.starts_with("\u{2014} session "))
        .map(str::to_string)
}

fn spawn_body(text: &str) -> Option<Vec<Line<'static>>> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let handles = v.get("handles")?.as_array()?;
    let cli = v.get("cli").and_then(|c| c.as_str()).unwrap_or("");
    let mut lines = Vec::with_capacity(handles.len());
    for handle in handles {
        let name = handle.get("name").and_then(|n| n.as_str())?.trim();
        if name.is_empty() {
            return None;
        }
        let id = handle.get("id").and_then(|n| n.as_str()).unwrap_or("");
        let model = handle
            .get("model")
            .and_then(|m| m.as_str())
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(str::to_string)
            .or_else(|| spawn_model(id, cli));
        let effort = handle
            .get("effort")
            .and_then(|e| e.as_str())
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .map(str::to_string);
        lines.push(spawn_row(name, model.as_deref(), effort.as_deref()));
    }
    Some(lines)
}

fn spawn_row(name: &str, model: Option<&str>, effort: Option<&str>) -> Line<'static> {
    let theme = crate::theme::theme();
    let mut spans = vec![
        Span::raw(INDENT),
        Span::styled(
            format!("{} ", ChildState::Running.glyph()),
            ChildState::Running.style(),
        ),
        Span::styled(name.to_string(), Style::default().fg(theme.text_body)),
    ];
    for part in [model, effort].into_iter().flatten() {
        spans.push(Span::styled(
            format!("{MODEL_SEP}{part}"),
            Style::default().fg(theme.tool_dim),
        ));
    }
    Line::from(spans)
}

fn spawn_model(id: &str, cli: &str) -> Option<String> {
    let short: String = id.chars().take(8).collect();
    let segment = cli
        .split(", ")
        .find(|s| !short.is_empty() && s.contains(&format!("[{short}]")))?;
    let after_arrow = segment.split_once(" \u{2192} ").map(|(_, m)| m)?;
    let model = after_arrow
        .split(" queued (")
        .next()
        .unwrap_or(after_arrow)
        .split(". ")
        .next()
        .unwrap_or(after_arrow)
        .rsplit(MODEL_SEP)
        .next()
        .unwrap_or(after_arrow)
        .trim();
    (!model.is_empty()).then(|| model.to_string())
}

fn model_from_preview(preview: &str) -> Option<String> {
    if let Some((_, rest)) = preview.split_once("<model-id>")
        && let Some((id, _)) = rest.split_once("</model-id>")
        && !id.trim().is_empty()
    {
        return Some(id.trim().to_string());
    }
    let head = preview.lines().next()?;
    let after_status = head.split_once(']').map_or(head, |(_, rest)| rest);
    let open = after_status.find(" <")?;
    let inner = after_status[open + 2..].split_once('>')?.0;
    let id = inner.split(MODEL_SEP).next()?.trim();
    (!id.is_empty()).then(|| id.to_string())
}

fn body_cells() -> usize {
    crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(120)
        .max(60)
        .saturating_sub(4)
        .max(20)
}

fn ellipsize(text: &str, max_cells: usize) -> String {
    if crate::text_width::display_width(text) <= max_cells {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let keep = crate::text_width::fit_char_count(&chars, max_cells.saturating_sub(1));
    let mut out: String = chars[..keep].iter().collect();
    out = out.trim_end().to_string();
    out.push('\u{2026}');
    out
}
