#!/bin/sh
d="$1/errors.log"
[ -f "$d" ] || { echo "missing errors.log"; exit 1; }
[ "$(wc -l < "$d")" -eq 2 ] || { echo "want 2 lines"; exit 1; }
grep -q "ERROR disk full" "$d" && grep -q "ERROR timeout on read" "$d" || { echo "wrong lines"; exit 1; }
grep -q "ERRORREF" "$d" && { echo "leaked ERRORREF"; exit 1; }
grep -q "^INFO\|^WARN" "$d" && { echo "leaked non-error"; exit 1; }
echo PASS
