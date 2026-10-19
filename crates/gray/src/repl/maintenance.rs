use super::TuiOpt;

const CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

pub(crate) async fn handle_update(tui: &TuiOpt, arg: Option<String>) {
    let shared = tui.as_ref().map(|(s, _)| s);
    if arg.as_deref().map(str::trim) == Some("everything") {
        crate::quality_of_life::update::handle(tui).await;
        return;
    }
    if let Some(rest) = arg.as_deref().map(str::trim)
        && let Some(sub) = rest.strip_prefix("auto")
    {
        let word = sub.trim();
        match word {
            "on" | "off" => {
                let on = word == "on";
                let text = match crate::auto_update::set_enabled(on) {
                    Ok(()) if on => "auto-update on".to_string(),
                    Ok(()) => "auto-update off — `gray update` still installs manually".to_string(),
                    Err(e) => format!("auto-update toggle failed: {e:#}"),
                };
                super::say(shared, &text);
            }
            "status" | "" => super::say(shared, &crate::auto_update::status_text()),
            other => super::say(
                shared,
                &format!("unknown `/update auto {other}` (on|off|status)"),
            ),
        }
        return;
    }
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

    let result = with_terminal(shared, crate::update::install);
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

pub(crate) async fn handle_restart(tui: &TuiOpt) {
    restart_with(tui, true).await;
}

pub(crate) async fn restart_with(tui: &TuiOpt, resume: bool) {
    let shared = tui.as_ref().map(|(s, _)| s);

    match restart_gateway().await {
        GatewayRestart::Restarted(line) => super::say(shared, &line),
        GatewayRestart::NotRunning => {
            super::say(shared, "gateway is not running — nothing to restart there")
        }
        GatewayRestart::Skipped => super::say(shared, "gateway: skipped (scratch GRAY_HOME)"),
        GatewayRestart::Failed(e) => super::say(shared, &format!("gateway restart failed: {e}")),
    }

    super::drain_in_flight_turn(std::time::Duration::from_millis(1500)).await;

    let exe = match crate::exe_watch::on_disk_exe() {
        Ok(p) => p,
        Err(e) => {
            super::say(
                shared,
                &format!("cannot find the running binary ({e}) — restart gray yourself"),
            );
            return;
        }
    };
    let original: Vec<String> = std::env::args().skip(1).collect();
    let args = if !resume {
        strip_resume_args(&original)
    } else {
        restart_argv(original, crate::host::live_session().as_deref())
    };
    let resumed = args.first().map(|a| a == "resume").unwrap_or(false);

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

    #[cfg(unix)]
    let spawned: std::io::Result<()> = {
        use std::os::unix::process::CommandExt as _;
        Err(std::process::Command::new(&exe).args(&args).exec())
    };
    #[cfg(not(unix))]
    let spawned: std::io::Result<()> = match std::process::Command::new(&exe).args(&args).spawn() {
        Ok(_) => std::process::exit(0),
        Err(e) => Err(e),
    };
    if let Err(e) = spawned {
        let _ = crossterm::terminal::enable_raw_mode();
        super::say(
            shared,
            &format!("restart failed to spawn ({e}) — still on the old build"),
        );
    }
}

enum GatewayRestart {
    Restarted(String),
    NotRunning,

    Skipped,
    Failed(String),
}

fn scratch_gray_home() -> bool {
    let Some(set) =
        std::env::var_os("GRAY_HOME").filter(|v| !v.to_string_lossy().trim().is_empty())
    else {
        return false;
    };
    let default = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".gray"));
    is_scratch_home(std::path::Path::new(&set), default.as_deref())
}

fn is_scratch_home(set: &std::path::Path, default: Option<&std::path::Path>) -> bool {
    default != Some(set)
}

fn gateway_cmd(args: &[&str]) -> std::io::Result<std::process::Output> {
    let exe = crate::exe_watch::on_disk_exe()?;
    std::process::Command::new(exe)
        .arg("gateway")
        .args(args)
        .output()
}

