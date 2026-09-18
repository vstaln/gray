//! Bounded-concurrency tests for `join_ordered` (ToolRush parity: 32).
//! Env is process-global: `LOCK` serializes the tests in this file.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use futures::future::BoxFuture;

use crate::agent::ToolOutput;
use crate::parallel::{join_ordered, parallel_max};

static LOCK: Mutex<()> = Mutex::new(());

#[test]
fn cap_defaults_when_unset() {
    let _g = LOCK.lock().unwrap();
    unsafe { std::env::remove_var("GRAY_PARALLEL_MAX") };
    assert_eq!(parallel_max(), 32);
}

#[test]
fn cap_parses_valid_values() {
    let _g = LOCK.lock().unwrap();
    unsafe { std::env::set_var("GRAY_PARALLEL_MAX", "4") };
    assert_eq!(parallel_max(), 4);
    unsafe { std::env::set_var("GRAY_PARALLEL_MAX", " 8 ") };
    assert_eq!(parallel_max(), 8);
    unsafe { std::env::remove_var("GRAY_PARALLEL_MAX") };
}

#[test]
fn cap_falls_back_on_invalid_or_zero() {
    let _g = LOCK.lock().unwrap();
    for bad in ["0", "abc", "", "  "] {
        unsafe { std::env::set_var("GRAY_PARALLEL_MAX", bad) };
        assert_eq!(parallel_max(), 32, "input {bad:?} must fall back to 32");
    }
    unsafe { std::env::remove_var("GRAY_PARALLEL_MAX") };
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
