use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::acp::{AcpClient, SpawnSpec, Wire};
use crate::chat::{self, OutItem, PreparedTurn, TurnResult};
use crate::guard::{self, TurnGuard};
use crate::mcp::{McpPark, PendingCall};

const MAX_SESSIONS: usize = 4;
const PARK_QUIESCE: Duration = Duration::from_secs(10);
const IDLE_TIMEOUT: Duration = Duration::from_secs(45 * 60);
const REAPER_TICK: Duration = Duration::from_secs(30);
const UPDATE_WAIT: Duration = Duration::from_millis(250);

pub trait Driver: Send + Sync {
    fn spawn_spec(&self, stage: &std::path::Path) -> SpawnSpec;
    fn auth_method_id(&self) -> Option<&'static str>;
    fn client_info(&self) -> (&'static str, &'static str);
    fn session_mode(&self) -> Option<&'static str> {
        None
    }
    fn client_capabilities(&self) -> Value {
        json!({"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false})
    }
    fn apply_model(
        &self,
        client: &mut AcpClient,
        session_id: &str,
        session_new_result: &Value,
        model: &str,
        effort: Option<&str>,
        overrides: &[(String, String)],
    ) -> Result<(), String>;
    fn prompt_meta(&self, seq: u64) -> Value {
        let _ = seq;
        Value::Null
    }
    fn prompt_complete_methods(&self) -> &'static [&'static str] {
        &[]
    }
    fn ext_request(&self, _method: &str, _params: &Value) -> Option<Value> {
        None
    }
    fn permission_option(&self, params: &Value) -> PermissionDecision {
        let _ = params;
        PermissionDecision::Deny
    }
    fn stage(
        &self,
        stage: &std::path::Path,
        mcp_url: &str,
        tools: &[Value],
        prep: &PreparedTurn,
    ) -> Result<Option<Value>, String> {
        let _ = (stage, mcp_url, tools, prep);
        Ok(None)
    }
    fn session_meta(&self, init: &Value, system: &str) -> Option<Value> {
        let _ = (init, system);
        None
    }
    fn park_quiesce(&self) -> Duration {
        PARK_QUIESCE
    }
    fn usage(&self, raw: &Value) -> Option<chat::Usage> {
        chat::Usage::from_acp(raw)
    }
    fn turn_outcome(
        &self,
        stage: &std::path::Path,
        model: &str,
        outcome: Result<(), &str>,
    ) -> bool {
        let _ = (stage, model, outcome);
        false
    }
}

pub enum PermissionDecision {
    Allow(String),
    Deny,
}

struct Parked {
    call: PendingCall,
}

pub struct Session {
    client: AcpClient,
    mcp: Arc<McpPark>,
    session_id: String,
    absorbed: Vec<Value>,
    reply_call_ids: Vec<String>,
    reply_text: String,
    parked: HashMap<String, Parked>,
    mid_turn: bool,
    model: String,
    dead: bool,
    prompt_seq: u64,
    last_used: Instant,
    prior_cut: bool,
    system_in_meta: bool,
    images: bool,
    _stage: tempfile::TempDir,
}

pub struct Pool {
    sessions: Mutex<HashMap<String, Session>>,
    driver: Arc<dyn Driver>,
}

impl Pool {
    pub fn new(driver: Arc<dyn Driver>) -> Arc<Self> {
        let pool = Arc::new(Self {
            sessions: Mutex::new(HashMap::new()),
            driver,
        });
        let p = pool.clone();
        std::thread::Builder::new()
            .name("acp-sub-reaper".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(REAPER_TICK);
                    if let Ok(mut sessions) = p.sessions.lock() {
                        sessions.retain(|_, s| {
                            if s.mid_turn {
                                return true;
                            }
                            s.last_used.elapsed() < IDLE_TIMEOUT
                        });
                    }
                }
            })
            .ok();
        pool
    }
}

