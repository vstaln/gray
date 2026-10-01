use super::*;
use crate::agent::{Agent, ProviderError, ToolContext, ToolExecutor, ToolOutput};
use crate::event::{AgentEvent, StopReason};
use crate::message::{Message, ToolDef};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use std::collections::VecDeque;

/// Claude Sonnet list prices, per token.
fn sonnet() -> Prices {
    Prices {
        input: 3e-6,
        output: 15e-6,
        cache_read: 0.3e-6,
        cache_write: 3.75e-6,
    }
}

fn policy(prices: Option<Prices>) -> CacheWarmPolicy {
    CacheWarmPolicy {
        ttl: Duration::from_secs(300),
        prices: Arc::new(move || prices),
    }
}

#[test]
fn refresh_at_ninety_percent_with_ten_seconds_of_margin() {
    assert_eq!(
        warming_delay(Duration::from_secs(300)),
        Some(Duration::from_secs(270))
    );
    assert_eq!(
        warming_delay(Duration::from_secs(20)),
        Some(Duration::from_secs(10))
    );
    assert_eq!(warming_delay(Duration::from_secs(10)), None);
}

#[test]
fn savings_follow_pi_and_clear_the_floor_only_for_big_prompts() {
    // 100k tokens: a miss costs a $0.375 write vs a $0.03 read.
    let big = expected_savings(&sonnet(), 100_000);
    assert!((big - (0.375 - 0.03 - 0.03 - 15e-6)).abs() < 1e-9, "{big}");
    assert!(big >= MIN_EXPECTED_SAVINGS);
    assert!(expected_savings(&sonnet(), 10_000) < MIN_EXPECTED_SAVINGS);
    // No write price: a miss is billed as plain input.
    let plain = Prices {
        cache_write: 0.0,
        ..sonnet()
    };
    assert!((expected_savings(&plain, 100_000) - (0.3 - 0.03 - 0.03 - 15e-6)).abs() < 1e-9);
}

/// Serves scripted rounds; a one-token request is a cache refresh.
struct Fake {
    scripts: Mutex<VecDeque<Vec<StreamEvent>>>,
    seen: Arc<Mutex<Vec<(Option<u32>, usize)>>>,
}

impl Provider for Fake {
    fn stream(&self, req: ChatRequest) -> BoxStream<'static, Result<StreamEvent, ProviderError>> {
        self.seen
            .lock()
            .unwrap()
            .push((req.max_tokens, req.messages.len()));
        let events = if req.max_tokens == Some(1) {
            let mut u = Usage::new(100_000, 1);
            u.cache_read_input_tokens = 100_000;
            vec![StreamEvent::message_complete(
                Some(StopReason::MaxTokens),
                Some(u),
            )]
        } else {
            self.scripts.lock().unwrap().pop_front().unwrap_or_default()
        };
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }
}

struct Slow(Duration);

impl ToolExecutor for Slow {
    fn execute(
        &self,
        _ctx: &ToolContext,
        _name: &str,
        _args: serde_json::Value,
    ) -> BoxFuture<'static, ToolOutput> {
        let d = self.0;
        Box::pin(async move {
            tokio::time::sleep(d).await;
            ToolOutput::ok("built")
        })
    }
}

async fn run_turn(
    tool_time: Duration,
    prices: Option<Prices>,
) -> (Vec<(Option<u32>, usize)>, Usage) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider = Fake {
        scripts: Mutex::new(VecDeque::from(vec![
            vec![
                StreamEvent::tool_call_delta(0, Some("c1".into()), Some("build".into()), "{}"),
                StreamEvent::message_complete(
                    Some(StopReason::ToolUse),
                    Some(Usage::new(100_000, 20)),
                ),
            ],
            vec![
                StreamEvent::text_delta("done"),
                StreamEvent::message_complete(
                    Some(StopReason::EndTurn),
                    Some(Usage::new(100_100, 5)),
                ),
            ],
        ])),
        seen: seen.clone(),
    };
    let mut agent = Agent::new(Box::new(provider), Arc::new(Slow(tool_time)))
        .with_tools(vec![ToolDef::new("build", "b", serde_json::json!({}))])
        .with_tool_timeout(Duration::from_secs(3600))
        .with_cache_warm(Some(policy(prices)));
    let events = agent
        .run(Message::user("go"), ToolContext::default())
        .await
        .unwrap();
    let billed = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::TurnEnd { usage, .. } => Some(*usage),
            _ => None,
        })
        .unwrap();
    let seen = seen.lock().unwrap().clone();
    (seen, billed)
}

#[tokio::test(start_paused = true)]
async fn a_long_tool_run_is_refreshed_and_billed() {
    // 10 minutes of tool time: refreshes at 270s and 540s, then the next
    // real request goes out on a warm cache.
    let (seen, billed) = run_turn(Duration::from_secs(600), Some(sonnet())).await;
    let warms: Vec<_> = seen.iter().filter(|(m, _)| *m == Some(1)).collect();
    assert_eq!(warms.len(), 2, "{seen:?}");
    // The refresh replays the first round's request exactly (1 message).
    assert!(warms.iter().all(|(_, n)| *n == 1), "{seen:?}");
    assert_eq!(
        seen.last().unwrap().0,
        None,
        "the real follow-up is uncapped"
    );
    // Two rounds of 20 + 5 output, plus one token per refresh.
    assert_eq!(billed.output_tokens, 27);
}

#[tokio::test(start_paused = true)]
async fn a_short_tool_run_sends_nothing_extra() {
    let (seen, _) = run_turn(Duration::from_secs(60), Some(sonnet())).await;
    assert!(seen.iter().all(|(m, _)| m.is_none()), "{seen:?}");
}

#[tokio::test(start_paused = true)]
async fn unknown_prices_never_warm() {
    let (seen, _) = run_turn(Duration::from_secs(600), None).await;
    assert!(seen.iter().all(|(m, _)| m.is_none()), "{seen:?}");
}

#[tokio::test(start_paused = true)]
async fn a_late_timer_skips_the_refresh() {
    // The real request went out 400s ago: the 270s refresh is past its
    // deadline (285s), so sending now would be a write, not a warm.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn Provider> = Arc::new(Fake {
        scripts: Mutex::new(VecDeque::new()),
        seen: seen.clone(),
    });
    let sent = Instant::now();
    tokio::time::advance(Duration::from_secs(400)).await;
    keep_warm(
        provider,
        ChatRequest::default(),
        policy(Some(sonnet())),
        100_000,
        sent,
        Arc::new(Mutex::new(Usage::default())),
    )
    .await;
    assert!(seen.lock().unwrap().is_empty());
}
