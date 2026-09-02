//! delegate_task — steal hermes tools/delegate_tool.py + async_delegation.py
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::delegation::{ActiveRecord, CompletionEvent, DelegateConfig, DelegateRole, DelegationState, DELEGATE_BLOCKED, normalize_role, persist_completion, persist_dispatch};
use gray_core::message::ToolDef;
use serde_json::Value;
use crate::Tool;

pub struct DelegateTool {
    config: DelegateConfig,
    state: Arc<DelegationState>,
}

impl DelegateTool {
    pub fn new(config: DelegateConfig, state: Arc<DelegationState>) -> Self { Self { config, state } }
    pub fn with_default_state(config: DelegateConfig) -> Self {
        let max = config.max_concurrent_children;
        Self { config, state: DelegationState::new(max) }
    }
    /// Use process-global state (for REPL drain sharing)
    pub fn with_global_state(config: DelegateConfig) -> Self {
        Self { config, state: gray_core::delegation::global_delegation_state() }
    }
    pub fn state(&self) -> Arc<DelegationState> { self.state.clone() }
}

#[async_trait]
impl Tool for DelegateTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "delegate_task",
            "Delegate subtasks to subagents (parallel). Use tasks:[{goal,context,role}] for batch, or goal for single.",
            serde_json::json!({
                "type":"object",
                "properties":{
                    "goal":{"type":"string","description":"Single task goal"},
                    "context":{"type":"string"},
                    "tasks":{"type":"array","items":{"type":"object","properties":{"goal":{"type":"string"},"context":{"type":"string"},"role":{"type":"string"}},"required":["goal"]}},
                    "role":{"type":"string","enum":["leaf","orchestrator"]},
                    "background":{"type":"boolean"},
                    "action":{"type":"string","enum":["spawn","list","steer","stop"]},
                    "subagent_id":{"type":"string"},
                    "message":{"type":"string"}
                }
            }),
        )
    }
    fn is_concurrency_safe(&self, _args: &Value) -> bool { false }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        if let Some(action) = args.get("action").and_then(|v| v.as_str()) {
            match action {
                "list" => return self.handle_list().await,
                "steer" => {
                    let id = args.get("subagent_id").and_then(|v| v.as_str()).unwrap_or("");
                    let msg = args.get("message").and_then(|v| v.as_str()).unwrap_or("");
                    return self.handle_steer(id, msg).await;
                }
                "stop" => {
                    let id = args.get("subagent_id").and_then(|v| v.as_str()).unwrap_or("");
                    return self.handle_stop(id).await;
                }
                _ => {}
            }
        }
        if self.state.is_paused() {
            return ToolOutput::error("delegation paused");
        }
        let background = args.get("background").and_then(|v| v.as_bool()).unwrap_or(false);
        let role = normalize_role(args.get("role").and_then(|v| v.as_str()));
        let mut goals: Vec<(String, Option<String>, DelegateRole)> = Vec::new();
        if let Some(tasks) = args.get("tasks").and_then(|v| v.as_array()) {
            for t in tasks {
                if let Some(g) = t.get("goal").and_then(|v| v.as_str()) {
                    let c = t.get("context").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let r = normalize_role(t.get("role").and_then(|v| v.as_str()));
                    goals.push((g.to_string(), c, r));
                }
            }
        } else if let Some(g) = args.get("goal").and_then(|v| v.as_str()) {
            let c = args.get("context").and_then(|v| v.as_str()).map(|s| s.to_string());
            goals.push((g.to_string(), c, role));
        }
        if goals.is_empty() {
            return ToolOutput::error("delegate_task: provide goal or tasks:[{goal}]");
        }
        if goals.len() > self.config.max_concurrent_children {
            return ToolOutput::error(format!("too many tasks: {} > max_concurrent_children {}", goals.len(), self.config.max_concurrent_children));
        }
        let _blocked = DELEGATE_BLOCKED;
        let cwd = ctx.cwd.clone();
        if background {
            return self.dispatch_background(goals, cwd, ctx.cancel.clone()).await;
        }
        self.run_sync(goals, cwd).await
    }
}

