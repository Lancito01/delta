//! RTSS compatibility. Apps such as HWiNFO, AIDA64 and OBS HUD put text (and images) on screen by
//! writing into RivaTuner Statistics Server's shared memory, `RTSSSharedMemoryV2`, and RTSS draws it.
//! While RTSS itself isn't running, Delta offers that same shared memory, so those apps work
//! unchanged, and Delta draws what they write. Layout follows RTSS's public SDK
//! (RTSSSharedMemory.h); only the parts clients use are filled in. Delta also publishes per-app
//! frame counters there, so tools that read FPS from RTSS can read Delta's.
//! When RTSS starts, Delta steps aside and closes its copy.

use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree};
use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::core::w;
use std::sync::atomic::{AtomicI32, Ordering};

const SIGNATURE: u32 = 0x52545353; // 'RTSS'
const VERSION: u32 = 0x0002_0014; // 2.20: text tags, busy lock, app resolution

const HEADER_SIZE: usize = 4096;
const OSD_ENTRY_SIZE: usize = 256 + 256 + 4096 + 262_144;
const OSD_COUNT: usize = 8;
const APP_ENTRY_SIZE: usize = 12_288;
const APP_COUNT: usize = 256;
const OSD_OFFSET: usize = HEADER_SIZE;
const APP_OFFSET: usize = OSD_OFFSET + OSD_ENTRY_SIZE * OSD_COUNT;
const TOTAL: usize = APP_OFFSET + APP_ENTRY_SIZE * APP_COUNT;

// Header fields
const H_OSD_FRAME: usize = 32;
const H_BUSY: usize = 36;
// OSD entry fields
const O_TEXT: usize = 0; // szOSD[256]
const O_OWNER: usize = 256; // szOSDOwner[256]
const O_TEXT_EX: usize = 512; // szOSDEx[4096]
const O_BUFFER: usize = 4608; // buffer[262144], for embedded objects
const O_BUFFER_SIZE: usize = 262_144;
// App entry fields
const A_PID: usize = 0;
const A_NAME: usize = 4;
const A_FLAGS: usize = 264;
const A_TIME0: usize = 268;
const A_TIME1: usize = 272;
const A_FRAMES: usize = 276;
const A_FRAMETIME: usize = 280;
const A_OSD_PIXEL: usize = 324;
const A_RES_X: usize = 9224;
const A_RES_Y: usize = 9228;
const APPFLAG_D3D11: u32 = 0x0007;

#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub owner: String,
    pub text: String,
}

pub struct AppInfo {
    pub pid: u32,
    pub name: String,
    pub frames_1s: u32,
    pub frametime_ms: f64,
    pub width: u32,
    pub height: u32,
}

pub enum Status {
    Off,
    /// We provide the shared memory.
    Serving,
    /// The real RTSS is running; Delta stays out of its way.
    RtssRunning,
    Failed(String),
}

pub struct Server {
    map: HANDLE,
    view: *mut u8,
    pub status: Status,
    last_check: u32,
    slots: Vec<Slot>,
    last_frame: Option<u32>,
}

impl Server {
    pub fn new() -> Server {
        Server { map: HANDLE::default(), view: std::ptr::null_mut(), status: Status::Off, last_check: 0, slots: vec![], last_frame: None }
    }

    /// Call regularly. Opens/closes the shared memory depending on `enabled` and whether RTSS runs.
    pub fn tick(&mut self, enabled: bool) {
        let now = unsafe { GetTickCount() };
        if !enabled {
            self.close();
            self.status = Status::Off;
            return;
        }
        if now.wrapping_sub(self.last_check) < 2000 && self.last_check != 0 {
            return;
        }
        self.last_check = now;
        if rtss_process_running() {
            self.close();
            self.status = Status::RtssRunning;
            return;
        }
        if self.view.is_null() {
            match self.open() {
                Ok(()) => self.status = Status::Serving,
                Err(e) => self.status = Status::Failed(e),
            }
        }
    }

