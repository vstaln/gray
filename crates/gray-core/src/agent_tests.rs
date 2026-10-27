use super::*;
use crate::event::{AgentEvent, StopReason};
use std::collections::VecDeque;
use std::sync::Mutex;

struct FakeProvider {
    scripted: Mutex<VecDeque<Vec<StreamEvent>>>,
    failures: Mutex<VecDeque<ProviderError>>,

    partial_failures: Mutex<VecDeque<(Vec<StreamEvent>, ProviderError)>>,
    seen_systems: std::sync::Arc<Mutex<Vec<Option<String>>>>,

    #[allow(clippy::type_complexity)]
    seen_requests: std::sync::Arc<Mutex<Vec<(Option<String>, Vec<Message>)>>>,

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

    #[allow(clippy::type_complexity)]
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

struct FakeExecutor {
    calls: std::sync::Arc<Mutex<Vec<String>>>,
    call_args: std::sync::Arc<Mutex<Vec<(String, serde_json::Value)>>>,
    by_name: Vec<(String, ToolOutput)>,
    default_output: ToolOutput,
    on_execute: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    delay: Option<std::time::Duration>,
    jobs_note: Option<String>,
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
            jobs_note: None,
        }
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

    fn running_jobs_note(&self, _ctx: &ToolContext) -> Option<String> {
        self.jobs_note.clone()
    }
}

fn end_script() -> Vec<StreamEvent> {
    vec![
        StreamEvent::text_delta("done"),
        StreamEvent::message_complete(Some(StopReason::EndTurn), None),
    ]
}

#[tokio::test]
async fn retryable_failure_with_no_output_retries_whole_turn_and_recovers() {
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
    let provider = FakeProvider::new(vec![]).with_failures(vec![
        ProviderError::ServerError("503".into()),
        ProviderError::ServerError("503".into()),
        ProviderError::ServerError("503".into()),
        ProviderError::ServerError("503".into()),
        ProviderError::ServerError("503".into()),
        ProviderError::ServerError("503".into()),
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
async fn quota_failure_retries_whole_turn_in_place_and_recovers() {
    let provider = FakeProvider::new(vec![end_script()]).with_failures(vec![ProviderError::Auth(
        "status 429: usage limit reached".into(),
    )]);
    let seen = provider.seen_requests();
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    );

    let events = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect("quota-window failure must retry inside the session");

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnEnd { .. })),
        "recovered turn must complete: {events:?}"
    );
    assert_eq!(
        seen.lock().expect("seen lock").len(),
        2,
        "failed request + one replay, same session"
    );
}

#[tokio::test]
async fn quota_failure_with_long_reset_window_is_terminal() {
    let provider = FakeProvider::new(vec![end_script()]).with_failures(vec![ProviderError::Auth(
        "usage limit reached, resets in ~2h".into(),
    )]);
    let seen = provider.seen_requests();
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    );

    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect_err("a multi-hour quota window must not be waited out");
    assert_eq!(seen.lock().expect("seen lock").len(), 1, "no replay");
}

#[tokio::test]
async fn mid_stream_quota_failure_replays() {
    let provider = FakeProvider::new(vec![end_script()]).with_partial_failures(vec![(
        vec![StreamEvent::text_delta("visible half")],
        ProviderError::Auth("status 429: usage limit reached".into()),
    )]);
    let seen = provider.seen_requests();
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    );

    agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect("a retryable failure after a visible delta must replay, not abort");
    assert_eq!(
        seen.lock().expect("seen lock").len(),
        2,
        "pi retries the whole assistant call even with partial output"
    );
    assert!(
        !agent
            .messages()
            .iter()
            .any(|m| m.text_content().contains("visible half")),
        "the failed attempt's partial text must not enter history"
    );
}

