//! Session persistence + agent-event dispatch (split from `repl`).

use super::*;

/// The session the resume hint names, mirrored out of the REPL loop because
/// the Ctrl-C and SIGHUP/SIGTERM exits run in signal tasks that can't
/// see `session_state`. Synced at the loop top and wherever a turn mints a
/// session (`ensure_session_state`).
static EXIT_SESSION: StdMutex<Option<String>> = StdMutex::new(None);

/// Set by the first exit path that prints the resume hint: every later path
/// (a signal exit racing a normal teardown) sees it and skips — the hint is
/// printed at most once per process.
static EXIT_HINT_PRINTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// True exactly once per process — the caller that gets true prints the hint.
pub(crate) fn claim_exit_hint() -> bool {
    !EXIT_HINT_PRINTED.swap(true, std::sync::atomic::Ordering::Relaxed)
}

/// Whether the resume hint was already printed (a normal exit path got there
/// first — the band is already down, so a signal exit can leave directly).
pub(crate) fn exit_hint_printed() -> bool {
    EXIT_HINT_PRINTED.load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) fn remember_exit_session(id: Option<&str>) {
    *EXIT_SESSION.lock().unwrap_or_else(|e| e.into_inner()) = id.map(str::to_string);
}

pub(crate) fn exit_session() -> Option<String> {
    EXIT_SESSION
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Which session is live and the shell line that reattaches to it. The
/// Ctrl+C exit hint's contents, shown by `/sessions` before the picker.
pub(crate) fn current_session_line(session_state: &Option<SessionState>) -> String {
    match session_state.as_ref() {
        Some(s) => format!(
            "session {} · {}",
            s.session_id,
            exit_hint_line(s.session_id.as_str(), false)
        ),
        None => "no session yet — the first turn creates one".into(),
    }
}

pub(crate) fn exit_hint_line(session_id: &str, styled: bool) -> String {
    if styled {
        format!("\x1b[2mTo resume: gray -r {session_id}\x1b[0m")
    } else {
        format!("To resume: gray -r {session_id}")
    }
}

pub(crate) fn print_exit_hint(session_state: &Option<SessionState>) {
    if let Some(state) = session_state
        && claim_exit_hint()
    {
        use std::io::IsTerminal as _;
        println!(
            "{}",
            exit_hint_line(state.session_id.as_str(), std::io::stdout().is_terminal())
        );
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
    if args.dismiss {
        let scope = if args.all { None } else { Some(cwd) };
        let n = crate::resume::dismiss_interrupted(scope).await;
        let msg = match (n, args.all) {
            (0, _) => "no interrupted sessions to dismiss".to_string(),
            (n, false) => format!("dismissed {n} interrupted session{}", plural(n)),
            (n, true) => format!(
                "dismissed {n} interrupted session{} in every directory",
                plural(n)
            ),
        };
        if let Some(shared) = &tui {
            shared
                .lock()
                .expect("tui lock")
                .push_dim(format!("\u{2514} {msg}"));
        } else {
            println!("{msg}");
        }
        return;
    }
    if args.target.is_none() {
        say(tui, &current_session_line(session_state));
    }
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
                            super::cron::push_notice(
                                &mut shared.lock().expect("tui lock"),
                                "Resume failed",
                                &msg,
                                true,
                            );
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
                    "no saved sessions in this directory (try /sessions --all or --all)"
                };
                if let Some(shared) = &tui {
                    super::cron::push_notice(
                        &mut shared.lock().expect("tui lock"),
                        "Sessions",
                        &msg,
                        false,
                    );
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
            println!("no saved sessions in this directory (try /sessions --all or --all)");
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
                    shared.lock().expect("tui lock").ensure_gap();
                }
                return;
            }
            Err(e) => {
                if let Some(shared) = &tui {
                    super::cron::push_notice(
                        &mut shared.lock().expect("tui lock"),
                        "Resume picker failed",
                        &e.to_string(),
                        true,
                    );
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
    // One owner per session: a live holder stays put — switching anyway
    // would interleave two processes' appends into one history. Claiming
    // before the load so the file can't change under another owner; a
    // session we already hold shares our lock (registry), never self-locks.
    let open_guard = match store.acquire_open(&sid).await {
        Ok(guard) => guard,
        Err(e) => {
            let msg = match &e {
                crate::session_store::SessionError::Locked { id, pid } => {
                    crate::session_store::SessionError::locked_notice(id, *pid)
                }
                _ => e.to_string(),
            };
            say(tui, &msg);
            return;
        }
    };
    match store.load(&sid).await {
        Ok((meta, entries)) => {
            let history: Vec<Message> = entries.iter().map(|e| e.message.clone()).collect();
            let n = history.len();
            // Single priority everywhere (mirrors the CLI `--session` block):
            // explicit/config model wins, else the session's recorded model.
            // Every consumer below (agent build, totals, TUI label) uses this
            // one string, so the resolved context window agrees on all paths.
            // A live or saved `<base>-<tier>` id canonicalizes first — a
            // session recorded on `swe-2-max` lands on `swe-2` at `max`.
            {
                let rows = crate::setup::canonical_model_rows(config);
                crate::setup::canonicalize_effort_variant(config, &rows);
            }
            let eff_model = crate::resume::effective_session_model(
                config.model.as_deref(),
                meta.model.as_str(),
            );
            let mut build_config = config.clone();
            build_config.model = eff_model.clone();
            {
                let rows = crate::setup::canonical_model_rows(&build_config);
                crate::setup::canonicalize_effort_variant(&mut build_config, &rows);
            }
            let model = build_config.model.clone().unwrap_or_default();
            let model = model.as_str();
            // Resume lands on the session's model: adopt that target's own
            // remembered effort (or the default) so the level a different
            // connection left behind never follows, then clamp whatever came
            // out (e.g. saved `max` under a Spark session) and paint the
            // result so the footer and the next boot agree.
            if !model.is_empty() {
                let before_effort = build_config.thinking_effort.clone();
                crate::setup::adopt_connection_effort(&mut build_config);
                let clamped = super::clamp_thinking_to_model_name(&mut build_config, model);
                if build_config.thinking_effort != before_effort {
                    config.thinking_effort = build_config.thinking_effort.clone();
                    *hide_thinking = build_config.reasoning_hidden();
                }
                if build_config.fast_mode != config.fast_mode {
                    config.fast_mode = build_config.fast_mode;
                }
                if let Some((old, new)) = clamped {
                    say(
                        tui,
                        &format!(
                            "Thinking effort clamped from {old} to {new} (not supported by this model)"
                        ),
                    );
                }
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
                        _open_guard: Some(open_guard),
                    });
                    *totals = SessionTotals::from_entries(&entries, model);
                    if let Some(shared) = &tui {
                        let mut t = shared.lock().expect("tui lock");
                        t.replay_session_history(&entries, cwd);
                        t.ensure_gap();
                        // Keep the status-bar model (and its context window)
                        // on the same effective model as the resumed agent.
                        if !model.is_empty() {
                            t.set_model(model.to_string());
                            t.set_model_label(crate::setup::composite_label_for(config));
                        }
                        // The resumed model keeps its own effort: paint it so
                        // the footer follows the switch instead of the stale
                        // pre-resume level.
                        if let Some(eff) = &build_config.thinking_effort {
                            t.set_thinking_effort(crate::setup::effort_chip(eff, &build_config));
                            t.set_hide_thinking(build_config.reasoning_hidden());
                        }
                        t.push_dim(format!(
                            "\u{2b22} Resumed session {} ({n} messages)",
                            sid.as_str()
                        ));
                        t.ensure_gap();
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
                        super::cron::push_notice(
                            &mut shared.lock().expect("tui lock"),
                            "Resume failed",
                            &msg,
                            true,
                        );
                    } else {
                        println!("{msg}");
                    }
                }
            }
        }
        Err(e) => {
            let msg = format!("could not resume session {}: {e}", sid.as_str());
            if let Some(shared) = &tui {
                super::cron::push_notice(
                    &mut shared.lock().expect("tui lock"),
                    "Resume failed",
                    &msg,
                    true,
                );
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
                // Same repair-then-retry as `save_full_history`, gated on
                // the same retryable errors: a torn tail heals via `load`,
                // a pruned file re-mints via `create_new` (never truncates).
                // Only a still-failing append flips to full-save mode — and
                // if the full save then succeeds, normal appends resume next
                // turn instead of warning forever.
                use crate::session_store::SessionError;
                let retryable = matches!(
                    &e,
                    SessionError::Io(_) | SessionError::Corrupt { .. } | SessionError::NotFound(_)
                );
                let healed =
                    retryable && repair_and_retry_append(state, msg, usage, duration).await;
                if !healed {
                    state.full_save_pending = true;
                    let warning = save_failure_message("session save", &e);
                    log::warn!(target: "gray_session", "{warning}");
                    crate::profile::queue_profile_warning(warning);
                    break;
                }
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
        let session_id = store.fresh_id().await;
        let timestamp = crate::print::now_millis();
        let meta = SessionMeta::new(
            session_id.clone(),
            timestamp,
            cwd.to_path_buf(),
            config.model.clone().unwrap_or_else(|| "unset".into()),
        )
        .with_origin(crate::session_store::session_origin_from_env());
        if let Err(e) = store.create(meta).await {
            log::warn!(target: "gray_session", "session create failed: {e}");
        }
        // Fresh id — the open lock should always be free; hold it so a
        // second gray can't `-r` this session out from under us. A failure
        // is pathological: degrade to guardless, like create-failure above.
        let open_guard = match store.acquire_open(&session_id).await {
            Ok(guard) => Some(guard),
            Err(e) => {
                log::warn!(target: "gray_session", "session open-lock failed: {e}");
                None
            }
        };
        *session_state = Some(SessionState {
            full_save_pending: false,
            store,
            session_id,
            _open_guard: open_guard,
        });
    }
    remember_exit_session(session_state.as_ref().map(|s| s.session_id.as_str()));
}

/// Streaming-only clock for one turn — the denominator every tps readout
/// should use, because a rate is only meaningful over the time tokens were
/// actually flowing. Response-side events (deltas, tool-call start/end)
/// each bill the gap since the previous event; boundary events (turn
/// start, tool results, round usage, compaction, reconnects) re-anchor
/// the window without billing, so the host-side waits they close — tool
/// runs, round-trips, retries — never land in the rate. The sum telescopes
/// to each burst's whole response window including the leg before the
/// first delta: a provider that delivers its reply in one batch (relay
/// plugins fold a finished turn into a single SSE burst) still reports
/// the generation time it took instead of a near-zero denominator that
/// would print millions of tps.
#[derive(Debug, Default)]
pub(crate) struct TurnStreamClock {
    /// Arrival time of the previous window-relevant event.
    anchor: Option<std::time::Instant>,
    /// Billed response-window total across every burst this turn, at full
    /// `Duration` precision. Millisecond-granular accumulation undercounts
    /// catastrophically: real events land microseconds apart, so each gap
    /// truncated to 0ms and a 5-minute turn ended with a ~10ms denominator
    /// (6,266 tok → 626,600 tps). The sum still telescopes to each burst's
    /// last-minus-first window; only the readout truncates to ms.
    streamed: std::time::Duration,
}

impl TurnStreamClock {
    /// Notes one response-side event (text/reasoning/arg deltas, tool-call
    /// start/end). Bills the gap since the previous event — the first one
    /// after a boundary bills the request's dispatch+generate leg, which
    /// for a batch provider IS the generation time.
    pub(crate) fn tick(&mut self) {
        self.tick_at(std::time::Instant::now());
    }

    fn tick_at(&mut self, now: std::time::Instant) {
        if let Some(prev) = self.anchor.replace(now) {
            self.streamed = self.streamed.saturating_add(now.duration_since(prev));
        }
    }

    /// Re-anchors the window at a boundary (turn start, tool result,
    /// round report, compaction, reconnect) without billing: the wait the
    /// boundary closes is host-side or transport, never token generation.
    pub(crate) fn open_span(&mut self) {
        self.open_span_at(std::time::Instant::now());
    }

    fn open_span_at(&mut self, now: std::time::Instant) {
        self.anchor = Some(now);
    }

    /// Closes the open window at turn end; post-turn events never bill.
    pub(crate) fn close_span(&mut self) {
        self.anchor = None;
    }

    /// Streaming-only elapsed ms, or `None` when nothing streamed.
    pub(crate) fn streamed_ms(&self) -> Option<u64> {
        let ms = u64::try_from(self.streamed.as_millis()).unwrap_or(u64::MAX);
        (ms > 0).then_some(ms)
    }
}

/// Label-aware tool header: the agent's display-only headline (plugin
/// `label`) rides inside args so `tool_fmt` names the card; the wire name
/// stays the executor key and events stay unchanged.
fn tool_header(
    labels: Option<&HashMap<String, String>>,
    previews: Option<&HashMap<String, String>>,
    name: &str,
    args: &serde_json::Value,
    cwd: Option<&std::path::Path>,
) -> ratatui::text::Line<'static> {
    let owned;
    let args = match labels.and_then(|m| m.get(name)) {
        Some(label) => {
            owned = crate::tool_fmt::with_tool_label(args, Some(label));
            &owned
        }
        None => args,
    };
    crate::tool_fmt::format_tool_call_header(
        name,
        args,
        cwd,
        previews.and_then(|m| m.get(name)).map(String::as_str),
    )
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
    wire_model: &str,
    totals: &mut SessionTotals,
    turn_start: std::time::Instant,
    turn_duration_ms: &mut Option<u64>,
    stream_clock: &mut TurnStreamClock,
    tool_labels: Option<&HashMap<String, String>>,
    tool_previews: Option<&HashMap<String, String>>,
) {
    // Single elapsed source — TurnEnd stamps duration once so footer,
    // totals, and persisted entry agree even when TUI + headless paths diverge.
    let elapsed_ms = || turn_start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    // Stream-clock routing: boundary events re-anchor the window without
    // billing (the tool waits, round-trips and retry stalls they close are
    // never token time); every other event is response-side and bills the
    // gap since the previous one. TurnEnd closes the window outright.
    match ev {
        AgentEvent::Start
        | AgentEvent::ToolResult { .. }
        | AgentEvent::StepUsage { .. }
        | AgentEvent::Compacted { .. }
        | AgentEvent::StreamError { .. }
        | AgentEvent::MessageBoundary => stream_clock.open_span(),
        AgentEvent::TurnEnd { .. } => stream_clock.close_span(),
        _ => stream_clock.tick(),
    }
    if let Some(shared) = tui_stream
        && let Ok(mut t) = shared.lock()
    {
        match ev {
            AgentEvent::ThinkingDelta { delta } => {
                t.stream_thinking(delta);
            }
            AgentEvent::TextDelta { delta } => {
                t.stream_text(delta);
            }
            AgentEvent::ToolCallStart { id, name } => {
                t.flush_markdown();
                t.end_thinking();
                t.mark_stream_round_boundary();
                pending_tools.insert(id.clone(), (name.clone(), None));
                // pi `ToolExecutionComponent` appears immediately (partial):
                // viewport-anchored live card, same header family as the
                // final scrollback card. Single scrollback commit stays at
                // `ToolResult`, so this never duplicates.
                let header = crate::tool_fmt::format_live_tool_header(name, "", None);
                t.atomic(|t| {
                    t.upsert_live_tool(id, header, false);
                    t.set_status(Some(&format!("Preparing tool: {name}")));
                });
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
                let preview = args_so_far.split_whitespace().collect::<Vec<_>>().join(" ");
                let preview = crate::repl::format::truncate_chars(&preview, 60);
                // Card + status label repaint as one frame per delta (was two).
                t.atomic(|t| {
                    t.upsert_live_tool(id, header, false);
                    if preview.is_empty() {
                        t.set_status(Some(&format!("Preparing tool: {name}")));
                    } else {
                        t.set_status(Some(&format!("Preparing tool: {name} {preview}")));
                    }
                });
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
                let header = tool_header(tool_labels, tool_previews, &name, args, Some(cwd));
                t.atomic(|t| {
                    t.upsert_live_tool(id, header, true);
                    t.set_status(Some("Working"));
                });
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
                    // One synchronized frame: the live card disappearing and
                    // the scrollback card landing must never be presented
                    // separately (that gap was the visible tool-call flash).
                    t.atomic(|t| {
                        t.remove_live_tool(id);
                        let header = args
                            .as_ref()
                            .map(|a| tool_header(tool_labels, tool_previews, &name, a, Some(cwd)))
                            .unwrap_or_else(|| ratatui::text::Line::from(name.clone()));
                        t.push_tool_box(header, lines);
                    });
                }
            }
            AgentEvent::StepUsage { usage } => {
                t.set_usage(*usage);
                // Prompt-cache bookkeeping: every round's report is one
                // provider request, so it re-arms the footer warmth timer
                // and is checked against the previous request for a
                // re-billed prompt (pi `maybeShowCacheMissNotice`).
                if let Some(miss) = t.note_cache_request(usage, model, wire_model) {
                    // Every miss feeds the `/usage` tally, warned or not.
                    totals.note_miss(&miss);
                    if let Some(notice) = miss.notice() {
                        // Close the open markdown/thinking runs first, or the
                        // row glues onto the streamed tail above it.
                        t.flush_markdown();
                        t.end_thinking();
                        t.mark_stream_round_boundary();
                        t.ensure_gap();
                        t.push_warning(&notice);
                    }
                }
            }
            // Compaction accounting (arXiv:2512.22087 / 2601.16746): the
            // rewrite is visible in the transcript instead of silent.
            AgentEvent::Compacted {
                tokens_before,
                tokens_after,
                messages_before,
                messages_after,
            } => {
                t.flush_markdown();
                t.end_thinking();
                t.mark_stream_round_boundary();
                t.push_dim(format!(
                    "↻ compacted {} → {} tok ({} → {} messages)",
                    crate::repl::fmt_usage(*tokens_before),
                    crate::repl::fmt_usage(*tokens_after),
                    messages_before,
                    messages_after
                ));
            }
            // Reconnecting rides the shimmer status dock (`⬡ Reconnecting…`)
            // like Thinking/Working instead of a static cell; the cause
            // lands as one dim detail row (single notice per burst).
            AgentEvent::StreamError { details, .. } => {
                t.flush_markdown();
                t.end_thinking();
                t.mark_stream_round_boundary();
                t.set_status(Some("Reconnecting"));
                // One row per distinct cause: a retry burst repeating the same
                // 429 is one line, not one per attempt.
                if !details.is_empty() && t.last_retry_detail.as_deref() != Some(details.as_str()) {
                    t.last_retry_detail = Some(details.clone());
                    let trunc = crate::repl::format::truncate_chars(details, 200);
                    super::cron::push_notice(&mut t, "Reconnecting", &trunc, false);
                }
            }
            // A mid-turn injected note (nudge / finished-job follow-up)
            // ended the message that was streaming: close the markdown
            // block so the next deltas open their own paragraph — without
            // this the reply glues onto the previous message verbatim.
            AgentEvent::MessageBoundary => {
                t.flush_markdown();
                t.end_thinking();
                t.mark_stream_round_boundary();
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
                // line (`· N tokens`, reasoning included). The
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
        t.set_turn_stream_ms(stream_clock.streamed_ms().unwrap_or(0));
        let _ = std::io::stdout().flush();
        return;
    }
    if !interactive {
        match ev {
            AgentEvent::TextDelta { delta } => {
                print!("{delta}");
            }
            AgentEvent::ThinkingDelta { delta } => {
                print!("{THINKING_STYLE}{delta}\x1b[0m");
            }
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
                    let owned;
                    let args = match tool_labels.and_then(|m| m.get(name)) {
                        Some(label) => {
                            owned = crate::tool_fmt::with_tool_label(args, Some(label));
                            &owned
                        }
                        None => args,
                    };
                    println!(
                        "\n{}",
                        crate::tool_fmt::format_tool_call_header_plain(
                            name,
                            args,
                            Some(cwd),
                            tool_previews.and_then(|m| m.get(name)).map(String::as_str),
                        )
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
                    eprintln!("\n\x1b[2m⚠ {message}\n  {details}\x1b[0m");
                }
            }
            AgentEvent::Compacted {
                tokens_before,
                tokens_after,
                messages_before,
                messages_after,
            } => {
                eprintln!(
                    "\n\x1b[2m↻ compacted {} → {} tok ({} → {} messages)\x1b[0m",
                    crate::repl::fmt_usage(*tokens_before),
                    crate::repl::fmt_usage(*tokens_after),
                    messages_before,
                    messages_after
                );
            }
            // Two messages, two paragraphs — the composer's markdown
            // flush, written here as a plain blank line.
            AgentEvent::MessageBoundary => {
                print!("\n\n");
            }
            AgentEvent::TurnEnd { usage, .. } => {
                *turn_usage = Some(*usage);
                let ms = elapsed_ms();
                *turn_duration_ms = Some(ms);
                if usage.total() > 0 {
                    totals.add(usage, model, Some(ms));
                    println!(
                        "\n\x1b[2m{}\x1b[0m",
                        turn_footer(usage, model, totals, Some(ms), stream_clock.streamed_ms())
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
        let mut t = shared.lock().expect("tui lock");
        let est = crate::compact::estimate_context_tokens(agent.messages(), None);
        t.seed_estimate_usage(est);
        // The context was just replaced: the next request's prompt is new
        // content, so the pre-compact request is no longer its baseline
        // (pi resets its miss scan on compaction entries for the same
        // reason — a fresh summary is not a re-billed prompt).
        t.reset_cache();
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
            // A failed save retries through repair instead of nagging:
            // `load` heals a torn tail (drops the torn fragment, backs the
            // original up); a pruned/deleted file re-mints via `create`
            // (`create_new` never truncates a live file). Retryable errors
            // are torn tails (`Io(InvalidData)`), tails that parse but fail
            // validation (`Corrupt` — only `load` can tell torn-fixable from
            // interior), and a file lost between turns (`NotFound`). Anything
            // else (real I/O, interior corruption `load` rejects) still warns.
            use crate::session_store::SessionError;
            let retryable = matches!(
                &error,
                SessionError::Io(_) | SessionError::Corrupt { .. } | SessionError::NotFound(_)
            );
            if retryable {
                // `load` heals a torn tail (or no-ops on a healthy file);
                // when the file is gone it still fails, so re-mint it.
                let repaired = state.store.load(&state.session_id).await.is_ok()
                    || remint_missing_session(state).await;
                if repaired {
                    match state
                        .store
                        .append_compaction_replacement(&state.session_id, messages)
                        .await
                    {
                        Ok(()) => {
                            state.full_save_pending = false;
                            return Ok(());
                        }
                        Err(retry_error) => {
                            let warning = save_failure_message("compaction save", &retry_error);
                            log::warn!(target: "gray_session", "{warning}");
                            return Err(warning);
                        }
                    }
                }
            }
            let warning = save_failure_message("compaction save", &error);
            log::warn!(target: "gray_session", "{warning}");
            Err(warning)
        }
    }
}

/// Re-mint a session file lost to prune/delete between turns: re-`create`
/// the header for this state's id and cwd. `create_new` refuses when the
/// file is back (a sibling re-minted it), so this never truncates live
/// history — the caller retries the append either way.
async fn remint_missing_session(state: &SessionState) -> bool {
    use crate::session_store::SessionMeta;
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
    let meta = SessionMeta::new(
        state.session_id.clone(),
        crate::print::now_millis(),
        cwd,
        "reminted",
    )
    .with_origin(crate::session_store::session_origin_from_env());
    match state.store.create(meta).await {
        Ok(_) => true,
        Err(crate::session_store::SessionError::AlreadyExists(_)) => true,
        Err(_) => false,
    }
}

/// Repair-then-retry for one per-turn append: heal a torn tail via `load`,
/// re-mint a pruned file via `create_new`, then retry the single append
/// once. Returns true when the message landed.
async fn repair_and_retry_append(
    state: &SessionState,
    msg: &Message,
    usage: Option<gray_core::event::Usage>,
    duration_ms: Option<u64>,
) -> bool {
    // `load` rejects interior corruption (returns Err, heals nothing),
    // which repair must never touch — that path surfaces once via the
    // normal warning instead.
    let repaired =
        state.store.load(&state.session_id).await.is_ok() || remint_missing_session(state).await;
    if !repaired {
        return false;
    }
    state
        .store
        .append_with_usage_and_duration(&state.session_id, msg, usage, duration_ms)
        .await
        .is_ok()
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
            let detail = format!(
                "{}/{} tokens",
                crate::setup::format_context_length(tokens),
                crate::setup::format_context_length(window)
            );
            super::status::push_compaction_card(
                tui,
                &crate::composer::fmt_elapsed_compact(elapsed.as_secs()),
                &detail,
            );
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
            super::status::push_compaction_card(
                tui,
                &crate::composer::fmt_elapsed_compact(elapsed.as_secs()),
                "after context overflow",
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

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}
