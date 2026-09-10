//! shell/tools/shell_kill.rs — kill a task, pid, or port (brief 2D).
//!
//! Exactly one of `task_id | pid | port`, else fail. Task kills signal the
//! group we created; foreign pid/port kills prompt first (fail-closed) and
//! signal the single pid only. (Registration in `plugin.rs` is 2E's.)

use async_trait::async_trait;
use gray_core::agent::{Tool, ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::{Value, json};

use crate::shell::contract::{KillTarget, TaskId};
use crate::{fail, get_opt_u64};

pub const SHELL_KILL_SNIPPET: &str =
    "Stop a task: shell_kill(task_id). Port in use: shell_kill(port=N).";

pub struct ShellKillTool;

#[async_trait]
impl Tool for ShellKillTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "shell_kill",
            "Kill a background task, a pid, or whatever listens on a port. \
             Give exactly one of task_id, pid, port. \
             Foreign processes ask the user first and fail closed.",
            json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "Task id (tN) from bash background start"},
                    "pid": {"type": "integer", "description": "Process id (foreign pids need approval)"},
                    "port": {"type": "integer", "description": "TCP port; kills its listener (foreign needs approval)"}
                }
            }),
        )
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(SHELL_KILL_SNIPPET)
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let has_task = args.get("task_id").is_some_and(|v| !v.is_null());
        let has_pid = args.get("pid").is_some_and(|v| !v.is_null());
        let has_port = args.get("port").is_some_and(|v| !v.is_null());
        if [has_task, has_pid, has_port].iter().filter(|&&b| b).count() != 1 {
            return fail("give exactly one of task_id, pid, port".to_string());
        }
        let target = if has_task {
            match parse_task_id(args.get("task_id")) {
                Some(id) => KillTarget::Task(id),
                None => return fail("task_id must look like \"t4\"".to_string()),
            }
        } else if has_pid {
            match get_opt_u64(&args, "pid") {
                Ok(Some(p)) if p > 0 && p <= u32::MAX as u64 => KillTarget::Pid(p as u32),
                _ => return fail("pid must be a positive integer".to_string()),
            }
        } else {
            match get_opt_u64(&args, "port") {
                Ok(Some(p)) if (1..=65535).contains(&p) => KillTarget::Port(p as u16),
                _ => return fail("port must be 1..=65535".to_string()),
            }
        };
        let session = ctx.session_id.clone().unwrap_or_else(|| "nosession".into());
        // Chosen fix for session-scoped tasks: keep scoping (frozen contract),
        // make errors unambiguous by naming sessions and where the id lives.
        let task_id = match &target {
            KillTarget::Task(id) => Some(*id),
            _ => None,
        };
        match super::super::kill::kill(target, &session, ctx).await {
            Ok(rep) => ToolOutput::ok(rep.describe),
            Err(e) => {
                if e.contains("unknown task")
                    && let Some(id) = task_id
                {
                    let others: Vec<String> = crate::shell::registry::registry()
                        .sessions_with_task(id)
                        .into_iter()
                        .filter(|s| s != &session)
                        .collect();
                    if !others.is_empty() {
                        return fail(format!(
                            "{e} in session \"{session}\". Exists in session-scoped session(s): {}. Tasks are session-scoped.",
                            others.join(", ")
                        ));
                    }
                    return fail(format!("{e} in session \"{session}\""));
                }
                fail(e)
            }
        }
    }
}

fn parse_task_id(v: Option<&Value>) -> Option<TaskId> {
    let s = match v {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => return None,
    };
    let s = s
        .strip_prefix('t')
        .or_else(|| s.strip_prefix('T'))
        .unwrap_or(&s);
    s.parse::<u32>().ok().map(TaskId)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gray_core::agent::ToolContext;

    #[tokio::test]
    async fn requires_exactly_one_target() {
        let ctx = ToolContext::default();
        assert!(ShellKillTool.execute(&ctx, json!({})).await.is_error);
        assert!(
            ShellKillTool
                .execute(&ctx, json!({"task_id": "t1", "pid": 123}))
                .await
                .is_error
        );
        let out = ShellKillTool.execute(&ctx, json!({})).await;
        assert!(out.content.contains("exactly one"), "{}", out.content);
    }

    #[tokio::test]
    async fn rejects_bad_shapes() {
        let ctx = ToolContext::default();
        assert!(
            ShellKillTool
                .execute(&ctx, json!({"task_id": "zzz"}))
                .await
                .is_error
        );
        assert!(
            ShellKillTool
                .execute(&ctx, json!({"pid": 0}))
                .await
                .is_error
        );
        assert!(
            ShellKillTool
                .execute(&ctx, json!({"port": 99999}))
                .await
                .is_error
        );
    }

    #[tokio::test]
    async fn unknown_task_fails_open_with_ids() {
        let ctx = ToolContext {
            session_id: Some(format!("killtool-{}-unknown", std::process::id())),
            ..ToolContext::default()
        };
        let out = ShellKillTool
            .execute(&ctx, json!({"task_id": "t999"}))
            .await;
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("unknown task"), "{}", out.content);
    }

    #[test]
    fn task_id_shapes() {
        assert_eq!(parse_task_id(Some(&json!("t4"))), Some(TaskId(4)));
        assert_eq!(parse_task_id(Some(&json!("T12"))), Some(TaskId(12)));
        assert_eq!(parse_task_id(Some(&json!(7))), Some(TaskId(7)));
        assert_eq!(parse_task_id(Some(&json!("zzz"))), None);
        assert_eq!(parse_task_id(None), None);
    }

    #[tokio::test]
    async fn cross_session_kill_error_names_other_session() {
        // Bug 3: session-scoped by design; error must say where tN actually lives.
        use crate::shell::tools::bash::BashTool;
        use gray_core::agent::Tool;
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let tag = N.fetch_add(1, Ordering::Relaxed);
        let sess_a = format!("kill-cross-a-{}-{tag}", std::process::id());
        let sess_b = format!("kill-cross-b-{}-{tag}", std::process::id());
        let ctx_a = ToolContext {
            session_id: Some(sess_a.clone()),
            ..ToolContext::default()
        };
        let bg = BashTool
            .execute(&ctx_a, json!({"command": "sleep 30", "background": true}))
            .await;
        assert!(!bg.is_error, "{}", bg.content);
        let ctx_b = ToolContext {
            session_id: Some(sess_b.clone()),
            ..ToolContext::default()
        };
        let out = ShellKillTool
            .execute(&ctx_b, json!({"task_id": "t1"}))
            .await;
        assert!(
            out.is_error,
            "cross-session kill must stay scoped, got {}",
            out.content
        );
        assert!(out.content.contains("unknown task"), "{}", out.content);
        assert!(
            out.content.contains(&sess_a) || out.content.contains("session-scoped"),
            "error must name where t1 lives (session {sess_a}), got {}",
            out.content
        );
        // Cleanup: kill from the owning session.
        let _ = ShellKillTool
            .execute(&ctx_a, json!({"task_id": "t1"}))
            .await;
    }
}
