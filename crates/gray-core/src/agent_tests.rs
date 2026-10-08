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
    /// Scripts that stream their events and then fail mid-turn.
    partial_failures: Mutex<VecDeque<(Vec<StreamEvent>, ProviderError)>>,
    seen_systems: std::sync::Arc<Mutex<Vec<Option<String>>>>,
    /// (system, messages) of every request served so far, in order.
    seen_requests: std::sync::Arc<Mutex<Vec<(Option<String>, Vec<Message>)>>>,
    /// Tool names advertised in every request served so far, in order.
    seen_tools: std::sync::Arc<Mutex<Vec<Vec<String>>>>,
}

impl FakeProvider {
    fn new(scripts: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            scripted: Mutex::new(VecDeque::from(scripts)),
            failures: Mutex::new(VecDeque::new()),
            partial_failures: Mutex::new(VecDeque::new()),
            seen_systems: std::sync::Arc::new(Mutex::new(Vec::new())),
            seen_requests: std::sync::Arc::new(Mutex::new(Vec::new())),
            seen_tools: std::sync::Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn with_failures(mut self, errs: Vec<ProviderError>) -> Self {
        self.failures = Mutex::new(VecDeque::from(errs));
        self
    }

    fn with_partial_failures(mut self, errs: Vec<(Vec<StreamEvent>, ProviderError)>) -> Self {
        self.partial_failures = Mutex::new(VecDeque::from(errs));
        self
    }

    /// System prompts of every request served so far, in order.
    fn seen_systems(&self) -> std::sync::Arc<Mutex<Vec<Option<String>>>> {
        self.seen_systems.clone()
    }

    /// Full (system, messages) shape of every request served so far.
    fn seen_requests(&self) -> std::sync::Arc<Mutex<Vec<(Option<String>, Vec<Message>)>>> {
        self.seen_requests.clone()
    }

    /// Tool names of every request served so far.
    fn seen_tools(&self) -> std::sync::Arc<Mutex<Vec<Vec<String>>>> {
        self.seen_tools.clone()
    }
}

#[async_trait]
impl Provider for FakeProvider {
    fn stream(&self, req: ChatRequest) -> ProviderStream {
        self.seen_systems
            .lock()
            .expect("seen lock poisoned")
            .push(req.system.clone());
        self.seen_requests
            .lock()
            .expect("seen lock poisoned")
            .push((req.system.clone(), req.messages.clone()));
        self.seen_tools
            .lock()
            .expect("seen lock poisoned")
            .push(req.tools.iter().map(|t| t.name.clone()).collect());
        if let Some(err) = self
            .failures
            .lock()
            .expect("failures lock poisoned")
            .pop_front()
        {
            return Box::pin(futures::stream::iter(vec![Err(err)]));
        }
        if let Some((events, err)) = self
            .partial_failures
            .lock()
            .expect("partial failures lock poisoned")
            .pop_front()
        {
            let stream: Vec<Result<StreamEvent, ProviderError>> = events
                .into_iter()
                .map(Ok)
                .chain(std::iter::once(Err(err)))
                .collect();
            return Box::pin(futures::stream::iter(stream));
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
async fn loop_guard_aborts_at_six_silently() {
    // Same tool+args 6× in a row → abort, with no nudge text injected.
    let scripts: Vec<Vec<StreamEvent>> = (0..6).map(|i| tool_script(&format!("c{i}"))).collect();
    let provider = FakeProvider::new(scripts);
    let executor = FakeExecutor::new(ToolOutput::ok("ok"));
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor)).with_tools(vec![tool_def()]);

    let err = agent
        .run(Message::user("loop forever"), ToolContext::default())
        .await
        .expect_err("six identical rounds must abort");

    assert!(matches!(err, CoreError::LoopDetected(_)), "got {err:?}");
    assert!(
        !agent
            .messages()
            .iter()
            .any(|m| m.text_content().contains("gray loop guard")),
        "the backstop must not inject text"
    );
}

#[tokio::test]
async fn loop_guard_lets_the_model_recover() {
    // Three identical rounds → one different call → clean end.
    let mut scripts: Vec<Vec<StreamEvent>> =
        (0..3).map(|i| tool_script(&format!("c{i}"))).collect();
    scripts.push(read_script("r1", "/tmp/other.rs"));
    scripts.push(end_script());
    let provider = FakeProvider::new(scripts);
    let executor = FakeExecutor::new(ToolOutput::ok("ok"));
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor)).with_tools(vec![tool_def()]);

    let events = agent
        .run(Message::user("poll then finish"), ToolContext::default())
        .await
        .expect("a run that changes approach must not abort");

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
    );
}

#[tokio::test]
async fn loop_guard_exempts_job_polls() {
    // A repeated command that gray answers with the live status of the job it
    // already started is a deliberate wait — the elapsed time moves — so the
    // signature streak must not accumulate. Real transcript case: pebble's
    // 58/59 run was aborted after three identical polls of a running test job.
    let mut scripts: Vec<Vec<StreamEvent>> =
        (0..10).map(|i| tool_script(&format!("c{i}"))).collect();
    scripts.push(end_script());
    let provider = FakeProvider::new(scripts);
    let executor = FakeExecutor::new(ToolOutput::ok(
        "job j1 · running · elapsed 50s · log /tmp/j1.log",
    ));
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor)).with_tools(vec![tool_def()]);

    let events = agent
        .run(Message::user("poll the job"), ToolContext::default())
        .await
        .expect("polling a live job must not abort the run");

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
    );
}

#[tokio::test]
async fn retryable_failure_with_no_output_retries_whole_turn_and_recovers() {
    // The provider's in-request budget (5 attempts) dies to a 503 burst; the
    // agent retries the whole turn instead of killing the run the user waits on.
    let provider = FakeProvider::new(vec![end_script()]).with_failures(vec![
        ProviderError::ServerError("provider-side status 503".into()),
        ProviderError::ServerError("provider-side status 503".into()),
    ]);
    let seen = provider.seen_requests();
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    );

    let events = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect("turn-level retry must recover");

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnEnd { .. })),
        "recovered turn must complete: {events:?}"
    );
    let seen = seen.lock().expect("seen lock");
    assert_eq!(seen.len(), 3, "two failed requests + one successful replay");
}

