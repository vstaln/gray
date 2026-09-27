use super::{MascotGrid, decode_grid, mascot_lines};
use ratatui::backend::TestBackend;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use crate::composer::build_welcome_lines;
use crate::text_width::display_width;

/// Luminance ramp used only by the preview test to eyeball the layout.
fn luminance_art(lines: &[Line<'static>]) -> String {
    let mut out = String::new();
    for line in lines {
        for span in &line.spans {
            let mut cell = String::new();
            for ch in span.content.chars() {
                if ch == ' ' {
                    cell.push(' ');
                    continue;
                }
                let style = span.style;
                let lum = |c: Option<ratatui::style::Color>| match c {
                    Some(ratatui::style::Color::Rgb(r, g, b)) => {
                        (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0
                    }
                    _ => 1.0,
                };
                let top = lum(style.fg);
                let bot = lum(style.bg);
                let v = top.min(bot);
                cell.push(match v {
                    x if x < 0.15 => '#',
                    x if x < 0.45 => '+',
                    x if x < 0.75 => '.',
                    _ => ' ',
                });
            }
            out.push_str(&cell);
        }
        out.push('\n');
    }
    out
}

#[test]
fn grid_fits_terminal_and_keeps_aspect() {
    let grid: MascotGrid = decode_grid(120, 40).expect("asset decodes");
    assert!(grid.cols <= 120, "must not exceed terminal width");
    // 9-row headroom cap: at most 31 cell rows (62 pixel rows) on 40 rows.
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
    // Height caps first on a short terminal: 15 cell rows -> ~34 columns.
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
    let grid = decode_grid(120, 40).expect("asset decodes");
    let lines = super::mascot_lines(120, 40, Some(120)).expect("truecolor path");
    assert_eq!(lines.len(), grid.rows, "one line per cell row");
    for line in &lines {
        let width: usize = line.spans.iter().map(|s| display_width(&s.content)).sum();
        assert!(width <= 120, "line wider than the terminal: {width}");
    }
}

/// Flattened text of a line block, one string per line.
fn plain(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
        .collect()
}

#[test]
fn welcome_is_the_ascii_logo_by_default() {
    // graychan is `/hehe` only: the startup screen is the plain logo.
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
    let banner = &welcome[welcome.len() - 2];
    assert!(
        banner.contains("gray") && banner.contains("/help"),
        "version banner must survive under the logo: {banner:?}"
    );
}

#[test]
fn mascot_art_paints_as_half_block_cells() {
    // Same size probe build_welcome_lines used to use (no TTY under cargo
    // test, so crossterm fails and the fallback wins on both sides).
    let (cols, rows) = crossterm::terminal::size().unwrap_or((120, 24));
    let grid = decode_grid(cols, rows).expect("asset decodes");
    let art = mascot_lines(cols, rows, Some(120)).expect("truecolor path");
    // Painted through a TestBackend the block is real cells, not escapes.
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
fn preview_layout_for_humans() {
    // Not an assertion: `cargo test -p gray preview_layout_for_humans -- --nocapture`
    // prints an ASCII luminance approximation of what a 120x40 terminal shows.
    let lines = build_welcome_lines(120);
    eprint!("{}", luminance_art(&lines));
}