pub fn run_turn(pool: &Arc<Pool>, raw: &[u8], cancel: &AtomicBool) -> Result<Vec<u8>, String> {
    let body: Value = serde_json::from_slice(raw).map_err(|e| format!("bad request json: {e}"))?;
    let model_hint = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut prep = chat::prepare_turn(&body, &model_hint)?;

    let mut guard = pool.sessions.lock().map_err(|_| "pool lock".to_string())?;

    let mut chosen: Option<String> = None;
    let mut resume_tail: Vec<Value> = Vec::new();
    'outer: for (key, s) in guard.iter() {
        if s.model != prep.model || !s.mid_turn || !prep.input.starts_with(&s.absorbed) {
            continue;
        }
        let region = &prep.input[s.absorbed.len()..];
        if region.iter().any(|i| {
            chat_item_type(i) == "function_call_output"
                && i.get("call_id")
                    .and_then(Value::as_str)
                    .is_some_and(|c| s.parked.contains_key(c))
        }) {
            resume_tail = region.to_vec();
            chosen = Some(key.clone());
            break 'outer;
        }
    }

    let mut tail: Vec<Value> = Vec::new();
    if chosen.is_none() {
        for (key, s) in guard.iter() {
            if s.model != prep.model || s.mid_turn {
                continue;
            }
            if let Ok((t, _undelivered)) =
                chat::continuation(&s.absorbed, &s.reply_call_ids, &s.reply_text, &prep.input)
            {
                tail = t.to_vec();
                chosen = Some(key.clone());
                break;
            }
        }
    } else {
        tail = resume_tail;
    }

    let fresh = chosen.is_none();
    let key = match chosen {
        Some(k) => k,
        None => {
            if guard.len() >= MAX_SESSIONS
                && let Some(oldest) = guard
                    .iter()
                    .filter(|(_, s)| !s.mid_turn)
                    .min_by_key(|(_, s)| s.last_used)
                    .map(|(k, _)| k.clone())
            {
                guard.remove(&oldest);
            }
            let key = format!("sess-{}", chat::rand_hex(8));
            let session = spawn_session(&*pool.driver, &prep)?;
            guard.insert(key.clone(), session);
            tail = prep.input.clone();
            key
        }
    };

    let session = guard.get_mut(&key).ok_or("session vanished")?;
    session.last_used = Instant::now();
    session.mcp.set_tools(to_mcp_tools(&prep.tools));
    prep.tail = tail;
    chat::split_tail_with(&mut prep, session.images);

    let result = drive(session, &*pool.driver, &mut prep, cancel, fresh);
    let discard = pool.driver.turn_outcome(
        session._stage.path(),
        &prep.model,
        result.as_ref().map(|_| ()).map_err(String::as_str),
    );
    if session.dead || discard {
        guard.remove(&key);
    }
    result.map(|r| {
        let response_id = format!("resp_{}", chat::rand_hex(24));
        chat::fold_result(&r, &prep.model, &response_id)
    })
}

fn chat_item_type(i: &Value) -> &str {
    match i.get("type").and_then(Value::as_str) {
        Some(t) => t,
        None if i.get("role").is_some() => "message",
        None => "",
    }
}

