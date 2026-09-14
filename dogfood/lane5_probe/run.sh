#!/bin/bash
# lane5 runner: isolated homes, seeded workdir, tmux discipline, ansi captures.
# Creates files ONLY under dogfood/runs/lane5_* and /tmp scratch. No commits.
set -u
REPO=/home/vstaln/gray
RUN="$REPO/dogfood/runs/lane5_tools-20260908-041102"
PROBE="$REPO/dogfood/lane5_probe/target/debug/lane5_probe"
export GRAY_HOME="$RUN/home" HOME="$RUN/fakehome" LANE5_WORK="$RUN/work"
export PATH="/home/vstaln/.empryo/bin:$PATH"
export GRAY_NO_UPDATE_CHECK=1 TERM=xterm-256color
mkdir -p "$GRAY_HOME" "$HOME" "$LANE5_WORK"

# Seed workdir (benign fixtures only)
printf 'lane5-fixture-note line1\nline2\n' > "$LANE5_WORK/note.txt"
printf 'alpha\n' > "$LANE5_WORK/a.txt"
mkdir -p "$LANE5_WORK/.gray/skills/lane5demo"
printf -- '---\nname: lane5demo\ndescription: lane5 fixture skill\n---\n\nlane5demo-body says hello $ARGUMENTS.\n' > "$LANE5_WORK/.gray/skills/lane5demo/SKILL.md"

# tmux discipline: unique prefixed session, killed at the end
tmux kill-session -t lane5_probe_run 2>/dev/null
tmux new-session -d -s lane5_probe_run -x 120 -y 30
tmux ls 2>/dev/null | grep lane5 | tee "$RUN/lane5_tmux_list.txt"

rc=0
for phase in tools approvals background; do
  script -qec "$PROBE $phase" "$RUN/lane5_${phase}.ansi" < /dev/null > /dev/null 2>&1
  code=$?
  echo "phase=$phase exit=$code"
  [ $code -ne 0 ] && rc=1
done

tmux kill-session -t lane5_probe_run 2>/dev/null
tmux ls 2>/dev/null > "$RUN/lane5_tmux_after.txt" || echo "(no tmux server left)" > "$RUN/lane5_tmux_after.txt"
cat "$RUN/lane5_tmux_after.txt"
echo "RUNNER_DONE rc=$rc"
exit $rc
