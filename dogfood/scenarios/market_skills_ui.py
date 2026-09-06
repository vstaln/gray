"""Skills manager + marketplace store UI dogfood (offline fixtures).

- `/skills` manager: Installed/Errors tabs, Esc each (empty states).
- `/marketplace` store: `Plugins | Skills | Marketplaces` tabs; Discover
  search is NOT run live (query row + prompt state asserted only).
- Parked behavior: a Claude-bundle skill hit IS reachable offline via a
  file:// fixture marketplace, so the scenario attempts the install and
  asserts the graceful dead-end: inline `failed:` text, modal stays open,
  error recorded (lockfile-side check on errors.json).
- Marketplaces tab: all 4 sources with status. No model, no keys.
"""

import json
import os

from scenarios.plugin_common import boot_fresh, run_cmd

SKILL = "grep-skills"
CLAUDE_SPEC = f"claude:{SKILL}"

_state = {}


def seed(gray_home, workdir):
    market = os.path.join(gray_home, "claude-fix", ".claude-plugin")
    os.makedirs(market, exist_ok=True)
    with open(os.path.join(market, "marketplace.json"), "w") as f:
        json.dump(
            {
                "name": "dogfood-fix",
                "plugins": [
                    {
                        "name": SKILL,
                        "description": "grep helpers for tests",
                        "source": "./plugins/grep-skills",
                    }
                ],
            },
            f,
        )
    _state["market"] = os.path.join(gray_home, "claude-fix")


def extra_env():
    return {
        "GRAY_PLUGIN_INDEX": "http://127.0.0.1:9/",
        "GRAY_NPM_REGISTRY": "http://127.0.0.1:9",
        "GRAY_CLAWHUB_BASE": "http://127.0.0.1:9",
        "GRAY_CLAUDE_MARKETPLACES": f"file://{_state['market']}",
    }


def _error_rows(gray_home, source, item):
    try:
        with open(os.path.join(gray_home, "errors.json")) as f:
            return [e for e in json.load(f) if e.get("source") == source and e.get("item") == item]
    except Exception:
        return []


def run(sess, ctx):
    if not boot_fresh(sess, "msk-ready"):
        return False, "skills-ui: boot prompt never appeared"

    # --- /skills manager -------------------------------------------------
    if not run_cmd(sess, "/skills", "Installed | Errors"):
        return False, "skills-ui: /skills did not open the manager tabs"
    if "no skills installed" not in sess.text():
        return False, "skills-ui: manager Installed empty state missing"
    sess.shot("msk-installed")
    sess.press("tab")
    if not sess.expect_text("no errors recorded", timeout=15):
        return False, "skills-ui: Tab did not reach the manager Errors tab"
    sess.shot("msk-errors")
    sess.press("esc")
    if not sess.expect_text("\u276f", timeout=15):
        return False, "skills-ui: Esc did not close the skills manager"

    # --- /marketplace store: Discover tab (no live search) ---------------
    if not run_cmd(sess, "/marketplace", "Plugins | Skills | Marketplaces"):
        return False, "skills-ui: /marketplace did not open the store tabs"
    if "Search: type + Enter" not in sess.text():
        return False, "skills-ui: store Plugins tab missing the query prompt row"
    sess.shot("msk-store-plugins")

    # --- Skills tab: fixture search -> preview (parked: reachable) -------
    sess.press("tab")
    if not sess.expect_text("Search: type + Enter", timeout=15):
        return False, "skills-ui: Tab did not reach the store Skills tab"
    sess.send("grep")
    sess.press("enter")
    if not sess.expect_text(SKILL, timeout=30):
        return False, "skills-ui: fixture skill search returned no hit"
    if "[Claude]" not in sess.text():
        return False, "skills-ui: skill hit missing its [Claude] source"
    sess.shot("msk-skills-hits")

    sess.press("enter")
    if not sess.expect_text(f"install: {CLAUDE_SPEC}", timeout=15):
        return False, "skills-ui: preview missing the install: origin note"
    sess.shot("msk-skills-preview")

    # --- Parked dead-end: install fails INLINE, modal stays open ---------
    sess.send("i")
    if not sess.expect_text("failed:", timeout=30):
        return False, "skills-ui: claude skill install did not fail inline"
    if f"install: {CLAUDE_SPEC}" not in sess.text():
        return False, "skills-ui: modal did not stay open after failed install"
    rows = _error_rows(ctx["gray_home"], "local", CLAUDE_SPEC)
    if not rows:
        return False, "skills-ui: failed install recorded no errors row"
    sess.shot("msk-skills-failed-inline")

    sess.press("esc")
    if not sess.expect_text("Search: grep", timeout=15):
        return False, "skills-ui: Esc did not back out of the preview"
    sess.press("esc")
    if not sess.expect_text("\u276f", timeout=15):
        return False, "skills-ui: Esc did not close the store"

    # --- Marketplaces tab: 4 sources with status --------------------------
    if not run_cmd(sess, "/marketplace", "Plugins | Skills | Marketplaces"):
        return False, "skills-ui: /marketplace reopen failed"
    sess.press("tab", "tab")
    if not sess.expect_text("Gray Index: unreachable", timeout=30):
        return False, "skills-ui: Marketplaces tab missing gray status"
    text = sess.text()
    for want in (
        "Pi Gallery (preview): unreachable",
        "ClawHub: unreachable",
        "Claude: ok",
    ):
        if want not in text:
            return False, f"skills-ui: Marketplaces tab missing {want!r}"
    sess.shot("msk-marketplaces")
    sess.press("esc")
    if not sess.expect_text("\u276f", timeout=15):
        return False, "skills-ui: Esc did not close the store"
    if not sess.child.isalive():
        return False, "skills-ui: gray died before the end"
    return True, (
        "skills-ui ok: manager tabs, store tabs, claude skill inline-fail "
        "(modal open, error recorded), 4 marketplace statuses"
    )
