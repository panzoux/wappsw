# wappsw roadmap

Working document, not a commitment. English, to match the code comments and
commit messages.

**Status legend**

| Mark | Meaning |
| --- | --- |
| ✅ | Agreed |
| 📝 | Recorded — wanted, not scheduled |
| ❓ | Needs a decision from you |
| ✔️ | Done |
| ❌ | Decided against |
| ❄️ | Parked — recorded so it stops resurfacing |

Effort is rough: **S** ~ an hour, **M** ~ an afternoon, **L** ~ multi-session
with design decisions to settle first.

**Key naming convention.** Config key names are ASCII only, following the Win32
virtual-key names with the `VK_` prefix dropped — `CONVERT`, `NONCONVERT`,
`KANA`, `SCROLL`. Existing names (`CapsLock`, `Caps`, `ScrollLock`, `Insert`)
stay as aliases so config files already in the wild keep working. Romaji is
used in this document only as a reading aid, never as a config value.

---

## ✅ 1. `Ctrl+Q` quits the app while the list is showing

**Decided:** quits immediately, no confirmation. Not configurable — `Ctrl+Q`
is the app's own key, in the same class as `Up`/`Down`/`Esc`, which are not
configurable either. The hotkey is configurable because it must coexist with
whatever else the machine uses; keys *inside* the popup are the app's to own.
`scripts\quit.bat` is deleted as part of this change.

**Where.** [popup.rs:485](src/popup.rs:485) `edit_subclass_proc`, alongside the
existing `Esc` / `Enter` / `Up` / `Down` arms.

**Shape.**

- On `WM_KEYDOWN` with `wparam == 'Q'` and `GetKeyState(VK_CONTROL) & 0x8000`:
  `hide()`, then `PostQuitMessage(0)`. `GetMessageW` at
  [main.rs:42](src/main.rs:42) returns 0, the loop breaks, both hooks
  uninstall, the mutex is released on exit. No new teardown path needed — this
  is strictly better than `taskkill /F`, which skips
  `hook::uninstall` / `mru::uninstall` at [main.rs:52](src/main.rs:52)
  entirely.
- Swallow the trailing `WM_CHAR`. `Ctrl+Q` produces `WM_CHAR 0x11` and a
  single-line `EDIT` beeps at control characters it cannot insert — the same
  reason `0x0D` / `0x1B` are already swallowed at
  [popup.rs:511](src/popup.rs:511). Widen that test to the whole `0x01..=0x1A`
  range rather than adding `0x11` alone; 📝 C5 will need the rest of it anyway.
- Delete `scripts\quit.bat` **in the same commit**, not before. Until `Ctrl+Q`
  exists it is the only quit path.

**One consequence worth accepting deliberately.** After this, quitting requires
a working hotkey: open the popup, press `Ctrl+Q`. If the hotkey is
misconfigured or its scan code is wrong on a given keyboard, the only way out
is Task Manager. 📝 E4 (`--quit` flag) is the proper fallback, and the two are
worth doing close together.

**Effort.** S.

---

## ✅ 2. More keys in `config.ini`, including modifier combos

**Decided:** modifier combos are wanted. Several hotkeys bound at once is
wanted. ASCII key names only.

**Why the current list is three keys long.** [hook.rs:63](src/hook.rs:63)
matches on a hardware scan code, and those three scan codes are hand-written
constants at [config.rs:79](src/config.rs:79).
[config.rs:85](src/config.rs:85) `key_name_to_vk` is the whole table. Adding
keys is data entry, not new logic — until modifier combos, which are.

### 2a. The key table

Rows carry `(name, vk, scancode, extended)`. The `extended` flag is not
optional once the editing cluster is included: in PS/2 set 1, `Insert` is
`E0 52` while Numpad `0` is a bare `52`, and the hook currently compares
`scanCode` alone. Every key in the second group below has a numpad twin that
shares its scan code.

