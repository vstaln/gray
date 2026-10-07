//! Live tool resolution for protocol-1.3 plugins.
//!
//! A static [`Registry`] snapshots every plugin's tools at build time. A
//! protocol-1.3 sidecar (MCP bridges and the like) changes its tool set
//! while the session runs, so its tools are not snapshotted: the
//! `LiveRegistry` wraps the static registry and resolves those plugins'
//! tools by name at call time, and reports the merged definitions through
//! [`ToolExecutor::live_defs`] so the agent re-advertises them every turn.

use std::sync::Arc;

use gray_core::agent::{ToolContext, ToolExecutor, ToolOutput};
use gray_core::message::ToolDef;
use gray_tools::Registry;
use serde_json::Value;

use crate::Plugin;

type BoxFut<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'static>>;

/// Static registry plus the plugins whose tools are resolved per call.
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

    /// Static defs first, then live ones; a live tool that collides with a
    /// static name is dropped (the static registry owns it, and `execute`
    /// routes that name there).
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
        // Static names go through the registry so its arg coercion and
        // metering keep applying; unknown names fall through to it too for
        // the uniform "does not exist" error.
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

#[path = "live_tests.rs"]
#[cfg(test)]
mod tests;
