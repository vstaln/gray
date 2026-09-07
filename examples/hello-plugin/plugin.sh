#!/bin/sh
# hello-plugin — minimal gray sidecar (protocol v1.1).
# One `hello` tool + one `/hello` command. Replies are static strings so
# every response is valid JSON even when tool args are empty.
# Wire spec: docs/plugins.md. Verify with: gray plugin check ./examples/hello-plugin
while IFS= read -r line; do
  case "$line" in
    *plugin/shutdown*)
      exit 0
      ;;
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"hello-plugin","version":"0.1.0","protocol":"1.1","tools":[{"name":"hello","description":"Say hello back","parameters":{"type":"object","properties":{},"required":[]},"snippet":"hello"}],"commands":["/hello"],"hooks":[],"capabilities":[],"subcommands":[]}}\n' "$id"
      ;;
    *command/run*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"text":"hello from hello-plugin"}}\n' "$id"
      ;;
    *tool/call*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"content":"hello from hello-plugin"}}\n' "$id"
      ;;
  esac
done
