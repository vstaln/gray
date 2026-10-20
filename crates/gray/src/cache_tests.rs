use super::*;

fn priced() -> ModelRate {
    ModelRate {
        tiers: Vec::new(),
        input: 3.0e-6,
        output: 15.0e-6,
        cache_read: 0.3e-6,
        cache_write: 3.75e-6,
        has_cache_prices: true,
    }
}

fn usage(input: usize, output: usize, read: usize, write: usize) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: output,
        non_cached_input_tokens: input - read - write,
        cache_read_input_tokens: read,
        cache_write_input_tokens: write,
        ..Usage::default()
    }
}

fn opaque(input: usize, output: usize) -> Usage {
    Usage::new(input, output)
}

fn t(base: Instant, offset_secs: u64) -> Instant {
    base + Duration::from_secs(offset_secs)
}

#[test]
fn first_request_is_never_a_miss_and_warms_the_cache() {
    let base = Instant::now();
    let mut tr = CacheTracker::default();
    let now = t(base, 0);
    assert_eq!(
        tr.note(
            &usage(100_000, 500, 90_000, 10_000),
            "m",
            Some(priced()),
            now
        ),
        None
    );
    let left = tr.remaining(t(base, 60)).expect("warm");
    assert_eq!(left, cache_ttl() - Duration::from_secs(60));
}

#[test]
fn cold_is_the_ttl_crossed_on_a_reported_cache_only() {
    let base = Instant::now();
    let mut tr = CacheTracker::default();

    assert!(!tr.is_cold(t(base, cache_ttl().as_secs() * 10)));
    tr.note(&usage(100_000, 500, 90_000, 10_000), "m", None, t(base, 0));
    assert!(!tr.is_cold(t(base, cache_ttl().as_secs() - 1)));
    assert!(tr.is_cold(t(base, cache_ttl().as_secs())));

    let mut opaque_tr = CacheTracker::default();
    opaque_tr.note(&opaque(100_000, 500), "m", None, t(base, 0));
    assert_eq!(opaque_tr.remaining(t(base, 1)), None);
    assert!(!opaque_tr.is_cold(t(base, cache_ttl().as_secs())));
}

#[test]
fn a_paused_turn_answers_cold_on_the_idle_gap_it_started_after() {
    let base = Instant::now();
    let mut tr = CacheTracker::default();
    tr.note(&usage(100_000, 500, 90_000, 10_000), "m", None, t(base, 0));

    tr.pause(t(base, cache_ttl().as_secs() + 30));
    assert!(tr.is_cold(t(base, cache_ttl().as_secs() + 30)));
    assert!(tr.is_cold(t(base, cache_ttl().as_secs() + 3_600)));

    tr.rearm(t(base, 30));
    tr.pause(t(base, 60));
    assert!(!tr.is_cold(t(base, 60)));
    assert!(!tr.is_cold(t(base, 60 + 3_600)));
}

#[test]
fn warmth_expires_at_the_ttl() {
    let base = Instant::now();
    let mut tr = CacheTracker::default();
    tr.note(&usage(100_000, 500, 90_000, 10_000), "m", None, t(base, 0));
    assert!(tr.remaining(t(base, cache_ttl().as_secs() - 1)).is_some());
    assert_eq!(tr.remaining(t(base, cache_ttl().as_secs())), None);
    assert_eq!(tr.remaining(t(base, cache_ttl().as_secs() * 3)), None);
}

#[test]
fn active_turn_freezes_then_rearms_the_countdown() {
    let base = Instant::now();
    let mut tr = CacheTracker::default();
    tr.note(&usage(100_000, 500, 90_000, 10_000), "m", None, t(base, 0));

    tr.pause(t(base, 60));

    tr.pause(t(base, 120));
    assert_eq!(
        tr.remaining(t(base, 360)),
        Some(cache_ttl() - Duration::from_secs(60))
    );

    tr.rearm(t(base, 360));
    assert_eq!(tr.remaining(t(base, 360)), Some(cache_ttl()));
    assert_eq!(
        tr.remaining(t(base, 361)),
        Some(cache_ttl() - Duration::from_secs(1))
    );
}

#[test]
fn aborted_turn_releases_the_pause_without_rearming() {
    let base = Instant::now();
    let mut tr = CacheTracker::default();
    tr.note(&usage(100_000, 500, 90_000, 10_000), "m", None, t(base, 0));
    tr.pause(t(base, 60));

    tr.resume(t(base, 360));
    assert_eq!(
        tr.remaining(t(base, 361)),
        Some(cache_ttl() - Duration::from_secs(61))
    );
}

#[test]
fn usage_during_an_active_turn_rearms_and_freezes_a_fresh_timer() {
    let base = Instant::now();
    let mut tr = CacheTracker::default();
    tr.note(&usage(100_000, 500, 90_000, 10_000), "m", None, t(base, 0));
    tr.pause(t(base, 10));

    tr.note(
        &usage(110_000, 500, 100_000, 10_000),
        "m",
        None,
        t(base, 20),
    );
    assert_eq!(tr.remaining(t(base, 80)), Some(cache_ttl()));

    tr.rearm(t(base, 80));
    assert_eq!(tr.remaining(t(base, 80)), Some(cache_ttl()));
    assert_eq!(
        tr.remaining(t(base, 81)),
        Some(cache_ttl() - Duration::from_secs(1))
    );
}

