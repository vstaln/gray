#!/bin/sh
# usage: test.sh <workdir>; exit 0 on pass
d="$1/deduped.jsonl"
[ -f "$d" ] || { echo "missing deduped.jsonl"; exit 1; }
[ "$(wc -l < "$d")" -eq 3 ] || { echo "want 3 lines"; exit 1; }
grep -q '"id":"a","v":1' "$d" && grep -q '"id":"b","v":2' "$d" && grep -q '"id":"c","v":4' "$d" || { echo "wrong rows kept"; exit 1; }
echo PASS
