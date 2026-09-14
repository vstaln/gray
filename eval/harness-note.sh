#!/bin/sh
# Join any existing session into the ledger: fabricates nothing, only links.
# Usage: ./harness-note.sh <session-id> [task] [pass:true|false] [note]
set -u
sid="${1:?session id}"; task="${2:-manual}"; pass="${3:-}"; note="${4:-}"
HARNESS_DIR="${GRAY_HOME:-$HOME/.gray}/harness-runs"
mkdir -p "$HARNESS_DIR/$sid"
S="$HARNESS_DIR/$sid"
SESS="${GRAY_HOME:-$HOME/.gray}/sessions/$sid.jsonl"
AGENTS_MD="${GRAY_HOME:-$HOME/.gray}/AGENTS.md"
python3 - "$S/manifest.json" "$sid" "$task" "$pass" "$note" "$SESS" "$AGENTS_MD" <<'PY'
import json,sys,os,hashlib,time,pathlib
out, sid, task, passed, note, sess, agents = sys.argv[1:8]
manifest = {
  "session_id": sid,
  "task": task,
  "ts": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
  "session_exists": os.path.exists(sess),
  "agents_md_sha8": hashlib.sha256(pathlib.Path(agents).read_bytes()).hexdigest()[:8] if os.path.exists(agents) else None,
}
if passed in ("true","false"):
    manifest["pass"] = (passed=="true")
if note:
    manifest["note"] = note
json.dump(manifest, open(out,"w"), indent=2)
print(out)
PY
[ -n "$pass" ] && { ts="$(date -u +%Y-%m-%dT%H:%M:%SZ)"; python3 -c 'import json,sys; print(json.dumps({"ts":sys.argv[1],"task":sys.argv[2],"pass":sys.argv[3]=="true","session_id":sys.argv[4],"note":sys.argv[5]}))' "$ts" "$task" "$pass" "$sid" "$note" >> "$HARNESS_DIR/summary.jsonl"; echo "ledger appended"; }
echo "template: keep failure notes <=30 lines:"
echo "$S/report.md (create only on failure)"
