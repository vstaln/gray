use super::{live_pill_suffix, pill_context_tokens};

fn pill_token_suffix(usage: Option<Usage>) -> String {
    live_pill_suffix(usage, 0)
}
use gray_core::event::Usage;

#[test]
fn pill_suffix_empty_before_first_report() {
    assert_eq!(pill_token_suffix(None), "");
    assert_eq!(pill_token_suffix(Some(Usage::new(0, 0))), "");
}

#[test]
fn pill_suffix_counts_last_report_only() {
    // Plain totals shape: context = input + output.
    assert_eq!(
        pill_token_suffix(Some(Usage::new(12_000, 500))),
        " · 12,500 tokens"
    );
    // Resume/compaction seed: estimate with no breakdown.
    assert_eq!(
        pill_token_suffix(Some(Usage::estimated_context(39_000))),
        " · 39,000 tokens"
    );
}

#[test]
fn pill_sums_non_overlapping_parts_opencode_parity() {
    // Anthropic-like report: inclusive input 100k, of which 90k cached
    // read + 1k cached write; output 2k includes 500 reasoning.
    let u = Usage {
        input_tokens: 100_000,
        output_tokens: 2_000,
        reasoning_tokens: 500,
        cached_tokens: 90_000,
        non_cached_input_tokens: 9_000,
        cache_read_input_tokens: 90_000,
        cache_write_input_tokens: 1_000,
        total_tokens: 0,
    };
    // 9k fresh + 2k out + 90k read + 1k write = full context, and it
    // must agree with total() (no double-counted reasoning/cache).
    assert_eq!(pill_context_tokens(&u), 102_000);
    assert_eq!(pill_context_tokens(&u), u.total());
    assert_eq!(pill_token_suffix(Some(u)), " · 102,000 tokens");
}

#[test]
fn live_pill_tracks_streamed_output_between_reports() {
    use super::live_pill_suffix;
    // No report yet: silent until the first chunk, then the bare estimate.
    assert_eq!(live_pill_suffix(None, 0), "");
    assert_eq!(live_pill_suffix(None, 4000), " · 1,000 tokens");
    // Report present: exact output wins until the stream passes it.
    let u = Some(Usage::new(12_000, 500));
    assert_eq!(live_pill_suffix(u.clone(), 0), " · 12,500 tokens");
    assert_eq!(live_pill_suffix(u.clone(), 400), " · 12,500 tokens");
    assert_eq!(live_pill_suffix(u, 4000), " · 13,000 tokens");
}

#[test]
fn live_tps_ticks_with_stream_and_hides_until_measurable() {
    use super::live_tps_suffix;
    // No output yet or no elapsed time: no rate (same contract as the
    // end-of-turn `turn_tokens_per_second`).
    assert_eq!(live_tps_suffix(0, 0, 1000), "");
    assert_eq!(live_tps_suffix(0, 4000, 0), "");
    // Streamed estimate only: 4000 bytes ~ 1000 out-tokens over 2s.
    assert_eq!(live_tps_suffix(0, 4000, 2000), " · 500 tps");
    // Completed rounds add in: 500 exact + 1000 streamed over 2s.
    assert_eq!(live_tps_suffix(500, 4000, 2000), " · 750 tps");
    // Matches the end-of-turn math at equal output/duration.
    let end = crate::repl::turn_tokens_per_second(1500, 2000).unwrap();
    assert_eq!(live_tps_suffix(500, 4000, 2000), format!(" · {end} tps"));
}

#[test]
fn live_turn_output_sums_rounds_plus_stream_estimate() {
    use super::live_turn_output_tokens;
    assert_eq!(live_turn_output_tokens(0, 0), 0);
    assert_eq!(live_turn_output_tokens(0, 4000), 1000);
    assert_eq!(live_turn_output_tokens(500, 4000), 1500);
}

#[test]
fn pill_falls_back_to_input_without_breakdown() {
    let u = Usage {
        input_tokens: 5_000,
        output_tokens: 300,
        ..Usage::default()
    };
    assert_eq!(pill_token_suffix(Some(u)), " · 5,300 tokens");
}
