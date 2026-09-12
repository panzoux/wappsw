use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Logging is off by default; only `-log`/`--log` on the command line turns
/// it on. The one exception is the startup hotkey-conflict MessageBox
/// (single_instance.rs), which always shows regardless of this setting.
pub fn init(enabled: bool) {
    ENABLED.store(enabled, Ordering::SeqCst);
}

pub fn log(msg: &str) {
    if !ENABLED.load(Ordering::SeqCst) {
        return;
    }
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    let dir = PathBuf::from(base).join("wappsw");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("log.txt")) {
        let _ = writeln!(f, "{}", msg);
    }
}
