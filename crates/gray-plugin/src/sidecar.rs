//! Sidecar plugin protocol (v1). See `docs/protocol-v1.md` for the
//! versioned spec (methods, TTLs, gating, host-emission audit).
//!
//! Transport: newline-delimited JSON over child stdio. Requests the host
//! sends are `{"id", "method", "params?"}`; sidecars reply with
//! `{"id", "result"}` for request/response methods only:
//! - `plugin/manifest` (request): no params, reply
//!   `{"name","version","tools":[{"name","description","parameters","snippet"}],
//!   "commands":["/x"],"hooks":[...]}`. Pre-v1 `"tools":["name"]` still parses.
//! - `tool/call` (request): params `{"name","args"}`, reply `{"content","is_error?"}`.
//! - `prompt/context` (request): params `{"cwd"}`, reply `{"text"}`.
//! - `tool/before` (request): params `{"name","args"}`, reply allow/deny/modify.
//! - `command/run` (request): params `{"name":"/x","argv"}`, reply `{"text"}`.
//! - `event/notify` (notification): NO `id`, NO reply expected. Params carry a
//!   minimal tagged event `{"type", ...}` where type is one of
//!   `pre_step` | `pre_tool` | `post_tool` | `turn_end` with only the fields
//!   the sidecar needs (tool name/args, output content, usage totals).
//!
//! Unknown methods/lines are ignored.
//!
//! The three v1 request methods are only sent to sidecars claiming them in
//! `hooks`/`commands`, so pre-v1 sidecars (which ignore unknown lines, hence
//! would never reply) keep working.
//!
//! Concurrency: one reader task per sidecar routes replies by `id` into
//! `pending`; writers take a short stdin lock only, so concurrent requests
//! resolve out of order instead of serializing on one mutex.

use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, oneshot};
use tokio::time::{Duration, timeout};

use gray_core::agent::{CommandOutcome, Tool, ToolContext, ToolOutput};
use gray_core::message::ToolDef;

use crate::{CoreEvent, Manifest, ManifestTool, Plugin, ToolBefore, manifest_tools};

