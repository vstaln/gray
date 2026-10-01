//! Bounded, tool-owned jobs. Workers own children, never the registry: dropping
//! the registry cancels every worker, without an Arc cycle or a global singleton.
use std::collections::BTreeMap;
use std::sync::Mutex;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::shell::contract::MAX_ACTION_WAIT_MS;

const MAX_RUNNING: usize = 32;
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
    pub(super) yielded: bool,
}

impl Drop for Jobs {
    fn drop(&mut self) {
        for job in self.0.get_mut().unwrap_or_else(|e| e.into_inner()).values() {
            job.cancel.cancel();
        }
    }
}

impl Jobs {
    pub(super) async fn start(
        &self,
        ctx: &ToolContext,
        command: String,
        secs: Option<u64>,
        cwd: std::path::PathBuf,
        window: Duration,
    ) -> ToolOutput {
        let (id, mut result) = {
            let mut jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if jobs
                .values()
                .filter(|j| j.result.borrow().is_none())
                .count()
                >= MAX_RUNNING
            {
                return fail(format!(
                    "background job limit ({MAX_RUNNING}) reached; finish or cancel a job first"
                ));
            }
            if jobs.len() >= MAX_RETAINED {
                let oldest = jobs
                    .iter()
                    .filter(|(_, j)| j.yielded && j.notified && j.result.borrow().is_some())
                    .min_by_key(|(_, j)| j.started)
                    .map(|(id, _)| id.clone());
                if let Some(id) = oldest {
                    jobs.remove(&id);
                } else {
                    return fail("job history full; retrieve completed outputs first".into());
                }
            }
            let log = log_path(ctx);
            let id = log.file_stem().unwrap().to_string_lossy().into_owned();
            let started = Instant::now();
            let spawned = match spawn(&command, &cwd, ctx.session_id.as_deref(), None) {
                Ok(s) => s,
                Err(e) => return fail(format!("failed to spawn `sh -c`: {e}")),
            };
            #[cfg(not(windows))]
            let guard = crate::shell::kill::GroupGuard::new(spawned.pgid);
            let cancel = ctx.cancel.child_token();
            let mut worker_ctx = ctx.clone();
            worker_ctx.cancel = cancel.clone();
            let (tx, rx) = watch::channel(None);
            jobs.insert(
                id.clone(),
                Job {
                    session: ctx.session_id.clone(),
                    log: log.clone(),
                    started,
                    cancel,
                    result: rx.clone(),
                    notified: false,
                    yielded: false,
                },
            );
            tokio::spawn(async move {
                let output = run_command(
                    command,
                    log,
                    secs,
                    started,
                    worker_ctx,
                    spawned,
                    #[cfg(not(windows))]
                    guard,
                    None,
                    Duration::ZERO,
                    Duration::ZERO,
                    None,
                )
                .await;
                tx.send_replace(Some(output));
            });
            (id, rx)
        };
        if !window.is_zero() {
            let _ = tokio::time::timeout(window, result.wait_for(|v| v.is_some())).await;
            if let Some(output) = result.borrow().clone() {
                self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
                return output;
            }
        }
        let mut jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let job = jobs
            .get_mut(&id)
            .expect("unpublished job cannot be evicted");
        job.yielded = true;
        let timeout_note = match secs {
            Some(s) => format!("timeout {s}s"),
            None => "no timeout".to_string(),
        };
        ToolOutput::ok(format!(
            "still running · job {id} · yielded after {} · {timeout_note} · log {}\nContinue other work. Use bash action:output/status with job_id:{id} and wait_ms (e.g. 30000) to await it in one call instead of polling; completion will also be reported between model rounds or on the next user turn.",
            format_elapsed(job.started.elapsed()),
            job.log.display()
        ))
    }

