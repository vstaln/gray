use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::chat::TOOL_PREFIX;

const PARK_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const MAX_BODY: usize = 64 << 20;

pub struct PendingCall {
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
    reply: SyncSender<Value>,
}

impl PendingCall {
    pub fn resolve(self, output: String) {
        let _ = self.reply.send(json!({
            "content": [{"type": "text", "text": output}],
            "isError": false,
        }));
    }
    pub fn fail(self, message: &str) {
        let _ = self.reply.send(json!({
            "content": [{"type": "text", "text": message}],
            "isError": true,
        }));
    }
}

struct Shared {
    tools: Vec<Value>,
    inbox_tx: mpsc::Sender<PendingCall>,
}

pub struct McpPark {
    pub url: String,
    shared: Arc<Mutex<Shared>>,
    inbox: Mutex<Receiver<PendingCall>>,
    _listener: std::thread::JoinHandle<()>,
}

impl McpPark {
    pub fn start() -> Result<Arc<McpPark>, String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("mcp bind: {e}"))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("mcp addr: {e}"))?
            .port();
        let (inbox_tx, inbox_rx) = mpsc::channel::<PendingCall>();
        let shared = Arc::new(Mutex::new(Shared {
            tools: Vec::new(),
            inbox_tx,
        }));
        let shared2 = shared.clone();
        let listener_thread = std::thread::Builder::new()
            .name("acp-sub-mcp".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut s) = stream else { continue };
                    let shared = shared2.clone();
                    std::thread::Builder::new()
                        .name("acp-sub-mcp-conn".into())
                        .spawn(move || handle_conn(&mut s, &shared))
                        .ok();
                }
            })
            .map_err(|e| format!("mcp thread: {e}"))?;
        Ok(Arc::new(McpPark {
            url: format!("http://127.0.0.1:{port}/mcp"),
            shared,
            inbox: Mutex::new(inbox_rx),
            _listener: listener_thread,
        }))
    }

    pub fn set_tools(&self, tools: Vec<Value>) {
        if let Ok(mut s) = self.shared.lock() {
            s.tools = tools;
        }
    }

    pub fn server_entry(&self, name: &str) -> Value {
        json!({"type": "http", "name": name, "url": self.url, "headers": []})
    }

    pub fn recv(&self, timeout: Duration) -> Option<PendingCall> {
        self.inbox.lock().ok()?.recv_timeout(timeout).ok()
    }

    pub fn drain(&self) -> Vec<PendingCall> {
        let mut out = Vec::new();
        if let Ok(rx) = self.inbox.lock() {
            while let Ok(c) = rx.try_recv() {
                out.push(c);
            }
        }
        out
    }
}

fn write_http(s: &mut std::net::TcpStream, code: u16, body: &[u8], content_type: &str) {
    let reason = match code {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Internal Server Error",
    };
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(body);
    let _ = s.flush();
}

fn handle_conn(s: &mut std::net::TcpStream, shared: &Arc<Mutex<Shared>>) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(120)));
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
    if request_line.starts_with("GET ") {
        write_http(s, 405, b"", "application/json");
        return;
    }
    if !request_line.starts_with("POST ") {
        write_http(s, 404, b"not found", "application/json");
        return;
    }
    let msg: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            write_http(s, 400, b"bad json", "application/json");
            return;
        }
    };
    if msg.get("id").is_none() {
        write_http(s, 202, b"", "application/json");
        return;
    }
    let id = msg["id"].clone();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let result = handle_rpc(method, &msg, shared);
    let out = match result {
        Ok(v) => json!({"jsonrpc": "2.0", "id": id, "result": v}),
        Err((code, message)) => json!({"jsonrpc": "2.0", "id": id,
            "error": {"code": code, "message": message}}),
    };
    write_http(
        s,
        200,
        serde_json::to_string(&out).unwrap_or_default().as_bytes(),
        "application/json",
    );
}

fn handle_rpc(
    method: &str,
    msg: &Value,
    shared: &Arc<Mutex<Shared>>,
) -> Result<Value, (i64, String)> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": msg.get("params")
                .and_then(|p| p.get("protocolVersion"))
                .and_then(Value::as_str)
                .unwrap_or("2025-03-26"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "gray", "version": "1.0.0"},
        })),
        "ping" => Ok(json!({})),
        "tools/list" => {
            let tools = shared.lock().map(|s| s.tools.clone()).unwrap_or_default();
            Ok(json!({"tools": tools}))
        }
        "tools/call" => {
            let params = msg.get("params").cloned().unwrap_or(json!({}));
            let full_name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let name = full_name.strip_prefix(TOOL_PREFIX).unwrap_or(full_name);
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            let known = {
                shared
                    .lock()
                    .map(|s| {
                        s.tools
                            .iter()
                            .any(|t| t.get("name").and_then(Value::as_str) == Some(name))
                    })
                    .unwrap_or(false)
            };
            if !known {
                return Err((-32602, format!("unknown tool {full_name}")));
            }
            let call_id = format!("call_{}", crate::chat::rand_hex(24));
            let (tx, rx) = mpsc::sync_channel::<Value>(1);
            let pending = PendingCall {
                call_id,
                name: name.to_string(),
                arguments,
                reply: tx,
            };
            let tx_inbox = shared
                .lock()
                .map(|s| s.inbox_tx.clone())
                .map_err(|_| (-32603, "park lock".to_string()))?;
            if tx_inbox.send(pending).is_err() {
                return Err((-32603, "park queue closed".to_string()));
            }
            match rx.recv_timeout(PARK_TIMEOUT) {
                Ok(v) => Ok(v),
                Err(_) => Err((-32603, "park timed out".to_string())),
            }
        }
        _ => Err((-32601, format!("no method {method}"))),
    }
}