| Group | Names | Notes |
| --- | --- | --- |
| Already supported | `CAPITAL` / `CapsLock`, `SCROLL`, `INSERT` | see item 3 for the CapsLock split |
| Japanese-keyboard keys | `CONVERT` (Henkan), `NONCONVERT` (Muhenkan), `KANA` (Katakana/Hiragana), `KANJI` (Hankaku/Zenkaku), `OEM_ATTN` (Eisu) | no numpad twin, no extended flag |
| Needs `extended` | `DELETE`, `HOME`, `END`, `PRIOR`, `NEXT`, `RCONTROL`, `RMENU`, `RSHIFT`, `NUMPAD_ENTER` | shares a scan code with a numpad key |
| Function keys | `F13`..`F24` | only exist on some keyboards; scan codes need verifying |
| Awkward, verify before promising | `PAUSE`, `SNAPSHOT` (PrintScreen), `NUMLOCK`, `APPS` | `PAUSE` arrives as an `E1`-prefixed sequence and `SNAPSHOT` as a pair of `E0` events; neither is a plain scan code |
| Deliberately excluded | letters, digits, `SPACE`, `TAB`, `RETURN`, `ESCAPE`, `BACK` | the hook swallows unconditionally ([hook.rs:91](src/hook.rs:91)), so binding one of these bricks typing system-wide with no recovery but Task Manager — allowed only when a modifier is also required |

**On your own key usage.** You use `NONCONVERT` (IME off) and `CONVERT`
(IME on) heavily, so those are poor hotkey candidates *for you* — the hook
swallows the key, which would break IME switching. `KANA` is the one you do not
use, so it is the free thumb key on your keyboard. All three stay in the table
because other users split differently; only the recommendation in the README
changes.

**Build the table empirically, not from memory.** The scan codes above are from
published set-1 tables and some of them (`F13`..`F24`, `PAUSE`, `SNAPSHOT`) I
would not ship without checking. Suggest adding a `-keylog` diagnostic mode
that logs `vk` / `scanCode` / `extended` for every key pressed and swallows
nothing. It makes the table verifiable on real hardware, and it doubles as the
support tool for "the hotkey does not work on my keyboard" — which will get
more common as the table grows. **Effort.** S, and it pays for itself
immediately.

### 2b. Modifier combos

`hotkey=Ctrl+Shift+Space`. This is the larger half.

`other_modifier_held()` at [hook.rs:55](src/hook.rs:55) currently means "any of
Ctrl/Alt/Win is held, so this is not ours — pass it through". That has to
become **set equality**: the held modifier set must exactly equal the
configured set. Two things fall out:

- A press whose modifiers do not match must pass through **unswallowed**. The
  current code only ever reaches the swallow path when no modifier is held, so
  this distinction does not exist yet.
- Modifiers make the excluded-key group above bindable. `Ctrl+Alt+Space` is
  safe in a way bare `Space` never is.

### 2c. Several hotkeys at once

`hotkey` becomes a list — `hotkey=CapsLock, NONCONVERT` — or numbered keys
(`hotkey1=`, `hotkey2=`). Straightforward once the parser returns a
`Vec<Binding>` and the hook matches against all of them.

**Open question.** Should each binding be able to do something *different*
(one opens the list, another switches straight to the previous window with no
UI)? That is a genuinely useful feature and a much bigger one; worth knowing
now whether the config shape should leave room for it.

**Effort.** M for 2a and 2c, L including 2b.
**Should be preceded by** 📝 E1 — see the argument there.

---

## ✅ 3. Explicit CapsLock variants by key name

**Decided:** no runtime layout detection, no `layout=` key. The keyboard
variant is encoded in the key name, e.g. `CAPS_JAPANESE_KEYBOARD`. The real
caps-lock state is never toggled in any variant.

**Why the variants differ at all.** On a Japanese keyboard driver, a *bare*
CapsLock press never arrives as `VK_CAPITAL` — Windows translates it to the
Eisu toggle, and `VK_CAPITAL` appears only when Shift is held. That is why
[hook.rs:63](src/hook.rs:63) matches on scan code `0x3A` and why
[hook.rs:69](src/hook.rs:69) deliberately lets Shift+CapsLock trigger too. On a
US/101 driver, bare CapsLock *is* `VK_CAPITAL`, so the Shift special case is
unnecessary there and actively in the way — it consumes a chord that could be
bound to something else under 2b.

**Proposed names and behaviour.**

| Name | Triggers on | Swallows |
| --- | --- | --- |
| `CAPS_JAPANESE_KEYBOARD` | scan `0x3A`, with or without Shift | both, always — today's behaviour, unchanged |
| `CAPS_ENGLISH_KEYBOARD` | scan `0x3A` only when Shift is **not** held | both; Shift+CapsLock is swallowed but does not open the popup |

