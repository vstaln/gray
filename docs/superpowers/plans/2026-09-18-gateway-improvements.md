# Gateway improvements Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the approved gateway improvements (GW-01/02/03/05/06) with minimal, behavior-preserving diffs.

**Architecture:** Single-pass concurrent tick on the same task (no spawn, `?Send` holds), additive read-only socket verbs at PROTOCOL 1, Skip+aligned ticker, warning-only linger text, chaining panic hook.

**Tech Stack:** Rust edition 2024, tokio (interval_at, watch, time), futures::FuturesUnordered (already a `gray` dependency), serde_json, libc 0.2.189.

**Spec:** `docs/superpowers/specs/2026-09-18-gateway-improvements-design.md`

## Deferred (per approved spec — GW-04 sysctl/launchd is out of scope)

GW-04 stays deferred: pinned `libc 0.2.189` apple headers have no
`kinfo_proc`, so the zip's sysctl snippet does not compile; hand-rolled FFI
is not minimal. `proc_start_time` keeps its `None` fallback on macOS.

## Global Constraints

- Baseline before changes: `cargo test -p gray --lib` — 425 passed.
- `AsyncRunner` stays `?Send` and is never `spawn`d.
- Socket `PROTOCOL` stays 1; `identify`/`status` payloads stay byte-identical.
- `fire_one` 600s timeout, `FIRE_CLAIM_TTL_SECS` (1200s), and `mark_done` on every job path are untouched.
- New paths fail soft (warn + fallback); no new `Err` on existing paths.
- Verify narrow per task: `CARGO_BUILD_JOBS=4 cargo test -p gray --lib`; pre-commit add `cargo fmt --check`.
- Touch only: `crates/gray/src/cron_serve.rs`, `crates/gray/src/cron_serve_tests.rs`, `crates/gray/src/gateway/run.rs`, `crates/gray/src/gateway/socket.rs`, `crates/gray/src/gateway/socket_tests.rs`, `crates/gray/src/gateway/service.rs`, `crates/gray/src/gateway/service_tests.rs`. Do NOT touch `composer/`, `gray-markdown`, `README.md` (sibling WIP lives there).

---

### Task 1: GW-01 bounded tick concurrency

**Files:**
- Modify: `crates/gray/src/cron_serve.rs:20-24` (TickReport — no change, read for counting semantics)
- Modify: `crates/gray/src/cron_serve.rs:262-300` (`tick_once` loop)
- Test: `crates/gray/src/cron_serve_tests.rs` (extend existing stub-runner shapes)

**Interfaces:**
- Consumes: `CronStore::claim_due_limited(now, owner, limit, exclude)`, `fire_one(store, runner, job, now, deliver) -> (RunStatus, Option<DeliveredFire>)`, `futures::StreamExt::next`
- Produces: `pub const MAX_CONCURRENT_FIRES: usize = 4`; `tick_once` same signature, `delivered` in claim order

