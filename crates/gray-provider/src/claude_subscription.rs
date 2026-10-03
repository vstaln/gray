//! Claude Pro/Max subscription provider: gray's agent loop drives the official
//! `claude` CLI fully inert, one upstream request per turn. Port of the Hermes
//! DirectSDK transport (NousResearch/hermes-plugin-claude-subscription-directsdk,
//! MIT) minus the HTTP admission relay: with `--tools ''` + `--max-turns` absent
//! (this CLI has no such flag) + `dontAsk`, native makes exactly one request on
//! its own; the relay returns with the in-tree admission module if retries ever
//! double-spend. Gray owns tools, approvals and compaction — Claude only answers.
//!
//! Request shape per turn: a private staging dir (`system.md`, `settings.json`
//! carrying `CLAUDE_CODE_EXTRA_BODY`, inert MCP config), then
//! `claude -p --model <native> --input-format stream-json --output-format
//! stream-json --verbose --include-partial-messages --tools '' ...` with the
//! translated frames on stdin. History frames replay first (`shouldQuery:
//! false`, zero-turn ack each), the final user/tool-result frame queries.
//!
//! Native assistant messages round-trip in `ContentBlock::Thinking`
//! (`item_id: NATIVE_ITEM_ID`, `encrypted_content`: native messages JSON) so the
//! next turn restores byte-identical frames instead of re-derived ones — same
//! carrier trick as the Anthropic thinking replay, gated on same-model.

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;

use futures::stream::{self, BoxStream, StreamExt};
use gray_core::agent::{Provider, ProviderError};
use gray_core::event::{StopReason, StreamEvent, Usage};
use gray_core::message::{ChatRequest, ContentBlock, Role};
use serde_json::{Value, json};
use std::io::BufRead as _;

/// Model-id prefix selecting this provider in `/model` and config.
/// Full ids look like `claude-sub/sonnet`, `claude-sub/opus[1m]`.
pub const MODEL_PREFIX: &str = "claude-sub/";
/// Tool-name prefix native sees (host names restored on the way back).
pub const TOOL_PREFIX: &str = "mcp__gray__";
/// `ReasoningItem::item_id` marking a native-assistant replay carrier: the
/// `encrypted_content` is the response's native assistant `message` objects as
/// a JSON array, so they go back byte for byte.
pub const NATIVE_ITEM_ID: &str = "claude-subscription-native";
/// Carrier version: a mismatch means "re-derive", never "restore stale".
const CARRIER_VERSION: u32 = 1;
/// `claude` is missing (or not on PATH): install hint, never a spawn panic.
const INSTALL_HINT: &str = "`claude` not found on PATH. Install it with \
    `npm install -g @anthropic-ai/claude-code`, then `claude auth login`. \
    Override the binary with CLAUDE_SUB_COMMAND=/path/to/claude.";
/// Pinned native routes: canonical id → context window. A loopback gateway
/// needs explicit long-context selection; unpinned ids run at the 200K native
/// default and are never guessed up to 1M (same invariant as the reference).
fn context_windows() -> &'static [(&'static str, usize)] {
    &[
        ("claude-sonnet-5", 1_000_000),
        ("claude-haiku-4-5-20251001", 200_000),
        ("claude-opus-5-5", 1_000_000),
        ("claude-opus-5", 1_000_000),
        ("claude-opus-4-8", 1_000_000),
        ("claude-fable-5-1", 1_000_000),
    ]
}

fn aliases() -> &'static [(&'static str, &'static str)] {
    &[
        ("sonnet", "claude-sonnet-5"),
        ("haiku", "claude-haiku-4-5-20251001"),
        ("claude-haiku-4-5", "claude-haiku-4-5-20251001"),
        ("opus", "claude-opus-5-5"),
        ("fable", "claude-fable-5-1"),
    ]
}

/// Pinned route ids behind the `claude-sub/` prefix (seed for `/model`).
pub fn pinned_ids() -> &'static [&'static str] {
    &["opus", "sonnet", "haiku", "fable"]
}

