#!/bin/sh
# sigterm_self.sh: dies by SIGTERM after 2 lines (143/SIGTERM honesty fixture).
echo "sigterm line 1"
echo "sigterm line 2"
kill -15 $$
