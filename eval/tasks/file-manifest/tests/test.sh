#!/bin/sh
d="$1/manifest.txt"
[ -f "$d" ] || { echo "missing manifest.txt"; exit 1; }
a_size="$(wc -c < "$1/a.txt" | tr -d ' ')"
b_size="$(wc -c < "$1/b.txt" | tr -d ' ')"
printf 'a.txt %s\nb.txt %s\n' "$a_size" "$b_size" > "$1/.expected.txt"
diff -u "$1/.expected.txt" "$d" || exit 1
echo PASS
