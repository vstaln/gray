#!/bin/sh
# Minimal harness-eval runner (meta-harness, gray-sized).
# One gray -p attempt per task in a temp workdir, graded by tests/test.sh.
# Appends one JSON line per task to ~/.gray/harness-runs/summary.jsonl
# (the join key is session_id; full trace stays in ~/.gray/sessions/),
# writes harness-runs/<session-id>/manifest.json (session<->harness join),
# and drops a report.md stub there on failure.
# Usage: ./run.sh [task...]   (default: all tasks)
set -u
EVAL_DIR="$(cd "$(dirname "$0")" && pwd)"
GRAY_BIN="${GRAY_BIN:-gray}"
HARNESS_DIR="${GRAY_HOME:-$HOME/.gray}/harness-runs"
mkdir -p "$HARNESS_DIR"
SUMMARY="$HARNESS_DIR/summary.jsonl"
AGENTS_MD="${GRAY_HOME:-$HOME/.gray}/AGENTS.md"
BIN_VER="$("$GRAY_BIN" --version 2>/dev/null | head -1)"
AGENTS_CKSUM="$(cksum < "$AGENTS_MD" 2>/dev/null | awk '{print $1}')"
HARNESS_HASH="${AGENTS_CKSUM:-na}-${BIN_VER:-unknown}"
TASKS="${*:-dedupe-events budget-rollups reconcile-ledger extract-errors yaml-to-json patch-config join-csv file-manifest fix-json wordcount-report}"
pass=0; total=0
for t in $TASKS; do
  total=$((total+1))
  work="$(mktemp -d)"
  cp -r "$EVAL_DIR/tasks/$t/tests/"* "$work/" 2>/dev/null
  prompt="$(cat "$EVAL_DIR/tasks/$t/instruction.md")"
  sid=""
  if "$GRAY_BIN" -p "$prompt
Work in this directory: $work" >"$work/stdout.log" 2>"$work/stderr.log"; then
    sid="$(grep -o 'gray resume [0-9a-f-]*' "$work/stderr.log" "$work/stdout.log" 2>/dev/null | head -1 | awk '{print $3}')"
    if [ -z "$sid" ]; then
      latest="$(ls -t "${GRAY_HOME:-$HOME/.gray}/sessions/"*.jsonl 2>/dev/null | head -1)"
      sid="$(basename "$latest" .jsonl 2>/dev/null)"
    fi
  fi
  if sh "$EVAL_DIR/tasks/$t/tests/test.sh" "$work" >"$work/grade.log" 2>&1; then
    res=true; pass=$((pass+1))
  else
    res=false
  fi
  ts="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  python3 - "$SUMMARY" "$HARNESS_DIR" "$EVAL_DIR" "$ts" "$t" "$res" "$sid" "$HARNESS_HASH" "$work" <<'PY'
import json, os, sys
summary, hdir, eval_dir, ts, task, res, sid, harness, work = sys.argv[1:10]
passed = res == "true"
model = None
if sid:
    sess = os.path.join(os.path.dirname(summary), "..", "sessions", sid + ".jsonl")
    try:
        with open(sess) as f:
            model = json.loads(f.readline()).get("model")
    except Exception:
        pass
row = {"ts": ts, "task": task, "pass": passed, "session_id": sid,
       "harness": harness, "workdir": work, "model": model}
with open(summary, "a") as f:
    f.write(json.dumps(row) + "\n")
if sid:
    d = os.path.join(hdir, sid)
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, "manifest.json"), "w") as f:
        json.dump(row, f, indent=2)
    if not passed:
        tpl = os.path.join(eval_dir, "report-template.md")
        note = open(tpl).read() if os.path.exists(tpl) else ""
        with open(os.path.join(d, "report.md"), "w") as f:
            f.write(f"# {task} FAILED {ts}\n\nsession: {sid}\nworkdir: {work}\n\n{note}")
PY
  if [ "$res" = true ]; then echo "$t: PASS (session ${sid:-?}, kept at $work)"; else echo "$t: FAIL (session ${sid:-?}, kept at $work)"; fi
done
echo "== $pass/$total passed =="
echo "ledger: $SUMMARY"