    fn open(&mut self) -> Result<(), String> {
        unsafe {
            // Someone else (RTSS starting up, another Delta) already has it: leave it alone.
            if let Ok(h) = OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, w!("RTSSSharedMemoryV2")) {
                let _ = CloseHandle(h);
                return Err("RTSSSharedMemoryV2 is already in use by another app".into());
            }
            // Open to everyone in this session at low integrity, so non-elevated apps can write to it
            // even when Delta runs as administrator (which frame timing needs).
            let mut sd = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(w!("D:(A;;GA;;;WD)(A;;GA;;;AC)S:(ML;;NW;;;LW)"), SDDL_REVISION_1, &mut sd, None)
                .map_err(|e| e.to_string())?;
            let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd.0, bInheritHandle: false.into() };
            let map = CreateFileMappingW(INVALID_HANDLE_VALUE, Some(&sa), PAGE_READWRITE, 0, TOTAL as u32, w!("RTSSSharedMemoryV2"));
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            let map = map.map_err(|e| e.to_string())?;
            let view = MapViewOfFile(map, FILE_MAP_ALL_ACCESS, 0, 0, TOTAL);
            if view.Value.is_null() {
                let _ = CloseHandle(map);
                return Err("couldn't map RTSS shared memory".into());
            }
            self.map = map;
            self.view = view.Value as *mut u8;
            let w32 = |off: usize, v: u32| std::ptr::write_unaligned(self.view.add(off) as *mut u32, v);
            w32(4, VERSION);
            w32(8, APP_ENTRY_SIZE as u32);
            w32(12, APP_OFFSET as u32);
            w32(16, APP_COUNT as u32);
            w32(20, OSD_ENTRY_SIZE as u32);
            w32(24, OSD_OFFSET as u32);
            w32(28, OSD_COUNT as u32);
            // Slot 0 belongs to MSI Afterburner by convention; mark it so clients skip it.
            write_cstr(self.view.add(OSD_OFFSET + O_OWNER), 256, "Delta");
            std::sync::atomic::fence(Ordering::SeqCst);
            w32(0, SIGNATURE); // last: clients check it first
        }
        Ok(())
    }

    fn close(&mut self) {
        if !self.view.is_null() {
            unsafe {
                std::ptr::write_volatile(self.view as *mut u32, 0xDEAD);
                let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.view as *mut _ });
                let _ = CloseHandle(self.map);
            }
            self.view = std::ptr::null_mut();
            self.map = HANDLE::default();
            self.slots.clear();
            self.last_frame = None;
        }
    }

    pub fn serving(&self) -> bool {
        !self.view.is_null()
    }

    fn busy(&self) -> &AtomicI32 {
        unsafe { &*(self.view.add(H_BUSY) as *const AtomicI32) }
    }

    /// Text that other apps put in their OSD slots (slot 0 is ours/Afterburner's and skipped).
    /// Returns (slots, changed since last call).
    pub fn slots(&mut self) -> (&[Slot], bool) {
        if self.view.is_null() {
            return (&[], false);
        }
        let frame = unsafe { std::ptr::read_volatile(self.view.add(H_OSD_FRAME) as *const u32) };
        let mut fresh = Vec::new();
        // Copy under RTSS's busy lock (bit 0), as RTSS's renderer does. If a writer holds it, keep
        // last frame's text.
        let busy = self.busy();
        let mut spins = 0;
        loop {
            let old = busy.load(Ordering::Acquire);
            if old & 1 == 0 && busy.compare_exchange(old, old | 1, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                break;
            }
            spins += 1;
            if spins > 200 {
                return (&self.slots, false);
            }
            std::hint::spin_loop();
        }
        for i in 1..OSD_COUNT {
            let slot = unsafe { self.view.add(OSD_OFFSET + i * OSD_ENTRY_SIZE) };
            let owner = unsafe { read_cstr(slot.add(O_OWNER), 256) };
            if owner.is_empty() {
                continue;
            }
            let mut text = unsafe { read_cstr(slot.add(O_TEXT_EX), 4096) };
            if text.is_empty() {
                text = unsafe { read_cstr(slot.add(O_TEXT), 256) };
            }
            if !text.is_empty() {
                fresh.push(Slot { owner, text });
            }
        }
        busy.fetch_and(!1, Ordering::Release);
        let changed = fresh != self.slots || self.last_frame != Some(frame);
        self.slots = fresh;
        self.last_frame = Some(frame);
        (&self.slots, changed)
    }

    /// Publishes the apps Delta is drawing over, with their frame rate, the way RTSS does.
    pub fn publish_apps(&mut self, apps: &[AppInfo]) {
        if self.view.is_null() {
            return;
        }
        let now = unsafe { GetTickCount() };
        unsafe {
            for i in 0..APP_COUNT {
                let e = self.view.add(APP_OFFSET + i * APP_ENTRY_SIZE);
                let pid = std::ptr::read_unaligned(e.add(A_PID) as *const u32);
                let Some(app) = apps.get(i) else {
                    if pid != 0 {
                        std::ptr::write_bytes(e, 0, 1024);
                        std::ptr::write_unaligned(e.add(A_RES_X) as *mut u32, 0);
                        std::ptr::write_unaligned(e.add(A_RES_Y) as *mut u32, 0);
                    }
                    continue;
                };
                let w = |off: usize, v: u32| std::ptr::write_unaligned(e.add(off) as *mut u32, v);
                if pid != app.pid {
                    std::ptr::write_bytes(e, 0, 1024);
                    write_cstr(e.add(A_NAME), 260, &app.name);
                }
                w(A_FLAGS, APPFLAG_D3D11);
                w(A_TIME0, now.wrapping_sub(1000));
                w(A_TIME1, now);
                w(A_FRAMES, app.frames_1s);
                w(A_FRAMETIME, (app.frametime_ms * 1000.0) as u32);
                w(A_OSD_PIXEL, 1);
                w(A_RES_X, app.width);
                w(A_RES_Y, app.height);
                w(A_PID, app.pid); // last, so a reader never sees a half-written entry for this pid
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.close();
    }
}

unsafe fn read_cstr(p: *const u8, max: usize) -> String {
    let bytes = unsafe { std::slice::from_raw_parts(p, max) };
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(max);
    // RTSS text is ANSI (the system code page). Latin-1 is a close enough decode for tags and
    // ASCII; non-ASCII glyphs are rare in OSD text.
    bytes[..end].iter().map(|&b| b as char).collect()
}

unsafe fn write_cstr(p: *mut u8, max: usize, s: &str) {
    let b: Vec<u8> = s.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).take(max - 1).collect();
    unsafe {
        std::ptr::copy_nonoverlapping(b.as_ptr(), p, b.len());
        *p.add(b.len()) = 0;
    }
}

