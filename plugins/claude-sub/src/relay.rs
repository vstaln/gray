//! Loopback admission relay: the single-request gate in front of `claude`.
//! Port of the Hermes DirectSDK `admission.py`
//! (NousResearch/hermes-plugin-claude-subscription-directsdk, MIT).
//!
//! Per chat turn the sidecar opens a loopback HTTP server and hands the
//! host a per-turn relay URL + bearer. The host POSTs its standard
//! Responses body there; the relay admits exactly ONE request (anything
//! past it gets 400 `ADMISSION_CONSUMED`), translates it to native frames,
//! spawns `claude` (which makes exactly one upstream request on its own),
//! folds the native transcript to Responses SSE and streams it back.
//!
//! Credentials are forwarded, never persisted: the bearer is a per-turn
//! token minted by the sidecar, never the user's OAuth token.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

/// What `provider/chat` parked for one turn: the relay fills the body in.
pub struct RelayIntent {
    pub model: String,
    pub effort: Option<String>,
}

pub type Intents = Arc<Mutex<HashMap<String, RelayIntent>>>;

/// Start the per-turn relay server. Returns the loopback port + handle.
/// The admitted POST does the whole turn synchronously; the server stays
/// open afterwards only to answer 400s to anything past the single
/// admission (native retries must see denial, not a hang).
pub fn start_turn_server(
    intents: Intents,
    bearer: String,
) -> Result<(u16, std::thread::JoinHandle<()>), String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("relay bind: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("relay addr: {e}"))?
        .port();
    let used = Arc::new(AtomicBool::new(false));
    let handle = std::thread::spawn(move || serve(listener, intents, bearer, used));
    Ok((port, handle))
}

fn serve(listener: TcpListener, intents: Intents, bearer: String, used: Arc<AtomicBool>) {
    for stream in listener.incoming() {
        let Ok(mut s) = stream else { continue };
        handle_conn(&mut s, &intents, &bearer, &used);
    }
}

fn handle_conn(
    s: &mut std::net::TcpStream,
    intents: &Intents,
    bearer: &str,
    used: &Arc<AtomicBool>,
) {
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(330)));
    let mut buf = vec![0u8; 65536];
    let mut head = Vec::new();
    loop {
        let Ok(n) = s.read(&mut buf) else { return };
        if n == 0 {
            return;
        }
        head.extend_from_slice(&buf[..n]);
        if head.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if head.len() > 1_048_576 {
            return;
        }
    }
    let head_str = String::from_utf8_lossy(&head).into_owned();
    let mut lines = head_str.lines();
    let request_line = lines.next().unwrap_or("").to_string();
    let mut len = 0usize;
    let mut auth = String::new();
    for line in lines {
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line
            .strip_prefix("Content-Length:")
            .or_else(|| line.strip_prefix("content-length:"))
        {
            len = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = line
            .strip_prefix("Authorization:")
            .or_else(|| line.strip_prefix("authorization:"))
        {
            auth = v.trim().to_string();
        }
    }
    let header_end = head
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
        .unwrap_or(head.len());
    let mut body = head[header_end..].to_vec();
    while body.len() < len.min(8_388_608) {
        let Ok(n) = s.read(&mut buf) else { return };
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
    }
    let ok_path = request_line.starts_with("POST /relay/")
        && request_line.contains("/responses")
        && request_line.contains(bearer);
    let ok_bearer = auth == format!("Bearer {bearer}");
    if !ok_path || !ok_bearer {
        write_resp(s, 404, b"not found");
        return;
    }
    if used.swap(true, Ordering::SeqCst) {
        let body = json!({"type": "error", "error": {
            "type": "invalid_request_error",
            "message": crate::chat::ADMISSION_CONSUMED,
        }});
        write_resp(s, 400, body.to_string().as_bytes());
        return;
    }
    // Admitted: the one request. Translate, spawn, fold, stream.
    match run_turn(intents, bearer, &body) {
        Ok(sse) => {
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                sse.len()
            );
            let _ = s.write_all(header.as_bytes());
            let _ = s.write_all(&sse);
            let _ = s.flush();
        }
        Err(detail) => {
            let body = json!({"type": "error", "error": {
                "type": "server_error", "message": detail,
            }});
            write_resp(s, 500, body.to_string().as_bytes());
        }
    }
}

fn run_turn(intents: &Intents, bearer: &str, raw: &[u8]) -> Result<Vec<u8>, String> {
    let intent = intents
        .lock()
        .map(|mut m| m.remove(bearer))
        .ok()
        .flatten()
        .ok_or_else(|| "relay intent expired".to_string())?;
    let body: Value =
        serde_json::from_slice(raw).map_err(|_| "relay body is not JSON".to_string())?;
    let turn = crate::chat::prepare_turn(&body, &intent.model)?;
    let tools: Vec<Value> = body
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let extra = crate::chat::extra_body(&turn.names, &tools);
    // History frames replay first (no-query each), the final frame queries:
    // native makes exactly one upstream request on its own.
    let lines = crate::chat::spawn_turn(
        &turn,
        &extra,
        &turn.system.clone(),
        intent.effort.as_deref(),
        std::time::Duration::from_secs(300),
    )?;
    let names: Vec<String> = tools
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_string))
        .collect();
    let say: Arc<dyn Fn(String) + Send + Sync> = Arc::new(|_| {});
    let (sse, _, _, _, _, _) = crate::chat::fold_lines(&lines, &names, &say)?;
    Ok(sse)
}

fn write_resp(s: &mut std::net::TcpStream, status: u16, body: &[u8]) {
    let reason = match status {
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = s.write_all(header.as_bytes());
    let _ = s.write_all(body);
    let _ = s.flush();
}
