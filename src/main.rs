#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Delta: an open-source, themeable performance overlay for Windows.

mod app;
mod bridge;
mod compose;
mod config;
mod hotkey;
mod log;
mod metrics;
mod osd;
mod overlay;
mod rtss;
mod theme;
mod tray;
mod ui;
mod update;

use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HINSTANCE};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, MB_ICONERROR, MB_OK, MessageBoxW, PostMessageW};
use windows::core::{HSTRING, w};

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        // One instance. When restarting elevated, the new copy waits for the old one to exit.
        let wait = std::env::args().any(|a| a == "--wait");
        let mut tries = if wait { 50 } else { 1 };
        // A separate profile is a separate instance.
        let name = match config::Paths::profile_arg() {
            Some(p) => format!("Local\\DeltaOverlay-{:x}", p.to_string_lossy().bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64))),
            None => "Local\\DeltaOverlaySingleInstance".to_string(),
        };
        let mutex = loop {
            let m = CreateMutexW(None, true, &HSTRING::from(name.as_str()));
            if GetLastError() != ERROR_ALREADY_EXISTS {
                break m;
            }
            if let Ok(h) = m {
                let _ = CloseHandle(h);
            }
            tries -= 1;
            if tries == 0 {
                // Already running: bring up its settings instead.
                if let Ok(other) = FindWindowW(w!("DeltaMain"), None) {
                    let _ = PostMessageW(Some(other), app::WM_OPEN_SETTINGS, WPARAM(0), LPARAM(0));
                }
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };

        let instance: HINSTANCE = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
        if let Err(e) = app::run(instance) {
            MessageBoxW(None, &HSTRING::from(format!("Delta couldn't start:\n\n{e}")), w!("Delta"), MB_OK | MB_ICONERROR);
        }
        drop(mutex);
    }
}
