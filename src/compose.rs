//! Builds what to draw for one surface (a monitor, or a game's back buffer): the stats panel and
//! the positioned RTSS layers, with their positions inside an area of the given size.

use crate::config::Config;
use crate::osd;
use crate::overlay::gfx::Fonts;
use crate::overlay::layout::{self, Scene, Sticky};
use crate::theme::{Layout, Theme};
use std::collections::{HashMap, VecDeque};

/// Everything measured this tick, shared by every surface.
#[derive(Default, Clone)]
pub struct Frame {
    pub values: HashMap<String, f64>,
    pub cores: Vec<f64>,
    pub history: HashMap<String, VecDeque<f64>>,
    pub frametimes: Vec<f32>,
    pub app: String,
    pub fps_note: Option<String>,
    pub osd: Vec<osd::Layer>,
}

pub struct Placed {
    pub panel: Option<(Scene, (f32, f32))>,
    pub layers: Vec<(Scene, (f32, f32))>,
}

/// Just the stats panel (no position).
pub fn panel(cfg: &Config, theme: &Theme, layout_kind: Layout, scale: f32, frame: &Frame, fonts: &Fonts, sticky: &mut Sticky) -> Option<Scene> {
    panel_with(cfg, theme, layout_kind, scale, frame, fonts, sticky, false)
}

/// The panel; `live` leaves FPS, frame time and its graph for RTSS to draw every frame.
#[allow(clippy::too_many_arguments)]
pub fn panel_with(cfg: &Config, theme: &Theme, layout_kind: Layout, scale: f32, frame: &Frame, fonts: &Fonts, sticky: &mut Sticky, live: bool) -> Option<Scene> {
    let has_content = !cfg.rows.is_empty() || frame.osd.iter().any(|l| !l.positioned());
    if !has_content {
        return None;
    }
    let inputs = layout::Inputs {
        cfg,
        theme,
        layout: layout_kind,
        scale,
        values: &frame.values,
        cores: &frame.cores,
        history: &frame.history,
        frametimes: &frame.frametimes,
        app: &frame.app,
        fps_note: frame.fps_note.as_deref(),
        osd: &frame.osd,
        live,
    };
    Some(layout::build(&inputs, fonts, sticky))
}

#[allow(clippy::too_many_arguments)]
pub fn compose(cfg: &Config, theme: &Theme, layout_kind: Layout, scale: f32, frame: &Frame, fonts: &Fonts, sticky: &mut Sticky, area: (f32, f32)) -> Placed {
    let (aw, ah) = area;
    let panel = panel(cfg, theme, layout_kind, scale, frame, fonts, sticky).map(|p| {
        let (ax, ay) = cfg.position.anchors();
        let (ox, oy) = (cfg.offset_x as f32 * scale, cfg.offset_y as f32 * scale);
        let x = match ax {
            0 => ox,
            1 => (aw - p.w) / 2.0,
            _ => aw - p.w - ox,
        };
        let y = match ay {
            0 => oy,
            1 => (ah - p.h) / 2.0,
            _ => ah - p.h - oy,
        };
        (p, (x, y))
    });
    let layers = frame
        .osd
        .iter()
        .filter(|l| l.positioned())
        .map(|l| {
            let sc = layout::osd_layer(l, fonts, theme);
            let pos = osd_position(l, &sc, area);
            (sc, pos)
        })
        .collect();
    Placed { panel, layers }
}

/// Where a positioned RTSS layer goes: one of 9 spots (flush to the edges, as RTSS does), or
/// absolute coordinates (negative = from the right/bottom).
fn osd_position(l: &osd::Layer, sc: &Scene, (aw, ah): (f32, f32)) -> (f32, f32) {
    if let Some(spot) = l.spot {
        let (col, row) = (spot % 3, spot / 3);
        let x = match col {
            0 => 0.0,
            1 => (aw - sc.w) / 2.0,
            _ => aw - sc.w,
        };
        let y = match row {
            0 => 0.0,
            1 => (ah - sc.h) / 2.0,
            _ => ah - sc.h,
        };
        return (x, y);
    }
    let (x, y) = l.pos.unwrap_or((0.0, 0.0));
    (if x < 0.0 { aw + x - sc.w } else { x }, if y < 0.0 { ah + y - sc.h } else { y })
}
