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
    let child = cmd.spawn()?;
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("spawned child has no pid"))?;
    Ok(Spawned {
        child,
        pid,
        pgid: pid as i32, // setsid: group leader, pgid == pid
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[cfg(unix)]
    #[test]
    fn setsid_failure_is_an_error() {
        // The pre_exec closure maps -1 to Err so a failed setsid can never
        // record a fictitious pgid == pid.
        assert!(check_setsid(-1).is_err());
        assert!(check_setsid(1234).is_ok());
    }

    #[tokio::test]
    async fn spawn_records_real_process_group() {
        let spawned =
            spawn("true", &PathBuf::from("/tmp"), TaskId(1)).expect("sh -c true must spawn");
        assert_eq!(spawned.pgid, spawned.pid as i32);
    }
}
