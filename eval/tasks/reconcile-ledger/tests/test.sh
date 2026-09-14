#!/bin/sh
d="$1/balances.json"
[ -f "$d" ] || { echo "missing balances.json"; exit 1; }
python3 - "$d" <<'PY'
import json,sys
b=json.load(open(sys.argv[1]))
assert b=={"cash":80,"food":-40}, b
print("PASS")
PY
