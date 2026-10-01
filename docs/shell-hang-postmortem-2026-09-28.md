# Bash tool hang — postmortem and fix proposal

Incident date: 2026-09-28. One `bash` tool call ran 9m44s in a live session and
ended only because the user pressed cancel. Nothing was corrupted and no work was
lost; the cost was ten minutes of a session the user was sitting in.

## Timeline (from gray's own records)

| When | What |
|------|------|
| 17:49:12 | Job starts. Command: write two files, then `node imgtest.cjs` (Playwright + Chromium render of `assets/logo-animated.svg` inside an `<img>`) |
| 17:49:12 | Job log `bash-195de3c234f047b29a97f9c3a1bff4c6.log` receives its only 3 bytes: `ok\n` — node had finished all its work and printed |
| 17:49:12 → 17:59 | `child.wait()` never returns. No timeout is set, so `wait_or_pend` is `pending()` forever and the `select!` has nothing left to fire on |
| ~17:59 | User cancels. `Cause::Cancel` → SIGTERM to the process group → `exit 143 (SIGTERM)` |

Reproduced deliberately under `timeout -s KILL 45`: node stayed alive after
printing `ok`, with live chromium children, and needed SIGKILL. The hang is inside
the script's `await browser.close()`.

## Root cause of the hang

Playwright's `browser.close()` occasionally never returns while waiting for
Chromium to shut down. Evidence that this is a race and not a defect in the
script: the identical script, unchanged, closed cleanly 6 out of 6 times
afterwards (1 standalone + 5 in a loop), each under a 40s SIGKILL guard. So the
trigger is nondeterministic and lives inside Chromium's shutdown path — a caller
cannot fix it in code, only bound it.

## Why it cost ten minutes (the real gap)

`crates/gray-tools/src/shell/contract.rs:15`:

```rust
pub const DEFAULT_TIMEOUT_SECS: Option<u64> = None;
```

Its comment records why: 30s and 120s defaults were both tried and reverted
because they killed real builds and test suites, and the agent read a kill as a
failed command rather than a short budget. With no default, a bash call has
exactly three exits: the command finishes, the caller passes an explicit
`timeout`, or the user cancels. A rare external race therefore turns into an
unbounded, silent wait — the agent cannot react and only the user can end it.

That is the part worth fixing. The trigger is outside anyone's control (a browser
driver, an editor, any subprocess that wedges in a library call); the harness is
where a bound belongs.

## Recommended fix: hand a stalled command to the background lane

The first draft of this report proposed killing the group after ten silent
minutes. That contradicts gray's own documented stance, found in the job lane
(`bash/jobs.rs`, `await_settled`): a liveness verdict is *"the agent's cue to
inspect or kill, never an auto-kill"*. Auto-killing a silent command would repeat
the reverted defaults' mistake in a new costume.

The asymmetry is the actual gap: the **background lane** has a bound and a
liveness verdict; the **blocking lane** has neither — no periodic note, no yield,
nothing but the exit. So the fix is to give the blocking lane what the job lane
already has:

1. The pump records the time of the last output chunk (`Arc<AtomicU64>`, unix
   millis) — no new task, no pipe polling.
2. `run_command` gains a stall arm: after `STALL` (proposal: 600s) with no output
   and no explicit `timeout` from the caller, the command is **not killed**. It is
   handed to the existing background lane and the call returns immediately with
   `still running · no output for 600s · log <path>`, plus the `action:output`
   /`wait_ms` hint the job lane already prints.
3. The command keeps running; the agent decides — inspect the log, keep waiting
   with `wait_ms`, or cancel. Nothing is ever killed silently, and no real build
   is affected because builds are chatty.

Check to leave behind (repo discipline: one runnable check per fix): a test that a
command silent past an injected bound stops blocking, lands in the job registry
with a liveness note, and leaves its process group alive and reaped.

## Decision

- **A (recommended):** implement the background-lane handoff above in
  `crates/gray-tools`, with the stall test, and update the tool description and
  `DEFAULT_TIMEOUT_SECS`'s comment to record why the blocking lane stays unbounded.
- **B:** leave gray unbounded (respecting the recorded decision) and instead bound
  every browser/driver command I run with `timeout -s KILL N ... > log 2>&1`.
  Cheaper, but the same race can still hang a user-facing session through any
  other subprocess.

## Observed state — and what I could not verify

Checked, not assumed:

| Check | Result |
|-------|--------|
| Binary actually in use | `/home/vstaln/.local/bin/gray` — `gray --version` → `0.1.5`, mtime 2026-09-28 14:59 |
| Working tree analysed | `2fe715eb`, workspace version `0.1.5`, branch `feat/foreign-plugin-adapter` |
| That binary's own tool text | says `timeout` has "no default: commands run until they exit" — the behaviour observed live during the incident |
| `strings` on that binary | contains the explicit-timeout kill path ("timed out after") and the 30s pump-drain note ("output truncated: pump drain timed out after"); contains no "no output for", "still running", "possibly stuck" or "liveness" text. Weak evidence: absence of a string is not proof of absence of behaviour |

Not checked, so this is unknown rather than resolved:

- Whether a gray release **newer than 0.1.5** exists, or whether it already carries a
  blocking-lane bound or liveness verdict. No `git fetch` was run; everything above
  describes the source at this commit and the binary installed here.
- Whether `main` differs from the branch this tree sits on.

So this report records one observed failure and the code that produced it. It is
**not** evidence that the bug still exists in the newest builds — that has to be
re-checked against a fresh build before anyone acts on it.

