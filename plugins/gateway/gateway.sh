#!/bin/sh
# gateway sidecar: answers `/gateway` over `command/run` by delegating argv
# to the existing `gray gateway …` CLI (status|install|uninstall|pairing|invite).
# NDJSON over stdio, same framing as plugins/echo/echo.sh: every request
# carries an "id"; reply {"id":N,"result":{...}}. `event/notify` has no id
# and gets no reply (ignored below like all unknowns).
# `gray` resolves via PATH, else the workspace build tree (target/ below —
# same shape as plugins/echo/echo.sh); without it `command/run` answers
# carry the miss as text and the manifest still serves.
gray_bin() {
  if command -v gray >/dev/null 2>&1; then
    command -v gray
    return 0
  fi
  HERE=$(dirname "$0")
  for c in "$HERE/../../target/debug/gray" "$HERE/../../target/release/gray"; do
    if [ -x "$c" ]; then
      printf '%s' "$c"
      return 0
    fi
  done
  return 1
}

json_escape() {
  sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e ':a' -e 'N' -e '$!ba' \
      -e 's/\n/\\n/g' -e 's/\r/\\r/g' -e 's/\t/\\t/g'
}

VERSION=$(git describe --tags --always --dirty 2>/dev/null || printf '0.1.0')

while IFS= read -r line; do
  case "$line" in
    *plugin/shutdown*)
      exit 0
      ;;
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"gateway","version":"%s","protocol":"1.1","commands":["/gateway"],"capabilities":["exec"]}}\n' "$id" "$VERSION"
      ;;
    *command/run*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      argv=$(printf '%s' "$line" | sed 's/.*"argv":\[//; s/\].*//; s/"//g; s/,/ /g')
      case "$argv" in
        # `gateway run` is the foreground daemon — it would outlive the 30 s
        # command/run TTL, so refuse fast instead of hanging the turn.
        run|"run "*)
          text="refusing 'gateway run' (foreground daemon): start it outside the REPL via 'gray gateway run'"
          ;;
        *)
          if GRAY=$(gray_bin); then
            # shellcheck disable=SC2086: argv splitting is intentional here.
            text=$($GRAY gateway $argv 2>&1)
          else
            text="gateway sidecar: gray not found (PATH, or cargo build in the workspace)"
          fi
          ;;
      esac
      esc=$(printf '%s' "$text" | json_escape)
      printf '{"id":%s,"result":{"text":"%s"}}\n' "$id" "$esc"
      ;;
  esac
done
