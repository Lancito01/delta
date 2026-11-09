//! Turns rows + values + theme into positioned primitives (a Scene). Measuring happens here; the
//! drawing code just paints what it's given.

use super::gfx::{Fonts, Role, TextBox};
use crate::config::{Config, Display, Row, ValueWidth};
use crate::metrics;
use crate::osd;
use crate::theme::{Color, ColorSource, GraphStyle, Layout, Theme};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub enum Prim {
    Text { x: f32, y: f32, tb: TextBox, color: Color, fx: bool },
    Graph { r: Rect, values: Vec<f32>, slots: usize, min: f32, max: f32, color: Color, style: GraphStyle },
    Bars { r: Rect, values: Vec<f32>, color: Color },
    Fill { r: Rect, color: Color, radius: f32 },
    Image { r: Rect, path: String, src: Option<[f32; 4]> },
}

pub struct Scene {
    pub w: f32,
    pub h: f32,
    /// Draw the theme's panel behind the content.
    pub panel: bool,
    pub prims: Vec<Prim>,
    /// Where live values were left out (drawing through RTSS, which fills them in every frame).
    pub live: Live,
}

/// Slots left empty for RTSS to fill in every frame.
#[derive(Default, Clone, Debug)]
pub struct Live {
    /// Numbers RTSS draws live (their units stay in the picture).
    pub cells: Vec<LiveCell>,
    /// The frame time graph's area and color.
    pub graph: Option<(Rect, Color)>,
}

/// One live number: right-aligned to `right`, its text box's top at `top`.
#[derive(Clone, Debug)]
pub struct LiveCell {
    pub id: String,
    pub right: f32,
    pub top: f32,
    /// Characters of room the panel kept for it.
    pub chars: usize,
    /// In the big "hero" size (else the normal value size).
    pub hero: bool,
    pub color: Color,
}

pub struct Inputs<'a> {
    pub cfg: &'a Config,
    pub theme: &'a Theme,
    pub layout: Layout,
    pub scale: f32,
    pub values: &'a HashMap<String, f64>,
    pub cores: &'a [f64],
    pub history: &'a HashMap<String, VecDeque<f64>>,
    pub frametimes: &'a [f32],
    pub app: &'a str,
    #[allow(dead_code)]
    pub fps_note: Option<&'a str>,
    pub osd: &'a [osd::Layer],
    /// Leave FPS, frame time and the frame time graph empty (RTSS draws them live).
    pub live: bool,
}

/// Column widths that grow at once but shrink only after staying smaller for a while,
/// so the panel doesn't twitch as numbers change.
#[derive(Default)]
pub struct Sticky {
    widths: HashMap<String, (f32, Instant)>,
}

impl Sticky {
    pub fn clear(&mut self) {
        self.widths.clear();
    }
    fn get(&mut self, key: String, w: f32) -> f32 {
        let now = Instant::now();
        let e = self.widths.entry(key).or_insert((w, now));
        if w >= e.0 - 0.01 {
            *e = (w, now);
        } else if now.duration_since(e.1) > Duration::from_secs(3) {
            *e = (w, now);
        }
        e.0
    }
}

struct Cell {
    id: String,
    value: TextBox,
    unit: Option<TextBox>,
    color: Color,
    /// Room reserved for the value and the unit (fixed spacing), at least the actual widths.
    min_value: f32,
    min_unit: f32,
    /// Drawn live by RTSS: the space is kept, the text isn't painted.
    live: Option<String>,
}

impl Cell {
    fn vw(&self) -> f32 {
        self.value.w.max(self.min_value)
    }
    fn uw(&self) -> f32 {
        self.unit.as_ref().map_or(0.0, |u| u.w).max(self.min_unit)
    }
}

/// Gaps after applying the user's spacing overrides to the theme's.
struct Gaps {
    column: f32,
    unit: f32,
    row: f32,
    label: f32,
}

fn gaps(inp: &Inputs) -> Gaps {
    let (t, sp, s) = (inp.theme, &inp.cfg.spacing, inp.scale);
    Gaps {
        column: sp.column_gap.unwrap_or(t.shape.column_gap) * s,
        unit: sp.unit_gap.unwrap_or(3.0) * s,
        row: sp.row_gap.unwrap_or(t.shape.row_gap) * s,
        label: sp.label_gap.unwrap_or(t.shape.label_gap) * s,
    }
}

