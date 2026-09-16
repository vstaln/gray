use super::Usage;

#[test]
fn estimated_context_carries_estimate_as_total_and_input() {
    let u = Usage::estimated_context(39_000);
    assert_eq!(u.total(), 39_000);
    assert_eq!(u.input_tokens, 39_000);
    // breakdown unknown, not zero-cache: must stay zero so no
    // consumer mistakes the seed for a measured cache report.
    assert_eq!(u.cache_read_input_tokens, 0);
    assert_eq!(u.cache_write_input_tokens, 0);
    assert_eq!(u.output_tokens, 0);
    assert_eq!(u.cache_hit_rate(), 0.0);
}
