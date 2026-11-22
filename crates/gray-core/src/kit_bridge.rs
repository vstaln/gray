use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::agent::{PluginHooks, ToolBefore, ToolContext, ToolExecutor};
use crate::event::AgentEvent;
use crate::kit;
use crate::message::ToolDef;

static BRIDGE_SEQ: AtomicU64 = AtomicU64::new(0);
static CALL_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub(crate) struct BridgeState {
    pub(crate) tools: Arc<RwLock<Vec<ToolDef>>>,
    pub(crate) executor: Arc<dyn ToolExecutor>,
    pub(crate) hooks: Arc<RwLock<Vec<Arc<dyn PluginHooks>>>>,
    pub(crate) ctx: Arc<Mutex<Option<ToolContext>>>,
    pub(crate) events: Arc<Mutex<Option<mpsc::UnboundedSender<AgentEvent>>>>,
    pub(crate) token: Option<String>,
}

pub(crate) struct KitBridge {
    pub(crate) state: BridgeState,
    dir: PathBuf,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for KitBridge {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl KitBridge {
    pub(crate) fn start(
        tools: Vec<ToolDef>,
        executor: Arc<dyn ToolExecutor>,
        hooks: Vec<Arc<dyn PluginHooks>>,
    ) -> std::io::Result<Self> {
        let seq = BRIDGE_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
        let dir = std::env::temp_dir().join(format!("gray-kit-{}-{seq}", std::process::id()));
        let bin_dir = dir.join("bin");
        std::fs::create_dir_all(&bin_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }

        let shim = bin_dir.join("kit");
        std::fs::write(&shim, "#!/bin/sh\nexec \"$GRAY_BIN\" kit \"$@\"\n")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755));
        }

        let gray_bin = std::env::current_exe()
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "gray".to_string());

        #[cfg(unix)]
        let (sock_addr, token, task, state) = {
            let sock_path = dir.join("kit.sock");
            let listener = tokio::net::UnixListener::bind(&sock_path)?;
            {
                use std::os::unix::fs::PermissionsExt;
                let _ =
                    std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600));
            }
            let state = BridgeState {
                tools: Arc::new(RwLock::new(tools)),
                executor: Arc::clone(&executor),
                hooks: Arc::new(RwLock::new(hooks)),
                ctx: Arc::new(Mutex::new(None)),
                events: Arc::new(Mutex::new(None)),
                token: None,
            };
            let st = state.clone();
            let task = tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        break;
                    };
                    let st = st.clone();
                    tokio::spawn(async move {
                        handle_stream(stream, st).await;
                    });
                }
            });
            (sock_path.to_string_lossy().into_owned(), None, task, state)
        };

        #[cfg(not(unix))]
        let (sock_addr, token, task, state) = {
            let std_listener = std::net::TcpListener::bind("127.0.0.1:0")?;
            std_listener.set_nonblocking(true)?;
            let addr = std_listener.local_addr()?.to_string();
            let listener = tokio::net::TcpListener::from_std(std_listener)?;
            let tok = uuid::Uuid::new_v4().to_string();
            let state = BridgeState {
                tools: Arc::new(RwLock::new(tools)),
                executor: Arc::clone(&executor),
                hooks: Arc::new(RwLock::new(hooks)),
                ctx: Arc::new(Mutex::new(None)),
                events: Arc::new(Mutex::new(None)),
                token: Some(tok.clone()),
            };
            let st = state.clone();
            let task = tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        break;
                    };
                    let st = st.clone();
                    tokio::spawn(async move {
                        handle_stream(stream, st).await;
                    });
                }
            });
            (addr, Some(tok), task, state)
        };

        let mut vars = vec![
            ("GRAY_KIT_SOCK".to_string(), sock_addr),
            ("GRAY_BIN".to_string(), gray_bin),
        ];
        if let Some(tok) = token {
            vars.push(("GRAY_KIT_TOKEN".to_string(), tok));
        }
        executor.inject_shell_env(vars, vec![bin_dir]);

        Ok(Self { state, dir, task })
    }

    pub(crate) fn sync_tools(&self, tools: &[ToolDef]) {
        if let Ok(mut guard) = self.state.tools.write() {
            *guard = tools.to_vec();
        }
    }

    pub(crate) fn sync_hooks(&self, hooks: &[Arc<dyn PluginHooks>]) {
        if let Ok(mut guard) = self.state.hooks.write() {
            *guard = hooks.to_vec();
        }
    }

    pub(crate) fn set_turn(
        &self,
        ctx: Option<ToolContext>,
        events: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) {
        if let Ok(mut guard) = self.state.ctx.lock() {
            *guard = ctx;
        }
        if let Ok(mut guard) = self.state.events.lock() {
            *guard = events;
        }
    }
}

