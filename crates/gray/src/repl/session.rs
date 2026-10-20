use super::*;

static EXIT_SESSION: StdMutex<Option<String>> = StdMutex::new(None);

static RESUME_TRAILER: StdMutex<Option<String>> = StdMutex::new(None);

pub(crate) fn note_resume_trailer(id: &str) {
    *RESUME_TRAILER.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.to_string());
}

pub(crate) fn clear_resume_trailer() {
    *RESUME_TRAILER.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn resume_trailer_fresh(session_state: &Option<SessionState>) -> bool {
    let Some(state) = session_state else {
        return false;
    };
    RESUME_TRAILER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_deref()
        == Some(state.session_id.as_str())
}

static EXIT_HINT_PRINTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn claim_exit_hint() -> bool {
    !EXIT_HINT_PRINTED.swap(true, std::sync::atomic::Ordering::Relaxed)
}

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

pub(crate) fn current_session_line(session_state: &Option<SessionState>) -> Option<String> {
    session_state.as_ref().map(|s| {
        format!(
            "session {} · {}",
            s.session_id,
            exit_hint_line(s.session_id.as_str(), false)
        )
    })
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
        say(
            None,
            &exit_hint_line(state.session_id.as_str(), std::io::stdout().is_terminal()),
        );
        let _ = std::io::stdout().flush();
    }
}

#[allow(clippy::too_many_arguments)]
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

        if let Some(shared) = &tui {
            let hint = crate::resume::startup_interrupted_hint(cwd).await;
            shared.lock().expect("tui lock").set_home_notice(hint);
        }
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
            say(None, &msg);
        }
        return;
    }
    if args.target.is_none() {
        let at_home = tui.is_some_and(|s| s.lock().expect("tui lock").is_home());
        if !at_home
            && !resume_trailer_fresh(session_state)
            && let Some(line) = current_session_line(session_state)
        {
            say(tui, &line);
        }
    }
    let bg = tui.as_ref().map(|s| s.lock().expect("tui lock").snapshot());

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
                            super::notice::push_notice(
                                &mut shared.lock().expect("tui lock"),
                                "Resume failed",
                                &msg,
                                true,
                            );
                        } else {
                            say(None, &msg);
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
                    super::notice::push_notice(
                        &mut shared.lock().expect("tui lock"),
                        "Sessions",
                        msg,
                        false,
                    );
                } else {
                    say(None, msg);
                }
                return;
            }
        }
    } else if tui.is_none() {
        let Some(root) = default_root() else {
            return;
        };
        let store = JsonlSessionStore::new(root);
        let summaries = crate::resume::recent_summaries(&store, args.all).await;
        if summaries.is_empty() {
            say(
                None,
                "no saved sessions in this directory (try /sessions --all or --all)",
            );
        } else {
            for s in &summaries {
                say(None, &crate::resume::format_summary_row(s));
            }
        }
        return;
    } else {
        let result = with_modal(
            tui,
            crate::resume::run_resume_picker(
                args.all,
                bg.as_ref(),
                session_state.as_ref().map(|s| s.session_id.clone()),
            ),
        )
        .await;
        match result {
            Ok(Some(id)) => Some(id),
            Ok(None) => {
                if let Some(shared) = &tui {
                    shared.lock().expect("tui lock").ensure_gap();
                }
                return;
            }
            Err(e) => {
                if let Some(shared) = &tui {
                    super::notice::push_notice(
                        &mut shared.lock().expect("tui lock"),
                        "Resume picker failed",
                        &e.to_string(),
                        true,
                    );
                } else {
                    say(None, &format!("resume picker error: {e}"));
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
            let n = entries.len();

            crate::setup::canonicalize_adopted_model(config);
            let eff_model = crate::resume::effective_session_model(
                config.model.as_deref(),
                meta.model.as_str(),
            );
            let mut build_config = config.clone();
            build_config.model = eff_model.clone();
            crate::setup::canonicalize_adopted_model(&mut build_config);
            let model = build_config.model.clone().unwrap_or_default();
            let model = model.as_str();

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
                    *totals = SessionTotals::from_entries(&entries, model);
                    let shared_history = std::sync::Arc::new(
                        entries
                            .into_iter()
                            .map(|e| e.message)
                            .collect::<Vec<Message>>(),
                    );
                    *agent = Some(built.with_messages((*shared_history).clone()));

                    if let Some(ledger) = gray_plugin::builder::current_file_ledger() {
                        ledger.clear();
                    }
                    *session_state = Some(SessionState {
                        full_save_pending: false,
                        session_id: sid.clone(),
                        store,
                        _open_guard: Some(open_guard),
                    });
                    export_session_env(sid.as_str());
                    if let Some(shared) = &tui {
                        let mut t = shared.lock().expect("tui lock");
                        let (tool_labels, tool_previews) = tool_display_maps(
                            agent.as_ref().map(|a| a.tool_defs()).unwrap_or_default(),
                        );
                        note_resume_trailer(sid.as_str());
                        t.replay_session_history_with_tools(
                            shared_history,
                            cwd,
                            crate::tool_fmt::ToolDisplay {
                                labels: Some(&tool_labels),
                                previews: Some(&tool_previews),
                            },
                            vec![format!(
                                "{} Resumed session {} ({n} messages)",
                                crate::tui::glyph("\u{2b22}", "#"),
                                sid.as_str()
                            )],
                        );
                        t.ensure_gap();

                        if !model.is_empty() {
                            t.set_model(model.to_string());
                            t.set_model_label(crate::setup::composite_label_for(config));
                            t.set_model_label_parts(crate::setup::composite_label_parts_for(
                                config,
                            ));
                        }

                        if let Some(eff) = &build_config.thinking_effort {
                            t.set_thinking_effort(crate::setup::effort_chip(eff, &build_config));
                            t.set_hide_thinking(build_config.reasoning_hidden());
                        }
                        t.ensure_gap();
                        let _ = t.draw();
                    } else {
                        say(
                            None,
                            &format!(
                                "\x1b[2m{} Resumed session {} ({n} messages)\x1b[0m",
                                crate::tui::glyph("\u{2b22}", "#"),
                                sid.as_str()
                            ),
                        );
                    }
                }
                Err(e) => {
                    let msg = format!("could not resume (no provider): {e}");
                    if let Some(shared) = &tui {
                        super::notice::push_notice(
                            &mut shared.lock().expect("tui lock"),
                            "Resume failed",
                            &msg,
                            true,
                        );
                    } else {
                        say(None, &msg);
                    }
                }
            }
        }
        Err(e) => {
            let msg = format!("could not resume session {}: {e}", sid.as_str());
            if let Some(shared) = &tui {
                super::notice::push_notice(
                    &mut shared.lock().expect("tui lock"),
                    "Resume failed",
                    &msg,
                    true,
                );
            } else {
                say(None, &msg);
            }
        }
    }
}

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

