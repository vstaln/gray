use super::*;

#[test]
fn sweep_due_covers_age_and_size() {
    assert!(sweep_due(0, Some(LOG_SWEEP_AGE + Duration::from_secs(1))));
    assert!(!sweep_due(0, Some(LOG_SWEEP_AGE)));
    assert!(!sweep_due(0, None));
    assert!(sweep_due(SHELL_LOG_MAX_BYTES + 1, None));
    assert!(!sweep_due(SHELL_LOG_MAX_BYTES, None));
}
