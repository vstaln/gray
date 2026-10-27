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

    fn in_session(job: &Job, ctx: &ToolContext) -> bool {
        job.session == ctx.session_id || job.session.is_none()
    }

    pub(super) fn notifications(&self, ctx: &ToolContext) -> Vec<String> {
        let mut jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
        jobs.iter_mut().filter_map(|(id, job)| {
            if !Self::in_session(job, ctx) || job.notified { return None; }
            if job.result.borrow().is_none() && job.result.has_changed().is_ok() { return None; }
            job.notified = true;

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

    pub(super) fn live_ids(&self, ctx: &ToolContext) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(_, j)| Self::in_session(j, ctx) && j.result.borrow().is_none())
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub(super) fn running(&self, ctx: &ToolContext) -> Vec<gray_core::agent::BackgroundJob> {
        let jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<_> = jobs
            .iter()
            .filter(|(_, j)| Self::in_session(j, ctx) && j.result.borrow().is_none())
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

    pub(super) fn cancel_running(&self, ctx: &ToolContext, id: &str) -> bool {
        let jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match jobs.get(id) {
            Some(j) if Self::in_session(j, ctx) && j.result.borrow().is_none() => {
                j.cancel.cancel();
                true
            }
            _ => false,
        }
    }

    pub(super) fn has_unfinished(&self, ctx: &ToolContext) -> bool {
        let jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
        jobs.values()
            .any(|j| Self::in_session(j, ctx) && j.result.borrow().is_none())
    }

    pub(super) async fn wait_any(&self, ctx: &ToolContext, timeout: Duration) -> bool {
        let rxs: Vec<_> = {
            let jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
            jobs.values()
                .filter(|j| Self::in_session(j, ctx) && j.result.borrow().is_none())
                .map(|j| j.result.clone())
                .collect()
        };
        if rxs.is_empty() {
            return false;
        }
        let wait_all = futures::future::select_all(rxs.into_iter().map(|mut rx| {
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

fn readable_id(jobs: &BTreeMap<String, Job>, command: &str) -> String {
    let name = crate::shell::label::job_name(command);
    crate::shell::label::unique_name(&name, |n| jobs.contains_key(n))
}

impl Jobs {
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
                .find(|(_, j)| Self::in_session(j, ctx) && j.stop == want)?;
            if job.result.borrow().is_some() {
                return Some(fail(format!(
                    "job {id} had already finished (no such process group)"
                )));
            }
            job.cancel.cancel();
            (id.clone(), job.result.clone())
        };

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
    pub(super) fn footer(&self, ctx: &ToolContext, ids: &[String]) -> Option<String> {
        if ids.is_empty() {
            return None;
        }
        let jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let mut rows: Vec<_> = jobs
            .iter()
            .filter(|(id, j)| {
                ids.contains(id) && Self::in_session(j, ctx) && j.result.borrow().is_none()
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
