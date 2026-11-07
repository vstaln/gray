use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub enum ProviderEvent {
    Text(String),
    Replace(String),
    Activity {
        kind: String,
        label: String,
        detail: String,
    },
    Session(String),
    Approval {
        label: String,
    },
    Done,
    Error(String),
}

/// A model row as the gray CLI's `/models` picker knows it (from
/// `gray --dump-models`), plus the static rows for the external CLI providers.
#[derive(Clone, Debug, Default)]
pub struct ModelEntry {
    /// Owning provider id — filled in when rows are gathered for the picker.
    pub provider: String,
    pub id: String,
    /// Display label; empty means show the id.
    pub name: String,
    /// Reasoning-effort levels the model supports (CLI order).
    pub efforts: Vec<String>,
    /// Currently selected on this provider in the gray CLI's saved config.
    pub current: bool,
}

#[derive(Clone, Debug)]
pub struct ProviderEntry {
    pub id: String,
    pub label: String,
    /// This provider is the gray CLI's active connection.
    pub active: bool,
    pub models: Vec<ModelEntry>,
}

fn static_provider(id: &str, label: &str, models: &[(&str, &[&str])]) -> ProviderEntry {
    ProviderEntry {
        id: id.into(),
        label: label.into(),
        active: false,
        models: models
            .iter()
            .map(|(m, efforts)| ModelEntry {
                provider: id.to_string(),
                id: (*m).to_string(),
                name: String::new(),
                efforts: efforts.iter().map(|e| (*e).to_string()).collect(),
                current: false,
            })
            .collect(),
    }
}

/// The external agent CLIs gray-tui drives directly: (id, label, binary env
/// override, binary name). Only those whose binary resolves are offered, and
/// their models come from the CLI itself (see `load_cli_providers`).
const CLI_PROVIDERS: &[(&str, &str, &str, &str)] = &[
    ("claude", "Claude", "GRAY_TUI_CLAUDE_BIN", "claude"),
    ("codex", "Codex", "GRAY_TUI_CODEX_BIN", "codex"),
];

/// How long a CLI gets to answer its model listing before we give up.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);

/// Every installed external CLI with the models it reports, discovered in
/// parallel. A CLI that is installed but won't list its models gets a single
/// "default" row (the CLI picks its own model); a missing one is left out.
pub fn load_cli_providers() -> Vec<ProviderEntry> {
    let handles: Vec<_> = CLI_PROVIDERS
        .iter()
        .map(|&(id, label, env, name)| {
            thread::spawn(move || {
                let path = resolve_bin(&bin(env, name))?;
                let models = match id {
                    "claude" => discover_claude(&path),
                    "codex" => discover_codex(&path),
                    _ => None,
                }
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| {
                    vec![ModelEntry {
                        provider: id.into(),
                        id: "default".into(),
                        current: true,
                        ..Default::default()
                    }]
                });
                Some(ProviderEntry {
                    id: id.into(),
                    label: label.into(),
                    active: false,
                    models,
                })
            })
        })
        .collect();
    handles
        .into_iter()
        .filter_map(|h| h.join().ok().flatten())
        .collect()
}

/// `name` as an executable path: as given when it has a slash, else the
/// first match on PATH.
fn resolve_bin(name: &str) -> Option<PathBuf> {
    let is_exe = |p: &Path| {
        use std::os::unix::fs::PermissionsExt;
        p.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if name.contains('/') {
        let p = PathBuf::from(name);
        return is_exe(&p).then_some(p);
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(name))
        .find(|p| is_exe(p))
}

/// Spawn `cmd` with piped stdio and hand each stdout line to `on_line`
/// (which may write to stdin) until it returns Some or the timeout passes.
fn probe<T>(
    mut cmd: Command,
    first: &[String],
    mut on_line: impl FnMut(&serde_json::Value, &mut dyn std::io::Write) -> Option<T>,
) -> Option<T> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;
    let (tx, rx) = channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut out = None;
    if first.iter().all(|l| writeln!(stdin, "{l}").is_ok()) {
        let deadline = std::time::Instant::now() + DISCOVERY_TIMEOUT;
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            let Ok(line) = rx.recv_timeout(left) else {
                break;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if let Some(r) = on_line(&v, &mut stdin) {
                out = Some(r);
                break;
            }
        }
    }
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    out
}

