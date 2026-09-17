//! Gateway control socket: `$GRAY_HOME/gateway.sock`.
//!
//! Wire contract (hermes v1, ported): ONE request per connection — one JSON
//! line in, one JSON line out, then the server closes. Answers are
//! `{"ok": true, "protocol": 1, "result": {...}}`; misses are
//! `{"ok": false, "error": ..., "supported_verbs": [...]}`. A connectable
//! socket with a well-formed `identify` answer IS liveness — the pid file is
//! only the fallback. Never a TCP port: the filesystem (0600, inside the
//! user's home) is the auth boundary.

#[cfg(unix)]
use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const PROTOCOL: u32 = 1;
pub const SUPPORTED_VERBS: [&str; 2] = ["identify", "status"];
/// sun_path is 104..108 bytes; keep margin (same margin hermes uses).
#[cfg(unix)]
const MAX_SOCK_PATH: usize = 100;
#[cfg(unix)]
const MAX_REQUEST_BYTES: usize = 64 * 1024;
#[cfg(unix)]
const IO_TIMEOUT: Duration = Duration::from_secs(3);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(2);

pub fn sock_path(home: &Path) -> PathBuf {
    home.join("gateway.sock")
}

/// `identify`: who is answering (per-process truth, not the file's claim).
pub fn identify_payload(home: &Path) -> serde_json::Value {
    let me = std::process::id();
    let started_at = super::pid::read(home)
        .map(|r| r.started_at)
        .unwrap_or_else(crate::cron::now_secs);
    serde_json::json!({
        "protocol": PROTOCOL,
        "kind": "gray-gateway",
        "pid": me,
        "start_time": super::pid::proc_start_time(me),
        "gray_home": home.display().to_string(),
        "version": env!("CARGO_PKG_VERSION"),
        "supervisor": super::service::supervisor_kind(),
        "uptime_secs": crate::cron::now_secs().saturating_sub(started_at),
        "argv": std::env::args().collect::<Vec<_>>(),
    })
}

/// `status`: identify + runtime state + cron liveness, answered live.
pub fn status_payload(home: &Path, now: i64) -> serde_json::Value {
    let mut out = identify_payload(home);
    let state = super::state::read(home);
    out["gateway_state"] = serde_json::json!(
        state
            .as_ref()
            .map(|s| s.gateway_state.clone())
            .unwrap_or_else(|| super::state::STATE_RUNNING.to_string())
    );
    out["exit_reason"] = serde_json::json!(state.as_ref().and_then(|s| s.exit_reason.clone()));
    out["started_at"] = serde_json::json!(state.as_ref().map(|s| s.started_at));
    out["cron"] = cron_payload(home, now);
    out["answered_at"] = serde_json::json!(now);
    out["answering_pid"] = serde_json::json!(std::process::id());
    out
}

fn cron_payload(home: &Path, now: i64) -> serde_json::Value {
    match crate::cron::CronStore::open(home.join("cron")) {
        Ok(store) => {
            let health = store.health(now).ok();
            let jobs = store.list().map(|v| v.len()).unwrap_or(0);
            let last = health.as_ref().and_then(|h| h.last_tick.as_ref());
            serde_json::json!({
                "ticker_live": health.as_ref().is_some_and(|h| h.ticker_live(now)),
                "last_tick_at": last.map(|t| t.at),
                "last_tick_kind": last.map(|t| t.kind.clone()),
                "last_tick_secs_ago": last.map(|t| now.saturating_sub(t.at)),
                "overdue": health.as_ref().map(|h| h.overdue.len()).unwrap_or(0),
                "jobs": jobs,
            })
        }
        Err(e) => serde_json::json!({ "error": format!("{e:#}") }),
    }
}

/// One request line -> one response line (trailing newline). Never panics;
/// a malformed request is answered, not dropped (hermes shape).
pub fn handle_request_line(home: &Path, raw: &[u8], now: i64) -> Vec<u8> {
    let parsed = serde_json::from_slice::<serde_json::Value>(raw);
    let request_id = parsed.as_ref().ok().and_then(|v| v.get("id").cloned());
    let mut response = match parsed {
        Ok(req) if req.is_object() => match req.get("verb").and_then(|v| v.as_str()) {
            Some("identify") => serde_json::json!({
                "ok": true, "protocol": PROTOCOL, "result": identify_payload(home),
            }),
            Some("status") => serde_json::json!({
                "ok": true, "protocol": PROTOCOL, "result": status_payload(home, now),
            }),
            other => serde_json::json!({
                "ok": false, "protocol": PROTOCOL,
                "error": format!("unknown verb: {other:?}"),
                "supported_verbs": SUPPORTED_VERBS,
            }),
        },
        Ok(_) => serde_json::json!({
            "ok": false, "protocol": PROTOCOL, "error": "request must be a JSON object",
        }),
        Err(e) => serde_json::json!({
            "ok": false, "protocol": PROTOCOL, "error": format!("bad JSON: {e}"),
        }),
    };
    if let Some(id) = request_id
        && let Some(map) = response.as_object_mut()
    {
        map.insert("id".to_string(), id);
    }
    let mut out = serde_json::to_vec(&response).unwrap_or_default();
    out.push(b'\n');
    out
}

