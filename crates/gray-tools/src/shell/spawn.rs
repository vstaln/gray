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
//!
//! An exec prefix moves the same command somewhere else (a container, a
//! remote box) without changing what runs: see [`spawn_with_prefix`].

use std::io;
use std::path::{Path, PathBuf};

use tokio::process::Command;

use super::contract::Spawned;

/// Env var naming the exec prefix: a program that runs the command elsewhere,
/// e.g. `docker exec -i dev sh -s` or `ssh box sh -s`. Written from
/// `exec_prefix` in the saved config; also settable directly in the
/// environment, next to the other `GRAY_*` shell knobs.
pub const EXEC_PREFIX_ENV: &str = "GRAY_EXEC_PREFIX";

/// Split a configured prefix into argv, honouring single and double quotes and
/// backslash escapes, so `ssh -p 2222 box sh -s` and `ssh 'my box' sh -s` both
/// work. An unterminated quote is an error, never a silently truncated command.
pub fn split_prefix(raw: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some('\'') => {
                if c == '\'' {
                    quote = None;
                } else {
                    cur.push(c);
                }
            }
            Some(_) => match c {
                c if c == quote.expect("double-quoted") => quote = None,
                '\\' => match chars.next() {
                    Some(escaped) => cur.push(escaped),
                    None => return Err("exec_prefix ends inside a quote".into()),
                },
                c => cur.push(c),
            },
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    started = true;
                }
                '\\' => {
                    started = true;
                    match chars.next() {
                        Some(escaped) => cur.push(escaped),
                        None => return Err("exec_prefix ends with a backslash".into()),
                    }
                }
                c if c.is_whitespace() => {
                    if started {
                        out.push(std::mem::take(&mut cur));
                        started = false;
                    }
                }
                c => {
                    started = true;
                    cur.push(c);
                }
            },
        }
    }
    if let Some(unterminated) = quote {
        return Err(format!(
            "exec_prefix has an unterminated {unterminated} quote"
        ));
    }
    if started {
        out.push(cur);
    }
    Ok(out)
}

