//! Paints a Scene with Direct2D: panel, decorations, text, graphs, bars, images, glow and shadow.

use super::gfx::{Gfx, color, rect};
use super::layout::{Prim, Rect, Scene};
use crate::theme::{Color, CornerStyle, GraphStyle, Theme};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::core::{Interface, Result};
use windows_numerics::{Matrix3x2, Vector2};

/// Decoded images for RTSS `<LI>` tags, by path. Failures are remembered briefly so a missing
/// file isn't re-read every frame.
#[derive(Default)]
pub struct Images {
    cache: HashMap<String, (Option<ID2D1Bitmap1>, Instant)>,
}

impl Images {
    fn get(&mut self, gfx: &Gfx, ctx: &ID2D1DeviceContext, path: &str) -> Option<ID2D1Bitmap1> {
        let now = Instant::now();
        if let Some((bmp, at)) = self.cache.get_mut(path) {
            if bmp.is_some() || now.duration_since(*at) < Duration::from_millis(500) {
                *at = if bmp.is_some() { now } else { *at };
                return bmp.clone();
            }
        }
        let bmp = gfx.load_image(ctx, path).ok();
        self.cache.insert(path.to_string(), (bmp.clone(), now));
        bmp
    }

    /// Drops images not drawn for a few seconds (RTSS clients cycle through file names).
    pub fn sweep(&mut self) {
        let now = Instant::now();
        self.cache.retain(|_, (_, at)| now.duration_since(*at) < Duration::from_secs(5));
    }

    pub fn clear(&mut self) {
        self.cache.clear();
    }
}

pub struct Painter<'a> {
    /// Clear the target first (overlay surfaces); off when drawing into another canvas (previews).
    pub clear: bool,
    pub gfx: &'a Gfx,
    pub ctx: &'a ID2D1DeviceContext,
    pub theme: &'a Theme,
    pub scale: f32,
    pub opacity: f32,
    /// Background opacity (panel, borders, graph backdrops, bar tracks).
    pub bg: f32,
    /// Foreground opacity (text, graph lines, bars).
    pub fg: f32,
    pub images: &'a mut Images,
}

