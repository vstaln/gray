use super::try_lock_tui;

// UNRUN (cargo test banned under X): run in TTY/CI.
#[test]
fn try_lock_recovers_poisoned_mutex() {
    let m = std::sync::Arc::new(std::sync::Mutex::new(1u32));
    let clone = m.clone();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = clone.lock().unwrap();
        panic!("poison the mutex");
    }));
    // Poisoned: still usable, value preserved.
    let g = try_lock_tui(&m).expect("poisoned mutex must recover");
    assert_eq!(*g, 1u32);
}

// UNRUN (cargo test banned under X): run in TTY/CI.
#[test]
fn try_lock_skips_contended_mutex() {
    let m = std::sync::Arc::new(std::sync::Mutex::new(0u32));
    let _held = m.lock().unwrap();
    std::thread::scope(|s| {
        let got = s
            .spawn(|| try_lock_tui(&m).is_some())
            .join()
            .expect("thread joins");
        assert!(!got, "contended lock must skip, never block");
    });
}

// UNRUN (cargo test banned under X): run in TTY/CI.
#[test]
fn try_lock_happy_path() {
    let m = std::sync::Arc::new(std::sync::Mutex::new(7u32));
    assert_eq!(*try_lock_tui(&m).expect("uncontended lock"), 7u32);
}
