use super::*;
use crate::event::{AgentEvent, StopReason, Usage};
use crate::parallel::ENV_LOCK;
use std::collections::VecDeque;
use std::sync::Mutex;

/// Provider whose responses are scripted up front, one event list per
/// expected request.
struct FakeProvider {
    scripted: Mutex<VecDeque<Vec<StreamEvent>>>,
    failures: Mutex<VecDeque<ProviderError>>,
    seen_systems: std::sync::Arc<Mutex<Vec<Option<String>>>>,
}

impl FakeProvider {
    fn new(scripts: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            scripted: Mutex::new(VecDeque::from(scripts)),
            failures: Mutex::new(VecDeque::new()),
            seen_systems: std::sync::Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn with_failures(mut self, errs: Vec<ProviderError>) -> Self {
        self.failures = Mutex::new(VecDeque::from(errs));
        self
    }

    /// System prompts of every request served so far, in order.
    fn seen_systems(&self) -> std::sync::Arc<Mutex<Vec<Option<String>>>> {
        self.seen_systems.clone()
    }
}

#[async_trait]
impl Provider for FakeProvider {
    fn stream(&self, req: ChatRequest) -> ProviderStream {
        self.seen_systems
            .lock()
            .expect("seen lock poisoned")
            .push(req.system.clone());
        if let Some(err) = self
            .failures
            .lock()
            .expect("failures lock poisoned")
            .pop_front()
        {
            return Box::pin(futures::stream::iter(vec![Err(err)]));
        }
        let script = self
            .scripted
            .lock()
            .expect("scripted lock poisoned")
            .pop_front()
            .unwrap_or_default();
        Box::pin(futures::stream::iter(script.into_iter().map(Ok)))
    }
}

/// Executor that records every call and answers from a canned table,
/// falling back to a default output for unknown tools.
struct FakeExecutor {
    calls: std::sync::Arc<Mutex<Vec<String>>>,
    call_args: std::sync::Arc<Mutex<Vec<(String, serde_json::Value)>>>,
    by_name: Vec<(String, ToolOutput)>,
    default_output: ToolOutput,
    on_execute: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    delay: Option<std::time::Duration>,
}

impl FakeExecutor {
    fn new(default_output: ToolOutput) -> Self {
        Self {
            calls: std::sync::Arc::new(Mutex::new(Vec::new())),
            call_args: std::sync::Arc::new(Mutex::new(Vec::new())),
            by_name: Vec::new(),
            default_output,
            on_execute: None,
            delay: None,
        }
    }

    fn with_output(mut self, name: &str, output: ToolOutput) -> Self {
        self.by_name.push((name.to_string(), output));
        self
    }
}

#[async_trait]
impl ToolExecutor for FakeExecutor {
    fn execute(
        &self,
        _ctx: &ToolContext,
        name: &str,
        args: serde_json::Value,
    ) -> BoxFuture<'static, ToolOutput> {
        self.calls
            .lock()
            .expect("calls lock poisoned")
            .push(name.to_string());
        self.call_args
            .lock()
            .expect("args lock poisoned")
            .push((name.to_string(), args.clone()));
        let output = self
            .by_name
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, o)| o.clone())
            .unwrap_or_else(|| self.default_output.clone());
        let hook = self.on_execute.clone();
        let delay = self.delay;
        Box::pin(async move {
            if let Some(hook) = &hook {
                hook();
            }
            if let Some(d) = delay {
                tokio::time::sleep(d).await;
            }
            output
        })
    }
}

const TOOL_NAME: &str = "lookup";

/// Concurrency probe: records peak in-flight executions (Task 4: proves
/// the parallel lane actually overlaps batchable calls).
struct PeakExecutor {
    cur: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    peak: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl ToolExecutor for PeakExecutor {
    fn execute(
        &self,
        _ctx: &ToolContext,
        name: &str,
        _args: serde_json::Value,
    ) -> BoxFuture<'static, ToolOutput> {
        let cur = self.cur.clone();
        let peak = self.peak.clone();
        let name = name.to_string();
        Box::pin(async move {
            let n = cur.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            peak.fetch_max(n, std::sync::atomic::Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(60)).await;
            cur.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            ToolOutput::ok(format!("{name}-done"))
        })
    }
}

fn read_tool() -> ToolDef {
    ToolDef::new("read", "r", serde_json::json!({"type": "object"}))
}

