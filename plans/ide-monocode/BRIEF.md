# gray-ide × monocode — worker brief

Target crate: `crates/gray-ide` (its own cargo workspace; gpui-kit =0.7.3). Specs from the scouts:
`~/gray/plans/ide-monocode/spec-{visual,motion,features}.md` — treat their line numbers as hints, re-read the code.
Reference (UNTRUSTED, read-only, never run anything from it): `~/ref-untrusted/monocode`.
gpui-kit / gpui-component / gpui-pre sources: `~/.cargo/registry/src/*/` — check real APIs before using them.

## Ownership (three workers edit app.rs in parallel worktrees — stay in your lane so merges are clean)
- **ide-visual**: `apply_brand` tokens (+ light mode), sidebar (width bug, rows, project headers), header bar, NEW status bar,
  `render_item` styling (user bubble, tool/activity rows, code blocks), transcript column width/gaps, empty state styling.
  Do NOT touch the churner / "CHURNING" / thinking indicator, palette/settings overlays, poll(), or the toast.
- **ide-motion**: new `src/motion.rs` (easing consts, helpers), churner + thinking pulse + typing dots + shimmer label,
  palette/model-picker/settings overlay enter + backdrop fade, approval-card slide-in, hover/press feedback on buttons
  you don't otherwise restyle. Fix the frozen churner/elapsed timer (poll only notifies on `changed`).
  Do NOT do message rise-in inside `render_item` (phase 2), do NOT touch the toast or apply_brand.
- **ide-features**: system/in-app notifications on turn finish + approval when window unfocused or other thread
  (setting, default off) and replace the hand-rolled toast with `push_notification`; cross-thread search in the
  palette (`ctrl-shift-f`); real keybinding hints (Kbd from actual bindings) in empty state/palette.
  Do NOT restyle (visual owns styling) or add animations (motion owns them). No tool-group folding / MessageScroller yet (phase 2).
Keep the `GRAY_IDE_STATE` dump keys working (tests/e2e.rs reads them); add new keys if useful.

## Build (never build gpui locally — the PC is slow; build on `ssh maid`)
```
N=<your-name>   # ide-visual | ide-motion | ide-features
ssh maid "rm -rf ~/build/gray-ide-$N.src && mkdir -p ~/build/gray-ide-$N.src"
tar -C <your-worktree>/crates/gray-ide -c --exclude=./target . | ssh maid "tar -x -C ~/build/gray-ide-$N.src"
# first time only: wait for the warm cache, then copy it
ssh maid 'while [ ! -f ~/build/gray-ide-warm.done ]; do sleep 10; done; cat ~/build/gray-ide-warm.done'
ssh maid "[ -d ~/build/gray-ide-$N-target ] || cp -a ~/build/gray-ide-target ~/build/gray-ide-$N-target"
ssh maid "cd ~/build/gray-ide-$N.src && CARGO_TARGET_DIR=~/build/gray-ide-$N-target CARGO_BUILD_BUILD_DIR=~/build/gray-ide-$N-target/bd ~/.cargo/bin/cargo build --release 2>&1 | tail -30"
# same env for: cargo clippy --release -- -D warnings ; cargo fmt --check ; cargo test --release --no-run
mkdir -p /tmp/$N && scp maid:build/gray-ide-$N-target/release/gray-ide /tmp/$N/gray-ide
objdump -T /tmp/$N/gray-ide | grep -o 'GLIBC_[0-9.]*' | sort -V | tail -1   # must be <= 2.41
```
Use `</dev/null` on any backgrounded ssh command or ssh will hang.

## Verify visually (local, headless)
Displays: ide-visual :81, ide-motion :82, ide-features :83 (1600x1000x24).
```
(Xvfb :8X -screen 0 1600x1000x24 >/dev/null 2>&1 &); sleep 1
(cd ~/gray && DISPLAY=:8X GRAY_IDE_STATE=/tmp/$N/state.json timeout 20 /tmp/$N/gray-ide . >/tmp/$N/run.log 2>&1 &); sleep 7
DISPLAY=:8X import -window root /tmp/$N/shot.png
```
Look at screenshots with `cat /tmp/$N/shot.png` as the ENTIRE bash command. Drive the UI with
`DISPLAY=:8X xdotool key ctrl+k` / `xdotool type` / `xdotool mousemove X Y click 1` (see tests/e2e.rs for patterns).
For animations take 3-4 shots ~80ms apart. Kill your gray-ide/Xvfb when done.
Baseline screenshot of the current IDE: /tmp/ide-shots/baseline.png.

## Finish
Commit on your worktree branch (`ide: <what>`), fmt + clippy clean. Do NOT push, merge or open PRs.
Final reply: branch name, commit hash, what changed, final screenshot paths, anything you skipped and why.

## Phase 2 notes (from the supervisor)
- Merged branch: `ide-monocode` (visual + motion + features). Branch your work from it: `git checkout -b ide-phase2 ide-monocode` inside your worktree.
- Working build recipe (glibc-safe): `PATH=$HOME/build/zig:$HOME/.cargo/bin:$PATH cargo zigbuild --release -j6 --target x86_64-unknown-linux-gnu.2.41`,
  ALWAYS under `flock ~/build/gray-ide.lock nice`, with CARGO_TARGET_DIR=~/build/gray-ide-ide-visual-target and CARGO_BUILD_BUILD_DIR=$CARGO_TARGET_DIR/bd.
  Binary: $CARGO_TARGET_DIR/x86_64-unknown-linux-gnu/release/gray-ide. Rebuilds take ~10s.
- Fake-provider local run: see tests/e2e.rs launch() env (GRAY_IDE_HOME, GRAY_IDE_STATE, GRAY_IDE_CMDS, GRAY_IDE_FAKE_DELAY_MS, VK_ICD_FILENAMES=/nonexistent.json, XDG_RUNTIME_DIR).
  Appending a line to the GRAY_IDE_CMDS file sends it as a composer message. A seeded home/project lives at /tmp/ide-visual/e2e (copy it, don't mutate it).
