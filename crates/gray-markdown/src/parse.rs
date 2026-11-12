use std::ops::Range;

use anstyle::Style;
use pulldown_cmark::{CodeBlockKind, CowStr, Event, Tag, TagEnd, TextMergeWithOffset};
use ratatui::style::Stylize as RatatuiStylize;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;

use crate::buffers::{
    Highlight, LinkTarget, MarkdownBuffers, Replace, StyledCell, TableHyperlink, TableReplace,
    TableState, Transform, unicode_display_width,
};
use crate::checkpoint::CheckpointKind;
use crate::colors::anstyle_to_ratatui_style;
use crate::style::{MarkdownStyle, TableBorders};
use crate::syntax::{Syntect, syntax_highlight_raw};

fn find_substring(
    haystack: &str,
    needle: &CowStr,
    allow_outside: bool,
    rfind: bool,
) -> Option<Range<usize>> {
    if needle.is_empty() || haystack.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    if !allow_outside {
        if let CowStr::Borrowed(needle) = needle {
            let hp = haystack.as_ptr().addr();
            let np = needle.as_ptr().addr();
            let (he, ne) = (hp + haystack.len(), np + needle.len());
            if np >= hp && ne <= he {
                let offset = np - hp;
                let range = offset..(offset + needle.len());
                if haystack.as_bytes().get(range.clone()) == Some(needle.as_bytes()) {
                    return Some(range);
                }
            }
        }
        None
    } else {
        if rfind {
            haystack.rfind(needle.as_ref())
        } else {
            haystack.find(needle.as_ref())
        }
        .map(|pos| pos..(pos + needle.len()))
    }
}

fn decode_html_entity(entity: &str) -> Option<String> {
    let decoded = html_escape::decode_html_entities(entity);

    if decoded.as_ref() == entity {
        return None;
    }
    if decoded.chars().any(char::is_control) {
        return None;
    }
    Some(decoded.into_owned())
}

pub fn decode_html_entities_text(text: &str) -> String {
    const MAX_ENTITY_LEN: usize = 33;
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut literal_start = 0;
    while let Some(&b) = bytes.get(i) {
        if b != b'&' {
            i += 1;
            continue;
        }
        let max = (i + MAX_ENTITY_LEN).min(bytes.len());
        let mut j = i + 1;
        let end = loop {
            if j >= max {
                break None;
            }
            match bytes.get(j) {
                Some(b';') => break Some(j),
                Some(b'#' | b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9') => j += 1,
                _ => break None,
            }
        };
        if let Some(end) = end
            && let Some(entity) = text.get(i..=end)
            && let Some(decoded) = decode_html_entity(entity)
        {
            out.push_str(&text[literal_start..i]);
            out.push_str(&decoded);
            i = end + 1;
            literal_start = i;
            continue;
        }
        i += 1;
    }
    out.push_str(&text[literal_start..]);
    out
}

fn has_blank_line_after(text: &str, pos: usize) -> bool {
    text.as_bytes()[pos..]
        .iter()
        .copied()
        .find(|&c| c != b' ' && c != b'\t')
        == Some(b'\n')
}

pub struct MarkdownParser<'a, 'b, 'syn> {
    text: &'a str,
    ms: MarkdownStyle,
    buffers: &'b mut MarkdownBuffers,
    syntect: Option<&'syn Syntect>,

    tag_stack: Vec<Tag<'a>>,
    table_state: Option<TableState>,
    depth: usize,

    bq_depth: usize,
    last_checkpoint: Option<(CheckpointKind, usize)>,

    max_table_width: Option<usize>,

    link_id_counter: u32,
}

fn is_br_tag(html: &str) -> bool {
    let Some(inner) = html
        .trim()
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
    else {
        return false;
    };
    let tag = inner.trim();
    let tag = tag.strip_suffix('/').map_or(tag, str::trim);
    tag.eq_ignore_ascii_case("br")
}

pub(crate) fn cell_word_separator<'a>(
    line: &'a str,
) -> Box<dyn Iterator<Item = textwrap::core::Word<'a>> + 'a> {
    let mut breaks: Vec<(usize, usize)> = Vec::new();
    {
        let mut in_whitespace = false;
        let mut after_break_char = false;
        let mut prev_is_digit = false;
        let mut digit_before_break = false;
        let mut last_break_ch: char = '\0';
        let mut break_char_start: usize = 0;
        for (idx, ch) in line.char_indices() {
            let is_space = ch == ' ';
            let is_break_char = !is_space && !ch.is_alphanumeric();

            let should_break = if in_whitespace && !is_space {
                true
            } else if after_break_char {
                if ch.is_alphabetic() {
                    true
                } else if ch.is_ascii_digit() && digit_before_break {
                    last_break_ch != ',' && last_break_ch != '.'
                } else {
                    false
                }
            } else {
                false
            };

            if should_break {
                if in_whitespace {
                    breaks.push((idx, idx));
                } else {
                    breaks.push((idx, break_char_start));
                }
            }

            if is_break_char {
                break_char_start = idx;
                last_break_ch = ch;
                digit_before_break = prev_is_digit;
            }
            prev_is_digit = ch.is_ascii_digit();
            in_whitespace = is_space;
            after_break_char = is_break_char;
        }
    }

    let url_ranges: Vec<Range<usize>> = {
        let mut ranges = Vec::new();
        let mut pos = 0;
        for token in line.split_whitespace() {
            let start = line[pos..].find(token).unwrap() + pos;
            let end = start + token.len();
            if url::Url::parse(token).is_ok() {
                ranges.push(start..end);
            }
            pos = end;
        }
        ranges
    };
    breaks.retain(|&(break_pos, _)| {
        !url_ranges
            .iter()
            .any(|r| break_pos > r.start && break_pos < r.end)
    });

    let mut split_positions: Vec<usize> = Vec::with_capacity(breaks.len());
    {
        let len = line.len();
        for (i, &(attach_left, attach_right)) in breaks.iter().enumerate() {
            if attach_left == attach_right {
                split_positions.push(attach_left);
            } else {
                let seg_start = if i == 0 { 0 } else { split_positions[i - 1] };
                let seg_end = if i + 1 < breaks.len() {
                    breaks[i + 1].0
                } else {
                    len
                };

                let left_if_attach_left = unicode_display_width(&line[seg_start..attach_left]);
                let right_if_attach_left = unicode_display_width(&line[attach_left..seg_end]);
                let max_attach_left = left_if_attach_left.max(right_if_attach_left);

                let left_if_attach_right = unicode_display_width(&line[seg_start..attach_right]);
                let right_if_attach_right = unicode_display_width(&line[attach_right..seg_end]);
                let max_attach_right = left_if_attach_right.max(right_if_attach_right);

                if max_attach_right < max_attach_left {
                    split_positions.push(attach_right);
                } else {
                    split_positions.push(attach_left);
                }
            }
        }
    }

    let mut pos = 0usize;
    let mut idx = 0usize;
    Box::new(std::iter::from_fn(move || {
        if pos >= line.len() {
            return None;
        }
        let end = if idx < split_positions.len() {
            let e = split_positions[idx];
            idx += 1;
            e
        } else {
            line.len()
        };
        let word = textwrap::core::Word::from(&line[pos..end]);
        pos = end;
        Some(word)
    }))
}

