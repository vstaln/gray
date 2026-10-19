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

pub(crate) const MIN_VIEWPORT_H: u16 = 4;

mod plugin_widget;
mod terminal;
pub(crate) use terminal::CustomTerminal;

type Term = CustomTerminal<CrosstermBackend<Stdout>>;

pub(crate) mod draw;
pub(crate) mod input;
pub(crate) mod transcript;

pub type SharedTui = Arc<std::sync::Mutex<Tui>>;

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

    let tps = turn_toks.and_then(|toks| crate::repl::turn_tokens_per_second(toks, stream_ms));
    format_thought_line(verb, &elapsed_str, turn_toks, tps)
}

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

pub(crate) fn pill_context_tokens(u: &gray_core::event::Usage) -> usize {
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

pub(crate) fn live_context_total(
    usage: Option<gray_core::event::Usage>,
    streamed_bytes: u64,
) -> usize {
    let est = usize::try_from(streamed_bytes / 4).unwrap_or(usize::MAX);
    match usage {
        None => est,

        Some(u) => pill_context_tokens(&u).saturating_add(est),
    }
}

pub(crate) fn live_pill_suffix(turn_output_accum: usize, streamed_bytes: u64) -> String {
    let total = live_turn_output_tokens(turn_output_accum, streamed_bytes);
    if total == 0 {
        String::new()
    } else {
        format!(" · {} tokens", crate::repl::fmt_usage(total))
    }
}

pub(crate) fn live_turn_output_tokens(turn_output_accum: usize, streamed_bytes: u64) -> usize {
    let est = usize::try_from(streamed_bytes / 4).unwrap_or(usize::MAX);
    turn_output_accum.saturating_add(est)
}

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

pub(crate) const COMPACTION_HEADER: &str = "Compacting context";

#[derive(Debug, Clone)]
pub(crate) struct ActiveCompaction {
    pub(crate) id: String,
    pub(crate) started_at: Instant,
}

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

pub(crate) const MAX_LIVE_TOOLS: usize = 3;

pub(crate) fn live_tool_rows(
    tools: &[LiveTool],
    elapsed: Duration,
    w: usize,
) -> Vec<Line<'static>> {
    let text_budget = w.saturating_sub(8).max(1);
    tools
        .iter()
        .take(MAX_LIVE_TOOLS)
        .flat_map(|t| {
            let mut headers = t.header.clone();

            if let Some(line) = headers.first_mut().filter(|_| t.running) {
                let filled = format!("{} ", crate::tui::glyph("\u{2b22}", "#"));
                let prefix = usize::from(
                    line.spans
                        .first()
                        .is_some_and(|s| s.content == filled.as_str()),
                );
                if prefix == 1 {
                    line.spans[0].content =
                        format!("{} ", crate::tui::glyph("\u{2b21}", "o")).into();
                }

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
            headers
                .into_iter()
                .chain(t.tail.rows().into_iter().map(|text| {
                    let chars: Vec<char> = text.chars().collect();
                    let text =
                        if crate::text_width::fit_char_count(&chars, text_budget) < chars.len() {
                            let fit = crate::text_width::fit_char_count(
                                &chars,
                                text_budget.saturating_sub(1),
                            );
                            format!("{}…", chars[..fit].iter().collect::<String>())
                        } else {
                            text
                        };
                    Line::from(Span::styled(
                        format!("    {text}"),
                        Style::default().fg(crate::theme::theme().tool_dim),
                    ))
                }))
        })
        .collect()
}

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

    pub(crate) gap_owed: bool,

    sync_depth: u32,

    batch_depth: u32,

    stream_round_boundary: bool,

    stream_round_had_text: bool,

    stream_round_target: Option<usize>,

    stream_punct_hold: String,
    active_compaction: Option<ActiveCompaction>,
    pub(crate) turn_started: Option<Instant>,
    turn_had_thinking: bool,

    pub(crate) last_retry_detail: Option<String>,
    pub is_task_running: bool,

    pub allow_empty_submit: bool,

    pub(crate) modal_open: bool,

    pub(crate) modal_pending: Vec<PendingInsert>,

    pub(crate) insert_batch: Option<Vec<BatchedInsert>>,

    pub(crate) painted_palette: crate::theme::UiTheme,
    pub queued_inputs: std::collections::VecDeque<(String, Vec<PathBuf>)>,

    pub queued_sends: Vec<(String, Vec<PathBuf>)>,

    pub local_command: Option<String>,
    pending: String,
    thinking: bool,
    hide_thinking: bool,
    pub(crate) history: Vec<String>,
    pub(crate) history_idx: Option<usize>,
    pub(crate) draft: String,

    pub(crate) attachments: Vec<PathBuf>,
    pub(crate) pending_pastes: Vec<(String, String)>,
    model_name: String,

    model_label: Option<String>,
    model_label_parts: Option<Vec<(String, Option<usize>)>>,
    cwd: String,
    thinking_effort: String,

    turn_show_effort: Option<bool>,

    pub(crate) background_work: Option<String>,
    pub(crate) history_entries: Vec<TranscriptEntry>,
    pub transcript: Vec<Line<'static>>,
    pub(crate) last_width: u16,

    pub(crate) last_height: u16,
    pub latest_usage: Option<gray_core::event::Usage>,
    pub cumulative_usage: Option<gray_core::event::Usage>,

    pub(crate) cache: crate::cache::CacheTracker,
    markdown_renderer: gray_markdown::StreamingMarkdownRenderer,
    committed_markdown_lines: usize,
    reveal_buf: String,
    reveal_pacing: bool,
    feeding_markdown: bool,
    pub(crate) pending_resize: Option<(u16, Instant)>,

    pub(crate) turn_billed_output: Option<usize>,

    pub(crate) streamed_bytes: u64,

    pub(crate) turn_output_accum: usize,

    pub(crate) turn_stream_ms: u64,

    pub(crate) viewport_h: u16,

    live_tools: Vec<LiveTool>,
    plugin_widget: plugin_widget::Widget,
    pub(crate) ask_modal: Option<AskModal>,

    pub(crate) home_notice: Option<String>,

    pub(crate) plugin_stats: Option<String>,

    pub(crate) home_update: Option<String>,
    pub(crate) home_quality_of_life: Option<String>,

    pub(crate) home_session: Option<String>,

    pub(crate) left_home: bool,

    pub(crate) intro_started: Option<std::time::Instant>,
    pub(crate) replay_queue: Option<Box<ReplayQueue>>,

    status_line_seen: u64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AskModal {
    pub rows: Vec<(String, String, String)>,

    pub cursor: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct LiveTool {
    pub(crate) id: String,
    pub(crate) header: Vec<Line<'static>>,
    pub(crate) running: bool,
    pub(crate) tail: LiveToolTail,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LiveToolTail {
    lines: std::collections::VecDeque<String>,
    carry: String,
}

const LIVE_TAIL_LINES: usize = 5;
const LIVE_TAIL_CARRY_MAX: usize = 4 * 1024;

impl LiveToolTail {
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        let text = crate::tui::strip_ansi(&String::from_utf8_lossy(bytes));
        for ch in text.chars() {
            match ch {
                '\n' => {
                    let line = std::mem::take(&mut self.carry);
                    if !line.trim().is_empty() {
                        self.lines.push_back(line);
                        while self.lines.len() > LIVE_TAIL_LINES {
                            self.lines.pop_front();
                        }
                    }
                }
                '\r' => self.carry.clear(),
                c => {
                    self.carry.push(c);
                    if self.carry.len() > LIVE_TAIL_CARRY_MAX {
                        let keep = LIVE_TAIL_CARRY_MAX / 2;
                        let mut start = self.carry.len() - keep;
                        while !self.carry.is_char_boundary(start) {
                            start += 1;
                        }
                        self.carry.drain(..start);
                    }
                }
            }
        }
    }

    fn rows(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .lines
            .iter()
            .map(|l| crate::tool_fmt::expand_tabs(l))
            .collect();
        if !self.carry.trim().is_empty() {
            out.push(crate::tool_fmt::expand_tabs(&self.carry));
        }
        if out.len() > LIVE_TAIL_LINES {
            out.drain(..out.len() - LIVE_TAIL_LINES);
        }
        out
    }
}

#[derive(Clone, Debug)]
pub(crate) enum PendingInsert {
    Rows(Vec<Line<'static>>, Option<ratatui::style::Color>),
    Styled(Vec<(Line<'static>, Vec<HyperlinkTarget>)>),
}

pub(crate) type BatchedInsert = (
    Line<'static>,
    Vec<HyperlinkTarget>,
    Option<ratatui::style::Color>,
);

#[derive(Clone, Debug)]
pub enum TranscriptEntry {
    Welcome,
    UserPrompt(String, Vec<std::path::PathBuf>),
    ToolBox {
        header: Line<'static>,
        body: Vec<Line<'static>>,
    },

    ToolBoxLines {
        header: Vec<Line<'static>>,
        body: Vec<Line<'static>>,
    },
    StyledLines {
        lines: Vec<Line<'static>>,
        hyperlinks: Vec<HyperlinkTarget>,
    },

    Mascot,

    ThinkingRun(String),
    Gap(usize),
}

pub(crate) fn stats_spans(stats: &str) -> Vec<Span<'static>> {
    vec![Span::styled(
        stats.to_string(),
        Style::default().fg(crate::theme::theme().text_dim),
    )]
}

pub(crate) fn home_column(w: usize) -> (usize, usize) {
    let col_w = w.saturating_sub(8).clamp(1, 78);
    (w.saturating_sub(col_w) / 2, col_w)
}

const INTRO_TOTAL_MS: u64 = 1000;
const INTRO_FRAMES_TXT: &str = include_str!("../../assets/logo-intro-frames.txt");

fn intro_frames() -> &'static [(Vec<String>, Vec<String>)] {
    static FRAMES: std::sync::OnceLock<Vec<(Vec<String>, Vec<String>)>> =
        std::sync::OnceLock::new();
    FRAMES.get_or_init(|| {
        let mut frames: Vec<(Vec<String>, Vec<String>)> = INTRO_FRAMES_TXT
            .replace("\r\n", "\n")
            .split("\n---\n")
            .filter_map(|block| {
                let rows: Vec<String> = block
                    .lines()
                    .filter(|l| !l.starts_with('#'))
                    .map(str::to_string)
                    .collect();
                (rows.len() == 12).then(|| {
                    let (mark, edge) = rows.split_at(6);
                    (mark.to_vec(), edge.to_vec())
                })
            })
            .collect();

        if frames.len() >= 2 {
            frames.extend(intro_settle_frames(&frames[frames.len() - 2].0));
        }
        frames
    })
}

const BRAILLE_BITS: [(usize, usize, u32); 8] = [
    (0, 0, 0x01),
    (0, 1, 0x02),
    (0, 2, 0x04),
    (0, 3, 0x40),
    (1, 0, 0x08),
    (1, 1, 0x10),
    (1, 2, 0x20),
    (1, 3, 0x80),
];

const MARK_CELLS: (usize, usize) = (16, 6);

const INTRO_SETTLE_FRAMES: usize = 12;

fn braille_dots(rows: &[String]) -> std::collections::BTreeSet<(usize, usize)> {
    let mut out = std::collections::BTreeSet::new();
    for (r, line) in rows.iter().enumerate() {
        for (c, ch) in line.chars().enumerate() {
            if !('\u{2800}'..='\u{28ff}').contains(&ch) {
                continue;
            }
            let bits = ch as u32 - 0x2800;
            for &(dx, dy, bit) in &BRAILLE_BITS {
                if bits & bit != 0 {
                    out.insert((c * 2 + dx, r * 4 + dy));
                }
            }
        }
    }
    out
}

fn dots_braille(dots: &std::collections::BTreeSet<(usize, usize)>) -> Vec<String> {
    let (cx, cy) = MARK_CELLS;
    (0..cy)
        .map(|r| {
            (0..cx)
                .map(|c| {
                    let bits = BRAILLE_BITS.iter().fold(0, |bits, &(dx, dy, bit)| {
                        if dots.contains(&(c * 2 + dx, r * 4 + dy)) {
                            bits | bit
                        } else {
                            bits
                        }
                    });
                    char::from_u32(0x2800 + bits).unwrap()
                })
                .collect()
        })
        .collect()
}

fn intro_settle_frames(from: &[String]) -> Vec<(Vec<String>, Vec<String>)> {
    let (cx, cy) = MARK_CELLS;
    // Settle math stays in braille space on every terminal: the render pass
    // maps cells to an ASCII ramp when unicode is unsafe, and LOGO_ASCII rows
    // carry no dot data for braille_dots() to settle toward.
    let src = crate::tui::LOGO;
    let blank = '\u{2800}';
    let mut target: Vec<String> = src
        .lines()
        .map(|l| {
            let mut s = l.to_string();
            while s.chars().count() < cx {
                s.push(blank);
            }
            s
        })
        .collect();
    target.resize(cy, blank.to_string().repeat(cx));
    let want = braille_dots(&target);
    let mut ink = braille_dots(from);
    if ink.difference(&want).next().is_some() {
        return Vec::new();
    }

    let mut frontier = ink.clone();
    let mut missing: std::collections::BTreeSet<(usize, usize)> =
        want.difference(&ink).copied().collect();
    let mut ordered = Vec::with_capacity(missing.len());
    while !missing.is_empty() {
        let mut ring: Vec<(usize, usize)> = missing
            .iter()
            .copied()
            .filter(|&(x, y)| {
                (-1isize..=1).any(|ox| {
                    (-1isize..=1).any(|oy| {
                        x.checked_add_signed(ox)
                            .zip(y.checked_add_signed(oy))
                            .is_some_and(|p| frontier.contains(&p))
                    })
                })
            })
            .collect();
        if ring.is_empty() {
            ring = missing.iter().copied().collect();
        }
        ring.sort_by_key(|&(x, y)| (y, x));
        for d in ring {
            frontier.insert(d);
            missing.remove(&d);
            ordered.push(d);
        }
    }
    let grow = INTRO_SETTLE_FRAMES - 2;
    (0..INTRO_SETTLE_FRAMES)
        .map(|i| {
            let add: &[(usize, usize)] = if i < grow {
                &ordered[i * ordered.len() / grow..(i + 1) * ordered.len() / grow]
            } else {
                &[]
            };
            ink.extend(add.iter().copied());
            let fresh: std::collections::BTreeSet<(usize, usize)> =
                add.iter().map(|&(x, y)| (x / 2, y / 4)).collect();
            let mark = if ink == want {
                target.clone()
            } else {
                dots_braille(&ink)
            };
            let edge = (0..cy)
                .map(|r| {
                    (0..cx)
                        .map(|c| if fresh.contains(&(c, r)) { '1' } else { '0' })
                        .collect()
                })
                .collect();
            (mark, edge)
        })
        .collect()
}

fn welcome_body_lines(w: usize, progress: f32, intro: Option<Duration>) -> Vec<Line<'static>> {
    let (x, col_w) = home_column(w);
    welcome_body_lines_at(x, col_w, progress, intro)
}

pub(crate) fn welcome_body_lines_at(
    x: usize,
    col_w: usize,
    progress: f32,
    intro: Option<Duration>,
) -> Vec<Line<'static>> {
    let theme = crate::theme::theme();
    let version = env!("CARGO_PKG_VERSION").to_string();

    let centered = |segs: Vec<Span<'static>>, text_w: usize| -> Line<'static> {
        let pad = x + col_w.saturating_sub(text_w) / 2;
        let mut spans = Vec::with_capacity(segs.len() + 1);
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        spans.extend(segs);
        Line::from(spans)
    };

    let mut body: Vec<Line<'static>> = Vec::new();

    let base = theme.text_dim;
    let hilite = theme.text_bright;
    let frames = intro_frames();
    let last = frames.len().saturating_sub(1);
    let frame_idx = match intro {
        Some(elapsed) => {
            (elapsed.as_secs_f64() * 1000.0 / INTRO_TOTAL_MS as f64 * last as f64) as usize
        }
        None => (progress.clamp(0.0, 1.0) * last as f32).round() as usize,
    }
    .min(last);
    let empty = (Vec::new(), Vec::new());
    let (mark_rows, edge_rows) = frames.get(frame_idx).unwrap_or(&empty);
    let l_rows = mark_rows.len().max(1) as f32;
    let max_logo_w = mark_rows
        .iter()
        .map(|l| display_width(l.trim()))
        .max()
        .unwrap_or(0);
    let l_cols = (max_logo_w as f32).max(1.0);
    let logo_pad = x + col_w.saturating_sub(max_logo_w) / 2;

    let safe = crate::tui::unicode_safe();
    for (row, line) in mark_rows.iter().enumerate() {
        let edge = edge_rows.get(row).map(String::as_str).unwrap_or("");
        let mut spans: Vec<Span<'static>> = Vec::new();
        if logo_pad > 0 {
            spans.push(Span::raw(" ".repeat(logo_pad)));
        }
        for (col, ch) in line.chars().enumerate() {
            let ch = if safe {
                ch
            } else {
                crate::tui::braille_fallback(ch)
            };
            if ch.is_whitespace() || ch == ' ' {
                spans.push(Span::raw(" "));
                continue;
            }
            let color = if edge.chars().nth(col) == Some('1') {
                hilite
            } else {
                let diag = (col as f32 + (l_rows - 1.0 - row as f32)) / (l_cols + l_rows);
                let t = (0.15 + 0.85 * diag).clamp(0.0, 1.0);
                crate::tui::blend_color(base, hilite, t)
            };
            spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
        }
        body.push(Line::from(spans));
    }
    body.push(Line::from(""));

    let mut title = match intro {
        Some(elapsed) => draw::shimmer_spans("gray", elapsed),
        None => vec![Span::styled(
            "gray",
            Style::default().bold().fg(theme.text_bright),
        )],
    };
    title.push(Span::raw(" "));
    if safe {
        title.push(Span::styled(
            "\u{25c6}",
            Style::default().fg(theme.text_muted),
        ));
        title.push(Span::raw(" "));
    }
    let title_w =
        display_width(if safe { "gray \u{25c6} " } else { "gray " }) + display_width(&version);
    title.push(Span::styled(version, Style::default().fg(theme.text_muted)));
    body.push(centered(title, title_w));
    body.push(Line::from(""));
    body
}

pub(crate) fn welcome_lines_at(
    w: usize,
    progress: f32,
    intro: Option<Duration>,
) -> Vec<Line<'static>> {
    let body = welcome_body_lines(w, progress, intro);
    let rows = crossterm::terminal::size()
        .map(|(_, r)| r as usize)
        .unwrap_or(0);
    let top = rows.saturating_sub(body.len() + draw::home_band_h(w)) / 2;
    let mut welcome_lines = Vec::with_capacity(top + body.len());
    for _ in 0..top {
        welcome_lines.push(Line::from(""));
    }
    welcome_lines.extend(body);
    welcome_lines
}

