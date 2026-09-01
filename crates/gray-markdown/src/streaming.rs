//! Streaming/incremental markdown renderer.
//!
//! This module provides `StreamingMarkdownRenderer` which efficiently renders
//! markdown that arrives in chunks (e.g., from an LLM streaming response).
//!
//! # How It Works
//!
//! Instead of re-rendering the entire document on each chunk, it:
//! 1. Accumulates incoming chunks into an internal buffer
//! 2. Detects "checkpoints" - stable block boundaries where output won't change
//! 3. Freezes rendered output up to the last checkpoint
//! 4. Only re-renders the "tail" after the checkpoint
//!
//! This reduces complexity from O(N²) to approximately O(N) for streaming.
//!
//! # Example
//!
//! ```ignore
//! let mut renderer = StreamingMarkdownRenderer::new(style, true);
//!
//! // As tokens arrive from LLM:
//! for token in stream {
//!     renderer.push_and_render(&token, Some(&syntect));
//!     let view = renderer.view();
//!     display(view.lines);
//! }
//! ```

use crate::open_code_highlighter::OpenCodeHighlighter;
use crate::{
    LatexDelimiterNormalizer, MarkdownBuffers, MarkdownRenderOutput, MarkdownRenderView,
    MarkdownStyle, Syntect, render_markdown_ratatui_with_link_id,
};

/// Tracks the frozen state for truncation.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FrozenState {
    /// Number of frozen lines (= number of frozen line_source_map entries).
    pub(crate) lines_len: usize,
    /// Number of frozen source bytes.
    pub(crate) source_bytes: usize,
    /// Next link ID, advanced ONLY when a checkpoint advances the frozen
    /// boundary (i.e. when frozen lines and their hyperlinks become
    /// permanent).  IDs assigned to url_scan hits inside a still-tail
    /// region are regenerated on every `rerender_tail` call — they only
    /// become stable once the line they live on becomes frozen.
    pub(crate) next_link_id: u32,
}

/// Count trailing blank lines in text.
///
/// This counts how many blank lines appear at the END of the text.
/// A trailing blank line is a line containing only whitespace followed by end-of-text,
/// or consecutive newlines at the end.
///
/// For markdown block separators:
/// - "\n\n" at the end = 1 blank line (one full blank line between blocks)
/// - "\n\n\n" at the end = 2 blank lines
/// - "text\n" at the end = 0 (just a line ending, no blank line)
///
/// Examples:
/// - "" → 0
/// - "hello" → 0
/// - "hello\n" → 0 (just a line ending)
/// - "hello\n\n" → 1
/// - "hello\n\n\n" → 2
/// - "hello\n  \n" → 1 (whitespace-only line counts as blank)
#[cfg(test)]
fn count_trailing_blank_lines(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }

    let bytes = text.as_bytes();
    let mut count = 0;
    let mut pos = bytes.len();

    // Work backwards through the text
    while pos > 0 {
        pos -= 1;

        match bytes[pos] {
            b'\n' => {
                // Found a newline - check if the line before it is blank
                // Scan backwards to find start of this line
                let line_end = pos;
                let mut line_start = pos;
                while line_start > 0 && bytes[line_start - 1] != b'\n' {
                    line_start -= 1;
                }

                // Check if the line is blank (only whitespace)
                let line_content = &bytes[line_start..line_end];
                let is_blank = line_content.iter().all(|&b| b == b' ' || b == b'\t');

                if is_blank {
                    count += 1;
                    pos = line_start;
                } else {
                    // Found a non-blank line, stop counting
                    break;
                }
            }
            b' ' | b'\t' => {
                // Trailing whitespace, continue scanning
            }
            _ => {
                // Non-whitespace character, stop
                break;
            }
        }
    }

    count
}

