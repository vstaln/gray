//! Built-in `pulse` tool: manage the 24/7 standing goal from the default
//! profile, without the sidecar plugin.

use async_trait::async_trait;
use gray_core::agent::{Tool, ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::Value;

use crate::pulse::plugin;

pub struct PulseTool;

#[async_trait]
impl Tool for PulseTool {
    fn def(&self) -> ToolDef {
        ToolDef::new("pulse", plugin::TOOL_DESCRIPTION, plugin::tool_parameters())
    }

    async fn execute(&self, _ctx: &ToolContext, args: Value) -> ToolOutput {
        match plugin::run_action(&args) {
            Ok(content) => ToolOutput::ok(content),
            Err(e) => ToolOutput::error(format!("{e:#}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn def_matches_the_shared_pulse_schema() {
        let def = PulseTool.def();
        assert_eq!(def.name, "pulse");
        assert_eq!(def.parameters, plugin::tool_parameters());
        assert_eq!(
            def.parameters["properties"]["action"]["enum"],
            json!(["status", "goal_get", "goal_set", "on", "off", "sync"])
        );
    }
}
