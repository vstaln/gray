use gray_core::message::{ContentBlock, Role};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::composer::TranscriptEntry;
use crate::session_store::SessionEntry;

pub(crate) enum PanelAction {
    Resume { session: String, name: String },

    Say(String),
}

#[derive(Debug, Clone)]
struct Row {
    name: String,
    task: String,
    icon: String,
    status: String,
    stats: String,
    activity: String,
    session: String,
    target: String,
    managed: bool,
    running: bool,
    resumable: bool,
}

fn plugin_argv(plugin: &str) -> Option<Vec<String>> {
    let home = crate::plugin_cli::home().ok()?;
    let lock: Value =
        serde_json::from_slice(&std::fs::read(home.join("plugins").join("lock.json")).ok()?)
            .ok()?;
    let entry = &lock["plugins"][plugin];
    for key in ["cli_argv", "argv"] {
        if let Some(argv) = entry[key].as_array() {
            let argv: Vec<String> = argv
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            if !argv.is_empty() {
                return Some(argv);
            }
        }
    }
    None
}

fn spawn_raw(argv: &[String], args: &[&str]) -> Option<String> {
    let (prog, base) = argv.split_first()?;
    let out = std::process::Command::new(prog)
        .args(base)
        .args(args)
        .env(
            "GRAY_BIN",
            crate::exe_watch::on_disk_exe().unwrap_or_else(|_| PathBuf::from("gray")),
        )
        .output()
        .ok()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let msg = err
            .lines()
            .rfind(|l| !l.trim().is_empty())
            .unwrap_or("plugin command failed");
        return Some(msg.to_string());
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn spawn_text(argv: &[String], args: &[&str]) -> Option<String> {
    let raw = spawn_raw(argv, args)?;
    let v: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    Some(
        v["content"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or(raw)
            .trim_end()
            .to_string(),
    )
}

fn fetch_rows(argv: &[String], all: bool) -> Vec<Row> {
    let mut args = vec!["entries"];
    if all {
        args.push("--all");
    }
    let Some(raw) = spawn_raw(argv, &args) else {
        return Vec::new();
    };
    let v: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    let Some(entries) = v["entries"].as_array() else {
        return Vec::new();
    };
    entries
        .iter()
        .map(|e| Row {
            name: e["name"].as_str().unwrap_or("agent").into(),
            task: e["task"].as_str().unwrap_or("").into(),
            icon: e["icon"].as_str().unwrap_or("·").into(),
            status: e["status"].as_str().unwrap_or("").into(),
            stats: e["stats"].as_str().unwrap_or("").into(),
            activity: e["activity"].as_str().unwrap_or("").into(),
            session: e["session"].as_str().unwrap_or("").into(),
            target: e["target"].as_str().unwrap_or("").into(),
            managed: e["managed"].as_bool().unwrap_or(false),
            running: e["running"].as_bool().unwrap_or(false),
            resumable: e["resumable"].as_bool().unwrap_or(false),
        })
        .collect()
}

struct ChatInput {
    target: String,

    name: String,
    buf: crate::composer::TextArea,
    focus: bool,
}

struct SessionFeed {
    path: PathBuf,
    offset: u64,
    lines_seen: usize,
    cwd: PathBuf,
    entries: Vec<SessionEntry>,

    reset: bool,
}

impl SessionFeed {
    fn open(session: &str) -> Option<Self> {
        let ok = !session.is_empty()
            && session.len() <= 128
            && session
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !ok {
            return None;
        }
        Some(Self {
            path: crate::session_store::default_root()?.join(format!("{session}.jsonl")),
            offset: 0,
            lines_seen: 0,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            entries: Vec::new(),
            reset: false,
        })
    }

    fn poll(&mut self) -> bool {
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return false;
        };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            self.offset = 0;
            self.lines_seen = 0;
            self.entries.clear();
            self.reset = true;
        }
        if len <= self.offset || file.seek(SeekFrom::Start(self.offset)).is_err() {
            return false;
        }
        let mut buf = Vec::new();
        if file.read_to_end(&mut buf).is_err() {
            return false;
        }
        let Some(last_nl) = buf.iter().rposition(|b| *b == b'\n') else {
            return false;
        };
        self.offset += last_nl as u64 + 1;
        let mut changed = false;
        for line in String::from_utf8_lossy(&buf[..=last_nl]).lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            self.lines_seen += 1;
            if self.lines_seen == 1 {
                if let Ok(v) = serde_json::from_str::<Value>(line)
                    && let Some(c) = v["cwd"].as_str()
                {
                    self.cwd = PathBuf::from(c);
                }
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<SessionEntry>(line) {
                self.entries.push(entry);
                changed = true;
            }
        }
        changed
    }
}

struct Viewer {
    target: String,
    session: String,
    name: String,
    icon: String,
    status: String,
    stats: String,
    running: bool,
    managed: bool,
    resumable: bool,
    feed: Option<SessionFeed>,
    transcript: Vec<TranscriptEntry>,

    built: usize,
    tool_calls: HashMap<String, (String, Value)>,

    tool_labels: HashMap<String, String>,
    tool_previews: HashMap<String, String>,

    tools_loaded: bool,
    gap_owed: bool,
    lines: Vec<Line<'static>>,

    layout_w: u16,
    dirty: bool,
    scroll: u16,