/// Plugin→host request handler (`host/run`, `host/say`). Set by the host via
/// [`SidecarPlugin::set_host_handler`]; without one the transport replies
/// `{"error": ...}` so plugin-initiated turns fail loudly, never hang.
/// Boxed-future shape (not `async_trait`) so the reader task can hold it
/// behind a plain `Mutex` without an extra dependency.
pub type HostHandler = Arc<
    dyn Fn(
            String,
            serde_json::Value,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = serde_json::Value> + Send + 'static>,
        > + Send
        + Sync,
>;

/// Host-side methods a sidecar may call (requests, unlike `event/notify`:
/// they carry a sidecar-originated **string** `id` and expect a
/// `{"id","result"}` reply). Host→sidecar ids stay numeric — the two
/// namespaces never collide, so replies route unambiguously.
pub const HOST_RUN: &str = "host/run";
pub const HOST_SAY: &str = "host/say";

/// Build the v1.1 `session` object: `{"id":<session_id or "">,"cwd":<cwd>}`.
/// `tool/call` uses `ctx` (cwd + session_id); every other wire point uses
/// the pinned boot cwd and `""` (no `ToolContext` there to read).
fn session_json(id: &str, cwd: &str) -> Value {
    json!({"id": id, "cwd": cwd})
}

/// In-flight request senders, keyed by request id. `epoch` marks the child
/// generation: a stale reader exiting late must not fail a new child's
/// requests (bumped on every respawn).
struct Pending {
    epoch: u64,
    map: HashMap<u64, oneshot::Sender<Value>>,
}

/// Shared sidecar transport: child handle (respawn only), stdin (short
/// writer lock), pending replies, id counter, spawn argv.
struct Transport {
    child: Mutex<Child>,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<Pending>>,
    next_id: AtomicU64,
    argv: Vec<String>,
    /// Plugin→host handler (`host/run`/`host/say`). `Arc` so respawned
    /// readers keep the same slot; `None` = reply `{"error":...}`.
    host_handler: Arc<Mutex<Option<HostHandler>>>,
}

pub struct SidecarPlugin {
    manifest: Manifest,
    tools: Vec<Arc<dyn Tool>>,
    transport: Arc<Transport>,
    /// Pinned boot cwd (captured at spawn from the process cwd). Used for
    /// the `session.cwd` of every wire point without a `ToolContext`
    /// (`prompt/context` uses its `cwd` arg instead; `tool/call` uses `ctx.cwd`).
    cwd: String,
}

fn spawn_child(argv: &[String]) -> anyhow::Result<(Child, ChildStdin, ChildStdout)> {
    let (prog, args) = argv
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("empty argv"))?;
    let mut child = Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("no stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("no stdout"))?;
    Ok((child, stdin, stdout))
}

/// Route reply lines to their request by `id`; lines without a known `id`
/// are ignored. On EOF (child gone) fail only our own generation's
/// in-flight requests so callers report crash instead of hanging.
/// Sidecar-originated requests (`method: "host/..."` + **string** `id`)
/// dispatch to the host handler and get a `{"id","result"}` reply on the
/// same stdio; anything else without a pending numeric id is dropped.
fn spawn_reader(
    stdout: ChildStdout,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<Pending>>,
    host_handler: Arc<Mutex<Option<HostHandler>>>,
    epoch: u64,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            // Plugin→host request: string id + host/ method.
            if let (Some(id), Some(method)) = (
                v.get("id").cloned(),
                v.get("method").and_then(|m| m.as_str()),
            ) && id.is_string()
                && method.starts_with("host/")
            {
                let params = v.get("params").cloned().unwrap_or(Value::Null);
                let method_owned = method.to_string();
                let stdin = stdin.clone();
                let host_handler = host_handler.clone();
                tokio::spawn(async move {
                    let handler = host_handler.lock().await.clone();
                    let result = match handler {
                        Some(h) => {
                            timeout(Duration::from_secs(30), h(method_owned.clone(), params))
                                .await
                                .unwrap_or_else(
                                    |_| json!({"error": format!("{method_owned} timed out")}),
                                )
                        }
                        None => json!({"error": format!("no host handler for {method_owned}")}),
                    };
                    let reply = json!({"id": id, "result": result});
                    let _ = stdin
                        .lock()
                        .await
                        .write_all(format!("{reply}\n").as_bytes())
                        .await;
                });
                continue;
            }
            let Some(id) = v.get("id").and_then(|i| i.as_u64()) else {
                continue;
            };
            let tx = pending.lock().await.map.remove(&id);
            if let Some(tx) = tx {
                let _ = tx.send(v.get("result").cloned().unwrap_or(Value::Null));
            }
        }
        let mut p = pending.lock().await;
        if p.epoch == epoch {
            p.map.clear();
        }
    });
}

impl Transport {
    fn new(child: Child, stdin: ChildStdin, stdout: ChildStdout, argv: Vec<String>) -> Arc<Self> {
        let pending = Arc::new(Mutex::new(Pending {
            epoch: 0,
            map: HashMap::new(),
        }));
        let stdin = Arc::new(Mutex::new(stdin));
        let host_handler = Arc::new(Mutex::new(None));
        spawn_reader(
            stdout,
            stdin.clone(),
            pending.clone(),
            host_handler.clone(),
            0,
        );
        Arc::new(Self {
            child: Mutex::new(child),
            stdin,
            pending,
            next_id: AtomicU64::new(1),
            argv,
            host_handler,
        })
    }

