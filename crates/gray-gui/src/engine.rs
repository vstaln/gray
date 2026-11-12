//! Persistent `gray --stdio` client: one long-lived engine process per IDE
//! thread, spoken over newline-delimited JSON.
//!
//! Wire shape is documented in `crates/gray/docs/stdio-protocol.md`: requests
//! go in as `{"type": "...", "id": N}`, every emitted row carries the protocol
//! envelope (`seq`, `session_id`, `req`, `turn_id`). `Engine` is deliberately
//! thin — it owns the child process, a stdin writer and a reader thread that
//! hands every decoded row to the app over `rx`.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{Value, json};

/// A decoded stdout row, or the engine process exiting.
pub enum EngineMsg {
    Row(Value),
    /// Process ended; carries the tail of stderr for error reporting.
    Died(String),
}

pub struct Engine {
    pub rx: Receiver<EngineMsg>,
    stdin: Arc<Mutex<ChildStdin>>,
    child: Arc<Mutex<Child>>,
    dead: Arc<AtomicBool>,
    next_req: AtomicU64,
}

impl Engine {
    /// Spawn `gray --stdio` in `cwd`. `provider`/`model` mirror the CLI flags;
    /// `effort` rides the env var the CLI already honors. The caller refines
    /// all three afterwards via `set_model` (effort is not a CLI flag and a
    /// `resume` may adopt the session's model).
    pub fn spawn(cwd: &Path, provider: &str, model: &str, effort: &str) -> Result<Self, String> {
        let bin = crate::provider::gray_bin();
        let mut cmd = Command::new(bin);
        cmd.arg("--stdio");
        if !provider.is_empty() && provider != "gray" {
            cmd.arg("--provider").arg(provider);
        }
        if !model.is_empty() && model != "default" {
            cmd.arg("--model").arg(model);
        }
        if !effort.is_empty() {
            cmd.env("GRAY_THINKING_EFFORT", effort);
        }
        cmd.current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to start gray --stdio: {e}"))?;
        let stdin = child.stdin.take().ok_or("gray --stdio: no stdin")?;
        let stdout = child.stdout.take().ok_or("gray --stdio: no stdout")?;
        let dead = Arc::new(AtomicBool::new(false));
        let stderr_tail = Arc::new(Mutex::new(String::new()));
        if let Some(err) = child.stderr.take() {
            let tail = stderr_tail.clone();
            thread::spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    if let Ok(mut b) = tail.lock() {
                        b.push_str(&line);
                        b.push('\n');
                        let keep = 4_000;
                        if b.len() > keep {
                            let cut = b.len() - keep;
                            b.drain(..cut);
                        }
                    }
                }
            });
        }
        let (tx, rx) = channel();
        let d = dead.clone();
        thread::spawn(move || pump_stdout(stdout, tx, d, stderr_tail));
        Ok(Self {
            rx,
            stdin: Arc::new(Mutex::new(stdin)),
            child: Arc::new(Mutex::new(child)),
            dead,
            next_req: AtomicU64::new(1),
        })
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::SeqCst)
    }

    /// Write one request. Returns the request id assigned (an existing `"id"`
    /// in `v` is kept verbatim).
    pub fn request(&self, v: &Value) -> Result<u64, String> {
        let id = self.next_req.fetch_add(1, Ordering::SeqCst);
        let mut row = v.clone();
        if let Some(o) = row.as_object_mut() {
            o.entry(String::from("id")).or_insert_with(|| json!(id));
        }
        self.send_line(&row)?;
        Ok(id)
    }

    fn send_line(&self, v: &Value) -> Result<(), String> {
        let line = v.to_string();
        let mut w = self
            .stdin
            .lock()
            .map_err(|_| "engine stdin lock poisoned".to_string())?;
        writeln!(w, "{line}")
            .and_then(|_| w.flush())
            .map_err(|e| format!("gray --stdio write failed: {e}"))
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.send_line(&json!({"type": "shutdown"}));
        if let Ok(mut g) = self.child.lock() {
            let _ = g.kill();
            let _ = g.wait();
        }
    }
}

fn pump_stdout(
    stdout: impl std::io::Read,
    tx: Sender<EngineMsg>,
    dead: Arc<AtomicBool>,
    stderr_tail: Arc<Mutex<String>>,
) {
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(t) else {
            continue;
        };
        if v["protocol"] != 2 {
            dead.store(true, Ordering::SeqCst);
            let _ = tx.send(EngineMsg::Died(format!(
                "gray speaks protocol {}, this build understands 2; update gray or the IDE",
                v["protocol"]
            )));
            return;
        }
        if tx.send(EngineMsg::Row(v)).is_err() {
            return;
        }
    }
    dead.store(true, Ordering::SeqCst);
    let tail = stderr_tail
        .lock()
        .map(|b| b.trim().to_string())
        .unwrap_or_default();
    let _ = tx.send(EngineMsg::Died(tail));
}
