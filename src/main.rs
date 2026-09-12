#![windows_subsystem = "windows"] // no console window for this background utility

mod config;
mod hook;
mod icons;
mod log;
mod matcher;
mod mru;
mod popup;
mod single_instance;
mod switch;
mod window_list;

use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, TranslateMessage, MSG};

fn main() {
    let log_enabled = std::env::args().skip(1).any(|a| a == "-log" || a == "--log");
    log::init(log_enabled);

    let Some(_mutex) = single_instance::acquire_or_show_error() else {
        return;
    };

    let cfg = config::load();

    let hook_handle = hook::install(cfg.hotkey_vk, cfg.hotkey_scancode);
    if hook_handle.is_null() {
        log::log("hook::install returned null -- keyboard hook did not install");
    }

    let mru_hook = mru::install();
    if mru_hook.is_null() {
        log::log("mru::install returned null -- MRU tracking hook did not install");
    }
    mru::seed_current_foreground();

    popup::set_auto_switch_delay(cfg.auto_switch_ms);
    popup::init();

    let mut msg: MSG = unsafe { std::mem::zeroed() };
    loop {
        let ret = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if ret <= 0 {
            break;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    mru::uninstall(mru_hook);
    hook::uninstall(hook_handle);
}
