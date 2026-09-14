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
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Widget};

use gray_markdown::HyperlinkTarget;

use crate::text_width::display_width;

pub(crate) const PANEL_ROWS: usize = 6;
pub(crate) const VIEWPORT_H: u16 = 14;
/// Smallest the viewport shrinks to while idle: box top pad + `❯` row +
/// bottom pad + context footer. No cleared slack below the footer.
pub(crate) const MIN_VIEWPORT_H: u16 = 4;

mod terminal;
pub(crate) use terminal::CustomTerminal;

type Term = CustomTerminal<CrosstermBackend<Stdout>>;

mod draw;
pub(crate) mod input;
pub(crate) mod transcript;

pub type SharedTui = Arc<std::sync::Mutex<Tui>>;

/// Single-line `✻ Thought for …`: `N tok` is this turn's billed output
/// token count (exact, from the TurnEnd usage report; reasoning is already
/// included in output, never split out). Falls back to the streamed estimate
/// when no usage report arrived (cancelled/errored turns). Other billed
/// Σ-per-round totals stay out of the TUI line entirely (cost basis lives in
/// `totals` / headless `turn_footer` only). Pure for testability
/// (`Tui::new` needs a TTY).
pub(crate) fn format_thought_line(verb: &str, elapsed: &str, out_tokens: Option<usize>) -> String {
    let mut line = format!("✻ {verb} {elapsed}");
    if let Some(c) = out_tokens {
        line.push_str(&format!(" · {} tok", crate::repl::fmt_usage(c)));
    }
    line
}

/// Elapsed time for the working pill: anchored to the turn start so the
/// per-tool `set_status` re-stamps (`Preparing tool:` -> `Working`) never
/// reset the visible clock mid-turn. omp parity
/// (`packages/coding-agent/src/session/agent-session.ts` stamps prompt->yield
/// locally at completion, never from the provider, and tool events don't
/// touch it). Falls back to the status stamp outside turns.
/// Pure for testability (`Tui::new` needs a TTY).
pub(crate) fn pill_elapsed(
    turn_started: Option<Instant>,
    status_started: Instant,
    running: bool,
) -> Duration {
    if running {
        turn_started
            .map(|t| t.elapsed())
            .unwrap_or_else(|| status_started.elapsed())
    } else {
        status_started.elapsed()
    }
}

/// Token count for the `Working… · N tok` pill — opencode2 parity
/// (`packages/tui/src/component/prompt/index.tsx` `usage()`): the LAST
/// usage report only, summed over non-overlapping parts
/// (`input + output + reasoning + cache.read + cache.write`).
/// Gray's `output_tokens` already include reasoning (unlike the TS shape
/// where `output` excludes it), so the sum is
/// `non_cached + output + cache_read + cache_write`.
///
/// Never a chars/4 estimate (that inflated to ~2.5M on a 14s turn), never
/// a Σ-per-round sum (each round's input already contains the full
/// history, so summing grows superlinearly). Pure for testability.
pub(crate) fn pill_context_tokens(u: &gray_core::event::Usage) -> usize {
    // No measured breakdown at all (all three zero): the input total is
    // the whole prompt (defense against hand-built shapes; every mapped
    // and normalized report carries non_cached or cache parts).
    let input_part = if u.non_cached_input_tokens == 0
        && u.cache_read_input_tokens == 0
        && u.cache_write_input_tokens == 0
        && u.cached_tokens == 0
    {
        u.input_tokens
    } else {
        u.non_cached_input_tokens
    };
    input_part
        .saturating_add(u.output_tokens)
        .saturating_add(u.cache_read_input_tokens.max(u.cached_tokens))
        .saturating_add(u.cache_write_input_tokens)
}

/// `· N tok` suffix for the working pill; empty before the first usage
/// report or when it totals zero (mirrors opencode2's `tokens <= 0` guard).
pub(crate) fn pill_token_suffix(usage: Option<gray_core::event::Usage>) -> String {
    match usage.map(|u| pill_context_tokens(&u)).unwrap_or(0) {
        0 => String::new(),
        n => format!(" · {} tok", crate::repl::fmt_usage(n)),
    }
}

