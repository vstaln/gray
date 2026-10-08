use super::*;

#[test]
fn admit_then_pending_round_trips_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let a = Event::new(Kind::User, MAIN, "first", None);
    let b = Event::new(Kind::Heartbeat, MAIN, "second", None);
    admit(dir.path(), &a).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    admit(dir.path(), &b).unwrap();
    let got: Vec<Event> = pending(dir.path()).into_iter().map(|(_, e)| e).collect();
    assert_eq!(got, vec![a, b]);
}

#[test]
fn a_corrupt_event_is_moved_aside_not_retried() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("events")).unwrap();
    std::fs::write(dir.path().join("events").join("0-x.json"), "{bad").unwrap();
    assert!(pending(dir.path()).is_empty());
    assert!(dir.path().join("events/bad/0-x.json").exists());
    assert!(pending(dir.path()).is_empty());
}

#[test]
fn cap_respects_char_boundaries() {
    let s = "é".repeat(10);
    let c = cap(&s, 5);
    assert!(c.starts_with("éé"));
    assert!(c.ends_with("[… truncated]"));
}
