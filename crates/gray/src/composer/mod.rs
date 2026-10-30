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

pub(crate) mod cache_stats;
mod glow;
pub(crate) mod kitty;
mod plugin_widget;
mod terminal;
pub(crate) use terminal::{CustomTerminal, LeveledBackend};

type Term = CustomTerminal<LeveledBackend<CrosstermBackend<Stdout>>>;

pub(crate) mod draw;
pub(crate) mod input;
pub(crate) mod transcript;

pub type SharedTui = Arc<std::sync::Mutex<Tui>>;

pub(crate) const COMPACTION_HEADER: &str = "Compacting context";

#[derive(Debug, Clone)]
pub(crate) struct ActiveCompaction {
    pub(crate) id: String,
    pub(crate) started_at: Instant,
}

pub(crate) const MAX_LIVE_TOOLS: usize = 3;

mod text_area;
pub(crate) use text_area::TextArea;

/// Reverse-history-search state: while set, the composer textarea previews
/// the currently matched history entry and the query is drawn on a
/// dedicated `search:` line above the box.
pub(crate) struct HistSearch {
    pub query: String,
    pub saved: String,
    pub saved_cursor: usize,
    pub idx: Option<usize>,
}

/// How long a transient composer hint stays on screen.
pub(crate) const HINT_TTL: Duration = Duration::from_secs(4);

/// One-slot kill ring: the draft (with its paste payloads and attachments)
/// last wiped by esc/ctrl-c, restored into the composer by ctrl-y.
#[derive(Clone, Debug, Default)]
pub(crate) struct KilledDraft {
    pub(crate) text: String,
    pub(crate) pending_pastes: Vec<(String, String)>,
    pub(crate) attachments: Vec<PathBuf>,
}

pub struct Tui {
    pub(crate) terminal: Term,
    background: Option<background::Background>,
    background_closed: bool,
    pub(crate) textarea: TextArea,
    pub(crate) matches: Vec<(String, String)>,
    /// Text the completion popup was dismissed for: sync_completion keeps
    /// the popup closed while the draft still equals it, and re-arms as
    /// soon as the text changes.
    pub(crate) muted_text: Option<String>,
    /// Active ctrl-r reverse-history-search state (None when not searching).
    pub(crate) search: Option<HistSearch>,
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

    /// One-slot kill ring for ctrl-y (see KilledDraft).
    pub(crate) killed_draft: Option<KilledDraft>,

    /// Short-lived composer hint drawn in the status dock slot while fresh.
    pub(crate) hint: Option<(Instant, String)>,
    /// Styled body for `hint` (a home-screen command receipt); plain dim
    /// `hint` text when None.
    pub(crate) hint_line: Option<Line<'static>>,

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
    pub(crate) cache_stats: crate::composer::cache_stats::CacheStats,
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
    /// Set when the intro hands off: the steady home band fades in from
    /// the background over `HOME_REVEAL_MS` (`draw::frame` applies it).
    pub(crate) home_reveal: Option<std::time::Instant>,
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

    ThinkingRun(String),
    Gap(usize),

    Mascot,
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
        crate::composer::kitty::warm_cell_pixels();
        Self::with_intro(true)
    }

    pub fn new_no_intro() -> anyhow::Result<Self> {
        crate::composer::kitty::warm_cell_pixels();
        Self::with_intro(false)
    }

    fn with_intro(play: bool) -> anyhow::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);

        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));

        let mut terminal = match CustomTerminal::with_options(LeveledBackend::new(
            CrosstermBackend::new(std::io::stdout()),
        )) {
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

        let mut tui = Self::base(
            terminal,
            cols,
            rows,
            welcome_lines,
            vec![TranscriptEntry::Welcome],
            intro,
        );
        tui.history = crate::prompt_history::load();
        Ok(tui)
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
            muted_text: None,
            search: None,
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
            killed_draft: None,
            hint: None,
            hint_line: None,
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
            cache_stats: crate::composer::cache_stats::CacheStats::default(),
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
            home_reveal: None,
            replay_queue: None,
        }
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        crate::composer::kitty::flush_transmits();
        let _ = self.hide_background();
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
        let _ = crossterm::execute!(std::io::stdout(), crossterm::cursor::Show);
    }
}

mod background;

mod live;
pub(crate) use live::*;
mod intro;
pub use intro::*;

mod frame;
mod home;
mod intro_anim;
mod live_tools;
mod render;
mod turn;
mod usage;