fn str_list(v: Option<&serde_json::Value>, key: Option<&str>) -> Vec<String> {
    v.and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| match key {
                    Some(k) => x.get(k).and_then(|s| s.as_str()),
                    None => x.as_str(),
                })
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Claude Code's own model list: the stream-json `list_models` control
/// request, sent once `initialize` is acknowledged.
fn discover_claude(path: &Path) -> Option<Vec<ModelEntry>> {
    let mut cmd = Command::new(path);
    cmd.args(["-p", "--output-format", "stream-json", "--verbose"])
        .args(["--input-format", "stream-json", "--no-session-persistence"])
        .args([
            "--strict-mcp-config",
            "--mcp-config",
            r#"{"mcpServers":{}}"#,
        ])
        .args(["--settings", r#"{"disableAllHooks":true}"#])
        .current_dir(std::env::temp_dir());
    let req = |id: &str, sub: &str| {
        serde_json::json!({"type": "control_request", "request_id": id, "request": {"subtype": sub}})
            .to_string()
    };
    let list = req("gray-tui-list-models", "list_models");
    let mut asked = false;
    probe(cmd, &[req("gray-tui-init", "initialize")], |v, stdin| {
        let resp = v
            .get("response")
            .filter(|_| v["type"] == "control_response");
        let rid = resp.and_then(|r| r["request_id"].as_str()).unwrap_or("");
        if rid == "gray-tui-init" && !asked {
            asked = true;
            let _ = writeln!(stdin, "{list}");
            return None;
        }
        if rid != "gray-tui-list-models" {
            return None;
        }
        let rows = resp?.get("response")?.get("models")?.as_array()?;
        Some(
            rows.iter()
                .filter_map(|m| {
                    Some(ModelEntry {
                        provider: "claude".into(),
                        id: m.get("value")?.as_str()?.to_string(),
                        name: m["displayName"].as_str().unwrap_or("").to_string(),
                        efforts: str_list(m.get("supportedEffortLevels"), None),
                        current: false,
                    })
                })
                .collect(),
        )
    })
}

/// Codex's own model list: app-server JSON-RPC `model/list`, paged.
fn discover_codex(path: &Path) -> Option<Vec<ModelEntry>> {
    let mut cmd = Command::new(path);
    cmd.arg("app-server").current_dir(std::env::temp_dir());
    let init = serde_json::json!({
        "id": 1,
        "method": "initialize",
        "params": {
            "clientInfo": {"name": "gray-tui", "title": "Gray TUI", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {"experimentalApi": true},
        },
    });
    let list = |id: u64, cursor: Option<&str>| {
        let params = match cursor {
            Some(c) => serde_json::json!({ "cursor": c }),
            None => serde_json::json!({}),
        };
        serde_json::json!({"id": id, "method": "model/list", "params": params}).to_string()
    };
    let mut models = Vec::new();
    let mut next_id = 2;
    probe(cmd, &[init.to_string()], |v, stdin| {
        // Server-initiated requests: answer empty so it doesn't stall.
        if v.get("method").is_some() {
            if let Some(id) = v.get("id") {
                let _ = writeln!(stdin, "{}", serde_json::json!({"id": id, "result": {}}));
            }
            return None;
        }
        let id = v.get("id")?.as_u64()?;
        if id == 1 {
            let _ = writeln!(stdin, r#"{{"method":"initialized"}}"#);
            let _ = writeln!(stdin, "{}", list(next_id, None));
            return None;
        }
        if id != next_id {
            return None;
        }
        if v.get("error").is_some() {
            return Some(std::mem::take(&mut models));
        }
        let res = v.get("result")?;
        for m in res["data"].as_array().into_iter().flatten() {
            if m["hidden"].as_bool() == Some(true) {
                continue;
            }
            let Some(mid) = m["model"].as_str().or(m["id"].as_str()) else {
                continue;
            };
            models.push(ModelEntry {
                provider: "codex".into(),
                id: mid.to_string(),
                name: m["displayName"].as_str().unwrap_or("").to_string(),
                efforts: str_list(m.get("supportedReasoningEfforts"), Some("reasoningEffort")),
                current: m["isDefault"].as_bool() == Some(true),
            });
        }
        match res["nextCursor"].as_str() {
            Some(c) => {
                next_id += 1;
                let _ = writeln!(stdin, "{}", list(next_id, Some(c)));
                None
            }
            None => Some(std::mem::take(&mut models)),
        }
    })
}

pub fn fake_provider() -> ProviderEntry {
    static_provider("fake", "Fake", &[("fake-1", &[])])
}

pub fn fallback_gray_provider() -> ProviderEntry {
    static_provider("gray", "Gray", &[("default", &[])])
}

/// The fake provider is test scaffolding: only offer it when the harness opts
/// in via env, never to users of the picker.
pub fn fake_enabled() -> bool {
    std::env::var_os("GRAY_TUI_FAKE").is_some()
        || std::env::var_os("GRAY_TUI_FAKE_DELAY_MS").is_some()
}

/// Providers that run through the gray CLI (`gray -p`) rather than an
/// external agent binary.
pub fn is_gray_provider(id: &str) -> bool {
    id != "fake" && !CLI_PROVIDERS.iter().any(|c| c.0 == id)
}

pub fn static_label(id: &str) -> String {
    match id {
        "gray" => "Gray".to_string(),
        "fake" => "Fake".to_string(),
        other => CLI_PROVIDERS
            .iter()
            .find(|c| c.0 == other)
            .map(|c| c.1.to_string())
            .unwrap_or_else(|| other.to_string()),
    }
}

pub const MODES: &[(&str, &str)] = &[
    ("full", "Full access"),
    ("edits", "Approve edits"),
    ("readonly", "Read only"),
];

/// Reasoning levels the effort pill row accepts — mirrors the CLI's
/// THINKING_LEVELS.
pub const EFFORT_LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

pub fn mode_label(id: &str) -> &'static str {
    MODES
        .iter()
        .find(|m| m.0 == id)
        .map(|m| m.1)
        .unwrap_or("Full access")
}

/// `gray --dump-models`: the same providers and model rows the CLI's /models
/// rail shows, in the same order. None when the installed gray is too old or
/// fails — callers fall back to the static gray entry.
pub fn load_model_catalog() -> Option<Vec<ProviderEntry>> {
    let out = Command::new(bin("GRAY_TUI_GRAY_BIN", "gray"))
        .arg("--dump-models")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    parse_model_catalog(&v)
}

/// The provider/model catalog shape shared by `gray --dump-models` and the
/// stdio `list_models` result's `catalog` field.
pub fn parse_model_catalog(v: &serde_json::Value) -> Option<Vec<ProviderEntry>> {
    let list = v.get("providers")?.as_array()?;
    let mut out_v = Vec::new();
    for p in list {
        let id = p.get("id").and_then(|x| x.as_str()).unwrap_or("");
        if id.is_empty() {
            continue;
        }
        let label = p
            .get("name")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| static_label(id));
        let models = p
            .get("models")
            .and_then(|m| m.as_array())
            .map(|ms| {
                ms.iter()
                    .filter_map(|m| {
                        let id = m.get("id").and_then(|x| x.as_str())?;
                        Some(ModelEntry {
                            provider: p
                                .get("id")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string(),
                            id: id.to_string(),
                            name: m
                                .get("name")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string(),
                            efforts: m
                                .get("efforts")
                                .and_then(|e| e.as_array())
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|x| x.as_str().map(str::to_string))
                                        .collect()
                                })
                                .unwrap_or_default(),
                            current: m.get("current").and_then(|x| x.as_bool()).unwrap_or(false),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        out_v.push(ProviderEntry {
            id: id.to_string(),
            label,
            active: p.get("active").and_then(|x| x.as_bool()).unwrap_or(false),
            models,
        });
    }
    (!out_v.is_empty()).then_some(out_v)
}

pub struct Turn {
    pub rx: Receiver<ProviderEvent>,
    pub pending: Arc<Mutex<Option<Sender<bool>>>>,
    child: Arc<Mutex<Option<Child>>>,
    cancelled: Arc<AtomicBool>,
}

impl Turn {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = self.child.lock()
            && let Some(child) = guard.as_mut()
        {
            let _ = child.kill();
        }
    }
}

pub struct TurnRequest {
    pub provider: String,
    pub model: String,
    pub mode: String,
    pub prompt: String,
    pub cwd: PathBuf,
    pub session: Option<String>,
    pub effort: String,
}

pub fn start(req: TurnRequest) -> Turn {
    let (tx, rx) = channel();
    let child = Arc::new(Mutex::new(None));
    let slot = child.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let pending = Arc::new(Mutex::new(None::<Sender<bool>>));
    let pend = pending.clone();
    thread::spawn(move || {
        let result = if flag.load(Ordering::SeqCst) {
            Ok(())
        } else {
            match req.provider.as_str() {
                "fake" => run_fake(&req, &tx, &flag, &pend),
                "claude" => run_process(claude_command(&req), &tx, &slot, &flag, parse_claude),
                "codex" => run_process(codex_command(&req), &tx, &slot, &flag, parse_codex),
                _ => Err("gray-backed providers run through the stdio engine".into()),
            }
        };
        if let Ok(mut p) = pend.lock() {
            *p = None;
        }
        match result {
            Ok(()) => {
                let _ = tx.send(ProviderEvent::Done);
            }
            Err(e) => {
                let _ = tx.send(ProviderEvent::Error(e));
            }
        }
    });
    Turn {
        rx,
        pending,
        child,
        cancelled,
    }
}

fn bin(env: &str, default: &str) -> String {
    std::env::var(env).unwrap_or_else(|_| default.to_string())
}

fn claude_command(req: &TurnRequest) -> Command {
    let mode = match req.mode.as_str() {
        "readonly" => "plan",
        "edits" => "acceptEdits",
        _ => "bypassPermissions",
    };
    let mut cmd = Command::new(bin("GRAY_TUI_CLAUDE_BIN", "claude"));
    cmd.arg("-p")
        .arg(&req.prompt)
        .args(["--output-format", "stream-json", "--verbose"])
        .args(["--permission-mode", mode]);
    if !req.model.is_empty() && req.model != "default" {
        cmd.args(["--model", &req.model]);
    }
    if let Some(sid) = &req.session {
        cmd.arg("--resume").arg(sid);
    }
    cmd.current_dir(&req.cwd);
    cmd
}

fn codex_command(req: &TurnRequest) -> Command {
    let sandbox = match req.mode.as_str() {
        "readonly" => "read-only",
        "edits" => "workspace-write",
        _ => "danger-full-access",
    };
    let mut cmd = Command::new(bin("GRAY_TUI_CODEX_BIN", "codex"));
    cmd.args(["exec", "--json", "--skip-git-repo-check"])
        .args(["--sandbox", sandbox]);
    if !req.model.is_empty() && req.model != "default" {
        cmd.args(["--model", &req.model]);
    }
    if !req.effort.is_empty() {
        cmd.arg("-c")
            .arg(format!("model_reasoning_effort=\"{}\"", req.effort));
    }
    if let Some(sid) = &req.session {
        cmd.arg("resume").arg(sid);
    }
    cmd.arg(&req.prompt);
    cmd.current_dir(&req.cwd);
    cmd
}

fn run_process(
    mut cmd: Command,
    tx: &Sender<ProviderEvent>,
    slot: &Arc<Mutex<Option<Child>>>,
    cancelled: &Arc<AtomicBool>,
    parse: fn(&serde_json::Value, &mut ParseState) -> Vec<ProviderEvent>,
) -> Result<(), String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to start {:?}: {e}", cmd.get_program()))?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take();
    if cancelled.load(Ordering::SeqCst) {
        let _ = child.kill();
        return Ok(());
    }
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(child);
        if cancelled.load(Ordering::SeqCst)
            && let Some(c) = guard.as_mut()
        {
            let _ = c.kill();
            return Ok(());
        }
    }
    let err_buf = Arc::new(Mutex::new(String::new()));
    let err_clone = err_buf.clone();
    let err_thread = stderr.map(|s| {
        thread::spawn(move || {
            for line in BufReader::new(s).lines().map_while(Result::ok) {
                if let Ok(mut b) = err_clone.lock() {
                    b.push_str(&line);
                    b.push('\n');
                }
            }
        })
    });
    let mut state = ParseState::default();
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(v) => {
                for ev in parse(&v, &mut state) {
                    let _ = tx.send(ev);
                }
            }
            Err(_) => {
                let _ = tx.send(ProviderEvent::Text(format!("{trimmed}\n")));
            }
        }
    }
    if let Some(t) = err_thread {
        let _ = t.join();
    }
    let status = slot
        .lock()
        .ok()
        .and_then(|mut g| g.as_mut().and_then(|c| c.wait().ok()));
    if let Some(err) = state.error.take() {
        return Err(err);
    }
    match status {
        Some(s) if !s.success() => {
            let stderr = err_buf.lock().map(|b| b.clone()).unwrap_or_default();
            let tail: String = stderr
                .lines()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            Err(if tail.is_empty() {
                format!("provider exited with {s}")
            } else {
                tail
            })
        }
        _ => Ok(()),
    }
}

