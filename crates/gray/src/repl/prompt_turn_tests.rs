// UNRUN (cargo test banned under X): run in TTY/CI.
// Guards the exact TURN_STATE idiom used above: a poisoned turn-state
// mutex must recover, never panic the REPL.
#[test]
fn turn_state_lock_survives_poison() {
    let m = std::sync::Mutex::new(Some(tokio_util::sync::CancellationToken::new()));
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = m.lock().unwrap();
        panic!("poison the mutex");
    }));
    *m.lock().unwrap_or_else(|e| e.into_inner()) = None;
    assert!(m.lock().unwrap_or_else(|e| e.into_inner()).is_none());
}
