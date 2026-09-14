"""Sloppy user abuses /plugin: typos, garbage installs, missing names,
double-install, Ctrl-C mid-search. Expects clean error lines, no crash,
no >60s stall (every wait is bounded; a stall returns FAIL). No model.
"""

import glob
import json
import os
import time

from scenarios.plugin_common import boot_fresh, run_cmd, start_index_server

PKG_SPEC = "npm:@normful/picadillo@6.0.0"
PKG_KEY = "normful-picadillo"

_state = {}


def seed(gray_home, workdir):
    _state["index_url"] = start_index_server(workdir)


def extra_env():
    return {"GRAY_PLUGIN_INDEX": _state["index_url"]}


def _check_jsonl_valid(gray_home):
    files = sorted(glob.glob(os.path.join(gray_home, "sessions", "*.jsonl")))
    checked = 0
    for path in files:
        with open(path) as f:
            for i, line in enumerate(f, 1):
                if line.strip():
                    json.loads(line)
                    checked += 1
    return checked


def run(sess, ctx):
    if not boot_fresh(sess, "adv-ready"):
        return False, "adversarial: boot prompt never appeared"

    if not run_cmd(sess, "/plguin", "unknown command"):
        return False, "adversarial: '/plguin' did not print unknown-command line"
    if not run_cmd(sess, "/plugin lst", "unknown plugin subcommand"):
        return False, "adversarial: '/plugin lst' did not print usage error"
    sess.shot("adv-typos")

    if not run_cmd(sess, "/plugin install nope-not-real-xyz", "not in index"):
        return False, "adversarial: garbage install did not say 'not in index'"
    if not run_cmd(sess, "/plugin disable ghost-plugin", "not installed"):
        return False, "adversarial: disable of missing plugin did not say 'not installed'"
    if not run_cmd(sess, "/plugin remove ghost-plugin", "not installed"):
        return False, "adversarial: remove of missing plugin did not say 'not installed'"
    sess.shot("adv-missing")

    # Double-install the same package: both must succeed cleanly (reinstall
    # preserves the enabled flag), then clean up.
    if not run_cmd(sess, f"/plugin install {PKG_SPEC}", f"installed {PKG_KEY}",
                    timeout=60):
        return False, "adversarial: first install failed"
    if not run_cmd(sess, f"/plugin install {PKG_SPEC}", f"installed {PKG_KEY}",
                    timeout=60):
        return False, "adversarial: double-install did not succeed cleanly"
    if not run_cmd(sess, f"/plugin remove {PKG_KEY}", f"removed {PKG_KEY}"):
        return False, "adversarial: cleanup remove failed"
    sess.shot("adv-double-install")

    # Ctrl-C mid-search: interrupt, then prove the session is still sane.
    sess.press("esc")
    sess.send("/plugin search context-mode")
    sess.press("enter")
    time.sleep(1.0)
    sess.ctrl("c")
    if not sess.child.isalive():
        return False, "adversarial: gray died after Ctrl-C mid-search"
    if not sess.wait_idle(quiet_secs=3, timeout=60):
        return False, "adversarial: session never went idle after Ctrl-C (>60s stall)"
    sess.shot("adv-interrupted")
    if not run_cmd(sess, "/plugin list", ["no plugins installed", PKG_KEY]):
        return False, "adversarial: follow-up '/plugin list' failed after Ctrl-C"

    try:
        checked = _check_jsonl_valid(ctx["gray_home"])
    except Exception as e:
        return False, f"adversarial: session JSONL invalid: {e!r}"
    sess.shot("adv-jsonl-ok")
    if not sess.child.isalive():
        return False, "adversarial: gray died before the end"
    return True, f"adversarial survived typos/garbage/double-install/Ctrl-C (jsonl lines checked: {checked})"
