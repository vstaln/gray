//! Gateway slash-command: parse, status, daemon (split from `repl`).

use super::*;

/// What `/gateway ...` should do. Parsed by [`parse_gateway_args`].
#[derive(Debug, PartialEq)]
pub(crate) enum GatewayAction {
    Status,
    Connect(gray_gateway::config::Platform, String),
    Disconnect(gray_gateway::config::Platform),
    Enable(gray_gateway::config::Platform),
    Run,
    Stop,
    Autostart(bool),
    Install,
    Uninstall,
    Pairing(PairingArgs),
    Help,
}

/// Args for `/gateway pairing ...` — same actions as `gray gateway pairing`.
#[derive(Debug, PartialEq)]
pub(crate) enum PairingArgs {
    Approve(String, String),
    List(Option<String>),
    Revoke(String, String),
}

/// Parses `/gateway [sub] [args]` — bare or unknown subcommands default to
/// Status/Help so a mistyped command never silently starts or stops anything.
pub(crate) fn parse_gateway_args(raw: &str) -> GatewayAction {
    let mut toks = raw.split_whitespace().skip(1); // drop "/gateway"
    match toks.next().map(|t| t.to_ascii_lowercase()).as_deref() {
        None | Some("status") => GatewayAction::Status,
        Some("run") => GatewayAction::Run,
        Some("stop") => GatewayAction::Stop,
        Some("autostart") => match toks.next().map(|t| t.to_ascii_lowercase()).as_deref() {
            Some("on") | Some("true") | Some("enable") => GatewayAction::Autostart(true),
            Some("off") | Some("false") | Some("disable") => GatewayAction::Autostart(false),
            _ => GatewayAction::Help,
        },
        Some("install") => GatewayAction::Install,
        Some("uninstall") => GatewayAction::Uninstall,
        Some("help") => GatewayAction::Help,
        Some("connect") => match (toks.next(), toks.next()) {
            (Some(p), Some(tok)) => match p.parse::<gray_gateway::config::Platform>() {
                Ok(plat) => GatewayAction::Connect(plat, tok.to_string()),
                Err(_) => GatewayAction::Help,
            },
            _ => GatewayAction::Help,
        },
        Some("disconnect") => match toks.next() {
            Some(p) => match p.parse::<gray_gateway::config::Platform>() {
                Ok(plat) => GatewayAction::Disconnect(plat),
                Err(_) => GatewayAction::Help,
            },
            None => GatewayAction::Help,
        },
        Some("enable") => match toks.next() {
            Some(p) => match p.parse::<gray_gateway::config::Platform>() {
                Ok(plat) => GatewayAction::Enable(plat),
                Err(_) => GatewayAction::Help,
            },
            None => GatewayAction::Help,
        },
        Some("pairing") => match toks.next().map(|t| t.to_ascii_lowercase()).as_deref() {
            Some("approve") => match (toks.next(), toks.next()) {
                (Some(p), Some(c)) => {
                    GatewayAction::Pairing(PairingArgs::Approve(p.to_string(), c.to_string()))
                }
                _ => GatewayAction::Help,
            },
            Some("revoke") => match (toks.next(), toks.next()) {
                (Some(p), Some(u)) => {
                    GatewayAction::Pairing(PairingArgs::Revoke(p.to_string(), u.to_string()))
                }
                _ => GatewayAction::Help,
            },
            Some("list") => {
                GatewayAction::Pairing(PairingArgs::List(toks.next().map(str::to_string)))
            }
            None => GatewayAction::Pairing(PairingArgs::List(None)),
            _ => GatewayAction::Help,
        },
        Some(_) => GatewayAction::Help,
    }
}

