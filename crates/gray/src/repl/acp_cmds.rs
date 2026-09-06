//! ACP slash-command: sticky `/acp <agent>` mode plus one-shot
//! `/acp <agent> <prompt>` delegate and `list` / `status` / `off` helpers.

use super::*;

#[derive(Debug, PartialEq)]
pub(crate) enum AcpAction {
    List,
    Status,
    Off,
    Help,
    Switch {
        agent: String,
        yolo: bool,
    },
    Delegate {
        agent: String,
        prompt: String,
        yolo: bool,
    },
}

pub(crate) fn parse_acp_args(raw: &str) -> AcpAction {
    let mut toks = raw.split_whitespace().skip(1);
    let first = toks.next().map(|t| t.to_ascii_lowercase());
    match first.as_deref() {
        None => AcpAction::List,
        Some("list") | Some("ls") | Some("agents") => AcpAction::List,
        Some("status") | Some("st") => AcpAction::Status,
        Some("off") | Some("native") | Some("gray") => AcpAction::Off,
        Some("help") | Some("-h") | Some("--help") => AcpAction::Help,
        Some(agent) => {
            let rest: Vec<&str> = toks.collect();
            let yolo = rest.iter().any(|t| *t == "--yolo" || *t == "-y");
            let prompt = rest
                .iter()
                .filter(|t| **t != "--yolo" && **t != "-y")
                .copied()
                .collect::<Vec<_>>()
                .join(" ");
            // Bare `/acp <agent>` (what the picker returns) switches the
            // whole REPL to that agent; with prompt text it stays a one-shot.
            if prompt.is_empty() {
                AcpAction::Switch {
                    agent: agent.to_string(),
                    yolo,
                }
            } else {
                AcpAction::Delegate {
                    agent: agent.to_string(),
                    prompt,
                    yolo,
                }
            }
        }
    }
}

fn acp_table(home: Option<&std::path::Path>) -> Vec<String> {
    let mut lines = vec!["agents:".to_string()];
    for spec in gray_acp::all_specs(home) {
        let mark = if gray_acp::installed(&spec) {
            "✓"
        } else {
            "×"
        };
        let display = if spec.display.is_empty() {
            spec.key
        } else {
            spec.display
        };
        if gray_acp::installed(&spec) {
            lines.push(format!("  {mark} {:<10} {display}", spec.key));
        } else {
            lines.push(format!(
                "  {mark} {:<10} {display} — {}",
                spec.key, spec.install_hint
            ));
        }
    }
    lines.push("usage: /acp <agent> [prompt] · /acp list · /acp off".to_string());
    lines
}

fn set_model_label(tui: Option<&crate::composer::SharedTui>, label: &str) {
    if let Some(t) = tui {
        t.lock().expect("tui lock").set_model(label.to_string());
    }
}

/// Display-only: switched-to line gains ` (auto-approve)` when the session
/// auto-approves (`--yolo` / `GRAY_ACP_AUTO_APPROVE=1`). No approval effect.
pub(crate) fn switch_message(key: &str, auto_approve: bool) -> String {
    if auto_approve {
        format!("switched to acp:{key} (auto-approve) — prompts route there until /acp off")
    } else {
        format!("switched to acp:{key} — prompts route there until /acp off")
    }
}

/// Display-only: `/acp status` line with `auto_approve: on/off` so the
/// permission posture is visible. No approval effect.
pub(crate) fn status_line(
    agent_key: &str,
    session_prefix: &str,
    usage: Option<&str>,
    auto_approve: bool,
) -> String {
    let mut line = format!(
        "acp:{agent_key} · session {session_prefix}… · auto_approve: {}",
        gray_acp::session::auto_approve_label(auto_approve)
    );
    if let Some(u) = usage {
        line.push_str(&format!(" · {u}"));
    }
    line
}

/// Shared spawn path for one-shot delegates and sticky switches: resolves,
/// starts, and announces the session. Returns `None` after printing why.
async fn start_session(
    spec: gray_acp::AgentSpec,
    display: String,
    cwd: &std::path::Path,
    yolo: bool,
    tui: Option<&crate::composer::SharedTui>,
) -> Option<gray_acp::AcpSession> {
    if !gray_acp::installed(&spec) {
        say(
            tui,
            &format!("agent '{}' not installed ({})", spec.key, spec.install_hint),
        );
        return None;
    }
    say(
        tui,
        &format!("acp:{key} starting {display}…", key = spec.key),
    );
    let auto_approve = yolo || std::env::var("GRAY_ACP_AUTO_APPROVE").as_deref() == Ok("1");
    let opts = gray_acp::AcpSessionOptions {
        spec,
        cwd: cwd.to_path_buf(),
        resume_session_id: None,
        auto_approve,
        permission_prompt: std::sync::Arc::new(gray_acp::DenyAllPrompt),
        display,
    };
    match gray_acp::AcpSession::start(opts).await {
        Ok(s) => {
            let sid = s.session_id().to_string();
            let prefix: String = sid.chars().take(8).collect();
            say(
                tui,
                &format!("acp:{key} session {prefix}…", key = s.agent_key()),
            );
            Some(s)
        }
        Err(gray_acp::AcpError::NotInstalled(key, hint)) => {
            say(tui, &format!("agent '{key}' not installed ({hint})"));
            None
        }
        Err(gray_acp::AcpError::AuthRequired(methods)) => {
            say(tui, &format!("agent requires auth: {methods}"));
            None
        }
        Err(e) => {
            say(tui, &format!("acp error: {e:#}"));
            None
        }
    }
}