Neither variant ever passes the key through, so the lock state never toggles —
that is the "no toggling lock state" decision, and it is what makes the two
variants safe to describe purely in terms of *when the popup opens*.

**Open question.** What should bare `CapsLock` / `Caps` mean now? Suggest
keeping it as an alias for `CAPS_JAPANESE_KEYBOARD`, since that is exactly
today's behaviour and existing config files should not change meaning under
the user. The alternative — making it mean the English variant — is more
intuitive for new users but silently changes behaviour for current ones.

**Note.** Under `CAPS_ENGLISH_KEYBOARD`, Shift+CapsLock becomes available as a
distinct binding once 2b lands. Worth confirming that is what you want rather
than having it simply ignored.

**Effort.** S once item 2's parser exists. **Depends on** item 2.

---

## ✔️ Done

### ✔️ E5 — README said Windows 11 only

Updated: the app is described as Windows 10 / 11, and a 32-bit build note is
added. You confirmed a working run on Windows 10 (32-bit build), which is what
makes the claim safe to print. The underlying APIs (`DWMWA_CLOAKED`, the
`ApplicationFrameHost` / `CoreWindow` walk) are all Windows 10-era, so no code
change was involved.

---

## ❓ Needs your decision

### ❓ B1 — one regex compile per keystroke instead of ~50

Answering your question: "per window" meant **per row in the list** — per open
task window being filtered — not per popup window. The count is
`2 x (number of open windows)` per keystroke, not 1000. With 25 apps open that
is about 50 compilations per character typed.

The path: `refilter` runs on every `EN_CHANGE`
([popup.rs:282](src/popup.rs:282)) and calls
[`window_matches`](src/matcher.rs:41) for each item;
`window_matches` calls `regex_matches` **twice** — once for the title, once for
the friendly name — and each of those runs `query()` *and*
`RegexBuilder::build()` ([matcher.rs:26](src/matcher.rs:26)). The query string
is identical every time, so all of that work produces the same regex over and
over.

**Fix.** Compile once at the top of `refilter`, pass the compiled `Regex` down.
`2N` becomes `1`. No behaviour change, no new dependency, contained to two
files.

Whether this is *perceptible* today depends on how many windows you keep open
and how fast migemo's `query()` is on your dictionary — it may well be fine at
your usual window count. Worth a measurement before the work, not after.
**Effort.** S.

### ❓ A1 — `hotkey=INSERT` probably also fires on Numpad 0

You marked this `-`, which I read as undecided rather than dropped — say the
word either way.

[hook.rs:63](src/hook.rs:63) compares `kb.scanCode` and ignores
`kb.flags & LLKHF_EXTENDED (0x01)`. Insert is `E0 52`, Numpad 0 is a bare `52`;
both reach the hook with `scanCode == 0x52`. If so, with `hotkey=Insert`,
pressing Numpad 0 opens the popup *and* has the keystroke swallowed, so `0`
cannot be typed on the numpad. Read off the code and the scan-code tables, not
reproduced. **Repro:** set `hotkey=Insert`, restart, press Numpad 0 with
NumLock on.

**This is not really optional if item 2 happens** — `DELETE`, `HOME`, `END`,
`PRIOR`, `NEXT`, `RCONTROL`, `RMENU` all have numpad twins, so the extended
flag has to be carried through `Config` regardless. Treat it as part of 2a
unless you want it fixed sooner on its own. **Effort.** S.

### ❓ A3 — window titles truncate at 512 characters

Also marked `-`. [window_list.rs:133](src/window_list.rs:133) uses a fixed
buffer. The popup cannot render that much text anyway — but truncation happens
*before* matching, so a search term late in a very long title silently fails to
match. Low impact; listed so the decision is on the record. **Effort.** S.

---

## 📝 Recorded

### 📝 A2 — drop MRU entries for windows that no longer exist

[mru.rs:63](src/mru.rs:63) never removes handles for closed windows; entries
only age out past `MAX_TRACKED = 64`. Windows recycles `HWND` values, so a new
window can inherit a dead one's handle and rank near the top for no reason.

Taking "close if not found" as: prune the entry when the window is gone.
Cheapest form is an `IsWindow` filter inside
[`order_by_mru`](src/mru.rs:76) — a few lines, runs only on popup open, and
needs no second hook. The alternative (hooking `EVENT_OBJECT_DESTROY` next to
`EVENT_SYSTEM_FOREGROUND`) is tidier in principle but adds a system-wide hook
for a cosmetic problem. **Effort.** S.