/// One turn issuing two `read` calls (stream indices 0 and 1).
fn two_read_script() -> Vec<StreamEvent> {
    vec![
        StreamEvent::tool_call_delta(0, Some("c1".into()), Some("read".into()), r#"{"path":"a"}"#),
        StreamEvent::tool_call_delta(1, Some("c2".into()), Some("read".into()), r#"{"path":"b"}"#),
        StreamEvent::message_complete(Some(StopReason::ToolUse), None),
    ]
}

fn tool_def() -> ToolDef {
    ToolDef::new(TOOL_NAME, "A fake lookup tool", serde_json::json!({}))
}

fn tool_script(id: &str) -> Vec<StreamEvent> {
    vec![
        StreamEvent::text_delta("checking..."),
        StreamEvent::tool_call_delta(
            0,
            Some(id.to_string()),
            Some(TOOL_NAME.to_string()),
            r#"{"q":"#,
        ),
        StreamEvent::tool_call_delta(0, None, None, r#""x"}"#),
        StreamEvent::message_complete(Some(StopReason::ToolUse), Some(Usage::new(10, 5))),
    ]
}

fn read_script(id: &str, path: &str) -> Vec<StreamEvent> {
    vec![
        StreamEvent::text_delta("peeking..."),
        StreamEvent::tool_call_delta(
            0,
            Some(id.to_string()),
            Some("read".to_string()),
            format!(r#"{{"path":"{path}"}}"#),
        ),
        StreamEvent::message_complete(Some(StopReason::ToolUse), None),
    ]
}

fn write_script(id: &str) -> Vec<StreamEvent> {
    vec![
        StreamEvent::text_delta("writing..."),
        StreamEvent::tool_call_delta(
            0,
            Some(id.to_string()),
            Some("write".to_string()),
            r#"{"path":"/tmp/x","content":"y"}"#,
        ),
        StreamEvent::message_complete(Some(StopReason::ToolUse), None),
    ]
}

fn end_script() -> Vec<StreamEvent> {
    vec![
        StreamEvent::text_delta("done"),
        StreamEvent::message_complete(Some(StopReason::EndTurn), None),
    ]
}

#[tokio::test]
async fn reasoning_deltas_stream_and_persist_into_history() {
    let provider = FakeProvider::new(vec![vec![
        StreamEvent::thinking_delta("hmm "),
        StreamEvent::thinking_delta("let me think"),
        StreamEvent::text_delta("answer"),
        StreamEvent::message_complete(Some(StopReason::EndTurn), None),
    ]]);
    let executor = FakeExecutor::new(ToolOutput::ok("unused"));
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor));

    let events = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .unwrap();

    // Thinking deltas surface in the event stream, before the text.
    assert!(events.contains(&AgentEvent::thinking_delta("hmm ")));
    assert!(events.contains(&AgentEvent::thinking_delta("let me think")));
    let text_pos = events
        .iter()
        .position(|e| *e == AgentEvent::text_delta("answer"))
        .unwrap();
    let think_pos = events
        .iter()
        .position(|e| *e == AgentEvent::thinking_delta("hmm "))
        .unwrap();
    assert!(think_pos < text_pos, "reasoning should precede prose");

    // ...and land in the transcript as a thinking block ahead of the text.
    let assistant = &agent.messages()[1];
    assert_eq!(
        assistant.content,
        vec![
            ContentBlock::Thinking {
                text: "hmm let me think".to_string(),
                encrypted_content: None,
                item_id: None,
                model: None
            },
            ContentBlock::Text {
                text: "answer".to_string()
            },
        ]
    );
}

#[tokio::test]
async fn happy_path_single_tool_round_trip() {
    let provider = FakeProvider::new(vec![
        tool_script("call_1"),
        vec![
            StreamEvent::text_delta("all done"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), Some(Usage::new(20, 10))),
        ],
    ]);
    let executor = FakeExecutor::new(ToolOutput::ok("result payload"));
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor))
        .with_system("be terse")
        .with_tools(vec![tool_def()]);

    let events = agent
        .run(Message::user("find it"), ToolContext::default())
        .await
        .expect("run should succeed");

    assert_eq!(
        events,
        vec![
            AgentEvent::Start,
            AgentEvent::text_delta("checking..."),
            AgentEvent::tool_call_start("call_1", TOOL_NAME),
            AgentEvent::tool_call_progress("call_1", TOOL_NAME, r#"{"q":"x"}"#),
            AgentEvent::StepUsage {
                usage: Usage::new(10, 5)
            },
            AgentEvent::tool_call_end("call_1", serde_json::json!({"q": "x"})),
            AgentEvent::tool_result("call_1", "result payload", false),
            AgentEvent::text_delta("all done"),
            AgentEvent::StepUsage {
                usage: Usage::new(20, 10)
            },
            // turn_end carries billed sums across rounds (10+20 in,
            // 5+10 out), not the latest report (the StepUsage gauge
            // above is latest-only: round 2's input already contains
            // round 1's history).
            AgentEvent::turn_end(
                StopReason::EndTurn,
                Usage {
                    input_tokens: 30,
                    output_tokens: 15,
                    non_cached_input_tokens: 30,
                    total_tokens: 45,
                    ..Usage::default()
                },
            ),
        ]
    );

    let msgs = agent.messages();
    assert_eq!(msgs.len(), 4);
    assert_eq!(msgs[0], Message::user("find it"));
    assert_eq!(
        msgs[1].content,
        vec![
            ContentBlock::text("checking..."),
            ContentBlock::tool_use("call_1", TOOL_NAME, serde_json::json!({"q": "x"})),
        ]
    );
    assert_eq!(
        msgs[2].content,
        vec![ContentBlock::tool_result("call_1", "result payload", false)]
    );
    assert_eq!(msgs[3], Message::assistant("all done"));
}

#[tokio::test]
async fn tool_error_is_fed_back_and_model_recovers() {
    let provider = FakeProvider::new(vec![
        tool_script("call_err"),
        vec![
            StreamEvent::text_delta("recovered"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), None),
        ],
    ]);
    let executor = FakeExecutor::new(ToolOutput::ok("unused"))
        .with_output(TOOL_NAME, ToolOutput::error("disk on fire"));
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor)).with_tools(vec![tool_def()]);

    let events = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect("run should succeed despite tool error");

    // Error surfaced as data in the event stream...
    assert_eq!(
        events,
        vec![
            AgentEvent::Start,
            AgentEvent::text_delta("checking..."),
            AgentEvent::tool_call_start("call_err", TOOL_NAME),
            AgentEvent::tool_call_progress("call_err", TOOL_NAME, r#"{"q":"x"}"#),
            AgentEvent::StepUsage {
                usage: Usage::new(10, 5)
            },
            AgentEvent::tool_call_end("call_err", serde_json::json!({"q": "x"})),
            AgentEvent::tool_result("call_err", "disk on fire", true),
            AgentEvent::text_delta("recovered"),
            AgentEvent::StepUsage {
                usage: Usage::new(10, 5)
            },
            // Billed across both rounds; the second reports no usage,
            // so only round one counts.
            AgentEvent::turn_end(StopReason::EndTurn, Usage::new(10, 5)),
        ]
    );
    // ...and in the transcript handed back to the model.
    let feedback = &agent.messages()[2];
    assert_eq!(
        feedback.content,
        vec![ContentBlock::tool_result("call_err", "disk on fire", true)]
    );
    assert_eq!(agent.messages()[3], Message::assistant("recovered"));
}

#[tokio::test]
async fn loop_guard_stops_identical_consecutive_tool_calls() {
    // Same tool+args 3× in a row → LoopDetected (replaces arbitrary max_turns).
    let provider = FakeProvider::new(vec![
        tool_script("c1"),
        tool_script("c2"),
        tool_script("c3"),
    ]);
    let executor = FakeExecutor::new(ToolOutput::ok("ok"));
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor)).with_tools(vec![tool_def()]);

    let err = agent
        .run(Message::user("loop forever"), ToolContext::default())
        .await
        .expect_err("should detect loop");

    assert!(matches!(err, CoreError::LoopDetected(_)), "got {err:?}");
    // 3rd identical turn aborting before tool result + synthetic tool_result: 1 user + 2 full rounds + 3rd assistant + 1 synthetic = 7
    assert_eq!(agent.messages().len(), 1 + 2 * 2 + 2);
}

#[tokio::test]
async fn exploration_stall_aborts_varied_read_rounds() {
    // 25 rounds of `read`, each with a DIFFERENT path: the consecutive-identical
    // signature guard never trips, so the exploration-stall guard must.
    let scripts: Vec<Vec<StreamEvent>> = (0..25)
        .map(|i| read_script(&format!("r{i}"), &format!("/tmp/f{i}.rs")))
        .collect();
    let provider = FakeProvider::new(scripts);
    let executor = FakeExecutor::new(ToolOutput::ok("file body"));
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor));

    let err = agent
        .run(Message::user("explore"), ToolContext::default())
        .await
        .expect_err("exploration-only loop should abort");

    assert!(matches!(err, CoreError::LoopDetected(_)), "got {err:?}");
}