/// Incremental markdown renderer that efficiently handles streaming input.
///
/// Maintains frozen (stable) content and only re-renders the unfrozen tail,
/// dramatically reducing render time for long streaming content.
pub struct StreamingMarkdownRenderer {
    /// Accumulated source text (all chunks concatenated).
    source: String,

    /// Single output buffer - frozen content at start, tail appended after.
    output: MarkdownRenderOutput,

    /// Frozen state - where to truncate before re-rendering tail.
    pub(crate) frozen: FrozenState,

    /// Reusable buffers for highlighting and rendering (avoids allocation per render).
    buffers: MarkdownBuffers,

    /// Rendering style.
    style: MarkdownStyle,

    /// Whether to use pretty mode (hide markdown syntax).
    pretty: bool,

    /// Maximum width for rendered tables (in display columns).
    max_table_width: Option<usize>,

    /// Whether CommonMark soft breaks collapse to a space (default `true`).
    /// Set `false` for source-faithful rendering (plan preview).
    collapse_soft_breaks: bool,

    /// Incremental highlighter for the trailing still-open fenced code block.
    ///
    /// Persists syntect's resumable per-line state across `rerender_tail` calls
    /// so a large open code block is highlighted in O(N) total instead of O(N²).
    /// Created lazily on the first render with syntect, and cleared (so it
    /// rebuilds) on any state reset that would change output — theme/style,
    /// pretty mode, table width, soft-break mode, or `clear()`.
    open_code: Option<OpenCodeHighlighter>,

    /// Streaming LaTeX delimiter normalizer. Rewrites `\(…\)` / `\[…\]` /
    /// `\begin{equation}` into the canonical `$` / `$$` forms before text is
    /// appended to `source`, so the math handlers convert them uniformly —
    /// including inside table cells. Held-back ambiguous bytes (a partial
    /// delimiter at a chunk boundary) are flushed by `finish()`.
    normalizer: LatexDelimiterNormalizer,
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
        // Create a fresh renderer and push all source text
        // This recreates the frozen state correctly.
        //
        // We must propagate `max_table_width` BEFORE pushing/rendering so
        // the clone produces identical output to the original.  Forgetting
        // this caused tables (and, after the url_scan-on-render fix, URLs
        // in width-constrained renders) to differ between original and
        // clone.
        let mut new = Self::new(self.style, self.pretty);
        new.set_max_table_width(self.max_table_width);
        new.set_collapse_soft_breaks(self.collapse_soft_breaks);
        // `self.source` is already normalized, so append it verbatim (do NOT
        // re-run the normalizer, which could hold back a trailing ambiguous
        // suffix and make the clone's source diverge). Copy the normalizer
        // state separately so any held-back bytes survive the clone.
        new.push_normalized(&self.source);
        new.render(None);
        new.normalizer = self.normalizer.clone();
        new
    }
}

impl StreamingMarkdownRenderer {
    /// Create a new streaming renderer.
    pub fn new(style: MarkdownStyle, pretty: bool) -> Self {
        Self {
            source: String::new(),
            output: MarkdownRenderOutput::new(),
            frozen: FrozenState::default(),
            buffers: MarkdownBuffers::new(),
            style,
            pretty,
            max_table_width: None,
            collapse_soft_breaks: true,
            open_code: None,
            normalizer: LatexDelimiterNormalizer::new(),
        }
    }

    /// Number of frozen lines rendered so far.
    pub fn frozen_lines_len(&self) -> usize {
        self.frozen.lines_len
    }

    /// Replace the markdown style and trigger a full re-render.
    ///
    /// Used when the theme changes at runtime so existing blocks pick up
    /// the new colors on the next render pass.
    pub fn set_style(&mut self, style: MarkdownStyle) {
        self.style = style;
        self.frozen = FrozenState::default();
        self.output.clear();
        // Theme/style change alters colors, so any cached highlight is stale.
        self.open_code = None;
    }

