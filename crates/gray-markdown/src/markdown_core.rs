//! Headless markdown analysis sharing Grok Build's exact `pulldown-cmark` config.
//!
//! This crate is intentionally lean -- it depends only on `pulldown-cmark` -- so it
//! can be used without pulling in the terminal-rendering stack (syntect, ratatui,
//! two-face). [`parser_options`] is the single source of truth for the parser
//! feature set, shared with `xai-grok-markdown` so analysis matches what Grok
//! Build actually renders 1:1.
//!
//! After parsing, Grok applies [`offset_events`]: only `~~…~~` is strikethrough.
//! Single-tilde pairs (`~text~`) are demoted to literal `~` text so LLM output
//! like `~**10%**` is not struck (pulldown treats those pairs as strike; we do not).

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::ops::Range;

/// The exact `pulldown-cmark` option set Grok Build uses to render markdown.
///
/// With `ENABLE_STRIKETHROUGH`, pulldown treats both `~~…~~` and single-`~` pairs as
/// strike. Callers must consume events via [`offset_events`] so only double-tilde
/// strikethrough is retained (LLM-friendly post-policy).
pub fn parser_options() -> Options {
    Options::ENABLE_GFM
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_MATH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_TABLES
}

/// Offset event stream from Grok's parser, with single-tilde strikethrough demoted.
///
/// Prefer this over `Parser::new_ext(...).into_offset_iter()` so analysis and
/// rendering agree on what counts as strikethrough.
pub fn offset_events(text: &str) -> impl Iterator<Item = (Event<'_>, Range<usize>)> + '_ {
    DoubleTildeOnlyStrike {
        text,
        events: Parser::new_ext(text, parser_options()).into_offset_iter(),
    }
}

/// Stackless filter: Start and End share the same byte span in pulldown, so both
/// are classified by whether that span opens with `~~`. Single-tilde frames emit
/// delimiter `Text` instead of strike tags (delimiters are not separate events).
struct DoubleTildeOnlyStrike<'a, I> {
    text: &'a str,
    events: I,
}

/// True when the strike span at `range.start` is the double-tilde form.
fn is_double_tilde_strike(text: &str, range: &Range<usize>) -> bool {
    text.get(range.start..).is_some_and(|s| s.starts_with("~~"))
}

/// Opening or closing delimiter byte as `Text`, with the matching source range.
fn strike_delim_text<'a>(
    text: &'a str,
    range: &Range<usize>,
    opening: bool,
) -> (Event<'a>, Range<usize>) {
    let delim = if opening {
        let end = range.start + 1;
        debug_assert!(text.is_char_boundary(end) && end <= text.len());
        (range.start..end, &text[range.start..end])
    } else {
        let start = range.end - 1;
        debug_assert!(text.is_char_boundary(start) && start < text.len());
        (start..range.end, &text[start..range.end])
    };
    (Event::Text(delim.1.into()), delim.0)
}