### 📝 A4 — force the log on panic

Yes, that is exactly it: install a `std::panic::set_hook` that writes to the
log file **unconditionally**, ignoring the `-log` flag and the config setting
from 📝 E2. Today `panic = "abort"` plus
`#![windows_subsystem = "windows"]` means a panic inside a hook proc kills the
app with no window, no log line and no trace — the app simply vanishes after
days in the background. Panics are exactly the case where the user has no
chance to have turned logging on in advance. **Effort.** S.

### 📝 B2 — dictionary construction on the first keystroke

`dictionary()` ([matcher.rs:13](src/matcher.rs:13)) builds the
`CompactDictionary` lazily inside the first search's first keystroke, and
`&DICT_BYTES.to_vec()` copies the whole embedded dictionary to do it. Warming
it on a background thread at startup would move the cost off the first
interaction. Noted, not scheduled — you do not observe it as slow, and it
should be measured before anything is changed. **Effort.** S.

### 📝 C2 — `PageUp` / `PageDown` / `Home` / `End` in the list

[popup.rs:485](src/popup.rs:485) handles only `Up`/`Down`. With
`VISIBLE_ROWS = 10` ([popup.rs:55](src/popup.rs:55)), paging starts to matter
past roughly 30 windows. Four match arms over the existing `move_selection`.
Nice-to-have, not mandatory. **Effort.** S.

### 📝 C3 — `Tab` / `Shift+Tab` in the list, and replacing `Alt+Tab` outright

Two parts, very different sizes.

**The small part:** `Tab` / `Shift+Tab` as aliases for `Down` / `Up` inside the
popup. Two match arms — with the catch that `Tab` currently moves focus out of
the edit control, so it must be intercepted rather than merely handled.
**Effort.** S.

**The large part — substituting `Alt+Tab` itself.** A `WH_KEYBOARD_LL` hook
does receive `Alt+Tab` and can swallow it, so this is technically possible, and
the elevated scheduled task from `setup\install-task.ps1` already solves the
UIPI half. The real obstacle is interaction model, not plumbing:

- wappsw today is *press to open, type to filter, Enter to commit*. `Alt+Tab`
  is *hold Alt, tap Tab to advance, release Alt to commit*. Supporting the
  second means tracking the Alt keyup as the commit event — a new state machine
  in the hook, not a new key binding.
- Both models in one popup is possible (tap to open and type; or hold and tap)
  but doubles the states that need testing.
- Failure is severe: a bug that swallows `Alt+Tab` without opening anything
  leaves the machine with no window switcher at all. This one wants 📝 E1 in
  place first.

Worth treating as its own feature with its own design pass, not a row in item
2's key table. **Effort.** L.

### 📝 C5 — readline/emacs editing keys in the search box

Beyond `Ctrl+Backspace` (which a plain single-line `EDIT` does not implement —
it inserts `0x7F` and beeps): `Ctrl+A` start of line, `Ctrl+E` end of line,
`Ctrl+U` clear line, `Ctrl+W` delete previous word, `Ctrl+K` kill to end.

**Note one conflict:** `Ctrl+A` is Select All in every Windows edit control.
Rebinding it to "start of line" is the bash/emacs convention but breaks a
Windows one — worth being deliberate about, since the popup is otherwise a
normal Windows text field. No conflict with `Ctrl+Q` from item 1.

Implementation shares the control-character swallowing widened in item 1.
**Effort.** S-M depending on how many keys.

### 📝 D3 — match counter in the empty strip at the bottom

There is real estate for it: `LIST_TOP` is 44px and ten 40px rows end at 444px
in a 480px-tall popup ([popup.rs:45](src/popup.rs:45)), leaving a 36px blank
strip. A `12 / 34` counter there tells the user both that the list scrolls
beyond the tenth row and how much the current query matched — the second is
useful feedback given that a zero-match query deliberately leaves the previous
list on screen ([popup.rs:302](src/popup.rs:302)), which is otherwise
indistinguishable from "nothing happened". **Effort.** S.

### 📝 E1 — tests

There is no `#[cfg(test)]` anywhere in `src/`. Four things are pure, Win32-free
functions with obvious inputs and outputs: `config::key_name_to_vk`,
`config::load`'s INI parsing, `mru::order_by_mru`, `popup::default_selected`.

