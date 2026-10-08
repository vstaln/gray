//! Turn runner: one agent turn for one session key, as a child process.
//!
//! Production spawns `gray -p <prompt> --json [--session <sid>]` (the same
//! headless path the Discord plugin uses), so a crashing or hung turn can
//! never take the gateway down and can always be killed. Tests use
//! [`StubRunner`].

use std::path::PathBuf;
use std::time::Duration;

use tokio::io::AsyncBufReadExt;

use super::event::{Kind, cap};
use crate::cron::store::Origin as Route;

/// Prompt argv cap: several events' worth of text still stays argv-safe.
const PROMPT_CAP: usize = 100 * 1024;
/// One NDJSON line: generous enough for any result, bounded so a broken child
/// cannot grow the daemon (same 1 MiB as the discord plugin's pipe).
const LINE_CAP: usize = 1024 * 1024;
/// stderr is only ever an error hint: the last 2 KiB is plenty.
const STDERR_TAIL: usize = 2 * 1024;
/// A dead child reaps instantly; the bound only covers a failed group kill or
/// a child that closed stdout and kept running.
const REAP_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct TurnRequest {
    /// Session key (`main` today), for logs and env.
    pub key: String,
    /// Gray session to continue; `None` starts a fresh one.
    pub session_id: Option<String>,
    pub prompt: String,
    /// Where a reply would go; exported as `GRAY_CRON_ORIGIN` so a
    /// `gray cron add` inside the turn binds back to this chat.
    pub route: Option<Route>,
    /// Provenance; exported as `GRAY_TURN_ORIGIN`.
    pub kind: Kind,
    pub cwd: PathBuf,
    pub timeout: Duration,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnOutcome {
    /// The session the child reported (new or continued).
    pub session_id: Option<String>,
    /// Final assistant text (may be empty).
    pub text: String,
    /// Set when the turn failed (spawn, exit code, timeout, protocol, error row).
    pub error: Option<String>,
}

#[async_trait::async_trait]
pub trait TurnRunner: Send + Sync {
    async fn run(&self, req: TurnRequest) -> TurnOutcome;
}

/// The production runner. `gray_bin` is the gray executable (normally
/// `std::env::current_exe()`); the child inherits `GRAY_HOME=home`.
pub struct ChildRunner {
    pub gray_bin: PathBuf,
    pub home: PathBuf,
}

#[async_trait::async_trait]
impl TurnRunner for ChildRunner {
    async fn run(&self, req: TurnRequest) -> TurnOutcome {
        let mut outcome = TurnOutcome::default();

        let mut cmd = tokio::process::Command::new(&self.gray_bin);
        cmd.arg("-p")
            .arg(cap(&req.prompt, PROMPT_CAP))
            .arg("--json")
            .arg("--max-requests")
            .arg("200");
        if let Some(sid) = req.session_id.as_deref() {
            cmd.arg("--session").arg(sid);
        }
        cmd.current_dir(&req.cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        // Own process group so a timeout SIGKILL reaches grandchildren too.
        #[cfg(unix)]
        unsafe {
            cmd.pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            });
        }
        cmd.env("GRAY_HOME", &self.home)
            .env("GRAY_TURN_ORIGIN", req.kind.as_str())
            .env("GRAY_SHOW_REASONING", "0")
            .env(
                "GRAY_MAX_WALL_SECS",
                req.timeout.as_secs().max(1).to_string(),
            );
        match &req.route {
            Some(route) => match serde_json::to_string(route) {
                Ok(json) => {
                    cmd.env("GRAY_CRON_ORIGIN", json);
                }
                Err(e) => log::warn!("gateway turn {}: route not serializable: {e}", req.key),
            },
            // A turn with no chat behind it must not inherit one: a `cron add`
            // inside would bind to a stale conversation.
            None => {
                cmd.env_remove("GRAY_CRON_ORIGIN");
            }
        }

        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                outcome.error = Some(format!("agent spawn failed: {e}"));
                return outcome;
            }
        };
        let pid = child.id().unwrap_or(0);
        let (Some(stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
            outcome.error = Some("agent spawn failed: no pipes".to_string());
            return outcome;
        };

        // stderr drains beside the reader so a chatty child cannot block on a
        // full pipe; only the tail survives for error messages.
        let mut stderr_task = tokio::spawn(async move {
            let mut tail: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                match tokio::io::AsyncReadExt::read(&mut stderr, &mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        tail.extend_from_slice(&chunk[..n]);
                        if tail.len() > STDERR_TAIL {
                            tail.drain(..tail.len() - STDERR_TAIL);
                        }
                    }
                }
            }
            tail
        });

        let consume = async {
            let mut saw_result = false;
            let mut rd = tokio::io::BufReader::new(stdout);
            loop {
                match read_line_capped(&mut rd).await {
                    Ok(Some(line)) => handle_line(&line, &req.key, &mut outcome, &mut saw_result),
                    Ok(None) => break,
                    Err(e) => {
                        log::warn!("gateway turn {}: stdout read failed: {e}", req.key);
                        break;
                    }
                }
            }
            saw_result
        };

        let saw_result = match tokio::time::timeout(req.timeout, consume).await {
            Ok(saw) => saw,
            Err(_) => {
                kill_tree(&mut child, pid).await;
                stderr_task.abort();
                outcome.error = Some(format!("turn timed out after {}s", req.timeout.as_secs()));
                return outcome;
            }
        };

        // stdout ended: the child is done or on its way. The grace bound
        // catches a child that closed its pipes and kept running.
        let status = match tokio::time::timeout(REAP_GRACE, child.wait()).await {
            Ok(Ok(status)) => Some(status),
            Ok(Err(e)) => {
                log::warn!("gateway turn {}: wait failed: {e}", req.key);
                None
            }
            Err(_) => {
                log::warn!("gateway turn {}: child still running after EOF", req.key);
                kill_tree(&mut child, pid).await;
                None
            }
        };
        let stderr_tail = match tokio::time::timeout(REAP_GRACE, &mut stderr_task).await {
            Ok(Ok(tail)) => tail,
            _ => {
                stderr_task.abort();
                Vec::new()
            }
        };

        let Some(status) = status else {
            if outcome.error.is_none() {
                outcome.error = Some("agent did not exit".to_string());
            }
            return outcome;
        };
        if !status.success() && !saw_result && outcome.error.is_none() {
            let tail = String::from_utf8_lossy(&stderr_tail);
            let tail = tail.trim();
            let code = status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| status.to_string());
            outcome.error = Some(if tail.is_empty() {
                format!("agent exited with status {code}")
            } else {
                format!("agent exited with status {code}: {tail}")
            });
        } else if status.success() && !saw_result && outcome.error.is_none() {
            log::warn!(
                "gateway turn {}: child exited cleanly with no result",
                req.key
            );
        }
        outcome
    }
}

