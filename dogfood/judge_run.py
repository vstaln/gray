"""Grade a layer-2 run: feed its session JSONL + rubric to gray -p.

Usage: python3 judge_run.py runs/<scenario>-<ts>
Uses the OPERATOR's real gray setup (default GRAY_HOME) and model.
Prints the judge output; exit 0 iff VERDICT: PASS.
"""
import glob
import os
import subprocess
import sys

GRAY_BIN = os.environ.get(
    "GRAY_BIN", os.path.join(os.path.dirname(__file__), "..", "target", "debug", "gray")
)


def main():
    run_dir = sys.argv[1]
    with open(os.path.join(os.path.dirname(__file__), "judge.md")) as f:
        rubric = f.read()
    logs = sorted(glob.glob(os.path.join(run_dir, "gray-home", "sessions", "*.jsonl")))
    if not logs:
        print("no session jsonl in run dir")
        return 2
    with open(logs[-1]) as f:
        transcript = f.read()[-6000:]  # tail: turn + tool records, not full history
    prompt = rubric + "\n--- SESSION ---\n" + transcript
    env = dict(os.environ, GRAY_NO_UPDATE_CHECK="1")
    # env-only key lookup: never pass secrets as args (ps-visible)
    proc = subprocess.run(
        [GRAY_BIN, "-p", prompt], capture_output=True, text=True, timeout=180, env=env
    )
    out = (proc.stdout + proc.stderr).strip()
    print(out[-2000:])
    return 0 if "VERDICT: PASS" in out else 1


if __name__ == "__main__":
    sys.exit(main())
