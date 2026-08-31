use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use fs2::FileExt;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub schedule: String,
    pub prompt: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub last_run: Option<DateTime<Utc>>,
    pub next_run: Option<DateTime<Utc>>,
}

impl CronJob {
    pub fn new(name: String, schedule: String, prompt: String) -> Self {
        let now = Utc::now();
        let next_run = crate::schedule::compute_next_run(&schedule, now);
        Self {
            id: uuid::Uuid::new_v4().to_string()[..8].to_string(),
            name,
            schedule,
            prompt,
            enabled: true,
            created_at: now,
            last_run: None,
            next_run,
        }
    }
}

// ---------------------------------------------------------------------------
// Paths — stolen from hermes-rs/crates/hermes-cron/src/jobs.rs:62-90
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CronStorePaths {
    pub cron_dir: PathBuf,
    pub jobs_file: PathBuf,
}

impl CronStorePaths {
    pub fn from_home(home: impl AsRef<Path>) -> Self {
        let cron_dir = home.as_ref().join("cron");
        Self {
            jobs_file: cron_dir.join("jobs.json"),
            cron_dir,
        }
    }
    pub fn active() -> Self {
        Self::from_home(cron_home())
    }
}

fn cron_home() -> PathBuf {
    std::env::var("GRAY_HOME")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".gray"))
                .unwrap_or_else(|_| PathBuf::from(".gray"))
        })
}

fn jobs_path() -> PathBuf {
    CronStorePaths::active().jobs_file
}

fn ensure_dirs(store: &CronStorePaths) {
    let _ = std::fs::create_dir_all(&store.cron_dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if store.cron_dir.exists() {
            let _ = std::fs::set_permissions(&store.cron_dir, std::fs::Permissions::from_mode(0o700));
        }
    }
}

// ---------------------------------------------------------------------------
// Locking — stolen from hermes-rs/crates/hermes-cron/src/jobs.rs:130-252
// trimmed to GRAY_HOME only, no fire-fence (added in Step 3)
// ---------------------------------------------------------------------------

fn job_lock_registry() -> &'static Mutex<HashMap<String, Arc<Mutex<()>>>> {
    static REG: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

thread_local! {
    static JOBS_LOCK_DEPTH: Cell<u32> = const { Cell::new(0) };
}

struct DepthGuard;
impl Drop for DepthGuard {
    fn drop(&mut self) {
        JOBS_LOCK_DEPTH.with(|d| d.set(0));
    }
}

fn registry_key(dir: &Path) -> String {
    dir.canonicalize()
        .unwrap_or_else(|_| dir.to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn lock_unpoisoned(m: &Mutex<()>) -> MutexGuard<'_, ()> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn acquire_flock(file: &std::fs::File, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return true,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return false,
        }
    }
}

const JOBS_LOCK_TIMEOUT_SECS: u64 = 30;