fn rtss_process_running() -> bool {
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return false };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut found = false;
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                if String::from_utf16_lossy(&e.szExeFile[..n]).eq_ignore_ascii_case("RTSS.exe") {
                    found = true;
                    break;
                }
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
        found
    }
}

// ---------------------------------------------------------------- client role
//
// While the real RTSS runs, Delta is one of its OSD clients, exactly like OBS HUD or HWiNFO: it
// claims a text slot, writes into it under RTSS's busy lock, and frees it when done. Offsets come
// from RTSS's header (its layout can be newer than ours).

const OWNER: &str = "Delta";
const MIN_CLIENT_VERSION: u32 = 0x0002_000E; // 2.14: text tags and the busy lock

fn with_rtss<T>(fallback: T, f: impl FnOnce(*mut u8) -> T) -> T {
    unsafe {
        let Ok(map) = OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, w!("RTSSSharedMemoryV2")) else { return fallback };
        let view = MapViewOfFile(map, FILE_MAP_ALL_ACCESS, 0, 0, 0);
        let mem = view.Value as *mut u8;
        let r = if mem.is_null() {
            fallback
        } else {
            let sig = std::ptr::read_unaligned(mem as *const u32);
            let ver = std::ptr::read_unaligned(mem.add(4) as *const u32);
            if sig == SIGNATURE && ver >> 16 == 2 && ver >= MIN_CLIENT_VERSION { f(mem) } else { fallback }
        };
        if !mem.is_null() {
            let _ = UnmapViewOfFile(view);
        }
        let _ = CloseHandle(map);
        r
    }
}