#[derive(Default)]
pub struct ParseState {
    pub got_text: bool,
    pub error: Option<String>,
    pub session: Option<String>,
}

impl ParseState {
    fn session(&mut self, id: String) -> Option<ProviderEvent> {
        if id.is_empty() || self.session.as_deref() == Some(id.as_str()) {
            return None;
        }
        self.session = Some(id.clone());
        Some(ProviderEvent::Session(id))
    }
}

fn s(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string()
}

pub fn parse_claude(v: &serde_json::Value, st: &mut ParseState) -> Vec<ProviderEvent> {
    let mut out: Vec<ProviderEvent> = st.session(s(v, "session_id")).into_iter().collect();
    match v.get("type").and_then(|t| t.as_str()) {
        Some("assistant") => {
            if let Some(content) = v.pointer("/message/content").and_then(|c| c.as_array()) {
                for block in content {
                    match block.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            st.got_text = true;
                            out.push(ProviderEvent::Text(format!("{}\n\n", s(block, "text"))));
                        }
                        Some("tool_use") => out.push(ProviderEvent::Activity {
                            kind: "tool".into(),
                            label: s(block, "name"),
                            detail: block
                                .get("input")
                                .map(|i| i.to_string())
                                .unwrap_or_default(),
                        }),
                        Some("thinking") => out.push(ProviderEvent::Activity {
                            kind: "thinking".into(),
                            label: "Thinking".into(),
                            detail: s(block, "thinking"),
                        }),
                        _ => {}
                    }
                }
            }
        }
        Some("result") => {
            if v.get("is_error").and_then(|e| e.as_bool()) == Some(true) {
                st.error = Some(s(v, "result"));
            } else if !st.got_text {
                out.push(ProviderEvent::Replace(s(v, "result")));
            }
        }
        _ => {}
    }
    out
}

