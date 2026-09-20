# DeepSWE retro → harness fixes (2026-09-20)

The record of one working session: run gray against the DeepSWE benchmark,
interview the agent after every task about what confused it and what the
harness should have given it, then fix what the interviews exposed.

Everything below is backed by either a test in this repo or a file under
`~/bench/runs/` (trial transcripts, interviews, metrics).

## The evidence

**Campaign.** 113 tasks, single run, `deepseek-v4.1-flash`, strict grading
(all hidden fail-to-pass *and* pass-to-pass tests must pass).

| | |
|---|---|
| decided | 108 of 113 |
| passed | 46 (42.6%) |
| of the 62 failures | 49 were harness cuts — the run never finished cleanly |

A control re-run of a subset reproduced identical fail counts on every
non-cut task, so the misses were systematic, not variance.

**Interviews.** 111 of 113 trials interviewed (0 failures), each fed its own
transcript plus the hidden tests it missed. Clustered, the themes were:

| # | theme | tasks |
|---|---|---|
| 1 | Self-confirming verification: tests written from the same assumptions as the code, then treated as proof | ~85 |
| 2 | Edge/negative semantics left unstated by the spec | ~60 |
| 3 | Exact string / format / exception contracts (byte-level coin flips) | ~45 |
| 4 | Network blocked, no upstream reference | ~55 |
| 5 | Missing tools and linters (`rg` in ~30 narratives) | ~50 |
| 6 | 30 s default timeout vs 4–6 min builds | ~35 |
| 7 | Output truncation forcing re-read loops | ~30 |
| 8 | Hidden tests invisible / half-reset verifier state | ~20 |
| 9 | Repo/branch and git-identity friction | ~20 |

Sharpest single data points: one task lost 7 of 8 hidden tests to **one space
after `BEGIN:`**; ~12 tasks scored **zero** with real work in the tree; one
agent "passed" by reading a stale `__pycache__` that was a broken draft.

## What changed in gray (this repo)

| fix | before → after | why |
|---|---|---|
| **No default bash timeout** | commands died at 30 s (docs claimed 120 s — the doc and the code disagreed) → run until they exit; `timeout` is opt-in, clamped 1–3600 s; agent-level last-resort stop at 3660 s | theme 6 |
| **Missing-tool hint** | `rg: not found` and nothing else → `` `rg` is not installed here · use an equivalent you already have (`grep`, `sed`, `awk`, `python3`) or confirm with `command -v rg` `` | theme 5 |
| **Exit codes through pipes** | a failing suite piped into `head`/`tail`/`grep`/`cat`/`less` reported `exit 0` → the report names the masked stage, and `awk`/`sed`/`tr`/`cut`/`column` are covered too; 3+ stage pipes get a generic note | themes 1, 3 |
| **Output elision** | "N chars omitted" and a 4 KiB `dd` command → the omitted byte window is named, the next offset is given, pages are 16 KiB | theme 7 |
| **Shell output budget** | 12 KiB → 48 KiB (it fired in 86% of sessions) | theme 7 |
| **Turn event cap** | 100k → 500k; 4 runs died on "turn event limit exceeded" | infra |
| **Loop guard** | abort at 3 identical tool+args → nudge at 3, abort at 6 | infra |
| **Stream EOF** | a dropped provider stream killed the run → complete tool args are kept with a warning; only truncated args error | infra |
| **Sampling** | temperature/top_p unreachable → `GRAY_TEMPERATURE`/`GRAY_TOP_P` (env + saved config) sent with every request, omitted when unset | parity with the reference scaffolds |
| **Memory** | size caps and a missing CLI → caps removed; `gray memory list/show/set/edit/remove/clear` | usability |
| **Stock prompt** | gained the implicit-contract clause, never-edit-a-test, write-tests-for-new-behavior, the parallel-work guideline, and "commands run until they exit" | themes 1, 2, 6 |

## What changed in the bench (`~/bench`, not in this repo)

| fix | before → after | why |
|---|---|---|
| **Patch collection (113 task configs)** | `git diff <base> HEAD` — committed work only → `find __pycache__ -delete; git add -A && git diff --cached --binary <base>` | ~12 tasks scored 0 with real work in the tree |
| **Adapter submission** | patch written only when the agent step succeeded → written in a `finally`, so a killed turn still grades; base commit recorded at setup; git identity pre-set | same |
| **Bench prompt v7** | v6 + "read the spec as a checklist of contracts" (exact bytes, exception class, state ownership, where-to-implement) + "your own tests are not evidence" — one adversarial negative-path test per clause | themes 1, 2, 3 |
| **retro tooling** | interviews were fails-only, hardcoded `reward=0`, died on 429 → outcome-aware (passes included), `--all` incremental, retries 429/5xx and socket timeouts with backoff, 3 s pacing, `--jobs N` | tooling |

Verified: the new patch command captures committed + unstaged + untracked
work (the old one captured only the first) and still applies cleanly to a
pristine checkout of the base commit.

## Still open

- **Image tools** (`rg`, `xxd`, `jq`, linters) need the task-image owners;
  each task ships its own prebuilt public image.
- **Offline upstream reference** — same constraint.
- **Provider quota**: opencode zen's Go plan hit its monthly limit, openrouter
  and HuggingFace ran out of credits, so no deepseek-capable provider had
  quota at session end. The bench is ready to run v7 the moment one does.