    /// Register an already-running command (handed off by the blocking lane
    /// after `bound` of silence). The child is already alive, so \u2014 unlike
    /// [`Jobs::start`] \u2014 there is nothing to refuse before spawn: this only
    /// books the job entry and returns the id, a result sender for the
    /// continuation worker, and a worker context whose cancel token is driven
    /// by both the session cancel and `action:cancel`. Never kills the child:
    /// the returned worker keeps driving it.
    pub(super) fn register(
        &self,
        parent: &ToolContext,
        log: std::path::PathBuf,
        started: Instant,
    ) -> (String, watch::Sender<Option<ToolOutput>>, ToolContext) {
        let id = log.file_stem().unwrap().to_string_lossy().into_owned();
        let cancel = parent.cancel.child_token();
        let mut worker_ctx = parent.clone();
        worker_ctx.cancel = cancel.clone();
        let (tx, rx) = watch::channel(None);
        let mut jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
        // Retention only: evict the oldest delivered job when history is full,
        // else just insert \u2014 losing track of a live child is far worse than a
        // one-entry overrun of MAX_RETAINED.
        if jobs.len() >= MAX_RETAINED
            && let Some(oldest) = jobs
                .iter()
                .filter(|(_, j)| j.yielded && j.notified && j.result.borrow().is_some())
                .min_by_key(|(_, j)| j.started)
                .map(|(id, _)| id.clone())
        {
            jobs.remove(&oldest);
        }
        jobs.insert(
            id.clone(),
            Job {
                session: parent.session_id.clone(),
                log: log.clone(),
                started,
                cancel,
                result: rx,
                notified: false,
                yielded: true,
            },
        );
        (id, tx, worker_ctx)
    }

    pub(super) async fn action(&self, ctx: &ToolContext, action: &str, args: &Value) -> ToolOutput {
        if !matches!(action, "list" | "status" | "output" | "cancel") {
            return fail(format!("unknown bash action: {action}"));
        }
        // Only run-surface keys belong here: `wait`/`wait_ms` ride the
        // blocking-wait contract, not the run surface (rejected with their
        // own loud errors below / in `BashTool::execute`, never this line).
        for (key, default) in [
            ("command", json!("")),
            ("timeout", json!(DEFAULT_TIMEOUT_SECS)),
            ("background", json!(false)),
            ("yield_ms", json!(1000)),
        ] {
            if args.get(key).is_some_and(|v| !v.is_null() && *v != default) {
                return fail(format!("{key} is only valid for action:run"));
            }
        }
        // Bounded blocking wait: `wait` left the surface entirely (it only
        // ever rides output/status as `wait_ms` now); `wait_ms` rides only
        // output/status. Anything else fails loudly, never silently.
        if args.get("wait").is_some_and(|v| !v.is_null()) {
            return fail("`wait` is not a bash argument; to await a job use action:output/status with wait_ms".into());
        }
        let wait_ms = match get_opt_u64(args, "wait_ms") {
            Ok(v) => v.unwrap_or(0),
            Err(e) => return e,
        };
        if !matches!(action, "output" | "status")
            && args.get("wait_ms").is_some_and(|v| !v.is_null())
        {
            return fail("wait_ms is only valid for action:output/status".into());
        }
        let wait = Duration::from_millis(wait_ms.clamp(0, MAX_ACTION_WAIT_MS));
        if action == "list" {
            let jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if args.get("job_id").is_some() {
                return fail("list does not accept job_id".into());
            }
            let lines: Vec<_> = jobs
                .iter()
                .filter(|(_, j)| j.session == ctx.session_id)
                .map(|(id, j)| status(id, j))
                .collect();
            return ToolOutput::ok(if lines.is_empty() {
                "no background jobs in this session".into()
            } else {
                lines.join("\n")
            });
        }
        let id = match get_str(args, "job_id") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let liveness = self.await_settled(ctx, &id, action, wait).await;
        let mut jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let Some(job) = jobs.get_mut(&id).filter(|j| j.session == ctx.session_id) else {
            return fail(format!("unknown job in this session: {id}"));
        };
        let output = job.result.borrow().clone();
        if action == "cancel" && output.is_none() {
            job.cancel.cancel();
            return ToolOutput::ok(format!(
                "cancellation requested for job {id}; use action:status/output for final result"
            ));
        }
        if let Some(output) = output {
            if action == "output" {
                job.notified = true;
                return ToolOutput {
                    content: format!("job {id}\n{}", output.content),
                    ..output
                };
            }
            return ToolOutput::ok(status(&id, job));
        }
        if job.result.has_changed().is_err() {
            return fail(format!(
                "job {id} worker stopped without a result; log {}",
                job.log.display()
            ));
        }
        let mut text = status(&id, job);
        if let Some(note) = liveness {
            text.push_str(&note);
        }
        if action == "output" {
            // Live snapshot is bounded; the final result uses the pump's exact summary.
            let summary = truncated_summary_from_disk(&job.log);
            let view = build_view(&job.log, &summary);
            text.push_str("\nPartial output (snapshot):\n");
            text.push_str(&fence(&view.body));
        }
        ToolOutput::ok(text)
    }

