//! The settings window: native, drawn with the same Direct2D stack as the overlay. It only exists
//! while open; closing it frees everything.

pub mod kit;
pub mod pages;

use crate::compose::{self, Frame};
use crate::config::Config;
use crate::overlay::draw::{Images, Painter};
use crate::overlay::gfx::{Fonts, Gfx};
use crate::overlay::layout::Sticky;
use crate::theme::Theme;
use kit::{Input, Key, Rect, State, Texts, Ui};
use pages::{Action, Ctx, PageState, Previewer, Status, ThemeInfo};
use std::collections::HashMap;
use std::rc::Rc;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::ID2D1DeviceContext;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Dwm::{DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Result, w};
use windows_numerics::Matrix3x2;

pub const CLASS: windows::core::PCWSTR = w!("DeltaSettings");

/// Live previews of every theme in the gallery, using the overlay's own renderer.
struct Previews {
    gfx: Rc<Gfx>,
    fonts: HashMap<String, Fonts>,
    sticky: HashMap<String, Sticky>,
    images: Images,
}

const PREVIEW_SCALE: f32 = 0.82;

impl Previewer for Previews {
    fn preview(&mut self, ui: &mut Ui, cfg: &Config, theme: &Theme, frame: &Frame, area: Rect) {
        let key = theme.name.clone();
        if !self.fonts.contains_key(&key) {
            match Fonts::new(&self.gfx, theme, PREVIEW_SCALE) {
                Ok(f) => {
                    self.fonts.insert(key.clone(), f);
                }
                Err(_) => return,
            }
        }
        let fonts = &self.fonts[&key];
        let sticky = self.sticky.entry(key).or_default();
        let layout = cfg.layout.unwrap_or(theme.layout);
        let Some(scene) = compose::panel(cfg, theme, layout, PREVIEW_SCALE, frame, fonts, sticky) else { return };
        // Shrink to fit the card if needed, and center it. Very wide (one-line) layouts would get
        // unreadably small, so those stay readable and run off the right edge instead.
        let mut fit = ((area.w - 20.0) / scene.w).min((area.h - 20.0) / scene.h).min(1.0);
        let wide = fit < 0.6 && scene.w > scene.h * 4.0;
        if wide {
            fit = ((area.h - 20.0) / scene.h).min(0.8);
        }
        let (sw, sh) = (scene.w * fit, scene.h * fit);
        let x = if wide { area.x + 10.0 } else { area.x + (area.w - sw) / 2.0 };
        let y = area.y + (area.h - sh) / 2.0;
        unsafe {
            let mut current = Matrix3x2::default();
            ui.ctx.GetTransform(&mut current);
            let outer = Matrix3x2::scale(fit, fit) * Matrix3x2::translation(x, y) * current;
            let mut p = Painter {
                clear: false,
                gfx: &self.gfx,
                ctx: ui.ctx,
                theme,
                scale: PREVIEW_SCALE,
                opacity: cfg.opacity,
                bg: cfg.background_opacity,
                fg: cfg.text_opacity,
                images: &mut self.images,
            };
            let _ = p.paint(&scene, (0.0, 0.0), outer);
            ui.ctx.SetTransform(&current);
        }
    }
}

pub struct Settings {
    pub hwnd: HWND,
    gfx: Rc<Gfx>,
    _target: IDCompositionTarget,
    visual: IDCompositionVisual2,
    surface: Option<IDCompositionSurface>,
    size: (u32, u32),
    texts: Texts,
    st: State,
    input: Input,
    ps: PageState,
    previews: Previews,
    /// The working copy the pages edit.
    pub cfg: Config,
    dpi: f32,
    pub needs_redraw: bool,
}

/// What the app gives the settings window to draw with.
pub struct Data<'a> {
    pub status: &'a Status,
    pub themes: &'a [ThemeInfo],
    pub frame: &'a Frame,
    pub apps: &'a [String],
}

