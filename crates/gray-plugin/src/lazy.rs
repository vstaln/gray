use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;
use serde_json::Value;

use gray_core::agent::{Tool, ToolContext, ToolOutput};
use gray_core::message::{Message, ToolDef};

use crate::sidecar::HostHandler;
use crate::{CommandOutcome, CoreEvent, InputSubmit, Manifest, Plugin, SidecarPlugin, ToolBefore};

pub fn lazy_enabled() -> bool {
    match std::env::var("GRAY_LAZY_PLUGINS") {
        Ok(v) => !matches!(v.as_str(), "0" | "false" | "off" | "no"),
        Err(_) => true,
    }
}

pub fn cached_manifest(path: &std::path::Path, lock_name: &str) -> Option<Manifest> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let m = Manifest::from_result(&v);
    if m.name != lock_name || m.protocol.as_deref() == Some(crate::DYNAMIC_PROTOCOL) {
        return None;
    }
    Some(m)
}

pub struct LazySidecarPlugin {
    argv: Vec<String>,
    manifest: Manifest,
    live: tokio::sync::OnceCell<SidecarPlugin>,
    capabilities: Mutex<Vec<String>>,
    host_handler: Mutex<Option<HostHandler>>,
    session_id: Mutex<String>,

    global: std::sync::atomic::AtomicBool,
    self_ref: Mutex<Weak<LazySidecarPlugin>>,
}

impl LazySidecarPlugin {
    pub fn new(argv: Vec<String>, manifest: Manifest) -> Self {
        Self {
            argv,
            manifest,
            live: tokio::sync::OnceCell::new(),
            capabilities: Mutex::new(Vec::new()),
            host_handler: Mutex::new(None),
            session_id: Mutex::new(String::new()),
            global: std::sync::atomic::AtomicBool::new(false),
            self_ref: Mutex::new(Weak::new()),
        }
    }

    pub fn arc(self) -> Arc<Self> {
        let arc = Arc::new(self);
        *arc.self_ref.lock().unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(&arc);
        arc
    }

    async fn live(&self) -> Result<&SidecarPlugin, String> {
        self.live
            .get_or_try_init(|| async {
                let plugin = SidecarPlugin::spawn(self.argv.clone())
                    .await
                    .map_err(|e| e.to_string())?;

                let caps = self
                    .capabilities
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                plugin.set_capabilities(caps);
                let h = self
                    .host_handler
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                if let Some(h) = h {
                    plugin.set_host_handler(h).await;
                }
                let sid = self
                    .session_id
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                if !sid.is_empty() {
                    plugin.set_session_id(&sid);
                }
                Ok(plugin)
            })
            .await
    }

    fn claims(&self, hook: &str) -> bool {
        self.manifest.hooks.iter().any(|h| h == hook)
    }

    pub fn set_capabilities(&self, granted: Vec<String>) {
        *self.capabilities.lock().unwrap_or_else(|e| e.into_inner()) = granted;
    }

    pub async fn set_host_handler(&self, handler: HostHandler) {
        if let Some(p) = self.live.get() {
            p.set_host_handler(handler).await;
        } else {
            *self.host_handler.lock().unwrap_or_else(|e| e.into_inner()) = Some(handler);
        }
    }
}

#[async_trait]
impl Plugin for LazySidecarPlugin {
    fn manifest(&self) -> Manifest {
        self.manifest.clone()
    }

    fn capabilities(&self) -> Vec<String> {
        self.capabilities
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        let me = self
            .self_ref
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        self.manifest
            .tools
            .iter()
            .map(|def| {
                Arc::new(LazyTool {
                    def: def.clone(),
                    owner: me.clone(),
                }) as Arc<dyn Tool>
            })
            .collect()
    }

    fn set_session_id(&self, id: &str) {
        *self.session_id.lock().unwrap_or_else(|e| e.into_inner()) = id.to_string();
        if let Some(p) = self.live.get() {
            p.set_session_id(id);
        }
    }

