#!/bin/bash
# lane2 modal dig: tmux TUI, isolated GRAY_HOME, ANSI captures under dogfood/runs/lane2_*
# Usage: lane2_dig.sh <RUN_DIR>
set -u
RUN_DIR="${1:?run dir}"
GRAY_BIN="/home/vstaln/gray/target/debug/gray"
mkdir -p "$RUN_DIR"
PASS=0; FAIL=0
log() { echo "[lane2] $*"; }

cap() { # sess label
  tmux capture-pane -e -p -t "$1" > "$RUN_DIR/$2.ansi" 2>/dev/null || echo "capture-fail $2" >&2
}

# dig_one <name> <seed_json_or_empty> <slash_cmd> <open_oracle> [blackhole]
dig_one() {
  local name="$1" seed="$2" cmd="$3" oracle="$4" blackhole="${5:-0}"
  local t0=$(date +%s)
  local gh=$(mktemp -d /tmp/lane2_gray_XXXXXX)
  local wd="$RUN_DIR/work_$name"; mkdir -p "$wd"
  if [ -n "$seed" ]; then echo "$seed" > "$gh/config.json"; fi
  local sess="lane2_$name"
  tmux kill-session -t "$sess" 2>/dev/null
  local envpre="GRAY_HOME=$gh GRAY_NO_UPDATE_CHECK=1 TERM=xterm-256color"
  if [ "$blackhole" = "1" ]; then
    envpre="$envpre http_proxy=http://127.0.0.1:9 https_proxy=http://127.0.0.1:9 HTTP_PROXY=http://127.0.0.1:9 HTTPS_PROXY=http://127.0.0.1:9"
  fi
  # shellcheck disable=SC2086
  tmux new-session -d -s "$sess" -x 120 -y 30 "env $envpre $GRAY_BIN" 2>&1
  sleep 3
  cap "$sess" "${name}_boot"
  # dismiss onboarding picker if present
  tmux send-keys -t "$sess" Escape; sleep 1
  cap "$sess" "${name}_prompt"
  # open modal via slash cmd
  tmux send-keys -t "$sess" "$cmd"; sleep 0.5
  tmux send-keys -t "$sess" Enter; sleep 2
  cap "$sess" "${name}_open"
  local opened=0
  if grep -qF "$oracle" "$RUN_DIR/${name}_open.ansi" 2>/dev/null; then opened=1; fi
  # navigate
  tmux send-keys -t "$sess" Down; sleep 0.6
  tmux send-keys -t "$sess" Down; sleep 0.6
  cap "$sess" "${name}_moved"
  tmux send-keys -t "$sess" Up; sleep 0.5
  # filter
  if [ "$name" = "skills" ]; then
    tmux send-keys -t "$sess" Tab; sleep 0.8
    cap "$sess" "${name}_tab"
    tmux send-keys -t "$sess" Tab; sleep 0.5
  else
    tmux send-keys -t "$sess" "zzzqxj"; sleep 1
    cap "$sess" "${name}_filtered"
    # clear filter
    for _ in 1 2 3 4 5 6; do tmux send-keys -t "$sess" BSpace; sleep 0.15; done
    sleep 0.5
  fi
  # cancel
  tmux send-keys -t "$sess" Escape; sleep 1.5
  cap "$sess" "${name}_closed"
  # tiny terminal
  # reopen first so tiny captures the modal itself
  tmux send-keys -t "$sess" "$cmd"; sleep 0.5
  tmux send-keys -t "$sess" Enter; sleep 2
  tmux resize-window -t "$sess" -x 60 -y 16; sleep 1
  cap "$sess" "${name}_tiny"
  tmux resize-window -t "$sess" -x 120 -y 30; sleep 1
  tmux send-keys -t "$sess" Escape; sleep 1
  # garbage direct arg (where applicable)
  cap "$sess" "${name}_pre_garbage"
  local t1=$(date +%s)
  echo "$((t1-t0))" > "$RUN_DIR/${name}_wallsecs.txt"
  if [ -f "$gh/logs/gray.log" ]; then cp "$gh/logs/gray.log" "$RUN_DIR/${name}_gray.log"; fi
  tmux kill-session -t "$sess" 2>/dev/null
  rm -rf "$gh"
  if [ "$opened" = "1" ]; then echo "PASS $name"; else echo "FAIL $name (oracle '$oracle' missing)"; fi
}

log "run dir: $RUN_DIR"
dig_one "onboarding" "" "" "Connect a provider"
dig_one "model" "" "/model" "Select model"
dig_one "thinking_spark" '{"model":"muse-spark-1.3-contributor"}' "/thinking" "Thinking effort" 1
dig_one "thinking_gpt56" '{"model":"openai/gpt-5.6-sol"}' "/thinking" "Thinking effort" 1
dig_one "provider" "" "/connect" "Connect a provider"
dig_one "context" "" "/context" "Context Usage"
dig_one "permissions" "" "/permissions" "Permissions"
dig_one "skills" "" "/skills" "Skills"
dig_one "resume" "" "/resume" "Resume session"
log "dig batch done"
ls "$RUN_DIR" | head -n 60
