#!/bin/sh
# spew.sh N: print N lines fast (middle-out truncation fixture).
# Usage: sh spew.sh 30000
n="${1:-1000}"
i=1
while [ "$i" -le "$n" ]; do
  echo "spew line $i payload 0123456789 abcdefghij"
  i=$((i + 1))
done
