mod buffers;
pub mod checkpoint;
mod colors;
mod hyperlinks;
pub mod markdown_core;
mod output;
mod parse;
mod render;
pub mod streaming;
pub mod style;
mod syntax;
mod table_records;
mod url_scan;

pub use buffers::MarkdownBuffers;
pub use checkpoint::{Checkpoint, CheckpointKind};
pub use colors::{ColorLevel, adapt_color, adapt_ratatui_color, adapt_style, detect_color_level};
pub use output::{HyperlinkTarget, MarkdownRenderOutput};
pub use parse::{MarkdownParser, ParsedMarkdown, decode_html_entities_text};
pub use streaming::StreamingMarkdownRenderer;
pub use style::{MarkdownStyle, Palette, TableBorders, ratatui_color_to_rgb, set_palette};
pub use syntax::{Syntect, get_syntect, syntect_to_ratatui_fg};
pub use syntect;

#[cfg(test)]
pub fn render_markdown_ratatui_full(
    text: &str,
    ms: MarkdownStyle,
    pretty: bool,
    syntect: Option<&Syntect>,
) -> (MarkdownRenderOutput, Option<Checkpoint>) {
    let mut buffers = MarkdownBuffers::new();
    render_markdown_ratatui_with_buffers_width(text, ms, pretty, &mut buffers, syntect, None)
}

pub fn render_markdown_ratatui_with_buffers_width(
    text: &str,
    ms: MarkdownStyle,
    pretty: bool,
    buffers: &mut MarkdownBuffers,
    syntect: Option<&Syntect>,
    max_table_width: Option<usize>,
) -> (MarkdownRenderOutput, Option<Checkpoint>) {
    let mut parsed = MarkdownParser::new(text, ms, buffers, syntect)
        .max_table_width(max_table_width)
        .parse();
    let next_link_id = parsed.next_link_id;
    let (mut output, checkpoint) = parsed.render_ratatui(pretty);

    let (extra_links, next_id) =
        url_scan::detect_plain_urls(&output.lines, &output.hyperlinks, next_link_id);
    output.hyperlinks.extend(extra_links);
    let (file_links, _post_scan_next_id) =
        url_scan::detect_file_paths(&output.lines, &output.hyperlinks, next_id);
    output.hyperlinks.extend(file_links);
    output
        .hyperlinks
        .sort_by_key(|h| (h.line_index, h.column_range.start));

    url_scan::apply_link_styling(&mut output.lines, &output.hyperlinks);
    (output, checkpoint)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_markdown_ratatui_with_link_id(
    text: &str,
    ms: MarkdownStyle,
    pretty: bool,
    buffers: &mut MarkdownBuffers,
    syntect: Option<&Syntect>,
    max_table_width: Option<usize>,
    link_id_start: u32,
) -> (MarkdownRenderOutput, Option<Checkpoint>, u32) {
    let mut parsed = MarkdownParser::new(text, ms, buffers, syntect)
        .max_table_width(max_table_width)
        .link_id_start(link_id_start)
        .parse();

    let next_link_id = parsed.next_link_id;
    let (output, checkpoint) = parsed.render_ratatui(pretty);
    (output, checkpoint, next_link_id)
}

pub fn gray_markdown_style() -> MarkdownStyle {
    use anstyle::{Color, RgbColor};
    let rgb = |r, g, b| Color::Rgb(RgbColor(r, g, b));
    let pal = |v: [u8; 3]| rgb(v[0], v[1], v[2]);
    let p = crate::style::palette();
    let peach = rgb(246, 173, 126);
    let cyan = rgb(125, 207, 255);
    let dim = rgb(140, 140, 140);
    let gray_text = rgb(225, 225, 225);
    let heading = p.map_or(peach, |p| pal(p.heading));
    let heading_minor = p.map_or(dim, |p| pal(p.heading_minor));
    let strong = p.map_or(gray_text, |p| pal(p.strong));
    let inline_code = p.map_or(cyan, |p| pal(p.inline_code));
    let quote = p.map_or(dim, |p| pal(p.quote));
    let checked = p.map_or(dim, |p| pal(p.success));
    let dim_c = p.map_or(dim, |p| pal(p.dim));
    let bullet = p.map_or(dim, |p| pal(p.bullet));
    let rule = p.map_or(dim, |p| pal(p.faint));
    let link = p.map_or(cyan, |p| pal(p.link));
    let text = p.map_or(gray_text, |p| pal(p.text));

    MarkdownStyle {
        heading_inner: [
            anstyle::Style::new().fg_color(Some(heading)).bold(),
            anstyle::Style::new().fg_color(Some(heading)).bold(),
            anstyle::Style::new().fg_color(Some(heading)).bold(),
            anstyle::Style::new().fg_color(Some(heading_minor)).italic(),
            anstyle::Style::new().fg_color(Some(heading_minor)).italic(),
            anstyle::Style::new().fg_color(Some(heading_minor)).italic(),
        ],
        heading_outer: [anstyle::Style::new().dimmed().hidden(); 6],
        strong_inner: anstyle::Style::new().bold().fg_color(Some(strong)),
        strong_outer: anstyle::Style::new().dimmed().hidden(),
        emphasis_inner: anstyle::Style::new().italic(),
        emphasis_outer: anstyle::Style::new().dimmed().hidden(),
        strikethrough_inner: anstyle::Style::new().strikethrough(),
        strikethrough_outer: anstyle::Style::new().dimmed().hidden(),
        inline_code_inner: anstyle::Style::new().fg_color(Some(inline_code)).bold(),
        inline_code_outer: anstyle::Style::new().dimmed().hidden(),
        blockquote_outer: anstyle::Style::new().fg_color(Some(quote)),
        task_checked: anstyle::Style::new().fg_color(Some(checked)),
        task_unchecked: anstyle::Style::new().fg_color(Some(dim_c)).dimmed(),
        list_item: anstyle::Style::new().fg_color(Some(bullet)),
        rule: anstyle::Style::new().fg_color(Some(rule)),
        link_outer: anstyle::Style::new().fg_color(Some(dim_c)),
        link_text: anstyle::Style::new().fg_color(Some(link)).underline(),
        link_url: anstyle::Style::new().fg_color(Some(dim_c)),
        link_title: anstyle::Style::new().fg_color(Some(dim_c)),
        code_outer: anstyle::Style::new().dimmed().hidden(),
        code_language: anstyle::Style::new().hidden(),
        code_untagged: anstyle::Style::new(),
        code_background: anstyle::Style::new(),
        table_outer: anstyle::Style::new().fg_color(Some(dim_c)).bold(),
        text: anstyle::Style::new().fg_color(Some(text)),
        math: anstyle::Style::new().italic(),
    }
    .adapt()
}

#[path = "lib_tests.rs"]
#[cfg(test)]
mod tests;