unsafe fn rd(mem: *const u8, off: usize) -> u32 {
    unsafe { std::ptr::read_unaligned(mem.add(off) as *const u32) }
}

/// What RTSS knows about a game it draws in.
#[derive(Clone, Copy, Debug)]
pub struct ClientApp {
    pub width: u32,
    pub height: u32,
    pub zoom: u32,
    pub fps: Option<f64>,
}

/// RTSS's entry for `pid`, if RTSS is drawing in it (it has seen a frame).
pub fn client_app(pid: u32) -> Option<ClientApp> {
    with_rtss(None, |mem| unsafe {
        if rd(mem, 4) < VERSION {
            return None; // resolution fields need 2.20
        }
        let (size, off, count) = (rd(mem, 8) as usize, rd(mem, 12) as usize, rd(mem, 16) as usize);
        for i in 0..count.min(1024) {
            let app = mem.add(off + i * size);
            if rd(app, A_PID) != pid {
                continue;
            }
            let (w, h) = (rd(app, A_RES_X), rd(app, A_RES_Y));
            if w == 0 || h == 0 {
                return None;
            }
            let (t0, t1, frames) = (rd(app, A_TIME0), rd(app, A_TIME1), rd(app, A_FRAMES));
            let fps = (t1 > t0 && frames > 0).then(|| frames as f64 * 1000.0 / (t1 - t0) as f64);
            return Some(ClientApp { width: w, height: h, zoom: rd(app, A_OSD_PIXEL).max(1), fps });
        }
        None
    })
}

/// Our slot, or with `claim` the first free one. Slot 0 is MSI Afterburner's, as the SDK asks.
unsafe fn client_slot(mem: *mut u8, claim: bool) -> Option<*mut u8> {
    unsafe {
        let (size, off, count) = (rd(mem, 20) as usize, rd(mem, 24) as usize, rd(mem, 28) as usize);
        let mut free = None;
        for i in 1..count.min(64) {
            let slot = mem.add(off + i * size);
            let owner = read_cstr(slot.add(O_OWNER), 256);
            if owner == OWNER {
                return Some(slot);
            }
            if owner.is_empty() && free.is_none() {
                free = Some(slot);
            }
        }
        let slot = free.filter(|_| claim)?;
        write_cstr(slot.add(O_OWNER), 256, OWNER);
        Some(slot)
    }
}

fn redraw(mem: *mut u8) {
    unsafe {
        let f = &*(mem.add(H_OSD_FRAME) as *const std::sync::atomic::AtomicU32);
        f.fetch_add(1, Ordering::AcqRel);
    }
}

/// Shows `text` plus embedded objects (graphs) that `<OBJ=offset>` tags in the text point into.
pub fn client_show_with(text: &str, objects: &[u8]) -> bool {
    with_rtss(false, |mem| unsafe {
        let Some(slot) = client_slot(mem, true) else { return false };
        let objects = &objects[..objects.len().min(O_BUFFER_SIZE)];
        let same_text = read_cstr(slot.add(O_TEXT_EX), 4096) == text;
        let same_objects = std::slice::from_raw_parts(slot.add(O_BUFFER), objects.len()) == objects;
        if same_text && same_objects {
            return true;
        }
        // RTSS's renderer holds bit 0 of the busy field only while copying.
        let busy = &*(mem.add(H_BUSY) as *const AtomicI32);
        let mut spins = 0;
        loop {
            let old = busy.load(Ordering::Acquire);
            if old & 1 == 0 && busy.compare_exchange(old, old | 1, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                break;
            }
            spins += 1;
            if spins > 2000 {
                return false;
            }
            std::hint::spin_loop();
        }
        std::ptr::copy_nonoverlapping(objects.as_ptr(), slot.add(O_BUFFER), objects.len());
        write_cstr(slot.add(O_TEXT_EX), 4096, text);
        busy.fetch_and(!1, Ordering::Release);
        redraw(mem);
        true
    })
}

/// RTSS's embedded graph object (RTSS_EMBEDDED_OBJECT_GRAPH in the SDK).
#[allow(dead_code)]
pub mod graph {
    pub const FILLED: u32 = 1;
    pub const FRAMERATE: u32 = 2;
    pub const FRAMETIME: u32 = 4;
    pub const BAR: u32 = 8;
    pub const BGND: u32 = 16;
    pub const AUTOSCALE: u32 = 128;

