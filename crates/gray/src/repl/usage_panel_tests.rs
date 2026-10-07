use super::*;
use crate::repl::status::SessionTotals;

/// Anthropic-ish cache-priced rate, seeded under this id like
/// `repl/tests.rs` seeds `test-persist-model`.
const MODEL: &str = "test-usage-panel-model";

fn seed_rate() {
    let v: serde_json::Value = serde_json::json!({
        MODEL: {
            "max_input_tokens": 1_000_000,
            "input_cost_per_token": 0.000005,
            "output_cost_per_token": 0.000025,
            "cache_read_input_token_cost": 0.0000005,
            "cache_creation_input_token_cost": 0.00000625,
        },
    });
    crate::setup::parse_litellm_context_json(&v);
}

/// Turn usage with an explicit cache breakdown (input stays inclusive).
fn cached_usage(input: usize, output: usize, read: usize, write: usize) -> gray_core::event::Usage {
    gray_core::event::Usage {
        input_tokens: input,
        output_tokens: output,
        non_cached_input_tokens: input - read - write,
        cache_read_input_tokens: read,
        cache_write_input_tokens: write,
        ..gray_core::event::Usage::default()
    }
}

fn input_for(totals: &SessionTotals, warmth: Warmth) -> PanelInput<'_> {
    PanelInput {
        totals,
        model: MODEL,
        rate: crate::setup::get_model_rate(MODEL),
        warmth,
    }
}

