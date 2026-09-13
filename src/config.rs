use std::path::PathBuf;

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_CAPITAL, VK_INSERT, VK_SCROLL};

const DEFAULT_INI: &str = "hotkey=CapsLock\n; autoswitch=500\n; regexcache=100\n; prewarm=true\n";

pub struct Config {
    pub hotkey_vk: u32,
    pub hotkey_scancode: u32,
    /// Milliseconds the filtered list must sit at exactly one match before
    /// it's auto-switched to, same as pressing Enter. `None` (the default --
    /// commented out in the generated INI) disables this entirely: a unique
    /// match then never switches on its own, no matter how long it's shown.
    pub auto_switch_ms: Option<u32>,
    /// How many recently typed search queries keep their compiled regex.
    /// 0 disables the cache.
    pub regex_cache_size: usize,
    /// Compile the 26 single-letter queries in the background at startup.
    pub prewarm: bool,
}

fn config_dir() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(base).join("wappsw")
}

fn config_path() -> PathBuf {
    config_dir().join("config.ini")
}

/// Loads `%APPDATA%\wappsw\config.ini`, creating it with defaults on first
/// run if missing. Changes require restarting the app -- no file-watching.
pub fn load() -> Config {
    let text = std::fs::read_to_string(config_path()).unwrap_or_else(|_| {
        let _ = std::fs::create_dir_all(config_dir());
        let _ = std::fs::write(config_path(), DEFAULT_INI);
        DEFAULT_INI.to_string()
    });
    parse(&text)
}

/// Parses the INI text. Unknown keys are ignored; an invalid value is
/// logged and ignored, leaving that setting at its default.
fn parse(text: &str) -> Config {
    let mut hotkey_name = "CapsLock".to_string();
    let mut auto_switch_ms = None;
    let mut regex_cache_size = crate::matcher::DEFAULT_CACHE_SIZE;
    let mut prewarm = true;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            if key.eq_ignore_ascii_case("hotkey") {
                hotkey_name = value.to_string();
            } else if key.eq_ignore_ascii_case("autoswitch") {
                match value.parse::<u32>() {
                    Ok(ms) => auto_switch_ms = Some(ms),
                    Err(_) => crate::log::log(&format!(
                        "config: autoswitch value \"{}\" is not a valid number of milliseconds, ignoring",
                        value
                    )),
                }
            } else if key.eq_ignore_ascii_case("regexcache") {
                match value.parse::<usize>() {
                    Ok(n) => regex_cache_size = n,
                    Err(_) => crate::log::log(&format!(
                        "config: regexcache value \"{}\" is not a valid entry count, ignoring",
                        value
                    )),
                }
            } else if key.eq_ignore_ascii_case("prewarm") {
                match parse_bool(value) {
                    Some(b) => prewarm = b,
                    None => crate::log::log(&format!(
                        "config: prewarm value \"{}\" is not true or false, ignoring",
                        value
                    )),
                }
            }
        }
    }

    let (hotkey_vk, hotkey_scancode) = key_name_to_vk(&hotkey_name).unwrap_or_else(|| {
        crate::log::log(&format!(
            "config: unrecognized hotkey \"{}\", falling back to CapsLock",
            hotkey_name
        ));
        (VK_CAPITAL as u32, CAPS_LOCK_SCANCODE)
    });

    Config { hotkey_vk, hotkey_scancode, auto_switch_ms, regex_cache_size, prewarm }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "on" | "yes" | "1" => Some(true),
        "false" | "off" | "no" | "0" => Some(false),
        _ => None,
    }
}

// Hardware scan codes (PC/AT set 1) for the supported hotkeys. These are
// fixed by scan-code position, not by keyboard layout -- unlike the VK
// codes above, they don't shift under layout-specific reinterpretation
// (e.g. JIS layouts routing bare Caps Lock through VK_OEM_ATTN instead of
// VK_CAPITAL). MapVirtualKeyW(vk, MAPVK_VK_TO_VSC) was tried at runtime
// instead of hardcoding these, but it can fail to resolve VK_CAPITAL at all
// on JIS-style layouts, since VK_CAPITAL isn't that scan code's primary
// (unshifted) output there -- so the hook must be told the scan code
// directly rather than deriving it from the VK.
const CAPS_LOCK_SCANCODE: u32 = 0x3A;
const SCROLL_LOCK_SCANCODE: u32 = 0x46;
const INSERT_SCANCODE: u32 = 0x52;

/// v1 supports bare single keys only -- no modifier+key combos (that's a
/// deliberate v2 deferral, see the design notes).
fn key_name_to_vk(name: &str) -> Option<(u32, u32)> {
    match name.to_ascii_lowercase().replace(['_', '-', ' '], "").as_str() {
        "capslock" | "caps" => Some((VK_CAPITAL as u32, CAPS_LOCK_SCANCODE)),
        "scrolllock" | "scroll" => Some((VK_SCROLL as u32, SCROLL_LOCK_SCANCODE)),
        "insert" | "ins" => Some((VK_INSERT as u32, INSERT_SCANCODE)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ini_uses_defaults() {
        let cfg = parse(DEFAULT_INI);
        assert_eq!(cfg.hotkey_vk, VK_CAPITAL as u32);
        assert_eq!(cfg.auto_switch_ms, None);
        assert_eq!(cfg.regex_cache_size, crate::matcher::DEFAULT_CACHE_SIZE);
        assert!(cfg.prewarm);
    }

    #[test]
    fn cache_and_prewarm_can_be_turned_off() {
        let cfg = parse("regexcache=0\nprewarm=false\n");
        assert_eq!(cfg.regex_cache_size, 0);
        assert!(!cfg.prewarm);
    }

    #[test]
    fn regexcache_takes_a_count() {
        assert_eq!(parse("regexcache=500").regex_cache_size, 500);
    }

    #[test]
    fn prewarm_accepts_common_boolean_spellings() {
        for off in ["false", "off", "no", "0", "FALSE", "Off"] {
            assert!(!parse(&format!("prewarm={}", off)).prewarm, "{}", off);
        }
        for on in ["true", "on", "yes", "1", "TRUE"] {
            assert!(parse(&format!("prewarm=false\nprewarm={}", on)).prewarm, "{}", on);
        }
    }

    #[test]
    fn invalid_values_keep_defaults() {
        let cfg = parse("regexcache=lots\nprewarm=maybe\n");
        assert_eq!(cfg.regex_cache_size, crate::matcher::DEFAULT_CACHE_SIZE);
        assert!(cfg.prewarm);
    }

    #[test]
    fn keys_are_case_insensitive_and_trimmed() {
        let cfg = parse("  RegexCache = 10 \nPREWARM = off\n");
        assert_eq!(cfg.regex_cache_size, 10);
        assert!(!cfg.prewarm);
    }
}