/// Serve the control socket until `stop` flips. A bind failure is the
/// caller's to judge — non-fatal by design: cron ticking must not depend on
/// this socket, and consumers fall back to the pid file.
///
/// Unix-only: Windows has no unix-domain sockets in this path, so the
/// daemon runs socketless there (pid file + state file still work).
#[cfg(unix)]
pub async fn serve(
    home: PathBuf,
    mut stop: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let path = sock_path(&home);
    if path.as_os_str().len() > MAX_SOCK_PATH {
        anyhow::bail!(
            "control socket path is {} bytes (max {MAX_SOCK_PATH}): {}",
            path.as_os_str().len(),
            path.display()
        );
    }
    // We hold the pid claim, so any file at this path is stale or ours.
    let _ = std::fs::remove_file(&path);
    // umask is process-wide: 0177 also removes OWNER directory traversal
    // from mkdir on unrelated threads (cron status then gets EACCES). 0077
    // keeps owner access and denies all group/other access even before chmod.
    let previous = unsafe { libc::umask(0o077) };
    let listener = tokio::net::UnixListener::bind(&path);
    unsafe { libc::umask(previous) };
    let listener = listener?;
    let _ = std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600));
    log::info!("gateway: control socket listening at {}", path.display());
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue }; // EMFILE etc: keep serving
                tokio::spawn(serve_connection(home.clone(), stream));
            }
        }
    }
    let _ = std::fs::remove_file(&path);
    log::info!("gateway: control socket closed");
    Ok(())
}

#[cfg(unix)]
async fn serve_connection(home: PathBuf, stream: tokio::net::UnixStream) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let (rd, mut wr) = stream.into_split();
    let mut rd = tokio::io::BufReader::new(rd).take(MAX_REQUEST_BYTES as u64 + 1);
    let mut raw = Vec::new();
    let read = tokio::time::timeout(
        IO_TIMEOUT,
        tokio::io::AsyncBufReadExt::read_until(&mut rd, b'\n', &mut raw),
    )
    .await;
    if let Ok(Ok(n)) = read
        && n > 0
        && n <= MAX_REQUEST_BYTES
    {
        let response = handle_request_line(&home, raw.trim_ascii_end(), crate::cron::now_secs());
        let _ = wr.write_all(&response).await;
    }
    let _ = wr.shutdown().await;
}

/// Ask the gateway serving `home` a control verb. `None` on any failure — no
/// socket, stale socket, timeout, malformed answer, `ok: false` — so callers
/// fall back to the pid file. Never raises.
pub fn query(home: &Path, verb: &str) -> Option<serde_json::Value> {
    query_within(home, verb, CLIENT_TIMEOUT)
}

pub fn query_within(home: &Path, verb: &str, timeout: Duration) -> Option<serde_json::Value> {
    #[cfg(not(unix))]
    {
        let _ = (home, verb, timeout);
        None
    }
    #[cfg(unix)]
    {
        unix_query_within(home, verb, timeout)
    }
}

#[cfg(unix)]
fn unix_query_within(home: &Path, verb: &str, timeout: Duration) -> Option<serde_json::Value> {
    let path = sock_path(home);
    if !path.exists() {
        return None;
    }
    let mut sock = std::os::unix::net::UnixStream::connect(&path).ok()?;
    sock.set_read_timeout(Some(timeout)).ok()?;
    sock.set_write_timeout(Some(timeout)).ok()?;
    let request = serde_json::json!({"verb": verb, "id": 1, "protocol": PROTOCOL});
    sock.write_all(request.to_string().as_bytes()).ok()?;
    sock.write_all(b"\n").ok()?;
    let mut line = String::new();
    std::io::BufReader::new(sock).read_line(&mut line).ok()?;
    let answer: serde_json::Value = serde_json::from_str(&line).ok()?;
    if answer.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        answer.get("result").cloned()
    } else {
        None
    }
}

#[path = "socket_tests.rs"]
#[cfg(test)]
mod tests;