/// Context window for a route: pinned table, else the 200K native default.
/// `[1m]`-suffixed ids report `None` (no guess exceeds the 1M native budget).
pub fn context_window(model: &str) -> Option<usize> {
    let base = model.strip_suffix("[1m]").unwrap_or(model);
    let canonical = aliases()
        .iter()
        .find(|(a, _)| *a == base)
        .map(|(_, c)| *c)
        .unwrap_or(base);
    if let Some((_, w)) = context_windows().iter().find(|(id, _)| *id == canonical) {
        return Some(*w);
    }
    if model.ends_with("[1m]") {
        return None;
    }
    Some(200_000)
}

/// Native `--model` selection: pinned 1M ids get `[1m]`, 200K ids go bare
/// (Haiku 4.5 has no 1M route), unpinned ids pass through untouched.
pub fn native_model(model: &str) -> Result<String, ProviderError> {
    let base = model.strip_suffix("[1m]").unwrap_or(model);
    let canonical = aliases()
        .iter()
        .find(|(a, _)| *a == base)
        .map(|(_, c)| *c)
        .unwrap_or(base);
    match context_windows().iter().find(|(id, _)| *id == canonical) {
        Some((_, 1_000_000)) => Ok(format!("{canonical}[1m]")),
        Some((_, 200_000)) => {
            if model.ends_with("[1m]") {
                return Err(ProviderError::BadRequest(
                    "Haiku 4.5 does not support a 1M context window".into(),
                ));
            }
            Ok(canonical.to_string())
        }
        _ => Ok(model.to_string()),
    }
}

/// Resolve the `claude` binary: explicit override, then PATH.
pub fn resolve_command() -> Option<String> {
    let env: Vec<(String, String)> = std::env::vars().collect();
    let lookup = |k: &str| env.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
    resolve_command_for_env(&lookup).or_else(which_claude)
}

#[cfg(test)]
fn resolve_command_for(pairs: &[(&str, &str)]) -> Option<String> {
    let lookup = |k: &str| {
        pairs
            .iter()
            .find(|(a, _)| *a == k)
            .map(|(_, v)| v.to_string())
    };
    resolve_command_for_env(&lookup)
}

fn resolve_command_for_env(lookup: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    for var in [
        "CLAUDE_SUB_COMMAND",
        "CLAUDE_SUBSCRIPTION_DIRECTSDK_COMMAND",
    ] {
        if let Some(v) = lookup(var)
            && !v.is_empty()
        {
            return Some(v);
        }
    }
    None
}

fn which_claude() -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in ["claude", "claude.exe", "claude.cmd"] {
            let p = dir.join(name);
            if p.is_file() {
                return Some(p.to_string_lossy().into_owned());
            }
        }
    }
    None
}

