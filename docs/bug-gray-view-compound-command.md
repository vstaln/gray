# `gray view` silently loses the vision part inside a compound bash command

Filed from a live session (gray 0.1.5, commit `d6dd776a`, Linux/X11). Reproduced
many times in one session; the plain form still works.

## Symptom

An agent calling bash with `gray view` as part of a compound command gets back
text that says the image was shown, while no image is attached to the turn:

```
$ gray view /tmp/phone.png
Image shown: /tmp/phone.png            # vision part attached — works

$ cd /home/vstaln/streamclips && gray view inspect_day23/t3230.jpg
viewed inspect_day23/t3230.jpg         # exit 0 — NO image attached
```

Both exit 0. Only the text differs, and the failing text ("viewed …") reads like
success. The agent keeps re-running the command believing `gray view` is broken
(this session wasted ~10 calls on it, and the user saw the same confusion).

## Root cause

`image_command_with_native_cap()` in
`crates/gray-tools/src/shell/tools/bash.rs` (~line 411) claims the command by
splitting on whitespace and requiring the **first** token to be `gray` and the
second to be `view`:

```rust
let mut parts = command.split_whitespace();
let (cmd, sub) = (parts.next()?, parts.next()?);
...
let (paths, native) = match (cmd, rest.is_empty()) {
    ("gray", false) if sub == "view" => { ... }
    _ => return None,          // `cd x && gray view y` lands here
};
```

The claim happens before the shell runs (by design: flags, pipes, redirects,
globs, `$`, quotes all fall through). So anything that is not a bare
`gray view PATH…` — a `cd … &&` prefix, `;`, a `for` loop, `2>&1`, `$var` in a
path — falls through to a normal shell run. The CLI then prints
`viewed <path>` (`crates/gray/src/view.rs::shown_line`), which is honest wording
for a *human* on a Kitty-protocol terminal but a lie to an agent: nothing was
attached.

The tool description already warns about the shape ("bare paths only, so pipes,
globs, `$`, quotes and flags fall through to a normal run"), but the fall-through
is silent and misleading.

## Reproduce

```sh
# works: whole command is `gray view` + bare absolute paths
gray view /tmp/a.jpg

# silently text-only, exit 0, prints "viewed …":
cd /tmp && gray view a.jpg
gray view /tmp/a.jpg 2>&1 | cat
for f in /tmp/*.jpg; do gray view "$f"; done
```

## Fix options

1. **Cheap, recommended:** when a command contains a `gray view` token run but
   was *not* claimed (compound, flag, `$`, glob), append a one-line note to the
   output: `note: gray view ran as a plain shell command here, so the image was
   NOT attached to this turn. Re-run it as the whole command with bare paths:
   gray view /abs/path.jpg`. Same treatment for the `cat <media>` pointer path.
   This turns a silent misleading success into one self-correcting turn.
2. **Better, more work:** claim the *last* `gray view PATH…` segment of an `&&`/`;`
   chain: run the prefix in the shell (for `cd`), then attach the paths, resolving
   them against the cwd the prefix produced. Keeps the "no shell interpretation of
   the paths themselves" rule intact.
3. Either way: make the CLI's fallback line say what actually happened on a
   non-Kitty terminal, e.g. `viewed <path> (not shown: terminal has no image
   protocol)` instead of a bare `viewed <path>`.

## Notes

- `--frames N` before the paths also falls through (documented at the claim site:
  the flag's argument is indistinguishable from a path).
- Plain multi-image claims (up to 8 paths / 20 MiB) do work: `MAX_IMAGE_CLAIM_PATHS`,
  `MAX_IMAGE_CLAIM_BYTES`.
