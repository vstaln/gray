use super::{line_end, line_start};

#[test]
fn line_bounds_single_line() {
    assert_eq!(line_start("hello", 3), 0);
    assert_eq!(line_end("hello", 3), 5);
}

#[test]
fn line_bounds_multiline_are_per_line() {
    let t = "one\ntwo\nthree";
    // cursor inside "two"
    assert_eq!(line_start(t, 5), 4);
    assert_eq!(line_end(t, 5), 7);
    // cursor at start of "three"
    assert_eq!(line_start(t, 8), 8);
    assert_eq!(line_end(t, 8), t.len());
    // cursor right after a newline's predecessor
    assert_eq!(line_end(t, 3), 3);
}

#[test]
fn line_bounds_clamp_out_of_range_cursor() {
    assert_eq!(line_start("ab", 99), 0);
    assert_eq!(line_end("ab", 99), 2);
}