#[tokio::test]
async fn retryable_failure_budget_exhausts_and_surfaces() {
    // Past MAX_TURN_RETRIES the error must surface (never loop forever).
    let provider = FakeProvider::new(vec![]).with_failures(vec![
        ProviderError::ServerError("503".into()),
        ProviderError::ServerError("503".into()),
        ProviderError::ServerError("503".into()),
    ]);
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    );

    let err = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect_err("exhausted retry budget must surface");
    assert!(
        err.to_string().contains("503"),
        "original error survives, got {err}"
    );
}

#[tokio::test]
async fn non_retryable_failure_never_retries_whole_turn() {
    // Auth is terminal: exactly one request, error surfaced.
    let provider = FakeProvider::new(vec![end_script()])
        .with_failures(vec![ProviderError::Auth("401".into())]);
    let seen = provider.seen_requests();
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    );

    let err = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect_err("auth failure must surface");
    assert!(err.to_string().contains("auth"), "got {err}");
    assert_eq!(seen.lock().expect("seen lock").len(), 1, "no replay");
}

#[tokio::test]
async fn mid_stream_failure_after_visible_delta_does_not_replay() {
    // Once a delta reached the user, a whole-turn replay would duplicate it.
    let provider = FakeProvider::new(vec![end_script()]).with_partial_failures(vec![(
        vec![StreamEvent::text_delta("visible half")],
        ProviderError::ServerError("503".into()),
    )]);
    let seen = provider.seen_requests();
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    );

    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect_err("post-delta failure must surface");
    assert_eq!(
        seen.lock().expect("seen lock").len(),
        1,
        "no replay after a visible delta"
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
    // Compaction retains the newest history within min(64k,
    // window−reserve) behind a leading summary (pi order): seed more than
    // the 64k retained budget (unknown window) so the oldest messages drop
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
    // Compaction is observable, not silent: one Compacted record whose
    // after-counts are strictly smaller (arXiv:2512.22087 / 2601.16746
    // accounting: token spend per task, not just the final score).
    let compacted: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Compacted {
                tokens_before,
                tokens_after,
                messages_before,
                messages_after,
            } => Some((
                *tokens_before,
                *tokens_after,
                *messages_before,
                *messages_after,
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        compacted.len(),
        1,
        "exactly one compaction record: {compacted:?}"
    );
    let (tb, ta, mb, ma) = compacted[0];
    assert!(ta < tb, "tokens shrink: {tb} -> {ta}");
    assert!(ma < mb, "messages shrink: {mb} -> {ma}");
    let msgs = agent.messages();
    assert!(
        msgs[0].text_content().starts_with("bulk0:"),
        "stable anchor leads: [anchor, summary, retained..., reply], got {:?}",
        msgs[0].text_content().chars().take(60).collect::<String>()
    );
    assert!(
        msgs[1]
            .text_content()
            .contains("compacted into the following summary"),
        "summary follows the pinned anchor"
    );
    let summaries = msgs
        .iter()
        .filter(|m| {
            m.text_content()
                .contains("compacted into the following summary")
        })
        .count();
    assert_eq!(summaries, 1, "exactly one summary");
    let last = msgs.len() - 1;
    assert_eq!(
        msgs[last].text_content(),
        "continued",
        "reply follows the tail"
    );
    assert_eq!(
        msgs[last - 1].text_content(),
        "go",
        "the retry request ended on the user's turn, not on a canned ack"
    );
    assert!(
        msgs[2..last - 1]
            .iter()
            .all(|m| m.text_content().starts_with("bulk")),
        "everything between summary and prompt is retained history \
         (index 0 is the pinned intent anchor, index 1 the summary)"
    );
}

/// History is append-only (mini-swe-agent / pi): the model keeps every tool
/// result it observed verbatim, so each request extends the previous one
/// and the provider prefix cache stays hot.
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
        "tool results are never rewritten: {observed:?}"
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

/// Text ending on EndTurn — distinct from `end_script` so the tail says what
/// the case needs (announced intent vs. a clean ending).
fn text_end_script(text: &str) -> Vec<StreamEvent> {
    vec![
        StreamEvent::text_delta(text.to_string()),
        StreamEvent::message_complete(Some(StopReason::EndTurn), None),
    ]
}

#[tokio::test]
async fn announced_step_ending_nudges_and_continues() {
    // The real-world miss: a text-only EndTurn whose tail commits to a step
    // ("Now let me find out … — searching for …") leaves the run dead while
    // the user is still waiting on the announced action.
    let provider = FakeProvider::new(vec![
        tool_script("c1"),
        text_end_script(
            "`ddgs` CLI is available. Now let me find out what it looks like — searching for images.",
        ),
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
            .any(|m| m.text_content().contains("announcing a next step")),
        "expected the announced-step nudge in history"
    );
}

#[tokio::test]
async fn announced_step_nudge_fires_once_per_run() {
    // intent ending -> nudge -> intent ending again: the repeat is the
    // model's answer to the nudge and must be honored, not nudged forever.
    let provider = FakeProvider::new(vec![
        text_end_script("Let me check the logs."),
        text_end_script("I'll look at the config next."),
    ]);
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    )
    .with_tools(vec![tool_def()]);

    let events = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect("repeat intent endings must still terminate");

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
    );
    let nudges = agent
        .messages()
        .iter()
        .filter(|m| m.text_content().contains("announcing a next step"))
        .count();
    assert_eq!(nudges, 1, "announced-step nudge must fire once per run");
}

#[tokio::test]
async fn clean_and_user_directed_endings_do_not_nudge() {
    // Each of these is a legitimate turn end: a question for the user, a
    // "let me know" deferral, a negated plan, and a plain summary.
    for text in [
        "Which file should I change?",
        "All done — let me know if you want changes.",
        "I won't touch the migration until you confirm.",
        "Updated the script and regenerated the plate.",
        "I can't proceed without the API key.",
    ] {
        let provider = FakeProvider::new(vec![text_end_script(text)]);
        let mut agent = Agent::new(
            Box::new(provider),
            Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
        )
        .with_tools(vec![tool_def()]);

        agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .expect("run should end cleanly");

        assert!(
            !agent
                .messages()
                .iter()
                .any(|m| m.text_content().contains("announcing a next step")),
            "no nudge expected for ending: {text}"
        );
    }
}

