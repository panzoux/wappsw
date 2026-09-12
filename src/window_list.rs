use std::ffi::c_void;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM};
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetAncestor, GetClassNameW, GetLastActivePopup, GetWindow,
    GetWindowLongPtrW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    GA_ROOTOWNER, GWL_EXSTYLE, GW_OWNER, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

const APPLICATION_FRAME_HOST: &str = "ApplicationFrameHost.exe";
const CORE_WINDOW_CLASS: &str = "Windows.UI.Core.CoreWindow";

#[derive(Debug, Clone)]
pub struct TaskWindow {
    pub hwnd: HWND,
    pub title: String,
    pub exe_path: String, // used for the exe-icon fallback added in stage 6
    pub friendly_name: String,
    pub minimized: bool,
}

/// Enumerate the current virtual desktop's Alt-Tab-eligible top-level windows,
/// excluding `exclude`, resolving each one's owning process and friendly app name.
pub fn enumerate_task_windows(exclude: HWND) -> Vec<TaskWindow> {
    let mut hwnds: Vec<HWND> = Vec::new();
    unsafe {
        EnumWindows(Some(collect_hwnd_proc), &mut hwnds as *mut _ as LPARAM);
    }

    let mut out = Vec::new();
    for hwnd in hwnds {
        if hwnd == exclude {
            continue;
        }
        if !passes_alt_tab_filter(hwnd) {
            continue;
        }
        if is_cloaked(hwnd) {
            continue;
        }

        let title = get_window_text(hwnd);
        let Some((_, mut exe_path)) = resolve_process(hwnd) else {
            continue;
        };

        // Only special-case the literal legacy-UWP host process; everything else
        // (including Windows Terminal) resolves directly with no guessing.
        if exe_path_file_name(&exe_path).eq_ignore_ascii_case(APPLICATION_FRAME_HOST) {
            if let Some(real_path) = resolve_core_window_process(hwnd) {
                exe_path = real_path;
            }
        }

        let friendly_name = friendly_app_name(&exe_path);
        let minimized = unsafe { IsIconic(hwnd) } != 0;

        out.push(TaskWindow {
            hwnd,
            title,
            exe_path,
            friendly_name,
            minimized,
        });
    }
    out
}

unsafe extern "system" fn collect_hwnd_proc(hwnd: HWND, lparam: LPARAM) -> i32 {
    let hwnds = unsafe { &mut *(lparam as *mut Vec<HWND>) };
    hwnds.push(hwnd);
    1
}

/// Raymond Chen's Alt-Tab representative-window check
/// (devblogs.microsoft.com/oldnewthing/20071008-00), layered with the
/// WS_EX_APPWINDOW / WS_EX_TOOLWINDOW override rules.
fn passes_alt_tab_filter(hwnd: HWND) -> bool {
    unsafe {
        if IsWindowVisible(hwnd) == 0 {
            return false;
        }

        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let is_tool = ex_style & WS_EX_TOOLWINDOW != 0;
        let is_appwindow = ex_style & WS_EX_APPWINDOW != 0;
        let owner = GetWindow(hwnd, GW_OWNER);

        if !is_appwindow {
            if is_tool {
                return false;
            }
            if !owner.is_null() {
                return false;
            }
        }

        let mut hwnd_walk: HWND = null_mut();
        let mut hwnd_try = GetAncestor(hwnd, GA_ROOTOWNER);
        loop {
            if hwnd_try == hwnd_walk {
                break;
            }
            hwnd_walk = hwnd_try;
            hwnd_try = GetLastActivePopup(hwnd_walk);
            if IsWindowVisible(hwnd_try) != 0 {
                break;
            }
        }
        hwnd_walk == hwnd
    }
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked: u32 = 0;
    let hr = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED as u32,
            &mut cloaked as *mut u32 as *mut c_void,
            std::mem::size_of::<u32>() as u32,
        )
    };
    hr == 0 && cloaked != 0
}

fn get_window_text(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let len = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if len <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..len as usize])
}

fn get_class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if len <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..len as usize])
}

