use crate::{
    HyperlinkTarget, MarkdownBuffers, MarkdownRenderOutput, MarkdownStyle, Syntect,
    render_markdown_ratatui_with_link_id,
};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FrozenState {
    pub(crate) lines_len: usize,

    pub(crate) source_bytes: usize,

    pub(crate) next_link_id: u32,
}

pub struct StreamingMarkdownRenderer {
    source: String,

    output: MarkdownRenderOutput,

    pub(crate) frozen: FrozenState,

    buffers: MarkdownBuffers,

    style: MarkdownStyle,

    pretty: bool,

    max_table_width: Option<usize>,

    highlighted: bool,
}

impl std::fmt::Debug for StreamingMarkdownRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamingMarkdownRenderer")
            .field("source_len", &self.source.len())
            .field("frozen_lines", &self.frozen.lines_len)
            .field("frozen_bytes", &self.frozen.source_bytes)
            .field("output_lines", &self.output.lines.len())
            .field("pretty", &self.pretty)
            .finish()
    }
}

impl Clone for StreamingMarkdownRenderer {
    fn clone(&self) -> Self {
        let mut new = Self::new(self.style, self.pretty);
        new.set_max_table_width(self.max_table_width);
        new.source.push_str(&self.source);

        new.render(if self.highlighted {
            Some(crate::get_syntect())
        } else {
            None
        });
        new
    }
}

impl StreamingMarkdownRenderer {
    pub fn new(style: MarkdownStyle, pretty: bool) -> Self {
        Self {
            source: String::new(),
            output: MarkdownRenderOutput::new(),
            frozen: FrozenState::default(),
            buffers: MarkdownBuffers::new(),
            style,
            pretty,
            max_table_width: None,
            highlighted: false,
        }
    }

    pub fn frozen_lines_len(&self) -> usize {
        self.frozen.lines_len
    }

    pub fn set_max_table_width(&mut self, width: Option<usize>) {
        // Frozen lines were already emitted to the terminal and can never be
        // re-rendered; only the tail below them may pick up a new width.
        self.max_table_width = width;
    }

    pub fn render(&mut self, syntect: Option<&Syntect>) {
        self.rerender_tail(syntect);
    }

    pub fn push_and_render(&mut self, chunk: &str, syntect: Option<&Syntect>) {
        self.source.push_str(chunk);
        self.rerender_tail(syntect);
    }

    fn rerender_tail(&mut self, syntect: Option<&Syntect>) {
        self.highlighted = syntect.is_some();

        self.output.lines.truncate(self.frozen.lines_len);
        self.output.line_source_map.truncate(self.frozen.lines_len);

        self.output
            .hyperlinks
            .retain(|h| h.line_index < self.frozen.lines_len);

        let frozen_hyperlinks = self.output.hyperlinks.len();

        let mut tail_start = self.frozen.source_bytes;
        if tail_start > 0
            && self.source.as_bytes().get(tail_start - 1) != Some(&b'\n')
            && self.source.as_bytes().get(tail_start) == Some(&b'\n')
        {
            tail_start += 1;
        }
        let tail = &self.source[tail_start..];

        let (tail_output, checkpoint, tail_next_link_id) = render_markdown_ratatui_with_link_id(
            tail,
            self.style,
            self.pretty,
            &mut self.buffers,
            syntect,
            self.max_table_width,
            self.frozen.next_link_id,
        );

        self.output.lines.extend(tail_output.lines);
        self.output
            .line_source_map
            .extend(tail_output.line_source_map);

        let frozen_lines = self.frozen.lines_len;
        self.output
            .hyperlinks
            .extend(tail_output.hyperlinks.into_iter().map(|mut h| {
                h.line_index += frozen_lines;
                h
            }));

        let tail_lines = &self.output.lines[frozen_lines..];
        let (extra_links, post_scan_next_id) = crate::url_scan::detect_plain_urls_with_offset(
            tail_lines,
            frozen_lines,
            &self.output.hyperlinks,
            tail_next_link_id,
        );
        self.output.hyperlinks.extend(extra_links);
        let (file_links, post_scan_next_id) = crate::url_scan::detect_file_paths_with_offset(
            &self.output.lines[frozen_lines..],
            frozen_lines,
            &self.output.hyperlinks,
            post_scan_next_id,
        );
        self.output.hyperlinks.extend(file_links);

        self.output.hyperlinks[frozen_hyperlinks..]
            .sort_by_key(|h| (h.line_index, h.column_range.start));

        let tail_links: Vec<HyperlinkTarget> = self.output.hyperlinks[frozen_hyperlinks..]
            .iter()
            .map(|h| HyperlinkTarget {
                line_index: h.line_index - frozen_lines,
                column_range: h.column_range.clone(),
                url: String::new(),
                id: h.id,
            })
            .collect();
        crate::url_scan::apply_link_styling(&mut self.output.lines[frozen_lines..], &tail_links);

        if let Some(cp) = checkpoint {
            self.frozen = FrozenState {
                lines_len: self.frozen.lines_len + cp.output_lines,
                source_bytes: tail_start + cp.source_bytes,
                next_link_id: post_scan_next_id,
            };
        }
    }

    pub fn view(&self) -> &MarkdownRenderOutput {
        &self.output
    }

    pub fn finish(&mut self, syntect: Option<&Syntect>) -> &MarkdownRenderOutput {
        self.highlighted = syntect.is_some();

        let mut buffers = MarkdownBuffers::new();
        let (full_output, _, full_next_link_id) = render_markdown_ratatui_with_link_id(
            &self.source,
            self.style,
            self.pretty,
            &mut buffers,
            syntect,
            self.max_table_width,
            0,
        );

        self.output = full_output;

        let (extra_links, post_scan_next_id) = crate::url_scan::detect_plain_urls(
            &self.output.lines,
            &self.output.hyperlinks,
            full_next_link_id,
        );
        self.output.hyperlinks.extend(extra_links);
        let (file_links, post_scan_next_id) = crate::url_scan::detect_file_paths(
            &self.output.lines,
            &self.output.hyperlinks,
            post_scan_next_id,
        );
        self.output.hyperlinks.extend(file_links);

        self.output
            .hyperlinks
            .sort_by_key(|h| (h.line_index, h.column_range.start));
        crate::url_scan::apply_link_styling(&mut self.output.lines, &self.output.hyperlinks);

        self.frozen = FrozenState {
            lines_len: self.output.lines.len(),
            source_bytes: self.source.len(),
            next_link_id: post_scan_next_id,
        };

        &self.output
    }

    pub fn finish_into_output(mut self, syntect: Option<&Syntect>) -> MarkdownRenderOutput {
        self.finish(syntect);
        self.output
    }

    /// Finalize like `finish` but keep the frozen prefix verbatim instead of
    /// re-parsing the whole source. Callers that already emitted the frozen
    /// lines to a terminal can slice `output.lines` by the emitted count —
    /// the prefix is guaranteed identical to what `view()` returned.
    pub fn finish_tail_into_output(mut self, syntect: Option<&Syntect>) -> MarkdownRenderOutput {
        self.rerender_tail(syntect);
        self.frozen = FrozenState {
            lines_len: self.output.lines.len(),
            source_bytes: self.source.len(),
            next_link_id: self.frozen.next_link_id,
        };
        self.output
    }
}

#[path = "streaming_torn_tests.rs"]
#[cfg(test)]
mod streaming_torn_tests;
