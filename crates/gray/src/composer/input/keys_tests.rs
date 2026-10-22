use super::{line_end, line_start};

#[test]
fn line_bounds_single_line() {
    assert_eq!(line_start("hello", 3), 0);
    assert_eq!(line_end("hello", 3), 5);
}

#[test]
fn line_bounds_multiline_are_per_line() {
    let t = "one\ntwo\nthree";

    assert_eq!(line_start(t, 5), 4);
    assert_eq!(line_end(t, 5), 7);

    assert_eq!(line_start(t, 8), 8);
    assert_eq!(line_end(t, 8), t.len());

    assert_eq!(line_end(t, 3), 3);
}

#[test]
fn line_bounds_clamp_out_of_range_cursor() {
    assert_eq!(line_start("ab", 99), 0);
    assert_eq!(line_end("ab", 99), 2);
}

#[test]
fn up_walks_past_a_multiline_history_entry_and_down_returns_to_the_draft() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
    let mut tui = crate::composer::Tui::test_tui(120, 30);
    tui.history = vec!["older".into(), "first\n\nsecond\n\nthird".into()];
    super::dispatch(&mut tui, &key(KeyCode::Up));
    assert_eq!(tui.textarea.text(), "first\n\nsecond\n\nthird");
    super::dispatch(&mut tui, &key(KeyCode::Up));
    assert_eq!(tui.textarea.text(), "older");
    super::dispatch(&mut tui, &key(KeyCode::Down));
    super::dispatch(&mut tui, &key(KeyCode::Down));
    assert_eq!(tui.textarea.text(), "");
    assert_eq!(tui.history_idx, None);
}
