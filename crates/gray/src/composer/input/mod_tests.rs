use super::*;

#[test]
fn ctrl_c_first_press_clears_second_within_window_exits() {
    let now = std::time::Instant::now();
    // Draft present → never exit (first press clears).
    assert!(!ctrl_c_should_exit(true, None, now));
    assert!(!ctrl_c_should_exit(true, Some(now), now));
    // Empty, no prior press → arm, don't exit.
    assert!(!ctrl_c_should_exit(false, None, now));
    // Empty, second press inside 5 s → exit.
    let first = now - std::time::Duration::from_secs(2);
    assert!(ctrl_c_should_exit(false, Some(first), now));
    // Empty, prior press expired → don't exit.
    let stale = now - std::time::Duration::from_secs(30);
    assert!(!ctrl_c_should_exit(false, Some(stale), now));
}