#[tokio::test]
async fn endings_waiting_on_the_user_do_not_nudge() {
    // The real misses: a plan that ends on "Say go … and I'll implement",
    // and an apology that ends waiting on the user's call. Nudging these
    // ("take it now") makes the model act on work the user never approved.
    for text in [
        "Two questions before I start:\n\n1. **Quit with jobs running?** Confirm, or leave them?\n2. **Verb scope.** Is that set right, or add `git` too?\n\nSay go, with your answers, and I'll implement both.",
        "Should I switch back and delete the new branch, or keep it? I'll wait for that, your answers to the two questions, and your go.",
        "Plan: wire the footer, then the picker.\n\nOnce you confirm, I'll start with the footer.",
        "Want me to open a PR?\nI'll push the branch after that.",
        "I'll hold off until you approve the design.",
    ] {
        let provider = FakeProvider::new(vec![text_end_script(text)]);
        let mut agent = Agent::new(
            Box::new(provider),
            Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
        )
        .with_tools(vec![tool_def()]);

        agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .expect("run should end cleanly");

        assert!(
            !agent
                .messages()
                .iter()
                .any(|m| m.text_content().contains("announcing a next step")),
            "no nudge expected for ending: {text}"
        );
    }
}

#[tokio::test]
async fn pending_enumeration_endings_nudge() {
    // The real miss: swe-2 ended a turn on "I'm mid-investigation. Two
    // things to pin down: whether …, and how …" — no first-person marker,
    // and the intent clause sat past the old 160-char window.
    for text in [
        "I'm mid-investigation. Two things to pin down: whether the CLI has a separate effort flag (or the tier only lives in the model id), and how the picker sends the chosen effort to `provider/chat`.",
        "Still need: the elided middle of `mod` and the host's param shapes.",
        "Progress so far: the relay answers but the calls never parse. Next: fix the funnel.",
        "I'm still checking whether the sidecar respawned.",
        "Retrying the reads — the previous calls didn't return output.",
        "We need to verify the wire format before writing the adapter.",
    ] {
        let provider = FakeProvider::new(vec![text_end_script(text), end_script()]);
        let mut agent = Agent::new(
            Box::new(provider),
            Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
        )
        .with_tools(vec![tool_def()]);

        agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .expect("run should nudge then end");

        assert!(
            agent
                .messages()
                .iter()
                .any(|m| m.text_content().contains("announcing a next step")),
            "expected nudge for ending: {text}"
        );
    }
}

#[tokio::test]
async fn leaked_call_markup_ending_nudges() {
    // swe-2 via the devin-sub funnel writes its calls as a fenced block
    // terminated by native markup instead of the close fence, then
    // hallucinates the next [User]/[Assistant] turns — the reply is
    // text-only on the wire, so the run used to die.
    let leaked = concat!(
        "Before writing code, let me grab the middle of `mod`.\n\n```gray_calls\n",
        "[{\"name\": \"bash\", \"arguments\": {\"command\": \"sed -n '167,355p' mod\"}}]",
        "<\x7cclose\x7c>argument<\x7csep\x7c><\x7cclose\x7c>call<\x7csep\x7c><\x7cclose\x7c>tools<\x7csep\x7c>",
        "\n\n[User]\nContinue.\n\n[Assistant]\nRetrying my reads — the previous calls didn't return output."
    );
    let provider = FakeProvider::new(vec![text_end_script(leaked), end_script()]);
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    )
    .with_tools(vec![tool_def()]);

    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect("markup leak should nudge");

    assert!(
        agent
            .messages()
            .iter()
            .any(|m| m.text_content().contains("announcing a next step")),
        "leaked tool-call markup should count as an announced step"
    );
}

