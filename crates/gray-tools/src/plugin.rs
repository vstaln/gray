use std::sync::Arc;

use gray_core::agent::Tool;
use gray_plugin::{Manifest, Plugin};

use crate::{
    BashTool, CronTool, EditTool, FindTool, GrepTool, LsTool, ReadTool, RequestUserInputTool,
    SkillTool, WriteTool,
};

pub struct ToolsBasicPlugin;

impl Plugin for ToolsBasicPlugin {
    fn manifest(&self) -> Manifest {
        // Names derived from tools() so the two can't drift.
        let tools = self.tools();
        Manifest {
            name: "tools-basic".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            tools: tools.iter().map(|t| t.def().name.clone()).collect(),
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
        let tools = self.tools();
        Manifest {
            name: "tools-search".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            tools: tools.iter().map(|t| t.def().name.clone()).collect(),
            provider: None,
        }
    }

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![Arc::new(GrepTool), Arc::new(FindTool), Arc::new(LsTool)]
    }
}

pub struct CronPlugin;

impl Plugin for CronPlugin {
    fn manifest(&self) -> Manifest {
        let tools = self.tools();
        Manifest {
            name: "cron".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            tools: tools.iter().map(|t| t.def().name.clone()).collect(),
            provider: None,
        }
    }

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![Arc::new(CronTool)]
    }
}