enum Extra {
    Graph { id: String, color: Color, after: Option<String>, heading: Option<Cell> },
    Cores { color: Color },
}

struct BuiltRow {
    label: Option<TextBox>,
    group_color: Color,
    label_color: Color,
    cells: Vec<Cell>,
    hero: bool,
    extras: Vec<Extra>,
    graph_max: Option<f64>,
}

pub fn threshold_color(cfg: &Config, theme: &Theme, id: &str, v: f64) -> Option<Color> {
    let [warn, crit] = *cfg.thresholds.get(id)?;
    let (is_crit, is_warn) = if warn <= crit { (v >= crit, v >= warn) } else { (v <= crit, v <= warn) };
    if is_crit {
        Some(theme.colors.crit)
    } else if is_warn {
        Some(theme.colors.warn)
    } else {
        None
    }
}

fn now_hhmm() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    format!("{:02}:{:02}", t.wHour, t.wMinute)
}

fn build_row(row: &Row, inp: &Inputs, fonts: &Fonts) -> BuiltRow {
    let theme = inp.theme;
    let first = row.metrics.first().map(String::as_str).unwrap_or("");
    let group = row.group.clone().unwrap_or_else(|| metrics::group_of(first).to_string());
    let group_color = row.color.unwrap_or_else(|| theme.group_color(&group));
    let label_color = match theme.label_color {
        ColorSource::Group => group_color,
        ColorSource::Plain => theme.colors.label,
    };
    let label_text = {
        let l = if fonts.uppercase_labels { row.label.to_uppercase() } else { row.label.clone() };
        if l.is_empty() { l } else { format!("{}{}", theme.text.label_prefix, l) }
    };
    let label = if label_text.is_empty() { None } else { fonts.text(Role::Label, &label_text) };
    // In a stacked "numbers and graph" row, a graphed value moves into the heading above its
    // graph. It's drawn at the normal size there, so the row has no big first number then.
    let first_headed = inp.layout == Layout::Stack && row.display == Display::Both && (row.graph.is_empty() || row.graph.iter().any(|g| g == first));
    let hero = first == "fps" && theme.font.hero_size > theme.font.size && !first_headed;

    let fixed = inp.cfg.spacing.values == ValueWidth::Fixed;
    let mut cells = vec![];
    let mut extras = vec![];
    let graph_color = match theme.graph_color {
        ColorSource::Group => group_color,
        ColorSource::Plain => theme.colors.accent,
    };
    let plain_value = match theme.value_color {
        ColorSource::Group => group_color,
        ColorSource::Plain => theme.colors.value,
    };
    for (i, id) in row.metrics.iter().enumerate() {
        let role = if hero && i == 0 { Role::Hero } else { Role::Value };
        match id.as_str() {
            "cpu.cores" => {
                if !inp.cores.is_empty() {
                    extras.push(Extra::Cores { color: graph_color });
                }
            }
            "app" => {
                if let Some(v) = fonts.text(role, if inp.app.is_empty() { "—" } else { inp.app }) {
                    cells.push(Cell { id: id.clone(), value: v, unit: None, color: plain_value, min_value: 0.0, min_unit: 0.0, live: None });
                }
            }
            "time" => {
                if let Some(v) = fonts.text(role, &now_hhmm()) {
                    let min_value = if fixed { fonts.width(role, "00:00") } else { 0.0 };
                    cells.push(Cell { id: id.clone(), value: v, unit: None, color: plain_value, min_value, min_unit: 0.0, live: None });
                }
            }
            _ => match inp.values.get(id) {
                Some(&v) => {
                    let f = metrics::format(id, v, inp.cfg.temperature);
                    let color = threshold_color(inp.cfg, theme, id, v).unwrap_or(plain_value);
                    if let Some(tb) = fonts.text(role, &f.value) {
                        let unit = if f.unit.is_empty() { None } else { fonts.text(Role::Unit, &f.unit) };
                        let (min_value, min_unit) = template_widths(fonts, role, id, fixed);
                        let live = is_live(inp, id).then(|| id.clone());
                        cells.push(Cell { id: id.clone(), value: tb, unit, color, min_value, min_unit, live });
                    }
                }
                None => {
                    // Missing values are left out: no game means no FPS to show, and hardware the
                    // machine doesn't report has nothing to show either. Only drawing through RTSS
                    // keeps a slot, which RTSS fills in live.
                    if group == "fps" && is_live(inp, id) {
                        if let Some(tb) = fonts.text(role, "—") {
                            let (min_value, min_unit) = template_widths(fonts, role, id, fixed);
                            let unit = fonts.text(Role::Unit, &metrics::format(id, 0.0, inp.cfg.temperature).unit);
                            let live = is_live(inp, id).then(|| id.clone());
                            cells.push(Cell { id: id.clone(), value: tb, unit, color: theme.colors.unit, min_value, min_unit, live });
                        }
                    }
                }
            },
        }
    }
    if matches!(row.display, Display::Graph | Display::Both) {
        let graph_ids = if row.graph.is_empty() { vec![first.to_string()] } else { row.graph.clone() };
        for gid in graph_ids {
            if gid.is_empty() {
                continue;
            }
            // A frame graph with no game to measure would just be an empty box.
            let no_frames = metrics::group_of(&gid) == "fps" && inp.frametimes.is_empty() && !inp.values.contains_key("fps") && !inp.live;
            // Likewise a value that can't be read right now (e.g. the CPU temperature with its
            // driver paused): its old history would otherwise linger as a frozen graph.
            let no_reading = metrics::group_of(&gid) != "fps" && !inp.values.contains_key(&gid);
            if !no_frames && !no_reading {
                let after = if gid == "frametime" && row.metrics.iter().any(|metric| metric.starts_with("fps")) {
                    row.metrics.iter().find(|metric| metric.starts_with("fps")).cloned()
                } else {
                    Some(gid.clone())
                };
                extras.push(Extra::Graph { id: gid, color: graph_color, after, heading: None });
            }
        }
        if inp.layout == Layout::Stack && row.display == Display::Both {
            for extra in &mut extras {
                if let Extra::Graph { id, heading, .. } = extra {
                    if let Some(index) = cells.iter().position(|cell| &cell.id == id) {
                        *heading = Some(cells.remove(index));
                    } else {
                        *heading = graph_heading_cell(id, inp, fonts, fixed, plain_value, theme);
                    }
                }
            }
        }
        if row.display == Display::Graph {
            cells.clear();
        }
    }
    BuiltRow { label, group_color, label_color, cells, hero, extras, graph_max: row.graph_max }
}

