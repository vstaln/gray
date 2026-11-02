//! End-to-end tests for `gray --stdio`: a stub OpenAI-compatible endpoint
//! stands in for the provider (the same "fake" the IDE e2e uses in spirit),
//! and the test drives the persistent NDJSON session over pipes.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

// ---------- stub provider ---------------------------------------------------

const CHAT_REPLY: &str = "fake-response";

fn sse_events(finish: bool) -> String {
    let mut out = String::new();
    for delta in ["fake", "-response"] {
        out.push_str(&format!(
            "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{delta}\"}}}}]}}\n\n"
        ));
    }
    if finish {
        out.push_str(
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2,\"total_tokens\":7}}\n\n",
        );
        out.push_str("data: [DONE]\n\n");
    }
    out
}

/// Read one HTTP request; returns the request path.
fn read_request(stream: &mut std::net::TcpStream) -> Option<String> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let headers_end = loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break pos + 4;
                }
                if buf.len() > 1_000_000 {
                    return None;
                }
            }
        }
    };
    let head = String::from_utf8_lossy(&buf[..headers_end]).to_string();
    let clen = head
        .split("content-length:")
        .nth(1)
        .and_then(|s| s.split("\r\n").next())
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut have = buf.len() - headers_end;
    let mut chunk = [0u8; 8192];
    while have < clen {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => have += n,
        }
    }
    head.split_whitespace().nth(1).map(str::to_string)
}

/// `stall_ms` > 0: send one delta, stall, then finish (or die on disconnect).
fn serve(listener: TcpListener, stall_ms: u64) {
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut stream) = conn else { return };
            thread::spawn(move || {
                let Some(path) = read_request(&mut stream) else {
                    return;
                };
                if path.starts_with("/models") {
                    let body = "{\"data\":[{\"id\":\"test-model\",\"name\":\"test-model\",\"context_length\":8192}]}";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    return;
                }
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
                );
                if stall_ms > 0 {
                    let _ = stream.write_all(
                        b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"stall\"}}]}\n\n",
                    );
                    let _ = stream.flush();
                    for _ in 0..(stall_ms / 50) {
                        thread::sleep(Duration::from_millis(50));
                        if stream.write_all(b": ka\n\n").is_err() {
                            return; // client hung up (interrupted turn)
                        }
                        let _ = stream.flush();
                    }
                }
                let _ = stream.write_all(sse_events(true).as_bytes());
                let _ = stream.flush();
            });
        }
    });
}

fn stub_server(stall_ms: u64) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub");
    let port = listener.local_addr().unwrap().port();
    serve(listener, stall_ms);
    format!("http://127.0.0.1:{port}")
}

// ---------- stdio client ----------------------------------------------------

struct Proc {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    stderr: Receiver<String>,
}

impl Proc {
    fn send(&mut self, v: &Value) {
        writeln!(self.stdin, "{v}").expect("write request");
        self.stdin.flush().unwrap();
    }

    fn next_line(&self, timeout: Duration) -> Option<Value> {
        let line = self.lines.recv_timeout(timeout).ok()?;
        serde_json::from_str(&line).ok()
    }

    /// Collect events until `pred` matches (returns all collected rows).
    fn wait_for(&self, pred: impl Fn(&Value) -> bool, timeout: Duration) -> Vec<Value> {
        let deadline = Instant::now() + timeout;
        let mut out = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                eprintln!(
                    "[test] wait_for hit {timeout:?} deadline, {} rows",
                    out.len()
                );
                return out;
            }
            match self.next_line(left) {
                Some(v) => {
                    let done = pred(&v);
                    out.push(v);
                    if done {
                        return out;
                    }
                }
                None => return out,
            }
        }
    }

    fn drain_stderr(&self) -> String {
        let mut out = String::new();
        while let Ok(l) = self.stderr.try_recv() {
            out.push_str(&l);
            out.push('\n');
        }
        out
    }

    fn result(&self, id: i64, timeout: Duration) -> Option<Value> {
        self.wait_for(
            |v| {
                v.get("type").and_then(|t| t.as_str()) == Some("result")
                    && v.get("id").and_then(|i| i.as_i64()) == Some(id)
                    && !v.get("queued").and_then(|q| q.as_bool()).unwrap_or(false)
            },
            timeout,
        )
        .into_iter()
        .last()
    }
}