    stick: bool,
    max_scroll: u16,
    view_h: u16,
    last_tick: Instant,

    note: Option<String>,
    chat: ChatInput,
}

impl Viewer {
    fn open(row: &Row) -> Self {
        let mut v = Self {
            target: row.target.clone(),
            session: row.session.clone(),
            name: row.name.clone(),
            icon: row.icon.clone(),
            status: row.status.clone(),
            stats: row.stats.clone(),
            running: row.running,
            managed: row.managed,
            resumable: row.resumable,
            feed: None,
            transcript: Vec::new(),
            built: 0,
            tool_calls: HashMap::new(),
            tool_labels: HashMap::new(),
            tool_previews: HashMap::new(),
            tools_loaded: false,
            gap_owed: false,
            lines: Vec::new(),
            layout_w: 0,
            dirty: true,
            scroll: 0,
            stick: true,
            max_scroll: 0,
            view_h: 1,
            last_tick: Instant::now(),
            note: None,
            chat: ChatInput {
                target: row.target.clone(),
                name: row.name.clone(),
                buf: crate::composer::TextArea::new(),
                focus: false,
            },
        };
        v.refresh_feed();
        v
    }

    fn sync_row(&mut self, row: Option<&Row>) {
        let Some(r) = row else {
            return;
        };
        self.name = r.name.clone();
        self.icon = r.icon.clone();
        self.status = r.status.clone();
        self.stats = r.stats.clone();
        self.running = r.running;
        self.managed = r.managed;
        self.resumable = r.resumable;
        if !r.session.is_empty() {
            self.session = r.session.clone();
        }
    }

    fn refresh_feed(&mut self) {
        if self.session.is_empty() {
            return;
        }
        if self.feed.is_none() {
            self.feed = SessionFeed::open(&self.session);
        }
        if !self.tools_loaded && self.feed.is_some() {
            let cwd = self
                .feed
                .as_ref()
                .map(|f| f.cwd.clone())
                .unwrap_or_default();
            (self.tool_labels, self.tool_previews) = viewer_tool_maps(&cwd);
            self.tools_loaded = true;
        }
        if let Some(feed) = self.feed.as_mut() {
            feed.poll();
        }
    }

    fn build_pending(&mut self) {
        {
            let Some(feed) = self.feed.as_ref() else {
                return;
            };
            if feed.reset || self.built > feed.entries.len() {
                self.transcript.clear();
                self.tool_calls.clear();
                self.gap_owed = false;
                self.built = 0;
            }
        }
        if let Some(feed) = self.feed.as_mut() {
            feed.reset = false;
        }
        let new: Vec<SessionEntry> = self
            .feed
            .as_ref()
            .map(|f| f.entries[self.built..].to_vec())
            .unwrap_or_default();
        if new.is_empty() {
            return;
        }
        let cwd = self
            .feed
            .as_ref()
            .map(|f| f.cwd.clone())
            .unwrap_or_default();
        let w = usize::from(self.layout_w.max(20));
        for e in &new {
            replay_entry(self, e, &cwd, w);
        }
        self.built += new.len();
        self.dirty = true;
    }

    fn layout(&mut self, w: usize) {
        let w16 = u16::try_from(w).unwrap_or(u16::MAX);
        if self.layout_w != w16 {
            self.layout_w = w16;
            self.transcript.clear();
            self.tool_calls.clear();
            self.gap_owed = false;
            self.built = 0;
        }
        self.build_pending();
        if !self.dirty {
            return;
        }
        let mut blocks = self.transcript.clone();
        let cwd = self.feed.as_ref().map(|f| f.cwd.clone());
        let display = crate::tool_fmt::ToolDisplay {
            labels: Some(&self.tool_labels),
            previews: Some(&self.tool_previews),
        };
        for (name, args) in self.tool_calls.values() {
            blocks.push(TranscriptEntry::Gap(1));
            blocks.push(TranscriptEntry::ToolBoxLines {
                header: crate::tool_fmt::format_tool_card_lines(
                    name,
                    args,
                    cwd.as_deref(),
                    display.label(name),
                    display.preview(name),
                ),
                body: Vec::new(),
            });
            blocks.push(TranscriptEntry::Gap(1));
        }
        self.lines = crate::composer::transcript::layout_history(&blocks, w, 0);
        self.dirty = false;
    }
}

fn gap_mark(v: &mut Viewer) {
    if !v.gap_owed {
        v.transcript.push(TranscriptEntry::Gap(1));
        v.gap_owed = true;
    }
}

fn push_block(v: &mut Viewer, e: TranscriptEntry) {
    v.transcript.push(e);
    v.gap_owed = false;
}

