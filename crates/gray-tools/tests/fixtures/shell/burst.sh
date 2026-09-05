#!/bin/sh
# burst.sh: 300k lines as fast as possible (baseline for truncation).
# awk keeps this under a few seconds; a POSIX while-loop would take minutes.
awk 'BEGIN { for (i = 1; i <= 300000; i++) print "line " i " payload 0123456789 abcdefghij" }'
