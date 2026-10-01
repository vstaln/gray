#!/usr/bin/env bash
# Down-only performance floor: the numbers gray's README sells itself on
# (one binary, a small dependency surface, a fast start) had no CI guard, so
# they were one careless merge away from being marketing.
#
#   scripts/perf-floor.sh          # check against scripts/perf-baseline.json
#   scripts/perf-floor.sh update   # rewrite the baseline (deliberate growth)
#
# Every metric is a ceiling that may only go down. A regression fails the job;
# raising a number is a reviewable commit to the baseline, not a silent edit.
#
# Machine-specific by design: the dependency counts are identical on every
# machine, the binary size is per-target and per-toolchain. Wall-clock startup
# is measured and printed but never enforced — a time budget on a shared CI
# runner fails on load, not on regressions — and the size gets a few percent of
# documented slack for exactly the same reason (`binary_bytes_slack_pct`).
set -euo pipefail

cd "$(dirname "$0")/.."
baseline_file="scripts/perf-baseline.json"
mode="${1:-check}"

# A release binary is only measured when one is already built, so this script
# stays usable on a dev checkout and in a fast dependency-only check.
binary="target/release/gray"
[ -x "$binary" ] || binary=""

metric() { printf '%s' "$2"; }

unique_crates() {
  # `cargo tree` marks an already-expanded subtree with `(*)`; counting those
  # inflates the total, so strip and dedupe before counting or the number is
  # wrong from day one.
  cargo tree --prefix none --workspace --edges normal,build,dev 2>/dev/null \
    | sed 's/ (\*)//' \
    | grep -v '^$' \
    | sort -u \
    | wc -l | tr -d ' '
}

build_script_crates() {
  # Crates with a C/C++ build step: whether building gray needs cc/cmake/
  # pkg-config on a bare machine is a portability fact, not a style question.
  cargo metadata --format-version 1 2>/dev/null \
    | python3 -c '
import json, sys
meta = json.load(sys.stdin)
print(sum(1 for p in meta["packages"]
          if any(t.get("kind") == ["custom-build"] for t in p["targets"])))
'
}

workspace_members() {
  cargo metadata --format-version 1 --no-deps 2>/dev/null \
    | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["workspace_members"]))'
}

binary_bytes() {
  [ -n "$binary" ] || { echo ""; return; }
  strip "$binary" 2>/dev/null || cp "$binary" "$binary.stripped-measure"
  local out
  if [ -f "$binary.stripped-measure" ]; then
    out=$(wc -c < "$binary.stripped-measure")
    rm -f "$binary.stripped-measure"
  else
    out=$(wc -c < "$binary")
  fi
  echo "$out"
}

declare -A current=(
  [unique_crates]=$(unique_crates)
  [build_script_crates]=$(build_script_crates)
  [workspace_members]=$(workspace_members)
  [binary_bytes]=$(binary_bytes)
)

if [ "$mode" = "update" ]; then
  # Policy, not a measurement: keep whatever slack the file already asked for,
  # and never invent one on the user's behalf.
  slack=3
  if [ -f "$baseline_file" ]; then
    prev=$(python3 -c "import json,sys; print(json.load(open(sys.argv[1])).get('binary_bytes_slack_pct', ''))" \
      "$baseline_file")
    [ -n "$prev" ] && slack="$prev"
  fi
  {
    echo '{'
    echo '  "_comment": "Down-only ceilings. Raise one on purpose, in a commit that says why. binary_bytes_slack_pct is the only soft edge: a stripped size moves with the toolchain and the target, everything else is exact. See scripts/perf-floor.sh.",'
    first=1
    for key in unique_crates build_script_crates workspace_members binary_bytes; do
      [ -n "${current[$key]}" ] || continue
      [ $first -eq 1 ] || echo ','
      first=0
      printf '  "%s": %s' "$key" "${current[$key]}"
    done
    echo ','
    printf '  "binary_bytes_slack_pct": %s' "$slack"
    echo ''
    echo '}'
  } > "$baseline_file"
  echo "wrote $baseline_file"
  cat "$baseline_file"
  exit 0
fi

[ -f "$baseline_file" ] || { echo "missing $baseline_file — run scripts/perf-floor.sh update"; exit 2; }

read_baseline() {
  python3 -c "import json,sys; print(json.load(open(sys.argv[1])).get(sys.argv[2], ''))" \
    "$baseline_file" "$1"
}

# The stripped size moves with the toolchain and the target, so it is the one
# metric compared with slack. Everything else is exact.
slack=$(read_baseline binary_bytes_slack_pct)
slack="${slack:-3}"

status=0
for key in unique_crates build_script_crates workspace_members binary_bytes; do
  limit=$(read_baseline "$key")
  value="${current[$key]}"
  if [ -z "$limit" ]; then
    printf '  %-22s %-10s (not baselined)\n' "$key" "${value:-skipped}"
    continue
  fi
  if [ -z "$value" ]; then
    printf '  %-22s %-10s baseline %s (no release binary to measure)\n' "$key" "skipped" "$limit"
    continue
  fi
  if [ "$key" = "binary_bytes" ]; then
    # Ceiling including slack, so the printed limit is what was enforced.
    limit=$(( limit * (100 + slack) / 100 ))
    printf '  %-22s %-10s baseline %s (+%s%% slack)\n' "$key" "$value" "$limit" "$slack"
    if [ "$value" -gt "$limit" ]; then status=1; fi
    continue
  fi
  if [ "$value" -gt "$limit" ]; then
    printf '  %-22s %-10s EXCEEDS baseline %s (+%s)\n' "$key" "$value" "$limit" "$((value - limit))"
    status=1
  else
    printf '  %-22s %-10s ok (baseline %s)\n' "$key" "$value" "$limit"
  fi
done

# Reported, never enforced — see the header.
if [ -n "$binary" ]; then
  start=$(date +%s%N)
  "$binary" --version > /dev/null
  end=$(date +%s%N)
  printf '  %-22s %s ms (reported, not enforced: machine-specific)\n' \
    "version_start_ms" "$(( (end - start) / 1000000 ))"
fi

if [ "$status" -ne 0 ]; then
  echo
  echo "performance floor: a ceiling moved up. Shrink the change, or run"
  echo "  scripts/perf-floor.sh update   # and justify it in the commit message"
fi
exit $status
