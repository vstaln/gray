//! `gray-cron-sidecar`: cron as a real out-of-process sidecar plugin.
//!
//! Reuses the `gray-cron` store/parser/scheduler (no reimplementation) and
//! speaks the frozen v1.1 wire: manifest with `capabilities:["session"]` and
//! `subcommands:["/cron"]`, real `tool/call` (`cron.add`/`cron.list`/
//! `cron.remove`) + `command/run` (`/cron …`) against the same
//! `$GRAY_HOME/cron/jobs.json` the in-process paths use, and a 60 s ticker
//! that fires due jobs via `host/run` and reports via `host/say`
//! (sidecar-originated **string** ids; host ids stay numeric).
//!
//! The first scan waits a full tick so short-lived hosts (`gray plugin
//! check`, `-p`, `--dump-manifest`) exit before any scan side effects. Each
//! due job is atomically claimed (`store::claim_job_run`, flock-guarded)
//! before dispatch, so concurrent tickers (two REPLs, the gateway loop)
//! never double-fire — the loser skips.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>;
type SharedStdout = Arc<tokio::sync::Mutex<tokio::io::Stdout>>;

/// Seconds between due-job scans. No immediate scan on boot (see module docs).
const TICK_SECS: u64 = 60;
/// `host/run` reply wait. Exceeds the host 30 s TTL, so a late reply is
/// always the host's loud `{"error":…}`, never a hang on our side.
const RUN_TIMEOUT_SECS: u64 = 60;

fn manifest() -> Value {
    json!({
        "name": "cron",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": "1.1",
        "capabilities": ["session"],
        "subcommands": ["/cron"],
        "tools": [
            {"name": "cron.add", "description": "Schedule a prompt to re-fire: 'in 10m', 'every 30m', cron '0 9 * * *', or '2026-02-03T14:00'", "parameters": {"type": "object", "properties": {"schedule": {"type": "string"}, "prompt": {"type": "string"}, "name": {"type": "string"}}, "required": ["schedule", "prompt"]}, "snippet": "cron.add <schedule> <prompt>"},
            {"name": "cron.list", "description": "List scheduled jobs", "parameters": {"type": "object"}, "snippet": "cron.list"},
            {"name": "cron.remove", "description": "Remove a job by id or name", "parameters": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}, "snippet": "cron.remove <id>"}
        ],
        "commands": [],
        "hooks": []
    })
}

fn list_text() -> String {
    let jobs = gray_cron::list_jobs();
    if jobs.is_empty() {
        return "no cron jobs — schedule one with cron.add or /cron add".to_string();
    }
    let mut out = format!("{:<10} {:<20} {:<16} NEXT RUN", "ID", "NAME", "SCHEDULE");
    for j in jobs {
        let next = j
            .next_run
            .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_else(|| "-".to_string());
        let enabled = if j.enabled { "" } else { " (paused)" };
        out.push_str(&format!(
            "\n{:<10} {:<20} {:<16} {}{}",
            j.id, j.name, j.schedule, next, enabled
        ));
    }
    out
}

