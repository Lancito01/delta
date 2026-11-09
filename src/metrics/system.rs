//! Memory, the foreground app, fullscreen detection and process helpers.

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, RECT};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId};
use windows::core::PWSTR;

pub struct Memory {
    pub used_gb: f64,
    pub total_gb: f64,
    pub pct: f64,
}

pub fn memory() -> Option<Memory> {
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe { GlobalMemoryStatusEx(&mut m) }.ok()?;
    let gb = 1024.0 * 1024.0 * 1024.0;
    let total = m.ullTotalPhys as f64 / gb;
    let used = (m.ullTotalPhys - m.ullAvailPhys) as f64 / gb;
    Some(Memory { used_gb: used, total_gb: total, pct: m.dwMemoryLoad as f64 })
}

#[derive(Clone, Default, Debug)]
pub struct Foreground {
    pub hwnd: HWND,
    pub pid: u32,
    pub exe: String,
    pub fullscreen: bool,
    /// Window rect (screen px).
    pub rect: (i32, i32, i32, i32),
}

pub fn foreground() -> Foreground {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return Foreground::default();
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let mut r = RECT::default();
        let _ = GetWindowRect(hwnd, &mut r);
        Foreground {
            hwnd,
            pid,
            exe: cached_name(pid),
            fullscreen: covers_monitor(hwnd, &r),
            rect: (r.left, r.top, r.right, r.bottom),
        }
    }
}

fn covers_monitor(hwnd: HWND, r: &RECT) -> bool {
    unsafe {
        let mut class = [0u16; 64];
        let n = GetClassNameW(hwnd, &mut class) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        // The desktop and shell windows cover the screen but aren't "fullscreen apps".
        if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Windows.UI.Core.CoreWindow") {
            return false;
        }
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(mon, &mut mi).as_bool() {
            return false;
        }
        let m = mi.rcMonitor;
        r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
    }
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Name of the foreground process, remembered while the pid stays the same (asked many times a second).
fn cached_name(pid: u32) -> String {
    static LAST: std::sync::Mutex<(u32, String)> = std::sync::Mutex::new((0, String::new()));
    let mut last = LAST.lock().unwrap();
    if last.0 != pid || last.1.is_empty() {
        *last = (pid, process_name(pid).unwrap_or_default());
    }
    last.1.clone()
}

pub fn process_name(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let h = Handle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).ok()?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        Some(path.rsplit('\\').next().unwrap_or(&path).to_string())
    }
}

/// Apps that are never games, whatever they draw with: Delta never loads into or draws inside them.
const NOT_GAMES: &[&str] = &[
    "explorer.exe", "dwm.exe", "searchhost.exe", "startmenuexperiencehost.exe", "shellexperiencehost.exe", "textinputhost.exe",
    "applicationframehost.exe", "systemsettings.exe", "lockapp.exe", "taskmgr.exe", "mmc.exe", "conhost.exe", "windowsterminal.exe",
    "wt.exe", "cmd.exe", "powershell.exe", "pwsh.exe", "notepad.exe", "mspaint.exe", "chrome.exe", "msedge.exe", "msedgewebview2.exe",
    "firefox.exe", "zen.exe", "brave.exe", "opera.exe", "opera_gx.exe", "vivaldi.exe", "arc.exe", "discord.exe", "slack.exe",
    "teams.exe", "ms-teams.exe", "spotify.exe", "code.exe", "cursor.exe", "devenv.exe", "rider64.exe", "idea64.exe", "obs64.exe",
    "obs32.exe", "steam.exe", "steamwebhelper.exe", "epicgameslauncher.exe", "battle.net.exe", "eadesktop.exe", "upc.exe",
    "galaxyclient.exe", "riotclientservices.exe", "vlc.exe", "mpc-hc64.exe", "mpv.exe", "potplayermini64.exe", "claude.exe",
    "msiafterburner.exe", "rtss.exe", "delta.exe", "nvidia app.exe", "nvcontainer.exe", "photoshop.exe", "blender.exe",
    "davinci resolve.exe", "resolve.exe", "premiere pro.exe", "afterfx.exe", "unity.exe", "unrealeditor.exe", "zoom.exe", "whatsapp.exe",
    "telegram.exe", "signal.exe", "outlook.exe", "winword.exe", "excel.exe", "powerpnt.exe", "onenote.exe", "acrobat.exe",
];

/// Apps Delta should never treat as games (system apps, browsers, chat, tools).
pub fn never_a_game(exe: &str, path: Option<&std::path::Path>) -> bool {
    let e = exe.to_ascii_lowercase();
    if e.is_empty() || NOT_GAMES.contains(&e.as_str()) {
        return true;
    }
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into()).to_ascii_lowercase();
    path.is_some_and(|p| p.to_string_lossy().to_ascii_lowercase().starts_with(&windir))
}

/// Working set of a process in GB.
pub fn process_ram_gb(pid: u32) -> Option<f64> {
    unsafe {
        let h = Handle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let mut c = PROCESS_MEMORY_COUNTERS { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
        K32GetProcessMemoryInfo(h.0, &mut c, c.cb).as_bool().then(|| c.WorkingSetSize as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Exe names of apps with a visible window (for picking apps in settings), sorted, no duplicates.
pub fn windowed_apps() -> Vec<String> {
    use windows::Win32::Foundation::LPARAM;
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextLengthW, IsWindowVisible};
    unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> windows::core::BOOL {
        unsafe {
            if IsWindowVisible(hwnd).as_bool() && GetWindowTextLengthW(hwnd) > 0 {
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                (*(data.0 as *mut Vec<u32>)).push(pid);
            }
        }
        true.into()
    }
    let mut pids: Vec<u32> = vec![];
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut pids as *mut _ as isize));
    }
    let me = std::process::id();
    let mut names: Vec<String> = pids.into_iter().filter(|p| *p != me).filter_map(process_name).collect();
    names.sort_by_key(|n| n.to_ascii_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    names.retain(|n| !matches!(n.to_ascii_lowercase().as_str(), "applicationframehost.exe" | "textinputhost.exe" | "systemsettings.exe"));
    names
}

pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let token = Handle(token);
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        GetTokenInformation(token.0, TokenElevation, Some(&mut e as *mut _ as *mut _), std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut len)
            .is_ok()
            && e.TokenIsElevated != 0
    }
}

pub fn cpu_name() -> String {
    // The registry has the marketing name; avoids pulling in WMI.
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::w;
    let mut buf = [0u16; 128];
    let mut len = (buf.len() * 2) as u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0"),
            w!("ProcessorNameString"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut len),
        )
    };
    if ok.is_err() {
        return "CPU".into();
    }
    let n = (len as usize / 2).saturating_sub(1);
    String::from_utf16_lossy(&buf[..n]).trim().to_string()
}
