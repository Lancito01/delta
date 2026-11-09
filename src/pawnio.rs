//! The PawnIO driver: finding it, and installing, pausing, resuming and removing it.
//!
//! Windows gives normal programs no way to read the processor's temperature sensor; it takes a
//! kernel driver. Delta doesn't ship one. It uses PawnIO (https://pawnio.eu), the open-source,
//! signed driver sensor apps share, and only if the user asks for it:
//!
//! - Nothing is installed without the user pressing Install in settings.
//! - The installer is PawnIO's own, downloaded from its GitHub release and checked against the
//!   SHA-256 pinned below before it runs.
//! - "Pause" disables the driver's device, which unloads the driver and keeps it unloaded across
//!   restarts, for games whose anti-cheat won't start next to it. "Resume" enables it again.
//!
//! All four need administrator rights, so they run in a short-lived elevated copy of Delta
//! (`delta.exe --pawnio <op>`, see `helper`); Windows asks the user first.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};
use windows::Win32::Devices::DeviceAndDriverInstallation::*;
use windows::core::{HSTRING, PCWSTR, w};

/// The PawnIO release Delta installs.
pub const VERSION: &str = "2.2.0";
const SETUP_URL: &str = "https://github.com/namazso/PawnIO.Setup/releases/download/2.2.0/PawnIO_setup.exe";
const SETUP_SIZE: usize = 3_410_960;
const SETUP_SHA256: &str = "1f519a22e47187f70a1379a48ca604981c4fcf694f4e65b734aaa74a9fba3032";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Install,
    Uninstall,
    Pause,
    Resume,
}

impl Op {
    fn arg(self) -> &'static str {
        match self {
            Op::Install => "install",
            Op::Uninstall => "uninstall",
            Op::Pause => "pause",
            Op::Resume => "resume",
        }
    }

    fn parse(s: &str) -> Option<Op> {
        [Op::Install, Op::Uninstall, Op::Pause, Op::Resume].into_iter().find(|o| o.arg() == s)
    }

    /// What to say while it runs.
    pub fn doing(self) -> &'static str {
        match self {
            Op::Install => "Downloading and installing the driver…",
            Op::Uninstall => "Removing the driver…",
            Op::Pause => "Pausing the driver…",
            Op::Resume => "Resuming the driver…",
        }
    }

    /// What to say when it worked.
    pub fn done(self) -> &'static str {
        match self {
            Op::Install => "The driver is installed.",
            Op::Uninstall => "The driver is removed.",
            Op::Pause => "The driver is paused. It stays unloaded, also after a restart, until you resume it.",
            Op::Resume => "The driver is running again.",
        }
    }
}

/// Where a driver job stands (shown in settings).
#[derive(Clone, Debug, PartialEq)]
pub enum Job {
    Idle,
    Running(Op),
    /// Finished: `Ok(true)` means it worked but Windows wants a restart to complete it.
    Done(Op, Result<bool, String>),
}

// ---------------------------------------------------------------- sharing the driver with the sensor thread

/// Set while a job runs: the sensor thread doesn't open the driver, so it can be unloaded.
static HOLD: AtomicBool = AtomicBool::new(false);
/// Bumped when a job ends, so the sensor thread looks again right away.
static EPOCH: AtomicU32 = AtomicU32::new(0);
/// Held by the sensor thread for the moment it has the driver open.
pub static GATE: Mutex<()> = Mutex::new(());

pub fn held() -> bool {
    HOLD.load(Ordering::Acquire)
}

pub fn epoch() -> u32 {
    EPOCH.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------- state

fn reg_string(key: PCWSTR, value: PCWSTR) -> Option<String> {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let mut buf = [0u16; 512];
    let mut len = (buf.len() * 2) as u32;
    unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key, value, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut len)) }.ok().ok()?;
    let n = (len as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..n]).trim().to_string())
}

/// The installed PawnIO version, as its installer registered it.
pub fn installed_version() -> Option<String> {
    reg_string(w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\PawnIO"), w!("DisplayVersion"))
}

pub fn installed() -> bool {
    if installed_version().is_some() {
        return true;
    }
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    std::path::Path::new(&windir).join("System32\\drivers\\PawnIO.sys").exists()
}

/// The state of PawnIO's device, as Windows' device manager sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    Missing,
    /// Switched off (what Pause does): the driver isn't loaded.
    Disabled,
    Started,
    /// Not running, with Device Manager's problem code (0: none given, e.g. waiting for a restart).
    Problem(u32),
}

const HARDWARE_ID: &str = "root\\pawnio";