#[tokio::test]
async fn exploration_stall_injects_nudge_then_recovers() {
    // Nudge after 12 exploration-only rounds must land in history without
    // killing the run — the model can still end the turn cleanly.
    let mut scripts: Vec<Vec<StreamEvent>> = (0..12)
        .map(|i| read_script(&format!("r{i}"), &format!("/tmp/f{i}.rs")))
        .collect();
    scripts.push(end_script());
    let provider = FakeProvider::new(scripts);
    let executor = FakeExecutor::new(ToolOutput::ok("file body"));
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor));

    let events = agent
        .run(Message::user("explore"), ToolContext::default())
        .await
        .expect("nudge should not kill the run");

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
    );
    assert!(
        agent
            .messages()
            .iter()
            .any(|m| m.text_content().contains("stall guard")),
        "expected a stall-guard nudge message in history"
    );
}

#[tokio::test]
async fn mutating_tool_resets_exploration_streak() {
    // 11 reads → 1 write → 11 reads = 23 rounds: without the reset the
    // streak would pass 20 and abort; with it the run completes.
    let mut scripts: Vec<Vec<StreamEvent>> = Vec::new();
    for i in 0..11 {
        scripts.push(read_script(&format!("a{i}"), &format!("/tmp/a{i}.rs")));
    }
    scripts.push(write_script("w1"));
    for i in 0..11 {
        scripts.push(read_script(&format!("b{i}"), &format!("/tmp/b{i}.rs")));
    }
    scripts.push(end_script());
    let provider = FakeProvider::new(scripts);
    let executor = FakeExecutor::new(ToolOutput::ok("ok"));
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor));

    let events = agent
        .run(Message::user("work"), ToolContext::default())
        .await
        .expect("a write must reset the stall streak");

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
    );
}

#[tokio::test]
async fn exploration_stall_aborts_six_rounds_after_nudge() {
    // Nudge at 12 + 6 post-nudge rounds = abort at 18.
    let scripts: Vec<Vec<StreamEvent>> = (0..18)
        .map(|i| read_script(&format!("r{i}"), &format!("/tmp/f{i}.rs")))
        .collect();
    let provider = FakeProvider::new(scripts);
    let executor = FakeExecutor::new(ToolOutput::ok("file body"));
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor));

    let err = agent
        .run(Message::user("explore"), ToolContext::default())
        .await
        .expect_err("18 exploration-only rounds should abort");

    match err {
        CoreError::LoopDetected(msg) => assert!(
            msg.starts_with("Stopped: 18 consecutive exploration rounds"),
            "got {msg:?}"
        ),
        other => panic!("got {other:?}"),
    }
}

#[tokio::test]
async fn exploration_stall_post_nudge_reset_continues() {
    // 12 reads (nudge) → 1 write (reset) → 12 reads (nudge again) → end:
    // post-nudge counter resets so the turn completes.
    let mut scripts: Vec<Vec<StreamEvent>> = (0..12)
        .map(|i| read_script(&format!("a{i}"), &format!("/tmp/a{i}.rs")))
        .collect();
    scripts.push(write_script("w1"));
    for i in 0..12 {
        scripts.push(read_script(&format!("b{i}"), &format!("/tmp/b{i}.rs")));
    }
    scripts.push(end_script());
    let provider = FakeProvider::new(scripts);
    let executor = FakeExecutor::new(ToolOutput::ok("ok"));
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor));

    let events = agent
        .run(Message::user("work"), ToolContext::default())
        .await
        .expect("a write after the nudge must reset the post-nudge counter");

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
    );
}

#[tokio::test]
async fn stream_error_notices_forward_live_without_ending_turn() {
    // Codex steal: provider `Reconnecting...` notices ride as Ok events
    // so the turn continues; agent must forward them verbatim.
    let provider = FakeProvider::new(vec![vec![
        StreamEvent::stream_error("Reconnecting... 1/3", "status 503: boom"),
        StreamEvent::text_delta("still here"),
        StreamEvent::message_complete(Some(StopReason::EndTurn), None),
    ]]);
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok(""))),
    );

    let events = agent
        .run(Message::user("hi"), ToolContext::default())
        .await
        .expect("retry notice must not fail the run");

    assert!(
        events.contains(&AgentEvent::stream_error(
            "Reconnecting... 1/3",
            "status 503: boom"
        )),
        "expected forwarded StreamError, got {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
    );
}

#[tokio::test]
async fn text_deltas_forwarded_in_stream_order() {
    let provider = FakeProvider::new(vec![vec![
        StreamEvent::text_delta("alpha "),
        StreamEvent::text_delta("beta "),
        StreamEvent::text_delta("gamma"),
        StreamEvent::message_complete(Some(StopReason::EndTurn), None),
    ]]);
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok(""))),
    );

    let events = agent
        .run(Message::user("hi"), ToolContext::default())
        .await
        .expect("run should succeed");

    let deltas: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta { delta } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas, vec!["alpha ", "beta ", "gamma"]);
    // Reassembled text lands in the assistant message verbatim.
    assert_eq!(agent.messages()[1], Message::assistant("alpha beta gamma"));
}

#[tokio::test]
async fn cancel_between_turns_aborts_gracefully() {
    let provider = FakeProvider::new(vec![tool_script("c1"), tool_script("never_reached")]);
    // The tool cancels the shared token mid-execution: the current turn
    // finishes cleanly, the *next* round observes cancellation.
    let cancel_token = CancellationToken::new();
    let mut executor = FakeExecutor::new(ToolOutput::ok("ok"));
    let token = cancel_token.clone();
    executor.on_execute = Some(std::sync::Arc::new(move || token.cancel()));
    let call_log = executor.calls.clone();
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor)).with_tools(vec![tool_def()]);

    let err = agent
        .run(
            Message::user("go"),
            ToolContext {
                cwd: ".".into(),
                cancel: cancel_token,
                session_id: None,
            },
        )
        .await
        .expect_err("cancelled run should surface Cancelled");

    assert!(matches!(err, CoreError::Cancelled), "got {err:?}");
    assert_eq!(
        call_log.lock().expect("calls lock poisoned").clone(),
        vec![TOOL_NAME.to_string()]
    );
}

/// Blank input is admitted nowhere: no provider request, no history
/// write, no events. Replaces the old queue's EmptyInput rejection.
#[tokio::test]
async fn blank_input_never_reaches_provider_or_history() {
    let provider = FakeProvider::new(vec![]);
    let seen = provider.seen_systems();
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok(""))),
    );
    agent.messages.push(Message::user("prior"));

    let events = agent
        .run(Message::user("   "), ToolContext::default())
        .await
        .expect("blank input is a no-op, not an error");

    assert!(events.is_empty(), "no events for blank input");
    assert_eq!(
        agent.messages().len(),
        1,
        "blank input must not be recorded"
    );
    assert!(
        seen.lock().expect("seen lock poisoned").is_empty(),
        "provider must not be called"
    );
}