/// Normalize a tool input schema: strip top-level `oneOf`/`allOf`/`anyOf`
/// (Anthropic hard-400s) and guarantee object schemas carry `properties`.
/// Nested unions stay untouched — handlers re-validate their arguments.
pub(crate) fn normalize_input_schema(schema: &Value) -> Value {
    let mut out = schema.clone();
    if let Some(obj) = out.as_object_mut() {
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
    out
}

/// Tool-name validity: unique ASCII identifiers, at most 50 chars.
fn check_tool_name(name: &str, seen: &HashSet<String>) -> Result<(), ProviderError> {
    if name.len() > 50
        || name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        || seen.contains(name)
    {
        return Err(ProviderError::BadRequest(format!(
            "tool names must be unique ASCII identifiers of at most 50 characters: {name:?}"
        )));
    }
    Ok(())
}

/// One translated turn: system text, native history frames, the inert tool
/// manifest, and the host tool names in order.
pub(crate) struct PreparedTurn {
    pub system: String,
    pub frames: Vec<Value>,
    pub names: Vec<String>,
    pub native_model: String,
}

/// Translate a gray request into native history frames. Assistant tool calls
/// go out prefixed; tool results come back as user frames. A same-model
/// native carrier restores byte-identical frames; anything else re-derives.
pub(crate) fn prepare_turn(req: &ChatRequest, model: &str) -> Result<PreparedTurn, ProviderError> {
    let mut system_parts: Vec<String> = Vec::new();
    if let Some(s) = &req.system
        && !s.is_empty()
    {
        system_parts.push(s.clone());
    }
    let mut frames: Vec<Value> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut tool_index: BTreeMap<String, String> = BTreeMap::new();
    for t in &req.tools {
        check_tool_name(&t.name, &seen_names)?;
        seen_names.insert(t.name.clone());
        names.push(t.name.clone());
        tool_index.insert(t.name.clone(), format!("{TOOL_PREFIX}{}", t.name));
    }
    for msg in &req.messages {
        match msg.role {
            Role::System => {
                if !frames.is_empty() {
                    return Err(ProviderError::BadRequest(
                        "system messages must precede conversation history".into(),
                    ));
                }
                let text = msg.text_content();
                system_parts.push(text);
            }
            Role::Assistant => {
                // Signed-carrier replay: only this model's own blocks go back.
                let mut restored = false;
                let carriers: Vec<&ContentBlock> = msg
                    .content
                    .iter()
                    .filter(|b| {
                        matches!(
                            b,
                            ContentBlock::Thinking { item_id: Some(id), model: Some(from), .. }
                            if id == NATIVE_ITEM_ID && from == model
                        )
                    })
                    .collect();
                if carriers.len() == 1
                    && let ContentBlock::Thinking {
                        text,
                        encrypted_content: Some(blob),
                        ..
                    } = carriers[0]
                {
                    // Blob is the full carrier object; the bare array is the
                    // pre-release shape (no version gate — re-derive on doubt).
                    let parsed = serde_json::from_str::<Value>(blob).ok();
                    let saved: Option<Vec<Value>> = match &parsed {
                        Some(Value::Array(natives)) => Some(natives.clone()),
                        Some(obj)
                            if obj.get("type").and_then(Value::as_str) == Some(NATIVE_ITEM_ID)
                                && obj.get("version").and_then(Value::as_u64)
                                    == Some(CARRIER_VERSION as u64) =>
                        {
                            obj.get("messages").and_then(Value::as_array).cloned()
                        }
                        _ => None,
                    };
                    if let Some(saved) = saved {
                        // Host compaction owns visible history: only restore when
                        // the carrier's projection still matches this message.
                        let expected = carrier_projection(text, msg);
                        let actual = live_projection(msg, &tool_index);
                        if expected == actual {
                            for native in saved {
                                frames.push(json!({"type": "assistant", "message": native}));
                            }
                            restored = true;
                        }
                    }
                }
                if !restored {
                    if !carriers.is_empty() && carriers.len() != 1 {
                        return Err(ProviderError::BadRequest(
                            "unsupported native assistant carrier version".into(),
                        ));
                    }
                    let mut blocks: Vec<Value> = Vec::new();
                    for block in &msg.content {
                        match block {
                            ContentBlock::Text { text } => {
                                if !text.is_empty() {
                                    blocks.push(json!({"type": "text", "text": text}));
                                }
                            }
                            ContentBlock::StructuredInput { .. } => {
                                if let Some(text) = block.provider_text()
                                    && !text.is_empty()
                                {
                                    blocks.push(json!({"type": "text", "text": text}));
                                }
                            }
                            ContentBlock::Image { media_type, data } => {
                                blocks.push(json!({"type": "image", "source": {
                                    "type": "base64", "media_type": media_type, "data": data}}));
                            }
                            ContentBlock::Video { .. } => {
                                return Err(crate::openai::video_rejected(model));
                            }
                            ContentBlock::ToolUse { id, name, args } => {
                                let input = if args.is_object() {
                                    args.clone()
                                } else {
                                    json!({})
                                };
                                let prefixed = tool_index
                                    .get(name)
                                    .cloned()
                                    .unwrap_or_else(|| format!("{TOOL_PREFIX}{name}"));
                                blocks.push(json!({"type": "tool_use", "id": id,
                                    "name": prefixed, "input": input}));
                            }
                            ContentBlock::ToolResult { .. } => {
                                return Err(ProviderError::BadRequest(
                                    "assistant messages cannot carry tool results".into(),
                                ));
                            }
                            ContentBlock::Thinking { .. } => {
                                // Foreign/display-only thinking never reaches native.
                            }
                        }
                    }
                    if !blocks.is_empty() {
                        frames.push(json!({"type": "assistant",
                            "message": {"role": "assistant", "content": blocks}}));
                    }
                }
            }
            Role::User => {
                let mut blocks: Vec<Value> = Vec::new();
                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            if !text.is_empty() {
                                blocks.push(json!({"type": "text", "text": text}));
                            }
                        }
                        ContentBlock::StructuredInput { .. } => {
                            if let Some(text) = block.provider_text()
                                && !text.is_empty()
                            {
                                blocks.push(json!({"type": "text", "text": text}));
                            }
                        }
                        ContentBlock::Image { media_type, data } => {
                            blocks.push(json!({"type": "image", "source": {
                                "type": "base64", "media_type": media_type, "data": data}}));
                        }
                        ContentBlock::Video { .. } => {
                            return Err(crate::openai::video_rejected(model));
                        }
                        ContentBlock::ToolResult {
                            id,
                            content,
                            is_error,
                        } => {
                            let text = crate::openai::wire_tool_output(content, *is_error);
                            let text = if text.is_empty() {
                                "(no output)".to_string()
                            } else {
                                text
                            };
                            blocks.push(json!({"type": "tool_result",
                                "tool_use_id": id, "content": text, "is_error": is_error}));
                        }
                        ContentBlock::ToolUse { .. } | ContentBlock::Thinking { .. } => {
                            return Err(ProviderError::BadRequest(
                                "user messages cannot carry tool calls or thinking".into(),
                            ));
                        }
                    }
                }
                if blocks.is_empty() {
                    continue;
                }
                if let Some(last) = frames.last_mut()
                    && last.get("type").and_then(Value::as_str) == Some("user")
                    && let Some(content) = last
                        .pointer_mut("/message/content")
                        .and_then(Value::as_array_mut)
                {
                    content.extend(blocks);
                    continue;
                }
                frames.push(json!({"type": "user",
                    "message": {"role": "user", "content": blocks}}));
            }
        }
    }
    let Some(last) = frames.last() else {
        return Err(ProviderError::BadRequest(
            "history must end in a nonempty user/tool-result message".into(),
        ));
    };
    if last.get("type").and_then(Value::as_str) != Some("user")
        || last
            .pointer("/message/content")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
    {
        return Err(ProviderError::BadRequest(
            "history must end in a nonempty user/tool-result message; assistant prefill is unsupported".into(),
        ));
    }
    Ok(PreparedTurn {
        system: system_parts.join("\n\n"),
        frames,
        names,
        native_model: native_model(model)?,
    })
}

