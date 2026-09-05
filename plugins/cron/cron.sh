#!/bin/sh
# STUB (Task 3 p2-6): cron as an out-of-process sidecar plugin.
#
# Intended shape: the scheduler lives here, keeps its own flat-file store,
# and fires due jobs via `host/run` (see docs/plugins.md "cron port").
# Until the host wires a real `host/run` runner, every schedule operation
# fails LOUDLY (is_error content, never silent) and in-process cron
# (`gray-cron`, gateway `run_cron_job`) remains the live path.
# `event/notify` has no id and gets no reply (ignored below like all unknowns).
STUB='cron plugin stub: out-of-process scheduler not yet cut over (see docs/plugins.md "cron port"); in-process gray-cron is still live'
while IFS= read -r line; do
  case "$line" in
    *plugin/shutdown*)
      exit 0
      ;;
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"cron","version":"0.1.0","protocol":"1.1","capabilities":["session"],"subcommands":["/cron"],"tools":[{"name":"cron.add","description":"Schedule a prompt (STUB)","parameters":{"type":"object"},"snippet":"cron.add <prompt>"},{"name":"cron.list","description":"List jobs (STUB)","parameters":{"type":"object"},"snippet":"cron.list"},{"name":"cron.remove","description":"Remove a job (STUB)","parameters":{"type":"object"},"snippet":"cron.remove <id>"}],"commands":[],"hooks":[]}}\n' "$id"
      ;;
    *command/run*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"text":"%s"}}\n' "$id" "$STUB"
      ;;
    *tool/call*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      # JSON-escape is overkill for a fixed stub string (no quotes in it).
      printf '{"id":%s,"result":{"content":"%s","is_error":true}}\n' "$id" "$STUB"
      ;;
  esac
done