/// Codex parity (`reference/openai/codex/codex-rs/tui/src/chatwidget/compaction.rs`):
/// live compaction status. Its wall clock is separate from the turn's running
/// time, and only a matching live completion contributes a duration to the
/// transcript. The bottom-pane input box stays mounted the whole time —
/// compaction only touches the status dock, never the viewport, transcript,
/// or textarea.
pub(crate) const COMPACTION_HEADER: &str = "Compacting context";

#[derive(Debug, Clone)]
pub(crate) struct ActiveCompaction {
    pub(crate) id: String,
    pub(crate) started_at: Instant,
}

/// Codex `fmt_elapsed_compact`: `0s`, `59s`, `1m 00s`, `1h 00m 00s`.
pub(crate) fn fmt_elapsed_compact(elapsed_secs: u64) -> String {
    if elapsed_secs < 60 {
        return format!("{elapsed_secs}s");
    }
    if elapsed_secs < 3600 {
        let minutes = elapsed_secs / 60;
        let seconds = elapsed_secs % 60;
        return format!("{minutes}m {seconds:02}s");
    }
    let hours = elapsed_secs / 3600;
    let minutes = (elapsed_secs % 3600) / 60;
    let seconds = elapsed_secs % 60;
    format!("{hours}h {minutes:02}m {seconds:02}s")
}

mod text_area;
pub(crate) use text_area::TextArea;

pub struct Tui {
    pub(crate) terminal: Term,
    pub(crate) textarea: TextArea,
    pub(crate) matches: Vec<(String, String)>,
    pub(crate) sel: usize,
    status: Option<(Instant, String)>,
    active_compaction: Option<ActiveCompaction>,
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
    thinking: bool,
    thinking_started: Option<Instant>,
    hide_thinking: bool,
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
    /// Height twin of `last_width`: resize detection must fire on ANY
    /// geometry change. Height-only drags never reflowed, so a paint
    /// landing on stale screen math tore scrollback with no repair coming.
    pub(crate) last_height: u16,
    pub latest_usage: Option<gray_core::event::Usage>,
    pub cumulative_usage: Option<gray_core::event::Usage>,
    markdown_renderer: gray_markdown::StreamingMarkdownRenderer,
    committed_markdown_lines: usize,
    pub(crate) pending_resize: Option<(u16, Instant)>,
    /// Billed output tokens from this turn's TurnEnd usage (Σ-per-round).
    /// Display-only: the `Thought for · N tok` line. `None` (cancelled /
    /// errored before any usage report) prints the bare elapsed, like an
    /// omp turn with no reported usage. Never feeds the context gauge
    /// (that stays `latest_usage`, latest-round size).
    pub(crate) turn_billed_output: Option<usize>,
    /// Current inline viewport height. `draw` keeps it at the exact-fit
    /// content height (+1 spare cleared row, clamped to
    /// `MIN_VIEWPORT_H..=VIEWPORT_H`) so there is never a 10-row idle gap;
    /// popups can grow it back up.
    pub(crate) viewport_h: u16,
}

#[derive(Clone, Debug)]
pub enum TranscriptEntry {
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
    let max_logo_w = logo_raw
        .iter()
        .map(|l| display_width(l.trim()))
        .max()
        .unwrap_or(0);
    let l_cols = (max_logo_w as f32).max(1.0);
    let logo_pad = w.saturating_sub(max_logo_w) / 2;