    /// Appends a graph object to `buf` and returns its offset (for `<OBJ=XXXXXXXX>`).
    /// Positive width/height are pixels, negative are character cells. With FRAMERATE or
    /// FRAMETIME, RTSS plots its own per-frame measurements and `data` only sets the history length.
    #[allow(clippy::too_many_arguments)]
    pub fn push(buf: &mut Vec<u8>, width: i32, height: i32, margin: i32, min: f32, max: f32, flags: u32, data: &[f32]) -> usize {
        while buf.len() % 4 != 0 {
            buf.push(0);
        }
        let at = buf.len();
        let size = 32 + data.len() * 4;
        let sig = u32::from_le_bytes(*b"00RG"); // 'GR00' as a C multi-character constant
        for v in [sig, size as u32] {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        for v in [width, height, margin] {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf.extend_from_slice(&flags.to_le_bytes());
        buf.extend_from_slice(&min.to_le_bytes());
        buf.extend_from_slice(&max.to_le_bytes());
        buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
        for d in data {
            buf.extend_from_slice(&d.to_le_bytes());
        }
        at
    }
}

/// Frees our slot so RTSS stops drawing it.
pub fn client_clear() {
    with_rtss((), |mem| unsafe {
        if let Some(slot) = client_slot(mem, false) {
            std::ptr::write_bytes(slot, 0, O_TEXT_EX + 4096); // text, owner and extended text
            redraw(mem);
        }
    });
}

/// Where RTSS is installed, if it is.
pub fn installed() -> Option<std::path::PathBuf> {
    let mut dirs = vec![];
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Ok(p) = std::env::var(var) {
            dirs.push(std::path::PathBuf::from(p).join("RivaTuner Statistics Server").join("RTSS.exe"));
        }
    }
    dirs.into_iter().find(|p| p.exists())
}

#[cfg(test)]
mod tests {
    /// Writes DELTA_TEST_OSD into a slot, with a frame time graph object at offset 0, for 8 s.
    /// `{G}` in the text becomes the graph's <OBJ> tag. Needs RTSS running.
    #[test]
    #[ignore]
    fn osd_experiment() {
        let text = std::env::var("DELTA_TEST_OSD").unwrap();
        let mut buf = vec![];
        let w: i32 = std::env::var("DELTA_TEST_GW").ok().and_then(|v| v.parse().ok()).unwrap_or(200);
        let h: i32 = std::env::var("DELTA_TEST_GH").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
        let flags: u32 = std::env::var("DELTA_TEST_GF").ok().and_then(|v| v.parse().ok()).unwrap_or(super::graph::FRAMETIME);
        let n: usize = std::env::var("DELTA_TEST_GN").ok().and_then(|v| v.parse().ok()).unwrap_or(128);
        let sine = std::env::var("DELTA_TEST_SINE").is_ok();
        let data: Vec<f32> = (0..n).map(|i| if sine { 16.0 + 12.0 * (i as f32 * 0.2).sin() } else { 0.0 }).collect();
        let max: f32 = std::env::var("DELTA_TEST_GMAX").ok().and_then(|v| v.parse().ok()).unwrap_or(33.3);
        let at = super::graph::push(&mut buf, w, h, 0, 0.0, max, flags, &data);
        let text = text.replace("{G}", &format!("<OBJ={at:08X}>"));
        assert!(super::client_show_with(&text, &buf));
        std::thread::sleep(std::time::Duration::from_secs(8));
        super::client_clear();
    }

    /// Reads RTSS's entry for the process in DELTA_TEST_PID (needs RTSS running and drawing in it).
    #[test]
    #[ignore]
    fn reads_client_app() {
        let pid: u32 = std::env::var("DELTA_TEST_PID").unwrap().parse().unwrap();
        let app = super::client_app(pid).expect("RTSS has no entry for that process");
        println!("RTSS sees {pid}: {app:?}");
        assert!(app.width > 0 && app.height > 0 && app.zoom >= 1);
    }
}