fn graph_heading_cell(id: &str, inp: &Inputs, fonts: &Fonts, fixed: bool, plain_value: Color, theme: &Theme) -> Option<Cell> {
    let live = is_live(inp, id).then(|| id.to_string());
    let (value, unit, color) = if let Some(&v) = inp.values.get(id) {
        let formatted = metrics::format(id, v, inp.cfg.temperature);
        let value = fonts.text(Role::Value, &formatted.value)?;
        let unit = if formatted.unit.is_empty() { None } else { fonts.text(Role::Unit, &formatted.unit) };
        let color = threshold_color(inp.cfg, theme, id, v).unwrap_or(plain_value);
        (value, unit, color)
    } else if live.is_some() {
        let (_, unit) = metrics::template(id);
        let value = fonts.text(Role::Value, "—")?;
        let unit = if unit.is_empty() { None } else { fonts.text(Role::Unit, unit) };
        (value, unit, theme.colors.unit)
    } else {
        return None;
    };
    let (min_value, min_unit) = template_widths(fonts, Role::Value, id, fixed);
    Some(Cell { id: id.to_string(), value, unit, color, min_value, min_unit, live })
}

/// Values RTSS can draw live, every frame.
fn is_live(inp: &Inputs, id: &str) -> bool {
    inp.live && (id == "fps" || id == "frametime")
}

fn template_widths(fonts: &Fonts, role: Role, id: &str, fixed: bool) -> (f32, f32) {
    if !fixed {
        return (0.0, 0.0);
    }
    let (v, u) = metrics::template(id);
    (fonts.width(role, v), fonts.width(Role::Unit, u))
}

