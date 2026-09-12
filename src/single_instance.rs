use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, HANDLE};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

const MUTEX_NAME: &str = "wappsw_single_instance_mutex";

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Acquires the app's named mutex. If another instance already holds it,
/// shows a blocking error dialog and returns None -- caller should exit
/// without installing the hook.
pub fn acquire_or_show_error() -> Option<HANDLE> {
    let name = to_wide(MUTEX_NAME);
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        crate::log::log("single_instance: CreateMutexW returned null");
        return None;
    }
    if unsafe { windows_sys::Win32::Foundation::GetLastError() } == ERROR_ALREADY_EXISTS {
        show_conflict_error();
        return None;
    }
    Some(handle)
}

fn show_conflict_error() {
    let text = to_wide("wappsw is already running.");
    let caption = to_wide("wappsw");
    unsafe {
        MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), MB_OK | MB_ICONERROR);
    }
}
