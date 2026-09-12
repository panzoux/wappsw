use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, EndPaint, FillRect,
    GetMonitorInfoW, GetStockObject, InvalidateRect, MonitorFromWindow, SelectObject,
    SetBkMode, SetTextColor, TextOutW, HBRUSH, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    PAINTSTRUCT, TRANSPARENT, WHITE_BRUSH,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::Ime::ImmAssociateContext;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_DOWN, VK_ESCAPE, VK_RETURN, VK_UP};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, CreateWindowExW, DefWindowProcW, DrawIconEx, GetClientRect,
    GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible, KillTimer,
    PostQuitMessage, RegisterClassW, SetTimer, SetWindowLongPtrW, SetWindowPos, SetWindowTextW,
    ShowWindow, CS_HREDRAW, CS_VREDRAW, DI_NORMAL, EN_CHANGE, GWLP_WNDPROC, HWND_TOPMOST,
    SWP_NOACTIVATE, SW_HIDE, SW_SHOW, WA_INACTIVE, WINDOW_LONG_PTR_INDEX, WM_ACTIVATE, WM_CHAR,
    WM_COMMAND, WM_DESTROY, WM_KEYDOWN, WM_PAINT, WM_TIMER, WNDCLASSW, WNDPROC, WS_BORDER,
    WS_CHILD, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};

use crate::icons::{self, ResolvedIcon};
use crate::matcher;
use crate::mru;
use crate::switch;
use crate::window_list::{self, TaskWindow};

/// On 32-bit targets `windows-sys` aliases `SetWindowLongPtrW` to
/// `SetWindowLongW`, which takes and returns `i32` rather than `isize`.
/// Wrap both behind one pointer-sized signature.
#[cfg(target_pointer_width = "64")]
unsafe fn set_window_long_ptr(hwnd: HWND, index: WINDOW_LONG_PTR_INDEX, value: isize) -> isize {
    unsafe { SetWindowLongPtrW(hwnd, index, value) }
}

#[cfg(target_pointer_width = "32")]
unsafe fn set_window_long_ptr(hwnd: HWND, index: WINDOW_LONG_PTR_INDEX, value: isize) -> isize {
    unsafe { SetWindowLongPtrW(hwnd, index, value as i32) as isize }
}

const CLASS_NAME: &str = "wappsw_popup";
const EDIT_CLASS_NAME: &str = "EDIT";
const POPUP_WIDTH: i32 = 640;
const POPUP_HEIGHT: i32 = 480;
const EDIT_HEIGHT: i32 = 28;
const EDIT_MARGIN: i32 = 8;
const LIST_TOP: i32 = EDIT_MARGIN * 2 + EDIT_HEIGHT;
const ROW_HEIGHT: i32 = 40;
const ROW_PADDING_X: i32 = 8;
const ICON_SIZE: i32 = 32;
const TEXT_START_X: i32 = ROW_PADDING_X + ICON_SIZE + 8;
const ES_AUTOHSCROLL: u32 = 0x0080;
const VISIBLE_ROWS: usize = ((POPUP_HEIGHT - LIST_TOP) / ROW_HEIGHT) as usize;

static POPUP_HWND: AtomicIsize = AtomicIsize::new(0);
static EDIT_HWND: AtomicIsize = AtomicIsize::new(0);
static ORIGINAL_EDIT_PROC: AtomicIsize = AtomicIsize::new(0);

/// Sentinel for "disabled" -- realistically never a configured delay.
const AUTO_SWITCH_DISABLED: u32 = u32::MAX;
static AUTO_SWITCH_MS: AtomicU32 = AtomicU32::new(AUTO_SWITCH_DISABLED);
const AUTO_SWITCH_TIMER_ID: usize = 1;

/// Sets the auto-switch delay from config. Must be called before `open()`
/// can be reached (i.e. before the hotkey is live). `None` leaves the
/// feature disabled: a unique match then never switches on its own.
pub fn set_auto_switch_delay(ms: Option<u32>) {
    AUTO_SWITCH_MS.store(ms.unwrap_or(AUTO_SWITCH_DISABLED), Ordering::SeqCst);
}

/// Called whenever the displayed list changes size: (re)starts the
/// auto-switch timer while it sits at exactly one match, cancels it
/// otherwise. A fresh keystroke while already at one match restarts the
/// timer, so the list must go genuinely untouched for the full delay.
fn update_auto_switch_timer(h: HWND, displayed_len: usize) {
    let ms = AUTO_SWITCH_MS.load(Ordering::SeqCst);
    if ms != AUTO_SWITCH_DISABLED && displayed_len == 1 {
        unsafe {
            SetTimer(h, AUTO_SWITCH_TIMER_ID, ms, None);
        }
    } else {
        unsafe {
            KillTimer(h, AUTO_SWITCH_TIMER_ID);
        }
    }
}

