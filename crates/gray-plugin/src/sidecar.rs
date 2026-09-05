//! Sidecar plugin protocol (v1).
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

use gray_core::agent::{Tool, ToolContext, ToolOutput};
use gray_core::message::ToolDef;

use crate::{CoreEvent, Manifest, ManifestTool, Plugin, ToolBefore, manifest_tools};

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
    stdin: Mutex<ChildStdin>,
    pending: Arc<Mutex<Pending>>,
    next_id: AtomicU64,
    argv: Vec<String>,
}

pub struct SidecarPlugin {
    manifest: Manifest,
    tools: Vec<Arc<dyn Tool>>,
    transport: Arc<Transport>,
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
fn spawn_reader(stdout: ChildStdout, pending: Arc<Mutex<Pending>>, epoch: u64) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
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
        spawn_reader(stdout, pending.clone(), 0);
        Arc::new(Self {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            pending,
            next_id: AtomicU64::new(1),
            argv,
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
                spawn_reader(stdout, self.pending.clone(), p.epoch);
                true
            }
            Err(e) => {
                log::warn!(target: "gray_plugin", "sidecar respawn failed: {e}");
                false
            }
        }
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
        Ok(Self {
            manifest,
            tools,
            transport,
        })
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
        let v = self
            .transport
            .request(
                "prompt/context",
                Some(json!({"cwd": cwd})),
                Duration::from_secs(30),
            )
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
        match self
            .transport
            .request(
                "tool/before",
                Some(json!({"name": name, "args": args})),
                Duration::from_secs(30),
            )
            .await
        {
            Ok(v) => ToolBefore::from_result(&v, args),
            Err(e) => {
                log::warn!(target: "gray_plugin", "sidecar tool/before failed, failing open: {e}");
                ToolBefore::Allow
            }
        }
    }
    async fn run_command(&self, name: &str, argv: Vec<String>) -> Option<String> {
        if !self.manifest.commands.iter().any(|c| c == name) {
            return None;
        }
        let v = self
            .transport
            .request(
                "command/run",
                Some(json!({"name": name, "argv": argv})),
                Duration::from_secs(30),
            )
            .await
            .ok()?;
        v.get("text")
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
    }
    async fn on_event(&self, e: CoreEvent) -> Option<CoreEvent> {
        // Minimal tagged JSON (see protocol v1 doc comment above).
        let params = match &e {
            CoreEvent::PreStep { .. } => json!({"type": "pre_step"}),
            CoreEvent::PreTool { name, args } => {
                json!({"type": "pre_tool", "name": name, "args": args})
            }
            CoreEvent::PostTool { name, output } => {
                json!({"type": "post_tool", "name": name, "content": output.content, "is_error": output.is_error})
            }
            CoreEvent::TurnEnd { usage } => json!({"type": "turn_end", "usage": usage}),
        };
        let name = self.manifest.name.clone();
        let req = json!({"method": "event/notify", "params": params});
        // True notification: send without id, never read a reply. Unconditional
        // (pre-v1 sidecars already ignore it) with a short writer lock only.
        match timeout(Duration::from_secs(5), async {
            if !self.transport.ensure_alive().await {
                return false;
            }
            self.transport
                .stdin
                .lock()
                .await
                .write_all(format!("{req}\n").as_bytes())
                .await
                .is_ok()
        })
        .await
        {
            Ok(true) => None, // notify never transforms the event
            Ok(false) => {
                log::warn!(target: "gray_plugin", "sidecar {name} hook failed, skipping");
                None
            }
            Err(_) => {
                log::warn!(target: "gray_plugin", "sidecar {name} hook timeout, skipping");
                None
            }
        }
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
    async fn execute(&self, _ctx: &ToolContext, args: Value) -> ToolOutput {
        let name = self.def.name.clone();
        match self
            .transport
            .request(
                "tool/call",
                Some(json!({"name": name, "args": args})),
                Duration::from_secs(30),
            )
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
