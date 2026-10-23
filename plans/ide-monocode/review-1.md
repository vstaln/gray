I found 6 bugs on `ide-monocode`; 2 are serious: a panic on user text and a constant redraw when the app should be idle. I didn't change any files. I checked everything by reading the diff and the gpui source; I didn't build the branch or run `tests/e2e.rs`.

### 1. Panic in the search preview (`app.rs:5168–5176`, `snippet`)
`hit = lower[..at].chars().count()` counts characters in the **lowercased** text. That index is then used on the **original** text's characters (`chars[start..end]`). Lowercasing can add characters: `İ` (Turkish) becomes `i̇`, which is 2 characters. So `hit` can be larger than the original's length, which puts `start` past `end` (`end` is capped at `chars.len()`). Example: a message of 100×`İ` followed by `ab`, searched for `ab`, gives `start=174`, `end=102`, and the app panics while you type in Ctrl+Shift+F. The byte slice `lower[..at]` is safe because `find` returns a char boundary; the character mapping is the problem.
**Fix:** cap it, e.g. `let start = hit.min(chars.len()).saturating_sub(width / 3);`. Or search a lowercased `Vec<char>` with one character per original character (`c.to_lowercase().next()`) so the indexes line up.

### 2. Redraws at 30 Hz forever on an empty or new thread (`app.rs:964–975`)
`scroll_to_bottom()` only sets a flag; it takes effect when the timeline is next laid out (gpui-pre `div.rs:4392`). The welcome screen, shown for an empty thread or no thread, never renders the `#timeline` div, so the offsets go stale and never change. To trigger it: scroll up in thread A, then press Ctrl+N. `follow` becomes true, the "not at bottom, offset unchanged, follow" branch sets `changed = true`, and that branch then fires every poll. Result: `cx.notify()` every 33 ms, plus a full re-render and `write_snapshot` while idle.
**Fix:** only re-pin when the content actually grew, by tracking `last_max` and requiring `max_offset().y != last_max`. Or skip this block when the thread is empty.

### 3. Jumping to a search hit can land on the wrong row (`app.rs:1011–1022`)
`ix` is shadowed with the **row** index. If `bounds_for_item` fails at try 2 or later, line 1022 saves that row index back into `scroll_target`. The next retry treats it as an item index and converts it again, so it moves to an earlier row whenever tool groups sit before the hit.
**Fix:** name it `row` and keep saving the original item `ix` on retry.

### 4. Two notifications for one failed turn (`app.rs:864` and `886`)
On `Err`, `notify_turn` pushes an in-app and system "Turn failed" notification. The existing `self.toast("Turn failed: …")` then queues a second in-app error notification in the same poll. With notify on and another thread on screen, the user sees two in-app error cards.
**Fix:** skip `notify_turn` for errors when the toast already fires, or skip the toast when `notify_turn` fired. Keep `self.toast = Some(...)`, because `e2e.rs:700` checks `s["toast"]`.

### 5. Rise-in base isn't reset when no thread is open (`app.rs:2883`)
`(None, _) => usize::MAX` leaves the saved `(thread_id, len)` alone. Going from thread A to no thread and back to A keeps A's old count, so items that arrived meanwhile replay the rise-in. Minor.
**Fix:** `*b = None` in that arm.

### 6. Text too faint in light mode
On a white background these opacities fall roughly below 3:1 contrast:
- `ink.opacity(0.32)` for "· N steps" (`app.rs:3143`)
- the activity detail at 0.4
- the sidebar thread timestamp at `sidebar_foreground.opacity(0.4)`
- "No threads yet" at `muted_foreground.opacity(0.6)`, which stacks on a colour that's already muted
- the inactive project name at `muted.opacity(0.7)`

**Fix:** use about 0.5 or more in light mode (`t.mode.is_dark()`), or plain `muted_foreground` without extra opacity.

### Checked and fine
- **Approval notifications:** once per approval event. Finish notifications: once per finished turn.
- **Animations:** the looping ones are capped at 30 fps and stop once they're off screen. The terminal's running bars stop because finished commands are removed from `terminal_rx`. Palette and settings fade-ins replay on each open, which is intended.
- **State dump:** the snapshot only gains keys (`groups`, `following`, `notify`) and `toast` is kept, so `e2e.rs` lookups like `activities`, `label` and `toast` should still work.
- **`group_info` indexing** is safe because groups are never empty.