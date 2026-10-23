# gray-ide vs monocode: what's missing in features/UX, and the top 6 to build

I only read code: no files edited, nothing built, nothing from monocode run. In refs below, `app.rs` is `~/gray/crates/gray-ide/src/app.rs` and `gc/` is `~/.cargo/registry/src/*/gpui-component-0.7.3/src`.

## What gray-ide already has
- **Palette:** a hand-rolled command palette and file picker (`app.rs:856-1100`, substring/fuzzy match at :1953).
- **Composer:** slash commands (:578), `@`-mentions (:4486), image attachments, and chips for model, mode and branch.
- **Editor:** editor tabs, a separate viewer, and save.
- **Git:** a diff panel built on gpui-kit's `Diff` (:3184), plus checkpoint, restore, discard, turn diff, branches and worktrees.
- **Turns:** approval bar (:2662), stop, effort.
- **Panels:** sessions, skills, plugins hub, settings sections, themes and backgrounds, snooze/settle/pin/archive, 19 keybindings (:52).

## Missing or weak
- **Tool activity:** every tool call is its own flat one-line row (`render_item`, :4229-4266). Nothing is grouped or folded.
- **Auto-scroll:** the timeline jumps to the bottom on every streamed change (:833). If you scroll up to read, it pulls you back down.
- **Notifications:** nothing tells you when a turn finishes or needs approval while the window is in the background. The only toast is one hand-made `Option<(String,bool)>` (:4459).
- **Search:** no way to search across threads (conversation titles or message text).
- **Shortcut hints:** typed in by hand ("Ctrl+N" strings, :899), so they can drift from the real bindings.
- **Terminal:** line-based, no PTY (`workspace::run_shell`, :838), fixed 240 or 300 px. A real PTY is a big job, so it's not in the top 6.
- **Not in gray-ide (from monocode):** BTW side-questions (`sessions/model/btw.ts`), prompt outline (`sessions/ui/PromptOutline.tsx`), quick composer (global hotkey), notes, inbox (GitHub/Jira PRs), per-hunk stage/revert. gpui-kit's `Diff` has no hunk callbacks (`gc/diff/*`), so hunk staging means building our own rows.

## Top 6, ranked by delight ÷ effort

**1. Notify on finish or approval when you're not looking (S)**
- **Rule** (monocode `notifications/model/notifications.ts:96-109`): notify only when it's enabled AND (the window isn't focused OR the thread isn't the one on screen). Default is off (:14).
- **Text** (:169-207): title "Gray"; subtitle is the thread title; body is the first ~120 characters of the reply, "Approve: {label}", or "{provider} finished".
- **API:** gray-ide's window is already wrapped in `Root` by `gpui_kit::open_window` (`gpui-kit/src/lib.rs:185`), so `window.push_notification(Notification::success(body).title(t).in_app_and_system(), cx)` works (`gc/window_ext.rs:212`, `gc/notification.rs:316`). Linux backend: `gpui-pre-linux/src/linux/system_notifications.rs`. Focus check: `window.is_window_active()` (gpui-pre `window.rs:2973`).
- **Where:** in `poll()` at the `finished`/`approvals` handling (app.rs:679-700). Clicking the notification (`.on_click`, notification.rs:327) sets `active_thread = tid`.
- **Also:** replace the custom toast (:449, :4459) with `push_notification(Notification::info/error)`.

**2. Fold tool activity into a "Worked for 1m 12s" group (M)**
- **Model** (monocode `transcriptActivity.ts:494 groupTurnItems`): consecutive Activity items between messages become one group.
- **Label** (`AgentTranscript.tsx:4825-4847`): "Working…" while live, "Worked for {elapsed}" when done. Elapsed: under 60s → "Ns"; otherwise "Mm Ss", or "Mm" if seconds are 0. Errors show red (map "failed/error/cancelled", :34).
- **API:** a header row with a chevron that toggles `HashSet<String>` group ids (the existing `expanded` field), body in `Collapsible::new().open(open).motion_id(id).content(rows)` (`gc/collapsible.rs:20-45`). Open while live, closed when done.
- **Where:** in `render_timeline` (:2627), replace the plain loop over items with a grouping pass.

**3. Stick to the bottom only if you're already there, with a jump button (M)**
- **API:** `MessageScrollerState::new(n, cx)` plus `MessageScroller::new("timeline", state, |ix, w, cx| …).jump_button(true).with_jump_button_label("Jump to latest").with_bottom_fade(t.background)` (`gc/message_scroller.rs:36,185,221,227,284`; fade default 200ms, :18-19).
- **Where:** call `append`/`remeasure` in `poll` instead of the unconditional `scroll.scroll_to_bottom()` (:833). `is_following_tail()` (:65) decides whether new content pulls the view down. Keep the scroll to bottom on send (:574).
- **Bonus:** it's a virtual list, so long threads stay fast.

**4. Search across threads in the palette (S/M)**
- **Model** (monocode `search/model/appSearch.ts:19-80`): scopes all, conversations, files and projects. Hit kinds: conversation (title fuzzy match), message (text preview), file. Order: conversations, then messages, then files (:402-405).
- **Where:** add `PaletteMode::Search` (:87). In `palette_items` (:882), scan `store.state` threads for title or message text containing the query, and show a preview: 80 characters around the match, plus `relative(at)` (:4307). A new `Cmd::OpenMessage(tid, msg_id)` sets the thread and scrolls to the message (`MessageScrollerState::scroll_to_item`, :139).
- **Binding:** `ctrl-shift-f`.

**5. Switch to gpui-kit's `Command` palette with real key hints (M)**
- **API:** `CommandState::new(window, cx)`; `Command::new(&state).group(CommandGroup::new().label("Thread").items([CommandItem::new().label(..).icon(..).action(Box::new(NewThread)).keywords([..])]))`, with `.on_confirm` and `.placeholder` (`gc/command/command.rs:72-222`, `item.rs:37-95`).
- **Why:** groups replace the second-column category strings, and hints come from the actual bindings instead of hardcoded text.
- **Elsewhere:** for the empty-state hints (:2570-2616), use `Kbd::binding_for_action(&NewThread, None, window)` (`gc/kbd.rs:58`).
- **Cost:** drops roughly 250 lines (:856-1127 and `render_palette` :3348).

**6. Streaming polish: shimmer the live label, fade/rise new prompts (S)**
- Swap `churner` plus "CHURNING" (:2636-2645) for `ShimmerText::new("Working · 3s").duration(Duration::from_millis(1600))` (`gc/shimmer.rs:150-196`).
- New user bubble rises: monocode uses 560ms, `cubic-bezier(0.22,1,0.36,1)` (`AgentTranscript.tsx:5355,5394`). In gpui: `.with_animation(id, Animation::new(Duration::from_millis(560)).with_easing(ease_out_quint()), |el, d| el.opacity(d).top(px(8.*(1.-d))))`. The curve is roughly ease-out-quint; check which easing helpers gpui-pre exports before using that one.
- Word fade: monocode uses 320ms, `cubic-bezier(0.33,0.53,0.67,1)` (`wordFade.tsx:13,26`). It's hard per word in `TextView`, so apply it only to the newest streamed block.

## Next after these
BTW side-question sheet, hunk stage/revert (needs custom rows), PTY terminal, notes.

## Uncertainties
- I didn't check that Linux system notifications work in the user's session, which needs a D-Bus notification daemon.
- I didn't check that `MessageScroller` handles rows whose height changes while text streams in. `remeasure_items` (:128) exists for that.
- Exact gpui easing names are unverified (see #6).