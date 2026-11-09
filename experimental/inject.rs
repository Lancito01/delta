//! Loads Delta's hook into a game (64-bit only for now): LoadLibraryW on a remote thread, the
//! classic way overlays like RivaTuner get into games.

use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Diagnostics::Debug::WriteProcessMemory;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Memory::{MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx, VirtualFreeEx};
use windows::Win32::System::SystemInformation::IMAGE_FILE_MACHINE_UNKNOWN;
use windows::Win32::System::Threading::*;
use windows::core::{s, w};

struct H(HANDLE);
impl Drop for H {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub enum Error {
    /// Can't open the game (it runs with more rights than Delta, or protects itself).
    Access,
    ThirtyTwoBit,
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::Access => write!(f, "Delta isn't allowed to open the game (it may run as administrator)"),
            Error::ThirtyTwoBit => write!(f, "32-bit games aren't supported yet"),
            Error::Other(e) => write!(f, "{e}"),
        }
    }
}

/// Where the hook DLL lives: next to delta.exe.
pub fn hook_source() -> Option<PathBuf> {
    let p = std::env::current_exe().ok()?.parent()?.join("delta_hook.dll");
    p.exists().then_some(p)
}

/// Copies the hook to a versioned file before loading it, so a game holding it open never blocks
/// updating Delta, and each version loads its own copy.
pub fn staged_hook(src: &Path, dir: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(src).ok()?;
    let stamp = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let dst = dir.join(format!("delta_hook-{stamp:x}-{:x}.dll", meta.len()));
    if !dst.exists() {
        std::fs::create_dir_all(dir).ok()?;
        std::fs::copy(src, &dst).ok()?;
    }
    Some(dst)
}

pub fn is_32bit(process: HANDLE) -> bool {
    let mut machine = Default::default();
    unsafe { IsWow64Process2(process, &mut machine, None).is_ok() && machine != IMAGE_FILE_MACHINE_UNKNOWN }
}

pub fn inject(pid: u32, dll: &Path) -> Result<(), Error> {
    unsafe {
        let access = PROCESS_CREATE_THREAD | PROCESS_QUERY_INFORMATION | PROCESS_VM_OPERATION | PROCESS_VM_WRITE | PROCESS_VM_READ;
        let p = H(OpenProcess(access, false, pid).map_err(|_| Error::Access)?);
        if is_32bit(p.0) {
            return Err(Error::ThirtyTwoBit);
        }
        let path: Vec<u16> = dll.as_os_str().encode_wide_null();
        let bytes = path.len() * 2;
        let mem = VirtualAllocEx(p.0, None, bytes, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
        if mem.is_null() {
            return Err(Error::Other("couldn't reserve memory in the game".into()));
        }
        let result = (|| {
            WriteProcessMemory(p.0, mem, path.as_ptr() as *const _, bytes, None).map_err(|e| Error::Other(e.to_string()))?;
            // kernel32 is at the same address in every 64-bit process this boot.
            let k32 = GetModuleHandleW(w!("kernel32.dll")).map_err(|e| Error::Other(e.to_string()))?;
            let load = GetProcAddress(k32, s!("LoadLibraryW")).ok_or_else(|| Error::Other("no LoadLibraryW".into()))?;
            let thread = H(CreateRemoteThread(p.0, None, 0, Some(std::mem::transmute::<_, LPTHREAD_START_ROUTINE>(load).unwrap()), Some(mem), 0, None).map_err(|_| Error::Access)?);
            if WaitForSingleObject(thread.0, 5000) != WAIT_OBJECT_0 {
                return Err(Error::Other("the game didn't finish loading the overlay in time".into()));
            }
            let mut code = 0u32;
            let _ = GetExitCodeThread(thread.0, &mut code);
            if code == 0 {
                return Err(Error::Other("the game refused to load the overlay".into()));
            }
            Ok(())
        })();
        let _ = VirtualFreeEx(p.0, mem, 0, MEM_RELEASE);
        result
    }
}

trait WideNull {
    fn encode_wide_null(&self) -> Vec<u16>;
}

impl WideNull for std::ffi::OsStr {
    fn encode_wide_null(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(std::iter::once(0)).collect()
    }
}
