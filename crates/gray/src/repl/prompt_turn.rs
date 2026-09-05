//! REPL prompt turn (split from `repl`).

use super::*;
use gray_core::questions::QuestionBridge;

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
    question_bridge: &QuestionBridge,
) -> anyhow::Result<()> {
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
                        let _ = shared.lock().expect("tui lock").draw();
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
        let sid = session_state
            .as_ref()
            .map(|s| s.session_id.as_str().to_string());
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
                println!("{e}");
                return Ok(());
            }
        }
    }
    let agent = agent.as_mut().expect("agent built above");
    let cancel = tokio_util::sync::CancellationToken::new();
    *TURN_STATE.lock().expect("turn state lock") = Some(cancel.clone());
    let ctx = ToolContext {
        cwd: cwd.to_path_buf(),
        cancel: cancel.clone(),
        questions: Some(question_bridge.clone()),
    };
    let images = std::mem::take(&mut *pending_images);
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

    let (shared, _) = if interactive {
        (Some(tui.as_ref().expect("interactive implies tui")), ())
    } else {
        (None, ())
    };

    // status row on; events stream straight into the composer
    let tui_stream = shared.as_ref().map(|(s, _)| (*s).clone());
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
    let _key_watcher = key_watcher::spawn_key_watcher_with_typing(
        watch_cancel,
        watcher_stopped,
        watcher_tui,
        cwd_for_watcher,
    );

    let mut pending_tools: HashMap<String, (String, Option<serde_json::Value>)> = HashMap::new();
    let mut turn_usage: Option<gray_core::event::Usage> = None;
    let turn_start = std::time::Instant::now();
    let mut turn_duration_ms: Option<u64> = None;
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
        let mut run_future = Box::pin(agent.run_streaming(user_msg, ctx, &mut on_event));
        tokio::select! {
            res = &mut run_future => res,
            _ = cancel.cancelled() => Err(CoreError::Cancelled),
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
            let ctx2 = ToolContext {
                cwd: cwd.to_path_buf(),
                cancel: cancel.clone(),
                questions: Some(question_bridge.clone()),
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
            let mut run_future2 =
                Box::pin(agent.run_streaming(user_msg_for_retry.clone(), ctx2, &mut on_event2));
            let retry_res = tokio::select! {
                res = &mut run_future2 => res,
                _ = cancel.cancelled() => Err(CoreError::Cancelled),
            };
            run_result = retry_res;
        }
    }
    TURN_STATE.lock().expect("turn state lock").take();
    // signal the watcher to exit; it dies within one 100ms tick.
    // (Never .await it here without the flag — deadlock.)
    watch_stop.store(true, std::sync::atomic::Ordering::Relaxed);
    if turn_duration_ms.is_none() {
        turn_duration_ms = Some(turn_start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64);
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
            let echo = crate::composer::transcript::redact_command_echo(&qtext);
            t.push_user_prompt(&echo, &qimages, !qtext.starts_with('/'));
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
