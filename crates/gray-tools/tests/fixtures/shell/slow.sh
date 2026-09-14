#!/bin/sh
# slow.sh: one line per second for 90s (timeout / cancel fixture).
# Tests cap it with timeout=1 or cancel, so only the first lines run.
i=1
while [ "$i" -le 90 ]; do
  echo "tick $i"
  sleep 1
  i=$((i + 1))
done