    /// Set the maximum width for rendered tables.
    ///
    /// When set, column widths are shrunk proportionally so the table
    /// fits within the given number of display columns.  If the width
    /// changes, frozen state is reset to ensure consistent rendering.
    pub fn set_max_table_width(&mut self, width: Option<usize>) {
        if self.max_table_width != width {
            self.max_table_width = width;
            // Reset frozen state since table formatting may change
            self.frozen = FrozenState::default();
            self.output.clear();
            self.open_code = None;
        }
    }

    /// Set whether CommonMark soft breaks collapse to a space.
    ///
    /// Defaults to `true`. Set `false` for source-faithful rendering (plan
    /// preview) where each source line keeps its own visual line and
    /// `line_source_map` entry. Resets frozen state when the mode changes.
    pub fn set_collapse_soft_breaks(&mut self, collapse: bool) {
        if self.collapse_soft_breaks != collapse {
            self.collapse_soft_breaks = collapse;
            self.frozen = FrozenState::default();
            self.output.clear();
            self.open_code = None;
        }
    }

    /// Push a new chunk of markdown text (no rendering).
    ///
    /// The chunk is run through the streaming LaTeX delimiter normalizer and the
    /// normalized result is appended to the internal buffer. A bounded ambiguous
    /// suffix (a partial delimiter at the chunk boundary) may be held back until
    /// the next `push`; `finish()` flushes it. Call `render()` to process
    /// accumulated content, or use `push_and_render()` for convenience.
    pub fn push(&mut self, chunk: &str) {
        let normalized = self.normalizer.push(chunk);
        self.source.push_str(&normalized);
    }

    /// Append already-normalized source text, bypassing the delimiter
    /// normalizer. Used by `clone()` to reproduce an existing (already
    /// normalized) `source` exactly; the cloned normalizer state is copied
    /// separately so any held-back bytes are preserved.
    fn push_normalized(&mut self, text: &str) {
        self.source.push_str(text);
    }

    /// Render accumulated content.
    ///
    /// Processes the unfrozen tail and updates the output. Call `view()` to
    /// get the rendered lines.
    ///
    /// Pass `None` for syntect to disable syntax highlighting for code blocks.
    ///
    /// Theme stability: a still-open fenced code block is highlighted
    /// incrementally, caching the colors of the `syntect` theme seen so far.
    /// The `syntect` theme must stay stable between renders; switch themes via
    /// [`set_style`](Self::set_style), which clears that cache. (Passing a
    /// different theme without a reset would leave already-committed lines in
    /// the old colors.)
    pub fn render(&mut self, syntect: Option<&Syntect>) {
        self.rerender_tail(syntect);
    }

    /// Push a chunk and render immediately (convenience method).
    ///
    /// Equivalent to `push(chunk)` followed by `render(syntect)`.
    /// Use this for real-time streaming where you want to display after each chunk.
    pub fn push_and_render(&mut self, chunk: &str, syntect: Option<&Syntect>) {
        let normalized = self.normalizer.push(chunk);
        self.source.push_str(&normalized);
        self.rerender_tail(syntect);
    }

