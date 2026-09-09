//! REPL image-only turn (split from `repl`).

use super::*;
use gray_core::questions::QuestionBridge;

#[allow(clippy::too_many_arguments)]
// Mechanical split of `run_repl_mode`: params are the loop state the turn borrows.
pub(crate) async fn run_empty_turn(
    pending_images: &mut Vec<std::path::PathBuf>,
    agent: &mut Option<Agent>,
    config: &mut Config,
    cwd: &std::path::Path,
    tui: &TuiOpt,
    interactive: bool,
    session_state: &mut Option<SessionState>,
    session_totals: &mut SessionTotals,
    pending_history: &mut Vec<Message>,
    unconfigured: &mut bool,
    question_bridge: &QuestionBridge,
    approval_gate: &gray_core::approvals::ApprovalGate,
) -> anyhow::Result<()> {
    let (shared, _) = if interactive {
        (Some(tui.as_ref().expect("interactive implies tui")), ())
    } else {
        (None, ())
    };
    let tui_stream = shared.as_ref().map(|(s, _)| (*s).clone());
    if !pending_images.is_empty() {
        // image(s) without text: treat as prompt with images
        let images = std::mem::take(&mut *pending_images);
        let prompt_text = String::new();
        // fall through to Prompt handling by constructing message immediately
        // reuse Prompt logic inline
        if agent.is_none() {
            if *unconfigured {
                let bg = tui
                    .as_ref()
                    .map(|(shared, _)| shared.lock().expect("tui lock").snapshot());
                let result = with_modal(
                    tui.as_ref().map(|(s, _)| s),
                    crate::setup::run_provider_menu(config, bg.as_ref()),
                )
                .await;
                match result {
                    Ok(true) => {
                        *unconfigured = false;
                        if let Some((shared, _)) = tui {
                            let mut t = shared.lock().expect("tui lock");
                            if let Some(m) = &config.model {
                                t.set_model(m.clone());
                            }
                            let model_str = config.model.as_deref().unwrap_or("default");
                            let prov_name = crate::setup::load_catalog()
                                .ok()
                                .and_then(|c| {
                                    c.values()
                                        .find(|p| p.base_url == config.base_url)
                                        .map(|p| p.name.clone())
                                })
                                .unwrap_or_else(|| "provider".to_string());
                            t.push_dim(format!("└ connected to {prov_name} · {model_str}"));
                            let _ = t.draw();
                        }
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
            // fallback cache shard) and minted the real SessionId only after
            // the first turn — the reused agent then sat on the fallback
            // shard all session. Ensure the real sid BEFORE the first build
            // so the provider gets it from turn one. Gated on model so the
            // no-model REPL still opens session-free (lazy build).
            if super::session::should_ensure_session_before_build(
                session_state.is_some(),
                config.model.as_deref(),
            ) {
                super::session::ensure_session_state(session_state, config, cwd).await;
            }
            let sid = session_state
                .as_ref()
                .map(|s| s.session_id.as_str().to_string());
            // Status on BEFORE the first build: see prompt_turn — the slow
            // first build otherwise leaves the fresh viewport blank ~1s.
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
                        // Live viewport: never raw println! over it (ghost
                        // input on the next draw) — render via the composer.
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
            questions: Some(question_bridge.clone()),
            session_id: session_state
                .as_ref()
                .map(|s| s.session_id.as_str().to_string()),
            permission: PermissionMode::resolve(false),
            approvals: Some(approval_gate.clone()),
        };
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
        // Status row on (already begun above when the agent was built;
        // re-assert here so the normal path keeps its exact paint).
        if let Some(s) = &tui_stream {
            s.lock().expect("tui lock").begin_turn("Working");
        }
        let watch_cancel = cancel.clone();
        let watch_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher_stopped = watch_stop.clone();
        let watcher_tui = tui_stream.clone();
        let _key_watcher =
            key_watcher::spawn_key_watcher(watch_cancel, watcher_stopped, watcher_tui);
        let mut pending_tools: HashMap<String, (String, Option<serde_json::Value>)> =
            HashMap::new();
        let mut turn_usage: Option<gray_core::event::Usage> = None;
        let turn_start = std::time::Instant::now();
        let mut turn_duration_ms: Option<u64> = None;
        let mut run_result = {
            let mut on_event = |ev: &gray_core::event::AgentEvent| {
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
            let mut run_future = Box::pin(agent.run_streaming(user_msg, ctx, &mut on_event));
            tokio::select! {
                res = &mut run_future => res,
                _ = cancel.cancelled() => {
                    drop(run_future);
                    agent.abort_turn();
                    Err(gray_core::error::CoreError::Cancelled)
                }
            }
        };
        // overflow recovery (one retry only)
        if let Err(ref e) = run_result {
            let latest = tui
                .as_ref()
                .and_then(|(s, _)| s.lock().ok().and_then(|t| t.latest_usage))
                .or(turn_usage);
            if maybe_overflow_compact(
                agent,
                config,
                &mut *session_state,
                cwd,
                tui.as_ref().map(|(s, _)| s),
                latest,
                &mut initial_count,
                e,
            )
            .await
            {
                pending_tools.clear();
                let ctx2 = gray_core::agent::ToolContext {
                    cwd: cwd.to_path_buf(),
                    cancel: cancel.clone(),
                    questions: Some(question_bridge.clone()),
                    session_id: session_state
                        .as_ref()
                        .map(|s| s.session_id.as_str().to_string()),
                    permission: gray_core::agent::PermissionMode::resolve(false),
                    approvals: Some(approval_gate.clone()),
                };
                let mut on_event2 = |ev: &gray_core::event::AgentEvent| {
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
                let mut run_future2 =
                    Box::pin(agent.run_streaming(user_msg_for_retry.clone(), ctx2, &mut on_event2));
                let retry_res = tokio::select! {
                    res = &mut run_future2 => res,
                    _ = cancel.cancelled() => {
                        drop(run_future2);
                        agent.abort_turn();
                        Err(gray_core::error::CoreError::Cancelled)
                    }
                };
                run_result = retry_res;
            }
        }
        TURN_STATE.lock().unwrap_or_else(|e| e.into_inner()).take();
        watch_stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if turn_duration_ms.is_none() {
            turn_duration_ms =
                Some(turn_start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64);
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
            Err(gray_core::error::CoreError::Cancelled) => {
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
        return Ok(());
    }
    pending_images.clear();
    Ok(())
}

#[cfg(test)]
mod tests {
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
}
