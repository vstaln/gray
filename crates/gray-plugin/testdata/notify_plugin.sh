#!/bin/sh
# Notification fixture: after the manifest reply, emits one id-less
# host/* frame (a plugin->host notification) so tests can assert the
# transport routes it to the notify handler instead of dropping it.
while IFS= read -r line; do
  case "$line" in
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"notify","version":"0.1.0","protocol":"1.3","tools":[]}}\n' "$id"
      printf '{"method":"host/tools_changed","params":{"reason":"test"}}\n'
      ;;
    *plugin/shutdown*) break ;;
  esac
done
