//! Supervisor-facing lifecycle: install/uninstall a user service and drive it.
//!
//! Two backends, chosen from the real init (pid 1) plus env — never inferred
//! outside-in (hermes doctrine):
//!   - runit: a `runsvdir` tree, `$SVDIR` or `~/.config/service` (Void's
//!     native user supervision),
//!   - systemd: `--user` units when pid 1 is systemd.
//!
//! `install` writes the service and (unless `--no-start`) starts it; `run` is
//! what the supervisor executes. The gateway never daemonizes itself: one
//! supervisor per box, and it owns restarts.

use std::path::{Path, PathBuf};

pub const SERVICE_NAME: &str = "gray-gateway";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Supervisor {
    Runit { dir: PathBuf },
    SystemdUser { unit_dir: PathBuf },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceStatus {
    pub installed: bool,
    pub up: bool,
    /// runsvdir has picked the service up (runit); always true for systemd units.
    pub supervised: bool,
    pub pid: Option<u32>,
    pub detail: String,
}

/// Where *this* process was launched from (identify's `supervisor` field).
pub fn supervisor_kind() -> String {
    if std::env::var_os("INVOCATION_ID").is_some() {
        return "systemd".to_string();
    }
    if std::env::var_os("SVDIR").is_some() || parent_comm().as_deref() == Some("runsv") {
        return "runit".to_string();
    }
    "manual".to_string()
}

fn parent_comm() -> Option<String> {
    #[cfg(unix)]
    {
        // SAFETY: getppid has no failure mode.
        let ppid = unsafe { libc::getppid() };
        proc_comm(ppid as u32)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn proc_comm(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
}

fn find_in_path(cmd: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(cmd))
        .find(|p| p.is_file())
}

/// `$SVDIR` when set (the same override `sv` honors), else `~/.config/service`.
pub fn svdir() -> PathBuf {
    if let Some(dir) = std::env::var_os("SVDIR") {
        return PathBuf::from(dir);
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".config/service");
    }
    PathBuf::from("/var/service")
}

fn systemd_user_dir() -> PathBuf {
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(x).join("systemd/user");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".config/systemd/user")
}

pub fn detect() -> Supervisor {
    let pid1 = proc_comm(1);
    if matches!(pid1.as_deref(), Some("runit" | "runsvdir")) {
        return Supervisor::Runit { dir: svdir() };
    }
    if pid1.as_deref() == Some("systemd") && find_in_path("systemctl").is_some() {
        return Supervisor::SystemdUser {
            unit_dir: systemd_user_dir(),
        };
    }
    // Not the init we expected: fall back on the tools that are actually here.
    if find_in_path("sv").is_some() {
        return Supervisor::Runit { dir: svdir() };
    }
    if find_in_path("systemctl").is_some() {
        return Supervisor::SystemdUser {
            unit_dir: systemd_user_dir(),
        };
    }
    Supervisor::None
}

