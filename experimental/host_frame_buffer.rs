//! Delta's side of in-game drawing: the shared frame buffer the hook reads, and an offscreen
//! renderer that turns the same scenes the window overlay shows into pixels.

use crate::overlay::draw::{Images, Painter};
use crate::overlay::gfx::Gfx;
use crate::overlay::layout::Scene;
use crate::overlay::Look;
use delta_shared::{HookSlot, Header, MAGIC, MAX_SPRITES, PIXEL_BYTES, PIXELS_OFFSET, Sprite, TOTAL, VERSION};
use std::sync::atomic::Ordering;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Memory::{CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile};
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::core::{HSTRING, Result, w};
use windows_numerics::Matrix3x2;

pub struct FrameShm {
    map: HANDLE,
    view: *mut u8,
    /// (size, pixel hash, version) of what's in each sprite slot now.
    last: Vec<(u32, u32, u64, u32)>,
    next_version: u32,
}

/// What a hook in a game reports.
#[derive(Clone, Copy, Debug)]
pub struct HookInfo {
    pub api: u32,
    pub width: u32,
    pub height: u32,
    pub frames: u64,
    pub fresh: bool,
}

impl FrameShm {
    pub fn create() -> std::result::Result<FrameShm, String> {
        unsafe {
            // Same reasoning as the RTSS memory: games (not elevated) must be able to open it even
            // when Delta runs as administrator.
            let mut sd = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(w!("D:(A;;GA;;;WD)(A;;GA;;;AC)S:(ML;;NW;;;LW)"), SDDL_REVISION_1, &mut sd, None).map_err(|e| e.to_string())?;
            let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd.0, bInheritHandle: false.into() };
            let map = CreateFileMappingW(INVALID_HANDLE_VALUE, Some(&sa), PAGE_READWRITE, 0, TOTAL as u32, &HSTRING::from(delta_shared::NAME));
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            let map = map.map_err(|e| e.to_string())?;
            let v = MapViewOfFile(map, FILE_MAP_ALL_ACCESS, 0, 0, TOTAL);
            if v.Value.is_null() {
                let _ = CloseHandle(map);
                return Err("couldn't map the in-game frame buffer".into());
            }
            let s = FrameShm { map, view: v.Value as *mut u8, last: vec![(0, 0, 0, 0); MAX_SPRITES], next_version: 1 };
            let h = s.header();
            h.version.store(VERSION, Ordering::Relaxed);
            h.host_pid.store(std::process::id(), Ordering::Relaxed);
            h.target_pid.store(0, Ordering::Relaxed);
            h.magic.store(MAGIC, Ordering::Release);
            Ok(s)
        }
    }

    fn header(&self) -> &Header {
        unsafe { &*(self.view as *const Header) }
    }

    pub fn heartbeat(&self) {
        self.header().host_heartbeat.store(unsafe { GetTickCount64() }, Ordering::Release);
    }

    pub fn set_target(&self, pid: u32) {
        self.header().target_pid.store(pid, Ordering::Release);
    }

    pub fn hook(&self, pid: u32) -> Option<HookInfo> {
        let now = unsafe { GetTickCount64() };
        self.header().hooks.iter().find(|s: &&HookSlot| s.pid.load(Ordering::Acquire) == pid).map(|s| HookInfo {
            api: s.api.load(Ordering::Relaxed),
            width: s.width.load(Ordering::Relaxed),
            height: s.height.load(Ordering::Relaxed),
            frames: s.frames.load(Ordering::Relaxed),
            fresh: now.saturating_sub(s.last_present.load(Ordering::Acquire)) < 1000,
        })
    }

    /// Processes with a hook that presented recently.
    pub fn live_hooks(&self) -> Vec<u32> {
        let now = unsafe { GetTickCount64() };
        self.header()
            .hooks
            .iter()
            .filter(|s| s.pid.load(Ordering::Acquire) != 0 && now.saturating_sub(s.last_present.load(Ordering::Acquire)) < 1000)
            .map(|s| s.pid.load(Ordering::Relaxed))
            .collect()
    }

    /// Replaces the sprites (x, y, w, h, premultiplied BGRA pixels).
    pub fn publish(&mut self, sprites: &[(i32, i32, u32, u32, Vec<u8>)]) {
        let h = unsafe { &mut *(self.view as *mut Header) };
        h.seq.fetch_add(1, Ordering::AcqRel); // odd: writing
        let mut offset = 0usize;
        let mut n = 0;
        for (i, (x, y, w, hh, px)) in sprites.iter().enumerate().take(MAX_SPRITES) {
            let bytes = px.len();
            if offset + bytes > PIXEL_BYTES {
                break;
            }
            let hash = fx_hash(px);
            let (lw, lh, lhash, lver) = self.last[i];
            // Same pixels as last time keep their version, so the hook doesn't upload them again.
            let version = if lw == *w && lh == *hh && lhash == hash && lver != 0 {
                lver
            } else {
                let v = self.next_version;
                self.next_version = self.next_version.wrapping_add(1).max(1);
                v
            };
            unsafe { std::ptr::copy_nonoverlapping(px.as_ptr(), self.view.add(PIXELS_OFFSET + offset), bytes) };
            self.last[i] = (*w, *hh, hash, version);
            h.sprites[i] = Sprite { x: *x, y: *y, w: *w, h: *hh, offset: offset as u32, version };
            offset += bytes.next_multiple_of(64);
            n += 1;
        }
        h.sprite_count.store(n as u32, Ordering::Release);
        h.seq.fetch_add(1, Ordering::AcqRel); // even: done
    }
}

