//! Tests for the Claude subscription provider: catalog routing, history
//! translation, schema normalization, and the native stream-json contract
//! against a fake `claude` executable (mirrors the reference FAKE).

use super::*;
use gray_core::message::{Message, ToolDef};

// ---------------------------------------------------------------------------
// catalog
// ---------------------------------------------------------------------------

#[test]
fn pinned_routes_get_windows_and_1m_selection() {
    assert_eq!(context_window("claude-opus-5-5"), Some(1_000_000));
    assert_eq!(context_window("claude-sonnet-5"), Some(1_000_000));
    assert_eq!(context_window("claude-haiku-4-5-20251001"), Some(200_000));
    assert_eq!(native_model("opus").unwrap(), "claude-opus-5-5[1m]");
    assert_eq!(native_model("sonnet").unwrap(), "claude-sonnet-5[1m]");
    assert_eq!(native_model("haiku").unwrap(), "claude-haiku-4-5-20251001");
    // Unpinned: plain id at the 200K native default, never a guessed 1M.
    assert_eq!(context_window("unqualified-future-model"), Some(200_000));
    assert_eq!(context_window("unqualified-future-model[1m]"), None);
    assert_eq!(
        native_model("unqualified-future-model").unwrap(),
        "unqualified-future-model"
    );
}

#[test]
fn haiku_rejects_1m() {
    assert!(native_model("claude-haiku-4-5-20251001[1m]").is_err());
}

#[test]
fn resolve_prefers_override_then_path() {
    // Scoped env without touching the process environment.
    assert_eq!(
        resolve_command_for(&[("CLAUDE_SUB_COMMAND", "/tmp/fake-claude-test")]).as_deref(),
        Some("/tmp/fake-claude-test")
    );
}

// ---------------------------------------------------------------------------
// translation
// ---------------------------------------------------------------------------

fn req() -> ChatRequest {
    ChatRequest {
        system: Some("sys".into()),
        messages: vec![Message::user("hi")],
        tools: vec![ToolDef::new(
            "probe",
            "p",
            json!({"type": "object", "properties": {"v": {"type": "string"}}}),
        )],
        max_tokens: None,
    }
}

#[test]
fn prepare_turn_shapes_frames_and_manifest() {
    let turn = prepare_turn(&req(), "sonnet").unwrap();
    assert_eq!(turn.system, "sys");
    assert_eq!(turn.native_model, "claude-sonnet-5[1m]");
    assert_eq!(turn.names, vec!["probe"]);
    assert_eq!(turn.frames.len(), 1);
    assert_eq!(turn.frames[0]["type"], "user");
    // The tool manifest now lives in CLAUDE_CODE_EXTRA_BODY at spawn time
    // (built from turn.names + req.tools); the turn carries names only.
    assert_eq!(turn.names, vec!["probe"]);
}

#[test]
fn tool_results_become_user_frames_and_calls_prefix() {
    let r = ChatRequest {
        system: None,
        messages: vec![
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::tool_use("c1", "probe", json!({"v": "x"}))],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    id: "c1".into(),
                    content: "out".into(),
                    is_error: false,
                }],
            },
        ],
        tools: vec![ToolDef::new("probe", "p", json!({"type": "object"}))],
        max_tokens: None,
    };
    let turn = prepare_turn(&r, "sonnet").unwrap();
    assert_eq!(turn.frames.len(), 2);
    assert_eq!(
        turn.frames[0]["message"]["content"][0]["name"],
        "mcp__gray__probe"
    );
    assert_eq!(turn.frames[1]["message"]["content"][0]["tool_use_id"], "c1");
}

#[test]
fn user_tool_use_is_rejected() {
    let r = ChatRequest {
        system: None,
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::tool_use("c1", "probe", json!({}))],
        }],
        tools: vec![],
        max_tokens: None,
    };
    assert!(prepare_turn(&r, "sonnet").is_err());
}

#[test]
fn empty_history_is_rejected() {
    let r = ChatRequest {
        system: None,
        messages: vec![],
        tools: vec![],
        max_tokens: None,
    };
    assert!(prepare_turn(&r, "sonnet").is_err());
}

#[test]
fn assistant_prefill_is_rejected() {
    let r = ChatRequest {
        system: None,
        messages: vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::text("prefill")],
        }],
        tools: vec![],
        max_tokens: None,
    };
    assert!(prepare_turn(&r, "sonnet").is_err());
}

#[test]
fn duplicate_tool_names_rejected() {
    let r = ChatRequest {
        system: None,
        messages: vec![Message::user("hi")],
        tools: vec![
            ToolDef::new("a", "x", json!({"type": "object"})),
            ToolDef::new("a", "y", json!({"type": "object"})),
        ],
        max_tokens: None,
    };
    assert!(prepare_turn(&r, "sonnet").is_err());
}

#[test]
fn normalize_strips_combinators_and_repairs_object() {
    let v = normalize_input_schema(&json!({"oneOf": [{"type": "string"}], "description": "x"}));
    assert!(v.get("oneOf").is_none());
    assert_eq!(v["type"], "object");
    assert_eq!(v["properties"], json!({}));
}

// ---------------------------------------------------------------------------
// native stream-json contract (fake claude)
// ---------------------------------------------------------------------------

