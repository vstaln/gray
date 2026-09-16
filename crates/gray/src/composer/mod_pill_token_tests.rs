use super::{pill_context_tokens, pill_token_suffix};
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
        " · 12,500 tok"
    );
    // Resume/compaction seed: estimate with no breakdown.
    assert_eq!(
        pill_token_suffix(Some(Usage::estimated_context(39_000))),
        " · 39,000 tok"
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
    assert_eq!(pill_token_suffix(Some(u)), " · 102,000 tok");
}

#[test]
fn pill_falls_back_to_input_without_breakdown() {
    let u = Usage {
        input_tokens: 5_000,
        output_tokens: 300,
        ..Usage::default()
    };
    assert_eq!(pill_token_suffix(Some(u)), " · 5,300 tok");
}
