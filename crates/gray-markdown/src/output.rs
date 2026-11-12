use std::ops::Range;

use ratatui::text::Line;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperlinkTarget {
    pub line_index: usize,

    pub column_range: Range<usize>,

    pub url: String,

    pub id: u32,
}

#[derive(Debug, Clone, Default)]
pub struct MarkdownRenderOutput {
    pub lines: Vec<Line<'static>>,

    pub line_source_map: Vec<usize>,

    pub hyperlinks: Vec<HyperlinkTarget>,
}
