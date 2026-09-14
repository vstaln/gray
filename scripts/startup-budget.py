#!/usr/bin/env python3
"""Startup latency gate (fx `benchmarks/startup.sh` port, stdlib only).

Measures wall-clock medians for cheap CLI entry points and fails when a
budget is exceeded. No hyperfine needed: plain spawn timing with warmup,
same method as the ad-hoc numbers in chat.

Budgets (Linux, release binary):
  gray --help          <= 30ms median   (today ~5ms; headroom for CI noise)
  gray --version       <= 30ms median
  gray sessions prune --help <= 30ms median

Usage:
  scripts/startup-budget.py [--bin PATH] [--runs N] [--quick]
"""
import statistics
import subprocess
import sys
import time

BUDGETS_MS = {
    ("--help",): 30.0,
    ("--version",): 30.0,
    ("sessions", "prune", "--help"): 30.0,
}

BIN = "./target/release/gray"
RUNS = 50
WARMUP = 5


def median_ms(cmd):
    samples = []
    for _ in range(WARMUP + RUNS):
        t = time.perf_counter()
        r = subprocess.run([BIN, *cmd], capture_output=True)
        dt = (time.perf_counter() - t) * 1000
        if r.returncode not in (0, 2):
            print(f"FAIL  {' '.join(cmd)}: exit {r.returncode}")
            return None
        samples.append(dt)
    samples = samples[WARMUP:]
    samples.sort()
    return statistics.median(samples)


def main():
    global BIN, RUNS, WARMUP
    args = sys.argv[1:]
    if "--bin" in args:
        BIN = args[args.index("--bin") + 1]
    if "--quick" in args:
        RUNS, WARMUP = 15, 2
    failed = False
    for cmd, budget in BUDGETS_MS.items():
        med = median_ms(cmd)
        if med is None:
            failed = True
            continue
        tag = "ok  " if med <= budget else "OVER"
        if med > budget:
            failed = True
        print(f"{tag}  gray {' '.join(cmd):28s} median={med:6.1f}ms  (budget {budget:.0f}ms)")
    print("All commands within budget" if not failed else "Latency budget exceeded")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