async fn restart_gateway() -> GatewayRestart {
    if scratch_gray_home() {
        return GatewayRestart::Skipped;
    }

    if let Ok(out) = gateway_cmd(&["restart"]) {
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if out.status.success() {
            return GatewayRestart::Restarted(text.trim().to_string());
        }
        if !text.contains("no service installed") && !text.contains("not running") {
            return GatewayRestart::Failed(text.trim().to_string());
        }
    }

    let Ok(home) = crate::setup::gray_home() else {
        return GatewayRestart::NotRunning;
    };
    let Some(rec) = crate::gateway_pid::running(&home) else {
        return GatewayRestart::NotRunning;
    };

    match gateway_cmd(&["stop"]) {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return GatewayRestart::Failed(format!(
                "could not stop the running gateway: {}",
                text.trim()
            ));
        }
        Err(e) => {
            return GatewayRestart::Failed(format!("could not stop the running gateway: {e}"));
        }
    }
    match relaunch(&home, rec.pid) {
        Ok(line) => GatewayRestart::Restarted(line),
        Err(e) => GatewayRestart::Failed(format!(
            "stopped the gateway (pid {}) but could not start it again: {e} — `gray gateway run` when you are back",
            rec.pid
        )),
    }
}

fn relaunch(home: &std::path::Path, old_pid: u32) -> anyhow::Result<String> {
    use std::process::Stdio;
    let exe = crate::exe_watch::on_disk_exe()?;
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

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if let Some(now) = crate::gateway_pid::running(home) {
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

pub(crate) fn restart_argv(original: Vec<String>, current: Option<&str>) -> Vec<String> {
    let Some(sid) = current else {
        return if original.is_empty() {
            vec!["resume".into(), "--last".into()]
        } else {
            original
        };
    };
    if has_session_selector(&original) {
        return original;
    }
    vec!["resume".into(), sid.to_string()]
}

fn has_session_selector(args: &[String]) -> bool {
    args.iter().any(|a| {
        matches!(
            a.as_str(),
            "resume" | "-c" | "--continue" | "-r" | "--resume" | "--session"
        ) || a.starts_with("--session=")
            || a.starts_with("--resume=")
            || (a.starts_with("-r") && a.len() > 2)
    })
}

fn strip_resume_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = args.iter().peekable();
    if it.peek().map(|a| a.as_str()) == Some("resume") {
        it.next();
        while let Some(a) = it.peek() {
            match a.as_str() {
                "--last" | "--all" => {
                    it.next();
                }
                s if !s.starts_with('-') => {
                    it.next();
                    break;
                }
                _ => break,
            }
        }
    }
    while let Some(a) = it.next() {
        match a.as_str() {
            "-c" | "--continue" | "--session" | "-r" | "--resume" => {
                if matches!(a.as_str(), "--session" | "-r" | "--resume")
                    && let Some(v) = it.peek()
                    && !v.starts_with('-')
                {
                    it.next();
                }
            }
            s if s.starts_with("--session=")
                || s.starts_with("--resume=")
                || s.starts_with("-r") && s.len() > 2 => {}
            _ => out.push(a.clone()),
        }
    }
    out
}

async fn confirm_yn() -> bool {
    use crossterm::event::{self, Event, KeyCode, KeyEvent};

    if crate::composer::input::stdin_gone() {
        return false;
    }

    let got = tokio::task::block_in_place(event::read);
    matches!(
        got,
        Ok(Event::Key(KeyEvent {
            code: KeyCode::Char('y' | 'Y'),
            ..
        }))
    )
}

pub(crate) fn with_terminal<T>(
    shared: Option<&crate::composer::SharedTui>,
    f: impl FnOnce() -> T,
) -> T {
    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    let _ = crossterm::terminal::disable_raw_mode();
    if let Some(shared) = shared {
        shared.lock().expect("tui lock").release_for_child();
    }
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
    fn a_scratch_gray_home_never_reaches_the_system_gateway() {
        let default = std::path::Path::new("/home/me/.gray");

        assert!(is_scratch_home(
            std::path::Path::new("/tmp/vp_home"),
            Some(default)
        ));
        assert!(is_scratch_home(std::path::Path::new("/tmp/vp_home"), None));
        assert!(!is_scratch_home(
            std::path::Path::new("/home/me/.gray/"),
            Some(default)
        ));
    }

    #[test]
    fn a_bare_session_resumes_its_own_conversation() {
        assert_eq!(
            restart_argv(vec![], Some("viscous-lithium-meridian")),
            vec!["resume", "viscous-lithium-meridian"]
        );

        assert_eq!(restart_argv(vec![], None), vec!["resume", "--last"]);
    }

    #[test]
    fn anything_the_user_typed_is_passed_through() {
        for args in [
            vec!["resume".to_string(), "--last".to_string()],
            vec!["-r".to_string(), "other-session".to_string()],
            vec!["--session=x".to_string()],
            vec!["-c".to_string()],
        ] {
            assert_eq!(
                restart_argv(args.clone(), Some("viscous-lithium-meridian")),
                args
            );
        }

        assert_eq!(
            restart_argv(vec!["-p".to_string(), "hello".to_string()], None),
            vec!["-p", "hello"]
        );
    }
}
