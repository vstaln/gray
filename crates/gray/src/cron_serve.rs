//! Cron tick/serve: one claim→fire→record pass + the 60s loop.
//!
//! The agent is behind [`AsyncRunner`] so tests fire jobs with a stub —
//! no model, no network. Production plugs the headless agent in `main.rs`.

use std::path::PathBuf;

pub struct TickReport {
    pub fired: usize,
    pub errors: usize,
}

/// Agent seam: production runs the headless agent; tests stub it.
/// `?Send`: the agent future is not `Send`; the ticker only ever awaits it
/// directly (never `spawn`s), so no `Send` bound is needed.
#[async_trait::async_trait(?Send)]
pub trait AsyncRunner {
    async fn run(&self, prompt: String) -> anyhow::Result<String>;
}

/// The production runner (CLI tick + gateway daemon): a fresh headless agent
/// per fire (no resume/history — hermes isolation), events collected without
/// streaming so ticker stdout stays log-clean. Fires are not persisted as
/// sessions; the transcript goes to the delivery target (local file today).
pub struct HeadlessRunner {
    pub config: crate::config::Config,
}

#[async_trait::async_trait(?Send)]
impl AsyncRunner for HeadlessRunner {
    async fn run(&self, prompt: String) -> anyhow::Result<String> {
        let cwd = std::env::current_dir()?;
        let mut agent = crate::build_agent(&self.config, &cwd, None).await?;
        let ctx = gray_core::agent::ToolContext {
            cwd,
            cancel: tokio_util::sync::CancellationToken::new(),
            session_id: None,
        };
        let events = agent
            .run(gray_core::message::Message::user(prompt), ctx)
            .await
            .map_err(|e| {
                anyhow::anyhow!(crate::repl::format_core_error(&e, &self.config.base_url))
            })?;
        Ok(crate::cron_fire::transcript_text(&events))
    }
}

/// Whole-fire wall clock (script + agent), matches the bash tool bound.
pub const FIRE_TIMEOUT_SECS: u64 = 600;

/// Delivery seam: the CLI passes `SaveLocalDeliver`; the gateway will pass
/// its router. `Err(String)` records `delivery_failed` with the string as
/// `last_delivery_error`; run columns stay untouched.
#[async_trait::async_trait(?Send)]
pub trait CronDeliver {
    async fn deliver(&self, job: &gray_cron::CronJob, now: i64, text: &str) -> Result<(), String>;
}

/// Local delivery: transcript to `$HOME/cron/output/<id>/<ts>.md`.
pub struct LocalDeliver {
    pub home: PathBuf,
}

#[async_trait::async_trait(?Send)]
impl CronDeliver for LocalDeliver {
    async fn deliver(&self, job: &gray_cron::CronJob, now: i64, text: &str) -> Result<(), String> {
        crate::cron_fire::write_local_output(&self.home, job, now, text)
            .map(|_| ())
            .map_err(|e| format!("local write failed: {e:#}"))
    }
}

/// CLI delivery: `local` writes the file; unknown (`origin`/named) targets
/// fail safe to save-only + warn (old-daemon rule — never misdeliver to a
/// wrong chat). The gateway will replace this with its router.
pub struct SaveLocalDeliver {
    pub home: PathBuf,
}

#[async_trait::async_trait(?Send)]
impl CronDeliver for SaveLocalDeliver {
    async fn deliver(&self, job: &gray_cron::CronJob, now: i64, text: &str) -> Result<(), String> {
        if !matches!(job.deliver, gray_cron::Deliver::Local) {
            log::warn!(
                "cron {}: unknown target {:?}, saved locally",
                job.id,
                job.deliver
            );
        }
        LocalDeliver {
            home: self.home.clone(),
        }
        .deliver(job, now, text)
        .await
    }
}

pub fn owner_stamp() -> String {
    format!("{}:{}", std::process::id(), uuid::Uuid::new_v4())
}