/// One human line per known platform: enabled/disabled. `running` is the
/// in-process daemon state (systemd status is reported separately by callers).
pub(crate) fn gateway_status_lines(
    cfg: &gray_gateway::config::GatewayConfig,
    running: bool,
) -> Vec<String> {
    use gray_gateway::config::Platform;
    let mut lines = vec![format!(
        "gateway {} — config: {}",
        if running {
            "connected (in-process)"
        } else {
            "not running"
        },
        gray_gateway::config::gray_gateway_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    )];
    for plat in [Platform::Telegram, Platform::Discord, Platform::Slack] {
        let state = match cfg.platforms.get(&plat) {
            Some(pc) if pc.enabled => "enabled",
            Some(pc) if pc.token.as_ref().is_some_and(|t| !t.is_empty()) => {
                "disabled (token saved)"
            }
            _ => "disabled",
        };
        lines.push(format!("  {}: {state}", plat.label()));
    }
    lines.push(format!(
        "  autostart: {}",
        if cfg.autostart { "on" } else { "off" }
    ));
    lines.push("usage: /gateway connect <platform> <token> | enable <platform> | disconnect <platform> | run | stop | autostart on|off | install | uninstall | pairing approve <platform> <code> | pairing list | pairing revoke <platform> <user> | status".to_string());
    lines
}

/// Starts the gateway daemon in-process (shared by /gateway run and launch
/// autostart). Returns the live connection board, or None when already running.
pub(crate) fn start_gateway_in_background(
    tui: Option<&crate::composer::SharedTui>,
) -> Option<gray_gateway::status::GatewayStatusBoard> {
    let already = GATEWAY_HANDLE.lock().map(|g| g.is_some()).unwrap_or(false);
    if already {
        return None;
    }
    // The board starts with every enabled platform in `Connecting`; the
    // daemon marks each result and the boot watcher paints the live card.
    let board = {
        use gray_gateway::config::Platform;
        let cfg = gray_gateway::config::load_gateway_config();
        let plats: Vec<Platform> = Platform::ALL
            .into_iter()
            .filter(|p| cfg.platforms.get(p).is_some_and(|pc| pc.enabled))
            .collect();
        gray_gateway::status::GatewayStatusBoard::new(&plats)
    };
    let board_task = board.clone();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let tui_arc = tui.cloned();
    let h = tokio::spawn(async move {
        let res =
            gray_gateway::daemon::run_gateway_shutdown_with_board(rx, Some(board_task.clone()))
                .await;
        if let Err(e) = &res {
            log::warn!("gateway exited: {e}");
        }
        // Never leave the boot card spinning when the task exits early.
        board_task.fail_unresolved("gateway exited");
        GATEWAY_HANDLE.lock().ok().and_then(|mut g| g.take());
        if let Some(shared) = tui_arc
            && let Ok(mut t) = shared.lock()
        {
            match res {
                Ok(()) => t.push_dim("└ gateway stopped".to_string()),
                Err(e) => t.push_dim(format!("└ gateway exited: {e}")),
            }
            let _ = t.draw();
        }
    });
    *GATEWAY_HANDLE.lock().unwrap() = Some((h, tx));
    Some(board)
}

/// One boot-card row per platform: `  └─ Discord — connecting…` →
/// `  └─ Discord — connected as GrayBot`. The two-space indent matches the
/// card header (`format_tool_box_lines`); shared verbatim by the live
/// viewport panel and the committed final card.
pub(crate) fn gateway_boot_rows(board: &gray_gateway::status::GatewayStatusBoard) -> Vec<String> {
    use gray_gateway::status::PlatformConnState as S;
    let snap = board.snapshot();
    snap.iter()
        .enumerate()
        .map(|(i, (plat, st))| {
            let branch = if i + 1 == snap.len() {
                "└─"
            } else {
                "├─"
            };
            let status = match st {
                S::Connecting { stage } => format!("{stage}…"),
                S::Connected { identity: Some(id) } => format!("connected as {id}"),
                S::Connected { identity: None } => "connected".to_string(),
                S::Failed(e) => format!("connect failed: {e}"),
            };
            format!("  {branch} {} — {status}", plat.label())
        })
        .collect()
}