You said many keys will need tests, and that is precisely the argument: a
30-row scan-code table is data that rots silently. A duplicated scan code, a
wrong `extended` flag, or a name colliding with an alias produces no compile
error and no crash — just a key that misbehaves on hardware you do not own.
The modifier-combo parser in 2b has the same property: set-equality logic with
five modifiers has more cases than anyone checks by hand.

Strongly suggest this lands **before** item 2 rather than after. **Effort.** S
to start, and cheap thereafter.

### 📝 E2 — `log=` in config.ini, with the command line overriding

Add a `log=true|false` key; `-log` / `--log` on the command line forces it on
regardless. Useful on its own, and it is what makes a mistyped config
discoverable at all — both failure paths ([config.rs:59](src/config.rs:59)
unknown hotkey, [config.rs:50](src/config.rs:50) unparseable `autoswitch`)
currently log and silently fall back to defaults, which from the outside looks
like the config file being ignored.

**One ordering problem to solve.** `log::init` runs at
[main.rs:18](src/main.rs:18), *before* `config::load()` at
[main.rs:24](src/main.rs:24) — so the setting that enables logging lives in the
file whose parse errors are the main thing worth logging. Either buffer
messages emitted during `load()` and flush them once the log state is known, or
re-init logging after `load()` and have `load()` return its diagnostics rather
than writing them directly. The second is cleaner and makes `load()` testable,
which 📝 E1 wants anyway. **Effort.** S-M.

### 📝 E3 — CI and published builds

No `.github/` at all. Now that the tree builds on both `i686` and `x86_64`
(commit `96e98d4`), a GitHub Actions workflow running `cargo build --release`
for both targets would stop that 64-bit-only break from recurring silently —
note its 64-bit half is *still unverified*, since only the `i686` toolchain is
installed on this machine — and would produce release artifacts so users do not
need a Rust toolchain. Publishing needs its own pass: artifact naming, which
targets are official, and whether releases are tagged by hand. **Effort.** M.

### 📝 E4 — `wappsw.exe --quit`

With `scripts\quit.bat` removed in item 1, quitting requires a working hotkey.
A `--quit` flag that signals the running instance — a named event, or
`PostThreadMessage(WM_QUIT)` to the owner located via the popup's window class
— restores a scriptable, non-force path and covers the case where the hotkey
itself is the thing that is broken. Pairs with item 1; worth doing soon after.
**Effort.** S.

---

## ❌ Decided against

- **C4** — `Ctrl+1`..`Ctrl+9` to jump to the *n*th row.
- **D1** — DPI awareness. The popup stays a fixed 640x480 with fixed fonts and
  is bitmap-stretched by the OS on scaled displays.
- **D2** — dark mode. Colours stay hardcoded.
- **D4** — mouse support. Keyboard-only by design.

## ❄️ Parked

- **Tray icon, settings GUI, window-position config, i18n** — declared out of
  scope by the README.
- **Screen-reader / UI Automation support** — the popup is custom-painted with
  no automation peers, so it is invisible to assistive tech. A real gap, but
  fixing it properly means abandoning custom painting, which is the app's whole
  architecture.
- **Cross-virtual-desktop switching** — cloaked windows are filtered at
  [window_list.rs:43](src/window_list.rs:43). Showing them is easy; *switching*
  to one needs the undocumented `IVirtualDesktopManagerInternal`, whose
  interface GUID changes between Windows builds.
- **Persisting MRU across restarts** — session-only by design
  ([mru.rs:18](src/mru.rs:18)); a stale on-disk MRU is worse than none.

---

## Suggested order

1. ✅ **Item 1** (`Ctrl+Q`) together with 📝 **E4** (`--quit`) — self-contained,
   and E4 covers the fallback that removing `quit.bat` takes away.
2. 📝 **E1** (first tests) and the `-keylog` diagnostic from 2a — both are
   infrastructure for item 2, and both are cheap.
3. ✅ **Item 2a/2c** (key table, multiple bindings), carrying the `extended`
   flag from ❓ A1 — then ✅ **item 3**, which is a small addition once the
   parser exists.
4. ✅ **Item 2b** (modifier combos) — the riskiest part of item 2, best done
   once tests exist.
5. 📝 **E2** (`log=`) and 📝 **A4** (panic logging) — small, and they make
   everything above supportable in the field.
6. 📝 **C3** (Alt+Tab substitution) as its own design pass, last.