fn spawn_stdio(base_url: &str) -> Proc {
    // `{:?}` of an Instant contains `:` and spaces, which Windows paths
    // reject; a process id plus a counter is unique and portable.
    static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let tmp: PathBuf = std::env::temp_dir().join(format!(
        "gray-stdio-test-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gray"));
    cmd.env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default());
    // Windows can't open sockets without these (Winsock reads SystemRoot);
    // carry them over so the child can reach the stub server.
    for key in ["SystemRoot", "SYSTEMROOT", "windir", "TEMP", "TMP"] {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    cmd.env("HOME", &tmp)
        .env("GRAY_HOME", &tmp)
        .env("GRAY_USER_HOME", &tmp)
        .args([
            "--bare",
            "--stdio",
            "--model",
            "test-model",
            "--base-url",
            base_url,
            "--api-key",
            "test-key",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn gray --stdio");
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let stdin = child.stdin.take().unwrap();
    let (ltx, lrx) = channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(l) = line else { return };
            if ltx.send(l).is_err() {
                return;
            }
        }
    });
    let (etx, erx) = channel();
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(l) = line else { return };
            if etx.send(l).is_err() {
                return;
            }
        }
    });
    Proc {
        child,
        stdin,
        lines: lrx,
        stderr: erx,
    }
}