    /// Respawn a dead child (new stdio + reader; old generation's in-flight
    /// requests fail fast). Lock order is always child → stdin → pending.
    async fn ensure_alive(&self) -> bool {
        let mut child = self.child.lock().await;
        if matches!(child.try_wait(), Ok(None)) {
            return true;
        }
        log::warn!(target: "gray_plugin", "sidecar child exited, respawning");
        match spawn_child(&self.argv) {
            Ok((new_child, stdin, stdout)) => {
                *child = new_child;
                *self.stdin.lock().await = stdin;
                let mut p = self.pending.lock().await;
                p.epoch += 1;
                p.map.clear();
                spawn_reader(
                    stdout,
                    self.stdin.clone(),
                    self.pending.clone(),
                    self.host_handler.clone(),
                    p.epoch,
                );
                true
            }
            Err(e) => {
                log::warn!(target: "gray_plugin", "sidecar respawn failed: {e}");
                false
            }
        }
    }

    /// Fire-and-forget notification: write one `{"method","params"}` line
    /// (no `id`, no reply) with a 5s timeout. Shared by `event/notify` and
    /// `plugin/shutdown` — pre-v1 sidecars already ignore unknown lines.
    async fn send_notification(&self, method: &str, params: Value) -> bool {
        let req = json!({"method": method, "params": params});
        timeout(Duration::from_secs(5), async {
            if !self.ensure_alive().await {
                return false;
            }
            self.stdin
                .lock()
                .await
                .write_all(format!("{req}\n").as_bytes())
                .await
                .is_ok()
        })
        .await
        .unwrap_or_default()
    }

    async fn request(
        &self,
        method: &str,
        params: Option<Value>,
        ttl: Duration,
    ) -> anyhow::Result<Value> {
        if !self.ensure_alive().await {
            anyhow::bail!("sidecar child dead and respawn failed");
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut req = json!({"id": id, "method": method});
        if let Some(p) = params {
            req["params"] = p;
        }
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.map.insert(id, tx);
        let write_err = async {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(format!("{req}\n").as_bytes()).await
        }
        .await
        .err();
        if let Some(e) = write_err {
            self.pending.lock().await.map.remove(&id);
            return Err(e.into());
        }
        match timeout(ttl, rx).await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(_)) => anyhow::bail!("sidecar child closed stdout (crashed?)"),
            Err(_) => {
                self.pending.lock().await.map.remove(&id);
                anyhow::bail!("sidecar request timed out ({method})");
            }
        }
    }
}

impl SidecarPlugin {
    pub async fn spawn(argv: Vec<String>) -> anyhow::Result<Self> {
        let (child, stdin, stdout) = spawn_child(&argv)?;
        let transport = Transport::new(child, stdin, stdout, argv.clone());
        let result = transport
            .request("plugin/manifest", None, Duration::from_secs(30))
            .await?;
        let mut manifest = Manifest::from_result(&result);
        let name = manifest.name.trim().to_string();
        if name.is_empty() {
            anyhow::bail!(
                "sidecar manifest has missing/empty name (argv: {})",
                argv.join(" ")
            );
        }
        manifest.name = name;
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
        for entry in manifest_tools(&result) {
            tools.push(Arc::new(SidecarTool::new(entry, transport.clone())));
        }
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| ".".to_string());
        Ok(Self {
            manifest,
            tools,
            transport,
            cwd,
        })
    }

    /// Graceful lifecycle teardown: send `plugin/shutdown` (`reason:
    /// `session_end`) to v1.1 sidecars only (pre-v1 `protocol: None` never
    /// receives the line — they ignore unknown input anyway), then wait up
    /// to `grace` for the child to exit voluntarily and `kill()` what remains.
    /// Never waits for a reply (notification has no `id`).
    pub async fn shutdown(&self, grace: Duration) {
        if self.manifest.protocol.is_some() {
            let _ = self
                .transport
                .send_notification("plugin/shutdown", json!({"reason": "session_end"}))
                .await;
        }
        let mut child = self.transport.child.lock().await;
        let _ = timeout(grace, child.wait()).await;
        let _ = child.kill().await;
    }
    /// Install the plugin→host handler (`host/run`/`host/say`). The host
    /// sets this once after spawn; sidecar requests then dispatch to it
    /// with a 30s TTL each. Without a handler the transport replies
    /// `{"error":...}` (loud failure, never a hang).
    pub async fn set_host_handler(&self, handler: HostHandler) {
        *self.transport.host_handler.lock().await = Some(handler);
    }
    /// v1 request methods are gated on the manifest's `hooks`/`commands`:
    /// pre-v1 sidecars ignore unknown lines and would never reply, so
    /// sending them anything new would hang every turn to the full timeout.
    fn claims(&self, hook: &str) -> bool {
        self.manifest.hooks.iter().any(|h| h == hook)
    }
}