#[derive(Default)]
struct FormattedTable {
    lines: Vec<String>,

    styled_lines: Vec<Line<'static>>,

    line_source_offsets: Vec<usize>,

    hyperlinks: Vec<TableHyperlink>,
}

impl<'a, 'b, 'syn> MarkdownParser<'a, 'b, 'syn> {
    pub fn new(
        text: &'a str,
        ms: MarkdownStyle,
        buffers: &'b mut MarkdownBuffers,
        syntect: Option<&'syn Syntect>,
    ) -> Self {
        Self {
            text,
            ms,
            buffers,
            syntect,
            tag_stack: Vec::new(),
            table_state: None,
            depth: 0,
            bq_depth: 0,
            last_checkpoint: None,
            max_table_width: None,
            link_id_counter: 0,
        }
    }

    pub fn max_table_width(mut self, width: Option<usize>) -> Self {
        self.max_table_width = width;
        self
    }

    pub(crate) fn link_id_start(mut self, id: u32) -> Self {
        self.link_id_counter = id;
        self
    }

    pub fn parse(mut self) -> ParsedMarkdown<'a, 'b> {
        self.tag_stack.clear();
        self.buffers.clear();
        self.table_state = None;
        self.depth = 0;
        self.last_checkpoint = None;

        for (event, range) in
            TextMergeWithOffset::new(crate::markdown_core::offset_events(self.text))
        {
            self.on_event(event, range);
        }

        ParsedMarkdown::new(
            self.text,
            self.ms,
            self.buffers,
            self.last_checkpoint,
            self.link_id_counter,
        )
    }

    fn push_highlight(&mut self, style: Option<Style>, range: &Range<usize>) {
        self.buffers.highlights.push(Highlight {
            style,
            range: range.clone(),
        });
    }

