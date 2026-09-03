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

pub struct SidecarPlugin {
    manifest: Manifest,
    tools: Vec<Arc<dyn Tool>>,
    io: Arc<Mutex<Io>>,
    #[allow(dead_code)]
    child: Child,
}

impl SidecarPlugin {
    pub async fn spawn(argv: Vec<String>) -> anyhow::Result<Self> {
        let (prog, args) = argv.split_first().ok_or_else(|| anyhow::anyhow!("empty argv"))?;
        let mut child = Command::new(prog)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow::anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("no stdout"))?;
        let io = Arc::new(Mutex::new(Io { stdin, stdout: BufReader::new(stdout) }));
        let manifest = Self::rpc(&io, &AtomicU64::new(1), "plugin/manifest", None, Duration::from_secs(30))
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
        let tool_names = manifest.tools.clone();
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
        for name in tool_names {
            tools.push(Arc::new(SidecarTool {
                name,
                io: io.clone(),
                next_id: AtomicU64::new(1000),
            }));
        }
        Ok(Self { manifest, tools, io, child })
    }

    async fn rpc(
        io: &Arc<Mutex<Io>>,
        next_id: &AtomicU64,
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
            let mut io = io.lock().await;
            io.stdin.write_all(format!("{req}\n").as_bytes()).await?;
            let mut line = String::new();
            io.stdout.read_line(&mut line).await?;
            let resp: Value = serde_json::from_str(&line)?;
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
        timeout(Duration::from_secs(5), async {
            let mut io = self.io.lock().await;
            let req = json!({"method": "event/notify", "params": params});
            io.stdin.write_all(format!("{req}\n").as_bytes()).await.ok()?;
            Some(e)
        })
        .await
        .ok()
        .flatten()
        .and_then(|_| None)
    }
}

struct SidecarTool {
    name: String,
    io: Arc<Mutex<Io>>,
    next_id: AtomicU64,
}

#[async_trait]
impl Tool for SidecarTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(self.name.clone(), format!("sidecar tool {}", self.name), json!({}))
    }
    async fn execute(&self, _ctx: &ToolContext, args: Value) -> ToolOutput {
        let name = self.name.clone();
        match SidecarPlugin::rpc(
            &self.io,
            &self.next_id,
            "tool/call",
            Some(json!({"name": name, "args": args})),
            Duration::from_secs(30),
        )
        .await
        {
            Ok(v) => ToolOutput {
                content: v.get("content").and_then(|c| c.as_str()).unwrap_or_default().into(),
                is_error: v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false),
            },
            Err(_) => ToolOutput::error(format!("plugin timeout: {name}")),
        }
    }
}
