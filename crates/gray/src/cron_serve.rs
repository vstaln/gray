//! Cron tick/serve: one claim→fire→record pass + the 60s loop.
//!
//! The agent is behind [`AsyncRunner`] so tests fire jobs with a stub —
//! no model, no network. Production plugs the headless agent in `main.rs`.

use std::path::PathBuf;

/// One fired job's delivery for live-chat rendering. `to_chat` is true only
/// for `Origin` jobs with a recorded origin session (hermes mirror parity);
/// `Local`/`Target`/session-less `Origin` are save-only (`to_chat: false`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveredFire {
    pub id: String,
    pub name: String,
    pub path: std::path::PathBuf,
    pub excerpt: String,
    pub to_chat: bool,
}

pub struct TickReport {
    pub fired: usize,
    pub errors: usize,
    pub delivered: Vec<DeliveredFire>,
}

/// Re-resolve the fire-time model + provider from the saved config file.
/// Every `/model` switch (picker and direct) persists base_url+model, but
/// long-lived tickers snapshot Config once at startup — without this refresh
/// a mid-session provider switch never reaches cron fires. Only non-empty
/// saved values apply, so a missing file keeps the snapshot untouched.
pub(crate) fn refresh_model_from_saved(config: &mut crate::config::Config) {
    let Ok(path) = crate::setup::saved_config_path() else {
        return;
    };
    refresh_model_from_saved_at(config, &path);
}

/// Testable seam: pure path, no env. Only non-empty saved values apply, so
/// a missing file (all-None) keeps the snapshot untouched.
fn refresh_model_from_saved_at(config: &mut crate::config::Config, path: &std::path::Path) {
    let saved = crate::setup::load_saved_config_at(path);
    if let Some(model) = saved.model.filter(|m| !m.trim().is_empty()) {
        config.model = Some(model);
    }
    if let Some(base) = saved.base_url.filter(|u| !u.trim().is_empty()) {
        config.base_url = base;
    }
}

/// Agent seam: production runs the headless agent; tests stub it.
/// `?Send`: the agent future is not `Send`; the ticker only ever awaits it
/// directly (never `spawn`s), so no `Send` bound is needed.
#[async_trait::async_trait(?Send)]
pub trait AsyncRunner {
    async fn run(&self, prompt: String, cwd: PathBuf) -> anyhow::Result<String>;
}

/// The production runner (CLI tick + gateway daemon): a fresh headless agent
/// per fire (no resume/history — hermes isolation), events collected without
/// streaming so ticker stdout stays log-clean. Fires are not persisted as
/// sessions; the transcript goes to the delivery target (local file today).
pub struct HeadlessRunner {
    pub config: crate::config::Config,
    /// Long-lived drivers (serve, gateway) follow `/model` switches via the
    /// saved config; one-shot tick/run keep their fresh snapshot so explicit
    /// CLI flags and env vars always win.
    pub follow_switches: bool,
}

#[async_trait::async_trait(?Send)]
impl AsyncRunner for HeadlessRunner {
    async fn run(&self, prompt: String, cwd: PathBuf) -> anyhow::Result<String> {
        let mut config = self.config.clone();
        if self.follow_switches {
            refresh_model_from_saved(&mut config);
        }
        let mut agent = crate::build_agent(&config, &cwd, None).await?;
        let ctx = gray_core::agent::ToolContext {
            cwd,
            cancel: tokio_util::sync::CancellationToken::new(),
            session_id: None,
        };
        let events = agent
            .run(gray_core::message::Message::user(prompt), ctx)
            .await
            .map_err(|e| anyhow::anyhow!(crate::repl::format_core_error(&e, &config.base_url)))?;
        Ok(crate::cron_fire::transcript_text(&events))
    }
}

/// Whole-fire wall clock (script + agent), matches the bash tool bound.
pub const FIRE_TIMEOUT_SECS: u64 = 600;

/// Delivery (hermes `_deliver_result` + `_cron_mirror_message` parity):
/// transcript to `$HOME/cron/output/<id>/<ts>.md` first, always — then, for
/// `Deliver::Origin` with a recorded origin session, a mirror append of the
/// clean (unwrapped, no header/footer, no file path) excerpt to that session
/// as a labelled `USER` turn so a reply continues in context. Unknown
/// (`Target`) or session-less `Origin` jobs fail safe to save-only + warn
/// (old-daemon rule — never misdeliver to a wrong chat). `Err(String)`
/// records `delivery_failed` with the string as `last_delivery_error`; run
/// columns stay untouched. The file is always saved first so an append
/// failure keeps the output on disk.
pub struct SaveLocalDeliver {
    pub home: PathBuf,
}

