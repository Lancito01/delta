//! Drawing inside exclusive-fullscreen games through RivaTuner Statistics Server.
//!
//! No window can show over a game in exclusive fullscreen. RTSS can: it draws inside games, and
//! most anti-cheats tolerate it. So while RTSS runs, Delta becomes one of its OSD clients, the same
//! way OBS HUD and HWiNFO are: it renders its themed panel into a PNG and asks RTSS to draw that
//! picture. Delta itself never loads anything into a game.
//!
//! RTSS caches pictures by file name, so each update gets a new name (older ones are deleted).

use crate::compose::Frame;
use crate::config::Config;
use crate::overlay::Look;
use crate::overlay::draw::{Images, Painter};
use crate::overlay::gfx::{Fonts, Gfx};
use crate::overlay::layout::{Scene, Sticky};
use crate::theme::Theme;
use std::path::PathBuf;
use windows::Win32::Foundation::GENERIC_WRITE;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::*;
use windows::core::{HSTRING, Result};
use windows_numerics::Matrix3x2;

/// Layer number for our picture: high, to stay clear of RTSS overlay layouts (numbered from 0).
const LAYER: u32 = 33;
/// RTSS repeats a picture after its first 512 pixels across.
const MAX_PICTURE_W: u32 = 512;
/// Width of the transparent column the invisible spacers are cut from.
const SPACER_COLUMN: u32 = 8;

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

    /// Paints `scene` at `at` on a transparent canvas of `size`; returns premultiplied BGRA rows.
    pub fn render(&mut self, gfx: &Gfx, scene: &Scene, look: &Look, size: (u32, u32), at: (f32, f32)) -> Result<Vec<u8>> {
        let (w, h) = (size.0.max(1), size.1.max(1));
        unsafe {
            if self.target.as_ref().is_none_or(|(s, _, _)| *s != (w, h)) {
                let pf = D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED };
                let props = |opts| D2D1_BITMAP_PROPERTIES1 { pixelFormat: pf, dpiX: 96.0, dpiY: 96.0, bitmapOptions: opts, ..Default::default() };
                let sz = D2D_SIZE_U { width: w, height: h };
                let target = self.ctx.CreateBitmap(sz, None, 0, &props(D2D1_BITMAP_OPTIONS_TARGET))?;
                let read = self.ctx.CreateBitmap(sz, None, 0, &props(D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW))?;
                self.target = Some(((w, h), target, read));
            }
            let (_, target, read) = self.target.as_ref().unwrap();
            self.ctx.SetTarget(target);
            self.ctx.BeginDraw();
            self.ctx.Clear(Some(&D2D1_COLOR_F::default()));
            let mut p = Painter { clear: false, gfx, ctx: &self.ctx, theme: look.theme, scale: look.scale, opacity: look.opacity, bg: look.bg, fg: look.fg, images: &mut self.images };
            let r = p.paint(scene, at, Matrix3x2::identity());
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
            Ok(out)
        }
    }
}

/// Saves premultiplied BGRA pixels as a PNG (PNG stores straight alpha).
fn save_png(gfx: &Gfx, path: &std::path::Path, w: u32, h: u32, mut px: Vec<u8>) -> Result<()> {
    for p in px.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    unsafe {
        let wic = &gfx.wic;
        let stream = wic.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(path.as_os_str()), GENERIC_WRITE.0)?;
        let enc = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        enc.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.unwrap();
        frame.Initialize(None)?;
        frame.SetSize(w, h)?;
        let mut fmt = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut fmt)?;
        frame.WritePixels(h, w * 4, &px)?;
        frame.Commit()?;
        enc.Commit()?;
    }
    Ok(())
}

fn hash(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for c in b.chunks(8) {
        let mut v = [0u8; 8];
        v[..c.len()].copy_from_slice(c);
        h = (h.rotate_left(5) ^ u64::from_le_bytes(v)).wrapping_mul(0x517cc1b727220a95);
    }
    h
}

/// What RTSS's text looks like in a font: line height, and the width of one character of an
/// alignment field (`<A=-n>`).
struct RtssFont {
    height: u32,
    cell: f32,
}