#[tokio::test]
async fn with_messages_preserves_prior_conversation_history() {
    let provider = FakeProvider::new(vec![vec![
        StreamEvent::text_delta("hello again"),
        StreamEvent::message_complete(Some(StopReason::EndTurn), None),
    ]]);
    let prior = vec![
        Message::user("first question"),
        Message::assistant("first answer"),
    ];
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("ok"))),
    )
    .with_messages(prior);

    let events = agent
        .run(Message::user("second question"), ToolContext::default())
        .await
        .expect("run should succeed");

    assert!(events.contains(&AgentEvent::text_delta("hello again")));
    assert_eq!(agent.messages().len(), 4);
    assert_eq!(agent.messages()[0], Message::user("first question"));
    assert_eq!(agent.messages()[1], Message::assistant("first answer"));
    assert_eq!(agent.messages()[2], Message::user("second question"));
    assert_eq!(agent.messages()[3], Message::assistant("hello again"));
}

#[tokio::test]
async fn malformed_tool_args_degrade_to_string_payload() {
    let provider = FakeProvider::new(vec![
        vec![
            StreamEvent::tool_call_delta(
                0,
                Some("c1".into()),
                Some(TOOL_NAME.into()),
                "not-json{{",
            ),
            StreamEvent::message_complete(Some(StopReason::ToolUse), None),
        ],
        vec![
            StreamEvent::text_delta("ok"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), None),
        ],
    ]);
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("ok"))),
    )
    .with_tools(vec![tool_def()]);
    let events = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .unwrap();
    assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolCallEnd { args: serde_json::Value::String(s), .. } if s == "not-json{{")));
}

#[tokio::test]
async fn unknown_tool_name_skips_executor_with_error_result() {
    let provider = FakeProvider::new(vec![
        vec![
            StreamEvent::tool_call_delta(
                0,
                Some("c-unknown".into()),
                Some("nope".into()),
                r#"{"q":"x"}"#,
            ),
            StreamEvent::message_complete(Some(StopReason::ToolUse), None),
        ],
        end_script(),
    ]);
    let executor = FakeExecutor::new(ToolOutput::ok("should-not-reach"));
    let call_log = executor.calls.clone();
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor)).with_tools(vec![tool_def()]);
    let events = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .unwrap();
    assert!(
        call_log.lock().expect("calls lock poisoned").is_empty(),
        "executor must not run for unknown tool"
    );
    let (output, is_error) = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolResult {
                id,
                output,
                is_error,
            } if id == "c-unknown" => Some((output.clone(), *is_error)),
            _ => None,
        })
        .expect("expected tool result for unknown tool");
    assert!(is_error, "unknown tool must be is_error, got {output}");
    assert!(
        output.contains("does not exist") && output.contains("nope") && output.contains(TOOL_NAME),
        "got {output}"
    );
    assert_eq!(agent.messages()[1].role, Role::Assistant);
    assert!(matches!(
        &agent.messages()[2].content[0],
        ContentBlock::ToolResult { is_error: true, .. }
    ));
}

#[tokio::test]
async fn null_args_skips_executor_with_error_result() {
    let provider = FakeProvider::new(vec![
        vec![
            StreamEvent::tool_call_delta(0, Some("c-null".into()), Some(TOOL_NAME.into()), ""),
            StreamEvent::message_complete(Some(StopReason::ToolUse), None),
        ],
        end_script(),
    ]);
    let executor = FakeExecutor::new(ToolOutput::ok("should-not-reach"));
    let call_log = executor.calls.clone();
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor)).with_tools(vec![tool_def()]);
    let events = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .unwrap();
    assert!(
        call_log.lock().expect("calls lock poisoned").is_empty(),
        "executor must not run for null args"
    );
    let (output, is_error) = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolResult {
                id,
                output,
                is_error,
            } if id == "c-null" => Some((output.clone(), *is_error)),
            _ => None,
        })
        .expect("expected tool result for null args");
    assert!(is_error, "null args must be is_error, got {output}");
    assert!(output.contains("valid JSON object"), "got {output}");
}

#[tokio::test]
async fn malformed_string_args_skips_executor_with_error_result() {
    let provider = FakeProvider::new(vec![
        vec![
            StreamEvent::tool_call_delta(
                0,
                Some("c-bad".into()),
                Some(TOOL_NAME.into()),
                "not-json{{",
            ),
            StreamEvent::message_complete(Some(StopReason::ToolUse), None),
        ],
        end_script(),
    ]);
    let executor = FakeExecutor::new(ToolOutput::ok("should-not-reach"));
    let call_log = executor.calls.clone();
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor)).with_tools(vec![tool_def()]);
    let events = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .unwrap();
    assert!(
        call_log.lock().expect("calls lock poisoned").is_empty(),
        "executor must not run for malformed args"
    );
    let (output, is_error) = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolResult {
                id,
                output,
                is_error,
            } if id == "c-bad" => Some((output.clone(), *is_error)),
            _ => None,
        })
        .expect("expected tool result for malformed args");
    assert!(is_error, "malformed args must be is_error, got {output}");
    assert!(output.contains("valid JSON object"), "got {output}");
}

// --- workstream A-core ---

fn empty_script() -> Vec<StreamEvent> {
    vec![StreamEvent::message_complete(
        Some(StopReason::EndTurn),
        None,
    )]
}

fn maxtokens_script(text: &str) -> Vec<StreamEvent> {
    vec![
        StreamEvent::text_delta(text.to_string()),
        StreamEvent::message_complete(Some(StopReason::MaxTokens), None),
    ]
}

#[test]
fn should_compress_true_only_for_context_overflow() {
    assert!(ProviderError::ContextOverflow("ctx".into()).should_compress());
    assert!(!ProviderError::RateLimited("x".into()).should_compress());
    assert!(!ProviderError::BadRequest("x".into()).should_compress());
    assert!(!ProviderError::ServerError("x".into()).should_compress());
}