pub fn status(sup: &Supervisor) -> ServiceStatus {
    match sup {
        Supervisor::Runit { dir } => {
            let svc = dir.join(SERVICE_NAME);
            if !svc.join("run").is_file() {
                return ServiceStatus {
                    installed: false,
                    up: false,
                    supervised: false,
                    pid: None,
                    detail: format!("not installed (runit tree {})", dir.display()),
                };
            }
            let supervised = supervise_ok(&svc);
            match run_sv(&svc, &["status"]) {
                Ok(out) => {
                    let text = output_text(&out);
                    ServiceStatus {
                        installed: true,
                        up: supervised && out.status.success() && text.contains("run:"),
                        supervised,
                        pid: parse_pid(&text),
                        detail: if supervised {
                            format!("runit ({})", svc.display())
                        } else {
                            format!(
                                "runit ({}), runsvdir has not picked it up yet",
                                svc.display()
                            )
                        },
                    }
                }
                Err(e) => ServiceStatus {
                    installed: true,
                    up: false,
                    supervised,
                    pid: None,
                    detail: format!("runit ({}), sv failed: {e}", svc.display()),
                },
            }
        }
        Supervisor::SystemdUser { unit_dir } => {
            let unit = unit_dir.join(format!("{SERVICE_NAME}.service"));
            if !unit.is_file() {
                return ServiceStatus {
                    installed: false,
                    up: false,
                    supervised: false,
                    pid: None,
                    detail: format!("not installed (systemd user units {})", unit_dir.display()),
                };
            }
            let active = systemctl(&["is-active", SERVICE_NAME])
                .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "active")
                .unwrap_or(false);
            let pid = systemctl(&["show", "-p", "MainPID", "--value", SERVICE_NAME])
                .ok()
                .and_then(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .trim()
                        .parse::<u32>()
                        .ok()
                })
                .filter(|p| *p > 0);
            ServiceStatus {
                installed: true,
                up: active,
                supervised: true,
                pid,
                detail: format!("systemd user unit ({})", unit.display()),
            }
        }
        Supervisor::None => ServiceStatus {
            installed: false,
            up: false,
            supervised: false,
            pid: None,
            detail: "no supervisor detected — run `gray gateway run` under your own daemon manager"
                .to_string(),
        },
    }
}

/// One line for `gateway status`.
pub fn describe(sup: &Supervisor) -> String {
    let st = status(sup);
    match (st.installed, st.up) {
        (false, _) if matches!(sup, Supervisor::None) => st.detail.clone(),
        (false, _) => format!("{}; install with `gray gateway install`", st.detail),
        (true, true) => format!(
            "{} — up{}",
            st.detail,
            st.pid.map(|p| format!(" (pid {p})")).unwrap_or_default()
        ),
        (true, false) if !st.supervised => st.detail.clone(),
        (true, false) => format!("{} — down", st.detail),
    }
}

pub fn install(
    home: &Path,
    exe: &Path,
    no_start: bool,
    print_only: bool,
) -> anyhow::Result<String> {
    match detect() {
        Supervisor::Runit { dir } => install_runit(home, exe, &dir, no_start, print_only),
        Supervisor::SystemdUser { unit_dir } => {
            install_systemd(home, exe, &unit_dir, no_start, print_only)
        }
        Supervisor::None => anyhow::bail!(
            "no supervisor detected (runit tree or systemd user session) — run `gray gateway run` under your own daemon manager"
        ),
    }
}

pub fn uninstall() -> anyhow::Result<String> {
    match detect() {
        Supervisor::Runit { dir } => {
            let svc = dir.join(SERVICE_NAME);
            if !svc.exists() {
                return Ok(format!("nothing installed at {}", svc.display()));
            }
            let _ = run_sv(&svc, &["-w", "10", "down"]);
            wait_runit_down(&svc, std::time::Duration::from_secs(20));
            std::fs::remove_dir_all(&svc)?;
            Ok(format!("removed {}", svc.display()))
        }
        Supervisor::SystemdUser { unit_dir } => {
            let unit = unit_dir.join(format!("{SERVICE_NAME}.service"));
            if !unit.exists() {
                return Ok(format!("nothing installed at {}", unit.display()));
            }
            let _ = systemctl(&["disable", "--now", SERVICE_NAME]);
            std::fs::remove_file(&unit)?;
            let _ = systemctl(&["daemon-reload"]);
            Ok(format!("removed {}", unit.display()))
        }
        Supervisor::None => Ok("no supervisor detected; nothing to uninstall".to_string()),
    }
}

