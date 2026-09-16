use crate::style::test_style;
use crate::{StreamingMarkdownRenderer, render_markdown_ratatui_full};

fn lines_text(lines: &[ratatui::text::Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

#[test]
fn repro_soft_break_space_across_chunks() {
    let full = "Analyzing possible latency causes like API delay, cold start, network, and system load for a concise explanation.\nSeparating local execution from external API latency and noting the absence of internal timing data.\n\n";
    let mut r = StreamingMarkdownRenderer::new(test_style::STYLE, true);
    let chars: Vec<char> = full.chars().collect();
    for w in chars.chunks(7) {
        let s: String = w.iter().collect();
        r.push_and_render(&s, None);
    }
    let view = r.finish(None);
    let flat: String = lines_text(&view.lines).join("");
    assert!(
        flat.contains("explanation. Separating"),
        "soft break must collapse to a space, got: {flat:?}"
    );
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

    assert_eq!(lines_text(&view.lines), lines_text(&expected.lines));

    // Parser-produced link-text hyperlink must cover exactly "click" (4 cells)
    let line0: String = view.lines[0]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    let hit = view
        .hyperlinks
        .iter()
        .find(|h| {
            h.url == "https://example.com" && {
                let slice: String = line0
                    .chars()
                    .skip(h.column_range.start)
                    .take(h.column_range.len())
                    .collect();
                slice == "click"
            }
        })
        .expect("hyperlink over link text should survive torn chunk");
    assert_eq!(hit.column_range.len(), 5);
}