    fn on_event(&mut self, event: Event<'a>, range: Range<usize>) {
        let mut parent_code_block = None;

        let skip_inner_style = matches!(
            event,
            Event::Start(_) | Event::End(_) | Event::Code(_) | Event::InlineMath(_)
        );

        let ancestor_styles: Vec<Option<Style>> = if !skip_inner_style {
            let in_link = self
                .tag_stack
                .iter()
                .any(|t| matches!(t, Tag::Link { .. } | Tag::Image { .. }));
            let strip_fg_in_link =
                |style: Style| if in_link { style.fg_color(None) } else { style };
            self.tag_stack
                .iter()
                .filter_map(|ancestor| match ancestor {
                    Tag::Heading { level, .. } => {
                        Some(Some(self.ms.heading_inner[(*level as i32) as usize - 1]))
                    }
                    Tag::Emphasis => Some(Some(strip_fg_in_link(self.ms.emphasis_inner))),
                    Tag::Strong => Some(Some(strip_fg_in_link(self.ms.strong_inner))),
                    Tag::Strikethrough => Some(Some(strip_fg_in_link(self.ms.strikethrough_inner))),

                    Tag::Link { .. } | Tag::Image { .. } => Some(None),
                    Tag::CodeBlock(block) => {
                        parent_code_block = Some(match block {
                            CodeBlockKind::Fenced(lang) if !lang.is_empty() => {
                                Some(lang.to_owned())
                            }
                            _ => None,
                        });
                        None
                    }
                    _ => None,
                })
                .collect()
        } else {
            if let Some(Tag::CodeBlock(block)) = self.tag_stack.last() {
                parent_code_block = Some(match block {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => Some(lang.to_owned()),
                    _ => None,
                });
            }
            Vec::new()
        };

        for style in &ancestor_styles {
            self.push_highlight(*style, &range);
        }

        match event {
            Event::Start(tag) => self.on_start(tag, range),
            Event::End(tag_end) => self.on_end(tag_end, range),
            Event::Text(text) => {
                if let Some(ref mut state) = self.table_state {
                    state.push_text(&text);
                }

                if let Some(parent_code_block) = parent_code_block {
                    let highlighted = match parent_code_block {
                        Some(lang) => syntax_highlight_raw(self.syntect, &lang, &text),
                        None => None,
                    };
                    if let Some(highlighted) = highlighted {
                        self.buffers.replaces.push(Replace {
                            highlighted,
                            range: range.clone(),
                        });
                    } else {
                        self.push_highlight(Some(self.ms.code_untagged), &range);
                        self.buffers.untagged_code_ranges.push(range.clone());
                    }
                } else {
                    if ancestor_styles.is_empty() {
                        self.push_highlight(Some(self.ms.text), &range);
                    } else {
                        self.push_highlight(None, &range);
                    }
                    if self.table_state.is_none() {
                        self.scan_inline_html_entities(&range);
                        self.expand_tabs(&range);
                    }
                }
            }
            Event::Code(code) => {
                if let Some(ref mut state) = self.table_state {
                    let prev_code = state.cell_style.code;
                    state.cell_style.code = true;
                    state.push_text(&code);
                    state.cell_style.code = prev_code;
                }
                self.style_inline_code_span(&code, &range);
            }
            Event::InlineMath(math) => {
                if let Some(ref mut state) = self.table_state {
                    let prev_code = state.cell_style.code;
                    state.cell_style.code = true;
                    state.push_text(&math);
                    state.cell_style.code = prev_code;
                }

                self.style_inline_code_span(&math, &range);
            }
            Event::SoftBreak => {
                if let Some(ref mut state) = self.table_state {
                    state.push_text(" ");
                } else {
                    let next = self.text.as_bytes().get(range.end);
                    let is_continuation = matches!(next, Some(b' ' | b'\t' | b'>' | b'|'));
                    if !is_continuation {
                        let span = range.end - range.start;
                        debug_assert!(span >= 1, "SoftBreak range must cover at least one byte");
                        self.buffers.transforms.push(Transform {
                            range: range.clone(),
                            to: " ".repeat(span),
                            force: true,
                        });
                    }
                }
                self.push_highlight(None, &range);
            }
            Event::HardBreak => {
                if let Some(ref mut state) = self.table_state {
                    state.push_text("\n");
                }
                self.push_highlight(None, &range);
            }
            Event::Html(_) => {
                self.push_highlight(Some(self.ms.text), &range);
            }
            Event::InlineHtml(html) => {
                if is_br_tag(&html) {
                    if let Some(ref mut state) = self.table_state {
                        state.push_text("\n");
                        self.push_highlight(Some(self.ms.text), &range);
                    } else {
                        self.buffers.transforms.push(Transform {
                            range: range.clone(),
                            to: "\n".to_string(),
                            force: false,
                        });
                        self.push_highlight(None, &range);
                    }
                } else if let Some(ref mut state) = self.table_state {
                    state.push_text(&html);
                    self.push_highlight(Some(self.ms.text), &range);
                } else if let Some(highlighted) = syntax_highlight_raw(self.syntect, "html", &html)
                {
                    self.buffers.replaces.push(Replace {
                        highlighted,
                        range: range.clone(),
                    });
                }
            }
            Event::DisplayMath(math) => {
                if let Some(ref mut state) = self.table_state {
                    let prev_code = state.cell_style.code;
                    state.cell_style.code = true;
                    state.push_text(&math);
                    state.cell_style.code = prev_code;
                    self.push_highlight(Some(self.ms.math), &range);
                } else {
                    self.push_highlight(Some(self.ms.code_outer), &range);
                    let outer_text = &self.text[range.clone()];
                    if let Some(r) = find_substring(outer_text, &math, true, false) {
                        let inner_range = (range.start + r.start)..(range.start + r.end);
                        if let Some(highlighted) = syntax_highlight_raw(self.syntect, "tex", &math)
                        {
                            self.buffers.replaces.push(Replace {
                                highlighted,
                                range: inner_range,
                            });
                        } else {
                            self.push_highlight(Some(self.ms.code_untagged), &inner_range);
                        }
                    }
                }
            }
            Event::FootnoteReference(_) => {
                self.push_highlight(Some(self.ms.link_outer), &range);
            }
            Event::Rule => {
                self.push_highlight(Some(self.ms.rule), &range);
                let rule_text = &self.text[range.clone()];
                if let Some(marker_end) = rule_text.find('\n') {
                    self.buffers.transforms.push(Transform {
                        range: range.start..range.start + marker_end,
                        to: "───".to_string(),
                        force: false,
                    });
                } else {
                    self.buffers.transforms.push(Transform {
                        range: range.clone(),
                        to: "───".to_string(),
                        force: false,
                    });
                }
                if self.depth == 0 {
                    self.last_checkpoint = Some((CheckpointKind::ThematicBreak, range.end));
                }
            }
            Event::TaskListMarker(checked) => {
                let style = if checked {
                    self.ms.task_checked
                } else {
                    self.ms.task_unchecked
                };
                self.push_highlight(Some(style), &range);
            }
        }
    }

