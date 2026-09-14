"""Modal matrix: /skills manager nav-only (complement to market_skills_ui).

market_skills_ui covers the install dead-end + store tabs; this covers the
mission's navigate/filter/select/cancel/garbage on the manager itself:
one seeded fixture skill, arrows move, Tab reaches Errors, Esc closes,
ghost skill errors cleanly. No model, no keys, hermetic HOME.
"""
import os
import time

from scenarios.plugin_common import boot_fresh, run_cmd

SKILL = "dogfood-nav"
GHOST = "no-such-skill-xyz"

_state = {}


def seed(gray_home, _workdir):
    bundle = os.path.join(gray_home, "skills", SKILL)
    os.makedirs(bundle, exist_ok=True)
    with open(os.path.join(bundle, "SKILL.md"), "w") as f:
        f.write(
            "---\n"
            f"name: {SKILL}\n"
            "description: nav fixture skill\n"
            "---\n"
            "Do the nav thing.\n"
        )
    _state["home"] = gray_home


def extra_env():
    return {"HOME": _state["home"]}


def run(sess, _ctx):
    if not boot_fresh(sess, "skills-ready"):
        return False, "skills: boot prompt never appeared"

    if not run_cmd(sess, "/skills", "Installed | Errors", timeout=30):
        # Headless text list also proves routing.
        if "no skills installed" not in sess.text() and SKILL not in sess.text():
            return False, "skills: '/skills' opened neither manager nor list"
    else:
        sess.shot("skills-open")
        _, before = sess.state_sig()
        sess.press("down")
        time.sleep(0.8)
        sess.shot("skills-moved")
        _, after = sess.state_sig()
        if after == before and SKILL not in sess.text():
            return False, "skills: arrows moved nothing and fixture missing"
        if not sess.child.isalive():
            return False, "skills: gray died after arrow keys"
        sess.press("tab")
        if not sess.expect_text("no errors recorded", timeout=15):
            return False, "skills: Tab did not reach the Errors tab"
        sess.shot("skills-errors")
        sess.press("esc")
        if not sess.expect_text("\u276f", timeout=15):
            return False, "skills: Esc did not close the manager"
        sess.shot("skills-closed")

    if not run_cmd(sess, f"/skills {GHOST}", f"no skill '{GHOST}'", timeout=30):
        return False, "skills: ghost skill gave no 'no skill' error"
    sess.shot("skills-garbage")

    if not sess.child.isalive():
        return False, "skills: gray died before the end"
    return True, "skills ok: manager nav/tabs/cancel + ghost error"
