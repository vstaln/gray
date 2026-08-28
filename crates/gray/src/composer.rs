//! Ratatui-backed composer: codex/grok-build architecture sized for gray.
//!
//! Inline viewport owns the bottom rows permanently — slash-completion
//! panel, status, `›` input — while transcript goes into scrollback via
//! `Terminal::insert_before`. Multiline, attachments, slash popup and
//! history are replicated from `codex-rs/tui/src/bottom_pane/chat_composer.rs`
//! and `textarea.rs` (ponytail minimal: one-file adaptation, stdlib only).

use std::io::{Stdout, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Widget};
use ratatui::Terminal;

use crate::repl::completion_matches;

const VIEWPORT_H: u16 = 12;
const BOT_PREFIX_FIRST: &str = "• ";
const BOT_PREFIX_REST: &str = "  ";
const PANEL_ROWS: usize = 8;

type Term = Terminal<CrosstermBackend<Stdout>>;

/// Shared handle: main thread drives prompts/streaming, ticker refreshes elapsed.
pub struct SharedTui(pub Arc<std::sync::Mutex<Tui>>);

impl std::ops::Deref for SharedTui {
    type Target = Arc<std::sync::Mutex<Tui>>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// ---------------------------------------------------------------------------
// Minimal TextArea — literal copy-paste of codex textarea logic, trimmed to
// stdlib. Full codex TextArea is 4518 lines with vim/kill-ring/unicode-
// segmentation; this keeps the essential multiline + atomic-element contract
// (cursor byte-boundary, wrap-aware up/down, element-shift on insert).
// ponytail: O(n) scan, no grapheme crate, word wrap via char count.
// Upgrade path: vendor full `textarea.rs` + `textarea/wrapping.rs` when
// unicode-width or vim bindings matter.
// ---------------------------------------------------------------------------
#[allow(dead_code)]
#[derive(Debug, Clone)]
struct TextElement {
    id: u64,
    range: std::ops::Range<usize>,
}

#[derive(Debug)]
struct TextArea {
    text: String,
    cursor: usize, // byte index
    elements: Vec<TextElement>,
    next_id: u64,
}

#[allow(dead_code)]
impl TextArea {
    fn new() -> Self {
        Self { text: String::new(), cursor: 0, elements: Vec::new(), next_id: 1 }
    }
    fn text(&self) -> &str { &self.text }
    fn is_empty(&self) -> bool { self.text.is_empty() }
    fn cursor(&self) -> usize { self.cursor }
    fn set_text(&mut self, s: &str) {
        self.text = s.to_string();
        self.cursor = self.cursor.min(self.text.len());
        self.cursor = self.clamp_to_boundary(self.cursor);
        self.elements.clear();
    }
    fn clamp_to_boundary(&self, pos: usize) -> usize {
        let mut p = pos.min(self.text.len());
        while p < self.text.len() && !self.text.is_char_boundary(p) { p += 1; }
        p
    }
    fn is_char_boundary(&self, pos: usize) -> bool { self.text.is_char_boundary(pos) }
    fn next_boundary(&self, pos: usize) -> usize {
        // next char boundary, but jump over atomic elements
        for el in &self.elements {
            if pos >= el.range.start && pos < el.range.end { return el.range.end; }
        }
        if pos >= self.text.len() { return self.text.len(); }
        let mut n = pos + 1;
        while n < self.text.len() && !self.text.is_char_boundary(n) { n += 1; }
        // if landing inside element, jump to its end
        for el in &self.elements {
            if n > el.range.start && n < el.range.end { return el.range.end; }
        }
        n.min(self.text.len())
    }
    fn prev_boundary(&self, pos: usize) -> usize {
        if pos == 0 { return 0; }
        for el in &self.elements {
            if pos > el.range.start && pos <= el.range.end { return el.range.start; }
        }
        let mut n = pos - 1;
        while n > 0 && !self.text.is_char_boundary(n) { n -= 1; }
        for el in &self.elements {
            if n > el.range.start && n < el.range.end { return el.range.start; }
        }
        n
    }
    fn insert_str(&mut self, s: &str) { self.insert_at(self.cursor, s); }
    fn insert_at(&mut self, pos: usize, s: &str) {
        let pos = self.clamp_to_boundary(pos.min(self.text.len()));
        self.text.insert_str(pos, s);
        if pos <= self.cursor { self.cursor += s.len(); }
        self.shift_elements(pos, 0, s.len());
    }
    fn insert_element(&mut self, placeholder: &str) -> u64 {
        let id = self.next_id; self.next_id += 1;
        let start = self.cursor;
        self.insert_str(placeholder);
        let end = start + placeholder.len();
        self.elements.push(TextElement { id, range: start..end });
        self.elements.sort_by_key(|e| e.range.start);
        id
    }
    fn shift_elements(&mut self, pos: usize, removed: usize, inserted: usize) {
        let diff = inserted as isize - removed as isize;
        for el in &mut self.elements {
            if el.range.start >= pos + removed { el.range.start = ((el.range.start as isize) + diff) as usize; el.range.end = ((el.range.end as isize) + diff) as usize; }
            else if el.range.end > pos { /* inside edit — collapse */ }
        }
    }
    fn delete_backward(&mut self, n: usize) {
        if n == 0 || self.cursor == 0 { return; }
        let mut target = self.cursor;
        for _ in 0..n { target = self.prev_boundary(target); if target == 0 { break; } }
        self.replace_range(target..self.cursor, "");
    }
    fn delete_forward(&mut self, n: usize) {
        if n == 0 || self.cursor >= self.text.len() { return; }
        let mut target = self.cursor;
        for _ in 0..n { target = self.next_boundary(target); if target >= self.text.len() { break; } }
        self.replace_range(self.cursor..target, "");
    }
    fn replace_range(&mut self, range: std::ops::Range<usize>, s: &str) {
        let start = range.start.min(self.text.len());
        let end = range.end.min(self.text.len());
        let removed = end - start;
        self.text.replace_range(start..end, s);
        if self.cursor < start {} else if self.cursor <= end { self.cursor = start + s.len(); } else { self.cursor = ((self.cursor as isize) + s.len() as isize - removed as isize) as usize; }
        self.cursor = self.cursor.min(self.text.len());
        self.cursor = self.clamp_to_boundary(self.cursor);
        self.shift_elements(start, removed, s.len());
    }
    fn set_cursor(&mut self, pos: usize) {
        self.cursor = self.clamp_to_boundary(pos.min(self.text.len()));
        // avoid landing inside element
        for el in &self.elements {
            if self.cursor > el.range.start && self.cursor < el.range.end { self.cursor = el.range.end; break; }
        }
    }
    fn move_left(&mut self) { self.cursor = self.prev_boundary(self.cursor); }
    fn move_right(&mut self) { self.cursor = self.next_boundary(self.cursor); }
    fn move_up(&mut self) {
        let bol = self.text[..self.cursor].rfind('\n').map(|i| i+1).unwrap_or(0);
        let col = self.text[bol..self.cursor].chars().count();
        if bol == 0 { self.cursor = 0; return; }
        let prev_eol = bol - 1;
        let prev_bol = self.text[..prev_eol].rfind('\n').map(|i| i+1).unwrap_or(0);
        let prev_line = &self.text[prev_bol..prev_eol];
        let byte_col = prev_line.char_indices().nth(col).map(|(i,_)| i).unwrap_or(prev_line.len());
        self.cursor = self.clamp_to_boundary(prev_bol + byte_col);
    }
    fn move_down(&mut self) {
        let eol = self.text[self.cursor..].find('\n').map(|i| i+self.cursor).unwrap_or(self.text.len());
        let bol = self.text[..self.cursor].rfind('\n').map(|i| i+1).unwrap_or(0);
        let col = self.text[bol..self.cursor].chars().count();
        if eol >= self.text.len() { self.cursor = self.text.len(); return; }
        let next_bol = eol + 1;
        let next_eol = self.text[next_bol..].find('\n').map(|i| i+next_bol).unwrap_or(self.text.len());
        let next_line = &self.text[next_bol..next_eol];
        let byte_col = next_line.char_indices().nth(col).map(|(i,_)| i).unwrap_or(next_line.len());
        self.cursor = self.clamp_to_boundary(next_bol + byte_col);
    }
    fn move_to_end(&mut self) { self.cursor = self.text.len(); }
}

pub struct Tui {
    terminal: Term,
    textarea: TextArea,
    matches: Vec<(&'static str, &'static str)>,
    sel: usize,
    status: Option<(Instant, String)>,
    pending: String,
    truecolor: bool,
    thinking: bool,
    hide_thinking: bool,
    history: Vec<String>,
    history_idx: Option<usize>,
    draft: String,
    attachments: Vec<PathBuf>,
    pending_pastes: Vec<(String, String)>,
    model_name: String,
    cwd: String,
    thinking_effort: String,
    pub transcript: Vec<Line<'static>>,
    bot_buffer: Option<String>,
    bot_emitted: usize,
    bot_prefix_first: bool,
}

fn thinking_style() -> Style {
    Style::default().add_modifier(Modifier::DIM | Modifier::ITALIC)
}

fn shimmer_spans(text: &str, elapsed: Duration, truecolor: bool) -> Vec<Span<'static>> {
    use ratatui::style::Color;
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() { return Vec::new(); }
    let padding = 10usize;
    let period = chars.len() + padding * 2;
    let sweep_seconds = 2.0f32;
    let pos = ((elapsed.as_secs_f32() % sweep_seconds) / sweep_seconds * (period as f32)) as usize;
    let band_half_width = 5.0f32;
    const BASE: (u8, u8, u8) = (150, 148, 144);
    const HIGHLIGHT: (u8, u8, u8) = (255, 255, 255);
    chars.iter().enumerate().map(|(i, ch)| {
        let dist = ((i as isize + padding as isize) - pos as isize).abs() as f32;
        let t = if dist <= band_half_width { 0.5 * (1.0 + (std::f32::consts::PI * (dist / band_half_width)).cos()) } else { 0.0 };
        let style = if truecolor {
            let k = t.clamp(0.0, 1.0) * 0.9;
            let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * k) as u8;
            Style::default().fg(Color::Rgb(lerp(BASE.0, HIGHLIGHT.0), lerp(BASE.1, HIGHLIGHT.1), lerp(BASE.2, HIGHLIGHT.2))).add_modifier(if t > 0.3 { Modifier::BOLD } else { Modifier::empty() })
        } else if t < 0.2 { Style::default().add_modifier(Modifier::DIM) } else if t < 0.6 { Style::default() } else { Style::default().add_modifier(Modifier::BOLD) };
        Span::styled(ch.to_string(), style)
    }).collect()
}

