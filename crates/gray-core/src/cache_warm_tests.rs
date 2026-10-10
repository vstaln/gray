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
    let big = expected_savings(&sonnet(), 100_000, 1);
    assert!((big - (0.375 - 0.03 - 0.03 - 15e-6)).abs() < 1e-9, "{big}");
    assert!(big >= MIN_EXPECTED_SAVINGS);
    assert!(expected_savings(&sonnet(), 10_000, 1) < MIN_EXPECTED_SAVINGS);
    // No write price: a miss is billed as plain input.
    let plain = Prices {
        cache_write: 0.0,
        ..sonnet()
    };
    assert!((expected_savings(&plain, 100_000, 1) - (0.3 - 0.03 - 0.03 - 15e-6)).abs() < 1e-9);
    // A budget-preserving cap (Claude thinking) bills the refresh's
    // larger output against the saving: 2048 tokens still clears the
    // floor on a big prompt, 32k never does.
    let capped = expected_savings(&sonnet(), 100_000, 2048);
    assert!(
        (capped - (0.375 - 0.03 - 0.03 - 2048.0 * 15e-6)).abs() < 1e-9,
        "{capped}"
    );
    assert!(capped >= MIN_EXPECTED_SAVINGS);
    assert!(expected_savings(&sonnet(), 100_000, 32_000) < 0.0);
}

/// Serves scripted rounds; a `warm_cap`-capped request is a cache refresh.
struct Fake {
    scripts: Mutex<VecDeque<Vec<StreamEvent>>>,
    seen: Arc<Mutex<Vec<(Option<u32>, usize)>>>,
    warm_cap: u32,
}

impl Provider for Fake {
    fn warm_output_cap(&self, _req: &ChatRequest) -> u32 {
        self.warm_cap
    }

    fn stream(&self, req: ChatRequest) -> BoxStream<'static, Result<StreamEvent, ProviderError>> {
        self.seen
            .lock()
            .unwrap()
            .push((req.max_tokens, req.messages.len()));
        let events = if req.max_tokens == Some(self.warm_cap) {
            let mut u = Usage::new(100_000, self.warm_cap as usize);
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
    run_turn_reading(tool_time, prices, 0, 1).await
}

/// `cache_read`: what the first round's usage reports as read from cache;
/// `warm_cap`: the provider's `warm_output_cap` for the replay.
async fn run_turn_reading(
    tool_time: Duration,
    prices: Option<Prices>,
    cache_read: usize,
    warm_cap: u32,
) -> (Vec<(Option<u32>, usize)>, Usage) {
    let mut first = Usage::new(100_000, 20);
    first.cache_read_input_tokens = cache_read;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider = Fake {
        scripts: Mutex::new(VecDeque::from(vec![
            vec![
                StreamEvent::tool_call_delta(0, Some("c1".into()), Some("build".into()), "{}"),
                StreamEvent::message_complete(Some(StopReason::ToolUse), Some(first)),
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
        warm_cap,
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
async fn unknown_prices_never_warm_a_provider_without_cache_reports() {
    let (seen, _) = run_turn(Duration::from_secs(600), None).await;
    assert!(seen.iter().all(|(m, _)| m.is_none()), "{seen:?}");
}

#[tokio::test(start_paused = true)]
async fn unknown_prices_warm_a_big_prompt_the_provider_caches() {
    // A subscription model: no dollar prices, but the provider reported a
    // cache read, so a 100k-token miss is worth a refresh.
    let (seen, _) = run_turn_reading(Duration::from_secs(600), None, 90_000, 1).await;
    assert_eq!(
        seen.iter().filter(|(m, _)| *m == Some(1)).count(),
        2,
        "{seen:?}"
    );
}

#[test]
fn the_token_floor_backs_up_the_dollar_floor() {
    // Unpriced: size plus reported caching decides.
    assert!(worth_refreshing(None, 25_000, 1, true));
    assert!(!worth_refreshing(None, 25_000, 1, false));
    assert!(!worth_refreshing(None, 5_000, 1, true));
    // All-zero prices are no prices (a free tier).
    let free = Prices {
        input: 0.0,
        output: 0.0,
        cache_read: 0.0,
        cache_write: 0.0,
    };
    assert!(worth_refreshing(Some(&free), 25_000, 1, true));
    // A cheap model under the dollar floor still warms a big prompt...
    let cheap = Prices {
        input: 0.27e-6,
        output: 1.1e-6,
        cache_read: 0.07e-6,
        cache_write: 0.0,
    };
    assert!(expected_savings(&cheap, 25_000, 1) < MIN_EXPECTED_SAVINGS);
    assert!(worth_refreshing(Some(&cheap), 25_000, 1, false));
    assert!(!worth_refreshing(Some(&cheap), 10_000, 1, true));
    // ...but never when a refresh costs more than the miss it prevents.
    let flat = Prices {
        cache_read: 0.27e-6,
        ..cheap
    };
    assert!(!worth_refreshing(Some(&flat), 100_000, 1, true));
    assert!(!worth_refreshing(Some(&sonnet()), 0, 1, true));
}

#[tokio::test(start_paused = true)]
async fn a_thinking_replay_warms_at_the_providers_cap() {
    // Claude + thinking: the provider reports the budget-preserving cap
    // and the replay carries it — a 1-token cap would drop `thinking`
    // and key a different cache entry.
    let (seen, billed) = run_turn_reading(Duration::from_secs(600), Some(sonnet()), 0, 2048).await;
    let warms: Vec<_> = seen.iter().filter(|(m, _)| *m == Some(2048)).collect();
    assert_eq!(warms.len(), 2, "{seen:?}");
    assert_eq!(
        seen.last().unwrap().0,
        None,
        "the real follow-up is uncapped"
    );
    // Two rounds of 20 + 5 output, plus the capped refresh replies.
    assert_eq!(billed.output_tokens, 20 + 5 + 2 * 2048);
}

#[tokio::test(start_paused = true)]
async fn a_late_timer_skips_the_refresh() {
    // The real request went out 400s ago: the 270s refresh is past its
    // deadline (285s), so sending now would be a write, not a warm.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn Provider> = Arc::new(Fake {
        scripts: Mutex::new(VecDeque::new()),
        seen: seen.clone(),
        warm_cap: 1,
    });
    let sent = Instant::now();
    tokio::time::advance(Duration::from_secs(400)).await;
    keep_warm(
        provider,
        ChatRequest::default(),
        policy(Some(sonnet())),
        Arc::new(Mutex::new(WarmHint {
            prompt_tokens: 100_000,
            cache_reported: true,
        })),
        sent,
        Arc::new(Mutex::new(Usage::default())),
    )
    .await;
    assert!(seen.lock().unwrap().is_empty());
}
