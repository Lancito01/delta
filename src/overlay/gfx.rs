//! Graphics devices and text: D3D11 -> Direct2D -> DirectComposition, DirectWrite fonts, WIC images.

use crate::theme::Theme;
use std::cell::RefCell;
use std::collections::HashMap;
use windows::Win32::Foundation::{GENERIC_READ, HMODULE};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::core::{HSTRING, Interface, Result, w};

pub struct Gfx {
    pub d2d_factory: ID2D1Factory1,
    pub d2d_device: ID2D1Device,
    pub dwrite: IDWriteFactory,
    pub wic: IWICImagingFactory,
    pub dcomp: IDCompositionDesktopDevice,
    _d3d: ID3D11Device,
}

impl Gfx {
    pub fn new() -> Result<Gfx> {
        unsafe {
            let mut d3d = None;
            let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
            let hw = D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), flags, None, D3D11_SDK_VERSION, Some(&mut d3d), None, None);
            if hw.is_err() {
                D3D11CreateDevice(None, D3D_DRIVER_TYPE_WARP, HMODULE::default(), flags, None, D3D11_SDK_VERSION, Some(&mut d3d), None, None)?;
            }
            let d3d = d3d.unwrap();
            let dxgi: IDXGIDevice = d3d.cast()?;
            let d2d_factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let d2d_device = d2d_factory.CreateDevice(&dxgi)?;
            let dcomp: IDCompositionDesktopDevice = DCompositionCreateDevice2(&d2d_device)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            Ok(Gfx { d2d_factory, d2d_device, dwrite, wic, dcomp, _d3d: d3d })
        }
    }

    /// First installed family from a comma-separated list.
    pub fn pick_family(&self, list: &str) -> String {
        let candidates: Vec<&str> = list.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
        unsafe {
            let mut coll = None;
            if self.dwrite.GetSystemFontCollection(&mut coll, false).is_ok() {
                if let Some(coll) = coll {
                    for c in &candidates {
                        let mut idx = 0u32;
                        let mut exists = windows::core::BOOL::default();
                        if coll.FindFamilyName(&HSTRING::from(*c), &mut idx, &mut exists).is_ok() && exists.as_bool() {
                            return c.to_string();
                        }
                    }
                }
            }
        }
        candidates.first().map(|s| s.to_string()).unwrap_or_else(|| "Segoe UI".into())
    }

    pub fn format(&self, family: &str, size: f32, weight: u32) -> Result<IDWriteTextFormat> {
        unsafe {
            let f = self.dwrite.CreateTextFormat(
                &HSTRING::from(family),
                None,
                DWRITE_FONT_WEIGHT(weight.clamp(100, 950) as i32),
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size.max(1.0),
                w!("en-us"),
            )?;
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            Ok(f)
        }
    }

    /// Decodes an image file into a D2D bitmap for `ctx`.
    pub fn load_image(&self, ctx: &ID2D1DeviceContext, path: &str) -> Result<ID2D1Bitmap1> {
        unsafe {
            let dec = self.wic.CreateDecoderFromFilename(&HSTRING::from(path), None, GENERIC_READ, WICDecodeMetadataCacheOnLoad)?;
            let frame = dec.GetFrame(0)?;
            let conv = self.wic.CreateFormatConverter()?;
            conv.Initialize(&frame, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)?;
            ctx.CreateBitmapFromWicBitmap(&conv, None)
        }
    }
}

/// A measured piece of text, ready to draw.
#[derive(Clone)]
pub struct TextBox {
    pub layout: IDWriteTextLayout,
    pub w: f32,
    pub h: f32,
    pub baseline: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Value,
    Label,
    Unit,
    Hero,
    Header,
}

/// The text formats of the current theme at the current scale.
pub struct Fonts {
    value: IDWriteTextFormat,
    label: IDWriteTextFormat,
    unit: IDWriteTextFormat,
    hero: IDWriteTextFormat,
    header: IDWriteTextFormat,
    value_family: String,
    label_tracking: f32,
    pub uppercase_labels: bool,
    pub value_size: f32,
    typography: IDWriteTypography,
    osd: RefCell<HashMap<u32, IDWriteTextFormat>>,
    widths: RefCell<HashMap<(Role, String), f32>>,
    dwrite: IDWriteFactory,
}

