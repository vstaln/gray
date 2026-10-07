#!/bin/sh
# Notification fixture: a static protocol-1.1 sidecar with one tool,
# `ping`. On `tool/call` it emits one id-less host/* frame (a plugin->host
# notification) BEFORE the tool reply on the same stdout stream, so tests
# can assert the transport routes it to the notify handler instead of
# dropping it. Nothing is sent at manifest time beyond the reply.
while IFS= read -r line; do
  case "$line" in
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"notify","version":"0.1.0","protocol":"1.1","tools":[{"name":"ping","description":"x","parameters":{"type":"object","properties":{}}}]}}\n' "$id"
      ;;
    *tool/call*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"method":"host/tools_changed","params":{"reason":"test"}}\n'
      printf '{"id":%s,"result":{"content":"pong"}}\n' "$id"
      ;;
    *plugin/shutdown*) break ;;
  esac
done
