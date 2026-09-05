//! shell/spawn.rs — process spawn (brief 1D).
//!
//! `sh -c` with null stdin, piped stdout/stderr, non-interactive env,
//! detached via setsid (pgid == pid). NO `kill_on_drop`: from now on a
//! child dies only by explicit kill (timeout/cancel arm) or by exiting.

use std::io;
use std::path::Path;

use tokio::process::Command;

use super::contract::{Spawned, TaskId};

/// Spawn `command` via `sh -c` in `cwd`, tagged with `task` for
/// `GRAY_TASK_ID`. (P1D ruling: the `task` param extends the contract's
/// 2-arg form — without it the mandated env tag cannot be set.)
pub fn spawn(command: &str, cwd: &Path, task: TaskId) -> io::Result<Spawned> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("DEBIAN_FRONTEND", "noninteractive")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("SUDO_ASKPASS", "/bin/false")
        .env("GRAY_TASK_ID", task.to_string())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("PAGER", "cat")
        .env("GIT_PAGER", "cat");
    #[cfg(unix)]
    {
        unsafe {
            cmd.pre_exec(|| {
                // Detach from controlling terminal (setsid) so child processes
                // cannot open /dev/tty to block on password prompts or leak
                // onto the TUI. The child becomes its own group leader.
                libc::setsid();
                Ok(())
            });
        }
    }
    let child = cmd.spawn()?;
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("spawned child has no pid"))?;
    Ok(Spawned {
        child,
        pid,
        pgid: pid as i32, // setsid: group leader, pgid == pid
        start_ticks: start_ticks_for(pid),
    })
}

/// Linux `/proc/{pid}/stat` field 22 (starttime); `None` elsewhere
/// (macOS best-effort, per brief 2D).
#[cfg(target_os = "linux")]
fn start_ticks_for(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit(')')
        .next()?
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

#[cfg(not(target_os = "linux"))]
fn start_ticks_for(_pid: u32) -> Option<u64> {
    None
}
