#!/bin/sh
# Out-of-order stub: answers manifest; holds the first tool/call reply until
# the second arrives, then replies second-first (exercises id routing).
held=""
while IFS= read -r line; do
  case "$line" in
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"reorder","version":"0.1.0","tools":["reorder"]}}\n' "$id"
      ;;
    *tool/call*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
      n=$(printf '%s' "$line" | sed 's/.*"n":"\([^"]*\)".*/\1/')
      if [ -z "$held" ]; then
        held="$id:$n"
      else
        printf '{"id":%s,"result":{"content":"%s"}}\n' "$id" "$n"
        hid=${held%%:*}; hn=${held#*:}
        printf '{"id":%s,"result":{"content":"%s"}}\n' "$hid" "$hn"
        held=""
      fi
      ;;
  esac
done
