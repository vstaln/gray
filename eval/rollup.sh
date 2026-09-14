#!/bin/sh
# Token rollup (goose P3 / meta-harness "token accounting"): join
# harness-runs/summary.jsonl -> sessions/<id>.jsonl `usage` and print one
# JSON line per run plus a totals line. Fields are summed as-is; per goose's
# invariant cache_read/cache_write are breakdown subsets of input_tokens,
# so don't add those three together. When a session has no persisted usage
# (`-p` runs today), the run gets `usage_est` = message chars // 4
# (domain_spec fallback); estimates are never mixed into `totals`. Zero deps.
# Usage: ./rollup.sh [summary.jsonl]
set -u
GRAY_HOME="${GRAY_HOME:-$HOME/.gray}"
SUMMARY="${1:-$GRAY_HOME/harness-runs/summary.jsonl}"
exec python3 - "$SUMMARY" "$GRAY_HOME" <<'PY'
import json, os, sys

summary, home = sys.argv[1:3]


def add(acc, usage):
    for k, v in usage.items():
        if isinstance(v, (int, float)):
            acc[k] = acc.get(k, 0) + v


def chars(x, acc):
    if isinstance(x, str):
        return acc + len(x)
    if isinstance(x, dict):
        for v in x.values():
            acc = chars(v, acc)
    elif isinstance(x, list):
        for v in x:
            acc = chars(v, acc)
    return acc


totals, runs = {}, 0
for line in open(summary):
    line = line.strip()
    if not line:
        continue
    r = json.loads(line)
    runs += 1
    sid = r.get("session_id") or ""
    usage, est, model = {}, 0, r.get("model")
    sess = os.path.join(home, "sessions", sid + ".jsonl")
    if sid and os.path.exists(sess):
        with open(sess) as f:
            for l in f:
                try:
                    o = json.loads(l)
                except Exception:
                    continue
                if model is None and o.get("model"):
                    model = o["model"]
                if isinstance(o.get("usage"), dict):
                    add(usage, o["usage"])
                est = chars(o.get("message") or o, est)
    add(totals, usage)
    row = {"task": r.get("task"), "pass": r.get("pass"),
           "harness": r.get("harness"), "model": model,
           "session_id": sid, "usage": usage}
    if not usage and est:
        row["usage_est"] = est // 4
    print(json.dumps(row))
print(json.dumps({"totals": True, "runs": runs, "usage": totals}))
PY
