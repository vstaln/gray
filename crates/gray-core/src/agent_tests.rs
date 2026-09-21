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
}

impl FakeProvider {
    fn new(scripts: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            scripted: Mutex::new(VecDeque::from(scripts)),
            failures: Mutex::new(VecDeque::new()),
            partial_failures: Mutex::new(VecDeque::new()),
            seen_systems: std::sync::Arc::new(Mutex::new(Vec::new())),
            seen_requests: std::sync::Arc::new(Mutex::new(Vec::new())),
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
async fn loop_guard_nudges_once_then_aborts_at_six() {
    // Same tool+args 3× → nudge, run continues; still identical at 6× → abort.
    let scripts: Vec<Vec<StreamEvent>> = (0..6).map(|i| tool_script(&format!("c{i}"))).collect();
    let provider = FakeProvider::new(scripts);
    let executor = FakeExecutor::new(ToolOutput::ok("ok"));
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor)).with_tools(vec![tool_def()]);

    let err = agent
        .run(Message::user("loop forever"), ToolContext::default())
        .await
        .expect_err("six identical rounds must abort");

    assert!(matches!(err, CoreError::LoopDetected(_)), "got {err:?}");
    let nudges = agent
        .messages()
        .iter()
        .filter(|m| m.text_content().contains("gray loop guard"))
        .count();
    assert_eq!(nudges, 1, "exactly one nudge lands before the abort");
}

#[tokio::test]
async fn loop_guard_nudge_lets_the_model_recover() {
    // Three identical rounds (nudge) → one different call → clean end. A poll
    // that changes approach after the nudge must not be killed.
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
        .expect("nudge must not abort a run that changes approach");

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
    );
    assert!(
        agent
            .messages()
            .iter()
            .any(|m| m.text_content().contains("gray loop guard")),
        "expected the loop-guard nudge in history"
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
    assert!(
        !agent
            .messages()
            .iter()
            .any(|m| m.text_content().contains("gray loop guard")),
        "polls of a live job must neither nudge nor abort"
    );
}

#[tokio::test]
async fn loop_guard_nudges_hung_job_polls() {
    // A job polled forever is still called out once — but the run ends
    // cleanly instead of being killed mid-wait.
    let mut scripts: Vec<Vec<StreamEvent>> =
        (0..20).map(|i| tool_script(&format!("c{i}"))).collect();
    scripts.push(end_script());
    let provider = FakeProvider::new(scripts);
    let executor = FakeExecutor::new(ToolOutput::ok(
        "job j1 · running · elapsed 900s · log /tmp/j1.log",
    ));
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor)).with_tools(vec![tool_def()]);

    let events = agent
        .run(Message::user("poll forever"), ToolContext::default())
        .await
        .expect("hung-job polls must not abort the run");

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
    );
    assert!(
        agent
            .messages()
            .iter()
            .any(|m| m.text_content().contains("has been polled 20 times")),
        "expected the hung-job nudge in history"
    );
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

/// Executor whose output changes on every call: models a poll that makes
/// progress (or a growing log). Same command, different result.
struct ChangingExecutor {
    n: std::sync::Mutex<usize>,
}

impl ChangingExecutor {
    fn new() -> Self {
        Self {
            n: std::sync::Mutex::new(0),
        }
    }
}

#[async_trait]
impl ToolExecutor for ChangingExecutor {
    fn execute(
        &self,
        _ctx: &ToolContext,
        _name: &str,
        _args: serde_json::Value,
    ) -> BoxFuture<'static, ToolOutput> {
        let mut n = self.n.lock().expect("n lock poisoned");
        *n += 1;
        let out = format!("output revision {}", *n);
        Box::pin(async move { ToolOutput::ok(out) })
    }
}

#[tokio::test]
async fn repeat_guard_nudges_interleaved_identical_calls() {
    // The DeepSWE campaign's worst case re-ran one test command 16x,
    // interleaved with other calls, so no two consecutive rounds ever matched
    // and the consecutive-signature guard never fired. Same call, same
    // result, four times total (not in a row) must nudge.
    let mut scripts = vec![tool_script("c0"), tool_script("c0")];
    scripts.push(read_script("r1", "/tmp/a.rs"));
    scripts.push(tool_script("c0"));
    scripts.push(read_script("r2", "/tmp/b.rs"));
    scripts.push(tool_script("c0"));
    scripts.push(read_script("r3", "/tmp/c.rs"));
    scripts.push(tool_script("c0"));
    scripts.push(end_script());
    let provider = FakeProvider::new(scripts);
    let executor = FakeExecutor::new(ToolOutput::ok("same output every time"));
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor)).with_tools(vec![tool_def()]);

    agent
        .run(Message::user("flail"), ToolContext::default())
        .await
        .expect("the run must finish; the repeat guard nudges, it does not abort");

    assert!(
        agent
            .messages()
            .iter()
            .any(|m| m.text_content().contains("gray repeat guard")),
        "expected the interleaved-repeat nudge in history"
    );
}

#[tokio::test]
async fn repeat_guard_ignores_calls_whose_output_keeps_changing() {
    // A poll that makes progress is deliberate: same command, different
    // output each time must never nudge.
    let mut scripts = vec![tool_script("c0"), tool_script("c0")];
    for i in 0..6 {
        scripts.push(read_script(&format!("r{i}"), &format!("/tmp/{i}.rs")));
        scripts.push(tool_script("c0"));
    }
    scripts.push(end_script());
    let provider = FakeProvider::new(scripts);
    let executor = ChangingExecutor::new();
    let mut agent = Agent::new(Box::new(provider), Arc::new(executor)).with_tools(vec![tool_def()]);

    agent
        .run(Message::user("poll and read"), ToolContext::default())
        .await
        .expect("changing output is progress");

    assert!(
        !agent
            .messages()
            .iter()
            .any(|m| m.text_content().contains("gray repeat guard")),
        "same command with changing output must not nudge"
    );
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
/// send the identical system prompt (base + hook context appended at the
/// very end, where volatile content belongs) and a strictly prefix-extending
/// message list (append-only history between compactions).
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
            Some("base system prompt\n\nHOOK-CTX"),
            "request {i}: hook context rides at the very end of the system prompt"
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