/// Measures a font the way RTSS lays it out. Fields are as wide as the font's characters in a
/// monospaced font; in proportional fonts RTSS makes them about 1.2 digits wide (measured).
fn rtss_font(family: &str, px: i32, weight: u32) -> RtssFont {
    use windows::Win32::Graphics::Gdi::*;
    unsafe {
        let dc = CreateCompatibleDC(None);
        let font = CreateFontW(-px, 0, 0, 0, weight as i32, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, ANTIALIASED_QUALITY, 0, &HSTRING::from(family));
        let old = SelectObject(dc, font.into());
        let mut tm = TEXTMETRICW::default();
        let _ = GetTextMetricsW(dc, &mut tm);
        let width = |s: &str| {
            let w: Vec<u16> = s.encode_utf16().collect();
            let mut sz = windows::Win32::Foundation::SIZE::default();
            let _ = GetTextExtentPoint32W(dc, &w, &mut sz);
            sz.cx as f32
        };
        let (zero, wide) = (width("0"), width("W"));
        SelectObject(dc, old);
        let _ = DeleteObject(font.into());
        let _ = DeleteDC(dc);
        let cell = if (zero - wide).abs() < 0.5 { zero } else { zero * 1.2 };
        RtssFont { height: tm.tmHeight.max(1) as u32, cell: cell.max(1.0) }
    }
}

/// Where RTSS should draw, from its entry for the game.
pub struct Target {
    pub width: u32,
    pub height: u32,
    /// RTSS's OSD zoom: it sizes pictures in whole zoomed pixels.
    pub zoom: u32,
}

pub struct Bridge {
    off: Offscreen,
    dir: PathBuf,
    serial: u32,
    last: Option<(u64, String)>,
    fonts: Option<((String, u32), Fonts)>,
    sticky: Sticky,
}

impl Bridge {
    pub fn new(gfx: &Gfx) -> Result<Bridge> {
        let dir = std::env::temp_dir().join("Delta");
        let _ = std::fs::create_dir_all(&dir);
        // Pictures left by an earlier run.
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with("rtss-") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        Ok(Bridge { off: Offscreen::new(gfx)?, dir, serial: 0, last: None, fonts: None, sticky: Sticky::default() })
    }

    pub fn reset(&mut self) {
        self.fonts = None;
        self.sticky.clear();
        self.last = None;
    }

