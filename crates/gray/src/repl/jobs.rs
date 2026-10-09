//! Background work a session is waiting on: its running bash jobs and its
//! scheduled wakes (cron jobs whose delivery comes back to this session).
//! One source feeds the footer segment (`2 jobs · wake in 28m`), the
//! `/jobs` picker and its headless listing.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gray_core::agent::{BackgroundJob, ToolContext, ToolExecutor};

use crate::setup::{ManagerItem, ManagerSpec, run_install_manager};

/// A scheduled job that will wake this session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Wake {
    pub id: String,
    pub name: String,
    pub at: i64,
}

/// The cron jobs that will come back to `sid`: active, enabled, scheduled,
/// and delivered to that REPL session. Soonest first.
pub(crate) fn session_wakes(jobs: &[crate::cron::CronJob], sid: &str) -> Vec<Wake> {
    let mut wakes: Vec<Wake> = jobs
        .iter()
        .filter(|j| {
            j.enabled
                && j.state == crate::cron::store::JobState::Active
                && matches!(j.deliver, crate::cron::Deliver::Origin)
                && j.origin.as_ref().is_some_and(|o| {
                    o.platform == crate::cron_serve::SESSION_PLATFORM && o.chat == sid
                })
        })
        .filter_map(|j| {
            Some(Wake {
                id: j.id.clone(),
                name: j.name.clone(),
                at: j.next_run_at?,
            })
        })
        .collect();
    wakes.sort_by_key(|w| w.at);
    wakes
}

/// `45s`, `28m`, `2h 5m`, `3d 4h`; `now` once due.
fn until(secs: i64) -> String {
    match secs {
        s if s <= 0 => "now".to_string(),
        s if s < 60 => format!("{s}s"),
        // Rounded up: `wake in 1m` until it is due, never `in 0m`.
        s if s < 3600 => format!("{}m", (s + 59) / 60),
        s if s < 86_400 => match (s / 3600, (s % 3600) / 60) {
            (h, 0) => format!("{h}h"),
            (h, m) => format!("{h}h {m}m"),
        },
        s => match (s / 86_400, (s % 86_400) / 3600) {
            (d, 0) => format!("{d}d"),
            (d, h) => format!("{d}d {h}h"),
        },
    }
}

/// A running job's age: `45s`, `3m 12s`, `2h 5m`.
fn age(secs: u64) -> String {
    match (secs / 3600, (secs % 3600) / 60, secs % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, s) => format!("{m}m {s:02}s"),
        (h, m, _) => format!("{h}h {m}m"),
    }
}

fn in_or_now(secs: i64) -> String {
    match until(secs) {
        now if now == "now" => now,
        t => format!("in {t}"),
    }
}

/// The footer segment, or `None` when nothing is pending:
/// `1 job`, `2 jobs · wake in 28m`, `3 wakes · next in 5m`.
pub(crate) fn footer_label(jobs: usize, wakes: &[Wake], now: i64) -> Option<String> {
    let mut parts = Vec::new();
    match jobs {
        0 => {}
        1 => parts.push("1 job".to_string()),
        n => parts.push(format!("{n} jobs")),
    }
    if let Some(next) = wakes.first() {
        let when = in_or_now(next.at - now);
        parts.push(match wakes.len() {
            1 => format!("wake {when}"),
            n => format!("{n} wakes \u{b7} next {when}"),
        });
    }
    (!parts.is_empty()).then(|| parts.join(" \u{b7} "))
}

fn job_row(job: &BackgroundJob) -> String {
    let state = if job.stopping { "stopping" } else { "running" };
    format!(
        "job {} \u{2014} {state} {}",
        job.id,
        age(job.elapsed.as_secs())
    )
}

fn wake_row(wake: &Wake, now: i64) -> String {
    format!(
        "wake {} ({}) \u{2014} {}",
        wake.name,
        &wake.id[..8.min(wake.id.len())],
        in_or_now(wake.at - now)
    )
}

