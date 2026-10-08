//! Bounded, tool-owned jobs. Workers own children, never the registry: dropping
//! the registry cancels every worker, without an Arc cycle or a global singleton.
//!
//! Nothing here is model-facing. A blocking command that reaches its timeout
//! is registered as a job (never killed); the model peeks with `tail <log>`,
//! stops it with the advertised `kill -- -<pgid>`, and waits by ending its
//! turn. The registry exists for the free completion wake, the headless
//! turn-end hold, `/jobs`, and the running-jobs footer.
use std::collections::BTreeMap;
use std::sync::Mutex;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::*;

const MAX_RETAINED: usize = 128;

#[derive(Default)]
pub(super) struct Jobs(pub(super) Mutex<BTreeMap<String, Job>>);

pub(super) struct Job {
    session: Option<String>,
    log: PathBuf,
    started: Instant,
    pub(super) cancel: CancellationToken,
    pub(super) result: watch::Receiver<Option<ToolOutput>>,
    notified: bool,
    /// The in-band command that stops the whole tree (`kill -- -<pgid>`).
    /// An exact match of this string is carried out by gray itself, never
    /// by a shell that might sit on the far side of an `exec_prefix` (see
    /// [`Jobs::intercept_stop`]).
    stop: String,
}

impl Drop for Jobs {
    fn drop(&mut self) {
        for job in self.0.get_mut().unwrap_or_else(|e| e.into_inner()).values() {
            job.cancel.cancel();
        }
    }
}

impl Jobs {
    /// Register an already-running command (a blocking call that reached its
    /// timeout). The child is alive, so there is nothing to refuse: this books
    /// the entry and returns the id, a result sender for the continuation
    /// worker, and a worker context whose cancel token follows the session's
    /// (and `/jobs` cancel). Never kills the child.
    pub(super) fn register(
        &self,
        parent: &ToolContext,
        command: &str,
        log: std::path::PathBuf,
        started: Instant,
        stop: String,
    ) -> (String, watch::Sender<Option<ToolOutput>>, ToolContext) {
        let cancel = parent.cancel.child_token();
        let mut worker_ctx = parent.clone();
        worker_ctx.cancel = cancel.clone();
        let (tx, rx) = watch::channel(None);
        let mut jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
        // Retention only: evict the oldest delivered job when history is full,
        // else just insert; losing track of a live child is far worse than a
        // one-entry overrun of MAX_RETAINED.
        if jobs.len() >= MAX_RETAINED
            && let Some(oldest) = jobs
                .iter()
                .filter(|(_, j)| j.notified && j.result.borrow().is_some())
                .min_by_key(|(_, j)| j.started)
                .map(|(id, _)| id.clone())
        {
            jobs.remove(&oldest);
        }
        let id = readable_id(&jobs, command);
        jobs.insert(
            id.clone(),
            Job {
                session: parent.session_id.clone(),
                log,
                started,
                cancel,
                result: rx,
                notified: false,
                stop,
            },
        );
        (id, tx, worker_ctx)
    }

    pub(super) fn notifications(&self, ctx: &ToolContext) -> Vec<String> {
        let mut jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
        jobs.iter_mut().filter_map(|(id, job)| {
            if job.session != ctx.session_id || job.notified { return None; }
            if job.result.borrow().is_none() && job.result.has_changed().is_ok() { return None; }
            job.notified = true;
            // Never promote process output (or command-derived exit notes) to
            // a user-role message: only gray's own outcome word and the log.
            let outcome = match job.result.borrow().as_ref() {
                Some(out) => outcome(out),
                None => "worker stopped without a result".to_string(),
            };
            Some(format!(
                "Background job {id} finished ({outcome}) after {} \u{b7} log {}\nRead its output with `tail` (or `grep`) on that log.",
                format_elapsed(job.started.elapsed()),
                job.log.display()
            ))
        }).collect()
    }

    /// Ids of this session's still-running jobs.
    pub(super) fn live_ids(&self, ctx: &ToolContext) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(_, j)| j.session == ctx.session_id && j.result.borrow().is_none())
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Unfinished jobs of this session, oldest first.
    pub(super) fn running(&self, ctx: &ToolContext) -> Vec<gray_core::agent::BackgroundJob> {
        let jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<_> = jobs
            .iter()
            .filter(|(_, j)| j.session == ctx.session_id && j.result.borrow().is_none())
            .map(|(id, j)| {
                (
                    j.started,
                    gray_core::agent::BackgroundJob {
                        id: id.clone(),
                        elapsed: j.started.elapsed(),
                        stopping: j.cancel.is_cancelled(),
                    },
                )
            })
            .collect();
        out.sort_by_key(|(started, _)| *started);
        out.into_iter().map(|(_, job)| job).collect()
    }

    /// Cancel one running job of this session (`/jobs`); `false` when there
    /// is no such running job.
    pub(super) fn cancel_running(&self, ctx: &ToolContext, id: &str) -> bool {
        let jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match jobs.get(id) {
            Some(j) if j.session == ctx.session_id && j.result.borrow().is_none() => {
                j.cancel.cancel();
                true
            }
            _ => false,
        }
    }

    /// True while at least one unfinished job belongs to this session — the
    /// loop's turn-end condition for completion-wake.
    pub(super) fn has_unfinished(&self, ctx: &ToolContext) -> bool {
        let jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
        jobs.values()
            .any(|j| j.session == ctx.session_id && j.result.borrow().is_none())
    }

    /// Bounded wait until ANY unfinished session job settles (or the timeout
    /// elapses / the session is cancelled). `Ok(())` = a job landed; the
    /// caller then drains notifications as usual. Clone-then-drop on the
    /// receivers so the registry mutex is never held across the await (the
    /// workers' `send_replace` would otherwise block forever).
    pub(super) async fn wait_any(&self, ctx: &ToolContext, timeout: Duration) -> bool {
        let rxs: Vec<_> = {
            let jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
            jobs.values()
                .filter(|j| j.session == ctx.session_id && j.result.borrow().is_none())
                .map(|j| j.result.clone())
                .collect()
        };
        if rxs.is_empty() {
            return false;
        }
        let wait_all = futures::future::select_all(rxs.into_iter().map(|mut rx| {
            // The receiver moves into the async block and is polled by
            // `changed()` — `wait_for` borrows the receiver for the whole
            // future, which cannot escape the closure.
            Box::pin(async move {
                while rx.borrow().is_none() {
                    if rx.changed().await.is_err() {
                        return;
                    }
                }
            })
        }));
        tokio::select! {
            _ = wait_all => true,
            _ = tokio::time::sleep(timeout) => false,
            _ = ctx.cancel.cancelled() => false,
        }
    }
}

