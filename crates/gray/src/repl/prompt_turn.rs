//! REPL prompt turn (split from `repl`).

use super::*;

/// One streaming attempt with cooperative cancel: on Ctrl-C, `ctx` shares
/// the token so the run is already signalled — give it a bounded window to
/// observe cancel and execute its own cleanup/transcript-repair paths
/// (partial-text salvage, turn_end) before the drop preempts it.
async fn run_streaming_cancellable(
    agent: &mut Agent,
    msg: Message,
    ctx: ToolContext,
    cancel: &tokio_util::sync::CancellationToken,
    on_event: &mut dyn FnMut(&AgentEvent),
) -> Result<Vec<AgentEvent>, CoreError> {
    let mut run_future = Box::pin(agent.run_streaming(msg, ctx, on_event));
    tokio::select! {
        res = &mut run_future => res,
        _ = cancel.cancelled() => {
            cancel.cancel();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), &mut run_future).await;
            // Fallback: a run that ignored cancel is still pending; the drop
            // preempts it. The entry guards blank input and owns no
            // cross-turn state, so no external abort is needed.
            drop(run_future);
            Err(CoreError::Cancelled)
        }
    }
}

#[allow(clippy::too_many_arguments)]
// Mechanical split of `run_repl_mode`: params are the loop state the turn borrows.
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
    pending_history: &mut Vec<Message>,
    unconfigured: &mut bool,
) -> anyhow::Result<()> {
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
                Ok(true) => {
                    *unconfigured = false;
                    push_provider_connected(config, tui, None);
                }
                Ok(false) => {
                    if let Some((shared, _)) = tui {
                        let mut t = shared.lock().expect("tui lock");
                        // Dismissed picker: gap so the card doesn't jam the input box.
                        t.ensure_gap(1);
                        let _ = t.draw();
                    }
                    return Ok(());
                }
                Err(e) => {
                    if let Some((shared, _)) = tui {
                        shared
                            .lock()
                            .expect("tui lock")
                            .push_dim(format!("└ provider error: {e}"));
                    } else {
                        println!("provider error: {e}");
                    }
                    return Ok(());
                }
            }
        }
        // Fresh sessions built the provider with session None (per-process
        // fallback cache shard) and minted the real SessionId only after the
        // first turn — the reused agent then sat on the fallback shard all
        // session. Ensure the real sid BEFORE the first build so the provider
        // gets it from turn one. Gated on model so the no-model REPL still
        // opens session-free (lazy build).
        if super::session::should_ensure_session_before_build(
            session_state.is_some(),
            config.model.as_deref(),
        ) {
            super::session::ensure_session_state(session_state, config, cwd).await;
        }
        let sid = session_state
            .as_ref()
            .map(|s| s.session_id.as_str().to_string());
        // Status on BEFORE the first build: skill discovery + provider setup
        // can take ~1s with nothing else painting (ticker skips while status
        // is unset), which left the fresh viewport blank after the user card
        // shifted it. Ticker keeps the elapsed ticking from here.
        if let Some(s) = &tui_stream {
            s.lock().expect("tui lock").begin_turn("Working");
        }
        let built = build_agent(config, cwd, sid.as_deref()).await;
        match built {
            Ok(built) => {
                if !pending_history.is_empty() {
                    *agent = Some(built.with_messages(std::mem::take(&mut *pending_history)));
                } else {
                    *agent = Some(built);
                }
            }
            Err(e) => {
                if tui_stream.is_some() {
                    // Live viewport: never raw println! over it (ghost input
                    // on the next draw) — render through the composer.
                    say(tui_stream.as_ref(), &format!("{e}"));
                    if let Some(s) = &tui_stream {
                        let mut t = s.lock().unwrap_or_else(|e| e.into_inner());
                        t.set_status(None);
                        t.is_task_running = false;
                        let _ = t.draw();
                    }
                } else {
                    println!("{e}");
                }
                return Ok(());
            }
        }
    }
    let agent = agent.as_mut().expect("agent built above");
    let cancel = tokio_util::sync::CancellationToken::new();
    *TURN_STATE.lock().unwrap_or_else(|e| e.into_inner()) = Some(cancel.clone());
    let ctx = ToolContext {
        cwd: cwd.to_path_buf(),
        cancel: cancel.clone(),
        session_id: session_state
            .as_ref()
            .map(|s| s.session_id.as_str().to_string()),
    };
    let mut images = std::mem::take(&mut *pending_images);
    // Typed/pasted-text file links (never paste-attached) still carry vision.
    for p in super::attachments::extract_inline_image_paths(&prompt_text, cwd) {
        if !images.contains(&p) {
            images.push(p);
        }
    }
    let user_msg = build_user_message_with_attachments(&prompt_text, &images);
    let user_msg_for_retry = user_msg.clone();
    let mut initial_count = agent.messages().len();
    {
        let latest = tui
            .as_ref()
            .and_then(|(s, _)| s.lock().ok().and_then(|t| t.latest_usage));
        maybe_threshold_compact(
            agent,
            config,
            &mut *session_state,
            cwd,
            tui.as_ref().map(|(s, _)| s),
            latest,
            &mut initial_count,
        )
        .await;
    }

    // status row on; events stream straight into the composer
    // (already begun above when the agent was built; re-assert here so the
    // normal path keeps its exact paint).
    if let Some(s) = &tui_stream {
        s.lock().expect("tui lock").begin_turn("Working");
    }

    // Raw mode swallows ^C (no SIGINT), so a watcher must read
    // key events during the turn and translate Ctrl-C into cancel.
    let watch_cancel = cancel.clone();
    let watch_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watcher_stopped = watch_stop.clone();
    let watcher_tui = tui_stream.clone();
    let cwd_for_watcher = cwd.to_path_buf();
    let key_watcher = key_watcher::spawn_key_watcher_with_typing(
        watch_cancel,
        watcher_stopped,
        watcher_tui,
        cwd_for_watcher,
    );

    let mut pending_tools: HashMap<String, (String, Option<serde_json::Value>)> = HashMap::new();
    let mut turn_usage: Option<gray_core::event::Usage> = None;
    let turn_start = std::time::Instant::now();
    let mut turn_duration_ms: Option<u64> = None;
    let history_revision = agent.history_revision();
    let mut run_result = {
        let mut on_event = |ev: &AgentEvent| {
            dispatch_agent_event(
                ev,
                tui_stream.as_ref(),
                interactive,
                &mut pending_tools,
                &mut turn_usage,
                cwd,
                config.model.as_deref().unwrap_or(""),
                &mut *session_totals,
                turn_start,
                &mut turn_duration_ms,
            );
        };
        run_streaming_cancellable(agent, user_msg, ctx, &cancel, &mut on_event).await
    };
    // overflow recovery (one retry only)
    if let Err(ref e) = run_result
        && maybe_overflow_compact(
            agent,
            config,
            &mut *session_state,
            cwd,
            tui.as_ref().map(|(s, _)| s),
            &mut initial_count,
            e,
        )
        .await
    {
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
            dispatch_agent_event(
                ev,
                tui_stream.as_ref(),
                interactive,
                &mut pending_tools,
                &mut turn_usage,
                cwd,
                config.model.as_deref().unwrap_or(""),
                &mut *session_totals,
                turn_start,
                &mut turn_duration_ms,
            );
        };
        run_result = run_streaming_cancellable(
            agent,
            user_msg_for_retry.clone(),
            ctx2,
            &cancel,
            &mut on_event2,
        )
        .await;
    }
    TURN_STATE.lock().unwrap_or_else(|e| e.into_inner()).take();
    // Stop and join before the idle reader starts. Otherwise the old watcher
    // can steal its first key between poll() and read(). No TUI lock held here.
    watch_stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = key_watcher.await;
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
    match run_result {
        Ok(_) => {
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
        }
        Err(CoreError::Cancelled) => {
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
                    t.ensure_gap(1); // never glue "(interrupted)" to the last streamed row
                    t.stream("(interrupted)\n");
                }
            } else {
                println!("(interrupted)");
            }
        }
        Err(e) => {
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
            if interactive {
                if let Some((shared, _)) = tui {
                    let mut t = shared.lock().expect("tui lock");
                    t.end_thinking();
                    t.ensure_gap(1);
                    t.stream(&format!("{msg}\n"));
                }
            } else {
                eprintln!("{msg}");
            }
        }
    }
    if let Some(s) = &tui_stream {
        s.lock().expect("tui lock").end_turn();
    }
    // if we queued input while working, start it immediately
    if interactive && let Some((shared, _)) = tui {
        let mut t = shared.lock().expect("tui lock");
        if let Some(text) = t.local_command.take() {
            // Esc mid-turn: command already echoed; run locally, never to the AI
            drop(t);
            *pending_command = Some(expand_skill_command(
                parse_command(&text),
                cwd,
                Some(shared),
                true,
            ));
        } else if let Some((qtext, qimages)) = t.queued_inputs.pop_front() {
            t.push_user_prompt(&qtext, &qimages, !qtext.starts_with('/'));
            drop(t);
            *pending_command = Some(expand_skill_command(
                parse_command(&qtext),
                cwd,
                Some(shared),
                false,
            ));
            *pending_images = qimages;
        }
    }
    Ok(())
}

#[path = "prompt_turn_tests.rs"]
#[cfg(test)]
mod tests;