async fn handle_stream<S>(stream: S, state: BridgeState)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut lines = BufReader::new(reader).lines();
    let Ok(Some(line)) = lines.next_line().await else {
        return;
    };
    let reply = match serde_json::from_str::<Value>(&line) {
        Ok(req) => dispatch(req, &state).await,
        Err(e) => json!({"ok": false, "error": format!("invalid request: {e}")}),
    };
    let mut bytes = serde_json::to_vec(&reply).unwrap_or_else(|_| b"{\"ok\":false}".to_vec());
    bytes.push(b'\n');
    let _ = writer.write_all(&bytes).await;
    let _ = writer.flush().await;
}

async fn dispatch(req: Value, state: &BridgeState) -> Value {
    if let Some(expected) = state.token.as_deref() {
        let got = req.get("token").and_then(Value::as_str).unwrap_or("");
        if got != expected {
            return json!({"ok": false, "error": "unauthorized"});
        }
    }
    let tools = state.tools.read().map(|g| g.clone()).unwrap_or_default();
    let action = req.get("action").and_then(Value::as_str).unwrap_or("");
    match action {
        "list" => json!({"ok": true, "text": kit::list_text(&tools)}),
        "search" => {
            let query = req.get("target").and_then(Value::as_str).unwrap_or("");
            let limit = req
                .get("limit")
                .and_then(Value::as_u64)
                .map(|n| n as usize)
                .unwrap_or(10);
            json!({"ok": true, "text": kit::search_text(&tools, query, limit)})
        }
        "help" | "open" => {
            let argv: Vec<String> = req
                .get("argv")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_else(|| {
                    req.get("target")
                        .and_then(Value::as_str)
                        .map(|s| vec![s.to_string()])
                        .unwrap_or_default()
                });
            let Some((name, _)) = kit::resolve_target(&tools, &argv) else {
                let label = argv.join(" ");
                return json!({"ok": false, "error": kit::unknown_tool_message(&label)});
            };
            match tools.iter().find(|t| t.name == name) {
                Some(def) => json!({"ok": true, "text": kit::format_help(def)}),
                None => json!({"ok": false, "error": kit::unknown_tool_message(&name)}),
            }
        }
        "run" => {
            let argv: Vec<String> = req
                .get("argv")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let (name, args) = if !argv.is_empty() {
                let Some((resolved, consumed)) = kit::resolve_target(&tools, &argv) else {
                    return json!({"ok": false, "error": kit::unknown_tool_message(&argv[0])});
                };
                let Some(def) = tools.iter().find(|t| t.name == resolved) else {
                    return json!({"ok": false, "error": kit::unknown_tool_message(&resolved)});
                };
                let stdin_json = req.get("stdin").and_then(Value::as_str);
                match kit::parse_cli_args(def, &argv[consumed..], stdin_json) {
                    Ok(v) => (resolved, v),
                    Err(e) => return json!({"ok": false, "error": e}),
                }
            } else {
                let target = req.get("target").and_then(Value::as_str).unwrap_or("");
                if !tools.iter().any(|t| t.name == target) {
                    return json!({"ok": false, "error": kit::unknown_tool_message(target)});
                }
                let args = req
                    .get("args")
                    .cloned()
                    .unwrap_or_else(|| Value::Object(Default::default()));
                (target.to_string(), args)
            };

            if name == "bash" || kit::is_kit_name(&name) {
                return json!({"ok": false, "error": format!("kit: cannot recurse into '{name}'")});
            }

            let Some(ctx) = state.ctx.lock().ok().and_then(|g| g.clone()) else {
                return json!({"ok": false, "error": "kit: no active session turn"});
            };

            let hooks = state.hooks.read().map(|g| g.clone()).unwrap_or_default();
            let mut effective_args = args;
            for hook in &hooks {
                match hook.tool_before(&name, &effective_args).await {
                    ToolBefore::Allow => {}
                    ToolBefore::Modify(rewritten) => effective_args = rewritten,
                    ToolBefore::Deny(reason) => {
                        return json!({"ok": false, "is_error": true, "content": reason});
                    }
                    ToolBefore::Cancel(reason) => {
                        ctx.cancel.cancel();
                        return json!({"ok": false, "is_error": true, "content": reason});
                    }
                }
            }
            for hook in &hooks {
                hook.pre_tool(&name, &effective_args).await;
            }

            let call_id = format!("kit-{}", CALL_SEQ.fetch_add(1, Ordering::Relaxed) + 1);
            let tx = state.events.lock().ok().and_then(|g| g.clone());
            if let Some(ref tx) = tx {
                let _ = tx.send(AgentEvent::tool_call_start(call_id.clone(), name.clone()));
                let _ = tx.send(AgentEvent::tool_call_end(
                    call_id.clone(),
                    effective_args.clone(),
                ));
            }

            let out = state
                .executor
                .execute(&ctx, &name, effective_args.clone())
                .await;

            for hook in &hooks {
                hook.post_tool(&name, &out).await;
            }
            if let Some(ref tx) = tx {
                let _ = tx.send(AgentEvent::tool_result(
                    call_id,
                    out.content.clone(),
                    out.is_error,
                ));
            }
            json!({
                "ok": !out.is_error,
                "content": out.content,
                "is_error": out.is_error,
            })
        }
        other => json!({"ok": false, "error": format!("kit: unknown action '{other}'")}),
    }
}