/// Hardware ids of every root-enumerated device, with its device instance handle.
fn root_devices() -> Vec<(u32, Vec<String>)> {
    let mut out = vec![];
    unsafe {
        let Ok(set) = SetupDiGetClassDevsW(None, w!("ROOT"), None, DIGCF_ALLCLASSES) else { return out };
        for i in 0.. {
            let mut d = SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            if SetupDiEnumDeviceInfo(set, i, &mut d).is_err() {
                break;
            }
            let mut buf = [0u8; 2048];
            let mut need = 0u32;
            if SetupDiGetDeviceRegistryPropertyW(set, &d, SPDRP_HARDWAREID, None, Some(&mut buf), Some(&mut need)).is_err() {
                continue;
            }
            // REG_MULTI_SZ: UTF-16 strings, each ending in a zero.
            let n = (need as usize).min(buf.len()) / 2;
            let wide: Vec<u16> = buf[..n * 2].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let ids = wide.split(|c| *c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect();
            out.push((d.DevInst, ids));
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
    }
    out
}

fn find() -> Option<u32> {
    root_devices().into_iter().find(|(_, ids)| ids.iter().any(|i| i.eq_ignore_ascii_case(HARDWARE_ID))).map(|(inst, _)| inst)
}

fn state_of(inst: u32) -> Device {
    let mut status = CM_DEVNODE_STATUS_FLAGS(0);
    let mut problem = CM_PROB(0);
    if unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, inst, 0) } != CR_SUCCESS {
        return Device::Missing; // registered, but not there (a leftover entry)
    }
    if status.0 & DN_STARTED.0 != 0 {
        Device::Started
    } else if problem == CM_PROB_DISABLED {
        Device::Disabled
    } else {
        Device::Problem(problem.0)
    }
}

pub fn device() -> Device {
    find().map_or(Device::Missing, state_of)
}

// ---------------------------------------------------------------- asking for a job (normal side)

// Exit codes of the elevated helper. Anything else is PawnIO's installer's own code (a Windows error number).
const X_OK: i32 = 0;
const X_RESTART: i32 = 3010; // ERROR_SUCCESS_REBOOT_REQUIRED, which the installer uses too
const X_DOWNLOAD: i32 = 9001;
const X_VERIFY: i32 = 9002;
const X_NO_DEVICE: i32 = 9003;
const X_IN_USE: i32 = 9004;
const X_DEVICE: i32 = 9005;
const X_FILE: i32 = 9006;
const X_RUN: i32 = 9007;
const X_NOT_ADMIN: i32 = 9008;

fn explain(op: Op, code: i32) -> String {
    match code {
        X_DOWNLOAD => "Couldn't download PawnIO's installer from GitHub. Check your internet connection and try again.".into(),
        X_VERIFY => "The downloaded installer doesn't match the checksum Delta expects, so it wasn't run.".into(),
        X_NO_DEVICE => "Windows doesn't list the driver's device. Uninstall it and install it again.".into(),
        X_IN_USE => "Another app is using the driver (a fan, RGB or sensor tool?). Close it and try again.".into(),
        X_DEVICE => format!("Windows wouldn't {} the driver's device.", if op == Op::Resume { "start" } else { "stop" }),
        X_FILE => "Couldn't save the installer to the temporary folder.".into(),
        X_RUN => "Couldn't start PawnIO's installer.".into(),
        X_NOT_ADMIN => "Delta didn't get administrator rights.".into(),
        c => format!("PawnIO's installer stopped with error {c}."),
    }
}

/// Runs a driver job in an elevated copy of Delta and waits for it. Blocks: call it off the UI thread.
pub fn run(op: Op) -> Result<bool, String> {
    HOLD.store(true, Ordering::Release);
    drop(GATE.lock()); // wait out a sensor read in flight; it closes the driver before unlocking
    let r = run_elevated(op);
    HOLD.store(false, Ordering::Release);
    EPOCH.fetch_add(1, Ordering::Relaxed);
    r
}

fn run_elevated(op: Op) -> Result<bool, String> {
    use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, WAIT_OBJECT_0};
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let file = HSTRING::from(exe.as_os_str());
    let params = HSTRING::from(format!("--pawnio {}", op.arg()));
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(params.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        if let Err(e) = ShellExecuteExW(&mut info) {
            return Err(if e.code() == ERROR_CANCELLED.to_hresult() {
                "Windows' permission prompt was declined, so nothing was changed.".into()
            } else {
                format!("Couldn't ask Windows for administrator rights: {}", e.message())
            });
        }
        let process = info.hProcess;
        if process.is_invalid() {
            return Err("Couldn't start the elevated helper.".into());
        }
        let waited = WaitForSingleObject(process, 5 * 60 * 1000);
        let mut code = 0u32;
        let got = GetExitCodeProcess(process, &mut code);
        let _ = CloseHandle(process);
        if waited != WAIT_OBJECT_0 || got.is_err() {
            return Err("The driver job didn't finish in time.".into());
        }
        match code as i32 {
            X_OK => Ok(false),
            X_RESTART => Ok(true),
            c => Err(explain(op, c)),
        }
    }
}

// ---------------------------------------------------------------- doing a job (elevated side)