impl SaveLocalDeliver {
    pub async fn deliver(
        &self,
        job: &crate::cron::CronJob,
        now: i64,
        text: &str,
    ) -> Result<DeliveredFire, String> {
        let path = crate::cron_fire::write_local_output(&self.home, job, now, text)
            .map_err(|e| format!("local write failed: {e:#}"))?;
        // Bounded excerpt: the full transcript is already on disk; the mirror
        // and the live box share this cap.
        let excerpt = crate::cron_fire::delivery_excerpt(text);
        let saved = |to_chat: bool| DeliveredFire {
            id: job.id.clone(),
            name: job.name.clone(),
            path: path.clone(),
            excerpt: excerpt.clone(),
            to_chat,
        };
        match &job.deliver {
            crate::cron::Deliver::Local => Ok(saved(false)),
            crate::cron::Deliver::Target(_) => {
                log::warn!(
                    "cron {}: unknown target {:?}, saved locally",
                    job.id,
                    job.deliver
                );
                Ok(saved(false))
            }
            crate::cron::Deliver::Origin => {
                let Some(origin) = &job.origin else {
                    log::warn!(
                        "cron {}: origin delivery without origin session, saved locally",
                        job.id
                    );
                    return Ok(saved(false));
                };
                // Clean mirror (hermes parity): no wrapper, no file path.
                // `USER`, never assistant — an assistant-role mirror lands
                // assistant→assistant and breaks strict alternation;
                // consecutive user turns merge safely.
                let note = crate::cron_fire::mirror_message(&job.name, &excerpt);
                let sessions =
                    crate::session_store::JsonlSessionStore::new(self.home.join("sessions"));
                let sid = crate::session_store::SessionId::new(origin.chat.clone());
                sessions
                    .append(&sid, &gray_core::message::Message::user(note))
                    .await
                    .map(|_| saved(true))
                    .map_err(|e| {
                        format!("origin append failed for session {:?}: {e:#}", origin.chat)
                    })
            }
        }
    }
}

pub fn owner_stamp() -> String {
    format!("{}:{}", std::process::id(), uuid::Uuid::new_v4())
}

/// Fire one already-claimed job and record the outcome via `mark_done`.
/// Returns the recorded status plus the delivery record for live-chat
/// rendering (`None` when the fire failed before delivery). Never propagates
/// job-level failure: every path ends in `mark_done` (claim released).
pub async fn fire_one(
    store: &crate::cron::CronStore,
    runner: &dyn AsyncRunner,
    job: crate::cron::CronJob,
    now: i64,
    deliver: &SaveLocalDeliver,
) -> (crate::cron::RunStatus, Option<DeliveredFire>) {
    use crate::cron::RunStatus;
    let fail = |msg: String| {
        let _ = store.mark_done(
            &job.id,
            job.fire_claim.as_ref(),
            RunStatus::Error,
            Some(&msg),
        );
        RunStatus::Error
    };
    let workdir: PathBuf = match &job.workdir {
        Some(w) => w.clone(),
        None => match std::env::current_dir() {
            Ok(c) => c,
            Err(e) => return (fail(format!("cannot resolve workdir: {e:#}")), None),
        },
    };
    if let Some(s) = &job.script
        && (!s.is_absolute() || !s.is_file())
    {
        return (fail(format!("pre-script missing: {}", s.display())), None);
    }
    let mut skill_paths = Vec::new();
    for name in &job.skills {
        match crate::skills_tool::resolve_skill_name(&workdir, name) {
            Some(p) => skill_paths.push(p),
            None => return (fail(format!("skill not found: {name}")), None),
        }
    }
    let mut script_stdout: Option<String> = None;
    if let Some(s) = &job.script {
        let outcome = crate::cron_fire::run_pre_script(s, &workdir).await;
        if !outcome.ok {
            return (
                fail(format!("pre-script failed: {}", outcome.stderr_tail)),
                None,
            );
        }
        if !crate::cron_fire::parse_wake_gate(&outcome.stdout) {
            let _ = store.mark_done(&job.id, job.fire_claim.as_ref(), RunStatus::Ok, None);
            return (RunStatus::Ok, None);
        }
        script_stdout = Some(outcome.stdout);
    }
    let prompt =
        crate::cron_fire::assemble_fire_prompt(&job.prompt, &skill_paths, script_stdout.as_deref());
    let run_fut = std::panic::AssertUnwindSafe(runner.run(prompt, workdir));
    let text = match tokio::time::timeout(
        std::time::Duration::from_secs(FIRE_TIMEOUT_SECS),
        futures::FutureExt::catch_unwind(run_fut),
    )
    .await
    {
        Err(_) => return (fail("fire exceeded 600s".to_string()), None),
        Ok(Err(_)) => return (fail("agent run panicked".to_string()), None),
        Ok(Ok(Err(e))) => return (fail(format!("agent run failed: {e:#}")), None),
        Ok(Ok(Ok(text))) => text,
    };
    if crate::cron_fire::is_silent_response(&text) {
        let _ = store.mark_done(&job.id, job.fire_claim.as_ref(), RunStatus::Ok, None);
        return (RunStatus::Ok, None);
    }
    match deliver.deliver(&job, now, &text).await {
        Ok(saved) => {
            let _ = store.mark_done(&job.id, job.fire_claim.as_ref(), RunStatus::Ok, None);
            (RunStatus::Ok, Some(saved))
        }
        Err(msg) => {
            let _ = store.mark_done(
                &job.id,
                job.fire_claim.as_ref(),
                RunStatus::DeliveryFailed,
                Some(&msg),
            );
            (RunStatus::DeliveryFailed, None)
        }
    }
}

