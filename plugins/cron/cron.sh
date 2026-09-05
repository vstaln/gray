#!/bin/sh
# cron sidecar (real): the scheduler is the gray-cron-sidecar binary, which
# reuses gray-cron's store/parser/scheduler (no shell reimplementation).
# Manifest declares capabilities ["session"] + subcommands ["/cron"]; due
# jobs fire via host/run and report via host/say (see docs/plugins.md).
# Needs the binary on PATH (cargo install -p gray-cron) or a workspace
# build tree (target/ below); without it spawn fails fast and loud.
if command -v gray-cron-sidecar >/dev/null 2>&1; then
  exec gray-cron-sidecar
fi
HERE=$(dirname "$0")
for c in "$HERE/../../target/debug/gray-cron-sidecar" "$HERE/../../target/release/gray-cron-sidecar"; do
  if [ -x "$c" ]; then
    exec "$c"
  fi
done
echo "cron sidecar: gray-cron-sidecar not found (cargo install -p gray-cron, or cargo build -p gray-cron)" >&2
exit 127
