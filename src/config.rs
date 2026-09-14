use std::path::PathBuf;

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_CAPITAL, VK_INSERT, VK_SCROLL};

const DEFAULT_INI: &str = "hotkey=CapsLock\n; autoswitch=500\n; regexcache=100\n; prewarm=true\n";

/// Auto-switch delay when the INI doesn't set `autoswitch` (or sets it to
/// a bare `on`/`yes`/`true`).
const DEFAULT_AUTO_SWITCH_MS: u32 = 500;

/// What the configured `hotkey=` value means. Exactly one of these is ever
/// active -- main.rs installs `hook::install` for `SingleKey` or
/// `alttab_hook::install` for `AltTab`, never both. See
/// docs/alt-tab-hotkey.md for why `AltTab` needs its own hook module rather
/// than being a third field on `SingleKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyMode {
    SingleKey { vk: u32, scancode: u32 },
    AltTab,
}

pub struct Config {
    pub hotkey_mode: HotkeyMode,
    /// Milliseconds the filtered list must sit at exactly one match before
    /// it's auto-switched to, same as pressing Enter. On by default
    /// (`DEFAULT_AUTO_SWITCH_MS`); `None` -- from `autoswitch=off`/`no`/`0` --
    /// disables it: a unique match then never switches on its own.
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
    let path = config_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => {
            crate::log::log(&format!("config: reading {}", path.display()));
            text
        }
        Err(e) => {
            crate::log::log(&format!(
                "config: cannot read {} ({}), creating it with defaults",
                path.display(),
                e
            ));
            let _ = std::fs::create_dir_all(config_dir());
            let _ = std::fs::write(&path, DEFAULT_INI);
            DEFAULT_INI.to_string()
        }
    };
    let cfg = parse(&text);
    let hotkey_desc = match cfg.hotkey_mode {
        HotkeyMode::SingleKey { vk, scancode } => format!("single-key vk={:#x} scan={:#x}", vk, scancode),
        HotkeyMode::AltTab => "AltTab".to_string(),
    };
    crate::log::log(&format!(
        "config: in effect: hotkey={}, autoswitch={}, regexcache={}, prewarm={}",
        hotkey_desc,
        match cfg.auto_switch_ms {
            Some(ms) => format!("{} ms", ms),
            None => "off".to_string(),
        },
        cfg.regex_cache_size,
        cfg.prewarm
    ));
    cfg
}

