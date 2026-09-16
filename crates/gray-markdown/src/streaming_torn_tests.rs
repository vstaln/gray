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

    assert_eq!(lines_text(&view.lines), lines_text(&expected.lines));
    // latex passthrough: `\alpha + \beta` -> `α + β`, delimiters hidden
    let joined = lines_text(&view.lines).join("\n");
    assert!(joined.contains("α + β"), "got: {joined:?}");
    assert!(
        !joined.contains("\\("),
        "delimiters must be hidden: {joined:?}"
    );
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

// UNRUN: marked #[ignore] — run explicitly, never in bulk.
// (`cargo test` under X kills the session; see repo AGENTS.md.)
// Run: `cargo test -p gray-markdown tail_hyperlink -- --ignored`
//
// Regression test for tail-scoped hyperlink handling in `rerender_tail`:
// the global sort invariant must hold after every push and hyperlinks on
// frozen lines must only grow (never be reordered/restyled). Link ids are
// intentionally NOT compared: streaming interleaves parser/url_scan ids
// per tail while a one-shot render numbers all parser links first.
#[test]
#[ignore]
fn unrun_tail_hyperlinks_sorted_and_frozen_stable() {
    let mut full = String::new();
    for i in 0..50 {
        full.push_str(&format!("See [link{i}](https://example.com/{i}) here.\n\n"));
    }
    let mut r = StreamingMarkdownRenderer::new(test_style::STYLE, true);
    let mut prev_frozen: Vec<(usize, std::ops::Range<usize>, String)> = Vec::new();
    let bytes = full.as_bytes();
    for chunk in bytes.chunks(7) {
        // ASCII-only doc, so byte chunks are always char boundaries.
        r.push_and_render(std::str::from_utf8(chunk).unwrap(), None);
        let view = r.view();
        let keys: Vec<(usize, usize)> = view
            .hyperlinks
            .iter()
            .map(|h| (h.line_index, h.column_range.start))
            .collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(keys, sorted, "hyperlinks must stay globally sorted");
        let frozen = r.frozen_lines_len();
        let cur: Vec<(usize, std::ops::Range<usize>, String)> = view
            .hyperlinks
            .iter()
            .filter(|h| h.line_index < frozen)
            .map(|h| (h.line_index, h.column_range.clone(), h.url.clone()))
            .collect();
        assert!(
            cur.starts_with(&prev_frozen),
            "frozen hyperlinks must only grow: {cur:?} prev {prev_frozen:?}"
        );
        prev_frozen = cur;
    }
    let view = r.finish(None);
    let (expected, _) = render_markdown_ratatui_full(&full, test_style::STYLE, true, None);
    assert_eq!(lines_text(&view.lines), lines_text(&expected.lines));
    let geom = |hs: &[crate::HyperlinkTarget]| {
        hs.iter()
            .map(|h| (h.line_index, h.column_range.clone(), h.url.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(geom(&view.hyperlinks), geom(&expected.hyperlinks));
}

// UNRUN: marked #[ignore] — run explicitly, never in bulk.
// (`cargo test` under X kills the session; see repo AGENTS.md.)
// Run: `cargo test -p gray-markdown clone_ -- --ignored`
//
// Wiring (1): `Clone` must preserve highlight state — the clone replays
// its render with `get_syntect()` when the original was highlighted.
#[test]
#[ignore]
fn unrun_clone_preserves_highlight_state() {
    let src = "Intro.\n\n```rust\nfn main() {}\n```\n\n";
    let mut r = StreamingMarkdownRenderer::new(test_style::STYLE, true);
    r.push_and_render(src, Some(crate::get_syntect()));
    let c = r.clone();
    // Exact equality: text AND styles (ratatui `Line: PartialEq`).
    assert_eq!(c.view().lines, r.view().lines);
    assert_eq!(c.frozen_lines_len(), r.frozen_lines_len());
    // And the clone actually carries highlight colors (not a `None` render).
    let mut plain = StreamingMarkdownRenderer::new(test_style::STYLE, true);
    plain.push_and_render(src, None);
    assert_ne!(c.view().lines, plain.view().lines);
    assert_eq!(lines_text(&c.view().lines), lines_text(&r.view().lines));
}

// UNRUN: marked #[ignore] — run explicitly, never in bulk.
// (`cargo test` under X kills the session; see repo AGENTS.md.)
// Run: `cargo test -p gray-markdown clone_ -- --ignored`
#[test]
#[ignore]
fn unrun_clone_without_highlight_stays_plain() {
    let src = "Intro.\n\n```rust\nfn main() {}\n```\n\n";
    let mut r = StreamingMarkdownRenderer::new(test_style::STYLE, true);
    r.push_and_render(src, None);
    let c = r.clone();
    assert_eq!(c.view().lines, r.view().lines);
}