/// Minimal fake: replays one assistant text + result with usage, acking
/// history frames with zero-turn results. Asserts the inert spawn shape.
const FAKE: &str = r#"#!/usr/bin/env python3
import json, sys
argv = sys.argv[1:]
assert "--tools" in argv and argv[argv.index("--tools") + 1] == "", argv
assert "--disable-slash-commands" in argv
assert "--no-session-persistence" in argv
assert "--strict-mcp-config" in argv
assert "--setting-sources" in argv and argv[argv.index("--setting-sources") + 1] == ""
assert "--permission-mode" in argv and argv[argv.index("--permission-mode") + 1] == "dontAsk"
rows = [json.loads(l) for l in sys.stdin if l.strip()]
for r in rows[:-1]:
    sys.stdout.write(json.dumps({"type": "result", "num_turns": 0, "is_error": False}) + "\n")
    sys.stdout.flush()
n = len(rows)
sys.stdout.write(json.dumps({"type": "assistant", "message": {"role": "assistant",
    "id": "msg_fake", "model": "sonnet", "stop_reason": "end_turn",
    "content": [{"type": "text", "text": "hello"}]}}) + "\n")
sys.stdout.write(json.dumps({"type": "stream_event", "event": {"type": "message_stop"}}) + "\n")
res = {"type": "result", "num_turns": n, "subtype": "success", "is_error": False}
res["usage"] = {"input_tokens": 10, "output_tokens": 3}
res["usage"]["cache_read_input_tokens"] = 4
res["usage"]["cache_creation_input_tokens"] = 2
res["total_cost_usd"] = 0.001
res["modelUsage"] = {}
sys.stdout.write(json.dumps(res) + "\n")
sys.stdout.flush()
"#;

fn fake_provider(dir: &std::path::Path) -> ClaudeSubscriptionProvider {
    let bin = dir.join("claude");
    std::fs::write(&bin, FAKE).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    ClaudeSubscriptionProvider::new("sonnet", None, Some(bin.to_string_lossy().into_owned()))
        .unwrap()
}

#[tokio::test]
async fn live_turn_streams_text_and_completes_with_usage() {
    let dir = tempfile::Builder::new()
        .prefix("claude-sub-test-")
        .tempdir()
        .unwrap();
    let provider = fake_provider(dir.path());
    let events: Vec<_> = provider.stream(req()).collect().await;
    assert!(!events.is_empty());
    let texts: String = events
        .iter()
        .filter_map(|e| match e {
            Ok(StreamEvent::TextDelta { delta }) => Some(delta.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, "hello");
    let complete = events.iter().find_map(|e| match e {
        Ok(StreamEvent::MessageComplete { usage, .. }) => *usage,
        _ => None,
    });
    let usage = complete.expect("MessageComplete with usage");
    assert_eq!(usage.output_tokens, 3);
    assert_eq!(usage.cache_read_input_tokens, 4);
    assert_eq!(usage.cache_write_input_tokens, 2);
    assert_eq!(usage.input_tokens, 16);
    // Signed carrier for next-turn replay.
    assert!(events.iter().any(|e| matches!(
        e,
        Ok(StreamEvent::ReasoningItem { item_id, .. }) if item_id == NATIVE_ITEM_ID
    )));
}

#[test]
fn carrier_object_restores_but_bare_array_still_does() {
    // Build a two-turn history: assistant text + carrier, then user follow-up.
    let natives = vec![json!({"role": "assistant", "id": "msg_native",
        "content": [{"type": "text", "text": "hello"}]})];
    let carrier_obj = json!({"type": NATIVE_ITEM_ID, "version": 1,
        "messages": natives,
        "projection": {"content": "hello", "tool_calls": []}})
    .to_string();
    let mk_req = |blob: String| ChatRequest {
        system: None,
        messages: vec![
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::text("hello"),
                    ContentBlock::Thinking {
                        text: "hello".into(),
                        encrypted_content: Some(blob),
                        item_id: Some(NATIVE_ITEM_ID.into()),
                        model: Some("sonnet".into()),
                    },
                ],
            },
            Message::user("again"),
        ],
        tools: vec![],
        max_tokens: None,
    };
    // Restore keeps the byte-identical native envelope (id survives);
    // re-derive rebuilds {role, content} with no id.
    let turn = prepare_turn(&mk_req(carrier_obj), "sonnet").unwrap();
    assert_eq!(turn.frames.len(), 2);
    assert_eq!(turn.frames[0]["message"]["id"], "msg_native");
    // Legacy bare array still restores.
    let legacy = serde_json::to_string(&natives).unwrap();
    let turn = prepare_turn(&mk_req(legacy), "sonnet").unwrap();
    assert_eq!(turn.frames.len(), 2);
    assert_eq!(turn.frames[0]["message"]["id"], "msg_native");
    // Wrong version re-derives instead of restoring.
    let bad = json!({"type": NATIVE_ITEM_ID, "version": 999, "messages": natives,
        "projection": {"content": "hello", "tool_calls": []}})
    .to_string();
    let turn = prepare_turn(&mk_req(bad), "sonnet").unwrap();
    assert_eq!(turn.frames.len(), 2);
    assert!(turn.frames[0]["message"].get("id").is_none());
}

#[tokio::test]
async fn missing_binary_is_a_clean_connection_error() {
    let provider =
        ClaudeSubscriptionProvider::new("sonnet", None, Some("/nonexistent/claude-xyz".into()))
            .unwrap();
    let events: Vec<_> = provider.stream(req()).collect().await;
    assert!(matches!(
        events.last(),
        Some(Err(ProviderError::Connection(_)))
    ));
}
