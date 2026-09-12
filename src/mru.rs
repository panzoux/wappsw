use std::cell::RefCell;
use std::collections::VecDeque;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT,
};

use crate::window_list::TaskWindow;

const MAX_TRACKED: usize = 64;

thread_local! {
    static MRU: RefCell<VecDeque<HWND>> = RefCell::new(VecDeque::new());
}

/// Installs a WinEventHook that tracks real foreground-activation order in
/// memory (session-only, not persisted). This is what makes ordering
/// reflect actual usage instead of window Z-order.
pub fn install() -> HWINEVENTHOOK {
    unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            std::ptr::null_mut(),
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    }
}

pub fn uninstall(hook: HWINEVENTHOOK) {
    unsafe {
        UnhookWinEvent(hook);
    }
}

/// The hook only observes *future* foreground changes; call this once at
/// startup so whichever window is already focused ranks first immediately,
/// rather than waiting for the next real focus change.
pub fn seed_current_foreground() {
    let fg = unsafe { GetForegroundWindow() };
    if !fg.is_null() {
        MRU.with(|m| m.borrow_mut().push_front(fg));
    }
}

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    if hwnd.is_null() {
        return;
    }
    MRU.with(|m| {
        let mut m = m.borrow_mut();
        m.retain(|&h| h != hwnd);
        m.push_front(hwnd);
        if m.len() > MAX_TRACKED {
            m.pop_back();
        }
    });
}

/// Reorders `items` by tracked MRU (most-recently-foregrounded first);
/// anything never observed keeps its original relative order, appended
/// after all tracked windows.
pub fn order_by_mru(items: Vec<TaskWindow>) -> Vec<TaskWindow> {
    MRU.with(|m| {
        let m = m.borrow();
        let mut ranked: Vec<(usize, TaskWindow)> = items
            .into_iter()
            .map(|w| {
                let rank = m.iter().position(|&h| h == w.hwnd).unwrap_or(usize::MAX);
                (rank, w)
            })
            .collect();
        ranked.sort_by_key(|(rank, _)| *rank);
        ranked.into_iter().map(|(_, w)| w).collect()
    })
}