#[tokio::test]
async fn instruction_endings_do_not_nudge() {
    // Answers that hand the user a step are finals, not pending work:
    // "you can …", "run X to verify", "here's how to fix it".
    for text in [
        "Fixed. You can verify with `cargo check`.",
        "Here's how to confirm it: run the probe again.",
        "The fix was the missing close fence — calls parse now.",
        "All green — tests pass, docs updated.",
    ] {
        let provider = FakeProvider::new(vec![text_end_script(text)]);
        let mut agent = Agent::new(
            Box::new(provider),
            Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
        )
        .with_tools(vec![tool_def()]);

        agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .expect("run should end cleanly");

        assert!(
            !agent
                .messages()
                .iter()
                .any(|m| m.text_content().contains("announcing a next step")),
            "no nudge expected for ending: {text}"
        );
    }
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
fn summary_message_envelope_is_byte_stable() {
    let m = super::summary_message("  hello world  ");
    assert_eq!(m.role, Role::User, "pi: the summary is a user turn, no ack");
    assert_eq!(
        m.text_content(),
        "The conversation history before this point was compacted into the following summary:\n\n<summary>\nhello world\n</summary>"
    );
    // Byte-equality: trimming + envelope must never drift.
    let m2 = super::summary_message("hello world");
    assert_eq!(m.text_content().as_bytes(), m2.text_content().as_bytes());
}

#[test]
fn context_estimate_anchors_on_provider_usage() {
    // pi `estimateContextTokens`: the provider's report already counts the
    // system prompt and tools; only messages appended after it are guessed.
    let mut agent = Agent::new(
        Box::new(FakeProvider::new(vec![])),
        Arc::new(FakeExecutor::new(ToolOutput::ok(""))),
    );
    agent.set_messages(vec![Message::user("q"), Message::assistant("a")]);
    agent.record_context_usage(&Usage::new(50_000, 1_000));
    assert_eq!(agent.estimate_tokens(), 51_000);
    agent.messages.push(Message::user("x".repeat(4_000)));
    assert_eq!(agent.estimate_tokens(), 52_000, "trailing bytes/4 on top");
    agent.record_context_usage(&Usage::default());
    assert_eq!(
        agent.estimate_tokens(),
        52_000,
        "a round without usage keeps the anchor"
    );
    agent.set_messages(vec![Message::user("x".repeat(400))]);
    assert_eq!(
        agent.estimate_tokens(),
        100,
        "a rewrite drops the stale anchor"
    );
}

#[tokio::test]
async fn provider_usage_triggers_pre_turn_compaction() {
    // 70 × ~1k-token messages: bytes/4 (~70k) + reserve stays under the
    // 200k window, so the old message-only estimate never compacted even
    // when the provider reported a nearly full context.
    let provider = FakeProvider::new(vec![
        vec![
            StreamEvent::tool_call_delta(
                0,
                Some("c1".into()),
                Some(TOOL_NAME.into()),
                r#"{"q":"x"}"#,
            ),
            StreamEvent::message_complete(Some(StopReason::ToolUse), Some(Usage::new(190_000, 10))),
        ],
        vec![
            StreamEvent::text_delta("S"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), None),
        ],
        end_script(),
    ]);
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("result"))),
    )
    .with_tools(vec![tool_def()])
    .with_context_window(Some(200_000))
    .with_messages(
        (0..70)
            .map(|i| Message::user(format!("bulk{i}:{}", "x".repeat(3990))))
            .collect(),
    );

    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect("compaction then reply");

    let msgs = agent.messages();
    assert!(
        msgs[0].text_content().starts_with("bulk0:"),
        "190k reported of 200k must compact before the next request; \
         the pinned intent leads the compacted transcript"
    );
    assert!(
        msgs[1]
            .text_content()
            .contains("compacted into the following summary"),
        "summary follows the pinned anchor"
    );
    assert_eq!(
        msgs.last().map(Message::text_content).as_deref(),
        Some("done")
    );
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
async fn prompt_context_replies_concatenate_into_context_note() {
    let provider = FakeProvider::new(vec![end_script()]);
    let seen = provider.seen_requests();
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
    let (system, messages) = &seen[0];
    assert_eq!(
        system.as_deref(),
        Some("BASE-SYSTEM"),
        "hook text must not perturb the system prefix"
    );
    let note = messages
        .iter()
        .find(|m| m.text_content().contains("[Context update]"))
        .expect("hook replies land as a context note in history");
    let note = note.text_content();
    let (a, b) = (note.find("PLUGIN-CTX-AAA"), note.find("PLUGIN-CTX-BBB"));
    assert!(
        a.is_some() && b.is_some(),
        "both hook replies present, got: {note}"
    );
    assert!(a.unwrap() < b.unwrap(), "hook order kept, got: {note}");
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
    let seen = provider.seen_requests();
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
    for (system, messages) in seen.iter() {
        assert!(
            messages
                .iter()
                .any(|m| m.text_content().contains("STABLE-CTX")),
            "every request carries the context note in history (system: {system:?})"
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
    let scripts: Vec<Vec<StreamEvent>> = (0..6).map(|i| tool_script(&format!("c{i}"))).collect();
    let mut agent = Agent::new(
        Box::new(FakeProvider::new(scripts)),
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
    // Every member is marked executing (`ToolCallEnd`) before any result:
    // a batch never sits in "Preparing tool" while it actually runs.
    let last_end = events
        .iter()
        .rposition(|e| matches!(e, AgentEvent::ToolCallEnd { .. }));
    let first_result = events
        .iter()
        .position(|e| matches!(e, AgentEvent::ToolResult { .. }));
    assert!(
        last_end.is_some() && last_end < first_result,
        "both ends precede the first result, {events:?}"
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
    // The lane emits every `ToolCallEnd` before the run starts, so the two
    // runs interleave differently; per-call event streams must still match.
    let call_events = |evs: &[AgentEvent], want: &str| -> Vec<AgentEvent> {
        evs.iter()
            .filter(|e| match e {
                AgentEvent::ToolCallStart { id, .. }
                | AgentEvent::ToolCallProgress { id, .. }
                | AgentEvent::ToolCallEnd { id, .. }
                | AgentEvent::ToolResult { id, .. } => id == want,
                _ => true,
            })
            .cloned()
            .collect()
    };
    for id in ["c1", "c2"] {
        assert_eq!(
            call_events(&runs[0], id),
            call_events(&runs[1], id),
            "lane off must match lane on for call {id}"
        );
    }
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

#[test]
fn tool_output_image_blocks_thread_after_text_result() {
    let out = ToolOutput::image(
        "Image read successfully: tiny.png",
        "image/png".into(),
        "AAA".into(),
    );
    let blocks = out.message_blocks("call_1");
    assert_eq!(blocks.len(), 2);
    assert!(matches!(
        &blocks[0],
        ContentBlock::ToolResult { id, content, is_error: false }
        if id == "call_1" && content.contains("Image read successfully")
    ));
    assert!(matches!(
        &blocks[1],
        ContentBlock::Image { media_type, data }
        if media_type == "image/png" && data == "AAA"
    ));
    // Plain results carry no vision blocks.
    assert_eq!(ToolOutput::ok("hi").message_blocks("c").len(), 1);
}

#[test]
fn tool_output_media_goes_native_or_as_its_fallback() {
    use crate::message::resolve_media;
    let out = ToolOutput {
        content: "clip.mp4 (video)".into(),
        is_error: false,
        images: Vec::new(),
        media: vec![crate::agent::AttachedMedia {
            media_type: "video/mp4".into(),
            data: "VID".into(),
            fallback: vec![
                ContentBlock::text("(contact sheet)"),
                ContentBlock::image("image/jpeg", "SHEET"),
            ],
        }],
    };
    let blocks = out.message_blocks("c");
    let native = resolve_media(blocks.clone(), |mt| mt == "video/mp4");
    assert_eq!(native.len(), 2);
    assert!(matches!(&native[1], ContentBlock::Media { data, .. } if data == "VID"));
    let sheet = resolve_media(blocks, |_| false);
    assert_eq!(sheet.len(), 3);
    assert!(matches!(&sheet[1], ContentBlock::Text { text } if text.contains("contact sheet")));
    assert!(matches!(&sheet[2], ContentBlock::Image { data, .. } if data == "SHEET"));
    // No fallback → a note, never a hard error.
    let bare = resolve_media(vec![ContentBlock::media("audio/wav", "A", vec![])], |_| {
        false
    });
    assert!(matches!(&bare[0], ContentBlock::Text { text } if text.contains("audio/wav")));
    // Over the native cap → fallback even when the type is accepted.
    let big = "A".repeat(crate::message::MAX_NATIVE_MEDIA_BYTES / 3 * 4 + 8);
    let over = resolve_media(vec![ContentBlock::media("video/mp4", big, vec![])], |_| {
        true
    });
    assert!(matches!(&over[0], ContentBlock::Text { .. }));
}

#[test]
fn old_session_video_block_still_parses() {
    let b: ContentBlock =
        serde_json::from_str(r#"{"type":"video","media_type":"video/mp4","data":"VID"}"#).unwrap();
    assert!(matches!(b, ContentBlock::Media { fallback, .. } if fallback.is_empty()));
}

#[tokio::test]
async fn eof_with_text_and_tool_pending_salvages_visible_text() {
    // The user already saw "hello" on screen; dropping it because a tool
    // delta was also pending rewrites history to something never shown.
    let provider = FakeProvider::new(vec![vec![
        StreamEvent::text_delta("hello"),
        StreamEvent::tool_call_delta(
            0,
            Some("c-x".into()),
            Some(TOOL_NAME.into()),
            r#"{"q":"x"}"#,
        ),
    ]]);
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("ok"))),
    )
    .with_tools(vec![tool_def()]);
    let err = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect_err("EOF without completion must fail");
    assert!(
        err.to_string().contains("without completion"),
        "got {err:?}"
    );
    assert!(
        agent
            .messages()
            .iter()
            .flat_map(|m| m.content.iter())
            .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("hello"))),
        "visible text must survive in history"
    );
    assert!(
        !agent
            .messages()
            .iter()
            .flat_map(|m| m.content.iter())
            .any(|b| matches!(
                b,
                ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }
            )),
        "truncated tool call must still never land in history"
    );
}