impl Drop for FrameShm {
    fn drop(&mut self) {
        unsafe {
            self.header().target_pid.store(0, Ordering::Release);
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.view as *mut _ });
            let _ = CloseHandle(self.map);
        }
    }
}

fn fx_hash(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for c in b.chunks(8) {
        let mut v = [0u8; 8];
        v[..c.len()].copy_from_slice(c);
        h = (h.rotate_left(5) ^ u64::from_le_bytes(v)).wrapping_mul(0x517cc1b727220a95);
    }
    h
}

/// Renders scenes into CPU-readable pixels.
pub struct Offscreen {
    ctx: ID2D1DeviceContext,
    target: Option<((u32, u32), ID2D1Bitmap1, ID2D1Bitmap1)>,
    images: Images,
}

impl Offscreen {
    pub fn new(gfx: &Gfx) -> Result<Offscreen> {
        let ctx = unsafe { gfx.d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)? };
        Ok(Offscreen { ctx, target: None, images: Images::default() })
    }

    pub fn render(&mut self, gfx: &Gfx, scene: &Scene, look: &Look) -> Result<(u32, u32, Vec<u8>)> {
        let (w, h) = (scene.w.max(1.0).ceil() as u32, scene.h.max(1.0).ceil() as u32);
        unsafe {
            if self.target.as_ref().is_none_or(|(s, _, _)| *s != (w, h)) {
                let pf = D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED };
                let props = |opts| D2D1_BITMAP_PROPERTIES1 { pixelFormat: pf, dpiX: 96.0, dpiY: 96.0, bitmapOptions: opts, ..Default::default() };
                let size = D2D_SIZE_U { width: w, height: h };
                let target = self.ctx.CreateBitmap(size, None, 0, &props(D2D1_BITMAP_OPTIONS_TARGET))?;
                let read = self.ctx.CreateBitmap(size, None, 0, &props(D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW))?;
                self.target = Some(((w, h), target, read));
            }
            let (_, target, read) = self.target.as_ref().unwrap();
            self.ctx.SetTarget(target);
            self.ctx.BeginDraw();
            let mut p = Painter { clear: true, gfx, ctx: &self.ctx, theme: look.theme, scale: look.scale, opacity: look.opacity, bg: look.bg, fg: look.fg, images: &mut self.images };
            let r = p.paint(scene, (0.0, 0.0), Matrix3x2::identity());
            self.ctx.EndDraw(None, None)?;
            r?;
            read.CopyFromBitmap(None, target, None)?;
            let m = read.Map(D2D1_MAP_OPTIONS_READ)?;
            let row = (w * 4) as usize;
            let mut out = vec![0u8; row * h as usize];
            for y in 0..h as usize {
                std::ptr::copy_nonoverlapping(m.bits.add(y * m.pitch as usize), out.as_mut_ptr().add(y * row), row);
            }
            read.Unmap()?;
            self.images.sweep();
            Ok((w, h, out))
        }
    }
}
