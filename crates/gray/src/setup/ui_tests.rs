use super::*;

fn buffer_rows(backend: &ratatui::backend::TestBackend, w: u16, h: u16) -> Vec<String> {
    let buf = backend.buffer();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect())
        .collect()
}

#[test]
fn backdrop_mirrors_live_layout_with_multiline_prompt() {
    // Live TUI: the composer input box and footer start below any transcript,
    // followed by empty filler space at the bottom of the screen.
    let backend = ratatui::backend::TestBackend::new(40, 10);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    let bg = BackgroundSnapshot {
        transcript: Vec::new(),
        history_entries: Vec::new(),
        prompt_text: "abcdefghijklmnopqrstuvwxyz0123456789!@#$ ".repeat(3),
        ..Default::default()
    };
    terminal
        .draw(|frame| render_dimmed_background(frame, &bg))
        .expect("draw");
    let rows = buffer_rows(terminal.backend(), 40, 10);
    // 4 wrapped prompt rows + top/bottom blank = 6 box rows, 1 footer row = 7 rows total.
    // In a 10-row viewport with 0 transcript rows, the box starts at row 0,
    // footer is at row 6, and rows 7..10 are trailing filler.
    assert!(rows[1].contains("❯"), "prompt box starts at top: {rows:?}");
    assert!(
        rows[6].contains("cache"),
        "footer follows the box: {rows:?}"
    );
    assert!(
        rows[7..].iter().all(|r| r.trim().is_empty()),
        "trailing filler after footer: {rows:?}"
    );
    let box_bg = terminal.backend().buffer()[(0, 1)].bg;
    assert_eq!(
        box_bg,
        // NOTE: updated with the input-box dim fix; UNRUN (cargo test
        // banned under X) — verify in TTY/CI. dim_color((22,22,22)).
        ratatui::style::Color::Rgb(8, 8, 8),
        "backdrop input box is dimmed composer gray"
    );
}

#[test]
fn backdrop_dims_card_box_and_inserts_gap_before_input() {
    let backend = ratatui::backend::TestBackend::new(40, 15);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    let bg = BackgroundSnapshot {
        history_entries: vec![crate::composer::TranscriptEntry::UserPrompt(
            "/thinking".to_string(),
            Vec::new(),
        )],
        ..Default::default()
    };
    terminal
        .draw(|frame| render_dimmed_background(frame, &bg))
        .expect("draw");
    let rows = buffer_rows(terminal.backend(), 40, 15);
    // Prompt card: 3 rows (margin, ' ❯ /thinking', margin)
    assert!(
        rows[1].contains("/thinking"),
        "card contains command: {rows:?}"
    );
    // Card background dims with everything else (no preservation:
    // a full-gray card glowed through behind the modal).
    let card_bg = terminal.backend().buffer()[(0, 1)].bg;
    assert_eq!(
        card_bg,
        ratatui::style::Color::Rgb(8, 8, 8),
        "card dims to dim_color((22,22,22)) like the input box"
    );
    // Row 3 is the gap row between card and input box
    assert!(
        rows[3].trim().is_empty(),
        "gap row between sent text and input box: {rows:?}"
    );
    // Row 4 is top margin of input box, row 5 is input prompt arrow
    assert!(
        rows[5].contains("❯"),
        "input box arrow follows gap row: {rows:?}"
    );
}
