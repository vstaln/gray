#!/bin/sh
# Crash stub: answers manifest, exits 1 on second call.
n=0
while IFS= read -r line; do
  case "$line" in
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"crash","version":"0.1.0","tools":["crash"]}}\n' "$id"
      ;;
    *tool/call*)
      n=$((n+1))
      if [ "$n" -ge 2 ]; then exit 1; fi
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"content":"once","is_error":false}}\n' "$id"
      ;;
    *event/notify*)
      n=$((n+1))
      if [ "$n" -ge 2 ]; then exit 1; fi
      ;;
  esac
done
