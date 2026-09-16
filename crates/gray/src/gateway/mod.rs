//! Gateway host: gray's daemon shape, ported from hermes' gateway craft.
//!
//! What lives here: process lifecycle (pid claim + state), the control socket
//! (`identify`/`status` — a connectable socket with a well-formed answer IS
//! liveness), supervisor integration (runit / systemd-user install + drive),
//! and the 60s cron tick loop that makes `gray cron` fire with no REPL open.
//!
//! What deliberately does NOT live here: any messaging adapter. Transports
//! are plugin-template territory (`gateway/inbound` + `gateway/deliver` on
//! the plugin wire); the daemon stays provider-agnostic.

pub mod pid;
pub mod service;
pub mod socket;
pub mod state;

mod run;

use std::path::Path;

/// Dispatch `gray gateway <cmd>`.
pub async fn run_cli(cmd: crate::GatewayCmd, config: &crate::config::Config) -> anyhow::Result<()> {
    use crate::GatewayCmd;
    // Native service/IPC lifecycle is not implemented in this preview. Reject
    // before creating pid/socket state or calling Unix service managers.
    anyhow::ensure!(
        !cfg!(windows),
        "gateway is not supported on native Windows; use WSL for background scheduling"
    );
    match cmd {
        GatewayCmd::Run => run::run_foreground(config).await,
        GatewayCmd::Status => {
            let home = crate::setup::gray_home()?;
            let (running, lines) = report(&home);
            for line in lines {
                println!("{line}");
            }
            if !running {
                std::process::exit(1);
            }
            Ok(())
        }
        GatewayCmd::Start => {
            println!("{}", service::start()?);
            Ok(())
        }
        GatewayCmd::Stop => {
            println!("{}", service::stop()?);
            Ok(())
        }
        GatewayCmd::Restart => {
            println!("{}", service::restart()?);
            Ok(())
        }
        GatewayCmd::Install { no_start, print } => {
            let home = crate::setup::gray_home()?;
            let exe = std::env::current_exe()?;
            println!("{}", service::install(&home, &exe, no_start, print)?);
            Ok(())
        }
        GatewayCmd::Uninstall => {
            println!("{}", service::uninstall()?);
            Ok(())
        }
    }
}

/// Human report for `gateway status`. Live socket first (hermes liveness
/// doctrine), pid file fallback, state file for "why did it stop".
fn report(home: &Path) -> (bool, Vec<String>) {
    let now = crate::cron::now_secs();
    let sup = service::detect();
    let mut lines = Vec::new();
    let mut running = false;

    if let Some(result) = socket::query(home, "status") {
        running = true;
        let pid = result.get("pid").and_then(|v| v.as_u64()).unwrap_or(0);
        let uptime = result
            .get("uptime_secs")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        lines.push(format!(
            "gateway: running (pid {pid}, up {})",
            crate::cron_status::ago_short(uptime)
        ));
        lines.push(format!(
            "  socket: {} — answered status (protocol {})",
            socket::sock_path(home).display(),
            socket::PROTOCOL
        ));
        if let Some(kind) = result.get("supervisor").and_then(|v| v.as_str()) {
            lines.push(format!("  supervisor: {kind}"));
        }
    } else if let Some(rec) = pid::running(home) {
        running = true;
        lines.push(format!(
            "gateway: running (pid {}, up {}) — pid file only, no socket answer",
            rec.pid,
            crate::cron_status::ago_short(now.saturating_sub(rec.started_at))
        ));
    } else {
        lines.push("gateway: not running".to_string());
        let sock = socket::sock_path(home);
        if sock.exists() {
            lines.push(format!("  socket: stale file at {}", sock.display()));
        }
        if let Some(st) = state::read(home) {
            lines.push(format!(
                "  last state: {} ({}) — {} ago",
                st.gateway_state,
                st.exit_reason.as_deref().unwrap_or("no reason recorded"),
                crate::cron_status::ago_short(now.saturating_sub(st.updated_at))
            ));
        }
    }

    // Cron liveness — the reason this daemon exists.
    match crate::cron::CronStore::open(home.join("cron")) {
        Ok(store) => {
            let health = store.health(now).unwrap_or_default();
            let jobs = store.list().map(|v| v.len()).unwrap_or(0);
            lines.push(format!("  jobs: {jobs}"));
            for l in crate::cron_status::ticker_line(&health, now).lines() {
                lines.push(format!("  cron: {l}"));
            }
        }
        Err(e) => lines.push(format!("  cron: store unavailable: {e:#}")),
    }
    lines.push(format!("  service: {}", service::describe(&sup)));
    (running, lines)
}
