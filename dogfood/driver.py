"""Dogfooder driver: spawn gray under a pty, screenshot via pyte, send keys.

Usage:
    python3 driver.py <scenario> [--gray-home DIR] [--workdir DIR]

A scenario is a function in scenarios/<name>.py taking a Session.
Runs land in runs/<scenario>-<timestamp>/ (screens.log + transcript).
"""
import datetime
import os
import sys
import time

import pexpect
import pyte

GRAY_BIN = os.environ.get(
    "GRAY_BIN", os.path.join(os.path.dirname(__file__), "..", "target", "debug", "gray")
)
COLS, ROWS = 120, 30

KEYS = {
    "enter": "\r",
    "esc": "\x1b",
    "tab": "\t",
    "up": "\x1b[A",
    "down": "\x1b[B",
    "left": "\x1b[D",
    "right": "\x1b[C",
}


def seed_home(gray_home, real_home=os.path.expanduser("~/.gray")):
    """Copy the operator's config+auth for keyed scenarios. Values are
    never printed or logged; the copy lives in gitignored runs/."""
    import shutil

    for name in ("config.json", "auth.json"):
        src = os.path.join(real_home, name)
        if os.path.exists(src):
            shutil.copy(src, os.path.join(gray_home, name))


class Session:
    def __init__(self, gray_home, workdir, run_dir, extra_env=None):
        env = dict(os.environ)
        env.update(
            {
                "GRAY_HOME": gray_home,
                "GRAY_NO_UPDATE_CHECK": "1",
                "TERM": "xterm-256color",
                "COLUMNS": str(COLS),
                "LINES": str(ROWS),
            }
        )
        env.update(extra_env or {})
        self.run_dir = run_dir
        self.frames = []  # saved screenshots (list of line-lists)
        # One persistent terminal: the pty is a stream, bytes are consumed
        # once, so every drain feeds the same screen or later shots only
        # see repaint deltas.
        self._screen = pyte.Screen(COLS, ROWS)
        self._stream = pyte.Stream(self._screen)
        self.child = pexpect.spawn(
            GRAY_BIN, cwd=workdir, dimensions=(ROWS, COLS), env=env, timeout=10,
            encoding="utf-8", codec_errors="replace",
        )
        # Drain from birth: gray's inline viewport probes the terminal
        # (DSR-CPR) with a short fuse. A blind sleep lets the probe time
        # out, so poll through quiet gaps until the deadline instead.
        self._drain_until(time.time() + 5.0)
        self.shot("boot")

    def _drain_until(self, deadline):
        while time.time() < deadline:
            try:
                chunk = self.child.read_nonblocking(size=65536, timeout=0.2)
            except pexpect.TIMEOUT:
                continue
            except pexpect.EOF:
                break
            if chunk:
                self._stream.feed(chunk)
                self._answer_queries(chunk)

    def screen(self, quiet=0.4, budget=3.0):
        # Bounded drain: TUIs that repaint on a ticker never go quiet,
        # so stop at `budget` secs regardless. Feeds the persistent
        # screen, so each shot is full terminal state, not a delta.
        end = time.time() + budget
        try:
            while time.time() < end:
                try:
                    chunk = self.child.read_nonblocking(size=65536, timeout=quiet)
                except pexpect.TIMEOUT:
                    break
                if chunk:
                    self._stream.feed(chunk)
                    self._answer_queries(chunk)
                else:
                    break
        except pexpect.EOF:
            pass
        return [ln.rstrip() for ln in self._screen.display]

    def _answer_queries(self, chunk):
        # Be "a real terminal" for gray's inline-viewport probe: it asks
        # DSR-CPR (ESC[6n) at composer init and bails without an answer.
        # Reply with pyte's cursor (1-based). Unknown queries are logged
        # to the run dir for future emulation, never answered blindly.
        if "\x1b[6n" in chunk:
            cur = self._screen.cursor
            self.child.send(f"\x1b[{cur.y + 1};{cur.x + 1}R")

    def text(self):
        return "\n".join(self.screen())

    def state_sig(self):
        """Screenshot + attribute signature: text plus cursor pos and
        reversed-video cells, so highlight moves are visible even when
        the characters don't change."""
        lines = self.screen()
        rev = set()
        for y in range(ROWS):
            for x in range(COLS):
                ch = self._screen.buffer[y][x]
                if ch.reverse:
                    rev.add((x, y))
        cur = self._screen.cursor
        return ("\n".join(lines), (cur.x, cur.y, frozenset(rev)))

    def shot(self, label):
        lines = self.screen()
        self.frames.append((label, lines))
        with open(os.path.join(self.run_dir, "screens.log"), "a") as f:
            f.write(f"===== {label} =====\n" + "\n".join(lines) + "\n")

    def send(self, s):
        self.child.send(s)

    def press(self, *names):
        for n in names:
            self.child.send(KEYS[n])
            time.sleep(0.3)

    def ctrl(self, c):
        self.child.sendcontrol(c)
        time.sleep(0.5)

    def expect_text(self, substr, timeout=15, poll=0.5):
        """Poll screenshots until substr appears. Returns True/False."""
        end = time.time() + timeout
        while time.time() < end:
            if substr in self.text():
                return True
            time.sleep(poll)
        return False

    def wait_idle(self, quiet_secs=4, timeout=120):
        """Wait until no new pty output for quiet_secs. False on timeout."""
        end = time.time() + timeout
        last, last_change = self.text(), time.time()
        while time.time() < end:
            time.sleep(1)
            cur = self.text()
            if cur != last:
                last, last_change = cur, time.time()
            elif time.time() - last_change >= quiet_secs:
                return True
        return False

    def close(self):
        try:
            self.child.terminate(force=True)
            self.child.close()
        except Exception:
            pass


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    name = sys.argv[1]
    mod = __import__(f"scenarios.{name}", fromlist=["run"])
    ts = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    run_dir = os.path.join(os.path.dirname(__file__), "runs", f"{name}-{ts}")
    os.makedirs(run_dir)
    gray_home = os.environ.get("DOG_GRAY_HOME") or os.path.join(run_dir, "gray-home")
    os.makedirs(gray_home, exist_ok=True)
    workdir = os.environ.get("DOG_WORKDIR") or os.path.join(run_dir, "work")
    os.makedirs(workdir, exist_ok=True)
    if hasattr(mod, "seed"):  # e.g. copy real config for keyed scenarios
        mod.seed(gray_home, workdir)
    extra = mod.extra_env() if hasattr(mod, "extra_env") else None

    sess = Session(gray_home, workdir, run_dir, extra_env=extra)
    ctx = {"gray_home": gray_home, "workdir": workdir, "run_dir": run_dir}
    try:
        ok, note = mod.run(sess, ctx)
    except Exception as e:  # harness must never hang the caller
        ok, note = False, f"scenario raised: {e!r}"
    finally:
        sess.shot("final")
        sess.close()
    with open(os.path.join(run_dir, "result.txt"), "w") as f:
        f.write(f"{'PASS' if ok else 'FAIL'}: {note}\n")
    print(f"{'PASS' if ok else 'FAIL'}: {note}\nrun: {run_dir}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