/// Projection of the visible message the carrier must still match: stripped
/// text + tool calls (host names). A mismatch means compaction or hooks
/// rewrote history — re-derive, never restore stale signed blocks.
fn live_projection(
    msg: &gray_core::message::Message,
    prefixed: &BTreeMap<String, String>,
) -> Value {
    let mut text = String::new();
    let mut calls: Vec<Value> = Vec::new();
    for b in &msg.content {
        match b {
            ContentBlock::Text { text: t } => text.push_str(t),
            ContentBlock::ToolUse { id, name, args } => {
                let prefixed_name = prefixed.get(name).cloned().unwrap_or_else(|| name.clone());
                calls.push(json!({"id": id, "name": prefixed_name, "input": args}));
            }
            _ => {}
        }
    }
    json!({"content": text.trim(), "tool_calls": calls})
}

/// Projection the carrier stored at capture time: same shape as
/// `live_projection`, with the saved prose text.
fn carrier_projection(saved_text: &str, msg: &gray_core::message::Message) -> Value {
    let mut calls: Vec<Value> = Vec::new();
    for b in &msg.content {
        if let ContentBlock::ToolUse { id, name, args } = b {
            calls.push(json!({"id": id, "name": name, "input": args}));
        }
    }
    json!({"content": saved_text.trim(), "tool_calls": calls})
}