struct PopupState {
    items: Vec<TaskWindow>,
    /// Parallel to `items`; resolved once per open() and kept alive for the
    /// popup's lifetime so paint() doesn't re-resolve icons on every redraw.
    icons: Vec<Option<ResolvedIcon>>,
    /// Indices into `items` currently displayed. On a query that matches
    /// nothing, this is deliberately left unchanged (the last list that had
    /// matches stays on screen) rather than cleared or reset to the full list.
    displayed: Vec<usize>,
    selected: usize,
    /// Index into `displayed` of the first row currently drawn. Adjusted
    /// (not reset) whenever `selected` moves outside the visible window, so
    /// the highlighted row is never scrolled off-screen.
    scroll_offset: usize,
}

/// Keeps `selected` within the visible window, scrolling the minimum
/// amount needed rather than re-centering.
fn clamp_scroll(s: &mut PopupState) {
    if VISIBLE_ROWS == 0 {
        return;
    }
    if s.selected < s.scroll_offset {
        s.scroll_offset = s.selected;
    } else if s.selected >= s.scroll_offset + VISIBLE_ROWS {
        s.scroll_offset = s.selected + 1 - VISIBLE_ROWS;
    }
}

thread_local! {
    static STATE: RefCell<PopupState> = RefCell::new(PopupState { items: Vec::new(), icons: Vec::new(), displayed: Vec::new(), selected: 0, scroll_offset: 0 });
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

fn hwnd() -> HWND {
    POPUP_HWND.load(Ordering::SeqCst) as HWND
}

fn edit_hwnd() -> HWND {
    EDIT_HWND.load(Ordering::SeqCst) as HWND
}

/// Registers the popup window class, creates the (hidden) popup window and
/// its query edit control. Must be called once from the thread that will
/// run the message loop.
pub fn init() {
    let class_name = to_wide(CLASS_NAME);
    let hinstance = unsafe { GetModuleHandleW(std::ptr::null()) };

    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: unsafe { GetStockObject(WHITE_BRUSH) } as HBRUSH,
        lpszMenuName: std::ptr::null(),
        lpszClassName: class_name.as_ptr(),
    };
    unsafe {
        RegisterClassW(&wc);
    }

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW, // topmost, and excluded from taskbar/Alt-Tab
            class_name.as_ptr(),
            class_name.as_ptr(),
            WS_POPUP | WS_BORDER,
            0,
            0,
            POPUP_WIDTH,
            POPUP_HEIGHT,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        )
    };
    POPUP_HWND.store(hwnd as isize, Ordering::SeqCst);

    let edit_class = to_wide(EDIT_CLASS_NAME);
    let edit = unsafe {
        CreateWindowExW(
            0,
            edit_class.as_ptr(),
            std::ptr::null(),
            WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL,
            EDIT_MARGIN,
            EDIT_MARGIN,
            POPUP_WIDTH - EDIT_MARGIN * 2,
            EDIT_HEIGHT,
            hwnd,
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        )
    };
    EDIT_HWND.store(edit as isize, Ordering::SeqCst);

    // Raw ASCII keystrokes must reach the migemo matcher undistorted -- the
    // system IME must never compose them.
    unsafe {
        ImmAssociateContext(edit, std::ptr::null_mut());
    }

    let old_proc =
        unsafe { set_window_long_ptr(edit, GWLP_WNDPROC, edit_subclass_proc as *const () as isize) };
    ORIGINAL_EDIT_PROC.store(old_proc, Ordering::SeqCst);
}

/// Called from the low-level keyboard hook when the configured hotkey fires.
/// If the popup is hidden, snapshot the window list and show it; if it's
/// already open, treat this exactly like Enter (confirm current selection).
pub fn on_hotkey() {
    let h = hwnd();
    if h.is_null() {
        crate::log::log("popup::on_hotkey: popup HWND is null, ignoring");
        return;
    }
    if unsafe { IsWindowVisible(h) } != 0 {
        crate::log::log("popup::on_hotkey: already visible -> confirm_selection()");
        confirm_selection();
    } else {
        crate::log::log("popup::on_hotkey: hidden -> open()");
        open(h);
    }
}

// The current window sits at index 0 (most-recently-foregrounded); default
// the highlight to index 1 (the previous window) so an immediate Enter --
// or a second hotkey tap -- swaps straight back to it. Repeated taps then
// toggle between the last two windows. Applies both on initial open and
// whenever the query is cleared back to empty.
fn default_selected(items_len: usize) -> usize {
    if items_len > 1 {
        1
    } else {
        0
    }
}