    fn is_global(&self) -> bool {
        self.global.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn set_global(&self, global: bool) {
        self.global
            .store(global, std::sync::atomic::Ordering::Relaxed);
    }

    async fn prompt_context(&self, cwd: &str) -> Option<String> {
        if !self.claims("prompt/context") {
            return None;
        }
        self.live().await.ok()?.prompt_context(cwd).await
    }

    async fn compact_before(&self) {
        if !self.claims("compact/before") {
            return;
        }
        if let Ok(p) = self.live().await {
            p.compact_before().await
        }
    }

    async fn tool_before(&self, name: &str, args: &Value) -> ToolBefore {
        if !self.claims("tool/before") {
            return ToolBefore::Allow;
        }
        match self.live().await {
            Ok(p) => p.tool_before(name, args).await,

            Err(e) => {
                log::warn!(target: "gray_plugin", "lazy sidecar {} spawn for tool/before failed: {e}", self.manifest.name);
                ToolBefore::Deny(format!(
                    "plugin '{}' policy check failed to start ({:.160}); tool not executed — retry may succeed",
                    self.manifest.name,
                    e.to_string()
                ))
            }
        }
    }

    async fn input_submit(&self, text: &str) -> InputSubmit {
        if !self.claims("input/submit") {
            return InputSubmit::Pass;
        }
        match self.live().await {
            Ok(p) => p.input_submit(text).await,
            Err(_) => InputSubmit::Pass,
        }
    }

    async fn agent_before_start(&self) -> Option<String> {
        if !self.claims("agent/before_start") {
            return None;
        }
        self.live().await.ok()?.agent_before_start().await
    }

    async fn context_build(&self, messages: &[Message]) -> Option<Vec<Message>> {
        if !self.claims("context/build") {
            return None;
        }
        self.live().await.ok()?.context_build(messages).await
    }

    async fn tool_after(&self, name: &str, output: &ToolOutput) -> Option<ToolOutput> {
        if !self.claims("tool/after") {
            return None;
        }
        self.live().await.ok()?.tool_after(name, output).await
    }

    async fn run_command(
        &self,
        name: &str,
        argv: Vec<String>,
        raw: Option<String>,
    ) -> Option<CommandOutcome> {
        if !self.manifest.commands.iter().any(|c| c == name)
            && !self.manifest.subcommands.iter().any(|c| c == name)
        {
            return None;
        }
        match self.live().await {
            Ok(p) => p.run_command(name, argv, raw).await,
            Err(e) => Some(CommandOutcome::Say(format!(
                "plugin {} failed to start: {e}",
                self.manifest.name
            ))),
        }
    }

    async fn on_event(&self, e: CoreEvent) {
        if self.live.get().is_none() && self.manifest.hooks.is_empty() {
            return;
        }
        if let Ok(p) = self.live().await {
            p.on_event(e).await;
        }
    }

    async fn shutdown(&self) {
        if let Some(p) = self.live.get() {
            p.shutdown(std::time::Duration::from_secs(2)).await;
        }
    }
}

struct LazyTool {
    def: ToolDef,
    owner: Weak<LazySidecarPlugin>,
}

#[async_trait]
impl Tool for LazyTool {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn def(&self) -> ToolDef {
        self.def.clone()
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let Some(owner) = self.owner.upgrade() else {
            return ToolOutput::error(format!("plugin tool {} unloaded", self.def.name));
        };
        let name = self.def.name.clone();
        let plugin = match owner.live().await {
            Ok(p) => p,
            Err(e) => {
                return ToolOutput::error(format!(
                    "plugin {} failed to start for {name}: {e}",
                    owner.manifest.name
                ));
            }
        };
        let Some(tool) = plugin.tools().into_iter().find(|t| t.def().name == name) else {
            return ToolOutput::error(format!("plugin tool {name} not found after spawn"));
        };
        tool.execute(ctx, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest(tools: usize, hooks: &[&str], commands: &[&str]) -> Manifest {
        Manifest::from_result(&json!({
            "name": "demo",
            "version": "0.1",
            "protocol": "1.1",
            "tools": (0..tools).map(|i| json!({"name": format!("t{i}"), "description": "d", "parameters": {}})).collect::<Vec<_>>(),
            "hooks": hooks,
            "commands": commands,
        }))
    }

    #[test]
    fn cached_manifest_rejects_dynamic_and_mismatch() {
        let dir = std::env::temp_dir().join(format!("lazy-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("demo-manifest.json");
        std::fs::write(
            &p,
            r#"{"name":"demo","version":"0.1","protocol":"1.3","tools":[]}"#,
        )
        .unwrap();
        assert!(cached_manifest(&p, "demo").is_none(), "1.3 must stay eager");
        std::fs::write(
            &p,
            r#"{"name":"other","version":"0.1","protocol":"1.1","tools":[]}"#,
        )
        .unwrap();
        assert!(
            cached_manifest(&p, "demo").is_none(),
            "name mismatch = stale cache"
        );
        std::fs::write(&p, "not json").unwrap();
        assert!(cached_manifest(&p, "demo").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn dormant_calls_never_spawn() {
        let p = LazySidecarPlugin::new(
            vec!["/nonexistent/binary".into()],
            manifest(0, &[], &["/demo"]),
        )
        .arc();
        assert!(p.run_command("/other", vec![], None).await.is_none());
        assert!(p.prompt_context("/tmp").await.is_none());
        assert_eq!(p.input_submit("hi").await, crate::InputSubmit::Pass);
        p.on_event(CoreEvent::TurnEnd {
            usage: gray_core::event::Usage::default(),
        })
        .await;
        assert!(p.live.get().is_none(), "dormant plugin must not spawn");
        assert_eq!(p.manifest().name, "demo");
        assert_eq!(p.tools().len(), 0);
    }

    #[tokio::test]
    async fn unclaimed_command_and_events_skip_spawn() {
        let p = LazySidecarPlugin::new(
            vec!["/nonexistent/binary".into()],
            manifest(2, &["prompt/context"], &[]),
        )
        .arc();
        assert!(p.run_command("/nope", vec![], None).await.is_none());
        assert!(p.live.get().is_none());

        assert!(p.prompt_context("/tmp").await.is_none());
        assert!(p.live.get().is_none(), "failed spawn leaves cell empty");
        assert_eq!(p.tools().len(), 2);
    }
}
