//! Ratatui-backed composer: codex/grok-build architecture sized for gray.
//!
//! An inline viewport (the band: slash-completion panel, status, `❯` input)
//! sits directly under the transcript, which goes into scrollback via
//! `CustomTerminal::insert_before` (codex's viewport model, see `terminal`).
//! Multiline, attachments, slash popup and history are replicated from
//! `codex-rs/tui/src/bottom_pane/chat_composer.rs` and `textarea.rs`
//! (one-file adaptation, stdlib only).

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
use draw::footer_badge_visible;

pub(crate) const PANEL_ROWS: usize = 6;
/// Smallest the viewport shrinks to while idle: box top pad + `❯` row +
/// bottom pad + context footer. No cleared slack below the footer.
pub(crate) const MIN_VIEWPORT_H: u16 = 4;

mod plugin_widget;
mod terminal;
pub(crate) use terminal::CustomTerminal;

type Term = CustomTerminal<CrosstermBackend<Stdout>>;

mod draw;
pub(crate) mod input;
pub(crate) mod transcript;

pub type SharedTui = Arc<std::sync::Mutex<Tui>>;

/// Single-line `✻ Thought for …`: `N tokens` is this turn's billed output
/// token count (exact, from the TurnEnd usage report; reasoning is already
/// included in output, never split out). Falls back to the streamed estimate
/// when no usage report arrived (cancelled/errored turns). Other billed
/// Σ-per-round totals stay out of the TUI line entirely (cost basis lives in
/// `totals` / headless `turn_footer` only). Pure for testability
/// (`Tui::new` needs a TTY).
pub(crate) fn format_thought_line(
    verb: &str,
    elapsed: &str,
    out_tokens: Option<usize>,
    tps: Option<u64>,
) -> String {
    let mut line = format!("✻ {verb} {elapsed}");
    if let Some(c) = out_tokens {
        line.push_str(&format!(" · {} tokens", crate::repl::fmt_usage(c)));
        if let Some(t) = tps {
            line.push_str(&format!(" · {t} tps"));
        }
    }
    line
}