pub fn parse_codex(v: &serde_json::Value, st: &mut ParseState) -> Vec<ProviderEvent> {
    let mut out = Vec::new();
    match v.get("type").and_then(|t| t.as_str()) {
        Some("thread.started") => out.extend(st.session(s(v, "thread_id"))),
        Some("item.completed") => {
            let item = v.get("item").cloned().unwrap_or_default();
            match item.get("type").and_then(|t| t.as_str()) {
                Some("agent_message") => {
                    st.got_text = true;
                    out.push(ProviderEvent::Text(format!("{}\n\n", s(&item, "text"))));
                }
                Some("reasoning") => out.push(ProviderEvent::Activity {
                    kind: "thinking".into(),
                    label: "Reasoning".into(),
                    detail: s(&item, "text"),
                }),
                Some("command_execution") => out.push(ProviderEvent::Activity {
                    kind: "command".into(),
                    label: s(&item, "command"),
                    detail: s(&item, "aggregated_output"),
                }),
                Some("file_change") => out.push(ProviderEvent::Activity {
                    kind: "edit".into(),
                    label: "Edited files".into(),
                    detail: item
                        .get("changes")
                        .map(|c| c.to_string())
                        .unwrap_or_default(),
                }),
                _ => {}
            }
        }
        Some("turn.failed") | Some("error") => {
            st.error = Some(
                v.pointer("/error/message")
                    .and_then(|m| m.as_str())
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| s(v, "message")),
            );
        }
        _ => {}
    }
    out
}

