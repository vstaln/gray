use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gray_core::agent::{BackgroundJob, ToolContext, ToolExecutor};

use crate::setup::{ManagerItem, ManagerOutcome, ManagerSpec, run_install_manager};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Wake {
    pub id: String,
    pub name: String,
    pub at: i64,
}

/// One row of `gray cron list --json`: the only slice of the job-file
/// contract core still consumes. The gateway plugin owns the format; fields
/// beyond these are ignored and a malformed row is skipped, never an error.
#[derive(Debug, serde::Deserialize)]
struct ListedJob {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    deliver: serde_json::Value,
    #[serde(default)]
    origin: Option<ListedOrigin>,
    #[serde(default)]
    next_run_at: Option<i64>,
}

#[derive(Debug, serde::Deserialize)]
struct ListedOrigin {
    platform: String,
    chat: String,
}

fn session_wakes(jobs: &[ListedJob], sid: &str) -> Vec<Wake> {
    let mut wakes: Vec<Wake> = jobs
        .iter()
        .filter(|j| {
            j.enabled.unwrap_or(true)
                && j.state.as_deref().unwrap_or("active") == "active"
                && j.deliver.as_str() == Some("origin")
                && j.origin.as_ref().is_some_and(|o| {
                    o.platform == crate::session_mailbox::SESSION_PLATFORM && o.chat == sid
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

fn until(secs: i64) -> String {
    match secs {
        s if s <= 0 => "now".to_string(),
        s if s < 60 => format!("{s}s"),

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

pub(crate) fn items(jobs: &[BackgroundJob], wakes: &[Wake], now: i64) -> Vec<ManagerItem> {
    let job_items = jobs.iter().map(|j| ManagerItem {
        name: format!("job:{}", j.id),
        row: job_row(j),
        lit: !j.stopping,
        enabled: false,

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
    enter_invokes: false,
    supports_remove: true,
    errors_tab: false,
    keep_stale_on_relist_error: false,
};

/// Hard cap on wake-list staleness; a changed jobs.json mtime usually forces
/// a refetch sooner, so this only bounds the same-mtime corner.
const WAKES_STALE_AFTER: Duration = Duration::from_secs(60);

struct WakesCache {
    home: PathBuf,
    sid: String,
    stamp: Option<std::time::SystemTime>,
    fetched: Instant,
    wakes: Vec<Wake>,
}

static WAKES_CACHE: Mutex<Option<WakesCache>> = Mutex::new(None);

fn jobs_stamp(home: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(home.join("cron").join("jobs.json"))
        .and_then(|m| m.modified())
        .ok()
}

fn run_with_timeout(
    cmd: &mut std::process::Command,
    limit: Duration,
) -> Option<std::process::Output> {
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().ok()?;
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let stdout_reader = std::thread::spawn(move || {
        let mut stdout = stdout;
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut stderr = stderr;
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });
    let started = Instant::now();
    let status = loop {
        if started.elapsed() >= limit {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    Some(std::process::Output {
        status,
        stdout: stdout_reader.join().ok()?,
        stderr: stderr_reader.join().ok()?,
    })
}

/// `gray cron list --json` through the installed plugin — the same verb
/// resolution a user-level `gray cron …` takes. Any failure (no gateway
/// plugin, nonzero exit) reads as "no jobs", never an error.
fn cron_list_json(home: &Path) -> Option<Vec<u8>> {
    let exe = crate::update::exe_watch::on_disk_exe()
        .or_else(|_| std::env::current_exe())
        .ok()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["cron", "list", "--json"])
        .env("GRAY_HOME", home)
        .stdin(std::process::Stdio::null());
    let out = run_with_timeout(&mut cmd, Duration::from_secs(5))?;
    out.status.success().then_some(out.stdout)
}

fn wakes_from_json(body: &[u8], sid: &str) -> Vec<Wake> {
    let rows = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("jobs").and_then(|j| j.as_array()).cloned())
        .unwrap_or_default();
    let jobs: Vec<ListedJob> = rows
        .iter()
        .filter_map(|r| serde_json::from_value(r.clone()).ok())
        .collect();
    session_wakes(&jobs, sid)
}

pub(crate) fn load_wakes(home: &Path, sid: Option<&str>) -> Vec<Wake> {
    let Some(sid) = sid else {
        return Vec::new();
    };
    let stamp = jobs_stamp(home);
    {
        let cache = WAKES_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = cache.as_ref() {
            let fresh =
                c.stamp == stamp && (stamp.is_none() || c.fetched.elapsed() <= WAKES_STALE_AFTER);
            if c.home == home && c.sid == sid && fresh {
                return c.wakes.clone();
            }
        }
    }
    let wakes = wakes_from_json(&cron_list_json(home).unwrap_or_default(), sid);
    *WAKES_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some(WakesCache {
        home: home.to_path_buf(),
        sid: sid.to_string(),
        stamp,
        fetched: Instant::now(),
        wakes: wakes.clone(),
    });
    wakes
}

pub(crate) fn remove(
    exec: &dyn ToolExecutor,
    ctx: &ToolContext,
    home: &Path,
    name: &str,
) -> anyhow::Result<()> {
    remove_with(exec, ctx, home, name, remove_wake)
}

fn remove_with(
    exec: &dyn ToolExecutor,
    ctx: &ToolContext,
    home: &Path,
    name: &str,
    cron_rm: impl Fn(&Path, &str) -> anyhow::Result<bool>,
) -> anyhow::Result<()> {
    if let Some(id) = name.strip_prefix("job:") {
        anyhow::ensure!(
            exec.cancel_background(ctx, id),
            "job {id} is not running \u{2014} it may have just finished"
        );
        return Ok(());
    }
    if let Some(id) = name.strip_prefix("wake:") {
        anyhow::ensure!(
            cron_rm(home, id)?,
            "unknown wake {id:?} \u{2014} it may have already fired"
        );
        return Ok(());
    }
    anyhow::bail!("not a job or a wake: {name}")
}

/// `gray cron remove <id>` through the plugin — the store is not core's to
/// edit. Ok(false) is the plugin's "unknown job" answer.
pub(crate) fn remove_wake(home: &Path, id: &str) -> anyhow::Result<bool> {
    let removed = cron_remove(home, id)?;
    if removed {
        *WAKES_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    Ok(removed)
}

fn cron_remove(home: &Path, id: &str) -> anyhow::Result<bool> {
    let exe = crate::update::exe_watch::on_disk_exe().or_else(|_| std::env::current_exe())?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["cron", "remove", id])
        .env("GRAY_HOME", home)
        .stdin(std::process::Stdio::null());
    let out = run_with_timeout(&mut cmd, Duration::from_secs(15))
        .ok_or_else(|| anyhow::anyhow!("`gray cron remove` timed out"))?;
    if out.status.success() {
        return Ok(true);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("unknown cron job") {
        return Ok(false);
    }
    let detail = stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.strip_prefix("Error: ").unwrap_or(l))
        .unwrap_or("`gray cron remove` failed");
    anyhow::bail!("{detail}")
}

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
            Some(items(&jobs, &wakes, gray_core::time::now_secs() as i64))
        },
        |name| remove(exec.as_ref(), &ctx, home, name),
        |_, _| anyhow::bail!("nothing to toggle here"),
    )
    .map(ManagerOutcome::into_changed)
}

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

            let sid = crate::host::live_session();
            if tick.is_multiple_of(5) || sid != wakes_for {
                if let Some(h) = home.clone() {
                    let sid_for_load = sid.clone();
                    if let Ok(loaded) =
                        tokio::task::spawn_blocking(move || load_wakes(&h, sid_for_load.as_deref()))
                            .await
                    {
                        wakes = loaded;
                    }
                } else {
                    wakes = Vec::new();
                }
                wakes_for = sid;
            }
            let label = footer_label(jobs, &wakes, gray_core::time::now_secs() as i64);
            if label == shown {
                continue;
            }
            let Some(shared) = crate::host::registered_tui() else {
                continue;
            };

            if let Ok(mut tui) = shared.try_lock() {
                tui.set_background_work(label.clone());
                shown = label;
            }
        }
    });
}

pub(crate) fn session_ctx(cwd: &Path, sid: Option<&str>) -> ToolContext {
    ToolContext {
        cwd: cwd.to_path_buf(),
        cancel: tokio_util::sync::CancellationToken::new(),
        session_id: sid.map(str::to_string),
    }
}

const QUIT_CONFIRM_WINDOW: Duration = Duration::from_secs(10);

static QUIT_ARMED_AT: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

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

pub(crate) fn confirm_quit(exec: Option<&dyn ToolExecutor>, ctx: &ToolContext) -> Option<String> {
    let running = exec.map(|e| e.background_jobs(ctx)).unwrap_or_default();
    let mut armed_at = QUIT_ARMED_AT.lock().unwrap_or_else(|e| e.into_inner());
    let armed = armed_at.is_some_and(|t| t.elapsed() < QUIT_CONFIRM_WINDOW);
    let warning = quit_warning(&running, armed);
    *armed_at = warning.as_ref().map(|_| std::time::Instant::now());
    warning
}

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
