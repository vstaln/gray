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
    /// A reminder job: `excerpt` is the stored text, delivered verbatim.
    pub reminder: bool,
    /// The fire failed; `excerpt` is a short, redacted error line.
    pub failed: bool,
    /// Wall time of the agent turn (0 for reminders).
    pub elapsed_ms: u64,
}

/// What one fire produced. `transcript` goes to `cron/output/*.md` (redacted,
/// 0600); only `final_text` is ever delivered to a chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FireOutput {
    pub transcript: String,
    pub final_text: String,
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

    /// Like `run`, but keeps the final assistant message apart from the full
    /// transcript. Default: the runner has no such split, so both are `run`'s
    /// text (keeps stub runners in tests unchanged).
    async fn run_full(&self, prompt: String, cwd: PathBuf) -> anyhow::Result<FireOutput> {
        let text = self.run(prompt, cwd).await?;
        Ok(FireOutput {
            transcript: text.clone(),
            final_text: text,
        })
    }
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
        Ok(self.run_full(prompt, cwd).await?.transcript)
    }

    async fn run_full(&self, prompt: String, cwd: PathBuf) -> anyhow::Result<FireOutput> {
        let mut config = self.config.clone();
        if self.follow_switches {
            refresh_model_from_saved(&mut config);
        }
        // Same canonicalization as REPL/print: a stored `<base>-<tier>` id
        // rides the family row + its effort, not the literal variant.
        {
            let rows = crate::setup::canonical_model_rows(&config);
            crate::setup::canonicalize_effort_variant(&mut config, &rows);
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
        Ok(FireOutput {
            transcript: crate::cron_fire::transcript_text(&events),
            final_text: crate::cron_fire::final_assistant_text(&events),
        })
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
    /// The transcript is saved redacted (mode 0600 via `write_local_output`)
    /// and the chat — plus the origin mirror — gets ONLY the final text. A
    /// reminder's stored text is delivered verbatim: it is the user's own
    /// words, not a model answer that could carry a secret.
    pub async fn deliver_full(
        &self,
        job: &crate::cron::CronJob,
        now: i64,
        out: &FireOutput,
        elapsed_ms: u64,
    ) -> Result<DeliveredFire, String> {
        let transcript = crate::cron_fire::redact_secrets(&out.transcript, &self.home);
        let path = crate::cron_fire::write_local_output(&self.home, job, now, &transcript)
            .map_err(|e| format!("local write failed: {e:#}"))?;
        let shown = if job.reminder {
            out.final_text.clone()
        } else {
            crate::cron_fire::redact_secrets(&out.final_text, &self.home)
        };
        // Bounded excerpt: the full transcript is already on disk; the mirror
        // and the live box share this cap.
        let excerpt = crate::cron_fire::delivery_excerpt(&shown);
        let saved = |to_chat: bool| DeliveredFire {
            id: job.id.clone(),
            name: job.name.clone(),
            path: path.clone(),
            excerpt: excerpt.clone(),
            to_chat,
            reminder: job.reminder,
            failed: false,
            elapsed_ms,
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
                // A job added from an interactive gray session goes back to
                // that session's inbox: the REPL showing it renders the card
                // and runs a turn, which persists the note — so no mirror
                // (it would land twice) and no chat route (`to_chat: false`).
                if origin.platform == SESSION_PLATFORM {
                    post_to_session_inbox(&self.home, &origin.chat, &saved(true))
                        .map_err(|e| format!("session inbox write failed: {e:#}"))?;
                    return Ok(saved(false));
                }
                // Clean mirror (hermes parity): no wrapper, no file path.
                // `USER`, never assistant — an assistant-role mirror lands
                // assistant→assistant and breaks strict alternation;
                // consecutive user turns merge safely.
                // The mirror is context for a later reply, not the delivery.
                // A host whose chat id has no session yet (a job added in a
                // conversation's first turn) must still get its message, so
                // a failed mirror logs and the delivery stands.
                let note = crate::cron_fire::mirror_message(&job.name, &excerpt);
                let sessions =
                    crate::session_store::JsonlSessionStore::new(self.home.join("sessions"));
                let sid = crate::session_store::SessionId::new(origin.chat.clone());
                if let Err(e) = sessions
                    .append(&sid, &gray_core::message::Message::user_injected(note))
                    .await
                {
                    log::warn!(
                        "cron {}: no session {:?} to mirror into ({e:#}); delivering anyway",
                        job.id,
                        origin.chat
                    );
                }
                if is_hosted(origin) {
                    post_to_outbox(&self.home, &saved(true), origin)
                        .map_err(|e| format!("outbox write failed: {e:#}"))?;
                }
                Ok(saved(true))
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
    if job.reminder {
        // Literal text: no workdir, no script, no skills, no model turn, no
        // tools. The whole point of `--reminder`.
        let out = FireOutput {
            transcript: job.prompt.clone(),
            final_text: job.prompt.clone(),
        };
        return match deliver.deliver_full(&job, now, &out, 0).await {
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
        };
    }
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
    let started = std::time::Instant::now();
    let run_fut = std::panic::AssertUnwindSafe(runner.run_full(prompt, workdir));
    let out = match tokio::time::timeout(
        std::time::Duration::from_secs(FIRE_TIMEOUT_SECS),
        futures::FutureExt::catch_unwind(run_fut),
    )
    .await
    {
        Err(_) => {
            let msg = "fire exceeded 600s";
            return (
                fail(msg.to_string()),
                failure_delivery(deliver, &job, now, msg),
            );
        }
        Ok(Err(_)) => {
            let msg = "agent run panicked";
            return (
                fail(msg.to_string()),
                failure_delivery(deliver, &job, now, msg),
            );
        }
        Ok(Ok(Err(e))) => {
            let msg = format!("agent run failed: {e:#}");
            return (
                fail(msg.clone()),
                failure_delivery(deliver, &job, now, &msg),
            );
        }
        Ok(Ok(Ok(out))) => out,
    };
    let elapsed_ms = started.elapsed().as_millis() as u64;
    if crate::cron_fire::is_silent_response(&out.final_text) {
        let _ = store.mark_done(&job.id, job.fire_claim.as_ref(), RunStatus::Ok, None);
        return (RunStatus::Ok, None);
    }
    match deliver.deliver_full(&job, now, &out, elapsed_ms).await {
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

/// Chat delivery for a fire that failed before it produced output, so the
/// channel shows the failure instead of silence. Origin jobs only (the same
/// rule as a successful delivery); the message is redacted and capped to one
/// line. The full text is saved to `cron/output/` like any other fire.
fn failure_delivery(
    deliver: &SaveLocalDeliver,
    job: &crate::cron::CronJob,
    now: i64,
    msg: &str,
) -> Option<DeliveredFire> {
    if !matches!(job.deliver, crate::cron::Deliver::Origin) || job.origin.is_none() {
        return None;
    }
    let msg = crate::cron_fire::redact_secrets(msg, &deliver.home);
    let path = crate::cron_fire::write_local_output(&deliver.home, job, now, &msg).ok()?;
    let fired = DeliveredFire {
        id: job.id.clone(),
        name: job.name.clone(),
        path,
        excerpt: msg.lines().next().unwrap_or("").chars().take(300).collect(),
        to_chat: true,
        reminder: job.reminder,
        failed: true,
        elapsed_ms: 0,
    };
    // Same split as `deliver_full`: a session origin hears about the failure
    // through its inbox, never through a chat route.
    if let Some(origin) = job
        .origin
        .as_ref()
        .filter(|o| o.platform == SESSION_PLATFORM)
    {
        if let Err(e) = post_to_session_inbox(&deliver.home, &origin.chat, &fired) {
            log::warn!("cron {}: session inbox write failed: {e:#}", job.id);
        }
        return None;
    }
    if let Some(origin) = job.origin.as_ref().filter(|o| is_hosted(o))
        && let Err(e) = post_to_outbox(&deliver.home, &fired, origin)
    {
        log::warn!("cron {}: outbox write failed: {e:#}", job.id);
    }
    Some(fired)
}

/// `Origin.platform` for a job added from inside a gray session (the bash
/// tool exports `GRAY_SESSION_ID`): its result comes back into that chat.
pub const SESSION_PLATFORM: &str = "repl";

/// A chat a host (a chat plugin) delivers to by draining `cron tick --json`.
/// Not a gray session (`repl`, inbox) and not `local` (`--origin-session`
/// alone: mirror only, nobody routes it).
pub fn is_hosted(origin: &crate::cron::store::Origin) -> bool {
    origin.platform != SESSION_PLATFORM && origin.platform != "local"
}

/// Spool a hosted delivery as its `cron_delivery` line. Whatever driver fired
/// the job (gateway, `serve`, a REPL, an in-turn `tick` or `run`), the line
/// waits on disk until the host's `tick --json` drains it — firing is never
/// the moment a chat delivery is decided and lost.
fn post_to_outbox(
    home: &std::path::Path,
    fired: &DeliveredFire,
    origin: &crate::cron::store::Origin,
) -> anyhow::Result<()> {
    write_entry(
        &home.join("cron").join("outbox"),
        &delivery_json(fired, Some(origin)),
    )
}

/// Take every spooled `cron_delivery` line, oldest first. Removed before
/// returned (at-most-once, like the session inbox).
// ponytail: no ack — a host post that fails after the drain is lost; add a
// `cron delivered <file>` ack when posts fail in practice.
pub fn drain_outbox(home: &std::path::Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(home.join("cron").join("outbox")) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path())
        .collect();
    paths.sort();
    let mut out = Vec::new();
    for path in paths {
        let text = std::fs::read_to_string(&path);
        if let Err(e) = std::fs::remove_file(&path) {
            log::warn!("cron outbox: cannot remove {}: {e}", path.display());
            continue;
        }
        match text {
            Ok(t) if !t.trim().is_empty() => out.push(t.trim().to_string()),
            _ => log::warn!("cron outbox: dropped unreadable {}", path.display()),
        }
    }
    out
}

/// `<home>/cron/inbox/<session>`, or `None` when the id could escape the
/// directory (it comes from an env var and a hand-editable jobs file).
fn session_inbox(home: &std::path::Path, session: &str) -> Option<PathBuf> {
    let safe = !session.is_empty()
        && session
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    safe.then(|| home.join("cron").join("inbox").join(session))
}

/// One delivery for a session, waiting for the REPL that shows it: the card
/// it renders and the user-role note its turn starts from. Written tmp+rename
/// so a polling reader never sees half a file.
fn post_to_session_inbox(
    home: &std::path::Path,
    session: &str,
    fired: &DeliveredFire,
) -> anyhow::Result<()> {
    let dir = session_inbox(home, session)
        .ok_or_else(|| anyhow::anyhow!("unsafe session id {session:?}"))?;
    let body = if fired.failed {
        format!("(failed) {}", fired.excerpt)
    } else {
        fired.excerpt.clone()
    };
    // `card` stays for REPLs that predate the structured fields; current
    // ones paint the box from `name`/`body`/`status` (see [`CronCard`]).
    let entry = serde_json::json!({
        "card": format!("cron: {}", format_fire_chat(fired)),
        "prompt": crate::cron_fire::mirror_message(&fired.name, &body),
        "name": fired.name,
        "body": fired.excerpt,
        "status": if fired.failed { "failed" } else { "ok" },
        "reminder": fired.reminder,
        "elapsed_ms": fired.elapsed_ms,
    });
    write_entry(&dir, &entry.to_string())
}

/// Atomic 0600 drop of one JSON entry into a spool dir (inbox or outbox).
fn write_entry(dir: &std::path::Path, body: &str) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    // Name sorts by delivery time; the uuid keeps two fires in one ms apart.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let name = format!("{stamp:020}-{}.json", uuid::Uuid::new_v4());
    let tmp = dir.join(format!(".{name}.tmp"));
    // 0600 like the transcript in cron/output: the entry carries the result.
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    std::io::Write::write_all(&mut options.open(&tmp)?, body.as_bytes())?;
    std::fs::rename(&tmp, dir.join(name))?;
    Ok(())
}

/// True when the session's inbox holds a delivery (the REPL's cheap poll).
pub fn session_inbox_pending(home: &std::path::Path, session: &str) -> bool {
    session_inbox(home, session)
        .and_then(|d| std::fs::read_dir(d).ok())
        .is_some_and(|mut it| {
            it.any(|e| e.is_ok_and(|e| !e.file_name().to_string_lossy().starts_with('.')))
        })
}

/// Take every delivery waiting for `session`, oldest first, as
/// `(card, prompt)`. Each file is removed before it is returned, so one
/// delivery starts at most one turn even if two readers race; an entry that
/// cannot be removed or parsed is skipped (and logged), never redelivered.
/// What a delivery card shows: the job, how it ended, and its final answer
/// (never the transcript — that stays in `cron/output`).
#[derive(Debug, Clone, PartialEq)]
pub struct CronCard {
    pub name: String,
    pub body: String,
    pub failed: bool,
    pub reminder: bool,
    pub elapsed_ms: u64,
}

/// One drained session-inbox entry. `cron` is `None` for entries written
/// before the structured fields existed; those paint `card` as text.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionDelivery {
    pub card: String,
    pub prompt: String,
    pub cron: Option<CronCard>,
}

pub fn drain_session_inbox(home: &std::path::Path, session: &str) -> Vec<SessionDelivery> {
    let Some(dir) = session_inbox(home, session) else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path())
        .collect();
    paths.sort();
    let mut out = Vec::new();
    for path in paths {
        let text = std::fs::read_to_string(&path);
        if let Err(e) = std::fs::remove_file(&path) {
            log::warn!("cron inbox: cannot remove {}: {e}", path.display());
            continue;
        }
        let parsed = text
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| {
                let field = |k: &str| v.get(k)?.as_str().map(str::to_string);
                let cron = field("name").map(|name| CronCard {
                    name,
                    body: field("body").unwrap_or_default(),
                    failed: field("status").as_deref() == Some("failed"),
                    reminder: v.get("reminder").and_then(|r| r.as_bool()).unwrap_or(false),
                    elapsed_ms: v.get("elapsed_ms").and_then(|e| e.as_u64()).unwrap_or(0),
                });
                Some(SessionDelivery {
                    card: field("card")?,
                    prompt: field("prompt")?,
                    cron,
                })
            });
        match parsed {
            Some(entry) => out.push(entry),
            None => log::warn!("cron inbox: dropped unreadable {}", path.display()),
        }
    }
    out
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

/// Plain-text live-chat delivery for one fired job: the fallback for hosts
/// with no native renderer. No job id, no dashes, no stop/manage footer and
/// no file path (hosts get the path in `delivery_json`'s `path`, for logs).
/// Pure, so every driver (REPL, `tick`, `run`) renders the same text.
pub fn format_fire_chat(saved: &DeliveredFire) -> String {
    crate::cron_fire::format_delivery_plain(
        &saved.name,
        &saved.excerpt,
        saved.reminder,
        saved.failed,
    )
}

/// The `cron_delivery` line `gray cron tick --json` prints: the rendered
/// frame plus the routing the host needs. Core renders, the platform
/// carries — `route` stays opaque here.
pub fn delivery_json(saved: &DeliveredFire, origin: Option<&crate::cron::store::Origin>) -> String {
    let (platform, chat, thread, route) = match origin {
        Some(o) => (
            o.platform.clone(),
            o.chat.clone(),
            o.thread.clone(),
            o.route.clone(),
        ),
        None => (String::new(), String::new(), None, None),
    };
    serde_json::json!({
        "type": "cron_delivery",
        "job_id": saved.id,
        "name": saved.name,
        "platform": platform,
        "chat": chat,
        "thread": thread,
        "route": route,
        "kind": if saved.reminder { "reminder" } else { "task" },
        "status": if saved.failed { "failed" } else { "ok" },
        "elapsed_ms": saved.elapsed_ms,
        "final_text": saved.excerpt,
        "text": format_fire_chat(saved),
        "path": saved.path.display().to_string(),
    })
    .to_string()
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
