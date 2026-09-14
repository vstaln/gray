#!/bin/sh
d="$1/report.txt"
[ -f "$d" ] || { echo "missing report.txt"; exit 1; }
grep -qx "lines: 2" "$d" || { echo "want 'lines: 2'"; exit 1; }
grep -qx "words: 5" "$d" || { echo "want 'words: 5'"; exit 1; }
echo PASS
