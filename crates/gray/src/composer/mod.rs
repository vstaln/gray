//! Ratatui-backed composer: codex/grok-build architecture sized for gray.
//!
//! Inline viewport owns the bottom rows permanently — slash-completion
//! panel, status, `›` input — while transcript goes into scrollback via
//! `Terminal::insert_before`. Multiline, attachments, slash popup and
//! history are replicated from `codex-rs/tui/src/bottom_pane/chat_composer.rs`
//! and `textarea.rs` (ponytail minimal: one-file adaptation, stdlib only).

use std::io::{Stdout, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Widget};
use ratatui::Terminal;

use gray_markdown::HyperlinkTarget;

use crate::repl::completion_matches;

pub(crate) const VIEWPORT_H: u16 = 7;
pub(crate) const PANEL_ROWS: usize = 6;
pub(crate) const RESIZE_DEBOUNCE: Duration = Duration::from_millis(75);

type Term = Terminal<CrosstermBackend<Stdout>>;

mod draw;
pub(crate) use draw::{shimmer_spans, thinking_style};
mod transcript;
pub(crate) use transcript::{batch_insert_before, word_wrap_line, wrap_styled_line};
mod input;

/// Shared handle: main thread drives prompts/streaming, ticker refreshes elapsed.
#[derive(Clone)]
pub struct SharedTui(pub Arc<std::sync::Mutex<Tui>>);

impl std::ops::Deref for SharedTui {
    type Target = Arc<std::sync::Mutex<Tui>>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

mod text_area;
pub(crate) use text_area::{TextArea, TextElement};

pub struct Tui {
    pub(crate) terminal: Term,
    pub(crate) textarea: TextArea,
    matches: Vec<(&'static str, &'static str)>,
    sel: usize,
    status: Option<(Instant, String)>,
    turn_started: Option<Instant>,
    turn_had_thinking: bool,
    pub is_task_running: bool,
    pub queued_inputs: std::collections::VecDeque<(String, Vec<PathBuf>)>,
    pending: String,
    truecolor: bool,
    thinking: bool,
    hide_thinking: bool,
    pending_tokens: Option<String>,
    history: Vec<String>,
    history_idx: Option<usize>,
    draft: String,
    pub(crate) attachments: Vec<(String, PathBuf)>,
    pub(crate) pending_pastes: Vec<(String, String)>,
    model_name: String,
    cwd: String,
    thinking_effort: String,
    pub transcript: Vec<Line<'static>>,
    pub(crate) last_width: u16,
    pub latest_usage: Option<gray_core::event::Usage>,
    pub cumulative_usage: Option<gray_core::event::Usage>,
    markdown_renderer: gray_markdown::StreamingMarkdownRenderer,
    committed_markdown_lines: usize,
    pub(crate) pending_resize: Option<(u16, Instant)>,
}


impl Tui {
    pub fn new() -> anyhow::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);