impl Tui {
    pub fn new() -> anyhow::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture);
        let (cols, _) = crossterm::terminal::size().unwrap_or((80, 24));
        let mut terminal = Terminal::with_options(
            CrosstermBackend::new(std::io::stdout()),
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Inline(VIEWPORT_H),
            },
        )?;
        terminal.clear()?;

        // Print welcome logo into scrollback once at startup
        let logo_raw = crate::tui::logo_lines();
        let l_rows = logo_raw.len().max(1) as f32;
        let max_logo_w = logo_raw.iter().map(|l| l.trim_end().chars().count()).max().unwrap_or(0);
        let l_cols = (max_logo_w as f32).max(1.0);
        let w = cols as usize;
        let logo_pad = w.saturating_sub(max_logo_w) / 2;

        let base = Color::Rgb(110, 110, 110);
        let hilite = Color::Rgb(240, 240, 240);

        let mut welcome_lines: Vec<Line<'static>> = Vec::new();
        welcome_lines.push(Line::from(""));
        for (row, line) in logo_raw.iter().enumerate() {
            let trimmed = line.trim_end();
            let mut spans: Vec<Span<'static>> = Vec::new();
            spans.push(Span::raw(" ".repeat(logo_pad)));
            for (col, ch) in trimmed.chars().enumerate() {
                let diag = (col as f32 + (l_rows - 1.0 - row as f32)) / (l_cols + l_rows);
                let t = (0.15 + 0.85 * diag).clamp(0.0, 1.0);
                let color = crate::tui::blend_color(base, hilite, t);
                spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
            }
            welcome_lines.push(Line::from(spans));
        }
        welcome_lines.push(Line::from(""));
        let banner_raw = format!("gray {} \u{b7} Run /help for commands", env!("CARGO_PKG_VERSION"));
        let banner_len = banner_raw.chars().count();
        let pad = w.saturating_sub(banner_len) / 2;
        welcome_lines.push(Line::from(vec![
            Span::raw(" ".repeat(pad)),
            Span::styled("gray", Style::default().bold().fg(Color::Rgb(225, 225, 225))),
            Span::styled(format!(" {} \u{b7} Run /help for commands", env!("CARGO_PKG_VERSION")), Style::default().fg(Color::Rgb(140, 140, 140))),
        ]));
        welcome_lines.push(Line::from(""));

        let welcome_h = welcome_lines.len() as u16;
        let _ = terminal.insert_before(welcome_h, |buf| {
            Paragraph::new(welcome_lines.clone()).render(buf.area, buf);
        });

        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        Ok(Self {
            terminal,
            textarea: TextArea::new(),
            matches: Vec::new(),
            sel: 0,
            status: None,
            pending: String::new(),
            truecolor: true,
            thinking: false,
            hide_thinking: true,
            history: Vec::new(),
            history_idx: None,
            draft: String::new(),
            attachments: Vec::new(),
            pending_pastes: Vec::new(),
            model_name: String::new(),
            cwd,
            thinking_effort: "high".to_string(),
            transcript: welcome_lines,
            bot_buffer: None,
            bot_emitted: 0,
            bot_prefix_first: true,
        })
    }

    pub fn set_model(&mut self, model: String) { self.model_name = model; }
    pub fn set_cwd(&mut self, cwd: String) { self.cwd = cwd; }
    pub fn set_thinking_effort(&mut self, effort: String) { self.thinking_effort = effort; }

    fn width(&self) -> usize { self.terminal.size().map(|a| a.width as usize).unwrap_or(80) }

    fn draw(&mut self) -> anyhow::Result<()> {
        self.terminal.draw(|frame| {
            let area = frame.area();
            let w = area.width as usize;

            let text = self.textarea.text().to_string();
            let content_w = w.saturating_sub(4).max(1);

            // Neutral Gray palette (no blue)
            let bg_color = Color::Rgb(22, 22, 22);
            let prompt_color = Color::Rgb(180, 180, 180);
            let text_primary = Color::Rgb(225, 225, 225);

            let mut box_lines: Vec<Line<'static>> = Vec::new();

            // 1. Top margin row (colored row, no symbols)
            box_lines.push(Line::from("").style(Style::default().bg(bg_color)));

            // 2. Prompt input rows
            let prompt_arrow = "❯ ";
            let arrow_span = Span::styled(prompt_arrow, Style::default().fg(prompt_color).add_modifier(Modifier::BOLD).bg(bg_color));

            if text.is_empty() {
                box_lines.push(Line::from(vec![
                    arrow_span.clone(),
                ]).style(Style::default().bg(bg_color)));
            } else {
                let lines_raw: Vec<&str> = text.split('\n').collect();
                for (i, raw_line) in lines_raw.iter().enumerate() {
                    let prefix_span = if i == 0 {
                        arrow_span.clone()
                    } else {
                        Span::styled("  ", Style::default().bg(bg_color))
                    };
                    if raw_line.is_empty() {
                        box_lines.push(Line::from(vec![prefix_span]).style(Style::default().bg(bg_color)));
                    } else {
                        let chars: Vec<char> = raw_line.chars().collect();
                        for (chunk_idx, chunk) in chars.chunks(content_w).enumerate() {
                            let s: String = chunk.iter().collect();
                            if chunk_idx == 0 {
                                box_lines.push(Line::from(vec![
                                    prefix_span.clone(),
                                    Span::styled(s, Style::default().fg(text_primary).bg(bg_color)),
                                ]).style(Style::default().bg(bg_color)));
                            } else {
                                box_lines.push(Line::from(vec![
                                    Span::styled("  ", Style::default().bg(bg_color)),
                                    Span::styled(s, Style::default().fg(text_primary).bg(bg_color)),
                                ]).style(Style::default().bg(bg_color)));
                            }
                        }
                    }
                }
            }

            // 3. Bottom margin row (colored row, no symbols)
            box_lines.push(Line::from("").style(Style::default().bg(bg_color)));

            let box_h = box_lines.len().max(1) as u16;
            let panel_h: u16 = self.matches.len().min(PANEL_ROWS) as u16;
            let has_attach = !self.attachments.is_empty();
            let attach_h: u16 = if has_attach { 1 } else { 0 };
            let has_status = self.status.is_some();
            let status_h: u16 = if has_status { 1 } else { 0 };

            // Render top-down starting right at area.y to eliminate any empty gap
            let box_y = area.y;
            let panel_y = box_y + box_h;
            let attach_y = panel_y + panel_h;
            let status_y = attach_y + attach_h;
            let footer_y = status_y + status_h;

            let rendered_box_h = box_h.min((area.y + area.height).saturating_sub(box_y));
            if rendered_box_h > 0 {
                frame.render_widget(
                    Paragraph::new(box_lines).block(Block::default().style(Style::default().bg(bg_color)).padding(ratatui::widgets::Padding::horizontal(1))),
                    Rect::new(area.x, box_y, area.width, rendered_box_h),
                );
            }

            if !self.matches.is_empty() {
                let start = self.sel.saturating_sub(PANEL_ROWS.saturating_sub(1)).min(self.sel);
                let visible_count = self.matches.len().min(PANEL_ROWS);
                for (i, (name, desc)) in self.matches.iter().enumerate().skip(start).take(visible_count) {
                    let y = (i - start) as u16;
                    let item_y = panel_y + y;
                    if item_y < area.y || item_y >= area.y + area.height {
                        continue;
                    }
                    let is_sel = i == self.sel;
                    let cmd_str = format!(" /{name} ");
                    let desc_str = format!(" {desc} ");
                    let line = if is_sel {
                        Line::from(vec![
                            Span::styled(cmd_str, Style::default().fg(Color::Black).bg(Color::Rgb(246, 173, 126)).add_modifier(Modifier::BOLD)),
                            Span::styled(desc_str, Style::default().fg(Color::Rgb(40, 40, 40)).bg(Color::Rgb(246, 173, 126))),
                        ])
                    } else {
                        Line::from(vec![
                            Span::styled(cmd_str, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(Color::Rgb(28, 28, 28))),
                            Span::styled(desc_str, Style::default().fg(Color::Rgb(140, 140, 140)).bg(Color::Rgb(28, 28, 28))),
                        ])
                    };
                    frame.render_widget(Paragraph::new(line), Rect::new(area.x, item_y, area.width, 1));
                }
            }

            if has_attach && attach_y < area.y + area.height {
                let label = self.attachments.iter().enumerate().map(|(i,p)| format!("[Image #{} {}]", i+1, p.display())).collect::<Vec<_>>().join(" ");
                frame.render_widget(Paragraph::new(Line::from(label.dim())), Rect::new(area.x, attach_y, area.width, 1));
            }

            if let Some((started, label)) = &self.status {
                if status_y < area.y + area.height {
                    let text = format!("\u{2022} {label}\u{2026} {}s (ctrl-c to cancel)", started.elapsed().as_secs());
                    let spans = shimmer_spans(&text, started.elapsed(), self.truecolor);
                    frame.render_widget(Paragraph::new(Line::from(spans)), Rect::new(area.x, status_y, area.width, 1));
                }
            }

            let cwd_display = if self.cwd.is_empty() { std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default() } else { self.cwd.clone() };
            let model_display = crate::setup::friendly_model_name(&self.model_name);
            let effort_display = &self.thinking_effort;
            let right_parts = if model_display.is_empty() {
                vec![Span::styled(effort_display.clone(), Style::default().fg(Color::Rgb(108, 108, 108)))]
            } else {
                vec![
                    Span::styled(model_display.clone(), Style::default().fg(Color::Rgb(140, 140, 140))),
                    Span::styled(" \u{b7} ", Style::default().fg(Color::Rgb(80, 80, 80))),
                    Span::styled(effort_display.clone(), Style::default().fg(Color::Rgb(108, 108, 108))),
                ]
            };
            let right_len = if model_display.is_empty() { effort_display.chars().count() } else { model_display.chars().count() + 3 + effort_display.chars().count() };
            let pad_len = w.saturating_sub(cwd_display.chars().count() + right_len);
            let mut footer_spans = vec![Span::styled(cwd_display, Style::default().fg(Color::Rgb(108, 108, 108))), Span::raw(" ".repeat(pad_len))];
            footer_spans.extend(right_parts);
            if footer_y < area.y + area.height {
                frame.render_widget(Paragraph::new(Line::from(footer_spans)), Rect::new(area.x, footer_y, area.width, 1));
            }

            // Clear any leftover rows below footer_y within the viewport
            let used_bottom = footer_y + 1;
            if used_bottom < area.y + area.height {
                frame.render_widget(
                    ratatui::widgets::Clear,
                    Rect::new(area.x, used_bottom, area.width, (area.y + area.height) - used_bottom),
                );
            }

            let (cursor_line_idx, cursor_col) = {
                let cursor = self.textarea.cursor().min(text.len());
                let before = &text[..cursor];
                (before.matches('\n').count(), before[before.rfind('\n').map(|i| i + 1).unwrap_or(0)..].chars().count())
            };
            let cur_x = (area.x + 3 + cursor_col as u16).min(area.x + area.width.saturating_sub(1));
            let cur_y = (box_y + 1 + cursor_line_idx as u16).min(area.y + area.height.saturating_sub(1));
            frame.set_cursor_position(Position::new(cur_x, cur_y));
        })?;
        Ok(())
    }

    pub fn attach_image(&mut self, path: PathBuf) {
        let idx = self.attachments.len() + 1;
        let placeholder = format!("[Image #{idx}]");
        self.textarea.insert_element(&placeholder);
        self.attachments.push(path);
        let _ = self.draw();
    }

    pub fn handle_paste(&mut self, pasted: String) -> bool {
        const THRESHOLD: usize = 1000;
        let pasted = pasted.replace("\r\n", "\n").replace('\r', "\n");
        let n = pasted.chars().count();
        if n > THRESHOLD {
            let placeholder = format!("[Pasted Content {n} chars]");
            self.textarea.insert_element(&placeholder);
            self.pending_pastes.push((placeholder, pasted));
        } else {
            self.textarea.insert_str(&pasted);
        }
        let _ = self.draw();
        true
    }

    pub fn read_line(&mut self) -> anyhow::Result<Option<String>> {
        use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind};
        crossterm::terminal::enable_raw_mode()?;
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture);
        crossterm::execute!(std::io::stdout(), crossterm::cursor::Show)?;
        let _ = self.terminal.clear();
        loop {
            let cur_text = self.textarea.text().to_string();
            self.matches = if cur_text.starts_with('/') && !cur_text[1..].contains(char::is_whitespace) {
                completion_matches(&cur_text[1..])
            } else { Vec::new() };
            if self.sel >= self.matches.len() { self.sel = self.matches.len().saturating_sub(1); }
            self.draw()?;
            if !poll(Duration::from_millis(250))? { continue; }
            match read()? {
                Event::Resize(_, _) => {}
                Event::Mouse(MouseEvent { kind: MouseEventKind::ScrollUp, .. }) => {
                    if !self.matches.is_empty() {
                        self.sel = self.sel.saturating_sub(1);
                    } else if self.history_idx.is_some() {
                        let idx = self.history_idx.unwrap();
                        if idx > 0 {
                            self.history_idx = Some(idx - 1);
                            let h = self.history[idx - 1].clone();
                            self.textarea.set_text(&h);
                            self.textarea.move_to_end();
                        }
                    } else if !self.history.is_empty() {
                        self.history_idx = Some(self.history.len() - 1);
                        self.draft = self.textarea.text().to_string();
                        let h = self.history.last().unwrap().clone();
                        self.textarea.set_text(&h);
                        self.textarea.move_to_end();
                    }
                }
                Event::Mouse(MouseEvent { kind: MouseEventKind::ScrollDown, .. }) => {
                    if !self.matches.is_empty() {
                        self.sel = (self.sel + 1).min(self.matches.len().saturating_sub(1));
                    } else if self.history_idx.is_some() {
                        let idx = self.history_idx.unwrap();
                        if idx + 1 >= self.history.len() {
                            self.textarea.set_text(&self.draft);
                            self.textarea.move_to_end();
                            self.history_idx = None;
                        } else {
                            self.history_idx = Some(idx + 1);
                            let h = self.history[idx + 1].clone();
                            self.textarea.set_text(&h);
                            self.textarea.move_to_end();
                        }
                    }
                }
                Event::Mouse(_) => {}
                Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. }) if modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
                Event::Key(KeyEvent { code: KeyCode::Char('d'), modifiers, kind: KeyEventKind::Press, .. }) if modifiers.contains(KeyModifiers::CONTROL) && self.textarea.is_empty() => return Ok(None),
                Event::Key(KeyEvent { code, modifiers, kind: KeyEventKind::Press, .. }) if modifiers.contains(KeyModifiers::CONTROL) => match code {
                    KeyCode::Char('p') => self.sel = self.sel.saturating_sub(1),
                    KeyCode::Char('n') => self.sel = (self.sel + 1).min(self.matches.len().saturating_sub(1)),
                    KeyCode::Char('u') => { self.textarea.set_text(""); self.history_idx = None; }
                    KeyCode::Char('a') => self.textarea.set_cursor(0),
                    KeyCode::Char('e') => self.textarea.move_to_end(),
                    KeyCode::Char('k') => {
                        let cur = self.textarea.cursor();
                        self.textarea.replace_range(cur..usize::MAX, "");
                    }
                    KeyCode::Char('w') => { self.textarea.delete_backward(1); }
                    KeyCode::Char('j') | KeyCode::Char('m') => { self.textarea.insert_str("\n"); }
                    _ => {}
                },
                Event::Key(KeyEvent { code, kind: KeyEventKind::Press, modifiers, .. }) => {
                    match code {
                        KeyCode::Enter => {
                            let is_newline = modifiers.contains(KeyModifiers::SHIFT) || modifiers.contains(KeyModifiers::ALT);
                            if is_newline { self.textarea.insert_str("\n"); continue; }
                            if !self.matches.is_empty() && let Some((name, _)) = self.matches.get(self.sel) {
                                if cur_text != format!("/{name}") && cur_text != format!("/{name} ") {
                                    self.textarea.set_text(&format!("/{name} "));
                                    self.textarea.move_to_end();
                                    continue;
                                }
                            }
                            let mut text = self.textarea.text().to_string();
                            for (ph, full) in &self.pending_pastes { text = text.replace(ph, full); }
                            self.pending_pastes.clear();
                            let trimmed = text.trim().to_string();
                            if trimmed.is_empty() && self.attachments.is_empty() { continue; }
                            if !trimmed.is_empty() {
                                self.history.push(trimmed.clone());
                                if self.history.len() > 100 { self.history.remove(0); }
                            }
                            self.history_idx = None;
                            self.draft.clear();
                            self.textarea.set_text("");
                            self.attachments.clear();
                            self.matches.clear();
                            self.sel = 0;
                            let is_slash_cmd = trimmed.starts_with('/') && !trimmed.contains('\n');
                            if !is_slash_cmd {
                                self.push_user_prompt(&trimmed);
                            }
                            return Ok(Some(trimmed));
                        }
                        KeyCode::Tab => {
                            if let Some((name, _)) = self.matches.get(self.sel) {
                                self.textarea.set_text(&format!("/{name} "));
                                self.textarea.move_to_end();
                            }
                        }
                        KeyCode::Char(c) => {
                            // plain char insert at cursor (codex textarea insert_str)
                            self.textarea.insert_str(&c.to_string());
                            self.history_idx = None;
                            self.sel = 0;
                        }
                        KeyCode::Backspace => {
                            self.textarea.delete_backward(1);
                            self.sel = 0;
                        }
                        KeyCode::Delete => {
                            self.textarea.delete_forward(1);
                            self.sel = 0;
                        }
                        KeyCode::Esc => {
                            self.textarea.set_text("");
                            self.attachments.clear();
                            self.history_idx = None;
                            self.sel = 0;
                        }
                        KeyCode::Left => self.textarea.move_left(),
                        KeyCode::Right => self.textarea.move_right(),
                        KeyCode::Up => {
                            if !self.matches.is_empty() {
                                self.sel = self.sel.saturating_sub(1);
                            } else {
                                // history navigation when at top or single-line; otherwise move cursor up (codex)
                                let has_multiline = self.textarea.text().contains('\n');
                                let at_top = self.textarea.cursor() == 0 || !has_multiline;
                                if at_top && !self.history.is_empty() {
                                    if self.history_idx.is_none() { self.draft = self.textarea.text().to_string(); self.history_idx = Some(self.history.len()); }
                                    if let Some(idx) = self.history_idx.as_mut() {
                                        if *idx > 0 { *idx -= 1; let h = self.history[*idx].clone(); self.textarea.set_text(&h); self.textarea.move_to_end(); }
                                    }
                                } else { self.textarea.move_up(); }
                            }
                        }
                        KeyCode::Down => {
                            if !self.matches.is_empty() {
                                self.sel = (self.sel + 1).min(self.matches.len().saturating_sub(1));
                            } else if self.history_idx.is_some() {
                                let idx = self.history_idx.unwrap();
                                if idx + 1 >= self.history.len() {
                                    self.textarea.set_text(&self.draft); self.textarea.move_to_end(); self.history_idx = None;
                                } else {
                                    self.history_idx = Some(idx+1); let h = self.history[idx+1].clone(); self.textarea.set_text(&h); self.textarea.move_to_end();
                                }
                            } else { self.textarea.move_down(); }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    pub fn begin_turn(&mut self, label: &str) {
        self.status = Some((Instant::now(), label.to_string()));
        self.bot_buffer = Some(String::new());
        self.bot_emitted = 0;
        self.bot_prefix_first = true;
        let _ = self.draw();
    }
    pub fn set_status(&mut self, label: Option<&str>) {
        self.status = label.map(|l| (Instant::now(), l.to_string()));
        let _ = self.draw();
    }
    pub fn end_turn(&mut self) {
        self.status = None;
        if let Some(buf) = self.bot_buffer.take() {
            let _w = self.width().saturating_sub(BOT_PREFIX_FIRST.len()).max(20);
            let style = gray_markdown::gray_markdown_style().adapt();
            let (out, _) = gray_markdown::render_markdown_ratatui_full(&buf, style, true, None);
            let lines = out.lines;
            let new_lines = if lines.len() > self.bot_emitted { lines[self.bot_emitted..].to_vec() } else { Vec::new() };
            for line in new_lines {
                let prefix = if self.bot_prefix_first { BOT_PREFIX_FIRST } else { BOT_PREFIX_REST };
                self.bot_prefix_first = false;
                let styled = if line.spans.is_empty() {
                    Line::from(vec![Span::styled(prefix, Style::default().add_modifier(Modifier::DIM)), Span::raw("")])
                } else {
                    let mut spans = vec![Span::styled(prefix, Style::default().add_modifier(Modifier::DIM))];
                    spans.extend(line.spans.clone());
                    Line::from(spans)
                };
                self.push_line_spans(styled);
            }
            self.bot_emitted = 0;
            self.bot_prefix_first = true;
        }
        if !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            let style = if self.thinking { thinking_style() } else { Style::default() };
            self.push_line_styled(rest, style);
        }
        let _ = std::io::stdout().flush();
        let _ = self.draw();
    }
    pub fn stream(&mut self, chunk: &str) {
        if self.bot_buffer.is_some() {
            self.stream_text(chunk);
            return;
        }
        self.pending.push_str(&strip_ansi(chunk));
        while let Some(idx) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=idx).collect();
            let line = line.trim_end_matches('\n').to_string();
            let style = if self.thinking { thinking_style() } else { Style::default() };
            self.push_line_styled(line, style);
        }
        let _ = self.draw();
    }
    pub fn stream_thinking(&mut self, chunk: &str) {
        if !self.thinking {
            self.thinking = true;
            self.set_status(Some("Thinking"));
            if !self.hide_thinking {
                self.push_line(String::new());
            }
        }
        if !self.hide_thinking {
            if self.bot_buffer.is_some() {
                self.stream_text(chunk);
            } else {
                self.stream(chunk);
            }
        }
    }
    pub fn set_hide_thinking(&mut self, hide: bool) { self.hide_thinking = hide; }
    pub fn stream_text(&mut self, chunk: &str) {
        self.end_thinking_run(true);
        self.set_status(Some("Working"));
        if self.bot_buffer.is_some() {
            let _w = self.width().saturating_sub(BOT_PREFIX_FIRST.len()).max(20);
            let buf_mut = self.bot_buffer.as_mut().unwrap();
            buf_mut.push_str(chunk);
            let style = gray_markdown::gray_markdown_style().adapt();
            let snapshot = buf_mut.clone();
            let (out, _) = gray_markdown::render_markdown_ratatui_full(&snapshot, style, true, None);
            let lines = out.lines;
            let new_slice = if lines.len() > self.bot_emitted { lines[self.bot_emitted..].to_vec() } else { Vec::new() };
            for line in new_slice {
                let prefix = if self.bot_prefix_first { BOT_PREFIX_FIRST } else { BOT_PREFIX_REST };
                self.bot_prefix_first = false;
                let styled = if line.spans.is_empty() {
                    Line::from(vec![Span::styled(prefix, Style::default().add_modifier(Modifier::DIM)), Span::raw("")])
                } else {
                    let mut spans = vec![Span::styled(prefix, Style::default().add_modifier(Modifier::DIM))];
                    spans.extend(line.spans.clone());
                    Line::from(spans)
                };
                self.push_line_spans(styled);
            }
            self.bot_emitted = lines.len();
            let _ = self.draw();
            return;
        }
        self.stream(chunk);
    }
    pub fn end_thinking(&mut self) {
        self.end_thinking_run(false);
        let _ = self.draw();
    }
    fn end_thinking_run(&mut self, spacer: bool) {
        if !self.thinking { return; }
        self.thinking = false;
        if !self.hide_thinking {
            if !self.pending.is_empty() {
                let rest = std::mem::take(&mut self.pending);
                self.push_line_styled(rest, thinking_style());
            }
            if spacer {
                self.push_line(String::new());
            }
        } else {
            self.pending.clear();
        }
    }
    pub fn push_user_prompt(&mut self, text: &str) {
        let _ = self.terminal.insert_before(1, |buf| {
            Paragraph::new(Line::from("")).render(buf.area, buf);
        });

        let sanitized = crate::tui::sanitize_user_text(text);
        let w = self.width().max(20);
        let content_w = w.saturating_sub(4).max(1);

        let bg_color = Color::Rgb(22, 22, 22);
        let prompt_color = Color::Rgb(180, 180, 180);
        let text_primary = Color::Rgb(225, 225, 225);

        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(Line::from("").style(Style::default().bg(bg_color)));

        let arrow_span = Span::styled("❯ ", Style::default().fg(prompt_color).add_modifier(Modifier::BOLD).bg(bg_color));

        let lines_raw: Vec<&str> = sanitized.split('\n').collect();
        for (i, raw_line) in lines_raw.iter().enumerate() {
            let prefix_span = if i == 0 {
                arrow_span.clone()
            } else {
                Span::styled("  ", Style::default().bg(bg_color))
            };
            if raw_line.is_empty() {
                lines.push(Line::from(vec![prefix_span]).style(Style::default().bg(bg_color)));
            } else {
                let chars: Vec<char> = raw_line.chars().collect();
                for (chunk_idx, chunk) in chars.chunks(content_w).enumerate() {
                    let s: String = chunk.iter().collect();
                    if chunk_idx == 0 {
                        lines.push(Line::from(vec![
                            prefix_span.clone(),
                            Span::styled(s, Style::default().fg(text_primary).bg(bg_color)),
                        ]).style(Style::default().bg(bg_color)));
                    } else {
                        lines.push(Line::from(vec![
                            Span::styled("  ", Style::default().bg(bg_color)),
                            Span::styled(s, Style::default().fg(text_primary).bg(bg_color)),
                        ]).style(Style::default().bg(bg_color)));
                    }
                }
            }
        }

        lines.push(Line::from("").style(Style::default().bg(bg_color)));

        let height = lines.len() as u16;
        let block = Block::default()
            .style(Style::default().bg(bg_color))
            .padding(ratatui::widgets::Padding::horizontal(1));

        let _ = self.terminal.insert_before(height, |buf| {
            Paragraph::new(lines.clone()).block(block).render(buf.area, buf);
        });
        self.transcript.push(Line::from(""));
        self.transcript.extend(lines);
        if self.transcript.len() > 1000 {
            self.transcript.drain(0..100);
        }
        let _ = std::io::stdout().flush();
    }
    pub fn push_line(&mut self, line: String) { self.push_line_styled(line, Style::default()); }
    fn push_line_styled(&mut self, line: String, style: Style) {
        let w = self.width().max(10);
        let chars: Vec<char> = line.chars().collect();
        let mut height = 0usize;
        for chunk in chars.chunks(w.saturating_sub(1)) {
            let text: String = chunk.iter().collect();
            height += 1;
            let styled_line = Line::from(Span::styled(text.clone(), style));
            self.transcript.push(styled_line.clone());
            let _ = self.terminal.insert_before(1, |buf| { Paragraph::new(styled_line).render(buf.area, buf); });
        }
        if height == 0 {
            self.transcript.push(Line::from(""));
            let _ = self.terminal.insert_before(1, |buf| { Paragraph::new(Line::from("")).render(buf.area, buf); });
        }
        if self.transcript.len() > 1000 {
            self.transcript.drain(0..100);
        }
        let _ = std::io::stdout().flush();
    }
    pub fn push_line_spans(&mut self, line: Line<'static>) {
        self.transcript.push(line.clone());
        if self.transcript.len() > 1000 {
            self.transcript.drain(0..100);
        }
        let _ = self.terminal.insert_before(1, |buf| {
            Paragraph::new(line).render(buf.area, buf);
        });
        let _ = std::io::stdout().flush();
    }
    pub fn push_styled_lines(&mut self, lines: Vec<Line<'static>>) {
        let count = lines.len() as u16;
        if count == 0 {
            return;
        }
        self.transcript.extend(lines.clone());
        if self.transcript.len() > 1000 {
            self.transcript.drain(0..100);
        }
        let _ = self.terminal.insert_before(count, |buf| {
            Paragraph::new(lines).render(buf.area, buf);
        });
        let _ = std::io::stdout().flush();
    }
    pub fn push_dim(&mut self, line: String) {
        let styled = Line::from(Span::styled(line, Style::new().add_modifier(Modifier::DIM)));
        self.transcript.push(styled.clone());
        if self.transcript.len() > 1000 {
            self.transcript.drain(0..100);
        }
        let _ = self.terminal.insert_before(1, |buf| {
            Paragraph::new(styled).render(buf.area, buf);
        });
        let _ = std::io::stdout().flush();
    }
    pub fn push_action(&mut self, text: &str, detail: Option<&str>) {
        let mut spans = vec![
            Span::styled("✓ ", Style::default().fg(Color::Rgb(74, 222, 128)).add_modifier(Modifier::BOLD)),
            Span::styled(text.to_string(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ];
        if let Some(d) = detail {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(d.to_string(), Style::default().fg(Color::Rgb(140, 140, 140))));
        }
        let line = Line::from(spans);
        self.transcript.push(line.clone());
        if self.transcript.len() > 1000 {
            self.transcript.drain(0..100);
        }
        let _ = self.terminal.insert_before(1, |buf| {
            Paragraph::new(line).render(buf.area, buf);
        });
        let _ = std::io::stdout().flush();
    }
    pub fn snapshot(&self) -> crate::setup::BackgroundSnapshot {
        crate::setup::BackgroundSnapshot {
            transcript: self.transcript.clone(),
            cwd: self.cwd.clone(),
            model_name: self.model_name.clone(),
            thinking_effort: self.thinking_effort.clone(),
            prompt_text: self.textarea.text().to_string(),
        }
    }
    pub fn tick_status(&mut self) { let _ = self.draw(); }
    pub fn shutdown(&mut self) {
        let _ = self.terminal.clear();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::cursor::Show,
            crossterm::cursor::MoveToColumn(0),
            crossterm::terminal::Clear(crossterm::terminal::ClearType::FromCursorDown),
        );
        let _ = std::io::stdout().flush();
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::cursor::Show);
    }
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for c2 in chars.by_ref() { if c2.is_ascii_alphabetic() { break; } }
        } else { out.push(c); }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shimmer_spans_change_across_ticks() {
        let text = "\u{2022} Working\u{2026} 1s (ctrl-c to cancel)";
        let mut prev = shimmer_spans(text, Duration::from_millis(300), false);
        for ms in (400..=1200).step_by(100) {
            let cur = shimmer_spans(text, Duration::from_millis(ms), false);
            assert_ne!(prev, cur, "no change between {}ms and {}ms", ms - 100, ms);
            prev = cur;
        }
    }
    #[test]
    fn shimmer_truecolor_changes_across_ticks() {
        let text = "\u{2022} Working\u{2026} 1s";
        let a = shimmer_spans(text, Duration::from_millis(500), true);
        let b = shimmer_spans(text, Duration::from_millis(600), true);
        assert_ne!(a, b);
    }
    #[test]
    fn textarea_multiline_and_history() {
        let mut ta = TextArea::new();
        ta.insert_str("hello");
        ta.insert_str("\nworld");
        assert_eq!(ta.text(), "hello\nworld");
        ta.set_cursor(0);
        ta.move_down();
        assert!(ta.cursor() > 0);
        ta.move_up();
        assert_eq!(ta.cursor(), 0);
        ta.move_to_end();
        assert_eq!(ta.cursor(), ta.text().len());
    }
    #[test]
    fn textarea_atomic_element() {
        let mut ta = TextArea::new();
        ta.insert_str("a");
        ta.insert_element("[Image #1]");
        assert!(ta.text().contains("[Image #1]"));
        let before = ta.cursor();
        ta.move_left();
        // cursor jumps over element atomically
        assert!(ta.cursor() < before);
    }
    #[test]
    fn consecutive_frames_differ_in_test_backend() {
        use ratatui::backend::TestBackend;
        use ratatui::layout::Rect;
        let mut terminal = Terminal::new(TestBackend::new(40, 3)).unwrap();
        let started = Instant::now();
        let frame_at = |ms: u64, term: &mut Terminal<TestBackend>| {
            let fake_started = started - Duration::from_millis(ms);
            term.draw(|f| {
                let text = format!("\u{2022} Working\u{2026} 1s");
                let spans = shimmer_spans(&text, fake_started.elapsed(), false);
                f.render_widget(Paragraph::new(Line::from(spans)), Rect::new(0, 0, 40, 1));
            }).unwrap();
            term.backend().buffer().clone()
        };
        let b1 = frame_at(500, &mut terminal);
        let b2 = frame_at(600, &mut terminal);
        assert_ne!(b1, b2, "ratatui diff should see changing spans across 100ms ticks");
    }
}