pub fn start() -> anyhow::Result<String> {
    let home = crate::setup::gray_home()?;
    match detect() {
        Supervisor::Runit { dir } => {
            let svc = dir.join(SERVICE_NAME);
            if !svc.join("run").is_file() {
                anyhow::bail!(
                    "no service installed — `gray gateway install` (or `gray gateway run` for the foreground)"
                );
            }
            require_supervised(&svc, &dir)?;
            let out = run_sv(&svc, &["-w", "7", "up"])?;
            let text = output_text(&out);
            if !out.status.success() {
                anyhow::bail!("sv up failed: {}", text.trim());
            }
            Ok(settle_line(format!("started {}", svc.display()), &home))
        }
        Supervisor::SystemdUser { unit_dir } => {
            let unit = unit_dir.join(format!("{SERVICE_NAME}.service"));
            if !unit.is_file() {
                anyhow::bail!(
                    "no service installed — `gray gateway install` (or `gray gateway run` for the foreground)"
                );
            }
            let out = systemctl(&["start", SERVICE_NAME])?;
            if !out.status.success() {
                anyhow::bail!("systemctl start failed: {}", output_text(&out).trim());
            }
            Ok(settle_line(format!("started {SERVICE_NAME}"), &home))
        }
        Supervisor::None => anyhow::bail!(
            "no supervisor detected — run `gray gateway run` under your own daemon manager"
        ),
    }
}

pub fn stop() -> anyhow::Result<String> {
    let home = crate::setup::gray_home()?;
    match detect() {
        Supervisor::Runit { dir } => {
            let svc = dir.join(SERVICE_NAME);
            if svc.join("run").is_file() && supervise_ok(&svc) {
                let out = run_sv(&svc, &["-w", "10", "down"])?;
                if !out.status.success() {
                    anyhow::bail!("sv down failed: {}", output_text(&out).trim());
                }
                wait_runit_down(&svc, std::time::Duration::from_secs(20));
                if status(&Supervisor::Runit { dir: dir.clone() }).up {
                    return Ok(format!(
                        "stopped {} (sv confirms it will stay down; an in-flight tick drains up to 65s)",
                        svc.display()
                    ));
                }
                Ok(format!("stopped {}", svc.display()))
            } else {
                stop_by_pid(&home)
            }
        }
        Supervisor::SystemdUser { .. } => {
            let out = systemctl(&["stop", SERVICE_NAME])?;
            if !out.status.success() {
                anyhow::bail!("systemctl stop failed: {}", output_text(&out).trim());
            }
            Ok(format!("stopped {SERVICE_NAME}"))
        }
        Supervisor::None => stop_by_pid(&home),
    }
}

pub fn restart() -> anyhow::Result<String> {
    let home = crate::setup::gray_home()?;
    match detect() {
        Supervisor::Runit { dir } => {
            let svc = dir.join(SERVICE_NAME);
            if !svc.join("run").is_file() {
                anyhow::bail!(
                    "no service installed — `gray gateway install` (or `gray gateway run` for the foreground)"
                );
            }
            require_supervised(&svc, &dir)?;
            let out = run_sv(&svc, &["-w", "10", "restart"])?;
            if !out.status.success() {
                anyhow::bail!("sv restart failed: {}", output_text(&out).trim());
            }
            Ok(settle_line(format!("restarted {}", svc.display()), &home))
        }
        Supervisor::SystemdUser { unit_dir } => {
            let unit = unit_dir.join(format!("{SERVICE_NAME}.service"));
            if !unit.is_file() {
                anyhow::bail!(
                    "no service installed — `gray gateway install` (or `gray gateway run` for the foreground)"
                );
            }
            let out = systemctl(&["restart", SERVICE_NAME])?;
            if !out.status.success() {
                anyhow::bail!("systemctl restart failed: {}", output_text(&out).trim());
            }
            Ok(settle_line(format!("restarted {SERVICE_NAME}"), &home))
        }
        Supervisor::None => anyhow::bail!(
            "no supervisor detected — run `gray gateway run` under your own daemon manager"
        ),
    }
}

/// runsv owns a picked-up service dir; without this, `sv` refuses to talk.
fn supervise_ok(svc: &Path) -> bool {
    svc.join("supervise/ok").exists()
}

fn wait_for_supervise(svc: &Path, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if supervise_ok(svc) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// True when a live `runsvdir` names this directory on its command line —
/// cheaper and more honest than timing out on a scan that will never come.
fn runsvdir_supervises(dir: &Path) -> bool {
    let needle = dir.display().to_string();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return true; // no /proc: let the wait loop decide
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
            continue;
        };
        let text = String::from_utf8_lossy(&cmdline).replace('\0', " ");
        if text.contains("runsvdir") && text.contains(&needle) {
            return true;
        }
    }
    false
}

