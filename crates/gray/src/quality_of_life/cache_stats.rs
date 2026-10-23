use gray_core::event::Usage;
use std::time::{Duration, Instant};

const NOISE_FLOOR_TOKENS: usize = 1024;
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
pub struct CacheStats {
    prev_prompt_tokens: usize,
    prev_model: String,
    prev_at: Option<Instant>,
    reported_cache: bool,
}

impl CacheStats {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn observe(
        &mut self,
        usage: &Usage,
        model: &str,
        session: Option<&str>,
        prices: Option<crate::setup::ModelRate>,
    ) {
        let mut u = *usage;
        u.normalize();
        let prompt = u.input_tokens;
        let read = u.cache_read_input_tokens.max(u.cached_tokens);
        let now = Instant::now();
        if prompt == 0 {
            return;
        }
        if self.prev_prompt_tokens > 0
            && (read + u.cache_write_input_tokens > 0 || self.reported_cache)
        {
            let missed = self.prev_prompt_tokens.min(prompt).saturating_sub(read);
            if missed > NOISE_FLOOR_TOKENS {
                let idle_ms = self
                    .prev_at
                    .map(|t| now.saturating_duration_since(t).as_millis())
                    .unwrap_or(0);
                let model_changed = !self.prev_model.is_empty() && self.prev_model != model;
                let missed_cost = match prices {
                    Some(p) => {
                        let paid = u.non_cached_input_tokens + u.cache_write_input_tokens;
                        let paid_rate = if paid > 0 {
                            (u.non_cached_input_tokens as f64 * p.input
                                + u.cache_write_input_tokens as f64 * p.cache_write)
                                / paid as f64
                        } else {
                            0.0
                        };
                        missed as f64 * (paid_rate - p.cache_read).max(0.0)
                    }
                    None => 0.0,
                };
                log::info!(
                    target: "gray_agent",
                    "cache miss: session={} missed_tokens={missed} missed_cost=${missed_cost:.4} idle_ms={idle_ms} ttl_expiry={} model_changed={model_changed}",
                    session.filter(|s| !s.is_empty()).unwrap_or("-"),
                    idle_ms >= CACHE_TTL.as_millis(),
                );
            }
        }
        self.prev_prompt_tokens = prompt;
        self.prev_model = model.to_string();
        self.prev_at = Some(now);
        self.reported_cache |= read + u.cache_write_input_tokens > 0;
    }
}
