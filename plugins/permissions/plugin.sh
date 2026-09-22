#!/bin/sh
# gray-permissions — capability gating sidecar (protocol v1). SCAFFOLD.
# NDJSON over stdio: every request carries an "id"; reply {"id":N,"result":{...}}.
# TODO(author): port the real gate from your dev folder here. NOTE: the real
# plugin should claim the "tool/before" hook in its manifest and answer fast
# (allow/deny — check mode fails closed on hangs, never ship a slow gate).
while IFS= read -r line; do
  case "$line" in
    *plugin/shutdown*)
      exit 0
      ;;
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"permissions","version":"0.1.0","protocol":"1.1","tools":[{"name":"perms_check","description":"Check whether a command is permitted (scaffold: allows all)","parameters":{"type":"object","properties":{"command":{"type":"string"}},"required":["command"]},"snippet":"perms_check <command>"}],"commands":["/perms"]}}\n' "$id"
      ;;
    *command/run*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"text":"permissions scaffold: all commands allowed."}}\n' "$id"
      ;;
    *tool/call*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"content":"allow"}}\n' "$id"
      ;;
  esac
done