/// Max jobs fired concurrently in one tick pass. `const`, not `Config`:
/// Config plumbing is a follow-up. `= 1` behaves exactly as the old serial loop.
pub const MAX_CONCURRENT_FIRES: usize = 4;

/// One bounded pass: claim up to [`MAX_CONCURRENT_FIRES`] due jobs, fire them
/// concurrently on this task, aggregate in claim order. Per-job failure is
/// recorded on the job and counted; only pass-level store failure propagates
/// as `Err`.
pub async fn tick_once(
    store: &crate::cron::CronStore,
    runner: &dyn AsyncRunner,
    deliver: &SaveLocalDeliver,
    kind: &str,
) -> anyhow::Result<TickReport> {
    tick_once_with(
        store,
        runner,
        deliver,
        kind,
        crate::setup::cron_auto_enabled(),
    )
    .await
}

/// Test seam for [`tick_once`]: the master switch arrives as a parameter so
/// the gate is exercised without touching the live config.
pub(crate) async fn tick_once_with(
    store: &crate::cron::CronStore,
    runner: &dyn AsyncRunner,
    deliver: &SaveLocalDeliver,
    kind: &str,
    auto: bool,
) -> anyhow::Result<TickReport> {
    // Liveness first, before any job runs: every pass stamps the store so a
    // later reader can tell "nothing was due" from "nothing was ticking".
    // Best-effort — a failed heartbeat must not stop jobs from firing.
    if let Err(e) = store.record_tick(kind) {
        log::warn!("cron: cannot record tick heartbeat: {e:#}");
    }
    // Master switch (`/cron off`): the ticker keeps ticking — the heartbeat
    // above stays truthful about liveness — but due jobs are left unclaimed
    // so they fire when the switch flips back. Claiming-then-skipping would
    // strand them; a skipped job is never a fired one.
    if !auto {
        return Ok(TickReport {
            fired: 0,
            errors: 0,
            delivered: Vec::new(),
        });
    }
    let owner = owner_stamp();
    let mut fired_ids = Vec::new();
    let mut report = TickReport {
        fired: 0,
        errors: 0,
        delivered: Vec::new(),
    };
    let now = crate::cron::now_secs();
    let due = store.claim_due_limited(now, &owner, MAX_CONCURRENT_FIRES, &fired_ids)?;
    // Same-task concurrency only: `AsyncRunner` is `?Send`, so never `spawn`.
    // Results re-attached by index, so counts and `delivered` order match serial.
    let mut pending = futures::stream::FuturesUnordered::new();
    for (idx, job) in due.into_iter().enumerate() {
        fired_ids.push(job.id.clone());
        pending.push(async move {
            let out = fire_one(store, runner, job, now, deliver).await;
            (idx, out)
        });
    }
    let mut ordered: Vec<Option<(crate::cron::RunStatus, Option<DeliveredFire>)>> = Vec::new();
    ordered.resize_with(pending.len(), || None);
    while let Some((idx, (status, saved))) = futures::StreamExt::next(&mut pending).await {
        ordered[idx] = Some((status, saved));
    }
    for slot in ordered.into_iter().flatten() {
        let (status, saved) = slot;
        report.fired += 1;
        if !matches!(status, crate::cron::RunStatus::Ok) {
            report.errors += 1;
        }
        if let Some(saved) = saved {
            report.delivered.push(saved);
        }
    }
    Ok(report)
}

/// Live-chat delivery for one fired job (hermes `_deliver_result` frame):
/// the wrapped `Cronjob Response:` box plus the output-file path. Pure, so
/// every driver (REPL, `tick`, `run`) renders the same line.
pub fn format_fire_chat(saved: &DeliveredFire) -> String {
    let body = crate::cron_fire::format_delivery(&saved.name, &saved.id, &saved.excerpt);
    format!("{body}\nFull output: {}", saved.path.display())
}

/// Tick every 60s until SIGINT. Supervision owns the process; there is no
/// daemonization here. Tick-level store errors log and continue.
pub async fn serve_loop(
    store: crate::cron::CronStore,
    deliver: SaveLocalDeliver,
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