fn require_supervised(svc: &Path, dir: &Path) -> anyhow::Result<()> {
    if supervise_ok(svc) {
        return Ok(());
    }
    if !runsvdir_supervises(dir) {
        anyhow::bail!(
            "runsvdir is not supervising {} — start `runsvdir -P {}` first",
            dir.display(),
            dir.display()
        );
    }
    if !wait_for_supervise(svc, std::time::Duration::from_secs(10)) {
        anyhow::bail!(
            "runsvdir has not picked up {} yet — try again in a moment",
            svc.display()
        );
    }
    Ok(())
}

fn stop_by_pid(home: &Path) -> anyhow::Result<String> {
    let Some(rec) = super::pid::running(home) else {
        return Ok("gateway is not running".to_string());
    };
    #[cfg(unix)]
    {
        // SAFETY: SIGTERM is the documented stop path for a manual foreground run.
        unsafe { libc::kill(rec.pid as libc::pid_t, libc::SIGTERM) };
    }
    #[cfg(not(unix))]
    {
        anyhow::bail!(
            "cannot signal pid {} on this platform — end the `gateway run` process manually",
            rec.pid
        );
    }
    #[cfg(unix)]
    {
        for _ in 0..80 {
            if super::pid::running(home).is_none() {
                return Ok(format!("stopped (pid {})", rec.pid));
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        anyhow::bail!(
            "pid {} is still up after 20s — kill -9 {} only if `gray gateway status` still shows it running",
            rec.pid,
            rec.pid
        )
    }
}

fn install_runit(
    home: &Path,
    exe: &Path,
    dir: &Path,
    no_start: bool,
    print_only: bool,
) -> anyhow::Result<String> {
    let svc = dir.join(SERVICE_NAME);
    let run = runit_run_script(home, exe);
    let log_run = runit_log_script(home);
    if print_only {
        return Ok(format!(
            "would write {}:\n{run}\nwould write {}/log/run:\n{log_run}",
            svc.join("run").display(),
            svc.display()
        ));
    }
    std::fs::create_dir_all(svc.join("log"))?;
    std::fs::create_dir_all(home.join("logs/gateway"))?;
    write_executable(&svc.join("run"), &run)?;
    write_executable(&svc.join("log/run"), &log_run)?;
    let down = svc.join("down");
    if no_start {
        std::fs::write(&down, "")?;
    } else {
        let _ = std::fs::remove_file(&down);
    }
    let mut msg = format!("installed runit service: {}", svc.display());
    if no_start {
        msg.push_str("\nnot started (--no-start); start with `gray gateway start`");
        return Ok(msg);
    }
    if !runsvdir_supervises(dir) {
        msg.push_str(&format!(
            "\n⚠ runsvdir is not supervising {} — start `runsvdir -P {}`, then `gray gateway start`",
            dir.display(),
            dir.display()
        ));
        return Ok(msg);
    }
    if !wait_for_supervise(&svc, std::time::Duration::from_secs(10)) {
        msg.push_str(&format!(
            "\n⚠ runsvdir has not picked up {} yet — try `gray gateway start` in a moment",
            svc.display()
        ));
        return Ok(msg);
    }
    match run_sv(&svc, &["-w", "7", "up"]) {
        Ok(out) if out.status.success() => {
            std::thread::sleep(std::time::Duration::from_millis(300));
            msg.push_str("\nstarted — check with `gray gateway status`");
        }
        Ok(out) => msg.push_str(&format!(
            "\n⚠ service not started: {}{}\n  is runsvdir supervising {}?",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
            dir.display()
        )),
        Err(e) => msg.push_str(&format!(
            "\n⚠ service not started: {e}\n  is runsvdir supervising {}?",
            dir.display()
        )),
    }
    Ok(msg)
}

fn install_systemd(
    home: &Path,
    exe: &Path,
    unit_dir: &Path,
    no_start: bool,
    print_only: bool,
) -> anyhow::Result<String> {
    let unit = unit_dir.join(format!("{SERVICE_NAME}.service"));
    let body = systemd_unit_body(home, exe);
    if print_only {
        return Ok(format!("would write {}:\n{body}", unit.display()));
    }
    std::fs::create_dir_all(unit_dir)?;
    std::fs::write(&unit, &body)?;
    let _ = systemctl(&["daemon-reload"]);
    let mut msg = format!("installed systemd user unit: {}", unit.display());
    if no_start {
        msg.push_str(
            "\nnot enabled (--no-start); enable with `systemctl --user enable --now gray-gateway`",
        );
        return Ok(msg);
    }
    match systemctl(&["enable", "--now", SERVICE_NAME]) {
        Ok(out) if out.status.success() => {
            msg.push_str("\nstarted — check with `gray gateway status`")
        }
        Ok(out) => msg.push_str(&format!(
            "\n⚠ not started: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )),
        Err(e) => msg.push_str(&format!("\n⚠ not started: {e}")),
    }
    Ok(msg)
}

fn settle_line(msg: String, home: &Path) -> String {
    std::thread::sleep(std::time::Duration::from_millis(300));
    match super::socket::query(home, "identify") {
        Some(answer) => {
            let pid = answer.get("pid").and_then(|v| v.as_u64()).unwrap_or(0);
            format!("{msg}\nlive: pid {pid} answers identify on the control socket")
        }
        None => format!("{msg}\n⚠ no socket answer yet — check `gray gateway status`"),
    }
}

fn wait_runit_down(svc: &Path, timeout: std::time::Duration) {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        let up = run_sv(svc, &["status"])
            .map(|o| output_text(&o).contains("run:"))
            .unwrap_or(false);
        if !up {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// runit `run` script: what the supervisor executes. `exec` keeps gray as
/// runsv's direct child, which is what `supervisor_kind()` reads back.
fn runit_run_script(home: &Path, exe: &Path) -> String {
    format!(
        "#!/bin/sh\n# {SERVICE_NAME} — written by `gray gateway install`; `gray gateway uninstall` removes it.\nexport GRAY_HOME={home}\ncd \"$HOME\"\nexec {exe} gateway run\n",
        home = shell_quote(&home.display().to_string()),
        exe = shell_quote(&exe.display().to_string()),
    )
}

fn runit_log_script(home: &Path) -> String {
    let logs = shell_quote(&home.join("logs/gateway").display().to_string());
    format!("#!/bin/sh\nmkdir -p {logs}\nexec svlogd -tt {logs}\n")
}

fn systemd_unit_body(home: &Path, exe: &Path) -> String {
    let exe = exe.display().to_string().replace('"', "\\\"");
    format!(
        "[Unit]\nDescription=gray gateway (cron ticker + control socket)\nAfter=default.target\n\n[Service]\nType=exec\nEnvironment=GRAY_HOME={home}\nExecStart=\"{exe}\" gateway run\nRestart=always\nRestartSec=2\nKillSignal=SIGTERM\nTimeoutStopSec=75\n\n[Install]\nWantedBy=default.target\n",
        home = home.display(),
    )
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn write_executable(path: &Path, body: &str) -> anyhow::Result<()> {
    std::fs::write(path, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perm = std::fs::metadata(path)?.permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(path, perm)?;
    }
    Ok(())
}

fn run_sv(svc: &Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    std::process::Command::new("sv")
        .args(args)
        .arg(svc)
        .output()
}

fn systemctl(args: &[&str]) -> std::io::Result<std::process::Output> {
    std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
}

fn output_text(out: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// First `(pid N)` in an `sv status` line.
fn parse_pid(text: &str) -> Option<u32> {
    let start = text.find("(pid ")? + 5;
    text[start..]
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

#[path = "service_tests.rs"]
#[cfg(test)]
mod tests;
