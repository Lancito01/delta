//! The on-screen overlay: per monitor, one click-through, topmost, never-activated window covering
//! it, with DirectComposition visuals for the stats panel and for positioned RTSS layers. Visuals are
//! only as big as their content, so the empty rest of the window costs nothing. All overlays share
//! one GPU device.

pub mod draw;
pub mod gfx;
pub mod layout;

use crate::theme::Theme;
use draw::{Images, Painter};
use gfx::{Fonts, Gfx};
use layout::{Scene, Sticky};
use std::rc::Rc;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::ID2D1DeviceContext;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
    MonitorFromWindow,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, Result, w};

/// How to paint: theme plus the user's size and opacity choices.
pub struct Look<'a> {
    pub theme: &'a Theme,
    pub scale: f32,
    pub opacity: f32,
    pub bg: f32,
    pub fg: f32,
}

struct Layer {
    visual: IDCompositionVisual2,
    surface: Option<IDCompositionSurface>,
    size: (u32, u32),
}

pub struct Overlay {
    pub hwnd: HWND,
    gfx: Rc<Gfx>,
    _target: IDCompositionTarget,
    root: IDCompositionVisual2,
    panel: Layer,
    osd: Vec<Layer>,
    fonts: Option<(String, u32, Fonts)>,
    pub sticky: Sticky,
    images: Images,
    pub monitor: RECT,
    shown: bool,
    capture_excluded: Option<bool>,
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(-1),    // HTTRANSPARENT
        WM_MOUSEACTIVATE => LRESULT(3), // MA_NOACTIVATE
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MonitorInfo {
    pub rect: RECT,
    pub dpi: f32,
}

fn info(hmon: HMONITOR) -> MonitorInfo {
    unsafe {
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(hmon, &mut mi);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        MonitorInfo { rect: mi.rcMonitor, dpi: dx as f32 / 96.0 }
    }
}

/// All monitors, in Windows' order (the order of "1", "2", ... in settings).
pub fn monitors() -> Vec<MonitorInfo> {
    let mut list: Vec<HMONITOR> = vec![];
    unsafe extern "system" fn collect(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        unsafe { (*(data.0 as *mut Vec<HMONITOR>)).push(m) };
        true.into()
    }
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut list as *mut _ as isize));
    }
    list.into_iter().map(info).collect()
}

/// The lowest visible always-on-top window of these apps (exe names) that is still in the
/// always-on-top band, if any are open.
pub fn lowest_topmost_of(apps: &[String]) -> Option<HWND> {
    if apps.is_empty() {
        return None;
    }
    struct Find<'a> {
        apps: &'a [String],
        found: Option<HWND>,
    }
    unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> BOOL {
        unsafe {
            let f = &mut *(data.0 as *mut Find);
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            if !IsWindowVisible(hwnd).as_bool() {
                return true.into();
            }
            // The first ordinary window ends the always-on-top band. A window can keep the
            // topmost flag while sitting lower than that (Hitodama has one); settling under it
            // would put the overlay below every normal app.
            if ex & WS_EX_TOPMOST.0 == 0 {
                return false.into();
            }
            {
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                if pid != std::process::id() {
                    if let Some(name) = crate::metrics::system::process_name(pid) {
                        if f.apps.iter().any(|a| a.trim().eq_ignore_ascii_case(&name)) {
                            f.found = Some(hwnd); // windows come top to bottom: keep the last
                        }
                    }
                }
            }
            true.into()
        }
    }
    let mut f = Find { apps, found: None };
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut f as *mut _ as isize));
    }
    f.found
}

/// Whether `overlay` is above `other` in the window stack.
pub fn is_above(overlay: HWND, other: HWND) -> bool {
    unsafe {
        let mut h = GetWindow(overlay, GW_HWNDNEXT).ok();
        let mut n = 0;
        while let Some(w) = h {
            if w == other {
                return true;
            }
            n += 1;
            if n > 2000 {
                break;
            }
            h = GetWindow(w, GW_HWNDNEXT).ok();
        }
        false
    }
}

pub fn primary_monitor() -> MonitorInfo {
    info(unsafe { MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY) })
}

/// The monitor the foreground window is on.
pub fn foreground_monitor() -> MonitorInfo {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() { primary_monitor() } else { info(MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST)) }
    }
}

impl Layer {
    fn new(gfx: &Gfx) -> Result<Layer> {
        Ok(Layer { visual: unsafe { gfx.dcomp.CreateVisual()? }, surface: None, size: (0, 0) })
    }
}