        let (cols, _rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let mut terminal = Terminal::with_options(
            CrosstermBackend::new(std::io::stdout()),
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Inline(VIEWPORT_H),
            },
        )?;

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
            turn_started: None,
            turn_had_thinking: false,
            is_task_running: false,
            queued_inputs: std::collections::VecDeque::new(),
            pending: String::new(),
            truecolor: true,
            thinking: false,
            hide_thinking: false,
            pending_tokens: None,
            history: Vec::new(),
            history_idx: None,
            draft: String::new(),
            attachments: Vec::new(),
            pending_pastes: Vec::new(),
            model_name: String::new(),
            cwd,
            thinking_effort: "high".to_string(),
            transcript: welcome_lines,
            last_width: cols,
            latest_usage: None,
            cumulative_usage: None,
            markdown_renderer: gray_markdown::StreamingMarkdownRenderer::new(gray_markdown::gray_markdown_style(), true),
            committed_markdown_lines: 0,
            pending_resize: None,
        })
    }

    pub fn set_model(&mut self, model: String) { self.model_name = model; }
    pub fn set_cwd(&mut self, cwd: String) { self.cwd = cwd; }
    pub fn set_thinking_effort(&mut self, effort: String) { self.thinking_effort = effort; }
    pub fn set_usage(&mut self, usage: gray_core::event::Usage) {
        self.latest_usage = Some(usage);
        self.cumulative_usage = Some(match self.cumulative_usage {
            Some(prev) => gray_core::event::Usage {
                input_tokens: prev.input_tokens + usage.input_tokens,
                output_tokens: prev.output_tokens + usage.output_tokens,
                reasoning_tokens: prev.reasoning_tokens + usage.reasoning_tokens,
                cached_tokens: prev.cached_tokens + usage.cached_tokens,
            },
            None => usage,
        });
    }
    pub fn reset_usage(&mut self) {
        self.latest_usage = None;
        self.cumulative_usage = None;
    }

    pub(crate) fn width(&self) -> usize { self.last_width.max(20) as usize }

    pub(crate) fn draw(&mut self) -> anyhow::Result<()> {
        draw::draw(self)
    }

    pub fn attach_image(&mut self, path: PathBuf) {
        input::attach_image(self, path)
    }

    pub(crate) fn sync_attachments(&mut self) {
        input::sync_attachments(self)
    }

    pub(crate) fn is_image_path(path: &str) -> bool {
        input::is_image_path(path)
    }

    pub(crate) fn try_attach_image_paste(&mut self, pasted: &str) -> bool {
        input::try_attach_image_paste(self, pasted)
    }

    pub(crate) fn try_attach_clipboard_image(&mut self) -> bool {
        input::try_attach_clipboard_image(self)
    }

    pub fn handle_paste(&mut self, pasted: String) -> bool {
        input::handle_paste(self, pasted)
    }

    pub fn read_line(&mut self) -> anyhow::Result<Option<(String, Vec<PathBuf>)>> {
        input::read_line(self)
    }

    pub fn begin_turn(&mut self, label: &str) {
        let now = Instant::now();
        if self.turn_started.is_none() {
            self.turn_started = Some(now);
            self.turn_had_thinking = false;
        }
        self.is_task_running = true;
        self.status = Some((now, label.to_string()));
        let _ = self.draw();
    }
    pub fn set_status(&mut self, label: Option<&str>) {
        self.status = label.map(|l| (Instant::now(), l.to_string()));
        let _ = self.draw();
    }
    pub fn end_turn(&mut self) {
        // capture elapsed before clearing
        let elapsed = self.turn_started.take().map(|s| s.elapsed());
        let had_thinking = self.turn_had_thinking;
        self.turn_had_thinking = false;
        self.is_task_running = false;
        self.status = None;
        if !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            let style = if self.thinking { thinking_style() } else { Style::default() };
            for line in rest.split('\n') {
                if !line.is_empty() {
                    self.push_line_styled(line.to_string(), style);
                }
            }
        }
        let output = std::mem::replace(
            &mut self.markdown_renderer,
            gray_markdown::StreamingMarkdownRenderer::new(gray_markdown::gray_markdown_style(), true),
        ).finish_into_output(Some(gray_markdown::get_syntect()));
        if output.lines.len() > self.committed_markdown_lines {
            let remaining_lines: Vec<Line<'static>> = output.lines[self.committed_markdown_lines..].to_vec();
            let offset = self.committed_markdown_lines;
            self.push_styled_lines_with_hyperlinks(remaining_lines, &output.hyperlinks, offset);
        }
        self.committed_markdown_lines = 0;

        let pending_tok = self.pending_tokens.take();
        if let Some(elapsed) = elapsed {
            let secs = elapsed.as_secs_f64();
            let elapsed_str = if secs < 1.0 {
                format!("{}ms", elapsed.as_millis())
            } else if secs < 60.0 {
                // keep one decimal, trim trailing .0
                let s = format!("{secs:.1}s");
                if s.ends_with(".0s") { s.replacen(".0s", "s", 1) } else { s }
            } else {
                let m = (secs as u64) / 60;
                let s = (secs as u64) % 60;
                if s == 0 { format!("{m}m") } else { format!("{m}m {s}s") }
            };
            let verb = if had_thinking { "Thought for" } else { "Worked for" };
            let tok_suffix = if let Some(u) = self.latest_usage {
                format!(" · {} tok", crate::repl::fmt_usage(u.total()))
            } else {
                String::new()
            };
            // Codex-style: ✻ Worked for 6s · N tok (dim)
            let line = format!("✻ {verb} {elapsed_str}{tok_suffix}");
            self.push_dim(line);
        } else if let Some(tok) = pending_tok {
            self.push_dim(tok);
        }
        let _ = std::io::stdout().flush();
        let _ = self.draw();
    }

    pub fn push_usage(&mut self, tok_line: String) {
        self.pending_tokens = Some(tok_line);
    }

    pub fn snapshot(&self) -> crate::setup::BackgroundSnapshot {
        let (used_tokens, cache_hit_rate) = if let Some(u) = self.cumulative_usage.or(self.latest_usage) {
            (u.total(), u.cache_hit_rate())
        } else {
            (0, 0.0)
        };
        crate::setup::BackgroundSnapshot {
            transcript: self.transcript.clone(),
            cwd: self.cwd.clone(),
            model_name: self.model_name.clone(),
            thinking_effort: self.thinking_effort.clone(),
            prompt_text: self.textarea.text().to_string(),
            used_tokens,
            cache_hit_rate,
        }
    }
    pub fn tick_status(&mut self) {
        // The ticker exists for the live turn status. Repainting an idle
        // composer (or one hidden behind a modal) competes for stdout and
        // continually resets the native input caret.
        if self.status.is_none() {
            return;
        }
        if let Some((cols, at)) = self.pending_resize {
            if let Some(elapsed) = Instant::now().checked_duration_since(at) {
                if elapsed >= RESIZE_DEBOUNCE {
                    self.pending_resize = None;
                    self.last_width = cols;
                } else {
                    return;
                }
            } else {
                self.pending_resize = None;
            }
        } else if let Ok((cols, _)) = crossterm::terminal::size() && cols != self.last_width {
            self.pending_resize = Some((cols, Instant::now()));
            return;
        }
        let _ = self.draw();
    }
    pub fn shutdown(&mut self) {
        let _ = self.terminal.clear();
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
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
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
        let _ = crossterm::execute!(std::io::stdout(), crossterm::cursor::Show);
    }
}

fn strip_ansi(s: &str) -> String {
    crate::tui::strip_ansi(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    pub(crate) fn textarea_multiline_and_history() {
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
    pub(crate) fn textarea_atomic_element() {
        let mut ta = TextArea::new();
        ta.insert_str("a");
        ta.insert_element("[Image #1]");
        assert!(ta.text().contains("[Image #1]"));
        let before = ta.cursor();
        ta.move_left();
        // cursor jumps over element atomically
        assert!(ta.cursor() < before);
    }
}
