use std::sync::Arc;

use gray_core::agent::{ToolContext, ToolExecutor, ToolOutput};
use gray_core::message::ToolDef;
use gray_tools::Registry;
use serde_json::Value;

use crate::Plugin;

type BoxFut<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'static>>;

pub struct LiveRegistry {
    static_: Arc<Registry>,
    live: Vec<Arc<dyn Plugin>>,
}

impl LiveRegistry {
    pub fn new(static_: Arc<Registry>, live: Vec<Arc<dyn Plugin>>) -> Self {
        Self { static_, live }
    }

    fn find_live(&self, name: &str) -> Option<Arc<dyn gray_core::agent::Tool>> {
        self.live
            .iter()
            .flat_map(|p| p.tools())
            .find(|t| t.def().name == name)
    }
}

impl ToolExecutor for LiveRegistry {
    fn drain_notifications(&self, ctx: &ToolContext) -> Vec<String> {
        self.static_.drain_notifications(ctx)
    }

    fn repin_session(&self, id: &str) {
        for p in &self.live {
            p.set_session_id(id);
        }
    }

    fn wait_for_notification(
        &self,
        ctx: &ToolContext,
        timeout: std::time::Duration,
    ) -> BoxFut<Option<()>> {
        self.static_.wait_for_notification(ctx, timeout)
    }

    fn has_pending_background(&self, ctx: &ToolContext) -> bool {
        self.static_.has_pending_background(ctx)
    }

    fn background_jobs(&self, ctx: &ToolContext) -> Vec<gray_core::agent::BackgroundJob> {
        self.static_.background_jobs(ctx)
    }

    fn cancel_background(&self, ctx: &ToolContext, id: &str) -> bool {
        self.static_.cancel_background(ctx, id)
    }

    fn running_jobs_note(&self, ctx: &ToolContext) -> Option<String> {
        self.static_.running_jobs_note(ctx)
    }

    fn live_defs(&self) -> Option<Vec<ToolDef>> {
        let mut defs = self.static_.defs();
        for p in &self.live {
            for t in p.tools() {
                let def = t.def();
                if !defs.iter().any(|d| d.name == def.name) {
                    defs.push(def);
                }
            }
        }
        Some(defs)
    }

    fn execute(&self, ctx: &ToolContext, name: &str, args: Value) -> BoxFut<ToolOutput> {
        if self.static_.tool_names().iter().any(|n| n == name) {
            return self.static_.execute(ctx, name, args);
        }
        match self.find_live(name) {
            Some(tool) => {
                let ctx = ctx.clone();
                let name = name.to_string();
                Box::pin(async move {
                    log::info!(target: "gray_tools", "live tool start: {name}");
                    tool.execute(&ctx, args).await
                })
            }
            None => self.static_.execute(ctx, name, args),
        }
    }
}
