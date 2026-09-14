"""Modal matrix: /permissions picker offline. Open, navigate, cancel, garbage + direct.

Fresh home, no keys. Bare /permissions opens the 3-mode picker; direct
forms set inline. Exercises arrows, Esc, unknown-mode error, and a direct
set back to auto (restores default).
"""
import time

from scenarios.plugin_common import boot_fresh, run_cmd


def run(sess, _ctx):
    if not boot_fresh(sess, "perms-ready"):
        return False, "permissions: boot prompt never appeared"

    if not run_cmd(sess, "/permissions", ["Ask for approval", "Permissions"],
                   timeout=30):
        return False, "permissions: '/permissions' did not open the picker"
    sess.shot("perms-open")

    _, before = sess.state_sig()
    sess.press("down")
    time.sleep(0.8)
    sess.shot("perms-moved")
    _, after = sess.state_sig()
    if after == before:
        return False, "permissions: down-arrow did not move highlight"
    if not sess.child.isalive():
        return False, "permissions: gray died after arrow keys"

    sess.press("esc")
    if not sess.expect_text("\u276f", timeout=15):
        return False, "permissions: Esc did not close the picker"
    sess.shot("perms-closed")

    if not run_cmd(sess, "/permissions garbage-xyz", "unknown mode", timeout=30):
        return False, "permissions: garbage mode gave no 'unknown mode' error"
    sess.shot("perms-garbage")

    if not run_cmd(sess, "/permissions full", ["Permissions updated", "Full Access"],
                   timeout=30):
        return False, "permissions: direct '/permissions full' did not confirm"
    sess.shot("perms-set-full")
    if not run_cmd(sess, "/permissions auto", ["Permissions updated", "Ask for approval"],
                   timeout=30):
        return False, "permissions: restore '/permissions auto' did not confirm"

    if not sess.child.isalive():
        return False, "permissions: gray died before the end"
    return True, "permissions ok: open/nav/cancel/garbage/direct"
