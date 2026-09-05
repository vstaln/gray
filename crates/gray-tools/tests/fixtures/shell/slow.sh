#!/bin/sh
# slow.sh: one line per second for 90s (timeout / auto-promotion fixture).
# The baseline test caps it with timeout=10, so only the first ~10 lines run.
i=1
while [ "$i" -le 90 ]; do
  echo "tick $i"
  sleep 1
  i=$((i + 1))
done
