#!/bin/sh
# utf8_split.sh: a 4-byte codepoint straddling the 8 KiB pump-read boundary.
# 8191 "A"s put the emoji (F0 9F 98 80 = U+1F600, octal \360\237\230\200)
# at bytes 8191-8194, crossing the first 8 KiB chunk edge. POSIX printf
# octal escapes only — no awk, no python.
head -c 8191 /dev/zero | tr '\0' 'A'
printf '\360\237\230\200\n'
i=1
while [ "$i" -le 100 ]; do
  echo "tail line $i"
  i=$((i + 1))
done