    /// `wait_ms` on output/status stretches the snapshot path into a
    /// bounded blocking wait (clamped 0..=MAX_ACTION_WAIT_MS). Returns the
    /// final result when the job lands inside the window, else today's
    /// snapshot plus a liveness verdict: whether the job's log grew while
    /// we waited (still producing) or stayed silent (possibly stuck — the
    /// agent's cue to inspect or kill, never an auto-kill). Finished jobs,
    /// cancel, and all errors answer immediately with no verdict.
    async fn await_settled(
        &self,
        ctx: &ToolContext,
        id: &str,
        action: &str,
        wait: Duration,
    ) -> Option<String> {
        if wait.is_zero() || !matches!(action, "output" | "status") {
            return None;
        }
        // Clone-then-drop: never hold the registry mutex across the await,
        // or the worker's send_replace can never land.
        let (rx_opt, log) = {
            let jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
            let job = jobs
                .get(id)
                .filter(|j| j.session == ctx.session_id)
                .filter(|j| j.result.borrow().is_none());
            (job.map(|j| j.result.clone()), job.map(|j| j.log.clone()))
        };
        let mut rx = rx_opt?;
        let before = log.as_deref().map(log_len);
        let landed = tokio::select! {
            r = tokio::time::timeout(wait, rx.wait_for(|v| v.is_some())) => {
                matches!(r, Ok(Ok(_)))
            }
            _ = ctx.cancel.cancelled() => false,
        };
        if landed {
            return None;
        }
        let (before, after) = (before?, log.as_deref().map(log_len)?);
        let (b0, _) = before;
        let (b1, wrote) = after;
        Some(if b0 == 0 && b1 == 0 && !wrote {
            liveness_note(0, wait, false)
        } else {
            liveness_note(b1.saturating_sub(b0), wait, true)
        })
    }