pub fn build_welcome_lines(w: usize) -> Vec<Line<'static>> {
    welcome_lines_at(w, 1.0, None)
}

fn intro_stage(terminal: &mut Term, w: usize, h: usize, welcome_h: usize) -> Option<u16> {
    let y0 = terminal.viewport_area.y as usize;
    let overflow = (y0 + welcome_h + draw::home_band_h(w)).saturating_sub(h);
    if overflow > y0 {
        return None;
    }
    let start = y0 - overflow;
    if overflow > 0 {
        let mut out = std::io::stdout().lock();
        let _ = write!(out, "\x1b[{h};1H{}", "\n".repeat(overflow));
        let _ = out.flush();
    }

    terminal.set_viewport_area(ratatui::layout::Rect::new(
        0,
        start as u16,
        w as u16,
        (h - start) as u16,
    ));

    for _ in 0..64 {
        match crossterm::event::poll(Duration::ZERO) {
            Ok(true) => {
                let _ = crossterm::event::read();
            }
            _ => break,
        }
    }
    Some(start as u16)
}

pub(crate) struct ReplayQueue {
    pub history: Arc<Vec<gray_core::Message>>,
    pub next: usize,
    pub tail: Vec<String>,
    pub tool_calls: std::collections::HashMap<String, (String, serde_json::Value)>,
    pub labels: Option<std::collections::HashMap<String, String>>,
    pub previews: Option<std::collections::HashMap<String, String>>,
    pub cwd: std::path::PathBuf,
    pub estimate: usize,
}

