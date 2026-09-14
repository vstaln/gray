//! Render output types for markdown.

use std::ops::Range;

use ratatui::text::Line;

/// A hyperlink target extracted from rendered markdown.
///
/// Each instance maps a contiguous cell range on one rendered line to a URL.
/// When a link wraps across lines, multiple `HyperlinkTarget`s share the same
/// `id` and `url` -- the `id` enables OSC 8 hover-grouping across wrapped lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperlinkTarget {
    /// Index of the rendered line this target appears on.
    pub line_index: usize,
    /// Column range (in display cells) of the link text on that line.
    pub column_range: Range<usize>,
    /// The destination URL.
    pub url: String,
    /// Stable identifier for grouping link fragments that belong to the
    /// same logical link (e.g., a link whose text wraps across lines).
    pub id: u32,
}

/// Output from rendering markdown to ratatui Lines.
///
/// Contains all the information needed to display rendered markdown and
/// support copy operations back to source text.
#[derive(Debug, Clone, Default)]
pub struct MarkdownRenderOutput {
    /// Rendered lines ready for display.
    pub lines: Vec<Line<'static>>,

    /// Maps each rendered line index to its source line number.
    /// `line_source_map[rendered_line_idx]` = source line number (0-indexed).
    pub line_source_map: Vec<usize>,

    /// Maps a cell range on a rendered line to a URL. Links that
    /// wrap across lines produce multiple entries with the same `id` and `url`.
    pub hyperlinks: Vec<HyperlinkTarget>,
}

impl MarkdownRenderOutput {
    /// Create a new empty output.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear all content, keeping allocated capacity.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.line_source_map.clear();
        self.hyperlinks.clear();
    }
}
