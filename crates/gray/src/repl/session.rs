//! Session persistence + agent-event dispatch (split from `repl`).

use super::*;

pub(crate) fn print_exit_hint(session_state: &Option<SessionState>) {
    if let Some(state) = session_state {
        println!(
            "\x1b[2mTo resume: gray resume {}\x1b[0m",
            state.session_id.as_str()
        );
        let _ = std::io::stdout().flush();
    }
}

pub(crate) async fn handle_resume(
    config: &Config,
    cwd: &Path,
    args: ResumeArgs,
    agent: &mut Option<Agent>,
    session_state: &mut Option<SessionState>,
    totals: &mut SessionTotals,
    tui: Option<&crate::composer::SharedTui>,
) {
    let bg = tui.as_ref().map(|s| s.lock().expect("tui lock").snapshot());
    let target_id: Option<SessionId> = if let Some(raw) = args.target.as_deref() {
        if let Some(root) = default_root() {
            let store = JsonlSessionStore::new(root);
            if let Some(id) = crate::resume::resolve_prefix(&store, raw, args.all).await {
                Some(id)
            } else {
                let sid = SessionId::new(raw);
                match store.load(&sid).await {
                    Ok(_) => Some(sid),
                    Err(e) => {
                        let msg = format!("no session matching '{raw}': {e}");
                        if let Some(shared) = &tui {
                            shared
                                .lock()
                                .expect("tui lock")
                                .push_dim(format!("└ {msg}"));
                        } else {
                            println!("{msg}");
                        }
                        return;
                    }
                }
            }
        } else {
            None
        }
    } else if args.last {
        let Some(root) = default_root() else {
            return;
        };
        let store = JsonlSessionStore::new(root);
        let summaries = store.list().await;
        let cwd_now = std::env::current_dir().ok();
        let filt = if args.all { None } else { cwd_now.as_deref() };
        match crate::resume::latest_summary(&summaries, filt) {
            Some(s) => Some(s.id.clone()),
            None => {
                let msg = if args.all {
                    "no saved sessions"
                } else {
                    "no saved sessions in this directory (try /resume --all or --all)"
                };
                if let Some(shared) = &tui {
                    shared
                        .lock()
                        .expect("tui lock")
                        .push_dim(format!("└ {msg}"));
                } else {
                    println!("{msg}");
                }
                return;
            }
        }
    } else {
        let result = with_modal(tui, crate::resume::run_resume_picker(args.all, bg.as_ref())).await;
        match result {
            Ok(Some(id)) => Some(id),
            Ok(None) => return,
            Err(e) => {
                if let Some(shared) = &tui {
                    shared
                        .lock()
                        .expect("tui lock")
                        .push_dim(format!("└ resume picker error: {e}"));
                } else {
                    println!("resume picker error: {e}");
                }
                return;
            }
        }
    };

    let Some(sid) = target_id else {
        return;
    };
    let Some(root) = default_root() else {
        return;
    };
    let store = JsonlSessionStore::new(root);
    match store.load(&sid).await {
        Ok((meta, entries)) => {
            let history: Vec<Message> = entries.iter().map(|e| e.message.clone()).collect();
            let n = history.len();
            let model = if meta.model.is_empty() {
                config.model.as_deref().unwrap_or("")
            } else {
                meta.model.as_str()
            };
            match build_agent(config, cwd, Some(sid.as_str())).await {
                Ok(built) => {
                    *agent = Some(built.with_messages(history));
                    *session_state = Some(SessionState {
                        session_id: sid.clone(),
                        store,
                    });
                    *totals = SessionTotals::from_entries(&entries, model);
                    if let Some(shared) = &tui {
                        let mut t = shared.lock().expect("tui lock");
                        t.replay_session_history(&entries, cwd);
                        t.ensure_gap(1);
                        t.push_dim(format!(
                            "\u{2b22} Resumed session {} ({n} messages)",
                            sid.as_str()
                        ));
                        t.ensure_gap(1);
                    } else {
                        println!(
                            "\x1b[2m\u{2b22} Resumed session {} ({n} messages)\x1b[0m",
                            sid.as_str()
                        );
                    }
                }
                Err(e) => {
                    let msg = format!("could not resume (no provider): {e}");
                    if let Some(shared) = &tui {
                        shared
                            .lock()
                            .expect("tui lock")
                            .push_dim(format!("└ {msg}"));
                    } else {
                        println!("{msg}");
                    }
                }
            }
        }
        Err(e) => {
            let msg = format!("could not resume session {}: {e}", sid.as_str());
            if let Some(shared) = &tui {
                shared
                    .lock()
                    .expect("tui lock")
                    .push_dim(format!("└ {msg}"));
            } else {
                println!("{msg}");
            }
        }
    }
}

