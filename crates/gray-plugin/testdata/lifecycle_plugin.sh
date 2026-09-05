#!/bin/sh
# lifecycle fixture (v1.1): fake obscura-serve pattern.
# On startup spawns `sleep 300`, writes fake endpoint
# `ws://127.0.0.1:<pid-derived-port>` to `$GRAY_TEST_DIR/endpoint-<pid>`,
# replies to `prompt/context` with that endpoint text.
# On `plugin/shutdown` kills the child and removes the file, then exits
# so the host wait succeeds. Tolerates pre-v1 hosts (shutdown line may
# never come — then we die via SIGKILL on drop like any sidecar child).
DIR="${GRAY_TEST_DIR:-/tmp}"
sleep 300 &
CHILD=$!
PORT=$((20000 + CHILD % 30000))
ENDPOINT="ws://127.0.0.1:$PORT"
FILE="$DIR/endpoint-$CHILD"
printf '%s' "$ENDPOINT" > "$FILE"
cleanup() {
  kill "$CHILD" 2>/dev/null
  rm -f "$FILE"
}
while IFS= read -r line; do
  case "$line" in
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"lifecycle","version":"0.1.0","protocol":"1.1","tools":[],"commands":[],"hooks":["prompt/context"]}}\n' "$id"
      ;;
    *prompt/context*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"text":"%s"}}\n' "$id" "$ENDPOINT"
      ;;
    *plugin/shutdown*)
      cleanup
      exit 0
      ;;
  esac
done
cleanup
