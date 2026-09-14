#!/bin/sh
d="$1/config.json"
[ -f "$d" ] || { echo "missing config.json"; exit 1; }
python3 - "$d" <<'PY'
import json,sys
v=json.load(open(sys.argv[1]))
assert v=={"name":"gray","port":8080,"debug":True,"limits":{"cpu":2,"mem":512},"tags":["fast","local"]}, v
print("PASS")
PY
