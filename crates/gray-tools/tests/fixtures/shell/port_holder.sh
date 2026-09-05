#!/bin/sh
# port_holder.sh PORT: listen on $1 until killed (kill-by-port fixture).
# Usage: sh port_holder.sh 38471  (ephemeral port passed in, never hardcoded)
PORT="${1:-18080}"
if command -v python3 >/dev/null 2>&1; then
  exec python3 -m http.server "$PORT" --bind 127.0.0.1
fi
if command -v nc >/dev/null 2>&1; then
  # BSD and GNU nc differ (`-l PORT` vs `-l -p PORT`); try plain first.
  nc -l "$PORT" 2>/dev/null || nc -l -p "$PORT" 2>/dev/null || true
  exit 0
fi
# Fallback: hold the task alive without a listener.
while true; do sleep 1; done
