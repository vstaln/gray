"""Resume/continue flow across two TUI processes, one GRAY_HOME.

Phase A: store codename BLUEBIRD, close the TUI. Phase B: fresh TUI on
the SAME home+workdir, /resume <first-session-id-prefix>, then ask for
the codename. PASS requires BLUEBIRD back (history actually restored,
not a fresh amnesiac session).
"""

from driver import Session
from scenarios.dfutil import auth_env, session_ids

CODENAME = "BLUEBIRD"


def extra_env():
    return auth_env()


def run(sess, ctx):
    gray_home, workdir, run_dir = ctx["gray_home"], ctx["workdir"], ctx["run_dir"]

    if not sess.expect_text("\u276f", timeout=25):
        return False, "resume: phase-A prompt never appeared"
    sess.send(f"Remember this codename for later: {CODENAME}. "
              "Reply with exactly: STORED. Do not call any tools.")
    sess.press("enter")
    if not sess.wait_idle(quiet_secs=5, timeout=300):
        return False, "resume: phase-A turn never went idle"
    sess.shot("resume-phase-a")
    if "STORED" not in sess.text():
        return False, "resume: phase-A did not acknowledge"

    ids = session_ids(gray_home)
    if not ids:
        return False, "resume: no session file persisted after phase A"
    target = ids[0][:8]
    # NOTE: do NOT close the passed sess: driver.main's finally block calls
    # sess.shot("final") on it, which raises on a closed pty and would skip
    # result.txt. It idles at the prompt while phase B runs.
    s2 = Session(gray_home, workdir, run_dir, extra_env=extra_env())
    try:
        if not s2.expect_text("\u276f", timeout=25):
            return False, "resume: phase-B prompt never appeared"
        s2.shot("resume-reopened")
        s2.send(f"/resume {target}")
        s2.press("enter")
        if not s2.expect_text(CODENAME, timeout=30):
            return False, "resume: history replay missing codename"
        s2.shot("resume-restored")
        s2.send("What was the codename I gave you? Reply with just the codename.")
        s2.press("enter")
        if not s2.wait_idle(quiet_secs=5, timeout=300):
            return False, "resume: follow-up turn never went idle"
        s2.shot("resume-answered")
        if CODENAME not in s2.text():
            return False, "resume: resumed session forgot the codename"
    finally:
        s2.close()
    return True, f"resume: /resume {target} restored history, codename recalled"
