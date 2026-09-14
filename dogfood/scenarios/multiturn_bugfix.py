"""Multi-turn agentic task with tool calls + /usage sanity.

Fixture: calc.py with an off-by-one in total(), test_calc.py (plain
python, 3 tests, 1 fails). Turn 1: diagnose (run tests, no edits).
Turn 2: fix + re-run green. Turn 3: /usage renders session totals.
PASS requires the suite green on disk AND the usage card on screen.
"""

import os
import subprocess

from scenarios.dfutil import auth_env

CALC = '''def total(xs):
    """Sum a list of numbers."""
    s = 0
    for i in range(len(xs) - 1):
        s += xs[i]
    return s
'''

TEST = '''import sys
sys.path.insert(0, ".")
from calc import total

fails = []
def check(name, got, want):
    print(("PASS " if got == want else "FAIL ") + name)
    if got != want:
        fails.append(name)

check("test_total_empty", total([]), 0)
check("test_total_single", total([7]), 7)
check("test_total_many", total([1, 2, 3, 4]), 10)
print("ALL-GREEN" if not fails else "FAILURES: " + ",".join(fails))
sys.exit(0 if not fails else 1)
'''


def extra_env():
    return auth_env()


def seed(gray_home, workdir):
    with open(os.path.join(workdir, "calc.py"), "w") as f:
        f.write(CALC)
    with open(os.path.join(workdir, "test_calc.py"), "w") as f:
        f.write(TEST)


def _suite_green(workdir):
    p = subprocess.run(
        ["python3", "test_calc.py"], cwd=workdir,
        capture_output=True, text=True, timeout=60,
    )
    return p.returncode == 0 and "ALL-GREEN" in p.stdout


def run(sess, ctx):
    if not sess.expect_text("\u276f", timeout=25):
        return False, "bugfix: prompt never appeared"
    sess.shot("bugfix-ready")

    sess.send("Run `python3 test_calc.py` and tell me which test fails "
              "and why. One short paragraph, do not edit anything yet.")
    sess.press("enter")
    if not sess.wait_idle(quiet_secs=6, timeout=420):
        return False, "bugfix: turn 1 (diagnose) never went idle"
    sess.shot("bugfix-diagnosed")
    if "test_total" not in sess.text():
        return False, "bugfix: turn 1 did not name the failing test"

    sess.send("Fix the bug in calc.py, re-run `python3 test_calc.py`, "
              "and confirm all green.")
    sess.press("enter")
    if not sess.wait_idle(quiet_secs=6, timeout=420):
        return False, "bugfix: turn 2 (fix) never went idle"
    sess.shot("bugfix-fixed")
    if not _suite_green(ctx["workdir"]):
        return False, "bugfix: suite still red on disk after turn 2"

    sess.send("/usage")
    sess.press("enter")
    if not sess.expect_text("Session usage", timeout=30):
        return False, "bugfix: /usage did not render the usage card"
    sess.shot("bugfix-usage")
    # usage body is "{n} in · {n} out · {n} total"
    if " in \u00b7 " not in sess.text():
        return False, "bugfix: usage card missing in/out totals"
    return True, "bugfix: diagnose -> fix green on disk -> /usage renders"
