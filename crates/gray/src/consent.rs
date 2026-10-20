//! First-use tool consent for interactive sessions.
//!
//! In the interactive TUI the first invocation of each tool pauses for
//! user consent — run once, allow the tool for the rest of the session,
//! or deny. The gate only engages when the ask service was installed
//! with a live TUI; print mode, piped stdin and CI keep the previous
//! auto-allow behavior.

use std::collections::HashSet;
use std::sync::Arc;

use futures::future::BoxFuture;
use gray_core::agent::{BackgroundJob, ToolContext, ToolExecutor, ToolOutput};
use gray_core::message::ToolDef;

const RUN_ONCE: &str = "Run once";
const ALLOW_SESSION: &str = "Allow for this session";
const DENY: &str = "Deny";

enum Verdict {
    Once,
    Session,
    Deny,
}

pub fn executor_wrap() -> gray_plugin::builder::ExecutorWrap {
    Box::new(|inner| Arc::new(ConsentExecutor::new(inner)))
}

struct ConsentExecutor {
    inner: Arc<dyn ToolExecutor>,
    allowed: Arc<tokio::sync::Mutex<HashSet<String>>>,
}

impl ConsentExecutor {
    fn new(inner: Arc<dyn ToolExecutor>) -> Self {
        Self {
            inner,
            allowed: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
        }
    }
}

fn describe(name: &str, args: &serde_json::Value) -> String {
    let detail = match name {
        "bash" | "shell" => args.get("command"),
        "read" | "write" | "edit" | "ls" => args.get("path").or_else(|| args.get("file_path")),
        "grep" | "find" | "search" => args.get("pattern").or_else(|| args.get("path")),
        "web_search" => args.get("query"),
        "web_fetch" => args.get("url"),
        _ => args
            .get("path")
            .or_else(|| args.get("command"))
            .or_else(|| args.get("query"))
            .or_else(|| args.get("url")),
    }
    .and_then(|v| v.as_str())
    .map(str::trim)
    .filter(|s| !s.is_empty())
    .map(|s| crate::repl::format::truncate_chars(s.lines().next().unwrap_or(s), 120).to_string());
    match detail {
        Some(d) => format!("{name}: {d}"),
        None => name.to_string(),
    }
}

async fn ask_consent(name: &str, args: &serde_json::Value) -> Verdict {
    let question = format!("agent wants to run {}", describe(name, args));
    let result = crate::ask::handle_ask(serde_json::json!({
        "questions": [{
            "id": "consent",
            "header": format!("Allow {name}?"),
            "question": question,
            "options": [
                {"label": RUN_ONCE, "description": "run this call only"},
                {"label": ALLOW_SESSION, "description": format!("allow {name} for the rest of this session")},
                {"label": DENY, "description": "refuse this call"},
            ],
        }],
        "blocking": true,
    }))
    .await;
    let picked = result
        .pointer("/answers/consent/answers")
        .and_then(|v| v.as_array())
        .and_then(|answers| {
            answers
                .iter()
                .filter_map(|a| a.as_str())
                .find(|a| !a.starts_with("user_note: "))
                .map(str::to_string)
        });
    match picked.as_deref() {
        Some(RUN_ONCE) => Verdict::Once,
        Some(ALLOW_SESSION) => Verdict::Session,
        _ => Verdict::Deny,
    }
}

impl ToolExecutor for ConsentExecutor {
    fn drain_notifications(&self, ctx: &ToolContext) -> Vec<String> {
        self.inner.drain_notifications(ctx)
    }

    fn live_defs(&self) -> Option<Vec<ToolDef>> {
        self.inner.live_defs()
    }

    fn wait_for_notification(
        &self,
        ctx: &ToolContext,
        timeout: std::time::Duration,
    ) -> BoxFuture<'static, Option<()>> {
        self.inner.wait_for_notification(ctx, timeout)
    }

    fn has_pending_background(&self, ctx: &ToolContext) -> bool {
        self.inner.has_pending_background(ctx)
    }

    fn repin_session(&self, id: &str) {
        self.inner.repin_session(id);
    }

    fn background_jobs(&self, ctx: &ToolContext) -> Vec<BackgroundJob> {
        self.inner.background_jobs(ctx)
    }

    fn running_jobs_note(&self, ctx: &ToolContext) -> Option<String> {
        self.inner.running_jobs_note(ctx)
    }

    fn cancel_background(&self, ctx: &ToolContext, id: &str) -> bool {
        self.inner.cancel_background(ctx, id)
    }

    fn execute(
        &self,
        ctx: &ToolContext,
        name: &str,
        args: serde_json::Value,
    ) -> BoxFuture<'static, ToolOutput> {
        let inner = self.inner.clone();
        let allowed = self.allowed.clone();
        let ctx = ctx.clone();
        let name = name.to_string();
        Box::pin(async move {
            if crate::ask::interactive_tui() {
                // The lock is held across the prompt so parallel tool calls
                // queue on it instead of stacking modals.
                let mut allowed = allowed.lock().await;
                if !allowed.contains(&name) {
                    match ask_consent(&name, &args).await {
                        Verdict::Session => {
                            allowed.insert(name.clone());
                        }
                        Verdict::Once => {}
                        Verdict::Deny => {
                            return ToolOutput::error(format!(
                                "the user declined the {name} tool call"
                            ));
                        }
                    }
                }
            }
            inner.execute(&ctx, &name, args).await
        })
    }
}