#[async_trait]
impl Plugin for SidecarPlugin {
    fn manifest(&self) -> Manifest {
        self.manifest.clone()
    }
    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.clone()
    }
    async fn prompt_context(&self, cwd: &str) -> Option<String> {
        if !self.claims("prompt/context") {
            return None;
        }
        let params = json!({"cwd": cwd, "session": session_json("", cwd)});
        let v = self
            .transport
            .request("prompt/context", Some(params), Duration::from_secs(30))
            .await
            .ok()?;
        v.get("text")
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
    }
    async fn tool_before(&self, name: &str, args: &Value) -> ToolBefore {
        if !self.claims("tool/before") {
            return ToolBefore::Allow;
        }
        let params = json!({"name": name, "args": args, "session": session_json("", &self.cwd)});
        match self
            .transport
            .request("tool/before", Some(params), Duration::from_secs(30))
            .await
        {
            Ok(v) => ToolBefore::from_result(&v, args),
            Err(e) => {
                log::warn!(target: "gray_plugin", "sidecar tool/before failed, failing open: {e}");
                ToolBefore::Allow
            }
        }
    }
    async fn run_command(&self, name: &str, argv: Vec<String>) -> Option<CommandOutcome> {
        // `subcommands` (cron, gateway, …) forward argv over the same
        // `command/run` wire as `commands` — one path, no special-casing.
        if !self.manifest.commands.iter().any(|c| c == name)
            && !self.manifest.subcommands.iter().any(|c| c == name)
        {
            return None;
        }
        let params = json!({"name": name, "argv": argv, "session": session_json("", &self.cwd)});
        let v = self
            .transport
            .request("command/run", Some(params), Duration::from_secs(30))
            .await
            .ok()?;
        // `{"prompt":...}` wins over `{"text":...}`; empty/missing → None.
        if let Some(p) = v
            .get("prompt")
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
        {
            return Some(CommandOutcome::Prompt(p.to_string()));
        }
        v.get("text")
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
            .map(|t| CommandOutcome::Say(t.to_string()))
    }
    async fn on_event(&self, e: CoreEvent) -> Option<CoreEvent> {
        // Minimal tagged JSON (see protocol v1 doc comment above) + v1.1 session.
        let session = session_json("", &self.cwd);
        let params = match &e {
            CoreEvent::PreStep { .. } => json!({"type": "pre_step", "session": session}),
            CoreEvent::PreTool { name, args } => {
                json!({"type": "pre_tool", "name": name, "args": args, "session": session})
            }
            CoreEvent::PostTool { name, output } => {
                json!({"type": "post_tool", "name": name, "content": output.content, "is_error": output.is_error, "session": session})
            }
            CoreEvent::TurnEnd { usage } => {
                json!({"type": "turn_end", "usage": usage, "session": session})
            }
        };
        let name = self.manifest.name.clone();
        // True notification via shared helper: no id, never a reply.
        if self
            .transport
            .send_notification("event/notify", params)
            .await
        {
            None // notify never transforms the event
        } else {
            log::warn!(target: "gray_plugin", "sidecar {name} hook failed, skipping");
            None
        }
    }
    async fn shutdown(&self) {
        self.shutdown(Duration::from_secs(2)).await;
    }
}