pub(crate) fn should_ensure_session_before_build(has_session: bool, model: Option<&str>) -> bool {
    !has_session && model.is_some_and(|m| !m.is_empty())
}

pub(crate) fn export_session_env(sid: &str) {
    unsafe { std::env::set_var("GRAY_SESSION_ID", sid) };
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
    if let Some(sid) = session_state.as_ref().map(|s| s.session_id.as_str()) {
        export_session_env(sid);
    }
    remember_exit_session(session_state.as_ref().map(|s| s.session_id.as_str()));
}

#[derive(Debug, Default)]
pub(crate) struct TurnStreamClock {
    anchor: Option<std::time::Instant>,

    streamed: std::time::Duration,
}

impl TurnStreamClock {
    pub(crate) fn tick(&mut self) {
        self.tick_at(std::time::Instant::now());
    }

    fn tick_at(&mut self, now: std::time::Instant) {
        if let Some(prev) = self.anchor.replace(now) {
            self.streamed = self.streamed.saturating_add(now.duration_since(prev));
        }
    }

    pub(crate) fn open_span(&mut self) {
        self.open_span_at(std::time::Instant::now());
    }

    fn open_span_at(&mut self, now: std::time::Instant) {
        self.anchor = Some(now);
    }

    pub(crate) fn close_span(&mut self) {
        self.anchor = None;
    }