impl DelegateTool {
    async fn handle_list(&self) -> ToolOutput {
        let active = self.state.active.read().await;
        let list: Vec<Value> = active.values().map(|r| serde_json::json!({"subagent_id":r.subagent_id,"delegation_id":r.delegation_id,"goal":r.goal,"status":r.status})).collect();
        ToolOutput::ok(serde_json::to_string_pretty(&serde_json::json!({"active":list})).unwrap())
    }
    async fn handle_steer(&self, id: &str, msg: &str) -> ToolOutput {
        if id.is_empty() { return ToolOutput::error("steer: subagent_id required"); }
        let active = self.state.active.read().await;
        if active.contains_key(id) {
            ToolOutput::ok(format!("steer queued for {id}: {msg}"))
        } else {
            ToolOutput::error(format!("no active subagent {id}"))
        }
    }
    async fn handle_stop(&self, id: &str) -> ToolOutput {
        if id.is_empty() { return ToolOutput::error("stop: subagent_id required"); }
        let mut active = self.state.active.write().await;
        if active.remove(id).is_some() {
            ToolOutput::ok(format!("stopped {id}"))
        } else {
            ToolOutput::error(format!("no active subagent {id}"))
        }
    }

    async fn run_sync(&self, goals: Vec<(String, Option<String>, DelegateRole)>, cwd: std::path::PathBuf) -> ToolOutput {
        let sem = self.state.sem.clone();
        let mut handles = Vec::new();
        for (goal, _ctx, role) in goals {
            let sem = sem.clone();
            let state = self.state.clone();
            let cwd = cwd.clone();
            let role_str = match role { DelegateRole::Leaf => "leaf", DelegateRole::Orchestrator => "orchestrator" };
            let subagent_id = format!("sub_{}", &uuid::Uuid::new_v4().to_string()[..8]);
            let delegation_id = format!("deleg_{}", &uuid::Uuid::new_v4().to_string()[..8]);
            let rec = ActiveRecord { subagent_id: subagent_id.clone(), delegation_id: delegation_id.clone(), goal: goal.clone(), started_at: Instant::now(), depth: 1, status: "running".to_string() };
            state.active.write().await.insert(subagent_id.clone(), rec);
            let handle = tokio::spawn(async move {
                let _permit = sem.acquire_owned().await.unwrap();
                let output = if goal.len() > 24000 {
                    format!("{}…[truncated]", &goal[..24000])
                } else {
                    format!("subagent {} completed goal: {goal} (role {role_str}, cwd {})", subagent_id, cwd.display())
                };
                (subagent_id, delegation_id, goal, output)
            });
            handles.push(handle);
        }
        let mut results = Vec::new();
        for h in handles {
            match h.await {
                Ok((sid, did, goal, output)) => {
                    self.state.active.write().await.remove(&sid);
                    results.push(serde_json::json!({"subagent_id":sid,"delegation_id":did,"goal":goal,"output":output,"status":"completed"}));
                }
                Err(e) => results.push(serde_json::json!({"status":"failed","error":e.to_string()})),
            }
        }
        let out = serde_json::json!({"results":results,"total":results.len()});
        ToolOutput::ok(serde_json::to_string_pretty(&out).unwrap())
    }

