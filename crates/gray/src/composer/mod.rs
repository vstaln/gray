//! Ratatui-backed composer: codex/grok-build architecture sized for gray.
//!
//! Inline viewport owns the bottom rows permanently — slash-completion
//! panel, status, `›` input — while transcript goes into scrollback via
//! `Terminal::insert_before`. Multiline, attachments, slash popup and
//! history are replicated from `codex-rs/tui/src/bottom_pane/chat_composer.rs`
//! and `textarea.rs` (one-file adaptation, stdlib only).

use std::io::{Stdout, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::backend::CrosstermBackend;
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Widget};
use ratatui::Terminal;

use gray_markdown::HyperlinkTarget;


pub(crate) const PANEL_ROWS: usize = 6;
pub(crate) const VIEWPORT_H: u16 = 10;

type Term = Terminal<CrosstermBackend<Stdout>>;

mod draw;
pub(crate) use draw::thinking_style;
mod transcript;
mod input;

pub type SharedTui = Arc<std::sync::Mutex<Tui>>;

mod text_area;
pub(crate) use text_area::TextArea;

mod question;
pub(crate) use question::{handle_question_key, tick_question};
pub use question::ComposerQuestionAsker;

pub struct Tui {
    pub(crate) terminal: Term,
    pub(crate) textarea: TextArea,
    pub(crate) matches: Vec<(String, String)>,
    pub(crate) sel: usize,
    status: Option<(Instant, String)>,
    turn_started: Option<Instant>,
    turn_had_thinking: bool,
    pub is_task_running: bool,
    /// An alternate-screen modal owns the terminal: the 100ms ticker must not
    /// draw (its frames land on the modal's screen as duplicated chrome).
    /// Set by with_modal/with_modal_sync around every modal call.
    pub(crate) modal_open: bool,
    pub queued_inputs: std::collections::VecDeque<(String, Vec<PathBuf>)>,
    /// Slash command submitted via Esc mid-turn: cancel + run locally, never to the AI.
    pub local_command: Option<String>,
    pending: String,
    truecolor: bool,
    thinking: bool,
    hide_thinking: bool,
    pending_tokens: Option<String>,
    pub(crate) history: Vec<String>,
    pub(crate) history_idx: Option<usize>,
    pub(crate) draft: String,
    pub(crate) attachments: Vec<(String, PathBuf)>,
    pub(crate) pending_pastes: Vec<(String, String)>,
    model_name: String,
    cwd: String,
    thinking_effort: String,
    pub(crate) history_entries: Vec<TranscriptEntry>,
    pub transcript: Vec<Line<'static>>,
    pub(crate) last_width: u16,
    pub latest_usage: Option<gray_core::event::Usage>,
    pub cumulative_usage: Option<gray_core::event::Usage>,
    markdown_renderer: gray_markdown::StreamingMarkdownRenderer,
    committed_markdown_lines: usize,
    pub(crate) pending_resize: Option<(u16, Instant)>,
    pub(crate) live_streamed_tokens: usize,
    // Cron ticking UI — next due job for footer clock
    pub(crate) next_cron: Option<(String, chrono::DateTime<chrono::Utc>)>,
    pub(crate) last_cron_tick: Option<Instant>,
    // request_user_input overlay (codex port) + late non-blocking answers
    pub(crate) active_question: Option<question::QuestionSession>,
    pub pending_question_answers: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) enum TranscriptEntry {
    Welcome,
    UserPrompt(String, Vec<std::path::PathBuf>),
    ToolBox {
        header: Line<'static>,
        body: Vec<Line<'static>>,
    },
    StyledLines {
        lines: Vec<Line<'static>>,
        hyperlinks: Vec<HyperlinkTarget>,
    },
    Gap(usize),
}

