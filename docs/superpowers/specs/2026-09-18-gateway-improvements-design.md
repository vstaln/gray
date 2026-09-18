# Gateway improvements (approved design)

Scope: approach A as approved 2026-09-18 — implement the zip deep-dive
vectors GW-01/02/03/05/06 with minimal diffs in the existing daemon;
cut what cannot stay behavior-preserving. GW-04 deferred.

Non-negotiables (verified against the tree before writing):

- `tick_once` keeps at-most-once claims, `mark_done` on every job path,
  and `TickReport { fired, errors, delivered }` counting semantics.
- `AsyncRunner` stays `?Send` and is never `spawn`d; `?Send` is load-bearing
  (the agent future is not `Send`).
- `fire_one` 600s timeout, `FIRE_CLAIM_TTL_SECS` (1200s), claim renewal, and
  delivery (`mark_done` on every path) are untouched.
- Socket `identify`/`status` payloads stay byte-identical; one JSON line in,
  one JSON line out, 0600 socket, no TCP, `PROTOCOL` stays 1.
- Linux `proc_start_time` (`/proc/<pid>/stat` field 22), runit/systemd
  backends, 65s drain, and `gateway.state.json` shapes are untouched.
- New paths fail soft (warn + fallback); no new `Err` on existing paths.

## GW-01 — bounded tick concurrency (cron_serve.rs)

Single `claim_due_limited(now, owner, MAX_CONCURRENT_FIRES)` pass instead of
claim-one/fire-one in a loop. Drive the claimed jobs with
`futures::FuturesUnordered` on the same task — no `tokio::spawn`, so the
`?Send` bound holds. Aggregate `fired`/`errors`/`delivered` in claim order so
`delivered` order matches serial execution.

`MAX_CONCURRENT_FIRES: usize = 4` as a `const`, not a `Config` field
(Config plumbing cut from this batch). `= 1` behaves exactly as today.
`futures` is already a dependency of the `gray` crate.

Callers (`cron tick`, `cron serve`, REPL tick, gateway loop) get concurrency
without signature changes.

## GW-02 — wall-clock tick alignment (gateway/run.rs)

`MissedTickBehavior::Delay` becomes `Skip`; the first tick aligns with
`tokio::time::interval_at(next_top_of_minute, 60s)` instead of
`interval()` + `sleep` (`sleep` ignores stop signals; `interval_at` keeps
the existing `select!` shutdown path). The catch-up-to-skip change is the
fix; everything else in the loop is identical.

## GW-03 — read-only socket verbs (gateway/socket.rs)

Additive only: `SUPPORTED_VERBS` grows 2 -> 4 with `metrics` and `logs_tail`;
`identify`/`status` handlers and payloads unchanged.

- `metrics`: pure function of existing state — uptime, tick health, job
  count. No new I/O beyond what `status` already reads.
- `logs_tail`: last 200 lines (64 KiB cap, tail-kept) of the existing
  `logs/gray.log` file, plus `total_lines_available`; best-effort — a
  missing/unreadable file reads as empty lines, never an error.

Cut from this batch: `trigger_tick` / `reload_config` / `pause_ticker`
(they need daemon channels plus shared mutable state — a follow-up, not a
minimal diff) and the `PROTOCOL = 2` bump (breaks old clients).

## GW-04 — deferred (pid.rs, service.rs)

`libc 0.2.189` apple headers expose `CTL_KERN` / `KERN_PROC` / `sysctl` but
no `kinfo_proc` / `extern_proc` / `p_starttime` struct, so the zip's sysctl
snippet does not compile as-is. Hand-rolled FFI is not minimal — rejected.
`proc_start_time` keeps today's `None` fallback on macOS; PID-reuse
detection there stays at the existence probe. `launchd` supervisor support
is a follow-up.

## GW-05 — linger warning only (gateway/service.rs)

`install_systemd` appends a warning to its success message when
`loginctl show-user $USER --property=Linger` does not report `Linger=yes`.
Warning text only; install/start flow otherwise identical.

Cut from this batch: `kill(-pid)` process-group kill (risks signalling the
supervisor's own group). `stop_by_pid` keeps single-PID `SIGTERM`; document
the orphan-child limit in the function comment.

## GW-06 — chaining panic hook (gateway/run.rs)

`run_foreground` installs a hook that chains the existing `main.rs` hook via
`std::panic::take_hook` (never replaces it) and best-effort writes
`stopped` state with a `panic: <msg>` exit reason to `gateway.state.json`.
Drain still waits the 65s window.

Cut from this batch: `AsyncRunner` cancel-token plumbing (changes the trait
and breaks `ReplRunner` / stubs / all callers). True cooperative
cancellation is a follow-up.

## Verification

- `cargo test -p gray` (covers `cron_serve_tests`, `socket_tests`,
  `pid_tests`, `service_tests` as the contract suite) plus `cargo fmt
  --check` pre-commit, per AGENTS.md.
- Baseline before changes: `cargo test -p gray --lib` — 425 passed.
- New tests extend existing file shapes only: concurrent tick ordering
  with the stub runner, `Skip` alignment unit check, `metrics`/`logs_tail`
  request-line tests, linger-warning text test, panic-hook state test.
- Manual: `gray gateway run` + `status` in a temp `GRAY_HOME`; `cron tick`
  with stub-free smoke unchanged.

## Follow-ups (explicitly out of scope)

Stateful socket verbs (needs daemon channels), GW-04 macOS sysctl/launchd,
process-group stop semantics, cooperative cancellation via trait change,
`Config`-plumbed concurrency limit.
