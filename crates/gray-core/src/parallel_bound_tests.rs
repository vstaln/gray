//! Concurrency-limit tests for `join_ordered`. The lane is unlimited by
//! default; `GRAY_PARALLEL_MAX` caps it. Env is process-global: `LOCK`
//! serializes the tests in this file.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures::future::BoxFuture;

use crate::agent::ToolOutput;
use crate::parallel::join_ordered;

static LOCK: Mutex<()> = Mutex::new(());

#[test]
fn cap_parsing_is_a_pure_table() {
    // No env access, no unsafe, no lock: the parser takes the raw value.
    let max = tokio::sync::Semaphore::MAX_PERMITS;
    for (raw, want) in [
        (None, max),
        (Some("4"), 4),
        (Some(" 8 "), 8),
        (Some("0"), max),
        (Some("abc"), max),
        (Some(""), max),
        (Some("  "), max),
        // Parses but clamps to what tokio accepts.
        (Some("18446744073709551615"), max),
        // Overflow falls back to unlimited rather than panicking.
        (Some("99999999999999999999999"), max),
    ] {
        assert_eq!(
            crate::parallel::parse_parallel_max(raw),
            want,
            "input {raw:?}"
        );
    }
}

#[tokio::test]
async fn no_cap_starts_every_call_at_once() {
    let _g = LOCK.lock().unwrap();
    unsafe { std::env::remove_var("GRAY_PARALLEL_MAX") };
    let current = std::sync::Arc::new(AtomicUsize::new(0));
    let observed_max = std::sync::Arc::new(AtomicUsize::new(0));
    let futs: Vec<(usize, BoxFuture<'static, ToolOutput>)> = (0..12)
        .map(|i| {
            let current = current.clone();
            let observed_max = observed_max.clone();
            (
                i,
                Box::pin(async move {
                    let n = current.fetch_add(1, Ordering::SeqCst) + 1;
                    observed_max.fetch_max(n, Ordering::SeqCst);
                    // Wait (bounded) for all twelve to be in flight.
                    for _ in 0..200 {
                        if current.load(Ordering::SeqCst) >= 12 {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    current.fetch_sub(1, Ordering::SeqCst);
                    ToolOutput::ok(format!("out{i}"))
                }) as BoxFuture<'static, ToolOutput>,
            )
        })
        .collect();
    let cancel = tokio_util::sync::CancellationToken::new();
    let got = join_ordered(futs, &cancel).await;
    assert_eq!(got.len(), 12);
    assert!(got.iter().all(|(_, o)| o.is_some()));
    assert_eq!(
        observed_max.load(Ordering::SeqCst),
        12,
        "with no cap every call starts at once"
    );
}

#[tokio::test]
async fn concurrency_never_exceeds_cap() {
    let _g = LOCK.lock().unwrap();
    unsafe { std::env::set_var("GRAY_PARALLEL_MAX", "4") };
    let current = std::sync::Arc::new(AtomicUsize::new(0));
    let observed_max = std::sync::Arc::new(AtomicUsize::new(0));
    let futs: Vec<(usize, BoxFuture<'static, ToolOutput>)> = (0..12)
        .map(|i| {
            let current = current.clone();
            let observed_max = observed_max.clone();
            (
                i,
                Box::pin(async move {
                    let n = current.fetch_add(1, Ordering::SeqCst) + 1;
                    observed_max.fetch_max(n, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    current.fetch_sub(1, Ordering::SeqCst);
                    ToolOutput::ok(format!("out{i}"))
                }) as BoxFuture<'static, ToolOutput>,
            )
        })
        .collect();
    let cancel = tokio_util::sync::CancellationToken::new();
    let got = join_ordered(futs, &cancel).await;
    unsafe { std::env::remove_var("GRAY_PARALLEL_MAX") };
    assert_eq!(got.len(), 12);
    assert_eq!(
        got.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
        (0..12).collect::<Vec<_>>(),
        "results stay in input-index order"
    );
    assert!(got.iter().all(|(_, o)| o.is_some()), "all tasks complete");
    assert!(
        observed_max.load(Ordering::SeqCst) <= 4,
        "max in flight was {}",
        observed_max.load(Ordering::SeqCst)
    );
}