#[tokio::test]
async fn reasoning_only_maxtokens_is_not_retried_as_empty() {
    // A reasoning model that stops with finish_reason=length after producing
    // only thinking must NOT fall into the empty-turn retry (same request,
    // same limit, up to 3x the output cost with the reasoning dropped): the
    // MaxTokens branch owns the turn and keeps the thinking.
    let provider = FakeProvider::new(vec![vec![
        StreamEvent::thinking_delta("deep reasoning..."),
        StreamEvent::message_complete(Some(StopReason::MaxTokens), None),
    ]]);
    let executor = FakeExecutor::new(ToolOutput::ok("unused"));
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor));

    let events = agent
        .run(Message::user("think hard"), ToolContext::default())
        .await
        .unwrap();

    let empty_ends = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::TurnEnd { .. }))
        .count();
    assert_eq!(empty_ends, 1, "exactly one turn end: {events:?}");
    let thinking = agent
        .messages()
        .last()
        .map(|m| {
            m.content
                .iter()
                .any(|b| matches!(b, ContentBlock::Thinking { .. }))
        })
        .unwrap_or(false);
    assert!(
        thinking,
        "truncated reasoning must persist: {:?}",
        agent.messages().last()
    );
}

/// A reasoning item that arrives with no summary text still attaches its
/// encrypted carrier to history (claude-sub relays the item alone —
/// thinking prose is never forwarded). The empty-text block replays the
/// same: the wire item carries the blob, not the text.
#[tokio::test]
async fn reasoning_item_without_summary_attaches_carrier() {
    struct Modeled {
        inner: FakeProvider,
    }
    #[async_trait]
    impl Provider for Modeled {
        fn stream(&self, req: ChatRequest) -> ProviderStream {
            self.inner.stream(req)
        }
        fn model_id(&self) -> &str {
            "m1"
        }
    }
    let provider = Modeled {
        inner: FakeProvider::new(vec![vec![
            StreamEvent::reasoning_item("rs_1", "blob"),
            StreamEvent::text_delta("visible"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), None),
        ]]),
    };
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok(""))),
    );
    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .unwrap();
    let last = agent.messages().last().unwrap();
    let carrier = last.content.iter().find_map(|b| match b {
        ContentBlock::Thinking {
            text,
            encrypted_content,
            item_id,
            model,
        } => Some((
            text.clone(),
            encrypted_content.clone(),
            item_id.clone(),
            model.clone(),
        )),
        _ => None,
    });
    assert_eq!(
        carrier,
        Some((
            String::new(),
            Some("blob".to_string()),
            Some("rs_1".to_string()),
            Some("m1".to_string())
        )),
        "carrier must attach even without thinking text: {last:?}"
    );
}

