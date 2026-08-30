//! Cron tool — steal from hermes model_tools.py + hermes-cron CreateJobArgs
//! Lets AI self-schedule timed actions: `schedule_task(schedule, prompt)`

use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::{json, Value};

use crate::{fail, get_str, Tool};

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
            .unwrap_or_else(|| format!("job-{}", &prompt.chars().take(12).collect::<String>()));

        // Validate via gray-cron parser (hermes compatible)
        if let Err(e) = gray_cron::parse_schedule(&schedule) {
            return fail(format!("invalid schedule '{schedule}': {e}"));
        }

        match gray_cron::create_job(name.clone(), schedule.clone(), prompt.clone()) {
            Ok(job) => {
                let next = job.next_run.map(|t| t.to_string()).unwrap_or_else(|| "-".to_string());
                let display = gray_cron::parse_schedule(&schedule)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> ToolContext {
        ToolContext {
            cwd: std::env::temp_dir(),
            cancel: Default::default(),
        }
    }

    #[tokio::test]
    async fn creates_one_shot() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = TempHome::set(dir.path());
        let out = CronTool
            .execute(&ctx(), json!({"schedule": "in 10m", "prompt": "test one-shot"}))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("scheduled"));
        assert!(out.content.contains("in 10m") || out.content.contains("once"));
        // cleanup
        if let Some(j) = gray_cron::list_jobs().into_iter().find(|j| j.prompt == "test one-shot") {
            let _ = gray_cron::remove_job(&j.id);
        }
    }

    #[tokio::test]
    async fn creates_recurring() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = TempHome::set(dir.path());
        let out = CronTool
            .execute(&ctx(), json!({"schedule": "every 10m", "prompt": "test recurring"}))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("every 10m"));
        if let Some(j) = gray_cron::list_jobs().into_iter().find(|j| j.prompt == "test recurring") {
            let _ = gray_cron::remove_job(&j.id);
        }
    }

    #[tokio::test]
    async fn rejects_bad_schedule() {
        let out = CronTool
            .execute(&ctx(), json!({"schedule": "not a schedule", "prompt": "hi"}))
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("invalid schedule"));
    }

    struct TempHome {
        prev: Option<String>,
    }
    impl TempHome {
        fn set(p: &std::path::Path) -> Self {
            let prev = std::env::var("GRAY_HOME").ok();
            unsafe { std::env::set_var("GRAY_HOME", p) };
            Self { prev }
        }
    }
    impl Drop for TempHome {
        fn drop(&mut self) {
            unsafe {
                if let Some(v) = &self.prev {
                    std::env::set_var("GRAY_HOME", v);
                } else {
                    std::env::remove_var("GRAY_HOME");
                }
            }
        }
    }
}