#[test]
fn active_turn_time_does_not_age_the_next_cache_miss() {
    let base = Instant::now();
    let mut tr = CacheTracker::default();
    tr.note(
        &usage(100_000, 100, 90_000, 10_000),
        "m",
        Some(priced()),
        t(base, 0),
    );
    tr.pause(t(base, 60));

    let miss = tr
        .note(
            &usage(100_000, 100, 0, 100_000),
            "m",
            Some(priced()),
            t(base, 360),
        )
        .expect("total miss");
    assert_eq!(miss.idle, Duration::from_secs(60));
    assert!(
        miss.notice()
            .is_some_and(|notice| notice.starts_with("Cache miss:")),
        "active generation time must not be labelled as idle"
    );
}

#[test]
fn reset_during_an_active_turn_keeps_the_next_timer_paused() {
    let base = Instant::now();
    let mut tr = CacheTracker::default();
    tr.pause(t(base, 10));
    tr.reset();

    tr.note(&usage(60_000, 100, 50_000, 10_000), "m", None, t(base, 20));
    assert_eq!(tr.remaining(t(base, 80)), Some(cache_ttl()));

    tr.rearm(t(base, 80));
    assert_eq!(tr.remaining(t(base, 80)), Some(cache_ttl()));
    assert_eq!(
        tr.remaining(t(base, 81)),
        Some(cache_ttl() - Duration::from_secs(1))
    );
}

#[test]
fn provider_without_cache_reporting_never_warns_or_times() {
    let base = Instant::now();
    let mut tr = CacheTracker::default();
    assert_eq!(
        tr.note(&opaque(50_000, 100), "m", Some(priced()), t(base, 0)),
        None
    );
    assert_eq!(tr.remaining(t(base, 10)), None);

    assert_eq!(
        tr.note(&opaque(52_000, 100), "m", Some(priced()), t(base, 30)),
        None
    );
}

#[test]
fn growing_prompt_fully_cached_is_not_a_miss() {
    let base = Instant::now();
    let mut tr = CacheTracker::default();
    tr.note(
        &usage(50_000, 100, 50_000, 0),
        "m",
        Some(priced()),
        t(base, 0),
    );

    assert_eq!(
        tr.note(
            &usage(60_000, 100, 50_000, 10_000),
            "m",
            Some(priced()),
            t(base, 10)
        ),
        None
    );
}

#[test]
fn miss_below_the_noise_floor_is_ignored() {
    let base = Instant::now();
    let mut tr = CacheTracker::default();
    tr.note(
        &usage(50_000, 100, 49_000, 1_000),
        "m",
        Some(priced()),
        t(base, 0),
    );

    assert_eq!(
        tr.note(
            &usage(50_000, 100, 49_500, 500),
            "m",
            Some(priced()),
            t(base, 10)
        ),
        None
    );
}

#[test]
fn idle_past_the_ttl_is_a_total_miss_with_the_idle_label() {
    let base = Instant::now();
    let mut tr = CacheTracker::default();
    tr.note(
        &usage(100_000, 100, 90_000, 10_000),
        "m",
        Some(priced()),
        t(base, 0),
    );

    let miss = tr
        .note(
            &usage(100_000, 100, 0, 100_000),
            "m",
            Some(priced()),
            t(base, 360),
        )
        .expect("total miss after idle");
    assert_eq!(miss.missed_tokens, 100_000);
    assert!(
        (miss.missed_cost - 0.345).abs() < 1e-9,
        "cost: {}",
        miss.missed_cost
    );
    assert_eq!(miss.idle, Duration::from_secs(360));
    assert!(!miss.model_changed);
    assert_eq!(
        miss.notice().as_deref(),
        Some("Cache miss after 6m idle: 100k tokens re-billed (~$0.345)")
    );
}

#[test]
fn model_switch_relabels_the_miss() {
    let base = Instant::now();
    let mut tr = CacheTracker::default();
    tr.note(
        &usage(100_000, 100, 90_000, 10_000),
        "claude-a",
        Some(priced()),
        t(base, 0),
    );
    let miss = tr
        .note(
            &usage(100_000, 100, 0, 100_000),
            "claude-b",
            Some(priced()),
            t(base, 5),
        )
        .expect("miss after switch");
    assert!(miss.model_changed);
    assert_eq!(
        miss.notice().as_deref(),
        Some("Cache miss after model switch: 100k tokens re-billed (~$0.345)")
    );
}

