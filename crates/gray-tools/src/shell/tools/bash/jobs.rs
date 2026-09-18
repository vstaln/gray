//! Bounded, tool-owned jobs. Workers own children, never the registry: dropping
//! the registry cancels every worker, without an Arc cycle or a global singleton.
use std::collections::BTreeMap;
use std::sync::Mutex;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::*;

const MAX_RUNNING: usize = 32;
const MAX_RETAINED: usize = 128;

#[derive(Default)]
pub(super) struct Jobs(Mutex<BTreeMap<String, Job>>);

struct Job {
    session: Option<String>,
    log: PathBuf,
    started: Instant,
    cancel: CancellationToken,
    result: watch::Receiver<Option<ToolOutput>>,
    notified: bool,
    yielded: bool,
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
        secs: u64,
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
            let spawned = match spawn(&command, &ctx.cwd) {
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
        ToolOutput::ok(format!(
            "still running · job {id} · yielded after {} · timeout {secs}s · log {}\nContinue other work. Use bash action:output/status/cancel with job_id:{id}; completion will be reported between model rounds or on the next user turn.",
            format_elapsed(job.started.elapsed()),
            job.log.display()
        ))
    }

    pub(super) fn action(&self, ctx: &ToolContext, action: &str, args: &Value) -> ToolOutput {
        if !matches!(action, "list" | "status" | "output" | "cancel") {
            return fail(format!("unknown bash action: {action}"));
        }
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
        let mut jobs = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if action == "list" {
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
        if action == "output" {
            // Live snapshot is bounded; the final result uses the pump's exact summary.
            let summary = truncated_summary_from_disk(&job.log);
            let view = build_view(&job.log, &summary);
            text.push_str("\nPartial output (snapshot):\n");
            text.push_str(&fence(&view.body));
        }
        ToolOutput::ok(text)
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
    async fn management_actions_tolerate_run_defaults_without_discarding_intent() {
        let tool = BashTool::default();
        let ctx = ToolContext::default();
        let id = "bash-fa27bb37f76c4e70ae88e927a29e4309";
        let (job, _tx) = entry(true, false, true);
        tool.jobs.0.lock().unwrap().insert(id.into(), job);
        let args = json!({
            "action": "output", "background": false, "command": "",
            "job_id": id, "timeout": 30, "yield_ms": 1000
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
            .start(&ctx, "echo should-not-start".into(), 30, Duration::ZERO)
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
            .start(&ctx, "echo should-not-start".into(), 30, Duration::ZERO)
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
            jobs.start(&ctx, "true".into(), 30, Duration::ZERO)
                .await
                .is_error
        );
        jobs.0.lock().unwrap().get_mut("0").unwrap().yielded = true;
        let out = jobs
            .start(&ctx, "echo admitted".into(), 30, Duration::from_secs(10))
            .await;
        assert!(
            !out.is_error && out.content.contains("admitted"),
            "{}",
            out.content
        );
        assert!(!jobs.0.lock().unwrap().contains_key("0"));
    }
}