#[test]
fn image_budget_is_bounded_and_rewrites_notify_session() {
    let mut agent = Agent::new(
        Box::new(FakeProvider::new(vec![])),
        Arc::new(FakeExecutor::new(ToolOutput::ok(""))),
    );
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hook_count = count.clone();
    agent = agent.with_history_rewrite_hook(Arc::new(move || {
        hook_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));
    let before = agent.history_revision();
    agent.set_messages(vec![Message {
        role: Role::User,
        content: vec![ContentBlock::image(
            "image/png",
            "a".repeat(5 * 1024 * 1024),
        )],
    }]);
    assert!(agent.estimate_tokens() <= 4096);
    assert!(agent.estimate_tokens() >= 1000);
    assert_ne!(agent.history_revision(), before);
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// Notices may arrive during inference or tool execution, but must not split
/// an assistant tool-call batch from its results or hold an idle turn open.
#[tokio::test]
async fn background_notices_land_at_safe_boundaries_without_waiting() {
    struct Notices {
        polls: std::sync::atomic::AtomicUsize,
        at: usize,
    }
    #[async_trait]
    impl ToolExecutor for Notices {
        fn execute(
            &self,
            _: &ToolContext,
            _: &str,
            _: serde_json::Value,
        ) -> BoxFuture<'static, ToolOutput> {
            Box::pin(async { ToolOutput::ok("job started; still running") })
        }
        fn drain_notifications(&self, _: &ToolContext) -> Vec<String> {
            let poll = self.polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if poll == self.at {
                vec!["job finished".into()]
            } else {
                vec![]
            }
        }
    }
    // Poll 0: before inference. Poll 1: next round after all tool results.
    // Poll 2: just before returning a final answer (job finished mid-inference).
    // Never: an unfinished job must not keep the turn open.
    for at in [0, 1, 2, usize::MAX] {
        let provider = FakeProvider::new(vec![tool_script("bg-call"), end_script(), end_script()]);
        let executor = Arc::new(Notices {
            polls: std::sync::atomic::AtomicUsize::new(0),
            at,
        });
        let mut agent = Agent::new(Box::new(provider), executor).with_tools(vec![tool_def()]);
        tokio::time::timeout(
            Duration::from_secs(2),
            agent.run(Message::user("go"), ToolContext::default()),
        )
        .await
        .unwrap()
        .unwrap();
        let messages = agent.messages();
        let call = messages
            .iter()
            .position(|m| {
                m.content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolUse { id, .. } if id == "bg-call"))
            })
            .unwrap();
        assert!(
            messages[call + 1]
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { id, .. } if id == "bg-call"))
        );
        let notices: Vec<_> = messages.iter().enumerate().filter(|(_, m)| m.content.iter().any(|b| matches!(b, ContentBlock::Text { text } if text.contains("[Background task notification]")))).collect();
        if at == usize::MAX {
            assert!(notices.is_empty());
        } else {
            assert_eq!(notices.len(), 1);
            if at == 0 {
                assert!(notices[0].0 < call);
            } else {
                assert!(notices[0].0 > call + 1);
            }
            assert!(
                messages
                    .last()
                    .unwrap()
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Text { text } if text == "done"))
            );
        }
    }
}

// --- arXiv-driven harness hardening (2026-09 sweep) -------------------------

/// Hook serving a constant per-turn context block.
struct StaticHook;

#[async_trait]
impl PluginHooks for StaticHook {
    async fn prompt_context(&self) -> Option<String> {
        Some("HOOK-CTX".to_string())
    }
}

/// arXiv:2601.06007 ("Don't Break the Cache"): provider prefix caching pays
/// only when the request prefix is byte-stable — every round of a turn must
/// send the identical system prompt (the base prompt alone; hook context
/// rides a deduped transcript note) and a strictly prefix-extending message
/// list (append-only history between compactions).
#[tokio::test]
async fn request_prefix_is_byte_stable_across_tool_rounds() {
    let provider = FakeProvider::new(vec![tool_script("c1"), tool_script("c2"), end_script()]);
    let seen = provider.seen_requests();
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("done"))),
    )
    .with_system("base system prompt")
    .with_tools(vec![tool_def()])
    .with_hooks(vec![Arc::new(StaticHook)]);

    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect("two tool rounds then end");

    let seen = seen.lock().expect("seen lock").clone();
    assert_eq!(seen.len(), 3, "one request per round");
    for (i, w) in seen.windows(2).enumerate() {
        assert_eq!(
            w[0].0.as_deref(),
            Some("base system prompt"),
            "request {i}: the system prompt carries no per-turn hook text"
        );
        assert_eq!(
            w[0].0, w[1].0,
            "request {i}: system prompt must be byte-stable across rounds"
        );
        assert!(
            w[1].1.len() > w[0].1.len() && w[1].1.starts_with(&w[0].1),
            "request {}: each round's messages must strictly prefix-extend the previous",
            i + 1
        );
    }
}

/// arXiv:2605.08563 (CCRM): a mid-stream failure salvages the partial into
/// history (the transcript must match what the user saw), but the next
/// turn's request must not carry it — a failed attempt left in context
/// contaminates the retry.
#[tokio::test]
async fn mid_stream_error_partial_is_scrubbed_from_the_next_request() {
    let provider = FakeProvider::new(vec![end_script()]).with_partial_failures(vec![(
        vec![StreamEvent::text_delta("half-finished answer")],
        ProviderError::Stream("connection reset".into()),
    )]);
    let seen = provider.seen_requests();
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    );

    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect_err("mid-stream error surfaces");
    // In-memory history (and so the persisted transcript) keeps the partial:
    // it matches what the user saw on screen.
    assert!(
        agent
            .messages()
            .iter()
            .any(|m| m.text_content().contains("half-finished answer")),
        "the salvaged partial stays in history"
    );

    agent
        .run(Message::user("try again"), ToolContext::default())
        .await
        .expect("retry succeeds");

    let seen = seen.lock().expect("seen lock");
    let retry = &seen[1].1;
    assert!(
        !retry
            .iter()
            .any(|m| m.text_content().contains("half-finished answer")),
        "the failed attempt must not ride the retry's context: {retry:?}"
    );
    assert!(
        retry
            .iter()
            .any(|m| m.text_content().contains("start fresh")),
        "a one-line marker takes its place: {retry:?}"
    );
}

/// Mid-turn steer: text typed while the turn runs joins that turn, at the
/// boundary before the next model request -- not after the turn, and never
/// between a tool call and its result.
#[tokio::test]
async fn steer_hook_text_reaches_the_running_turn() {
    let provider = FakeProvider::new(vec![
        tool_script("call_1"),
        vec![
            StreamEvent::text_delta("adjusted"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), Some(Usage::new(5, 5))),
        ],
    ]);
    let executor = FakeExecutor::new(ToolOutput::ok("result payload"));
    let seen = provider.seen_requests();
    // One injection, then silence: the hook is polled before every request.
    let pending = std::sync::Arc::new(std::sync::Mutex::new(Some(
        "actually, use the other flag".to_string(),
    )));
    let hook = {
        let pending = pending.clone();
        std::sync::Arc::new(move || pending.lock().unwrap().take())
    };
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor)).with_tools(vec![tool_def()]);
    agent.set_steer(hook);

    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect("run should succeed");

    let requests = seen.lock().expect("seen lock").clone();
    assert_eq!(requests.len(), 2, "the tool round makes a second request");
    let second: Vec<String> = requests[1].1.iter().map(|m| m.text_content()).collect();
    assert!(
        second
            .iter()
            .any(|text| text.contains("actually, use the other flag")),
        "steer text must ride the next request: {second:?}"
    );
    // It is a real user message, so the persisted transcript keeps it too.
    assert!(
        agent
            .messages()
            .iter()
            .any(|m| m.role == Role::User && m.text_content().contains("use the other flag")),
        "steer text must stay in the transcript"
    );
}