/// Map native `result` usage onto gray's inclusive `Usage`.
pub(crate) fn map_usage(u: &Value) -> Usage {
    let input = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0) as usize;
    let output = u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0) as usize;
    let read = u
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let write = u
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let thinking = u
        .pointer("/output_tokens_details/thinking_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    Usage {
        input_tokens: input.saturating_add(read).saturating_add(write),
        output_tokens: output,
        reasoning_tokens: thinking.min(output),
        cached_tokens: read,
        non_cached_input_tokens: input,
        cache_read_input_tokens: read,
        cache_write_input_tokens: write,
        total_tokens: input
            .saturating_add(read)
            .saturating_add(write)
            .saturating_add(output),
    }
}

/// Subscription provider: spawns the `claude` CLI per turn, fully inert.
#[derive(Clone)]
pub struct ClaudeSubscriptionProvider {
    command: Option<String>,
    model: String,
    reasoning_effort: Option<String>,
}

impl std::fmt::Debug for ClaudeSubscriptionProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeSubscriptionProvider")
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl ClaudeSubscriptionProvider {
    pub fn new(
        model: impl Into<String>,
        reasoning_effort: Option<String>,
        command: Option<String>,
    ) -> Result<Self, String> {
        Ok(Self {
            command,
            model: model.into(),
            reasoning_effort,
        })
    }

    /// The native route id behind the `claude-sub/` prefix (what `native_model`
    /// resolves to `--model`).
    pub fn native_model_id(&self) -> &str {
        &self.model
    }

    fn claude_binary(&self) -> Result<String, ProviderError> {
        if let Some(c) = &self.command
            && !c.is_empty()
        {
            return Ok(c.clone());
        }
        resolve_command().ok_or_else(|| ProviderError::Connection(INSTALL_HINT.into()))
    }
}

struct Collector {
    text: String,
    thinking: String,
    calls: Vec<(String, String, String)>,
    usage: Usage,
    stop: StopReason,
    natives: Vec<Value>,
}

impl Default for Collector {
    fn default() -> Self {
        Self {
            text: String::new(),
            thinking: String::new(),
            calls: Vec::new(),
            usage: Usage::default(),
            stop: StopReason::EndTurn,
            natives: Vec::new(),
        }
    }
}