    /// Internal: render the unfrozen tail and update frozen state.
    fn rerender_tail(&mut self, syntect: Option<&Syntect>) {
        // Truncate output to frozen state (discard stale tail)
        self.output.lines.truncate(self.frozen.lines_len);
        self.output.line_source_map.truncate(self.frozen.lines_len);
        // Discard stale tail hyperlinks (keep frozen ones)
        self.output
            .hyperlinks
            .retain(|h| h.line_index < self.frozen.lines_len);
        // Discard stale tail code-block spans (keep frozen ones — those whose
        // body lies entirely within the frozen prefix). A still-open fence in
        // the tail has no span at all, so spans become stable only once frozen.
        self.output
            .code_blocks
            .retain(|cb| cb.output_line_range.end <= self.frozen.lines_len);

        // Render the tail (unfrozen portion) using reusable buffers.
        // When the frozen source ends without a trailing newline (e.g., a
        // thematic break `---` at the end of a chunk) but the tail starts
        // with `\n`, that newline is the block-terminating newline consumed
        // by the frozen block.  Skip it to avoid a spurious blank line.
        let mut tail_start = self.frozen.source_bytes;
        if tail_start > 0
            && self.source.as_bytes().get(tail_start - 1) != Some(&b'\n')
            && self.source.as_bytes().get(tail_start) == Some(&b'\n')
        {
            tail_start += 1;
        }
        let tail = &self.source[tail_start..];
        // Lazily create the incremental open-code cache once syntect is present.
        // It rebuilds itself on fence/offset change, so a stale cache from a
        // previous tail (e.g. after a checkpoint advanced) is self-correcting.
        let open_code = match syntect {
            Some(syn) => Some(
                self.open_code
                    .get_or_insert_with(|| OpenCodeHighlighter::new(syn)),
            ),
            None => None,
        };
        let (tail_output, checkpoint, tail_next_link_id) = render_markdown_ratatui_with_link_id(
            tail,
            self.style,
            self.pretty,
            &mut self.buffers,
            syntect,
            self.max_table_width,
            self.frozen.next_link_id,
            self.collapse_soft_breaks,
            open_code,
        );

        // Append tail to output
        self.output.lines.extend(tail_output.lines);
        self.output
            .line_source_map
            .extend(tail_output.line_source_map);

        // Offset tail hyperlink line indices by frozen line count and append
        let frozen_lines = self.frozen.lines_len;
        self.output
            .hyperlinks
            .extend(tail_output.hyperlinks.into_iter().map(|mut h| {
                h.line_index += frozen_lines;
                h
            }));

        // Append tail code-block spans, rebasing their tail-relative ranges to
        // document coordinates (output lines by frozen line count, source bytes
        // by the tail's start offset) — mirroring the hyperlink offsetting.
        self.output
            .code_blocks
            .extend(tail_output.code_blocks.into_iter().map(|mut cb| {
                cb.output_line_range.start += frozen_lines;
                cb.output_line_range.end += frozen_lines;
                cb.source_byte_range.start += tail_start;
                cb.source_byte_range.end += tail_start;
                cb
            }));

        // Detect plain URLs (e.g. the `(url)` suffix in pretty-mode
        // markdown links, bare URLs in prose).  We run this here — not
        // only in `finish()` — for two reasons:
        //   (a) Non-streaming callers (e.g. `AgentMessageBlock::new(text)`
        //       during session replay) never call `finish()`, so without
        //       running url_scan here their URLs would never become
        //       HyperlinkTargets at all.
        //   (b) State resets (`set_max_table_width`, `set_pretty`,
        //       `set_style`) rebuild the output from scratch via a
        //       subsequent `render()`; URL hyperlinks added by an earlier
        //       `finish()` would otherwise be silently dropped here.
        //
        // We scan only the newly-rendered tail (`frozen_lines..end`); URLs
        // already on frozen lines were kept by the `retain` filter above
        // and the offset-aware scan emits document-absolute line indices.
        // `detect_plain_urls_with_offset` dedups against existing
        // hyperlinks per line, so it is idempotent.
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

        // Sort hyperlinks by (line_index, column_range.start) so downstream
        // consumers (`map_hyperlinks_to_overlay`, link map builders) see a
        // well-ordered list — matching the invariant `finish()` enforces.
        self.output
            .hyperlinks
            .sort_by_key(|h| (h.line_index, h.column_range.start));
        // Make file/web links always underlined (cyan) no matter what
        crate::url_scan::apply_link_styling(&mut self.output.lines, &self.output.hyperlinks);

        // If checkpoint found, update frozen state.
        // The checkpoint's source_bytes is relative to the tail we rendered,
        // so add tail_start (which may be > frozen.source_bytes if we skipped
        // a leading newline).
        if let Some(cp) = checkpoint {
            self.frozen = FrozenState {
                lines_len: self.frozen.lines_len + cp.output_lines,
                source_bytes: tail_start + cp.source_bytes,
                next_link_id: post_scan_next_id,
            };
        }
    }