#[test]
fn reconnect_relabels_the_miss_and_consumes_the_flag() {
    let base = Instant::now();
    let mut tr = CacheTracker::default();
    tr.note(
        &usage(100_000, 100, 90_000, 10_000),
        "m",
        Some(priced()),
        t(base, 0),
    );
    tr.note_reconnect();
    let miss = tr
        .note(
            &usage(100_000, 100, 0, 100_000),
            "m",
            Some(priced()),
            t(base, 5),
        )
        .expect("miss after reconnect");
    assert!(miss.reconnected);
    assert!(!miss.model_changed);
    assert_eq!(
        miss.notice().as_deref(),
        Some("Cache miss after reconnect: 100k tokens re-billed (~$0.345)")
    );

    let miss = tr
        .note(
            &usage(110_000, 100, 0, 110_000),
            "m",
            Some(priced()),
            t(base, 6),
        )
        .expect("second miss");
    assert!(!miss.reconnected);
    assert_eq!(
        miss.notice().as_deref(),
        Some("Cache miss: 100k tokens re-billed (~$0.345)")
    );
}

#[test]
fn read_only_provider_total_miss_counts() {
    let base = Instant::now();
    let mut tr = CacheTracker::default();
    tr.note(&usage(50_000, 100, 40_000, 0), "gpt", None, t(base, 0));
    let miss = tr
        .note(&usage(52_000, 100, 0, 0), "gpt", None, t(base, 20))
        .expect("total miss");
    assert_eq!(miss.missed_tokens, 50_000);
    assert_eq!(miss.missed_cost, 0.0, "unpriced: no cost claim");
    assert_eq!(
        miss.notice().as_deref(),
        Some("Cache miss: 50k tokens re-billed")
    );
}

#[test]
fn small_miss_stays_below_the_warning_bar() {
    let base = Instant::now();
    let mut tr = CacheTracker::default();
    tr.note(
        &usage(60_000, 100, 50_000, 10_000),
        "m",
        Some(priced()),
        t(base, 0),
    );

    let miss = tr
        .note(
            &usage(60_000, 100, 55_000, 5_000),
            "m",
            Some(priced()),
            t(base, 10),
        )
        .expect("counted miss");
    assert_eq!(miss.missed_tokens, 5_000);
    assert!(miss.missed_cost < WARN_MISS_COST);
    assert_eq!(miss.notice(), None);
}

#[test]
fn expensive_small_miss_still_warns() {
    let base = Instant::now();
    let mut tr = CacheTracker::default();
    tr.note(
        &usage(30_000, 100, 25_000, 5_000),
        "m",
        Some(priced()),
        t(base, 0),
    );

    let premium = ModelRate {
        input: 15.0e-6,
        cache_read: 1.5e-6,
        ..priced()
    };
    let miss = tr
        .note(
            &usage(30_000, 100, 18_000, 2_000),
            "m",
            Some(premium),
            t(base, 10),
        )
        .expect("counted miss");
    assert_eq!(miss.missed_tokens, 12_000);
    assert!(
        miss.missed_cost >= WARN_MISS_COST,
        "cost: {}",
        miss.missed_cost
    );
    let notice = miss.notice().expect("warns on cost alone");
    assert!(
        notice.starts_with("Cache miss: 12k tokens re-billed"),
        "{notice}"
    );
}

#[test]
fn reemitted_report_is_not_a_second_request() {
    let base = Instant::now();
    let mut tr = CacheTracker::default();
    let first = usage(100_000, 100, 40_000, 60_000);
    tr.note(&first, "m", Some(priced()), t(base, 0));

    assert_eq!(tr.note(&first, "m", Some(priced()), t(base, 1)), None);

    let miss = tr
        .note(
            &usage(100_000, 100, 40_000, 55_000),
            "m",
            Some(priced()),
            t(base, 2),
        )
        .expect("miss vs the first request");
    assert_eq!(miss.missed_tokens, 60_000);
}

#[test]
fn compaction_resets_the_baseline() {
    let base = Instant::now();
    let mut tr = CacheTracker::default();
    tr.note(
        &usage(200_000, 100, 150_000, 50_000),
        "m",
        Some(priced()),
        t(base, 0),
    );
    tr.reset();
    assert_eq!(tr.remaining(t(base, 1)), None);

    assert_eq!(
        tr.note(
            &usage(60_000, 100, 10_000, 50_000),
            "m",
            Some(priced()),
            t(base, 10)
        ),
        None
    );

    assert!(tr.remaining(t(base, 11)).is_some());
}

#[test]
fn format_remaining_switches_to_seconds_inside_the_last_minute() {
    assert_eq!(format_remaining(Duration::from_secs(299)), "4m");
    assert_eq!(format_remaining(Duration::from_secs(60)), "1m");
    assert_eq!(format_remaining(Duration::from_secs(59)), "59s");
    assert_eq!(format_remaining(Duration::from_secs(1)), "1s");
}

#[test]
fn unpriced_model_reports_tokens_without_a_cost() {
    let base = Instant::now();
    let mut tr = CacheTracker::default();
    tr.note(
        &usage(100_000, 100, 90_000, 10_000),
        "local-model",
        None,
        t(base, 0),
    );
    let miss = tr
        .note(
            &usage(100_000, 100, 0, 100_000),
            "local-model",
            None,
            t(base, 10),
        )
        .expect("total miss");
    assert_eq!(miss.missed_tokens, 100_000);
    assert_eq!(miss.missed_cost, 0.0);
    assert_eq!(
        miss.notice().as_deref(),
        Some("Cache miss: 100k tokens re-billed")
    );
}