/// Watches the gateway board and drives the live boot panel → final card.
/// Repaints on every board mutation (via [`GatewayStatusBoard::notified`])
/// so short-lived stages still paint; the 250ms tick stays as backstop for
/// missed signals. Capped at 6 minutes so a wedged daemon can't leak the task.
pub(crate) fn spawn_gateway_boot_watcher(
    tui: crate::composer::SharedTui,
    board: gray_gateway::status::GatewayStatusBoard,
) {
    tokio::spawn(async move {
        let fut = async move {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(360);
            let mut iv = tokio::time::interval(std::time::Duration::from_millis(250));
            loop {
                let done = board.all_terminal() || std::time::Instant::now() >= deadline;
                if let Ok(mut t) = tui.try_lock() {
                    if done {
                        t.finish_gateway_boot(&board);
                    } else {
                        t.refresh_gateway_boot(&board);
                    }
                    let _ = t.draw();
                } else if done {
                    // TUI busy — retry the commit next tick instead of dropping it.
                    tokio::select! {
                        _ = board.notified() => {}
                        _ = iv.tick() => {}
                    }
                    continue;
                }
                if done
                    && tui
                        .try_lock()
                        .map(|t| t.gateway_boot.is_none())
                        .unwrap_or(false)
                {
                    break;
                }
                // Wait for the next stage transition or the backstop tick.
                tokio::select! {
                    _ = board.notified() => {}
                    _ = iv.tick() => {}
                }
            }
        };
        // Hard outer cap: never outlive the session.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(420), fut).await;
    });
}

/// Enables `plat` in `cfg` with `token` (mutates in place; caller saves).
pub(crate) fn apply_connect(
    cfg: &mut gray_gateway::config::GatewayConfig,
    plat: gray_gateway::config::Platform,
    token: &str,
) {
    let pc = cfg.platforms.entry(plat).or_default();
    pc.enabled = true;
    pc.token = Some(token.to_string());
}

/// Disables `plat` but keeps its token so re-enabling doesn't ask again.
pub(crate) fn apply_disconnect(
    cfg: &mut gray_gateway::config::GatewayConfig,
    plat: gray_gateway::config::Platform,
) {
    let pc = cfg.platforms.entry(plat).or_default();
    pc.enabled = false;
}

/// Re-enables `plat` with its saved token. Returns false when no token is
/// stored (caller should ask for `/gateway connect <platform> <token>`).
pub(crate) fn apply_enable(
    cfg: &mut gray_gateway::config::GatewayConfig,
    plat: gray_gateway::config::Platform,
) -> bool {
    let Some(pc) = cfg.platforms.get_mut(&plat) else {
        return false;
    };
    if pc.token.as_ref().is_some_and(|t| !t.is_empty()) {
        pc.enabled = true;
        true
    } else {
        false
    }
}

type GatewayHandle = (
    tokio::task::JoinHandle<()>,
    tokio::sync::oneshot::Sender<()>,
);
pub(crate) static GATEWAY_HANDLE: StdMutex<Option<GatewayHandle>> = StdMutex::new(None);

