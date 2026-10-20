use super::*;

const TURN_CANCEL_GRACE: std::time::Duration = std::time::Duration::from_secs(4);

pub(crate) const INTERRUPTED_NOTICE: &str = "■ Conversation interrupted";

async fn run_streaming_cancellable(
    agent: &mut Agent,
    msg: Message,
    ctx: ToolContext,
    cancel: &tokio_util::sync::CancellationToken,
    on_event: &mut dyn FnMut(&AgentEvent),
) -> Result<Vec<AgentEvent>, CoreError> {
    let mut seen_thinking = String::new();
    let mut seen_text = String::new();
    let mut sink = |ev: &AgentEvent| {
        match ev {
            AgentEvent::ThinkingDelta { delta } => seen_thinking.push_str(delta),
            AgentEvent::TextDelta { delta } => seen_text.push_str(delta),
            _ => {}
        }
        on_event(ev);
    };
    let mut run_future = Box::pin(agent.run_streaming(msg, ctx, &mut sink));
    tokio::select! {
        res = &mut run_future => res,
        _ = cancel.cancelled() => {
            cancel.cancel();
            let observed = tokio::time::timeout(TURN_CANCEL_GRACE, &mut run_future).await;

            let preempted = observed.is_err();
            drop(run_future);
            if preempted {
                agent.repair_dropped_cancel(&seen_thinking, &seen_text);
            }
            Err(CoreError::Cancelled)
        }
    }
}

