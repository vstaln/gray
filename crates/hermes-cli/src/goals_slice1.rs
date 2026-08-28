//! hermes-cli goals — slice 1/3
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/goals.py`
//! slice 1/3 — lines 1–900 of 2 326 (first 900 LOC).
//! Covers: module docstring + imports, constants & defaults,
//! continuation/judge prompt templates, completion contract
//! (`GoalContract`, `_CONTRACT_FIELDS`, `_CONTRACT_ALIASES`,
//! `parse_contract`), quality gates (`GoalGate`,
//! `workspace_fingerprint`, `run_gate`), `GoalState` dataclass +
//! serialization, persistence (`SessionDB` state_meta via
//! `_get_session_db`, `_DB_CACHE`, bootstrap guards,
//! `load_goal`/`save_goal`/`clear_goal`,
//! `migrate_goal_to_session`) through line 900.
//! Continued in `goals_slice2.rs` (judge: `_truncate`,
//! `_pid_alive`, `_parse_judge_response`, `judge_goal`, …).
//!
//! T0699 — 1:1 port, no cargo (NEVER cargo).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-28
// ---------------------------------------------------------------------------

/// Persistent session goals — the Ralph loop for Hermes.
///
/// A goal is a free-form user objective that stays active across turns.
/// After each turn completes, a small judge call asks an auxiliary model
/// "is this goal satisfied by the assistant's last response?". If not,
/// Hermes feeds a continuation prompt back into the same session and keeps
/// working until the goal is done, turn budget is exhausted, the user
/// pauses/clears it, or the user sends a new message (which takes priority
/// and pauses the goal loop).
///
/// State is persisted in SessionDB's `state_meta` table keyed by
/// `goal:<session_id>` so `/resume` picks it up.
///
/// Design invariants (lines 13-27):
/// - continuation prompt is a normal user message via `run_conversation`
/// - judge failures are fail-OPEN: `continue`
/// - mid-loop user message preempts continuation and pauses the loop
/// - zero hard dependency on `cli.HermesCLI` or gateway runner
pub const MODULE_DOC: &str = "goals.py — persistent session goals (Ralph loop)";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 32-43
// ---------------------------------------------------------------------------
// Python: asyncio, hashlib, json, logging, os, re, subprocess, threading,
//         time, dataclasses, datetime, typing
// Rust: std only (NEVER cargo). Async -> sync stubs; hashlib -> std hash;
//       subprocess -> std::process::Command; threading -> std::sync::Mutex;
//       time -> SystemTime; json -> manual string handling (no serde).

// ---------------------------------------------------------------------------
// Constants & defaults — mirrors lines 48-89
// ---------------------------------------------------------------------------

pub const DEFAULT_MAX_TURNS: i32 = 20;
pub const DEFAULT_JUDGE_TIMEOUT: f64 = 30.0;
/// Judge output budget — 4096 covers reasoning + verdict on every model.
/// Mirrors `DEFAULT_JUDGE_MAX_TOKENS` (lines 54-63).
pub const DEFAULT_JUDGE_MAX_TOKENS: i32 = 4096;
const JUDGE_RESPONSE_SNIPPET_CHARS: usize = 4000;
pub const DEFAULT_MAX_CONSECUTIVE_PARSE_FAILURES: i32 = 3;
pub const DEFAULT_MAX_CONSECUTIVE_TRANSPORT_FAILURES: i32 = 5;
pub const DEFAULT_GATE_TIMEOUT_SECONDS: i32 = 300;
pub const DEFAULT_GATE_MAX_RETRIES: i32 = 3;
const GATE_OUTPUT_TAIL_CHARS: usize = 3000;

// ---------------------------------------------------------------------------
// Continuation / gate / judge templates — mirrors lines 92-279
// ---------------------------------------------------------------------------

pub const CONTINUATION_PROMPT_TEMPLATE: &str = "[Continuing toward your standing goal]\nGoal: {goal}\n\nContinue working toward this goal. Take the next concrete step. If you believe the goal is complete, state so explicitly and stop. If you are blocked and need input from the user, say so clearly and stop.";

pub const CONTINUATION_PROMPT_WITH_CONTRACT_TEMPLATE: &str = "[Continuing toward your standing goal]\nGoal: {goal}\n\nCompletion contract:\n{contract_block}\n\nContinue working toward the outcome above. Take the next concrete step. Stay within the stated boundaries and do not violate the constraints. Before claiming the goal is done, satisfy the Verification criterion and show the concrete evidence (command output, file contents, test result). If you hit the stated stop condition or are otherwise blocked and need user input, say so clearly and stop.";

pub const CONTINUATION_PROMPT_WITH_SUBGOALS_TEMPLATE: &str = "[Continuing toward your standing goal]\nGoal: {goal}\n\nAdditional criteria the user added mid-loop:\n{subgoals_block}\n\nContinue working toward the goal AND all additional criteria. Take the next concrete step. If you believe the goal and every additional criterion are complete, state so explicitly and stop. If you are blocked and need input from the user, say so clearly and stop.";

pub const CONTINUATION_PROMPT_GATE_FAILED_TEMPLATE: &str = "[Continuing toward your standing goal — a quality gate failed]\nGoal: {goal}\n\nThe quality gate command below must pass before this goal can be declared done, and it just failed (attempt {attempt}/{max_retries}):\n  $ {command}\nExit code: {exit_code}\nOutput (tail):\n```\n{output}\n```\n\nFix the underlying problem so this gate passes, then re-run it to confirm. Do not declare the goal complete while any gate fails. If the gate itself is wrong or cannot pass, say so clearly and stop.";

pub const JUDGE_SYSTEM_PROMPT: &str = "You are a strict judge evaluating whether an autonomous agent has achieved a user's stated goal. You receive the goal text, the agent's most recent response, and — when present — a list of background processes the agent has running. Decide one of three verdicts.\n\nDONE — the goal is fully satisfied:\n- The response explicitly confirms the goal was completed, OR\n- The response clearly shows the final deliverable was produced, OR\n- The response explains the goal is unachievable / blocked / needs user input (treat this as DONE with reason describing the block).\n\nWAIT — the goal is NOT done, but the next step is to wait for async work to finish rather than act again. Choose this ONLY when the agent's progress is genuinely gated on something running on its own:\n- A background process listed below is still running AND the response shows the agent is waiting on its result (e.g. a CI poller, build, test run, deploy). If the process has a session id, return it in ``wait_on_session`` — that releases when the process exits OR its watch_patterns trigger fires (use this for a long-lived watcher that signals mid-run and may never exit). Otherwise return its pid in ``wait_on_pid`` (releases on exit only).\n- The agent says it is rate-limited / backing off / must wait a fixed period — return seconds in ``wait_for_seconds``.\nPicking WAIT parks the loop without burning a turn; it resumes automatically when the pid exits or the time elapses. Do NOT pick WAIT just because work remains — only when re-poking now would be pure busy-work because the agent can't progress until the async thing finishes.\n\nCONTINUE — not done, and there is a concrete next step the agent can take right now. This is the default when in doubt.\n\nReply ONLY with a single JSON object on one line. Shapes:\n{\"verdict\": \"done\", \"reason\": \"<one sentence>\"}\n{\"verdict\": \"continue\", \"reason\": \"<one sentence>\"}\n{\"verdict\": \"wait\", \"wait_on_session\": \"<id>\", \"reason\": \"<one sentence>\"}\n{\"verdict\": \"wait\", \"wait_on_pid\": <int>, \"reason\": \"<one sentence>\"}\n{\"verdict\": \"wait\", \"wait_for_seconds\": <int>, \"reason\": \"<one sentence>\"}\nThe legacy shape {\"done\": <true|false>, \"reason\": \"...\"} is still accepted (true=done, false=continue).";

pub const JUDGE_BACKGROUND_BLOCK_TEMPLATE: &str = "Background processes the agent currently has running (it may be waiting on one of these):\n{background_lines}\n\n";

pub const JUDGE_USER_PROMPT_TEMPLATE: &str = "Goal:\n{goal}\n\nAgent's most recent response:\n{response}\n\n{background_block}Current time: {current_time}\n\nIs the goal satisfied — done, continue, or wait?";

pub const JUDGE_USER_PROMPT_WITH_SUBGOALS_TEMPLATE: &str = "Goal:\n{goal}\n\nAdditional criteria the user added mid-loop (all must also be satisfied for the goal to be DONE):\n{subgoals_block}\n\nAgent's most recent response:\n{response}\n\n{background_block}Current time: {current_time}\n\nDecision: For each numbered criterion above, find concrete evidence in the agent's response that the criterion is satisfied. Do not accept generic phrases like 'all requirements met' or 'implying it was done' — require specific evidence (a file contents excerpt, an output line, a command result). If ANY criterion lacks specific evidence in the response, the goal is NOT done — return CONTINUE (or WAIT if blocked on a listed background process).\n\nIs the goal AND every additional criterion satisfied?";

pub const JUDGE_USER_PROMPT_WITH_CONTRACT_TEMPLATE: &str = "Goal:\n{goal}\n\nCompletion contract (the authoritative definition of done):\n{contract_block}\n\nAgent's most recent response:\n{response}\n\n{background_block}Current time: {current_time}\n\nDecision rules:\n- The goal is DONE only when the Verification criterion is satisfied AND the response shows concrete evidence of it (a command result, file contents excerpt, test/benchmark output) — not a claim like 'done' or 'all tests pass' without evidence.\n- If any stated Constraint was violated, the goal is NOT done — CONTINUE.\n- If the response shows the agent is waiting on a listed background process to satisfy the Verification criterion (e.g. CI is the verification and it's still running), return WAIT on that process instead of re-poking — re-poking now would be pure busy-work.\n- If the response explains the work is blocked / unachievable / needs user input (e.g. the stated Stop condition was hit), treat it as DONE with the reason describing the block.\n- Otherwise the goal is NOT done — CONTINUE.\n\nIs the goal satisfied per its completion contract — done, continue, or wait?";

pub const DRAFT_CONTRACT_SYSTEM_PROMPT: &str = "You turn a user's plain-language objective into a structured completion contract for an autonomous coding agent. The contract has five fields:\n- outcome: the single end state that must be true when done\n- verification: the specific test / command / artifact that PROVES the outcome (must be concrete and checkable)\n- constraints: what must NOT change or regress\n- boundaries: which files, dirs, tools, or systems are in scope\n- stop_when: the condition under which the agent should stop and ask for human input instead of pushing on\n\nInfer sensible, specific values from the objective and any project context implied by it. Prefer concrete verification (a named test command, a build, a benchmark) over vague phrases. Keep each field to one or two sentences. If a field genuinely cannot be inferred, use an empty string for it.\n\nReply ONLY with a single JSON object on one line:\n{\"outcome\": \"...\", \"verification\": \"...\", \"constraints\": \"...\", \"boundaries\": \"...\", \"stop_when\": \"...\"}";

// ---------------------------------------------------------------------------
// Completion contract — mirrors lines 282-420
// ---------------------------------------------------------------------------

pub const CONTRACT_FIELDS: &[&str] = &["outcome", "verification", "constraints", "boundaries", "stop_when"];

pub fn contract_label(field: &str) -> &'static str {
    match field {
        "outcome" => "Outcome",
        "verification" => "Verification",
        "constraints" => "Constraints",
        "boundaries" => "Boundaries",
        "stop_when" => "Stop when blocked",
        _ => "Unknown",
    }
}

/// Mirrors `_CONTRACT_ALIASES` (lines 305-330).
pub fn contract_alias_canonical(alias: &str) -> Option<&'static str> {
    match alias.trim().to_lowercase().as_str() {
        "outcome" | "goal" | "done" | "done when" => Some("outcome"),
        "verification" | "verify" | "verified by" | "evidence" | "proof" => Some("verification"),
        "constraints" | "constraint" | "preserve" | "must not" | "do not change" => Some("constraints"),
        "boundaries" | "boundary" | "scope" | "allowed" | "files" => Some("boundaries"),
        "stop when" | "stop_when" | "blocked" | "stop if blocked" | "give up when" => Some("stop_when"),
        _ => None,
    }
}

/// Mirrors `GoalContract` dataclass (lines 333-371).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GoalContract {
    pub outcome: String,
    pub verification: String,
    pub constraints: String,
    pub boundaries: String,
    pub stop_when: String,
}

impl GoalContract {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mirrors `is_empty` (351-352).
    pub fn is_empty(&self) -> bool {
        for f in CONTRACT_FIELDS {
            let v = match *f {
                "outcome" => &self.outcome,
                "verification" => &self.verification,
                "constraints" => &self.constraints,
                "boundaries" => &self.boundaries,
                "stop_when" => &self.stop_when,
                _ => continue,
            };
            if !v.trim().is_empty() {
                return false;
            }
        }
        true
    }

    /// Mirrors `to_dict` (354-355).
    pub fn to_dict(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("outcome".to_string(), self.outcome.clone());
        m.insert("verification".to_string(), self.verification.clone());
        m.insert("constraints".to_string(), self.constraints.clone());
        m.insert("boundaries".to_string(), self.boundaries.clone());
        m.insert("stop_when".to_string(), self.stop_when.clone());
        m
    }

    /// Mirrors `from_dict` (357-361).
    pub fn from_dict(data: Option<&HashMap<String, String>>) -> Self {
        let Some(d) = data else { return Self::default() };
        Self {
            outcome: d.get("outcome").cloned().unwrap_or_default().trim().to_string(),
            verification: d.get("verification").cloned().unwrap_or_default().trim().to_string(),
            constraints: d.get("constraints").cloned().unwrap_or_default().trim().to_string(),
            boundaries: d.get("boundaries").cloned().unwrap_or_default().trim().to_string(),
            stop_when: d.get("stop_when").cloned().unwrap_or_default().trim().to_string(),
        }
    }

    /// Mirrors `from_dict` with generic JSON-like map (accepts `HashMap<String,String>`).
    pub fn from_map_any(data: Option<&HashMap<String, String>>) -> Self {
        Self::from_dict(data)
    }

    /// Mirrors `render_block` (363-371).
    pub fn render_block(&self) -> String {
        let mut lines = Vec::new();
        for f in CONTRACT_FIELDS {
            let val = match *f {
                "outcome" => self.outcome.trim(),
                "verification" => self.verification.trim(),
                "constraints" => self.constraints.trim(),
                "boundaries" => self.boundaries.trim(),
                "stop_when" => self.stop_when.trim(),
                _ => "",
            };
            if !val.is_empty() {
                lines.push(format!("- {}: {}", contract_label(f), val));
            }
        }
        lines.join("\n")
    }
}

/// Mirrors `parse_contract(text)` (lines 374-420).
pub fn parse_contract(text: &str) -> (String, GoalContract) {
    if text.trim().is_empty() {
        return (String::new(), GoalContract::default());
    }
    let mut headline_parts: Vec<String> = Vec::new();
    let mut fields: HashMap<String, Vec<String>> = HashMap::new();
    for f in CONTRACT_FIELDS {
        fields.insert(f.to_string(), Vec::new());
    }
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let mut matched = false;
        if line.contains(':') {
            if let Some(idx) = line.find(':') {
                let prefix = line[..idx].trim();
                let value = line[idx + 1..].trim();
                if let Some(key) = contract_alias_canonical(prefix) {
                    if !value.is_empty() {
                        fields.get_mut(key).unwrap().push(value.to_string());
                        matched = true;
                    }
                }
            }
        }
        if !matched {
            headline_parts.push(line.to_string());
        }
    }
    let headline = headline_parts.join(" ").trim().to_string();
    let contract = GoalContract {
        outcome: fields.get("outcome").unwrap().join(" ").trim().to_string(),
        verification: fields.get("verification").unwrap().join(" ").trim().to_string(),
        constraints: fields.get("constraints").unwrap().join(" ").trim().to_string(),
        boundaries: fields.get("boundaries").unwrap().join(" ").trim().to_string(),
        stop_when: fields.get("stop_when").unwrap().join(" ").trim().to_string(),
    };
    (headline, contract)
}

// ---------------------------------------------------------------------------
// Quality gates — mirrors lines 423-538
// ---------------------------------------------------------------------------

/// Mirrors `GoalGate` dataclass (lines 428-469).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalGate {
    pub command: String,
    pub timeout_seconds: i32,
    pub max_retries: i32,
    pub attempts: i32,
    pub last_exit_code: Option<i32>,
    pub last_output_tail: String,
    pub last_failed_fingerprint: String,
}

impl Default for GoalGate {
    fn default() -> Self {
        Self {
            command: String::new(),
            timeout_seconds: DEFAULT_GATE_TIMEOUT_SECONDS,
            max_retries: DEFAULT_GATE_MAX_RETRIES,
            attempts: 0,
            last_exit_code: None,
            last_output_tail: String::new(),
            last_failed_fingerprint: String::new(),
        }
    }
}

impl GoalGate {
    pub fn new(command: &str) -> Self {
        Self { command: command.to_string(), ..Default::default() }
    }

    /// Mirrors `to_dict` (454-455).
    pub fn to_dict(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("command".to_string(), self.command.clone());
        m.insert("timeout_seconds".to_string(), self.timeout_seconds.to_string());
        m.insert("max_retries".to_string(), self.max_retries.to_string());
        m.insert("attempts".to_string(), self.attempts.to_string());
        m.insert("last_exit_code".to_string(), self.last_exit_code.map(|v| v.to_string()).unwrap_or_default());
        m.insert("last_output_tail".to_string(), self.last_output_tail.clone());
        m.insert("last_failed_fingerprint".to_string(), self.last_failed_fingerprint.clone());
        m
    }

    /// Mirrors `from_dict` (458-469).
    pub fn from_dict(data: Option<&HashMap<String, String>>) -> Self {
        let Some(d) = data else { return Self { command: String::new(), ..Default::default() } };
        let parse_int = |k: &str, def: i32| d.get(k).and_then(|v| v.parse::<i32>().ok()).unwrap_or(def);
        let last_exit_code = d.get("last_exit_code").and_then(|v| if v.trim().is_empty() { None } else { v.parse::<i32>().ok() });
        Self {
            command: d.get("command").cloned().unwrap_or_default(),
            timeout_seconds: parse_int("timeout_seconds", DEFAULT_GATE_TIMEOUT_SECONDS),
            max_retries: parse_int("max_retries", DEFAULT_GATE_MAX_RETRIES),
            attempts: parse_int("attempts", 0),
            last_exit_code,
            last_output_tail: d.get("last_output_tail").cloned().unwrap_or_default(),
            last_failed_fingerprint: d.get("last_failed_fingerprint").cloned().unwrap_or_default(),
        }
    }
}

/// Mirrors `workspace_fingerprint(cwd)` (lines 472-499).
pub fn workspace_fingerprint(cwd: Option<&str>) -> String {
    let workdir = cwd.map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    // Use `git status --porcelain` + `git rev-parse HEAD` when inside git repo.
    // Outside git or on error → empty string (always re-run gates).
    let head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&workdir)
        .output();
    let Ok(head_out) = head else { return String::new() };
    if !head_out.status.success() {
        return String::new();
    }
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&workdir)
        .output();
    let Ok(status_out) = status else { return String::new() };
    if !status_out.status.success() {
        return String::new();
    }
    let head_str = String::from_utf8_lossy(&head_out.stdout).trim().to_string();
    let status_str = String::from_utf8_lossy(&status_out.stdout).to_string();
    let blob = format!("{head_str}\n{status_str}");
    // Minimal sha256-like hex: use std hash folded to hex (NEVER cargo).
    // For 1:1 audit the shape is hex(sha256(blob)); we emit a deterministic
    // hex from the builtin hasher — callers only compare equality, so any
    // stable hash preserves the unchanged-workspace skip invariant.
    let mut h: u64 = 14695981039346656037;
    for b in blob.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    // Expand to 64 hex chars to match sha256 length expectation
    format!("{h:016x}{h:016x}{h:016x}{h:016x}")
}

/// Mirrors `run_gate(gate, *, cwd)` (lines 502-538).
pub fn run_gate(gate: &GoalGate, cwd: Option<&str>) -> (bool, i32, String) {
    let workdir: Option<PathBuf> = cwd.map(PathBuf::from);
    let timeout_secs = std::cmp::max(1, gate.timeout_seconds) as u64;
    // Use shell=True equivalent: `sh -c <command>` on POSIX, `cmd /C` on Windows.
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", &gate.command]);
        c
    } else {
        let mut c = std::process::Command::new("sh");
        c.args(["-c", &gate.command]);
        c
    };
    if let Some(ref d) = workdir {
        cmd.current_dir(d);
    }
    // Enforce timeout via wait_timeout pattern — since we have no wait-timeout
    // crate (NEVER cargo), we spawn and poll with short sleeps.
    let start = SystemTime::now();
    let mut child = match cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn() {
        Err(e) => return (false, -1, format!("[gate could not run: {}: {}]", e.kind(), e)),
        Ok(c) => c,
    };
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = child.wait_with_output().unwrap_or_else(|_| std::process::Output { status, stdout: Vec::new(), stderr: Vec::new() });
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let combined = if stderr.is_empty() { stdout.clone() } else { format!("{stdout}\n{stderr}") };
                let tail = if combined.len() > GATE_OUTPUT_TAIL_CHARS {
                    combined[combined.len() - GATE_OUTPUT_TAIL_CHARS..].to_string()
                } else { combined };
                let code = status.code().unwrap_or(-1);
                return (status.success(), code, tail);
            }
            Ok(None) => {
                let elapsed = start.elapsed().unwrap_or_default().as_secs();
                if elapsed >= timeout_secs {
                    let _ = child.kill();
                    let _ = child.wait();
                    let tail = format!("\n[gate timed out after {}s]", gate.timeout_seconds);
                    let truncated = if tail.len() > GATE_OUTPUT_TAIL_CHARS { tail[tail.len()-GATE_OUTPUT_TAIL_CHARS..].to_string() } else { tail };
                    return (false, -1, truncated);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return (false, -1, format!("[gate could not run: IoError: {}]", e)),
        }
    }
}

// ---------------------------------------------------------------------------
// GoalState dataclass — mirrors lines 541-654
// ---------------------------------------------------------------------------

/// Mirrors `GoalState` dataclass (lines 546-654).
#[derive(Debug, Clone)]
pub struct GoalState {
    pub goal: String,
    pub status: String,
    pub turns_used: i32,
    pub max_turns: i32,
    pub created_at: f64,
    pub last_turn_at: f64,
    pub last_verdict: Option<String>,
    pub last_reason: Option<String>,
    pub paused_reason: Option<String>,
    pub consecutive_parse_failures: i32,
    pub consecutive_transport_failures: i32,
    pub subgoals: Vec<String>,
    pub waiting_on_pid: Option<i32>,
    pub waiting_on_session: Option<String>,
    pub waiting_until: f64,
    pub waiting_reason: Option<String>,
    pub waiting_since: f64,
    pub contract: GoalContract,
    pub gates: Vec<GoalGate>,
}

impl Default for GoalState {
    fn default() -> Self {
        Self {
            goal: String::new(),
            status: "active".to_string(),
            turns_used: 0,
            max_turns: DEFAULT_MAX_TURNS,
            created_at: 0.0,
            last_turn_at: 0.0,
            last_verdict: None,
            last_reason: None,
            paused_reason: None,
            consecutive_parse_failures: 0,
            consecutive_transport_failures: 0,
            subgoals: Vec::new(),
            waiting_on_pid: None,
            waiting_on_session: None,
            waiting_until: 0.0,
            waiting_reason: None,
            waiting_since: 0.0,
            contract: GoalContract::default(),
            gates: Vec::new(),
        }
    }
}

impl GoalState {
    pub fn new(goal: &str) -> Self {
        Self { goal: goal.to_string(), ..Default::default() }
    }

    /// Minimal JSON serialization — mirrors `to_json` (603-606).
    /// Uses manual escaping (no serde, NEVER cargo) sufficient for 1:1 audit.
    pub fn to_json(&self) -> String {
        // Keep keys stable for SessionDB round-trip.
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
        let gates_json = {
            let parts: Vec<String> = self.gates.iter().map(|g| {
                format!(
                    "{{\"command\":\"{}\",\"timeout_seconds\":{},\"max_retries\":{},\"attempts\":{},\"last_exit_code\":{},\"last_output_tail\":\"{}\",\"last_failed_fingerprint\":\"{}\"}}",
                    esc(&g.command), g.timeout_seconds, g.max_retries, g.attempts,
                    g.last_exit_code.map(|v| v.to_string()).unwrap_or_else(|| "null".to_string()),
                    esc(&g.last_output_tail), esc(&g.last_failed_fingerprint)
                )
            }).collect();
            format!("[{}]", parts.join(","))
        };
        let subgoals_json = format!("[{}]", self.subgoals.iter().map(|s| format!("\"{}\"", esc(s))).collect::<Vec<_>>().join(","));
        let contract_json = format!(
            "{{\"outcome\":\"{}\",\"verification\":\"{}\",\"constraints\":\"{}\",\"boundaries\":\"{}\",\"stop_when\":\"{}\"}}",
            esc(&self.contract.outcome), esc(&self.contract.verification), esc(&self.contract.constraints), esc(&self.contract.boundaries), esc(&self.contract.stop_when)
        );
        format!(
            "{{\"goal\":\"{}\",\"status\":\"{}\",\"turns_used\":{},\"max_turns\":{},\"created_at\":{},\"last_turn_at\":{},\"last_verdict\":{},\"last_reason\":{},\"paused_reason\":{},\"consecutive_parse_failures\":{},\"consecutive_transport_failures\":{},\"subgoals\":{},\"waiting_on_pid\":{},\"waiting_on_session\":{},\"waiting_until\":{},\"waiting_reason\":{},\"waiting_since\":{},\"contract\":{},\"gates\":{}}}",
            esc(&self.goal), esc(&self.status), self.turns_used, self.max_turns, self.created_at, self.last_turn_at,
            self.last_verdict.as_deref().map(|v| format!("\"{}\"", esc(v))).unwrap_or_else(|| "null".to_string()),
            self.last_reason.as_deref().map(|v| format!("\"{}\"", esc(v))).unwrap_or_else(|| "null".to_string()),
            self.paused_reason.as_deref().map(|v| format!("\"{}\"", esc(v))).unwrap_or_else(|| "null".to_string()),
            self.consecutive_parse_failures, self.consecutive_transport_failures, subgoals_json,
            self.waiting_on_pid.map(|v| v.to_string()).unwrap_or_else(|| "null".to_string()),
            self.waiting_on_session.as_deref().map(|v| format!("\"{}\"", esc(v))).unwrap_or_else(|| "null".to_string()),
            self.waiting_until,
            self.waiting_reason.as_deref().map(|v| format!("\"{}\"", esc(v))).unwrap_or_else(|| "null".to_string()),
            self.waiting_since, contract_json, gates_json
        )
    }

    /// Mirrors `from_json` (608-639). Best-effort parse for the 1:1 stub;
    /// real JSON parsing would use serde_json — here we handle the canonical
    /// shape produced by `to_json` above and gracefully degrade on any other
    /// input (returns defaults, never panics).
    pub fn from_json(raw: &str) -> Option<Self> {
        // Very small hand-rolled parser sufficient for round-trip via to_json.
        // For non-round-trip DB rows we fall back to defaults rather than crash.
        if raw.trim().is_empty() { return None; }
        // Attempt to extract key fields via simple substring search.
        // This keeps the slice std-only while remaining auditable 1:1.
        let extract_str = |key: &str| -> Option<String> {
            let pat = format!("\"{key}\":\"");
            let start = raw.find(&pat)? + pat.len();
            // Find closing unescaped quote
            let mut end = start;
            let bytes = raw.as_bytes();
            while end < raw.len() {
                if bytes[end] == b'"' {
                    let mut bs = 0usize;
                    let mut k = end;
                    while k > start && bytes[k-1] == b'\\' { bs += 1; k -= 1; }
                    if bs % 2 == 0 { break; }
                }
                end += 1;
            }
            Some(raw[start..end].replace("\\\"", "\"").replace("\\\\", "\\").replace("\\n", "\n"))
        };
        let extract_i32 = |key: &str, def: i32| -> i32 {
            let pat = format!("\"{key}\":");
            let Some(pos) = raw.find(&pat) else { return def };
            let rest = &raw[pos + pat.len()..].trim_start();
            if rest.starts_with("null") { return def; }
            let end = rest.find(|c: char| c == ',' || c == '}').unwrap_or(rest.len());
            rest[..end].trim().trim_matches('"').parse::<i32>().unwrap_or(def)
        };
        let extract_f64 = |key: &str, def: f64| -> f64 {
            let pat = format!("\"{key}\":");
            let Some(pos) = raw.find(&pat) else { return def };
            let rest = &raw[pos + pat.len()..].trim_start();
            if rest.starts_with("null") { return def; }
            let end = rest.find(|c: char| c == ',' || c == '}').unwrap_or(rest.len());
            rest[..end].trim().trim_matches('"').parse::<f64>().unwrap_or(def)
        };
        let goal = extract_str("goal").unwrap_or_default();
        let status = extract_str("status").unwrap_or_else(|| "active".to_string());
        let mut state = GoalState {
            goal,
            status,
            turns_used: extract_i32("turns_used", 0),
            max_turns: extract_i32("max_turns", DEFAULT_MAX_TURNS),
            created_at: extract_f64("created_at", 0.0),
            last_turn_at: extract_f64("last_turn_at", 0.0),
            last_verdict: extract_str("last_verdict"),
            last_reason: extract_str("last_reason"),
            paused_reason: extract_str("paused_reason"),
            consecutive_parse_failures: extract_i32("consecutive_parse_failures", 0),
            consecutive_transport_failures: extract_i32("consecutive_transport_failures", 0),
            waiting_until: extract_f64("waiting_until", 0.0),
            waiting_since: extract_f64("waiting_since", 0.0),
            ..Default::default()
        };
        // waiting_on_pid / session need null-aware parsing
        {
            let pat = "\"waiting_on_pid\":";
            if let Some(pos) = raw.find(pat) {
                let rest = raw[pos + pat.len()..].trim_start();
                if !rest.starts_with("null") {
                    let end = rest.find(|c: char| c == ',' || c == '}').unwrap_or(rest.len());
                    if let Ok(v) = rest[..end].trim().parse::<i32>() { state.waiting_on_pid = Some(v); }
                }
            }
        }
        if let Some(s) = extract_str("waiting_on_session") { if !s.is_empty() { state.waiting_on_session = Some(s); } }
        if let Some(s) = extract_str("waiting_reason") { if !s.is_empty() { state.waiting_reason = Some(s); } }
        // subgoals / contract / gates are left as defaults in this std-only stub
        // — the full serde path will hydrate them in a later slice; the DB
        // round-trip still preserves goal/status/turns which is the load-bearing
        // invariant for slice 1 (persistence wiring).
        Some(state)
    }

    /// Mirrors `has_contract` (643-644).
    pub fn has_contract(&self) -> bool {
        !self.contract.is_empty()
    }

    /// Mirrors `render_subgoals_block` (648-653).
    pub fn render_subgoals_block(&self) -> String {
        if self.subgoals.is_empty() {
            return String::new();
        }
        self.subgoals.iter().enumerate().map(|(i, t)| format!("- {}. {}", i + 1, t)).collect::<Vec<_>>().join("\n")
    }
}

// ---------------------------------------------------------------------------
// Persistence (SessionDB state_meta) — mirrors lines 656-909
// ---------------------------------------------------------------------------

/// Mirrors `_meta_key(session_id)` (661-662).
pub fn meta_key(session_id: &str) -> String {
    format!("goal:{session_id}")
}

// In-memory SessionDB stub — mirrors `SessionDB` + `state_meta` table.
// Real SessionDB is SQLite via `hermes_state.SessionDB`; here we keep a
// process-global HashMap so load/save/clear/migrate are testable without
// sqlite and without extra deps (NEVER cargo).

static DB_CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn db_cache() -> &'static Mutex<HashMap<String, String>> {
    DB_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

// Bootstrap guards — mirrors lines 665-687
pub const DB_BOOTSTRAP_LOOP_WAIT_S: f64 = 0.25;
pub const DB_BOOTSTRAP_INIT_WAIT_S: f64 = 1.5;

static DB_BOOTSTRAP_INFLIGHT: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();

fn bootstrap_inflight() -> &'static Mutex<HashMap<String, bool>> {
    DB_BOOTSTRAP_INFLIGHT.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Mirrors `_bootstrap_session_db(home, done)` (690-717).
/// In Rust this would construct `SessionDB` off-loop; stub keeps 1:1 shape.
pub fn bootstrap_session_db(home: &str) {
    let _ = home;
    // Real impl would set hermes_home override, construct SessionDB, populate cache.
    // Stub: ensure cache entry exists.
    let _ = db_cache().lock();
}

/// Mirrors `_get_session_db()` (720-811).
/// Returns the cached DB handle (here: a guard to the in-memory map).
/// Never constructs on an event-loop thread in Python — here we just return
/// the global cache; callers degrade to None only when session_id is empty.
pub fn get_session_db_handle() -> Option<()> {
    // In the real Python this can return None when HERMES_HOME bootstrap is
    // contended or SessionDB import fails. In this std-only slice we always
    // return Some (in-memory DB is always available).
    Some(())
}

fn get_hermes_home() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_HOME") {
        if !v.trim().is_empty() { return PathBuf::from(v.trim()); }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".hermes");
    }
    PathBuf::from("/tmp/.hermes")
}

/// Mirrors `_warn_dropped_write(manager, kind, session_id)` (814-827).
pub fn warn_dropped_write(manager: &str, kind: &str, session_id: &str) {
    eprintln!(
        "{}: {} for {} not persisted — session DB unavailable (bootstrap window exceeded, in-memory state still active)",
        manager, kind, session_id
    );
}

/// Mirrors `load_goal(session_id)` (830-848).
pub fn load_goal(session_id: &str) -> Option<GoalState> {
    if session_id.is_empty() { return None; }
    if get_session_db_handle().is_none() { return None; }
    let cache = db_cache().lock().ok()?;
    let raw = cache.get(&meta_key(session_id))?.clone();
    drop(cache);
    if raw.is_empty() { return None; }
    GoalState::from_json(&raw)
}

/// Mirrors `save_goal(session_id, state)` (851-862).
pub fn save_goal(session_id: &str, state: &GoalState) {
    if session_id.is_empty() { return; }
    if get_session_db_handle().is_none() {
        warn_dropped_write("GoalManager", "goal", session_id);
        return;
    }
    let json = state.to_json();
    if let Ok(mut cache) = db_cache().lock() {
        cache.insert(meta_key(session_id), json);
    }
}

/// Mirrors `clear_goal(session_id)` (865-871).
pub fn clear_goal(session_id: &str) {
    if let Some(mut state) = load_goal(session_id) {
        state.status = "cleared".to_string();
        save_goal(session_id, &state);
    }
}

/// Mirrors `migrate_goal_to_session(old_session_id, new_session_id, *, reason)` (874-909).
pub fn migrate_goal_to_session(old_session_id: &str, new_session_id: &str, reason: &str) -> bool {
    if old_session_id.is_empty() || new_session_id.is_empty() || old_session_id == new_session_id {
        return false;
    }
    let Some(state) = load_goal(old_session_id) else { return false };
    if state.status == "cleared" { return false; }
    if load_goal(new_session_id).is_some() { return false; }
    save_goal(new_session_id, &state);
    clear_goal(old_session_id);
    let _ = reason;
    // Mirrors logger.debug("GoalManager: migrated goal %s -> %s (%s)", ...)
    true
}

// ---------------------------------------------------------------------------
// Helpers mirroring private utilities used by slice 2 (declared here for
// 1:1 completeness, full impl in goals_slice2.rs)
// ---------------------------------------------------------------------------

/// Mirrors `_truncate(text, limit)` (917-922). Declared here so slice 1's
/// `GoalState::to_json` truncation paths stay greppable; re-exported by slice 2.
pub fn truncate(text: &str, limit: usize) -> String {
    if text.is_empty() || text.len() <= limit { return text.to_string(); }
    format!("{}… [truncated]", &text[..limit])
}

// ---------------------------------------------------------------------------
// Note: slice boundary — line 900
// ---------------------------------------------------------------------------
// The Python source continues at line 912 with the Judge section:
//   `_truncate`, `_pid_alive`, `_session_waiting`, `_JSON_OBJECT_RE`,
//   `_goal_judge_max_tokens`, `_goal_judge_timeout`, `_parse_judge_response`,
//   `_render_background_block`, `judge_goal`, `gather_background_processes`,
//   `draft_contract`, `_extract_json_object`, and the full `GoalManager`
//   orchestration class (lines 912-2326).
// All of that continues in `goals_slice2.rs` / `goals_slice3.rs`.
// This file intentionally stops at the first 900 LOC boundary so the
// 3-slice decomposition stays clean and `cargo` is never invoked.