- [ ] **Step 1: Write the failing test** — in `crates/gray/src/cron_serve_tests.rs`, add (reusing the file's `StubRunner`, `due_store`, `one_due` helpers):

```rust
#[tokio::test]
async fn tick_fires_two_due_jobs_and_keeps_claim_order() {
    let home = tempfile::tempdir().unwrap();
    let store = due_store(
        &home,
        serde_json::json!([
            {"id": "a", "name": "a", "prompt": "x",
             "schedule": {"Interval": {"secs": 3600}}, "enabled": true,
             "created_at": 1, "next_run_at": 1},
            {"id": "b", "name": "b", "prompt": "y",
             "schedule": {"Interval": {"secs": 3600}}, "enabled": true,
             "created_at": 1, "next_run_at": 1},
        ]),
    );
    let runner = StubRunner {
        text: "hi".to_string(),
        fail: false,
        seen: Default::default(),
    };
    let rep = tick_once(
        &store,
        &runner,
        &SaveLocalDeliver {
            home: home.path().to_path_buf(),
        },
        "test",
    )
    .await
    .unwrap();
    assert_eq!(rep.fired, 2);
    assert_eq!(rep.errors, 0);
    assert_eq!(rep.delivered.len(), 2);
    assert_eq!(rep.delivered[0].id, "a");
    assert_eq!(rep.delivered[1].id, "b");
    for id in ["a", "b"] {
        let job = store.get(id).unwrap().unwrap();
        assert_eq!(job.last_status, Some(crate::cron::RunStatus::Ok));
        assert!(job.fire_claim.is_none());
    }
}
```

- [ ] **Step 2: Run test to verify it passes serially (baseline)**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray --lib cron_serve_tests::tick_fires_two_due_jobs_and_keeps_claim_order`
Expected: PASS (serial code already fires both; locks in the ordering contract before the rewrite)

- [ ] **Step 3: Write minimal implementation** — in `crates/gray/src/cron_serve.rs`, add above `tick_once`:

```rust
/// Max jobs fired concurrently in one tick pass. `const`, not `Config`:
/// Config plumbing is a follow-up. `= 1` behaves exactly as the old serial loop.
pub const MAX_CONCURRENT_FIRES: usize = 4;
```

Replace the `loop { claim 1 ... fire ... }` body (keep the heartbeat and `owner` lines as-is) with:

```rust
    let now = crate::cron::now_secs();
    let due = store.claim_due_limited(now, &owner, MAX_CONCURRENT_FIRES, &fired_ids)?;
    // Same-task concurrency only: `AsyncRunner` is `?Send`, so never `spawn`.
    // Results re-attached by index, so counts and `delivered` order match serial.
    let mut pending = futures::stream::FuturesUnordered::new();
    for (idx, job) in due.into_iter().enumerate() {
        fired_ids.push(job.id.clone());
        pending.push(async move {
            let out = fire_one(store, runner, job, now, deliver).await;
            (idx, out)
        });
    }
    let mut ordered: Vec<Option<(crate::cron::RunStatus, Option<DeliveredFire>)>> = Vec::new();
    ordered.resize_with(pending.len(), || None);
    while let Some((idx, (status, saved))) = futures::StreamExt::next(&mut pending).await {
        ordered[idx] = Some((status, saved));
    }
    for slot in ordered.into_iter().flatten() {
        let (status, saved) = slot;
        report.fired += 1;
        if !matches!(status, crate::cron::RunStatus::Ok) {
            report.errors += 1;
        }
        if let Some(saved) = saved {
            report.delivered.push(saved);
        }
    }
    Ok(report)
```

Notes: `futures` is already a dependency of the `gray` crate (`crates/gray/Cargo.toml`); `futures::stream::FuturesUnordered` + `futures::StreamExt::next` need no new deps. The closure borrows `store`/`runner`/`deliver` (all `&`-shared, as today); `job`/`now` move per item. `fired_ids` is pushed before firing, preserving the old exclusion semantics for the single pass.

- [ ] **Step 4: Run tests to verify**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray --lib`
Expected: all pass including the new ordering test and the existing `tick_agent_failure_records_error_and_continues` (2-job error counting)

- [ ] **Step 5: Commit**

```bash
git add crates/gray/src/cron_serve.rs crates/gray/src/cron_serve_tests.rs
git commit -m "feat(cron): bounded concurrent tick pass (GW-01)"
```

---

### Task 2: GW-02 Skip + wall-clock alignment

**Files:**
- Modify: `crates/gray/src/gateway/run.rs:68-70` (interval setup)
- Test: none in `gateway/` (no run_tests file exists; alignment is time-dependent) — verify by build + existing suite

**Interfaces:**
- Consumes: `tokio::time::interval_at`, `crate::cron::now_secs`
- Produces: first tick aligned to next top-of-minute; `MissedTickBehavior::Skip`

- [ ] **Step 1: Write minimal implementation** — in `crates/gray/src/gateway/run.rs`, replace:

```rust
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
```

with:

```rust
    // Aligned to wall-clock :00 so recurring schedules fire on minute
    // boundaries instead of drifting. `interval_at` (not `sleep`) keeps the
    // existing `select!` shutdown path responsive during the wait.
    let first = std::time::Instant::now()
        + Duration::from_secs(60 - (crate::cron::now_secs() as u64 % 60));
    let mut interval = tokio::time::interval_at(first, Duration::from_secs(60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
```

- [ ] **Step 2: Run tests to verify**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray --lib`
Expected: PASS (no behavior contract covers the interval itself; suite guards the tick body)

- [ ] **Step 3: Commit**

```bash
git add crates/gray/src/gateway/run.rs
git commit -m "fix(gateway): align ticks to wall-clock minutes, skip missed (GW-02)"
```

---

### Task 3: GW-03 read-only socket verbs

**Files:**
- Modify: `crates/gray/src/gateway/socket.rs:16-17` (PROTOCOL stays 1, verbs 2 -> 4)
- Modify: `crates/gray/src/gateway/socket.rs:64-90` (add `metrics_payload` + `tail_gray_log` beside `cron_payload`)
- Modify: `crates/gray/src/gateway/socket.rs:89-115` (`handle_request_line` match arms)
- Test: `crates/gray/src/gateway/socket_tests.rs` (extend `handle_request_line` shapes)

**Interfaces:**
- Consumes: `identify_payload`, `cron_payload`, `crate::cron::now_secs`, `home.join("logs").join("gray.log")`
- Produces: `SUPPORTED_VERBS: [&str; 4]`; `metrics_payload(home, now) -> serde_json::Value`; `tail_gray_log(home) -> (Vec<String>, usize)`; arms `Some("metrics")`, `Some("logs_tail")`

- [ ] **Step 1: Write the failing tests** — append to `crates/gray/src/gateway/socket_tests.rs`:

```rust
#[test]
fn metrics_reports_existing_state_shapes() {
    let home = tempfile::tempdir().unwrap();
    let line = handle_request_line(home.path(), br#"{"id":3,"verb":"metrics"}"#, 1000);
    let answer: serde_json::Value = serde_json::from_slice(&line).unwrap();
    assert_eq!(answer["ok"], serde_json::json!(true));
    assert_eq!(answer["protocol"], serde_json::json!(1));
    assert_eq!(answer["id"], serde_json::json!(3));
    assert!(answer["result"]["uptime_secs"].is_number(), "{answer}");
    assert!(answer["result"]["cron"].is_object(), "{answer}");
    assert_eq!(answer["result"]["cron"]["jobs"], serde_json::json!(0));
}

#[test]
fn logs_tail_is_empty_without_a_log_file() {
    let home = tempfile::tempdir().unwrap();
    let line = handle_request_line(home.path(), br#"{"verb":"logs_tail"}"#, 1000);
    let answer: serde_json::Value = serde_json::from_slice(&line).unwrap();
    assert_eq!(answer["ok"], serde_json::json!(true));
    assert_eq!(answer["result"]["lines"], serde_json::json!([]));
    assert_eq!(answer["result"]["total_lines_available"], serde_json::json!(0));
}

#[test]
fn logs_tail_returns_last_lines_capped() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("logs");
    std::fs::create_dir_all(&dir).unwrap();
    let mut body = String::new();
    for i in 0..250 {
        body.push_str(&format!("line {i}\n"));
    }
    std::fs::write(dir.join("gray.log"), &body).unwrap();
    let line = handle_request_line(home.path(), br#"{"verb":"logs_tail"}"#, 1000);
    let answer: serde_json::Value = serde_json::from_slice(&line).unwrap();
    let lines = answer["result"]["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 200);
    assert_eq!(lines[0], serde_json::json!("line 50"));
    assert_eq!(lines[199], serde_json::json!("line 249"));
    assert_eq!(answer["result"]["total_lines_available"], serde_json::json!(250));
}

#[test]
fn unknown_verbs_still_list_all_four() {
    let home = tempfile::tempdir().unwrap();
    let line = handle_request_line(home.path(), br#"{"verb":"nope"}"#, 1000);
    let answer: serde_json::Value = serde_json::from_slice(&line).unwrap();
    assert_eq!(answer["ok"], serde_json::json!(false));
    for verb in ["identify", "status", "metrics", "logs_tail"] {
        assert!(
            answer["supported_verbs"].to_string().contains(verb),
            "{answer}"
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray --lib gateway::socket`
Expected: FAIL — `metrics`/`logs_tail` answer `ok: false` (unknown verb)

- [ ] **Step 3: Write minimal implementation** — in `crates/gray/src/gateway/socket.rs`:

Replace:

```rust
pub const PROTOCOL: u32 = 1;
pub const SUPPORTED_VERBS: [&str; 2] = ["identify", "status"];
```

with:

```rust
pub const PROTOCOL: u32 = 1;
pub const SUPPORTED_VERBS: [&str; 4] = ["identify", "status", "metrics", "logs_tail"];
/// `logs_tail` shape: last lines of `logs/gray.log`, newest last.
pub const LOGS_TAIL_LINES: usize = 200;
/// Byte cap on the tail read; the tail is kept, the head is dropped.
const LOGS_TAIL_MAX_BYTES: u64 = 64 * 1024;
```

Add beside `cron_payload` (after it, before `handle_request_line`):

```rust
/// Read-only telemetry reusing the shapes `status` already reports.
/// No new I/O beyond what `status_payload` reads today.
pub fn metrics_payload(home: &Path, now: i64) -> serde_json::Value {
    let mut out = identify_payload(home);
    out["cron"] = cron_payload(home, now);
    out["answered_at"] = serde_json::json!(now);
    out["answering_pid"] = serde_json::json!(std::process::id());
    out
}

/// Last `LOGS_TAIL_LINES` lines of `logs/gray.log` (newest last) plus the
/// total line count. Best-effort: a missing/unreadable file reads as empty,
/// never an error (same fail-soft rule as every socket path).
pub fn tail_gray_log(home: &Path) -> (Vec<String>, usize) {
    let body = std::fs::read(home.join("logs").join("gray.log")).unwrap_or_default();
    let tail: Vec<u8> = if body.len() as u64 > LOGS_TAIL_MAX_BYTES {
        body[body.len() - LOGS_TAIL_MAX_BYTES as usize..].to_vec()
    } else {
        body
    };
    let text = String::from_utf8_lossy(&tail);
    // A mid-line cut from the byte cap must not fabricate a first line.
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if body.len() as u64 > LOGS_TAIL_MAX_BYTES && !lines.is_empty() {
        lines.remove(0);
    }
    let total = lines.len();
    let kept = if total > LOGS_TAIL_LINES {
        lines[total - LOGS_TAIL_LINES..].to_vec()
    } else {
        lines
    };
    (kept, total)
}
```

Add match arms (after `Some("status")`, before `other`):

```rust
            Some("metrics") => serde_json::json!({
                "ok": true, "protocol": PROTOCOL, "result": metrics_payload(home, now),
            }),
            Some("logs_tail") => {
                let (lines, total) = tail_gray_log(home);
                serde_json::json!({
                    "ok": true, "protocol": PROTOCOL,
                    "result": { "lines": lines, "total_lines_available": total },
                })
            }
```

Also update the module doc comment's verb list (`identify`/`status` -> plus `metrics`/`logs_tail` read-only). `identify`/`status` arms and payloads stay byte-identical.

- [ ] **Step 4: Run tests to verify**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray --lib gateway::socket`
Expected: PASS, including the pre-existing `identify_answers_about_this_process_and_misses_list_verbs` and `status_reports_cron_fields_even_on_an_empty_home`

- [ ] **Step 5: Commit**

```bash
git add crates/gray/src/gateway/socket.rs crates/gray/src/gateway/socket_tests.rs
git commit -m "feat(gateway): read-only metrics and logs_tail socket verbs (GW-03)"
```

---

### Task 4: GW-05 linger warning (warning text only)

**Files:**
- Modify: `crates/gray/src/gateway/service.rs:524-553` (`install_systemd` message only)
- Test: `crates/gray/src/gateway/service_tests.rs` (pure helper for the warning condition)

**Interfaces:**
- Consumes: `std::process::Command::new("loginctl")`
- Produces: `pub(crate) fn linger_warning_for(output: &str) -> Option<&'static str>`; warning appended to the install message

- [ ] **Step 1: Write the failing test** — append to `crates/gray/src/gateway/service_tests.rs`:

```rust
#[test]
fn linger_warning_fires_only_without_linger_yes() {
    assert_eq!(linger_warning_for("Linger=yes\n"), None);
    assert!(linger_warning_for("Linger=no\n").is_some());
    assert!(linger_warning_for("").is_some());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray --lib gateway::service::tests::linger_warning_fires_only_without_linger_yes`
Expected: FAIL with "not found in this scope" (helper does not exist yet)

- [ ] **Step 3: Write minimal implementation** — in `crates/gray/src/gateway/service.rs`, add near `systemctl`:

```rust
/// Pure check for the linger warning: `Some(text)` unless loginctl output
/// confirms `Linger=yes`. Kept pure so tests cover it without loginctl.
pub(crate) fn linger_warning_for(loginctl_stdout: &str) -> Option<&'static str> {
    if loginctl_stdout.contains("Linger=yes") {
        return None;
    }
    Some("⚠ systemd linger is off — user services stop at logout; run `sudo loginctl enable-linger $USER` to keep the gateway up")
}
```

In `install_systemd`, after `let mut msg = format!("installed systemd user unit: {}", unit.display());`, insert:

```rust
    // Warning text only: never changes the install/start flow. Best-effort;
    // a missing loginctl (or empty USER) warns rather than errors.
    let user = std::env::var("USER").unwrap_or_default();
    let linger_out = std::process::Command::new("loginctl")
        .args(["show-user", &user, "--property=Linger"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    if let Some(warn) = linger_warning_for(&linger_out) {
        msg.push('\n');
        msg.push_str(warn);
    }
```

Also extend the `stop_by_pid` doc comment with one line: children it spawned (pre-scripts, tool subshells) are not signalled — single-PID `SIGTERM` is deliberate; group-kill is a rejected follow-up.

- [ ] **Step 4: Run tests to verify**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray --lib gateway::service`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/gray/src/gateway/service.rs crates/gray/src/gateway/service_tests.rs
git commit -m "feat(gateway): linger warning on systemd install (GW-05)"
```

---

### Task 5: GW-06 chaining panic hook

**Files:**
- Modify: `crates/gray/src/gateway/run.rs:22-30` (`run_foreground` head)
- Test: none in `gateway/` for the hook itself (process-global hook; covered by build + suite) — but add a state-shape assertion via existing `state::record` in the new test below if feasible; otherwise verify by suite

**Interfaces:**
- Consumes: `std::panic::take_hook`, `state::write`, `state::record`, `state::STATE_STOPPED`
- Produces: panic hook chained in `run_foreground` only; `main.rs` hook untouched

- [ ] **Step 1: Write minimal implementation** — in `crates/gray/src/gateway/run.rs`, after `let started_at = record.started_at;`, insert:

```rust
    // Post-mortem observability only: chain (never replace) the `main.rs`
    // hook, and best-effort record the panic so `gateway status` stops
    // reporting a stale `running` after a crash. Writes via the same atomic
    // state path as the normal stop record.
    let panic_home = home.clone();
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = state::write(
            &panic_home,
            &state::record(state::STATE_STOPPED, Some(&format!("panic: {info}")), started_at),
        );
        prev_hook(info);
    }));
```

- [ ] **Step 2: Run tests to verify**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray --lib`
Expected: PASS (hook is additive; existing `state_tests` guard the record shape)

- [ ] **Step 3: Commit**

```bash
git add crates/gray/src/gateway/run.rs
git commit -m "feat(gateway): chain panic hook to record crash state (GW-06)"
```

---

### Task 6: Final verification

- [ ] **Step 1: Run the full narrow suite**

Run: `CARGO_BUILD_JOBS=4 cargo test -p gray --lib`
Expected: 425 + new tests (Tasks 1, 3, 4), 0 failures

- [ ] **Step 2: Run fmt check**

Run: `cargo fmt --check`
Expected: clean; if not, `cargo fmt` the touched files and re-run

- [ ] **Step 3: Manual smoke (temp home, no provider needed)**

```bash
export GRAY_HOME="$(mktemp -d)/.gray"
./target/debug/gray gateway status
```

Expected: `gateway: not running` + state/socket lines, no panic. Then unset `GRAY_HOME`.

- [ ] **Step 4: Report per-PR state** — per AGENTS.md, do NOT push/merge unasked. Report branch state and ask whether to open a PR and against which branch.