#[tokio::test]
async fn context_overflow_compacts_once_then_continues() {
    let provider = FakeProvider::new(vec![
        vec![
            StreamEvent::text_delta("summarized"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), None),
        ],
        vec![
            StreamEvent::text_delta("continued"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), None),
        ],
    ])
    .with_failures(vec![ProviderError::ContextOverflow(
        "context exhausted — start /new or compact".into(),
    )]);
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    )
    // v2 compaction retains the newest history within min(64k,
    // window−reserve) and appends the summary LAST: seed more than the
    // 64k retained budget (unknown window) so the oldest messages drop
    // and the replacement strictly shrinks (a fully-retained history
    // correctly reports "nothing to gain" and surfaces the error).
    .with_messages(
        (0..80)
            .map(|i| Message::user(format!("bulk{i}:{}", "x".repeat(3990))))
            .collect(),
    );

    let events = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect("compact+retry should succeed");

    assert!(
        events
            .iter()
            .any(|e| *e == AgentEvent::text_delta("continued"))
    );
    let msgs = agent.messages();
    let summary_at = msgs
        .iter()
        .position(|m| m.text_content().contains("Another language model started"))
        .expect("history must contain the summary pair");
    assert!(
        summary_at > 0 && summary_at + 2 < msgs.len(),
        "v2 order: [retained..., summary_user, summary_ack, ...], got summary at {summary_at} of {}",
        msgs.len()
    );
    assert!(
        msgs[summary_at + 1].text_content().contains("Understood"),
        "summary_ack follows summary_user"
    );
    assert!(
        msgs[..summary_at].iter().all(|m| {
            let t = m.text_content();
            t.starts_with("bulk") || t == "go"
        }),
        "everything before the summary is retained history (bulks + the turn's go)"
    );
}

/// Elision is a pressure valve, not a per-turn habit: with no window
/// pressure the model keeps every tool result it observed (it must not go
/// blind on earlier searches/reads mid-task).
#[tokio::test]
async fn tool_observations_survive_without_context_pressure() {
    let body = format!("payload-keepme {}", "x".repeat(200));
    let mut scripts: Vec<Vec<StreamEvent>> = (0..7)
        .map(|i| tool_script_with_args(&format!("c{i}"), &format!(r#"{{"q":"x{i}"}}"#)))
        .collect();
    scripts.push(end_script());
    let provider = FakeProvider::new(scripts);
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok(body))),
    )
    .with_tools(vec![tool_def()]);

    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect("7 tool rounds should finish");

    let observed: Vec<&str> = agent
        .messages()
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(observed.len(), 7, "one result per round");
    assert!(
        observed.iter().all(|c| c.starts_with("payload-keepme")),
        "no observation may be elided without window pressure: {observed:?}"
    );
}

#[tokio::test]
async fn context_overflow_surfaces_actionable_error_when_compact_fails() {
    let provider = FakeProvider::new(vec![]).with_failures(vec![
        ProviderError::ContextOverflow("boom".into()),
        ProviderError::ContextOverflow("boom".into()),
    ]);
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    );

    let err = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect_err("must surface when compact also overflows");

    assert!(
        err.to_string().contains("context exhausted"),
        "actionable message, got {err}"
    );
}

#[tokio::test]
async fn empty_turn_retries_twice_then_ends_with_sentinel() {
    let provider = FakeProvider::new(vec![
        empty_script(),
        empty_script(),
        empty_script(),
        empty_script(),
    ]);
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    );

    let events = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect("empty turn should end gracefully");

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
    );
    assert_eq!(agent.messages().last().unwrap().text_content(), "(empty)");
}

#[tokio::test]
async fn empty_after_tool_results_nudges_once_then_continues() {
    let provider = FakeProvider::new(vec![
        tool_script("c1"),
        empty_script(),
        empty_script(),
        empty_script(),
        end_script(),
    ]);
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("tool says hi"))),
    )
    .with_tools(vec![tool_def()]);

    let events = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect("nudge should recover the turn");

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
    );
    assert!(
        agent
            .messages()
            .iter()
            .any(|m| m.text_content().contains("process results")),
        "expected the empty-after-tools nudge in history"
    );
}

fn tool_script_with_args(id: &str, args: &str) -> Vec<StreamEvent> {
    vec![
        StreamEvent::text_delta("checking..."),
        StreamEvent::tool_call_delta(0, Some(id.to_string()), Some(TOOL_NAME.to_string()), args),
        StreamEvent::message_complete(Some(StopReason::ToolUse), Some(Usage::new(10, 5))),
    ]
}

#[tokio::test]
async fn alternating_tool_empty_stays_bounded_and_nudges_once() {
    // tool -> empty x3 (nudge) -> tool(varied) -> empty x3 -> end.
    // Without a once-per-run nudge gate the second empty burst would nudge
    // again and the alternation could continue unbounded.
    let provider = FakeProvider::new(vec![
        tool_script_with_args("c1", r#"{"q":"a"}"#),
        empty_script(),
        empty_script(),
        empty_script(),
        tool_script_with_args("c2", r#"{"q":"b"}"#),
        empty_script(),
        empty_script(),
        empty_script(),
        end_script(),
    ]);
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("ok"))),
    )
    .with_tools(vec![tool_def()]);

    let events = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect("alternation must terminate");

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
    );
    let nudges = agent
        .messages()
        .iter()
        .filter(|m| m.text_content().contains("process results"))
        .count();
    assert_eq!(nudges, 1, "post-tool empty nudge must fire once per run");
}

#[tokio::test]
async fn maxtokens_truncation_continues_and_stitches_partial() {
    let provider = FakeProvider::new(vec![
        maxtokens_script("part1 "),
        vec![
            StreamEvent::text_delta("part2"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), None),
        ],
    ]);
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    );

    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect("continuation should succeed");

    let joined = agent
        .messages()
        .iter()
        .map(|m| m.text_content())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("part1 ") && joined.contains("part2"),
        "stitched: {joined}"
    );
    assert!(
        joined.contains("continue exactly where you left off"),
        "continuation prompt: {joined}"
    );
}

#[tokio::test]
async fn maxtokens_continuations_are_capped_then_partial_kept() {
    let provider = FakeProvider::new(vec![
        maxtokens_script("a"),
        maxtokens_script("b"),
        maxtokens_script("c"),
        maxtokens_script("d"),
    ]);
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    );

    let events = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect("capped continuation keeps partial");

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
    );
    let nudges = agent
        .messages()
        .iter()
        .filter(|m| {
            m.text_content()
                .contains("continue exactly where you left off")
        })
        .count();
    assert_eq!(nudges, 2, "continuations must be capped");
}

#[test]
fn tool_timeout_builder_keeps_120s_default() {
    let agent = Agent::new(
        Box::new(FakeProvider::new(vec![])),
        Arc::new(FakeExecutor::new(ToolOutput::ok(""))),
    );
    assert_eq!(agent.tool_timeout, std::time::Duration::from_secs(120));
    let agent = agent.with_tool_timeout(std::time::Duration::from_millis(50));
    assert_eq!(agent.tool_timeout, std::time::Duration::from_millis(50));
}

#[tokio::test]
async fn tool_timeout_becomes_error_result_and_continues() {
    let provider = FakeProvider::new(vec![tool_script("c1"), end_script()]);
    let mut executor = FakeExecutor::new(ToolOutput::ok("too slow"));
    executor.delay = Some(std::time::Duration::from_secs(5));
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor))
        .with_tools(vec![tool_def()])
        .with_tool_timeout(std::time::Duration::from_millis(50));

    let events = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect("timeout must not fail the run");

    let (output, is_error) = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolResult {
                id,
                output,
                is_error,
            } if id == "c1" => Some((output.clone(), *is_error)),
            _ => None,
        })
        .expect("expected tool result after timeout");
    assert!(is_error, "timeout must be is_error, got {output}");
    assert!(output.contains("timed out"), "got {output}");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
    );
}