impl Overlay {
    pub fn new(instance: HINSTANCE, gfx: Rc<Gfx>) -> Result<Overlay> {
        unsafe {
            let class = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: instance, lpszClassName: w!("DeltaOverlay"), ..Default::default() };
            RegisterClassW(&class); // fails harmlessly after the first time
            let ex = WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED;
            let hwnd = CreateWindowExW(ex, w!("DeltaOverlay"), w!("Delta overlay"), WS_POPUP, 0, 0, 1, 1, None, None, Some(instance), None)?;
            // Layered + transparent = clicks go straight through to the game.
            SetLayeredWindowAttributes(hwnd, Default::default(), 255, LWA_ALPHA)?;
            let target = gfx.dcomp.CreateTargetForHwnd(hwnd, true)?;
            let root = gfx.dcomp.CreateVisual()?;
            target.SetRoot(&root)?;
            let panel = Layer::new(&gfx)?;
            root.AddVisual(&panel.visual, false, None)?;
            Ok(Overlay {
                hwnd,
                gfx,
                _target: target,
                root,
                panel,
                osd: vec![],
                fonts: None,
                sticky: Sticky::default(),
                images: Images::default(),
                monitor: RECT::default(),
                shown: false,
                capture_excluded: None,
            })
        }
    }

    /// Runs `f` with this theme's fonts at `scale` and the sticky column widths.
    pub fn with_fonts<R>(&mut self, theme: &Theme, scale: f32, f: impl FnOnce(&Fonts, &mut Sticky) -> R) -> Option<R> {
        let key = (theme.name.clone(), (scale * 1000.0) as u32);
        if self.fonts.as_ref().is_none_or(|(n, s, _)| *n != key.0 || *s != key.1) {
            let fonts = Fonts::new(&self.gfx, theme, scale).ok()?;
            self.fonts = Some((key.0, key.1, fonts));
            self.sticky.clear();
        }
        let fonts = &self.fonts.as_ref()?.2;
        Some(f(fonts, &mut self.sticky))
    }

    /// Forces fonts to be rebuilt (theme or spacing changed).
    pub fn reset_theme(&mut self) {
        self.fonts = None;
        self.sticky.clear();
        self.images.clear();
    }

    pub fn place(&mut self, monitor: RECT) {
        if monitor != self.monitor {
            self.monitor = monitor;
            unsafe {
                let _ = SetWindowPos(
                    self.hwnd,
                    None,
                    monitor.left,
                    monitor.top,
                    monitor.right - monitor.left,
                    monitor.bottom - monitor.top,
                    SWP_NOACTIVATE | SWP_NOZORDER,
                );
            }
        }
    }

    pub fn set_capture_excluded(&mut self, on: bool) {
        if self.capture_excluded != Some(on) {
            self.capture_excluded = Some(on);
            unsafe {
                let _ = SetWindowDisplayAffinity(self.hwnd, if on { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE });
            }
        }
    }

    pub fn show(&mut self, on: bool) {
        if on != self.shown {
            self.shown = on;
            unsafe {
                let _ = ShowWindow(self.hwnd, if on { SW_SHOWNOACTIVATE } else { SW_HIDE });
            }
        }
    }

    pub fn shown(&self) -> bool {
        self.shown
    }

    /// Re-asserts topmost (other topmost windows, e.g. a game's, can end up above us).
    /// With `below`, settles just under that window instead (it's also always on top, and the
    /// user wants it above the overlay): no fighting over the top spot.
    pub fn raise(&self, below: Option<HWND>) {
        unsafe {
            let after = below.unwrap_or(HWND_TOPMOST);
            let _ = SetWindowPos(self.hwnd, Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    fn render_into(gfx: &Gfx, layer: &mut Layer, scene: &Scene, pos: (f32, f32), look: &Look, images: &mut Images) -> Result<()> {
        let w = scene.w.max(1.0).ceil() as u32;
        let h = scene.h.max(1.0).ceil() as u32;
        unsafe {
            if layer.surface.is_none() || layer.size != (w, h) {
                let surf = gfx.dcomp.CreateSurface(w, h, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_ALPHA_MODE_PREMULTIPLIED)?;
                layer.visual.SetContent(&surf)?;
                layer.surface = Some(surf);
                layer.size = (w, h);
            }
            let surf = layer.surface.as_ref().unwrap();
            let mut offset = POINT::default();
            let ctx: ID2D1DeviceContext = surf.BeginDraw(None, &mut offset)?;
            let mut p = Painter { clear: true, gfx, ctx: &ctx, theme: look.theme, scale: look.scale, opacity: look.opacity, bg: look.bg, fg: look.fg, images };
            let r = p.paint(scene, (offset.x as f32, offset.y as f32), windows_numerics::Matrix3x2::identity());
            surf.EndDraw()?;
            r?;
            layer.visual.SetOffsetX2(pos.0.round())?;
            layer.visual.SetOffsetY2(pos.1.round())?;
        }
        Ok(())
    }

    /// Draws the panel at `pos` and each positioned OSD scene at its position (monitor-relative px).
    pub fn render(&mut self, panel: Option<(&Scene, (f32, f32))>, osd: &[(Scene, (f32, f32))], look: &Look) -> Result<()> {
        match panel {
            Some((scene, pos)) => Self::render_into(&self.gfx, &mut self.panel, scene, pos, look, &mut self.images)?,
            None => {
                if self.panel.surface.take().is_some() {
                    unsafe { self.panel.visual.SetContent(None)? };
                    self.panel.size = (0, 0);
                }
            }
        }
        while self.osd.len() < osd.len() {
            let l = Layer::new(&self.gfx)?;
            unsafe { self.root.AddVisual(&l.visual, true, None)? };
            self.osd.push(l);
        }
        while self.osd.len() > osd.len() {
            let l = self.osd.pop().unwrap();
            unsafe { self.root.RemoveVisual(&l.visual)? };
        }
        for (layer, (scene, pos)) in self.osd.iter_mut().zip(osd) {
            Self::render_into(&self.gfx, layer, scene, *pos, look, &mut self.images)?;
        }
        self.images.sweep();
        unsafe { self.gfx.dcomp.Commit() }
    }
}
