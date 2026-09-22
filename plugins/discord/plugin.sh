#!/bin/sh
# graydiscord — Discord bridge sidecar (protocol v1). SCAFFOLD.
# NDJSON over stdio: every request carries an "id"; reply {"id":N,"result":{...}}.
# `event/notify` has no id and gets no reply (ignored below like all unknowns).
#
# TODO(author): port the real bridge from your dev folder here. Wiring notes:
# - Bot token lives in ~/.config/gray-discord/config.json (that is what the
#   gateway setup panel probes). Never log or echo it.
# - /discord should report link status; discord_send should POST to the
#   Discord API and stream replies back as the tool result.
# Until then this answers the manifest + check handshake so
# `gray plugin check` and `gray plugin install` pass.
while IFS= read -r line; do
  case "$line" in
    *plugin/shutdown*)
      exit 0
      ;;
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"discord","version":"0.1.0","protocol":"1.1","tools":[{"name":"discord_send","description":"Send a message to the linked Discord channel (scaffold: not wired)","parameters":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]},"snippet":"discord_send <text>"}],"commands":["/discord"]}}\n' "$id"
      ;;
    *command/run*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"text":"discord bridge scaffold: link your bot token in ~/.config/gray-discord/config.json, then replace this handler with the real bridge."}}\n' "$id"
      ;;
    *tool/call*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"content":"discord_send scaffold: bridge not wired yet.","is_error":true}}\n' "$id"
      ;;
  esac
done