/// Appends whatever messages reached memory this turn (success, cancel, or
/// error) to the session store, so the JSONL transcript never diverges from
/// in-memory history.
pub(crate) async fn persist_turn_messages(
    session_state: &mut Option<SessionState>,
    agent: &Agent,
    config: &Config,
    cwd: &Path,
    initial_count: usize,
    latest_usage: Option<gray_core::event::Usage>,
    duration_ms: Option<u64>,
) {
    if session_state.is_none()
        && let Some(root) = default_root()
    {
        let store = JsonlSessionStore::new(root);
        let session_id = SessionId::generate();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let meta = SessionMeta::new(
            session_id.clone(),
            timestamp,
            cwd.to_path_buf(),
            config.model.clone().unwrap_or_else(|| "unset".into()),
        );
        if let Err(e) = store.create(meta).await {
            log::warn!(target: "gray_session", "session create failed: {e}");
        }
        *session_state = Some(SessionState { store, session_id });
    }
    if let Some(state) = session_state
        && agent.messages().len() > initial_count
    {
        let new_messages = &agent.messages()[initial_count..];
        for (i, msg) in new_messages.iter().enumerate() {
            let is_last = i == new_messages.len() - 1;
            let usage = if is_last { latest_usage } else { None };
            let duration = if is_last { duration_ms } else { None };
            if let Err(e) = state
                .store
                .append_with_usage_and_duration(&state.session_id, msg, usage, duration)
                .await
            {
                log::warn!(target: "gray_session", "session append failed: {e}");
            }
        }
    }
}

