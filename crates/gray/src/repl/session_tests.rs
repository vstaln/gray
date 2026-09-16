// UNRUN (cargo test banned under X): run in TTY/CI.
// The lazy-build gate: a fresh session with a model mints before the
// first build (real sid from turn one); no model (or existing session)
// never mints, so the unconfigured REPL still opens session-free.
use super::should_ensure_session_before_build;

#[test]
fn first_build_ensures_session_only_when_model_configured() {
    assert!(should_ensure_session_before_build(
        false,
        Some("openai/gpt-4o")
    ));
    assert!(!should_ensure_session_before_build(false, None));
    assert!(!should_ensure_session_before_build(false, Some("")));
    assert!(!should_ensure_session_before_build(
        true,
        Some("openai/gpt-4o")
    ));
    assert!(!should_ensure_session_before_build(true, None));
}
