use std::thread::sleep;
use std::time::Duration;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, IsIconic, SetForegroundWindow, ShowWindow,
    SW_RESTORE,
};

const MAX_ATTEMPTS: u32 = 5;

/// Restores `hwnd` if minimized and brings it to the foreground.
pub fn switch_to(hwnd: HWND) {
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
    }
    force_foreground(hwnd);
}

/// Makes `hwnd` the foreground window, retrying with AttachThreadInput.
///
/// A single plain `SetForegroundWindow` call is unreliable in practice --
/// verified via debug logging, it can get rejected even when the exemption
/// rules documented by Microsoft appear to apply (e.g. the calling process
/// currently owns the foreground window). Attaching this thread's input
/// state to the current foreground thread and retrying a few times clears
/// it up immediately in testing.
pub fn force_foreground(hwnd: HWND) -> bool {
    unsafe {
        for attempt in 0..MAX_ATTEMPTS {
            let fg = GetForegroundWindow();
            if fg == hwnd {
                return true;
            }
            let fg_thread = GetWindowThreadProcessId(fg, std::ptr::null_mut());
            let current_thread = GetCurrentThreadId();
            let attached = fg_thread != 0
                && fg_thread != current_thread
                && AttachThreadInput(current_thread, fg_thread, 1) != 0;

            let ok = SetForegroundWindow(hwnd);

            if attached {
                AttachThreadInput(current_thread, fg_thread, 0);
            }

            if ok != 0 {
                return true;
            }
            if attempt + 1 < MAX_ATTEMPTS {
                sleep(Duration::from_millis(10));
            }
        }
    }
    false
}
