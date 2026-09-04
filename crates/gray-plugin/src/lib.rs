use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use serde_json::Value;

use gray_core::agent::ToolOutput;
use gray_core::event::Usage;
use gray_core::message::Message;

pub mod profile;
pub mod sidecar;

#[derive(Debug, Clone)]
pub enum CoreEvent {
    PreStep { messages: Vec<Message> },
    PreTool { name: String, args: Value },
    PostTool { name: String, output: ToolOutput },
    TurnEnd { usage: Usage },
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub tools: Vec<String>,
    pub provider: Option<String>,
}

#[async_trait]
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> Manifest;
    fn tools(&self) -> Vec<Arc<dyn gray_core::agent::Tool>> {
        vec![]
    }
    // NOTE (ponytail-audit #6): an earlier `provider()` hook was deleted — every
    // impl returned None and nothing called it. `on_event`/`CoreEvent` stay:
    // SidecarPlugin dispatches them to the subprocess over stdio.
    async fn on_event(&self, _e: CoreEvent) -> Option<CoreEvent> {
        None
    }
}

/// Later manifests win on tool-name conflict. Returns owner per tool name.
pub fn merge_manifests(manifests: Vec<Manifest>) -> HashMap<String, String> {
    let mut owner = HashMap::new();
    for m in manifests {
        for t in &m.tools {
            owner.insert(t.clone(), m.name.clone());
        }
    }
    owner
}
