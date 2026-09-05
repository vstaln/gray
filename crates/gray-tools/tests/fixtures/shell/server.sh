#!/bin/sh
# server.sh: listen on $PORT until killed (background + kill-by-port fixture).
# The baseline test caps it with timeout=10.
PORT="${PORT:-18080}"
if command -v python3 >/dev/null 2>&1; then
  exec python3 -m http.server "$PORT"
fi
# Fallback: hold the task alive without a listener.
while true; do sleep 1; done
