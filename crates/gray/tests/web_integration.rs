use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gray::config::Config;
use gray::web::{create_router, AppState, ChatRequestPayload};
use gray_core::agent::{Agent, Provider, ProviderStream};
use gray_core::event::{AgentEvent, StopReason, StreamEvent, Usage};
use gray_core::message::{ChatRequest, Message};
use gray_session::{JsonlSessionStore, SessionEntry, SessionSummary};
use gray_tools::Registry;

#[derive(Clone)]
struct ScriptedProvider {
    scripts: Arc<Mutex<VecDeque<Vec<StreamEvent>>>>,
}

impl ScriptedProvider {
    fn new(scripts: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            scripts: Arc::new(Mutex::new(VecDeque::from(scripts))),
        }
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn stream(&self, _req: ChatRequest) -> ProviderStream {
        let script = self
            .scripts
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_default();
        Box::pin(futures::stream::iter(script.into_iter().map(Ok)))
    }
}

fn test_config() -> Config {
    Config {
        model: "test-provider/test-model".to_string(),
        base_url: "https://api.test.com/v1".to_string(),
        api_key: "test-key-123".to_string(),
        port: 0,
    }
}

#[tokio::test]
async fn test_web_chat_sse_and_sessions_lifecycle() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(JsonlSessionStore::new(tmp.path()));
    let config = test_config();
    let test_file = tmp.path().join("test_write.txt");
    let test_file_str = test_file.to_str().unwrap().to_string();

    let provider = ScriptedProvider::new(vec![
        // Turn 1: single text reply
        vec![
            StreamEvent::text_delta("Hello from "),
            StreamEvent::text_delta("the fake agent!"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), Some(Usage::new(15, 8))),
        ],
        // Turn 2: tool execution turn
        vec![
            StreamEvent::tool_call_delta(
                0,
                Some("call_write_1".to_string()),
                Some("write".to_string()),
                &format!(r#"{{"path":"{}","#, test_file_str),
            ),
            StreamEvent::tool_call_delta(
                0,
                None,
                None,
                r#""content":"hello file"}"#,
            ),
            StreamEvent::message_complete(Some(StopReason::ToolUse), Some(Usage::new(20, 10))),
        ],
        // Turn 2 continuation after tool result
        vec![
            StreamEvent::text_delta("File created successfully."),
            StreamEvent::message_complete(Some(StopReason::EndTurn), Some(Usage::new(25, 5))),
        ],
    ]);

    let provider_clone = provider.clone();
    let agent_factory = Arc::new(move |_cwd: &std::path::Path| {
        let p = provider_clone.clone();
        let registry = Registry::builtin();
        let tool_defs = registry.defs();
        let agent = Agent::new(Box::new(p), Box::new(registry))
            .with_tools(tool_defs);
        Ok(agent)
    });

    let state = AppState {
        config: config.clone(),
        store: store.clone(),
        agent_factory,
    };

    let router = create_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let client = reqwest::Client::new();
    let base_url = format!("http://{}", addr);

    // 1. Assert GET / serves index.html
    let res_index = client.get(&base_url).send().await.unwrap();
    assert_eq!(res_index.status(), 200);
    assert_eq!(
        res_index.headers().get("content-type").unwrap(),
        "text/html; charset=utf-8"
    );
    let html = res_index.text().await.unwrap();
    assert!(html.contains("<title>gray</title>"));
    assert!(html.contains("id=\"composer-form\""));

    // 2. Assert GET /main.js serves javascript
    let res_js = client.get(format!("{}/main.js", base_url)).send().await.unwrap();
    assert_eq!(res_js.status(), 200);
    assert_eq!(
        res_js.headers().get("content-type").unwrap(),
        "application/javascript; charset=utf-8"
    );
    let js = res_js.text().await.unwrap();
    assert!(js.contains("sendMessage"));

    // 3. Assert GET /api/config returns model info
    let res_cfg = client.get(format!("{}/api/config", base_url)).send().await.unwrap();
    assert_eq!(res_cfg.status(), 200);
    let cfg_json: serde_json::Value = res_cfg.json().await.unwrap();
    assert_eq!(cfg_json["model"], "test-provider/test-model");

    // 4. Assert POST /api/chat without session_id (creates session & streams SSE)
    let payload = ChatRequestPayload {
        session_id: None,
        message: "Hi there".to_string(),
    };
    let res_chat = client
        .post(format!("{}/api/chat", base_url))
        .json(&payload)
        .send()
        .await
        .unwrap();

    assert_eq!(res_chat.status(), 200);
    assert!(
        res_chat
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );

    let session_id_str = res_chat
        .headers()
        .get("x-session-id")
        .expect("x-session-id header missing")
        .to_str()
        .unwrap()
        .to_string();
    assert!(!session_id_str.is_empty());

    let stream_body = res_chat.text().await.unwrap();
    let mut events = Vec::new();
    for line in stream_body.lines() {
        if let Some(data_str) = line.strip_prefix("data: ") {
            let data_str = data_str.trim();
            if !data_str.is_empty() && data_str != "[DONE]" {
                let ev: AgentEvent = serde_json::from_str(data_str)
                    .unwrap_or_else(|e| panic!("failed to parse AgentEvent from '{data_str}': {e}"));
                events.push(ev);
            }
        }
    }

    assert_eq!(
        events,
        vec![
            AgentEvent::Start,
            AgentEvent::text_delta("Hello from "),
            AgentEvent::text_delta("the fake agent!"),
            AgentEvent::turn_end(StopReason::EndTurn, Usage::new(15, 8)),
        ]
    );

    // 5. Assert GET /api/sessions lists the created session
    let res_sessions = client.get(format!("{}/api/sessions", base_url)).send().await.unwrap();
    assert_eq!(res_sessions.status(), 200);
    let sessions: Vec<SessionSummary> = res_sessions.json().await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id.as_str(), session_id_str);
    assert_eq!(sessions[0].first_user_text, Some("Hi there".to_string()));

    // 6. Assert GET /api/sessions/:id returns replayed messages
    let res_detail = client
        .get(format!("{}/api/sessions/{}", base_url, session_id_str))
        .send()
        .await
        .unwrap();
    assert_eq!(res_detail.status(), 200);
    let entries: Vec<SessionEntry> = res_detail.json().await.unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].message, Message::user("Hi there"));
    assert_eq!(
        entries[1].message,
        Message::assistant("Hello from the fake agent!")
    );

    // 7. Assert POST /api/chat resuming existing session with tool calls
    let payload2 = ChatRequestPayload {
        session_id: Some(session_id_str.clone()),
        message: "Create test file".to_string(),
    };
    let res_chat2 = client
        .post(format!("{}/api/chat", base_url))
        .json(&payload2)
        .send()
        .await
        .unwrap();
    assert_eq!(res_chat2.status(), 200);

    let stream_body2 = res_chat2.text().await.unwrap();
    let mut events2 = Vec::new();
    for line in stream_body2.lines() {
        if let Some(data_str) = line.strip_prefix("data: ") {
            let data_str = data_str.trim();
            if !data_str.is_empty() && data_str != "[DONE]" {
                let ev: AgentEvent = serde_json::from_str(data_str)
                    .unwrap_or_else(|e| panic!("failed to parse AgentEvent from '{data_str}': {e}"));
                events2.push(ev);
            }
        }
    }

    assert!(events2.contains(&AgentEvent::Start));
    assert!(events2.contains(&AgentEvent::tool_call_start("call_write_1", "write")));
    assert!(events2.contains(&AgentEvent::tool_call_end(
        "call_write_1",
        serde_json::json!({"path": test_file_str, "content": "hello file"})
    )));
    assert!(events2.contains(&AgentEvent::text_delta("File created successfully.")));

    // Verify session entries after turn 2
    let res_detail2 = client
        .get(format!("{}/api/sessions/{}", base_url, session_id_str))
        .send()
        .await
        .unwrap();
    assert_eq!(res_detail2.status(), 200);
    let entries2: Vec<SessionEntry> = res_detail2.json().await.unwrap();
    assert!(entries2.len() >= 4);

    // 8. Assert DELETE /api/sessions/:id
    let res_del = client
        .delete(format!("{}/api/sessions/{}", base_url, session_id_str))
        .send()
        .await
        .unwrap();
    assert_eq!(res_del.status(), 200);

    // After delete, session load returns 404
    let res_after_del = client
        .get(format!("{}/api/sessions/{}", base_url, session_id_str))
        .send()
        .await
        .unwrap();
    assert_eq!(res_after_del.status(), 404);

    // And list is empty
    let res_sessions_after = client.get(format!("{}/api/sessions", base_url)).send().await.unwrap();
    let sessions_after: Vec<SessionSummary> = res_sessions_after.json().await.unwrap();
    assert!(sessions_after.is_empty());
}