/// The turn-end footer: the SINGLE `✻ Thought for …` line per turn (per-run
/// summaries were deleted — they duplicated every reasoning round and
/// re-stamped bogus 0ms lines on stray trailing chunks, each bringing its
/// own pair of blanks). `N tokens` is this turn's billed output (exact, from
/// the TurnEnd usage report; reasoning is already included in output, never
/// split out) — a per-run line could never know it, and a chars/4 fallback
/// here would reintroduce the 2.5M-on-a-14s-turn inflation the pill dropped.
/// `None` (cancelled/errored before any usage report) prints the bare
/// elapsed. Rate over streaming time only; the clock stays whole-turn on
/// purpose (it is a duration, not a rate denominator). Pure for testability
/// (`Tui::new` needs a TTY).
pub(crate) fn turn_footer_line(
    had_thinking: bool,
    elapsed: Duration,
    turn_toks: Option<usize>,
    stream_ms: u64,
) -> String {
    let elapsed_str =
        crate::repl::format::fmt_duration_ms(elapsed.as_millis().min(u128::from(u64::MAX)) as u64);
    let verb = if had_thinking {
        "Thought for"
    } else {
        "Worked for"
    };
    // Other billed Σ-per-round totals stay out of the TUI line entirely
    // (cost basis lives in `totals` / headless `turn_footer` only).
    let tps = turn_toks.and_then(|toks| crate::repl::turn_tokens_per_second(toks, stream_ms));
    format_thought_line(verb, &elapsed_str, turn_toks, tps)
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

/// Token count for the `Working… · N tokens` pill — opencode2 parity
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

/// Live context total for the footer gauge (bottom): the latest report
/// plus the streamed estimate since that report (bytes/4, the repo's
/// estimate heuristic), or the bare estimate before any report lands.
/// Pure for testability. Shares the streamed delta with the pill so `+1`
/// on top is always `+1` on the bottom, but the bases differ (context vs
/// output).
pub(crate) fn live_context_total(
    usage: Option<gray_core::event::Usage>,
    streamed_bytes: u64,
) -> usize {
    let est = usize::try_from(streamed_bytes / 4).unwrap_or(usize::MAX);
    match usage {
        None => est,
        // The streamed bytes always belong to a round no report covers yet
        // (each report resets the estimate in `set_usage`), so they add on
        // top. The old max-replace froze the pill whenever the estimate sat
        // below the previous round's output (e.g. a no-CoT turn following a
        // 14k-output turn never ticked).
        Some(u) => pill_context_tokens(&u).saturating_add(est),
    }
}

/// `· N tokens` suffix for the working pill (top): live output tokens
/// ([`live_turn_output_tokens`] formatted), empty before the first streamed
/// byte or when it totals zero (mirrors opencode2's `tokens <= 0` guard).
/// Exact reports always win on arrival (the estimate resets with each
/// report), so the counter ticks per chunk mid-stream and snaps exact at
/// TurnEnd, converging to the `Thought for · N tokens` line.
pub(crate) fn live_pill_suffix(turn_output_accum: usize, streamed_bytes: u64) -> String {
    let total = live_turn_output_tokens(turn_output_accum, streamed_bytes);
    if total == 0 {
        String::new()
    } else {
        format!(" · {} tokens", crate::repl::fmt_usage(total))
    }
}

/// Live output tokens for the working-pill TPS readout: completed rounds'
/// exact `StepUsage` outputs plus the streamed estimate since the last
/// report (bytes/4, same heuristic as the token pill). Turn-level, so the
/// live rate converges to the final `Thought for · N tokens · T tps` line.
/// Pure for testability.
pub(crate) fn live_turn_output_tokens(turn_output_accum: usize, streamed_bytes: u64) -> usize {
    let est = usize::try_from(streamed_bytes / 4).unwrap_or(usize::MAX);
    turn_output_accum.saturating_add(est)
}

/// `· N tps` suffix for the working pill. The denominator is
/// streaming-only time (`stream_ms`), so a turn sitting in a tool call
/// never drags the live rate down. Empty before the first output byte or
/// when nothing has streamed yet. Pure for testability.
pub(crate) fn live_tps_suffix(
    turn_output_accum: usize,
    streamed_bytes: u64,
    stream_ms: u64,
) -> String {
    let out = live_turn_output_tokens(turn_output_accum, streamed_bytes);
    crate::repl::turn_tokens_per_second(out, stream_ms)
        .map(|t| format!(" · {t} tps"))
        .unwrap_or_default()
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

/// Cap for viewport-anchored live tool cards: mirrors the queued-preview
/// cap so the 14-row viewport never pushes the input box off.
pub(crate) const MAX_LIVE_TOOLS: usize = 3;

/// Live tool headers for the viewport, oldest-first, capped. Free fn so
/// tests cover the cap/marker policy without `Tui::new` (needs a TTY).
pub(crate) fn live_tool_rows(tools: &[LiveTool], elapsed: Duration) -> Vec<Line<'static>> {
    tools
        .iter()
        .take(MAX_LIVE_TOOLS)
        .map(|t| {
            let mut line = t.header.clone();
            if t.running {
                // ponytail: reuse the status shimmer; only the live bash verb changes.
                let prefix =
                    usize::from(line.spans.first().is_some_and(|s| s.content == "\u{2b22} "));
                if prefix == 1 {
                    line.spans[0].content = "\u{2b21} ".into();
                }
                // The finished verb (`Ran `, `Waited on `, …) shimmers as
                // its live form; a header without one gets `Running `.
                let live = line
                    .spans
                    .get(prefix)
                    .and_then(|s| crate::tool_fmt::live_bash_verb(&s.content));
                let end = prefix + usize::from(live.is_some());
                line.spans.splice(
                    prefix..end,
                    draw::shimmer_spans(live.unwrap_or("Running "), elapsed),
                );
            }
            line
        })
        .collect()
}

/// Overflow count past [`MAX_LIVE_TOOLS`] (pure companion for tests).
pub(crate) fn live_tool_overflow(len: usize) -> usize {
    len.saturating_sub(MAX_LIVE_TOOLS)
}

mod text_area;
pub(crate) use text_area::TextArea;

pub struct Tui {
    pub(crate) terminal: Term,
    background: Option<background::Background>,
    background_closed: bool,
    pub(crate) textarea: TextArea,
    pub(crate) matches: Vec<(String, String)>,
    pub(crate) sel: usize,
    status: Option<(Instant, String)>,
    /// A block boundary was marked and its gap is not paid yet: the next
    /// content row lands one blank row down (see `transcript::margins`).
    pub(crate) gap_owed: bool,
    /// Nesting depth of the DEC 2026 synchronized-update bracket (see
    /// `Tui::begin_sync`): only the outermost begin/end reach the terminal.
    sync_depth: u32,
    /// Nesting depth of `Tui::atomic`; while > 0 `draw` is deferred to the
    /// outermost batch end so a multi-step scrollback commit paints once.
    batch_depth: u32,
    /// A provider round ended before the next text delta.  A punctuation-only
    /// continuation in that first delta is a live-only orphan, not a new row.
    stream_round_boundary: bool,
    /// Whether the current provider round has emitted visible prose.  Keeps
    /// the orphan guard from hiding a legitimate standalone punctuation-only
    /// response after a tool-only round.
    stream_round_had_text: bool,
    /// Index of the prose history block that owns a pending continuation.
    stream_round_target: Option<usize>,
    /// Punctuation-only delta held back one chunk (`stream_text`): the
    /// markdown renderer freezes complete blocks, so a trailing punctuation
    /// chunk (mid-round split or post-interrupt tail) would freeze as its
    /// own lone-`.` paragraph row. Released into the renderer when the next
    /// meaningful delta, thinking, a tool event, or the turn end proves it
    /// wasn't an orphan continuation.
    stream_punct_hold: String,
    active_compaction: Option<ActiveCompaction>,
    turn_started: Option<Instant>,
    turn_had_thinking: bool,
    /// Last retry cause shown this turn: an identical one is not repeated.
    pub(crate) last_retry_detail: Option<String>,
    pub is_task_running: bool,
    /// Bare Enter resumes the last turn when it was interrupted or errored
    /// (opencode "press Enter to continue"). Set by the REPL loop; the input
    /// layer only gates the empty-submit swallow on it.
    pub allow_empty_submit: bool,
    /// An alternate-screen modal owns the terminal: the 100ms ticker must not
    /// draw (its frames land on the modal's screen as duplicated chrome).
    /// Set by with_modal/with_modal_sync around every modal call.
    pub(crate) modal_open: bool,
    pub queued_inputs: std::collections::VecDeque<(String, Vec<PathBuf>)>,
    /// Slash command submitted via Esc mid-turn: cancel + run locally, never to the AI.
    pub local_command: Option<String>,
    pending: String,
    thinking: bool,
    hide_thinking: bool,
    pub(crate) history: Vec<String>,
    pub(crate) history_idx: Option<usize>,
    pub(crate) draft: String,
    pub(crate) attachments: Vec<(String, PathBuf)>,
    pub(crate) pending_pastes: Vec<(String, String)>,
    model_name: String,
    /// Footer label overriding the friendly model name (a composite
    /// selection: `Fusion · Opus 5.5 High + SWE-2 High`).
    model_label: Option<String>,
    cwd: String,
    thinking_effort: String,
    /// Effort-badge visibility snapshotted for the turn in flight.
    /// `Some` freezes the footer right segment's shape until `end_turn`:
    /// the reasoning flag it hides behind is filled asynchronously by
    /// discovery (provider `/models` and models.dev, `repl/mod.rs`), and a
    /// mid-turn flip would resize the right-anchored segment and walk the
    /// model name across the bar while the answer streams.
    /// `None` between turns resolves the flag live, so the converged
    /// provider answer still lands.
    turn_show_effort: Option<bool>,
    /// Footer segment for pending background work (`2 jobs · wake in 28m`),
    /// kept current by `repl::jobs::spawn_footer_poller`; `None` hides it.
    pub(crate) background_work: Option<String>,
    pub(crate) history_entries: Vec<TranscriptEntry>,
    pub transcript: Vec<Line<'static>>,
    pub(crate) last_width: u16,
    /// Height twin of `last_width`: resize detection must fire on ANY
    /// geometry change. Height-only drags never reflowed, so a paint
    /// landing on stale screen math tore scrollback with no repair coming.
    pub(crate) last_height: u16,
    pub latest_usage: Option<gray_core::event::Usage>,
    pub cumulative_usage: Option<gray_core::event::Usage>,
    /// Last provider request seen this session: drives the footer's
    /// prompt-cache warmth countdown and the cache-miss warning. Fed one
    /// `StepUsage` report per round (see [`Tui::note_cache_request`]).
    pub(crate) cache: crate::cache::CacheTracker,
    markdown_renderer: gray_markdown::StreamingMarkdownRenderer,
    committed_markdown_lines: usize,
    pub(crate) pending_resize: Option<(u16, Instant)>,
    /// Billed output tokens from this turn's TurnEnd usage (Σ-per-round).
    /// Display-only: the `Thought for · N tokens` line. `None` (cancelled /
    /// errored before any usage report) prints the bare elapsed, like an
    /// omp turn with no reported usage. Never feeds the context gauge
    /// (that stays `latest_usage`, latest-round size).
    pub(crate) turn_billed_output: Option<usize>,
    /// Assistant-text bytes streamed this turn/round (TextDelta +
    /// ThinkingDelta — both ride inside output_tokens). Feeds both live
    /// estimates (top output pill + bottom context gauge, same delta);
    /// reset per turn and per usage report, exact bills win.
    pub(crate) streamed_bytes: u64,
    /// Exact `StepUsage` output tokens finalized so far this turn
    /// (Σ-per-round). Plus the streamed estimate gives the live pill output
    /// count and the TPS numerator, both converging to the TurnEnd billed
    /// output. Reset per turn; never feeds the context gauge base (that
    /// stays `latest_usage`, latest-round size).
    pub(crate) turn_output_accum: usize,
    /// Streaming-only ms for this turn, mirrored from the REPL's
    /// [`crate::repl::TurnStreamClock`]: the denominator for every live tps
    /// readout. Reset per turn; the end-of-turn `Thought for` line reads the
    /// same value so the pill and the final line agree.
    pub(crate) turn_stream_ms: u64,
    /// Current inline viewport height. `draw` keeps it at the exact-fit
    /// content height (+1 spare cleared row, clamped to
    /// `MIN_VIEWPORT_H..=viewport_cap(rows)`) so there is never a 10-row idle
    /// popups can grow it back up.
    pub(crate) viewport_h: u16,
    /// Live tool cards (pi `ToolExecutionComponent`, viewport-anchored):
    /// streaming `ToolCallStart`/`Progress` and executing `ToolCallEnd`
    /// state, keyed by call id in first-seen order. Rendered above the
    /// input box by `draw`; the scrollback commit at `ToolResult` stays the
    /// single transcript render, so resume/reflow never see this.
    live_tools: Vec<LiveTool>,
    plugin_widget: plugin_widget::Widget,
    pub(crate) ask_modal: Option<AskModal>,
}

/// Inline `host/ask` modal rows, rendered by `draw` above the input box
/// while a sidecar plugin question is live. Owned by `crate::ask` (state +
/// rows); the composer only stores the current snapshot. Never enters
/// `history_entries` or the transcript — the resolved summary does that.
#[derive(Clone, Debug, Default)]
pub(crate) struct AskModal {
    /// (glyph, label, description) rows: header first, options, notes last.
    pub rows: Vec<(String, String, String)>,
    /// Highlighted row index (header counts, so option `i` is `i + 1`).
    pub cursor: usize,
}

/// One in-flight tool call rendered live above the input box while the
/// model streams its args (`streaming`) or the executor runs it
/// (`running`). `header` is always the full
/// [`crate::tool_fmt::format_tool_call_header`]-family line so the live
/// card keeps its command styling; execution adds a shimmering leading label
/// instead of the completed bash verb. Never enters `history_entries`.
#[derive(Clone, Debug)]
pub(crate) struct LiveTool {
    pub(crate) id: String,
    pub(crate) header: Line<'static>,
    pub(crate) running: bool,
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
    /// The graychan art from `/hehe`, stored as a marker rather than a line
    /// snapshot: `/hehe` again drops this entry (reverting to the default
    /// ASCII banner) and a reflow re-derives the art at the new width.
    Mascot,
    /// One live reasoning run as raw source text (`\n`-terminated logical
    /// lines plus word-cut continuations concatenated back-to-back). The
    /// live path paints incrementally without touching history; reflow
    /// re-wraps from this source, so widening re-joins what the live
    /// word-cut split — unlike `StyledLines` fragments, which re-wrap
    /// alone and can only shrink, never expand.
    ThinkingRun(String),
    Gap(usize),
}

/// The welcome banner: the gray ASCII logo, the version line, nothing else.
/// Deliberately not the graychan art — that is `/hehe` only, so the startup
/// screen is the logo and `/hehe` again puts the logo back.
pub fn build_welcome_lines(w: usize) -> Vec<Line<'static>> {
    let base = crate::theme::theme().text_dim;
    let hilite = crate::theme::theme().text_bright;

    let mut welcome_lines: Vec<Line<'static>> = Vec::new();
    welcome_lines.push(Line::from(""));
    let logo_raw = crate::tui::logo_lines();
    let l_rows = logo_raw.len().max(1) as f32;
    let max_logo_w = logo_raw
        .iter()
        .map(|l| display_width(l.trim()))
        .max()
        .unwrap_or(0);
    let l_cols = (max_logo_w as f32).max(1.0);
    let logo_pad = w.saturating_sub(max_logo_w) / 2;
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
        let mut terminal =
            match CustomTerminal::with_options(CrosstermBackend::new(std::io::stdout())) {
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
            background: None,
            background_closed: false,
            textarea: TextArea::new(),
            matches: Vec::new(),
            sel: 0,
            status: None,
            gap_owed: false,
            sync_depth: 0,
            batch_depth: 0,
            stream_round_boundary: false,
            stream_round_had_text: false,
            stream_round_target: None,
            stream_punct_hold: String::new(),
            active_compaction: None,
            turn_started: None,
            turn_had_thinking: false,
            last_retry_detail: None,
            is_task_running: false,
            allow_empty_submit: false,
            modal_open: false,
            queued_inputs: std::collections::VecDeque::new(),
            local_command: None,
            pending: String::new(),
            thinking: false,
            hide_thinking: false,
            history: Vec::new(),
            history_idx: None,
            draft: String::new(),
            attachments: Vec::new(),
            pending_pastes: Vec::new(),
            model_name: String::new(),
            model_label: None,
            cwd,
            thinking_effort: String::new(),
            turn_show_effort: None,
            background_work: None,
            history_entries: vec![TranscriptEntry::Welcome],
            transcript: welcome_lines,
            last_width: cols,
            last_height: rows,
            latest_usage: None,
            cumulative_usage: None,
            cache: crate::cache::CacheTracker::default(),
            markdown_renderer: gray_markdown::StreamingMarkdownRenderer::new(
                gray_markdown::gray_markdown_style(),
                true,
            ),
            committed_markdown_lines: 0,
            pending_resize: None,
            turn_billed_output: None,
            streamed_bytes: 0,
            turn_output_accum: 0,
            turn_stream_ms: 0,
            viewport_h: MIN_VIEWPORT_H,
            live_tools: Vec::new(),
            plugin_widget: plugin_widget::Widget::new(std::env::current_dir().unwrap_or_default()),
            ask_modal: None,
        })
    }

    /// Puts the band back after an alternate-screen modal (codex
    /// `leave_alt_screen`: restore the saved viewport, repaint it).
    /// `LeaveAlternateScreen` restores the main screen exactly as it was, so
    /// the band's row is still right: it is cleared and repainted in place.
    /// It used to rebuild the terminal from a cursor probe, which landed on
    /// the old prompt row, scrolled the screen when the band overflowed it,
    /// and left the old input box painted above the new one (a doubled `❯`
    /// after `/resume`). Nothing is purged or re-emitted either: that is
    /// what destroyed the transcript behind modals.
    pub(crate) fn reanchor_viewport(&mut self, cols: u16) {
        self.last_width = cols;
        if let Ok((_, rows)) = crossterm::terminal::size() {
            self.last_height = rows;
        }
        self.pending_resize = None;
        // Mode 2004 (bracketed paste) is terminal-global; re-assert after any
        // alternate-screen modal in case a child cleared it.
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);
        self.atomic(|t| {
            let _ = t.terminal.clear();
        });
    }

    /// Finishes the live markdown renderer and commits every row past the
    /// last committed offset. Shared by the reflow drain and `flush_markdown`.
    fn commit_markdown_tail(&mut self) {
        // A held punctuation burst is a continuation of the paragraph the
        // renderer is about to finish: fold it into the source before the
        // full re-render, or `finish()` would freeze it as its own lone
        // `.` row — the very thing the hold exists to prevent.
        if !self.stream_punct_hold.is_empty() {
            let held = std::mem::take(&mut self.stream_punct_hold);
            self.markdown_renderer
                .push_and_render(&held, Some(gray_markdown::get_syntect()));
        }
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
                self.ensure_gap();
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

    /// Opens (or nests inside) a DEC 2026 synchronized-update bracket.
    /// Terminals do not nest the mode, so only the outermost call writes
    /// the escape; `end_sync` mirrors it.
    pub(crate) fn begin_sync(&mut self) {
        if self.sync_depth == 0 {
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::terminal::BeginSynchronizedUpdate
            );
        }
        self.sync_depth += 1;
    }

    pub(crate) fn end_sync(&mut self) {
        self.sync_depth = self.sync_depth.saturating_sub(1);
        if self.sync_depth == 0 {
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::terminal::EndSynchronizedUpdate
            );
        }
    }

    /// Runs `f` as ONE atomic terminal frame: every scrollback insert inside
    /// it, plus the viewport repaint, sits in a single synchronized update.
    ///
    /// `insert_before` ends by clearing the viewport and used to run outside
    /// any bracket, so the dock/input box/footer vanished until some later
    /// `draw` (tool results, gaps and reflows all inserted without one) —
    /// the flicker on every tool call. Nested calls coalesce: `draw` is
    /// deferred while a batch is open and runs once when the outermost
    /// batch closes, still inside the bracket.
    pub(crate) fn atomic<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.begin_sync();
        self.batch_depth += 1;
        let out = f(self);
        self.batch_depth -= 1;
        if self.batch_depth == 0 {
            let _ = self.draw();
        }
        self.end_sync();
        out
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
        self.atomic(|t| {
            let _ = draw::settle_band(t);
            let _ = t.terminal.insert_before(height, |buf| {
                let mut p = Paragraph::new(lines.to_vec());
                if let Some(bg) = bg {
                    p = p.block(Block::default().style(Style::default().bg(bg)));
                }
                p.render(buf.area, buf);
            });
        });
    }

    /// Moves in-flight (painted-nowhere / stored-nowhere) buffers into
    /// `history_entries` so an immediately-following clear + re-emit cannot
    /// drop them. Called at the top of [`Self::reflow_on_resize`], before the
    /// scrollback purge.
    ///
    /// Two buffers qualify:
    /// - `pending`: the live thinking tail [`Tui::stream_thinking`] has not
    ///   flushed yet. Folded into the open `ThinkingRun` (raw source, no
    ///   paint — the reflow below repaints everything from source).
    /// - the unfrozen markdown renderer: frozen rows are already committed
    ///   through [`Tui::stream_text`]; only the not-yet-frozen tail is
    ///   forced out here. The renderer only freezes complete block rows, so
    ///   an incremental reflow may re-close an open block (e.g. repeat a
    ///   table header) once the turn's real close arrives — a cosmetic
    ///   duplicate row, never lost text.
    pub(crate) fn drain_inflight_for_reflow(&mut self) {
        if self.thinking && !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            self.append_thinking_text(&rest, false);
        }
        self.commit_markdown_tail();
    }

    /// Codex-style transcript reflow on terminal resize:
    /// Clears scrollback and visible screen, re-anchors the inline viewport at the new dimensions,
    /// and re-emits the stored transcript history so lines wrap cleanly without distortion.
    pub(crate) fn reflow_on_resize(&mut self, new_cols: u16) {
        // One atomic frame: the clear + re-emit of the whole scrollback must
        // never be presented half-done.
        self.atomic(|t| t.reflow_on_resize_inner(new_cols));
    }

    fn reflow_on_resize_inner(&mut self, new_cols: u16) {
        let _ = self.hide_background();
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

        // Codex-style: purge scrollback and the screen; the band starts over
        // on row 0 and the re-emitted history pushes it down.
        let (cols, rows) = crossterm::terminal::size().unwrap_or((new_cols, self.last_height));
        let _ = self
            .terminal
            .clear_scrollback_and_screen(ratatui::layout::Size::new(cols, rows));

        // Re-emit the history through the same funnel that laid it out
        // live (see `transcript::margins`), so a resize keeps every margin.
        let w = new_cols as usize;
        self.transcript.clear();
        self.gap_owed = false;
        let entries = std::mem::take(&mut self.history_entries);
        for entry in &entries {
            match entry {
                TranscriptEntry::Welcome => {
                    // The banner keeps its own spacing, blank rows included.
                    let lines = build_welcome_lines(w);
                    self.insert_paragraph(&lines, None);
                    self.record_rows(lines);
                }
                TranscriptEntry::UserPrompt(text, attached) => {
                    let lines =
                        crate::composer::transcript::format_user_prompt_lines(text, attached, w);
                    self.emit_rows(lines, Some(crate::theme::theme().surface_bg));
                }
                TranscriptEntry::ToolBox { header, body } => {
                    let lines =
                        crate::composer::transcript::format_tool_box_lines(header.clone(), body, w);
                    self.emit_rows(lines, Some(crate::theme::theme().surface_bg));
                }
                TranscriptEntry::StyledLines { lines, hyperlinks } => {
                    self.emit_styled(lines.clone(), hyperlinks.clone());
                }
                TranscriptEntry::ThinkingRun(text) => {
                    let rows = crate::composer::transcript::thinking_run_rows(
                        text,
                        crate::composer::transcript::prose_width(w),
                        false,
                    );
                    self.emit_rows(rows, None);
                }
                TranscriptEntry::Mascot => {
                    if let Some(lines) = crate::mascot::mascot_lines(
                        u16::try_from(w).unwrap_or(u16::MAX),
                        self.last_height.max(1),
                        Some(w),
                    ) {
                        self.emit_styled(lines, Vec::new());
                    }
                }
                TranscriptEntry::Gap(_) => self.gap_owed = true,
            }
        }
        self.history_entries = entries;

        let _ = self.draw();
    }

    /// `/hehe` again: drops the graychan art so the transcript is back to
    /// the default gray ASCII welcome. Scrollback has no per-line delete, so
    /// the removal re-emits the history the way a resize does.
    ///
    /// The toggle flag clears first, before the marker is even looked for:
    /// the history cap evicts the oldest entries, so in a long session the
    /// marker can be gone while `mascot_shown()` still says the art is up.
    /// Returning early then would leave `/hehe` answering nothing forever —
    /// a press that paints nothing and drops nothing. Clearing first makes
    /// the next press paint graychan again.
    ///
    /// Returns false when no mascot entry was there to drop.
    pub(crate) fn pop_mascot(&mut self) -> bool {
        crate::mascot::set_mascot_shown(false);
        if !crate::composer::transcript::drop_mascot_entry(&mut self.history_entries) {
            return false;
        }
        self.reflow_on_resize(self.last_width);
        true
    }

    pub fn set_model(&mut self, model: String) {
        self.model_name = model;
    }
    /// Footer label for the model (`None` = its friendly name).
    pub fn set_model_label(&mut self, label: Option<String>) {
        self.model_label = label;
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
        // Turn-level TPS numerator: every round bills its full output, so
        // the live rate sums each report (converges to the TurnEnd total).
        // Input stays last-report-only (see NOTE below) — only output sums.
        self.turn_output_accum = self.turn_output_accum.saturating_add(usage.output_tokens);
        // New round segment: the report carries this round's exact output,
        // so the streamed estimate restarts rather than double-counting it.
        self.streamed_bytes = 0;
        // NOTE: no per-turn accumulation of the context size. `StepUsage`
        // carries the latest context size (each round's input already
        // contains the full history), so summing `usage.total()` across
        // rounds grows superlinearly (e.g. 16 rounds x ~120k = ~1.9M). The
        // context gauge reads this last report only (opencode2 `usage()`
        // parity); the `Working… · N tok` pill instead reads the turn-level
        // output accum above (exact per-round outputs + stream estimate,
        // same delta as the gauge, output base). Exact turn/session bills
        // live in the TurnEnd totals.
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
        self.streamed_bytes = 0;
        self.turn_output_accum = 0;
        self.turn_stream_ms = 0;
        // New conversation: the cache state belongs to the old context.
        self.cache.reset();
    }

    /// Records one per-round provider usage report (the `StepUsage` arm of
    /// the REPL dispatch) and returns the cache miss it paid for, if any.
    /// `model` is the active model id — a switch re-bills the whole prompt
    /// and shows up as its own miss label. Pricing comes from the same
    /// LiteLLM table `/usage` charges against; an unpriced model reports
    /// tokens with no cost claim.
    pub fn note_cache_request(
        &mut self,
        usage: &gray_core::event::Usage,
        model: &str,
    ) -> Option<crate::cache::CacheMiss> {
        let rate = crate::setup::get_model_rate(model);
        self.cache.note(usage, model, rate, Instant::now())
    }

    /// Time left before the prompt cache goes cold, or `None` when the
    /// provider never reported cache activity (or it already expired).
    pub fn cache_remaining(&self) -> Option<Duration> {
        self.cache.remaining(Instant::now())
    }

    /// True when the prompt cache has expired since the last request, so the
    /// next one re-bills the whole prefix whatever it holds.
    pub fn cache_is_cold(&self) -> bool {
        self.cache.is_cold(Instant::now())
    }

    /// Drops the cache baseline: a compaction replaced the context, so the
    /// next request's prompt is new content, not re-billed content.
    pub fn reset_cache(&mut self) {
        self.cache.reset();
    }

    /// Ends the active-turn cache pause on paths that abort before the
    /// normal `end_turn` lifecycle boundary without refreshing the timer.
    pub(crate) fn resume_cache(&mut self) {
        self.cache.resume(Instant::now());
    }

    /// Ends a completed turn and starts a fresh cache warmth window.
    pub(crate) fn rearm_cache(&mut self) {
        self.cache.rearm(Instant::now());
    }

    /// Mirror of the turn's streaming-only elapsed ms (the tps denominator).
    pub(crate) fn set_turn_stream_ms(&mut self, ms: u64) {
        self.turn_stream_ms = ms;
    }

    /// Stashes the TurnEnd billed output + reasoning counts for the `end_turn`
    /// Thought line. Called from the TurnEnd dispatch arm (which already holds
    /// the billed usage); `end_turn` consumes both exactly once.
    pub fn set_turn_billed(&mut self, output_tokens: usize) {
        self.turn_billed_output = Some(output_tokens);
    }

    /// Upserts a live tool card (pi `updateArgs` / `markExecutionStarted`):
    /// `header` replaces the previous one for the same `id`, first-seen
    /// order preserved; cap mirrors the queued-preview cap so the
    /// 14-row viewport never pushes the input box off.
    pub(crate) fn upsert_live_tool(&mut self, id: &str, header: Line<'static>, running: bool) {
        if let Some(slot) = self.live_tools.iter_mut().find(|t| t.id == id) {
            slot.header = header;
            slot.running = running;
        } else {
            self.live_tools.push(LiveTool {
                id: id.to_string(),
                header,
                running,
            });
        }
        let _ = self.draw();
    }

    /// Drops a live tool card (pi `updateResult` flips the same card:
    /// here the scrollback commit at `ToolResult` is the flip, so the live
    /// card just goes away).
    pub(crate) fn remove_live_tool(&mut self, id: &str) {
        if let Some(pos) = self.live_tools.iter().position(|t| t.id == id) {
            // The band keeps its top as it shrinks; the commit that follows
            // (`push_tool_box`) shifts it down into the rows it gave up.
            self.live_tools.remove(pos);
            let _ = self.draw();
        }
    }

    /// Live tool headers for the viewport, oldest-first, capped so the
    /// input box always stays visible.
    pub(crate) fn live_tool_rows(&self) -> Vec<Line<'static>> {
        let elapsed = self
            .turn_started
            .or_else(|| self.status.as_ref().map(|(started, _)| *started))
            .map(|started| started.elapsed())
            .unwrap_or_default();
        live_tool_rows(&self.live_tools, elapsed)
    }

    /// Overflow count past the live-tool cap (`+N more`, like the queued
    /// preview's `… +N more`).
    pub(crate) fn live_tool_overflow(&self) -> usize {
        live_tool_overflow(self.live_tools.len())
    }

    /// Clears live tool cards without touching scrollback (cancel/error/
    /// turn end: pi `settle_pending_cards` — leftovers never stick).
    pub(crate) fn clear_live_tools(&mut self) {
        if !self.live_tools.is_empty() {
            self.live_tools.clear();
            let _ = self.draw();
        }
    }

    pub(crate) fn width(&self) -> usize {
        self.last_width.max(20) as usize
    }

    pub(crate) fn set_background(&mut self, path: Option<&std::path::Path>) -> anyhow::Result<()> {
        anyhow::ensure!(!self.background_closed, "Gray TUI has closed");
        anyhow::ensure!(
            !self.modal_open,
            "close the modal before changing background"
        );
        let next = if let Some(path) = path {
            anyhow::ensure!(
                std::env::var_os("TMUX").is_none() && std::env::var_os("STY").is_none(),
                "background through tmux/screen is not supported"
            );
            anyhow::ensure!(
                matches!(
                    std::env::var("TERM_PROGRAM")
                        .unwrap_or_default()
                        .to_lowercase()
                        .as_str(),
                    "ghostty" | "kitty"
                ),
                "background requires Ghostty or Kitty"
            );
            Some(background::Background::load(path)?)
        } else {
            None
        };
        self.hide_background()?;
        self.background = next;
        self.draw()
    }

    pub(crate) fn hide_background(&mut self) -> std::io::Result<()> {
        if let Some(bg) = &mut self.background {
            bg.hide(&mut std::io::stdout().lock())?;
        }
        Ok(())
    }

    pub(crate) fn set_modal_open(&mut self, open: bool) {
        if open {
            let _ = self.hide_background();
        }
        self.modal_open = open;
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
        let now = Instant::now();
        // The pending resume is now in flight: drop the flag so neither the
        // ghost nor a bare (empty) Enter mid-turn re-submits it.
        self.allow_empty_submit = false;
        // Freeze the footer's effort badge for this turn: its flag is a
        // process-global cache that background discovery keeps writing,
        // and a flip mid-turn would resize the right-anchored footer text.
        self.turn_show_effort = Some(footer_badge_visible(&self.model_name, None));
        self.cache.pause(now);
        self.stream_round_boundary = false;
        self.stream_round_had_text = false;
        self.stream_round_target = None;
        // Fresh turn: a hold left by an interrupted turn is its orphan tail,
        // not a continuation of what this turn is about to stream.
        self.discard_held_punctuation();
        // Codex `status_controls.rs`: follow-up input and background activity
        // must not obscure an active compaction — keep its header/clock.
        if let Some(active) = self.active_compaction.clone() {
            self.is_task_running = true;
            self.status = Some((active.started_at, COMPACTION_HEADER.to_string()));
            let _ = self.draw();
            return;
        }
        if self.turn_started.is_none() {
            self.turn_started = Some(now);
            self.turn_had_thinking = false;
        }
        self.turn_billed_output = None;
        self.streamed_bytes = 0;
        self.turn_output_accum = 0;
        self.turn_stream_ms = 0;
        self.is_task_running = true;
        self.status = Some((now, label.to_string()));
        let _ = self.draw();
    }
    /// Takes the live status dock label (for `host/ask` modal parking).
    pub(crate) fn take_status(&mut self) -> Option<(Instant, String)> {
        self.status.take()
    }

    /// Restores a parked status dock label (for `host/ask` modal teardown).
    pub(crate) fn restore_status(&mut self, status: Option<(Instant, String)>) {
        self.status = status;
    }

    /// Show (or hide, `None`) the footer's background-work segment.
    pub(crate) fn set_background_work(&mut self, label: Option<String>) {
        if self.background_work != label {
            self.background_work = label;
            let _ = self.draw();
        }
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

    /// Marks the end of a live provider round.  The next text delta may be a
    /// punctuation-only continuation of the already-painted assistant block.
    pub(crate) fn mark_stream_round_boundary(&mut self) {
        if self.is_task_running {
            if self.stream_round_had_text {
                let was_boundary = self.stream_round_boundary;
                // Keep the exact prose block as the anchor. A later tool,
                // warning, compaction, or reconnect row must not steal the
                // punctuation when the continuation finally arrives.
                let target = self.history_entries.iter().rposition(|entry| {
                    matches!(
                        entry,
                        TranscriptEntry::StyledLines { lines, .. }
                            if lines.iter().any(|line| {
                                !crate::composer::transcript::transcript_row_is_blank(line)
                            })
                    )
                });
                if target.is_some() {
                    self.stream_round_target = target;
                    self.stream_round_boundary = true;
                } else if !was_boundary {
                    self.stream_round_target = None;
                    self.stream_round_boundary = false;
                }
            }
            // ToolCallStart can precede the round's StepUsage. Preserve an
            // already-armed continuation guard across that bookkeeping event.
            self.stream_round_had_text = false;
        }
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
        // A compaction is a turn too: same stale-resume rule as `begin_turn`.
        self.allow_empty_submit = false;
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

    pub fn end_turn(&mut self, stream_ms: u64) {
        self.rearm_cache();
        // Codex `turn_runtime.rs`: a turn ending without item completion
        // clears a live compaction silently (no `Context compacted` line).
        self.active_compaction = None;
        // Belt-and-suspenders with the TurnEnd dispatch clear: prompt_turn
        // always calls `end_turn`, so orphaned live cards can never leak
        // across turns even if dispatch missed the event.
        self.live_tools.clear();
        // capture elapsed before clearing
        let elapsed = self.turn_started.take().map(|s| s.elapsed());
        let had_thinking = self.turn_had_thinking;
        self.last_retry_detail = None;
        self.turn_had_thinking = false;
        if self.thinking {
            self.end_thinking_run(true);
        }
        // Finish the final stream while the live boundary rules are still
        // active; otherwise the final markdown tail can reintroduce a
        // duplicate blank immediately before the turn footer.
        self.flush_markdown();
        self.stream_round_boundary = false;
        self.stream_round_had_text = false;
        self.stream_round_target = None;
        // The turn is over: a still-held punctuation burst was the final
        // delta (interrupt/stream end) — an orphan continuation that must
        // not freeze as a lone `.` transcript row.
        self.discard_held_punctuation();
        self.is_task_running = false;
        self.status = None;
        self.turn_show_effort = None;
        // Drop the status dock from the band before the footer lands, so
        // the footer and its gap fill the rows the dock gave up instead of
        // scrolling history while the band is still at its taller height.
        let _ = self.draw();
        // Billed output only (exact, reasoning included). `None` prints the
        // bare elapsed — a chars/4 fallback here would reintroduce the very
        // inflation the pill just dropped (2.5M on a 14s turn).
        let turn_toks = self.turn_billed_output;
        self.turn_billed_output = None;
        self.turn_output_accum = 0;
        self.turn_stream_ms = 0;

        // Profile warnings queued mid-turn (lib code can't print while the
        // viewport is live) surface here as dim transcript lines, once each.
        let warnings = crate::take_profile_warnings();
        if !warnings.is_empty() {
            self.ensure_gap();
            for w in warnings {
                self.push_dim(format!("warning: {w}"));
            }
        }

        if let Some(elapsed) = elapsed {
            let line = turn_footer_line(had_thinking, elapsed, turn_toks, stream_ms);
            self.ensure_gap();
            self.push_dim(line);
            self.ensure_gap();
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
        let widget_changed = self.plugin_widget.refresh();
        // A warm prompt cache keeps the footer countdown (◷ 4m) live, so
        // the 100ms ticker must keep painting while it runs — the same
        // reason the status dock and its shimmer tick.
        let cache_ticking = self.cache_remaining().is_some();
        if self.status.is_none()
            && self.live_tools.is_empty()
            && !self.plugin_widget.active()
            && !widget_changed
            && !cache_ticking
        {
            return;
        }
        let _ = self.draw();
    }
    pub fn shutdown(&mut self) {
        self.background_closed = true;
        let _ = self.hide_background();
        self.background = None;
        // The band's seam row was the gap under the last block; it leaves
        // with the band, so pay it into scrollback before the shell prompt.
        self.gap_owed = true;
        self.pay_gap();
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
        let _ = self.hide_background();
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
        let _ = crossterm::execute!(std::io::stdout(), crossterm::cursor::Show);
    }
}

#[path = "mod_live_tool_tests.rs"]
#[cfg(test)]
mod live_tool_tests;

#[path = "mod_pill_token_tests.rs"]
#[cfg(test)]
mod pill_token_tests;

#[path = "mod_thought_line_tests.rs"]
#[cfg(test)]
mod thought_line_tests;

#[path = "mod_compaction_tests.rs"]
#[cfg(test)]
mod compaction_tests;

mod background;