/// SIGKILL the whole process group so a timeout takes grandchildren down too
/// (the `pre_exec` setpgid makes the child the leader). Off unix there is no
/// group: the child alone is killed.
async fn kill_tree(child: &mut tokio::process::Child, pid: u32) {
    #[cfg(unix)]
    if pid != 0 {
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        let _ = child.kill().await;
    }
    // SIGKILL makes this wait immediate; the bound covers a failed group kill
    // (pid 0 or a sandbox quirk), where the plain kill finishes the job.
    if tokio::time::timeout(REAP_GRACE, child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
}

/// Next stdout line, `None` at EOF. A line past [`LINE_CAP`] is truncated in
/// place but still consumed to the newline, so one endless line cannot OOM
/// the daemon.
async fn read_line_capped(
    rd: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let avail = rd.fill_buf().await?;
        if avail.is_empty() {
            return Ok(if line.is_empty() { None } else { Some(line) });
        }
        let nl = avail.iter().position(|&b| b == b'\n');
        let end = nl.map(|p| p + 1).unwrap_or(avail.len());
        let room = LINE_CAP.saturating_sub(line.len());
        line.extend_from_slice(&avail[..end.min(room)]);
        let take = end;
        rd.consume(take);
        if nl.is_some() {
            return Ok(Some(line));
        }
    }
}

/// One stdout row: `session_id` is taken from any row, `result`/`error`
/// become the outcome. Non-JSON lines are noise —
/// the child owns its stdout, so they mean a broken build — and are dropped.
fn handle_line(line: &[u8], key: &str, outcome: &mut TurnOutcome, saw_result: &mut bool) {
    let text = String::from_utf8_lossy(line);
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let Ok(row) = serde_json::from_str::<serde_json::Value>(text) else {
        log::debug!(
            "gateway turn {key}: non-JSON child line: {}",
            cap(text, 200)
        );
        return;
    };
    if let Some(sid) = row.get("session_id").and_then(|v| v.as_str()) {
        if crate::session_store::valid_session_id(sid) {
            outcome.session_id = Some(sid.to_string());
        } else {
            log::warn!("gateway turn {key}: child reported bad session_id");
        }
    }
    match row.get("type").and_then(|v| v.as_str()) {
        Some("result") => {
            *saw_result = true;
            if let Some(t) = row.get("text").and_then(|v| v.as_str()) {
                outcome.text = t.to_string();
            }
        }
        Some("error") => {
            let msg = row
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown agent error");
            outcome.error = Some(match row.get("code").and_then(|v| v.as_str()) {
                Some(code) => format!("{code}: {msg}"),
                None => msg.to_string(),
            });
        }
        _ => {}
    }
}

/// Test runner: answers every prompt with a fixed reply (or a closure).
pub struct StubRunner {
    pub reply: std::sync::Arc<dyn Fn(&TurnRequest) -> TurnOutcome + Send + Sync>,
}

#[async_trait::async_trait]
impl TurnRunner for StubRunner {
    async fn run(&self, req: TurnRequest) -> TurnOutcome {
        (self.reply)(&req)
    }
}

#[path = "turn_tests.rs"]
#[cfg(test)]
mod tests;