    /// Get a view of the current rendered output.
    ///
    /// This is cheap - just returns a reference to cached output.
    /// The output was computed during `render()` or `push_and_render()`.
    pub fn view(&self) -> MarkdownRenderView<'_> {
        self.output.as_view()
    }

    /// Get the accumulated source text.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Get the number of frozen source bytes.
    pub fn frozen_bytes(&self) -> usize {
        self.frozen.source_bytes
    }

    /// Get the number of frozen output lines.
    pub fn frozen_lines_count(&self) -> usize {
        self.frozen.lines_len
    }

    /// Reset the renderer, clearing all accumulated content.
    ///
    /// Also resets `max_table_width` to `None` for symmetry with the
    /// freshly-constructed state — otherwise a subsequent
    /// `set_max_table_width(Some(prev_width))` is silently a no-op
    /// (no state reset) because the inner equality check sees no change.
    pub fn clear(&mut self) {
        self.source.clear();
        self.output.clear();
        self.frozen = FrozenState::default();
        self.max_table_width = None;
        self.open_code = None;
        self.normalizer.reset();
    }

    /// Set pretty mode (true = hide syntax, false = show raw markdown).
    ///
    /// If the mode changes, frozen state is reset to ensure consistent rendering.
    pub fn set_pretty(&mut self, pretty: bool) {
        if self.pretty != pretty {
            self.pretty = pretty;
            // Reset frozen state - need to re-render everything with new mode
            self.frozen = FrozenState::default();
            self.output.clear();
            self.open_code = None;
        }
    }

    /// Get current pretty mode.
    pub fn pretty(&self) -> bool {
        self.pretty
    }

    /// Consume the renderer and return the owned output.
    ///
    /// Use this when streaming is complete and you want owned data.
    pub fn into_output(self) -> MarkdownRenderOutput {
        self.output
    }

    /// Finalize streaming with a full re-render.
    ///
    /// This does a complete non-streaming render of the accumulated source,
    /// replacing the incrementally-built output. Use this when streaming is
    /// complete to ensure correctness - it catches any edge cases where
    /// streaming might have produced slightly different output.
    ///
    /// After the parser pass, this runs `url_scan::detect_plain_urls`
    /// and sorts the hyperlink list by `(line_index, column_range.start)`
    /// so downstream consumers see a well-ordered list.  Note that
    /// `render()` ALSO runs the URL detector and sort — `finish()` no
    /// longer adds anything those calls didn't already produce; its
    /// distinguishing value is the unconditional full re-render
    /// (independent of frozen-state truncation).
    ///
    /// Returns a view of the finalized output.
    pub fn finish(&mut self, syntect: Option<&Syntect>) -> MarkdownRenderView<'_> {
        // Flush any bytes the normalizer held back at the last chunk boundary
        // (e.g. a trailing partial delimiter) so the full re-render sees the
        // complete, normalized source.
        let flushed = self.normalizer.finish();
        self.source.push_str(&flushed);

        // Do a full re-render of the entire source, preserving max_table_width.
        let mut buffers = MarkdownBuffers::new();
        let (full_output, _, full_next_link_id) = render_markdown_ratatui_with_link_id(
            &self.source,
            self.style,
            self.pretty,
            &mut buffers,
            syntect,
            self.max_table_width,
            // NOTE: Since full render restarts link IDs at 0, we MUST also reset our
            // counter to the post-render value :sadge:
            0,
            self.collapse_soft_breaks,
            // finish() is a full batch re-render: never use the incremental cache.
            None,
        );

        // Replace the output with the full render
        self.output = full_output;

        // Scan rendered lines for plain, non-md URLs that pulldown-cmark didn't
        // emit as Tag::Link. Dedup against existing hyperlinks by (line_index, column_range)
        // overlap to avoid double-linking.
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

        // Sort hyperlinks by (line_index, column_range.start) so downstream
        // consumers see a well-ordered list.
        self.output
            .hyperlinks
            .sort_by_key(|h| (h.line_index, h.column_range.start));
        crate::url_scan::apply_link_styling(&mut self.output.lines, &self.output.hyperlinks);

        // Mark everything as frozen (streaming is complete)
        self.frozen = FrozenState {
            lines_len: self.output.lines.len(),
            source_bytes: self.source.len(),
            next_link_id: post_scan_next_id,
        };

        // Streaming is over: release the highlighter caches (open-block state
        // + closed-fence memo) instead of retaining them for the lifetime of
        // the rendered block. Lazily rebuilt if rendering ever resumes.
        self.open_code = None;

        self.output.as_view()
    }

    /// Finalize streaming and return owned output.
    ///
    /// Combines `finish()` and `into_output()` - does a full re-render
    /// and returns the owned result.
    pub fn finish_into_output(mut self, syntect: Option<&Syntect>) -> MarkdownRenderOutput {
        self.finish(syntect);
        self.output
    }
}