pub(crate) async fn ensure_session_state(
    session_state: &mut Option<SessionState>,
    config: &Config,
    cwd: &Path,
) {
    if session_state.is_none()
        && let Some(root) = default_root()
    {
        let store = JsonlSessionStore::new(root);
        let session_id = SessionId::generate();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let meta = SessionMeta::new(
            session_id.clone(),
            timestamp,
            cwd.to_path_buf(),
            config.model.clone().unwrap_or_else(|| "unset".into()),
        );
        if let Err(e) = store.create(meta).await {
            log::warn!(target: "gray_session", "session create failed: {e}");
        }
        *session_state = Some(SessionState { store, session_id });
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn dispatch_agent_event(
    ev: &AgentEvent,
    tui_stream: Option<&crate::composer::SharedTui>,
    interactive: bool,
    pending_tools: &mut HashMap<String, (String, Option<serde_json::Value>)>,
    turn_usage: &mut Option<gray_core::event::Usage>,
    cwd: &Path,
    model: &str,
    totals: &mut SessionTotals,
    turn_start: std::time::Instant,
    turn_duration_ms: &mut Option<u64>,
) {
    // Single elapsed source — TurnEnd stamps duration once so footer,
    // totals, and persisted entry agree even when TUI + headless paths diverge.
    let elapsed_ms = || turn_start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    if let Some(shared) = tui_stream
        && let Ok(mut t) = shared.lock()
    {
        match ev {
            AgentEvent::ThinkingDelta { delta } => t.stream_thinking(delta),
            AgentEvent::TextDelta { delta } => t.stream_text(delta),
            AgentEvent::ToolCallStart { id, name } => {
                t.flush_markdown();
                t.end_thinking();
                pending_tools.insert(id.clone(), (name.clone(), None));
            }
            AgentEvent::ToolCallEnd { id, args } => {
                t.end_thinking();
                pending_tools
                    .entry(id.clone())
                    .and_modify(|e| e.1 = Some(args.clone()))
                    .or_insert((String::new(), Some(args.clone())));
            }
            AgentEvent::ToolResult {
                id,
                output,
                is_error,
                ..
            } => {
                // Keyed by call id so parallel/retried calls can never swap
                // names and args (single-slot tracking rendered `● command=…`
                // headers with no tool name).
                let (name, args) = pending_tools
                    .remove(id)
                    .map(|(n, a)| (if n.is_empty() { "tool".to_string() } else { n }, a))
                    .unwrap_or_else(|| ("tool".to_string(), None));
                if name != "request_user_input" {
                    let lines = crate::tool_fmt::format_tool_result_lines_with_context(
                        &name,
                        args.as_ref(),
                        output,
                        *is_error,
                        Some(cwd),
                    );
                    let header = args
                        .as_ref()
                        .map(|a| crate::tool_fmt::format_tool_call_header(&name, a, Some(cwd)))
                        .unwrap_or_else(|| ratatui::text::Line::from(name.clone()));
                    t.push_tool_box(header, lines);
                }
            }
            AgentEvent::StepUsage { usage } => {
                t.set_usage(*usage);
            }
            // Reconnecting rides the shimmer status dock (`⬡ Reconnecting…`)
            // like Thinking/Working instead of a static cell; the cause
            // lands as one dim detail row (single notice per burst).
            AgentEvent::StreamError { details, .. } => {
                t.flush_markdown();
                t.end_thinking();
                t.set_status(Some("Reconnecting"));
                if !details.is_empty() {
                    let trunc = crate::repl::format::truncate_chars(details, 200);
                    t.push_dim(format!("└ {trunc}"));
                }
            }
            AgentEvent::TurnEnd { usage, .. } => {
                *turn_usage = Some(*usage);
                let ms = elapsed_ms();
                *turn_duration_ms = Some(ms);
                t.end_thinking();
                t.set_usage(*usage);
                if usage.total() > 0 {
                    totals.add(usage, model, Some(ms));
                    t.push_usage(turn_footer(usage, model, totals, Some(ms)));
                }
            }
            _ => {}
        }
        let _ = std::io::stdout().flush();
        return;
    }
    if !interactive {
        match ev {
            AgentEvent::TextDelta { delta } => print!("{delta}"),
            AgentEvent::ThinkingDelta { delta } => print!("{THINKING_STYLE}{delta}\x1b[0m"),
            AgentEvent::ToolCallStart { id, name } => {
                pending_tools.insert(id.clone(), (name.clone(), None));
            }
            AgentEvent::ToolCallEnd { id, args } => {
                let entry = pending_tools
                    .entry(id.clone())
                    .or_insert((String::new(), None));
                entry.1 = Some(args.clone());
                let name = if entry.0.is_empty() {
                    "tool"
                } else {
                    entry.0.as_str()
                };
                if name != "request_user_input" {
                    println!(
                        "\n{}",
                        crate::tool_fmt::format_tool_call_header_plain(name, args, Some(cwd))
                    );
                }
            }
            AgentEvent::ToolResult {
                id,
                output,
                is_error,
                ..
            } => {
                let (name, args) = pending_tools
                    .remove(id)
                    .map(|(n, a)| (if n.is_empty() { "tool".to_string() } else { n }, a))
                    .unwrap_or_else(|| ("tool".to_string(), None));
                let res = crate::tool_fmt::format_tool_result_plain_with_context(
                    &name,
                    args.as_ref(),
                    output,
                    *is_error,
                    Some(cwd),
                );
                if !res.is_empty() {
                    print!("{res}");
                }
            }
            AgentEvent::StreamError { message, details } => {
                if details.is_empty() {
                    eprintln!("\n\x1b[2m⚠ {message}\x1b[0m");
                } else {
                    eprintln!("\n\x1b[2m⚠ {message}\n└ {details}\x1b[0m");
                }
            }
            AgentEvent::TurnEnd { usage, .. } => {
                *turn_usage = Some(*usage);
                let ms = elapsed_ms();
                *turn_duration_ms = Some(ms);
                if usage.total() > 0 {
                    totals.add(usage, model, Some(ms));
                    println!(
                        "\n\x1b[2m{}\x1b[0m",
                        turn_footer(usage, model, totals, Some(ms))
                    );
                }
            }
            _ => {}
        }
        let _ = std::io::stdout().flush();
    }
}

/// Evaluated once per process on the threshold-compact path (the REPL's only
/// auto-compact entry): picks up `GRAY_NO_AUTO_COMPACT=1` from the environment.
static AUTO_COMPACT_ENV_ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();

pub(crate) async fn maybe_threshold_compact(
    agent: &mut Agent,
    config: &Config,
    session_state: &mut Option<SessionState>,
    cwd: &Path,
    tui: Option<&crate::composer::SharedTui>,
    latest: Option<gray_core::event::Usage>,
    initial_count: &mut usize,
) {
    AUTO_COMPACT_ENV_ONCE.get_or_init(crate::compact::init_auto_compact_from_env);
    let window = crate::setup::resolve_model_context_length(config.model.as_deref().unwrap_or(""));
    let tokens = crate::compact::estimate_context_tokens(agent.messages(), latest);
    if !crate::compact::should_compact(
        tokens,
        window,
        &crate::compact::compaction_settings_for(window),
    ) {
        return;
    }
    let notice = format!(
        "auto-compacting {}/{} tokens...",
        crate::setup::format_context_length(tokens),
        crate::setup::format_context_length(window)
    );
    match crate::compact::auto_compact_if_needed(agent, config, latest, "threshold").await {
        Ok(true) => {
            say(tui, &notice);
            ensure_session_state(session_state, config, cwd).await;
            if let Some(state) = session_state {
                for msg in agent.messages().to_vec() {
                    let _ = state.store.append(&state.session_id, &msg).await;
                }
            }
            *initial_count = agent.messages().len();
        }
        Ok(false) => {}
        Err(e) => log::warn!(target: "gray_compact", "threshold auto-compact failed: {e}"),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn maybe_overflow_compact(
    agent: &mut Agent,
    config: &Config,
    session_state: &mut Option<SessionState>,
    cwd: &Path,
    tui: Option<&crate::composer::SharedTui>,
    latest: Option<gray_core::event::Usage>,
    initial_count: &mut usize,
    err: &CoreError,
) -> bool {
    if !crate::compact::is_context_overflow_error(err) {
        return false;
    }
    // Mid-turn notice (not after a card): keep the separating blank say() used to add.
    if let Some(t) = tui {
        t.lock().expect("tui lock").ensure_gap(1);
    }
    say(tui, "context overflow — compacting...");
    match crate::compact::auto_compact_if_needed(agent, config, latest, "overflow").await {
        Ok(true) => {
            ensure_session_state(session_state, config, cwd).await;
            if let Some(state) = session_state {
                for msg in agent.messages().to_vec() {
                    let _ = state.store.append(&state.session_id, &msg).await;
                }
            }
            *initial_count = agent.messages().len();
            true
        }
        Ok(false) => {
            log::warn!(target: "gray_compact", "overflow auto-compact returned false (nothing to compact)");
            false
        }
        Err(e) => {
            log::warn!(target: "gray_compact", "overflow auto-compact failed: {e}");
            false
        }
    }
}
