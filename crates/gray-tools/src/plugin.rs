use std::sync::Arc;

use gray_core::agent::Tool;
use gray_plugin::{Manifest, Plugin};

use crate::{
    BashTool, EditTool, FindTool, GrepTool, LsTool, ReadTool, RequestUserInputTool,
    REQUEST_USER_INPUT_TOOL_NAME, SkillTool, WriteTool,
};

pub struct ToolsBasicPlugin;

impl Plugin for ToolsBasicPlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: "tools-basic".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            tools: vec![
                "read".to_string(),
                "write".to_string(),
                "edit".to_string(),
                "bash".to_string(),
                "skill".to_string(),
                REQUEST_USER_INPUT_TOOL_NAME.to_string(),
            ],
            provider: None,
        }
    }

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![
            Arc::new(ReadTool),
            Arc::new(WriteTool),
            Arc::new(EditTool),
            Arc::new(BashTool),
            Arc::new(SkillTool),
            Arc::new(RequestUserInputTool),
        ]
    }
}

pub struct ToolsSearchPlugin;

impl Plugin for ToolsSearchPlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: "tools-search".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            tools: vec!["grep".to_string(), "find".to_string(), "ls".to_string()],
            provider: None,
        }
    }

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![Arc::new(GrepTool), Arc::new(FindTool), Arc::new(LsTool)]
    }
}
