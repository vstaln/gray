#!/bin/bash
# lane2 dig part 2: garbage/select/resume-seeded/skills-seeded/context-edit/onboarding-filter
set -u
RUN_DIR="${1:?run dir}"
GRAY_BIN="/home/vstaln/gray/target/debug/gray"
cap() { tmux capture-pane -e -p -t "$1" > "$RUN_DIR/$2.ansi" 2>/dev/null; }
spawn() { # sess gh wd extra-env...
  local sess="$1" gh="$2" wd="$3"; shift 3
  tmux kill-session -t "$sess" 2>/dev/null
  mkdir -p "$wd"
  tmux new-session -d -s "$sess" -x 120 -y 30 "env GRAY_HOME=$gh GRAY_NO_UPDATE_CHECK=1 TERM=xterm-256color $* $GRAY_BIN" 2>&1
  sleep 3
}

# --- garbage batch: one session, sequential slash cmds ---
GH=$(mktemp -d /tmp/lane2_gray_XXXXXX); WD="$RUN_DIR/work_lane2_garbage"; SESS="lane2_garbage"
spawn "$SESS" "$GH" "$WD"
tmux send-keys -t "$SESS" Escape; sleep 1
for c in "/model nope-not-real-xyz-123" "/thinking garbage-level-xyz" "/context bogus-xyz" "/permissions garbage-xyz" "/skills:no-such-skill-xyz"; do
  safe=$(echo "$c" | tr '/: ' '__' | tr -cd 'A-Za-z0-9_')
  tmux send-keys -t "$SESS" "$c"; sleep 0.4; tmux send-keys -t "$SESS" Enter; sleep 3
  cap "$SESS" "garbage_$safe"
done
cp "$GH/logs/gray.log" "$RUN_DIR/garbage_gray.log" 2>/dev/null
tmux kill-session -t "$SESS" 2>/dev/null; rm -rf "$GH"

# --- select batch: model + thinking + permissions direct set ---
GH=$(mktemp -d /tmp/lane2_gray_XXXXXX); WD="$RUN_DIR/work_lane2_select"; SESS="lane2_select"
spawn "$SESS" "$GH" "$WD"
tmux send-keys -t "$SESS" Escape; sleep 1
# model: open, move down x1, Enter (select), capture config after
tmux send-keys -t "$SESS" "/model"; sleep 0.4; tmux send-keys -t "$SESS" Enter; sleep 2
cap "$SESS" "select_model_open"
tmux send-keys -t "$SESS" Down; sleep 0.6
tmux send-keys -t "$SESS" Enter; sleep 2
cap "$SESS" "select_model_done"
cat "$GH/config.json" > "$RUN_DIR/select_model_config.json" 2>/dev/null || echo "no-config" > "$RUN_DIR/select_model_config.json"
# thinking: open, Down, Enter
tmux send-keys -t "$SESS" "/thinking"; sleep 0.4; tmux send-keys -t "$SESS" Enter; sleep 2
cap "$SESS" "select_thinking_open"
tmux send-keys -t "$SESS" Down; sleep 0.6
tmux send-keys -t "$SESS" Enter; sleep 2
cap "$SESS" "select_thinking_done"
cat "$GH/config.json" > "$RUN_DIR/select_thinking_config.json" 2>/dev/null
# permissions direct
tmux send-keys -t "$SESS" "/permissions full"; sleep 0.4; tmux send-keys -t "$SESS" Enter; sleep 2
cap "$SESS" "select_perms_full"
cp "$GH/logs/gray.log" "$RUN_DIR/select_gray.log" 2>/dev/null
tmux kill-session -t "$SESS" 2>/dev/null; rm -rf "$GH"