fn pt(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

fn clamp_alpha(c: Color) -> Color {
    Color([c.0[0], c.0[1], c.0[2], c.0[3].clamp(0.0, 1.0)])
}

impl Painter<'_> {
    /// Paints `scene` with its top-left at `offset`, on top of the transform `outer`.
    pub fn paint(&mut self, scene: &Scene, offset: (f32, f32), outer: Matrix3x2) -> Result<()> {
        let ctx = self.ctx;
        let t = self.theme;
        let s = self.scale;
        let base = Matrix3x2::translation(offset.0, offset.1) * outer;
        unsafe {
            ctx.SetTransform(&base);
            ctx.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            ctx.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            if self.clear {
                ctx.Clear(Some(&D2D1_COLOR_F::default()));
            }
            let brush = ctx.CreateSolidColorBrush(&color(t.colors.value), None)?;

            let faded = self.opacity < 0.999;
            if faded {
                let params = D2D1_LAYER_PARAMETERS1 {
                    contentBounds: rect(-1e6, -1e6, 2e6, 2e6),
                    maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                    maskTransform: Matrix3x2::identity(),
                    opacity: self.opacity,
                    ..Default::default()
                };
                ctx.PushLayer(&params, None);
            }

            let panel = if scene.panel { Some(self.panel_geometry(scene.w, scene.h)?) } else { None };
            if let Some(g) = &panel {
                let bg = self.gradient(t.colors.background.with_alpha(self.bg), t.colors.background_end.with_alpha(self.bg), 0.0, scene.h)?;
                ctx.FillGeometry(g, &bg, None);
                if t.decor.scanlines && t.decor.scanline_alpha > 0.0 {
                    let params = D2D1_LAYER_PARAMETERS1 {
                        contentBounds: rect(0.0, 0.0, scene.w, scene.h),
                        geometricMask: std::mem::ManuallyDrop::new(Some(g.cast()?)),
                        maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                        maskTransform: Matrix3x2::identity(),
                        opacity: 1.0,
                        ..Default::default()
                    };
                    ctx.PushLayer(&params, None);
                    brush.SetColor(&color(t.colors.accent.with_alpha(t.decor.scanline_alpha * self.bg)));
                    let step = (3.0 * s).max(2.0);
                    let mut y = 0.0;
                    while y < scene.h {
                        ctx.FillRectangle(&rect(0.0, y, scene.w, (s * 0.8).max(1.0)), &brush);
                        y += step;
                    }
                    ctx.PopLayer();
                    let _: Option<ID2D1Geometry> = std::mem::ManuallyDrop::into_inner(params.geometricMask);
                }
            }

            // Soft shadow and glow come from blurred copies of the content.
            let wants_shadow = t.text.shadow;
            let wants_glow = t.text.glow > 0.0;
            if wants_shadow || wants_glow {
                ctx.SetTransform(&Matrix3x2::identity());
                let old = ctx.GetTarget()?;
                if wants_shadow {
                    let cl = ctx.CreateCommandList()?;
                    ctx.SetTarget(&cl);
                    self.prims(scene, &brush, false)?;
                    ctx.SetTarget(&old);
                    cl.Close()?;
                    let fx = ctx.CreateEffect(&CLSID_D2D1Shadow)?;
                    fx.SetInput(0, &cl, true);
                    set_f32(&fx, D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION.0 as u32, 1.4 * s)?;
                    let c = t.colors.shadow.0;
                    set_vec4(&fx, D2D1_SHADOW_PROP_COLOR.0 as u32, [c[0], c[1], c[2], c[3] * self.fg])?;
                    let out = fx.GetOutput()?;
                    ctx.SetTransform(&base);
                    let off = t.text.shadow_offset * s;
                    ctx.DrawImage(&out, Some(&pt(off, off)), None, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_COMPOSITE_MODE_SOURCE_OVER);
                    ctx.DrawImage(&out, Some(&pt(0.0, 0.0)), None, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_COMPOSITE_MODE_SOURCE_OVER);
                    ctx.SetTransform(&Matrix3x2::identity());
                }
                if wants_glow {
                    let cl = ctx.CreateCommandList()?;
                    ctx.SetTarget(&cl);
                    self.prims(scene, &brush, true)?;
                    ctx.SetTarget(&old);
                    cl.Close()?;
                    let fx = ctx.CreateEffect(&CLSID_D2D1GaussianBlur)?;
                    fx.SetInput(0, &cl, true);
                    set_f32(&fx, D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32, 3.5 * s)?;
                    let out = fx.GetOutput()?;
                    ctx.SetTransform(&base);
                    let params = D2D1_LAYER_PARAMETERS1 {
                        contentBounds: rect(-1e6, -1e6, 2e6, 2e6),
                        maskTransform: Matrix3x2::identity(),
                        opacity: (t.text.glow * 0.85 * self.fg).clamp(0.0, 1.0),
                        ..Default::default()
                    };
                    ctx.PushLayer(&params, None);
                    ctx.DrawImage(&out, None, None, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_COMPOSITE_MODE_SOURCE_OVER);
                    ctx.PopLayer();
                }
                ctx.SetTransform(&base);
            }

            self.prims(scene, &brush, false)?;

            if let Some(g) = &panel {
                self.border(g, scene, &brush)?;
            }
            if faded {
                ctx.PopLayer();
            }
        }
        Ok(())
    }

    fn gradient(&self, top: Color, bottom: Color, y0: f32, y1: f32) -> Result<ID2D1LinearGradientBrush> {
        unsafe {
            let stops = [
                D2D1_GRADIENT_STOP { position: 0.0, color: color(clamp_alpha(top)) },
                D2D1_GRADIENT_STOP { position: 1.0, color: color(clamp_alpha(bottom)) },
            ];
            let rt: &ID2D1RenderTarget = self.ctx;
            let coll = rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
            self.ctx.CreateLinearGradientBrush(
                &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: pt(0.0, y0), endPoint: pt(0.0, y1) },
                None,
                &coll,
            )
        }
    }

    fn panel_geometry(&self, w: f32, h: f32) -> Result<ID2D1Geometry> {
        let t = self.theme;
        let s = self.scale;
        let inset = t.shape.border_width * s / 2.0;
        let (x0, y0, x1, y1) = (inset, inset, w - inset, h - inset);
        let r = (t.shape.corner_radius * s).min((x1 - x0) / 2.0).min((y1 - y0) / 2.0).max(0.0);
        let f = &self.gfx.d2d_factory;
        unsafe {
            match t.shape.corner_style {
                CornerStyle::Rounded => Ok(f
                    .CreateRoundedRectangleGeometry(&D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left: x0, top: y0, right: x1, bottom: y1 }, radiusX: r, radiusY: r })?
                    .cast()?),
                CornerStyle::Brackets => Ok(f.CreateRectangleGeometry(&D2D_RECT_F { left: x0, top: y0, right: x1, bottom: y1 })?.cast()?),
                CornerStyle::Cut => {
                    // Chamfer the top-left and bottom-right corners only; asymmetric reads more "tech".
                    let path = f.CreatePathGeometry()?;
                    let sink = path.Open()?;
                    sink.BeginFigure(pt(x0 + r, y0), D2D1_FIGURE_BEGIN_FILLED);
                    let pts = [pt(x1, y0), pt(x1, y1 - r), pt(x1 - r, y1), pt(x0, y1), pt(x0, y0 + r)];
                    sink.AddLines(&pts);
                    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                    sink.Close()?;
                    Ok(path.cast()?)
                }
            }
        }
    }

    fn border(&self, g: &ID2D1Geometry, scene: &Scene, brush: &ID2D1SolidColorBrush) -> Result<()> {
        let t = self.theme;
        let s = self.scale;
        let bw = t.shape.border_width * s;
        if bw <= 0.0 || t.colors.border.0[3] <= 0.0 {
            return Ok(());
        }
        unsafe {
            match t.shape.corner_style {
                CornerStyle::Brackets => {
                    brush.SetColor(&color(t.colors.border.with_alpha(0.35 * self.bg)));
                    self.ctx.DrawGeometry(g, brush, bw, None);
                    brush.SetColor(&color(clamp_alpha(t.colors.border.with_alpha(2.5)).with_alpha(self.bg)));
                    let len = (t.shape.corner_radius * s * 1.4).max(6.0 * s);
                    let w2 = bw * 2.0;
                    let (w, h) = (scene.w, scene.h);
                    for (x, y, dx, dy) in [(0.0, 0.0, 1.0, 1.0), (w, 0.0, -1.0, 1.0), (0.0, h, 1.0, -1.0), (w, h, -1.0, -1.0)] {
                        let hx = if dx > 0.0 { x } else { x - len };
                        let vy = if dy > 0.0 { y } else { y - len };
                        let hy = if dy > 0.0 { y } else { y - w2 };
                        let vx = if dx > 0.0 { x } else { x - w2 };
                        self.ctx.FillRectangle(&rect(hx, hy, len, w2), brush);
                        self.ctx.FillRectangle(&rect(vx, vy, w2, len), brush);
                    }
                }
                _ => {
                    // Border fades toward the bottom: a hairline that catches the light at the top.
                    let b = self.gradient(t.colors.border.with_alpha(self.bg), t.colors.border.with_alpha(0.45 * self.bg), 0.0, scene.h)?;
                    self.ctx.DrawGeometry(g, &b, bw, None);
                }
            }
        }
        Ok(())
    }

    /// Draws the primitives; with `fx_only`, just the ones that glow.
    fn prims(&mut self, scene: &Scene, brush: &ID2D1SolidColorBrush, fx_only: bool) -> Result<()> {
        let ctx = self.ctx;
        let t = self.theme;
        let s = self.scale;
        unsafe {
            for p in &scene.prims {
                match p {
                    Prim::Text { x, y, tb, color: c, fx } => {
                        if fx_only && !fx {
                            continue;
                        }
                        brush.SetColor(&color(clamp_alpha(*c).with_alpha(self.fg)));
                        ctx.DrawTextLayout(pt(*x, *y), &tb.layout, brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
                    }
                    Prim::Fill { r, color: c, radius } => {
                        if fx_only {
                            continue;
                        }
                        brush.SetColor(&color(clamp_alpha(*c).with_alpha(self.fg)));
                        if *radius > 0.0 {
                            ctx.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect(r.x, r.y, r.w, r.h), radiusX: *radius, radiusY: *radius }, brush);
                        } else {
                            ctx.FillRectangle(&rect(r.x, r.y, r.w, r.h), brush);
                        }
                    }
                    Prim::Graph { r, values, slots, min, max, color: c, style } => self.graph(*r, values, *slots, *min, *max, *c, *style, brush, fx_only)?,
                    Prim::Bars { r, values, color: c } => {
                        if fx_only {
                            continue;
                        }
                        let n = values.len().max(1) as f32;
                        let gap = t.cores.bar_gap * s;
                        let bw = ((r.w + gap) / n - gap).max(1.0);
                        let rad = (t.cores.radius * s).min(bw / 2.0);
                        for (i, v) in values.iter().enumerate() {
                            let x = r.x + i as f32 * (bw + gap);
                            brush.SetColor(&color(t.colors.bar_track.with_alpha(self.bg)));
                            ctx.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect(x, r.y, bw, r.h), radiusX: rad, radiusY: rad }, brush);
                            let frac = (v / 100.0).clamp(0.0, 1.0);
                            if frac > 0.0 {
                                let h = (r.h * frac).max(s);
                                let col = if *v >= 95.0 {
                                    c.lerp(t.colors.crit, 0.7)
                                } else if *v >= 80.0 {
                                    c.lerp(t.colors.warn, 0.5)
                                } else {
                                    *c
                                };
                                brush.SetColor(&color(col.with_alpha(self.fg)));
                                ctx.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect(x, r.y + r.h - h, bw, h), radiusX: rad, radiusY: rad }, brush);
                            }
                        }
                    }
                    Prim::Image { r, path, src } => {
                        if fx_only {
                            continue;
                        }
                        if let Some(bmp) = self.images.get(self.gfx, ctx, path) {
                            let src = src.map(|s| D2D_RECT_F { left: s[0], top: s[1], right: s[0] + s[2], bottom: s[1] + s[3] });
                            ctx.DrawBitmap(&bmp, Some(&rect(r.x, r.y, r.w, r.h)), 1.0, D2D1_INTERPOLATION_MODE_LINEAR, src.as_ref().map(|r| r as *const _), None);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn graph(&self, r: Rect, values: &[f32], slots: usize, min: f32, max: f32, c: Color, style: GraphStyle, brush: &ID2D1SolidColorBrush, fx_only: bool) -> Result<()> {
        let ctx = self.ctx;
        let t = self.theme;
        let s = self.scale;
        let lw = t.graph.line_width * s;
        unsafe {
            if !fx_only {
                if t.graph.background.0[3] > 0.0 {
                    brush.SetColor(&color(t.graph.background.with_alpha(self.bg)));
                    ctx.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect(r.x, r.y, r.w, r.h), radiusX: 2.0 * s, radiusY: 2.0 * s }, brush);
                }
                if t.graph.grid {
                    brush.SetColor(&color(t.colors.graph_grid.with_alpha(self.bg)));
                    for f in [0.25, 0.5, 0.75] {
                        ctx.FillRectangle(&rect(r.x, (r.y + r.h * f).round(), r.w, s.max(1.0)), brush);
                    }
                }
            }
            if values.len() < 2 {
                return Ok(());
            }
            let slots = slots.max(values.len()).max(2);
            let step = r.w / (slots - 1) as f32;
            let start = slots - values.len();
            let range = (max - min).max(1e-6);
            let inner = r.h - lw;
            let px = |i: usize| r.x + (start + i) as f32 * step;
            let py = |v: f32| r.y + lw / 2.0 + inner - ((v - min) / range).clamp(0.0, 1.0) * inner;

            ctx.PushAxisAlignedClip(&rect(r.x, r.y, r.w, r.h), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            let c = c.with_alpha(self.fg);
            let fill_c = if t.colors.graph_fill.0[3] > 0.0 { t.colors.graph_fill.with_alpha(self.fg) } else { c.with_alpha(0.38) };
            match style {
                GraphStyle::Bars => {
                    let bw = (step - s).max(1.0);
                    brush.SetColor(&color(if fx_only { c } else { c.with_alpha(0.9) }));
                    for (i, v) in values.iter().enumerate() {
                        let y = py(*v);
                        ctx.FillRectangle(&rect(px(i) - bw / 2.0, y, bw, r.y + r.h - y), brush);
                    }
                }
                _ => {
                    let f = &self.gfx.d2d_factory;
                    let line = f.CreatePathGeometry()?;
                    let area = f.CreatePathGeometry()?;
                    let mut pts = Vec::with_capacity(values.len() * 2);
                    for (i, v) in values.iter().enumerate() {
                        let (x, y) = (px(i), py(*v));
                        if style == GraphStyle::Step {
                            if let Some(last) = pts.last().copied() {
                                let last: Vector2 = last;
                                pts.push(pt(x, last.Y));
                            }
                        }
                        pts.push(pt(x, y));
                    }
                    let sink = line.Open()?;
                    sink.BeginFigure(pts[0], D2D1_FIGURE_BEGIN_HOLLOW);
                    sink.AddLines(&pts[1..]);
                    sink.EndFigure(D2D1_FIGURE_END_OPEN);
                    sink.Close()?;
                    if style != GraphStyle::Line && !fx_only {
                        let sink = area.Open()?;
                        sink.BeginFigure(pt(pts[0].X, r.y + r.h), D2D1_FIGURE_BEGIN_FILLED);
                        sink.AddLines(&pts);
                        sink.AddLine(pt(pts[pts.len() - 1].X, r.y + r.h));
                        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                        sink.Close()?;
                        let top = if style == GraphStyle::Step { fill_c.with_alpha(0.5) } else { fill_c };
                        let g = self.gradient(top, fill_c.with_alpha(0.0), r.y, r.y + r.h)?;
                        ctx.FillGeometry(&area, &g, None);
                    }
                    let props = D2D1_STROKE_STYLE_PROPERTIES1 {
                        startCap: D2D1_CAP_STYLE_ROUND,
                        endCap: D2D1_CAP_STYLE_ROUND,
                        lineJoin: D2D1_LINE_JOIN_ROUND,
                        ..Default::default()
                    };
                    let stroke = f.CreateStrokeStyle(&props, None)?;
                    brush.SetColor(&color(c));
                    ctx.DrawGeometry(&line, brush, lw, &stroke);
                }
            }
            ctx.PopAxisAlignedClip();
        }
        Ok(())
    }
}

fn set_f32(fx: &ID2D1Effect, index: u32, v: f32) -> Result<()> {
    unsafe { fx.SetValue(index, D2D1_PROPERTY_TYPE_FLOAT, &v.to_ne_bytes()) }
}

fn set_vec4(fx: &ID2D1Effect, index: u32, v: [f32; 4]) -> Result<()> {
    let mut b = [0u8; 16];
    for i in 0..4 {
        b[i * 4..i * 4 + 4].copy_from_slice(&v[i].to_ne_bytes());
    }
    unsafe { fx.SetValue(index, D2D1_PROPERTY_TYPE_VECTOR4, &b) }
}