fn graph_values(inp: &Inputs, id: &str, fixed_max: Option<f64>) -> (Vec<f32>, usize, f32, f32) {
    if id == "frametime" && inp.live {
        return (vec![], inp.cfg.frametime_samples, 0.0, 1.0);
    }
    if id == "frametime" {
        let v = inp.frametimes.to_vec();
        // Scale to the slowest recent frame with headroom, snapped so it doesn't breathe constantly.
        let peak = v.iter().cloned().fold(0.0f32, f32::max);
        let max = [8.0f32, 16.7, 33.4, 50.0, 66.7, 100.0, 200.0, 500.0].into_iter().find(|m| *m >= peak * 1.15).unwrap_or(peak * 1.15);
        return (v, inp.cfg.frametime_samples, 0.0, max);
    }
    let v: Vec<f32> = inp.history.get(id).map(|h| h.iter().map(|x| *x as f32).collect()).unwrap_or_default();
    let max = fixed_max
        .or_else(|| metrics::graph_max(id))
        .map(|m| m as f32)
        .unwrap_or_else(|| (v.iter().cloned().fold(0.0f32, f32::max) * 1.15).max(1.0));
    (v, inp.cfg.history, 0.0, max)
}

pub fn build(inp: &Inputs, fonts: &Fonts, sticky: &mut Sticky) -> Scene {
    // Rows left with nothing to show (e.g. FPS while no game is running) are dropped.
    let rows: Vec<BuiltRow> = inp.cfg.rows.iter().map(|r| build_row(r, inp, fonts)).filter(|r| !r.cells.is_empty() || !r.extras.is_empty()).collect();
    match inp.layout {
        Layout::Stack => stack(inp, fonts, rows, sticky),
        Layout::Bar => bar(inp, fonts, rows, sticky),
    }
}