/// `tool/call` dispatch. Returns `(content, is_error)`; failures are data,
/// never silence (the host surfaces `is_error` content to the model).
fn tool_call(name: &str, args: &Value) -> (String, bool) {
    let err = |m: String| (m, true);
    match name {
        "cron.add" => {
            let schedule = args
                .get("schedule")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .trim();
            let prompt = args
                .get("prompt")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .trim();
            if schedule.is_empty() || prompt.is_empty() {
                return err("cron.add needs {schedule, prompt}".to_string());
            }
            if let Err(e) = gray_cron::parse_schedule(schedule) {
                return err(format!("invalid schedule '{schedule}': {e}"));
            }
            let job_name = args
                .get("name")
                .and_then(|n| n.as_str())
                .filter(|n| !n.trim().is_empty())
                .map(|n| n.to_string())
                .unwrap_or_else(|| format!("job-{}", prompt.chars().take(12).collect::<String>()));
            match gray_cron::create_job(job_name.clone(), schedule.to_string(), prompt.to_string())
            {
                Ok(job) => {
                    let next = job
                        .next_run
                        .map(|t| t.to_string())
                        .unwrap_or_else(|| "-".to_string());
                    (
                        format!(
                            "scheduled '{job_name}' — id {} — schedule: {} — next: {next} — prompt: \"{prompt}\" — manage with /cron list|remove",
                            job.id, job.schedule
                        ),
                        false,
                    )
                }
                Err(e) => err(format!("cron.add failed: {e}")),
            }
        }
        "cron.list" => (list_text(), false),
        "cron.remove" => {
            let id = args.get("id").and_then(|s| s.as_str()).unwrap_or("").trim();
            if id.is_empty() {
                return err("cron.remove needs {id}".to_string());
            }
            match gray_cron::remove_job(id) {
                Ok(true) => (format!("removed {id}"), false),
                Ok(false) => err(format!("no job found for '{id}'")),
                Err(e) => err(format!("cron.remove failed: {e}")),
            }
        }
        _ => err(format!(
            "unknown tool '{name}' (available: cron.add, cron.list, cron.remove)"
        )),
    }
}

/// `command/run` for `/cron`. Always returns non-empty text: the adapter
/// filters empty `Say` to `None`, which the REPL would read as "unknown command".
fn command_run(argv: &[String]) -> String {
    let (cmd, rest) = match argv.split_first() {
        Some((c, r)) => (c.as_str(), r),
        None => return list_text(),
    };
    match cmd {
        "list" => list_text(),
        "show" => {
            let id = rest.first().map(|s| s.as_str()).unwrap_or("");
            if id.is_empty() {
                return "usage: /cron show <id>".to_string();
            }
            match gray_cron::find_job(id) {
                Some(j) => serde_json::to_string_pretty(&j).unwrap_or_else(|_| "{}".to_string()),
                None => format!("no job found for '{id}'"),
            }
        }
        "remove" => {
            let id = rest.first().map(|s| s.as_str()).unwrap_or("");
            if id.is_empty() {
                return "usage: /cron remove <id>".to_string();
            }
            match gray_cron::remove_job(id) {
                Ok(true) => format!("removed {id}"),
                Ok(false) => format!("no job found for '{id}'"),
                Err(e) => format!("cron remove failed: {e}"),
            }
        }
        "add" => {
            let input = rest.join(" ");
            match gray_cron::schedule::split_human_input(&input) {
                Some((schedule, prompt)) => {
                    if let Err(e) = gray_cron::parse_schedule(&schedule) {
                        return format!("invalid schedule '{schedule}': {e}");
                    }
                    let name = format!("job-{}", prompt.chars().take(12).collect::<String>());
                    match gray_cron::create_job(
                        name.clone(),
                        schedule.clone(),
                        prompt.clone(),
                    ) {
                        Ok(job) => format!(
                            "created cron job {} (\"{}\") — schedule: {} — next: {}",
                            job.id,
                            job.name,
                            job.schedule,
                            job.next_run
                                .map(|t| t.to_string())
                                .unwrap_or_else(|| "-".to_string())
                        ),
                        Err(e) => format!("cron add failed: {e}"),
                    }
                }
                None => "usage: /cron add <prompt every 30m|in 10m> — e.g. /cron add check inbox every 30m"
                    .to_string(),
            }
        }
        _ => "usage: /cron list | add <prompt every 30m|in 10m> | remove <id> | show <id>"
            .to_string(),
    }
}

async fn write_line(stdout: &SharedStdout, v: &Value) {
    let mut out = stdout.lock().await;
    let _ = out.write_all(format!("{v}\n").as_bytes()).await;
    let _ = out.flush().await;
}

