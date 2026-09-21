//! shell/spawn.rs: process spawn.
//!
//! `sh -c` with null stdin, piped stdout/stderr, non-interactive env,
//! detached via setsid (pgid == pid). NO `kill_on_drop`: from now on a
//! Unix children die by explicit kill or exit. Windows owns a Job Object
//! instead: dropping the job closes any remaining descendants too.
//!
//! Intentional non-gate (GRY-01 triage): no command guard, no approval
//! prompt, no container/VM isolation — model `bash` runs with user
//! privileges (see SECURITY.md + README Safety). Containment here is only
//! timeout + process-group kill + output redact/fence upstream.

use std::io;
use std::path::Path;

use tokio::process::Command;

use super::contract::Spawned;

/// Spawn `command` via `sh -c` in `cwd`.
///
/// `session` is exported as `GRAY_SESSION_ID` so anything the command runs can
/// name the session it belongs to: a `gray memory set` issued from inside a
/// session stamps its entry with that session rather than the generic `cli`.
pub fn spawn(command: &str, cwd: &Path, session: Option<&str>) -> io::Result<Spawned> {
    #[cfg(not(windows))]
    let mut cmd = Command::new("sh");
    #[cfg(windows)]
    let mut cmd = {
        let shell = super::windows::shell_path()?;
        let mut cmd = Command::new(&shell);
        // Git's shell can be found without usr/bin being on PATH. Its tools
        // (cat, sleep, etc.) must be reachable inside non-login shell commands.
        let mut paths = vec![
            shell
                .parent()
                .expect("absolute executable has parent")
                .to_path_buf(),
        ];
        if let Some(path) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&path));
        }
        cmd.env(
            "PATH",
            std::env::join_paths(paths).map_err(io::Error::other)?,
        );
        // Do not source user startup files in this non-interactive tool.
        cmd.env_remove("BASH_ENV").env_remove("ENV");
        cmd
    };
    cmd.arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("DEBIAN_FRONTEND", "noninteractive")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("SUDO_ASKPASS", "/bin/false")
        .env("TERM", "dumb")
        .env("PAGER", "cat")
        .env("MANPAGER", "cat")
        .env("GIT_PAGER", "cat")
        .env("SYSTEMD_PAGER", "cat");
    if let Some(session) = session.filter(|s| !s.trim().is_empty()) {
        cmd.env("GRAY_SESSION_ID", session.trim());
    }
    #[cfg(unix)]
    {
        unsafe {
            cmd.pre_exec(|| {
                // Detach from controlling terminal (setsid) so child processes
                // cannot open /dev/tty to block on password prompts or leak
                // onto the TUI. The child becomes its own group leader.
                // A failed setsid must fail the spawn: recording pgid == pid
                // afterwards would be fictitious (kill paths would signal the
                // wrong group).
                if check_setsid(libc::setsid()).is_err() {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(not(windows))]
    let child = cmd.spawn()?;
    #[cfg(windows)]
    let (child, job) = super::windows::spawn_owned(&mut cmd)?;
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("spawned child has no pid"))?;
    Ok(Spawned {
        child,
        pid,
        #[cfg(not(windows))]
        pgid: pid as i32, // setsid: group leader, pgid == pid
        #[cfg(windows)]
        job,
    })
}

/// Pure setsid-result check so the pre_exec failure path is unit-testable
/// (a real setsid failure cannot be forced from a test).
#[cfg(unix)]
fn check_setsid(ret: libc::pid_t) -> io::Result<()> {
    if ret == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[path = "spawn_tests.rs"]
#[cfg(test)]
mod tests;
