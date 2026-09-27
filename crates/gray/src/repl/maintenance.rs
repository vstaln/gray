//! `/update` and `/restart` — the two commands that make a running gray
//! become a *current* gray.
//!
//! Both exist because a self-update is a two-step thing nobody completes:
//! `gray update` installs a new binary and tells you to restart, and the
//! thing that needs restarting is not the shell you typed it in. It is the
//! REPL you are sitting in, and the gateway daemon, which keeps running the
//! build it started with until someone restarts it. So `/update` installs and
//! then says "run /restart", and `/restart` does both halves.
//!
//! Neither codex nor hermes has these: codex shows an update *popup* at
//! startup and hands you a `brew upgrade` line, and hermes-rs's `restart`
//! tears down LSP clients. A CLI that installs its own updates owes the user
//! a way to actually land on them.
//!
//! The re-exec is the delicate part. `/restart` restores the terminal before
//! spawning, exactly like the account commands do, and re-enters the
//! conversation with `resume --last` when it was an interactive session — a
//! restart that silently dropped your conversation would be a worse bug than
//! not shipping it.

use super::TuiOpt;
use crate::gateway::service;

/// How long a version check may take before we call the network a day. Same
/// budget as the startup check: this is a convenience, not a dependency.
const CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

/// `/update` — check the channel, and install when there is something newer.
///
/// Installs only after the user says so: the installer is a mutable HTTPS
/// script, so `/update` asks the way `gray update` does and reports exactly
/// what happened, including the PATH-shadowing warning that bites right after
/// a self-update.
pub(crate) async fn handle_update(tui: &TuiOpt) {
    let shared = tui.as_ref().map(|(s, _)| s);
    let current = env!("CARGO_PKG_VERSION");
    super::say(
        shared,
        &format!("checking for a new gray ({})...", crate::update::CHANNEL),
    );

    let check = tokio::time::timeout(CHECK_TIMEOUT, crate::update::latest_version()).await;
    let latest = match check {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            super::say(shared, &format!("update check failed: {e}"));
            return;
        }
        Err(_) => {
            super::say(
                shared,
                "update check timed out — try `gray update` in a shell",
            );
            return;
        }
    };

    if !crate::update::update_available(
        crate::update::CHANNEL,
        &latest,
        current,
        env!("GRAY_BUILD_ID"),
    ) {
        super::say(shared, &format!("gray {current} is current"));
        return;
    }

    super::say(
        shared,
        &format!("gray {latest} available (you have {current}). Install it? [y/N]"),
    );
    if !confirm_yn().await {
        super::say(shared, "not updated");
        return;
    }

    // The installer is a child process writing to the real terminal, so raw
    // mode has to go for the duration and the composer re-anchors after.
    let result = with_terminal(shared, || crate::update::install());
    match result {
        Ok(()) => {
            crate::update::warn_on_shadow();
            super::say(
                shared,
                &format!("updated to {latest} — `/restart` to run it (the gateway too)"),
            );
        }
        Err(e) => super::say(shared, &format!("update failed: {e}")),
    }
}

/// `/restart` — put the running gray on the binary that is on disk.
///
/// Both halves, in the order that makes the message true: the gateway first
/// (it is a separate process on the old build), then this REPL re-exec'd on
/// the new one. The conversation survives via `resume --last`.
pub(crate) async fn handle_restart(tui: &TuiOpt) {
    let shared = tui.as_ref().map(|(s, _)| s);

    match restart_gateway().await {
        GatewayRestart::Restarted(line) => super::say(shared, &line),
        GatewayRestart::NotRunning => {
            super::say(shared, "gateway is not running — nothing to restart there")
        }
        GatewayRestart::Failed(e) => super::say(shared, &format!("gateway restart failed: {e}")),
    }

    // A turn may still be flushing its transcript; a restart that truncates
    // the last exchange is the one thing this command must not do.
    super::drain_in_flight_turn(std::time::Duration::from_millis(1500)).await;

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            super::say(
                shared,
                &format!("cannot find the running binary ({e}) — restart gray yourself"),
            );
            return;
        }
    };
    let args = restart_argv(std::env::args().skip(1).collect());
    let resumed = args.first().map(|a| a == "resume").unwrap_or(false);

    // Hand the terminal back before the child takes it: raw mode off, cursor
    // visible, the composer's viewport re-anchored — otherwise the child
    // inherits a terminal this process still owns.
    let _ = crossterm::terminal::disable_raw_mode();
    super::restore_viewport(shared);
    let line = if resumed {
        "\r\nrestarting gray (new build, same conversation)…\r\n"
    } else {
        "\r\nrestarting gray (new build)…\r\n"
    };
    print!("{line}");
    use std::io::Write as _;
    let _ = std::io::stdout().flush();

    match std::process::Command::new(&exe).args(&args).spawn() {
        Ok(_) => std::process::exit(0),
        Err(e) => {
            // The spawn failed, so this process is still the one running and
            // still usable: say so rather than exiting into a dead shell.
            let _ = crossterm::terminal::enable_raw_mode();
            super::say(
                shared,
                &format!("restart failed to spawn ({e}) — still on the old build"),
            );
        }
    }
}

enum GatewayRestart {
    Restarted(String),
    NotRunning,
    Failed(String),
}

