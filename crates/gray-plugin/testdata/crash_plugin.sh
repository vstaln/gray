#!/bin/sh
# Crash stub: answers manifest, exits 1 on first tool/call.
while IFS= read -r line; do
  case "$line" in
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"crash","version":"0.1.0","tools":["crash"]}}\n' "$id"
      ;;
    *tool/call*)
      exit 1
      ;;
    *event/notify*)
      exit 1
      ;;
  esac
done