#[test]
fn summary_pair_envelope_is_byte_stable() {
    let [u, a] = super::summary_pair("  hello world  ");
    assert_eq!(
        u.text_content(),
        "Another language model started to solve this problem and produced a summary of its thinking process. Use this to build on the work already done and avoid duplicating work. Here is the summary, use the information in it to assist with your own analysis:\n\n<s>\nhello world\n</s>"
    );
    assert_eq!(
        a.text_content(),
        "Understood. I have reviewed the conversation summary and context, and I am ready to continue."
    );
    // Byte-equality: trimming + envelope must never drift.
    let [u2, a2] = super::summary_pair("hello world");
    assert_eq!(u.text_content().as_bytes(), u2.text_content().as_bytes());
    assert_eq!(a.text_content().as_bytes(), a2.text_content().as_bytes());
}

/// Stub `prompt/context` hook: returns fixed text like a sidecar's
/// `{"result":{"text":"…"}}` reply.
struct CtxHook {
    text: Option<String>,
}

#[async_trait]
impl PluginHooks for CtxHook {
    async fn prompt_context(&self) -> Option<String> {
        self.text.clone()
    }
}

#[tokio::test]
async fn prompt_context_replies_concatenate_onto_system() {
    let provider = FakeProvider::new(vec![end_script()]);
    let seen = provider.seen_systems();
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    )
    .with_system("BASE-SYSTEM")
    .with_hooks(vec![
        Arc::new(CtxHook {
            text: Some("PLUGIN-CTX-AAA".to_string()),
        }),
        Arc::new(CtxHook { text: None }),
        Arc::new(CtxHook {
            text: Some("PLUGIN-CTX-BBB".to_string()),
        }),
    ]);

    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .unwrap();

    let seen = seen.lock().expect("seen lock poisoned");
    assert_eq!(seen.len(), 1, "one turn → one request, got {seen:?}");
    let system = seen[0].as_deref().unwrap_or("");
    assert!(
        system.contains("BASE-SYSTEM"),
        "base prompt preserved, got: {system}"
    );
    let (a, b) = (system.find("PLUGIN-CTX-AAA"), system.find("PLUGIN-CTX-BBB"));
    assert!(
        a.is_some() && b.is_some(),
        "both hook replies present, got: {system}"
    );
    assert!(a.unwrap() < b.unwrap(), "hook order kept, got: {system}");
}

/// Counting `prompt/context` hook: proves the turn fetches once.
struct CountingCtxHook {
    text: String,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl PluginHooks for CountingCtxHook {
    async fn prompt_context(&self) -> Option<String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Some(self.text.clone())
    }
}

#[tokio::test]
async fn prompt_context_fetched_once_per_turn() {
    // Prefix-cache invariant: hook context is turn-scoped, so a
    // multi-round turn reuses one fetch across all its requests.
    let provider = FakeProvider::new(vec![tool_script("c1"), end_script()]);
    let seen = provider.seen_systems();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("ok"))),
    )
    .with_system("BASE-SYSTEM")
    .with_tools(vec![tool_def()])
    .with_hooks(vec![Arc::new(CountingCtxHook {
        text: "STABLE-CTX".to_string(),
        calls: Arc::clone(&calls),
    })]);

    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .unwrap();

    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "hook must run once per turn"
    );
    let seen = seen.lock().expect("seen lock poisoned");
    assert_eq!(seen.len(), 2, "two rounds → two requests, got {seen:?}");
    for system in seen.iter() {
        let system = system.as_deref().unwrap_or("");
        assert!(
            system.contains("STABLE-CTX"),
            "every request carries hook context, got: {system}"
        );
    }
}

/// Stub `tool/before` hook: denies or rewrites every call, like a
/// sidecar's `{"decision":"deny"|"modify",…}` reply.
struct VetoHook {
    deny: Option<String>,
    rewrite: Option<serde_json::Value>,
}

#[async_trait]
impl PluginHooks for VetoHook {
    async fn tool_before(&self, _name: &str, _args: &serde_json::Value) -> ToolBefore {
        if let Some(reason) = &self.deny {
            return ToolBefore::Deny(reason.clone());
        }
        if let Some(args) = &self.rewrite {
            return ToolBefore::Modify(args.clone());
        }
        ToolBefore::Allow
    }
}

#[tokio::test]
async fn tool_before_deny_skips_executor_with_error_result() {
    let provider = FakeProvider::new(vec![tool_script("c1"), end_script()]);
    let executor = FakeExecutor::new(ToolOutput::ok("must-not-run"));
    let call_log = executor.calls.clone();
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor))
        .with_tools(vec![tool_def()])
        .with_hooks(vec![Arc::new(VetoHook {
            deny: Some("DENIED-XYZ".to_string()),
            rewrite: None,
        })]);

    let events = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .unwrap();

    assert!(
        call_log.lock().expect("calls lock poisoned").is_empty(),
        "denied tool must never execute"
    );
    let (output, is_error) = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolResult {
                output, is_error, ..
            } => Some((output.clone(), *is_error)),
            _ => None,
        })
        .expect("deny must still emit a tool result");
    assert!(is_error, "deny result is an error, got {output:?}");
    assert!(
        output.contains("DENIED-XYZ"),
        "reason surfaced, got {output:?}"
    );
    // History carries the same error result so alternation stays intact.
    let stored = agent
        .messages()
        .iter()
        .flat_map(|m| m.content.iter())
        .find_map(|b| match b {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => Some((content.clone(), *is_error)),
            _ => None,
        })
        .expect("deny must leave a history tool result");
    assert!(
        stored.1 && stored.0.contains("DENIED-XYZ"),
        "got {stored:?}"
    );
}

#[tokio::test]
async fn tool_before_modify_rewrites_executor_args() {
    let provider = FakeProvider::new(vec![tool_script("c1"), end_script()]);
    let executor = FakeExecutor::new(ToolOutput::ok("ok"));
    let arg_log = executor.call_args.clone();
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor))
        .with_tools(vec![tool_def()])
        .with_hooks(vec![Arc::new(VetoHook {
            deny: None,
            rewrite: Some(serde_json::json!({"q": "rewritten"})),
        })]);

    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .unwrap();

    let logged = arg_log.lock().expect("args lock poisoned");
    assert_eq!(logged.len(), 1, "executor runs once, got {logged:?}");
    assert_eq!(
        logged[0].1,
        serde_json::json!({"q": "rewritten"}),
        "got {logged:?}"
    );
}

