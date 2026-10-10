use super::{is_turn_local_command, lock_tui, try_lock_tui};

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

#[test]
fn try_lock_happy_path() {
    let m = std::sync::Arc::new(std::sync::Mutex::new(7u32));
    assert_eq!(*try_lock_tui(&m).expect("uncontended lock"), 7u32);
}

#[test]
fn consumed_key_is_not_lost_while_renderer_holds_tui() {
    let state = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let renderer = state.lock().unwrap();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let state = state.clone();
        let reader = scope.spawn(move || {
            // Event has already been consumed by crossterm::read().
            ready_tx.send(()).unwrap();
            let mut draft = lock_tui(&state);
            draft.push('x');
        });
        ready_rx.recv().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(40));
        drop(renderer);
        reader.join().unwrap();
    });
    assert_eq!(
        *state.lock().unwrap(),
        "x",
        "a consumed event must survive renderer contention"
    );
}

#[test]
fn consumed_paste_and_keys_preserve_order_under_contention() {
    let state = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let renderer = state.lock().unwrap();
    std::thread::scope(|scope| {
        let state = state.clone();
        let reader = scope.spawn(move || {
            for input in ["a", "pasted words", "b"] {
                lock_tui(&state).push_str(input);
            }
        });
        std::thread::sleep(std::time::Duration::from_millis(40));
        drop(renderer);
        reader.join().unwrap();
    });
    assert_eq!(*state.lock().unwrap(), "apasted wordsb");
}

#[test]
fn turn_local_commands_are_read_only() {
    for cmd in [
        "/usage",
        "/cost",
        "/jobs",
        "/cron",
        "/cron nightly",
        "/help",
        "/subagents",
        "/subagents status",
        "/subagents status --all",
        "/subagents view e2e",
        "/subagents open e2e",
        "/subagents list",
        "/subagents menu",
        "/subagents entries",
        "/subagents manifest",
        "/subagents e2e",
    ] {
        assert!(is_turn_local_command(cmd), "{cmd}");
    }
}

#[test]
fn mutating_or_agent_commands_still_queue() {
    for cmd in [
        "/subagents run do work",
        "/subagents steer e2e hello",
        "/subagents chat e2e hello",
        "/subagents stop e2e",
        "/subagents stop all",
        "/subagents settings",
        "/subagents setup",
        "/subagents widget --demo",
        "/model",
        "/compact",
        "/cron on",
        "/cron off",
        "/cron enable",
        "/cron disable",
        "normal follow-up",
    ] {
        assert!(!is_turn_local_command(cmd), "{cmd}");
    }
}