/// Fire one already-claimed job and record the outcome via `mark_done`.
/// Returns the recorded status for the tick report. Never propagates
/// job-level failure: every path ends in `mark_done` (claim released).
pub async fn fire_one(
    store: &gray_cron::CronStore,
    runner: &dyn AsyncRunner,
    job: gray_cron::CronJob,
    now: i64,
    deliver: &dyn CronDeliver,
) -> gray_cron::RunStatus {
    use gray_cron::RunStatus;
    let fail = |msg: String| {
        let _ = store.mark_done(&job.id, RunStatus::Error, Some(&msg));
        RunStatus::Error
    };
    let workdir: PathBuf = match &job.workdir {
        Some(w) => w.clone(),
        None => match std::env::current_dir() {
            Ok(c) => c,
            Err(e) => return fail(format!("cannot resolve workdir: {e:#}")),
        },
    };
    if let Some(s) = &job.script
        && (!s.is_absolute() || !s.is_file())
    {
        return fail(format!("pre-script missing: {}", s.display()));
    }
    let mut skill_paths = Vec::new();
    for name in &job.skills {
        match crate::skills_tool::resolve_skill_name(&workdir, name) {
            Some(p) => skill_paths.push(p),
            None => return fail(format!("skill not found: {name}")),
        }
    }
    let mut script_stdout: Option<String> = None;
    if let Some(s) = &job.script {
        let outcome = crate::cron_fire::run_pre_script(s, &workdir).await;
        if !outcome.ok {
            return fail(format!("pre-script failed: {}", outcome.stderr_tail));
        }
        if !crate::cron_fire::parse_wake_gate(&outcome.stdout) {
            let _ = store.mark_done(&job.id, RunStatus::Ok, None);
            return RunStatus::Ok;
        }
        script_stdout = Some(outcome.stdout);
    }
    let prompt =
        crate::cron_fire::assemble_fire_prompt(&job.prompt, &skill_paths, script_stdout.as_deref());
    let run_fut = std::panic::AssertUnwindSafe(runner.run(prompt));
    let text = match tokio::time::timeout(
        std::time::Duration::from_secs(FIRE_TIMEOUT_SECS),
        futures::FutureExt::catch_unwind(run_fut),
    )
    .await
    {
        Err(_) => return fail("fire exceeded 600s".to_string()),
        Ok(Err(_)) => return fail("agent run panicked".to_string()),
        Ok(Ok(Err(e))) => return fail(format!("agent run failed: {e:#}")),
        Ok(Ok(Ok(text))) => text,
    };
    if crate::cron_fire::is_silent_response(&text) {
        let _ = store.mark_done(&job.id, RunStatus::Ok, None);
        return RunStatus::Ok;
    }
    match deliver.deliver(&job, now, &text).await {
        Ok(()) => {
            let _ = store.mark_done(&job.id, RunStatus::Ok, None);
            RunStatus::Ok
        }
        Err(msg) => {
            let _ = store.mark_done(&job.id, RunStatus::DeliveryFailed, Some(&msg));
            RunStatus::DeliveryFailed
        }
    }
}

/// One pass: claim all due jobs, fire sequentially, record each outcome.
/// Per-job failure is recorded on the job and counted; only pass-level
/// store failure propagates as `Err`.
pub async fn tick_once(
    store: &gray_cron::CronStore,
    runner: &dyn AsyncRunner,
    deliver: &dyn CronDeliver,
    kind: &str,
) -> anyhow::Result<TickReport> {
    let now = gray_cron::now_secs();
    // Liveness first, before any job runs: every pass stamps the store so a
    // later reader can tell "nothing was due" from "nothing was ticking".
    // Best-effort — a failed heartbeat must not stop jobs from firing.
    if let Err(e) = store.record_tick(kind) {
        log::warn!("cron: cannot record tick heartbeat: {e:#}");
    }
    let owner = owner_stamp();
    let due = store.claim_due(now, &owner)?;
    let mut report = TickReport {
        fired: 0,
        errors: 0,
    };
    for job in due {
        report.fired += 1;
        if !matches!(
            fire_one(store, runner, job, now, deliver).await,
            gray_cron::RunStatus::Ok
        ) {
            report.errors += 1;
        }
    }
    Ok(report)
}

/// Tick every 60s until SIGINT. Supervision owns the process; there is no
/// daemonization here. Tick-level store errors log and continue.
pub async fn serve_loop(
    store: gray_cron::CronStore,
    deliver: impl CronDeliver + 'static,
    runner: impl AsyncRunner + 'static,
) -> anyhow::Result<()> {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = interval.tick() => {
                match tick_once(&store, &runner, &deliver, "serve").await {
                    Ok(rep) => log::info!("cron tick: fired={} errors={}", rep.fired, rep.errors),
                    Err(e) => log::warn!("cron tick failed: {e:#}"),
                }
            }
        }
    }
    Ok(())
}

#[path = "cron_serve_tests.rs"]
#[cfg(test)]
mod tests;
