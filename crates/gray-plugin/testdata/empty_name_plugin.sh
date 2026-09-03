#!/bin/sh
# Empty-name stub: manifest reply with missing name must make spawn bail.
while IFS= read -r line; do
  case "$line" in
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"version":"0.1.0","tools":[]}}\n' "$id"
      ;;
  esac
done
