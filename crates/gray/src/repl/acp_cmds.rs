//! ACP slash-command: one-shot delegate `/acp <agent> <prompt>` plus
//! `list` / `status` helpers (split from `repl`).

use super::*;

#[derive(Debug, PartialEq)]
pub(crate) enum AcpAction {
    List,
    Status,
    Off,
    Help,
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
            AcpAction::Delegate {
                agent: agent.to_string(),
                prompt,
                yolo,
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
    lines.push("usage: /acp <agent> <prompt> · /acp list · /acp off".to_string());
    lines
}

pub(crate) async fn handle_acp(
    raw: &str,
    cwd: &std::path::Path,
    tui: Option<&crate::composer::SharedTui>,
) {
    let home = gray_acp::gray_home_dir();
    let home_opt = Some(home.as_path());
    match parse_acp_args(raw) {
        AcpAction::List => {
            for line in acp_table(home_opt) {
                say(tui, &line);
            }
        }
        AcpAction::Status => {
            say(
                tui,
                "acp: native mode (no persistent session yet — /acp <agent> <prompt> delegates one-shot)",
            );
        }
        AcpAction::Off => {
            say(tui, "acp: already native");
        }
        AcpAction::Help => {
            say(
                tui,
                "usage: /acp <agent> <prompt> [--yolo] · /acp list · /acp status · /acp off",
            );
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
            let Some(spec) = gray_acp::resolve(&agent, home_opt) else {
                say(tui, &format!("unknown agent '{agent}'"));
                for line in acp_table(home_opt) {
                    say(tui, &line);
                }
                return;
            };
            if !gray_acp::installed(&spec) {
                say(
                    tui,
                    &format!("agent '{}' not installed ({})", spec.key, spec.install_hint),
                );
                return;
            }
            let display = if spec.display.is_empty() {
                spec.key.to_string()
            } else {
                spec.display.to_string()
            };
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
            let mut session = match gray_acp::AcpSession::start(opts).await {
                Ok(s) => s,
                Err(gray_acp::AcpError::NotInstalled(key, hint)) => {
                    say(tui, &format!("agent '{key}' not installed ({hint})"));
                    return;
                }
                Err(gray_acp::AcpError::AuthRequired(methods)) => {
                    say(tui, &format!("agent requires auth: {methods}"));
                    return;
                }
                Err(e) => {
                    say(tui, &format!("acp error: {e:#}"));
                    return;
                }
            };
            let sid = session.session_id().to_string();
            let prefix: String = sid.chars().take(8).collect();
            say(
                tui,
                &format!("acp:{key} session {prefix}…", key = session.agent_key()),
            );
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

#[cfg(test)]
mod acp_tests {
    use super::AcpAction;
    use super::parse_acp_args;

    #[test]
    fn acp_args_parse() {
        assert!(matches!(parse_acp_args("/acp"), AcpAction::List));
        assert!(matches!(parse_acp_args("/acp list"), AcpAction::List));
        assert!(matches!(parse_acp_args("/acp status"), AcpAction::Status));
        assert!(matches!(parse_acp_args("/acp off"), AcpAction::Off));
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
}