fn slow_turn_end_step(step: &str, since: std::time::Instant) {
    let took = since.elapsed();
    if took > std::time::Duration::from_secs(5) {
        log::warn!(target: "gray_turn", "turn end stalled: {step} took {took:?}");
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_prompt_turn(
    prompt_text: String,
    pending_images: &mut Vec<std::path::PathBuf>,
    agent: &mut Option<Agent>,
    config: &mut Config,
    cwd: &std::path::Path,
    tui: &TuiOpt,
    interactive: bool,
    session_state: &mut Option<SessionState>,
    session_totals: &mut SessionTotals,
    pending_command: &mut Option<ReplCommand>,
    pending_history: &mut std::sync::Arc<Vec<Message>>,
    unconfigured: &mut bool,
    resumable: &mut bool,
) -> anyhow::Result<()> {
    super::session::clear_resume_trailer();
    let (shared, _) = if interactive {
        (Some(tui.as_ref().expect("interactive implies tui")), ())
    } else {
        (None, ())
    };
    let tui_stream = shared.as_ref().map(|(s, _)| (*s).clone());
    if agent.is_none() {
        if *unconfigured {
            let bg = tui
                .as_ref()
                .map(|(shared, _)| shared.lock().expect("tui lock").snapshot());
            let result = with_modal_sync(tui.as_ref().map(|(s, _)| s), || {
                crate::setup::run_connect_modal(config, bg.as_ref())
            });
            match result {
                Ok(crate::setup::ConnectOutcome::Connected) => {
                    *unconfigured = false;
                    push_provider_connected(config, tui, None);
                }

                Ok(crate::setup::ConnectOutcome::Removed(_))
                | Ok(crate::setup::ConnectOutcome::Dismissed) => {
                    if let Some((shared, _)) = tui {
                        let mut t = shared.lock().expect("tui lock");

                        t.ensure_gap();
                        let _ = t.draw();
                    }
                    return Ok(());
                }
                Err(e) => {
                    if let Some((shared, _)) = tui {
                        super::notice::push_notice(
                            &mut shared.lock().expect("tui lock"),
                            "Provider error",
                            &e.to_string(),
                            true,
                        );
                    } else {
                        say(None, &format!("provider error: {e}"));
                    }
                    return Ok(());
                }
            }
        }

        if super::session::should_ensure_session_before_build(
            session_state.is_some(),
            config.model.as_deref(),
        ) {
            super::session::ensure_session_state(session_state, config, cwd).await;
        }
        let sid = session_state
            .as_ref()
            .map(|s| s.session_id.as_str().to_string());

        if let Some(s) = &tui_stream {
            s.lock().expect("tui lock").begin_turn("Working");
        }
        let built = build_agent(config, cwd, sid.as_deref()).await;
        match built {
            Ok(built) => {
                if !pending_history.is_empty() {
                    *agent =
                        Some(built.with_messages(super::take_pending_history(pending_history)));
                } else {
                    *agent = Some(built);
                }
            }
            Err(e) => {
                if tui_stream.is_some() {
                    say(tui_stream.as_ref(), &format!("{e}"));
                    if let Some(s) = &tui_stream {
                        let mut t = s.lock().unwrap_or_else(|e| e.into_inner());
                        t.set_status(None);
                        t.is_task_running = false;

                        t.resume_cache();
                        let _ = t.draw();
                    }
                } else {
                    say(None, &format!("{e}"));
                }
                return Ok(());
            }
        }
    }

    if let Some(ag) = agent.as_mut() {
        let steer_tui = tui_stream.clone();
        let steer_sid = session_state
            .as_ref()
            .map(|s| s.session_id.as_str().to_string());
        ag.set_steer(std::sync::Arc::new(move || {
            crate::ide::steer_source()
                .or_else(|| steer_queued_input(steer_tui.as_ref()))
                .or_else(|| steer_direct_send(steer_tui.as_ref(), steer_sid.as_deref()))
        }));
        let out_tui = tui_stream.clone();
        ag.set_tool_output_hook(Some(std::sync::Arc::new(move |id: &str, chunk: &[u8]| {
            if let Some(shared) = out_tui.as_ref()
                && let Ok(mut t) = shared.lock()
            {
                t.push_live_tool_output(id, chunk);
            }
        })));
    }
    let agent = agent.as_mut().expect("agent built above");
    let cancel = tokio_util::sync::CancellationToken::new();
    *TURN_STATE.lock().unwrap_or_else(|e| e.into_inner()) = Some(cancel.clone());

    let _turn_in_flight = super::mark_turn_in_flight();
    super::session::ensure_session_state(session_state, config, cwd).await;
    let ctx = ToolContext {
        cwd: cwd.to_path_buf(),
        cancel: cancel.clone(),
        session_id: session_state
            .as_ref()
            .map(|s| s.session_id.as_str().to_string()),
    };
    let mut images = std::mem::take(&mut *pending_images);

    let link_scan: String = {
        let mut s = String::with_capacity(prompt_text.len());
        let mut last = 0usize;
        for (range, _) in super::attachments::attachment_markers(&prompt_text) {
            s.push_str(&prompt_text[last..range.start]);
            last = range.end;
        }
        s.push_str(&prompt_text[last..]);
        s
    };
    for p in super::attachments::extract_inline_image_paths(&link_scan, cwd) {
        if !images.contains(&p) {
            images.push(p);
        }
    }
    let user_msg = build_user_message_with_attachments(&prompt_text, &images);
    let user_msg_for_retry = user_msg.clone();
    let mut initial_count = agent.messages().len();

    let watch_cancel = cancel.clone();
    let watch_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watcher_stopped = watch_stop.clone();
    let watcher_tui = tui_stream.clone();
    let cwd_for_watcher = cwd.to_path_buf();
    let turn_locals = tui_stream.as_ref().map(|shared| {
        let mut local_config = config.clone();
        local_config.api_key = None;
        key_watcher::TurnLocalCommands {
            tui: shared.clone(),
            cwd: cwd.to_path_buf(),
            session_id: ctx.session_id.clone(),
            config: local_config,
            totals: session_totals.clone(),
            executor: Some(agent.executor_handle()),
            hooks: agent.hooks().to_vec(),
        }
    });
    let key_watcher = if crate::ide::tap_active() {
        // A host (the IDE) cancels via cancel_turn() and has no terminal —
        // watching crossterm keys would write enhancement escapes to stdout
        // and let keys in an attached console interrupt the turn.
        tokio::task::spawn(async {})
    } else {
        key_watcher::spawn_key_watcher_with_typing(
            watch_cancel,
            watcher_stopped,
            watcher_tui,
            cwd_for_watcher,
            turn_locals,
        )
    };

    {
        let latest = tui
            .as_ref()
            .and_then(|(s, _)| s.lock().ok().and_then(|t| t.latest_usage));
        tokio::select! {
            _ = cancel.cancelled() => {}
            _ = maybe_threshold_compact(
                agent,
                config,
                &mut *session_state,
                cwd,
                tui.as_ref().map(|(s, _)| s),
                latest,
                &mut initial_count,
            ) => {}
        }
    }

    if agent.prefix_rewrite_ok()
        && let Some(s) = &tui_stream
        && s.lock().expect("tui lock").cache_is_cold()
    {
        agent.mask_stale_tool_output();
    }

    if let Some(s) = &tui_stream {
        s.lock().expect("tui lock").begin_turn("Working");
    }

    let mut pending_tools: HashMap<String, (String, Option<serde_json::Value>)> = HashMap::new();
    let mut turn_usage: Option<gray_core::event::Usage> = None;
    let turn_start = std::time::Instant::now();
    let mut turn_duration_ms: Option<u64> = None;

    let mut stream_clock = super::session::TurnStreamClock::default();
    let history_revision = agent.history_revision();

    super::session::ensure_session_state(&mut *session_state, config, cwd).await;

    if let Some(sid) = session_state.as_ref().map(|s| s.session_id.as_str()) {
        agent.repin_session_id(sid);
    }
    let checkpointed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(initial_count));
    let checkpoint_failed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Some(state) = session_state.as_ref().filter(|s| !s.full_save_pending) {
        let store = std::sync::Arc::new(crate::session_store::JsonlSessionStore::new(
            state.store.root_dir(),
        ));
        let sid = state.session_id.clone();
        let (done, failed) = (checkpointed.clone(), checkpoint_failed.clone());
        agent.set_checkpoint(Some(std::sync::Arc::new(
            move |msgs: &[Message], rev: u64| {
                use std::sync::atomic::Ordering::Relaxed;
                let from = done.load(Relaxed);
                let fresh =
                    if rev == history_revision && !failed.load(Relaxed) && from <= msgs.len() {
                        msgs[from..].to_vec()
                    } else {
                        Vec::new()
                    };
                let (store, sid, done, failed) =
                    (store.clone(), sid.clone(), done.clone(), failed.clone());
                Box::pin(async move {
                    for m in &fresh {
                        if store.append(&sid, m).await.is_err() {
                            failed.store(true, Relaxed);
                            return;
                        }
                        done.fetch_add(1, Relaxed);
                    }
                })
            },
        )));
    }

    let turn_labels: std::collections::HashMap<String, String> = agent
        .tool_defs()
        .iter()
        .filter_map(|t| {
            t.label
                .as_ref()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .map(|l| (t.name.clone(), l.to_string()))
        })
        .collect();
    let turn_previews: std::collections::HashMap<String, String> = agent
        .tool_defs()
        .iter()
        .filter_map(|t| {
            t.preview
                .as_ref()
                .map(|x| x.trim())
                .filter(|x| !x.is_empty())
                .map(|x| (t.name.clone(), x.to_string()))
        })
        .collect();

    let wire_model = crate::setup::wire_model_for(config)
        .unwrap_or_else(|| config.model.clone().unwrap_or_default());
    let mut run_result = {
        let mut on_event = |ev: &AgentEvent| {
            crate::ide::tap_agent_event(ev);
            dispatch_agent_event(
                ev,
                tui_stream.as_ref(),
                interactive,
                &mut pending_tools,
                &mut turn_usage,
                cwd,
                config.model.as_deref().unwrap_or(""),
                &wire_model,
                &mut *session_totals,
                turn_start,
                &mut turn_duration_ms,
                &mut stream_clock,
                Some(&turn_labels),
                Some(&turn_previews),
            );
        };
        run_streaming_cancellable(agent, user_msg, ctx, &cancel, &mut on_event).await
    };

    let mut retry_msg = user_msg_for_retry.clone();
    let recover = match &run_result {
        Err(e) => {
            tokio::select! {
                _ = cancel.cancelled() => false,
                r = maybe_overflow_compact(
                    agent,
                    config,
                    &mut *session_state,
                    cwd,
                    tui.as_ref().map(|(s, _)| s),
                    &mut initial_count,
                    e,
                ) => r,
            }
        }
        Ok(evs)
            if evs.iter().rev().any(|e| {
                matches!(
                    e,
                    AgentEvent::TurnEnd {
                        stop_reason: gray_core::event::StopReason::MaxTokens,
                        ..
                    }
                )
            }) =>
        {
            let recovered = tokio::select! {
                _ = cancel.cancelled() => false,
                r = super::session::recovery_compact(
                    agent,
                    config,
                    &mut *session_state,
                    cwd,
                    tui.as_ref().map(|(s, _)| s),
                    &mut initial_count,
                    "after truncated response",
                ) => r,
            };
            if recovered {
                let mut msgs = agent.messages().to_vec();
                if msgs
                    .last()
                    .is_some_and(|m| m.role == gray_core::message::Role::Assistant)
                {
                    msgs.pop();
                    agent.set_messages(msgs);
                }
                retry_msg = Message::user_injected(
                    "the previous response hit the output token limit before completing; context was compacted — continue where you left off",
                );
            }
            recovered
        }
        _ => false,
    };
    if recover {
        pending_tools.clear();
        if let Some(t) = &tui_stream {
            t.lock().expect("tui lock").clear_live_tools();
        }
        let ctx2 = ToolContext {
            cwd: cwd.to_path_buf(),
            cancel: cancel.clone(),
            session_id: session_state
                .as_ref()
                .map(|s| s.session_id.as_str().to_string()),
        };
        let mut on_event2 = |ev: &AgentEvent| {
            crate::ide::tap_agent_event(ev);
            dispatch_agent_event(
                ev,
                tui_stream.as_ref(),
                interactive,
                &mut pending_tools,
                &mut turn_usage,
                cwd,
                config.model.as_deref().unwrap_or(""),
                &wire_model,
                &mut *session_totals,
                turn_start,
                &mut turn_duration_ms,
                &mut stream_clock,
                Some(&turn_labels),
                Some(&turn_previews),
            );
        };
        run_result =
            run_streaming_cancellable(agent, retry_msg, ctx2, &cancel, &mut on_event2).await;
    }
    TURN_STATE.lock().unwrap_or_else(|e| e.into_inner()).take();
    agent.set_checkpoint(None);

    if checkpoint_failed.load(std::sync::atomic::Ordering::Relaxed) {
        if let Some(state) = session_state.as_mut() {
            state.full_save_pending = true;
        }
    } else {
        initial_count = initial_count.max(checkpointed.load(std::sync::atomic::Ordering::Relaxed));
    }

    watch_stop.store(true, std::sync::atomic::Ordering::Relaxed);
    // A run that ended but left "Working…" up for hours had nothing in the
    // log after "agent run end"; these name which step held the turn open.
    let run_returned = std::time::Instant::now();
    let _ = key_watcher.await;
    slow_turn_end_step("key watcher stop", run_returned);
    if turn_duration_ms.is_none() {
        turn_duration_ms = Some(turn_start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64);
    }

    if agent.history_revision() != history_revision {
        super::session::persist_compaction_tail(
            agent,
            config,
            session_state,
            cwd,
            tui.as_ref().map(|(state, _)| state),
        )
        .await;
        initial_count = agent.messages().len();
    }
    let mut auth_relogin: Option<String> = None;
    match run_result {
        Ok(_) => {
            *resumable = false;
            persist_turn_messages(
                &mut *session_state,
                agent,
                config,
                cwd,
                initial_count,
                turn_usage,
                turn_duration_ms,
            )
            .await;
            if let Some(state) = &*session_state {
                crate::auto_title::maybe_start(state, &*agent, initial_count);
            }
        }
        Err(CoreError::Cancelled) => {
            *resumable = true;
            persist_turn_messages(
                &mut *session_state,
                agent,
                config,
                cwd,
                initial_count,
                turn_usage,
                turn_duration_ms,
            )
            .await;
            if interactive {
                if let Some((shared, _)) = tui {
                    let mut t = shared.lock().expect("tui lock");
                    t.end_thinking();
                    t.discard_held_punctuation();
                    t.flush_markdown();

                    t.ensure_gap();
                    t.push_dim(INTERRUPTED_NOTICE.to_string());
                }
            } else {
                say(
                    None,
                    if crate::ide::sink_active() {
                        "(interrupted)"
                    } else {
                        "(interrupted — press Enter to continue)"
                    },
                );
            }
        }
        Err(e) => {
            *resumable = true;
            persist_turn_messages(
                &mut *session_state,
                agent,
                config,
                cwd,
                initial_count,
                turn_usage,
                turn_duration_ms,
            )
            .await;
            let msg = format_core_error(&e, &config.base_url);

            let previous_key = config.api_key.clone();
            let mut reloaded = matches!(e.class(), gray_core::error::CoreError::Auth(_))
                && crate::setup::adopt_saved_key(config);
            if reloaded {
                let sid = session_state.as_ref().map(|s| s.session_id.as_str());
                match build_agent(config, cwd, sid).await {
                    Ok(built) => {
                        let history = agent.messages().to_vec();
                        *agent = built.with_messages(history);
                    }

                    Err(_) => {
                        config.api_key = previous_key;
                        reloaded = false;
                    }
                }
            }

            if !reloaded
                && interactive
                && tui.is_some()
                && matches!(e.class(), gray_core::error::CoreError::Auth(_))
            {
                auth_relogin = crate::setup::active_connect_id(config);
            }
            const RELOADED: &str =
                "the API key saved for this provider changed on disk; reloaded it";
            if interactive {
                if let Some((shared, _)) = tui {
                    let mut t = shared.lock().expect("tui lock");
                    t.end_thinking();
                    t.flush_markdown();
                    t.ensure_gap();

                    t.push_error(&msg);
                    if reloaded {
                        super::notice::push_notice(&mut t, "API key reloaded", RELOADED, false);
                    }
                }
            } else if crate::ide::sink_active() {
                say_err(None, &msg);
                if reloaded {
                    say_err(None, RELOADED);
                }
            } else {
                say_err(None, &format!("{msg}\n(press Enter to continue)"));
                if reloaded {
                    say_err(None, RELOADED);
                }
            }
        }
    }
    slow_turn_end_step("run end to end_turn", run_returned);
    if let Some(s) = &tui_stream {
        s.lock()
            .expect("tui lock")
            .end_turn(stream_clock.streamed_ms().unwrap_or(0));
    }

    if interactive && let Some((shared, _)) = tui {
        let mut t = shared.lock().expect("tui lock");
        if let Some(text) = t.local_command.take() {
            let parsed = parse_command(&text);
            if !parsed.runs_local() && !matches!(parsed, ReplCommand::Skill(_)) {
                t.push_user_prompt(&text, &[], false);
            }
            drop(t);
            *pending_command = Some(expand_skill_command(parsed, cwd, Some(shared), true));
        } else if let Some((qtext, qimages)) = t.queued_inputs.pop_front() {
            let parsed = parse_command(&qtext);
            if !parsed.runs_local() {
                t.push_user_prompt(&qtext, &qimages, !qtext.starts_with('/'));
            }
            drop(t);
            *pending_command = Some(expand_skill_command(parsed, cwd, Some(shared), false));
            *pending_images = qimages;
        }
    }
    if pending_command.is_none()
        && let Some(id) = auth_relogin
    {
        *pending_command = Some(ReplCommand::ProviderLogin(id));
    }
    Ok(())
}

fn take_steerable(
    queued: &mut std::collections::VecDeque<(String, Vec<std::path::PathBuf>)>,
) -> Option<String> {
    let (text, images) = queued.front()?;
    if !images.is_empty() || text.starts_with('/') {
        return None;
    }
    queued.pop_front().map(|(text, _)| text)
}

fn steer_direct_send(
    tui: Option<&crate::composer::SharedTui>,
    sid: Option<&str>,
) -> Option<String> {
    let home = crate::setup::gray_home().ok()?;
    let sid = sid?;
    let msg = crate::session_send::claim_steerable(&home, sid);
    if let Some(shared) = tui {
        let pending: Vec<(String, Vec<std::path::PathBuf>)> =
            crate::session_send::peek_sends(&home, sid)
                .into_iter()
                .map(|m| {
                    (
                        crate::session_send::prompt_for(&m.from, &m.text),
                        m.attachments,
                    )
                })
                .collect();
        let mut t = shared.lock().expect("tui lock");
        if t.queued_sends != pending {
            t.queued_sends = pending;
            let _ = t.draw();
        }
        if let Some(msg) = &msg {
            let text = crate::session_send::prompt_for(&msg.from, &msg.text);
            t.flush_markdown();
            t.end_thinking();
            t.push_user_prompt(&text, &[], true);
            let _ = t.draw();
        }
    }
    msg.map(|m| crate::session_send::prompt_for(&m.from, &m.text))
}

fn steer_queued_input(tui: Option<&crate::composer::SharedTui>) -> Option<String> {
    let mut t = tui?.lock().expect("tui lock");
    let text = take_steerable(&mut t.queued_inputs)?;
    t.flush_markdown();
    t.end_thinking();
    t.push_user_prompt(&text, &[], true);
    let _ = t.draw();
    Some(text)
}

#[path = "prompt_turn_tests.rs"]
#[cfg(test)]
mod tests;