fn ask_approval(
    tx: &Sender<ProviderEvent>,
    pending: &Arc<Mutex<Option<Sender<bool>>>>,
    label: String,
) -> bool {
    let (atx, arx) = channel();
    if let Ok(mut p) = pending.lock() {
        *p = Some(atx);
    }
    let _ = tx.send(ProviderEvent::Approval { label });
    arx.recv().unwrap_or(false)
}

fn run_fake(
    req: &TurnRequest,
    tx: &Sender<ProviderEvent>,
    cancelled: &Arc<AtomicBool>,
    pending: &Arc<Mutex<Option<Sender<bool>>>>,
) -> Result<(), String> {
    let delay = Duration::from_millis(
        std::env::var("GRAY_TUI_FAKE_DELAY_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(25),
    );
    let prompt = req.prompt.trim();
    let session = req
        .session
        .clone()
        .unwrap_or_else(|| format!("fake-{}", &uuid::Uuid::new_v4().to_string()[..8]));
    let effort = req.effort.clone();
    let _ = tx.send(ProviderEvent::Session(session.clone()));
    if prompt.contains("please fail") {
        thread::sleep(delay);
        return Err("fake provider failure requested".into());
    }
    let _ = tx.send(ProviderEvent::Activity {
        kind: "tool".into(),
        label: "Read workspace".into(),
        detail: req.cwd.display().to_string(),
    });
    if let Some(rest) = prompt.strip_prefix("write ") {
        let (path, content) = rest.split_once(' ').unwrap_or((rest, ""));
        if req.mode == "readonly" && !ask_approval(tx, pending, format!("Write {path}")) {
            let _ = tx.send(ProviderEvent::Activity {
                kind: "approval".into(),
                label: "Rejected".into(),
                detail: format!("Write {path}"),
            });
        } else {
            write_file(&req.cwd, path, content)?;
            let _ = tx.send(ProviderEvent::Activity {
                kind: "edit".into(),
                label: format!("Edited {path}"),
                detail: format!("+{} lines", content.lines().count().max(1)),
            });
        }
    }
    if let Some(cmd) = prompt.strip_prefix("run ") {
        if req.mode == "readonly" && !ask_approval(tx, pending, format!("Run `{cmd}`")) {
            let _ = tx.send(ProviderEvent::Activity {
                kind: "approval".into(),
                label: "Rejected".into(),
                detail: format!("Run {cmd}"),
            });
        } else {
            let output = Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .current_dir(&req.cwd)
                .output()
                .map_err(|e| e.to_string())?;
            let _ = tx.send(ProviderEvent::Activity {
                kind: "command".into(),
                label: cmd.to_string(),
                detail: String::from_utf8_lossy(&output.stdout).to_string(),
            });
        }
    }
    if prompt.contains("slow") {
        thread::sleep(Duration::from_secs(5));
    }
    let reply = format!(
        "Gray fake agent received: **{prompt}**\n\nRunning in `{}` with model `{}` and mode `{}` in session `{session}` at effort `{effort}`.\n\n```rust\nfn main() {{\n    println!(\"hello from gray\");\n}}\n```\n",
        req.cwd
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        req.model,
        mode_label(&req.mode)
    );
    for chunk in reply.split_inclusive(' ') {
        if cancelled.load(Ordering::SeqCst) {
            return Ok(());
        }
        let _ = tx.send(ProviderEvent::Text(chunk.to_string()));
        thread::sleep(delay);
    }
    Ok(())
}

fn write_file(root: &Path, rel: &str, content: &str) -> Result<(), String> {
    if rel.contains("..") {
        return Err("refusing to write outside the workspace".into());
    }
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, format!("{}\n", content.replace("\\n", "\n"))).map_err(|e| e.to_string())
}
