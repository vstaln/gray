use super::{live_context_total, live_pill_suffix, live_turn_output_tokens, pill_context_tokens};
use gray_core::event::Usage;

#[test]
fn pill_suffix_empty_before_first_output() {
    // Top pill is output-only: silent until the first streamed byte or the
    // first completed round, even when a stale context report is in force.
    assert_eq!(live_pill_suffix(0, 0), "");
    assert_eq!(live_context_total(None, 0), 0);
}

#[test]
fn context_counts_last_report_only() {
    // Plain totals shape: context = input + output.
    assert_eq!(live_context_total(Some(Usage::new(12_000, 500)), 0), 12_500);
    // Resume/compaction seed: estimate with no breakdown.
    assert_eq!(
        live_context_total(Some(Usage::estimated_context(39_000)), 0),
        39_000
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
    assert_eq!(live_context_total(Some(u), 0), 102_000);
}

#[test]
fn live_pill_tracks_streamed_output_between_reports() {
    // No completed round yet: silent until the first chunk, then the bare
    // output estimate.
    assert_eq!(live_pill_suffix(0, 0), "");
    assert_eq!(live_pill_suffix(0, 4000), " · 1,000 tokens");
    // Completed round + stream: exact output plus the estimate for the
    // round in flight, so the pill ticks immediately per chunk.
    assert_eq!(live_pill_suffix(500, 0), " · 500 tokens");
    assert_eq!(live_pill_suffix(500, 400), " · 600 tokens");
    assert_eq!(live_pill_suffix(500, 4000), " · 1,500 tokens");
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
    assert_eq!(live_turn_output_tokens(0, 0), 0);
    assert_eq!(live_turn_output_tokens(0, 4000), 1000);
    assert_eq!(live_turn_output_tokens(500, 4000), 1500);
}

#[test]
fn context_falls_back_to_input_without_breakdown() {
    let u = Usage {
        input_tokens: 5_000,
        output_tokens: 300,
        ..Usage::default()
    };
    assert_eq!(live_context_total(Some(u), 0), 5_300);
}

#[test]
fn live_pill_ticks_immediately_without_freezing_on_previous_output() {
    // Cross-turn freeze repro (muse-spark-1.3, no CoT): previous turn billed
    // 14k output, so the stale context report is 20k in + 14,129 out. The
    // new turn streamed 20k bytes (~5k tokens) with no report yet: the top
    // pill must tick from zero instead of sitting frozen on the stale
    // output, and the bottom gauge must tick past the stale context total.
    let prev = Usage::new(20_000, 14_129);
    assert_eq!(live_pill_suffix(0, 20_000), " · 5,000 tokens");
    assert_eq!(live_context_total(Some(prev), 20_000), 39_129);
    // Within-round: 500 out finalized, ~100 tokens streamed since — the
    // pill must already include them.
    assert_eq!(live_pill_suffix(500, 400), " · 600 tokens");
}

#[test]
fn live_top_and_bottom_share_delta_not_base() {
    // Same streamed delta (+100 from 400 bytes), different bases: the top
    // pill ticks output, the bottom gauge ticks context.
    let u = Usage::new(12_000, 500);
    assert_eq!(live_context_total(Some(u), 0), 12_500);
    assert_eq!(live_pill_suffix(500, 0), " · 500 tokens");
    assert_eq!(live_context_total(Some(u), 400), 12_600);
    assert_eq!(live_pill_suffix(500, 400), " · 600 tokens");
    assert_eq!(live_context_total(Some(u), 4000), 13_500);
    assert_eq!(live_pill_suffix(500, 4000), " · 1,500 tokens");
    // User session shape: in=182,311, out=41,489, ctx=223,800; +400 bytes
    // (~100 tok) moves both by +100 from their own bases.
    let session = Usage::new(182_311, 41_489);
    assert_eq!(live_context_total(Some(session), 0), 223_800);
    assert_eq!(live_pill_suffix(41_489, 0), " · 41,489 tokens");
    assert_eq!(live_context_total(Some(session), 400), 223_900);
    assert_eq!(live_pill_suffix(41_489, 400), " · 41,589 tokens");
}