fn stack(inp: &Inputs, fonts: &Fonts, rows: Vec<BuiltRow>, sticky: &mut Sticky) -> Scene {
    let t = inp.theme;
    let s = inp.scale;
    let (px, py) = (t.shape.padding_x * s, t.shape.padding_y * s);
    let g = gaps(inp);
    let col_gap = g.column;
    let unit_gap = g.unit;
    let line_gap = 3.0 * s;
    let row_gap = g.row;
    let mut prims = vec![];

    let any_label = rows.iter().any(|r| r.label.is_some());
    let label_w = if any_label {
        let w = rows.iter().filter_map(|r| r.label.as_ref().map(|l| l.w)).fold(t.shape.label_width * s, f32::max);
        sticky.get("label".into(), w)
    } else {
        0.0
    };
    let content_x = px + if any_label { label_w + g.label } else { 0.0 };

    // Shared columns across the regular (non-hero) rows.
    let ncols = rows.iter().filter(|r| !r.hero).map(|r| r.cells.len()).max().unwrap_or(0);
    let mut val_w = vec![0.0f32; ncols];
    let mut unit_w = vec![0.0f32; ncols];
    for r in rows.iter().filter(|r| !r.hero) {
        for (j, c) in r.cells.iter().enumerate() {
            val_w[j] = val_w[j].max(c.vw());
            unit_w[j] = unit_w[j].max(c.uw());
        }
    }
    for j in 0..ncols {
        val_w[j] = sticky.get(format!("v{j}"), val_w[j]);
        unit_w[j] = sticky.get(format!("u{j}"), unit_w[j]);
    }

    // Widths first, so graphs can span the panel.
    let mut text_right = content_x;
    for r in &rows {
        let right = if r.hero {
            content_x + r.cells.iter().map(|c| c.vw() + if c.uw() > 0.0 { unit_gap + c.uw() } else { 0.0 }).sum::<f32>() + col_gap * r.cells.len().saturating_sub(1) as f32
        } else {
            let mut x = content_x;
            for j in 0..r.cells.len() {
                x += val_w[j] + if unit_w[j] > 0.0 { unit_gap + unit_w[j] } else { 0.0 } + col_gap;
            }
            x - if r.cells.is_empty() { 0.0 } else { col_gap }
        };
        text_right = text_right.max(right);
    }
    let cores_w = |n: usize| n as f32 * (t.cores.bar_width + t.cores.bar_gap) * s - t.cores.bar_gap * s;
    let mut content_w = text_right - content_x;
    if rows.iter().any(|r| r.extras.iter().any(|e| matches!(e, Extra::Cores { .. }))) {
        content_w = content_w.max(cores_w(inp.cores.len()));
    }
    let graph_w = if t.graph.width > 0.0 { t.graph.width * s } else { content_w.max(150.0 * s) };
    for e in rows.iter().flat_map(|r| &r.extras) {
        if let Extra::Graph { id, heading, .. } = e {
            // The graph, and the "name value unit" heading above it, both have to fit the panel.
            let mut w = fonts.width(Role::Value, &metrics::display_name(id));
            if let Some(c) = heading {
                w += g.column * 0.55 + c.vw() + c.unit.as_ref().map_or(0.0, |u| g.unit + u.w);
            }
            content_w = content_w.max(graph_w).max(w);
        }
    }

    let mut live = Live::default();
    let mut y = py;
    let header = if t.text.header.is_empty() { None } else { fonts.text(Role::Header, &t.text.header) };
    let mut width = content_x + content_w + px;
    if let Some(h) = header {
        width = width.max(px * 2.0 + h.w);
        let hh = h.h;
        prims.push(Prim::Text { x: px, y, tb: h, color: t.colors.accent, fx: true });
        y += hh + row_gap * 0.6;
        prims.push(Prim::Fill { r: Rect { x: px, y, w: width - px * 2.0, h: s.max(1.0) }, color: t.colors.separator, radius: 0.0 });
        y += row_gap + s;
    }

    for (ri, r) in rows.iter().enumerate() {
        let row_top = y;
        // Text line, everything on one baseline.
        let mut base = r.label.as_ref().map_or(0.0, |l| l.baseline);
        let mut line_h = r.label.as_ref().map_or(0.0, |l| l.h);
        for c in &r.cells {
            base = base.max(c.value.baseline);
            if let Some(u) = &c.unit {
                base = base.max(u.baseline);
            }
        }
        for c in &r.cells {
            line_h = line_h.max(base - c.value.baseline + c.value.h);
            if let Some(u) = &c.unit {
                line_h = line_h.max(base - u.baseline + u.h);
            }
        }
        if let Some(l) = &r.label {
            line_h = line_h.max(base - l.baseline + l.h);
            prims.push(Prim::Text { x: px, y: y + base - l.baseline, tb: l.clone(), color: r.label_color, fx: false });
        }
        let mut x = content_x;
        for (j, c) in r.cells.iter().enumerate() {
            let (vw, uw) = if r.hero { (c.vw(), c.uw()) } else { (val_w[j], unit_w[j]) };
            if let Some(id) = &c.live {
                note_live(&mut live, id, x + vw, y + base - c.value.baseline, r.hero && j == 0, c.color);
            } else {
                prims.push(Prim::Text { x: x + vw - c.value.w, y: y + base - c.value.baseline, tb: c.value.clone(), color: c.color, fx: true });
            }
            if let Some(u) = &c.unit {
                prims.push(Prim::Text { x: x + vw + unit_gap, y: y + base - u.baseline, tb: u.clone(), color: t.colors.unit, fx: false });
            }
            x += vw + if uw > 0.0 { unit_gap + uw } else { 0.0 } + col_gap;
        }
        if !r.cells.is_empty() || r.label.is_some() && r.extras.is_empty() {
            y += line_h;
        }

        // Graphs / core bars under the text (or beside the label when there's no text).
        for (k, e) in r.extras.iter().enumerate() {
            let on_label_line = r.cells.is_empty() && k == 0;
            if !on_label_line {
                y += line_gap;
            }
            let h = match e {
                Extra::Graph { id, color, heading, .. } => {
                    let graph_h = t.graph.height * s;
                    let name = fonts.text(Role::Value, &metrics::display_name(id));
                    let mut heading_base = name.as_ref().map_or(0.0, |text| text.baseline);
                    let mut heading_h = name.as_ref().map_or(0.0, |text| text.h);
                    if let Some(value) = heading {
                        heading_base = heading_base.max(value.value.baseline);
                        heading_h = heading_h.max(value.value.h);
                        if let Some(unit) = &value.unit {
                            heading_base = heading_base.max(unit.baseline);
                            heading_h = heading_h.max(unit.h);
                        }
                    }
                    let heading_gap = if heading_h > 0.0 { line_gap } else { 0.0 };
                    let h = heading_h + heading_gap + graph_h;
                    if on_label_line {
                        if r.label.is_some() {
                            if let Some(Prim::Text { y: ly, tb, .. }) = prims.iter_mut().rev().find(|p| matches!(p, Prim::Text { .. })) {
                                *ly = y + (h - tb.h) / 2.0;
                            }
                        }
                    }
                    if let Some(name) = name {
                        prims.push(Prim::Text { x: content_x, y: y + heading_base - name.baseline, tb: name.clone(), color: t.colors.unit, fx: false });
                        let mut heading_x = content_x + name.w + g.column * 0.55;
                        if let Some(value) = heading {
                            if let Some(id) = &value.live {
                                note_live(&mut live, id, heading_x + value.vw(), y + heading_base - value.value.baseline, false, value.color);
                            } else {
                                prims.push(Prim::Text {
                                    x: heading_x + value.vw() - value.value.w,
                                    y: y + heading_base - value.value.baseline,
                                    tb: value.value.clone(),
                                    color: value.color,
                                    fx: true,
                                });
                            }
                            heading_x += value.vw();
                            if let Some(unit) = &value.unit {
                                prims.push(Prim::Text {
                                    x: heading_x + g.unit,
                                    y: y + heading_base - unit.baseline,
                                    tb: unit.clone(),
                                    color: t.colors.unit,
                                    fx: false,
                                });
                            }
                        }
                    }
                    let (values, slots, min, max) = graph_values(inp, id, r.graph_max);
                    let rect = Rect { x: content_x, y: y + heading_h + heading_gap, w: graph_w, h: graph_h };
                    if inp.live && id == "frametime" {
                        live.graph = Some((rect, *color));
                    }
                    prims.push(Prim::Graph { r: rect, values, slots, min, max, color: *color, style: t.graph.style });
                    h
                }
                Extra::Cores { color } => {
                    let h = t.cores.height * s;
                    if on_label_line && r.label.is_some() {
                        if let Some(Prim::Text { y: ly, tb, .. }) = prims.iter_mut().rev().find(|p| matches!(p, Prim::Text { .. })) {
                            *ly = y + (h - tb.h) / 2.0;
                        }
                    }
                    prims.push(Prim::Bars { r: Rect { x: content_x, y, w: cores_w(inp.cores.len()), h }, values: inp.cores.iter().map(|v| *v as f32).collect(), color: *color });
                    h
                }
            };
            y += h;
        }
        if t.shape.row_marker {
            prims.push(Prim::Fill { r: Rect { x: (px * 0.45 - s).max(1.0), y: row_top + 2.0 * s, w: 2.0 * s, h: (y - row_top - 4.0 * s).max(2.0) }, color: r.group_color, radius: s });
        }
        if ri + 1 < rows.len() {
            y += row_gap;
        }
    }

    // RTSS clients' plain text, under Delta's own rows.
    let flowing: Vec<&osd::Layer> = inp.osd.iter().filter(|l| !l.positioned()).collect();
    if !flowing.is_empty() {
        if !rows.is_empty() {
            y += row_gap;
            prims.push(Prim::Fill { r: Rect { x: px, y, w: width - px * 2.0, h: s.max(1.0) }, color: t.colors.separator, radius: 0.0 });
            y += row_gap;
        }
        for layer in flowing {
            let (w, h) = flow(&layer.items, px, y, fonts, t.colors.value, &mut prims);
            width = width.max(px * 2.0 + w);
            y += h;
        }
    }

    Scene { w: width.ceil(), h: (y + py).ceil(), panel: true, prims, live }
}