pub fn with_jobs_lock<T>(store: &CronStorePaths, f: impl FnOnce() -> T) -> T {
    if JOBS_LOCK_DEPTH.with(|d| d.get()) > 0 {
        return f();
    }
    let key = registry_key(&store.cron_dir);
    let reg = job_lock_registry();
    let proc_lock = reg
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(key.clone())
        .or_default()
        .clone();
    let _proc_guard = lock_unpoisoned(&proc_lock);

    ensure_dirs(store);
    let lock_path = store.cron_dir.join(".jobs.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(&lock_path)
        .ok();
    let locked = file
        .as_ref()
        .map(|f| acquire_flock(f, Duration::from_secs(JOBS_LOCK_TIMEOUT_SECS)))
        .unwrap_or(false);
    if file.is_some() && !locked {
        log::error!(
            "Timed out after {}s waiting for cron jobs lock ({}) — proceeding with in-process lock only",
            JOBS_LOCK_TIMEOUT_SECS,
            lock_path.display()
        );
    }
    JOBS_LOCK_DEPTH.with(|d| d.set(1));
    let _depth = DepthGuard;
    let out = f();
    if let Some(f) = file.as_ref() {
        if locked {
            let _ = f.unlock();
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Load / save — with hermes merge + envelope compat (hermes/jobs.rs:1023-1072)
// ---------------------------------------------------------------------------

fn peek_jobs_unlocked(store: &CronStorePaths) -> Option<Vec<CronJob>> {
    let data = std::fs::read_to_string(&store.jobs_file).ok()?;
    let trimmed = data.trim();
    if trimmed.is_empty() {
        return Some(Vec::new());
    }
    // Handle both Hermes envelope {"jobs": [...]} and Gray bare [...]
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(arr) = v.get("jobs").and_then(|j| j.as_array()) {
            return Some(
                arr.iter()
                    .filter_map(|x| serde_json::from_value::<CronJob>(x.clone()).ok())
                    .collect(),
            );
        }
        if let Some(arr) = v.as_array() {
            return Some(
                arr.iter()
                    .filter_map(|x| serde_json::from_value::<CronJob>(x.clone()).ok())
                    .collect(),
            );
        }
    }
    // Fallback: try bare Vec<CronJob>
    serde_json::from_str(trimmed).ok()
}

fn load_jobs_inner(store: &CronStorePaths) -> Vec<CronJob> {
    ensure_dirs(store);
    let data = match std::fs::read_to_string(&store.jobs_file) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let trimmed = data.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    // Hermes trims BOM
    let cleaned = trimmed.trim_start_matches('\u{feff}');
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(cleaned) {
        match v {
            serde_json::Value::Object(mut obj) => {
                if let Some(jobs_val) = obj.remove("jobs") {
                    match jobs_val {
                        serde_json::Value::Array(items) => {
                            let jobs: Vec<CronJob> = items
                                .into_iter()
                                .filter_map(|x| serde_json::from_value(x).ok())
                                .collect();
                            // Auto-repair: if jobs were stored as envelope, keep it (no write here)
                            return jobs;
                        }
                        serde_json::Value::Object(map) => {
                            // id-keyed map (hand-edited) — flatten
                            let jobs: Vec<CronJob> = map
                                .into_iter()
                                .filter_map(|(k, v)| {
                                    let mut rec = v.as_object()?.clone();
                                    if rec.get("id").and_then(|x| x.as_str()).is_none() {
                                        rec.insert("id".into(), serde_json::Value::String(k.clone()));
                                    }
                                    serde_json::from_value::<CronJob>(serde_json::Value::Object(rec)).ok()
                                })
                                .collect();
                            return jobs;
                        }
                        _ => return Vec::new(),
                    }
                }
                return Vec::new();
            }
            serde_json::Value::Array(items) => {
                return items
                    .into_iter()
                    .filter_map(|x| serde_json::from_value::<CronJob>(x).ok())
                    .collect();
            }
            _ => return Vec::new(),
        }
    }
    Vec::new()
}

pub fn load_jobs() -> Vec<CronJob> {
    with_jobs_lock(&CronStorePaths::active(), || load_jobs_inner(&CronStorePaths::active()))
}

fn merge_unexpected_disk_jobs(
    disk_jobs: &[CronJob],
    jobs: &[CronJob],
    removed_ids: &[String],
) -> Vec<CronJob> {
    let removed: std::collections::HashSet<&str> = removed_ids.iter().map(|s| s.as_str()).collect();
    let new_ids: std::collections::HashSet<&str> = jobs.iter().map(|j| j.id.as_str()).collect();
    let mut merged = jobs.to_vec();
    let mut seen = new_ids;
    for dj in disk_jobs {
        if seen.contains(dj.id.as_str()) || removed.contains(dj.id.as_str()) {
            continue;
        }
        // also check name for legacy callers that used name as id
        if seen.contains(dj.name.as_str()) {
            continue;
        }
        seen.insert(dj.id.as_str());
        merged.push(dj.clone());
    }
    merged
}

pub fn save_jobs(jobs: &[CronJob]) -> anyhow::Result<()> {
    let store = CronStorePaths::active();
    with_jobs_lock(&store, || save_jobs_inner(&store, jobs.to_vec(), &[], false))
}

fn save_jobs_inner(
    store: &CronStorePaths,
    mut jobs: Vec<CronJob>,
    removed_ids: &[String],
    replace: bool,
) -> anyhow::Result<()> {
    ensure_dirs(store);
    // Merge concurrent writers unless replace
    if !replace {
        if let Some(disk) = peek_jobs_unlocked(store) {
            jobs = merge_unexpected_disk_jobs(&disk, &jobs, removed_ids);
        }
    }
    // Try 5 times with re-peek to avoid stomping
    for _ in 0..5 {
        if !replace {
            let disk = peek_jobs_unlocked(store).unwrap_or_default();
            let stale = disk.iter().any(|dj| {
                let in_payload = jobs.iter().any(|j| j.id == dj.id);
                !in_payload && !removed_ids.iter().any(|r| r == &dj.id)
            });
            if stale {
                // re-merge and retry
                jobs = merge_unexpected_disk_jobs(&disk, &jobs, removed_ids);
                continue;
            }
        }
        // Atomic write via tempfile in same dir
        let tmp = tempfile::Builder::new()
            .prefix(".jobs_")
            .suffix(".tmp")
            .tempfile_in(&store.cron_dir)?;
        {
            use std::io::Write;
            let mut f = tmp.as_file();
            // Keep bare-array compat for now (Gray readers expect it), but also handle envelope on read
            let body = serde_json::to_string_pretty(&jobs)?;
            f.write_all(body.as_bytes())?;
            f.flush()?;
            f.sync_all()?;
        }
        tmp.persist(&store.jobs_file)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if store.jobs_file.exists() {
                let _ = std::fs::set_permissions(&store.jobs_file, std::fs::Permissions::from_mode(0o600));
            }
        }
        return Ok(());
    }
    // Final fallback
    let disk = peek_jobs_unlocked(store).unwrap_or_default();
    jobs = merge_unexpected_disk_jobs(&disk, &jobs, removed_ids);
    let body = serde_json::to_string_pretty(&jobs)?;
    std::fs::write(&store.jobs_file, body)?;
    Ok(())
}

pub fn list_jobs() -> Vec<CronJob> {
    let mut jobs = load_jobs();
    jobs.sort_by_key(|j| j.next_run);
    jobs
}

pub fn create_job(name: String, schedule: String, prompt: String) -> anyhow::Result<CronJob> {
    // Daily wall-clock cron "M H * * *" → interpret H:M as local time, store as UTC
    let schedule = normalize_daily_cron_to_utc(&schedule);
    let sched = crate::schedule::parse_schedule(&schedule)?;
    if sched.is_once() && sched.next_after(Utc::now()).is_none() {
        anyhow::bail!("one-shot time is in the past (beyond 2m grace) — use a future time like 'in 10m' or '2026-09-01T14:00'");
    }
    let store = CronStorePaths::active();
    with_jobs_lock(&store, || {
        let mut jobs = load_jobs_inner(&store);
        let job = CronJob::new(name, schedule, prompt);
        jobs.push(job.clone());
        save_jobs_inner(&store, jobs, &[], false)?;
        Ok(job) as anyhow::Result<CronJob>
    })
}

fn normalize_daily_cron_to_utc(s: &str) -> String {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() != 5 {
        return s.to_string();
    }
    // only simple daily "M H * * *" with numeric M/H
    if parts[2] != "*" || parts[3] != "*" || parts[4] != "*" {
        return s.to_string();
    }
    let (Ok(min), Ok(hour)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) else {
        return s.to_string();
    };
    if min >= 60 || hour >= 24 {
        return s.to_string();
    }
    // Convert local wall time H:M to UTC
    let offset_secs = chrono::Local::now().offset().local_minus_utc();
    let local_mins = (hour as i32) * 60 + min as i32;
    let mut utc_mins = local_mins - offset_secs / 60;
    utc_mins = ((utc_mins % 1440) + 1440) % 1440;
    let utc_hour = (utc_mins / 60) as u32;
    let utc_min = (utc_mins % 60) as u32;
    format!("{utc_min} {utc_hour} * * *")
}

pub fn remove_job(id: &str) -> anyhow::Result<bool> {
    let store = CronStorePaths::active();
    with_jobs_lock(&store, || {
        let mut jobs = load_jobs_inner(&store);
        let before = jobs.len();
        jobs.retain(|j| j.id != id && j.name != id);
        if jobs.len() == before {
            return Ok(false);
        }
        save_jobs_inner(&store, jobs, &[id.to_string()], false)?;
        Ok(true)
    })
}

pub fn find_job(id: &str) -> Option<CronJob> {
    load_jobs().into_iter().find(|j| j.id == id || j.name == id)
}

pub fn update_job_run(id: &str, now: DateTime<Utc>) -> anyhow::Result<()> {
    let store = CronStorePaths::active();
    with_jobs_lock(&store, || {
        let mut jobs = load_jobs_inner(&store);
        if let Some(job) = jobs.iter_mut().find(|j| j.id == id) {
            job.last_run = Some(now);
            // Herme s once logic: no next after first fire
            let is_once = crate::schedule::parse_schedule(&job.schedule)
                .map(|s| s.is_once())
                .unwrap_or(false);
            if is_once {
                job.next_run = None;
            } else {
                job.next_run = crate::schedule::compute_next_run(&job.schedule, now);
            }
        }
        save_jobs_inner(&store, jobs, &[], false)?;
        Ok(())
    })
}

/// For scheduler internals — paths pinned to active home
pub fn cron_store_paths() -> CronStorePaths {
    CronStorePaths::active()
}
