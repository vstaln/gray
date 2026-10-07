#!/bin/sh
# Protocol-1.3 fixture: an empty manifest tool list, and the FIRST
# `plugin/tools` reply is immediately followed by a `host/tools_changed`
# notification — the window in which a handler installed only after the
# initial refresh would drop it. Later `plugin/tools` replies add
# `early_b`, so a host that caught the notification ends with
# [early_a, early_b].
n=0
while IFS= read -r line; do
  case "$line" in
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"early","version":"0.1.0","protocol":"1.3","tools":[]}}\n' "$id"
      ;;
    *plugin/tools*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      n=$((n + 1))
      if [ "$n" -eq 1 ]; then
        printf '{"id":%s,"result":{"tools":[{"name":"early_a","description":"a","parameters":{"type":"object","properties":{}}}]}}\n' "$id"
        printf '{"method":"host/tools_changed","params":{"reason":"test"}}\n'
      else
        printf '{"id":%s,"result":{"tools":[{"name":"early_a","description":"a","parameters":{"type":"object","properties":{}}},{"name":"early_b","description":"b","parameters":{"type":"object","properties":{}}}]}}\n' "$id"
      fi
      ;;
    *plugin/shutdown*) break ;;
  esac
done