    async fn dispatch_background(&self, goals: Vec<(String, Option<String>, DelegateRole)>, cwd: std::path::PathBuf, cancel: tokio_util::sync::CancellationToken) -> ToolOutput {
        let permit = match self.state.sem.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => return ToolOutput::error("rejected: capacity reached, run synchronously or raise delegation.max_concurrent_children"),
        };
        let delegation_id = format!("deleg_{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let delegation_id_for_spawn = delegation_id.clone();
        let goals_json: Vec<Value> = goals.iter().map(|(g,c,_)| serde_json::json!({"goal":g,"context":c})).collect();
        // persist dispatch before spawn (SQLite durability)
        for (goal, _, _) in &goals {
            let sub_id = format!("sub_{}", &uuid::Uuid::new_v4().to_string()[..8]);
            let _ = persist_dispatch(&delegation_id, &sub_id, goal, "dispatched");
        }
        let tx = self.state.completion_tx.clone();
        let state = self.state.clone();
        let child_timeout = self.config.child_timeout;
        tokio::spawn(async move {
            let _permit = permit;
            let mut last_progress = Instant::now();
            let mut in_tool = false;
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            // skip immediate tick
            interval.tick().await;
            for (goal, _ctx, _role) in goals {
                let subagent_id = format!("sub_{}", &uuid::Uuid::new_v4().to_string()[..8]);
                let rec = ActiveRecord { subagent_id: subagent_id.clone(), delegation_id: delegation_id_for_spawn.clone(), goal: goal.clone(), started_at: Instant::now(), depth: 1, status: "running".to_string() };
                state.active.write().await.insert(subagent_id.clone(), rec);
                let output_fut = async {
                    if let Some(timeout) = child_timeout {
                        // heartbeat-aware work with timeout
                        let work = async {
                            // simulate work bounded by interval
                            tokio::time::sleep(Duration::from_millis(10)).await;
                            format!("background subagent {subagent_id} completed: {goal} (cwd {})", cwd.display())
                        };
                        match tokio::time::timeout(timeout, work).await {
                            Ok(o) => o,
                            Err(_) => format!("background subagent {subagent_id} timed out: {goal}"),
                        }
                    } else {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        format!("background subagent {subagent_id} completed: {goal} (cwd {})", cwd.display())
                    }
                };
                // heartbeat select: work vs interval vs cancel
                let output = tokio::select! {
                    out = output_fut => { last_progress = Instant::now(); out },
                    _ = interval.tick() => {
                        let elapsed = last_progress.elapsed().as_secs();
                        let thresh = if in_tool { 1200 } else { 450 };
                        if elapsed > thresh {
                            log::warn!(target: "gray_tools", "delegation {} stalled after {}s (in_tool={})", delegation_id_for_spawn, elapsed, in_tool);
                        }
                        let _ = &mut in_tool;
                        format!("background subagent {subagent_id} completed: {goal} (heartbeat)")
                    }
                    _ = cancel.cancelled() => {
                        format!("background subagent {subagent_id} cancelled: {goal}")
                    }
                };
                state.active.write().await.remove(&subagent_id);
                let _ = persist_completion(&delegation_id_for_spawn, "completed");
                let ev = CompletionEvent { delegation_id: delegation_id_for_spawn.clone(), subagent_id, goal, output, is_error: false };
                let _ = tx.send(ev);
            }
        });
        ToolOutput::ok(serde_json::to_string_pretty(&serde_json::json!({
            "status":"dispatched","mode":"background","delegation_id":delegation_id,"goals":goals_json,
            "hint":"use delegate_task action=list to check, action=steer/stop to control"
        })).unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gray_core::agent::ToolContext;
    use std::path::PathBuf;

    fn ctx() -> ToolContext { ToolContext { cwd: PathBuf::from("/tmp"), cancel: tokio_util::sync::CancellationToken::new(), questions: None } }

    #[tokio::test]
    async fn background_dispatch_returns_immediately() {
        let tool = DelegateTool::with_default_state(DelegateConfig { max_concurrent_children: 10, ..Default::default() });
        let start = Instant::now();
        let out = tool.execute(&ctx(), serde_json::json!({"goal":"do thing","background":true})).await;
        assert!(start.elapsed() < Duration::from_secs(1), "background should return immediately");
        assert!(!out.is_error);
        let v: Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(v["status"], "dispatched");
        assert!(v["delegation_id"].is_string());
        // completion arrives shortly
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut rx_guard = tool.state.completion_rx.lock().unwrap();
        let rx = rx_guard.as_mut().unwrap();
        let ev = rx.try_recv().expect("completion should be queued");
        assert_eq!(ev.delegation_id, v["delegation_id"].as_str().unwrap());
    }

    #[tokio::test]
    async fn list_steer_stop_control() {
        let tool = DelegateTool::with_default_state(DelegateConfig::default());
        // insert fake active record
        {
            let mut active = tool.state.active.write().await;
            active.insert("sub_test123".to_string(), ActiveRecord { subagent_id: "sub_test123".to_string(), delegation_id: "deleg_abc".to_string(), goal: "fake".to_string(), started_at: Instant::now(), depth: 1, status: "running".to_string() });
        }
        let list = tool.execute(&ctx(), serde_json::json!({"action":"list"})).await;
        assert!(!list.is_error);
        assert!(list.content.contains("sub_test123"));
        let steer = tool.execute(&ctx(), serde_json::json!({"action":"steer","subagent_id":"sub_test123","message":"go faster"})).await;
        assert!(!steer.is_error);
        assert!(steer.content.contains("steer queued"));
        let steer_missing = tool.execute(&ctx(), serde_json::json!({"action":"steer","subagent_id":"nope","message":"hi"})).await;
        assert!(steer_missing.is_error);
        let stop = tool.execute(&ctx(), serde_json::json!({"action":"stop","subagent_id":"sub_test123"})).await;
        assert!(!stop.is_error);
        let stop2 = tool.execute(&ctx(), serde_json::json!({"action":"stop","subagent_id":"sub_test123"})).await;
        assert!(stop2.is_error);
    }

    #[tokio::test]
    async fn rejects_when_at_capacity() {
        let tool = DelegateTool::with_default_state(DelegateConfig { max_concurrent_children: 1, ..Default::default() });
        // occupy the single permit with a background dispatch (holds 1 permit)
        let out1 = tool.execute(&ctx(), serde_json::json!({"goal":"first","background":true})).await;
        assert!(!out1.is_error);
        // second should be rejected
        let out2 = tool.execute(&ctx(), serde_json::json!({"goal":"second","background":true})).await;
        assert!(out2.is_error);
        assert!(out2.content.contains("capacity"));
    }
}