/// A job id a person (and the model) can read: the command's name
/// (`cargo-check`), numbered when that name is already held (`cargo-check-2`).
/// The log file keeps its random name: logs share the temp dir across
/// sessions, ids only need to be unique in this registry.
fn readable_id(jobs: &BTreeMap<String, Job>, command: &str) -> String {
    let name = crate::shell::label::job_name(command);
    crate::shell::label::unique_name(&name, |n| jobs.contains_key(n))
}

impl Jobs {
    /// `command` is exactly the advertised stop of one of this session's
    /// jobs (`kill -- -<pgid>`, `taskkill /T /F /PID <pid>`): carry it out
    /// here and answer with the outcome. `None` — run it as written.
    ///
    /// A shell would usually do the same, but under an `exec_prefix`
    /// (docker, ssh) it runs on the far side, where that pgid is some
    /// unrelated group; and a finished job's pgid may already belong to
    /// another process. Only the exact advertised string is ours — any
    /// variation (a signal flag, a `&&` chain, `sudo`, a stray pgid)
    /// stays the shell's.
    pub(super) async fn intercept_stop(
        &self,
        ctx: &ToolContext,
        command: &str,
    ) -> Option<ToolOutput> {
        let want = command.trim();
        let (id, mut rx) = {
            let jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
            let (id, job) = jobs
                .iter()
                .find(|(_, j)| j.session == ctx.session_id && j.stop == want)?;
            if job.result.borrow().is_some() {
                return Some(fail(format!(
                    "job {id} had already finished (no such process group)"
                )));
            }
            job.cancel.cancel();
            (id.clone(), job.result.clone())
        };
        // Termination is synchronous, like `kill` followed by the group
        // actually being gone: what runs next sees it stopped.
        let settled = tokio::time::timeout(Duration::from_secs(10), rx.wait_for(|v| v.is_some()))
            .await
            .is_ok_and(|r| r.is_ok());
        Some(if settled {
            ToolOutput::ok(format!(
                "stopped job {id}: its whole process group was terminated"
            ))
        } else {
            fail(format!(
                "job {id} was signalled but has not exited after 10s; it is still being stopped"
            ))
        })
    }
}

/// What a finished job's notice may say about how it ended: gray's own
/// first word of the result header (`exit 0`, `cancelled`), never anything
/// the command printed.
fn outcome(out: &ToolOutput) -> String {
    let first = out.content.lines().next().unwrap_or("");
    if first.starts_with("cancelled") {
        return "cancelled".into();
    }
    let word = first
        .split(" \u{b7} ")
        .next()
        .unwrap_or("")
        .split(" (")
        .next()
        .unwrap_or("");
    let is_exit = word
        .strip_prefix("exit ")
        .is_some_and(|code| code.parse::<i64>().is_ok());
    if is_exit {
        word.to_string()
    } else if out.is_error {
        "failed".into()
    } else {
        "done".into()
    }
}

impl Jobs {
    /// One line per still-running job among `ids`, appended to later bash
    /// results so a model that lost the notice (compaction, a long detour)
    /// still has each job's log and stop command in recent context.
    pub(super) fn footer(&self, ctx: &ToolContext, ids: &[String]) -> Option<String> {
        if ids.is_empty() {
            return None;
        }
        let jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let mut rows: Vec<_> = jobs
            .iter()
            .filter(|(id, j)| {
                ids.contains(id) && j.session == ctx.session_id && j.result.borrow().is_none()
            })
            .collect();
        if rows.is_empty() {
            return None;
        }
        rows.sort_by_key(|(_, j)| j.started);
        let lines: Vec<String> = rows
            .iter()
            .map(|(id, j)| {
                format!(
                    "background job {id} still going ({}) \u{b7} log {} \u{b7} stop: `{}`",
                    format_elapsed(j.started.elapsed()),
                    j.log.display(),
                    j.stop
                )
            })
            .collect();
        Some(lines.join("\n"))
    }
}

#[path = "jobs_tests.rs"]
#[cfg(test)]
mod tests;