struct SidecarTool {
    def: ToolDef,
    snippet: Option<&'static str>,
    transport: Arc<Transport>,
}

impl SidecarTool {
    fn new(entry: ManifestTool, transport: Arc<Transport>) -> Self {
        // `Tool::prompt_snippet` returns `&'static str` but sidecar snippets
        // arrive at runtime: one tiny leak per tool per spawn. Manifest
        // snippet wins; description keeps snippet-less tools visible (the
        // pre-v1 gap was `None` hiding every sidecar tool).
        let text = entry
            .snippet
            .filter(|s| !s.is_empty())
            .or_else(|| (!entry.def.description.is_empty()).then(|| entry.def.description.clone()));
        let snippet = text.map(|s| Box::leak(s.into_boxed_str()) as &'static str);
        Self {
            def: entry.def,
            snippet,
            transport,
        }
    }
}

#[async_trait]
impl Tool for SidecarTool {
    fn def(&self) -> ToolDef {
        self.def.clone()
    }
    fn prompt_snippet(&self) -> Option<&'static str> {
        self.snippet
    }
    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let name = self.def.name.clone();
        let cwd = ctx.cwd.to_string_lossy();
        let sid = ctx.session_id.as_deref().unwrap_or("");
        let params = json!({"name": name, "args": args, "session": session_json(sid, &cwd)});
        match self
            .transport
            .request("tool/call", Some(params), Duration::from_secs(30))
            .await
        {
            Ok(v) => ToolOutput {
                content: v
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default()
                    .into(),
                is_error: v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false),
            },
            Err(e) => {
                log::warn!(target: "gray_plugin", "sidecar {name} tool call failed, skipping: {e}");
                let msg = e.to_string();
                let kind = if msg.contains("timed out") || msg.contains("elapsed") {
                    "timeout"
                } else if msg.contains("closed stdout") || msg.contains("respawn failed") {
                    "crashed"
                } else {
                    "protocol error"
                };
                ToolOutput::error(format!("plugin {kind}: {name}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gray_core::agent::ToolContext;
    use gray_core::event::Usage;

    #[tokio::test]
    async fn hanging_hook_times_out_and_skips() {
        let p = SidecarPlugin::spawn(vec!["testdata/hang_plugin.sh".into()])
            .await
            .unwrap();
        let t = std::time::Instant::now();
        assert!(
            p.on_event(CoreEvent::TurnEnd {
                usage: Usage::default()
            })
            .await
            .is_none()
        );
        assert!(t.elapsed() < std::time::Duration::from_secs(10));
    }

    #[tokio::test]
    async fn crashed_plugin_returns_error_not_panic() {
        let p = SidecarPlugin::spawn(vec!["testdata/crash_plugin.sh".into()])
            .await
            .unwrap();
        let out = p.tools()[0]
            .execute(&ToolContext::default(), serde_json::json!({}))
            .await;
        assert!(out.is_error);
        assert!(
            out.content.contains("plugin crashed: crash"),
            "got: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn empty_manifest_name_bails() {
        let err = SidecarPlugin::spawn(vec!["testdata/empty_name_plugin.sh".into()])
            .await
            .err()
            .expect("spawn must bail on missing/empty name");
        assert!(err.to_string().contains("empty name"), "got: {err:#}");
    }

    #[tokio::test]
    async fn notify_sends_no_id_and_needs_no_reply() {
        // hang fixture never replies to event/notify; if on_event waited for a
        // reply it would hit the 5s timeout. True notification returns fast.
        let p = SidecarPlugin::spawn(vec!["testdata/hang_plugin.sh".into()])
            .await
            .unwrap();
        let t = std::time::Instant::now();
        assert!(
            p.on_event(CoreEvent::TurnEnd {
                usage: Usage::default()
            })
            .await
            .is_none()
        );
        assert!(t.elapsed() < std::time::Duration::from_secs(5));
    }
}
