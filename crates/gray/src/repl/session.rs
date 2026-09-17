//! Session persistence + agent-event dispatch (split from `repl`).

use super::*;

pub(crate) fn print_exit_hint(session_state: &Option<SessionState>) {
    if let Some(state) = session_state {
        use std::io::IsTerminal as _;
        if std::io::stdout().is_terminal() {
            println!(
                "\x1b[2mTo resume: gray resume {}\x1b[0m",
                state.session_id.as_str()
            );
        } else {
            println!("To resume: gray resume {}", state.session_id.as_str());
        }
        let _ = std::io::stdout().flush();
    }
}

#[allow(clippy::too_many_arguments)]
// Mechanical split of `run_repl_mode`: params are the loop state the arm borrows.
pub(crate) async fn handle_resume(
    config: &mut Config,
    cwd: &Path,
    args: ResumeArgs,
    agent: &mut Option<Agent>,
    session_state: &mut Option<SessionState>,
    totals: &mut SessionTotals,
    tui: Option<&crate::composer::SharedTui>,
    hide_thinking: &mut bool,
) {
    let bg = tui.as_ref().map(|s| s.lock().expect("tui lock").snapshot());
    // Recall-first resolution for cwd-scoped `--last` (`--all` keeps the
    // list scan for the global latest). A validated pointer skips the scan;
    // any miss flows into the existing list path below.
    let mut recalled_id: Option<SessionId> = None;
    if args.target.is_none()
        && args.last
        && !args.all
        && let Some(root) = default_root()
    {
        let store = JsonlSessionStore::new(root);
        if let Some(c) = std::env::current_dir().ok()
            && let Some(rid) = store.recall_validated(&c).await
            && store.load(&rid).await.is_ok()
        {
            recalled_id = Some(rid);
        }
    }
    let target_id: Option<SessionId> = if let Some(rid) = recalled_id {
        Some(rid)
    } else if let Some(raw) = args.target.as_deref() {
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
        let cwd_now = std::env::current_dir().ok();
        let summaries = store.list().await;
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
    } else if tui.is_none() {
        // Headless (piped stdout): the picker needs a TTY — print the list.
        let Some(root) = default_root() else {
            return;
        };
        let store = JsonlSessionStore::new(root);
        let summaries = crate::resume::recent_summaries(&store, args.all).await;
        if summaries.is_empty() {
            println!("no saved sessions in this directory (try /resume --all or --all)");
        } else {
            for s in &summaries {
                println!("{}", crate::resume::format_summary_row(s));
            }
        }
        return;
    } else {
        let result = with_modal(tui, crate::resume::run_resume_picker(args.all, bg.as_ref())).await;
        match result {
            Ok(Some(id)) => Some(id),
            Ok(None) => {
                // Dismissed picker leaves the slash card with no feedback:
                // gap so it doesn't jam the input box.
                if let Some(shared) = &tui {
                    shared.lock().expect("tui lock").ensure_gap(1);
                }
                return;
            }
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
            // Single priority everywhere (mirrors the CLI `--session` block):
            // explicit/config model wins, else the session's recorded model.
            // Every consumer below (agent build, totals, TUI label) uses this
            // one string, so the resolved context window agrees on all paths.
            let eff_model = crate::resume::effective_session_model(
                config.model.as_deref(),
                meta.model.as_str(),
            );
            let model = eff_model.as_deref().unwrap_or("");
            let mut build_config = config.clone();
            build_config.model = eff_model.clone();
            // Resume lands on the session's model: clamp a stale effort
            // (e.g. saved `max` under a Spark session) before painting,
            // and write the result back to the live config + saved file
            // so the footer and the next boot agree.
            if !model.is_empty()
                && let Some((old, new)) =
                    super::clamp_thinking_to_model_name(&mut build_config, model)
            {
                config.thinking_effort = Some(new.clone());
                if let Ok(path) = crate::setup::saved_config_path() {
                    let mut saved = crate::setup::load_saved_config_at(&path);
                    saved.thinking_effort = Some(new.clone());
                    let _ = crate::setup::save_saved_config_at(&path, &saved);
                }
                *hide_thinking = build_config.reasoning_hidden();
                say(
                    tui,
                    &format!(
                        "Thinking effort clamped from {old} to {new} (not supported by this model)"
                    ),
                );
            }
            match build_agent(&build_config, cwd, Some(sid.as_str())).await {
                Ok(built) => {
                    *agent = Some(built.with_messages(history));
                    // T3.4 lifecycle: a resumed session has no guarantee the
                    // old tool results survived — it saw nothing yet.
                    if let Some(ledger) = gray_plugin::builder::current_file_ledger() {
                        ledger.clear();
                    }
                    *session_state = Some(SessionState {
                        full_save_pending: false,
                        session_id: sid.clone(),
                        store,
                    });
                    *totals = SessionTotals::from_entries(&entries, model);
                    if let Some(shared) = &tui {
                        let mut t = shared.lock().expect("tui lock");
                        t.replay_session_history(&entries, cwd);
                        t.ensure_gap(1);
                        // Keep the status-bar model (and its context window)
                        // on the same effective model as the resumed agent.
                        if !model.is_empty() {
                            t.set_model(model.to_string());
                        }
                        // The resumed model keeps its own effort: paint it so
                        // the footer follows the switch instead of the stale
                        // pre-resume level.
                        if let Some(eff) = &build_config.thinking_effort {
                            t.set_thinking_effort(eff.clone());
                            t.set_hide_thinking(build_config.reasoning_hidden());
                        }
                        t.push_dim(format!(
                            "\u{2b22} Resumed session {} ({n} messages)",
                            sid.as_str()
                        ));
                        t.ensure_gap(1);
                        let _ = t.draw();
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
    ensure_session_state(session_state, config, cwd).await;
    if let Some(state) = session_state {
        if state.full_save_pending {
            if let Err(error) = save_full_history(state, agent.messages()).await {
                crate::profile::queue_profile_warning(error);
            }
            return;
        }
        if agent.messages().len() <= initial_count {
            return;
        }
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
                state.full_save_pending = true;
                let warning = save_failure_message("session save", &e);
                log::warn!(target: "gray_session", "{warning}");
                crate::profile::queue_profile_warning(warning);
                break;
            }
        }
    }
}

/// Decision gate for the lazy first build: only mint a session before
/// `build_agent` when there is none yet AND a model is configured. A missing
/// model means `build_agent` bails anyway — minting then would leave junk
/// empty sessions and break the no-model REPL-open behavior.
pub(crate) fn should_ensure_session_before_build(has_session: bool, model: Option<&str>) -> bool {
    !has_session && model.is_some_and(|m| !m.is_empty())
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
        let timestamp = crate::print::now_millis();
        let meta = SessionMeta::new(
            session_id.clone(),
            timestamp,
            cwd.to_path_buf(),
            config.model.clone().unwrap_or_else(|| "unset".into()),
        );
        if let Err(e) = store.create(meta).await {
            log::warn!(target: "gray_session", "session create failed: {e}");
        }
        *session_state = Some(SessionState {
            full_save_pending: false,
            store,
            session_id,
        });
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
                // pi `ToolExecutionComponent` appears immediately (partial):
                // viewport-anchored live card, same header family as the
                // final scrollback card. Single scrollback commit stays at
                // `ToolResult`, so this never duplicates.
                let header = crate::tool_fmt::format_live_tool_header(name, "", None);
                t.upsert_live_tool(id, header, false);
                t.set_status(Some(&format!("Preparing tool: {name}")));
            }
            AgentEvent::ToolCallProgress {
                id,
                name,
                args_so_far,
            } => {
                // pi `updateArgs`: stream the in-progress command into the
                // live card head (same header family as the final card).
                // (No token accounting: the pill carries no estimate — exact
                // counts come from usage reports, never chars/4.)
                pending_tools.insert(id.clone(), (name.clone(), None));
                let header = crate::tool_fmt::format_live_tool_header(name, args_so_far, None);
                t.upsert_live_tool(id, header, false);
                let preview = args_so_far.split_whitespace().collect::<Vec<_>>().join(" ");
                let preview = crate::repl::format::truncate_chars(&preview, 60);
                if preview.is_empty() {
                    t.set_status(Some(&format!("Preparing tool: {name}")));
                } else {
                    t.set_status(Some(&format!("Preparing tool: {name} {preview}")));
                }
            }
            AgentEvent::ToolCallEnd { id, args } => {
                t.end_thinking();
                let name = pending_tools
                    .get(id)
                    .map(|(n, _)| n.clone())
                    .unwrap_or_default();
                pending_tools
                    .entry(id.clone())
                    .and_modify(|e| {
                        if e.0.is_empty() {
                            e.0 = name.clone();
                        }
                        e.1 = Some(args.clone());
                    })
                    .or_insert((name.clone(), Some(args.clone())));
                // pi `markExecutionStarted` + `setArgsComplete`: the live
                // card uses its full header with a leading execution shimmer. No
                // transcript line here: the result card below is the single
                // scrollback render, so a duplicate never lands.
                let header = crate::tool_fmt::format_tool_call_header(&name, args, Some(cwd));
                t.upsert_live_tool(id, header, true);
                t.set_status(Some("Working"));
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
                {
                    let lines = crate::tool_fmt::format_tool_result_lines_with_context(
                        &name,
                        args.as_ref(),
                        output,
                        *is_error,
                        Some(cwd),
                    );
                    // pi `updateResult`: the scrollback commit is the flip —
                    // drop the live card, then push the single render.
                    t.remove_live_tool(id);
                    {
                        let header = args
                            .as_ref()
                            .map(|a| crate::tool_fmt::format_tool_call_header(&name, a, Some(cwd)))
                            .unwrap_or_else(|| ratatui::text::Line::from(name.clone()));
                        t.push_tool_box(header, lines);
                    }
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
                // pi `settle_pending_cards`: leftovers never stick (cancel/
                // error paths emit no ToolResult for in-flight calls).
                t.clear_live_tools();
                t.end_thinking();
                // Billed Σ-per-round totals are the cost basis (`totals`,
                // `turn_footer`, persisted entry) — they must NOT overwrite
                // the StepUsage context gauge. The per-turn live counter is
                // left intact too: `end_turn` captures it for the final
                // `Thought for` line and does the single reset there. The
                // billed output is the one exception: stashed for the Thought
                // line (`· N tok`, reasoning included). The
                // streamed estimate misses tool results and input, so without
                // this the final line reads absurdly low — display-only,
                // never gauge input.
                if usage.total() > 0 {
                    totals.add(usage, model, Some(ms));
                    t.set_turn_billed(usage.output_tokens);
                    // TUI Thought line shows billed output only (reasoning
                    // included); billed totals + cost live in `totals` /
                    // headless footer.
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
                {
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

/// Shared post-compaction persistence for the threshold and overflow paths:
/// reseed the context gauge, ensure a session exists, and write the
/// replacement boundary (reload replays the active transcript, not
/// original + replacement duplicated).
pub(crate) async fn persist_compaction_tail(
    agent: &mut Agent,
    config: &Config,
    session_state: &mut Option<SessionState>,
    cwd: &Path,
    tui: Option<&crate::composer::SharedTui>,
) {
    if let Some(shared) = tui {
        let est = crate::compact::estimate_context_tokens(agent.messages(), None);
        shared.lock().expect("tui lock").seed_estimate_usage(est);
    }
    ensure_session_state(session_state, config, cwd).await;
    if let Some(state) = session_state {
        if let Err(warning) = save_full_history(state, agent.messages()).await {
            say(tui, &warning);
        }
    } else {
        say(
            tui,
            "WARNING: compaction save failed: no session storage available. History remains in memory; do not exit before saving it.",
        );
    }
}

fn save_failure_message(operation: &str, error: &impl std::fmt::Display) -> String {
    let detail = crate::print::scrub_error_text(&error.to_string());
    format!(
        "WARNING: {operation} failed: {detail}. History remains in memory; a full save will be retried on the next turn. Do not exit before it succeeds."
    )
}

async fn save_full_history(state: &mut SessionState, messages: &[Message]) -> Result<(), String> {
    state.full_save_pending = true;
    match state
        .store
        .append_compaction_replacement(&state.session_id, messages)
        .await
    {
        Ok(()) => {
            state.full_save_pending = false;
            Ok(())
        }
        Err(error) => {
            let warning = save_failure_message("compaction save", &error);
            log::warn!(target: "gray_session", "{warning}");
            Err(warning)
        }
    }
}

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
    // Codex parity (`compaction.rs::on_context_compaction_started`): raise a
    // dedicated `Compacting context` status with its own clock BEFORE the
    // summarization call. The input box stays mounted — only the status dock
    // changes — so the chat box can never disappear mid-compact.
    let compaction_id = format!("auto-{}", uuid::Uuid::new_v4().as_simple());
    if let Some(shared) = tui {
        shared
            .lock()
            .expect("tui lock")
            .begin_compaction(compaction_id.clone());
    }
    match crate::compact::auto_compact_if_needed(agent).await {
        Ok(true) => {
            // Codex `on_context_compaction_completed`: single
            // `Context compacted · {elapsed}` line, then header back to
            // `Working`. The turn clock underneath is untouched.
            let elapsed = tui
                .and_then(|shared| {
                    shared
                        .lock()
                        .expect("tui lock")
                        .finish_compaction(&compaction_id, Some("Working"))
                })
                .unwrap_or_default();
            let notice = format!(
                "Context compacted · {} ({}/{} tokens)",
                crate::composer::fmt_elapsed_compact(elapsed.as_secs()),
                crate::setup::format_context_length(tokens),
                crate::setup::format_context_length(window)
            );
            say(tui, &notice);
            // History just shrank: the gauge still holds the pre-compact
            // StepUsage (stale-high until the next turn's first StepUsage).
            // Reseed from the post-compact estimate so the footer, /context,
            // and the next trigger all see the compacted size immediately.
            persist_compaction_tail(agent, config, session_state, cwd, tui).await;
            *initial_count = agent.messages().len();
        }
        Ok(false) => {
            // Nothing gained: drop the status silently, no transcript line
            // (Codex: turn ends without completion clears without a message).
            if let Some(shared) = tui {
                shared
                    .lock()
                    .expect("tui lock")
                    .finish_compaction(&compaction_id, None);
            }
        }
        Err(e) => {
            if let Some(shared) = tui {
                shared
                    .lock()
                    .expect("tui lock")
                    .finish_compaction(&compaction_id, None);
            }
            log::warn!(target: "gray_compact", "threshold auto-compact failed: {e}")
        }
    }
}

pub(crate) async fn maybe_overflow_compact(
    agent: &mut Agent,
    config: &Config,
    session_state: &mut Option<SessionState>,
    cwd: &Path,
    tui: Option<&crate::composer::SharedTui>,
    initial_count: &mut usize,
    err: &CoreError,
) -> bool {
    if !crate::compact::is_context_overflow_error(err) {
        return false;
    }
    // Codex parity: no pre-message — the `Compacting context` status dock is
    // the ongoing signal (it survives follow-up input/retries). The single
    // transcript line lands on completion below.
    let compaction_id = format!("overflow-{}", uuid::Uuid::new_v4().as_simple());
    if let Some(shared) = tui {
        shared
            .lock()
            .expect("tui lock")
            .begin_compaction(compaction_id.clone());
    }
    match crate::compact::auto_compact_if_needed(agent).await {
        Ok(true) => {
            let elapsed = tui
                .and_then(|shared| {
                    shared
                        .lock()
                        .expect("tui lock")
                        .finish_compaction(&compaction_id, Some("Working"))
                })
                .unwrap_or_default();
            say(
                tui,
                &format!(
                    "context overflow — Context compacted · {}",
                    crate::composer::fmt_elapsed_compact(elapsed.as_secs())
                ),
            );
            // See threshold path: reseed the gauge to the compacted size.
            persist_compaction_tail(agent, config, session_state, cwd, tui).await;
            *initial_count = agent.messages().len();
            true
        }
        Ok(false) => {
            if let Some(shared) = tui {
                shared
                    .lock()
                    .expect("tui lock")
                    .finish_compaction(&compaction_id, Some("Working"));
            }
            log::warn!(target: "gray_compact", "overflow auto-compact returned false (nothing to compact)");
            false
        }
        Err(e) => {
            if let Some(shared) = tui {
                shared
                    .lock()
                    .expect("tui lock")
                    .finish_compaction(&compaction_id, Some("Working"));
            }
            log::warn!(target: "gray_compact", "overflow auto-compact failed: {e}");
            false
        }
    }
}

#[path = "session_tests.rs"]
#[cfg(test)]
mod tests;
