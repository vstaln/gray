#!/bin/sh
# Protocol-1.3 fixture: an empty manifest tool list, a live `plugin/tools`
# answer that grows after the first `tool/call`, and a `host/tools_changed`
# notification that tells the host to re-ask. $1 = flag file path.
flag="$1"
while IFS= read -r line; do
  case "$line" in
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"dyn","version":"0.1.0","protocol":"1.3","tools":[]}}\n' "$id"
      ;;
    *plugin/tools*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      if [ -e "$flag" ]; then
        printf '{"id":%s,"result":{"tools":[{"name":"dyn_a","description":"a","parameters":{"type":"object","properties":{}}},{"name":"dyn_b","description":"b","parameters":{"type":"object","properties":{}}}]}}\n' "$id"
      else
        printf '{"id":%s,"result":{"tools":[{"name":"dyn_a","description":"a","parameters":{"type":"object","properties":{}}}]}}\n' "$id"
      fi
      ;;
    *tool/call*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      : > "$flag"
      printf '{"method":"host/tools_changed","params":{}}\n'
      printf '{"id":%s,"result":{"content":"called"}}\n' "$id"
      ;;
    *plugin/shutdown*) break ;;
  esac
done