fn open(h: HWND) {
    let items = mru::order_by_mru(window_list::enumerate_task_windows(h));
    let displayed: Vec<usize> = (0..items.len()).collect();
    let displayed_len = displayed.len();
    let selected = default_selected(items.len());
    let icon_handles: Vec<Option<ResolvedIcon>> = items
        .iter()
        .map(|w| icons::resolve(w.hwnd, &w.exe_path))
        .collect();
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.items = items;
        s.icons = icon_handles; // old Vec drops here, freeing any owned icons
        s.displayed = displayed;
        s.selected = selected;
        s.scroll_offset = 0;
        clamp_scroll(&mut s);
    });
    update_auto_switch_timer(h, displayed_len);

    let empty = to_wide("");
    unsafe {
        SetWindowTextW(edit_hwnd(), empty.as_ptr());
    }

    position_on_foreground_monitor(h);
    unsafe {
        ShowWindow(h, SW_SHOW);
    }
    switch::force_foreground(h);
    unsafe {
        SetFocus(edit_hwnd());
    }
}

fn hide() {
    unsafe {
        KillTimer(hwnd(), AUTO_SWITCH_TIMER_ID);
        ShowWindow(hwnd(), SW_HIDE);
    }
}

fn refilter() {
    let h = hwnd();
    let query_text = read_edit_text();
    let displayed_len = STATE.with(|s| {
        let mut s = s.borrow_mut();
        if query_text.trim().is_empty() {
            s.displayed = (0..s.items.len()).collect();
            s.selected = default_selected(s.items.len());
            s.scroll_offset = 0;
            clamp_scroll(&mut s);
            return s.displayed.len();
        }
        let new_displayed: Vec<usize> = s
            .items
            .iter()
            .enumerate()
            .filter(|(_, w)| matcher::window_matches(&query_text, w))
            .map(|(i, _)| i)
            .collect();
        // Zero matches: deliberately keep whatever was displayed before.
        if !new_displayed.is_empty() {
            s.displayed = new_displayed;
            s.selected = 0;
            s.scroll_offset = 0;
            clamp_scroll(&mut s);
        }
        s.displayed.len()
    });
    update_auto_switch_timer(h, displayed_len);
    unsafe {
        InvalidateRect(h, std::ptr::null(), 0);
    }
}

fn read_edit_text() -> String {
    let e = edit_hwnd();
    let len = unsafe { GetWindowTextLengthW(e) };
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; len as usize + 1];
    let copied = unsafe { GetWindowTextW(e, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..copied.max(0) as usize])
}

fn move_selection(delta: i32) {
    let h = hwnd();
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.displayed.is_empty() {
            return;
        }
        let len = s.displayed.len() as i32;
        let cur = s.selected as i32;
        s.selected = ((cur + delta).rem_euclid(len)) as usize;
        clamp_scroll(&mut s);
    });
    unsafe {
        InvalidateRect(h, std::ptr::null(), 0);
    }
}

fn confirm_selection() {
    let target = STATE.with(|s| {
        let s = s.borrow();
        s.displayed.get(s.selected).and_then(|&i| s.items.get(i)).map(|w| w.hwnd)
    });
    // Switch *before* hiding: the popup must still hold the foreground when
    // SetForegroundWindow(target) is called, or Windows' foreground-lock
    // restriction can reject the call once something else has reclaimed it.
    if let Some(target) = target {
        switch::switch_to(target);
    }
    hide();
}

fn position_on_foreground_monitor(h: HWND) {
    let fg = unsafe { GetForegroundWindow() };
    let monitor = unsafe { MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        rcMonitor: RECT { left: 0, top: 0, right: 0, bottom: 0 },
        rcWork: RECT { left: 0, top: 0, right: 0, bottom: 0 },
        dwFlags: 0,
    };
    let rect = if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
        info.rcMonitor
    } else {
        RECT { left: 0, top: 0, right: 1920, bottom: 1080 }
    };

    let mon_w = rect.right - rect.left;
    let mon_h = rect.bottom - rect.top;
    let x = rect.left + (mon_w - POPUP_WIDTH) / 2;
    let y = rect.top + (mon_h - POPUP_HEIGHT) / 2;

    unsafe {
        SetWindowPos(h, HWND_TOPMOST, x, y, POPUP_WIDTH, POPUP_HEIGHT, SWP_NOACTIVATE);
    }
}