fn replay_entry(v: &mut Viewer, entry: &SessionEntry, cwd: &Path, w: usize) {
    match entry.message.role {
        Role::User => {
            let mut user_text = String::new();
            let mut attached: Vec<PathBuf> = Vec::new();
            for block in &entry.message.content {
                match block {
                    ContentBlock::Text { text } => {
                        if !user_text.is_empty() {
                            user_text.push('\n');
                        }
                        user_text.push_str(text);
                    }
                    ContentBlock::StructuredInput { kind, .. } => {
                        if !user_text.is_empty() {
                            user_text.push('\n');
                        }
                        user_text.push_str("[component event: ");
                        user_text.push_str(kind);
                        user_text.push(']');
                    }
                    ContentBlock::Image { media_type, data }
                    | ContentBlock::Media {
                        media_type, data, ..
                    } => {
                        if let Some(p) = gray_core::paths::materialize_media(media_type, data) {
                            attached.push(p);
                        }
                    }
                    ContentBlock::ToolResult {
                        id,
                        content,
                        is_error,
                    } => replay_tool_result(v, id, content, *is_error, cwd),
                    _ => {}
                }
            }
            let (visible, notices) =
                crate::composer::transcript::split_synthetic_prompt(&user_text);
            if !visible.is_empty() || !attached.is_empty() {
                gap_mark(v);
                push_block(v, TranscriptEntry::UserPrompt(visible, attached));
                gap_mark(v);
            }
            for notice in notices {
                gap_mark(v);
                push_block(
                    v,
                    TranscriptEntry::StyledLines {
                        lines: vec![Line::from(Span::styled(
                            format!("\u{2b22} {notice}"),
                            Style::default().add_modifier(Modifier::DIM),
                        ))],
                        hyperlinks: Vec::new(),
                    },
                );
                gap_mark(v);
            }
        }
        Role::Assistant => {
            for block in &entry.message.content {
                match block {
                    ContentBlock::Thinking { text, .. } => {
                        let rows = crate::composer::transcript::thinking_replay_lines(text);
                        if !rows.is_empty() {
                            gap_mark(v);
                            push_block(
                                v,
                                TranscriptEntry::StyledLines {
                                    lines: rows,
                                    hyperlinks: Vec::new(),
                                },
                            );
                        }
                    }
                    ContentBlock::Text { text } => {
                        let (lines, hyperlinks) =
                            crate::composer::transcript::render_markdown_lines(text, Some(w));
                        if !lines.is_empty() {
                            gap_mark(v);
                            push_block(v, TranscriptEntry::StyledLines { lines, hyperlinks });
                        }
                    }
                    ContentBlock::ToolUse { id, name, args, .. } => {
                        v.tool_calls
                            .insert(id.clone(), (name.clone(), args.clone()));
                    }
                    _ => {}
                }
            }
        }
        Role::System => {
            for block in &entry.message.content {
                if let ContentBlock::ToolResult {
                    id,
                    content,
                    is_error,
                } = block
                {
                    replay_tool_result(v, id, content, *is_error, cwd);
                }
            }
        }
    }
}

fn viewer_tool_maps(cwd: &Path) -> (HashMap<String, String>, HashMap<String, String>) {
    let mut defs = Vec::new();
    let Ok(home) = crate::setup::gray_home() else {
        return crate::repl::session::tool_display_maps(&defs);
    };
    let load_lock = |path: PathBuf| {
        gray_plugin::lock::LockFile::load(&path).unwrap_or_else(|_| gray_plugin::lock::LockFile {
            schema: 0,
            plugins: Default::default(),
        })
    };
    let user = load_lock(gray_plugin::lock::lock_path(&home));
    let project = load_lock(gray_plugin::lock::project_lock_path(cwd));
    for name in user.plugins.keys().chain(project.plugins.keys()) {
        if !gray_plugin::lock::effective_enabled(&user, &project, name) {
            continue;
        }
        for dir in [home.join("plugins"), cwd.join(".gray/plugins")] {
            let Ok(raw) = std::fs::read_to_string(dir.join(format!("{name}-manifest.json"))) else {
                continue;
            };
            if let Ok(json) = serde_json::from_str::<Value>(&raw) {
                defs.extend(gray_plugin::manifest_tools(&json));
                break;
            }
        }
    }
    crate::repl::session::tool_display_maps(&defs)
}

fn replay_tool_result(v: &mut Viewer, id: &str, content: &str, is_error: bool, cwd: &Path) {
    let (name, args) = v
        .tool_calls
        .remove(id)
        .map(|(n, a)| (n, Some(a)))
        .unwrap_or_else(|| ("tool".to_string(), None));
    let display = crate::tool_fmt::ToolDisplay {
        labels: Some(&v.tool_labels),
        previews: Some(&v.tool_previews),
    };
    let header = args
        .as_ref()
        .map(|a| {
            crate::tool_fmt::format_tool_card_lines(
                &name,
                a,
                Some(cwd),
                display.label(&name),
                display.preview(&name),
            )
        })
        .unwrap_or_else(|| vec![Line::from(name.clone())]);
    let body = crate::tool_fmt::format_tool_result_lines_with_context(
        &name,
        args.as_ref(),
        content,
        is_error,
        Some(cwd),
    );
    gap_mark(v);
    push_block(v, TranscriptEntry::ToolBoxLines { header, body });
    gap_mark(v);
}

fn viewport_window(len: usize, view: usize, scroll: usize) -> (usize, usize, usize) {
    let start = scroll.min(len.saturating_sub(view));
    let end = (start + view).min(len);
    (view.saturating_sub(end - start), start, end)
}

fn scroll_by(v: &mut Viewer, delta: i64) {
    let next = (i64::from(v.scroll) + delta).clamp(0, i64::from(v.max_scroll));
    v.scroll = next as u16;
    v.stick = v.scroll >= v.max_scroll;
}

