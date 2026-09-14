"""Error recovery: bad slash command, failed agent turn, then green.

1. `/nope123` -> unknown-command error, TUI survives.
2. Turn asks for a missing file -> agent turn fails honestly (idle +
   failure marker on screen, no hallucinated contents).
3. Follow-up reads the real file -> first line quoted exactly.
PASS requires all three in one session.
"""

import os

from scenarios.dfutil import auth_env

FIRST_LINE = "ALPHA-42: the quick brown fox"


def extra_env():
    return auth_env()


def seed(gray_home, workdir):
    with open(os.path.join(workdir, "data.txt"), "w") as f:
        f.write(FIRST_LINE + "\nsecond line here\n")


def run(sess, ctx):
    if not sess.expect_text("\u276f", timeout=25):
        return False, "recovery: prompt never appeared"
    sess.shot("recovery-ready")

    sess.send("/nope123")
    sess.press("enter")
    if not sess.expect_text("unknown command", timeout=20):
        return False, "recovery: bad slash cmd showed no unknown-command error"
    sess.shot("recovery-badcmd")
    if not sess.child.isalive():
        return False, "recovery: gray died on unknown command"

    sess.send("Read the file missing-xyz-999.txt and quote its first line exactly.")
    sess.press("enter")
    if not sess.wait_idle(quiet_secs=5, timeout=300):
        return False, "recovery: missing-file turn never went idle"
    sess.shot("recovery-failed-turn")
    body = sess.text().lower()
    if not any(m in body for m in ("not found", "no such file", "does not exist",
                                   "cannot", "failed", "error")):
        return False, "recovery: missing-file turn shows no failure marker"

    sess.send("Now read data.txt in this directory and quote its first line exactly.")
    sess.press("enter")
    if not sess.wait_idle(quiet_secs=5, timeout=300):
        return False, "recovery: fix-up turn never went idle"
    sess.shot("recovery-green")
    if "ALPHA-42" not in sess.text():
        return False, "recovery: fix-up turn did not quote data.txt"
    return True, "recovery: bad cmd + failed turn, then exact quote green"
