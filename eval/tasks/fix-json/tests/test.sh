#!/bin/sh
d="$1/fixed.json"
[ -f "$d" ] || { echo "missing fixed.json"; exit 1; }
python3 - "$d" <<'PY'
import json,sys
v=json.load(open(sys.argv[1]))
assert v=={"a":1,"b":[1,2,3]}, v
print("PASS")
PY