/// Picker rows. Names carry the kind (`job:<id>`, `wake:<id>`) so one
/// remove handler stops a job or deletes a wake.
pub(crate) fn items(jobs: &[BackgroundJob], wakes: &[Wake], now: i64) -> Vec<ManagerItem> {
    let job_items = jobs.iter().map(|j| ManagerItem {
        name: format!("job:{}", j.id),
        row: job_row(j),
        lit: !j.stopping,
        enabled: false,
        // A job already stopping has nothing left to remove.
        read_only: j.stopping,
        needs_setup: false,
    });
    let wake_items = wakes.iter().map(|w| ManagerItem {
        name: format!("wake:{}", w.id),
        row: wake_row(w, now),
        lit: true,
        enabled: false,
        read_only: false,
        needs_setup: false,
    });
    job_items.chain(wake_items).collect()
}

/// Headless `/jobs`: the picker's rows as text.
pub(crate) fn dashboard(jobs: &[BackgroundJob], wakes: &[Wake], now: i64) -> String {
    if jobs.is_empty() && wakes.is_empty() {
        return JOBS_SPEC.empty_hint.to_string();
    }
    jobs.iter()
        .map(job_row)
        .chain(wakes.iter().map(|w| wake_row(w, now)))
        .collect::<Vec<_>>()
        .join("\n")
}

const JOBS_SPEC: ManagerSpec = ManagerSpec {
    title: "Background",
    empty_hint: "nothing pending \u{2014} no background jobs or scheduled wakes",
    error_verb: "stop failed",
    supports_toggle: false,
    supports_remove: true,
    errors_tab: false,
    keep_stale_on_relist_error: false,
};

/// This session's wakes from the cron store; empty when it can't be read.
pub(crate) fn load_wakes(home: &Path, sid: Option<&str>) -> Vec<Wake> {
    let Some(sid) = sid else {
        return Vec::new();
    };
    crate::cron::CronStore::open(home.join("cron"))
        .and_then(|store| store.list())
        .map(|jobs| session_wakes(&jobs, sid))
        .unwrap_or_default()
}

/// Stop a job (`job:<id>`) or delete a wake (`wake:<id>`).
pub(crate) fn remove(
    exec: &dyn ToolExecutor,
    ctx: &ToolContext,
    home: &Path,
    name: &str,
) -> anyhow::Result<()> {
    if let Some(id) = name.strip_prefix("job:") {
        anyhow::ensure!(
            exec.cancel_background(ctx, id),
            "job {id} is not running \u{2014} it may have just finished"
        );
        return Ok(());
    }
    if let Some(id) = name.strip_prefix("wake:") {
        let store = crate::cron::CronStore::open(home.join("cron"))?;
        anyhow::ensure!(
            store.remove(id)?,
            "unknown wake {id:?} \u{2014} it may have already fired"
        );
        return Ok(());
    }
    anyhow::bail!("not a job or a wake: {name}")
}

/// The `/jobs` picker: `u`/Delete twice stops a job or deletes a wake.
pub(crate) fn run_jobs_modal(
    bg: Option<&crate::setup::BackgroundSnapshot>,
    home: &Path,
    exec: Arc<dyn ToolExecutor>,
    ctx: ToolContext,
) -> anyhow::Result<bool> {
    let sid = ctx.session_id.clone();
    run_install_manager(
        bg,
        None,
        &JOBS_SPEC,
        || {
            let wakes = load_wakes(home, sid.as_deref());
            let jobs = exec.background_jobs(&ctx);
            Some(items(&jobs, &wakes, crate::cron::now_secs()))
        },
        |name| remove(exec.as_ref(), &ctx, home, name),
        |_, _| anyhow::bail!("nothing to toggle here"),
    )
}