#[tokio::test]
async fn mid_stream_stall_after_visible_delta_replays() {
    let provider = FakeProvider::new(vec![end_script()]).with_partial_failures(vec![(
        vec![StreamEvent::text_delta("visible half")],
        ProviderError::Stream("stream stalled: no output for 75s".into()),
    )]);
    let seen = provider.seen_requests();
    let mut agent = Agent::new(
        Box::new(provider),
        Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    );

    let events = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .expect("a stalled stream mid-write must retry, not abort the run");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnEnd { .. })),
        "the retried turn completes: {events:?}"
    );
    assert_eq!(
        seen.lock().expect("seen lock").len(),
        2,
        "one replay after the stalled attempt"
    );
    assert!(
        !agent
            .messages()
            .iter()
            .any(|m| m.text_content().contains("visible half")),
        "the stalled attempt's partial text must not enter history"
    );
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

    let compacted: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Compacted {
                tokens_before,
                tokens_after,
                messages_before,
                messages_after,
                ..
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
async fn reasoning_only_maxtokens_is_not_retried_as_empty() {
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

#[tokio::test]
async fn background_summary_is_discarded_when_history_changes() {
    // 100k window - 16k reserve = 84k threshold; 6 x 13k tokens = 78k sits in
    // the 85%..100% band where a background summary starts.
    let mut agent = Agent::new(
        Box::new(FakeProvider::new(vec![])),
        std::sync::Arc::new(FakeExecutor::new(ToolOutput::ok("unused"))),
    )
    .with_context_window(Some(100_000));
    agent.set_background_compaction(true);
    for i in 0..6 {
        let body = "x".repeat(52_000);
        agent.messages.push(if i % 2 == 0 {
            Message::user(body)
        } else {
            Message::assistant(body)
        });
    }
    agent.maybe_start_background_compaction();
    let held = agent.held_summary.take().expect("summary should start");
    assert_eq!(
        held.discard_reason(&agent),
        None,
        "unchanged history keeps it"
    );

    agent.messages[0] = Message::user("rewritten");
    assert_eq!(held.discard_reason(&agent), Some("digest"));
}

struct LiveToolsExecutor {
    defs: Arc<Mutex<Vec<ToolDef>>>,
}

#[async_trait]
impl ToolExecutor for LiveToolsExecutor {
    fn execute(
        &self,
        _ctx: &ToolContext,
        _name: &str,
        _args: serde_json::Value,
    ) -> BoxFuture<'static, ToolOutput> {
        Box::pin(std::future::ready(ToolOutput::ok("unused")))
    }

    fn live_defs(&self) -> Option<Vec<ToolDef>> {
        Some(self.defs.lock().expect("defs lock").clone())
    }
}

fn tool(name: &str) -> ToolDef {
    ToolDef::new(name, "t", serde_json::json!({"type": "object"}))
}

fn names(defs: &[ToolDef]) -> Vec<&str> {
    defs.iter().map(|d| d.name.as_str()).collect()
}

/// An agent that has sent a request just now with a large prompt — the
/// state where a tool-list change would re-bill the whole cached prefix.
fn warm_big_agent() -> (Agent, Arc<Mutex<Vec<ToolDef>>>) {
    let defs = Arc::new(Mutex::new(vec![tool("bash")]));
    let mut agent = Agent::new(
        Box::new(FakeProvider::new(vec![])),
        Arc::new(LiveToolsExecutor { defs: defs.clone() }),
    );
    agent.tools = vec![tool("bash")];
    agent.messages.push(Message::user("hi"));
    agent.context_usage = Some((100_000, agent.messages.len()));
    agent.last_request_at = Some(std::time::Instant::now());
    (agent, defs)
}

#[test]
fn tool_change_before_first_request_is_adopted() {
    let (mut agent, defs) = warm_big_agent();
    agent.last_request_at = None;
    defs.lock().unwrap().push(tool("memory"));
    agent.refresh_tools();
    assert_eq!(names(&agent.tools), ["bash", "memory"]);
    assert!(agent.pending_tools.is_none());
}

#[test]
fn tool_change_on_warm_big_prompt_is_held() {
    let (mut agent, defs) = warm_big_agent();
    defs.lock().unwrap().push(tool("memory"));
    agent.refresh_tools();
    assert_eq!(names(&agent.tools), ["bash"], "sent list must not change");
    assert_eq!(
        names(agent.pending_tools.as_deref().unwrap()),
        ["bash", "memory"]
    );
}

#[test]
fn tool_change_on_small_prompt_is_adopted() {
    let (mut agent, defs) = warm_big_agent();
    agent.context_usage = Some((2_000, agent.messages.len()));
    defs.lock().unwrap().push(tool("memory"));
    agent.refresh_tools();
    assert_eq!(names(&agent.tools), ["bash", "memory"]);
}

#[test]
fn tool_change_after_cache_went_cold_is_adopted() {
    let (mut agent, defs) = warm_big_agent();
    agent.last_request_at = std::time::Instant::now().checked_sub(Duration::from_secs(2 * 60 * 60));
    defs.lock().unwrap().push(tool("memory"));
    agent.refresh_tools();
    assert_eq!(names(&agent.tools), ["bash", "memory"]);
}

#[test]
fn held_tool_change_lands_at_history_rewrite() {
    let (mut agent, defs) = warm_big_agent();
    defs.lock().unwrap().push(tool("memory"));
    agent.refresh_tools();
    assert_eq!(names(&agent.tools), ["bash"]);
    agent.history_rewritten();
    assert_eq!(names(&agent.tools), ["bash", "memory"]);
    assert!(agent.pending_tools.is_none());
}

#[test]
fn reverted_tool_change_clears_the_hold() {
    let (mut agent, defs) = warm_big_agent();
    defs.lock().unwrap().push(tool("memory"));
    agent.refresh_tools();
    defs.lock().unwrap().pop();
    agent.refresh_tools();
    assert!(agent.pending_tools.is_none());
    agent.history_rewritten();
    assert_eq!(names(&agent.tools), ["bash"]);
}