async fn tick(stdout: SharedStdout, pending: Pending, cwd: String) {
    let due = match gray_cron::Scheduler::from_active().scan_due_jobs() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("cron sidecar scan failed: {e}");
            return;
        }
    };
    for job in due {
        // Atomic claim first: a concurrent ticker that won the race already
        // advanced this job, so `claim` is false and we skip (no double-fire).
        if !gray_cron::store::claim_job_run(&job.id, chrono::Utc::now()) {
            continue;
        }
        let (stdout, pending, cwd) = (stdout.clone(), pending.clone(), cwd.clone());
        tokio::spawn(async move {
            fire_one(&stdout, &pending, &cwd, job).await;
        });
    }
}

async fn fire_one(stdout: &SharedStdout, pending: &Pending, cwd: &str, job: gray_cron::CronJob) {
    eprintln!("cron sidecar firing '{}' ({})", job.name, job.id);
    let id = format!(
        "cron-run-{}-{}",
        job.id,
        chrono::Utc::now().timestamp_millis()
    );
    let (tx, rx) = oneshot::channel();
    pending.lock().unwrap().insert(id.clone(), tx);
    write_line(
        stdout,
        &json!({"id": id, "method": "host/run", "params": {"session": {"id": "", "cwd": cwd}, "prompt": job.prompt}}),
    )
    .await;
    let result = match tokio::time::timeout(Duration::from_secs(RUN_TIMEOUT_SECS), rx).await {
        Ok(Ok(v)) => v,
        Ok(Err(_)) => json!({"error": "host/run reply channel closed"}),
        Err(_) => {
            pending.lock().unwrap().remove(&id);
            json!({"error": "host/run timed out"})
        }
    };
    let text = match result.get("text").and_then(|t| t.as_str()) {
        Some(t) => t.to_string(),
        None => {
            let detail = result
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("unexpected reply");
            format!("cron '{}' failed: {}", job.name, detail)
        }
    };
    let say_id = format!(
        "cron-say-{}-{}",
        job.id,
        chrono::Utc::now().timestamp_millis()
    );
    write_line(
        stdout,
        &json!({"id": say_id, "method": "host/say", "params": {"text": format!("⏰ {} ({})\n\n{text}", job.name, job.id)}}),
    )
    .await;
    // The host's `host/say` reply arrives as another string-id line and is
    // dropped by the reader (nothing pending) — fire-and-forget by design.
}

#[tokio::main]
async fn main() {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string());
    let stdout: SharedStdout = Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    {
        let (stdout, pending, cwd) = (stdout.clone(), pending.clone(), cwd.clone());
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(TICK_SECS)).await;
                tick(stdout.clone(), pending.clone(), cwd.clone()).await;
            }
        });
    }
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
        // No method + string id: reply to one of our `host/*` requests.
        if method.is_empty() {
            if let Some(id) = v.get("id").and_then(|i| i.as_str())
                && let Some(tx) = pending.lock().unwrap().remove(id)
            {
                let _ = tx.send(v.get("result").cloned().unwrap_or(Value::Null));
            }
            continue;
        }
        match method {
            "plugin/shutdown" => std::process::exit(0),
            "plugin/manifest" => {
                let Some(n) = v.get("id").and_then(|i| i.as_u64()) else {
                    continue;
                };
                write_line(&stdout, &json!({"id": n, "result": manifest()})).await;
            }
            "tool/call" => {
                let Some(n) = v.get("id").and_then(|i| i.as_u64()) else {
                    continue;
                };
                let params = v.get("params").cloned().unwrap_or(Value::Null);
                let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let args = params.get("args").cloned().unwrap_or(Value::Null);
                let (content, is_error) = tool_call(name, &args);
                let mut result = json!({"content": content});
                if is_error {
                    result["is_error"] = json!(true);
                }
                write_line(&stdout, &json!({"id": n, "result": result})).await;
            }
            "command/run" => {
                let Some(n) = v.get("id").and_then(|i| i.as_u64()) else {
                    continue;
                };
                let argv: Vec<String> = v
                    .get("params")
                    .and_then(|p| p.get("argv"))
                    .and_then(|a| a.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|e| e.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                write_line(
                    &stdout,
                    &json!({"id": n, "result": {"text": command_run(&argv)}}),
                )
                .await;
            }
            _ => {} // event/notify + unknowns: no id, no reply
        }
    }
}