impl Fonts {
    pub fn new(gfx: &Gfx, theme: &Theme, scale: f32) -> Result<Fonts> {
        let f = &theme.font;
        let value_family = gfx.pick_family(&f.family);
        let label_family = gfx.pick_family(&f.label_family);
        let hero = if f.hero_size > 0.0 { f.hero_size } else { f.size };
        let typography = unsafe {
            let t = gfx.dwrite.CreateTypography()?;
            // Tabular figures: digits all the same width, so numbers don't wobble as they change.
            t.AddFontFeature(DWRITE_FONT_FEATURE { nameTag: DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES, parameter: 1 })?;
            t
        };
        Ok(Fonts {
            value: gfx.format(&value_family, f.size * scale, f.weight)?,
            label: gfx.format(&label_family, f.label_size * scale, f.label_weight)?,
            unit: gfx.format(&label_family, f.unit_size * scale, f.label_weight.min(600))?,
            hero: gfx.format(&value_family, hero * scale, f.weight)?,
            header: gfx.format(&label_family, f.label_size * scale, f.label_weight)?,
            value_family,
            label_tracking: f.label_tracking * scale,
            uppercase_labels: f.uppercase_labels,
            value_size: f.size * scale,
            typography,
            osd: RefCell::new(HashMap::new()),
            widths: RefCell::new(HashMap::new()),
            dwrite: gfx.dwrite.clone(),
        })
    }

    pub fn text(&self, role: Role, text: &str) -> Option<TextBox> {
        let (fmt, tracking, tabular) = match role {
            Role::Value => (&self.value, 0.0, true),
            Role::Hero => (&self.hero, 0.0, true),
            Role::Label => (&self.label, self.label_tracking, false),
            Role::Header => (&self.header, self.label_tracking, false),
            Role::Unit => (&self.unit, 0.0, false),
        };
        self.measure(fmt, text, tracking, tabular)
    }

    /// Width of `text` in `role` (cached; used for spacing templates).
    pub fn width(&self, role: Role, text: &str) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let key = (role, text.to_string());
        if let Some(w) = self.widths.borrow().get(&key) {
            return *w;
        }
        let w = self.text(role, text).map_or(0.0, |t| t.w);
        self.widths.borrow_mut().insert(key, w);
        w
    }

    /// Text for RTSS OSD content at `scale` of the value size.
    pub fn osd(&self, text: &str, scale: f32) -> Option<TextBox> {
        let size = (self.value_size * scale).max(4.0);
        let key = (size * 100.0) as u32;
        let fmt = {
            let mut cache = self.osd.borrow_mut();
            if cache.len() > 32 {
                cache.clear();
            }
            match cache.get(&key) {
                Some(f) => f.clone(),
                None => {
                    let f = unsafe {
                        let f = self
                            .dwrite
                            .CreateTextFormat(
                                &HSTRING::from(self.value_family.as_str()),
                                None,
                                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                                DWRITE_FONT_STYLE_NORMAL,
                                DWRITE_FONT_STRETCH_NORMAL,
                                size,
                                w!("en-us"),
                            )
                            .ok()?;
                        let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                        f
                    };
                    cache.insert(key, f.clone());
                    f
                }
            }
        };
        self.measure(&fmt, text, 0.0, true)
    }

    fn measure(&self, fmt: &IDWriteTextFormat, text: &str, tracking: f32, tabular: bool) -> Option<TextBox> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let layout = self.dwrite.CreateTextLayout(&wide, fmt, 10_000.0, 10_000.0).ok()?;
            let range = DWRITE_TEXT_RANGE { startPosition: 0, length: wide.len() as u32 };
            if tabular {
                let _ = layout.SetTypography(&self.typography, range);
            }
            if tracking != 0.0 {
                if let Ok(l1) = layout.cast::<IDWriteTextLayout1>() {
                    let _ = l1.SetCharacterSpacing(0.0, tracking, 0.0, range);
                }
            }
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m).ok()?;
            let mut lines = [DWRITE_LINE_METRICS::default(); 1];
            let mut count = 0u32;
            let _ = layout.GetLineMetrics(Some(&mut lines), &mut count);
            let w = (m.widthIncludingTrailingWhitespace - if wide.is_empty() { 0.0 } else { tracking }).max(0.0);
            Some(TextBox { layout, w, h: m.height, baseline: lines[0].baseline })
        }
    }
}

pub fn color(c: crate::theme::Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c.0[0], g: c.0[1], b: c.0[2], a: c.0[3] }
}

pub fn rect(x: f32, y: f32, w: f32, h: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }
}
