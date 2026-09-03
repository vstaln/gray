use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio::time::{Duration, timeout};

use gray_core::agent::{Tool, ToolContext, ToolOutput};
use gray_core::message::ToolDef;

use crate::{CoreEvent, Manifest, Plugin};

struct Io {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

struct State {
    child: Child,
    io: Io,
}

pub struct SidecarPlugin {
    manifest: Manifest,
    tools: Vec<Arc<dyn Tool>>,
    state: Arc<Mutex<State>>,
    argv: Vec<String>,
    next_id: Arc<AtomicU64>,
}

fn spawn_child(argv: &[String]) -> anyhow::Result<(Child, Io)> {
    let (prog, args) = argv.split_first().ok_or_else(|| anyhow::anyhow!("empty argv"))?;
    let mut child = Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdin = child.stdin.take().ok_or_else(|| anyhow::anyhow!("no stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("no stdout"))?;
    Ok((child, Io { stdin, stdout: BufReader::new(stdout) }))
}

impl SidecarPlugin {
    pub async fn spawn(argv: Vec<String>) -> anyhow::Result<Self> {
        let (child, io) = spawn_child(&argv)?;
        let state = Arc::new(Mutex::new(State { child, io }));
        let next_id = Arc::new(AtomicU64::new(1));
        let manifest = Self::rpc(&state, &next_id, "plugin/manifest", None, Duration::from_secs(30))
            .await
            .and_then(|v| {
                Ok(Manifest {
                    name: v.get("name").and_then(|s| s.as_str()).unwrap_or_default().into(),
                    version: v.get("version").and_then(|s| s.as_str()).unwrap_or_default().into(),
                    tools: v
                        .get("tools")
                        .and_then(|t| serde_json::from_value(t.clone()).ok())
                        .unwrap_or_default(),
                    provider: v.get("provider").and_then(|s| s.as_str()).map(|s| s.into()),
                })
            })?;
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
        for name in manifest.tools.clone() {
            tools.push(Arc::new(SidecarTool {
                name,
                state: state.clone(),
                argv: argv.clone(),
                next_id: next_id.clone(),
            }));
        }
        Ok(Self { manifest, tools, state, argv, next_id })
    }

    async fn ensure_alive_locked(state: &mut State, argv: &[String]) -> bool {
        match state.child.try_wait() {
            Ok(Some(_)) => {
                log::warn!(target: "gray_plugin", "sidecar child exited, respawning");
                match spawn_child(argv) {
                    Ok((child, io)) => {
                        state.child = child;
                        state.io = io;
                        true
                    }
                    Err(e) => {
                        log::warn!(target: "gray_plugin", "sidecar respawn failed: {e}");
                        false
                    }
                }
            }
            Ok(None) => true,
            Err(_) => false,
        }
    }

    async fn rpc(
        state: &Arc<Mutex<State>>,
        next_id: &Arc<AtomicU64>,
        method: &str,
        params: Option<Value>,
        ttl: Duration,
    ) -> anyhow::Result<Value> {
        let id = next_id.fetch_add(1, Ordering::SeqCst);
        let mut req = json!({"id": id, "method": method});
        if let Some(p) = params {
            req["params"] = p;
        }
        timeout(ttl, async {
            let mut state = state.lock().await;
            state.io.stdin.write_all(format!("{req}\n").as_bytes()).await?;
            let mut line = String::new();
            let n = state.io.stdout.read_line(&mut line).await?;
            if n == 0 {
                anyhow::bail!("sidecar child closed stdout (crashed?)");
            }
            let resp: Value = serde_json::from_str(&line)?;
            let rid = resp.get("id").and_then(|v| v.as_u64());
            if rid != Some(id) {
                anyhow::bail!("sidecar id mismatch: expected {id}, got {rid:?}");
            }
            Ok::<Value, anyhow::Error>(resp.get("result").cloned().unwrap_or(Value::Null))
        })
        .await?
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
    async fn on_event(&self, e: CoreEvent) -> Option<CoreEvent> {
        let params = match serde_json::to_value(format!("{e:?}")) {
            Ok(v) => v,
            Err(_) => return None,
        };
        let name = self.manifest.name.clone();
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = json!({"id": id, "method": "event/notify", "params": params});
        match timeout(Duration::from_secs(5), async {
            let mut state = self.state.lock().await;
            if !Self::ensure_alive_locked(&mut state, &self.argv).await {
                return None;
            }
            state.io.stdin.write_all(format!("{req}\n").as_bytes()).await.ok()?;
            let mut line = String::new();
            // Fire-and-forget notify: don't require a reply; just drain if one comes.
            tokio::select! {
                r = state.io.stdout.read_line(&mut line) => {
                    let _ = r;
                    if !line.is_empty() {
                        if let Ok(resp) = serde_json::from_str::<Value>(&line) {
                            if resp.get("id").and_then(|v| v.as_u64()) != Some(id) {
                                log::warn!(target: "gray_plugin", "sidecar {name} hook id mismatch, skipping");
                                return None;
                            }
                        }
                    }
                }
            }
            Some(true)
        })
        .await
        {
            Ok(Some(_)) => None, // notify never transforms the event
            Ok(None) => {
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
    name: String,
    state: Arc<Mutex<State>>,
    argv: Vec<String>,
    next_id: Arc<AtomicU64>,
}

#[async_trait]
impl Tool for SidecarTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(self.name.clone(), format!("sidecar tool {}", self.name), json!({}))
    }
    async fn execute(&self, _ctx: &ToolContext, args: Value) -> ToolOutput {
        let name = self.name.clone();
        // Lazy respawn if the child died.
        {
            let mut state = self.state.lock().await;
            if !SidecarPlugin::ensure_alive_locked(&mut state, &self.argv).await {
                return ToolOutput::error(format!("plugin crashed: {name}"));
            }
        }
        match SidecarPlugin::rpc(
            &self.state,
            &self.next_id,
            "tool/call",
            Some(json!({"name": name, "args": args})),
            Duration::from_secs(5),
        )
        .await
        {
            Ok(v) => ToolOutput {
                content: v.get("content").and_then(|c| c.as_str()).unwrap_or_default().into(),
                is_error: v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false),
            },
            Err(e) => {
                log::warn!(target: "gray_plugin", "sidecar {name} tool call failed, skipping: {e}");
                ToolOutput::error(format!("plugin timeout: {name}"))
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
        let p = SidecarPlugin::spawn(vec!["testdata/hang_plugin.sh".into()]).await.unwrap();
        let t = std::time::Instant::now();
        assert!(
            p.on_event(CoreEvent::TurnEnd { usage: Usage::default() }).await.is_none()
        );
        assert!(t.elapsed() < std::time::Duration::from_secs(10));
    }

    #[tokio::test]
    async fn crashed_plugin_returns_error_not_panic() {
        let p = SidecarPlugin::spawn(vec!["testdata/crash_plugin.sh".into()]).await.unwrap();
        let out = p.tools()[0].execute(&ToolContext::default(), serde_json::json!({})).await;
        assert!(!out.is_error);
        let out2 = p.tools()[0].execute(&ToolContext::default(), serde_json::json!({})).await;
        assert!(out2.is_error);
    }
}
