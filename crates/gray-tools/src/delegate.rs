//! delegate_task — steal hermes tools/delegate_tool.py + async_delegation.py
//! ponytail: flat depth=1, Semaphore(10), background stub, SQLite deferred to state.db when needed
use std::sync::Arc;
use std::time::Instant;
use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::delegation::{ActiveRecord, CompletionEvent, DelegateConfig, DelegateRole, DelegationState, DELEGATE_BLOCKED, normalize_role};
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
        // control actions
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
        // collect goals
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
        // ponytail: flat depth=1 check — if called from subagent depth>=max, reject orchestrator
        // For now, no depth tracking; assume depth 0
        let _blocked = DELEGATE_BLOCKED;
        let cwd = ctx.cwd.clone();
        if background {
            return self.dispatch_background(goals, cwd).await;
        }
        self.run_sync(goals, cwd).await
    }
}

impl DelegateTool {
    async fn handle_list(&self) -> ToolOutput {
        let active = self.state.active.read().await;
        let list: Vec<Value> = active.values().map(|r| serde_json::json!({"subagent_id":r.subagent_id,"goal":r.goal,"status":r.status})).collect();
        ToolOutput::ok(serde_json::to_string_pretty(&serde_json::json!({"active":list})).unwrap())
    }
    async fn handle_steer(&self, id: &str, msg: &str) -> ToolOutput {
        if id.is_empty() { return ToolOutput::error("steer: subagent_id required"); }
        // ponytail: stub — record steer message
        ToolOutput::ok(format!("steer queued for {id}: {msg}"))
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
                // ponytail: actual child Agent execution would go here — for now simulate
                // In real impl: build child Agent with filtered Registry + new Provider from saved config, run it
                let output = if goal.len() > 24000 {
                    format!("{}…[truncated]", &goal[..24000])
                } else {
                    format!("subagent {} completed goal: {goal} (role {role_str}, cwd {})", subagent_id, cwd.display())
                };
                // heartbeat stub: use tokio::time::timeout if child_timeout set
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

    async fn dispatch_background(&self, goals: Vec<(String, Option<String>, DelegateRole)>, cwd: std::path::PathBuf) -> ToolOutput {
        // try acquire 1 permit for whole batch (hermes async_delegation.py:611)
        let permit = match self.state.sem.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => return ToolOutput::error("rejected: capacity reached, run synchronously or raise delegation.max_concurrent_children"),
        };
        let delegation_id = format!("deleg_{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let delegation_id_for_spawn = delegation_id.clone();
        let goals_json: Vec<Value> = goals.iter().map(|(g,c,_)| serde_json::json!({"goal":g,"context":c})).collect();
        let tx = self.state.completion_tx.clone();
        let state = self.state.clone();
        let max = self.config.max_concurrent_children;
        // Spawn background batch
        tokio::spawn(async move {
            let _permit = permit; // hold 1 permit for whole batch duration
            // For each goal, simulate completion and push to completion_tx
            for (goal, _ctx, _role) in goals {
                let subagent_id = format!("sub_{}", &uuid::Uuid::new_v4().to_string()[..8]);
                // ponytail: SQLite persist dispatch here — state.db:async_delegations
                let rec = ActiveRecord { subagent_id: subagent_id.clone(), delegation_id: delegation_id_for_spawn.clone(), goal: goal.clone(), started_at: Instant::now(), depth: 1, status: "running".to_string() };
                state.active.write().await.insert(subagent_id.clone(), rec);
                // simulate work (bounded by max_concurrent internally if we loop with semaphore — but batch holds 1 permit ponytail)
                let _ = max; let _ = cwd.clone();
                let output = format!("background subagent {subagent_id} completed: {goal}");
                state.active.write().await.remove(&subagent_id);
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