fn wait_deadline(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(Some(s)) = child.try_wait() {
            return Some(s);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn hello(p: &Proc) -> Value {
    let rows = p.wait_for(
        |v| v.get("type").and_then(|t| t.as_str()) == Some("hello"),
        Duration::from_secs(30),
    );
    rows.into_iter()
        .last()
        .unwrap_or_else(|| panic!("no hello row from gray --stdio"))
}

const T: Duration = Duration::from_secs(60);

#[test]
fn stdio_roundtrip() {
    let url = stub_server(0);
    let mut p = spawn_stdio(&url);
    let hello = hello(&p);
    assert_eq!(hello["model"], json!("test-model"), "hello {hello}");

    // prompt → turn events + result
    p.send(&json!({"type": "prompt", "id": 1, "text": "say hi"}));
    let rows = p.wait_for(
        |v| {
            v.get("type").and_then(|t| t.as_str()) == Some("result")
                && v.get("id").and_then(|i| i.as_i64()) == Some(1)
        },
        T,
    );
    assert!(
        rows.iter().any(|v| v["type"] == "turn_start"),
        "no turn_start: {rows:?}"
    );
    let text: String = rows
        .iter()
        .filter(|v| v["type"] == "text")
        .filter_map(|v| v["delta"].as_str().map(str::to_string))
        .collect();
    assert_eq!(text, CHAT_REPLY, "text deltas: {rows:?}");
    assert!(
        rows.iter()
            .any(|v| v["type"] == "turn_end" && v["ok"] == true),
        "no turn_end ok: {rows:?}"
    );
    assert!(
        rows.last()
            .is_some_and(|v| v["type"] == "result" && v["ok"] == true),
        "prompt result missing: {rows:?}"
    );

    // queued steer runs as its own turn once idle
    p.send(&json!({"type": "steer", "id": 2, "text": "again"}));
    let rows = p.wait_for(
        |v| {
            v.get("type").and_then(|t| t.as_str()) == Some("turn_end")
                && v.get("req").and_then(|r| r.as_i64()) == Some(2)
        },
        T,
    );
    assert!(
        rows.iter().any(|v| {
            v["type"] == "result" && v["id"].as_i64() == Some(2) && v["queued"] == true
        }),
        "steer was not queued-acked: {rows:?}"
    );
    assert!(rows.iter().any(|v| v["type"] == "turn_start"));

    // slash command via the real dispatcher: /help emits `say` + result
    p.send(&json!({"type": "command", "id": 3, "line": "/help"}));
    let rows = p.wait_for(
        |v| {
            v.get("type").and_then(|t| t.as_str()) == Some("result")
                && v.get("id").and_then(|i| i.as_i64()) == Some(3)
        },
        T,
    );
    assert!(
        rows.iter()
            .any(|v| v["type"] == "say" && v["text"].as_str().is_some_and(|t| t.contains("help"))),
        "no help output: {rows:?}"
    );

    // status
    p.send(&json!({"type": "status", "id": 4}));
    let r = p.result(4, T).expect("status result");
    assert_eq!(r["status"]["model"], json!("test-model"), "{r}");
    assert_eq!(r["status"]["bare"], json!(true));
    assert!(r["status"]["session_id"].is_string(), "{r}");

    // list_commands
    p.send(&json!({"type": "list_commands", "id": 5}));
    let r = p.result(5, T).expect("list_commands result");
    let names: Vec<&str> = r["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    for want in ["help", "models", "compact", "sessions", "quit"] {
        assert!(names.contains(&want), "missing /{want} in {names:?}");
    }

    // list_models → catalog shape (same JSON as --dump-models)
    p.send(&json!({"type": "list_models", "id": 6}));
    let r = p.result(6, T).expect("list_models result");
    assert!(r["catalog"]["providers"].is_array(), "{r}");
    assert!(r["catalog"].get("active").is_some(), "{r}");

    // set_model
    p.send(&json!({"type": "set_model", "id": 7, "model": "test-model", "effort": "low"}));
    let r = p.result(7, T).expect("set_model result");
    assert_eq!(r["ok"], json!(true), "{r}");

    // list_sessions
    p.send(&json!({"type": "list_sessions", "id": 8, "all": true}));
    let r = p.result(8, T).expect("list_sessions result");
    assert!(r["sessions"].is_array(), "{r}");

    // resume with a bogus id → result, not a hang
    p.send(&json!({"type": "resume", "id": 9, "session": "no-such-session"}));
    let _r = p.result(9, T).expect("resume result");

    // clean shutdown
    p.send(&json!({"type": "shutdown", "id": 10}));
    let _ = p.result(10, Duration::from_secs(10));
    drop(p.stdin);
    let status =
        wait_deadline(&mut p.child, Duration::from_secs(20)).expect("gray --stdio did not exit");
    assert!(status.success(), "exit status {status:?}");
}

#[test]
fn stdio_interrupt() {
    let url = stub_server(4_000);
    let mut p = spawn_stdio(&url);
    hello(&p);

    p.send(&json!({"type": "prompt", "id": 1, "text": "hang on"}));
    let rows = p.wait_for(|v| v["type"] == "turn_start" || v["type"] == "text", T);
    assert!(!rows.is_empty(), "turn never started");

    // mid-turn queueing gets an immediate ack, runs later
    p.send(&json!({"type": "prompt", "id": 2, "text": "queued follow-up"}));
    let queued = p.wait_for(
        |v| v["type"] == "result" && v["id"].as_i64() == Some(2) && v["queued"] == true,
        T,
    );
    assert!(
        queued
            .iter()
            .any(|v| v["queued"] == true && v["id"].as_i64() == Some(2)),
        "mid-turn prompt not queued: {queued:?}"
    );

    p.send(&json!({"type": "interrupt", "id": 3}));
    // Collect everything through the interrupted turn_end. The queued
    // prompt may resolve two ways — consumed mid-turn as a steer (a
    // `steered` event + `result {steered:true}`) or run as turn 2 after —
    // and its steered result can land anywhere in this window.
    let rows = p.wait_for(
        |v| v["type"] == "turn_end" && v["req"].as_i64() == Some(1) && v["interrupted"] == true,
        T,
    );
    assert!(
        rows.iter().any(|v| v["type"] == "result"
            && v["id"].as_i64() == Some(3)
            && v["interrupted"] == true),
        "no interrupt ack: {rows:?}"
    );
    assert!(
        rows.last().is_some_and(|v| {
            v["type"] == "turn_end" && v["req"].as_i64() == Some(1) && v["interrupted"] == true
        }),
        "turn 1 not interrupted: {rows:?}"
    );
    let steered = rows.iter().any(|v| {
        (v["type"] == "result" && v["id"].as_i64() == Some(2) && v["steered"] == true)
            || v["type"] == "steered"
    });
    if !steered {
        let resolved = p.wait_for(
            |v| v["type"] == "turn_end" && v["req"].as_i64() == Some(2),
            T,
        );
        assert!(
            resolved
                .last()
                .is_some_and(|v| v["type"] == "turn_end" && v["req"].as_i64() == Some(2)),
            "queued prompt never ran as its own turn: {resolved:?}\nstderr: {}",
            p.drain_stderr()
        );
    }

    // process still serves requests afterwards
    p.send(&json!({"type": "status", "id": 4}));
    let r = p.result(4, T).expect("status after interrupt");
    assert_eq!(r["ok"], json!(true), "{r}");

    p.send(&json!({"type": "shutdown", "id": 5}));
    let _ = p.result(5, Duration::from_secs(10));
    drop(p.stdin);
    let _ = wait_deadline(&mut p.child, Duration::from_secs(20));
}
