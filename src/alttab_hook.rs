use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, VK_CONTROL, VK_ESCAPE, VK_F12, VK_LMENU, VK_MENU, VK_Q, VK_RMENU,
    VK_SHIFT, VK_TAB, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, SetWindowsHookExW, HHOOK, KBDLLHOOKSTRUCT, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP,
    WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use crate::popup;

/// Set once by Ctrl+Shift+F12 and never cleared: every later event is passed
/// straight to CallNextHookEx, so native Alt+Tab works again for the rest of
/// the process's life. No config or UI to turn it back on -- restarting the
/// app is the only way, same as a misconfigured single-key hotkey today has
/// no in-app recovery either.
static DISABLED: AtomicBool = AtomicBool::new(false);

/// Set by Alt+Q while a session is open: hands the popup off to its normal
/// typing/Up/Down/Enter/Esc behaviour (see popup.rs) instead of committing
/// when Alt is released. Reset to false every time a fresh session opens.
/// Without this, `hotkey=AltTab` is a pure Alt+Tab clone with no way to
/// reach wappsw's actual strength -- migemo filtering.
static DETACHED: AtomicBool = AtomicBool::new(false);

/// Installs the WH_KEYBOARD_LL hook that replaces the native Alt+Tab
/// switcher with wappsw's own popup, for `hotkey=AltTab`. Mutually
/// exclusive with hook.rs's single-key hook -- main.rs installs exactly one
/// of the two, so this never runs alongside e.g. CapsLock. See
/// docs/alt-tab-hotkey.md for the full design and the state machine below.
///
/// Unlike hook.rs, this doesn't claim a fixed key: it watches Tab and Alt
/// together, so a bare Alt press/release (e.g. to focus a menu bar) is left
/// completely untouched, and only a genuine Alt+Tab (or Shift+Alt+Tab, or
/// the interception itself) is ever swallowed.
pub fn install() -> HHOOK {
    unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), GetModuleHandleW(std::ptr::null()), 0) }
}

