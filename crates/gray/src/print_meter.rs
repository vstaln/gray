//! Accounting around the provider seam: includes compaction calls as well as tool rounds.
//! Stops subsequent requests at the cap. A request already sent can exceed its
//! remaining allowance; only provider-side limits can guarantee an invoice cap.
use futures::StreamExt;
use gray_core::{ChatRequest, Provider, ProviderError, ProviderStream, StreamEvent, Usage};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Snapshot {
    pub requests: u32,
    pub usage: Usage,
    pub cost_micros: Option<u64>,
    pub usage_complete: bool,
}

#[derive(Clone)]
pub struct Meter {
    state: Arc<Mutex<Snapshot>>,
    limit: u32,
    rates: Option<(f64, f64)>,
    budget: Option<u64>,
}

impl Meter {
    pub fn new(
        limit: u32,
        input: Option<f64>,
        output: Option<f64>,
        budget: Option<u64>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(limit > 0, "model request limit must be positive");
        let rates = match (input, output) {
            (Some(i), Some(o)) if i.is_finite() && o.is_finite() && i >= 0.0 && o >= 0.0 => {
                Some((i, o))
            }
            (None, None) if budget.is_none() => None,
            _ => {
                anyhow::bail!("budgeted execution requires finite nonnegative input/output prices")
            }
        };
        Ok(Self {
            state: Arc::new(Mutex::new(Snapshot {
                usage_complete: true,
                cost_micros: rates.map(|_| 0),
                ..Snapshot::default()
            })),
            limit,
            rates,
            budget,
        })
    }
    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().unwrap().clone()
    }
    pub fn wrap(&self, inner: Box<dyn Provider>) -> Box<dyn Provider> {
        Box::new(Metered {
            inner,
            meter: self.clone(),
        })
    }
}

struct Metered {
    inner: Box<dyn Provider>,
    meter: Meter,
}
impl Provider for Metered {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
    fn stream(&self, request: ChatRequest) -> ProviderStream {
        let mut state = self.meter.state.lock().unwrap();
        if state.requests >= self.meter.limit
            || (self.meter.budget.is_some() && !state.usage_complete)
            || self
                .meter
                .budget
                .is_some_and(|b| state.cost_micros.unwrap_or(u64::MAX) >= b)
        {
            return futures::stream::once(async {
                Err(ProviderError::BadRequest(
                    "request budget exhausted or previous usage unknown".into(),
                ))
            })
            .boxed();
        }
        let previous_complete = state.usage_complete;
        state.requests += 1;
        state.usage_complete = false;
        drop(state);
        let meter = self.meter.clone();
        self.inner
            .stream(request)
            .map(move |event| {
                if let Ok(StreamEvent::MessageComplete {
                    usage: Some(usage), ..
                }) = &event
                {
                    let mut state = meter.state.lock().unwrap();
                    state.usage.accumulate(usage);
                    // A missing/zero usage report is unknown, not a free request.
                    state.usage_complete =
                        previous_complete && (usage.input_tokens > 0 || usage.output_tokens > 0);
                    if let Some((input, output)) = meter.rates {
                        // USD per million tokens == micro-USD per token. Ignore cache
                        // discounts conservatively; rates must cover reasoning/cache writes.
                        let cost = (usage.input_tokens as f64 * input
                            + usage.output_tokens as f64 * output)
                            .ceil() as u64;
                        state.cost_micros =
                            Some(state.cost_micros.unwrap_or(0).saturating_add(cost));
                    }
                }
                event
            })
            .boxed()
    }
}
