#!/bin/sh
d="$1/joined.csv"
[ -f "$d" ] || { echo "missing joined.csv"; exit 1; }
printf 'id,name,total\n1,ann,10\n2,bob,20\n3,cat,30\n' > "$1/.expected.csv"
diff -u "$1/.expected.csv" "$d" || exit 1
echo PASS
