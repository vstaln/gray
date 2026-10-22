use super::{is_turn_local_command, lock_tui, try_lock_tui};

#[test]
fn try_lock_recovers_poisoned_mutex() {
    let m = std::sync::Arc::new(std::sync::Mutex::new(1u32));
    let clone = m.clone();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = clone.lock().unwrap();
        panic!("poison the mutex");
    }));

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
fn consumed_key_is_not_lost_while_renderer_holds_tui() {
    let state = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let renderer = state.lock().unwrap();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let state = state.clone();
        let reader = scope.spawn(move || {
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

struct TurnLocalHook;

#[async_trait::async_trait]
impl gray_core::agent::PluginHooks for TurnLocalHook {
    fn commands(&self) -> Vec<gray_core::agent::PluginCommand> {
        vec![
            gray_core::agent::PluginCommand {
                name: "/subagents".to_string(),
                description: "agents panel".to_string(),
                turn_local_verbs: ["", "status", "view", "list", "open"]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            },
            gray_core::agent::PluginCommand {
                name: "/board".to_string(),
                description: "dashboard".to_string(),
                turn_local_verbs: vec!["*".to_string()],
            },
            gray_core::agent::PluginCommand {
                name: "/queue".to_string(),
                description: "never turn-local".to_string(),
                turn_local_verbs: Vec::new(),
            },
        ]
    }
}

fn hooks() -> Vec<std::sync::Arc<dyn gray_core::agent::PluginHooks>> {
    vec![std::sync::Arc::new(TurnLocalHook)]
}

#[test]
fn builtin_commands_stay_turn_local() {
    for cmd in ["/jobs", "/help", "/plugins", "/reload", "/hehe"] {
        assert!(is_turn_local_command(cmd, &hooks()), "{cmd}");
        assert!(is_turn_local_command(cmd, &[]), "{cmd} needs no hooks");
    }
}

#[test]
fn plugin_declared_verbs_are_turn_local() {
    for cmd in [
        "/subagents",
        "/subagents status",
        "/subagents status --all",
        "/subagents view e2e",
        "/subagents open e2e",
        "/subagents list",
    ] {
        assert!(is_turn_local_command(cmd, &hooks()), "{cmd}");
    }
}

#[test]
fn plugin_star_verb_allows_any_argv() {
    for cmd in ["/board", "/board anything", "/board sub verb"] {
        assert!(is_turn_local_command(cmd, &hooks()), "{cmd}");
    }
}

#[test]
fn plugin_undeclared_verbs_and_commands_queue() {
    for cmd in [
        "/subagents run do work",
        "/subagents steer e2e hello",
        "/subagents chat e2e hello",
        "/subagents stop e2e",
        "/subagents stop all",
        "/subagents settings",
        "/subagents setup",
        "/subagents widget --demo",
        "/subagents menu",
        "/subagents e2e",
        "/queue",
        "/queue status",
        "/unknown",
        "/unknown status",
        "/subagents-quiet status",
    ] {
        assert!(!is_turn_local_command(cmd, &hooks()), "{cmd}");
    }
    assert!(
        !is_turn_local_command("/subagents status", &[]),
        "no hooks, no turn-local plugin commands"
    );
}

#[test]
fn mutating_or_agent_commands_still_queue() {
    for cmd in ["/model", "/compact", "normal follow-up"] {
        assert!(!is_turn_local_command(cmd, &hooks()), "{cmd}");
    }
}
