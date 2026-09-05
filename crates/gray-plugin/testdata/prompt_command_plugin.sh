#!/bin/sh
# prompt-command fixture (v1.1): manifest claims `/ask`;
# `command/run` replies with the `{"prompt":...}` variant.
while IFS= read -r line; do
  case "$line" in
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"promptcmd","version":"0.1.0","protocol":"1.1","tools":[],"commands":["/ask"],"hooks":[]}}\n' "$id"
      ;;
    *plugin/shutdown*)
      exit 0
      ;;
    *command/run*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"prompt":"hello from plugin"}}\n' "$id"
      ;;
  esac
done