fn is_down(vk: u16) -> bool {
    unsafe { (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 }
}

fn is_keydown(msg: u32) -> bool {
    msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN
}

fn is_keyup(msg: u32) -> bool {
    msg == WM_KEYUP || msg == WM_SYSKEYUP
}

fn synth_key(vk: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Replaces the real, committing Alt-up with a synthetic one, preceded by a
/// decoy Ctrl tap -- sent as a single `SendInput` batch, which guarantees
/// delivery order (no other input can be interspersed within one call).
/// Solves two problems together:
///
/// - Simply forwarding the real Alt-up via `CallNextHookEx` correctly
///   clears Windows' global "Alt is held" state, but the window just
///   switched to then sees what looks like a bare Alt tap -- Tab's own
///   events never reached it, since this hook swallows them -- and focuses
///   its menu bar. Confirmed in manual testing (e.g. Notepad).
/// - The decoy Ctrl tap, delivered immediately before the synthetic Alt-up,
///   satisfies the "was another key pressed during this hold" check apps
///   use to suppress that behaviour, without typing or doing anything else
///   visible.
///
/// `vk` should be the real event's own `vkCode` (`VK_MENU`/`VK_LMENU`/
/// `VK_RMENU`, whichever Windows reported), so the synthetic replacement
/// matches it exactly.
///
/// This synthetic Alt-up loops back through this same hook (SendInput
/// events pass through low-level hooks same as real input), but by the
/// time it arrives, `popup::alttab_commit()` has already run and closed
/// the popup, so `popup::is_open()` is false and it just falls through to
/// a plain pass-through rather than re-triggering a commit.
fn release_alt_without_activating_menu(vk: u32) {
    let inputs = [synth_key(VK_CONTROL, false), synth_key(VK_CONTROL, true), synth_key(vk as u16, true)];
    let sent =
        unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        crate::log::log(&format!(
            "alttab_hook: SendInput sent {}/{} synthetic events",
            sent,
            inputs.len()
        ));
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let kb = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let msg = wparam as u32;
        let vk = kb.vkCode;

        // Emergency disable/re-enable toggle: Ctrl+Shift+F12. Checked
        // before the DISABLED short-circuit below (and before every other
        // arm) so it keeps working both mid-session and while disabled --
        // otherwise there would be no way back on short of restarting.
        if vk == VK_F12 as u32 && is_keydown(msg) && is_down(VK_CONTROL) && is_down(VK_SHIFT) {
            let now_disabled = !DISABLED.load(Ordering::SeqCst);
            if now_disabled && popup::is_open() {
                popup::alttab_cancel();
            }
            DISABLED.store(now_disabled, Ordering::SeqCst);
            crate::log::log(&format!(
                "alttab_hook: Ctrl+Shift+F12 -- {}",
                if now_disabled { "disabled" } else { "re-enabled" }
            ));
            return 1;
        }

        if DISABLED.load(Ordering::SeqCst) {
            return unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) };
        }

        // Alt+Q: hand the open popup off to normal filter mode. Checked
        // before the Tab arm so it works on the same keystroke pattern
        // (Alt held, another key tapped) without the Tab arm's swallow
        // logic getting in the way.
        if vk == VK_Q as u32 && is_keydown(msg) && is_down(VK_MENU) && popup::is_open() {
            if !DETACHED.swap(true, Ordering::SeqCst) {
                crate::log::log("alttab_hook: Alt+Q -- detaching into filter mode");
            }
            return 1;
        }

        if vk == VK_TAB as u32 {
            if is_down(VK_MENU) && !DETACHED.load(Ordering::SeqCst) {
                if is_keydown(msg) {
                    let reverse = is_down(VK_SHIFT);
                    if !popup::is_open() {
                        crate::log::log("alttab_hook: Alt+Tab -- opening");
                        DETACHED.store(false, Ordering::SeqCst);
                        popup::alttab_open();
                    } else {
                        crate::log::log(&format!(
                            "alttab_hook: Tab while held -- {}",
                            if reverse { "previous" } else { "next" }
                        ));
                        popup::alttab_advance(if reverse { -1 } else { 1 });
                    }
                }
                // Swallow both down and up -- Tab must never reach anything
                // else while Alt is held, or focus would move in whatever
                // window is behind the popup.
                return 1;
            }
            // Alt not held, or the session was detached into filter mode
            // (where Tab is just an ordinary key, same as single-key mode):
            // not ours.
            return unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) };
        }

        if vk == VK_MENU as u32 || vk == VK_LMENU as u32 || vk == VK_RMENU as u32 {
            if is_keyup(msg) && popup::is_open() && !DETACHED.load(Ordering::SeqCst) {
                // Trust this keyup directly rather than additionally
                // checking GetAsyncKeyState(VK_MENU): queried from inside
                // Alt's own keyup event, it was observed to still report
                // "down" every single time in manual testing -- the same
                // kind of self-referential staleness hook.rs's docs already
                // describe for Shift during its own Caps Lock event, just
                // for a different key.
                crate::log::log("alttab_hook: Alt released -- committing");
                popup::alttab_commit();
                // Swallow the real keyup and replace it with a synthetic
                // one instead of passing this one through directly: see
                // release_alt_without_activating_menu's doc comment for why
                // a straight pass-through flashes the target window's menu
                // bar, and docs/alt-tab-hotkey.md for why swallowing it
                // outright (the very first cut) corrupted global Alt state
                // instead.
                release_alt_without_activating_menu(vk);
                return 1;
            }
            // Alt down, or up outside a committing session: never touched,
            // so a bare Alt tap keeps working normally.
            return unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) };
        }

        if vk == VK_ESCAPE as u32 && popup::is_open() {
            if is_keydown(msg) {
                crate::log::log("alttab_hook: Esc -- cancelling");
                popup::alttab_cancel();
            }
            // Swallow down and up; Alt is very likely still physically held
            // at this point, and its eventual release passes through
            // normally since the popup is already closed.
            return 1;
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}