impl<'a, I> Iterator for DoubleTildeOnlyStrike<'a, I>
where
    I: Iterator<Item = (Event<'a>, Range<usize>)>,
{
    type Item = (Event<'a>, Range<usize>);

    fn next(&mut self) -> Option<Self::Item> {
        let (event, range) = self.events.next()?;
        match &event {
            Event::Start(Tag::Strikethrough) if !is_double_tilde_strike(self.text, &range) => {
                Some(strike_delim_text(self.text, &range, true))
            }
            Event::End(TagEnd::Strikethrough) if !is_double_tilde_strike(self.text, &range) => {
                Some(strike_delim_text(self.text, &range, false))
            }
            _ => Some((event, range)),
        }
    }
}

/// Counts of markdown elements found in a document.
///
/// Counting mirrors the `pulldown-cmark` event stream the renderer walks, so a few
/// overlaps are intentional and documented per-field below.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MarkdownStats {
    pub h1: u32,
    pub h2: u32,
    pub h3: u32,
    pub h4: u32,
    pub h5: u32,
    pub h6: u32,
    pub tables: u32,
    pub fenced_code: u32,
    pub indented_code: u32,
    pub inline_code: u32,
    pub strong: u32,
    pub emphasis: u32,
    pub strikethrough: u32,
    /// All GFM link types: inline, reference, collapsed, shortcut-when-defined,
    /// angle-bracket autolink, and email autolink.
    pub links: u32,
    /// Markup inside an image's alt text is still counted (e.g. `![**x**](u)` bumps `strong`).
    pub images: u32,
    pub blockquotes: u32,
    pub thematic_breaks: u32,
    pub inline_math: u32,
    pub display_math: u32,
    /// Subset of `list_items`: a task-list item is also a list item.
    pub task_list_items: u32,
    /// Container lists are not counted (no `lists` field); nested list items are included.
    pub list_items: u32,
}

impl MarkdownStats {
    /// Total heading count across all levels, derived from `h1..=h6`.
    pub fn headings(&self) -> u32 {
        self.h1 + self.h2 + self.h3 + self.h4 + self.h5 + self.h6
    }

    /// Single source of truth for name->value mapping; the exhaustive destructure makes adding a field a compile error here, so downstream consumers cannot drift from the struct.
    pub fn as_pairs(&self) -> [(&'static str, u32); 22] {
        // Exhaustive (no `..`): adding a field to MarkdownStats fails to compile until it is mapped below.
        let Self {
            h1,
            h2,
            h3,
            h4,
            h5,
            h6,
            tables,
            fenced_code,
            indented_code,
            inline_code,
            strong,
            emphasis,
            strikethrough,
            links,
            images,
            blockquotes,
            thematic_breaks,
            inline_math,
            display_math,
            task_list_items,
            list_items,
        } = *self;
        [
            ("headings", self.headings()),
            ("h1", h1),
            ("h2", h2),
            ("h3", h3),
            ("h4", h4),
            ("h5", h5),
            ("h6", h6),
            ("tables", tables),
            ("fenced_code", fenced_code),
            ("indented_code", indented_code),
            ("inline_code", inline_code),
            ("strong", strong),
            ("emphasis", emphasis),
            ("strikethrough", strikethrough),
            ("links", links),
            ("images", images),
            ("blockquotes", blockquotes),
            ("thematic_breaks", thematic_breaks),
            ("inline_math", inline_math),
            ("display_math", display_math),
            ("task_list_items", task_list_items),
            ("list_items", list_items),
        ]
    }
}

/// A render-fidelity failure: the model emitted markdown that does not render as
/// the structure it clearly intended.
///
/// Distinct from [`MarkdownStats`] counts: a count answers "how many tables", an
/// issue answers "did a construct silently degrade". `pulldown-cmark` never errors
/// (CommonMark is total), so each issue is detected by comparing intent (the raw
/// syntax) against what actually parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructuralIssue {
    /// A GFM table delimiter row (`|---|---|`) sits under a header line, but the
    /// table did not parse (e.g. the delimiter's column count != the header's), so
    /// the lines render as a paragraph -- the "made a table but it didn't show" bug.
    MalformedTable,
    /// A fenced code block runs to EOF without a closing fence, swallowing the rest of the message.
    UnterminatedCodeBlock,
}

impl StructuralIssue {
    /// Stable snake_case name for this issue (for logs, metrics, or FFI bindings).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MalformedTable => "malformed_table",
            Self::UnterminatedCodeBlock => "unterminated_code_block",
        }
    }
}

/// Element counts plus any structural issues from a single parse pass.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MarkdownAnalysis {
    pub stats: MarkdownStats,
    pub issues: Vec<StructuralIssue>,
}

/// Strip the container markers pulldown keeps on each source line (`>` for blockquotes, indent for lists).
fn strip_block_prefix(line: &str) -> &str {
    line.trim_start_matches(['>', ' ', '\t'])
}

/// Unterminated iff no line after the opener is a closing fence matching the opener's char and length.
///
/// Works off raw block source (the only thing carrying closure info), so a verbatim fence line in
/// content could mask a real EOF -- a rare, safe-direction (under-penalizing) miss we accept for simplicity.
fn fenced_block_is_unterminated(block_src: &str) -> bool {
    let mut lines = block_src.lines();
    let Some(open) = lines.next().map(strip_block_prefix) else {
        return false;
    };
    let Some(fence_char) = open.chars().next().filter(|c| matches!(c, '`' | '~')) else {
        return false;
    };
    let open_len = open.chars().take_while(|c| *c == fence_char).count();
    !lines.any(|line| {
        let close = strip_block_prefix(line).trim_end();
        // `open_len >= 3`, so `close.len() >= open_len` already implies non-empty.
        close.len() >= open_len && close.chars().all(|c| c == fence_char)
    })
}