pub fn build_welcome_lines(w: usize) -> Vec<Line<'static>> {
    let logo_raw = crate::tui::logo_lines();
    let l_rows = logo_raw.len().max(1) as f32;
    let max_logo_w = logo_raw.iter().map(|l| l.trim().chars().count()).max().unwrap_or(0);
    let l_cols = (max_logo_w as f32).max(1.0);
    let logo_pad = w.saturating_sub(max_logo_w) / 2;

    let base = Color::Rgb(110, 110, 110);
    let hilite = Color::Rgb(240, 240, 240);

    let mut welcome_lines: Vec<Line<'static>> = Vec::new();
    welcome_lines.push(Line::from(""));
    for (row, line) in logo_raw.iter().enumerate() {
        let trimmed = line.trim();
        let mut spans: Vec<Span<'static>> = Vec::new();
        if logo_pad > 0 {
            spans.push(Span::raw(" ".repeat(logo_pad)));
        }
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
    welcome_lines
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
        let welcome_lines = build_welcome_lines(cols as usize);
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
            modal_open: false,
            queued_inputs: std::collections::VecDeque::new(),
            local_command: None,
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
            thinking_effort: String::new(),
            history_entries: vec![TranscriptEntry::Welcome],
            transcript: welcome_lines,
            last_width: cols,
            latest_usage: None,
            cumulative_usage: None,
            markdown_renderer: gray_markdown::StreamingMarkdownRenderer::new(gray_markdown::gray_markdown_style(), true),
            committed_markdown_lines: 0,
            pending_resize: None,
            live_streamed_tokens: 0,
            next_cron: None,
            last_cron_tick: None,
            active_question: None,
            pending_question_answers: Vec::new(),
        })
    }

    /// Re-anchors the inline viewport after an alternate-screen modal
    /// (`EnterAlternateScreen`/`LeaveAlternateScreen` breaks ratatui's
    /// `Inline` anchor, so the next draw would render off-screen).
    /// `LeaveAlternateScreen` already restores the main-screen scrollback,
    /// so unlike `reflow_on_resize` this must NOT clear or re-emit anything —
    /// purging here is what destroyed the transcript behind modals.
    pub(crate) fn reanchor_viewport(&mut self, cols: u16) {
        self.last_width = cols;
        self.pending_resize = None;
        if let Ok(term) = Terminal::with_options(
            CrosstermBackend::new(std::io::stdout()),
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Inline(VIEWPORT_H),
            },
        ) {
            self.terminal = term;
        }
        let _ = self.draw();
    }

    /// Codex-style transcript reflow on terminal resize:
    /// Clears scrollback and visible screen, re-anchors the inline viewport at the new dimensions,
    /// and re-emits the stored transcript history so lines wrap cleanly without distortion.
    pub(crate) fn reflow_on_resize(&mut self, new_cols: u16) {
        self.last_width = new_cols;

        // Codex-style: reset scroll region, clear visible screen and purge scrollback, home cursor
        let mut out = std::io::stdout();
        let _ = write!(out, "\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[3J\x1b[H");
        let _ = out.flush();

        if let Ok(term) = Terminal::with_options(
            CrosstermBackend::new(std::io::stdout()),
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Inline(VIEWPORT_H),
            },
        ) {
            self.terminal = term;
        }

        let w = new_cols as usize;
        let mut new_transcript: Vec<Line<'static>> = Vec::new();
        let entries = std::mem::take(&mut self.history_entries);
        for entry in &entries {
            match entry {
                TranscriptEntry::Welcome => {
                    let lines = build_welcome_lines(w);
                    let th = lines.len() as u16;
                    let _ = self.terminal.insert_before(th, |buf| {
                        Paragraph::new(lines.clone()).render(buf.area, buf);
                    });
                    new_transcript.extend(lines);
                }
                TranscriptEntry::UserPrompt(text, attached) => {
                    let lines = crate::composer::transcript::format_user_prompt_lines(text, attached, w);
                    let th = lines.len() as u16;
                    let block = Block::default().style(Style::default().bg(Color::Rgb(22, 22, 22)));
                    let _ = self.terminal.insert_before(th, |buf| {
                        Paragraph::new(lines.clone()).block(block).render(buf.area, buf);
                    });
                    new_transcript.extend(lines);
                }
                TranscriptEntry::ToolBox { header, body } => {
                    let lines = crate::composer::transcript::format_tool_box_lines(header.clone(), body, w);
                    let th = lines.len() as u16;
                    let block = Block::default().style(Style::default().bg(Color::Rgb(22, 22, 22)));
                    let _ = self.terminal.insert_before(th, |buf| {
                        Paragraph::new(lines.clone()).block(block).render(buf.area, buf);
                    });
                    new_transcript.extend(lines);
                }
                TranscriptEntry::StyledLines { lines, hyperlinks } => {
                    let lines_only = self.render_and_insert_styled_lines(lines, hyperlinks, w);
                    new_transcript.extend(lines_only);
                }
                TranscriptEntry::Gap(need) => {
                    let trailing = new_transcript.iter().rev().take_while(|l| {
                        l.style.bg.is_none() && l.spans.iter().all(|s| s.style.bg.is_none() && s.content.trim().is_empty())
                    }).count();
                    let need_actual = need.saturating_sub(trailing);
                    if need_actual > 0 {
                        let blank: Vec<Line<'static>> = (0..need_actual).map(|_| Line::from("")).collect();
                        let th = need_actual as u16;
                        let _ = self.terminal.insert_before(th, |buf| {
                            Paragraph::new(blank.clone()).render(buf.area, buf);
                        });
                        new_transcript.extend(blank);
                    }
                }
            }
        }
        self.history_entries = entries;
        self.transcript = new_transcript;

        let _ = self.draw();
    }

    pub fn set_model(&mut self, model: String) { self.model_name = model; }
    pub fn set_cwd(&mut self, cwd: String) { self.cwd = cwd; }
    pub fn set_thinking_effort(&mut self, effort: String) { self.thinking_effort = effort; }
    pub fn set_next_cron(&mut self, name: Option<String>, next: Option<chrono::DateTime<chrono::Utc>>) {
        match (name, next) {
            (Some(n), Some(t)) => self.next_cron = Some((n, t)),
            _ => self.next_cron = None,
        }
        let _ = self.draw();
    }
    pub fn set_usage(&mut self, usage: gray_core::event::Usage) {
        self.latest_usage = Some(usage);
        self.live_streamed_tokens = 0;
        self.cumulative_usage = Some(usage);
    }
    pub fn reset_usage(&mut self) {
        self.latest_usage = None;
        self.cumulative_usage = None;
        self.live_streamed_tokens = 0;
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
        self.live_streamed_tokens = 0;
        self.is_task_running = true;
        self.status = Some((now, label.to_string()));
        let _ = self.draw();
    }
    pub fn set_status(&mut self, label: Option<&str>) {
        self.status = label.map(|l| (Instant::now(), l.to_string()));
        let _ = self.draw();
    }
    pub fn flush_markdown(&mut self) {
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
            if self.committed_markdown_lines == 0 {
                self.ensure_gap(1);
            }
            let remaining_lines: Vec<Line<'static>> = output.lines[self.committed_markdown_lines..].to_vec();
            let offset = self.committed_markdown_lines;
            self.push_styled_lines_with_hyperlinks(remaining_lines, &output.hyperlinks, offset);
        }
        self.committed_markdown_lines = 0;
    }

    pub fn end_turn(&mut self) {
        // Question overlay teardown: dropping the session fails any still-
        // pending askers (queued senders) cleanly; cancelled turns land here.
        if self.active_question.take().is_some() {
            self.textarea.set_text("");
        }
        // capture elapsed before clearing
        let elapsed = self.turn_started.take().map(|s| s.elapsed());
        let had_thinking = self.turn_had_thinking;
        self.turn_had_thinking = false;
        self.is_task_running = false;
        self.status = None;
        self.live_streamed_tokens = 0;
        if self.thinking {
            self.end_thinking_run(true);
        }
        self.flush_markdown();

        // Profile warnings queued mid-turn (lib code can't print while the
        // viewport is live) surface here as dim transcript lines, once each.
        let warnings = crate::take_profile_warnings();
        if !warnings.is_empty() {
            self.ensure_gap(1);
            for w in warnings {
                self.push_dim(format!("warning: {w}"));
            }
        }

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
            self.ensure_gap(1);
            self.push_dim(line);
            self.ensure_gap(1);
        } else if let Some(tok) = pending_tok {
            self.ensure_gap(1);
            self.push_dim(tok);
            self.ensure_gap(1);
        }
        let _ = std::io::stdout().flush();
        let _ = self.draw();
    }

    pub fn push_usage(&mut self, tok_line: String) {
        self.pending_tokens = Some(tok_line);
    }

    pub fn snapshot(&self) -> crate::setup::BackgroundSnapshot {
        let (used_tokens, cache_hit_rate) = if let Some(u) = self.latest_usage.or(self.cumulative_usage) {
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
        // A modal owns the screen: any draw here lands on its alternate
        // screen as duplicated/garbled chrome. Skip until it closes.
        if self.modal_open {
            return;
        }
        // Non-blocking question countdown rides the same ticker.
        if self.active_question.is_some() {
            tick_question(self);
        }
        // Cron ticking clock — needs repaint even when idle, once per second
        let needs_cron_tick = if let Some((_, next)) = &self.next_cron {
            let now = chrono::Utc::now();
            let secs = (*next - now).num_seconds();
            let interval = if secs.abs() < 3600 { Duration::from_secs(1) } else { Duration::from_secs(5) };
            self.last_cron_tick.map(|t| t.elapsed() >= interval).unwrap_or(true)
        } else {
            false
        };
        // Reference: codex screen_size.rs + transcript_reflow.rs — trailing 75ms debounce.
        if let Some((cols, deadline)) = self.pending_resize {
            if Instant::now() >= deadline {
                self.pending_resize = None;
                if cols != self.last_width {
                    self.reflow_on_resize(cols);
                    return;
                }
            }
        } else if let Ok((cols, _)) = crossterm::terminal::size() {
            if cols != self.last_width {
                self.pending_resize = Some((cols, Instant::now() + Duration::from_millis(75)));
                if !needs_cron_tick && self.status.is_none() {
                    return;
                }
            }
        }
        if self.status.is_none() && !needs_cron_tick {
            return;
        }
        if needs_cron_tick {
            self.last_cron_tick = Some(Instant::now());
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