/// Keeps the footer segment current: every second, count the live
/// session's running jobs and its wakes and push a changed label to the
/// composer. Runs for the whole REPL, turns included, so a job started
/// mid-turn shows at once and a wake's countdown ticks.
pub(crate) fn spawn_footer_poller() {
    tokio::spawn(async move {
        let home = crate::setup::gray_home().ok();
        let mut shown: Option<String> = None;
        let mut wakes: Vec<Wake> = Vec::new();
        let mut wakes_for: Option<String> = None;
        let mut tick = 0u64;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            tick = tick.wrapping_add(1);
            let source = crate::host::background_source();
            let jobs = source
                .as_ref()
                .map_or(0, |(exec, ctx)| exec.background_jobs(ctx).len());
            // The store read takes its file lock: every 5s (or at once on a
            // session switch) is plenty, the countdown runs off the cache.
            let sid = crate::host::live_session();
            if tick.is_multiple_of(5) || sid != wakes_for {
                wakes = home
                    .as_deref()
                    .map(|h| load_wakes(h, sid.as_deref()))
                    .unwrap_or_default();
                wakes_for = sid;
            }
            let label = footer_label(jobs, &wakes, crate::cron::now_secs());
            if label == shown {
                continue;
            }
            let Some(shared) = crate::host::registered_tui() else {
                continue;
            };
            // Never block a painter: a busy lock just retries next tick.
            if let Ok(mut tui) = shared.try_lock() {
                tui.set_background_work(label.clone());
                shown = label;
            }
        }
    });
}

/// The context host-side job calls use: jobs belong to a session id.
pub(crate) fn session_ctx(cwd: &Path, sid: Option<&str>) -> ToolContext {
    ToolContext {
        cwd: cwd.to_path_buf(),
        cancel: tokio_util::sync::CancellationToken::new(),
        session_id: sid.map(str::to_string),
    }
}

/// How long a quit warning stays armed: a second quit inside it confirms.
const QUIT_CONFIRM_WINDOW: Duration = Duration::from_secs(10);

static QUIT_ARMED_AT: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// The warning a first quit gets while jobs run, or `None` to go ahead.
/// Pure: `armed` says whether a warning is already standing.
pub(crate) fn quit_warning(running: &[BackgroundJob], armed: bool) -> Option<String> {
    if running.is_empty() || armed {
        return None;
    }
    let names: Vec<&str> = running.iter().map(|j| j.id.as_str()).collect();
    let count = match running.len() {
        1 => "1 background job is".to_string(),
        n => format!("{n} background jobs are"),
    };
    Some(format!(
        "{count} still running ({}). Quit again within {}s to stop {} and exit, or /jobs to manage.",
        names.join(", "),
        QUIT_CONFIRM_WINDOW.as_secs(),
        if running.len() == 1 { "it" } else { "them" },
    ))
}

/// The quit gate: `Some(warning)` the first time a quit meets running jobs
/// (and arms the confirm window); `None` when nothing runs or the user
/// already confirmed by quitting again in time.
pub(crate) fn confirm_quit(exec: Option<&dyn ToolExecutor>, ctx: &ToolContext) -> Option<String> {
    let running = exec.map(|e| e.background_jobs(ctx)).unwrap_or_default();
    let mut armed_at = QUIT_ARMED_AT.lock().unwrap_or_else(|e| e.into_inner());
    let armed = armed_at.is_some_and(|t| t.elapsed() < QUIT_CONFIRM_WINDOW);
    let warning = quit_warning(&running, armed);
    *armed_at = warning.as_ref().map(|_| std::time::Instant::now());
    warning
}

/// Stop every running job of the session and give the workers a moment to
/// kill their process groups. Without this a job outlives gray: its group
/// is only killed when the worker's guard drops, which `process::exit`
/// (the signal exit) never does.
pub(crate) async fn stop_all(exec: &dyn ToolExecutor, ctx: &ToolContext) {
    let running = exec.background_jobs(ctx);
    if running.is_empty() {
        return;
    }
    for job in &running {
        exec.cancel_background(ctx, &job.id);
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while !exec.background_jobs(ctx).is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// [`stop_all`] for the signal exit, which runs outside the loop and is
/// sync: cancel through the registered source, then wait briefly on this
/// thread while the runtime's workers do the killing.
pub(crate) fn stop_all_blocking() {
    let Some((exec, ctx)) = crate::host::background_source() else {
        return;
    };
    let running = exec.background_jobs(&ctx);
    if running.is_empty() {
        return;
    }
    for job in &running {
        exec.cancel_background(&ctx, &job.id);
    }
    let deadline = std::time::Instant::now() + Duration::from_millis(1500);
    while !exec.background_jobs(&ctx).is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[path = "jobs_tests.rs"]
#[cfg(test)]
mod tests;
