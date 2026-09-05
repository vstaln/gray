#!/bin/sh
# Test stub: answers plugin/manifest and tool/call with canned JSON.
while IFS= read -r line; do
  case "$line" in
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"echo","version":"0.1.0","tools":["echo"]}}\n' "$id"
      ;;
    *tool/call*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"content":"hi","is_error":false}}\n' "$id"
      ;;
  esac
done
