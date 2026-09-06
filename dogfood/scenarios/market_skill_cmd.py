"""`/skill <name>` vs `/skills:<name>` parity at the pty level.

Fixture skill in temp GRAY_HOME (HOME is pointed at GRAY_HOME so discovery
is hermetic: no machine-global skills leak into `available:` lists).
- Valid skill: both forms reach the identical provider gate ("Connect a
  provider" — no model/keys here, so the turn stops at setup); Esc recovers.
  (Byte-identical Prompt expansion is pinned by the Task 4 unit test; the
  pty level proves identical routing + gating.)
- Bogus args: identical single-line errors, compared equal.
- Unknown skill: identical `no skill ...` lines, compared equal.
No model, no keys.
"""

import os

from scenarios.plugin_common import boot_fresh, run_cmd

SKILL = "dogfood-demo"
GHOST = "no-such-skill-xyz"

_state = {}


def seed(gray_home, workdir):
    bundle = os.path.join(gray_home, "skills", SKILL)
    os.makedirs(bundle, exist_ok=True)
    with open(os.path.join(bundle, "SKILL.md"), "w") as f:
        f.write(
            "---\n"
            f"name: {SKILL}\n"
            "description: dogfood fixture skill\n"
            "---\n"
            "Do the demo thing.\n"
        )
    _state["home"] = gray_home


def extra_env():
    # Hermetic discovery: resolve_home() reads $HOME first, so the real
    # home's global skill dirs stay out of `available:` lists.
    return {"HOME": _state["home"]}


def _lines_with(sess, marker):
    return [ln.strip() for ln in sess.text().splitlines() if marker in ln]


def run(sess, ctx):
    if not boot_fresh(sess, "skc-ready"):
        return False, "skill-cmd: boot prompt never appeared"

    # --- valid skill: identical provider gating ---------------------------
    if not run_cmd(sess, f"/skill {SKILL}", "Connect a provider", timeout=30):
        return False, "skill-cmd: '/skill <name>' did not reach the provider gate"
    sess.shot("skc-skill-provider")
    sess.press("esc")
    if not sess.expect_text("\u276f", timeout=15):
        return False, "skill-cmd: Esc did not recover from the provider menu"
    if not run_cmd(sess, f"/skills:{SKILL}", "Connect a provider", timeout=30):
        return False, "skill-cmd: '/skills:<name>' did not reach the provider gate"
    sess.shot("skc-skillscolon-provider")
    sess.press("esc")
    if not sess.expect_text("\u276f", timeout=15):
        return False, "skill-cmd: Esc did not recover from the provider menu"

    # --- bogus args: identical errors --------------------------------------
    if not run_cmd(sess, f"/skill {SKILL} bogus-args", "takes no arguments"):
        return False, "skill-cmd: '/skill <name> bogus-args' error missing"
    err_space = _lines_with(sess, "takes no arguments")
    if not run_cmd(sess, f"/skills:{SKILL} bogus-args", "takes no arguments"):
        return False, "skill-cmd: '/skills:<name> bogus-args' error missing"
    err_colon = _lines_with(sess, "takes no arguments")
    # The screen keeps history: the second capture also holds the first
    # error, so compare the newest occurrence (both errors are single rows).
    if not err_space or not err_colon or err_space[-1] != err_colon[-1]:
        return False, f"skill-cmd: bogus-arg errors differ: {err_space!r} vs {err_colon!r}"
    if SKILL not in err_space[0] or "(none)" not in err_space[0]:
        return False, f"skill-cmd: bogus-arg error names neither skill nor (none): {err_space!r}"
    sess.shot("skc-bogus-args")

    # --- unknown skill: identical errors ------------------------------------
    if not run_cmd(sess, f"/skill {GHOST}", f"no skill '{GHOST}'"):
        return False, "skill-cmd: '/skill <ghost>' error missing"
    unk_space = _lines_with(sess, f"no skill '{GHOST}'")
    if not run_cmd(sess, f"/skills:{GHOST}", f"no skill '{GHOST}'"):
        return False, "skill-cmd: '/skills:<ghost>' error missing"
    unk_colon = _lines_with(sess, f"no skill '{GHOST}'")
    if not unk_space or not unk_colon or unk_space[-1] != unk_colon[-1]:
        return False, f"skill-cmd: unknown-skill errors differ: {unk_space!r} vs {unk_colon!r}"
    sess.shot("skc-unknown")

    if not sess.child.isalive():
        return False, "skill-cmd: gray died before the end"
    return True, (
        "skill-cmd ok: identical provider gating + identical bogus-arg "
        f"({err_space[0][:60]}...) + identical unknown-skill errors"
    )
