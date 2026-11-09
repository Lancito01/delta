//! Notification-area icon, its menu and balloon messages.

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR};

pub const CALLBACK: u32 = WM_APP + 1;

pub struct Tray {
    hwnd: HWND,
    icon: HICON,
}

fn copy_wide(dst: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..w.len()].copy_from_slice(&w);
    dst[w.len()] = 0;
}

impl Tray {
    pub fn new(hwnd: HWND) -> Tray {
        let t = Tray { hwnd, icon: app_icon() };
        t.add();
        t
    }

    fn data(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            ..Default::default()
        }
    }

    /// (Re)adds the icon, e.g. after Explorer restarts.
    pub fn add(&self) {
        let mut d = self.data();
        d.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        d.uCallbackMessage = CALLBACK;
        d.hIcon = self.icon;
        copy_wide(&mut d.szTip, "Delta overlay");
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &d);
            d.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &d);
        }
    }

    pub fn tooltip(&self, text: &str) {
        let mut d = self.data();
        d.uFlags = NIF_TIP;
        copy_wide(&mut d.szTip, text);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
        }
    }

    pub fn notify(&self, title: &str, text: &str) {
        let mut d = self.data();
        d.uFlags = NIF_INFO;
        d.dwInfoFlags = NIIF_INFO | NIIF_NOSOUND;
        copy_wide(&mut d.szInfoTitle, title);
        copy_wide(&mut d.szInfo, text);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
        }
    }

    pub fn remove(&self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.data());
        }
    }
}

pub enum Entry {
    Item { id: u32, text: String, checked: bool, enabled: bool },
    Sub { text: String, items: Vec<Entry> },
    Separator,
}

fn build(entries: &[Entry]) -> HMENU {
    unsafe {
        let m = CreatePopupMenu().unwrap_or_default();
        for e in entries {
            match e {
                Entry::Item { id, text, checked, enabled } => {
                    let mut flags = MF_STRING;
                    if *checked {
                        flags |= MF_CHECKED;
                    }
                    if !*enabled {
                        flags |= MF_GRAYED;
                    }
                    let _ = AppendMenuW(m, flags, *id as usize, &HSTRING::from(text.as_str()));
                }
                Entry::Sub { text, items } => {
                    let sub = build(items);
                    let _ = AppendMenuW(m, MF_POPUP, sub.0 as usize, &HSTRING::from(text.as_str()));
                }
                Entry::Separator => {
                    let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
                }
            }
        }
        m
    }
}

/// Shows the menu at the cursor and returns the chosen id (0 = dismissed).
pub fn popup(hwnd: HWND, entries: &[Entry]) -> u32 {
    unsafe {
        let m = build(entries);
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        // Needed so the menu closes when clicking elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let id = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, p.x, p.y, None, hwnd, None).0 as u32;
        let _ = DestroyMenu(m);
        id
    }
}

/// The app icon (made once).
pub fn app_icon() -> HICON {
    static ICON: std::sync::OnceLock<isize> = std::sync::OnceLock::new();
    HICON(*ICON.get_or_init(|| make_icon(32).0 as isize) as *mut _)
}

/// Draws the Δ logo: an anti-aliased triangle outline, generated so there is no binary asset.
fn make_icon(size: i32) -> HICON {
    let n = size as usize;
    let mut px = vec![0u32; n * n];
    let outer = [(0.5f32, 0.06f32), (0.97, 0.92), (0.03, 0.92)];
    let inner = [(0.5f32, 0.42f32), (0.74, 0.80), (0.26, 0.80)];
    let inside = |t: &[(f32, f32); 3], x: f32, y: f32| {
        let s = |a: (f32, f32), b: (f32, f32)| (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0);
        let (d1, d2, d3) = (s(t[0], t[1]), s(t[1], t[2]), s(t[2], t[0]));
        !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
    };
    let (r, g, b) = (0x89u32, 0xB4u32, 0xFAu32);
    for y in 0..n {
        for x in 0..n {
            let mut hits = 0;
            for sy in 0..4 {
                for sx in 0..4 {
                    let fx = (x as f32 + (sx as f32 + 0.5) / 4.0) / size as f32;
                    let fy = (y as f32 + (sy as f32 + 0.5) / 4.0) / size as f32;
                    if inside(&outer, fx, fy) && !inside(&inner, fx, fy) {
                        hits += 1;
                    }
                }
            }
            let a = hits * 255 / 16;
            // premultiplied BGRA
            px[y * n + x] = (a << 24) | ((r * a / 255) << 16) | ((g * a / 255) << 8) | (b * a / 255);
        }
    }
    unsafe {
        let color = CreateBitmap(size, size, 1, 32, Some(px.as_ptr() as *const _));
        let mask_bits = vec![0u8; n * n / 8];
        let mask = CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr() as *const _));
        let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}
