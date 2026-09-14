# eval/ — minimal harness loop (meta-harness, gray-sized)

Zero Rust, zero deps, zero UI. Append-only files under `~/.gray/harness-runs/`
(gitignored); this dir holds only task definitions + three shell scripts
(`run.sh`, `harness-note.sh`, `rollup.sh`).

## The 3 pieces

1. **Join the trace** — `harness-note.sh <session-id> [task] [pass] [note]`
   writes `~/.gray/harness-runs/<session-id>/manifest.json` linking the
   existing full trace (`~/.gray/sessions/<id>.jsonl`) to the harness hash
   (`AGENTS.md` sha). Nothing is summarized away; grep stays possible.
2. **Ledger + failure notes** — every run appends ONE JSON line to
   `~/.gray/harness-runs/summary.jsonl`. On failure, write `report.md`
   (<=30 lines: what changed, what broke, why) next to the manifest.
3. **Eval set** — 10 tasks in `tasks/`, each `instruction.md` + `tests/test.sh`
   (`test.sh <workdir>`, exit 0 = pass). `./run.sh [task...]` runs gray
   (`gray -p`) once per task in a temp dir, grades, appends ledger lines.
   Full spec: [domain_spec.md](domain_spec.md) (meta-harness onboarding template).

## Split convention (anti-leakage)

Search set (8, iterate freely): `dedupe-events`, `budget-rollups`,
`reconcile-ledger`, `extract-errors`, `yaml-to-json`, `patch-config`,
`join-csv`, `file-manifest`. Held-out (`fix-json`, `wordcount-report`):
score only, never steer edits.

## Query history

```sh
grep -c '"pass": true' ~/.gray/harness-runs/summary.jsonl
grep '"task": "dedupe-events"' ~/.gray/harness-runs/summary.jsonl | tail -5
sh rollup.sh | tail -3   # per-run tokens + totals; usage_est = chars/4 fallback ('-p' runs)
```
