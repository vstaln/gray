use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use gray_core::agent::{Tool, ToolContext, ToolExecutor, ToolOutput};
use gray_core::message::ToolDef;
use gray_tools::Registry;
use serde_json::{Value, json};

use super::LiveRegistry;
use crate::{Manifest, Plugin};

struct NamedTool(&'static str);

#[async_trait]
impl Tool for NamedTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(self.0, "t", json!({"type":"object","properties":{}}))
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(&self, _ctx: &ToolContext, _args: Value) -> ToolOutput {
        ToolOutput::ok(format!("ran {}", self.0))
    }
}

type SharedTools = Arc<RwLock<Vec<Arc<dyn Tool>>>>;

struct FakePlugin {
    tools: SharedTools,
}

#[async_trait]
impl Plugin for FakePlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: "fake".into(),
            protocol: Some("1.3".into()),
            ..Default::default()
        }
    }
    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.read().unwrap().clone()
    }
}

fn setup() -> (LiveRegistry, SharedTools) {
    let live: SharedTools = Arc::new(RwLock::new(vec![
        Arc::new(NamedTool("live_t")),
        Arc::new(NamedTool("static_t")),
    ]));
    let static_ = Arc::new(Registry::new(vec![Arc::new(NamedTool("static_t"))]));
    let reg = LiveRegistry::new(
        static_,
        vec![Arc::new(FakePlugin {
            tools: live.clone(),
        })],
    );
    (reg, live)
}

fn names(reg: &LiveRegistry) -> Vec<String> {
    reg.live_defs()
        .unwrap()
        .into_iter()
        .map(|d| d.name)
        .collect()
}

#[test]
fn live_defs_lists_static_then_live_and_drops_duplicates() {
    let (reg, _) = setup();
    assert_eq!(names(&reg), vec!["static_t", "live_t"]);
}

#[tokio::test]
async fn execute_reaches_live_tool_and_static_tool() {
    let (reg, _) = setup();
    let out = reg
        .execute(&ToolContext::default(), "live_t", json!({}))
        .await;
    assert!(!out.is_error);
    assert_eq!(out.content, "ran live_t");
    let out = reg
        .execute(&ToolContext::default(), "static_t", json!({}))
        .await;
    assert_eq!(out.content, "ran static_t");
}

#[test]
fn live_defs_reflect_plugin_changes_without_rebuild() {
    let (reg, live) = setup();
    live.write().unwrap().push(Arc::new(NamedTool("live_u")));
    assert_eq!(names(&reg), vec!["static_t", "live_t", "live_u"]);
    live.write().unwrap().clear();
    assert_eq!(names(&reg), vec!["static_t"]);
}

#[tokio::test]
async fn unknown_tool_is_an_error() {
    let (reg, _) = setup();
    let out = reg
        .execute(&ToolContext::default(), "nope", json!({}))
        .await;
    assert!(out.is_error, "got: {}", out.content);
    assert!(
        out.content.contains("does not exist"),
        "got: {}",
        out.content
    );
}
