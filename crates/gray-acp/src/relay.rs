use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;

const HEADER_GRACE: Duration = Duration::from_secs(20);
const KEEPALIVE: Duration = Duration::from_secs(15);
const PEER_POLL: Duration = Duration::from_millis(250);
const MAX_BODY: usize = 256 << 20;

pub const ADMISSION_CONSUMED: &str = "ACP_RELAY_ADMISSION_CONSUMED";

pub type TurnFn = Arc<dyn Fn(&[u8], &AtomicBool) -> Result<Vec<u8>, String> + Send + Sync>;

pub struct RelayIntent {
    pub model: String,
    pub turn: TurnFn,
}

pub type Intents = Arc<Mutex<HashMap<String, RelayIntent>>>;

fn client_gone(s: &std::net::TcpStream) -> bool {
    if s.set_nonblocking(true).is_err() {
        return false;
    }
    let gone = match s.peek(&mut [0u8; 1]) {
        Ok(n) => n == 0,
        Err(e) => e.kind() != std::io::ErrorKind::WouldBlock,
    };
    let _ = s.set_nonblocking(false);
    gone
}

enum Wait {
    Done(Result<Vec<u8>, String>),
    Timeout,
    Died,
    Gone,
}

fn wait_turn(
    rx: &mpsc::Receiver<Result<Vec<u8>, String>>,
    s: &std::net::TcpStream,
    cancel: &AtomicBool,
    span: Duration,
) -> Wait {
    let end = Instant::now() + span;
    loop {
        let step = end.saturating_duration_since(Instant::now()).min(PEER_POLL);
        match rx.recv_timeout(step) {
            Ok(r) => return Wait::Done(r),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Wait::Died,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if client_gone(s) {
                    cancel.store(true, Ordering::SeqCst);
                    return Wait::Gone;
                }
                if Instant::now() >= end {
                    return Wait::Timeout;
                }
            }
        }
    }
}

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
    let handle = std::thread::Builder::new()
        .name("acp-sub-relay".into())
        .spawn(move || serve(listener, intents, bearer, used))
        .map_err(|e| format!("relay thread: {e}"))?;
    Ok((port, handle))
}

fn serve(listener: TcpListener, intents: Intents, bearer: String, used: Arc<AtomicBool>) {
    for stream in listener.incoming() {
        let Ok(mut s) = stream else { continue };
        handle_conn(&mut s, &intents, &bearer, &used);
    }
}

fn write_resp(s: &mut std::net::TcpStream, code: u16, body: &[u8]) {
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        413 => "Payload Too Large",
        _ => "Internal Server Error",
    };
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(body);
    let _ = s.flush();
}

fn handle_conn(
    s: &mut std::net::TcpStream,
    intents: &Intents,
    bearer: &str,
    used: &Arc<AtomicBool>,
) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(330)));
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
        let Some((name, v)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            len = v.trim().parse().unwrap_or(0);
        } else if name.eq_ignore_ascii_case("authorization") {
            auth = v.trim().to_string();
        }
    }
    let header_end = head
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
        .unwrap_or(head.len());
    let mut body = head[header_end..].to_vec();
    while body.len() < len.min(MAX_BODY) {
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
    if len > MAX_BODY || body.len() < len {
        let body = json!({"type": "error", "error": {
            "type": "invalid_request_error",
            "message": format!("relay body incomplete: read {} of {len} bytes (limit {MAX_BODY})", body.len()),
        }});
        write_resp(
            s,
            if len > MAX_BODY { 413 } else { 400 },
            body.to_string().as_bytes(),
        );
        return;
    }
    if used.swap(true, Ordering::SeqCst) {
        let body = json!({"type": "error", "error": {
            "type": "invalid_request_error",
            "message": ADMISSION_CONSUMED,
        }});
        write_resp(s, 400, body.to_string().as_bytes());
        return;
    }
    let (tx, rx) = mpsc::channel::<Result<Vec<u8>, String>>();
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let intents = intents.clone();
        let bearer = bearer.to_string();
        let cancel = cancel.clone();
        let spawned = std::thread::Builder::new()
            .name("acp-sub-turn".into())
            .spawn(move || {
                let _ = tx.send(run_turn(&intents, &bearer, &body, &cancel));
            });
        if let Err(e) = spawned {
            let body = json!({"type": "error", "error": {
                "type": "server_error", "message": format!("turn worker unavailable: {e}"),
            }});
            write_resp(s, 500, body.to_string().as_bytes());
            return;
        }
    }
    match wait_turn(&rx, s, &cancel, HEADER_GRACE) {
        Wait::Gone => {}
        Wait::Done(Ok(sse)) => {
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                sse.len()
            );
            let _ = s.write_all(header.as_bytes());
            let _ = s.write_all(&sse);
            let _ = s.flush();
        }
        Wait::Done(Err(detail)) => {
            let body = json!({"type": "error", "error": {
                "type": "server_error", "message": detail,
            }});
            write_resp(s, 500, body.to_string().as_bytes());
        }
        Wait::Died => {
            let body = json!({"type": "error", "error": {
                "type": "server_error", "message": "turn worker died",
            }});
            write_resp(s, 500, body.to_string().as_bytes());
        }
        Wait::Timeout => {
            if s
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
                )
                .and_then(|_| s.flush())
                .is_err()
            {
                cancel.store(true, Ordering::SeqCst);
                return;
            }
            loop {
                match wait_turn(&rx, s, &cancel, KEEPALIVE) {
                    Wait::Done(Ok(sse)) => {
                        let _ = s.write_all(&sse);
                        let _ = s.flush();
                        return;
                    }
                    Wait::Done(Err(detail)) => {
                        let failed = json!({"type": "response.failed",
                            "response": {"status": "failed",
                                "error": {"code": "server_error", "message": detail}}});
                        let _ =
                            s.write_all(format!("data: {failed}\n\ndata: [DONE]\n\n").as_bytes());
                        let _ = s.flush();
                        return;
                    }
                    Wait::Timeout => {
                        if s.write_all(b": keepalive\n\n")
                            .and_then(|_| s.flush())
                            .is_err()
                        {
                            cancel.store(true, Ordering::SeqCst);
                            return;
                        }
                    }
                    Wait::Gone | Wait::Died => return,
                }
            }
        }
    }
}

fn run_turn(
    intents: &Intents,
    bearer: &str,
    raw: &[u8],
    cancel: &Arc<AtomicBool>,
) -> Result<Vec<u8>, String> {
    let intent = intents
        .lock()
        .map(|mut m| m.remove(bearer))
        .map_err(|_| "intent lock".to_string())?
        .ok_or_else(|| "stale or replayed relay intent".to_string())?;
    (intent.turn)(raw, cancel)
}

pub fn mint_turn(
    intents: &Intents,
    model: &str,
    turn: TurnFn,
    token_prefix: &str,
) -> Result<(String, String), String> {
    let token = format!("{token_prefix}{}", crate::chat::rand_hex(16));
    intents
        .lock()
        .map_err(|_| "intent lock".to_string())?
        .insert(
            token.clone(),
            RelayIntent {
                model: model.to_string(),
                turn,
            },
        );
    let (port, handle) = start_turn_server(intents.clone(), token.clone())?;
    std::mem::forget(handle);
    Ok((
        format!("http://127.0.0.1:{port}/relay/{token}/responses"),
        token,
    ))
}