fn paint_viewer(frame: &mut ratatui::Frame, v: &mut Viewer) {
    use ratatui::layout::{Alignment, Position};
    use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

    let area = frame.area();
    let theme = crate::theme::theme();

    let w = (u32::from(area.width) * 4 / 5)
        .max(40)
        .min(u32::from(area.width)) as u16;
    let h = (u32::from(area.height) * 4 / 5)
        .max(10)
        .min(u32::from(area.height)) as u16;
    let modal = Rect::new(
        area.x + area.width.saturating_sub(w) / 2,
        area.y + area.height.saturating_sub(h) / 4,
        w,
        h,
    );
    frame.render_widget(Clear, modal);

    let dim = Style::default().fg(theme.text_dim).bg(theme.surface_bg);
    let state = if v.running {
        "running".to_string()
    } else {
        v.status.clone()
    };
    let hints = if v.chat.focus {
        "enter send · esc back"
    } else if v.managed {
        "type to chat · x stop · o resume · esc close"
    } else {
        "x stop · o resume · esc close"
    };
    let footer = match &v.note {
        Some(n) => format!(" {n} · {hints} "),
        None => format!(" {hints} "),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(Style::default().bg(theme.surface_bg))
        .border_style(Style::default().fg(theme.bezel_mid).bg(theme.surface_bg))
        .title_top(Line::from(vec![
            Span::styled(
                format!(" {} {} ", v.icon, v.name),
                Style::default()
                    .fg(theme.text_bright)
                    .bg(theme.surface_bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("· {state} · {} ", v.stats), dim),
        ]))
        .title_top(Line::from(Span::styled(" esc ", dim)).alignment(Alignment::Right))
        .title_bottom(Line::from(Span::styled(footer, dim)).alignment(Alignment::Center));
    let inner = block.inner(modal);
    frame.render_widget(block, modal);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let ghost = if v.managed {
        format!("message {}", v.name)
    } else {
        "raw worker — read only".to_string()
    };
    let ibox = crate::composer::draw::build_input_box(
        v.chat.buf.text(),
        v.chat.buf.cursor(),
        inner.width as usize,
        Some(&ghost),
        None,
        &[],
    );
    let cap = (usize::from(inner.height) / 3).clamp(3, 8);
    let len = ibox.lines.len();
    let first = if len > cap {
        (ibox.cur_row + 1).saturating_sub(cap).min(len - cap)
    } else {
        0
    };
    let box_h = if v.managed {
        (len - first).min(cap) as u16
    } else {
        0
    };
    let body_h = inner.height.saturating_sub(box_h);
    let body = Rect::new(inner.x, inner.y, inner.width, body_h);
    let input = Rect::new(inner.x, inner.y + body_h, inner.width, box_h);

    v.view_h = body_h;
    v.layout(inner.width as usize);
    let max = v.lines.len().saturating_sub(body_h as usize);
    let max16 = u16::try_from(max).unwrap_or(u16::MAX);
    if v.stick {
        v.scroll = max16;
    }
    v.scroll = v.scroll.min(max16);
    v.max_scroll = max16;

    if v.lines.is_empty() {
        let hint = if v.session.is_empty() {
            "(no child session yet — nothing to show)"
        } else {
            "(empty transcript)"
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(format!("  {hint}"), dim))),
            body,
        );
    } else {
        let (pad, start, end) = viewport_window(v.lines.len(), body_h as usize, v.scroll as usize);
        let mut rows = vec![Line::default(); pad];
        rows.extend(v.lines[start..end].iter().cloned());
        frame.render_widget(Paragraph::new(rows), body);
    }

    let mut rows: Vec<Line<'static>> = if box_h > 0 {
        ibox.lines[first..first + box_h as usize].to_vec()
    } else {
        Vec::new()
    };
    for line in &mut rows {
        for (i, span) in line.spans.iter_mut().enumerate() {
            span.style =
                if i == 0 && span.content == format!(" {} ", crate::tui::arrow_glyph()).as_str() {
                    span.style.fg(if v.chat.focus {
                        theme.accent
                    } else {
                        theme.text_dim
                    })
                } else if !v.chat.focus {
                    span.style.fg(theme.text_dim)
                } else {
                    span.style
                };
        }
    }
    frame.render_widget(Paragraph::new(rows), input);
    if v.chat.focus && box_h > 0 {
        let cx = (input.x + 3 + u16::try_from(ibox.cur_col).unwrap_or(u16::MAX))
            .min(input.x + input.width.saturating_sub(1));
        let cy = (input.y + ibox.cur_row.saturating_sub(first) as u16)
            .min(input.y + box_h.saturating_sub(1));
        frame.set_cursor_position(Position::new(cx, cy));
    }
}

fn picker_modal_w(term_w: u16) -> u16 {
    term_w.saturating_sub(2).max(42).min(term_w)
}

fn picker_list_h(term_h: u16, runs: usize) -> usize {
    (usize::from(term_h) * 3 / 5)
        .saturating_sub(7)
        .max(3)
        .min(runs.max(1))
}

fn picker_scroll_top(runs: usize, sel: usize, top: usize, visible: usize) -> usize {
    let visible = visible.max(1);
    let mut top = top.min(runs.saturating_sub(visible));
    if sel < top {
        top = sel;
    } else if sel >= top + visible {
        top = sel + 1 - visible;
    }
    top
}

fn picker_range_label(runs: usize, top: usize, visible: usize) -> Option<String> {
    (runs > visible).then(|| format!("{}–{} of {runs}", top + 1, (top + visible).min(runs)))
}