/// Restart the gateway, and only ever leave it in a better state than we
/// found it.
///
/// Three cases, in the order that decides who owns the process:
///
/// 1. **Installed under a supervisor** — the supervisor restarts it. That is
///    the right owner and the only one that knows the service contract.
/// 2. **Running with nobody supervising it** — a `gray gateway run` someone
///    launched by hand, which is the common shape on a box without a service
///    installed. Nothing else will bring it back, so we stop it *and* start
///    it again on the new build. Stopping without the relaunch would take a
///    working gateway down and call it a restart.
/// 3. **Not running** — say so and move on; `/restart` is about picking up a
///    new build, not about resurrecting a daemon the user did not start.
async fn restart_gateway() -> GatewayRestart {
    let supervised = matches!(
        service::detect(),
        service::Supervisor::Runit { .. } | service::Supervisor::SystemdUser { .. }
    );
    if supervised {
        match service::restart() {
            Ok(line) => return GatewayRestart::Restarted(line),
            // Installed and failing is a real failure worth printing. Not
            // installed is not: fall through and handle the process directly.
            Err(e) if !e.to_string().contains("no service installed") => {
                return GatewayRestart::Failed(e.to_string());
            }
            Err(_) => {}
        }
    }

    let Ok(home) = crate::setup::gray_home() else {
        return GatewayRestart::NotRunning;
    };
    let Some(rec) = crate::gateway::pid::running(&home) else {
        return GatewayRestart::NotRunning;
    };
    // `stop` waits up to 20s for the pid to go and refuses to return while it
    // is still up, so the relaunch below cannot race a dying daemon.
    if let Err(e) = service::stop() {
        return GatewayRestart::Failed(format!("could not stop the running gateway: {e}"));
    }
    match relaunch(&home, rec.pid) {
        Ok(line) => GatewayRestart::Restarted(line),
        Err(e) => GatewayRestart::Failed(format!(
            "stopped the gateway (pid {}) but could not start it again: {e} — `gray gateway run` when you are back",
            rec.pid
        )),
    }
}

/// Start the gateway again, detached, on the binary that is on disk.
///
/// Its own process group, so a closed terminal or a SIGHUP aimed at this
/// REPL's group does not take the daemon with it — the same shape as the
/// `gateway run` the user had launched by hand.
fn relaunch(home: &std::path::Path, old_pid: u32) -> anyhow::Result<String> {
    use std::process::Stdio;
    let exe = std::env::current_exe()?;
    let log = home.join("logs").join("gateway.log");
    std::fs::create_dir_all(
        log.parent()
            .ok_or_else(|| anyhow::anyhow!("{} has no parent", log.display()))?,
    )?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["gateway", "run"])
        .stdin(Stdio::null())
        .stdout(Stdio::from(file.try_clone()?))
        .stderr(Stdio::from(file));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }
    cmd.spawn()?;

    // Report what is true, not what we hope: a spawn is not a running daemon.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if let Some(now) = crate::gateway::pid::running(home) {
            return Ok(format!(
                "gateway restarted on the new build (pid {} was {old_pid}, log: {})",
                now.pid,
                log.display()
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Ok(format!(
        "gateway stopped (pid {old_pid}) and relaunched; it has not reported in yet — log: {}",
        log.display()
    ))
}

/// What the child process gets. An interactive session (a bare `gray`, no
/// subcommand) resumes the conversation it was having; anything else — a
/// `-p` prompt, `resume --last`, a subcommand — is passed through verbatim,
/// because re-running a one-shot prompt or a subcommand is the caller's
/// business, not ours.
pub(crate) fn restart_argv(original: Vec<String>) -> Vec<String> {
    if original.is_empty() {
        vec!["resume".into(), "--last".into()]
    } else {
        original
    }
}

/// One keypress, y/N, read the way the TUI reads keys.
///
/// Not `update::confirm()`: that helper switches raw mode off when it is
/// done, which is right for a shell and wrong here — the composer owns raw
/// mode and would be left reading a cooked terminal. Dispatch runs between
/// input events, so nobody else is reading stdin at this moment.
async fn confirm_yn() -> bool {
    use crossterm::event::{self, Event, KeyCode, KeyEvent};
    // Callers run under the multi-thread runtime `main` installs, which
    // `block_in_place` requires.
    let got = tokio::task::block_in_place(|| event::read());
    matches!(
        got,
        Ok(Event::Key(KeyEvent {
            code: KeyCode::Char('y' | 'Y'),
            ..
        }))
    )
}

/// Run a blocking child-process flow with the terminal in cooked mode, then
/// put the composer back. Same contract as the account commands.
fn with_terminal<T>(shared: Option<&crate::composer::SharedTui>, f: impl FnOnce() -> T) -> T {
    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    let _ = crossterm::terminal::disable_raw_mode();
    let out = f();
    if was_raw {
        let _ = crossterm::terminal::enable_raw_mode();
    }
    super::restore_viewport(shared);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_session_resumes_its_own_conversation() {
        // The conversation is the session; a restart that dropped it would be
        // worse than not shipping the command.
        assert_eq!(restart_argv(vec![]), vec!["resume", "--last"]);
    }

    #[test]
    fn anything_the_user_typed_is_passed_through() {
        for args in [
            vec!["-p".to_string(), "hello".to_string()],
            vec!["resume".to_string(), "--last".to_string()],
            vec!["gateway".to_string(), "status".to_string()],
        ] {
            assert_eq!(restart_argv(args.clone()), args);
        }
    }
}