    /// Renders the panel for the game. Returns the RTSS text and embedded objects that show it,
    /// or None if nothing changed since last time.
    ///
    /// The themed panel goes to RTSS as a picture (it updates about once a second). FPS, frame time
    /// and the frame time graph are left empty in it and drawn live by RTSS, every frame, on
    /// layers pinned to the same spot: RTSS's `<FR>`/`<FT>` and its own frame time graph, placed
    /// over the empty slots with transparent spacers cut from the picture.
    #[allow(clippy::too_many_arguments)]
    pub fn update(&mut self, gfx: &Gfx, cfg: &Config, theme: &Theme, frame: &Frame, t: &Target, dpi: f32) -> Option<(String, Vec<u8>)> {
        let scale = dpi * cfg.scale;
        let key = (theme.name.clone(), (scale * 1000.0) as u32);
        if self.fonts.as_ref().is_none_or(|(k, _)| *k != key) {
            self.fonts = Some((key, Fonts::new(gfx, theme, scale).ok()?));
            self.sticky.clear();
        }
        let fonts = &self.fonts.as_ref()?.1;
        let layout = cfg.layout.unwrap_or(theme.layout);
        // RTSS's own OSD text from other apps is drawn by RTSS itself; only Delta's panel goes here.
        let own = Frame { osd: vec![], ..frame.clone() };
        let scene = crate::compose::panel_with(cfg, theme, layout, scale, &own, fonts, &mut self.sticky, true)?;

        // Same spot on screen as the window overlay: the picture runs from the pinned screen edge
        // to the far side of the panel, so the offsets carry over (RTSS pins it to that edge).
        let (aw, ah) = (t.width as f32, t.height as f32);
        let (ax, ay) = cfg.position.anchors();
        let (ox, oy) = (cfg.offset_x as f32 * scale, cfg.offset_y as f32 * scale);
        let px = match ax {
            0 => ox,
            1 => (aw - scene.w) / 2.0,
            _ => aw - scene.w - ox,
        };
        let py = match ay {
            0 => oy,
            1 => (ah - scene.h) / 2.0,
            _ => ah - scene.h - oy,
        };
        let (mut x0, x1) = match ax {
            0 => (0.0, px + scene.w),
            1 => (px, px + scene.w),
            _ => (px, aw),
        };
        let (mut y0, y1) = match ay {
            0 => (0.0, py + scene.h),
            1 => (py, py + scene.h),
            _ => (py, ah),
        };
        // Whole zoomed pixels, growing away from the pinned edge. When Delta sets RTSS's font (live
        // numbers) it also sets the zoom to 1, so units are plain pixels.
        let has_live_text = !scene.live.cells.is_empty();
        let z = if has_live_text { 1 } else { t.zoom.max(1) };
        let w = (((x1 - x0).ceil() as u32).div_ceil(z)) * z;
        let h = (((y1 - y0).ceil() as u32).div_ceil(z)) * z;
        if ax != 0 {
            x0 = x1 - w as f32;
        }
        if ay != 0 {
            y0 = y1 - h as f32;
        }
        let (dx, dy) = ((px - x0).round(), (py - y0).round());
        let look = Look { theme, scale, opacity: cfg.opacity, bg: cfg.background_opacity, fg: cfg.text_opacity };
        let pixels = self.off.render(gfx, &scene, &look, (w, h), (dx, dy)).ok()?;

        // RTSS only keeps the first 512 pixels of a picture's width (it repeats them after that), so
        // wide panels are cut into strips stacked in the picture and drawn back side by side.
        // To their right, a transparent column of z pixels the spacers are cut from.
        // The transparent column is a few pixels wide and spacers sample its middle: RTSS smooths
        // what it draws, and sampling right next to the panel would blend its edge in.
        let pad = SPACER_COLUMN.max(z);
        let strip = ((MAX_PICTURE_W - pad) / z * z).min(w).max(z);
        let strips = w.div_ceil(strip);
        let (atlas_w, atlas_h) = (strip + pad, h * strips);
        let mut atlas = vec![0u8; (atlas_w * atlas_h * 4) as usize];
        for i in 0..strips {
            let sx = i * strip;
            let cw = strip.min(w - sx);
            for row in 0..h {
                let src = ((row * w + sx) * 4) as usize;
                let dst = (((i * h + row) * atlas_w) * 4) as usize;
                atlas[dst..dst + (cw * 4) as usize].copy_from_slice(&pixels[src..src + (cw * 4) as usize]);
            }
        }
        let picture: String = (0..strips)
            .map(|i| {
                let cw = strip.min(w - i * strip);
                format!("<I={},{},0,{},{cw},{h}>", cw / z, h / z, i * h)
            })
            .collect();

        let spot = ay as u32 * 3 + ax as u32;
        let zu = |v: f32| (v / z as f32).round().max(0.0) as u32; // px -> zoomed units
        let (wz, hz) = (w / z, h / z);
        let spacer = |sw: u32, sh: u32| if sw == 0 || sh == 0 { String::new() } else { format!("<I={sw},{sh},{},{},1,1>", strip + pad / 2, h.min(atlas_h) / 2) };
        // A layer the size of the picture, with `line` at (lx, ly) zoomed units inside it.
        let framed = |layer: u32, lx: u32, ly: u32, lh: u32, line: &str| {
            let top = ly.min(hz);
            let bottom = hz.saturating_sub(top + lh);
            // The full width must appear on a spacer line so RTSS pins the layer like the picture.
            let (tw, bw) = if top > 0 { (wz, 0) } else { (0, wz) };
            let mut out = format!("<P{spot}><L{layer}><C=FFFFFF>");
            if top > 0 {
                out += &spacer(tw.max(1), top);
                out.push('\n');
            }
            out += &spacer(lx, lh.max(1));
            out += line;
            if bottom > 0 || bw > 0 {
                out.push('\n');
                out += &spacer(wz, bottom.max(1));
            }
            out + "<C>"
        };
        let hex = |c: crate::theme::Color| {
            let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            format!("{:02X}{:02X}{:02X}", b(c.0[0]), b(c.0[1]), b(c.0[2]))
        };

        let mut live = String::new();
        let mut objects = vec![];
        if let Some((r, color)) = &scene.live.graph {
            let (gx, gy, gw, gh) = (zu(dx + r.x), zu(dy + r.y), zu(r.w).max(4), zu(r.h).max(2));
            // Frame times in microseconds. Scale to the slowest recent frames with headroom, snapped
            // so the graph doesn't keep rescaling.
            let ft = frame.values.get("frametime").copied().unwrap_or(16.7) as f32;
            let max_ms = [8.0f32, 16.7, 33.4, 50.0, 66.7, 100.0, 200.0].into_iter().find(|m| *m >= ft * 2.5).unwrap_or(200.0);
            let mut flags = crate::rtss::graph::FRAMETIME;
            flags |= match theme.graph.style {
                // RTSS's BAR style is a single level gauge, not a history: draw bars as a filled graph.
                crate::theme::GraphStyle::Area | crate::theme::GraphStyle::Step | crate::theme::GraphStyle::Bars => crate::rtss::graph::FILLED,
                crate::theme::GraphStyle::Line => 0,
            };
            let samples = cfg.frametime_samples.clamp(16, 1024);
            let at = crate::rtss::graph::push(&mut objects, gw as i32, gh as i32, 0, 0.0, max_ms * 1000.0, flags, &vec![0.0; samples]);
            let line = format!("<C={}><OBJ={at:08X}><C=FFFFFF>", hex(*color));
            live += &framed(LAYER + 1, gx, gy, gh, &line);
        }
        // Live numbers, in the theme's font (RTSS's <FNT> tag), each right-aligned in a field of
        // its own (<A>) so it hugs its unit in the picture, with its top where the theme puts it.
        let mut font_tag = String::new();
        if !scene.live.cells.is_empty() {
            let family = gfx.pick_family(&theme.font.family);
            let weight = theme.font.weight;
            let hero_px = (if theme.font.hero_size > 0.0 { theme.font.hero_size } else { theme.font.size }) * scale;
            let value_px = theme.font.size * scale;
            let base_px = if scene.live.cells.iter().any(|c| c.hero) { hero_px } else { value_px };
            font_tag = format!("<FNT={family},{},{weight},1>", -(base_px.round() as i32));
            let m = rtss_font(&family, base_px.round() as i32, weight);
            for (i, c) in scene.live.cells.iter().enumerate() {
                let pct = if c.hero { hero_px / base_px } else { value_px / base_px };
                let field = (c.chars as f32 * m.cell * pct).round();
                let x = (dx + c.right - field).max(0.0).round() as u32;
                let y = (dy + c.top).max(0.0).round() as u32;
                let lh = ((m.height as f32) * pct).round().max(1.0) as u32;
                let tag = if c.id == "fps" { "<FR>" } else { "<FT>" };
                let size = if (pct - 1.0).abs() > 0.01 { format!("<S={}>", (pct * 100.0).round()) } else { String::new() };
                let line = format!("{size}<A{i}=-{}><A{i}><C={}>{tag}<A><C=FFFFFF>{}", c.chars, hex(c.color), if size.is_empty() { "" } else { "<S>" });
                live += &framed(LAYER + 2 + i as u32, x, y, lh, &line);
            }
        }

        let hsh = hash(&atlas) ^ hash(live.as_bytes()).rotate_left(7) ^ hash(&objects).rotate_left(13) ^ ((w as u64) << 32 | h as u64) ^ (z as u64) << 56;
        if self.last.as_ref().is_some_and(|(last, _)| *last == hsh) {
            return None;
        }
        self.serial += 1;
        let file = self.dir.join(format!("rtss-{}.png", self.serial));
        save_png(gfx, &file, atlas_w, atlas_h, atlas).ok()?;
        if self.serial > 2 {
            // RTSS may still be reading the previous one.
            let _ = std::fs::remove_file(self.dir.join(format!("rtss-{}.png", self.serial - 2)));
        }
        // <LI> loads the picture (one per slot: every <I> cuts from it), <Pn> pins layer <L> to one
        // of 9 screen spots, <I> draws it at w/zoom x h/zoom zoomed pixels. White stops RTSS from
        // tinting pictures with its text color.
        let text = format!("{font_tag}<LI=\"{}\"><P{spot}><L{LAYER}><C=FFFFFF>{picture}<C>{live}", file.display());
        self.last = Some((hsh, text.clone()));
        Some((text, objects))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Frame {
        let mut f = Frame::default();
        for (k, v) in [("fps", 143.0), ("frametime", 7.0), ("gpu.usage", 97.0), ("gpu.temp", 71.0), ("cpu.usage", 35.0), ("ram.used", 18.2)] {
            f.values.insert(k.into(), v);
        }
        f
    }

    /// Renders the panel for a 1920x1080 game at zoom 2 and checks the picture and RTSS text.
    #[test]
    fn renders_picture_and_tags() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        }
        let gfx = Gfx::new().unwrap();
        let mut cfg = Config::default();
        cfg.position = crate::config::Position::TopRight;
        let theme = crate::theme::load(std::path::Path::new("."), "minimal").unwrap();
        let mut b = Bridge::new(&gfx).unwrap();
        let t = Target { width: 1920, height: 1080, zoom: 2 };
        let (text, objects) = b.update(&gfx, &cfg, &theme, &sample(), &t, 1.0).expect("first update draws");
        // The default FPS row has a frame time graph: drawn live by RTSS as an embedded object.
        assert!(text.contains("<OBJ=00000000>"), "{text}");
        assert!(text.contains("<FR>") && text.contains("<FT>"), "{text}");
        assert!(text.starts_with("<FNT="), "{text}");
        assert_eq!(&objects[0..4], b"00RG");
        assert!(text.contains("<LI=\""), "{text}");
        assert!(text.contains("<P2><L33><C=FFFFFF><I="), "{text}");
        let path = text.split('"').nth(1).unwrap();
        let meta = std::fs::metadata(path).unwrap();
        assert!(meta.len() > 500);
        // Setting RTSS's font also sets its zoom to 1: pictures are drawn 1:1.
        let nums: Vec<u32> = text.split("<I=").nth(1).unwrap().split('>').next().unwrap().split(',').map(|n| n.parse().unwrap()).collect();
        assert_eq!(nums[0], nums[4]);
        assert_eq!(nums[1], nums[5]);
        // A one-line panel wider than RTSS's 512-pixel limit is cut into strips.
        let mut wide = cfg.clone();
        wide.layout = Some(crate::theme::Layout::Bar);
        let mut f = sample();
        for (k, v) in [("gpu.clock", 1920.0), ("gpu.power", 240.0), ("gpu.vram", 3800.0), ("cpu.clock", 4730.0)] {
            f.values.insert(k.into(), v);
        }
        let mut b2 = Bridge::new(&gfx).unwrap();
        let (wtext, _) = b2.update(&gfx, &wide, &theme, &f, &Target { width: 2560, height: 1440, zoom: 1 }, 1.0).unwrap();
        let first = wtext.split("<L33>").nth(1).unwrap().split("<P").next().unwrap();
        assert!(!wtext.contains("<FR> FPS"), "units come from the picture now: {wtext}");
        assert!(first.matches("<I=").count() >= 2, "expected strips: {first}");
        // Same content: nothing new to send.
        assert!(b.update(&gfx, &cfg, &theme, &sample(), &t, 1.0).is_none());
        if let Ok(dir) = std::env::var("DELTA_TEST_OUT") {
            let _ = std::fs::copy(path, std::path::Path::new(&dir).join("bridge.png"));
        }
    }

    /// Sends a picture through the RTSS client protocol. Needs something serving RTSS's shared
    /// memory (RTSS, or a Delta instance standing in for it): `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn client_roundtrip() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        }
        let gfx = Gfx::new().unwrap();
        let mut cfg = Config::default();
        cfg.position = crate::config::Position::BottomRight;
        let mut theme = crate::theme::load(std::path::Path::new("."), "neon").unwrap();
        let mut b = Bridge::new(&gfx).unwrap();
        // DELTA_TEST_PID: lay out for that game as RTSS sees it (resolution and zoom).
        let t = match std::env::var("DELTA_TEST_PID").ok().and_then(|p| p.parse().ok()).and_then(crate::rtss::client_app) {
            Some(a) => Target { width: a.width, height: a.height, zoom: a.zoom },
            None => Target { width: 2560, height: 1440, zoom: 1 },
        };
        if let Ok(p) = std::env::var("DELTA_TEST_POS") {
            cfg.position = match p.as_str() {
                "tl" => crate::config::Position::TopLeft,
                "tc" => crate::config::Position::TopCenter,
                "tr" => crate::config::Position::TopRight,
                "bl" => crate::config::Position::BottomLeft,
                _ => crate::config::Position::BottomRight,
            };
        }
        if let Ok(l) = std::env::var("DELTA_TEST_LAYOUT") {
            cfg.layout = Some(if l == "bar" { crate::theme::Layout::Bar } else { crate::theme::Layout::Stack });
        }
        let dpi: f32 = std::env::var("DELTA_TEST_DPI").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
        if let Ok(th) = std::env::var("DELTA_TEST_THEME") {
            theme = crate::theme::load(std::path::Path::new("."), &th).unwrap();
        }
        let mut frame = sample();
        for (k, v) in [("gpu.clock", 1920.0), ("gpu.power", 240.0), ("gpu.vram", 3800.0), ("gpu.vram_pct", 37.0), ("cpu.clock", 4730.0), ("ram.pct", 92.0)] {
            frame.values.insert(k.into(), v);
        }
        frame.cores = vec![40.0, 10.0, 25.0, 5.0, 60.0, 15.0, 30.0, 8.0, 12.0, 45.0, 3.0, 20.0, 33.0, 7.0, 50.0, 18.0];
        let (text, objects) = b.update(&gfx, &cfg, &theme, &frame, &t, dpi).unwrap();
        println!("{text}");
        assert!(crate::rtss::client_show_with(&text, &objects), "no RTSS shared memory, or no free slot");
        std::thread::sleep(std::time::Duration::from_secs(10));
        crate::rtss::client_clear();
    }
}

