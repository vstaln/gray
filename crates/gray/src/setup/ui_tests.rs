use super::*;

fn buffer_rows(backend: &ratatui::backend::TestBackend, w: u16, h: u16) -> Vec<String> {
    let buf = backend.buffer();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect())
        .collect()
}

#[test]
fn backdrop_mirrors_live_layout_with_multiline_prompt() {
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

    // The band's rows keep their height and dimmed fill, but composer text is
    // deliberately not painted so a stray `❯` can't peek out beside a modal.
    assert!(
        rows[1].trim().is_empty(),
        "prompt band keeps its silhouette, not its text: {rows:?}"
    );
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
            "/model".to_string(),
            Vec::new(),
        )],
        ..Default::default()
    };
    terminal
        .draw(|frame| render_dimmed_background(frame, &bg))
        .expect("draw");
    let rows = buffer_rows(terminal.backend(), 40, 15);

    assert!(
        rows[1].contains("/model"),
        "card contains command: {rows:?}"
    );

    let card_bg = terminal.backend().buffer()[(0, 1)].bg;
    assert_eq!(
        card_bg,
        ratatui::style::Color::Rgb(8, 8, 8),
        "card dims to dim_color((22,22,22)) like the input box"
    );

    assert!(
        rows[3].trim().is_empty(),
        "gap row between sent text and input box: {rows:?}"
    );

    assert!(
        rows[5].trim().is_empty(),
        "input box row keeps its silhouette, not its arrow: {rows:?}"
    );
    assert_eq!(
        terminal.backend().buffer()[(0, 5)].bg,
        ratatui::style::Color::Rgb(8, 8, 8),
        "input box row still dims to composer gray"
    );
}