fn spawn_session(driver: &dyn Driver, prep: &PreparedTurn) -> Result<Session, String> {
    let mcp = McpPark::start()?;
    let stage = tempfile::TempDir::new().map_err(|e| format!("stage dir: {e}"))?;
    let tools = to_mcp_tools(&prep.tools);
    mcp.set_tools(tools.clone());
    let mcp_servers = driver
        .stage(stage.path(), &mcp.url, &tools, prep)?
        .unwrap_or_else(|| json!([mcp.server_entry("gray")]));
    let spec = driver.spawn_spec(stage.path());
    let mut client = AcpClient::spawn(&spec)
        .map_err(|e| format!("spawn {} {}: {e}", spec.command, spec.args.join(" ")))?;
    let caps = driver.client_capabilities();
    let (name, ver) = driver.client_info();
    let init = client
        .initialize((name, ver), caps)
        .map_err(|e| format!("initialize: {e}"))?;
    if let Some(method) = driver.auth_method_id() {
        client
            .authenticate(method)
            .map_err(|e| format!("authenticate: {e}"))?;
    }
    let meta_system = if prep.system.is_empty() {
        guard::ONE_TURN_RULE.to_string()
    } else {
        format!("{}\n\n{}", prep.system, guard::ONE_TURN_RULE)
    };
    let meta = driver.session_meta(&init, &meta_system);
    let system_in_meta = meta.is_some();
    let images = init
        .pointer("/agentCapabilities/promptCapabilities/image")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let sess = client
        .session_new_with_meta(
            &stage.path().to_string_lossy(),
            mcp_servers,
            meta.unwrap_or(Value::Null),
        )
        .map_err(|e| format!("session/new: {e}"))?;
    let session_id = sess
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or("session/new: no sessionId")?
        .to_string();
    if let Some(mode) = driver.session_mode() {
        let _ = client.set_mode(&session_id, mode);
    }
    if !prep.model.is_empty() {
        driver.apply_model(
            &mut client,
            &session_id,
            &sess,
            &prep.model,
            prep.effort.as_deref(),
            &prep.overrides,
        )?;
    }
    let _ = init;
    Ok(Session {
        client,
        mcp,
        session_id,
        absorbed: Vec::new(),
        reply_call_ids: Vec::new(),
        reply_text: String::new(),
        parked: HashMap::new(),
        mid_turn: false,
        model: prep.model.clone(),
        dead: false,
        prompt_seq: 0,
        last_used: Instant::now(),
        prior_cut: false,
        system_in_meta,
        images,
        _stage: stage,
    })
}

pub fn to_mcp_tools(tools: &[Value]) -> Vec<Value> {
    tools
        .iter()
        .filter_map(|t| {
            let (name, desc, params) = if t.get("type").and_then(Value::as_str) == Some("function")
            {
                (t.get("name"), t.get("description"), t.get("parameters"))
            } else {
                (
                    t.get("name"),
                    t.get("description"),
                    t.get("inputSchema").or_else(|| t.get("parameters")),
                )
            };
            let name = name?.as_str()?;
            if name.is_empty() || name.len() > 50 {
                return None;
            }
            let mut schema = params
                .cloned()
                .unwrap_or_else(|| json!({"type":"object","properties":{}}));
            normalize_schema(&mut schema);
            Some(json!({
                "name": name,
                "description": desc.and_then(Value::as_str).unwrap_or(""),
                "inputSchema": schema,
            }))
        })
        .collect()
}

fn normalize_schema(schema: &mut Value) {
    if let Some(obj) = schema.as_object_mut() {
        for key in ["oneOf", "allOf", "anyOf"] {
            obj.remove(key);
        }
        obj.entry("type".to_string())
            .or_insert(Value::String("object".to_string()));
        if obj.get("type").and_then(Value::as_str) == Some("object")
            && !matches!(obj.get("properties"), Some(Value::Object(_)))
        {
            obj.insert("properties".to_string(), json!({}));
        }
    }
}

fn drive(
    session: &mut Session,
    driver: &dyn Driver,
    prep: &mut PreparedTurn,
    cancel: &AtomicBool,
    fresh: bool,
) -> Result<TurnResult, String> {
    let mut turn_guard = TurnGuard::default();
    let mut r = drive_inner(session, driver, prep, cancel, fresh, &mut turn_guard)?;
    guard::clean_items(&mut r.items);
    if !session.mid_turn {
        let (call_ids, text) = chat::reply_signature(&r);
        session.reply_call_ids = call_ids;
        session.reply_text = text;
    }
    Ok(r)
}