impl Drop for Settings {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn lo(l: LPARAM) -> i16 {
    (l.0 & 0xFFFF) as i16
}
fn hi(l: LPARAM) -> i16 {
    ((l.0 >> 16) & 0xFFFF) as i16
}

impl Settings {
    pub fn open(instance: HINSTANCE, wndproc: WNDPROC, gfx: Rc<Gfx>, cfg: Config, page: usize) -> Result<Settings> {
        unsafe {
            let class = WNDCLASSW {
                lpfnWndProc: wndproc,
                hInstance: instance,
                lpszClassName: CLASS,
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hIcon: crate::tray::app_icon(),
                ..Default::default()
            };
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP,
                CLASS,
                w!("Delta settings"),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                100,
                100,
                None,
                None,
                Some(instance),
                None,
            )?;
            // Size for the window's DPI, centered on its monitor.
            let dpi = GetDpiForWindow(hwnd) as f32 / 96.0;
            let (w, h) = ((1080.0 * dpi) as i32, (760.0 * dpi) as i32);
            let mon = crate::overlay::foreground_monitor().rect;
            let x = mon.left + ((mon.right - mon.left) - w) / 2;
            let y = mon.top + ((mon.bottom - mon.top) - h) / 2;
            let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER);
            // Dark title bar in the window's own color.
            let dark: i32 = 1;
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, 4);
            let bg = kit::pal::SIDEBAR.0;
            let cref = COLORREF((bg[0] * 255.0) as u32 | (((bg[1] * 255.0) as u32) << 8) | (((bg[2] * 255.0) as u32) << 16));
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR, &cref as *const _ as _, 4);
            let border = COLORREF(0x2A2522);
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &border as *const _ as _, 4);

            let target = gfx.dcomp.CreateTargetForHwnd(hwnd, false)?;
            let visual = gfx.dcomp.CreateVisual()?;
            target.SetRoot(&visual)?;
            let texts = Texts::new(&gfx);
            let s = Settings {
                hwnd,
                previews: Previews { gfx: gfx.clone(), fonts: HashMap::new(), sticky: HashMap::new(), images: Images::default() },
                gfx,
                _target: target,
                visual,
                surface: None,
                size: (0, 0),
                texts,
                st: State::default(),
                input: Input::default(),
                ps: PageState { page, ..Default::default() },
                cfg,
                dpi,
                needs_redraw: true,
            };
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            Ok(s)
        }
    }

    pub fn animating(&self) -> bool {
        self.st.animating
    }

    /// The theme fonts used by previews must be rebuilt when a theme file changes.
    pub fn themes_changed(&mut self) {
        self.previews.fonts.clear();
        self.previews.sticky.clear();
        self.needs_redraw = true;
    }

    /// Feeds window messages in. Returns Some to skip the default handling.
    pub fn message(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        let d = self.dpi;
        let mods = || unsafe {
            Key {
                vk: 0,
                ctrl: GetKeyState(VK_CONTROL.0 as i32) < 0,
                alt: GetKeyState(VK_MENU.0 as i32) < 0,
                shift: GetKeyState(VK_SHIFT.0 as i32) < 0,
                win: GetKeyState(VK_LWIN.0 as i32) < 0 || GetKeyState(VK_RWIN.0 as i32) < 0,
            }
        };
        match msg {
            WM_MOUSEMOVE => {
                self.input.mouse = (lo(lp) as f32 / d, hi(lp) as f32 / d);
                unsafe {
                    let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.hwnd, dwHoverTime: 0 };
                    let _ = TrackMouseEvent(&mut tme);
                }
            }
            0x02A3 /* WM_MOUSELEAVE */ => self.input.mouse = (-1000.0, -1000.0),
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                self.input.mouse = (lo(lp) as f32 / d, hi(lp) as f32 / d);
                self.input.down = true;
                self.input.pressed = true;
                unsafe { SetCapture(self.hwnd) };
            }
            WM_LBUTTONUP => {
                self.input.mouse = (lo(lp) as f32 / d, hi(lp) as f32 / d);
                self.input.down = false;
                self.input.released = true;
                unsafe {
                    let _ = ReleaseCapture();
                }
            }
            WM_MOUSEWHEEL => self.input.wheel += hi(LPARAM(wp.0 as isize)) as f32,
            WM_CHAR => {
                if let Some(c) = char::from_u32(wp.0 as u32) {
                    self.input.chars.push(c);
                }
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let vk = wp.0 as u32;
                let is_mod = [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN, VK_LCONTROL, VK_RCONTROL, VK_LSHIFT, VK_RSHIFT, VK_LMENU, VK_RMENU].iter().any(|k| k.0 as u32 == vk);
                if !is_mod {
                    self.input.keys.push(Key { vk, ..mods() });
                }
                // While recording a hotkey, keep Alt+key from reaching the system menu.
                if msg == WM_SYSKEYDOWN && self.st.focus != 0 {
                    self.needs_redraw = true;
                    return Some(LRESULT(0));
                }
            }
            WM_SETCURSOR => {
                if (lp.0 & 0xFFFF) as u32 == HTCLIENT {
                    unsafe {
                        let c = if self.st.hand { IDC_HAND } else { IDC_ARROW };
                        SetCursor(LoadCursorW(None, c).ok());
                    }
                    return Some(LRESULT(1));
                }
                return None;
            }
            WM_GETMINMAXINFO => unsafe {
                let mm = &mut *(lp.0 as *mut MINMAXINFO);
                mm.ptMinTrackSize = POINT { x: (860.0 * d) as i32, y: (560.0 * d) as i32 };
                return Some(LRESULT(0));
            },
            WM_DPICHANGED => unsafe {
                self.dpi = (wp.0 & 0xFFFF) as f32 / 96.0;
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(self.hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            },
            WM_SIZE => {}
            WM_ACTIVATE => {}
            _ => return None,
        }
        self.needs_redraw = true;
        None
    }

    /// Draws a frame. Returns what the user asked for, and whether the config changed.
    pub fn render(&mut self, data: &Data) -> (Vec<Action>, bool) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        let (w, h) = ((rc.right - rc.left).max(1) as u32, (rc.bottom - rc.top).max(1) as u32);
        let mut out = (vec![], false);
        let r = (|| -> Result<()> {
            unsafe {
                if self.surface.is_none() || self.size != (w, h) {
                    let surf = self.gfx.dcomp.CreateSurface(w, h, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_ALPHA_MODE_PREMULTIPLIED)?;
                    self.visual.SetContent(&surf)?;
                    self.surface = Some(surf);
                    self.size = (w, h);
                }
                let surf = self.surface.as_ref().unwrap();
                let mut offset = POINT::default();
                let ctx: ID2D1DeviceContext = surf.BeginDraw(None, &mut offset)?;
                let d = self.dpi;
                let base = Matrix3x2::scale(d, d) * Matrix3x2::translation(offset.x as f32, offset.y as f32);
                ctx.SetTransform(&base);
                ctx.SetTextAntialiasMode(windows::Win32::Graphics::Direct2D::D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
                let size = (w as f32 / d, h as f32 / d);
                let mut ui = Ui::new(&ctx, &self.gfx, &mut self.texts, &self.input, &mut self.st, size);
                let mut cx = Ctx {
                    cfg: &mut self.cfg,
                    changed: false,
                    actions: vec![],
                    status: data.status,
                    themes: data.themes,
                    frame: data.frame,
                    apps: data.apps,
                    ps: &mut self.ps,
                    previewer: &mut self.previews,
                };
                pages::draw(&mut ui, &mut cx);
                ui.finish();
                out = (cx.actions, cx.changed);
                surf.EndDraw()?;
                self.gfx.dcomp.Commit()?;
            }
            Ok(())
        })();
        if let Err(e) = r {
            crate::log!("settings render failed: {e}");
            self.surface = None;
        }
        self.input.next_frame();
        self.needs_redraw = false;
        out
    }
}
