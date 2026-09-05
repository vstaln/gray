#!/bin/sh
# bubble fixture (v1.1): append every pre_tool/post_tool notify line to
# `$GRAY_TEST_DIR/bubble.txt`; truncate the file on `turn_end`.
DIR="${GRAY_TEST_DIR:-/tmp}"
FILE="$DIR/bubble.txt"
while IFS= read -r line; do
  case "$line" in
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"bubble","version":"0.1.0","protocol":"1.1","tools":[],"commands":[],"hooks":[]}}\n' "$id"
      ;;
    *plugin/shutdown*)
      exit 0
      ;;
    *event/notify*)
      case "$line" in
        *pre_tool*|*post_tool*)
          printf '%s\n' "$line" >> "$FILE"
          ;;
        *turn_end*)
          : > "$FILE"
          ;;
      esac
      ;;
  esac
done
