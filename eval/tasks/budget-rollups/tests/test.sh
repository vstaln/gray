#!/bin/sh
d="$1/rollup.csv"
[ -f "$d" ] || { echo "missing rollup.csv"; exit 1; }
grep -qx "dept,total" "$d" >/dev/null || head -1 "$d" | grep -qi dept || { echo "bad header"; exit 1; }
grep -q "^eng,125" "$d" || { echo "want eng,125"; exit 1; }
grep -q "^ops,125" "$d" || { echo "want ops,125"; exit 1; }
echo PASS
