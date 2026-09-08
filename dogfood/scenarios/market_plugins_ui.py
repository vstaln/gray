"""Plugins manager modal dogfood: tabs, uninstall arm/disarm, close.

One fake installed plugin is seeded via the lockfile, so no network is
touched anywhere: open `/plugin`, screenshot `Installed | Errors`, Tab to
Errors and back, arm uninstall with `u` then disarm with Up (assert no
removal, lockfile-side), Esc closes. No model, no keys.
"""

import json
import os
import time

from scenarios.plugin_common import boot_fresh, run_cmd

PLUGIN = "dogfood-demo"


def seed(gray_home, workdir):
    plugdir = os.path.join(gray_home, "plugins")
    os.makedirs(plugdir, exist_ok=True)
    with open(os.path.join(plugdir, "lock.json"), "w") as f:
        json.dump(
            {
                "schema": 1,
                "plugins": {
                    PLUGIN: {
                        "ecosystem": "gray-native",
                        "version": "1.2.3",
                        "hash": "",
                        "source": "",
                        "argv": [],
                        "adapter_version": "",
                        "installed_at": "",
                        "scope": "user",
                        "enabled": True,
                    }
                },
            },
            f,
        )


def _lock_has(gray_home, name):
    try:
        with open(os.path.join(gray_home, "plugins", "lock.json")) as f:
            return name in json.load(f).get("plugins", {})
    except Exception:
        return False


def run(sess, ctx):
    if not boot_fresh(sess, "plug-ready"):
        return False, "plugins-ui: boot prompt never appeared"

    if not run_cmd(sess, "/plugin", "Installed | Errors"):
        return False, "plugins-ui: /plugin did not open the manager tabs"
    if PLUGIN not in sess.text():
        return False, "plugins-ui: seeded plugin missing from Installed tab"
    sess.shot("plug-installed")

    sess.press("tab")
    if not sess.expect_text("no errors recorded", timeout=15):
        return False, "plugins-ui: Tab did not reach the Errors tab"
    if "c clear" not in sess.text():
        return False, "plugins-ui: Errors footer missing 'c clear'"
    sess.shot("plug-errors")

    sess.press("tab")
    if not sess.expect_text("u remove", timeout=15):
        return False, "plugins-ui: Tab did not cycle back to Installed"
    sess.shot("plug-installed-again")

    sess.send("u")
    if not sess.expect_text(f"really remove {PLUGIN}? (u again)", timeout=15):
        return False, "plugins-ui: 'u' did not arm the uninstall confirm"
    sess.shot("plug-armed")

    sess.press("up")
    time.sleep(1.0)
    if f"really remove {PLUGIN}? (u again)" in sess.text():
        return False, "plugins-ui: Up did not disarm the uninstall confirm"
    if not _lock_has(ctx["gray_home"], PLUGIN):
        return False, "plugins-ui: seeded plugin vanished from the lockfile"
    sess.shot("plug-disarmed")

    sess.press("esc")
    if not sess.expect_text("\u276f", timeout=15):
        return False, "plugins-ui: Esc did not close the manager"
    sess.shot("plug-closed")
    if not sess.child.isalive():
        return False, "plugins-ui: gray died before the end"
    return True, "plugins-ui ok: tabs cycle, u-arms/Up-disarms with no removal, Esc closes"
