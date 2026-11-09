//! A small immediate-mode UI kit drawn with Direct2D/DirectWrite, styled after Windows 11 settings.
//! Everything is in DIPs; the window applies its DPI scale as a transform.

use crate::overlay::gfx::Gfx;
use crate::theme::Color;
use std::collections::HashMap;
use std::time::Instant;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows_numerics::Vector2;

// ---------------------------------------------------------------- palette

const fn c(hex: u32, a: u8) -> Color {
    Color([((hex >> 16) & 0xFF) as f32 / 255.0, ((hex >> 8) & 0xFF) as f32 / 255.0, (hex & 0xFF) as f32 / 255.0, a as f32 / 255.0])
}

pub mod pal {
    use super::c;
    use crate::theme::Color;
    pub const BG: Color = c(0x17181D, 255);
    pub const SIDEBAR: Color = c(0x121318, 255);
    pub const CARD: Color = c(0x1F2128, 255);
    pub const CARD_HOVER: Color = c(0x252830, 255);
    pub const STROKE: Color = c(0xFFFFFF, 14);
    pub const STROKE_STRONG: Color = c(0xFFFFFF, 30);
    pub const TEXT: Color = c(0xECEEF3, 255);
    pub const DIM: Color = c(0xA0A7B7, 255);
    pub const FAINT: Color = c(0x6E7686, 255);
    pub const ACCENT: Color = c(0x8AB4FF, 255);
    pub const ACCENT_SOFT: Color = c(0x8AB4FF, 36);
    pub const ON_ACCENT: Color = c(0x0E1320, 255);
    pub const CONTROL: Color = c(0x2B2E37, 255);
    pub const CONTROL_HOVER: Color = c(0x343843, 255);
    pub const TRACK_OFF: Color = c(0x454A56, 255);
    pub const GOOD: Color = c(0xA6E3A1, 255);
    pub const WARN: Color = c(0xF9E2AF, 255);
    pub const BAD: Color = c(0xF38BA8, 255);
    pub const POPUP: Color = c(0x272A32, 255);
    pub const SCRIM: Color = c(0x000000, 140);
}

pub mod icon {
    pub const HOME: char = '\u{E80F}';
    pub const LIST: char = '\u{E8FD}';
    pub const COLOR: char = '\u{E790}';
    pub const VIEW: char = '\u{E890}';
    pub const GAME: char = '\u{E7FC}';
    pub const SETTINGS: char = '\u{E713}';
    pub const ADD: char = '\u{E710}';
    pub const DELETE: char = '\u{E74D}';
    pub const UP: char = '\u{E70E}';
    pub const DOWN: char = '\u{E70D}';
    pub const CLOSE: char = '\u{E711}';
    pub const CHECK: char = '\u{E73E}';
    pub const SHIELD: char = '\u{EA18}';
    pub const FOLDER: char = '\u{E838}';
    pub const COPY: char = '\u{E8C8}';
    pub const KEYBOARD: char = '\u{E765}';
    pub const MONITOR: char = '\u{E7F4}';
    pub const REFRESH: char = '\u{E72C}';
    pub const SPEED: char = '\u{EC4A}';
    pub const CHIP: char = '\u{EEA1}';
    pub const EYE_OFF: char = '\u{ED1A}';
    pub const TEXT: char = '\u{E8D2}';
    pub const RESIZE: char = '\u{E740}';
    pub const OPACITY: char = '\u{E793}';
    pub const POSITION: char = '\u{E81E}';
    pub const SPACING: char = '\u{E8E4}';
    pub const POWER: char = '\u{E7E8}';
}

// ---------------------------------------------------------------- geometry & input

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn contains(&self, p: (f32, f32)) -> bool {
        p.0 >= self.x && p.0 < self.x + self.w && p.1 >= self.y && p.1 < self.y + self.h
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn inset(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w - dx * 2.0, self.h - dy * 2.0)
    }
    fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.x, top: self.y, right: self.x + self.w, bottom: self.y + self.h }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub vk: u32,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

#[derive(Default)]
pub struct Input {
    pub mouse: (f32, f32),
    pub down: bool,
    pub pressed: bool,
    pub released: bool,
    pub wheel: f32,
    pub chars: Vec<char>,
    pub keys: Vec<Key>,
}

impl Input {
    /// Clears per-frame events (keeps position and button state).
    pub fn next_frame(&mut self) {
        self.pressed = false;
        self.released = false;
        self.wheel = 0.0;
        self.chars.clear();
        self.keys.clear();
    }
}

pub enum Popup {
    Menu { id: u64, anchor: Rect, items: Vec<String>, selected: Vec<usize>, multi: bool },
}

/// UI state that survives between frames.
pub struct State {
    pub hot: u64,
    pub active: u64,
    pub focus: u64,
    anims: HashMap<u64, f32>,
    pub popup: Option<Popup>,
    pub picked: Option<(u64, usize)>,
    pub modal: Option<u32>,
    pub edit: String,
    pub scroll: f32,
    pub scroll_target: f32,
    pub modal_scroll: f32,
    pub modal_scroll_target: f32,
    pub content_h: f32,
    pub view_h: f32,
    pub hand: bool,
    /// (row index, metric index) for a metric pill being dragged.
    pub metric_drag: Option<(usize, usize)>,
    /// Row whose pills are finishing a reorder animation.
    pub metric_reorder: Option<usize>,
    pub animating: bool,
    last: Instant,
    pub(crate) dt: f32,
    swallow: bool,
    caret: Instant,
}

