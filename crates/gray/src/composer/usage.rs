use super::*;

impl Tui {
    pub fn set_usage(&mut self, usage: gray_core::event::Usage) {
        if !usage.any() {
            return;
        }
        self.latest_usage = Some(usage);
        self.cumulative_usage = Some(usage);

        self.turn_output_accum = self.turn_output_accum.saturating_add(usage.output_tokens);

        self.streamed_bytes = 0;
    }

    pub fn seed_estimate_usage(&mut self, tokens: usize) {
        if tokens == 0 {
            return;
        }
        let u = gray_core::event::Usage::estimated_context(tokens);
        self.latest_usage = Some(u);
        self.cumulative_usage = Some(u);
    }

    pub fn reset_usage(&mut self) {
        self.latest_usage = None;
        self.cumulative_usage = None;
        self.turn_billed_output = None;
        self.streamed_bytes = 0;
        self.turn_output_accum = 0;
        self.turn_stream_ms = 0;

        self.cache.reset();
        self.cache_stats.reset();
    }

    pub fn note_cache_request(
        &mut self,
        usage: &gray_core::event::Usage,
        model: &str,
        wire_model: &str,
    ) -> Option<crate::cache::CacheMiss> {
        let rate = crate::setup::get_model_rate(model);
        self.cache.note(usage, wire_model, rate, Instant::now())
    }

    pub(crate) fn note_prefix_change(&mut self, cause: String) {
        self.cache.note_prefix_change(cause);
    }

    pub fn cache_remaining(&self) -> Option<Duration> {
        self.cache.remaining(Instant::now())
    }

    pub fn cache_is_cold(&self) -> bool {
        self.cache.is_cold(Instant::now())
    }

    pub fn reset_cache(&mut self) {
        self.cache.reset();
    }

    pub(crate) fn resume_cache(&mut self) {
        self.cache.resume(Instant::now());
    }

    pub(crate) fn note_cache_reconnect(&mut self) {
        self.cache.note_reconnect();
    }

    pub(crate) fn rearm_cache(&mut self) {
        self.cache.rearm(Instant::now());
    }

    pub(crate) fn set_turn_stream_ms(&mut self, ms: u64) {
        self.turn_stream_ms = ms;
    }

    pub fn set_turn_billed(&mut self, output_tokens: usize) {
        self.turn_billed_output = Some(output_tokens);
    }
}
