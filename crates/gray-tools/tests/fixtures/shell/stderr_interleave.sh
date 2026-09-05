#!/bin/sh
# stderr_interleave.sh: alternate stdout/stderr lines (arrival-order fixture).
# The pump must preserve both streams with neither starved.
i=1
while [ "$i" -le 20 ]; do
  echo "out $i"
  echo "err $i" >&2
  i=$((i + 1))
done
