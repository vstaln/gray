use std::ops::Range;

use anstyle::Style as AnsiStyle;
use ratatui::text::{Line, Span};
use syntect::highlighting::Style as SyntectStyle;

#[derive(Debug, Clone)]
pub struct Highlight {
    pub style: Option<AnsiStyle>,
    pub range: Range<usize>,
}

#[derive(Debug, Clone)]
pub struct Replace {
    pub highlighted: Vec<Vec<(SyntectStyle, String)>>,

    pub range: Range<usize>,
}

#[derive(Debug, Clone)]
pub struct LinkTarget {
    pub source_range: Range<usize>,

    pub url: String,

    pub id: u32,
}

#[derive(Debug, Clone)]
pub struct Transform {
    pub(crate) range: Range<usize>,

    pub(crate) to: String,

    pub(crate) force: bool,
}

#[derive(Debug, Clone)]
pub struct CellSpan {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,

    pub link: Option<(String, u32)>,
}

impl CellSpan {
    pub fn new(
        text: String,
        bold: bool,
        italic: bool,
        code: bool,
        link: Option<(String, u32)>,
    ) -> Self {
        Self {
            text,
            bold,
            italic,
            code,
            link,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct StyledCell {
    pub spans: Vec<CellSpan>,
}

impl StyledCell {
    pub fn new() -> Self {
        Self { spans: Vec::new() }
    }

    pub fn plain_text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    pub fn clear(&mut self) {
        self.spans.clear();
    }
}

#[derive(Debug, Clone, Default)]
pub struct CellStyleState {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: Option<(String, u32)>,
}

impl CellStyleState {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Clone)]
pub struct TableState {
    pub alignments: Vec<pulldown_cmark::Alignment>,

    pub header: Vec<StyledCell>,

    pub rows: Vec<Vec<StyledCell>>,

    pub current_row: Vec<StyledCell>,

    pub current_cell: StyledCell,

    pub cell_style: CellStyleState,

    pub in_header: bool,

    pub range: Range<usize>,
}

impl TableState {
    pub fn new(alignments: Vec<pulldown_cmark::Alignment>, start: usize) -> Self {
        Self {
            alignments,
            header: Vec::new(),
            rows: Vec::new(),
            current_row: Vec::new(),
            current_cell: StyledCell::new(),
            cell_style: CellStyleState::default(),
            in_header: false,
            range: start..start,
        }
    }

    pub fn push_text(&mut self, text: &str) {
        self.current_cell.spans.push(CellSpan::new(
            text.to_string(),
            self.cell_style.bold,
            self.cell_style.italic,
            self.cell_style.code,
            self.cell_style.link.clone(),
        ));
    }
}

#[derive(Debug, Clone)]
pub struct TableHyperlink {
    pub line_offset: usize,

    pub column_range: Range<usize>,

    pub url: String,

    pub id: u32,
}

#[derive(Debug, Clone)]
pub struct TableReplace {
    pub lines: Vec<String>,

    pub styled_lines: Vec<Line<'static>>,

    pub range: Range<usize>,

    pub line_source_offsets: Vec<usize>,

    pub hyperlinks: Vec<TableHyperlink>,
}

pub fn unicode_display_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr;

    s.width()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum RenderEventKind {
    Highlight = 0,
    Replace = 1,
    Table = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RenderEvent {
    pub pos: usize,
    pub kind: RenderEventKind,
    pub index: usize,
    pub is_end: bool,
}

pub struct MarkdownBuffers {
    pub highlights: Vec<Highlight>,
    pub replaces: Vec<Replace>,
    pub transforms: Vec<Transform>,
    pub untagged_code_ranges: Vec<Range<usize>>,
    pub table_replaces: Vec<TableReplace>,
    pub link_targets: Vec<LinkTarget>,

    pub current_spans: Vec<Span<'static>>,
    pub active_highlights: Vec<usize>,
}

impl MarkdownBuffers {
    pub fn new() -> Self {
        Self {
            highlights: Vec::new(),
            replaces: Vec::new(),
            transforms: Vec::new(),
            untagged_code_ranges: Vec::new(),
            table_replaces: Vec::new(),
            link_targets: Vec::new(),
            current_spans: Vec::new(),
            active_highlights: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.highlights.clear();
        self.replaces.clear();
        self.transforms.clear();
        self.untagged_code_ranges.clear();
        self.table_replaces.clear();
        self.link_targets.clear();
        self.current_spans.clear();
        self.active_highlights.clear();
    }
}

impl Default for MarkdownBuffers {
    fn default() -> Self {
        Self::new()
    }
}
