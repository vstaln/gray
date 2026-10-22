use super::{MascotGrid, ascii_lines_unchecked, decode_grid, mascot_lines_unchecked};
use ratatui::backend::TestBackend;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use crate::composer::build_welcome_lines;
use crate::text_width::display_width;

#[test]
fn grid_fits_terminal_and_keeps_aspect() {
    let grid: MascotGrid = decode_grid(120, 40).expect("asset decodes");
    assert!(grid.cols <= 120, "must not exceed terminal width");

    assert!(grid.rows <= 40 - 9 + 1, "must leave the banner visible");
    let aspect = grid.cols as f32 / (grid.rows * 2) as f32;
    assert!(
        (aspect - 1024.0 / 902.0).abs() < 0.02,
        "aspect {aspect} drifted from the asset"
    );
}

#[test]
fn grid_is_width_bound_on_a_wide_short_terminal() {
    let grid = decode_grid(200, 24).expect("asset decodes");

    assert!(grid.rows * 2 <= 2 * (24 - 9 + 1));
    assert!(grid.cols < 200);
}

#[test]
fn tiny_terminals_decline_the_mascot() {
    assert!(decode_grid(10, 8).is_none());
    assert!(decode_grid(0, 0).is_none());
}

#[test]
fn lines_tile_the_grid_with_merged_runs() {
    unsafe { std::env::remove_var("NO_COLOR") };
    let grid = decode_grid(120, 40).expect("asset decodes");
    let lines = super::mascot_lines_unchecked(120, 40, Some(120)).expect("truecolor path");
    assert_eq!(lines.len(), grid.rows, "one line per cell row");
    for line in &lines {
        let width: usize = line.spans.iter().map(|s| display_width(&s.content)).sum();
        assert!(width <= 120, "line wider than the terminal: {width}");
    }
}

fn plain(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
        .collect()
}

#[test]
fn welcome_is_the_ascii_logo_by_default() {
    let welcome = plain(&build_welcome_lines(120));
    assert!(
        !welcome.iter().any(|l| l.contains('\u{2580}')),
        "half-block art must not be in the default welcome: {welcome:?}"
    );
    let logo = crate::tui::logo_lines();
    assert!(!logo.is_empty(), "logo asset is empty");
    for row in &logo {
        let want = row.trim();
        assert!(
            welcome.iter().any(|l| l.contains(want)),
            "logo row missing from the welcome: {want:?}"
        );
    }

    assert!(
        welcome
            .iter()
            .any(|l| l.trim() == format!("gray \u{25c6} {}", env!("CARGO_PKG_VERSION"))),
        "title line missing from the welcome: {welcome:?}"
    );
    let version = env!("CARGO_PKG_VERSION").to_string();
    assert_eq!(
        welcome.iter().filter(|l| l.contains(&version)).count(),
        1,
        "the version shows once: {welcome:?}"
    );
    assert!(
        !welcome
            .iter()
            .any(|l| l.contains("● Tip") || l.contains("newline")),
        "hints and tip belong under the panel: {welcome:?}"
    );
}

#[test]
fn mascot_art_paints_as_half_block_cells() {
    unsafe { std::env::remove_var("NO_COLOR") };

    let (cols, rows) = crossterm::terminal::size().unwrap_or((120, 24));
    let grid = decode_grid(cols, rows).expect("asset decodes");
    let art = mascot_lines_unchecked(cols, rows, Some(120)).expect("truecolor path");

    let backend = TestBackend::new(120, 40);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|f| Paragraph::new(art.clone()).render(f.area(), f.buffer_mut()))
        .expect("draw");
    let buf = terminal.backend().buffer().clone();
    let blocks = buf
        .content
        .iter()
        .filter(|c| c.symbol() == "\u{2580}")
        .count();
    assert_eq!(
        blocks,
        grid.cols * grid.rows,
        "every grid cell must paint as one half-block"
    );
}

#[test]
fn ascii_mascot_uses_only_plain_glyphs() {
    let lines = ascii_lines_unchecked(120, 40, Some(120)).expect("ascii path decodes");
    assert!(!lines.is_empty());
    let mut inked = 0usize;
    for line in &lines {
        let width: usize = line.spans.iter().map(|s| display_width(&s.content)).sum();
        assert!(width <= 120, "line wider than the terminal: {width}");
        for span in &line.spans {
            for c in span.content.chars() {
                assert!(
                    c.is_ascii() && (c.is_ascii_graphic() || c == ' '),
                    "non-ascii glyph in fallback art: {:?}",
                    span.content
                );
                if c.is_ascii_graphic() {
                    inked += 1;
                }
            }
        }
    }
    assert!(inked > 100, "ascii art rendered almost empty: {inked}");
}

#[test]
fn hehe_toggle_drops_the_mascot_entry() {
    use crate::composer::TranscriptEntry;
    let mut entries = vec![
        TranscriptEntry::Welcome,
        TranscriptEntry::Mascot,
        TranscriptEntry::Gap(1),
    ];
    assert!(crate::composer::transcript::drop_mascot_entry(&mut entries));
    assert!(
        !entries.iter().any(|e| matches!(e, TranscriptEntry::Mascot)),
        "the art must be gone: {entries:?}"
    );
    assert!(
        entries
            .iter()
            .any(|e| matches!(e, TranscriptEntry::Welcome)),
        "the rest of the transcript must survive: {entries:?}"
    );
    assert!(
        !crate::composer::transcript::drop_mascot_entry(&mut entries),
        "a third /hehe has nothing left to drop"
    );
}

#[test]
fn welcome_layout_is_devin_home() {
    let w = 100usize;
    let text = plain(&build_welcome_lines(w));
    let col_w = (w - 8).min(78);
    let x = (w - col_w) / 2;

    let version = env!("CARGO_PKG_VERSION").to_string();
    let label = format!("gray \u{25c6} {version}");
    let title = text.iter().position(|l| l.trim() == label).unwrap();
    let title_pad = text[title].len() - text[title].trim_start().len();
    assert_eq!(
        title_pad,
        x + (col_w - display_width(&label)) / 2,
        "title not centered"
    );

    assert_eq!(title + 2, text.len(), "{text:?}");
    assert!(text.last().unwrap().trim().is_empty());
}

#[test]
fn welcome_sweep_draws_to_the_full_mark() {
    use crate::composer::welcome_lines_at;
    let w = 120usize;
    let glyphs = |reveal: f32| -> usize {
        plain(&welcome_lines_at(w, reveal, None))
            .iter()
            .map(|l| {
                l.chars()
                    .filter(|c| !c.is_whitespace() && *c != '\u{2800}')
                    .count()
            })
            .sum()
    };

    let first = glyphs(0.4);
    let full = glyphs(1.0);
    assert!(first < full, "first {first}, full {full}");
    assert!(
        glyphs(0.8) >= first && glyphs(0.8) <= full,
        "the sweep only grows"
    );
    assert_eq!(
        plain(&welcome_lines_at(w, 1.0, None)),
        plain(&build_welcome_lines(w)),
        "the last frame IS the banner"
    );

    assert!(
        plain(&welcome_lines_at(w, 0.0, None))
            .iter()
            .any(|l| l.trim().starts_with("gray ")),
        "title must be present from the first frame"
    );
    assert!(
        plain(&welcome_lines_at(w, 1.0, None))
            .iter()
            .any(|l| l.trim().starts_with("gray ")),
        "title missing at full reveal"
    );

    assert_eq!(
        welcome_lines_at(w, 0.3, None).len(),
        welcome_lines_at(w, 1.0, None).len(),
        "frames must hold the banner's height"
    );
}
