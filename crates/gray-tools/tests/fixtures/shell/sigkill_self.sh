#!/bin/sh
# sigkill_self.sh: dies by SIGKILL after 2 lines (137/SIGKILL + OOM-note fixture).
# Canonical V2 name for what WP0's oom.sh already covers; oom.sh is kept as-is.
echo "sigkill line 1"
echo "sigkill line 2"
kill -9 $$
