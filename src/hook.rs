use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_SYSKEYDOWN,
};

use crate::popup;

static TARGET_VK: AtomicU32 = AtomicU32::new(0);
static TARGET_SCANCODE: AtomicU32 = AtomicU32::new(0);

/// Installs the WH_KEYBOARD_LL hook that watches for the configured key and
/// swallows it entirely (never forwarded via CallNextHookEx), so e.g. Caps
/// Lock's native lock-state toggle never fires while this app is running.
///
/// Matches on the raw hardware *scan code* (`scan_code`, e.g. 0x3A for Caps
/// Lock) rather than the layout-translated virtual key (`vk_code`, kept
/// around only for logging/fallback). On JIS-style layouts, the Caps Lock
/// key only translates to VK_CAPITAL when Shift is held -- pressed bare,
/// Windows translates the same physical key to a different VK (the
/// Eisu/alphanumeric toggle), so a vkCode match alone can never see a bare
/// press on those keyboards. The scan code is positional and stays constant
/// regardless of that shift-dependent translation, so it's the only
/// reliable way to identify "this physical key" across layouts.
///
/// The scan code must be passed in rather than derived at runtime via
/// MapVirtualKeyW(vk, MAPVK_VK_TO_VSC): that reverse lookup can fail to
/// resolve VK_CAPITAL at all on JIS-style layouts, since VK_CAPITAL isn't
/// that scan code's primary (unshifted) output there.
pub fn install(vk_code: u32, scan_code: u32) -> HHOOK {
    TARGET_VK.store(vk_code, Ordering::SeqCst);
    TARGET_SCANCODE.store(scan_code, Ordering::SeqCst);
    unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), GetModuleHandleW(std::ptr::null()), 0) }
}

pub fn uninstall(hook: HHOOK) {
    unsafe {
        UnhookWindowsHookEx(hook);
    }
}

fn is_down(vk: u16) -> bool {
    unsafe { (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 }
}

/// v1 only fires on a *bare* press of the configured key -- if Ctrl/Alt/Win
/// is held, this is presumably part of some other shortcut and must pass
/// through untouched rather than being swallowed.
///
/// Shift is deliberately excluded: on JIS-style keyboard layouts (where the
/// Caps Lock key doubles as the Eisu/alphanumeric key and only becomes Caps
/// Lock via a "Shift Generates Caps" translation at the driver level),
/// Windows reports VK_SHIFT as held via GetAsyncKeyState for *every* Caps
/// Lock press regardless of whether Shift is physically down -- confirmed by
/// diagnostic logging showing shift=true on 100% of real hardware presses.
/// Treating that as a real modifier made every bare press look like an
/// accelerator combo and silently blocked the hotkey entirely.
fn any_modifier_held() -> bool {
    is_down(VK_CONTROL) || is_down(VK_MENU) || is_down(VK_LWIN) || is_down(VK_RWIN)
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let kb = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let target_scancode = TARGET_SCANCODE.load(Ordering::SeqCst);
        let is_target = if target_scancode != 0 {
            kb.scanCode == target_scancode
        } else {
            kb.vkCode == TARGET_VK.load(Ordering::SeqCst)
        };
        if is_target {
            let blocked = any_modifier_held();
            crate::log::log(&format!(
                "hook: target key event msg={:#x} vk={:#x} scan={:#x} flags={:#x} ctrl={} alt={} lwin={} rwin={} -> {}",
                wparam,
                kb.vkCode,
                kb.scanCode,
                kb.flags,
                is_down(VK_CONTROL),
                is_down(VK_MENU),
                is_down(VK_LWIN),
                is_down(VK_RWIN),
                if blocked { "PASSED THROUGH (modifier held)" } else { "SWALLOWED (bare press)" }
            ));
            if !blocked {
                let msg = wparam as u32;
                if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
                    popup::on_hotkey();
                }
                // Swallow both down and up: never call CallNextHookEx for this key.
                return 1;
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}