/// No hook (or a hook that returns `None`) must leave a turn byte-identical:
/// steering is additive, never a rewrite.
#[tokio::test]
async fn a_silent_steer_hook_changes_nothing() {
    let provider = FakeProvider::new(vec![
        tool_script("call_1"),
        vec![
            StreamEvent::text_delta("done"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), None),
        ],
    ]);
    let executor = FakeExecutor::new(ToolOutput::ok("ok"));
    let seen = provider.seen_requests();
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor)).with_tools(vec![tool_def()]);
    agent.set_steer(std::sync::Arc::new(|| None));

    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect("run should succeed");

    let requests = seen.lock().expect("seen lock").clone();
    assert_eq!(requests.len(), 2);
    // Exactly the unsteered shape: the prompt, the assistant tool call and its
    // result. A silent hook must not add a message of its own.
    let roles: Vec<Role> = requests[1].1.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        vec![Role::User, Role::Assistant, Role::User],
        "nothing typed, nothing added"
    );
}

/// One tool round: assistant `ToolUse` + the paired `ToolResult` carrying
/// `bytes` of output. Tool results are what the stale mask claims.
fn big_tool_round(id: &str, bytes: usize) -> Vec<Message> {
    vec![
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::tool_use(
                id,
                TOOL_NAME,
                serde_json::json!({"q": "x"}),
            )],
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::tool_result(id, "y".repeat(bytes), false)],
        },
    ]
}

