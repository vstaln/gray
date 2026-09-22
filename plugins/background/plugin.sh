#!/bin/sh
# gray-background — background runner sidecar (protocol v1). SCAFFOLD.
# NDJSON over stdio: every request carries an "id"; reply {"id":N,"result":{...}}.
# TODO(author): port the real runner from your dev folder here. Until then
# this answers the manifest + check handshake.
while IFS= read -r line; do
  case "$line" in
    *plugin/shutdown*)
      exit 0
      ;;
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"background","version":"0.1.0","protocol":"1.1","tools":[{"name":"bg_run","description":"Run a command in the background (scaffold: not wired)","parameters":{"type":"object","properties":{"command":{"type":"string"}},"required":["command"]},"snippet":"bg_run <command>"}],"commands":["/bg"]}}\n' "$id"
      ;;
    *command/run*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"text":"background runner scaffold: no jobs yet."}}\n' "$id"
      ;;
    *tool/call*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"content":"bg_run scaffold: runner not wired yet.","is_error":true}}\n' "$id"
      ;;
  esac
done