/// Feed one native stream-json line; returns a stream event to forward, if any.
/// `names` is the host tool inventory: a tool outside it is a hard error.
fn feed_line(
    line: &Value,
    names: &[String],
    out: &mut Vec<StreamEvent>,
    col: &mut Collector,
) -> Result<(), ProviderError> {
    let kind = line.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "assistant" => {
            if line.get("error").is_some() && line.get("error") != Some(&Value::Null)
                || line.pointer("/message/error").is_some()
            {
                let detail = line
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter_map(|b| b.get("text").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                if line.get("error").and_then(Value::as_str) == Some("authentication_failed") {
                    return Err(ProviderError::Auth(format!(
                        "Claude Code has no usable login here; run `claude auth login` (native: {detail})"
                    )));
                }
                return Err(ProviderError::ServerError(format!(
                    "native API error: {detail}"
                )));
            }
            if let Some(msg) = line.get("message") {
                col.natives.push(msg.clone());
                // Incremental parity: surface native text/thinking as it lands
                // so the turn streams instead of popping in at the end.
                if let Some(blocks) = msg.get("content").and_then(Value::as_array) {
                    for b in blocks {
                        match b.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                if let Some(t) = b.get("text").and_then(Value::as_str) {
                                    col.text.push_str(t);
                                    out.push(StreamEvent::text_delta(t));
                                }
                            }
                            Some("thinking") => {
                                if let Some(t) = b.get("thinking").and_then(Value::as_str) {
                                    col.thinking.push_str(t);
                                    out.push(StreamEvent::thinking_delta(t));
                                }
                            }
                            Some("tool_use") => {
                                let id = b
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string();
                                let full = b.get("name").and_then(Value::as_str).unwrap_or("");
                                let Some(short) = full.strip_prefix(TOOL_PREFIX) else {
                                    return Err(ProviderError::BadRequest(format!(
                                        "native returned a tool outside the current host inventory: {full:?}"
                                    )));
                                };
                                if !names.contains(&short.to_string()) {
                                    return Err(ProviderError::BadRequest(format!(
                                        "native returned a tool outside the current host inventory: {full:?}"
                                    )));
                                }
                                let input = b.get("input").cloned().unwrap_or(json!({}));
                                let args =
                                    serde_json::to_string(&input).unwrap_or_else(|_| "{}".into());
                                col.calls
                                    .push((id.clone(), short.to_string(), args.clone()));
                                out.push(StreamEvent::tool_call_delta(
                                    col.calls.len() - 1,
                                    Some(id),
                                    Some(short.to_string()),
                                    args,
                                ));
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        "stream_event" => {
            // Incremental text/thinking already surfaced from the `assistant`
            // envelope above; partial-message deltas here would double-emit
            // (reference reconciles emitted-vs-final instead). Track stop only.
            let ev = line.get("event").cloned().unwrap_or(Value::Null);
            if ev.get("type").and_then(Value::as_str) == Some("message_stop") {
                // Assistant + message_stop + one result required at finalize.
            }
        }
        "result" => {
            if let Some(u) = line.get("usage") {
                col.usage = map_usage(u);
            }
            let subtype = line.get("subtype").and_then(Value::as_str).unwrap_or("");
            let is_error = line
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if is_error && subtype != "success" {
                let detail = line
                    .get("result")
                    .and_then(Value::as_str)
                    .unwrap_or(subtype);
                // error_max_turns with tool calls is the tool boundary, not a
                // failure — but this CLI has no --max-turns flag, so native
                // never stops at one turn for us; treat any error result hard.
                return Err(ProviderError::ServerError(format!(
                    "native request failed: {detail}"
                )));
            }
            match subtype {
                "success" => col.stop = StopReason::EndTurn,
                _ if !col.calls.is_empty() => col.stop = StopReason::ToolUse,
                _ => col.stop = StopReason::EndTurn,
            }
            for a in &col.natives {
                let sr = a.get("stop_reason").and_then(Value::as_str).unwrap_or("");
                if sr == "max_tokens" || sr == "model_context_window_exceeded" {
                    col.stop = StopReason::MaxTokens;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn run_turn(
    provider: ClaudeSubscriptionProvider,
    turn: PreparedTurn,
    req: ChatRequest,
    effort: Option<String>,
) -> BoxStream<'static, Result<StreamEvent, ProviderError>> {
    // Fail fast before spawning: conflicting auth env would forward the
    // subscription bearer somewhere else, or confuse native backend routing.
    for key in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_FOUNDRY_API_KEY",
    ] {
        if std::env::var_os(key).is_some() {
            let msg = format!(
                "subscription provider refuses conflicting {key}: unset it so native uses your Claude login"
            );
            return stream::once(async move { Err(ProviderError::Auth(msg)) }).boxed();
        }
    }
    for key in [
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
    ] {
        if let Some(v) = std::env::var_os(key)
            && !matches!(
                v.to_string_lossy().to_lowercase().as_str(),
                "" | "0" | "false" | "no" | "off"
            )
        {
            let msg = format!(
                "subscription provider refuses conflicting {key}: unset it so native uses your Claude login"
            );
            return stream::once(async move { Err(ProviderError::Auth(msg)) }).boxed();
        }
    }
    let binary = match provider.claude_binary() {
        Ok(b) => b,
        Err(e) => return stream::once(async move { Err(e) }).boxed(),
    };
    // Per-request staging only; native runs in the real cwd (its env block
    // carries the cwd into the prompt-cache prefix, so a stable cwd keeps
    // the cache warm across turns).
    let stage = match tempfile::Builder::new().prefix("claude-sub-").tempdir() {
        Ok(d) => d,
        Err(e) => {
            return stream::once(async move {
                Err(ProviderError::Connection(format!("staging dir: {e}")))
            })
            .boxed();
        }
    };
    let stage_path = stage.path().to_path_buf();
    let extra_body = json!({
        "tools": turn.names.iter().map(|n| {
            let t = req.tools.iter().find(|t| &t.name == n);
            json!({"name": format!("{TOOL_PREFIX}{n}"),
                "description": t.map(|t| t.description.as_str()).unwrap_or(""),
                "input_schema": t.map(|t| normalize_input_schema(&t.parameters)).unwrap_or(json!({}))})
        }).collect::<Vec<_>>(),
    });
    let write_file = |name: &str, content: &str| -> Result<PathBuf, ProviderError> {
        let p = stage_path.join(name);
        std::fs::write(&p, content)
            .map_err(|e| ProviderError::Connection(format!("staging {name}: {e}")))?;
        Ok(p)
    };
    let settings_path = match write_file(
        "settings.json",
        &json!({"env": {"CLAUDE_CODE_EXTRA_BODY": extra_body.to_string()}}).to_string(),
    ) {
        Ok(p) => p,
        Err(e) => return stream::once(async move { Err(e) }).boxed(),
    };
    let mcp_config = json!({"mcpServers": {}}).to_string();
    let mut argv: Vec<String> = vec![
        "-p".into(),
        "--model".into(),
        turn.native_model.clone(),
        "--input-format".into(),
        "stream-json".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--include-partial-messages".into(),
        "--tools".into(),
        String::new(),
        "--system-prompt".into(),
        turn.system.clone(),
        "--settings".into(),
        settings_path.to_string_lossy().into_owned(),
        "--setting-sources".into(),
        String::new(),
        "--strict-mcp-config".into(),
        "--disable-slash-commands".into(),
        "--permission-mode".into(),
        "dontAsk".into(),
        "--no-session-persistence".into(),
        "--mcp-config".into(),
        mcp_config,
    ];
    if let Some(e) = effort {
        argv.push("--effort".into());
        argv.push(e);
    }
    // Blocking spawn would stall the loop: run the whole turn on a worker.
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<StreamEvent, ProviderError>>();
    let _handle = tokio::task::spawn_blocking(move || {
        let _stage = stage;
        let mut child = match std::process::Command::new(&binary)
            .args(&argv)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env("ENABLE_TOOL_SEARCH", "false")
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .env("DISABLE_AUTO_COMPACT", "1")
            .env("DISABLE_COMPACT", "1")
            .env("CLAUDE_CODE_TOTAL_TOKENS_REMINDER", "off")
            .spawn()
        {
            Ok(c) => c,
            Err(_) => {
                let _ = tx.send(Err(ProviderError::Connection(INSTALL_HINT.into())));
                return;
            }
        };
        // History replay first (no-query frames, one line each), then the
        // final user/tool-result frame, then close stdin to query.
        let frames = turn.frames;
        let mut stdin = match child.stdin.take() {
            Some(s) => s,
            None => {
                let _ = tx.send(Err(ProviderError::Connection(
                    "native stdin unavailable".into(),
                )));
                return;
            }
        };
        for (i, frame) in frames.iter().enumerate() {
            let mut f = frame.clone();
            if frame.get("type").and_then(Value::as_str) == Some("user") && i + 1 < frames.len() {
                f["shouldQuery"] = json!(false);
            }
            let line = match serde_json::to_string(&f) {
                Ok(l) => l + "\n",
                Err(e) => {
                    let _ = tx.send(Err(ProviderError::BadRequest(format!("frame encode: {e}"))));
                    return;
                }
            };
            if stdin.write_all(line.as_bytes()).is_err() {
                let _ = tx.send(Err(ProviderError::Connection("native stdin closed".into())));
                return;
            }
        }
        drop(stdin);
        let stdout = match child.stdout.take() {
            Some(s) => s,
            None => {
                let _ = tx.send(Err(ProviderError::Connection(
                    "native stdout unavailable".into(),
                )));
                return;
            }
        };
        let reader = std::io::BufReader::new(stdout);
        let mut col = Collector::default();
        let mut saw_result = false;
        for line in reader.lines().map_while(Result::ok) {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            let v: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => {
                    let _ = tx.send(Err(ProviderError::Stream(format!(
                        "invalid native stream-json output: {:?}",
                        line.chars().take(300).collect::<String>()
                    ))));
                    return;
                }
            };
            if v.get("type").and_then(Value::as_str) == Some("result") {
                saw_result = true;
            }
            let mut out: Vec<StreamEvent> = Vec::new();
            if let Err(e) = feed_line(&v, &turn.names, &mut out, &mut col) {
                let _ = tx.send(Err(e));
                return;
            }
            for ev in out {
                if tx.send(Ok(ev)).is_err() {
                    return;
                }
            }
        }
        let status = child.wait();
        let ok = matches!(status, Ok(s) if s.success());
        if !saw_result || col.natives.is_empty() {
            let _ = tx.send(Err(ProviderError::Stream(
                "incomplete native response: assistant and one result required".into(),
            )));
            return;
        }
        if !ok {
            // Nonzero exit with tool calls is the tool boundary only when the
            // result said so; anything else is a native failure.
            let _ = tx.send(Err(ProviderError::ServerError(
                "native request failed (nonzero exit without a success result)".into(),
            )));
            return;
        }
        // Finalize: tool calls one delta each were already emitted; attach the
        // signed carrier so the next turn restores byte-identical frames.
        // The blob is the full carrier object (type + version + messages +
        // projection); restore accepts the legacy bare array too.
        let carrier = json!({"type": NATIVE_ITEM_ID, "version": CARRIER_VERSION,
            "messages": col.natives,
            "projection": {"content": col.text.trim(),
                "tool_calls": col.calls.iter().map(|(id, name, args)| {
                    let input: Value = serde_json::from_str(args).unwrap_or(json!({}));
                    json!({"id": id, "name": format!("{TOOL_PREFIX}{name}"), "input": input})
                }).collect::<Vec<_>>()
            }
        });
        let _ = tx.send(Ok(StreamEvent::ReasoningItem {
            item_id: NATIVE_ITEM_ID.to_string(),
            encrypted_content: carrier.to_string(),
        }));
        let _ = tx.send(Ok(StreamEvent::MessageComplete {
            stop_reason: Some(if col.calls.is_empty() {
                col.stop
            } else {
                StopReason::ToolUse
            }),
            usage: Some(col.usage),
        }));
    });
    // Bridge the worker channel onto the provider stream: each step polls one
    // worker send; the worker drops the sender when the turn ends.
    futures::stream::unfold(rx, |mut rx| async move {
        let ev = rx.recv().await?;
        Some((ev, rx))
    })
    .boxed()
}

impl Provider for ClaudeSubscriptionProvider {
    fn model_id(&self) -> &str {
        &self.model
    }

    fn stream(&self, req: ChatRequest) -> BoxStream<'static, Result<StreamEvent, ProviderError>> {
        let model = self.model.clone();
        // Sampling controls are rejected by subscription models; gray's caller
        // defaults never reach native. Effort maps to --effort.
        let effort = self.reasoning_effort.clone().filter(|e| e != "off");
        let turn = match prepare_turn(&req, &model) {
            Ok(t) => t,
            Err(e) => return stream::once(async move { Err(e) }).boxed(),
        };
        let provider = self.clone();
        run_turn_sync(provider, turn, req, effort)
    }
}

fn run_turn_sync(
    provider: ClaudeSubscriptionProvider,
    turn: PreparedTurn,
    req: ChatRequest,
    effort: Option<String>,
) -> BoxStream<'static, Result<StreamEvent, ProviderError>> {
    // The worker owns the whole turn; no await inside, so no block_on
    // (which would panic on a Tokio worker). Sync all the way down.
    run_turn(provider, turn, req, effort)
}

#[path = "claude_subscription_tests.rs"]
#[cfg(test)]
mod tests;