fn bar_cells(bar: &[Span<'static>]) -> usize {
    bar.iter().map(|s| s.content.chars().count()).sum()
}

#[test]
fn cache_bar_sums_to_24_and_nonzero_shares_keep_a_cell() {
    // Proportional shares.
    let bar = cache_bar(1_670_000, 112_000, 58_000);
    assert_eq!(bar_cells(&bar), 24);
    assert_eq!(bar.len(), 3);
    // A sliver still earns its cell.
    let bar = cache_bar(1, 1, 1_000);
    assert_eq!(bar.len(), 3);
    assert_eq!(bar_cells(&bar), 24);
    assert!(bar.iter().all(|s| !s.content.is_empty()));
    // Zero input → no bar at all.
    assert!(cache_bar(0, 0, 0).is_empty());
    // One share fills the whole bar.
    let bar = cache_bar(42, 0, 0);
    assert_eq!(bar.len(), 1);
    assert_eq!(bar[0].content.chars().count(), 24);
    assert_eq!(bar[0].content.as_ref(), "█".repeat(24));
}

#[test]
fn uncached_cost_prices_all_input_fresh() {
    seed_rate();
    let u = cached_usage(1_000, 100, 800, 100);
    let full = crate::setup::uncached_cost(&u, MODEL).expect("priced");
    let actual = crate::setup::turn_cost(&u, MODEL).expect("priced");
    // uncached: 1000×$5e-6 + 100×$25e-6; cached: fresh 100×$5e-6 +
    // read 800×$0.5e-6 + write 100×$6.25e-6 + output.
    let want_full = 1_000.0 * 5e-6 + 100.0 * 25e-6;
    assert!((full - want_full).abs() < 1e-12, "{full} vs {want_full}");
    assert!(full > actual, "cache must save: {full} vs {actual}");
    // Unpriced model → None.
    assert_eq!(
        crate::setup::uncached_cost(&u, "definitely-unpriced-model"),
        None
    );
}

#[test]
fn totals_accumulate_cache_shares_and_saved() {
    seed_rate();
    let mut t = SessionTotals::default();
    t.add(&cached_usage(1_000, 100, 800, 100), MODEL, Some(60_000));
    t.add(&cached_usage(2_000, 200, 1_500, 200), MODEL, None);
    assert_eq!(t.cache_read, 2_300);
    assert_eq!(t.cache_write, 300);
    assert!(t.cache_reported);
    let want = crate::setup::uncached_cost(&cached_usage(1_000, 100, 800, 100), MODEL).unwrap()
        - crate::setup::turn_cost(&cached_usage(1_000, 100, 800, 100), MODEL).unwrap()
        + crate::setup::uncached_cost(&cached_usage(2_000, 200, 1_500, 200), MODEL).unwrap()
        - crate::setup::turn_cost(&cached_usage(2_000, 200, 1_500, 200), MODEL).unwrap();
    assert!((t.saved - want).abs() < 1e-9, "{} vs {want}", t.saved);
    assert!(t.saved > 0.0);
    assert!(t.write_premium > 0.0);
    // Legacy `cached_tokens` counts as a read.
    let mut t2 = SessionTotals::default();
    let mut legacy = gray_core::event::Usage::new(500, 50);
    legacy.cached_tokens = 400;
    t2.add(&legacy, MODEL, None);
    assert_eq!(t2.cache_read, 400);
    assert!(t2.cache_reported);
}

#[test]
fn note_miss_buckets_by_cause() {
    let mut t = SessionTotals::default();
    let miss = |idle_secs: u64, model_changed: bool| crate::cache::CacheMiss {
        missed_tokens: 100_000,
        missed_cost: 0.5,
        idle: std::time::Duration::from_secs(idle_secs),
        model_changed,
    };
    t.note_miss(&miss(crate::cache::CACHE_TTL.as_secs() + 60, false));
    t.note_miss(&miss(10, true));
    t.note_miss(&miss(10, false));
    assert_eq!(t.misses.count, 3);
    assert_eq!(t.misses.tokens, 300_000);
    assert_eq!(t.misses.idle, 1);
    assert_eq!(t.misses.model_switch, 1);
    assert_eq!(t.misses.other, 1);
    // A model switch wins over a long idle gap.
    let mut t2 = SessionTotals::default();
    t2.note_miss(&miss(crate::cache::CACHE_TTL.as_secs() + 60, true));
    assert_eq!(t2.misses.model_switch, 1);
    assert_eq!(t2.misses.idle, 0);
}

#[test]
fn plain_render_shows_cache_rows_when_reported() {
    seed_rate();
    let mut t = SessionTotals::default();
    t.add(&cached_usage(1_000, 100, 800, 100), MODEL, Some(60_000));
    t.note_miss(&crate::cache::CacheMiss {
        missed_tokens: 50_000,
        missed_cost: 0.25,
        idle: crate::cache::CACHE_TTL + std::time::Duration::from_secs(1),
        model_changed: false,
    });
    let lines = usage_plain(&input_for(&t, Warmth::Unknown));
    let text = lines.join("\n");
    assert!(text.contains("Tokens"), "{text}");
    assert!(text.contains("1k in"), "{text}");
    assert!(text.contains("Cache"), "{text}");
    assert!(text.contains("80% hit"), "{text}");
    assert!(text.contains("800 read"), "{text}");
    assert!(text.contains("Saved"), "{text}");
    assert!(text.contains("Misses"), "{text}");
    assert!(text.contains("50k re-billed"), "{text}");
    assert!(text.contains("idle 1"), "{text}");
    assert!(text.contains("Time"), "{text}");
    assert!(text.contains("Rate"), "{text}");
    // Unknown warmth → no warm/cold suffix.
    assert!(!text.contains("warm"), "{text}");
    assert!(!text.contains("cold"), "{text}");
    // Rows are the concatenated spans — headless needs no ANSI.
    assert!(!text.contains('\x1b'), "{text}");
}

#[test]
fn plain_render_hides_cache_rows_without_cache_activity() {
    seed_rate();
    let mut t = SessionTotals::default();
    t.add(
        &gray_core::event::Usage::new(1_000, 100),
        MODEL,
        Some(60_000),
    );
    let lines = usage_plain(&input_for(&t, Warmth::Unknown));
    let text = lines.join("\n");
    assert!(text.contains("provider reports no cache usage"), "{text}");
    assert!(!text.contains("Saved"), "{text}");
    assert!(!text.contains("Misses"), "{text}");
    assert!(!text.contains("hit"), "{text}");
    // Rate + Time still render.
    assert!(text.contains("Rate"), "{text}");
    assert!(text.contains("cached"), "{text}");
}

#[test]
fn misses_none_shows_only_with_cache_activity() {
    seed_rate();
    let mut t = SessionTotals::default();
    t.add(&cached_usage(1_000, 100, 800, 100), MODEL, None);
    let text = usage_plain(&input_for(&t, Warmth::Unknown)).join("\n");
    assert!(text.contains("Misses   none"), "{text}");
}

#[test]
fn warmth_suffix_variants() {
    seed_rate();
    let mut t = SessionTotals::default();
    t.add(&cached_usage(1_000, 100, 800, 100), MODEL, None);
    let warm = usage_plain(&input_for(
        &t,
        Warmth::Warm(std::time::Duration::from_secs(180)),
    ))
    .join("\n");
    assert!(warm.contains("warm 3m left"), "{warm}");
    let cold = usage_plain(&input_for(&t, Warmth::Cold)).join("\n");
    assert!(cold.contains("cold"), "{cold}");
}

#[test]
fn negative_saved_flips_to_write_premium_note() {
    seed_rate();
    let mut t = SessionTotals::default();
    // All writes, no reads: the write premium outruns savings.
    t.add(&cached_usage(1_000, 100, 0, 900), MODEL, None);
    assert!(t.saved < 0.0, "saved should go negative: {}", t.saved);
    let text = usage_plain(&input_for(&t, Warmth::Unknown)).join("\n");
    assert!(text.contains("cache writes not yet paid back"), "{text}");
}
