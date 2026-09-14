#!/bin/sh
# gray reference sidecar (protocol v1): one `echo` tool + `/echo` command.
# NDJSON over stdio: every request carries an "id"; reply {"id":N,"result":{...}}.
# `event/notify` has no id and gets no reply (ignored below like all unknowns).
while IFS= read -r line; do
  case "$line" in
    *plugin/shutdown*)
      exit 0
      ;;
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"echo","version":"0.1.0","protocol":"1.1","tools":[{"name":"echo","description":"Echo text back","parameters":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]},"snippet":"echo <text>"}],"commands":["/echo"],"hooks":["turn/end"]}}\n' "$id"
      ;;
    *command/run*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      argv=$(printf '%s' "$line" | sed 's/.*"argv":\[//; s/\].*//; s/"//g; s/,/ /g')
      printf '{"id":%s,"result":{"text":"%s"}}\n' "$id" "$argv"
      ;;
    *tool/call*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      # ponytail: empty when no "text" arg (was: echoed the whole request line,
      # invalid JSON -> host hung to timeout); escape for JSON. Ceiling: sed-level
      # parse, text with embedded quotes truncates (reference fixture only).
      text=$(printf '%s' "$line" | sed -n 's/.*"text"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
      esc=$(printf '%s' "$text" | sed 's/\\/\\\\/g; s/"/\\"/g')
      printf '{"id":%s,"result":{"content":"%s"}}\n' "$id" "$esc"
      ;;
  esac
done
