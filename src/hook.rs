use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
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

/// If Ctrl/Alt/Win is held, this is presumably part of some other shortcut
/// and must pass through untouched -- neither triggering the hotkey nor
/// being swallowed.
fn other_modifier_held() -> bool {
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
        if is_target && !other_modifier_held() {
            // Shift+this-key still triggers (see is_target: on JIS-style
            // layouts the physical key only reaches this hook as VK_CAPITAL
            // when Shift is held, so excluding Shift here would silently
            // drop that case rather than just this key's own toggle). It's
            // swallowed the same as a bare press rather than passed
            // through: letting it through toggles the real Caps Lock
            // lock-state on every other switch, which is worse than losing
            // whatever else Shift+CapsLock might otherwise have done.
            let shift = is_down(VK_SHIFT);
            crate::log::log(&format!(
                "hook: target key event msg={:#x} vk={:#x} scan={:#x} flags={:#x} shift={} -> SWALLOWED",
                wparam,
                kb.vkCode,
                kb.scanCode,
                kb.flags,
                shift,
            ));
            let msg = wparam as u32;
            if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
                popup::on_hotkey();
            }
            // Swallow both down and up: never call CallNextHookEx for this key.
            return 1;
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}
