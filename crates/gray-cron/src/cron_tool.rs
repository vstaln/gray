//! Cron tool: lets AI self-schedule timed actions: `schedule_task(schedule, prompt)`
//!
//! (Moved from `gray-tools` so `gray-tools` depends on `gray-core` only;
//! the tool lives with the job store it drives.)

use std::sync::Arc;

use async_trait::async_trait;
use gray_core::agent::{Tool, ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use gray_core::tool_out::{fail, get_str};
use serde_json::{Value, json};

use crate::{create_job, parse_schedule};

pub const CRON_SNIPPET: &str = "Schedule a timed action: `schedule_task(schedule, prompt)`";
pub const CRON_GUIDELINES: &[&str] = &[
    "Use schedule_task to schedule yourself: schedule like 'in 10m', 'every 30m', '0 9 * * *' (cron), or '2026-02-03T14:00'.",
    "One-shot: 'in 20m' or timestamp. Recurring: 'every 1h' or cron. Keep prompt short and actionable.",
];

pub struct CronTool;

#[async_trait]
impl Tool for CronTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "schedule_task",
            "Schedule a timed action for yourself. Creates a cron job that will re-inject the prompt. Use 'in 10m', 'every 30m', cron '0 9 * * *', or timestamp '2026-02-03T14:00'. One-shot if schedule is 'in X' or timestamp; recurring if 'every X' or cron.",
            json!({
                "type": "object",
                "properties": {
                    "schedule": {
                        "type": "string",
                        "description": "When to run: 'in 10m', 'every 30m', 'every 1h', cron '0 9 * * *', or ISO timestamp '2026-02-03T14:00:00Z'"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Prompt to re-inject when job fires — short, actionable"
                    },
                    "name": {
                        "type": "string",
                        "description": "Optional name for the job"
                    }
                },
                "required": ["schedule", "prompt"]
            }),
        )
    }

    fn prompt_snippet(&self) -> Option<&'static str> {
        Some(CRON_SNIPPET)
    }

    fn prompt_guidelines(&self) -> Option<&'static [&'static str]> {
        Some(CRON_GUIDELINES)
    }

    async fn execute(&self, _ctx: &ToolContext, args: Value) -> ToolOutput {
        let schedule = match get_str(&args, "schedule") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let prompt = match get_str(&args, "prompt") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("job-{}", prompt.chars().take(12).collect::<String>()));

        // Validate via the local parser
        if let Err(e) = parse_schedule(&schedule) {
            return fail(format!("invalid schedule '{schedule}': {e}"));
        }

        match create_job(name.clone(), schedule.clone(), prompt.clone()) {
            Ok(job) => {
                let next = job
                    .next_run
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "-".to_string());
                let display = parse_schedule(&schedule)
                    .map(|s| s.display())
                    .unwrap_or(schedule.clone());
                ToolOutput::ok(format!(
                    "scheduled '{name}' ({display}) — id {} — schedule: {} — next: {} — prompt: \"{}\" — manage with /cron list|remove",
                    job.id, job.schedule, next, prompt
                ))
            }
            Err(e) => fail(format!("schedule_task failed: {e}")),
        }
    }
}

pub struct CronPlugin;

impl gray_plugin::Plugin for CronPlugin {
    fn manifest(&self) -> gray_plugin::Manifest {
        let tools = self.tools();
        gray_plugin::Manifest {
            name: "cron".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            tools: tools.iter().map(|t| t.def()).collect(),
            commands: vec![],
            hooks: vec![],
            provider: None,
        }
    }

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![Arc::new(CronTool)]
    }
}