/// Resolves the process id and full exe path that owns `hwnd`.
fn resolve_process(hwnd: HWND) -> Option<(u32, String)> {
    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == 0 {
        return None;
    }
    exe_path_for_pid(pid).map(|path| (pid, path))
}

fn exe_path_for_pid(pid: u32) -> Option<String> {
    let handle: HANDLE = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut buf = [0u16; 1024];
    let mut size = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size) };
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..size as usize]))
}

fn exe_path_file_name(exe_path: &str) -> &str {
    exe_path.rsplit(['\\', '/']).next().unwrap_or(exe_path)
}

/// Legacy UWP apps are hosted inside ApplicationFrameHost.exe; the real app's
/// process owns a `Windows.UI.Core.CoreWindow` child of the frame window.
fn resolve_core_window_process(frame_hwnd: HWND) -> Option<String> {
    let mut found: HWND = null_mut();
    unsafe {
        EnumChildWindows(
            frame_hwnd,
            Some(find_core_window_proc),
            &mut found as *mut _ as LPARAM,
        );
    }
    if found.is_null() {
        return None;
    }
    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(found, &mut pid) };
    if pid == 0 {
        return None;
    }
    exe_path_for_pid(pid)
}

unsafe extern "system" fn find_core_window_proc(hwnd: HWND, lparam: LPARAM) -> i32 {
    if get_class_name(hwnd) == CORE_WINDOW_CLASS {
        let out = unsafe { &mut *(lparam as *mut HWND) };
        *out = hwnd;
        return 0; // stop enumeration, found it
    }
    1
}

/// Reads `FileDescription` from the exe's version resource; falls back to the
/// raw exe filename if the resource or field is missing.
fn friendly_app_name(exe_path: &str) -> String {
    file_description(exe_path).unwrap_or_else(|| exe_path_file_name(exe_path).to_string())
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn file_description(exe_path: &str) -> Option<String> {
    let wide_path = to_wide(exe_path);
    let mut handle: u32 = 0;
    let size = unsafe { GetFileVersionInfoSizeW(wide_path.as_ptr(), &mut handle) };
    if size == 0 {
        return None;
    }

    let mut data = vec![0u8; size as usize];
    let ok = unsafe {
        GetFileVersionInfoW(
            wide_path.as_ptr(),
            0,
            size,
            data.as_mut_ptr() as *mut c_void,
        )
    };
    if ok == 0 {
        return None;
    }

    // \VarFileInfo\Translation gives the language/codepage pairs actually
    // present in this exe's resources.
    let translation_key = to_wide("\\VarFileInfo\\Translation");
    let mut trans_ptr: *mut c_void = null_mut();
    let mut trans_len: u32 = 0;
    let ok = unsafe {
        VerQueryValueW(
            data.as_ptr() as *const c_void,
            translation_key.as_ptr(),
            &mut trans_ptr,
            &mut trans_len,
        )
    };
    if ok == 0 || trans_ptr.is_null() || trans_len < 4 {
        return None;
    }

    let pairs = unsafe {
        std::slice::from_raw_parts(trans_ptr as *const u16, (trans_len / 2) as usize)
    };
    // Each pair is [langId: u16, codePage: u16].
    for chunk in pairs.chunks_exact(2) {
        let lang = chunk[0];
        let codepage = chunk[1];
        let key = to_wide(&format!(
            "\\StringFileInfo\\{:04x}{:04x}\\FileDescription",
            lang, codepage
        ));
        let mut val_ptr: *mut c_void = null_mut();
        let mut val_len: u32 = 0;
        let ok = unsafe {
            VerQueryValueW(
                data.as_ptr() as *const c_void,
                key.as_ptr(),
                &mut val_ptr,
                &mut val_len,
            )
        };
        if ok != 0 && !val_ptr.is_null() && val_len > 0 {
            let chars =
                unsafe { std::slice::from_raw_parts(val_ptr as *const u16, val_len as usize) };
            let end = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
            let desc = String::from_utf16_lossy(&chars[..end]);
            if !desc.trim().is_empty() {
                return Some(desc);
            }
        }
    }
    None
}