pub(crate) async fn handle_gateway(raw: &str, tui: Option<&crate::composer::SharedTui>) {
    match parse_gateway_args(raw) {
        GatewayAction::Status => {
            let cfg = gray_gateway::config::load_gateway_config();
            let running = GATEWAY_HANDLE.lock().map(|g| g.is_some()).unwrap_or(false);
            for line in gateway_status_lines(&cfg, running) {
                say(tui, &line);
            }
            // systemd state, best-effort (mirrors gray_gateway::systemd::status)
            if let Ok(out) = std::process::Command::new("systemctl")
                .args(["--user", "is-active", "gray-gateway.service"])
                .output()
            {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                say(tui, &format!("systemd unit: {s}"));
            }
        }
        GatewayAction::Connect(plat, token) => {
            let mut cfg = gray_gateway::config::load_gateway_config();
            apply_connect(&mut cfg, plat, &token);
            match gray_gateway::config::save_gateway_config(&cfg) {
                Ok(()) => say(
                    tui,
                    &format!(
                        "{} connected — token saved to ~/.gray/gateway.yaml (start with /gateway run or /gateway install)",
                        plat.label()
                    ),
                ),
                Err(e) => say(tui, &format!("gateway config error: {e}")),
            }
        }
        GatewayAction::Disconnect(plat) => {
            let mut cfg = gray_gateway::config::load_gateway_config();
            apply_disconnect(&mut cfg, plat);
            match gray_gateway::config::save_gateway_config(&cfg) {
                Ok(()) => say(tui, &format!("{} disabled", plat.label())),
                Err(e) => say(tui, &format!("gateway config error: {e}")),
            }
        }
        GatewayAction::Enable(plat) => {
            let mut cfg = gray_gateway::config::load_gateway_config();
            if apply_enable(&mut cfg, plat) {
                match gray_gateway::config::save_gateway_config(&cfg) {
                    Ok(()) => say(tui, &format!("{} enabled (saved token)", plat.label())),
                    Err(e) => say(tui, &format!("gateway config error: {e}")),
                }
            } else {
                say(
                    tui,
                    &format!(
                        "no saved token for {} — use /gateway connect {plat} <token>",
                        plat.label()
                    ),
                );
            }
        }
        GatewayAction::Run => {
            match start_gateway_in_background(tui) {
                Some(board) => {
                    if let Some(shared) = tui {
                        // Live boot panel + shimmer bar; the final state lands
                        // as ONE card, no follow-up lines.
                        shared
                            .lock()
                            .expect("tui lock")
                            .begin_gateway_boot("Gateway started", &board);
                        spawn_gateway_boot_watcher(shared.clone(), board);
                    } else {
                        say(
                            tui,
                            "gateway starting — platforms connect in background (~45s timeout each)",
                        );
                    }
                }
                None => say(tui, "gateway already running"),
            }
        }
        GatewayAction::Autostart(on) => {
            let mut cfg = gray_gateway::config::load_gateway_config();
            cfg.autostart = on;
            match gray_gateway::config::save_gateway_config(&cfg) {
                Ok(()) => say(
                    tui,
                    &format!(
                        "gateway autostart {}",
                        if on { "on — starts with gray" } else { "off" }
                    ),
                ),
                Err(e) => say(tui, &format!("gateway config error: {e}")),
            }
        }
        GatewayAction::Stop => {
            let mut g = GATEWAY_HANDLE.lock().ok();
            if let Some((h, tx)) = g.as_mut().and_then(|g| g.take()) {
                let _ = tx.send(());
                h.abort();
                say(tui, "gateway stopped");
            } else {
                say(
                    tui,
                    "gateway not running in this session (if installed as a service: /gateway uninstall)",
                );
            }
        }
        GatewayAction::Install => match with_modal_sync(tui, gray_gateway::systemd::install) {
            Ok(()) => say(tui, "gateway installed as systemd user service"),
            Err(e) => say(tui, &format!("gateway install failed: {e}")),
        },
        GatewayAction::Uninstall => match with_modal_sync(tui, gray_gateway::systemd::uninstall) {
            Ok(()) => say(tui, "gateway systemd service removed"),
            Err(e) => say(tui, &format!("gateway uninstall failed: {e}")),
        },
        GatewayAction::Pairing(args) => {
            use gray_gateway::pairing::{pairing_approve, pairing_list, pairing_revoke};
            let out = match args {
                PairingArgs::Approve(p, c) => pairing_approve(&p, &c),
                PairingArgs::List(p) => pairing_list(p.as_deref()),
                PairingArgs::Revoke(p, u) => pairing_revoke(&p, &u),
            };
            match out {
                Ok(s) => say(tui, &s),
                Err(e) => say(tui, &format!("pairing: {e}")),
            }
        }
        GatewayAction::Help => {
            for line in gateway_status_lines(&gray_gateway::config::load_gateway_config(), false) {
                say(tui, &line);
            }
        }
    }
}
