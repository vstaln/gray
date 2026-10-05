#!/bin/sh
# prompt/context echo fixture: records each request line to $1
# so tests can assert exactly what the host sent (session.id, cwd).
while IFS= read -r line; do
  case "$line" in
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"session-echo","version":"0.1.0","protocol":"1.1","tools":[],"hooks":["prompt/context"]}}\n' "$id"
      ;;
    *prompt/context*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '%s\n' "$line" >> "$1"
      printf '{"id":%s,"result":{"text":"ok"}}\n' "$id"
      ;;
    *plugin/shutdown*) break ;;
  esac
done