impl Default for State {
    fn default() -> Self {
        State {
            hot: 0,
            active: 0,
            focus: 0,
            anims: HashMap::new(),
            popup: None,
            picked: None,
            modal: None,
            edit: String::new(),
            scroll: 0.0,
            scroll_target: 0.0,
            modal_scroll: 0.0,
            modal_scroll_target: 0.0,
            content_h: 0.0,
            view_h: 0.0,
            hand: false,
            metric_drag: None,
            metric_reorder: None,
            animating: false,
            last: Instant::now(),
            dt: 0.016,
            swallow: false,
            caret: Instant::now(),
        }
    }
}

pub fn id(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

pub fn idn(s: &str, n: usize) -> u64 {
    id(s) ^ (n as u64 + 1).wrapping_mul(0x9E3779B97F4A7C15)
}

// ---------------------------------------------------------------- text

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Font {
    Body,
    BodyStrong,
    Small,
    Caption,
    Title,
    Section,
    Icon,
    IconLarge,
    Brand,
}

pub struct Texts {
    formats: HashMap<Font, IDWriteTextFormat>,
    cache: HashMap<(Font, String, u32), (IDWriteTextLayout, f32, f32)>,
    dwrite: IDWriteFactory,
}

impl Texts {
    pub fn new(gfx: &Gfx) -> Texts {
        let text = gfx.pick_family("Segoe UI Variable Text, Segoe UI");
        let display = gfx.pick_family("Segoe UI Variable Display, Segoe UI");
        let icons = gfx.pick_family("Segoe Fluent Icons, Segoe MDL2 Assets");
        let mut formats = HashMap::new();
        let mut add = |f: Font, family: &str, size: f32, weight: u32| {
            if let Ok(fmt) = gfx.format(family, size, weight) {
                formats.insert(f, fmt);
            }
        };
        add(Font::Body, &text, 14.0, 400);
        add(Font::BodyStrong, &text, 14.0, 600);
        add(Font::Small, &text, 12.0, 400);
        add(Font::Caption, &text, 11.0, 600);
        add(Font::Title, &display, 28.0, 600);
        add(Font::Section, &text, 14.0, 600);
        add(Font::Icon, &icons, 16.0, 400);
        add(Font::IconLarge, &icons, 20.0, 400);
        add(Font::Brand, &display, 17.0, 700);
        Texts { formats, cache: HashMap::new(), dwrite: gfx.dwrite.clone() }
    }

    /// (layout, width, height). `wrap` > 0 wraps at that width.
    pub fn get(&mut self, f: Font, s: &str, wrap: f32) -> Option<(IDWriteTextLayout, f32, f32)> {
        let key = (f, s.to_string(), wrap as u32);
        if let Some(v) = self.cache.get(&key) {
            return Some(v.clone());
        }
        if self.cache.len() > 3000 {
            self.cache.clear();
        }
        let fmt = self.formats.get(&f)?;
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let layout = self.dwrite.CreateTextLayout(&wide, fmt, if wrap > 0.0 { wrap } else { 10_000.0 }, 10_000.0).ok()?;
            if wrap > 0.0 {
                let _ = layout.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
            }
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m).ok()?;
            let v = (layout, m.widthIncludingTrailingWhitespace, m.height);
            self.cache.insert(key, v.clone());
            Some(v)
        }
    }
}

fn col(c: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c.0[0], g: c.0[1], b: c.0[2], a: c.0[3].clamp(0.0, 1.0) }
}

fn pt(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Subtle,
    Danger,
}

// ---------------------------------------------------------------- the UI

pub struct Ui<'a> {
    pub ctx: &'a ID2D1DeviceContext,
    pub brush: ID2D1SolidColorBrush,
    pub texts: &'a mut Texts,
    pub inp: &'a Input,
    pub st: &'a mut State,
    /// Layout cursor.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    /// Window size in DIPs.
    pub size: (f32, f32),
    /// Clip of the scrolling area (mouse outside it doesn't reach widgets inside).
    pub clip: Option<Rect>,
    /// Everything under a modal or popup ignores the mouse.
    pub blocked: bool,
}