/// POSIX single-quote one word: wrap in `'…'`, close-escape-reopen for an
/// embedded quote. The export preamble is parsed by the far side's shell
/// rather than passed as argv, so it needs real quoting.
fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// The environment every spawned shell gets. Under an exec prefix the same
/// pairs are also emitted as an `export` preamble, because neither `ssh` nor
/// `docker exec` forwards the client's environment: without it a prefixed
/// command silently loses `GRAY_SESSION_ID`, `GRAY_CWD_REPORT` and the
/// non-interactive guards — `GIT_TERMINAL_PROMPT=0` is what stops a remote
/// `git` from blocking forever on a password prompt.
fn shell_env(session: Option<&str>, cwd_report: Option<&Path>) -> Vec<(String, String)> {
    let mut env: Vec<(&str, &str)> = vec![
        ("DEBIAN_FRONTEND", "noninteractive"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("SUDO_ASKPASS", "/bin/false"),
        ("TERM", "dumb"),
        ("PAGER", "cat"),
        ("MANPAGER", "cat"),
        ("GIT_PAGER", "cat"),
        ("SYSTEMD_PAGER", "cat"),
    ];
    if let Some(session) = session.filter(|s| !s.trim().is_empty()) {
        env.push(("GRAY_SESSION_ID", session.trim()));
    }
    if let Some(report) = cwd_report {
        env.push(("GRAY_CWD_REPORT", report.to_str().unwrap_or_default()));
    }
    env.into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// The script the far side's shell reads: the export preamble, then the
/// command exactly as gray would have run it locally. Fed from a file so a
/// large script can never block us on a full stdin pipe while the child is
/// still starting.
fn write_script(command: &str, env: &[(String, String)]) -> io::Result<PathBuf> {
    let mut script = String::new();
    for (key, value) in env {
        script.push_str(&format!("export {key}={}\n", shell_quote(value)));
    }
    script.push_str(command);
    if !script.ends_with('\n') {
        script.push('\n');
    }
    let path = std::env::temp_dir().join(format!("gray-exec-{}.sh", uuid::Uuid::new_v4()));
    std::fs::write(&path, script)?;
    Ok(path)
}

/// Spawn `command` via `sh - c` in `cwd`, or through the exec prefix when one
/// is configured (see [`spawn_with_prefix`]).
///
/// `session` is exported as `GRAY_SESSION_ID` so anything the command runs can
/// name the session it belongs to: a `gray memory set` issued from inside a
/// session stamps its entry with that session rather than the generic `cli`.
///
/// `cwd_report`, when given, is exported as `GRAY_CWD_REPORT`; the command is
/// expected to write its final directory there so the caller can keep a
/// session's working directory across calls. It is a file rather than stdout
/// so the command's output — and the durable log — stay byte-identical.
pub fn spawn(
    command: &str,
    cwd: &Path,
    session: Option<&str>,
    cwd_report: Option<&Path>,
) -> io::Result<Spawned> {
    let raw = std::env::var(EXEC_PREFIX_ENV).unwrap_or_default();
    let prefix = match raw.trim() {
        "" => None,
        _ => match split_prefix(&raw)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?
            .split_first()
        {
            Some((program, args)) if !program.is_empty() => {
                let mut argv = vec![program.clone()];
                argv.extend(args.iter().cloned());
                Some(argv)
            }
            _ => None,
        },
    };
    spawn_with_prefix(command, cwd, session, cwd_report, prefix)
}

/// [`spawn`] with the prefix passed in, so tests never mutate the process
/// environment (they run in parallel) and both paths stay testable on their own.
///
/// The prefix is a program that ends in a shell reading its script from stdin:
/// `docker exec -i dev sh -s`, `ssh box sh -s`. That is what lets one setting
/// cover a local container and a remote box with no quoting rules to get wrong
/// — the command crosses as text, not as an argv the far side re-splits (the
/// `ssh box sh -c 'ls'` trap). The command's own words, `$`, globs and heredocs
/// are parsed exactly once, by the shell that runs it.
///
/// ponytail: a prefix that wants `-c`, or that does not read stdin at all,
/// silently runs nothing. That is the ceiling of the one-knob design; the
/// upgrade path is a second key choosing argv (`<prefix> … sh -c`) over stdin.
pub fn spawn_with_prefix(
    command: &str,
    cwd: &Path,
    session: Option<&str>,
    cwd_report: Option<&Path>,
    prefix: Option<Vec<String>>,
) -> io::Result<Spawned> {
    let env = shell_env(session, cwd_report);
    let mut cmd = match &prefix {
        Some(argv) => {
            let mut cmd = Command::new(&argv[0]);
            cmd.args(&argv[1..]);
            cmd
        }
        None => local_command()?,
    };
    cmd.current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (key, value) in &env {
        cmd.env(key, value);
    }
    match &prefix {
        Some(_) => {
            let script = write_script(command, &env)?;
            let file = std::fs::File::open(&script);
            // A script we cannot open leaves nothing to clean up.
            cmd.stdin(std::process::Stdio::from(file?));
            match spawn_child(&mut cmd) {
                Ok(spawned) => {
                    let _ = std::fs::remove_file(&script);
                    Ok(spawned)
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&script);
                    Err(e)
                }
            }
        }
        None => {
            cmd.arg("-c")
                .arg(command)
                .stdin(std::process::Stdio::null());
            spawn_child(&mut cmd)
        }
    }
}

/// The no-prefix command: `sh -c`, or Git's sh on Windows with a PATH that
/// reaches its tools.
fn local_command() -> io::Result<Command> {
    #[cfg(not(windows))]
    {
        Ok(Command::new("sh"))
    }
    #[cfg(windows)]
    {
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
        Ok(cmd)
    }
}

/// The spawn both paths share: detach from the controlling terminal (Unix) or
/// hand the tree to a Job Object (Windows), then record the real pid.
fn spawn_child(cmd: &mut Command) -> io::Result<Spawned> {
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
    let (child, job) = super::windows::spawn_owned(cmd)?;
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