const PICKER_ACTIVITY_MAX: usize = 60;

fn cut_cells(s: &str, max: usize) -> String {
    use crate::text_width::{char_width, display_width};
    if display_width(s) <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".chars().take(max).collect();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = char_width(c);
        if used + w > max - 1 {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push('…');
    out
}

fn picker_row_text(row: &Row, selected: bool, width: usize) -> String {
    use crate::text_width::display_width;
    let mark = if selected { "▸" } else { " " };
    let task = row.task.lines().next().unwrap_or("").trim();
    let activity = row.activity.lines().next().unwrap_or("").trim();
    let head = format!(" {mark} {} {}", row.icon, row.name);
    let sep = if task.is_empty() { "" } else { " — " };
    let stats = format!(" · {}", row.stats);
    let state = if row.running {
        String::new()
    } else {
        format!(" · {}", row.status)
    };
    let spare = width.saturating_sub(
        display_width(&head) + display_width(sep) + display_width(&stats) + display_width(&state),
    );
    let act_room = spare
        .saturating_sub(display_width(task).min(40))
        .min(PICKER_ACTIVITY_MAX);
    let act = if row.running && !activity.is_empty() && act_room >= 15 {
        format!(" · {}", cut_cells(activity, act_room - 3))
    } else {
        String::new()
    };
    let task_room = spare.saturating_sub(display_width(&act));
    let task = if task.is_empty() {
        String::new()
    } else {
        cut_cells(task, task_room)
    };
    let line = cut_cells(&format!("{head}{sep}{task}{stats}{act}{state}"), width);
    let pad = width.saturating_sub(display_width(&line));
    format!("{line}{}", " ".repeat(pad))
}

pub(crate) fn run_agents_picker(
    plugin: &str,
    bg: Option<&crate::setup::BackgroundSnapshot>,
) -> anyhow::Result<Option<PanelAction>> {
    run_agents_panel(plugin, bg, false)
}

pub(crate) fn run_agents_viewer(
    plugin: &str,
    bg: Option<&crate::setup::BackgroundSnapshot>,
) -> anyhow::Result<Option<PanelAction>> {
    run_agents_panel(plugin, bg, true)
}

fn run_agents_panel(
    plugin: &str,
    bg: Option<&crate::setup::BackgroundSnapshot>,
    in_turn: bool,
) -> anyhow::Result<Option<PanelAction>> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, poll, read};
    use ratatui::layout::Rect;
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use std::time::Duration;

    let Some(argv) = plugin_argv(plugin) else {
        return Ok(Some(PanelAction::Say(format!(
            "{} Agents\n  {} {plugin}: no CLI in the plugin lock — install it first",
            crate::tui::glyph("\u{2b22}", "#"),
            crate::tui::glyph("\u{23bf}", "-")
        ))));
    };

    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(crate::setup::BackgroundSnapshot::default_initial);
    let (_session, mut terminal) = crate::setup::open_modal()?;

    let box_bg = crate::theme::theme().surface_bg;
    let accent_peach = crate::theme::theme().accent;
    let text_dim = crate::theme::theme().text_dim;
    let on_selection = crate::theme::theme().on_selection;

    let mut all = false;
    let mut rows = fetch_rows(&argv, all);
    let mut sel: usize = 0;
    let mut scroll_top: usize = 0;
    let mut status = String::new();
    let mut chat_in: Option<ChatInput> = None;
    let mut viewer: Option<Viewer> = None;
    let mut result: Option<PanelAction> = None;

    let out: anyhow::Result<()> = (|| {
        loop {
            if rows.is_empty() {
                sel = 0;
            } else if sel >= rows.len() {
                sel = rows.len() - 1;
            }
            if let Some(vw) = viewer.as_mut()
                && vw.last_tick.elapsed() >= Duration::from_secs(1)
            {
                vw.last_tick = Instant::now();
                if vw.running {
                    rows = fetch_rows(&argv, all);
                    let upd = rows.iter().find(|r| r.target == vw.target).cloned();
                    vw.sync_row(upd.as_ref());
                }
                vw.refresh_feed();
            }
            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 40 || area.height < 10 {
                    return;
                }
                crate::setup::render_dimmed_background(frame, &bg_snapshot);
                if let Some(vw) = viewer.as_mut() {
                    paint_viewer(frame, vw);
                    return;
                }

                let list_h = picker_list_h(area.height, rows.len());
                let modal_w = picker_modal_w(area.width);
                let modal_h = (list_h as u16 + 7)
                    .min(area.height.saturating_sub(2))
                    .max(8)
                    .min(area.height);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 4;
                let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);

                frame.render_widget(Clear, modal_rect);
                frame.render_widget(
                    Block::default().style(Style::default().bg(box_bg)),
                    modal_rect,
                );

                let pad_x = 2u16;
                let inner = Rect::new(
                    modal_x + pad_x,
                    modal_y + 1,
                    modal_w.saturating_sub(pad_x * 2),
                    modal_h.saturating_sub(2),
                );
                let mut put = |y: u16, line: Line| {
                    frame.render_widget(
                        Paragraph::new(line),
                        Rect::new(inner.x, inner.y + y, inner.width, 1),
                    );
                };

                let visible = list_h.min(usize::from(modal_h.saturating_sub(7))).max(1);
                scroll_top = picker_scroll_top(rows.len(), sel, scroll_top, visible);
                let range = picker_range_label(rows.len(), scroll_top, visible)
                    .map(|label| format!("  {label}"))
                    .unwrap_or_default();

                let title = if all {
                    "Agents — all sessions"
                } else {
                    "Agents"
                };
                let esc_str = "esc";
                let pad_len = (inner.width as usize).saturating_sub(
                    title.chars().count() + range.chars().count() + esc_str.chars().count() + 8,
                );
                put(
                    0,
                    Line::from(vec![
                        Span::styled(
                            title,
                            Style::default()
                                .fg(crate::theme::theme().text_bright)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled(range, Style::default().fg(text_dim).bg(box_bg)),
                        Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                        Span::styled("a: all/cwd  ", Style::default().fg(text_dim).bg(box_bg)),
                        Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                    ]),
                );

                let list_y = 2u16;
                if rows.is_empty() {
                    put(
                        list_y,
                        Line::from(Span::styled(
                            "  no runs yet — gray subagents run 'task' to start one",
                            Style::default().fg(text_dim).bg(box_bg),
                        )),
                    );
                } else {
                    for r in 0..visible {
                        let idx = scroll_top + r;
                        if idx >= rows.len() {
                            break;
                        }
                        let is_sel = idx == sel;
                        let row_str = picker_row_text(&rows[idx], is_sel, inner.width as usize);
                        let line = if is_sel {
                            Line::from(Span::styled(
                                row_str,
                                Style::default()
                                    .fg(on_selection)
                                    .bg(accent_peach)
                                    .add_modifier(Modifier::BOLD),
                            ))
                        } else {
                            Line::from(Span::styled(
                                row_str,
                                Style::default()
                                    .fg(crate::theme::theme().text_bright)
                                    .bg(box_bg),
                            ))
                        };
                        put(list_y + r as u16, line);
                    }
                }

                let status_y = inner.height.saturating_sub(2);
                if let Some(c) = &chat_in {
                    let prompt = format!(" chat {}: {}_", c.name, c.buf.text());
                    let clipped: String = prompt.chars().take(inner.width as usize).collect();
                    put(
                        status_y,
                        Line::from(Span::styled(
                            clipped,
                            Style::default()
                                .fg(accent_peach)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        )),
                    );
                } else {

                    let hint = rows.get(sel).map_or("", |r| {
                        if r.running {
                            " ⏎ watch live · c follow-up · x stop · v tail"
                        } else if r.resumable && in_turn {
                            " ⏎ view · c continues · v tail · o resumes after the turn"
                        } else if r.resumable {
                            " ⏎ view · o steps into its session · c continues · v tail"
                        } else if r.managed {
                            " ⏎ view · v tail · c starts a new turn"
                        } else {
                            " ⏎ view · v tail"
                        }
                    });
                    let text = if status.is_empty() {
                        hint
                    } else {
                        status.as_str()
                    };
                    let clipped: String = text.chars().take(inner.width as usize).collect();
                    if !clipped.is_empty() {
                        put(
                            status_y,
                            Line::from(Span::styled(
                                format!(" {clipped}"),
                                Style::default().fg(text_dim).bg(box_bg),
                            )),
                        );
                    }
                }
                put(
                    inner.height.saturating_sub(1),
                    Line::from(Span::styled(
                        " ↑↓ move · ⏎ view · o resume · c chat · x stop · v tail · a all · r refresh",
                        Style::default().fg(text_dim).bg(box_bg),
                    )),
                );
            })?;

            if crate::composer::input::stdin_gone() {
                return Ok(());
            }
            if !poll(Duration::from_millis(250))? {
                continue;
            }
            let Event::Key(KeyEvent {
                code,
                modifiers,
                kind,
                ..
            }) = read()?
            else {
                continue;
            };
            if kind == KeyEventKind::Release {
                continue;
            }
            use crossterm::event::KeyModifiers;
            if chat_in.is_some() {
                match code {
                    KeyCode::Esc => chat_in = None,
                    KeyCode::Enter => {
                        let Some(c) = chat_in.take() else {
                            continue;
                        };
                        let msg = c.buf.text().trim().to_string();
                        if msg.is_empty() {
                            status = "chat cancelled".to_string();
                        } else {
                            status = spawn_text(&argv, &["chat", &c.target, &msg])
                                .unwrap_or_else(|| "chat failed".into());
                            rows = fetch_rows(&argv, all);
                        }
                    }
                    KeyCode::Backspace => {
                        if let Some(c) = chat_in.as_mut() {
                            c.buf.delete_backward();
                        }
                    }
                    KeyCode::Left => {
                        if let Some(c) = chat_in.as_mut() {
                            c.buf.move_left();
                        }
                    }
                    KeyCode::Right => {
                        if let Some(c) = chat_in.as_mut() {
                            c.buf.move_right();
                        }
                    }
                    KeyCode::Char(ch) if !modifiers.contains(KeyModifiers::CONTROL) => {
                        if let Some(c) = chat_in.as_mut() {
                            c.buf.insert_str(&ch.to_string());
                        }
                    }
                    _ => {}
                }
                continue;
            }
            if let Some(vw) = viewer.as_mut() {
                if vw.chat.focus {
                    match code {
                        KeyCode::Esc => vw.chat.focus = false,
                        KeyCode::Enter => {
                            let msg = vw.chat.buf.text().trim().to_string();
                            if !msg.is_empty() {
                                let receipt = spawn_text(&argv, &["chat", &vw.chat.target, &msg])
                                    .unwrap_or_else(|| "chat failed".into());
                                vw.chat.buf.set_text("");
                                vw.note = Some(receipt);
                                vw.running = true;
                                vw.stick = true;
                                rows = fetch_rows(&argv, all);
                                let upd = rows.iter().find(|r| r.target == vw.target).cloned();
                                vw.sync_row(upd.as_ref());
                            }
                        }
                        KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                            break;
                        }
                        KeyCode::Backspace => vw.chat.buf.delete_backward(),
                        KeyCode::Delete => vw.chat.buf.delete_forward(),
                        KeyCode::Left => vw.chat.buf.move_left(),
                        KeyCode::Right => vw.chat.buf.move_right(),
                        KeyCode::Up => scroll_by(vw, -1),
                        KeyCode::Down => scroll_by(vw, 1),
                        KeyCode::PageUp => scroll_by(vw, -i64::from(vw.view_h.max(1))),
                        KeyCode::PageDown => scroll_by(vw, i64::from(vw.view_h.max(1))),
                        KeyCode::Home => scroll_by(vw, -i64::from(vw.scroll)),
                        KeyCode::End => scroll_by(vw, i64::from(vw.max_scroll)),
                        KeyCode::Char(ch) if !modifiers.contains(KeyModifiers::CONTROL) => {
                            vw.chat.buf.insert_str(&ch.to_string());
                        }
                        _ => {}
                    }
                    continue;
                }
                match code {
                    KeyCode::Esc | KeyCode::Char('q') => {
                        viewer = None;
                        continue;
                    }
                    KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => break,
                    KeyCode::Up | KeyCode::Char('k') => scroll_by(vw, -1),
                    KeyCode::Down | KeyCode::Char('j') => scroll_by(vw, 1),
                    KeyCode::PageUp => scroll_by(vw, -i64::from(vw.view_h.max(1))),
                    KeyCode::PageDown => scroll_by(vw, i64::from(vw.view_h.max(1))),
                    KeyCode::Home => scroll_by(vw, -i64::from(vw.scroll)),
                    KeyCode::End => scroll_by(vw, i64::from(vw.max_scroll)),
                    KeyCode::Char('x') => {
                        if vw.running {
                            let receipt = spawn_text(&argv, &["stop", &vw.target])
                                .unwrap_or_else(|| "stop failed".into());
                            rows = fetch_rows(&argv, all);
                            let upd = rows.iter().find(|r| r.target == vw.target).cloned();
                            vw.note = Some(receipt);
                            vw.sync_row(upd.as_ref());
                        } else {
                            vw.note = Some(format!("{} already {}", vw.name, vw.status));
                        }
                    }
                    KeyCode::Char('o') => {
                        if in_turn {
                            vw.note = Some("resume after the current turn".into());
                        } else if !vw.running && vw.resumable {
                            result = Some(PanelAction::Resume {
                                session: vw.session.clone(),
                                name: vw.name.clone(),
                            });
                            break;
                        } else if vw.running {
                            vw.note = Some(format!("{} is still running", vw.name));
                        } else {
                            vw.note = Some(format!("{} has no session to resume", vw.name));
                        }
                    }
                    KeyCode::Enter | KeyCode::Char('c') | KeyCode::Char('i') => {
                        if !vw.managed {
                            vw.note = Some(format!(
                                "{} is a raw worker — nothing to chat through",
                                vw.name
                            ));
                        } else {
                            vw.chat.focus = true;
                        }
                    }
                    KeyCode::Char(ch) if !modifiers.contains(KeyModifiers::CONTROL) => {
                        if !vw.managed {
                            vw.note = Some(format!(
                                "{} is a raw worker — nothing to chat through",
                                vw.name
                            ));
                        } else {
                            vw.chat.focus = true;
                            vw.chat.buf.insert_str(&ch.to_string());
                        }
                    }
                    _ => {}
                }
                continue;
            }
            match code {
                KeyCode::Esc | KeyCode::Char('q') => break,
                KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => break,
                KeyCode::Up | KeyCode::Char('k') => {
                    sel = sel.checked_sub(1).unwrap_or(rows.len().saturating_sub(1));
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if !rows.is_empty() {
                        sel = (sel + 1) % rows.len();
                    }
                }
                KeyCode::Enter => {
                    let Some(row) = rows.get(sel) else {
                        continue;
                    };
                    viewer = Some(Viewer::open(row));
                }
                KeyCode::Char('o') => {
                    if in_turn {
                        status = "resume after the current turn".into();
                    } else if let Some(row) = rows.get(sel) {
                        if !row.running && row.resumable {
                            result = Some(PanelAction::Resume {
                                session: row.session.clone(),
                                name: row.name.clone(),
                            });
                            break;
                        }
                        status = if row.running {
                            format!("{} is still running", row.name)
                        } else {
                            format!("{} has no session to resume", row.name)
                        };
                    }
                }
                KeyCode::Char('v') => {
                    if let Some(row) = rows.get(sel) {
                        let text = spawn_text(&argv, &["view", &row.target, "--lines", "200"])
                            .unwrap_or_else(|| "(no transcript)".into());
                        result = Some(PanelAction::Say(text));
                        break;
                    }
                }
                KeyCode::Char('x') => {
                    if let Some(row) = rows.get(sel).cloned() {
                        if row.running {
                            status = spawn_text(&argv, &["stop", &row.target])
                                .unwrap_or_else(|| "stop failed".into());
                        } else {
                            status = format!("{} already {}", row.name, row.status);
                        }
                        rows = fetch_rows(&argv, all);
                    }
                }
                KeyCode::Char('c') => {
                    if let Some(row) = rows.get(sel) {
                        if row.managed {
                            chat_in = Some(ChatInput {
                                target: row.target.clone(),
                                name: row.name.clone(),
                                buf: crate::composer::TextArea::new(),
                                focus: true,
                            });
                        } else {
                            status =
                                format!("{} is a raw worker — nothing to chat through", row.name);
                        }
                    }
                }
                KeyCode::Char('a') => {
                    all = !all;
                    rows = fetch_rows(&argv, all);
                }
                KeyCode::Char('r') => {
                    rows = fetch_rows(&argv, all);
                    status = "refreshed".into();
                }
                _ => {}
            }
        }
        Ok(())
    })();
    out?;
    Ok(result)
}

