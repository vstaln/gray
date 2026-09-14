"""Modal matrix: /help table + unknown-command + completion popup. Offline.

Fresh home. /help must list every registry command incl. alias rows
(/provider, /exit); garbage gives the unknown hint; /co Tab completes.
"""
from scenarios.plugin_common import boot_fresh, run_cmd


def run(sess, _ctx):
    if not boot_fresh(sess, "help-ready"):
        return False, "help: boot prompt never appeared"

    if not run_cmd(sess, "/help", "/quit", timeout=30):
        return False, "help: '/help' missing /quit row"
    body = sess.text()
    for want in ("/model", "/thinking", "/context", "/permissions",
                 "/provider", "/exit", "/skills", "/plugin"):
        if want not in body:
            return False, f"help: table missing {want!r}"
    sess.shot("help-table")

    if not run_cmd(sess, "/nope123", "unknown command", timeout=30):
        return False, "help: garbage '/nope123' gave no unknown-command line"
    sess.shot("help-unknown")

    # Completion popup: /co surfaces a command row (context matches 'co').
    sess.press("esc")
    sess.send("/co")
    if not sess.expect_text("/context", timeout=10):
        return False, "help: typing '/co' did not surface /context completion"
    sess.shot("help-completion")
    sess.press("esc")
    sess.send("\x15")  # Ctrl-U: clear the line if the impl maps it; else harmless
    sess.press("esc")

    if not sess.child.isalive():
        return False, "help: gray died before the end"
    return True, "help ok: table rows + unknown hint + /co completion"
