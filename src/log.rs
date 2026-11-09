//! A tiny log file next to the config (delta.log), restarted on each run.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

static FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);

pub fn init(path: PathBuf) {
    if let Ok(f) = std::fs::File::create(path) {
        *FILE.lock().unwrap() = Some(f);
    }
}

pub fn write(msg: &str) {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let line = format!("{:02}:{:02}:{:02}.{:03}  {msg}\n", t.wHour, t.wMinute, t.wSecond, t.wMilliseconds);
    if let Ok(mut g) = FILE.lock() {
        if let Some(f) = g.as_mut() {
            let _ = f.write_all(line.as_bytes());
        }
    }
    #[cfg(debug_assertions)]
    eprint!("{line}");
}

#[macro_export]
macro_rules! log {
    ($($t:tt)*) => { $crate::log::write(&format!($($t)*)) };
}
