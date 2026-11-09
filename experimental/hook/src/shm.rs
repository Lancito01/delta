//! The hook's view of Delta's shared memory.

use delta_shared::{Header, HookSlot, MAGIC, MAX_HOOKS, NAME, Sprite, TOTAL};
use std::sync::OnceLock;
use std::sync::atomic::Ordering;
use windows::Win32::System::Memory::{FILE_MAP_READ, FILE_MAP_WRITE, MapViewOfFile, OpenFileMappingW};
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::core::HSTRING;

struct View(*mut u8);
unsafe impl Send for View {}
unsafe impl Sync for View {}

static VIEW: OnceLock<View> = OnceLock::new();

/// Maps the shared memory (stays mapped for the life of the process).
pub fn open() {
    unsafe {
        let Ok(h) = OpenFileMappingW((FILE_MAP_READ | FILE_MAP_WRITE).0, false, &HSTRING::from(NAME)) else {
            crate::log("Delta's shared memory isn't there");
            return;
        };
        let v = MapViewOfFile(h, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, TOTAL);
        if !v.Value.is_null() {
            let _ = VIEW.set(View(v.Value as *mut u8));
        }
    }
}

pub fn header() -> Option<&'static Header> {
    let h = unsafe { &*(VIEW.get()?.0 as *const Header) };
    (h.magic.load(Ordering::Acquire) == MAGIC).then_some(h)
}

pub fn pixels() -> Option<*const u8> {
    VIEW.get().map(|v| unsafe { v.0.add(delta_shared::PIXELS_OFFSET) as *const u8 })
}

/// This process's slot, claimed on first use.
pub fn slot() -> Option<&'static HookSlot> {
    let h = header()?;
    let pid = std::process::id();
    if let Some(s) = h.hooks.iter().find(|s| s.pid.load(Ordering::Relaxed) == pid) {
        return Some(s);
    }
    let now = unsafe { GetTickCount64() };
    for s in h.hooks.iter().take(MAX_HOOKS) {
        let old = s.pid.load(Ordering::Relaxed);
        // Free, or left behind by a game that stopped presenting a while ago.
        let stale = old != 0 && now.saturating_sub(s.last_present.load(Ordering::Relaxed)) > 30_000;
        if (old == 0 || stale) && s.pid.compare_exchange(old, pid, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
            s.frames.store(0, Ordering::Relaxed);
            return Some(s);
        }
    }
    None
}

/// Whether Delta wants this process drawn into right now.
pub fn active() -> bool {
    let Some(h) = header() else { return false };
    let now = unsafe { GetTickCount64() };
    h.target_pid.load(Ordering::Acquire) == std::process::id() && now.saturating_sub(h.host_heartbeat.load(Ordering::Acquire)) < delta_shared::STALE_MS
}

/// A consistent copy of the sprite list (None if Delta is mid-write).
pub fn sprites() -> Option<(u32, Vec<Sprite>)> {
    let h = header()?;
    let seq = h.seq.load(Ordering::Acquire);
    if seq & 1 == 1 {
        return None;
    }
    let n = (h.sprite_count.load(Ordering::Acquire) as usize).min(h.sprites.len());
    let list = h.sprites[..n].to_vec();
    std::sync::atomic::fence(Ordering::Acquire);
    (h.seq.load(Ordering::Acquire) == seq).then_some((seq, list))
}

pub fn seq_unchanged(seq: u32) -> bool {
    header().is_some_and(|h| h.seq.load(Ordering::Acquire) == seq)
}