#[cfg(test)]
mod streaming_torn_tests {
    use crate::style::test_style;
    use crate::{StreamingMarkdownRenderer, render_markdown_ratatui_full};

    fn lines_text(lines: &[ratatui::text::Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn torn_inline_latex_delimiter_across_chunks_matches_full_render() {
        // `\(...\)` split mid-delimiter (between `\` and `(`) — the
        // normalizer must hold back the trailing `\` until the next chunk.
        let full = "Intro \\(\\alpha + \\beta\\) end.\n\n";
        let split = full.find("\\(").unwrap() + 1; // after `\`, before `(`
        let (a, b) = full.split_at(split);
        assert!(a.ends_with('\\'), "a={a:?}");
        assert!(b.starts_with('('), "b={b:?}");

        let (expected, _) = render_markdown_ratatui_full(full, test_style::STYLE, true, None);
        let mut r = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        r.push_and_render(a, None);
        r.push_and_render(b, None);
        let view = r.finish(None);

        assert_eq!(lines_text(view.lines), lines_text(&expected.lines));
        // latex passthrough: `\alpha + \beta` -> `α + β`, delimiters hidden
        let joined = lines_text(view.lines).join("\n");
        assert!(joined.contains("α + β"), "got: {joined:?}");
        assert!(!joined.contains("\\("), "delimiters must be hidden: {joined:?}");
    }

    #[test]
    fn torn_hyperlink_brackets_across_chunks_preserve_hyperlink_offset() {
        // `[click](url)` split inside `](` — pretty mode rewrites `[`/`](` so
        // column ranges must still land on the visible "click" glyphs.
        let full = "See [click](https://example.com) here.\n\n";
        let split = full.find("](").unwrap() + 1; // after `]`, before `(`
        let (a, b) = full.split_at(split);
        assert!(a.ends_with(']'), "a={a:?}");
        assert!(b.starts_with('('), "b={b:?}");

        let (expected, _) = render_markdown_ratatui_full(full, test_style::STYLE, true, None);
        let mut r = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        r.push_and_render(a, None);
        r.push_and_render(b, None);
        let view = r.finish(None);

        assert_eq!(lines_text(view.lines), lines_text(&expected.lines));

        // Parser-produced link-text hyperlink must cover exactly "click" (4 cells)
        let line0: String = view.lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        let hit = view
            .hyperlinks
            .iter()
            .find(|h| h.url == "https://example.com" && {
                let slice: String = line0.chars().skip(h.column_range.start).take(h.column_range.len()).collect();
                slice == "click"
            })
            .expect("hyperlink over link text should survive torn chunk");
        assert_eq!(hit.column_range.len(), 5);
    }
}