fn resolve_spec(
    agent: &str,
    tui: Option<&crate::composer::SharedTui>,
) -> Option<(gray_acp::AgentSpec, String)> {
    let home = gray_acp::gray_home_dir();
    let home_opt = Some(home.as_path());
    let Some(spec) = gray_acp::resolve(agent, home_opt) else {
        say(tui, &format!("unknown agent '{agent}'"));
        for line in acp_table(home_opt) {
            say(tui, &line);
        }
        return None;
    };
    let display = if spec.display.is_empty() {
        spec.key.to_string()
    } else {
        spec.display.to_string()
    };
    Some((spec, display))
}

/// Full `/acp` command surface including sticky mode: `/acp <agent>` parks
/// an `AcpSession` in `acp` so later prompts route through it until `/acp off`.
/// `native_model` restores the status-line label when leaving ACP mode.
pub(crate) async fn handle_acp_command(
    raw: &str,
    cwd: &std::path::Path,
    tui: Option<&crate::composer::SharedTui>,
    acp: &mut Option<gray_acp::AcpSession>,
    native_model: Option<&str>,
) {
    let home = gray_acp::gray_home_dir();
    let home_opt = Some(home.as_path());
    let action = if raw.trim() == "/acp" && tui.is_some() {
        let bg = tui.map(|s| s.lock().expect("tui lock").snapshot());
        match super::with_modal_sync(tui, || crate::setup::run_acp_modal(bg.as_ref())) {
            Ok(Some(cmd)) => parse_acp_args(&cmd),
            _ => return,
        }
    } else {
        parse_acp_args(raw)
    };
    match action {
        AcpAction::List => {
            for line in acp_table(home_opt) {
                say(tui, &line);
            }
        }
        AcpAction::Status => {
            if let Some(s) = acp.as_ref() {
                let sid = s.session_id().to_string();
                let prefix: String = sid.chars().take(8).collect();
                say(
                    tui,
                    &status_line(s.agent_key(), &prefix, s.usage_text(), s.auto_approve()),
                );
            } else {
                say(tui, "acp: native mode — /acp <agent> to switch");
            }
        }
        AcpAction::Off => {
            if let Some(s) = acp.take() {
                let key = s.agent_key().to_string();
                s.shutdown().await;
                set_model_label(tui, native_model.unwrap_or("default"));
                say(tui, &format!("acp:{key} off — back to native"));
            } else {
                say(tui, "acp: already native");
            }
        }
        AcpAction::Help => {
            say(
                tui,
                "usage: /acp <agent> [--yolo] · /acp <agent> <prompt> [--yolo] · /acp list · /acp status · /acp off",
            );
        }
        AcpAction::Switch { agent, yolo } => {
            let Some((spec, display)) = resolve_spec(&agent, tui) else {
                return;
            };
            if acp.as_ref().is_some_and(|s| s.agent_key() == spec.key) {
                say(tui, &format!("acp:{} already active", spec.key));
                return;
            }
            if let Some(old) = acp.take() {
                old.shutdown().await;
            }
            if let Some(s) = start_session(spec, display, cwd, yolo, tui).await {
                let key = s.agent_key().to_string();
                let msg = switch_message(&key, s.auto_approve());
                set_model_label(tui, &format!("acp:{key}"));
                say(tui, &msg);
                *acp = Some(s);
            }
        }
        AcpAction::Delegate {
            agent,
            prompt,
            yolo,
        } => {
            if prompt.is_empty() {
                say(tui, "usage: /acp <agent> <prompt> — prompt text required");
                return;
            }
            let Some((spec, display)) = resolve_spec(&agent, tui) else {
                return;
            };
            let Some(mut session) = start_session(spec, display, cwd, yolo, tui).await else {
                return;
            };
            let mut text = String::new();
            let mut thinking = String::new();
            let mut on_event = |ev: &gray_core::event::AgentEvent| {
                use gray_core::event::AgentEvent;
                match ev {
                    AgentEvent::TextDelta { delta } => text.push_str(delta),
                    AgentEvent::ThinkingDelta { delta } => thinking.push_str(delta),
                    AgentEvent::ToolCallStart { name, .. } => {
                        say(tui, &format!("  ⏺ {name}…"));
                    }
                    AgentEvent::ToolResult {
                        output, is_error, ..
                    } => {
                        let head: String = output.chars().take(300).collect();
                        if *is_error {
                            say(tui, &format!("  ✗ {head}"));
                        }
                    }
                    _ => {}
                }
            };
            match session.prompt(&prompt, &mut on_event).await {
                Ok(_) => {
                    if text.trim().is_empty() {
                        say(tui, "acp: (empty response)");
                    } else {
                        for line in text.lines() {
                            say(tui, line);
                        }
                    }
                }
                Err(gray_acp::AcpError::Cancelled) => say(tui, "acp: cancelled"),
                Err(e) => say(tui, &format!("acp error: {e:#}")),
            }
            session.shutdown().await;
        }
    }
}