fn note_live(live: &mut Live, id: &str, right: f32, top: f32, hero: bool, color: Color) {
    let chars = metrics::template(id).0.chars().count().max(1);
    live.cells.push(LiveCell { id: id.to_string(), right, top, chars, hero, color });
}

fn bar(inp: &Inputs, fonts: &Fonts, rows: Vec<BuiltRow>, sticky: &mut Sticky) -> Scene {
    let t = inp.theme;
    let s = inp.scale;
    let (px, py) = (t.shape.padding_x * s, t.shape.padding_y * s);
    let g = gaps(inp);
    let gap = g.column;
    let unit_gap = g.unit * 0.7;
    let sep = if t.text.separator.is_empty() { None } else { fonts.text(Role::Unit, &t.text.separator) };

    // One shared baseline; graphs and bars are centered on the line.
    let mut base: f32 = 0.0;
    let mut below: f32 = 0.0;
    for r in &rows {
        for tb in r.label.iter().chain(r.cells.iter().map(|c| &c.value)).chain(r.cells.iter().filter_map(|c| c.unit.as_ref())) {
            base = base.max(tb.baseline);
            below = below.max(tb.h - tb.baseline);
        }
    }
    let text_h = base + below;
    let graph_h = (t.graph.height * s).min(text_h.max(10.0 * s));
    let bars_h = (t.cores.height * s).min(text_h.max(10.0 * s));
    let line_h = text_h.max(graph_h).max(bars_h);
    let top = py + (line_h - text_h) / 2.0;
    let mut prims = vec![];
    let mut live = Live::default();
    let mut x = px;
    let mut first = true;
    for (ri, r) in rows.iter().enumerate() {
        if r.cells.is_empty() && r.extras.is_empty() {
            continue;
        }
        if !first {
            if let Some(sp) = &sep {
                x += gap;
                prims.push(Prim::Text { x, y: top + base - sp.baseline, tb: sp.clone(), color: t.colors.separator.with_alpha(4.0), fx: false });
                x += sp.w + gap;
            } else {
                x += gap * 2.0;
            }
        }
        first = false;
        if t.shape.row_marker {
            prims.push(Prim::Fill { r: Rect { x, y: py + line_h * 0.2, w: 2.0 * s, h: line_h * 0.6 }, color: r.group_color, radius: s });
            x += 6.0 * s;
        }
        if let Some(l) = &r.label {
            prims.push(Prim::Text { x, y: top + base - l.baseline, tb: l.clone(), color: r.label_color, fx: false });
            x += l.w + g.label;
        }
        for (j, c) in r.cells.iter().enumerate() {
            let vw = sticky.get(format!("b{ri}.{j}"), c.vw());
            if let Some(id) = &c.live {
                note_live(&mut live, id, x + vw, top + base - c.value.baseline, r.hero && j == 0, c.color);
            } else {
                prims.push(Prim::Text { x: x + vw - c.value.w, y: top + base - c.value.baseline, tb: c.value.clone(), color: c.color, fx: true });
            }
            x += vw;
            if let Some(u) = &c.unit {
                prims.push(Prim::Text { x: x + unit_gap, y: top + base - u.baseline, tb: u.clone(), color: t.colors.unit, fx: false });
                x += unit_gap + sticky.get(format!("bu{ri}.{j}"), c.uw());
            }
            x += gap * 0.75;
            for e in r.extras.iter().filter(|extra| matches!(extra, Extra::Graph { after: Some(after), .. } if after == &c.id)) {
                let Extra::Graph { id, color, .. } = e else { continue };
                let w = if t.graph.width > 0.0 { t.graph.width * s } else { 60.0 * s };
                let (values, slots, min, max) = graph_values(inp, id, r.graph_max);
                let rect = Rect { x, y: py + (line_h - graph_h) / 2.0, w, h: graph_h };
                if inp.live && id == "frametime" {
                    live.graph = Some((rect, *color));
                }
                prims.push(Prim::Graph { r: rect, values, slots, min, max, color: *color, style: t.graph.style });
                x += w + gap * 0.75;
            }
        }
        if !r.cells.is_empty() {
            x -= gap * 0.75;
        }
        for e in &r.extras {
            if matches!(e, Extra::Graph { after: Some(after), .. } if r.cells.iter().any(|cell| &cell.id == after)) {
                continue;
            }
            if !r.cells.is_empty() || r.label.is_some() {
                x += gap * 0.75;
            }
            match e {
                Extra::Graph { id, color, .. } => {
                    let w = if t.graph.width > 0.0 { t.graph.width * s } else { 60.0 * s };
                    let (values, slots, min, max) = graph_values(inp, id, r.graph_max);
                    let rect = Rect { x, y: py + (line_h - graph_h) / 2.0, w, h: graph_h };
                    if inp.live && id == "frametime" {
                        live.graph = Some((rect, *color));
                    }
                    prims.push(Prim::Graph { r: rect, values, slots, min, max, color: *color, style: t.graph.style });
                    x += w;
                }
                Extra::Cores { color } => {
                    let n = inp.cores.len();
                    let w = n as f32 * (t.cores.bar_width + t.cores.bar_gap) * s - t.cores.bar_gap * s;
                    prims.push(Prim::Bars { r: Rect { x, y: py + (line_h - bars_h) / 2.0, w, h: bars_h }, values: inp.cores.iter().map(|v| *v as f32).collect(), color: *color });
                    x += w;
                }
            }
        }
    }
    let mut h = py * 2.0 + line_h;
    let mut w = x + px;
    // RTSS clients' plain text goes on lines below the strip.
    let flowing: Vec<&osd::Layer> = inp.osd.iter().filter(|l| !l.positioned()).collect();
    let mut y = py + line_h;
    for layer in flowing {
        y += g.row;
        let (fw, fh) = flow(&layer.items, px, y, fonts, t.colors.value, &mut prims);
        w = w.max(px * 2.0 + fw);
        y += fh;
        h = y + py;
    }
    Scene { w: w.ceil(), h: h.ceil(), panel: true, prims, live }
}

