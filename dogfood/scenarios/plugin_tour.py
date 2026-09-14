"""Eager adopter tours /plugin + Pi Gallery: list, search, install one small
real pi skill, list, disable, re-enable, remove. No model, no keys.

Package choice: npm:@normful/picadillo@6.0.0 (pinned, ~146KB unpacked,
ships real skills/*/SKILL.md: mulch, run-in-tmux; installs as
normful-picadillo). Gray Index is served empty over loopback so search
fans out to the live pi side (production index 404s — see report).
"""

from scenarios.plugin_common import boot_fresh, run_cmd, start_index_server

PKG_SPEC = "npm:@normful/picadillo@6.0.0"
PKG_KEY = "normful-picadillo"

_state = {}


def seed(gray_home, workdir):
    _state["index_url"] = start_index_server(workdir)


def extra_env():
    return {"GRAY_PLUGIN_INDEX": _state["index_url"]}


def run(sess, ctx):
    if not boot_fresh(sess, "tour-ready"):
        return False, "tour: boot prompt never appeared"

    # Wart 1: eager typo, corrected right away.
    if not run_cmd(sess, "/plugin lst", "unknown plugin subcommand"):
        return False, "tour: typo '/plugin lst' did not print usage error"
    sess.shot("tour-typo")

    if not run_cmd(sess, "/plugin list", "no plugins installed"):
        return False, "tour: fresh '/plugin list' did not say 'no plugins installed'"
    sess.shot("tour-list-empty")

    if not run_cmd(sess, "/plugin search picadillo", "[Pi Gallery (preview)]"):
        return False, "tour: search did not return Pi Gallery (preview) hits"
    sess.shot("tour-search")

    if not run_cmd(sess, f"/plugin install {PKG_SPEC}", f"installed {PKG_KEY} 6.0.0"):
        return False, "tour: install did not report success"
    sess.shot("tour-installed")

    if not run_cmd(sess, "/plugin list", f"{PKG_KEY} 6.0.0 (user)"):
        return False, "tour: list after install missing the new plugin"
    sess.shot("tour-list-full")

    if not run_cmd(sess, f"/plugin disable {PKG_KEY}", f"disabled {PKG_KEY}"):
        return False, "tour: disable did not confirm"
    if not run_cmd(sess, "/plugin list", "[disabled]"):
        return False, "tour: list after disable missing [disabled] mark"
    sess.shot("tour-disabled")

    if not run_cmd(sess, f"/plugin enable {PKG_KEY}", f"enabled {PKG_KEY}"):
        return False, "tour: re-enable did not confirm"
    sess.shot("tour-enabled")

    if not run_cmd(sess, f"/plugin remove {PKG_KEY}", f"removed {PKG_KEY}"):
        return False, "tour: remove did not confirm"
    if not run_cmd(sess, "/plugin list", "no plugins installed"):
        return False, "tour: list after remove not empty again"
    sess.shot("tour-cleaned")

    if not sess.child.isalive():
        return False, "tour: gray died before the end"
    return True, "tour completed: search/install/disable/enable/remove round-trip"
