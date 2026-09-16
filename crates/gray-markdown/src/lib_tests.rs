use super::*;
use crate::buffers::unicode_display_width;

fn line_text(l: &ratatui::text::Line<'_>) -> String {
    l.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[test]
fn wide_table_fits_max_width() {
    let md = "| Check | Result |\n|---|---|\n| node --check on extracted module with a very long trailing description that keeps going | SYNTAX OK with even more trailing words to force wrapping across several visual lines |\n| persistence survived reload across sessions | {\"total\":417,\"bestCatch\":158} plus extra prose to widen the second column further |\n";
    let mut buffers = MarkdownBuffers::new();
    let (out, _) = render_markdown_ratatui_with_buffers_width(
        md,
        gray_markdown_style(),
        true,
        &mut buffers,
        None,
        Some(40),
    );
    assert!(!out.lines.is_empty());
    for l in &out.lines {
        let t = line_text(l);
        assert!(
            unicode_display_width(&t) <= 40,
            "table row overflows: {t:?}"
        );
    }
}