/// Lays out flowing OSD items (text runs, line breaks, images) at (x0, y0). Returns (w, h).
fn flow(items: &[osd::Item], x0: f32, y0: f32, fonts: &Fonts, default: Color, prims: &mut Vec<Prim>) -> (f32, f32) {
    // Group into lines first so each line can share a baseline.
    let mut lines: Vec<Vec<&osd::Item>> = vec![vec![]];
    for it in items {
        if matches!(it, osd::Item::Newline) {
            lines.push(vec![]);
        } else {
            lines.last_mut().unwrap().push(it);
        }
    }
    while lines.last().is_some_and(|l| l.is_empty()) && lines.len() > 1 {
        lines.pop();
    }
    let mut y = y0;
    let mut max_w: f32 = 0.0;
    for line in lines {
        let mut parts: Vec<(Option<TextBox>, Option<(String, f32, f32, Option<[f32; 4]>)>, Color)> = vec![];
        let mut base: f32 = 0.0;
        let mut below: f32 = 0.0;
        for it in line {
            match it {
                osd::Item::Text { text, color, scale } => {
                    if let Some(tb) = fonts.osd(text, *scale) {
                        base = base.max(tb.baseline);
                        below = below.max(tb.h - tb.baseline);
                        parts.push((Some(tb), None, color.unwrap_or(default)));
                    }
                }
                osd::Item::Image { path, w, h, src } => {
                    base = base.max(*h);
                    parts.push((None, Some((path.clone(), *w, *h, *src)), default));
                }
                osd::Item::Newline => {}
            }
        }
        if parts.is_empty() {
            y += fonts.value_size * 1.2;
            continue;
        }
        let mut x = x0;
        for (tb, img, color) in parts {
            if let Some(tb) = tb {
                let w = tb.w;
                prims.push(Prim::Text { x, y: y + base - tb.baseline, tb, color, fx: true });
                x += w;
            } else if let Some((path, w, h, src)) = img {
                prims.push(Prim::Image { r: Rect { x, y: y + base - h, w, h }, path, src });
                x += w;
            }
        }
        max_w = max_w.max(x - x0);
        y += base + below;
    }
    (max_w, y - y0)
}

/// A positioned OSD layer (RTSS `<P>`) as its own little scene, without a panel.
pub fn osd_layer(layer: &osd::Layer, fonts: &Fonts, theme: &Theme) -> Scene {
    let mut prims = vec![];
    let (w, h) = flow(&layer.items, 0.0, 0.0, fonts, theme.colors.value, &mut prims);
    Scene { w: w.ceil().max(1.0), h: h.ceil().max(1.0), panel: false, prims, live: Live::default() }
}
