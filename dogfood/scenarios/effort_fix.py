"""Thinking-effort sweep: ONE identical task at low vs high effort.

Effort comes from DOG_EFFORT env (run_matrix.py runs this twice).
Fixture: is_palindrome is case-sensitive ("Racecar" -> False).
Task: fix + verify by running + one-line cause. Grading is behavioral
(subprocess checks 3 cases), so quality comparison is objective.
"""

import os
import subprocess

from scenarios.dfutil import auth_env

PAL = '''def is_palindrome(s):
    """True if s reads the same forwards and backwards."""
    cleaned = "".join(ch for ch in s if ch.isalnum())
    return cleaned == cleaned[::-1]
'''

PROMPT = (
    "Fix is_palindrome in palindrome.py so that 'Racecar' and "
    "'A man a plan a canal Panama' return True and 'hello' returns False. "
    "Verify by running python3 against those three inputs, then report "
    "the one-line cause of the bug."
)


def extra_env():
    return auth_env()


def seed(gray_home, workdir):
    with open(os.path.join(workdir, "palindrome.py"), "w") as f:
        f.write(PAL)


def _grade(workdir):
    code = (
        "import sys; sys.path.insert(0, '.');"
        "from palindrome import is_palindrome;"
        "print(is_palindrome('Racecar'),"
        " is_palindrome('A man a plan a canal Panama'),"
        " is_palindrome('hello'))"
    )
    p = subprocess.run(["python3", "-c", code], cwd=workdir,
                       capture_output=True, text=True, timeout=60)
    return p.stdout.strip() == "True True False"


def run(sess, ctx):
    if not sess.expect_text("\u276f", timeout=25):
        return False, "effort: prompt never appeared"
    sess.shot("effort-ready")
    sess.send(PROMPT)
    sess.press("enter")
    if not sess.wait_idle(quiet_secs=6, timeout=600):
        return False, "effort: fix turn never went idle"
    sess.shot("effort-done")
    if not _grade(ctx["workdir"]):
        return False, "effort: palindrome cases still wrong on disk"
    return True, "effort: all three palindrome cases correct on disk"
