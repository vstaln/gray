use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

pub const PROTOCOL_VERSION: u64 = 1;

const STDERR_RING: usize = 40;

pub enum Wire {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Bad,
    Eof,
}

pub struct SpawnSpec {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
}

pub struct RpcFailure {
    pub code: Option<i64>,
    pub message: String,
}

impl RpcFailure {
    fn new(code: Option<i64>, message: String) -> Self {
        Self { code, message }
    }
}

pub struct AcpClient {
    pub child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    next_id: u64,
    pending: Arc<Mutex<HashMap<String, SyncSender<Value>>>>,
    inbox: Receiver<Wire>,
    pub stderr_ring: Arc<Mutex<VecDeque<String>>>,
}

impl AcpClient {
    pub fn spawn(spec: &SpawnSpec) -> std::io::Result<Self> {
        let mut cmd = Command::new(&spec.command);
        cmd.args(&spec.args)
            .current_dir(&spec.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(spec.env.iter().cloned());
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        let pending: Arc<Mutex<HashMap<String, SyncSender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = mpsc::channel::<Wire>();

        {
            let pending = pending.clone();
            let tx = tx.clone();
            std::thread::Builder::new()
                .name("acp-stdout".into())
                .spawn(move || {
                    for line in BufReader::new(stdout).lines() {
                        let line = match line {
                            Ok(l) => l,
                            Err(_) => break,
                        };
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        if std::env::var("GRAY_ACP_SUB_DEBUG").is_ok() {
                            eprintln!("[acp<-] {}", &trimmed[..trimmed.len().min(300)]);
                        }
                        let v: Value = match serde_json::from_str(trimmed) {
                            Ok(v) => v,
                            Err(_) => {
                                if tx.send(Wire::Bad).is_err() {
                                    return;
                                }
                                continue;
                            }
                        };
                        let has_method = v.get("method").and_then(Value::as_str).is_some();
                        if has_method {
                            let method = v["method"].as_str().unwrap().to_string();
                            let params = v.get("params").cloned().unwrap_or(Value::Null);
                            match v.get("id") {
                                Some(id) if !id.is_null() => {
                                    if tx
                                        .send(Wire::Request {
                                            id: id.clone(),
                                            method,
                                            params,
                                        })
                                        .is_err()
                                    {
                                        return;
                                    }
                                }
                                _ => {
                                    if tx.send(Wire::Notification { method, params }).is_err() {
                                        return;
                                    }
                                }
                            }
                        } else if let Some(id) = v.get("id") {
                            let key = id_key(id);
                            let sender = pending.lock().ok().and_then(|mut p| p.remove(&key));
                            if let Some(sender) = sender {
                                let _ = sender.send(v);
                            }
                        }
                    }
                    if let Ok(mut p) = pending.lock() {
                        for (_, s) in p.drain() {
                            let _ = s.send(
                                json!({"error":{"code":-32000,"message":"agent stdout EOF"}}),
                            );
                        }
                    }
                    let _ = tx.send(Wire::Eof);
                })
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        }

        let stderr_ring = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_RING + 1)));
        {
            let ring = stderr_ring.clone();
            std::thread::Builder::new()
                .name("acp-stderr".into())
                .spawn(move || {
                    for line in BufReader::new(stderr).lines() {
                        let line = match line {
                            Ok(l) => l,
                            Err(_) => break,
                        };
                        if let Ok(mut r) = ring.lock() {
                            if r.len() >= STDERR_RING {
                                r.pop_front();
                            }
                            r.push_back(line);
                        }
                    }
                })
                .ok();
        }

        Ok(Self {
            child,
            stdin: Arc::new(Mutex::new(stdin)),
            next_id: 1,
            pending,
            inbox: rx,
            stderr_ring,
        })
    }

    fn write_line(&self, v: &Value) -> std::io::Result<()> {
        if std::env::var("GRAY_ACP_SUB_DEBUG").is_ok() {
            let s = v.to_string();
            eprintln!("[acp->] {}", &s[..s.len().min(300)]);
        }
        let mut guard = self.stdin.lock().map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stdin lock poisoned")
        })?;
        let mut buf = serde_json::to_vec(v).map_err(std::io::Error::other)?;
        buf.push(b'\n');
        guard.write_all(&buf)?;
        guard.flush()
    }

    pub fn call(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.call_failure(method, params, timeout)
            .map_err(|f| f.message)
    }

    pub fn call_failure(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, RpcFailure> {
        let id = self.next_id;
        self.next_id += 1;
        let (tx, rx) = mpsc::sync_channel::<Value>(1);
        self.pending
            .lock()
            .map_err(|_| RpcFailure::new(None, "pending lock".to_string()))?
            .insert(id.to_string(), tx);
        let req = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        if let Err(e) = self.write_line(&req) {
            self.pending
                .lock()
                .ok()
                .and_then(|mut p| p.remove(&id.to_string()));
            return Err(RpcFailure::new(None, format!("write {method}: {e}")));
        }
        let msg = match rx.recv_timeout(timeout) {
            Ok(m) => m,
            Err(_) => {
                self.pending
                    .lock()
                    .ok()
                    .and_then(|mut p| p.remove(&id.to_string()));
                return Err(RpcFailure::new(None, format!("{method} timed out")));
            }
        };
        if let Some(err) = msg.get("error") {
            let code = err.get("code").and_then(Value::as_i64);
            let base = err.get("message").and_then(Value::as_str).unwrap_or("?");
            let detail = match err.get("data") {
                Some(d @ Value::Object(_)) => d
                    .get("details")
                    .or_else(|| d.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_default(),
                Some(Value::Array(items)) => items
                    .first()
                    .and_then(|i| i.get("message").and_then(Value::as_str))
                    .map(str::to_string)
                    .unwrap_or_else(|| serde_json::to_string(&items).unwrap_or_default()),
                _ => String::new(),
            };
            let detail: String = detail.chars().take(400).collect();
            let tail = self.stderr_tail();
            let stderr_note = if tail.is_empty() {
                String::new()
            } else {
                format!(" [stderr: {}]", tail.last().cloned().unwrap_or_default())
            };
            return Err(RpcFailure::new(
                code,
                format!(
                    "{method} failed: {}{}{}",
                    base,
                    if detail.is_empty() {
                        String::new()
                    } else {
                        format!(" — {detail}")
                    },
                    stderr_note,
                ),
            ));
        }
        Ok(msg.get("result").cloned().unwrap_or(Value::Null))
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.write_line(&json!({"jsonrpc":"2.0","method":method,"params":params}))
            .map_err(|e| format!("notify {method}: {e}"))
    }

    pub fn respond(&self, id: &Value, result: Value) -> Result<(), String> {
        self.write_line(&json!({"jsonrpc":"2.0","id":id,"result":result}))
            .map_err(|e| format!("respond: {e}"))
    }

    pub fn respond_error(&self, id: &Value, code: i64, message: &str) -> Result<(), String> {
        self.write_line(&json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}}))
            .map_err(|e| format!("respond_error: {e}"))
    }

    pub fn drain(&self, out: &mut Vec<Wire>) {
        while let Ok(w) = self.inbox.try_recv() {
            out.push(w);
        }
    }

    pub fn recv(&self, timeout: Duration) -> Option<Wire> {
        self.inbox.recv_timeout(timeout).ok()
    }

    pub fn recv_turn(&self, timeout: Duration) -> Option<Wire> {
        self.inbox.recv_timeout(timeout).ok()
    }

    pub fn initialize(&mut self, client_info: (&str, &str), caps: Value) -> Result<Value, String> {
        self.call(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "clientCapabilities": caps,
                "clientInfo": {"name": client_info.0, "version": client_info.1},
            }),
            Duration::from_secs(60),
        )
    }

    pub fn authenticate(&mut self, method_id: &str) -> Result<Value, String> {
        self.call(
            "authenticate",
            json!({"methodId": method_id}),
            Duration::from_secs(120),
        )
    }

    pub fn session_new(&mut self, cwd: &str, mcp_servers: Value) -> Result<Value, String> {
        self.session_new_with_meta(cwd, mcp_servers, Value::Null)
    }

    pub fn session_new_with_meta(
        &mut self,
        cwd: &str,
        mcp_servers: Value,
        meta: Value,
    ) -> Result<Value, String> {
        let mut params = json!({"cwd": cwd, "mcpServers": mcp_servers});
        if !meta.is_null() {
            params["_meta"] = meta;
        }
        self.call("session/new", params, Duration::from_secs(60))
    }

    pub fn session_load(
        &mut self,
        session_id: &str,
        cwd: &str,
        mcp_servers: Value,
    ) -> Result<Value, String> {
        self.call(
            "session/load",
            json!({"sessionId": session_id, "cwd": cwd, "mcpServers": mcp_servers}),
            Duration::from_secs(60),
        )
    }

    pub fn prompt_begin(
        &mut self,
        session_id: &str,
        prompt: Value,
        meta: Value,
    ) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;
        let mut params = json!({"sessionId": session_id, "prompt": prompt});
        if !meta.is_null() {
            params["_meta"] = meta;
        }
        let req = json!({"jsonrpc":"2.0","id":id,"method":"session/prompt","params":params});
        self.write_line(&req)
            .map_err(|e| format!("session/prompt write: {e}"))?;
        Ok(id)
    }

    pub fn prompt_waiter(&mut self, id: u64) -> Receiver<Value> {
        let (tx, rx) = mpsc::sync_channel::<Value>(1);
        if let Ok(mut p) = self.pending.lock() {
            p.insert(id.to_string(), tx);
        }
        rx
    }

    pub fn prompt(
        &mut self,
        session_id: &str,
        prompt: Value,
        meta: Value,
    ) -> Result<(u64, Receiver<Value>), String> {
        let id = self.next_id;
        self.next_id += 1;
        let (tx, rx) = mpsc::sync_channel::<Value>(1);
        self.pending
            .lock()
            .map_err(|_| "pending lock".to_string())?
            .insert(id.to_string(), tx);
        let mut params = json!({"sessionId": session_id, "prompt": prompt});
        if !meta.is_null() {
            params["_meta"] = meta;
        }
        let req = json!({"jsonrpc":"2.0","id":id,"method":"session/prompt","params":params});
        if let Err(e) = self.write_line(&req) {
            self.pending
                .lock()
                .ok()
                .and_then(|mut p| p.remove(&id.to_string()));
            return Err(format!("session/prompt write: {e}"));
        }
        Ok((id, rx))
    }

    pub fn cancel(&self, session_id: &str) {
        let _ = self.notify("session/cancel", json!({"sessionId": session_id}));
    }

    pub fn set_model(
        &mut self,
        session_id: &str,
        model_id: &str,
        meta: Value,
    ) -> Result<(), String> {
        self.set_model_failure(session_id, model_id, meta)
            .map_err(|f| f.message)
    }

    pub fn set_model_failure(
        &mut self,
        session_id: &str,
        model_id: &str,
        meta: Value,
    ) -> Result<(), RpcFailure> {
        let mut params = json!({"sessionId": session_id, "modelId": model_id});
        if !meta.is_null() {
            params["_meta"] = meta;
        }
        self.call_failure("session/set_model", params, Duration::from_secs(15))?;
        Ok(())
    }

    pub fn set_config_option(
        &mut self,
        session_id: &str,
        config_id: &str,
        value: Value,
    ) -> Result<(), String> {
        self.call(
            "session/set_config_option",
            json!({"sessionId": session_id, "configId": config_id, "value": value}),
            Duration::from_secs(15),
        )?;
        Ok(())
    }

    pub fn set_mode(&mut self, session_id: &str, mode_id: &str) -> Result<(), String> {
        self.call(
            "session/set_mode",
            json!({"sessionId": session_id, "modeId": mode_id}),
            Duration::from_secs(15),
        )?;
        Ok(())
    }

    pub fn ext_call(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.call(method, params, timeout)
    }

    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr_ring
            .lock()
            .map(|r| r.iter().cloned().collect())
            .unwrap_or_default()
    }
}

fn id_key(id: &Value) -> String {
    match id {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

impl Drop for AcpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
