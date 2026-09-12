use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_SMALLICON, SHGFI_USEFILEATTRIBUTES};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, SendMessageW, GCLP_HICON, GCLP_HICONSM, GET_CLASS_LONG_INDEX, HICON, ICON_SMALL,
    WM_GETICON,
};
#[cfg(target_pointer_width = "64")]
use windows_sys::Win32::UI::WindowsAndMessaging::GetClassLongPtrW;
#[cfg(target_pointer_width = "32")]
use windows_sys::Win32::UI::WindowsAndMessaging::GetClassLongW;

/// `GetClassLongPtrW` only exists on 64-bit Windows; on 32-bit it is spelled
/// `GetClassLongW` and returns a `u32`. Present one pointer-sized signature so
/// the call sites stay the same for both targets.
#[cfg(target_pointer_width = "64")]
unsafe fn get_class_long_ptr(hwnd: HWND, index: GET_CLASS_LONG_INDEX) -> usize {
    unsafe { GetClassLongPtrW(hwnd, index) }
}

#[cfg(target_pointer_width = "32")]
unsafe fn get_class_long_ptr(hwnd: HWND, index: GET_CLASS_LONG_INDEX) -> usize {
    unsafe { GetClassLongW(hwnd, index) as usize }
}

/// A resolved icon handle plus whether we own it (and must `DestroyIcon` it
/// ourselves) or it's borrowed from the window/class (must not be destroyed).
pub struct ResolvedIcon {
    pub handle: HICON,
    owned: bool,
}

impl Drop for ResolvedIcon {
    fn drop(&mut self) {
        if self.owned && !self.handle.is_null() {
            unsafe {
                DestroyIcon(self.handle);
            }
        }
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Resolves a small icon for `hwnd`: the window's own small icon first
/// (`WM_GETICON`, then the window class's icon), falling back to the
/// resolved exe's default icon via the shell.
pub fn resolve(hwnd: HWND, exe_path: &str) -> Option<ResolvedIcon> {
    unsafe {
        let from_window = SendMessageW(hwnd, WM_GETICON, ICON_SMALL as _, 0);
        if from_window != 0 {
            return Some(ResolvedIcon { handle: from_window as HICON, owned: false });
        }

        let from_class = get_class_long_ptr(hwnd, GCLP_HICONSM);
        if from_class != 0 {
            return Some(ResolvedIcon { handle: from_class as HICON, owned: false });
        }
        let from_class_large = get_class_long_ptr(hwnd, GCLP_HICON);
        if from_class_large != 0 {
            return Some(ResolvedIcon { handle: from_class_large as HICON, owned: false });
        }
    }

    let wide_path = to_wide(exe_path);
    let mut info: SHFILEINFOW = unsafe { std::mem::zeroed() };
    let result = unsafe {
        SHGetFileInfoW(
            wide_path.as_ptr(),
            FILE_ATTRIBUTE_NORMAL,
            &mut info,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES,
        )
    };
    if result != 0 && !info.hIcon.is_null() {
        return Some(ResolvedIcon { handle: info.hIcon, owned: true });
    }
    None
}
