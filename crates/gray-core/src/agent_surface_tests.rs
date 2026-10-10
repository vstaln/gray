// `Agent::with_tool_surface_of` — a model switch keeps the running plugins
// (executor, tools, hooks) and swaps only the model side.

use super::*;
use crate::message::Message;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Named(&'static str);

impl Provider for Named {
    fn stream(&self, _: ChatRequest) -> ProviderStream {
        Box::pin(futures::stream::empty())
    }

    fn model_id(&self) -> &str {
        self.0
    }
}

struct Exec(&'static str);

impl ToolExecutor for Exec {
    fn execute(
        &self,
        _ctx: &ToolContext,
        _name: &str,
        _args: serde_json::Value,
    ) -> futures::future::BoxFuture<'static, ToolOutput> {
        let tag = self.0;
        Box::pin(async move { ToolOutput::ok(tag) })
    }
}

struct Hook;

#[async_trait]
impl PluginHooks for Hook {}

fn tool(name: &str) -> ToolDef {
    ToolDef::new(name, "", serde_json::json!({"type": "object"}))
}

#[test]
fn model_switch_keeps_plugin_surface_and_new_model_side() {
    let rewrites = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&rewrites);
    let old = Agent::new(Box::new(Named("old-model")), Arc::new(Exec("old-exec")))
        .with_system("old system".to_string())
        .with_tools(vec![tool("bash"), tool("web_fetch")])
        .with_tool_labels(vec![("bash".to_string(), "Ran".to_string())])
        .with_hooks(vec![Arc::new(Hook) as Arc<dyn PluginHooks>])
        .with_history_rewrite_hook(Arc::new(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        }))
        .with_context_window(Some(100_000));
    let executor = old.executor_handle();
    let history = vec![Message::user("hi")];

    let fresh = Agent::new(Box::new(Named("new-model")), Arc::new(Exec("throwaway")))
        .with_system("new system".to_string())
        .with_context_window(Some(256_000))
        .with_tool_surface_of(old)
        .with_messages(history);

    // Plugin side carried over.
    assert!(Arc::ptr_eq(&fresh.executor_handle(), &executor));
    let names: Vec<_> = fresh.tool_defs().iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["bash", "web_fetch"]);
    assert_eq!(
        fresh.tool_labels.get("bash").map(String::as_str),
        Some("Ran")
    );
    assert_eq!(fresh.hooks().len(), 1);
    // The donor's rewrite hook is the one `with_messages` fires.
    assert_eq!(rewrites.load(Ordering::SeqCst), 1);
    // Model side is the new agent's.
    assert_eq!(fresh.provider.model_id(), "new-model");
    assert_eq!(fresh.system, "new system");
    assert_eq!(fresh.context_window, Some(256_000));
    assert_eq!(fresh.messages().len(), 1);
}
