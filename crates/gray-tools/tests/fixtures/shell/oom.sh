#!/bin/sh
# oom.sh: dies by SIGKILL after 2 lines (signal-honesty fixture).
echo "oom line 1"
echo "oom line 2"
kill -9 $$
