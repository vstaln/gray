#!/bin/sh
d="$1/settings.toml"
[ -f "$d" ] || { echo "missing settings.toml"; exit 1; }
grep -qx 'port = 9090' "$d" || { echo "port not patched"; exit 1; }
grep -qx '# service settings' "$d" || { echo "header changed"; exit 1; }
grep -qx 'host = "localhost"' "$d" || { echo "host changed"; exit 1; }
grep -qx 'workers = 4  # do not touch' "$d" || { echo "workers line changed"; exit 1; }
[ "$(wc -l < "$d")" -eq 4 ] || { echo "line count changed"; exit 1; }
echo PASS