#[cfg(test)]
mod docs {
    use super::*;

    /// Renders every built-in theme with sample readings into docs/themes/*.png, for the README.
    /// `cargo test docs_images -- --ignored`
    #[test]
    #[ignore]
    fn docs_images() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        }
        let gfx = Gfx::new().unwrap();
        let mut off = Offscreen::new(&gfx).unwrap();
        let mut f = Frame::default();
        for (k, v) in [
            ("fps", 144.0), ("frametime", 6.9), ("fps.low1", 118.0), ("gpu.usage", 97.0), ("gpu.temp", 68.0), ("gpu.clock", 1920.0),
            ("gpu.power", 241.0), ("gpu.vram", 6840.0), ("gpu.vram_pct", 67.0), ("cpu.usage", 41.0), ("cpu.clock", 4730.0), ("ram.used", 17.6), ("ram.pct", 55.0),
        ] {
            f.values.insert(k.into(), v);
        }
        f.frametimes = (0..180).map(|i| 6.9 + ((i as f32 * 0.61).sin() * 0.5) + if i % 41 == 0 { 5.5 } else if i % 17 == 0 { 1.6 } else { 0.0 }).collect();
        f.cores = vec![62.0, 18.0, 45.0, 9.0, 71.0, 22.0, 38.0, 12.0, 55.0, 30.0, 8.0, 41.0, 66.0, 15.0, 49.0, 27.0];
        let dir = std::path::Path::new("docs/themes");
        std::fs::create_dir_all(dir).unwrap();
        let cfg = Config::default();
        for (name, _) in crate::theme::BUILTIN {
            let theme = crate::theme::load(std::path::Path::new("themes"), name).unwrap();
            let scale = 1.5;
            let fonts = Fonts::new(&gfx, &theme, scale).unwrap();
            let mut sticky = Sticky::default();
            let layout = theme.layout;
            let scene = crate::compose::panel(&cfg, &theme, layout, scale, &f, &fonts, &mut sticky).unwrap();
            let look = Look { theme: &theme, scale, opacity: 1.0, bg: 1.0, fg: 1.0 };
            let (w, h) = (scene.w.ceil() as u32 + 24, scene.h.ceil() as u32 + 24);
            let px = off.render(&gfx, &scene, &look, (w, h), (12.0, 12.0)).unwrap();
            save_png(&gfx, &dir.join(format!("{name}.png")), w, h, px).unwrap();
        }
    }
}