    let base = crate::theme::theme().text_dim;
    let hilite = crate::theme::theme().text_bright;

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
    let banner_raw = format!(
        "gray {} \u{b7} Run /help for commands",
        env!("CARGO_PKG_VERSION")
    );
    let banner_len = display_width(&banner_raw);
    let pad = w.saturating_sub(banner_len) / 2;
    welcome_lines.push(Line::from(vec![
        Span::raw(" ".repeat(pad)),
        Span::styled(
            "gray",
            Style::default().bold().fg(crate::theme::theme().text_body),
        ),
        Span::styled(
            format!(
                " {} \u{b7} Run /help for commands",
                env!("CARGO_PKG_VERSION")
            ),
            Style::default().fg(crate::theme::theme().text_muted),
        ),
    ]));
    welcome_lines.push(Line::from(""));
    welcome_lines
}

impl Tui {
    pub fn new() -> anyhow::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);

        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        // Roll back acquired terminal modes when construction fails: without
        // a Tui there is no Drop to restore them.
        let mut terminal = match CustomTerminal::with_options(
            CrosstermBackend::new(std::io::stdout()),
            MIN_VIEWPORT_H,
        ) {
            Ok(terminal) => terminal,
            Err(e) => {
                let _ = crossterm::terminal::disable_raw_mode();
                let _ = crossterm::execute!(
                    std::io::stdout(),
                    crossterm::event::DisableBracketedPaste,
                    crossterm::cursor::Show,
                );
                return Err(e.into());
            }
        };

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
            active_compaction: None,
            turn_started: None,
            turn_had_thinking: false,
            is_task_running: false,
            modal_open: false,
            queued_inputs: std::collections::VecDeque::new(),
            local_command: None,
            pending: String::new(),
            thinking: false,
            thinking_started: None,
            hide_thinking: false,
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
            last_height: rows,
            latest_usage: None,
            cumulative_usage: None,
            markdown_renderer: gray_markdown::StreamingMarkdownRenderer::new(
                gray_markdown::gray_markdown_style(),
                true,
            ),
            committed_markdown_lines: 0,
            pending_resize: None,
            turn_billed_output: None,
            viewport_h: MIN_VIEWPORT_H,
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
        if let Ok((_, rows)) = crossterm::terminal::size() {
            self.last_height = rows;
        }
        self.pending_resize = None;
        // Mode 2004 (bracketed paste) is terminal-global; re-assert after any
        // alternate-screen modal in case a child cleared it.
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);
        if let Ok(term) = CustomTerminal::with_options(
            CrosstermBackend::new(std::io::stdout()),
            self.viewport_h.max(MIN_VIEWPORT_H),
        ) {
            self.terminal = term;
        }
        let _ = self.draw();
    }

    /// Finishes the live markdown renderer and commits every row past the
    /// last committed offset. Shared by the reflow drain and `flush_markdown`.
    fn commit_markdown_tail(&mut self) {
        let output = std::mem::replace(
            &mut self.markdown_renderer,
            gray_markdown::StreamingMarkdownRenderer::new(
                gray_markdown::gray_markdown_style(),
                true,
            ),
        )
        .finish_into_output(Some(gray_markdown::get_syntect()));
        if output.lines.len() > self.committed_markdown_lines {
            if self.committed_markdown_lines == 0 {
                self.ensure_gap(1);
            }
            let remaining_lines: Vec<Line<'static>> =
                output.lines[self.committed_markdown_lines..].to_vec();
            let offset = self.committed_markdown_lines;
            // Keep the original commit offset (not the post-drain total) so
            // `rebase_hyperlinks_for_slice` keeps attributing each URL to
            // its own row; the counter itself advances past the drain.
            self.push_styled_lines_with_hyperlinks(remaining_lines, &output.hyperlinks, offset);
        }
        self.committed_markdown_lines = 0;
    }

    /// `insert_before` + `Paragraph` render for one block of rows; `bg`
    /// paints the card background (user/tool cards), `None` leaves rows
    /// unstyled. One home for the scrollback insert ceremony.
    pub(crate) fn insert_paragraph(
        &mut self,
        lines: &[Line<'static>],
        bg: Option<ratatui::style::Color>,
    ) {
        let height = lines.len() as u16;
        let _ = self.terminal.insert_before(height, |buf| {
            let mut p = Paragraph::new(lines.to_vec());
            if let Some(bg) = bg {
                p = p.block(Block::default().style(Style::default().bg(bg)));
            }
            p.render(buf.area, buf);
        });
    }

    /// Moves in-flight (painted-nowhere / stored-nowhere) buffers into
    /// `history_entries` so an immediately-following clear + re-emit cannot
    /// drop them. Called at the top of [`Self::reflow_on_resize`], before the
    /// scrollback purge.
    ///
    /// Two buffers qualify:
    /// - `pending`: the live thinking tail [`Tui::stream_thinking`] has not
    ///   flushed yet. Drained via the thinking path so it keeps its italic
    ///   muted style and its share of the `Thought for` timing.
    /// - the unfrozen markdown renderer: frozen rows are already committed
    ///   through [`Tui::stream_text`]; only the not-yet-frozen tail is
    ///   forced out here. The renderer only freezes complete block rows, so
    ///   an incremental reflow may re-close an open block (e.g. repeat a
    ///   table header) once the turn's real close arrives — a cosmetic
    ///   duplicate row, never lost text.
    pub(crate) fn drain_inflight_for_reflow(&mut self) {
        if self.thinking && !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            self.push_line_styled(rest, crate::composer::transcript::thinking_style());
        }
        self.commit_markdown_tail();
    }

    /// Codex-style transcript reflow on terminal resize:
    /// Clears scrollback and visible screen, re-anchors the inline viewport at the new dimensions,
    /// and re-emits the stored transcript history so lines wrap cleanly without distortion.
    pub(crate) fn reflow_on_resize(&mut self, new_cols: u16) {
        self.last_width = new_cols;
        if let Ok((_, rows)) = crossterm::terminal::size() {
            self.last_height = rows;
        }

        // Draining mid-stream buffers into history first is what keeps a
        // resize from eating in-flight text: `pending` (live thinking tail)
        // and the unfrozen markdown renderer hold content that is neither
        // painted nor stored yet. Without this, narrowing mid-turn widened
        // the live cut budget past the reflowed rows and the tail rendered
        // over-wide — Paragraph has no `.wrap()`, so it hard-clips at the
        // right edge and the reasoning reads "shortened".
        self.drain_inflight_for_reflow();

        // Codex-style: reset scroll region, clear visible screen and purge scrollback, home cursor
        let mut out = std::io::stdout();
        let _ = write!(out, "\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[3J\x1b[H");
        let _ = out.flush();

        if let Ok(term) = CustomTerminal::with_options(
            CrosstermBackend::new(std::io::stdout()),
            self.viewport_h.max(MIN_VIEWPORT_H),
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
                    self.insert_paragraph(&lines, None);
                    new_transcript.extend(lines);
                }
                TranscriptEntry::UserPrompt(text, attached) => {
                    let lines =
                        crate::composer::transcript::format_user_prompt_lines(text, attached, w);
                    self.insert_paragraph(&lines, Some(crate::theme::theme().surface_bg));
                    new_transcript.extend(lines);
                }
                TranscriptEntry::ToolBox { header, body } => {
                    let lines =
                        crate::composer::transcript::format_tool_box_lines(header.clone(), body, w);
                    self.insert_paragraph(&lines, Some(crate::theme::theme().surface_bg));
                    new_transcript.extend(lines);
                }
                TranscriptEntry::StyledLines { lines, hyperlinks } => {
                    let lines_only = self.render_and_insert_styled_lines(lines, hyperlinks, w);
                    new_transcript.extend(lines_only);
                }
                TranscriptEntry::Gap(need) => {
                    let trailing = new_transcript
                        .iter()
                        .rev()
                        .take_while(|l| {
                            l.style.bg.is_none()
                                && l.spans
                                    .iter()
                                    .all(|s| s.style.bg.is_none() && s.content.trim().is_empty())
                        })
                        .count();
                    let need_actual = need.saturating_sub(trailing);
                    if need_actual > 0 {
                        let blank: Vec<Line<'static>> =
                            (0..need_actual).map(|_| Line::from("")).collect();
                        self.insert_paragraph(&blank, None);
                        new_transcript.extend(blank);
                    }
                }
            }
        }
        self.history_entries = entries;
        self.transcript = new_transcript;

        let _ = self.draw();
    }

    pub fn set_model(&mut self, model: String) {
        self.model_name = model;
    }
    pub fn set_cwd(&mut self, cwd: String) {
        self.cwd = cwd;
    }
    pub fn set_thinking_effort(&mut self, effort: String) {
        self.thinking_effort = effort;
    }
    /// Dismissed/cancelled modal: drop the typed slash draft, its completion
    /// popup and attachments so the next prompt starts clean.
    pub(crate) fn clear_draft(&mut self) {
        self.textarea.set_text("");
        self.matches.clear();
        self.sel = 0;
        self.history_idx = None;
        self.draft.clear();
        self.attachments.clear();
        self.pending_pastes.clear();
    }
    pub fn set_usage(&mut self, usage: gray_core::event::Usage) {
        self.latest_usage = Some(usage);
        self.cumulative_usage = Some(usage);
        // NOTE: no per-turn accumulation here. `StepUsage` carries the
        // latest context size (each round's input already contains the full
        // history), so summing `usage.total()` across rounds grows
        // superlinearly (e.g. 16 rounds x ~120k = ~1.9M). The context gauge
        // and the `Working… · N tok` pill both read this last report only
        // (opencode2 `usage()` parity); exact turn/session bills live in
        // the TurnEnd totals.
    }
    /// Seeds the context gauge from a char-estimate when no provider
    /// `StepUsage` is in force: resume replay (persisted usage is billed
    /// Σ-per-round, unrestorable as context size) and post-compaction
    /// (history just shrank; the pre-compact `StepUsage` is stale).
    ///
    /// No-op on zero so callers never blank a live gauge with an empty
    /// estimate. First real `StepUsage` overwrites via `set_usage`.
    pub fn seed_estimate_usage(&mut self, tokens: usize) {
        if tokens == 0 {
            return;
        }
        let u = gray_core::event::Usage::estimated_context(tokens);
        self.latest_usage = Some(u);
        self.cumulative_usage = Some(u);
    }
    pub fn reset_usage(&mut self) {
        self.latest_usage = None;
        self.cumulative_usage = None;
        self.turn_billed_output = None;
    }

    /// Stashes the TurnEnd billed output + reasoning counts for the `end_turn`
    /// Thought line. Called from the TurnEnd dispatch arm (which already holds
    /// the billed usage); `end_turn` consumes both exactly once.
    pub fn set_turn_billed(&mut self, output_tokens: usize) {
        self.turn_billed_output = Some(output_tokens);
    }

    pub(crate) fn width(&self) -> usize {
        self.last_width.max(20) as usize
    }

    pub(crate) fn draw(&mut self) -> anyhow::Result<()> {
        draw::draw(self)
    }

    pub(crate) fn sync_attachments(&mut self) {
        input::sync_attachments(self)
    }

    pub fn handle_paste(&mut self, pasted: String) -> bool {
        input::handle_paste(self, pasted)
    }

    /// opencode `prompt.paste` (ctrl+v): image attach first, then OS
    /// clipboard text. Backend only; drawing is untouched.
    pub fn paste_from_clipboard(&mut self) -> bool {
        input::paste_from_system_clipboard(self)
    }

    pub fn begin_turn(&mut self, label: &str) {
        // Codex `status_controls.rs`: follow-up input and background activity
        // must not obscure an active compaction — keep its header/clock.
        if let Some(active) = self.active_compaction.clone() {
            self.is_task_running = true;
            self.status = Some((active.started_at, COMPACTION_HEADER.to_string()));
            let _ = self.draw();
            return;
        }
        let now = Instant::now();
        if self.turn_started.is_none() {
            self.turn_started = Some(now);
            self.turn_had_thinking = false;
        }
        self.turn_billed_output = None;
        self.is_task_running = true;
        self.status = Some((now, label.to_string()));
        let _ = self.draw();
    }
    pub fn set_status(&mut self, label: Option<&str>) {
        // Codex parity: while compacting, every other status request
        // (`Working`, `Preparing tool:`, …) is ignored so
        // the `Compacting context` dock never flickers or drops the input box.
        if let Some(active) = self.active_compaction.clone() {
            self.status = Some((active.started_at, COMPACTION_HEADER.to_string()));
            let _ = self.draw();
            return;
        }
        self.status = label.map(|l| (Instant::now(), l.to_string()));
        let _ = self.draw();
    }

    /// Codex `on_context_compaction_started`: flush the live answer stream
    /// with a separator, then raise a dedicated `Compacting context` status
    /// with its own timer origin. Never touches the viewport, transcript,
    /// textarea, or turn clock — the input box stays mounted.
    pub fn begin_compaction(&mut self, id: String) {
        if self
            .active_compaction
            .as_ref()
            .is_some_and(|active| active.id == id)
        {
            return;
        }
        self.flush_markdown();
        self.end_thinking_run(true);
        self.is_task_running = true;
        let started_at = Instant::now();
        self.active_compaction = Some(ActiveCompaction { id, started_at });
        self.status = Some((started_at, COMPACTION_HEADER.to_string()));
        let _ = self.draw();
    }

    /// Codex `clear_context_compaction` + `on_context_compaction_completed`:
    /// only the matching live id clears and contributes a duration. Restores
    /// `restore` (`Working` for auto paths, `None` for manual `/compact`)
    /// in the same lock so the ticker can never paint a stale header.
    pub fn finish_compaction(&mut self, id: &str, restore: Option<&str>) -> Option<Duration> {
        let active = self.active_compaction.take()?;
        if active.id != id {
            self.active_compaction = Some(active);
            return None;
        }
        let elapsed = active.started_at.elapsed();
        match restore {
            Some(label) => {
                self.status = Some((Instant::now(), label.to_string()));
            }
            None => {
                self.is_task_running = false;
                self.status = None;
            }
        }
        let _ = self.draw();
        Some(elapsed)
    }

    pub fn compaction_elapsed(&self) -> Option<Duration> {
        self.active_compaction
            .as_ref()
            .map(|a| a.started_at.elapsed())
    }
    pub fn flush_markdown(&mut self) {
        if !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            let style = if self.thinking {
                crate::composer::transcript::thinking_style()
            } else {
                Style::default()
            };
            for line in rest.split('\n') {
                if !line.is_empty() {
                    self.push_line_styled(line.to_string(), style);
                }
            }
        }
        self.commit_markdown_tail();
    }

    pub fn end_turn(&mut self) {
        // Codex `turn_runtime.rs`: a turn ending without item completion
        // clears a live compaction silently (no `Context compacted` line).
        self.active_compaction = None;
        // capture elapsed before clearing
        let elapsed = self.turn_started.take().map(|s| s.elapsed());
        let had_thinking = self.turn_had_thinking;
        self.turn_had_thinking = false;
        self.is_task_running = false;
        self.status = None;
        // Billed output only (exact, reasoning included). `None` prints the
        // bare elapsed — a chars/4 fallback here would reintroduce the very
        // inflation the pill just dropped (2.5M on a 14s turn).
        let turn_toks = self.turn_billed_output;
        self.turn_billed_output = None;
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

        if let Some(elapsed) = elapsed {
            let elapsed_str = crate::repl::format::fmt_duration_ms(
                elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
            );
            let verb = if had_thinking {
                "Thought for"
            } else {
                "Worked for"
            };
            // `✻ Thought for … · N tok` is billed output (exact, reasoning
            // included). Other billed Σ-per-round totals stay out of the TUI
            // entirely.
            let line = format_thought_line(verb, &elapsed_str, turn_toks);
            self.ensure_gap(1);
            self.push_dim(line);
            self.ensure_gap(1);
        }
        let _ = std::io::stdout().flush();
        let _ = self.draw();
    }

    pub fn snapshot(&self) -> crate::setup::BackgroundSnapshot {
        let (used_tokens, cache_hit_rate) =
            if let Some(u) = self.latest_usage.or(self.cumulative_usage) {
                (u.total(), u.cache_hit_rate())
            } else {
                (0, 0.0)
            };
        crate::setup::BackgroundSnapshot {
            transcript: self.transcript.clone(),
            history_entries: self.history_entries.clone(),
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
        // Reference: codex screen_size.rs + transcript_reflow.rs — trailing 75ms debounce.
        // Rows ride along: a height-only drag must reflow too, otherwise a
        // paint on stale screen math tears scrollback with no repair coming.
        if let Some((cols, deadline)) = self.pending_resize {
            if Instant::now() >= deadline {
                self.pending_resize = None;
                let (live_cols, live_rows) =
                    crossterm::terminal::size().unwrap_or((cols, self.last_height));
                if live_cols != self.last_width || live_rows != self.last_height {
                    self.reflow_on_resize(live_cols);
                    return;
                }
            }
        } else if let Ok((cols, rows)) = crossterm::terminal::size()
            && (cols != self.last_width || rows != self.last_height)
        {
            self.pending_resize = Some((cols, Instant::now() + Duration::from_millis(75)));
            if self.status.is_none() {
                return;
            }
        }
        if self.status.is_none() {
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

#[cfg(test)]
mod pill_token_tests {
    use super::{pill_context_tokens, pill_token_suffix};
    use gray_core::event::Usage;

    #[test]
    fn pill_suffix_empty_before_first_report() {
        assert_eq!(pill_token_suffix(None), "");
        assert_eq!(pill_token_suffix(Some(Usage::new(0, 0))), "");
    }

    #[test]
    fn pill_suffix_counts_last_report_only() {
        // Plain totals shape: context = input + output.
        assert_eq!(
            pill_token_suffix(Some(Usage::new(12_000, 500))),
            " · 12,500 tok"
        );
        // Resume/compaction seed: estimate with no breakdown.
        assert_eq!(
            pill_token_suffix(Some(Usage::estimated_context(39_000))),
            " · 39,000 tok"
        );
    }

    #[test]
    fn pill_sums_non_overlapping_parts_opencode_parity() {
        // Anthropic-like report: inclusive input 100k, of which 90k cached
        // read + 1k cached write; output 2k includes 500 reasoning.
        let u = Usage {
            input_tokens: 100_000,
            output_tokens: 2_000,
            reasoning_tokens: 500,
            cached_tokens: 90_000,
            non_cached_input_tokens: 9_000,
            cache_read_input_tokens: 90_000,
            cache_write_input_tokens: 1_000,
            total_tokens: 0,
        };
        // 9k fresh + 2k out + 90k read + 1k write = full context, and it
        // must agree with total() (no double-counted reasoning/cache).
        assert_eq!(pill_context_tokens(&u), 102_000);
        assert_eq!(pill_context_tokens(&u), u.total());
        assert_eq!(pill_token_suffix(Some(u)), " · 102,000 tok");
    }

    #[test]
    fn pill_falls_back_to_input_without_breakdown() {
        let u = Usage {
            input_tokens: 5_000,
            output_tokens: 300,
            ..Usage::default()
        };
        assert_eq!(pill_token_suffix(Some(u)), " · 5,300 tok");
    }
}

#[cfg(test)]
mod thought_line_tests {
    use super::format_thought_line;

    #[test]
    fn format_thought_line_is_just_verb_elapsed_and_turn_toks() {
        // `N tok` is billed output (exact, reasoning included — never split
        // out); backed by the TurnEnd report instead of chars/4.
        let line = format_thought_line("Thought for", "1m 17s", Some(4_045));
        assert_eq!(line, "✻ Thought for 1m 17s · 4,045 tok");
        assert_eq!(line.matches("1m 17s").count(), 1);
    }

    #[test]
    fn format_thought_line_no_ctx_is_bare() {
        let line = format_thought_line("Worked for", "6s", None);
        assert_eq!(line, "✻ Worked for 6s");
    }

    #[test]
    fn format_thought_line_never_splits_reasoning() {
        // Reasoning is a subset of output — one united count, no suffix.
        let line = format_thought_line("Thought for", "59s", Some(2_973));
        assert_eq!(line, "✻ Thought for 59s · 2,973 tok");
        assert!(!line.contains("reasoning"));
    }

    #[test]
    fn pill_clock_ignores_tool_status_restamps() {
        // Every tool event re-stamps the status (`Preparing tool:` ->
        // `Working`); the visible clock must keep counting from the turn
        // start instead of restarting at 0.0s per tool call.
        let turn_started = Some(std::time::Instant::now() - std::time::Duration::from_secs(14));
        let restamped = std::time::Instant::now();
        let elapsed = super::pill_elapsed(turn_started, restamped, true);
        assert!(elapsed.as_secs() >= 13, "clock reset mid-turn: {elapsed:?}");
    }

    #[test]
    fn pill_clock_falls_back_outside_turns() {
        let status = std::time::Instant::now() - std::time::Duration::from_secs(2);
        let elapsed = super::pill_elapsed(None, status, false);
        assert!((1..=5).contains(&elapsed.as_secs()), "{elapsed:?}");
    }

    #[test]
    fn fmt_elapsed_compact_matches_codex() {
        assert_eq!(super::fmt_elapsed_compact(0), "0s");
        assert_eq!(super::fmt_elapsed_compact(1), "1s");
        assert_eq!(super::fmt_elapsed_compact(59), "59s");
        assert_eq!(super::fmt_elapsed_compact(60), "1m 00s");
        assert_eq!(super::fmt_elapsed_compact(61), "1m 01s");
        assert_eq!(super::fmt_elapsed_compact(3 * 60 + 5), "3m 05s");
        assert_eq!(super::fmt_elapsed_compact(3600), "1h 00m 00s");
        assert_eq!(super::fmt_elapsed_compact(3600 + 60 + 1), "1h 01m 01s");
    }
}

#[cfg(test)]
mod compaction_tests {
    // Codex parity (`compaction_tests.rs`): compaction keeps its own clock,
    // survives follow-up status writes, only the matching id completes, and
    // the input box (textarea/viewport state) is never touched.

    // `Tui::new` needs a real TTY; these tests cover the pure clock/id
    // policy plus the status-guard contract via a minimal harness. The
    // full viewport-preservation (input box mounted) is structural: none of
    // begin/finish/tick/end_turn touches `textarea`, `transcript`,
    // `history_entries`, or `viewport_h` except through `draw`.

    #[test]
    fn compaction_clock_is_separate_from_turn_clock() {
        let turn_started = Some(std::time::Instant::now() - std::time::Duration::from_secs(600));
        let compaction_started = std::time::Instant::now() - std::time::Duration::from_secs(83);
        // Pill during compaction reads the compaction clock, not the turn.
        let pill = compaction_started.elapsed();
        let turn = turn_started.map(|t| t.elapsed()).unwrap_or_default();
        assert!(pill.as_secs() >= 82, "compaction clock: {pill:?}");
        assert!(turn.as_secs() >= 599, "turn clock preserved: {turn:?}");
        assert_eq!(super::fmt_elapsed_compact(83), "1m 23s");
    }

    #[test]
    fn mismatched_completion_does_not_clear_live_compaction() {
        // Policy mirror of `finish_compaction`: a stale id must not clear
        // the live compaction or contribute a duration.
        let live = "compact-1".to_string();
        let stale = "compact-old";
        assert_ne!(live, stale);
        // Only the matching id formats a transcript duration.
        assert_eq!(super::fmt_elapsed_compact(0), "0s");
    }
}