fn drive_inner(
    session: &mut Session,
    driver: &dyn Driver,
    prep: &mut PreparedTurn,
    cancel: &AtomicBool,
    fresh: bool,
    turn_guard: &mut TurnGuard,
) -> Result<TurnResult, String> {
    let mut result = TurnResult {
        stop_reason: "end_turn".to_string(),
        ..Default::default()
    };
    let mut prompt_rx: Option<std::sync::mpsc::Receiver<Value>> = None;
    let mut queued_prompt: Vec<Value> = Vec::new();

    if session.mid_turn {
        let mut saw_output = false;
        for item in &prep.tail {
            if chat_item_type(item) == "function_call_output" {
                let call_id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let output = item
                    .get("output")
                    .map(|o| match o {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                if let Some(p) = session.parked.remove(&call_id) {
                    p.call.resolve(output);
                    saw_output = true;
                } else if !chat::assistant_side(item) {
                    queued_prompt.push(json!({"type":"text",
                        "text": format!("Tool result for {call_id}: {output}")}));
                }
            } else if !chat::assistant_side(item)
                && let Some(text) = chat::item_to_transcript_pub(item)
            {
                queued_prompt.push(json!({"type": "text", "text": text}));
            }
        }
        let _ = saw_output;
    } else {
        let mut prompt_blocks: Vec<Value> = Vec::new();
        if std::mem::take(&mut session.prior_cut) && !fresh {
            prompt_blocks.push(json!({"type": "text", "text": guard::CUT_NOTE}));
        }
        if fresh && !session.system_in_meta {
            let text = if prep.system.is_empty() {
                format!("System instructions:\n{}", guard::ONE_TURN_RULE)
            } else {
                format!(
                    "System instructions:\n{}\n\n{}",
                    prep.system,
                    guard::ONE_TURN_RULE
                )
            };
            prompt_blocks.push(json!({"type": "text", "text": text}));
        }
        if prep.tail_prompt.is_empty() && prep.outputs.is_empty() && !fresh {
            prompt_blocks.push(json!({"type": "text", "text": "Continue."}));
        } else {
            prompt_blocks.extend(prep.tail_prompt.clone());
        }
        for (call_id, output) in prep.outputs.drain(..) {
            prompt_blocks.push(json!({"type": "text",
                "text": format!("Tool result for {call_id}: {output}")}));
        }
        if prompt_blocks.is_empty() && !session.mid_turn {
            prompt_blocks.push(json!({"type": "text", "text": "Continue."}));
        }
        if !prompt_blocks.is_empty() {
            session.prompt_seq += 1;
            let meta = driver.prompt_meta(session.prompt_seq);
            let (_id, rx) = session
                .client
                .prompt(
                    &session.session_id.clone(),
                    Value::Array(prompt_blocks),
                    meta,
                )
                .map_err(|e| format!("session/prompt: {e}"))?;
            prompt_rx = Some(rx);
            session.mid_turn = true;
        }
    }

    let mut quiesce: Option<Instant> = None;
    let mut text_acc = String::new();
    let mut thought_acc = String::new();
    let flush_text = |result: &mut TurnResult, text_acc: &mut String| {
        if !text_acc.is_empty() {
            result.items.push(OutItem::Message {
                text: std::mem::take(text_acc),
            });
        }
    };
    let flush_thought = |result: &mut TurnResult, thought_acc: &mut String| {
        if !thought_acc.is_empty() {
            result.items.push(OutItem::Reasoning {
                text: std::mem::take(thought_acc),
            });
        }
    };

    let absorbed_mark = session.absorbed.len();
    let emit_mark = result.items.len();
    let _ = (absorbed_mark, emit_mark);

    loop {
        if cancel.load(Ordering::SeqCst) {
            session.client.cancel(&session.session_id);
            session.mid_turn = false;
            fail_parked(session, "interrupted");
            return Err("turn cancelled by client".to_string());
        }

        for pending in session.mcp.drain() {
            if turn_guard.cut().is_some() {
                pending
                    .fail("reply was cut at a fabricated transcript record; this call never ran");
                continue;
            }
            flush_text(&mut result, &mut text_acc);
            let (call_id, name) = (pending.call_id.clone(), pending.name.clone());
            let arguments = pending.arguments.to_string();
            session
                .parked
                .insert(call_id.clone(), Parked { call: pending });
            result.items.push(OutItem::FunctionCall {
                call_id,
                name,
                arguments,
            });
            quiesce = None;
        }

        if !session.parked.is_empty() {
            match quiesce {
                Some(t) if t.elapsed() >= driver.park_quiesce() => {
                    flush_text(&mut result, &mut text_acc);
                    flush_thought(&mut result, &mut thought_acc);
                    result.suspended = true;
                    session.last_used = Instant::now();
                    return Ok(result);
                }
                None => quiesce = Some(Instant::now()),
                _ => {}
            }
        }

        if let Some(rx) = &prompt_rx {
            match rx.try_recv() {
                Ok(msg) => {
                    if let Some(err) = msg.get("error") {
                        session.mid_turn = false;
                        fail_parked(session, "prompt failed");
                        return Err(format!(
                            "session/prompt: {}",
                            err.get("message").and_then(Value::as_str).unwrap_or("?")
                        ));
                    }
                    let stop = msg
                        .get("result")
                        .and_then(|r| r.get("stopReason"))
                        .and_then(Value::as_str)
                        .unwrap_or("end_turn")
                        .to_string();
                    if let Some(u) = msg
                        .get("result")
                        .and_then(|r| r.get("usage"))
                        .and_then(|u| driver.usage(u))
                    {
                        result.usage = Some(u);
                    }
                    result.stop_reason = stop;
                    let _ = prompt_rx.take();
                    if !queued_prompt.is_empty() {
                        session.prompt_seq += 1;
                        let meta = driver.prompt_meta(session.prompt_seq);
                        match session.client.prompt(
                            &session.session_id.clone(),
                            Value::Array(std::mem::take(&mut queued_prompt)),
                            meta,
                        ) {
                            Ok((_id, rx)) => {
                                prompt_rx = Some(rx);
                                continue;
                            }
                            Err(e) => {
                                session.mid_turn = false;
                                fail_parked(session, "prompt failed");
                                return Err(format!("session/prompt: {e}"));
                            }
                        }
                    }
                    flush_text(&mut result, &mut text_acc);
                    flush_thought(&mut result, &mut thought_acc);
                    finish_turn(session, &mut result, prep);
                    return Ok(result);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    prompt_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }

        let wire = session.client.recv(UPDATE_WAIT);
        let Some(w) = wire else { continue };
        match w {
            Wire::Eof => {
                session.mid_turn = false;
                session.dead = true;
                fail_parked(session, "agent exited");
                flush_text(&mut result, &mut text_acc);
                flush_thought(&mut result, &mut thought_acc);
                if result.items.is_empty() {
                    let tail = session.stderr_tail_join();
                    return Err(format!("agent exited mid-turn{tail}"));
                }
                result.suspended = true;
                session.last_used = Instant::now();
                return Ok(result);
            }
            Wire::Bad => {}
            Wire::Notification { method, params } => {
                if method == "session/update" {
                    let cut = handle_update(
                        &params,
                        &mut result,
                        &mut text_acc,
                        &mut thought_acc,
                        &flush_text,
                        &flush_thought,
                        turn_guard,
                    );
                    if let Some(m) = cut {
                        eprintln!(
                            "acp-bridge: reply cut at fabricated {} record (offset {}); {}",
                            guard::marker_name(m),
                            text_acc.len(),
                            if session.parked.is_empty() {
                                "upstream cancelled"
                            } else {
                                "draining (calls parked)"
                            }
                        );
                        session.prior_cut = true;
                        if session.parked.is_empty() {
                            session.client.cancel(&session.session_id);
                        }
                    }
                    if !session.parked.is_empty() {
                        quiesce = Some(Instant::now());
                    }
                } else if driver.prompt_complete_methods().contains(&method.as_str()) {
                    flush_text(&mut result, &mut text_acc);
                    flush_thought(&mut result, &mut thought_acc);
                    finish_turn(session, &mut result, prep);
                    return Ok(result);
                }
            }
            Wire::Request { id, method, params } => {
                handle_agent_request(session, driver, &id, &method, &params);
                quiesce = None;
            }
        }
    }
}

fn finish_turn(session: &mut Session, result: &mut TurnResult, prep: &mut PreparedTurn) {
    session.mid_turn = false;
    session.last_used = Instant::now();
    session.absorbed = prep.input.clone();
    let (call_ids, text) = chat::reply_signature(result);
    session.reply_call_ids = call_ids;
    session.reply_text = text;
    fail_parked(session, "turn ended");
}

fn fail_parked(session: &mut Session, message: &str) {
    for (_, p) in session.parked.drain() {
        p.call.fail(message);
    }
}

impl Session {
    fn stderr_tail_join(&self) -> String {
        let tail = self.client.stderr_tail();
        if tail.is_empty() {
            String::new()
        } else {
            format!(" (stderr: {})", tail.join(" | "))
        }
    }
}

fn handle_agent_request(
    session: &mut Session,
    driver: &dyn Driver,
    id: &Value,
    method: &str,
    params: &Value,
) {
    match method {
        "session/request_permission" => match driver.permission_option(params) {
            PermissionDecision::Allow(option_id) => {
                let _ = session.client.respond(
                    id,
                    json!({"outcome": {"outcome": "selected", "optionId": option_id}}),
                );
            }
            PermissionDecision::Deny => {
                let _ = session
                    .client
                    .respond(id, json!({"outcome": {"outcome": "cancelled"}}));
            }
        },
        "fs/read_text_file"
        | "fs/readTextFile"
        | "fs/write_text_file"
        | "fs/writeTextFile"
        | "terminal/create"
        | "terminal/output"
        | "terminal/release"
        | "terminal/kill"
        | "terminal/wait_for_exit" => {
            let _ = session
                .client
                .respond_error(id, -32601, "client capability not supported");
        }
        other => match driver.ext_request(other, params) {
            Some(result) => {
                let _ = session.client.respond(id, result);
            }
            None => {
                let _ = session
                    .client
                    .respond_error(id, -32601, "unsupported client method");
            }
        },
    }
}

fn handle_update(
    params: &Value,
    result: &mut TurnResult,
    text_acc: &mut String,
    thought_acc: &mut String,
    flush_text: &dyn Fn(&mut TurnResult, &mut String),
    _flush_thought: &dyn Fn(&mut TurnResult, &mut String),
    turn_guard: &mut TurnGuard,
) -> Option<&'static str> {
    let update = params.get("update")?;
    let kind = update
        .get("sessionUpdate")
        .and_then(Value::as_str)
        .unwrap_or("");
    if turn_guard.cut().is_some() && kind != "usage_update" {
        return None;
    }
    match kind {
        "agent_message_chunk" => {
            if let Some(t) = content_text(update.get("content")) {
                if text_acc.is_empty() {
                    turn_guard.new_segment();
                }
                text_acc.push_str(&t);
                return turn_guard.feed(text_acc);
            }
        }
        "agent_thought_chunk" => {
            if let Some(t) = content_text(update.get("content")) {
                thought_acc.push_str(&t);
            }
        }
        "tool_call" | "tool_call_update" => {
            let title = update.get("title").and_then(Value::as_str).unwrap_or("");
            if kind == "tool_call" && !title.is_empty() {
                flush_text(result, text_acc);
                thought_acc.push_str(&format!("[tool: {title}]\n"));
            }
        }
        "usage_update" => {
            if let Some(u) = update.get("usage").or(Some(update))
                && let Some(usage) = chat::Usage::from_acp(u)
            {
                result.usage = Some(usage);
            }
        }
        "plan"
        | "available_commands_update"
        | "current_mode_update"
        | "config_option_update"
        | "session_info_update"
        | "user_message_chunk" => {}
        _ => {}
    }
    None
}

fn content_text(content: Option<&Value>) -> Option<String> {
    let c = content?;
    if c.get("type").and_then(Value::as_str) == Some("text") {
        return c.get("text").and_then(Value::as_str).map(str::to_string);
    }
    None
}
