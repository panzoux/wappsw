# `hotkey=AltTab` — replacing the native Alt+Tab switcher

Design doc for roadmap item [C3](../ROADMAP.md#-c3--tab--shifttab-in-the-list-and-replacing-alttab-outright).
Implemented in [src/alttab_hook.rs](../src/alttab_hook.rs).

## Goal

An opt-in second value for `hotkey=`: `AltTab` (alias `Alt+Tab`). When set,
wappsw intercepts the real Alt+Tab / Shift+Alt+Tab sequence instead of
watching a single key. The default (`CapsLock`) stays the default; this is
never on unless configured.

## Why not just add "AltTab" to the single-key table

Every other hotkey in `config.rs` is *press to open, tap again to confirm*
— that's what [hook.rs](../src/hook.rs) and `popup::on_hotkey` implement.
Native Alt+Tab is a different interaction entirely: *hold Alt, tap Tab
(repeatably) to advance, release Alt to commit*. Making that work means
tracking Alt's own up/down as the commit signal — a small state machine,
not a new row in the key table. That's why this is its own module
(`alttab_hook.rs`) rather than a new case inside `hook.rs`.

## Config surface

- `hotkey=AltTab` or `hotkey=Alt+Tab` (case-insensitive, same normalization
  as other key names). Mutually exclusive with every `SingleKey` value —
  `config::HotkeyMode` is an enum, and `main.rs` installs exactly one of
  `hook::install` (single key) or `alttab_hook::install` (Alt+Tab), never
  both. This is what makes "sets `hotkey=AltTab`" also mean "CapsLock's
  hook never installs" — there was never a second, independent switch
  needed for that; it falls out of the existing one-hotkey-at-a-time
  design.
- `RegisterHotKey` is not used — it cannot reliably claim Alt+Tab away from
  the shell. This is a `WH_KEYBOARD_LL` hook, same mechanism as the
  single-key path.
- No new config for the emergency-disable chord (`Ctrl+Shift+F12`, see
  below) — it's always active whenever `hotkey=AltTab` is, not configurable
  in v1, the same way Esc/Enter/Ctrl+Q inside the popup aren't configurable
  either (see `popup.rs`'s `edit_subclass_proc` doc comment).

## Threading

**Deviation from the originally sketched spec, approved before
implementation:** no dedicated thread, no channel. The hook runs on the
same thread and message loop as everything else, exactly like `hook.rs`
today — which already calls into `popup::on_hotkey()` (including a full
`EnumWindows` window-list rebuild) directly from inside its
`WH_KEYBOARD_LL` callback, with no observed hook-timeout problems in
practice. The Alt+Tab state machine below does less work per event than
that already does, so there's no concrete reason to add cross-thread
synchronization for a first cut. Revisit only if real timeouts show up.

## State machine

One module-level flag in `alttab_hook.rs`: `DISABLED` (has the emergency
chord fired). Whether a session is "in progress" is **not** tracked with a
second flag — it's read straight from `popup::is_open()`
(`IsWindowVisible`) every time, which is why the table below never says
"session active" as its own condition.

**This wasn't the first cut.** The first version kept an independent
`SESSION_ACTIVE` boolean set by Tab and cleared by Alt-up/Esc/the F12
chord. It desynced from the popup's real state whenever the popup closed
through a path that doesn't go through this hook at all — `Enter` typed
into the edit control (handled directly by `popup.rs`'s
`edit_subclass_proc`) or the `autoswitch` timer firing — leaving the flag
stuck `true`. Every later bare Alt-up was then wrongly treated as "commit a
session" and swallowed (bare Alt stopped focusing menus at all), and every
later Alt+Tab press was wrongly treated as "advance an existing session"
instead of opening (Alt+Tab appeared to just stop working). Checking
`popup::is_open()` directly instead removes the second source of truth
that could ever drift, since every closing path already updates it
correctly.

| Input | Condition | Action |
| --- | --- | --- |
| Alt down | any time | pass through untouched |
| Alt up | popup not open | pass through untouched — a bare Alt tap (e.g. menu-bar focus) keeps working |
| Alt up | popup open, not detached | swallow the real event; `popup::alttab_commit()`; synthesize a replacement (decoy Ctrl tap + synthetic Alt-up) — see "Alt getting stuck" and "menu-bar flash" below |
| Tab down | Alt not held | pass through untouched — plain Tab elsewhere is not ours |
| Tab down | Alt held, popup not open | swallow; `popup::alttab_open()` |
| Tab down | Alt held, popup already open | swallow; `popup::alttab_advance(-1 if Shift else 1)` |
| Tab up | Alt held | swallow always (paired with every swallowed Tab down, so no stray keyup reaches anything else) |
| Esc down | popup open | swallow; `popup::alttab_cancel()`. Alt is likely still physically held; its eventual release passes through normally since the popup is already closed |
| `Ctrl+Shift+F12` down | any time | **toggle** `DISABLED`; if turning it on and the popup was open, cancel it first. Checked before everything else, including the `DISABLED` short-circuit itself, so it works both mid-session and while already disabled — otherwise there'd be no way back on short of restarting |
| anything, while `DISABLED` | — | pass straight to `CallNextHookEx`, unconditionally — native Alt+Tab works again |

**`Ctrl+Shift+F12` is a toggle, not a one-way switch** (changed after the
first round of manual testing): a one-way disable left the process running
but doing nothing, with no way to get the popup back short of restarting —
raised directly as "the app that does nothing is of no use". Pressing the
chord again turns interception back on.

**Committing trusts the keyup directly, without re-checking
`GetAsyncKeyState(VK_MENU)`.** An early version additionally required
`!is_down(VK_MENU)` ("fully up, covering both L/R") before committing, and
in manual testing this *never once* fired across many long sessions —
`GetAsyncKeyState`, queried from inside Alt's own keyup event, reported
"still down" every single time. This is the same category of
self-referential staleness [hook.rs](../src/hook.rs) already documents for
`GetAsyncKeyState(VK_SHIFT)` queried during Caps Lock's own event on JIS
layouts, just for a different key and (evidently) not limited to that one
quirky layout. The fix: a keyup message existing at all means that
physical key is now up, so act on it directly. The one case this gets
wrong — both Alt keys held, releasing only one — is accepted as out of
scope rather than reintroducing the staleness risk to handle it.

`popup::alttab_open/advance/commit/cancel` are thin `pub(crate)` wrappers
added to `popup.rs` around its existing private `open`/`move_selection`/
`confirm_selection`/`hide` — no popup UI or rendering changes.
`popup::is_open()` is the same kind of wrapper around `IsWindowVisible`.
The first Tab always lands on `default_selected` (index 1, "previous
window"), **regardless of whether Shift was already held on that first
press** — see Known simplifications.

`DISABLED` never resets at runtime; there's no UI or config to turn Alt+Tab
interception back on short of restarting the app, mirroring how a
misconfigured single-key hotkey today has no in-app recovery either
(Task Manager, or the documented `--quit`-style fallbacks in the roadmap).

## Interaction with `autoswitch`

`autoswitch` (a unique match auto-confirms after a delay) directly
conflicts with Alt+Tab's commit-on-release model if left alone: it would
otherwise auto-confirm out from under a still-held Alt, which looks exactly
like "releasing Alt does nothing" (the popup is already closed by the time
Alt actually comes up).

**First fix attempt, and why it wasn't enough.** `popup::alttab_open()`
initially called `KillTimer` right after `open()` returned, to cancel
whatever `open()` had just armed. This mostly worked, but not reliably:
`open()` clears the query text via `SetWindowTextW`, which fires an
`EN_CHANGE` notification that's delivered *asynchronously* through the
message queue — not synchronously inside the `SetWindowTextW` call. By the
time that queued notification reaches `refilter()` (which can be several
Tab keydowns later, since the low-level hook callback has to return and
the message loop has to pump before a queued `WM_COMMAND` is dispatched),
it calls `update_auto_switch_timer` again and re-arms the very timer that
had already been killed. Confirmed in a log from manual testing: `"1
match, switching in 500 ms"` followed by `"timer fired, still 1 match:
true"` appeared several Tab presses into an otherwise-normal session.

**Actual fix:** `update_auto_switch_timer` itself now refuses to arm the
timer at all while Alt is physically held (`GetAsyncKeyState(VK_MENU)`),
regardless of which caller (`open`, `refilter`, at whatever point in time)
triggered it. This closes the gap regardless of call ordering, and also
generalizes correctly beyond `hotkey=AltTab`: committing on a timer while
Alt is down conflicts with any hold-to-browse interaction, not just this
one. `alttab_open()` no longer needs its own `KillTimer` call as a result.

## `Alt+Q`: detaching into filter mode

Raised directly after the first round of manual testing confirmed the core
commit/cancel/toggle mechanics worked: a pure Alt+Tab clone loses wappsw's
actual strength (migemo filtering), since the hold-to-browse model has no
typing built in.

`Alt+Q`, while a session is open, sets a `DETACHED` flag and swallows the
keystroke. From that point on, for the rest of this session:

- Alt's own keyup no longer commits — the popup stays open regardless of
  when (or whether) Alt is released, and behaves exactly like the
  single-key path from here: type to filter (already worked throughout,
  since letter keys were never intercepted by this hook to begin with —
  only Tab/Alt/Esc/F12/Q are), `Up`/`Down`/`Enter`/`Esc`/`Ctrl+Q` via
  `popup.rs`'s existing `edit_subclass_proc`.
- Tab stops being intercepted too (becomes an ordinary key, same as in
  single-key mode) — it's no longer meaningful as "advance" once Alt isn't
  the commit signal.

`DETACHED` resets to `false` every time a fresh session opens (the `!open
-> open` branch), so it's scoped to one session, not sticky across the
whole run.

## Alt getting stuck "held" system-wide after a switch, and the menu-bar flash

Two more bugs from manual testing, both about the committing Alt-up, and
each fix for one reintroduced the other until they were solved together.

**Bug 1: Alt stuck "held" system-wide.** After committing, Windows itself
(and every other app) kept behaving as if Alt was still physically held —
e.g. the Left arrow key acting like `Alt+Left` — until the user pressed
Alt again.

*Cause:* the committing Alt-up event was swallowed (`return 1`, never
`CallNextHookEx`). `WH_KEYBOARD_LL` fires *before* the rest of the input
pipeline, including whatever downstream code updates the keyboard state
`GetAsyncKeyState`/`GetKeyState` (and every other app) reads from — so
swallowing that event means that downstream update never runs, and the
system-wide state stays stuck at "Alt down" until a later, correctly
forwarded Alt-down/up cycle resets it (matching "pressing Alt again will
unlock" exactly).

*First fix attempt:* always call `CallNextHookEx` for the real Alt-up
regardless, fixing the stuck state. This reintroduced a second bug:

**Bug 2: the newly-focused window's menu bar flashes/focuses.** Real
Alt+Tab doesn't do this — many apps decide whether a bare Alt-up should
focus their menu bar by checking whether any *other* key was pressed
during the hold. Tab's own events never reach that check, because this
hook swallows them entirely (never forwarded to anyone). So passing the
real Alt-up straight through makes the window just switched to look
exactly like it received a bare Alt tap.

**Actual fix, solving both at once:** the real Alt-up is swallowed (as
originally), but immediately replaced with a *synthetic* one — a decoy
`Ctrl` tap followed by a synthetic Alt-up matching the real event's own
`vkCode`, sent together as a single `SendInput` batch. A single call's
events are guaranteed delivered in order with nothing interspersed, which
matters here: the decoy has to be seen as happening *before* the Alt-up,
not after. The synthetic Alt-up goes through the full normal pipeline like
any other input, fixing bug 1; the decoy Ctrl tap satisfies the "was
another key pressed" check, fixing bug 2, without typing or doing anything
else visible.

**Reentrancy note.** The synthetic Alt-up loops back through this same
hook (`SendInput` events pass through low-level hooks same as real input).
By the time it arrives, `popup::alttab_commit()` has already run and
closed the popup, so `popup::is_open()` is false and the synthetic event
just falls through to an ordinary pass-through rather than re-triggering a
commit — no explicit tagging of synthetic events was needed to prevent a
loop.

## Alt+Tab permanently stopped working after using Alt+Q once, until restart

Reported as "`Ctrl+Shift+F12` disable, then re-enable, and Alt+Tab needs a
restart to work again" — the trigger turned out to be broader than just the
`F12` chord.

*Cause:* the Tab-down handler's outer condition was
`is_down(VK_MENU) && !DETACHED.load(...)`, guarding *both* "open a new
session" and "advance an existing one" behind the same check. `DETACHED` is
only ever cleared inside the "open a new session" branch itself. So once a
session was detached (`Alt+Q`) and then closed through any path that
doesn't go through this hook — `Enter`/`Esc`/`Ctrl+Q` in the edit control
(`popup.rs`'s `edit_subclass_proc`), or the `Ctrl+Shift+F12` emergency
disable cancelling it — `DETACHED` stayed `true` forever. The next Alt+Tab
press then failed `!DETACHED.load(...)` before ever reaching the branch that
would have reset it, and fell straight through to plain pass-through:
permanently, since nothing else in the hook ever clears `DETACHED`.

*Fix:* the gate now checks `!popup::is_open() || !DETACHED.load(...)` — a
*closed* popup can always be reopened regardless of whatever `DETACHED` was
left at, matching the file's own "single source of truth" principle already
used for session-active tracking (see the state machine section above).
`Ctrl+Shift+F12` also now force-resets `DETACHED` as a second layer of
defense. Not yet confirmed against the user's exact repro — needs a retest.

## Known simplifications (deliberately deferred, not fixed here)

- **Shift held on the very first Tab of a session** does not reverse the
  initial jump — it always opens onto index 1 first, same as every other
  session start, and only affects direction from the *second* Tab onward.
  Real Windows Alt+Shift+Tab held from the start jumps backward
  immediately. Matching that exactly is a minor UX nuance, not core
  correctness; can be revisited if it's noticeable in practice.
- **No visual difference** between Alt+Tab mode and the normal popup — it's
  the same window, same rendering, same typing-to-filter behavior (typing
  while Alt is held will filter the list same as ever, since only Tab/Esc/
  Alt itself are intercepted specially).

## Real risks, not yet resolved by design alone

- **Hook ordering.** `WH_KEYBOARD_LL` hooks fire in reverse install order
  (most recently installed first). wappsw is normally started well after
  logon, so it should usually see Alt+Tab before the shell's own handling
  — but this isn't guaranteed, and if some other tool installs a
  lower-level hook later, a brief native-switcher flash before suppression
  is possible. Only observable by testing on real hardware.
- **Synthetic-input testing is unreliable for this specific combo.**
  Windows treats Alt+Tab specially even for injected input in ways that
  don't always match real hardware scan-code behavior (the same class of
  gotcha [hook.rs](../src/hook.rs)'s doc comments already document for
  Caps Lock on JIS layouts). `tests/popup-keyboard.ps1`-style `keybd_event`
  injection may only partially exercise this; manual testing on real
  hardware is expected to be the primary verification method.

## Opening the popup on top of Notepad needed two Alt+Tab presses

Reported: switching *away from* Notepad (to open wappsw's popup) sometimes
needed two full Alt+Tab gestures before the popup's selection was actually
usable — other apps tested only needed one. Root-caused from a `-log`
capture (`popup::open`/`popup::confirm_selection`/`switch::force_foreground`
diagnostics added while chasing this):

```
alttab_hook: Alt+Tab -- opening
switch::force_foreground: attempt 1/5 target=<popup> prior_fg=<notepad> attached=true ok=false
switch::force_foreground: attempt 2/5 target=<popup> prior_fg=<other>   attached=true ok=true
popup::open: force_foreground -> true, now foreground=<popup>
alttab_hook: Alt+Tab -- opening        <-- a SECOND "opening" before any commit
popup::open: force_foreground -> true, now foreground=<popup>
alttab_hook: Alt released -- committing
```

*Cause:* reclaiming foreground from Notepad specifically needed a second
`SetForegroundWindow` attempt (attempt 1 was rejected) inside
`switch::force_foreground`'s retry loop, each attempt wrapped in its own
`AttachThreadInput` attach/detach pair. That extra attach/detach churn has a
side effect: it generates a `WM_ACTIVATE`/`WA_INACTIVE` notification for the
popup, delivered *asynchronously* (queued, not synchronous with the
`SetForegroundWindow` call that produced it) — so it lands on the popup's
`wndproc` moments *after* `open()` has already returned and successfully
shown it. `wndproc`'s `WM_ACTIVATE` handler unconditionally called `hide()`
on any `WA_INACTIVE`, immediately hiding the popup that had just opened.
`is_open()` (a plain `IsWindowVisible` check) then correctly reports `false`
to the very next Tab-down, so the hook treats it as a fresh session and opens
again from scratch — which is what the user perceived as "needing to press
twice." Apps that don't need a retried `SetForegroundWindow` attempt never
generate this spurious deactivation, which is why only some apps (so far
just Notepad) showed it.

*Fix:* the `WM_ACTIVATE` handler now skips `hide()` while Alt is physically
held (`alt_held()`, a new small helper factored out of
`update_auto_switch_timer`, which already uses the exact same guard for the
exact same class of problem — see "Interaction with `autoswitch`" above). A
real dismissal (`Esc`, `Enter`/`Ctrl+Q` in the edit control, the Alt-release
commit, or `Ctrl+Shift+F12` disabling) never depends on this handler, so
nothing legitimate is suppressed by the guard.

## `Ctrl+Alt+Tab` / `Win+Alt+Tab` also open wappsw's popup

Expected, not a bug: this hook only checks whether `Alt` is held when `Tab`
arrives — it never looks at `Ctrl` or `Win`. Native Windows Alt+Tab has the
same non-distinction (that's *why* `Ctrl+Alt+Tab` opens the OS switcher at
all — Ctrl only changes whether releasing Alt commits immediately or leaves
the switcher "sticky" open). Matching that "sticky" behavior for
`Ctrl+Alt+Tab` specifically would be a real feature, not a bug fix — noted
here as a possible future roadmap item, not undertaken.

## Test plan

Manual, with `-log` on, `hotkey=AltTab` set:

1. Bare Alt tap (press, release, nothing else) — nothing happens, and
   whatever Alt normally does (e.g. focus a menu bar) still happens.
2. Alt+Tab (single tap) — popup opens on index 1.
3. Alt held, Tab tapped repeatedly — selection advances each time, wrapping
   at the ends.
4. Alt held, Shift+Tab — selection moves backward.
5. Release Alt — commits (switches to the highlighted window, popup
   closes) — same as `Enter` today.
6. Esc while Alt still held — cancels (popup closes, no switch); releasing
   Alt afterward does nothing further.
7. `Ctrl+Shift+F12` — disables; native Alt+Tab (the real OS switcher) works
   again; pressing the chord again re-enables it.
8. After a commit (step 5), check Alt-modified behavior elsewhere (e.g.
   `Alt+Left`/`Alt+Right` in a browser, or a menu-bar app) works normally —
   confirms Alt isn't stuck "held".
9. Alt held, `Alt+Q` — popup stays open after releasing Alt; typing filters
   the list via migemo; `Up`/`Down`/`Enter`/`Esc` behave as in single-key
   mode.
10. Regression for the "stuck after Alt+Q" fix: `Alt+Q` to detach, then
    `Enter` (or `Esc`, or `Ctrl+Q`) to close that detached session, then a
    fresh Alt+Tab — must open normally, not silently do nothing.
11. Regression for the same fix via the `F12` path: get a session detached
    (`Alt+Q`), then `Ctrl+Shift+F12` to disable while it's still open, then
    `Ctrl+Shift+F12` again to re-enable, then a fresh Alt+Tab — must open
    normally.
12. Regression for the "needed Alt+Tab twice on Notepad" fix: with Notepad
    focused, press Alt+Tab once — the popup must open and be usable
    (selection visible, Tab advances it) on that single press, not just on a
    second one.