pub(crate) fn run_subagent_settings(
    plugin: &str,
    shared: &crate::composer::SharedTui,
) -> Option<String> {
    use crate::ask::{AskOption, AskQuestion};
    let argv = plugin_argv(plugin)?;
    let raw = spawn_raw(&argv, &["settings"])?;
    let Ok(cur) = serde_json::from_str::<Value>(raw.trim()) else {
        return Some(format!("couldn't read {plugin} settings: {}", raw.trim()));
    };
    let cur_of = |k: &str| {
        cur[k]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| "inherit".to_string())
    };
    let cur_max = cur["max_running"].as_u64().unwrap_or(128);

    let opt = |label: &str, desc: &str| AskOption {
        label: label.to_string(),
        description: desc.to_string(),
        opt_in: false,
    };
    let q = |id: &str, question: String, options: Vec<AskOption>| AskQuestion {
        id: id.to_string(),
        header: "subagents".to_string(),
        question,
        options,
        multi: false,
        default_all: false,
    };
    let inherit = || {
        opt(
            "inherit",
            "no subagent default — children follow the session",
        )
    };
    let questions = vec![
        q(
            "model",
            format!("default subagent model? (now: {})", cur_of("model")),
            vec![opt("keep", "leave unchanged"), inherit()],
        ),
        q(
            "effort",
            format!("default reasoning effort? (now: {})", cur_of("effort")),
            vec![
                opt("keep", "leave unchanged"),
                opt("off", ""),
                opt("minimal", ""),
                opt("low", ""),
                opt("medium", ""),
                opt("high", ""),
                opt("xhigh", ""),
                opt("max", ""),
                inherit(),
            ],
        ),
        q(
            "context",
            format!("context ceiling per child? (now: {})", cur_of("context")),
            vec![
                opt("keep", "leave unchanged"),
                opt("auto", "model default"),
                opt("64k", ""),
                opt("90k", ""),
                opt("128k", ""),
                opt("200k", ""),
                opt("256k", ""),
                opt("300k", ""),
                opt("1m", ""),
                inherit(),
            ],
        ),
        q(
            "max_running",
            format!("max concurrent subagents? (now: {cur_max})"),
            vec![
                opt("keep", "leave unchanged"),
                opt("8", ""),
                opt("16", ""),
                opt("32", ""),
                opt("64", ""),
                opt("128", ""),
                opt("256", ""),
            ],
        ),
    ];
    let answers = crate::ask::ask_questions_modal(shared, questions)?;

    let mut flags: Vec<String> = Vec::new();
    for a in &answers {
        let flag = match a.id.as_str() {
            "model" => "--model",
            "effort" => "--effort",
            "context" => "--context",
            "max_running" => "--max-running",
            _ => continue,
        };
        let note = a.answers.iter().find_map(|s| s.strip_prefix("user_note: "));
        let pick = a.answers.iter().find(|s| !s.starts_with("user_note:"));
        let value = note
            .map(str::to_owned)
            .or_else(|| match pick.map(String::as_str) {
                Some("keep") | Some("None of the above") | None => None,
                Some("inherit") => Some("none".to_string()),
                Some(l) => Some(l.to_string()),
            });
        if let Some(v) = value {
            flags.push(flag.to_string());
            flags.push(v);
        }
    }
    if flags.is_empty() {
        return Some("subagents settings — unchanged".to_string());
    }
    let args: Vec<&str> = ["settings"]
        .into_iter()
        .chain(flags.iter().map(String::as_str))
        .collect();
    let out = spawn_raw(&argv, &args).unwrap_or_default();
    match serde_json::from_str::<Value>(out.trim()) {
        Ok(v) if v.is_object() => Some(format!(
            "subagents settings saved — model {} · effort {} · context {} · max-running {}",
            v["model"].as_str().unwrap_or("inherit"),
            v["effort"].as_str().unwrap_or("inherit"),
            v["context"].as_str().unwrap_or("inherit"),
            v["max_running"]
                .as_u64()
                .map(|n| n.to_string())
                .unwrap_or_else(|| "?".into())
        )),
        _ => Some(format!("settings failed: {}", out.trim())),
    }
}

#[path = "agents_panel_tests.rs"]
#[cfg(test)]
mod tests;