impl<'a> Ui<'a> {
    pub fn new(ctx: &'a ID2D1DeviceContext, gfx: &'a Gfx, texts: &'a mut Texts, inp: &'a Input, st: &'a mut State, size: (f32, f32)) -> Ui<'a> {
        let brush = unsafe { ctx.CreateSolidColorBrush(&col(pal::TEXT), None).unwrap() };
        let now = Instant::now();
        st.dt = now.duration_since(st.last).as_secs_f32().clamp(0.001, 0.05);
        st.last = now;
        st.hot = 0;
        st.hand = false;
        st.animating = false;
        // Click outside an open popup closes it, and that click does nothing else.
        let mut blocked = st.modal.is_some();
        if let Some(Popup::Menu { anchor, items, .. }) = &st.popup {
            let r = menu_rect(*anchor, items.len(), size);
            if r.contains(inp.mouse) {
                blocked = true;
            } else if inp.pressed {
                st.popup = None;
                st.swallow = true;
            } else {
                blocked = true;
            }
        }
        if st.swallow && !inp.down && !inp.released {
            st.swallow = false;
        }
        let _ = gfx;
        Ui { ctx, brush, texts, inp, st, x: 0.0, y: 0.0, w: 0.0, size, clip: None, blocked }
    }

    // ------------------------------------------------ drawing primitives

    pub fn fill(&self, r: Rect, c: Color, radius: f32) {
        unsafe {
            self.brush.SetColor(&col(c));
            if radius > 0.0 {
                self.ctx.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius }, &self.brush);
            } else {
                self.ctx.FillRectangle(&r.d2d(), &self.brush);
            }
        }
    }

    pub fn stroke(&self, r: Rect, c: Color, radius: f32, width: f32) {
        unsafe {
            self.brush.SetColor(&col(c));
            let r = r.inset(width / 2.0, width / 2.0);
            self.ctx.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius }, &self.brush, width, None);
        }
    }

    pub fn circle(&self, cx: f32, cy: f32, r: f32, c: Color) {
        unsafe {
            self.brush.SetColor(&col(c));
            self.ctx.FillEllipse(&D2D1_ELLIPSE { point: pt(cx, cy), radiusX: r, radiusY: r }, &self.brush);
        }
    }

    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: Color, w: f32) {
        unsafe {
            self.brush.SetColor(&col(c));
            self.ctx.DrawLine(pt(x0, y0), pt(x1, y1), &self.brush, w, None);
        }
    }

    /// Draws text with its top-left at (x, y). Returns (w, h).
    pub fn text(&mut self, f: Font, s: &str, x: f32, y: f32, c: Color) -> (f32, f32) {
        self.text_wrapped(f, s, x, y, 0.0, c)
    }

    pub fn text_wrapped(&mut self, f: Font, s: &str, x: f32, y: f32, wrap: f32, c: Color) -> (f32, f32) {
        let Some((layout, w, h)) = self.texts.get(f, s, wrap) else { return (0.0, 0.0) };
        unsafe {
            self.brush.SetColor(&col(c));
            self.ctx.DrawTextLayout(pt(x.round(), y.round()), &layout, &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
        (w, h)
    }

    pub fn measure(&mut self, f: Font, s: &str) -> (f32, f32) {
        self.texts.get(f, s, 0.0).map_or((0.0, 0.0), |(_, w, h)| (w, h))
    }

    pub fn measure_wrapped(&mut self, f: Font, s: &str, wrap: f32) -> (f32, f32) {
        self.texts.get(f, s, wrap).map_or((0.0, 0.0), |(_, w, h)| (w, h))
    }

    /// Text vertically centered in `r` at x.
    pub fn text_mid(&mut self, f: Font, s: &str, x: f32, r: Rect, c: Color) -> f32 {
        let (w, h) = self.measure(f, s);
        self.text(f, s, x, r.y + (r.h - h) / 2.0, c);
        w
    }

    pub fn glyph(&mut self, g: char, cx: f32, cy: f32, c: Color, large: bool) {
        let s = g.to_string();
        let f = if large { Font::IconLarge } else { Font::Icon };
        let (w, h) = self.measure(f, &s);
        self.text(f, &s, cx - w / 2.0, cy - h / 2.0, c);
    }

    pub fn push_clip(&self, r: Rect) {
        unsafe { self.ctx.PushAxisAlignedClip(&r.d2d(), D2D1_ANTIALIAS_MODE_ALIASED) };
    }

    pub fn pop_clip(&self) {
        unsafe { self.ctx.PopAxisAlignedClip() };
    }

    // ------------------------------------------------ interaction

    /// Smoothly approaches `target`; returns the current value.
    pub fn anim(&mut self, id: u64, target: f32, speed: f32) -> f32 {
        let dt = self.st.dt;
        let v = self.st.anims.entry(id).or_insert(target);
        let k = 1.0 - (-speed * dt).exp();
        *v += (target - *v) * k;
        if (target - *v).abs() > 0.002 {
            self.st.animating = true;
        } else {
            *v = target;
        }
        *v
    }

    /// Places an animation at its target without easing, for layouts that should track scrolling.
    pub fn snap_anim(&mut self, id: u64, target: f32) -> f32 {
        self.st.anims.insert(id, target);
        target
    }

    fn mouse_ok(&self, r: Rect) -> bool {
        !self.blocked && !self.st.swallow && r.contains(self.inp.mouse) && self.clip.is_none_or(|c| c.contains(self.inp.mouse))
    }

    /// (hovered, clicked) for a clickable area.
    pub fn interact(&mut self, id: u64, r: Rect) -> (bool, bool) {
        let over = self.mouse_ok(r);
        if over {
            self.st.hot = id;
            self.st.hand = true;
            if self.inp.pressed {
                self.st.active = id;
            }
        }
        let clicked = over && self.inp.released && self.st.active == id;
        (over, clicked)
    }

    pub fn finish(&mut self) {
        if self.inp.released {
            self.st.active = 0;
        }
        // Smooth scrolling.
        let max = (self.st.content_h - self.st.view_h).max(0.0);
        self.st.scroll_target = self.st.scroll_target.clamp(0.0, max);
        let target = self.st.scroll_target;
        let d = target - self.st.scroll;
        if d.abs() > 0.5 {
            self.st.scroll += d * (1.0 - (-18.0 * self.st.dt).exp());
            self.st.animating = true;
        } else {
            self.st.scroll = target;
        }
        if self.inp.pressed && self.st.hot == 0 && self.st.focus != 0 {
            self.st.focus = 0; // click elsewhere ends text editing
        }
        if self.st.focus != 0 {
            self.st.animating = true; // caret blink
        }
    }

    // ------------------------------------------------ containers

    /// A settings card: icon, title and description on the left; `control` gets the area on the right.
    pub fn setting(&mut self, glyph: Option<char>, title: &str, desc: &str, control_w: f32, control: impl FnOnce(&mut Ui, Rect)) {
        let text_x = self.x + if glyph.is_some() { 52.0 } else { 18.0 };
        let text_w = (self.w - (text_x - self.x) - control_w - 36.0).max(80.0);
        let (_, th) = self.measure(Font::Body, title);
        let (_, dh) = if desc.is_empty() { (0.0, 0.0) } else { self.measure_wrapped(Font::Small, desc, text_w) };
        let h = (th + dh + 30.0).max(64.0).max(if desc.is_empty() { 56.0 } else { 0.0 });
        let r = Rect::new(self.x, self.y, self.w, h);
        self.fill(r, pal::CARD, 6.0);
        self.stroke(r, pal::STROKE, 6.0, 1.0);
        if let Some(g) = glyph {
            self.glyph(g, self.x + 26.0, r.y + h / 2.0, pal::TEXT, true);
        }
        let ty = r.y + (h - th - dh - if desc.is_empty() { 0.0 } else { 2.0 }) / 2.0;
        self.text(Font::Body, title, text_x, ty, pal::TEXT);
        if !desc.is_empty() {
            self.text_wrapped(Font::Small, desc, text_x, ty + th + 2.0, text_w, pal::DIM);
        }
        let cr = Rect::new(r.right() - control_w - 18.0, r.y, control_w, h);
        control(self, cr);
        self.y += h + 4.0;
    }

    /// A blank card of height `h`; `content` draws inside it.
    pub fn card(&mut self, h: f32, content: impl FnOnce(&mut Ui, Rect)) {
        let r = Rect::new(self.x, self.y, self.w, h);
        self.fill(r, pal::CARD, 6.0);
        self.stroke(r, pal::STROKE, 6.0, 1.0);
        content(self, r);
        self.y += h + 4.0;
    }

    pub fn section(&mut self, title: &str) {
        self.y += 18.0;
        self.text(Font::Section, title, self.x + 2.0, self.y, pal::TEXT);
        self.y += 28.0;
    }

    pub fn note(&mut self, text: &str, c: Color) {
        let (_, h) = self.text_wrapped(Font::Small, text, self.x + 2.0, self.y, self.w - 4.0, c);
        self.y += h + 8.0;
    }

    // ------------------------------------------------ widgets

    pub fn toggle(&mut self, id: u64, value: &mut bool, area: Rect) -> bool {
        let (tw, th) = (40.0, 20.0);
        let label = if *value { "On" } else { "Off" };
        let (lw, _) = self.measure(Font::Body, label);
        let track = Rect::new(area.right() - tw, area.y + (area.h - th) / 2.0, tw, th);
        let hit = Rect::new(track.x - lw - 14.0, track.y - 6.0, tw + lw + 14.0, th + 12.0);
        let (hover, clicked) = self.interact(id, hit);
        if clicked {
            *value = !*value;
        }
        let on = self.anim(id, if *value { 1.0 } else { 0.0 }, 22.0);
        let hov = self.anim(id ^ 1, if hover { 1.0 } else { 0.0 }, 20.0);
        self.text_mid(Font::Body, label, track.x - lw - 12.0, track, pal::DIM);
        let track_c = pal::TRACK_OFF.lerp(pal::ACCENT, on);
        self.fill(track, track_c, th / 2.0);
        if on < 0.99 {
            self.stroke(track, pal::STROKE_STRONG.with_alpha(1.0 - on), th / 2.0, 1.0);
        }
        let r = 5.0 + 1.0 * hov;
        let cx = track.x + 10.0 + (tw - 20.0) * on;
        self.circle(cx, track.y + th / 2.0, r, pal::DIM.lerp(pal::ON_ACCENT, on));
        clicked
    }

    /// A row of options with a sliding highlight. Returns true when the choice changed.
    pub fn segmented(&mut self, id: u64, options: &[&str], value: &mut usize, area: Rect) -> bool {
        let h = 32.0;
        let pad = 3.0;
        let widths: Vec<f32> = options.iter().map(|o| self.measure(Font::Body, o).0 + 24.0).collect();
        let total: f32 = widths.iter().sum::<f32>() + pad * 2.0;
        let r = Rect::new(area.right() - total, area.y + (area.h - h) / 2.0, total, h);
        self.fill(r, pal::CONTROL, 6.0);
        let mut x = r.x + pad;
        let mut changed = false;
        let mut xs = vec![];
        for (i, w) in widths.iter().enumerate() {
            let seg = Rect::new(x, r.y + pad, *w, h - pad * 2.0);
            let (hover, clicked) = self.interact(idn(&id.to_string(), i), seg);
            if clicked && *value != i {
                *value = i;
                changed = true;
            }
            if hover && *value != i {
                self.fill(seg, pal::CONTROL_HOVER, 4.0);
            }
            xs.push(x);
            x += w;
        }
        let sel = (*value).min(options.len().saturating_sub(1));
        // These are layout coordinates, so animating them from the previous frame makes the
        // selection slide across the control when the window is resized.
        let ax = xs[sel];
        let aw = widths[sel];
        self.fill(Rect::new(ax, r.y + pad, aw, h - pad * 2.0), pal::ACCENT_SOFT, 4.0);
        self.fill(Rect::new(ax + aw / 2.0 - 8.0, r.bottom() - pad - 3.0, 16.0, 2.5), pal::ACCENT, 1.25);
        for (i, o) in options.iter().enumerate() {
            let seg = Rect::new(xs[i], r.y, widths[i], h);
            let c = if i == sel { pal::TEXT } else { pal::DIM };
            let tw = self.measure(Font::Body, o).0;
            self.text_mid(Font::Body, o, xs[i] + (widths[i] - tw) / 2.0, seg, c);
        }
        changed
    }

    /// A dropdown box. Returns true when a new item was picked.
    pub fn dropdown(&mut self, id: u64, items: &[String], value: &mut usize, area: Rect, width: f32) -> bool {
        let h = 32.0;
        let r = Rect::new(area.right() - width, area.y + (area.h - h) / 2.0, width, h);
        let (hover, clicked) = self.interact(id, r);
        let open = matches!(&self.st.popup, Some(Popup::Menu { id: pid, .. }) if *pid == id);
        self.fill(r, if hover || open { pal::CONTROL_HOVER } else { pal::CONTROL }, 5.0);
        self.stroke(r, pal::STROKE, 5.0, 1.0);
        let label = items.get(*value).cloned().unwrap_or_default();
        self.push_clip(r.inset(1.0, 1.0));
        self.text_mid(Font::Body, &label, r.x + 12.0, r, pal::TEXT);
        self.pop_clip();
        self.glyph(icon::DOWN, r.right() - 18.0, r.y + h / 2.0 + 1.0, pal::DIM, false);
        if clicked {
            self.st.popup = if open { None } else { Some(Popup::Menu { id, anchor: r, items: items.to_vec(), selected: vec![*value], multi: false }) };
        }
        if let Some((pid, i)) = self.st.picked {
            if pid == id {
                self.st.picked = None;
                if i != *value {
                    *value = i;
                    return true;
                }
            }
        }
        false
    }

    /// A dropdown that keeps the menu open while toggling multiple choices.
    pub fn multi_dropdown(&mut self, id: u64, items: &[String], selected: &mut Vec<usize>, area: Rect, width: f32) -> bool {
        let h = 32.0;
        let r = Rect::new(area.right() - width, area.y + (area.h - h) / 2.0, width, h);
        let (hover, clicked) = self.interact(id, r);
        let open = matches!(&self.st.popup, Some(Popup::Menu { id: pid, .. }) if *pid == id);
        self.fill(r, if hover || open { pal::CONTROL_HOVER } else { pal::CONTROL }, 5.0);
        self.stroke(r, pal::STROKE, 5.0, 1.0);
        let label = if selected.len() == 1 {
            items.get(selected[0]).cloned().unwrap_or_else(|| "Select graphs".into())
        } else {
            format!("{} graphs selected", selected.len())
        };
        self.push_clip(r.inset(1.0, 1.0));
        self.text_mid(Font::Body, &label, r.x + 12.0, r, pal::TEXT);
        self.pop_clip();
        self.glyph(icon::DOWN, r.right() - 18.0, r.y + h / 2.0 + 1.0, pal::DIM, false);
        if clicked {
            self.st.popup = if open { None } else { Some(Popup::Menu { id, anchor: r, items: items.to_vec(), selected: selected.clone(), multi: true }) };
        }
        let mut changed = false;
        if let Some((pid, i)) = self.st.picked {
            if pid == id {
                self.st.picked = None;
                if let Some(at) = selected.iter().position(|selected_index| *selected_index == i) {
                    if selected.len() > 1 {
                        selected.remove(at);
                        changed = true;
                    }
                } else {
                    selected.push(i);
                    changed = true;
                }
            }
        }
        if let Some(Popup::Menu { id: pid, selected: popup_selected, .. }) = &mut self.st.popup {
            if *pid == id {
                *popup_selected = selected.clone();
            }
        }
        changed
    }

    /// Opens a menu anchored at `anchor`; read the choice with `menu_choice`.
    pub fn open_menu(&mut self, id: u64, anchor: Rect, items: Vec<String>) {
        self.st.popup = Some(Popup::Menu { id, anchor, items, selected: vec![], multi: false });
    }

    pub fn menu_choice(&mut self, id: u64) -> Option<usize> {
        match self.st.picked {
            Some((pid, i)) if pid == id => {
                self.st.picked = None;
                Some(i)
            }
            _ => None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn slider(&mut self, id: u64, value: &mut f32, min: f32, max: f32, step: f32, label: &dyn Fn(f32) -> String, area: Rect, width: f32) -> bool {
        let track_w = width - 64.0;
        let track = Rect::new(area.right() - track_w, area.y + area.h / 2.0 - 2.0, track_w, 4.0);
        let hit = Rect::new(track.x - 10.0, track.y - 12.0, track.w + 20.0, 28.0);
        let (hover, _) = self.interact(id, hit);
        let mut changed = false;
        if self.st.active == id && self.inp.down {
            let t = ((self.inp.mouse.0 - track.x) / track.w).clamp(0.0, 1.0);
            let mut v = min + (max - min) * t;
            if step > 0.0 {
                v = (v / step).round() * step;
            }
            v = v.clamp(min, max);
            if (v - *value).abs() > f32::EPSILON {
                *value = v;
                changed = true;
            }
        }
        let t = ((*value - min) / (max - min)).clamp(0.0, 1.0);
        // Keep the thumb anchored to the actual value when the track changes size.
        let at = t;
        let label = label(*value);
        let (lw, _) = self.measure(Font::Body, &label);
        self.text_mid(Font::Body, &label, track.x - lw - 16.0, area, pal::DIM);
        self.fill(track, pal::TRACK_OFF, 2.0);
        self.fill(Rect::new(track.x, track.y, track.w * at, track.h), pal::ACCENT, 2.0);
        let cx = track.x + track.w * at;
        let cy = track.y + 2.0;
        let grow = self.anim(id ^ 5, if hover || self.st.active == id { 1.0 } else { 0.0 }, 20.0);
        self.circle(cx, cy, 10.0, pal::CONTROL_HOVER);
        self.circle(cx, cy, 10.0 - 0.5, pal::STROKE_STRONG);
        self.circle(cx, cy, 9.0, pal::CONTROL);
        self.circle(cx, cy, 5.0 + 1.5 * grow, pal::ACCENT);
        changed
    }

    pub fn button(&mut self, id: u64, glyph: Option<char>, label: &str, kind: ButtonKind, x: f32, y: f32) -> (bool, Rect) {
        let (tw, _) = self.measure(Font::Body, label);
        let gw = if glyph.is_some() { 24.0 } else { 0.0 };
        let r = Rect::new(x, y, tw + gw + if label.is_empty() { 16.0 } else { 28.0 }, 32.0);
        let (hover, clicked) = self.interact(id, r);
        let press = self.st.active == id && self.inp.down;
        let (bg, fg) = match kind {
            ButtonKind::Primary => (if press { pal::ACCENT.lerp(pal::BG, 0.25) } else if hover { pal::ACCENT.lerp(pal::TEXT, 0.15) } else { pal::ACCENT }, pal::ON_ACCENT),
            ButtonKind::Secondary => (if hover { pal::CONTROL_HOVER } else { pal::CONTROL }, pal::TEXT),
            ButtonKind::Subtle => (if hover { pal::CONTROL } else { Color([0.0; 4]) }, pal::TEXT),
            ButtonKind::Danger => (if hover { pal::BAD.with_alpha(0.25) } else { pal::BAD.with_alpha(0.14) }, pal::BAD),
        };
        self.fill(r, bg, 5.0);
        if kind == ButtonKind::Secondary {
            self.stroke(r, pal::STROKE, 5.0, 1.0);
        }
        let mut tx = r.x + if label.is_empty() { 8.0 } else { 14.0 };
        if let Some(g) = glyph {
            self.glyph(g, tx + 8.0, r.y + 16.0, fg, false);
            tx += gw;
        }
        if !label.is_empty() {
            self.text_mid(Font::Body, label, tx, r, fg);
        }
        (clicked, r)
    }

    /// A square icon-only button.
    pub fn icon_button(&mut self, id: u64, glyph: char, x: f32, y: f32, c: Color) -> bool {
        let r = Rect::new(x, y, 30.0, 30.0);
        let (hover, clicked) = self.interact(id, r);
        if hover {
            self.fill(r, pal::CONTROL_HOVER, 5.0);
        }
        self.glyph(glyph, r.x + 15.0, r.y + 15.0, if hover { pal::TEXT } else { c }, false);
        clicked
    }

    fn caret_on(&self) -> bool {
        (self.st.caret.elapsed().as_millis() / 530) % 2 == 0
    }

    /// Single-line text box. Edits apply live. Returns true when the text changed.
    pub fn text_field(&mut self, id: u64, value: &mut String, r: Rect, placeholder: &str) -> bool {
        let (hover, clicked) = self.interact(id, r);
        if clicked && self.st.focus != id {
            self.st.focus = id;
            self.st.edit = value.clone();
            self.st.caret = Instant::now();
        }
        let focused = self.st.focus == id;
        let mut changed = false;
        if focused {
            for ch in &self.inp.chars {
                match ch {
                    '\u{8}' => {
                        self.st.edit.pop();
                    }
                    '\r' | '\u{1b}' => self.st.focus = 0,
                    c if !c.is_control() => self.st.edit.push(*c),
                    _ => {}
                }
                self.st.caret = Instant::now();
            }
            if *value != self.st.edit {
                *value = self.st.edit.clone();
                changed = true;
            }
        }
        self.fill(r, if focused { pal::BG } else if hover { pal::CONTROL_HOVER } else { pal::CONTROL }, 5.0);
        self.stroke(r, pal::STROKE, 5.0, 1.0);
        if focused {
            self.fill(Rect::new(r.x + 1.0, r.bottom() - 2.0, r.w - 2.0, 2.0), pal::ACCENT, 1.0);
        }
        self.push_clip(r.inset(2.0, 2.0));
        let shown = if value.is_empty() && !focused { placeholder } else { value.as_str() };
        let w = self.text_mid(Font::Body, shown, r.x + 10.0, r, if value.is_empty() { pal::FAINT } else { pal::TEXT });
        if focused && self.caret_on() {
            let x = r.x + 10.0 + if value.is_empty() { 0.0 } else { w } + 1.0;
            self.fill(Rect::new(x, r.y + 8.0, 1.0, r.h - 16.0), pal::TEXT, 0.0);
        }
        self.pop_clip();
        changed
    }

    /// Click, then press a key combination. Backspace clears; Escape cancels.
    pub fn hotkey_field(&mut self, id: u64, value: &mut String, r: Rect) -> bool {
        let (hover, clicked) = self.interact(id, r);
        if clicked {
            self.st.focus = if self.st.focus == id { 0 } else { id };
        }
        let focused = self.st.focus == id;
        let mut changed = false;
        if focused {
            for k in &self.inp.keys {
                use windows::Win32::UI::Input::KeyboardAndMouse::*;
                let vk = VIRTUAL_KEY(k.vk as u16);
                if vk == VK_ESCAPE {
                    self.st.focus = 0;
                } else if vk == VK_BACK || vk == VK_DELETE {
                    value.clear();
                    changed = true;
                    self.st.focus = 0;
                } else if let Some(name) = key_name(k.vk).filter(|_| k.ctrl || k.alt || k.shift || k.win || standalone_ok(k.vk)) {
                    let mut s = String::new();
                    for (on, m) in [(k.ctrl, "Ctrl"), (k.alt, "Alt"), (k.shift, "Shift"), (k.win, "Win")] {
                        if on {
                            s.push_str(m);
                            s.push('+');
                        }
                    }
                    s.push_str(&name);
                    *value = s;
                    changed = true;
                    self.st.focus = 0;
                }
            }
        }
        self.fill(r, if focused { pal::ACCENT_SOFT } else if hover { pal::CONTROL_HOVER } else { pal::CONTROL }, 5.0);
        self.stroke(r, if focused { pal::ACCENT } else { pal::STROKE }, 5.0, 1.0);
        self.glyph(icon::KEYBOARD, r.x + 18.0, r.y + r.h / 2.0, pal::DIM, false);
        let label = if focused {
            "Press keys, e.g. Ctrl + Shift + O".to_string()
        } else if value.is_empty() {
            "Not set".to_string()
        } else {
            value.replace('+', " + ")
        };
        self.text_mid(Font::Body, &label, r.x + 36.0, r, if value.is_empty() && !focused { pal::FAINT } else { pal::TEXT });
        changed
    }

    /// A pill. Returns (clicked, remove clicked).
    pub fn chip(&mut self, id: u64, label: &str, color: Color, removable: bool, x: f32, y: f32) -> (bool, bool, f32) {
        let (tw, _) = self.measure(Font::Body, label);
        let w = tw + 26.0 + if removable { 22.0 } else { 0.0 };
        let r = Rect::new(x, y, w, 30.0);
        let (hover, clicked) = self.interact(id, Rect::new(r.x, r.y, r.w - if removable { 26.0 } else { 0.0 }, r.h));
        self.fill(r, if hover { pal::CONTROL_HOVER } else { pal::CONTROL }, 15.0);
        self.circle(r.x + 13.0, r.y + 15.0, 3.5, color);
        self.text_mid(Font::Body, label, r.x + 22.0, r, pal::TEXT);
        let mut removed = false;
        if removable {
            let xr = Rect::new(r.right() - 27.0, r.y + 3.0, 24.0, 24.0);
            let (xh, xc) = self.interact(id ^ 0xABCD, xr);
            if xh {
                self.fill(xr, pal::BAD.with_alpha(0.2), 12.0);
            }
            self.glyph(icon::CLOSE, xr.x + 12.0, xr.y + 12.0, if xh { pal::BAD } else { pal::FAINT }, false);
            removed = xc;
        }
        (clicked, removed, w)
    }

    // ------------------------------------------------ overlays (popups, modal frame)

    /// Draws the open popup (call last) and handles its clicks.
    pub fn popups(&mut self) {
        let Some(Popup::Menu { id, anchor, items, selected, multi }) = &self.st.popup else { return };
        let (id, anchor, items, selected, multi) = (*id, *anchor, items.clone(), selected.clone(), *multi);
        let r = menu_rect(anchor, items.len(), self.size);
        // soft shadow
        for i in 1..6 {
            let f = i as f32;
            self.fill(Rect::new(r.x - f, r.y - f + 3.0, r.w + f * 2.0, r.h + f * 2.0), Color([0.0, 0.0, 0.0, 0.05]), 8.0 + f);
        }
        self.fill(r, pal::POPUP, 8.0);
        self.stroke(r, pal::STROKE_STRONG, 8.0, 1.0);
        let saved = self.blocked;
        self.blocked = false;
        for (i, it) in items.iter().enumerate() {
            let ir = Rect::new(r.x + 4.0, r.y + 4.0 + i as f32 * 34.0, r.w - 8.0, 32.0);
            let (hover, clicked) = self.interact(idn("menu-item", i), ir);
            if hover {
                self.fill(ir, pal::CONTROL_HOVER, 4.0);
            }
            if selected.contains(&i) {
                if multi {
                    self.glyph(icon::CHECK, ir.right() - 18.0, ir.y + 16.0, pal::ACCENT, false);
                } else {
                    self.fill(Rect::new(ir.x, ir.y + 9.0, 3.0, 14.0), pal::ACCENT, 1.5);
                }
            }
            self.text_mid(Font::Body, it, ir.x + 14.0, ir, pal::TEXT);
            if clicked {
                self.st.picked = Some((id, i));
                if !multi {
                    self.st.popup = None;
                }
                self.st.animating = true;
            }
        }
        self.blocked = saved;
    }

    /// Dims the window and draws a centered sheet; returns its content rect. Unblocks input inside.
    pub fn modal_frame(&mut self, w: f32, h: f32, title: &str) -> (Rect, bool) {
        self.fill(Rect::new(0.0, 0.0, self.size.0, self.size.1), pal::SCRIM, 0.0);
        let r = Rect::new(((self.size.0 - w) / 2.0).round(), ((self.size.1 - h) / 2.0).round(), w, h);
        for i in 1..8 {
            let f = i as f32 * 1.5;
            self.fill(Rect::new(r.x - f, r.y - f + 4.0, r.w + f * 2.0, r.h + f * 2.0), Color([0.0, 0.0, 0.0, 0.05]), 10.0 + f);
        }
        self.fill(r, pal::BG, 10.0);
        self.stroke(r, pal::STROKE_STRONG, 10.0, 1.0);
        self.blocked = false;
        self.text(Font::Section, title, r.x + 24.0, r.y + 20.0, pal::TEXT);
        let close = self.icon_button(id("modal-close"), icon::CLOSE, r.right() - 44.0, r.y + 14.0, pal::DIM);
        (Rect::new(r.x + 24.0, r.y + 56.0, r.w - 48.0, r.h - 76.0), close)
    }
}

fn menu_rect(anchor: Rect, n: usize, size: (f32, f32)) -> Rect {
    let h = n as f32 * 34.0 + 8.0;
    let w = anchor.w.max(160.0);
    let mut y = anchor.bottom() + 4.0;
    if y + h > size.1 - 8.0 {
        y = (anchor.y - h - 4.0).max(8.0);
    }
    Rect::new(anchor.right() - w, y, w, h)
}

/// Keys fine to use as a hotkey on their own (a bare letter or arrow would be taken from every app).
fn standalone_ok(vk: u32) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    (VK_F1.0 as u32..=VK_F24.0 as u32).contains(&vk) || vk == VK_PAUSE.0 as u32 || vk == VK_SCROLL.0 as u32
}

/// Names hotkeys the same way the config does ("F10", "O", "PageUp", ...).
pub fn key_name(vk: u32) -> Option<String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    let v = VIRTUAL_KEY(vk as u16);
    let named = match v {
        VK_HOME => "Home",
        VK_END => "End",
        VK_INSERT => "Insert",
        VK_PRIOR => "PageUp",
        VK_NEXT => "PageDown",
        VK_UP => "Up",
        VK_DOWN => "Down",
        VK_LEFT => "Left",
        VK_RIGHT => "Right",
        VK_SPACE => "Space",
        VK_TAB => "Tab",
        VK_PAUSE => "Pause",
        VK_SCROLL => "ScrollLock",
        VK_OEM_3 => "Backquote",
        _ => "",
    };
    if !named.is_empty() {
        return Some(named.into());
    }
    if (VK_F1.0 as u32..=VK_F24.0 as u32).contains(&vk) {
        return Some(format!("F{}", vk - VK_F1.0 as u32 + 1));
    }
    if (VK_NUMPAD0.0 as u32..=VK_NUMPAD9.0 as u32).contains(&vk) {
        return Some(format!("Num{}", vk - VK_NUMPAD0.0 as u32));
    }
    let c = char::from_u32(vk)?;
    (c.is_ascii_uppercase() || c.is_ascii_digit()).then(|| c.to_string())
}