# --- context edit mode ---
GH=$(mktemp -d /tmp/lane2_gray_XXXXXX); WD="$RUN_DIR/work_lane2_ctxedit"; SESS="lane2_ctxedit"
spawn "$SESS" "$GH" "$WD"
tmux send-keys -t "$SESS" Escape; sleep 1
tmux send-keys -t "$SESS" "/context"; sleep 0.4; tmux send-keys -t "$SESS" Enter; sleep 2
cap "$SESS" "ctxedit_open"
tmux send-keys -t "$SESS" Enter; sleep 1
cap "$SESS" "ctxedit_editing"
tmux send-keys -t "$SESS" "bogus!!"; sleep 0.5; tmux send-keys -t "$SESS" Enter; sleep 1
cap "$SESS" "ctxedit_invalid"
tmux send-keys -t "$SESS" Escape; sleep 1
cap "$SESS" "ctxedit_closed"
cp "$GH/logs/gray.log" "$RUN_DIR/ctxedit_gray.log" 2>/dev/null
tmux kill-session -t "$SESS" 2>/dev/null; rm -rf "$GH"

# --- skills seeded ---
GH=$(mktemp -d /tmp/lane2_gray_XXXXXX); WD="$RUN_DIR/work_lane2_skillsseed"; SESS="lane2_skillsseed"
mkdir -p "$GH/skills/lane2-nav/SKILL.md" 2>/dev/null; mkdir -p "$GH/skills/lane2-nav"
printf -- '---\nname: lane2-nav\ndescription: nav fixture skill\n---\nDo the nav thing.\n' > "$GH/skills/lane2-nav/SKILL.md"
export HOME="$GH"
spawn "$SESS" "$GH" "$WD"
tmux send-keys -t "$SESS" Escape; sleep 1
tmux send-keys -t "$SESS" "/skills"; sleep 0.4; tmux send-keys -t "$SESS" Enter; sleep 2
cap "$SESS" "skillsseed_open"
tmux send-keys -t "$SESS" Down; sleep 0.8
cap "$SESS" "skillsseed_moved"
tmux send-keys -t "$SESS" u; sleep 0.8
cap "$SESS" "skillsseed_armed"
tmux send-keys -t "$SESS" Escape; sleep 1
cap "$SESS" "skillsseed_closed"
cp "$GH/logs/gray.log" "$RUN_DIR/skillsseed_gray.log" 2>/dev/null
tmux kill-session -t "$SESS" 2>/dev/null; rm -rf "$GH"

# --- resume seeded: fake session via gray_session jsonl? minimal: create store dir w/ one summary ---
GH=$(mktemp -d /tmp/lane2_gray_XXXXXX); WD="$RUN_DIR/work_lane2_resume"; SESS="lane2_resume"
spawn "$SESS" "$GH" "$WD"
tmux send-keys -t "$SESS" Escape; sleep 1
tmux send-keys -t "$SESS" "/resume"; sleep 0.4; tmux send-keys -t "$SESS" Enter; sleep 3
cap "$SESS" "resume_empty"
tmux send-keys -t "$SESS" Escape; sleep 1
tmux send-keys -t "$SESS" "/resume --last"; sleep 0.4; tmux send-keys -t "$SESS" Enter; sleep 2
cap "$SESS" "resume_last_empty"
cp "$GH/logs/gray.log" "$RUN_DIR/resume2_gray.log" 2>/dev/null
tmux kill-session -t "$SESS" 2>/dev/null; rm -rf "$GH"

# --- onboarding filter path ---
GH=$(mktemp -d /tmp/lane2_gray_XXXXXX); WD="$RUN_DIR/work_lane2_onboard"; SESS="lane2_onboard"
spawn "$SESS" "$GH" "$WD"
sleep 1; cap "$SESS" "onboard_boot"
tmux send-keys -t "$SESS" Down; sleep 0.5; tmux send-keys -t "$SESS" Down; sleep 0.5
cap "$SESS" "onboard_moved"
tmux send-keys -t "$SESS" "deep"; sleep 1
cap "$SESS" "onboard_filtered"
tmux send-keys -t "$SESS" Escape; sleep 1
cap "$SESS" "onboard_esc"
cp "$GH/logs/gray.log" "$RUN_DIR/onboard_gray.log" 2>/dev/null
tmux kill-session -t "$SESS" 2>/dev/null; rm -rf "$GH"

echo "dig2 done"; ls "$RUN_DIR" | grep -E "garbage|select|ctxedit|skillsseed|resume_empty|resume_last|onboard" | head -n 40