    fn on_start(&mut self, tag: Tag<'a>, range: Range<usize>) {
        match &tag {
            Tag::BlockQuote(_) | Tag::List(_) | Tag::Item | Tag::Table(_) => {
                self.depth += 1;
            }
            _ => {}
        }
        if matches!(&tag, Tag::BlockQuote(_)) {
            self.bq_depth += 1;
        }

        let mut more = Vec::new();
        let style = match &tag {
            Tag::Paragraph => None,
            Tag::Heading { level, .. } => {
                let level_usize = (*level as usize).saturating_sub(1).min(5);
                let heading_text = &self.text[range.clone()];
                if let Some(marker_end) = heading_text.find(|c: char| c != '#' && c != ' ') {
                    let marker_range = range.start..range.start + marker_end;
                    more.push(Highlight {
                        style: Some(self.ms.heading_outer[level_usize]),
                        range: marker_range,
                    });
                    None
                } else {
                    Some(self.ms.heading_outer[level_usize])
                }
            }
            Tag::BlockQuote(_) => {
                let bq_text = &self.text[range.clone()];
                let mut pos = range.start;

                for line in bq_text.split_inclusive('\n') {
                    let at_line_start =
                        pos == 0 || self.text.as_bytes().get(pos - 1) == Some(&b'\n');

                    let skip = if at_line_start { self.bq_depth - 1 } else { 0 };

                    let mut found = 0usize;
                    for (byte_offset, ch) in line.char_indices() {
                        if ch == '>' {
                            if found == skip {
                                let gt_pos = pos + byte_offset;
                                self.buffers.transforms.push(Transform {
                                    range: gt_pos..gt_pos + 1,
                                    to: "│".to_string(),
                                    force: false,
                                });
                                more.push(Highlight {
                                    style: Some(self.ms.blockquote_outer),
                                    range: gt_pos..gt_pos + 1,
                                });
                                break;
                            }
                            found += 1;
                        }
                    }

                    pos += line.len();
                }

                None
            }
            Tag::CodeBlock(code) => {
                let line_start = self.text[..range.start].rfind('\n').map_or(0, |p| p + 1);
                let fence_start = if self.text[line_start..range.start]
                    .bytes()
                    .all(|b| b == b' ' || b == b'\t')
                {
                    line_start
                } else {
                    range.start
                };
                more.push(Highlight {
                    style: Some(self.ms.code_outer),
                    range: fence_start..range.end,
                });
                match code {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => {
                        if let Some(r) =
                            find_substring(&self.text[range.clone()], lang, true, false)
                        {
                            let range = (r.start + range.start)..(r.end + range.start);
                            more.push(Highlight {
                                style: Some(self.ms.code_language),
                                range,
                            });
                        }
                    }
                    _ => (),
                }
                None
            }
            Tag::HtmlBlock => None,
            Tag::List(_) => None,
            Tag::Item => {
                let item_text = &self.text[range.clone()];
                let trimmed = item_text.trim_start();
                let leading_ws = item_text.len() - trimmed.len();

                let marker_len = if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
                    2
                } else if let Some(pos) = trimmed.find(". ") {
                    if pos > 0 && trimmed[..pos].chars().all(|c| c.is_ascii_digit()) {
                        pos + 2
                    } else {
                        0
                    }
                } else if let Some(pos) = trimmed.find(") ") {
                    if pos > 0 && trimmed[..pos].chars().all(|c| c.is_ascii_digit()) {
                        pos + 2
                    } else {
                        0
                    }
                } else {
                    0
                };

                if marker_len > 0 {
                    let marker_start = range.start + leading_ws;
                    let marker_end = marker_start + marker_len;
                    more.push(Highlight {
                        style: Some(self.ms.list_item),
                        range: marker_start..marker_end,
                    });

                    if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
                        self.buffers.transforms.push(Transform {
                            range: marker_start..marker_start + 1,
                            to: "•".to_string(),
                            force: false,
                        });
                    }
                }
                None
            }
            Tag::Table(alignments) => {
                self.table_state = Some(TableState::new(alignments.to_vec(), range.start));
                Some(self.ms.table_outer)
            }
            Tag::TableHead => {
                if let Some(ref mut state) = self.table_state {
                    state.in_header = true;
                }
                Some(self.ms.table_outer)
            }
            Tag::TableRow => {
                if let Some(ref mut state) = self.table_state {
                    state.current_row.clear();
                }
                Some(self.ms.table_outer)
            }
            Tag::TableCell => {
                if let Some(ref mut state) = self.table_state {
                    state.current_cell.clear();
                }
                self.push_highlight(None, &range);
                None
            }
            Tag::Emphasis => {
                if let Some(ref mut state) = self.table_state {
                    state.cell_style.italic = true;
                }
                Some(self.ms.emphasis_outer)
            }
            Tag::Strong => {
                if let Some(ref mut state) = self.table_state {
                    state.cell_style.bold = true;
                }
                Some(self.ms.strong_outer)
            }
            Tag::Strikethrough => Some(self.ms.strikethrough_outer),
            Tag::Link {
                dest_url, title, ..
            }
            | Tag::Image {
                dest_url, title, ..
            } => {
                if let Some(ref mut state) = self.table_state {
                    let id = self.link_id_counter;
                    self.link_id_counter += 1;
                    state.cell_style.link = Some((dest_url.to_string(), id));
                    self.tag_stack.push(tag);
                    return;
                }

                let tag_str = &self.text[range.clone()];

                if !title.is_empty() {
                    for t in [format!("\"{title}\""), format!("'{title}'")].map(CowStr::from) {
                        if let Some(r) = find_substring(tag_str, &t, true, true) {
                            let title_range = (r.start + range.start)..(r.end + range.start);
                            more.push(Highlight {
                                style: Some(self.ms.link_title),
                                range: title_range,
                            });
                        }
                    }
                }

                let hide_dest = !tag_str.starts_with("![");
                let url_rel_opt = find_substring(tag_str, dest_url, true, true);
                if !hide_dest && let Some(r) = &url_rel_opt {
                    let url_range = (r.start + range.start)..(r.end + range.start);
                    more.push(Highlight {
                        style: Some(self.ms.link_url),
                        range: url_range,
                    });
                }

                let bracket_pos_opt = url_rel_opt
                    .as_ref()
                    .and_then(|r| tag_str[..r.start].rfind("](").map(|p| p..p + 2));
                if let Some(bracket_pos) = bracket_pos_opt {
                    let open_bracket = if tag_str.starts_with("![") { 1 } else { 0 };
                    if open_bracket > 0 {
                        more.push(Highlight {
                            style: Some(self.ms.link_outer),
                            range: range.start..range.start + 1,
                        });
                    }
                    more.push(Highlight {
                        style: Some(self.ms.link_outer),
                        range: range.start + open_bracket..range.start + open_bracket + 1,
                    });
                    let text_start = range.start + open_bracket + 1;
                    let text_end = bracket_pos.start + range.start;
                    if text_end > text_start {
                        more.push(Highlight {
                            style: Some(self.ms.link_text),
                            range: text_start..text_end,
                        });
                    }
                    let bracket_abs = bracket_pos.start + range.start;
                    more.push(Highlight {
                        style: Some(self.ms.link_outer),
                        range: bracket_abs..bracket_abs + 2,
                    });
                    more.push(Highlight {
                        style: Some(self.ms.link_outer),
                        range: range.end - 1..range.end,
                    });
                    self.buffers.transforms.push(Transform {
                        range: range.start + open_bracket..range.start + open_bracket + 1,
                        to: "".to_string(),
                        force: false,
                    });

                    self.buffers.transforms.push(Transform {
                        range: bracket_abs..if hide_dest {
                            range.end
                        } else {
                            bracket_abs + 2
                        },
                        to: if hide_dest { "" } else { " (" }.to_string(),
                        force: false,
                    });
                    if text_end > text_start {
                        self.buffers.link_targets.push(LinkTarget {
                            source_range: text_start..text_end,
                            url: dest_url.to_string(),
                            id: self.link_id_counter,
                        });
                        self.link_id_counter += 1;
                    }
                    None
                } else {
                    self.buffers.link_targets.push(LinkTarget {
                        source_range: range.clone(),
                        url: dest_url.to_string(),
                        id: self.link_id_counter,
                    });
                    self.link_id_counter += 1;
                    Some(self.ms.link_outer)
                }
            }
            _ => None,
        };

        if let Some(style) = style {
            self.push_highlight(Some(style), &range);
        }
        for hl in more {
            self.buffers.highlights.push(hl);
        }

        self.tag_stack.push(tag);
    }

    fn on_end(&mut self, tag_end: TagEnd, range: Range<usize>) {
        self.tag_stack.pop();

        let style = match &tag_end {
            TagEnd::Emphasis => {
                if let Some(ref mut state) = self.table_state {
                    state.cell_style.italic = false;
                }
                None
            }
            TagEnd::Strong => {
                if let Some(ref mut state) = self.table_state {
                    state.cell_style.bold = false;
                }
                None
            }
            TagEnd::Strikethrough => None,
            TagEnd::Link | TagEnd::Image => {
                if let Some(ref mut state) = self.table_state {
                    state.cell_style.link = None;
                }
                None
            }
            TagEnd::TableCell => {
                if let Some(ref mut state) = self.table_state {
                    state
                        .current_row
                        .push(std::mem::take(&mut state.current_cell));

                    state.cell_style.reset();
                }
                None
            }
            TagEnd::TableRow => {
                if let Some(ref mut state) = self.table_state
                    && !state.in_header
                {
                    let row = std::mem::take(&mut state.current_row);
                    state.rows.push(row);
                }
                None
            }
            TagEnd::TableHead => {
                if let Some(ref mut state) = self.table_state {
                    state.header = std::mem::take(&mut state.current_row);
                    state.in_header = false;
                }
                None
            }
            TagEnd::Table => {
                if let Some(mut state) = self.table_state.take() {
                    state.range.end = range.end;
                    let FormattedTable {
                        lines,
                        styled_lines,
                        line_source_offsets,
                        hyperlinks,
                    } = self.format_table(&state);
                    self.buffers.table_replaces.push(TableReplace {
                        lines,
                        styled_lines,
                        range: state.range,
                        line_source_offsets,
                        hyperlinks,
                    });
                }
                None
            }
            _ => None,
        };

        if let Some(style) = style {
            self.push_highlight(Some(style), &range);
        }

        match &tag_end {
            TagEnd::BlockQuote(_) | TagEnd::List(_) | TagEnd::Item | TagEnd::Table => {
                self.depth = self.depth.saturating_sub(1);
            }
            _ => {}
        }
        if matches!(&tag_end, TagEnd::BlockQuote(_)) {
            self.bq_depth = self.bq_depth.saturating_sub(1);
        }

        if self.depth == 0 {
            let kind = match &tag_end {
                TagEnd::Paragraph => Some(CheckpointKind::Paragraph),
                TagEnd::Heading(_) => Some(CheckpointKind::Heading),
                TagEnd::CodeBlock => Some(CheckpointKind::CodeBlock),
                TagEnd::BlockQuote(_) => Some(CheckpointKind::BlockQuote),
                TagEnd::List(_) => Some(CheckpointKind::List),
                TagEnd::Table => Some(CheckpointKind::Table),
                TagEnd::HtmlBlock => Some(CheckpointKind::HtmlBlock),
                _ => None,
            };

            if let Some(kind) = kind {
                let has_blank = has_blank_line_after(self.text, range.end);
                let is_code_block = matches!(kind, CheckpointKind::CodeBlock);
                let at_eof = range.end >= self.text.len();
                let code_block_properly_closed = is_code_block && !at_eof;

                if has_blank || code_block_properly_closed {
                    let checkpoint_pos = if is_code_block && has_blank {
                        range.end + 1
                    } else {
                        range.end
                    };
                    self.last_checkpoint = Some((kind, checkpoint_pos));
                }
            }
        }
    }

    fn style_inline_code_span(&mut self, code: &CowStr<'_>, range: &Range<usize>) {
        let outer_text = &self.text[range.clone()];
        if let Some(inner_range) = find_substring(outer_text, code, false, false)
            .or_else(|| find_substring(outer_text, code, true, false))
        {
            let absolute_inner = (range.start + inner_range.start)..(range.start + inner_range.end);

            if inner_range.start > 0 {
                self.push_highlight(
                    Some(self.ms.inline_code_outer),
                    &(range.start..absolute_inner.start),
                );
            }

            self.push_highlight(Some(self.ms.inline_code_inner), &absolute_inner);

            if range.end > absolute_inner.end {
                self.push_highlight(
                    Some(self.ms.inline_code_outer),
                    &(absolute_inner.end..range.end),
                );
            }
        } else {
            self.push_highlight(Some(self.ms.inline_code_inner), range);
        }
    }

    fn expand_tabs(&mut self, range: &Range<usize>) {
        let Some(slice) = self.text.get(range.clone()) else {
            return;
        };
        if !slice.contains('\t') {
            return;
        }
        for (i, b) in slice.bytes().enumerate() {
            if b != b'\t' {
                continue;
            }
            let abs = range.start + i;
            let line_start = self.text[..abs].rfind('\n').map_or(0, |p| p + 1);
            let col = unicode_display_width(&self.text[line_start..abs]);
            self.buffers.transforms.push(Transform {
                range: abs..abs + 1,
                to: " ".repeat(8 - col % 8),
                force: false,
            });
        }
    }

    fn scan_inline_html_entities(&mut self, range: &Range<usize>) {
        let Some(slice) = self.text.get(range.clone()) else {
            debug_assert!(false, "pulldown-cmark text range out of bounds");
            return;
        };
        if !slice.contains('&') {
            return;
        }

        const MAX_ENTITY_LEN: usize = 33;
        let bytes = slice.as_bytes();
        let mut i = 0;
        while let Some(&b) = bytes.get(i) {
            if b != b'&' {
                i += 1;
                continue;
            }

            let max = (i + MAX_ENTITY_LEN).min(bytes.len());
            let mut j = i + 1;
            let end = loop {
                if j >= max {
                    break None;
                }
                match bytes.get(j) {
                    Some(b';') => break Some(j),
                    Some(b'#' | b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9') => j += 1,
                    _ => break None,
                }
            };
            if let Some(end) = end
                && let Some(entity) = slice.get(i..=end)
                && let Some(decoded) = decode_html_entity(entity)
            {
                let abs = (range.start + i)..(range.start + end + 1);

                let overlaps = self
                    .buffers
                    .transforms
                    .iter()
                    .any(|t| t.range.start < abs.end && abs.start < t.range.end);
                if !overlaps {
                    self.push_highlight(None, &abs);
                    self.buffers.transforms.push(Transform {
                        range: abs,
                        to: decoded,
                        force: false,
                    });
                }
                i = end + 1;
                continue;
            }
            i += 1;
        }
    }

    fn format_table(&self, state: &TableState) -> FormattedTable {
        let borders = TableBorders::BOX;
        let padding = 1;

        let border_style: ratatui::style::Style = anstyle_to_ratatui_style(self.ms.rule).dim();

        let all_rows: Vec<&Vec<StyledCell>> = std::iter::once(&state.header)
            .chain(state.rows.iter())
            .filter(|r| !r.is_empty())
            .collect();

        if all_rows.is_empty() {
            return FormattedTable::default();
        }

        let num_cols = all_rows.iter().map(|r| r.len()).max().unwrap_or(0);
        if num_cols == 0 {
            return FormattedTable::default();
        }

        let mut col_widths: Vec<usize> = vec![0; num_cols];
        for row in &all_rows {
            for (col_idx, cell) in row.iter().enumerate() {
                let text = cell.plain_text();
                let cell_width = text
                    .split('\n')
                    .map(unicode_display_width)
                    .max()
                    .unwrap_or(0);
                if col_idx < col_widths.len() {
                    col_widths[col_idx] = col_widths[col_idx].max(cell_width);
                }
            }
        }

        if let Some(max_width) = self.max_table_width {
            let overhead = num_cols * (2 * padding + 1) + 1;
            let content_budget = max_width.saturating_sub(overhead);
            let total_content: usize = col_widths.iter().sum();

            if total_content > content_budget && total_content > 0 {
                let mut min_col_widths: Vec<usize> = vec![1; num_cols];

                let mut hard_floors: Vec<usize> = vec![0; num_cols];
                for row in &all_rows {
                    for (col, cell) in row.iter().enumerate() {
                        if col >= num_cols {
                            break;
                        }
                        let text = cell.plain_text();
                        for word in cell_word_separator(&text) {
                            let w = unicode_display_width(word.word);
                            min_col_widths[col] = min_col_widths[col].max(w);
                        }
                        if !text.is_empty()
                            && let Some(floor) = hard_floors.get_mut(col)
                        {
                            let widest_grapheme = text
                                .graphemes(true)
                                .map(unicode_display_width)
                                .max()
                                .unwrap_or(0);
                            *floor = (*floor).max(widest_grapheme.max(1));
                        }
                    }
                }

                let min_total: usize = min_col_widths.iter().sum();
                let hard_total: usize = hard_floors.iter().sum();

                let (base_widths, target_widths) =
                    if min_total > content_budget && hard_total <= content_budget {
                        (hard_floors, min_col_widths)
                    } else {
                        (min_col_widths, col_widths.clone())
                    };
                let base_total: usize = base_widths.iter().sum();
                let extra_budget = content_budget.saturating_sub(base_total);

                let extra_wants: Vec<usize> = target_widths
                    .iter()
                    .zip(&base_widths)
                    .map(|(&target, &base)| target.saturating_sub(base))
                    .collect();
                let total_extra_want: usize = extra_wants.iter().sum();

                let mut new_widths = base_widths.clone();
                if total_extra_want > 0 && extra_budget > 0 {
                    for (width, &want) in new_widths.iter_mut().zip(&extra_wants) {
                        let share = (want as f64 * extra_budget as f64 / total_extra_want as f64)
                            .floor() as usize;
                        *width += share;
                    }

                    let used: usize = new_widths.iter().sum();
                    let mut remaining = content_budget.saturating_sub(used);
                    if remaining > 0 {
                        let mut indices: Vec<usize> = (0..num_cols).collect();

                        let unmet = |i: usize| {
                            target_widths
                                .get(i)
                                .copied()
                                .unwrap_or(0)
                                .saturating_sub(new_widths.get(i).copied().unwrap_or(0))
                        };
                        indices.sort_by_key(|&b| std::cmp::Reverse(unmet(b)));
                        for &idx in &indices {
                            if remaining == 0 {
                                break;
                            }

                            let target = target_widths.get(idx).copied().unwrap_or(0);
                            if let Some(width) = new_widths.get_mut(idx)
                                && *width < target
                            {
                                *width += 1;
                                remaining -= 1;
                            }
                        }
                    }
                }

                col_widths = new_widths;
            }
        }

        let alignments: Vec<_> = (0..num_cols)
            .map(|i| {
                state
                    .alignments
                    .get(i)
                    .copied()
                    .unwrap_or(pulldown_cmark::Alignment::None)
            })
            .collect();

        if crate::table_records::should_render_records(&state.header, &state.rows, &col_widths) {
            let label_style: ratatui::style::Style = anstyle_to_ratatui_style(self.ms.rule).bold();
            let rec = crate::table_records::render_records(
                &state.header,
                &state.rows,
                self.max_table_width,
                label_style,
                border_style,
            );
            return FormattedTable {
                lines: rec.lines,
                styled_lines: rec.styled_lines,
                line_source_offsets: rec.line_source_offsets,
                hyperlinks: Vec::new(),
            };
        }

        let mut lines = Vec::new();
        let mut styled_lines = Vec::new();
        let mut line_source_offsets: Vec<usize> = Vec::new();
        let mut hyperlinks: Vec<TableHyperlink> = Vec::new();

        let header_offset = 0usize;
        let separator_offset = 1usize;

        let top_border = self.format_border_line(
            &col_widths,
            padding,
            borders.c_tl(),
            borders.t_t(),
            borders.c_tr(),
            borders.h(),
        );
        styled_lines.push(Line::styled(top_border.clone(), border_style));
        lines.push(top_border);
        line_source_offsets.push(header_offset);

        if !state.header.is_empty() {
            let (row_plains, row_styleds, row_links) = self.format_styled_content_lines(
                &state.header,
                &col_widths,
                &alignments,
                padding,
                borders.v(),
                border_style,
                true,
            );
            let base_line = styled_lines.len();
            for (p, s) in row_plains.into_iter().zip(row_styleds) {
                lines.push(p);
                styled_lines.push(s);
                line_source_offsets.push(header_offset);
            }
            for mut link in row_links {
                link.line_offset += base_line;
                hyperlinks.push(link);
            }

            let sep = self.format_border_line(
                &col_widths,
                padding,
                borders.t_l(),
                borders.x(),
                borders.t_r(),
                borders.h(),
            );
            styled_lines.push(Line::styled(sep.clone(), border_style));
            lines.push(sep);
            line_source_offsets.push(separator_offset);
        }

        for (i, row) in state.rows.iter().enumerate() {
            let row_offset = separator_offset + 1 + i;

            let (row_plains, row_styleds, row_links) = self.format_styled_content_lines(
                row,
                &col_widths,
                &alignments,
                padding,
                borders.v(),
                border_style,
                false,
            );
            let base_line = styled_lines.len();
            for (p, s) in row_plains.into_iter().zip(row_styleds) {
                lines.push(p);
                styled_lines.push(s);
                line_source_offsets.push(row_offset);
            }
            for mut link in row_links {
                link.line_offset += base_line;
                hyperlinks.push(link);
            }

            if i < state.rows.len().saturating_sub(1) {
                let row_sep = self.format_border_line(
                    &col_widths,
                    padding,
                    borders.t_l(),
                    borders.x(),
                    borders.t_r(),
                    borders.h(),
                );
                styled_lines.push(Line::styled(row_sep.clone(), border_style));
                lines.push(row_sep);
                line_source_offsets.push(row_offset);
            }
        }

        let last_row_offset = separator_offset + state.rows.len();
        let bottom_border = self.format_border_line(
            &col_widths,
            padding,
            borders.c_bl(),
            borders.t_b(),
            borders.c_br(),
            borders.h(),
        );
        styled_lines.push(Line::styled(bottom_border.clone(), border_style));
        lines.push(bottom_border);
        line_source_offsets.push(last_row_offset);

        FormattedTable {
            lines,
            styled_lines,
            line_source_offsets,
            hyperlinks,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn format_styled_content_lines(
        &self,
        cells: &[StyledCell],
        col_widths: &[usize],
        alignments: &[pulldown_cmark::Alignment],
        padding: usize,
        v: char,
        border_style: ratatui::style::Style,
        is_header: bool,
    ) -> (Vec<String>, Vec<Line<'static>>, Vec<TableHyperlink>) {
        let wrapped_cells: Vec<Vec<String>> = (0..col_widths.len())
            .map(|i| {
                let text = cells.get(i).map(|c| c.plain_text()).unwrap_or_default();
                wrap_cell_text(&text, col_widths[i])
            })
            .collect();

        let num_visual_lines = wrapped_cells.iter().map(|c| c.len()).max().unwrap_or(1);

        let mut all_plains = Vec::with_capacity(num_visual_lines);
        let mut all_styled = Vec::with_capacity(num_visual_lines);
        let mut all_links: Vec<TableHyperlink> = Vec::new();

        let mut source_cursors: Vec<usize> = vec![0; col_widths.len()];

        for vis_line in 0..num_visual_lines {
            let mut plain = String::new();
            let mut spans: Vec<Span<'static>> = Vec::new();

            let mut display_col: usize = 0;

            plain.push(v);
            spans.push(Span::styled(v.to_string(), border_style));
            display_col += unicode_display_width(&v.to_string());

            for (i, width) in col_widths.iter().enumerate() {
                let cell_line_text = wrapped_cells[i]
                    .get(vis_line)
                    .map(|s| s.as_str())
                    .unwrap_or("");
                let cell_line_width = unicode_display_width(cell_line_text);
                let total_padding = width.saturating_sub(cell_line_width);

                let alignment = alignments
                    .get(i)
                    .copied()
                    .unwrap_or(pulldown_cmark::Alignment::None);
                let (left_pad, right_pad) = match alignment {
                    pulldown_cmark::Alignment::Left | pulldown_cmark::Alignment::None => {
                        (0, total_padding)
                    }
                    pulldown_cmark::Alignment::Right => (total_padding, 0),
                    pulldown_cmark::Alignment::Center => {
                        let left = total_padding / 2;
                        (left, total_padding - left)
                    }
                };

                let left_space = " ".repeat(padding + left_pad);
                let left_space_width = unicode_display_width(&left_space);
                plain.push_str(&left_space);
                spans.push(Span::raw(left_space));
                display_col += left_space_width;

                if !cell_line_text.is_empty() {
                    if let Some(cell) = cells.get(i) {
                        let full_text = cell.plain_text();

                        let cursor = full_text
                            .floor_char_boundary(source_cursors.get(i).copied().unwrap_or(0));
                        let line_start = full_text
                            .get(cursor..)
                            .and_then(|rest| rest.find(cell_line_text))
                            .map(|off| cursor + off)
                            .unwrap_or(cursor);
                        let line_end = (line_start + cell_line_text.len()).min(full_text.len());
                        if let Some(next_cursor) = source_cursors.get_mut(i) {
                            *next_cursor = line_end;
                        }

                        let mut offset = 0usize;
                        for cell_span in &cell.spans {
                            let span_start = offset;
                            let span_end = offset + cell_span.text.len();
                            offset = span_end;

                            let start = span_start.max(line_start);
                            let end = span_end.min(line_end);
                            if start >= end {
                                continue;
                            }

                            let Some(slice) = full_text.get(start..end) else {
                                continue;
                            };
                            if slice.is_empty() {
                                continue;
                            }

                            let mut style: ratatui::style::Style =
                                anstyle_to_ratatui_style(self.ms.text);
                            if is_header || cell_span.bold {
                                style = style.bold();
                            }
                            if cell_span.italic {
                                style = style.italic();
                            }
                            if cell_span.code {
                                style = anstyle_to_ratatui_style(self.ms.inline_code_inner);
                            }
                            if let Some((url, id)) = &cell_span.link {
                                let link_style: ratatui::style::Style =
                                    anstyle_to_ratatui_style(self.ms.link_text);
                                style = style.patch(link_style);

                                let slice_width = unicode_display_width(slice);
                                all_links.push(TableHyperlink {
                                    line_offset: vis_line,
                                    column_range: display_col..(display_col + slice_width),
                                    url: url.clone(),
                                    id: *id,
                                });
                            }
                            let slice_width = unicode_display_width(slice);
                            plain.push_str(slice);
                            spans.push(Span::styled(slice.to_string(), style));
                            display_col += slice_width;
                        }
                    } else {
                        plain.push_str(cell_line_text);
                        spans.push(Span::raw(cell_line_text.to_string()));
                        display_col += cell_line_width;
                    }
                }

                let right_space = " ".repeat(right_pad + padding);
                let right_space_width = unicode_display_width(&right_space);
                plain.push_str(&right_space);
                spans.push(Span::raw(right_space));
                display_col += right_space_width;

                plain.push(v);
                spans.push(Span::styled(v.to_string(), border_style));
                display_col += unicode_display_width(&v.to_string());
            }

            all_plains.push(plain);
            all_styled.push(Line::from(spans));
        }

        (all_plains, all_styled, all_links)
    }

    fn format_border_line(
        &self,
        col_widths: &[usize],
        padding: usize,
        left: char,
        mid: char,
        right: char,
        h: char,
    ) -> String {
        let mut line = String::new();
        line.push(left);
        for (i, &width) in col_widths.iter().enumerate() {
            let total_width = width + padding * 2;
            for _ in 0..total_width {
                line.push(h);
            }
            if i < col_widths.len() - 1 {
                line.push(mid);
            }
        }
        line.push(right);
        line
    }
}

pub(crate) fn wrap_cell_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let opts = textwrap::Options::new(width)
        .wrap_algorithm(textwrap::WrapAlgorithm::FirstFit)
        .word_separator(textwrap::WordSeparator::Custom(cell_word_separator))
        .break_words(false);
    let wrapped = textwrap::wrap(text, opts);
    let mut lines: Vec<String> = Vec::with_capacity(wrapped.len());
    for cow in wrapped {
        let line = cow.into_owned();
        if unicode_display_width(&line) <= width {
            lines.push(line);
            continue;
        }

        let mut piece = String::new();
        let mut piece_width = 0usize;
        for grapheme in line.graphemes(true) {
            let grapheme_width = unicode_display_width(grapheme);
            if piece_width > 0 && piece_width.saturating_add(grapheme_width) > width {
                lines.push(std::mem::take(&mut piece));
                piece_width = 0;
            }
            piece.push_str(grapheme);
            piece_width = piece_width.saturating_add(grapheme_width);
        }
        if !piece.is_empty() {
            lines.push(piece);
        }
    }
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

pub struct ParsedMarkdown<'a, 'b> {
    pub(crate) text: &'a str,
    pub(crate) ms: MarkdownStyle,
    pub(crate) buffers: &'b mut MarkdownBuffers,
    pub(crate) last_checkpoint: Option<(CheckpointKind, usize)>,
    pub(crate) next_link_id: u32,
}

impl<'a, 'b> ParsedMarkdown<'a, 'b> {
    pub fn new(
        text: &'a str,
        ms: MarkdownStyle,
        buffers: &'b mut MarkdownBuffers,
        last_checkpoint: Option<(CheckpointKind, usize)>,
        next_link_id: u32,
    ) -> Self {
        Self {
            text,
            ms,
            buffers,
            last_checkpoint,
            next_link_id,
        }
    }
}
