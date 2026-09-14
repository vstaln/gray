# Domain Spec: gray harness optimization

Method: [Meta-Harness](https://yoonholee.com/meta-harness/) ([paper](https://arxiv.org/abs/2603.28052),
[code](file:///home/vstaln/gray/reference/stanford-iris-lab/meta-harness), onboarding template:
[ONBOARDING.md](file:///home/vstaln/gray/reference/stanford-iris-lab/meta-harness/ONBOARDING.md)).
Meta-Harness searches over **harness code** — the code around a fixed base model that decides
what to store, retrieve, and show — not over model weights. Local layout: [eval/](file:///home/vstaln/gray/eval).

## Domain Summary

Gray is a minimal local coding agent (bash-only tools, `gray -p "<prompt>"` one-shot mode).
Goal: improve gray's **harness** — system prompt (`~/.gray/AGENTS.md` → byte-stable build in
`crates/gray/src/system_prompt.rs`), per-turn skill context (`skills_tool.rs`), tool definitions
(blocking `bash`), and what traces get kept — so task success rises and context cost falls.
Unit of evaluation: **one task run** = one `gray -p` attempt in a fresh temp workdir, graded
pass/fail by that task's `tests/test.sh`. Fixed: base model(s), tool surface (blocking bash only),
task fixtures. Allowed to change: `AGENTS.md` text, skill surfacing, prompt construction,
logging/ledger format — everything that shapes what the model sees. Total budget (default):
≤10 candidates per iteration, 1 trial per search task, ≤5 min per task run; held-out scored once
per iteration. Mark any deviation `unknown` + propose a default.

## Harness and Search Plan

Candidate harness shape: a **harness hash** = `cksum(AGENTS.md)` + `gray --version`
(see [run.sh](file:///home/vstaln/gray/eval/run.sh)); every ledger line records it, so any
score is attributable to an exact harness version. Baseline candidates (hand-written, keep):
current `AGENTS.md` as-is; `AGENTS.md` minus one section (ablation); skill-list-off variant.
Interface compliance = `run.sh` exits 0 and appends well-formed ledger lines; `test.sh <dir>`
contract (exit 0 = pass, nonzero + one-line reason = fail). Out of scope: model weights /
provider switches, new tools (bash-only is fixed), editing task fixtures or graders to make
them pass (benchmark-specific encoding — cf. harbor `select-harness` rule: "do not encode
benchmark-specific data, validator behavior, test paths, or task-specific output paths").
First loop: propose ≤3 harness edits (mix exploitation/exploration per the meta-harness
[SKILL.md](file:///home/vstaln/gray/reference/stanford-iris-lab/meta-harness/reference_examples/text_classification/.claude/skills/meta-harness/SKILL.md)
axes: prompt template / memory content / selection / sizing / learning trigger / LLM use),
prototype each against real traces in `/tmp`, implement, score on the search set only.

## Evaluation Plan

Search set (iterate freely, 8 tasks): `dedupe-events`, `budget-rollups`, `reconcile-ledger`,
`extract-errors`, `yaml-to-json`, `patch-config`, `join-csv`, `file-manifest`.
Held-out set (score only, never steer edits, 2 tasks): `fix-json`, `wordcount-report`.
Primary metric: pass rate per set. Secondary: median context tokens per run (from session
`usage`), wall-clock per task, timeout rate. Noise: single-trial grading is noisy on
ambiguous tasks — on ties, re-run the tied tasks ×3 and majority-vote (mark `unknown` if
budget forbids). One candidate evaluation ≈ 10 tasks × ≤5 min ≈ ≤50 min wall-clock, mostly
model latency. Cheap validation before full runs: `sh -n` on scripts; each `test.sh` must
PASS on a hand-made good output and FAIL on an empty dir (verified at authoring time).
Leakage: fixtures are public in-repo, so isolation is operational (search/held-out split +
`harness-note.sh` manifests), not access control — same caveat as the meta-harness text-class
release. Contamination: tasks are synthetic and gray-specific; risk low.

## Experience and Logging

Offline traces: `~/.gray/sessions/*.jsonl` (full message history + `usage` per assistant turn)
warm-start all analysis; prior failure notes in `~/.gray/harness-runs/*/report.md`.
References worth encoding into proposer context: meta-harness [SKILL.md](file:///home/vstaln/gray/reference/stanford-iris-lab/meta-harness/reference_examples/text_classification/.claude/skills/meta-harness/SKILL.md)
(anti-parameter-tuning + anti-overfitting rules), [inner_loop.py](file:///home/vstaln/gray/reference/stanford-iris-lab/meta-harness/reference_examples/text_classification/inner_loop.py)
(offline/online loop, early stopping), TB2 [baseline_kira.py](file:///home/vstaln/gray/reference/stanford-iris-lab/meta-harness/reference_examples/terminal_bench_2/agents/baseline_kira.py)
(native-tool scaffold evolution).
Online, store per candidate run (append-only, never rewrite): session jsonl (already stored),
`harness-runs/<session-id>/manifest.json` (run.sh writes it per run; use
[harness-note.sh](file:///home/vstaln/gray/eval/harness-note.sh) to join manual runs)
(session↔harness-hash link), one ledger line per task in `harness-runs/summary.jsonl`
(`{ts, task, pass, session_id, harness, workdir, model}`), and on failure a `report.md` ≤30 lines
(run.sh drops a stub; template: [report-template.md](file:///home/vstaln/gray/eval/report-template.md)).
Highest-signal artifacts for debugging: the session jsonl (prompts, tool calls, errors verbatim)
first, `stderr.log`/`grade.log` in the kept workdir second, scores last — never decide from
scores alone (the core meta-harness finding: ≤26K tok of scores/summaries vs up to ~10M tok of
queryable raw traces). Query CLI (no new code): `grep '"task": "<name>"' summary.jsonl | tail`,
`grep -c '"pass": true' summary.jsonl`.

## Open Questions and Unknowns

- Frozen model set: `unknown` — default: whatever `gray` is configured with today; record per-run.
- Budget per iteration: `unknown` — default above; tighten once first loop is timed.
- Multi-trial variance: `unknown` — default single trial; escalate to ×3 on ties.
- Token accounting: per-run rollup = `rollup.sh` (session `usage`; `-p` sessions
  don't persist usage yet, so it falls back to `usage_est` = message chars/4).
  Root fix (persist usage in print mode) when the estimate stops being enough.
- Full proposer automation (`meta_harness.py` equivalent): deferred until 2+ manual loops validate the split.