/// Parses the INI text. Unknown keys are ignored; an invalid value is
/// logged and ignored, leaving that setting at its default.
fn parse(text: &str) -> Config {
    let mut hotkey_name = "CapsLock".to_string();
    let mut auto_switch_ms = Some(DEFAULT_AUTO_SWITCH_MS);
    let mut regex_cache_size = crate::matcher::DEFAULT_CACHE_SIZE;
    let mut prewarm = true;
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        crate::log::log(&format!("config: line {}: {}", n + 1, line));
        let Some((key, value)) = line.split_once('=') else {
            crate::log::log(&format!("config: line {} has no '=', ignoring", n + 1));
            continue;
        };
        {
            let key = key.trim();
            let value = value.trim();
            if key.eq_ignore_ascii_case("hotkey") {
                hotkey_name = value.to_string();
            } else if key.eq_ignore_ascii_case("autoswitch")
                || key.eq_ignore_ascii_case("autoselect")
            {
                match parse_auto_switch(value) {
                    Some(ms) => auto_switch_ms = ms,
                    None => crate::log::log(&format!(
                        "config: autoswitch value \"{}\" is not a number of milliseconds or off, ignoring",
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
            } else {
                crate::log::log(&format!("config: unknown key \"{}\", ignoring", key));
            }
        }
    }

    let hotkey_mode = key_name_to_mode(&hotkey_name).unwrap_or_else(|| {
        crate::log::log(&format!(
            "config: unrecognized hotkey \"{}\", falling back to CapsLock",
            hotkey_name
        ));
        HotkeyMode::SingleKey { vk: VK_CAPITAL as u32, scancode: CAPS_LOCK_SCANCODE }
    });

    Config { hotkey_mode, auto_switch_ms, regex_cache_size, prewarm }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "on" | "yes" | "1" => Some(true),
        "false" | "off" | "no" | "0" => Some(false),
        _ => None,
    }
}

/// `Some(delay)` for a valid value, `None` if unparseable. A number is the
/// delay in ms, with `0` meaning off; the boolean spellings turn it off or
/// back on at the default delay.
fn parse_auto_switch(value: &str) -> Option<Option<u32>> {
    if let Ok(ms) = value.parse::<u32>() {
        return Some((ms != 0).then_some(ms));
    }
    parse_bool(value).map(|on| on.then_some(DEFAULT_AUTO_SWITCH_MS))
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

/// v1 single-key values support bare keys only -- no modifier+key combos
/// (that's a deliberate v2 deferral, see the design notes). `AltTab` is the
/// one non-single-key value; see docs/alt-tab-hotkey.md.
fn key_name_to_mode(name: &str) -> Option<HotkeyMode> {
    match name.to_ascii_lowercase().replace(['_', '-', ' '], "").as_str() {
        "capslock" | "caps" => Some(HotkeyMode::SingleKey { vk: VK_CAPITAL as u32, scancode: CAPS_LOCK_SCANCODE }),
        "scrolllock" | "scroll" => Some(HotkeyMode::SingleKey { vk: VK_SCROLL as u32, scancode: SCROLL_LOCK_SCANCODE }),
        "insert" | "ins" => Some(HotkeyMode::SingleKey { vk: VK_INSERT as u32, scancode: INSERT_SCANCODE }),
        "alttab" | "alt+tab" => Some(HotkeyMode::AltTab),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ini_uses_defaults() {
        let cfg = parse(DEFAULT_INI);
        assert_eq!(cfg.hotkey_mode, HotkeyMode::SingleKey { vk: VK_CAPITAL as u32, scancode: CAPS_LOCK_SCANCODE });
        assert_eq!(cfg.auto_switch_ms, Some(DEFAULT_AUTO_SWITCH_MS));
        assert_eq!(cfg.regex_cache_size, crate::matcher::DEFAULT_CACHE_SIZE);
        assert!(cfg.prewarm);
    }

    #[test]
    fn hotkey_alttab_is_recognized() {
        for spelling in ["AltTab", "alttab", "Alt+Tab", "alt+tab", "Alt-Tab", "Alt Tab"] {
            assert_eq!(
                parse(&format!("hotkey={}", spelling)).hotkey_mode,
                HotkeyMode::AltTab,
                "{}",
                spelling
            );
        }
    }

    #[test]
    fn unrecognized_hotkey_falls_back_to_capslock() {
        assert_eq!(
            parse("hotkey=NotAKey").hotkey_mode,
            HotkeyMode::SingleKey { vk: VK_CAPITAL as u32, scancode: CAPS_LOCK_SCANCODE }
        );
    }

    #[test]
    fn autoswitch_is_on_at_500ms_when_absent() {
        assert_eq!(parse("").auto_switch_ms, Some(500));
        assert_eq!(parse("hotkey=Insert\n").auto_switch_ms, Some(500));
    }

    #[test]
    fn autoswitch_can_be_turned_off() {
        for off in ["off", "no", "0", "false", "OFF", "No"] {
            assert_eq!(parse(&format!("autoswitch={}", off)).auto_switch_ms, None, "{}", off);
        }
    }

    #[test]
    fn autoswitch_takes_a_delay_or_on() {
        assert_eq!(parse("autoswitch=250").auto_switch_ms, Some(250));
        assert_eq!(parse("autoswitch=off\nautoswitch=on").auto_switch_ms, Some(500));
    }

    #[test]
    fn autoselect_is_an_alias() {
        assert_eq!(parse("autoselect=off").auto_switch_ms, None);
        assert_eq!(parse("AutoSelect=300").auto_switch_ms, Some(300));
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
        let cfg = parse("regexcache=lots\nprewarm=maybe\nautoswitch=soon\n");
        assert_eq!(cfg.regex_cache_size, crate::matcher::DEFAULT_CACHE_SIZE);
        assert!(cfg.prewarm);
        assert_eq!(cfg.auto_switch_ms, Some(DEFAULT_AUTO_SWITCH_MS));
    }

    #[test]
    fn keys_are_case_insensitive_and_trimmed() {
        let cfg = parse("  RegexCache = 10 \nPREWARM = off\n");
        assert_eq!(cfg.regex_cache_size, 10);
        assert!(!cfg.prewarm);
    }
}
