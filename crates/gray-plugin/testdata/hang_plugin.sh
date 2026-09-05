#!/bin/sh
# Hanging stub: answers manifest, never replies otherwise.
while IFS= read -r line; do
  case "$line" in
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"hang","version":"0.1.0","tools":["hang"]}}\n' "$id"
      ;;
  esac
done