/// One turn through the sticky ACP session: same streaming scaffold as the
/// native prompt turn (live events, Ctrl-C, persist) but no provider boot,
/// no agent build, and no overflow-compact retry — the agent owns its context.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_acp_turn(
    prompt_text: String,
    pending_images: &mut Vec<std::path::PathBuf>,
    acp: &mut Option<gray_acp::AcpSession>,
    config: &Config,
    cwd: &std::path::Path,
    tui: &TuiOpt,
    interactive: bool,
    session_state: &mut Option<SessionState>,
    session_totals: &mut SessionTotals,
    pending_command: &mut Option<ReplCommand>,
    native_model: Option<&str>,
) -> anyhow::Result<()> {
    let Some(session) = acp.as_mut() else {
        return Ok(());
    };
    let images = std::mem::take(&mut *pending_images);
    if !images.is_empty() {
        say(
            tui.as_ref().map(|(s, _)| s),
            &format!(
                "(+{} image(s) kept in transcript but not sent — ACP turns are text-only for now)",
                images.len()
            ),
        );
    }
    let model_label = format!("acp:{}", session.agent_key());
    let cancel = tokio_util::sync::CancellationToken::new();
    *super::TURN_STATE.lock().expect("turn state lock") = Some(cancel.clone());

    let tui_stream = if interactive {
        Some(tui.as_ref().expect("interactive implies tui").0.clone())
    } else {
        None
    };
    if let Some(s) = &tui_stream {
        s.lock().expect("tui lock").begin_turn("Working");
    }

    // Raw mode swallows ^C (no SIGINT): same key-watcher bridge as native turns.
    let watch_cancel = cancel.clone();
    let watch_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watcher_stopped = watch_stop.clone();
    let watcher_tui = tui_stream.clone();
    let cwd_for_watcher = cwd.to_path_buf();
    let _key_watcher = super::key_watcher::spawn_key_watcher_with_typing(
        watch_cancel,
        watcher_stopped,
        watcher_tui,
        cwd_for_watcher,
    );

    let mut pending_tools: HashMap<String, (String, Option<serde_json::Value>)> = HashMap::new();
    let mut turn_usage: Option<gray_core::event::Usage> = None;
    let turn_start = std::time::Instant::now();
    let mut turn_duration_ms: Option<u64> = None;
    let mut text = String::new();
    let run_result = {
        let mut on_event = |ev: &AgentEvent| {
            if let AgentEvent::TextDelta { delta } = ev {
                text.push_str(delta);
            }
            dispatch_agent_event(
                ev,
                tui_stream.as_ref(),
                interactive,
                &mut pending_tools,
                &mut turn_usage,
                cwd,
                &model_label,
                &mut *session_totals,
                turn_start,
                &mut turn_duration_ms,
            );
        };
        let mut run_future = Box::pin(session.prompt(&prompt_text, &mut on_event));
        tokio::select! {
            res = &mut run_future => res,
            _ = cancel.cancelled() => {
                drop(run_future);
                session.cancel().await;
                Err(gray_acp::AcpError::Cancelled)
            }
        }
    };
    super::TURN_STATE.lock().expect("turn state lock").take();
    watch_stop.store(true, std::sync::atomic::Ordering::Relaxed);
    if turn_duration_ms.is_none() {
        turn_duration_ms = Some(turn_start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64);
    }

    // Transcript first: the JSONL log must match what ran, whatever the outcome.
    super::session::ensure_session_state(session_state, config, cwd).await;
    if let Some(state) = session_state {
        let user_msg = build_user_message_with_attachments(&prompt_text, &images);
        if let Err(e) = state
            .store
            .append_with_usage_and_duration(&state.session_id, &user_msg, None, None)
            .await
        {
            log::warn!(target: "gray_session", "session append failed: {e}");
        }
        if !text.trim().is_empty() {
            let asst = Message::assistant(std::mem::take(&mut text));
            if let Err(e) = state
                .store
                .append_with_usage_and_duration(
                    &state.session_id,
                    &asst,
                    turn_usage,
                    turn_duration_ms,
                )
                .await
            {
                log::warn!(target: "gray_session", "session append failed: {e}");
            }
        }
    }

    match run_result {
        Ok(_) => {}
        Err(gray_acp::AcpError::Cancelled) => {
            end_thinking_gap(tui);
            if interactive {
                if let Some((shared, _)) = tui {
                    shared.lock().expect("tui lock").stream("(interrupted)\n");
                }
            } else {
                println!("(interrupted)");
            }
        }
        // The agent died mid-turn: fall back to native rather than wedging
        // the REPL on a dead session.
        Err(gray_acp::AcpError::ProcessExited { code, stderr_tail }) => {
            if let Some(dead) = acp.take() {
                dead.shutdown().await;
            }
            set_model_label(
                tui.as_ref().map(|(s, _)| s),
                native_model.unwrap_or("default"),
            );
            let tail: String = stderr_tail.chars().take(500).collect();
            let msg = format!("agent exited (code {code}): {tail} — back to native");
            end_thinking_gap(tui);
            if interactive {
                if let Some((shared, _)) = tui {
                    shared.lock().expect("tui lock").stream(&format!("{msg}\n"));
                }
            } else {
                eprintln!("{msg}");
            }
        }
        Err(e) => {
            let msg = format!("acp error: {e:#}");
            end_thinking_gap(tui);
            if interactive {
                if let Some((shared, _)) = tui {
                    shared.lock().expect("tui lock").stream(&format!("{msg}\n"));
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
        if let Some(cmd_text) = t.local_command.take() {
            // Esc mid-turn: command already echoed; run locally, never to the AI
            drop(t);
            *pending_command = Some(expand_skill_command(
                parse_command(&cmd_text),
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

fn end_thinking_gap(tui: &TuiOpt) {
    if let Some((shared, _)) = tui {
        let mut t = shared.lock().expect("tui lock");
        t.end_thinking();
        t.ensure_gap(1);
    }
}

#[cfg(test)]
mod acp_tests {
    use super::AcpAction;
    use super::parse_acp_args;
    use super::status_line;
    use super::switch_message;

    #[test]
    fn acp_args_parse() {
        assert!(matches!(parse_acp_args("/acp"), AcpAction::List));
        assert!(matches!(parse_acp_args("/acp list"), AcpAction::List));
        assert!(matches!(parse_acp_args("/acp status"), AcpAction::Status));
        assert!(matches!(parse_acp_args("/acp off"), AcpAction::Off));
        // Bare agent (what the picker returns) switches sticky mode.
        match parse_acp_args("/acp claude") {
            AcpAction::Switch { agent, yolo } => {
                assert_eq!(agent, "claude");
                assert!(!yolo);
            }
            _ => panic!("expected switch"),
        }
        match parse_acp_args("/acp codex --yolo") {
            AcpAction::Switch { agent, yolo } => {
                assert_eq!(agent, "codex");
                assert!(yolo);
            }
            _ => panic!("expected switch"),
        }
        match parse_acp_args("/acp claude hello world") {
            AcpAction::Delegate {
                agent,
                prompt,
                yolo,
            } => {
                assert_eq!(agent, "claude");
                assert_eq!(prompt, "hello world");
                assert!(!yolo);
            }
            _ => panic!("expected delegate"),
        }
        match parse_acp_args("/acp codex --yolo do things") {
            AcpAction::Delegate { yolo, .. } => assert!(yolo),
            _ => panic!("expected delegate"),
        }
    }

    #[test]
    fn switch_message_marks_auto_approve() {
        assert_eq!(
            switch_message("codex", false),
            "switched to acp:codex — prompts route there until /acp off"
        );
        assert_eq!(
            switch_message("codex", true),
            "switched to acp:codex (auto-approve) — prompts route there until /acp off"
        );
    }

    #[test]
    fn status_line_includes_auto_approve() {
        assert_eq!(
            status_line("codex", "abc12345", None, false),
            "acp:codex · session abc12345… · auto_approve: off"
        );
        assert_eq!(
            status_line("codex", "abc12345", None, true),
            "acp:codex · session abc12345… · auto_approve: on"
        );
        assert_eq!(
            status_line("codex", "abc12345", Some("1k tok"), true),
            "acp:codex · session abc12345… · auto_approve: on · 1k tok"
        );
    }
}