    pub(crate) fn streamed_ms(&self) -> Option<u64> {
        let ms = u64::try_from(self.streamed.as_millis()).unwrap_or(u64::MAX);
        (ms > 0).then_some(ms)
    }
}

fn tool_header(
    display: crate::tool_fmt::ToolDisplay<'_>,
    name: &str,
    args: &serde_json::Value,
    cwd: Option<&std::path::Path>,
) -> Vec<ratatui::text::Line<'static>> {
    crate::tool_fmt::format_tool_card_lines(
        name,
        args,
        cwd,
        display.label(name),
        display.preview(name),
    )
}

pub(crate) fn tool_display_maps(
    defs: &[gray_core::message::ToolDef],
) -> (HashMap<String, String>, HashMap<String, String>) {
    let mut labels = HashMap::new();
    let mut previews = HashMap::new();
    for t in defs {
        if let Some(label) = t.label.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
            labels.insert(t.name.clone(), label.to_string());
        }
        if let Some(preview) = t
            .preview
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            previews.insert(t.name.clone(), preview.to_string());
        }
    }
    (labels, previews)
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
    let elapsed_ms = || turn_start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;

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
        let display = crate::tool_fmt::ToolDisplay {
            labels: tool_labels,
            previews: tool_previews,
        };
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

                let header = crate::tool_fmt::format_live_tool_header(
                    name,
                    "",
                    Some(cwd),
                    display.label(name),
                    display.preview(name),
                );
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
                pending_tools.insert(id.clone(), (name.clone(), None));
                let header = crate::tool_fmt::format_live_tool_header(
                    name,
                    args_so_far,
                    Some(cwd),
                    display.label(name),
                    display.preview(name),
                );
                let preview = args_so_far.split_whitespace().collect::<Vec<_>>().join(" ");
                let preview = crate::repl::format::truncate_chars(&preview, 60);

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

                let header = tool_header(display, &name, args, Some(cwd));
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
                let (name, args) = pending_tools
                    .remove(id)
                    .map(|(n, a)| (if n.is_empty() { "tool".to_string() } else { n }, a))
                    .unwrap_or_else(|| ("tool".to_string(), None));
                {
                    let mut lines = crate::tool_fmt::format_tool_result_lines_with_context(
                        &name,
                        args.as_ref(),
                        output,
                        *is_error,
                        Some(cwd),
                    );
                    if !*is_error
                        && let Some(rows) = crate::composer::transcript::rows::viewed_image_lines(
                            &name,
                            args.as_ref(),
                            cwd,
                            t.width(),
                        )
                    {
                        lines.extend(rows);
                    }

                    t.atomic(|t| {
                        t.remove_live_tool(id);
                        let header = match args.as_ref() {
                            Some(a) => tool_header(display, &name, a, Some(cwd)),
                            None => crate::tool_fmt::format_live_tool_header(
                                &name,
                                "",
                                Some(cwd),
                                display.label(&name),
                                display.preview(&name),
                            ),
                        };
                        t.push_tool_box_lines(header, lines);
                    });
                }
            }
            AgentEvent::StepUsage { usage } => {
                t.set_usage(*usage);

                if let Some(miss) = t.note_cache_request(usage, model, wire_model) {
                    totals.note_miss(&miss);
                    if let Some(notice) = miss.notice() {
                        t.flush_markdown();
                        t.end_thinking();
                        t.mark_stream_round_boundary();
                        t.ensure_gap();
                        t.push_warning(&notice);
                    }
                }
            }

            AgentEvent::Compacted {
                tokens_before,
                tokens_after,
                messages_before,
                messages_after,
                reason,
            } => {
                t.flush_markdown();
                t.end_thinking();
                t.mark_stream_round_boundary();
                t.push_dim(format!(
                    "↻ compacted {} → {} tok ({} → {} messages){}",
                    crate::repl::fmt_usage(*tokens_before),
                    crate::repl::fmt_usage(*tokens_after),
                    messages_before,
                    messages_after,
                    crate::compact::compact_reason_note(*reason)
                ));
            }

            AgentEvent::StreamError { details, .. } => {
                t.flush_markdown();
                t.end_thinking();
                t.mark_stream_round_boundary();
                t.set_status(Some("Reconnecting"));
                t.note_cache_reconnect();

                if !details.is_empty() && t.last_retry_detail.as_deref() != Some(details.as_str()) {
                    t.last_retry_detail = Some(details.clone());
                    let trunc = crate::repl::format::truncate_chars(details, 200);
                    super::notice::push_notice(&mut t, "Reconnecting", trunc, false);
                }
            }

            AgentEvent::MessageBoundary => {
                t.flush_markdown();
                t.end_thinking();
                t.mark_stream_round_boundary();
            }
            AgentEvent::TurnEnd { usage, .. } => {
                *turn_usage = Some(*usage);
                let ms = elapsed_ms();
                *turn_duration_ms = Some(ms);

                t.clear_live_tools();
                t.end_thinking();

                if usage.total() > 0 {
                    totals.add(usage, model, Some(ms));
                    t.set_turn_billed(usage.output_tokens);
                }
            }
            _ => {}
        }
        t.set_turn_stream_ms(stream_clock.streamed_ms().unwrap_or(0));
        let _ = std::io::stdout().flush();
        return;
    }
    if !interactive {
        if crate::ide::tap_active() {
            if let AgentEvent::TurnEnd { usage, .. } = ev {
                *turn_usage = Some(*usage);
                let ms = elapsed_ms();
                *turn_duration_ms = Some(ms);
                if usage.total() > 0 {
                    totals.add(usage, model, Some(ms));
                }
            }
            return;
        }
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
                        crate::tool_fmt::format_tool_call_lines_plain(
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
                let mark = crate::tui::glyph("⚠", "!");
                if details.is_empty() {
                    eprintln!("\n\x1b[2m{mark} {message}\x1b[0m");
                } else {
                    eprintln!("\n\x1b[2m{mark} {message}\n  {details}\x1b[0m");
                }
            }
            AgentEvent::Compacted {
                tokens_before,
                tokens_after,
                messages_before,
                messages_after,
                reason,
            } => {
                eprintln!(
                    "\n\x1b[2m↻ compacted {} → {} tok ({} → {} messages){}\x1b[0m",
                    crate::repl::fmt_usage(*tokens_before),
                    crate::repl::fmt_usage(*tokens_after),
                    messages_before,
                    messages_after,
                    crate::compact::compact_reason_note(*reason)
                );
            }

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

static AUTO_COMPACT_ENV_ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();

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
            use crate::session_store::SessionError;
            let retryable = matches!(
                &error,
                SessionError::Io(_) | SessionError::Corrupt { .. } | SessionError::NotFound(_)
            );
            if retryable {
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

async fn repair_and_retry_append(
    state: &SessionState,
    msg: &Message,
    usage: Option<gray_core::event::Usage>,
    duration_ms: Option<u64>,
) -> bool {
    if !remint_missing_session(state).await {
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
    let settings = crate::compact::compaction_settings_for(
        window,
        agent.compact_ceiling(),
        agent.price_cliff(),
    );
    if !crate::compact::should_compact(tokens, window, &settings) {
        return;
    }
    let reason = crate::compact::binding_reason(tokens, &settings);

    let compaction_id = format!("auto-{}", uuid::Uuid::new_v4().as_simple());
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
            let detail = format!(
                "{}/{} tokens{}",
                crate::setup::format_context_length(tokens),
                crate::setup::format_context_length(window),
                reason
                    .map(crate::compact::compact_reason_note)
                    .unwrap_or_default()
            );
            super::status::push_compaction_card(
                tui,
                &crate::composer::fmt_elapsed_compact(elapsed.as_secs()),
                &detail,
            );

            persist_compaction_tail(agent, config, session_state, cwd, tui).await;
            *initial_count = agent.messages().len();
        }
        Ok(false) => {
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
    recovery_compact(
        agent,
        config,
        session_state,
        cwd,
        tui,
        initial_count,
        "after context overflow",
    )
    .await
}

pub(crate) async fn recovery_compact(
    agent: &mut Agent,
    config: &Config,
    session_state: &mut Option<SessionState>,
    cwd: &Path,
    tui: Option<&crate::composer::SharedTui>,
    initial_count: &mut usize,
    detail: &str,
) -> bool {
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
                detail,
            );

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