pub(crate) fn palette_repaint_due(
    painted: crate::theme::UiTheme,
    current: crate::theme::UiTheme,
    modal_open: bool,
) -> bool {
    !modal_open && painted != current
}

impl Tui {
    pub fn new() -> anyhow::Result<Self> {
        Self::with_intro(true)
    }

    pub fn new_no_intro() -> anyhow::Result<Self> {
        Self::with_intro(false)
    }

    fn with_intro(play: bool) -> anyhow::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);

        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));

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

        let welcome_lines = build_welcome_lines(cols as usize);
        let welcome_h = welcome_lines.len() as u16;

        let intro = if play && crate::setup::intro_enabled() {
            intro_stage(
                &mut terminal,
                cols as usize,
                rows as usize,
                welcome_h as usize,
            )
            .map(|_| std::time::Instant::now())
        } else {
            None
        };
        let _ = terminal.insert_before(welcome_h, |buf| {
            Paragraph::new(welcome_lines.clone())
                .style(Style::default().bg(crate::theme::theme().window()))
                .render(buf.area, buf);
        });

        Ok(Self::base(
            terminal,
            cols,
            rows,
            welcome_lines,
            vec![TranscriptEntry::Welcome],
            intro,
        ))
    }

    fn base(
        terminal: Term,
        cols: u16,
        rows: u16,
        transcript: Vec<Line<'static>>,
        history_entries: Vec<TranscriptEntry>,
        intro: Option<std::time::Instant>,
    ) -> Self {
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        Self {
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
            modal_pending: Vec::new(),
            insert_batch: None,
            painted_palette: crate::theme::theme(),
            queued_inputs: std::collections::VecDeque::new(),
            queued_sends: Vec::new(),
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
            model_label_parts: None,
            cwd,
            thinking_effort: String::new(),
            turn_show_effort: None,
            background_work: None,
            history_entries,
            transcript,
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
            reveal_buf: String::new(),
            reveal_pacing: false,
            feeding_markdown: false,
            pending_resize: None,
            turn_billed_output: None,
            streamed_bytes: 0,
            turn_output_accum: 0,
            turn_stream_ms: 0,
            viewport_h: MIN_VIEWPORT_H,
            live_tools: Vec::new(),
            plugin_widget: plugin_widget::Widget::new(std::env::current_dir().unwrap_or_default()),
            status_line_seen: crate::statusline::version(),
            ask_modal: None,
            home_notice: None,
            plugin_stats: None,
            home_update: None,
            home_quality_of_life: None,
            home_session: None,
            left_home: false,
            intro_started: intro,
            replay_queue: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_tui(cols: u16, rows: u16) -> Self {
        Self::base(
            CustomTerminal::at_row(
                CrosstermBackend::new(std::io::stdout()),
                ratatui::layout::Size::new(cols, rows),
                0,
                terminal::Scrollback::Regions,
            ),
            cols,
            rows,
            Vec::new(),
            Vec::new(),
            None,
        )
    }

    pub(crate) fn reanchor_viewport(&mut self, cols: u16) {
        self.last_width = cols;
        if let Ok((_, rows)) = crossterm::terminal::size() {
            self.last_height = rows;
        }
        self.pending_resize = None;

        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);
        self.atomic(|t| {
            let _ = t.terminal.clear();
        });
    }

    fn commit_markdown_tail(&mut self) {
        self.drain_reveal();

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
        .finish_tail_into_output(Some(gray_markdown::get_syntect()));
        if output.lines.len() > self.committed_markdown_lines {
            if self.committed_markdown_lines == 0 {
                self.ensure_gap();
            }
            let remaining_lines: Vec<Line<'static>> =
                output.lines[self.committed_markdown_lines..].to_vec();
            let offset = self.committed_markdown_lines;

            self.push_styled_lines_with_hyperlinks(remaining_lines, &output.hyperlinks, offset);
        }
        self.committed_markdown_lines = 0;
    }

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

    pub(crate) fn insert_paragraph(
        &mut self,
        lines: &[Line<'static>],
        bg: Option<ratatui::style::Color>,
    ) {
        if self.modal_open {
            self.modal_pending
                .push(PendingInsert::Rows(lines.to_vec(), bg));
            return;
        }
        if let Some(batch) = &mut self.insert_batch {
            batch.extend(lines.iter().cloned().map(|line| (line, Vec::new(), bg)));
            return;
        }
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

    pub(crate) fn drain_inflight_for_reflow(&mut self) {
        if self.thinking && !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            self.append_thinking_text(&rest, false);
        }
        self.commit_markdown_tail();
    }

    pub(crate) fn repaint_for_palette(&mut self) {
        if palette_repaint_due(self.painted_palette, crate::theme::theme(), self.modal_open) {
            self.reflow_on_resize(self.last_width);
        }
    }

    pub(crate) fn reflow_on_resize(&mut self, new_cols: u16) {
        self.atomic(|t| t.reflow_on_resize_inner(new_cols));
    }

    fn reflow_on_resize_inner(&mut self, new_cols: u16) {
        self.painted_palette = crate::theme::theme();
        let _ = self.hide_background();
        self.last_width = new_cols;
        if let Ok((_, rows)) = crossterm::terminal::size() {
            self.last_height = rows;
        }

        self.drain_inflight_for_reflow();

        self.modal_pending.clear();

        let (cols, rows) = crossterm::terminal::size().unwrap_or((new_cols, self.last_height));
        let _ = self
            .terminal
            .clear_scrollback_and_screen(ratatui::layout::Size::new(cols, rows));

        let w = new_cols as usize;
        self.transcript.clear();
        self.gap_owed = false;
        self.begin_insert_batch();
        let entries = std::mem::take(&mut self.history_entries);
        for entry in &entries {
            match entry {
                TranscriptEntry::Welcome => {
                    let lines = build_welcome_lines(w);
                    self.record_rows(lines.clone());
                    self.insert_paragraph(&lines, Some(crate::theme::theme().window_bg));
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
                TranscriptEntry::ToolBoxLines { header, body } => {
                    let lines =
                        crate::composer::transcript::format_tool_box_lines_multi(header, body, w);
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
                    self.emit_rows(rows, Some(crate::theme::theme().window_bg));
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
        self.end_insert_batch();

        let _ = self.draw();
    }

    pub(crate) fn pop_mascot(&mut self) -> bool {
        crate::mascot::set_mascot_shown(false);
        if !crate::composer::transcript::drop_mascot_entry(&mut self.history_entries) {
            return false;
        }
        self.reflow_on_resize(self.last_width);
        true
    }

    pub(crate) fn retract_slash_echo(&mut self, cmd: &str) -> bool {
        let echoed = matches!(
            self.history_entries.last(),
            Some(TranscriptEntry::UserPrompt(text, _)) if text.trim() == cmd
        );
        if !echoed {
            return false;
        }
        self.history_entries.pop();
        self.reflow_on_resize(self.last_width);
        true
    }

    pub(crate) fn start_fresh_conversation(&mut self) {
        self.history_entries = vec![TranscriptEntry::Welcome];
        self.left_home = false;
        self.reflow_on_resize(self.last_width);
    }

    pub fn set_model(&mut self, model: String) {
        self.model_name = model;
    }

    pub fn set_home_notice(&mut self, notice: Option<String>) {
        self.home_notice = notice;
    }

    pub fn set_plugin_stats(&mut self, stats: Option<String>) {
        self.plugin_stats = stats;
    }

    pub fn set_home_quality_of_life(&mut self, line: Option<String>) {
        self.home_quality_of_life = line;
    }

    pub fn set_home_update(&mut self, notice: Option<String>) {
        self.home_update = notice;
    }

    pub fn set_home_session(&mut self, line: Option<String>) {
        self.home_session = line;
    }

    pub(crate) fn is_home(&self) -> bool {
        !self.left_home
            && self.history_entries.iter().all(|e| match e {
                TranscriptEntry::Welcome
                | TranscriptEntry::Gap(_)
                | TranscriptEntry::Mascot
                | TranscriptEntry::StyledLines { .. } => true,
                TranscriptEntry::UserPrompt(text, _) => text.starts_with('/'),
                TranscriptEntry::ToolBox { .. }
                | TranscriptEntry::ToolBoxLines { .. }
                | TranscriptEntry::ThinkingRun(_) => false,
            })
    }

    pub fn set_model_label(&mut self, label: Option<String>) {
        self.model_label = label;
    }

    pub fn set_model_label_parts(&mut self, parts: Option<Vec<(String, Option<usize>)>>) {
        self.model_label_parts = parts;
    }
    pub fn set_cwd(&mut self, cwd: String) {
        self.cwd = cwd;
    }
    pub fn set_thinking_effort(&mut self, effort: String) {
        self.thinking_effort = effort;
    }

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
        if !usage.any() {
            return;
        }
        self.latest_usage = Some(usage);
        self.cumulative_usage = Some(usage);

        self.turn_output_accum = self.turn_output_accum.saturating_add(usage.output_tokens);

        self.streamed_bytes = 0;
    }

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

        self.cache.reset();
    }

    pub fn note_cache_request(
        &mut self,
        usage: &gray_core::event::Usage,
        model: &str,
        wire_model: &str,
    ) -> Option<crate::cache::CacheMiss> {
        let rate = crate::setup::get_model_rate(model);
        self.cache.note(usage, wire_model, rate, Instant::now())
    }

    pub fn cache_remaining(&self) -> Option<Duration> {
        self.cache.remaining(Instant::now())
    }

    pub fn cache_is_cold(&self) -> bool {
        self.cache.is_cold(Instant::now())
    }

    pub fn reset_cache(&mut self) {
        self.cache.reset();
    }

    pub(crate) fn resume_cache(&mut self) {
        self.cache.resume(Instant::now());
    }

    pub(crate) fn rearm_cache(&mut self) {
        self.cache.rearm(Instant::now());
    }

    pub(crate) fn set_turn_stream_ms(&mut self, ms: u64) {
        self.turn_stream_ms = ms;
    }

    pub fn set_turn_billed(&mut self, output_tokens: usize) {
        self.turn_billed_output = Some(output_tokens);
    }

    pub(crate) fn upsert_live_tool(&mut self, id: &str, header: Vec<Line<'static>>, running: bool) {
        if let Some(slot) = self.live_tools.iter_mut().find(|t| t.id == id) {
            slot.header = header;
            slot.running = running;
        } else {
            self.live_tools.push(LiveTool {
                id: id.to_string(),
                header,
                running,
                tail: LiveToolTail::default(),
            });
        }
        let _ = self.draw();
    }

    pub(crate) fn push_live_tool_output(&mut self, id: &str, bytes: &[u8]) {
        if let Some(slot) = self.live_tools.iter_mut().find(|t| t.id == id) {
            slot.tail.feed(bytes);
        }
    }

    pub(crate) fn remove_live_tool(&mut self, id: &str) {
        if let Some(pos) = self.live_tools.iter().position(|t| t.id == id) {
            self.live_tools.remove(pos);
            let _ = self.draw();
        }
    }

    pub(crate) fn live_tool_rows(&self, w: usize) -> Vec<Line<'static>> {
        let elapsed = self
            .turn_started
            .or_else(|| self.status.as_ref().map(|(started, _)| *started))
            .map(|started| started.elapsed())
            .unwrap_or_default();
        live_tool_rows(&self.live_tools, elapsed, w)
    }

    pub(crate) fn stream_tail_rows(&self, w: usize) -> Vec<Line<'static>> {
        let view = self.markdown_renderer.view();
        let start = self.committed_markdown_lines.min(view.lines.len());
        let lines = &view.lines[start..];
        let max_w = transcript::prose_width(w);
        let mut out = Vec::new();
        for line in lines {
            for mut row in transcript::wrap_styled_line(line.clone(), max_w) {
                if !row.spans.is_empty() {
                    row.spans.insert(0, transcript::left_pad());
                }
                out.push(row);
            }
        }
        out
    }

    pub(crate) fn live_tool_overflow(&self) -> usize {
        live_tool_overflow(self.live_tools.len())
    }

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
        if !open {
            self.flush_modal_pending();
        }
    }

    pub(crate) fn flush_modal_pending(&mut self) {
        if self.modal_pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.modal_pending);
        for item in pending {
            match item {
                PendingInsert::Rows(lines, bg) => self.insert_paragraph(&lines, bg),
                PendingInsert::Styled(rows) => self.insert_styled_rows(rows),
            }
        }
        let _ = self.draw();
    }

    pub(crate) fn begin_insert_batch(&mut self) {
        self.insert_batch.get_or_insert_with(Vec::new);
    }

    pub(crate) fn end_insert_batch(&mut self) {
        let Some(rows) = self.insert_batch.take() else {
            return;
        };
        if rows.is_empty() {
            return;
        }
        self.atomic(|t| {
            let _ = draw::settle_band(t);
            let total_h = rows.len() as u16;
            let _ = t.terminal.insert_before(total_h, |buf| {
                transcript::paint_insert_rows(buf, &rows);
            });
        });
    }

    pub(crate) fn draw(&mut self) -> anyhow::Result<()> {
        if self.intro_started.is_some() {
            self.draw_intro();
            return Ok(());
        }
        draw::draw(self)
    }

    pub(crate) fn end_intro(&mut self) {
        if self.intro_started.take().is_some() {
            self.reflow_on_resize_inner(self.last_width);
        }
    }

    fn draw_intro(&mut self) {
        if self.modal_open {
            return;
        }
        let Some(started) = self.intro_started else {
            return;
        };
        let elapsed = started.elapsed();
        if elapsed >= Duration::from_millis(INTRO_TOTAL_MS) {
            self.end_intro();
            return;
        }
        let w = self.last_width as usize;
        let progress = (elapsed.as_secs_f64() * 1000.0 / INTRO_TOTAL_MS as f64) as f32;
        let dim = Style::default().fg(crate::theme::theme().text_dim);
        let _ = self.terminal.draw(|frame| {
            let area = frame.area();
            frame.render_widget(
                Paragraph::new(welcome_lines_at(w, progress, Some(elapsed))),
                area,
            );
            let row = ratatui::layout::Rect::new(
                area.x,
                area.y + area.height.saturating_sub(1),
                area.width,
                1,
            );
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "enter to skip · /intro disable to disable",
                    dim,
                ))
                .alignment(ratatui::layout::Alignment::Center),
                row,
            );
        });
    }

    pub fn handle_paste(&mut self, pasted: String) -> bool {
        input::handle_paste(self, pasted)
    }

    pub fn begin_turn(&mut self, label: &str) {
        self.left_home = true;
        let now = Instant::now();

        self.allow_empty_submit = false;

        self.turn_show_effort = Some(footer_badge_visible(&self.model_name, None));
        self.cache.pause(now);
        self.stream_round_boundary = false;
        self.stream_round_had_text = false;
        self.stream_round_target = None;

        self.discard_held_punctuation();

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

    pub(crate) fn take_status(&mut self) -> Option<(Instant, String)> {
        self.status.take()
    }

    pub(crate) fn restore_status(&mut self, status: Option<(Instant, String)>) {
        self.status = status;
    }

    pub(crate) fn set_background_work(&mut self, label: Option<String>) {
        if self.background_work != label {
            self.background_work = label;
            let _ = self.draw();
        }
    }

    pub fn set_status(&mut self, label: Option<&str>) {
        if let Some(active) = self.active_compaction.clone() {
            self.status = Some((active.started_at, COMPACTION_HEADER.to_string()));
            let _ = self.draw();
            return;
        }
        self.status = label.map(|l| (Instant::now(), l.to_string()));
        let _ = self.draw();
    }

    pub(crate) fn mark_stream_round_boundary(&mut self) {
        if self.is_task_running {
            if self.stream_round_had_text {
                let was_boundary = self.stream_round_boundary;

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

            self.stream_round_had_text = false;
        }
    }

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

        self.allow_empty_submit = false;
        let started_at = Instant::now();
        self.active_compaction = Some(ActiveCompaction { id, started_at });
        self.status = Some((started_at, COMPACTION_HEADER.to_string()));
        let _ = self.draw();
    }

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

        self.active_compaction = None;

        self.live_tools.clear();

        let elapsed = self.turn_started.take().map(|s| s.elapsed());
        let had_thinking = self.turn_had_thinking;
        self.last_retry_detail = None;
        self.turn_had_thinking = false;
        if self.thinking {
            self.end_thinking_run(true);
        }

        self.flush_markdown();
        self.stream_round_boundary = false;
        self.stream_round_had_text = false;
        self.stream_round_target = None;

        self.discard_held_punctuation();
        self.is_task_running = false;
        self.status = None;
        self.turn_show_effort = None;

        let _ = self.draw();

        let turn_toks = self.turn_billed_output;
        self.turn_billed_output = None;
        self.turn_output_accum = 0;
        self.turn_stream_ms = 0;

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
            home: self.is_home(),
            home_notice: self.home_notice.clone(),
            plugin_stats: self.plugin_stats.clone(),
            home_update: self.home_update.clone(),
            home_quality_of_life: self.home_quality_of_life.clone(),
            home_session: self.home_session.clone(),
            attachments: self.attachments.clone(),
        }
    }
    pub fn tick_status(&mut self) {
        if self.modal_open {
            return;
        }

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

        let cache_ticking = self.cache_remaining().is_some();
        let footer_changed = crate::statusline::version() != self.status_line_seen;
        if self.intro_started.is_none() {
            self.tick_replay();
        }
        if self.intro_started.is_none()
            && self.status.is_none()
            && self.live_tools.is_empty()
            && !self.plugin_widget.active()
            && !widget_changed
            && !cache_ticking
            && !footer_changed
        {
            return;
        }
        let _ = self.draw();
    }
    pub fn shutdown(&mut self) {
        self.background_closed = true;
        let _ = self.hide_background();
        self.background = None;
        crate::quality_of_life::kitty::flush_transmits();

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
        crate::quality_of_life::kitty::flush_transmits();
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

#[path = "mod_palette_tests.rs"]
#[cfg(test)]
mod palette_tests;

#[path = "mod_fresh_conversation_tests.rs"]
#[cfg(test)]
mod fresh_conversation_tests;

#[path = "mod_intro_tests.rs"]
#[cfg(test)]
mod intro_tests;

mod background;