#[tokio::test]
async fn tool_before_absent_leaves_args_untouched() {
    let provider = FakeProvider::new(vec![tool_script("c1"), end_script()]);
    let executor = FakeExecutor::new(ToolOutput::ok("ok"));
    let arg_log = executor.call_args.clone();
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor)).with_tools(vec![tool_def()]);

    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .unwrap();

    let logged = arg_log.lock().expect("args lock poisoned");
    assert_eq!(logged.len(), 1, "executor runs once, got {logged:?}");
    assert_eq!(logged[0].0, TOOL_NAME);
    assert_eq!(logged[0].1, serde_json::json!({"q": "x"}), "got {logged:?}");
}

/// Counting hook for lifecycle emission: records pre/post order and
/// counts turn_end calls.
struct LifecycleHook {
    turn_ends: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    calls: std::sync::Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl PluginHooks for LifecycleHook {
    async fn pre_tool(&self, name: &str, _args: &serde_json::Value) {
        self.calls
            .lock()
            .expect("calls lock poisoned")
            .push(format!("pre:{name}"));
    }
    async fn post_tool(&self, name: &str, _output: &ToolOutput) {
        self.calls
            .lock()
            .expect("calls lock poisoned")
            .push(format!("post:{name}"));
    }
    async fn turn_end(&self, _usage: &Usage) {
        self.turn_ends
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[tokio::test]
async fn turn_end_hook_called_once_on_end_and_on_error() {
    // Success path: exactly once.
    let ends = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
    let mut agent = Agent::new(
        Box::new(FakeProvider::new(vec![end_script()])),
        Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    )
    .with_hooks(vec![Arc::new(LifecycleHook {
        turn_ends: ends.clone(),
        calls,
    })]);
    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .unwrap();
    assert_eq!(
        ends.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "turn_end once on success"
    );

    // Error path (identical-tool loop → LoopDetected): still exactly once.
    let ends_err = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls_err = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
    let mut agent = Agent::new(
        Box::new(FakeProvider::new(vec![
            tool_script("c1"),
            tool_script("c2"),
            tool_script("c3"),
        ])),
        Arc::new(FakeExecutor::new(ToolOutput::ok("ok"))),
    )
    .with_tools(vec![tool_def()])
    .with_hooks(vec![Arc::new(LifecycleHook {
        turn_ends: ends_err.clone(),
        calls: calls_err,
    })]);
    let err = agent
        .run(Message::user("loop"), ToolContext::default())
        .await
        .expect_err("loop must fail");
    assert!(matches!(err, CoreError::LoopDetected(_)), "got {err:?}");
    assert_eq!(
        ends_err.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "turn_end once on error"
    );
}

#[tokio::test]
async fn pre_post_hooks_emit_around_tool_execution() {
    let ends = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
    let mut agent = Agent::new(
        Box::new(FakeProvider::new(vec![tool_script("c1"), end_script()])),
        Arc::new(FakeExecutor::new(ToolOutput::ok("ok"))),
    )
    .with_tools(vec![tool_def()])
    .with_hooks(vec![Arc::new(LifecycleHook {
        turn_ends: ends.clone(),
        calls: calls.clone(),
    })]);

    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .unwrap();

    let logged = calls.lock().expect("calls lock poisoned").clone();
    assert_eq!(
        logged,
        vec![format!("pre:{TOOL_NAME}"), format!("post:{TOOL_NAME}")],
        "order pre→post, got {logged:?}"
    );
    // Sidecar-provided tools run through the same executor call site as
    // builtin tools, so this emission covers both by construction.
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // ENV_LOCK is test-only env serialization; holding it across await is its job
async fn parallel_batch_overlaps_reads_with_ordered_results() {
    let _g = ENV_LOCK.lock().unwrap();
    let prev = std::env::var("GRAY_PARALLEL_READS").ok();
    unsafe { std::env::remove_var("GRAY_PARALLEL_READS") };
    let provider = FakeProvider::new(vec![two_read_script(), end_script()]);
    let peak = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let executor = PeakExecutor {
        cur: Default::default(),
        peak: peak.clone(),
    };
    let mut agent =
        Agent::new(Box::new(provider), Arc::new(executor)).with_tools(vec![read_tool()]);
    let events = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .unwrap();
    match prev {
        Some(v) => unsafe { std::env::set_var("GRAY_PARALLEL_READS", v) },
        None => unsafe { std::env::remove_var("GRAY_PARALLEL_READS") },
    }
    assert_eq!(
        peak.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "both reads must overlap"
    );
    let results: Vec<(String, String)> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolResult { id, output, .. } => Some((id.clone(), output.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        results,
        vec![
            ("c1".to_string(), "read-done".to_string()),
            ("c2".to_string(), "read-done".to_string()),
        ],
        "results stay in input order, {events:?}"
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // ENV_LOCK is test-only env serialization; holding it across await is its job
async fn lane_off_matches_sequential_events() {
    let _g = ENV_LOCK.lock().unwrap();
    let prev = std::env::var("GRAY_PARALLEL_READS").ok();
    let mut runs = Vec::new();
    for val in [Some("0"), None] {
        match val {
            Some(v) => unsafe { std::env::set_var("GRAY_PARALLEL_READS", v) },
            None => unsafe { std::env::remove_var("GRAY_PARALLEL_READS") },
        }
        let provider = FakeProvider::new(vec![two_read_script(), end_script()]);
        let executor = FakeExecutor::new(ToolOutput::ok("x"));
        let mut agent =
            Agent::new(Box::new(provider), Arc::new(executor)).with_tools(vec![read_tool()]);
        runs.push(
            agent
                .run(Message::user("go"), ToolContext::default())
                .await
                .unwrap(),
        );
    }
    match prev {
        Some(v) => unsafe { std::env::set_var("GRAY_PARALLEL_READS", v) },
        None => unsafe { std::env::remove_var("GRAY_PARALLEL_READS") },
    }
    assert_eq!(runs.len(), 2);
    assert_eq!(
        runs[0], runs[1],
        "lane off must match lane on event-for-event"
    );
}

/// Cancels the run's token inside the first `tool_before` verdict, so the
/// parallel pre-pass observes cancellation at the SECOND index — past the
/// first ready item.
struct CancelOnFirstBefore {
    token: tokio_util::sync::CancellationToken,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl PluginHooks for CancelOnFirstBefore {
    async fn tool_before(&self, _name: &str, _args: &serde_json::Value) -> ToolBefore {
        if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            self.token.cancel();
        }
        ToolBefore::Allow
    }
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // ENV_LOCK is test-only env serialization; holding it across await is its job
async fn parallel_prepass_cancel_backfills_whole_run_exactly_once() {
    let _g = ENV_LOCK.lock().unwrap();
    let prev = std::env::var("GRAY_PARALLEL_READS").ok();
    unsafe { std::env::remove_var("GRAY_PARALLEL_READS") };
    let cancel = tokio_util::sync::CancellationToken::new();
    let provider = FakeProvider::new(vec![two_read_script(), end_script()]);
    let executor = FakeExecutor::new(ToolOutput::ok("x"));
    let call_log = executor.calls.clone();
    let ctx = ToolContext {
        cancel: cancel.clone(),
        ..ToolContext::default()
    };
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor))
        .with_tools(vec![read_tool()])
        .with_hooks(vec![Arc::new(CancelOnFirstBefore {
            token: cancel,
            calls: std::sync::atomic::AtomicUsize::new(0),
        })]);
    let err = agent
        .run(Message::user("go"), ctx)
        .await
        .expect_err("pre-pass cancel must abort the run");
    match prev {
        Some(v) => unsafe { std::env::set_var("GRAY_PARALLEL_READS", v) },
        None => unsafe { std::env::remove_var("GRAY_PARALLEL_READS") },
    }
    assert!(matches!(err, CoreError::Cancelled), "got {err:?}");
    assert!(
        call_log.lock().expect("calls lock poisoned").is_empty(),
        "cancelled pre-pass must never reach the executor"
    );
    for id in ["c1", "c2"] {
        let n = agent
            .messages()
            .iter()
            .flat_map(|m| m.content.iter())
            .filter(|b| matches!(b, ContentBlock::ToolResult { id: i, .. } if i.as_str() == id))
            .count();
        assert_eq!(n, 1, "tool {id} must have exactly one ToolResult message");
    }
}

#[tokio::test]
async fn name_before_id_emits_no_start_until_id_arrives() {
    // Name-only delta buffers args without any start/progress; the ID
    // arriving later emits the single start with the provider ID.
    let provider = FakeProvider::new(vec![
        vec![
            StreamEvent::tool_call_delta(0, None, Some(TOOL_NAME.into()), r#"{"q":"#),
            StreamEvent::tool_call_delta(0, Some("c-late".into()), None, r#""x"}"#),
            StreamEvent::message_complete(Some(StopReason::ToolUse), None),
        ],
        end_script(),
    ]);
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("ok"))),
    )
    .with_tools(vec![tool_def()]);

    let events = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .unwrap();

    let starts: Vec<(String, String)> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolCallStart { id, name } => Some((id.clone(), name.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        starts,
        vec![("c-late".to_string(), TOOL_NAME.to_string())],
        "exactly one start with the late provider ID, got {starts:?}"
    );
    for e in &events {
        match e {
            AgentEvent::ToolCallProgress { id, .. }
            | AgentEvent::ToolCallEnd { id, .. }
            | AgentEvent::ToolResult { id, .. } => {
                assert_eq!(id, "c-late", "no fallback ID may leak, got {e:?}");
            }
            _ => {}
        }
    }
}

#[tokio::test]
async fn conflicting_id_reset_is_provider_protocol_error() {
    let provider = FakeProvider::new(vec![vec![
        StreamEvent::tool_call_delta(
            0,
            Some("c-a".into()),
            Some(TOOL_NAME.into()),
            r#"{"q":"x"}"#,
        ),
        StreamEvent::tool_call_delta(0, Some("c-b".into()), None, ""),
        StreamEvent::message_complete(Some(StopReason::ToolUse), None),
    ]]);
    let executor = FakeExecutor::new(ToolOutput::ok("must-not-run"));
    let call_log = executor.calls.clone();
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor)).with_tools(vec![tool_def()]);

    let err = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect_err("conflicting ID must fail the turn");
    assert!(
        err.to_string().contains("changed tool-call id"),
        "protocol error, got {err:?}"
    );
    assert!(
        call_log.lock().expect("calls lock poisoned").is_empty(),
        "conflicted call must never execute"
    );
}

#[tokio::test]
async fn no_id_call_gets_single_fallback_used_everywhere() {
    let provider = FakeProvider::new(vec![
        vec![
            StreamEvent::tool_call_delta(0, None, Some(TOOL_NAME.into()), r#"{"q":"x"}"#),
            StreamEvent::message_complete(Some(StopReason::ToolUse), None),
        ],
        end_script(),
    ]);
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("ok"))),
    )
    .with_tools(vec![tool_def()]);

    let events = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .unwrap();

    let mut ids = std::collections::HashSet::new();
    for e in &events {
        match e {
            AgentEvent::ToolCallStart { id, .. }
            | AgentEvent::ToolCallProgress { id, .. }
            | AgentEvent::ToolCallEnd { id, .. }
            | AgentEvent::ToolResult { id, .. } => {
                ids.insert(id.clone());
            }
            _ => {}
        }
    }
    assert_eq!(ids.len(), 1, "one fallback ID everywhere, got {ids:?}");
    let fallback = ids.into_iter().next().unwrap();
    assert!(
        fallback.starts_with("gray_call_"),
        "fallback must be conversation-unique, got {fallback:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(e,
                AgentEvent::ToolCallStart { id, .. } if id == "call_0")),
        "positional call_{{index}} fallback must be gone: {events:?}"
    );
    // History agrees: ToolUse and ToolResult share the fallback.
    let use_id = agent.messages().iter().find_map(|m| {
        m.content.iter().find_map(|b| match b {
            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
    });
    let result_id = agent.messages().iter().find_map(|m| {
        m.content.iter().find_map(|b| match b {
            ContentBlock::ToolResult { id, .. } => Some(id.clone()),
            _ => None,
        })
    });
    assert_eq!(use_id.as_deref(), Some(fallback.as_str()));
    assert_eq!(result_id.as_deref(), Some(fallback.as_str()));
}

#[tokio::test]
async fn eof_with_tool_pending_executes_nothing() {
    // Stream ends without MessageComplete after a tool delta: the
    // pending call must never execute and the turn errors.
    let provider = FakeProvider::new(vec![vec![StreamEvent::tool_call_delta(
        0,
        Some("c-eof".into()),
        Some(TOOL_NAME.into()),
        r#"{"q":"x"}"#,
    )]]);
    let executor = FakeExecutor::new(ToolOutput::ok("must-not-run"));
    let call_log = executor.calls.clone();
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor)).with_tools(vec![tool_def()]);

    let err = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect_err("EOF without completion must fail");
    assert!(
        err.to_string().contains("without completion"),
        "got {err:?}"
    );
    assert!(
        call_log.lock().expect("calls lock poisoned").is_empty(),
        "truncated-stream tool must never execute"
    );
    assert!(
        !agent.messages().iter().any(|m| m.content.iter().any(|b| {
            matches!(
                b,
                ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }
            )
        })),
        "no orphaned call/result may land in history"
    );
}
