use super::*;

#[test]
fn defaults_survive_a_partial_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.json"), r#"{"max_turns": 3}"#).unwrap();
    let s = Settings::load(dir.path());
    assert_eq!(s.max_turns, 3);
    assert_eq!(s.turn_timeout_secs, 1800);
}

#[test]
fn a_broken_file_is_defaults() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.json"), "{nope").unwrap();
    assert_eq!(Settings::load(dir.path()).max_turns, 2);
}

#[test]
fn windows_wrap_midnight() {
    let w = Window::parse("23:00-07:00").unwrap();
    assert!(w.contains(23 * 60));
    assert!(w.contains(3 * 60));
    assert!(!w.contains(7 * 60));
    assert!(!w.contains(12 * 60));
    let day = Window::parse("08:00-22:00").unwrap();
    assert!(day.contains(8 * 60));
    assert!(!day.contains(22 * 60));
    assert!(Window::parse("8-22").is_none());
    assert!(Window::parse("25:00-01:00").is_none());
}