/// The outbound view's tool-result bodies, in order.
fn tool_result_bodies(msgs: &[Message]) -> Vec<&str> {
    msgs.iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn stale_tool_output_masks_with_citations_and_keeps_history_intact() {
    let bytes = crate::compact::ARC_STUB_MIN_BYTES + 1;
    let mut agent = Agent::new(
        Box::new(FakeProvider::new(vec![])),
        Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    );
    agent.session_id = Some("sess-mask".into());
    // 16 groups (opening user turn + 14 tool rounds + closing user turn) put
    // the six oldest out of reach of the model, and one of those six is the
    // opening turn.
    let mut msgs = vec![Message::user("go")];
    for i in 0..14 {
        msgs.extend(big_tool_round(&format!("call_{i}"), bytes));
    }
    msgs.push(Message::user("status?"));
    agent.set_messages(msgs);
    agent.mask_stale_tool_output();
    let view = agent.scrubbed_messages();
    let bodies = tool_result_bodies(&view);
    assert_eq!(bodies.len(), 14, "every result still rides the request");
    for (i, body) in bodies.iter().enumerate() {
        if i < 5 {
            assert!(body.contains("elided from context"), "round {i}: {body}");
            let id = format!("call_{i}");
            assert!(body.contains(&id), "citation names the id: {body}");
            assert!(
                body.contains("~/.gray/sessions/sess-mask.jsonl"),
                "citation names the file: {body}"
            );
        } else {
            assert!(!body.contains("elided"), "fresh round {i} stays whole");
        }
    }
    // The transcript is untouched: the user saw these bytes, and the stub
    // says where to get them back.
    assert!(
        agent.messages().iter().flat_map(|m| m.content.iter()).all(
            |b| !matches!(b, ContentBlock::ToolResult { content, .. } if content.contains("elided"))
        ),
        "history keeps every byte"
    );
}

/// The mask never moves mid-turn: rewriting an old result into a stub
/// changes the cached prefix, and the next request re-bills all of it.
#[tokio::test]
async fn a_long_turn_never_rewrites_its_own_prefix() {
    let big = "y".repeat(crate::compact::ARC_STUB_MIN_BYTES + 1);
    let mut scripts: Vec<_> = (0..14)
        .map(|i| tool_script_with_args(&format!("c{i}"), &format!(r#"{{"q":"{i}"}}"#)))
        .collect();
    scripts.push(end_script());
    let provider = FakeProvider::new(scripts);
    let seen = provider.seen_requests();
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok(big))),
    )
    .with_tools(vec![tool_def()]);

    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect("fourteen tool rounds then end");

    let seen = seen.lock().expect("seen lock").clone();
    assert_eq!(seen.len(), 15, "one request per round");
    for (i, w) in seen.windows(2).enumerate() {
        assert!(
            w[1].1.starts_with(&w[0].1),
            "request {}: a stale result was rewritten mid-turn",
            i + 1
        );
    }
}

#[test]
fn a_cold_cache_masks_every_stale_result_now() {
    let bytes = crate::compact::ARC_STUB_MIN_BYTES + 1;
    let mut agent = Agent::new(
        Box::new(FakeProvider::new(vec![])),
        Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    );
    let mut msgs = vec![Message::user("go")];
    for i in 0..12 {
        msgs.extend(big_tool_round(&format!("call_{i}"), bytes));
    }
    msgs.push(Message::user("status?"));
    agent.set_messages(msgs);

    agent.mask_stale_tool_output();
    let view = agent.scrubbed_messages();
    let bodies = tool_result_bodies(&view);
    // 14 groups (opening user turn + 12 tool rounds + closing user turn), so
    // the four oldest hold the first three rounds: every one of them goes at
    // once, not one batch at a time.
    assert_eq!(
        bodies
            .iter()
            .filter(|b| b.contains("elided from context"))
            .count(),
        3,
        "all stale rounds go at once"
    );
    assert!(
        bodies[3..].iter().all(|b| !b.contains("elided")),
        "the fresh rounds stay whole"
    );
}

#[test]
fn small_results_and_recent_rounds_are_never_masked() {
    let mut agent = Agent::new(
        Box::new(FakeProvider::new(vec![])),
        Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    );
    let mut msgs = vec![Message::user("go")];
    // 12 stale rounds, but every result is small: below the threshold there
    // is nothing worth citing and nothing worth losing.
    for i in 0..14 {
        msgs.extend(big_tool_round(&format!("call_{i}"), 10));
    }
    msgs.push(Message::user("status?"));
    agent.set_messages(msgs);
    agent.mask_stale_tool_output();
    assert!(
        tool_result_bodies(&agent.scrubbed_messages())
            .iter()
            .all(|b| !b.contains("elided")),
        "a small result is cheaper to keep than to cite"
    );

    // 10 rounds exactly: nothing has aged out yet.
    let mut msgs = vec![Message::user("go")];
    for i in 0..10 {
        msgs.extend(big_tool_round(
            &format!("call_{i}"),
            crate::compact::ARC_STUB_MIN_BYTES + 1,
        ));
    }
    agent.set_messages(msgs);
    agent.mask_stale_tool_output();
    assert!(
        tool_result_bodies(&agent.scrubbed_messages())
            .iter()
            .all(|b| !b.contains("elided")),
        "the tenth round back is still in play"
    );
}

#[tokio::test]
async fn checkpoint_fires_once_per_step_with_consistent_prefix() {
    use std::sync::Arc;
    // First script ends with a tool call, second ends the turn.
    let provider = FakeProvider::new(vec![tool_script("call_1"), end_script()]);
    let executor = FakeExecutor::new(ToolOutput::ok("result payload"));
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor)).with_tools(vec![tool_def()]);

    // One atomic group per step: assistant tool-use + its result travel together.
    let seen: Arc<Mutex<Vec<Vec<Message>>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_hook = seen.clone();
    agent.set_checkpoint(Some(Arc::new(move |msgs: &[Message], _rev: u64| {
        seen_hook
            .lock()
            .expect("seen lock poisoned")
            .push(msgs.to_vec());
        Box::pin(async move {})
    })));

    agent
        .run(Message::user("find it"), ToolContext::default())
        .await
        .expect("run should succeed");

    let seen = seen.lock().expect("seen lock poisoned");
    // Once before the (slow, killable) prompt hooks, then once per round
    // boundary after notifications/steer: [user], [user] again, then
    // [user, assistant, result] before round 2.
    assert_eq!(seen.len(), 3, "hook must fire before each killable span");
    assert_eq!(seen[0].len(), 1, "first fire sees only the user message");
    assert_eq!(seen[1], seen[0], "round 1 boundary repeats the prefix");
    assert_eq!(
        seen[2],
        agent.messages()[..3],
        "second round sees full prefix"
    );
    // Every snapshot is answerable: each ToolUse has its ToolResult.
    for snap in seen.iter() {
        let uses: Vec<&str> = snap
            .iter()
            .flat_map(|m| m.content.iter())
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        let results: Vec<&str> = snap
            .iter()
            .flat_map(|m| m.content.iter())
            .filter_map(|b| match b {
                ContentBlock::ToolResult { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            uses, results,
            "snapshot must pair every call with its result"
        );
    }
}

/// Executor whose tool set is live: `live_defs` answers from a shared
/// slot the test can rewrite between turns.
struct LiveExecutor(std::sync::Arc<Mutex<Vec<ToolDef>>>);

#[async_trait]
impl ToolExecutor for LiveExecutor {
    fn execute(
        &self,
        _ctx: &ToolContext,
        _name: &str,
        _args: serde_json::Value,
    ) -> BoxFuture<'static, ToolOutput> {
        Box::pin(async { ToolOutput::ok("unused") })
    }

    fn live_defs(&self) -> Option<Vec<ToolDef>> {
        Some(self.0.lock().expect("live lock").clone())
    }
}

#[tokio::test]
async fn run_refreshes_tool_defs_from_live_executor() {
    let live = std::sync::Arc::new(Mutex::new(vec![ToolDef::new(
        "live_a",
        "live",
        serde_json::json!({"type":"object","properties":{}}),
    )]));
    let provider = FakeProvider::new(vec![
        vec![
            StreamEvent::text_delta("ok"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), None),
        ],
        vec![
            StreamEvent::text_delta("ok"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), None),
        ],
    ]);
    let seen = provider.seen_tools();
    let mut agent = Agent::new(
        Box::new(provider),
        std::sync::Arc::new(LiveExecutor(live.clone())),
    )
    .with_tools(vec![ToolDef::new("stale", "stale", serde_json::json!({}))]);

    agent
        .run(Message::user("hi"), ToolContext::default())
        .await
        .unwrap();
    assert_eq!(seen.lock().expect("seen lock")[0], vec!["live_a"]);
    assert_eq!(
        agent
            .tool_defs()
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        vec!["live_a"]
    );

    // The set changes between turns; the next run picks it up.
    live.lock().expect("live lock").push(ToolDef::new(
        "live_b",
        "live",
        serde_json::json!({"type":"object","properties":{}}),
    ));
    agent
        .run(Message::user("again"), ToolContext::default())
        .await
        .unwrap();
    assert_eq!(seen.lock().expect("seen lock")[1], vec!["live_a", "live_b"]);
}

#[tokio::test]
async fn static_executor_keeps_with_tools_defs() {
    let provider = FakeProvider::new(vec![vec![
        StreamEvent::text_delta("ok"),
        StreamEvent::message_complete(Some(StopReason::EndTurn), None),
    ]]);
    let seen = provider.seen_tools();
    let mut agent = Agent::new(
        Box::new(provider),
        std::sync::Arc::new(FakeExecutor::new(ToolOutput::ok("x"))),
    )
    .with_tools(vec![tool_def()]);
    agent
        .run(Message::user("hi"), ToolContext::default())
        .await
        .unwrap();
    assert_eq!(seen.lock().expect("seen lock")[0], vec![TOOL_NAME]);
}
