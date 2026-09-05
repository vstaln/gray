#!/bin/sh
# v1 stub: manifest with schema+snippet tool, /echo command, all hooks;
# prompt/context -> text, tool/before -> allow, command/run -> joined argv.
while IFS= read -r line; do
  case "$line" in
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"hooks","version":"0.1.0","tools":[{"name":"shout","description":"Shout text","parameters":{"type":"object"},"snippet":"shout <text>"}],"commands":["/echo"],"hooks":["prompt/context","tool/before","turn/end"]}}\n' "$id"
      ;;
    *prompt/context*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"text":"CTX"}}\n' "$id"
      ;;
    *tool/before*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
      case "$line" in
        *blocked*) printf '{"id":%s,"result":{"decision":"deny","reason":"BLOCKED-BY-E2E"}}\n' "$id" ;;
        *) printf '{"id":%s,"result":{"decision":"allow"}}\n' "$id" ;;
      esac
      ;;
    *command/run*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
      argv=$(printf '%s' "$line" | sed 's/.*"argv":\[//; s/\].*//; s/"//g; s/,/ /g')
      printf '{"id":%s,"result":{"text":"%s"}}\n' "$id" "$argv"
      ;;
    *tool/call*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"content":"hi"}}\n' "$id"
      ;;
  esac
done