/// `delta.exe --pawnio <op>`: does the job and returns the process exit code.
pub fn helper(arg: &str) -> i32 {
    if !crate::metrics::system::is_elevated() {
        return X_NOT_ADMIN;
    }
    match Op::parse(arg) {
        Some(Op::Install) => setup(&["-install", "-silent"]),
        Some(Op::Uninstall) => {
            // A disabled device is still there; removing a paused driver is fine. The installer is
            // fetched again because its own uninstaller's command line isn't a documented interface.
            setup(&["-uninstall", "-silent"])
        }
        Some(Op::Pause) => set_enabled(false),
        Some(Op::Resume) => set_enabled(true),
        None => 87, // ERROR_INVALID_PARAMETER
    }
}

fn genuine(data: &[u8]) -> bool {
    data.len() == SETUP_SIZE && data.starts_with(b"MZ") && crate::update::sha256_hex(data) == SETUP_SHA256
}

/// Downloads PawnIO's installer, checks it, and runs it with `args`.
fn setup(args: &[&str]) -> i32 {
    let Ok(data) = crate::update::get(SETUP_URL, "application/octet-stream") else { return X_DOWNLOAD };
    if !genuine(&data) {
        return X_VERIFY;
    }
    let dir = std::env::temp_dir().join(format!("delta-pawnio-{}", std::process::id()));
    let path = dir.join("PawnIO_setup.exe");
    if std::fs::create_dir_all(&dir).is_err() || std::fs::write(&path, &data).is_err() {
        return X_FILE;
    }
    let code = run_checked(&path, args, genuine);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
    code
}

/// Runs the installer at `path`, after checking the file once more through a handle that stays
/// open until it exits. While that handle is open nothing can write to, replace or rename the
/// file, so what runs elevated is what was checked.
fn run_checked(path: &std::path::Path, args: &[&str], genuine: fn(&[u8]) -> bool) -> i32 {
    use std::io::Read;
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 1;
    let Ok(mut pinned) = std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(path) else { return X_FILE };
    let mut on_disk = vec![];
    if pinned.read_to_end(&mut on_disk).is_err() {
        return X_FILE;
    }
    if !genuine(&on_disk) {
        return X_VERIFY;
    }
    let code = std::process::Command::new(path).args(args).status().ok().and_then(|s| s.code()).unwrap_or(X_RUN);
    drop(pinned);
    code
}

fn set_enabled(on: bool) -> i32 {
    let Some(inst) = find() else { return X_NO_DEVICE };
    let r = unsafe {
        if on {
            CM_Enable_DevNode(inst, 0)
        } else {
            // Persist: stay off after a restart too (anti-cheats that load at boot look then).
            CM_Disable_DevNode(inst, CM_DISABLE_UI_NOT_OK | CM_DISABLE_PERSIST)
        }
    };
    if r == CR_REMOVE_VETOED {
        return X_IN_USE;
    }
    if r != CR_SUCCESS {
        return X_DEVICE;
    }
    // Starting and stopping finish a moment later.
    let want = if on { Device::Started } else { Device::Disabled };
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let now = state_of(inst);
        if now == want {
            return X_OK;
        }
        if Instant::now() > until {
            // Disabled on paper, but still loaded until Windows restarts.
            return if !on && now != Device::Started { X_RESTART } else { X_DEVICE };
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ops_round_trip() {
        for op in [Op::Install, Op::Uninstall, Op::Pause, Op::Resume] {
            assert_eq!(Op::parse(op.arg()), Some(op));
        }
        assert_eq!(Op::parse("format-c"), None);
    }

    #[test]
    fn rejects_other_files() {
        assert!(!genuine(b"MZ not the installer"));
        assert!(!genuine(&vec![0u8; SETUP_SIZE]));
    }

    /// A file that isn't the pinned installer never runs, whatever its name.
    #[test]
    fn checked_run_refuses_a_swapped_file() {
        let dir = std::env::temp_dir().join(format!("delta-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("PawnIO_setup.exe");
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        std::fs::copy(format!("{windir}\\System32\\whoami.exe"), &path).unwrap();
        assert_eq!(run_checked(&path, &[], genuine), X_VERIFY);
        // And a file that passes the check does run while it's pinned.
        assert_eq!(run_checked(&path, &[], |_| true), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Windows lists root devices to a normal user, and their state can be read (nothing is changed).
    #[test]
    fn reads_root_devices() {
        let devices = root_devices();
        assert!(!devices.is_empty(), "no root devices listed");
        assert!(devices.iter().any(|(_, ids)| !ids.is_empty()));
        assert!(devices.iter().any(|(inst, _)| state_of(*inst) == Device::Started));
        println!("PawnIO installed: {} ({:?}), device: {:?}", installed(), installed_version(), device());
    }

    /// Downloads the real installer and checks it against the pinned checksum (doesn't run it).
    /// `cargo test live_installer -- --ignored`
    #[test]
    #[ignore]
    fn live_installer() {
        let data = crate::update::get(SETUP_URL, "application/octet-stream").unwrap();
        assert!(genuine(&data), "{} bytes, sha256 {}", data.len(), crate::update::sha256_hex(&data));
    }
}