    pub(super) fn notifications(&self, ctx: &ToolContext) -> Vec<String> {
        let mut jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
        jobs.iter_mut().filter_map(|(id, job)| {
            if job.session != ctx.session_id || job.notified || !job.yielded { return None; }
            if job.result.borrow().is_none() && job.result.has_changed().is_ok() { return None; }
            job.notified = true;
            // Do not promote process output (or command-derived exit notes) to instructions.
            Some(format!("Background job {id} finished. Use bash action:output with job_id:{id} for its exit status and output."))
        }).collect()
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

/// (log bytes, had any output yet): the pump appends as the child
/// produces, so growth across the wait window means the job is alive.
fn log_len(path: &std::path::Path) -> (u64, bool) {
    match std::fs::metadata(path) {
        Ok(m) => (m.len(), m.len() > 0),
        Err(_) => (0, false),
    }
}

/// One-line liveness verdict for an unexpired wait window: `delta` bytes
/// arrived in `waited`. Never an auto-kill — the agent decides whether a
/// silent job deserves `status`, a log read, or `cancel`.
fn liveness_note(delta: u64, waited: Duration, ever_wrote: bool) -> String {
    if delta > 0 {
        format!(
            "\nLiveness: producing ({} new output in the last {}s) — still working, keep waiting or do other work.",
            crate::truncate::format_size(delta.min(usize::MAX as u64) as usize),
            waited.as_secs(),
        )
    } else if ever_wrote {
        format!(
            "\nLiveness: silent for the last {}s (wrote before, nothing since) — may be thinking, paging, or stuck; peek at the log before killing.",
            waited.as_secs(),
        )
    } else {
        format!(
            "\nLiveness: no output yet after {}s — still starting up or silently stuck; peek at the log before killing.",
            waited.as_secs(),
        )
    }
}

fn status(id: &str, job: &Job) -> String {
    let result = job.result.borrow();
    let state = match result.as_ref() {
        Some(out) => out.content.lines().next().unwrap_or("finished"),
        None if job.cancel.is_cancelled() => "cancelling",
        None => "running",
    };
    format!(
        "job {id} · {state} · elapsed {} · log {}",
        format_elapsed(job.started.elapsed()),
        job.log.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(
        done: bool,
        notified: bool,
        yielded: bool,
    ) -> (Job, watch::Sender<Option<ToolOutput>>) {
        let (tx, rx) = watch::channel(done.then(|| ToolOutput::ok("exit 0")));
        (
            Job {
                session: None,
                log: PathBuf::from("test.log"),
                started: Instant::now(),
                cancel: CancellationToken::new(),
                result: rx,
                notified,
                yielded,
            },
            tx,
        )
    }

    #[tokio::test]
    async fn action_wait_returns_final_when_job_lands_inside_window() {
        // wait_ms stretches the snapshot path into the job's final result:
        // one call replaces N polls. Sender fires mid-wait; the waiter must
        // see the final output (and mark it notified) without polling.
        let tool = BashTool::default();
        let ctx = ToolContext::default();
        let id = "bash-wait-lands-inside-window";
        let (job, tx) = entry(false, false, true);
        tool.jobs.0.lock().unwrap().insert(id.into(), job);
        let waiter = tool.execute(
            &ctx,
            json!({"action": "output", "job_id": id, "wait_ms": 5000}),
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        tx.send_replace(Some(ToolOutput::ok("exit 0\nwaited-result")));
        let out = waiter.await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, format!("job {id}\nexit 0\nwaited-result"));
        assert!(
            tool.jobs
                .0
                .lock()
                .unwrap()
                .get(id)
                .is_some_and(|j| j.notified),
            "final delivery marks notified so no duplicate notice drains"
        );
    }

    #[tokio::test]
    async fn action_wait_timeout_keeps_snapshot_and_finished_job_ignores_wait() {
        // Past the window with no result: today's snapshot, no error.
        let tool = BashTool::default();
        let ctx = ToolContext::default();
        let id = "bash-wait-window-expires";
        let (job, _tx) = entry(false, false, true);
        tool.jobs.0.lock().unwrap().insert(id.into(), job);
        let t0 = Instant::now();
        let out = tool
            .execute(
                &ctx,
                json!({"action": "output", "job_id": id, "wait_ms": 100}),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("running"), "{}", out.content);
        assert!(t0.elapsed() < Duration::from_secs(10), "bounded wait");
        // Finished jobs answer immediately: wait_ms changes nothing.
        let done = "bash-wait-already-finished";
        let (finished, _tx) = entry(true, false, true);
        tool.jobs.0.lock().unwrap().insert(done.into(), finished);
        let out = tool
            .execute(
                &ctx,
                json!({"action": "output", "job_id": done, "wait_ms": 5000}),
            )
            .await;
        assert_eq!(out.content, format!("job {done}\nexit 0"));
        // status honors wait_ms too, without flipping notified.
        let out = tool
            .execute(
                &ctx,
                json!({"action": "status", "job_id": done, "wait_ms": 5000}),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains(done), "{}", out.content);
    }

    #[tokio::test]
    async fn action_wait_rejected_outside_output_status() {
        // wait_ms on run/list/cancel fails loudly — never silently ignored.
        // `wait` left the surface entirely: rejected on run AND management.
        let tool = BashTool::default();
        let ctx = ToolContext::default();
        let id = "bash-wait-rejected-elsewhere";
        let (job, _tx) = entry(false, false, true);
        tool.jobs.0.lock().unwrap().insert(id.into(), job);
        for args in [
            json!({"command": "echo hi", "wait_ms": 1000}),
            json!({"command": "echo hi", "wait": true}),
            json!({"action": "list", "wait_ms": 1000}),
            json!({"action": "cancel", "job_id": id, "wait_ms": 1000}),
            json!({"action": "output", "job_id": id, "wait": true}),
        ] {
            let out = tool.execute(&ctx, args.clone()).await;
            assert!(out.is_error, "{args}: {}", out.content);
            assert!(
                out.content.contains("wait_ms") || out.content.contains("`wait`"),
                "{args}: {}",
                out.content
            );
        }
        // Malformed wait_ms fails through the shared u64 validator.
        let out = tool
            .execute(
                &ctx,
                json!({"action": "output", "job_id": id, "wait_ms": "soon"}),
            )
            .await;
        assert!(
            out.is_error && out.content.contains("wait_ms"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn management_actions_tolerate_run_defaults_without_discarding_intent() {
        let tool = BashTool::default();
        let ctx = ToolContext::default();
        let id = "bash-fa27bb37f76c4e70ae88e927a29e4309";
        let (job, _tx) = entry(true, false, true);
        tool.jobs.0.lock().unwrap().insert(id.into(), job);
        let args = json!({
            "action": "output", "background": false, "command": "",
            "job_id": id, "timeout": DEFAULT_TIMEOUT_SECS, "yield_ms": 1000
        });
        let out = tool.execute(&ctx, args.clone()).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, format!("job {id}\nexit 0"));

        for action in ["list", "status", "output", "cancel"] {
            let mut request = args.clone();
            request["action"] = json!(action);
            if action == "list" {
                request.as_object_mut().unwrap().remove("job_id");
            }
            let out = tool.execute(&ctx, request.clone()).await;
            assert!(!out.is_error, "{action}: {}", out.content);
            for (key, value) in [
                ("command", json!("echo intent")),
                ("background", json!(true)),
            ] {
                let mut rejected = request.clone();
                rejected[key] = value;
                let out = tool.execute(&ctx, rejected).await;
                assert!(out.is_error && out.content.contains(key), "{}", out.content);
            }
        }

        let other = ToolContext {
            session_id: Some("other-session".into()),
            ..ToolContext::default()
        };
        let out = tool.execute(&other, args.clone()).await;
        assert!(
            out.is_error && out.content.contains("unknown job in this session"),
            "{}",
            out.content
        );
        let mut unknown = args;
        unknown["action"] = json!("unknown");
        let out = tool.execute(&ctx, unknown).await;
        assert!(
            out.is_error && out.content.contains("unknown bash action"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn capacity_refuses_before_spawn_and_evicts_only_delivered_jobs() {
        let jobs = Jobs::default();
        let mut senders = vec![];
        for n in 0..MAX_RUNNING {
            let (job, tx) = entry(false, false, true);
            jobs.0.lock().unwrap().insert(n.to_string(), job);
            senders.push(tx);
        }
        let ctx = ToolContext::default();
        let out = jobs
            .start(
                &ctx,
                "echo should-not-start".into(),
                Some(30),
                std::path::PathBuf::from("."),
                Duration::ZERO,
            )
            .await;
        assert!(
            out.is_error && out.content.contains("job limit"),
            "{}",
            out.content
        );
        jobs.0.lock().unwrap().clear();
        for n in 0..MAX_RETAINED {
            let (job, _) = entry(true, false, true);
            jobs.0.lock().unwrap().insert(n.to_string(), job);
        }
        let out = jobs
            .start(
                &ctx,
                "echo should-not-start".into(),
                Some(30),
                std::path::PathBuf::from("."),
                Duration::ZERO,
            )
            .await;
        assert!(
            out.is_error && out.content.contains("history full"),
            "{}",
            out.content
        );
        // A still-unpublished yield-window result cannot be evicted even if
        // another caller has seen it via list/output.
        {
            let mut map = jobs.0.lock().unwrap();
            let j = map.get_mut("0").unwrap();
            j.notified = true;
            j.yielded = false;
        }
        assert!(
            jobs.start(
                &ctx,
                "true".into(),
                Some(30),
                std::path::PathBuf::from("."),
                Duration::ZERO
            )
            .await
            .is_error
        );
        jobs.0.lock().unwrap().get_mut("0").unwrap().yielded = true;
        let out = jobs
            .start(
                &ctx,
                "echo admitted".into(),
                Some(30),
                std::path::PathBuf::from("."),
                Duration::from_secs(10),
            )
            .await;
        assert!(
            !out.is_error && out.content.contains("admitted"),
            "{}",
            out.content
        );
        assert!(!jobs.0.lock().unwrap().contains_key("0"));
    }
    #[tokio::test]
    async fn expired_wait_reports_liveness_from_log_growth() {
        // A job that writes mid-window is alive: the verdict says producing.
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("alive.log");
        std::fs::write(&log, "early\n").unwrap();
        let tool = BashTool::default();
        let ctx = ToolContext::default();
        let id = "bash-wait-alive";
        let (job, _tx) = entry(false, false, true);
        let mut job = job;
        job.log = log.clone();
        tool.jobs.0.lock().unwrap().insert(id.into(), job);
        let writer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
            use std::io::Write as _;
            f.write_all(&vec![b'x'; 2048]).unwrap();
        });
        let out = tool
            .execute(
                &ctx,
                json!({"action": "output", "job_id": id, "wait_ms": 500}),
            )
            .await;
        let _ = writer.await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("running"), "{}", out.content);
        assert!(
            out.content.contains("Liveness: producing"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn expired_wait_flags_a_silent_job() {
        // Nothing written during the window: possibly stuck, never auto-killed.
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("stuck.log");
        std::fs::write(&log, "old news\n").unwrap();
        let tool = BashTool::default();
        let ctx = ToolContext::default();
        let id = "bash-wait-stuck";
        let (job, _tx) = entry(false, false, true);
        let mut job = job;
        job.log = log;
        tool.jobs.0.lock().unwrap().insert(id.into(), job);
        let out = tool
            .execute(
                &ctx,
                json!({"action": "output", "job_id": id, "wait_ms": 1100}),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("Liveness: silent"), "{}", out.content);
        assert!(!out.content.contains("cancel"), "{}", out.content);
    }
}