fn paint(h: HWND) {
    let mut ps: PAINTSTRUCT = unsafe { std::mem::zeroed() };
    let hdc = unsafe { BeginPaint(h, &mut ps) };

    let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetClientRect(h, &mut client) };
    unsafe { FillRect(hdc, &client, GetStockObject(WHITE_BRUSH) as HBRUSH) };

    let font = unsafe {
        CreateFontW(
            18, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 0, 0,
            to_wide("Yu Gothic UI").as_ptr(),
        )
    };
    let old_font = unsafe { SelectObject(hdc, font as _) };
    unsafe { SetBkMode(hdc, TRANSPARENT as i32) };

    let highlight_brush = unsafe { CreateSolidBrush(rgb(51, 122, 216)) };

    STATE.with(|s| {
        let s = s.borrow();
        if s.displayed.is_empty() {
            let text = to_wide("No windows found");
            unsafe {
                SetTextColor(hdc, rgb(120, 120, 120));
                TextOutW(hdc, ROW_PADDING_X, LIST_TOP + 4, text.as_ptr(), (text.len() - 1) as i32);
            }
            return;
        }
        for (abs_row, &item_idx) in s.displayed.iter().enumerate().skip(s.scroll_offset).take(VISIBLE_ROWS) {
            let item = &s.items[item_idx];
            let screen_row = abs_row - s.scroll_offset;
            let row_top = LIST_TOP + screen_row as i32 * ROW_HEIGHT;
            if row_top > client.bottom {
                break;
            }
            let row_rect = RECT {
                left: 0,
                top: row_top,
                right: client.right,
                bottom: row_top + ROW_HEIGHT,
            };
            if abs_row == s.selected {
                unsafe { FillRect(hdc, &row_rect, highlight_brush) };
                unsafe { SetTextColor(hdc, rgb(255, 255, 255)) };
            } else {
                unsafe { SetTextColor(hdc, rgb(20, 20, 20)) };
            }

            if let Some(Some(icon)) = s.icons.get(item_idx) {
                let icon_y = row_top + (ROW_HEIGHT - ICON_SIZE) / 2;
                unsafe {
                    DrawIconEx(
                        hdc,
                        ROW_PADDING_X,
                        icon_y,
                        icon.handle,
                        ICON_SIZE,
                        ICON_SIZE,
                        0,
                        std::ptr::null_mut(),
                        DI_NORMAL,
                    );
                }
            }

            let base = if item.title.trim().is_empty() {
                item.friendly_name.clone()
            } else {
                format!("{}  \u{2014}  {}", item.title, item.friendly_name)
            };
            let label = if item.minimized {
                format!("[min] {}", base)
            } else {
                base
            };
            let wide = to_wide(&label);
            let text_y = row_top + (ROW_HEIGHT - 22) / 2;
            unsafe {
                TextOutW(
                    hdc,
                    TEXT_START_X,
                    text_y,
                    wide.as_ptr(),
                    (wide.len() - 1) as i32,
                );
            }
        }
    });

    unsafe {
        SelectObject(hdc, old_font);
        DeleteObject(font as _);
        DeleteObject(highlight_brush as _);
        EndPaint(h, &ps);
    }
}

/// Special keys (Up/Down/Enter/Escape) must control the list even while the
/// edit control has keyboard focus, so they're intercepted here before
/// falling through to the edit control's own WndProc for normal typing.
unsafe extern "system" fn edit_subclass_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_KEYDOWN {
        match wparam as u16 {
            VK_ESCAPE => {
                hide();
                return 0;
            }
            VK_RETURN => {
                confirm_selection();
                return 0;
            }
            VK_UP => {
                move_selection(-1);
                return 0;
            }
            VK_DOWN => {
                move_selection(1);
                return 0;
            }
            _ => {}
        }
    }
    // TranslateMessage() still turns Enter/Escape's WM_KEYDOWN into a
    // trailing WM_CHAR('\r'/ESC) regardless of what we did with the
    // keydown above. A plain single-line edit control beeps on receiving
    // those as characters to insert, so swallow them here too.
    if msg == WM_CHAR && matches!(wparam as u8, 0x0D | 0x1B) {
        return 0;
    }
    let old = ORIGINAL_EDIT_PROC.load(Ordering::SeqCst);
    let old_proc: WNDPROC = if old == 0 {
        None
    } else {
        Some(unsafe {
            std::mem::transmute::<isize, unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>(old)
        })
    };
    unsafe { CallWindowProcW(old_proc, hwnd, msg, wparam, lparam) }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let notify_code = (wparam as u32) >> 16;
            let source = lparam as HWND;
            if notify_code == EN_CHANGE && source == edit_hwnd() {
                refilter();
            }
            0
        }
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_ACTIVATE => {
            if (wparam as u32 & 0xFFFF) == WA_INACTIVE {
                hide();
            }
            0
        }
        WM_TIMER => {
            if wparam == AUTO_SWITCH_TIMER_ID {
                unsafe {
                    KillTimer(hwnd, AUTO_SWITCH_TIMER_ID);
                }
                // Defensive re-check: the list could only have changed via
                // refilter(), which already resets this timer on any change,
                // so this should always still be true when the timer fires.
                let still_unique = STATE.with(|s| s.borrow().displayed.len() == 1);
                if still_unique {
                    confirm_selection();
                }
            }
            0
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