/// A GFM table delimiter row: only `|`, `-`, `:`, and whitespace, with at least one
/// pipe and one dash. The pipe requirement rejects a bare `---` thematic break or a
/// setext `-----` underline; the dash requirement rejects a `|||`-only row.
fn is_table_delimiter_line(line: &str) -> bool {
    let line = line.trim();
    line.contains('|')
        && line.contains('-')
        && line
            .chars()
            .all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

/// A line that could be a table header: non-empty, containing a column pipe, and
/// not itself delimiter-shaped (a `|---|` row arming the next line would chain one
/// broken table into a duplicate flag per extra delimiter row).
fn line_looks_like_header(line: &str) -> bool {
    let line = line.trim();
    !line.is_empty() && line.contains('|') && !is_table_delimiter_line(line)
}

/// Flag delimiter rows the model intended as a table but that `pulldown-cmark` did
/// not parse as one (so they render as a paragraph -- the broken-table bug).
///
/// `parsed_spans` are the byte ranges of real tables and code blocks: a delimiter
/// line starting inside one is either part of a valid table or literal code text, so
/// it never signals a malformed table. Everything else is fair game -- a delimiter
/// row directly under a pipe-bearing header line is an intended-but-unparsed table.
fn detect_malformed_tables(
    text: &str,
    parsed_spans: &[Range<usize>],
    issues: &mut Vec<StructuralIssue>,
) {
    let in_parsed_span = |offset: usize| parsed_spans.iter().any(|span| span.contains(&offset));

    let mut prev_is_header = false;
    let mut offset = 0;
    // `split_inclusive` keeps the trailing `\n`, so summing lengths tracks byte offsets exactly.
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();

        // A `|---|` line inside a parsed table/code block is not an intended-but-broken table.
        let excluded = in_parsed_span(start);
        let content = strip_block_prefix(line.trim_end_matches(['\n', '\r']));

        if !excluded && prev_is_header && is_table_delimiter_line(content) {
            issues.push(StructuralIssue::MalformedTable);
        }
        // The delimiter's header must be the immediately-preceding, non-excluded pipe line.
        prev_is_header = !excluded && line_looks_like_header(content);
    }
}

/// Parse `text` with Grok Build's options; count elements and flag structural issues.
pub fn analyze(text: &str) -> MarkdownAnalysis {
    let mut stats = MarkdownStats::default();
    let mut issues = Vec::new();
    // Byte ranges of constructs where a `|---|`-shaped line is legitimately not an
    // intended table (real tables and code blocks); consumed by `detect_malformed_tables`.
    let mut parsed_spans: Vec<Range<usize>> = Vec::new();

    // u32 element counters can't overflow: model output is token-bounded, far below `u32::MAX`.
    // `offset_events` attaches byte ranges and demotes single-tilde strike so counts match render.
    for (event, range) in offset_events(text) {
        // Structural-issue bookkeeping, tracked alongside the element counting below.
        match &event {
            // A parsed table's span covers its delimiter row -- exclude it from malformed-table scanning.
            Event::Start(Tag::Table(_)) => parsed_spans.push(range.clone()),
            // The range spans the opening fence through the close (or to EOF when unterminated).
            Event::Start(Tag::CodeBlock(kind)) => {
                if matches!(kind, CodeBlockKind::Fenced(_))
                    && fenced_block_is_unterminated(&text[range.clone()])
                {
                    issues.push(StructuralIssue::UnterminatedCodeBlock);
                }
                parsed_spans.push(range.clone());
            }
            _ => {}
        }

        match event {
            Event::Start(Tag::Heading { level, .. }) => match level {
                HeadingLevel::H1 => stats.h1 += 1,
                HeadingLevel::H2 => stats.h2 += 1,
                HeadingLevel::H3 => stats.h3 += 1,
                HeadingLevel::H4 => stats.h4 += 1,
                HeadingLevel::H5 => stats.h5 += 1,
                HeadingLevel::H6 => stats.h6 += 1,
            },
            Event::Start(Tag::Table(_)) => stats.tables += 1,
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(_))) => stats.fenced_code += 1,
            Event::Start(Tag::CodeBlock(CodeBlockKind::Indented)) => stats.indented_code += 1,
            Event::Start(Tag::Strong) => stats.strong += 1,
            Event::Start(Tag::Emphasis) => stats.emphasis += 1,
            Event::Start(Tag::Strikethrough) => stats.strikethrough += 1,
            Event::Start(Tag::Link { .. }) => stats.links += 1,
            Event::Start(Tag::Image { .. }) => stats.images += 1,
            Event::Start(Tag::BlockQuote(_)) => stats.blockquotes += 1,
            Event::Start(Tag::Item) => stats.list_items += 1,
            Event::Code(_) => stats.inline_code += 1,
            Event::InlineMath(_) => stats.inline_math += 1,
            Event::DisplayMath(_) => stats.display_math += 1,
            Event::Rule => stats.thematic_breaks += 1,
            Event::TaskListMarker(_) => stats.task_list_items += 1,
            _ => {}
        }
    }

    // Second pass: with every real-table/code-block span known, flag delimiter rows
    // the model intended as tables but that did not parse as one.
    detect_malformed_tables(text, &parsed_spans, &mut issues);

    MarkdownAnalysis { stats, issues }
}

