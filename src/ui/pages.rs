//! The settings pages. Each draws itself with the kit and edits the working copy of the config.

use super::kit::{ButtonKind, Font, Rect, Ui, icon, id, idn, pal};
use crate::compose::Frame;
use crate::config::{Bridge, Config, Display, Monitor, Position, Row, ShowMode, TempUnit, ValueWidth};
use crate::metrics;
use crate::theme::{Color, Layout, Theme};

pub const PAGES: &[(char, &str)] = &[
    (icon::HOME, "Overview"),
    (icon::LIST, "Metrics"),
    (icon::COLOR, "Appearance"),
    (icon::VIEW, "When to show"),
    (icon::GAME, "In-game"),
    (icon::SETTINGS, "Advanced"),
];

pub const MODAL_METRICS: u32 = 1;

pub const RTSS_DOWNLOAD: &str = "https://www.guru3d.com/download/rtss-rivatuner-statistics-server-download/";

/// Something the settings window asks the app to do.
pub enum Action {
    ToggleOverlay,
    RestartAdmin,
    DuplicateTheme(String),
    OpenThemes,
    OpenConfig,
    OpenUrl(String),
    StartRtss,
    CheckUpdates,
    RestartUpdate,
}

pub struct ThemeInfo {
    pub name: String,
    pub theme: Theme,
    pub builtin: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RtssState {
    Missing,
    Installed,
    Running,
}

pub struct Status {
    pub visible: bool,
    pub elevated: bool,
    pub fps: Option<String>,
    pub gpu: String,
    pub rtss: String,
    pub monitors: usize,
    pub version: &'static str,
    pub rtss_state: RtssState,
    pub bridge_active: bool,
    pub exclusive_note: Option<String>,
    pub diag: String,
    /// (what to say, a new version is ready)
    pub update: (String, bool),
}

/// Draws theme previews (the settings window owns the fonts for them).
pub trait Previewer {
    fn preview(&mut self, ui: &mut Ui, cfg: &Config, theme: &Theme, frame: &Frame, area: Rect);
}

/// Page-level state that isn't part of the config.
#[derive(Default)]
pub struct PageState {
    pub page: usize,
    pub modal_row: usize,
    pub new_exclude: String,
    pub new_include: String,
    pub new_above: String,
}

pub struct Ctx<'a> {
    pub cfg: &'a mut Config,
    pub changed: bool,
    pub actions: Vec<Action>,
    pub status: &'a Status,
    pub themes: &'a [ThemeInfo],
    pub frame: &'a Frame,
    pub apps: &'a [String],
    pub ps: &'a mut PageState,
    pub previewer: &'a mut dyn Previewer,
}

const SIDEBAR_W: f32 = 248.0;

pub fn draw(ui: &mut Ui, cx: &mut Ctx) {
    let (w, h) = ui.size;
    ui.fill(Rect::new(0.0, 0.0, w, h), pal::BG, 0.0);
    sidebar(ui, cx);

    // Scrolling content column.
    let view = Rect::new(SIDEBAR_W, 0.0, w - SIDEBAR_W, h);
    ui.st.view_h = h;
    if view.contains(ui.inp.mouse) && ui.st.modal.is_none() && ui.st.popup.is_none() {
        ui.st.scroll_target -= ui.inp.wheel * 0.9;
    }
    let col_w = (view.w - 88.0).clamp(420.0, 920.0);
    ui.x = SIDEBAR_W + ((view.w - col_w) / 2.0).max(44.0);
    ui.w = col_w;
    ui.y = 36.0 - ui.st.scroll;
    let top = ui.y;
    ui.clip = Some(view);
    ui.push_clip(view);
    let (_, title) = PAGES[cx.ps.page];
    ui.text(Font::Title, title, ui.x, ui.y, pal::TEXT);
    ui.y += 52.0;
    match cx.ps.page {
        0 => overview(ui, cx),
        1 => metrics_page(ui, cx),
        2 => appearance(ui, cx),
        3 => when(ui, cx),
        4 => ingame(ui, cx),
        _ => advanced(ui, cx),
    }
    ui.y += 40.0;
    ui.st.content_h = ui.y - top + 36.0;
    ui.pop_clip();
    ui.clip = None;
    scrollbar(ui, view);

    if let Some(m) = ui.st.modal {
        match m {
            MODAL_METRICS => metric_picker(ui, cx),
            _ => ui.st.modal = None,
        }
    }
    ui.popups();
}

fn scrollbar(ui: &mut Ui, view: Rect) {
    let (content, viewh) = (ui.st.content_h, ui.st.view_h);
    if content <= viewh + 1.0 {
        return;
    }
    let track_h = view.h - 16.0;
    let thumb_h = (track_h * viewh / content).max(40.0);
    let t = ui.st.scroll / (content - viewh);
    let r = Rect::new(view.right() - 8.0, view.y + 8.0 + (track_h - thumb_h) * t, 4.0, thumb_h);
    let hover = Rect::new(view.right() - 16.0, view.y, 16.0, view.h).contains(ui.inp.mouse);
    let a = ui.anim(id("scrollbar"), if hover { 1.0 } else { 0.0 }, 12.0);
    ui.fill(r, pal::FAINT.with_alpha(0.45 + 0.4 * a), 2.0);
}

fn sidebar(ui: &mut Ui, cx: &mut Ctx) {
    let h = ui.size.1;
    ui.fill(Rect::new(0.0, 0.0, SIDEBAR_W, h), pal::SIDEBAR, 0.0);
    ui.fill(Rect::new(SIDEBAR_W - 1.0, 0.0, 1.0, h), pal::STROKE, 0.0);

    // Brand: the Δ mark and name.
    let (bx, by) = (24.0, 26.0);
    let tri = |ui: &mut Ui, s: f32, c: Color| {
        ui.line(bx + s / 2.0, by, bx + s, by + s * 0.88, c, 2.4);
        ui.line(bx + s, by + s * 0.88, bx, by + s * 0.88, c, 2.4);
        ui.line(bx, by + s * 0.88, bx + s / 2.0, by, c, 2.4);
    };
    tri(ui, 22.0, pal::ACCENT);
    ui.text(Font::Brand, "Delta", bx + 34.0, by - 1.0, pal::TEXT);

    let mut y = 84.0;
    let mut sel_y = 0.0;
    for (i, (g, name)) in PAGES.iter().enumerate() {
        let r = Rect::new(12.0, y, SIDEBAR_W - 24.0, 40.0);
        let (hover, clicked) = ui.interact(idn("nav", i), r);
        if clicked && cx.ps.page != i {
            cx.ps.page = i;
            ui.st.scroll = 0.0;
            ui.st.scroll_target = 0.0;
            ui.st.focus = 0;
        }
        let selected = cx.ps.page == i;
        if selected {
            ui.fill(r, pal::CONTROL, 6.0);
            sel_y = y;
        } else if hover {
            ui.fill(r, pal::CARD, 6.0);
        }
        ui.glyph(*g, r.x + 24.0, r.y + 20.0, if selected { pal::ACCENT } else { pal::DIM }, false);
        ui.text_mid(Font::Body, name, r.x + 48.0, r, if selected { pal::TEXT } else { pal::DIM });
        y += 44.0;
    }
    let iy = ui.anim(id("nav-indicator"), sel_y, 18.0);
    ui.fill(Rect::new(12.0, iy + 11.0, 3.0, 18.0), pal::ACCENT, 1.5);

    // Footer: overlay state and a quick toggle.
    let fr = Rect::new(12.0, h - 66.0, SIDEBAR_W - 24.0, 52.0);
    ui.fill(fr, pal::CARD, 8.0);
    let on = cx.status.visible;
    ui.circle(fr.x + 20.0, fr.y + 26.0, 4.0, if on { pal::GOOD } else { pal::FAINT });
    ui.text(Font::BodyStrong, if on { "Overlay on" } else { "Overlay off" }, fr.x + 34.0, fr.y + 8.0, pal::TEXT);
    let hk = if cx.cfg.hotkeys.toggle.is_empty() { "No hotkey".to_string() } else { cx.cfg.hotkeys.toggle.replace('+', " + ") };
    ui.text(Font::Small, &hk, fr.x + 34.0, fr.y + 28.0, pal::FAINT);
    let mut v = on;
    if ui.toggle(id("footer-toggle"), &mut v, Rect::new(fr.right() - 52.0, fr.y, 44.0, fr.h)) {
        cx.actions.push(Action::ToggleOverlay);
    }
}

// ---------------------------------------------------------------- Overview

fn overview(ui: &mut Ui, cx: &mut Ctx) {
    let st = cx.status;
    let mut visible = st.visible;
    ui.setting(Some(icon::VIEW), "Show the overlay", "Turn the overlay on or off. You can also use the hotkey or click the tray icon.", 120.0, |ui, r| {
        if ui.toggle(id("ov-visible"), &mut visible, r) {
            cx.actions.push(Action::ToggleOverlay);
        }
    });

    ui.section("Status");
    let fps_ok = st.fps.is_none();
    let fps_desc = match &st.fps {
        None => "Measuring every game's frames through Windows' graphics events.".to_string(),
        Some(_) if !st.elevated => "Windows only shares frame timing with administrator apps. Restart Delta as administrator to see FPS.".to_string(),
        Some(n) => n.clone(),
    };
    let elevated = st.elevated;
    ui.setting(Some(icon::SPEED), "FPS and frame times", &fps_desc, 260.0, |ui, r| {
        if fps_ok {
            pill(ui, r, "Working", pal::GOOD);
        } else if !elevated {
            let (c, _) = ui.button(id("ov-admin"), Some(icon::SHIELD), "Restart as administrator", ButtonKind::Primary, r.right() - 236.0, r.y + (r.h - 32.0) / 2.0);
            if c {
                cx.actions.push(Action::RestartAdmin);
            }
        } else {
            pill(ui, r, "Unavailable", pal::WARN);
        }
    });
    ui.setting(Some(icon::CHIP), "Graphics card", &st.gpu, 160.0, |ui, r| pill(ui, r, "Detected", pal::GOOD));
    let rtss_ok = st.rtss.starts_with("Delta");
    ui.setting(Some(icon::TEXT), "Apps made for RTSS", &format!("HWiNFO, AIDA64, OBS HUD and others that draw through RivaTuner. {}", st.rtss), 160.0, |ui, r| {
        pill(ui, r, if rtss_ok { "Supported" } else { "Off" }, if rtss_ok { pal::GOOD } else { pal::FAINT })
    });
    let (label, color, desc) = match (cx.status.rtss_state, cx.cfg.rtss.bridge, cx.status.bridge_active) {
        (_, _, true) => ("Drawing", pal::GOOD, "Delta is drawing inside the game in front, through RivaTuner."),
        (_, Bridge::Off, _) => ("Off", pal::FAINT, "Delta only shows as a window, which can't appear over exclusive-fullscreen games."),
        (RtssState::Running, _, _) => ("Ready", pal::GOOD, "RivaTuner is running, so Delta can draw inside exclusive-fullscreen games."),
        (RtssState::Installed, _, _) => ("RivaTuner not running", pal::WARN, "Start RivaTuner Statistics Server to see the overlay inside exclusive-fullscreen games."),
        (RtssState::Missing, _, _) => ("Needs RivaTuner", pal::WARN, "Exclusive-fullscreen games need Borderless mode, or the free RivaTuner Statistics Server."),
    };
    ui.setting(Some(icon::GAME), "Exclusive fullscreen games", desc, 220.0, |ui, r| pill(ui, r, label, color));
    if let Some(n) = &cx.status.exclusive_note {
        ui.note(n, pal::WARN);
    }

    ui.section("Quick start");
    ui.note("Pick what to show under Metrics, how it looks under Appearance, and when it appears under When to show. Every change shows up on the overlay right away.", pal::DIM);
    if !cx.status.diag.is_empty() {
        ui.section("Last app in front");
        ui.note(&cx.status.diag, pal::FAINT);
    }
}

fn pill(ui: &mut Ui, r: Rect, text: &str, c: Color) {
    let (tw, _) = ui.measure(Font::Small, text);
    let p = Rect::new(r.right() - tw - 30.0, r.y + (r.h - 26.0) / 2.0, tw + 30.0, 26.0);
    ui.fill(p, c.with_alpha(0.14), 13.0);
    ui.circle(p.x + 12.0, p.y + 13.0, 3.5, c);
    ui.text_mid(Font::Small, text, p.x + 21.0, p, c);
}

// ---------------------------------------------------------------- Metrics

fn metrics_page(ui: &mut Ui, cx: &mut Ctx) {
    ui.note("The overlay shows these rows from top to bottom. Each row has a label and one or more values, shown as numbers, a graph, or both.", pal::DIM);
    ui.y += 4.0;
    let n = cx.cfg.rows.len();
    let mut swap: Option<(usize, usize)> = None;
    let mut remove: Option<usize> = None;
    for i in 0..n {
        // Chip layout first, so the card knows its height.
        let chip_w: Vec<f32> = cx.cfg.rows[i].metrics.iter().map(|m| ui.measure(Font::Body, &metrics::display_name(m)).0 + 48.0).collect();
        let avail = ui.w - 36.0;
        let (mut x, mut lines) = (0.0, 1);
        for w in chip_w.iter().chain(std::iter::once(&92.0)) {
            if x + w > avail && x > 0.0 {
                lines += 1;
                x = 0.0;
            }
            x += w + 8.0;
        }
        let has_graph = cx.cfg.rows[i].display != Display::Text;
        let h = 60.0 + lines as f32 * 38.0 + if has_graph { 44.0 } else { 0.0 } + 8.0;
        ui.card(h, |ui, r| {
            let row = &mut cx.cfg.rows[i];
            let hy = r.y + 14.0;
            if ui.icon_button(idn("row-up", i), icon::UP, r.x + 10.0, hy, if i == 0 { pal::FAINT.with_alpha(0.4) } else { pal::DIM }) && i > 0 {
                swap = Some((i, i - 1));
            }
            if ui.icon_button(idn("row-down", i), icon::DOWN, r.x + 40.0, hy, if i + 1 == n { pal::FAINT.with_alpha(0.4) } else { pal::DIM }) && i + 1 < n {
                swap = Some((i, i + 1));
            }
            if ui.text_field(idn("row-label", i), &mut row.label, Rect::new(r.x + 80.0, hy - 1.0, 150.0, 32.0), "No label") {
                cx.changed = true;
            }
            let mut d = match row.display {
                Display::Text => 0,
                Display::Graph => 1,
                Display::Both => 2,
            };
            let seg_area = Rect::new(r.x, hy - 1.0, r.w - 56.0, 32.0);
            if ui.segmented(idn("row-display", i), &["Numbers", "Graph", "Both"], &mut d, seg_area) {
                row.display = [Display::Text, Display::Graph, Display::Both][d];
                cx.changed = true;
            }
            if ui.icon_button(idn("row-del", i), icon::DELETE, r.right() - 44.0, hy, pal::DIM) {
                remove = Some(i);
            }
            // Metric chips.
            let mut x = r.x + 18.0;
            let mut y = r.y + 60.0;
            let mut drop_metric = None;
            for (j, m) in row.metrics.iter().enumerate() {
                let w = chip_w[j];
                if x + w > r.right() - 18.0 && x > r.x + 18.0 {
                    x = r.x + 18.0;
                    y += 38.0;
                }
                let c = group_color_static(cx.themes, &cx.cfg.theme, m);
                let (_, rm, cw) = ui.chip(idn(&format!("chip{i}"), j), &metrics::display_name(m), c, true, x, y);
                if rm {
                    drop_metric = Some(j);
                }
                x += cw + 8.0;
            }
            let row = &mut cx.cfg.rows[i];
            if let Some(j) = drop_metric {
                row.metrics.remove(j);
                cx.changed = true;
            }
            if x + 92.0 > r.right() - 18.0 && x > r.x + 18.0 {
                x = r.x + 18.0;
                y += 38.0;
            }
            let (add, _) = ui.button(idn("chip-add", i), Some(icon::ADD), "Add", ButtonKind::Subtle, x, y - 1.0);
            if add {
                cx.ps.modal_row = i;
                ui.st.modal_scroll = 0.0;
                ui.st.modal_scroll_target = 0.0;
                ui.st.modal = Some(MODAL_METRICS);
            }
            if has_graph {
                let gy = y + 40.0;
                let row = &mut cx.cfg.rows[i];
                let mut options: Vec<String> = row.metrics.iter().filter(|m| *m != "cpu.cores" && *m != "app").cloned().collect();
                if !options.iter().any(|m| m == "frametime") && row.metrics.iter().any(|m| m.starts_with("fps")) {
                    options.push("frametime".into());
                }
                if !options.is_empty() {
                    let current = row.graph.clone().unwrap_or_else(|| options[0].clone());
                    let mut sel = options.iter().position(|o| *o == current).unwrap_or(0);
                    ui.text_mid(Font::Body, "Graph shows", r.x + 18.0, Rect::new(r.x, gy, 0.0, 36.0), pal::DIM);
                    let names: Vec<String> = options.iter().map(|o| metrics::display_name(o)).collect();
                    if ui.dropdown(idn("row-graph", i), &names, &mut sel, Rect::new(r.x + 120.0, gy, 220.0, 36.0), 220.0) {
                        row.graph = Some(options[sel].clone());
                        cx.changed = true;
                    }
                }
            }
        });
    }
    if let Some((a, b)) = swap {
        cx.cfg.rows.swap(a, b);
        cx.changed = true;
    }
    if let Some(i) = remove {
        cx.cfg.rows.remove(i);
        cx.changed = true;
    }
    ui.y += 8.0;
    let (add, r) = ui.button(id("row-add"), Some(icon::ADD), "Add a row", ButtonKind::Primary, ui.x, ui.y);
    if add {
        cx.cfg.rows.push(Row { label: "New".into(), metrics: vec![], ..Default::default() });
        cx.ps.modal_row = cx.cfg.rows.len() - 1;
        ui.st.modal_scroll = 0.0;
        ui.st.modal_scroll_target = 0.0;
        ui.st.modal = Some(MODAL_METRICS);
        cx.changed = true;
    }
    let (reset, _) = ui.button(id("row-reset"), Some(icon::REFRESH), "Reset to default", ButtonKind::Secondary, r.right() + 8.0, ui.y);
    if reset {
        cx.cfg.rows = Config::default().rows;
        cx.changed = true;
    }
    ui.y += 40.0;

    ui.section("Colors by value");
    ui.note("Values turn the theme's warning and critical colors past these points. For frame rates, lower is worse.", pal::DIM);
    let mut keys: Vec<String> = cx.cfg.thresholds.keys().cloned().collect();
    keys.sort();
    for k in keys {
        let [mut warn, mut crit] = cx.cfg.thresholds[&k];
        let lower_worse = warn > crit;
        let max = if k.starts_with("fps") { 240.0 } else { 100.0 };
        ui.setting(None, &metrics::display_name(&k), if lower_worse { "Warning below / critical below" } else { "Warning above / critical above" }, 440.0, |ui, r| {
            let half = Rect::new(r.x, r.y, r.w / 2.0 - 8.0, r.h);
            let (mut w, mut c) = (warn as f32, crit as f32);
            let a = ui.slider(id(&format!("th-w-{k}")), &mut w, 0.0, max, 1.0, &|v| format!("{v:.0}"), half, 210.0);
            let b = ui.slider(id(&format!("th-c-{k}")), &mut c, 0.0, max, 1.0, &|v| format!("{v:.0}"), r, 210.0);
            if a || b {
                warn = w as f64;
                crit = c as f64;
                cx.changed = true;
            }
        });
        cx.cfg.thresholds.insert(k, [warn, crit]);
    }
}

fn group_color_static(themes: &[ThemeInfo], current: &str, metric: &str) -> Color {
    themes.iter().find(|t| t.name.eq_ignore_ascii_case(current)).map_or(pal::ACCENT, |t| t.theme.group_color(metrics::group_of(metric)))
}

fn metric_picker(ui: &mut Ui, cx: &mut Ctx) {
    let w = (ui.size.0 - 120.0).min(760.0);
    let h = (ui.size.1 - 100.0).min(640.0);
    let (r, close) = ui.modal_frame(w, h, "Add a metric");
    if close {
        ui.st.modal = None;
        return;
    }
    let row = cx.ps.modal_row.min(cx.cfg.rows.len().saturating_sub(1));
    let Some(current) = cx.cfg.rows.get(row).map(|r| r.metrics.clone()) else {
        ui.st.modal = None;
        return;
    };
    let mut entries: Vec<(String, String, String, String)> = Vec::new();
    let core_count = cx.frame.cores.len().max(
        cx.frame
            .values
            .keys()
            .filter_map(|key| {
                key.strip_prefix("cpu.core.")
                    .or_else(|| key.strip_prefix("cpu.core_clock."))
                    .and_then(|n| n.parse::<usize>().ok())
            })
            .max()
            .map_or(0, |n| n + 1),
    );
    for (mid, name, category, desc) in metrics::CATALOG {
        entries.push((mid.to_string(), name.to_string(), category.to_string(), desc.to_string()));
        if *mid == "cpu.cores" {
            for core in 0..core_count {
                let load = format!("cpu.core.{core}");
                if cx.frame.values.contains_key(&load) {
                    entries.push((load, format!("Core {} load", core + 1), "Processor".into(), "Load of this logical processor".into()));
                }
                let clock = format!("cpu.core_clock.{core}");
                if cx.frame.values.contains_key(&clock) {
                    entries.push((clock, format!("Core {} clock", core + 1), "Processor".into(), "Effective clock speed of this logical processor".into()));
                }
            }
        }
    }

    // Measure the full list so the sheet can scroll even when its content is taller than the window.
    let mut content_h = 0.0;
    let mut measured_cat = "";
    let mut measured_col = 0;
    for (_, _, category, _) in &entries {
        if category != measured_cat {
            if measured_col == 1 {
                content_h += 58.0;
            }
            measured_col = 0;
            if !measured_cat.is_empty() {
                content_h += 8.0;
            }
            measured_cat = category;
            content_h += 22.0;
        }
        measured_col += 1;
        if measured_col == 2 {
            measured_col = 0;
            content_h += 58.0;
        }
    }
    if measured_col == 1 {
        content_h += 58.0;
    }
    if r.contains(ui.inp.mouse) {
        ui.st.modal_scroll_target -= ui.inp.wheel * 0.35;
    }
    let max_scroll = (content_h - r.h).max(0.0);
    ui.st.modal_scroll_target = ui.st.modal_scroll_target.clamp(0.0, max_scroll);
    let d = ui.st.modal_scroll_target - ui.st.modal_scroll;
    if d.abs() > 0.5 {
        ui.st.modal_scroll += d * (1.0 - (-18.0 * ui.st.dt).exp());
        ui.st.animating = true;
    } else {
        ui.st.modal_scroll = ui.st.modal_scroll_target;
    }
    let mut y = r.y - ui.st.modal_scroll;
    let col_w = (r.w - 12.0) / 2.0;
    let mut cat = "";
    let mut i = 0;
    let mut col = 0;
    let old_clip = ui.clip;
    ui.clip = Some(r);
    ui.push_clip(r);
    for (mid, name, category, desc) in &entries {
        if category != cat {
            if col == 1 {
                y += 58.0;
            }
            col = 0;
            if !cat.is_empty() {
                y += 8.0;
            }
            cat = category;
            ui.text(Font::Caption, &category.to_uppercase(), r.x + 2.0, y, pal::FAINT);
            y += 22.0;
        }
        let cr = Rect::new(r.x + col as f32 * (col_w + 12.0), y, col_w, 52.0);
        let added = current.iter().any(|m| m == mid);
        let (hover, clicked) = ui.interact(idn("pick", i), cr);
        ui.fill(cr, if hover { pal::CARD_HOVER } else { pal::CARD }, 6.0);
        ui.stroke(cr, if added { pal::ACCENT.with_alpha(0.6) } else { pal::STROKE }, 6.0, 1.0);
        let c = group_color_static(cx.themes, &cx.cfg.theme, mid);
        ui.circle(cr.x + 18.0, cr.y + 18.0, 4.0, c);
        ui.text(Font::Body, name, cr.x + 32.0, cr.y + 8.0, pal::TEXT);
        ui.text(Font::Small, desc, cr.x + 32.0, cr.y + 28.0, pal::DIM);
        if added {
            ui.glyph(icon::CHECK, cr.right() - 22.0, cr.y + 26.0, pal::ACCENT, false);
        }
        if clicked {
            let m = &mut cx.cfg.rows[row].metrics;
            if added {
                m.retain(|x| x != mid);
            } else {
                m.push(mid.to_string());
            }
            cx.changed = true;
        }
        i += 1;
        col += 1;
        if col == 2 {
            col = 0;
            y += 58.0;
        }
    }
    ui.pop_clip();
    ui.clip = old_clip;
}

// ---------------------------------------------------------------- Appearance

fn appearance(ui: &mut Ui, cx: &mut Ctx) {
    ui.text(Font::Section, "Theme", ui.x + 2.0, ui.y, pal::TEXT);
    ui.y += 30.0;
    let cols = if ui.w > 760.0 { 3 } else { 2 };
    let gap = 12.0;
    let cw = (ui.w - gap * (cols - 1) as f32) / cols as f32;
    let ch = 226.0;
    let n = cx.themes.len();
    for i in 0..n {
        let (c, rw) = (i % cols, i / cols);
        let r = Rect::new(ui.x + c as f32 * (cw + gap), ui.y + rw as f32 * (ch + gap), cw, ch);
        let t = &cx.themes[i];
        let selected = t.name.eq_ignore_ascii_case(&cx.cfg.theme);
        let (hover, clicked) = ui.interact(idn("theme", i), r);
        ui.fill(r, if hover { pal::CARD_HOVER } else { pal::CARD }, 8.0);
        // Preview well: a little "desktop" behind the real overlay renderer.
        let well = Rect::new(r.x + 8.0, r.y + 8.0, r.w - 16.0, ch - 80.0);
        ui.fill(well, Color([0.09, 0.11, 0.15, 1.0]), 6.0);
        ui.fill(Rect::new(well.x, well.y + well.h * 0.55, well.w, well.h * 0.45), Color([0.12, 0.16, 0.13, 1.0]), 0.0);
        ui.fill(Rect::new(well.x, well.bottom() - 6.0, well.w, 6.0), Color([0.12, 0.16, 0.13, 1.0]), 6.0);
        ui.push_clip(well);
        let theme = t.theme.clone();
        cx.previewer.preview(ui, cx.cfg, &theme, cx.frame, well);
        ui.pop_clip();
        let t = &cx.themes[i];
        ui.text(Font::BodyStrong, &t.theme.name, r.x + 14.0, well.bottom() + 10.0, pal::TEXT);
        let desc = if t.builtin { t.theme.description.clone() } else { format!("Your theme · {}", t.theme.description) };
        ui.push_clip(Rect::new(r.x, r.y, r.w, r.h - 6.0));
        ui.text_wrapped(Font::Small, &desc, r.x + 14.0, well.bottom() + 31.0, r.w - 28.0, pal::DIM);
        ui.pop_clip();
        if selected {
            ui.stroke(r, pal::ACCENT, 8.0, 2.0);
            ui.circle(r.right() - 20.0, well.bottom() + 20.0, 9.0, pal::ACCENT);
            ui.glyph(icon::CHECK, r.right() - 20.0, well.bottom() + 20.0, pal::ON_ACCENT, false);
        } else {
            ui.stroke(r, pal::STROKE, 8.0, 1.0);
        }
        if clicked && !selected {
            cx.cfg.theme = t.name.clone();
            cx.changed = true;
        }
    }
    ui.y += n.div_ceil(cols) as f32 * (ch + gap) + 4.0;
    let (dup, r) = ui.button(id("theme-dup"), Some(icon::COPY), "Duplicate to customize", ButtonKind::Secondary, ui.x, ui.y);
    if dup {
        cx.actions.push(Action::DuplicateTheme(cx.cfg.theme.clone()));
    }
    let (open, _) = ui.button(id("theme-folder"), Some(icon::FOLDER), "Open themes folder", ButtonKind::Subtle, r.right() + 8.0, ui.y);
    if open {
        cx.actions.push(Action::OpenThemes);
    }
    ui.y += 44.0;

    ui.section("Layout and position");
    let mut lay = match cx.cfg.layout {
        None => 0,
        Some(Layout::Stack) => 1,
        Some(Layout::Bar) => 2,
    };
    ui.setting(Some(icon::LIST), "Layout", "Rows stacked in a panel, or everything on one line.", 330.0, |ui, r| {
        if ui.segmented(id("layout"), &["Theme's choice", "Rows", "One line"], &mut lay, r) {
            cx.cfg.layout = [None, Some(Layout::Stack), Some(Layout::Bar)][lay];
            cx.changed = true;
        }
    });
    ui.setting(Some(icon::POSITION), "Position", "Where on the screen the overlay sits.", 150.0, |ui, r| {
        if position_picker(ui, &mut cx.cfg.position, r) {
            cx.changed = true;
        }
    });
    ui.setting(None, "Distance from the edge", "Horizontal and vertical gap, in pixels.", 440.0, |ui, r| {
        let mut x = cx.cfg.offset_x as f32;
        let mut y = cx.cfg.offset_y as f32;
        let half = Rect::new(r.x, r.y, r.w / 2.0 - 8.0, r.h);
        if ui.slider(id("off-x"), &mut x, 0.0, 400.0, 1.0, &|v| format!("{v:.0}"), half, 210.0) {
            cx.cfg.offset_x = x as i32;
            cx.changed = true;
        }
        if ui.slider(id("off-y"), &mut y, 0.0, 400.0, 1.0, &|v| format!("{v:.0}"), r, 210.0) {
            cx.cfg.offset_y = y as i32;
            cx.changed = true;
        }
    });
    let mut scale = cx.cfg.scale;
    ui.setting(Some(icon::RESIZE), "Size", "On top of Windows' display scaling.", 300.0, |ui, r| {
        if ui.slider(id("scale"), &mut scale, 0.5, 2.5, 0.05, &|v| format!("{:.0}%", v * 100.0), r, 300.0) {
            cx.cfg.scale = scale;
            cx.changed = true;
        }
    });

    ui.section("Transparency");
    for (key, title, desc, min) in [
        ("op-all", "Whole overlay", "Fades everything together.", 0.1f32),
        ("op-bg", "Background", "The panel behind the text. All the way down hides it.", 0.0),
        ("op-fg", "Text and graphs", "Numbers, labels, graphs and bars.", 0.1),
    ] {
        let mut v = match key {
            "op-all" => cx.cfg.opacity,
            "op-bg" => cx.cfg.background_opacity,
            _ => cx.cfg.text_opacity,
        };
        ui.setting(Some(icon::OPACITY), title, desc, 300.0, |ui, r| {
            if ui.slider(id(key), &mut v, min, 1.0, 0.01, &|v| format!("{:.0}%", v * 100.0), r, 300.0) {
                cx.changed = true;
            }
        });
        match key {
            "op-all" => cx.cfg.opacity = v,
            "op-bg" => cx.cfg.background_opacity = v,
            _ => cx.cfg.text_opacity = v,
        }
    }

    ui.section("Spacing");
    let mut vw = if cx.cfg.spacing.values == ValueWidth::Fixed { 0 } else { 1 };
    ui.setting(Some(icon::SPACING), "Value width", "Fixed keeps room for each value's widest reading, so numbers never shift. Snug fits them tightly.", 200.0, |ui, r| {
        if ui.segmented(id("vw"), &["Fixed", "Snug"], &mut vw, r) {
            cx.cfg.spacing.values = [ValueWidth::Fixed, ValueWidth::Fit][vw];
            cx.changed = true;
        }
    });
    let theme = cx.themes.iter().find(|t| t.name.eq_ignore_ascii_case(&cx.cfg.theme)).map(|t| t.theme.shape.clone());
    let defaults = theme.map(|s| (s.column_gap, 3.0, s.row_gap, s.label_gap)).unwrap_or((12.0, 3.0, 6.0, 14.0));
    for (k, title, desc, def, max) in [
        ("sp-col", "Between values", "Gap between one value and the next.", defaults.0, 40.0f32),
        ("sp-unit", "Before units", "Gap between a number and its unit.", defaults.1, 16.0),
        ("sp-row", "Between rows", "Vertical gap between rows.", defaults.2, 24.0),
        ("sp-label", "After labels", "Gap between a row's label and its values.", defaults.3, 40.0),
    ] {
        let slot = match k {
            "sp-col" => &mut cx.cfg.spacing.column_gap,
            "sp-unit" => &mut cx.cfg.spacing.unit_gap,
            "sp-row" => &mut cx.cfg.spacing.row_gap,
            _ => &mut cx.cfg.spacing.label_gap,
        };
        let mut v = slot.unwrap_or(def);
        let custom = slot.is_some();
        let mut changed = false;
        let mut reset = false;
        ui.setting(None, title, &format!("{desc}{}", if custom { "" } else { " Using the theme's spacing." }), 350.0, |ui, r| {
            let sr = Rect::new(r.x, r.y, r.w - 40.0, r.h);
            changed = ui.slider(id(k), &mut v, 0.0, max, 1.0, &|v| format!("{v:.0} px"), sr, 300.0);
            if custom && ui.icon_button(id(&format!("{k}-reset")), icon::REFRESH, r.right() - 30.0, r.y + (r.h - 30.0) / 2.0, pal::DIM) {
                reset = true;
            }
        });
        if changed {
            *slot = Some(v);
            cx.changed = true;
        }
        if reset {
            *slot = None;
            cx.changed = true;
        }
    }

    ui.section("Units");
    let mut t = if cx.cfg.temperature == TempUnit::C { 0 } else { 1 };
    ui.setting(None, "Temperature", "", 160.0, |ui, r| {
        if ui.segmented(id("temp"), &["°C", "°F"], &mut t, r) {
            cx.cfg.temperature = [TempUnit::C, TempUnit::F][t];
            cx.changed = true;
        }
    });
}

/// A small screen with 8 spots to click.
fn position_picker(ui: &mut Ui, pos: &mut Position, area: Rect) -> bool {
    let (w, h) = (112.0, 64.0);
    let r = Rect::new(area.right() - w, area.y + (area.h - h) / 2.0, w, h);
    ui.fill(r, pal::CONTROL, 6.0);
    ui.stroke(r, pal::STROKE_STRONG, 6.0, 1.0);
    let mut changed = false;
    for (i, p) in Position::ALL.iter().enumerate() {
        let (ax, ay) = p.anchors();
        let cx = r.x + 16.0 + ax as f32 * (w - 32.0) / 2.0;
        let cy = r.y + 14.0 + ay as f32 * (h - 28.0) / 2.0;
        let hit = Rect::new(cx - 12.0, cy - 10.0, 24.0, 20.0);
        let (hover, clicked) = ui.interact(idn("pos", i), hit);
        let sel = *pos == *p;
        if sel {
            ui.fill(Rect::new(cx - 10.0, cy - 6.0, 20.0, 12.0), pal::ACCENT, 3.0);
        } else {
            ui.fill(Rect::new(cx - 7.0, cy - 4.0, 14.0, 8.0), if hover { pal::DIM } else { pal::TRACK_OFF }, 2.0);
        }
        if clicked && !sel {
            *pos = *p;
            changed = true;
        }
    }
    changed
}

// ---------------------------------------------------------------- When to show

fn when(ui: &mut Ui, cx: &mut Ctx) {
    let mut mode = match cx.cfg.show.mode {
        ShowMode::Always => 0,
        ShowMode::Games => 1,
        ShowMode::Fullscreen => 2,
    };
    ui.setting(Some(icon::VIEW), "Show the overlay", "Always, only while a game is rendering, or only over fullscreen apps.", 350.0, |ui, r| {
        if ui.segmented(id("mode"), &["Always", "In games", "Fullscreen"], &mut mode, r) {
            cx.cfg.show.mode = [ShowMode::Always, ShowMode::Games, ShowMode::Fullscreen][mode];
            cx.changed = true;
        }
    });

    let mut items: Vec<String> = vec!["Main monitor".into(), "Where the game is".into(), "Where the active window is".into(), "All monitors".into()];
    for i in 1..=cx.status.monitors {
        items.push(format!("Monitor {i}"));
    }
    let mut sel = match cx.cfg.monitor() {
        Monitor::Main => 0,
        Monitor::FollowGame => 1,
        Monitor::Follow => 2,
        Monitor::All => 3,
        Monitor::Number(n) => (3 + n as usize).min(items.len() - 1),
    };
    ui.setting(Some(icon::MONITOR), "Monitor", "Which screen the overlay appears on. \"Where the game is\" stays on the main monitor until a game starts.", 260.0, |ui, r| {
        if ui.dropdown(id("monitor"), &items, &mut sel, r, 260.0) {
            cx.cfg.monitor = match sel {
                0 => Monitor::Main,
                1 => Monitor::FollowGame,
                2 => Monitor::Follow,
                3 => Monitor::All,
                n => Monitor::Number((n - 3) as u32),
            }
            .key();
            cx.changed = true;
        }
    });
    let mut v = cx.cfg.show.start_visible;
    ui.setting(Some(icon::POWER), "Visible when Delta starts", "Off means the overlay waits for the hotkey.", 120.0, |ui, r| {
        if ui.toggle(id("start-visible"), &mut v, r) {
            cx.changed = true;
        }
    });
    cx.cfg.show.start_visible = v;
    let mut hc = cx.cfg.show.hide_from_capture;
    ui.setting(Some(icon::EYE_OFF), "Hide from recordings and screenshots", "OBS, Game Bar and screenshots won't see the window overlay. You still do. Some anti-cheat systems take note of apps that hide from capture, so leave it off unless you need it.", 120.0, |ui, r| {
        if ui.toggle(id("hide-capture"), &mut hc, r) {
            cx.changed = true;
        }
    });
    cx.cfg.show.hide_from_capture = hc;

    ui.section("Hotkeys");
    for (k, title, desc) in [("hk-toggle", "Show or hide", "Turns the overlay on and off."), ("hk-theme", "Next theme", "Cycles through your themes."), ("hk-pos", "Next position", "Moves the overlay around the screen.")] {
        let slot = match k {
            "hk-toggle" => &mut cx.cfg.hotkeys.toggle,
            "hk-theme" => &mut cx.cfg.hotkeys.cycle_theme,
            _ => &mut cx.cfg.hotkeys.cycle_position,
        };
        let mut changed = false;
        ui.setting(Some(icon::KEYBOARD), title, desc, 240.0, |ui, r| {
            changed = ui.hotkey_field(id(k), slot, Rect::new(r.right() - 240.0, r.y + (r.h - 34.0) / 2.0, 240.0, 34.0));
        });
        if changed {
            cx.changed = true;
        }
    }
    ui.note("Click a box and press the keys. Use Ctrl, Alt, Shift or Win with a key (F-keys, Pause and Scroll Lock also work alone) so the hotkey doesn't take that key away from your apps. Backspace clears it.", pal::FAINT);

    ui.section("Apps");
    app_list(ui, cx, "Never show over", "The overlay hides while one of these apps is in front.", 0);
    app_list(ui, cx, "Only show over", "If you add apps here, the overlay only shows over them.", 1);
    app_list(ui, cx, "Let these stay on top", "Other always-on-top apps (notches, widgets, chat overlays) that should stay above Delta instead of trading places with it.", 2);
}

/// An editable list of exe names (0 = exclude, 1 = include, 2 = stay on top of Delta).
fn app_list(ui: &mut Ui, cx: &mut Ctx, title: &str, desc: &str, which: usize) {
    let key = ["excl", "incl", "above"][which];
    let list: Vec<String> = match which {
        0 => cx.cfg.show.exclude.clone(),
        1 => cx.cfg.show.include.clone(),
        _ => cx.cfg.show.stay_below.clone(),
    };
    let (_, dh) = ui.measure_wrapped(Font::Small, desc, ui.w - 40.0);
    let chips_w: Vec<f32> = list.iter().map(|a| ui.measure(Font::Body, a).0 + 48.0).collect();
    let avail = ui.w - 36.0;
    let (mut x, mut lines) = (0.0, if list.is_empty() { 0 } else { 1 });
    for w in &chips_w {
        if x + w > avail && x > 0.0 {
            lines += 1;
            x = 0.0;
        }
        x += w + 8.0;
    }
    let h = 64.0 + dh + lines as f32 * 38.0 + 44.0;
    let mut remove = None;
    let mut add: Option<String> = None;
    ui.card(h, |ui, r| {
        ui.text(Font::Body, title, r.x + 18.0, r.y + 14.0, pal::TEXT);
        ui.text_wrapped(Font::Small, desc, r.x + 18.0, r.y + 36.0, r.w - 36.0, pal::DIM);
        let mut x = r.x + 18.0;
        let mut y = r.y + 46.0 + dh;
        for (i, a) in list.iter().enumerate() {
            if x + chips_w[i] > r.right() - 18.0 && x > r.x + 18.0 {
                x = r.x + 18.0;
                y += 38.0;
            }
            let (_, rm, w) = ui.chip(idn(key, i), a, pal::DIM, true, x, y);
            if rm {
                remove = Some(i);
            }
            x += w + 8.0;
        }
        let y = r.bottom() - 46.0;
        let buf = match which {
            0 => &mut cx.ps.new_exclude,
            1 => &mut cx.ps.new_include,
            _ => &mut cx.ps.new_above,
        };
        ui.text_field(id(&format!("{key}-new")), buf, Rect::new(r.x + 18.0, y, 220.0, 32.0), "app.exe");
        let (b, br) = ui.button(id(&format!("{key}-add")), Some(icon::ADD), "Add", ButtonKind::Secondary, r.x + 246.0, y);
        if b && !buf.trim().is_empty() {
            let mut name = buf.trim().to_string();
            if !name.to_ascii_lowercase().ends_with(".exe") {
                name.push_str(".exe");
            }
            add = Some(name);
            buf.clear();
        }
        let (pick, pr) = ui.button(id(&format!("{key}-pick")), Some(icon::LIST), "Pick a running app", ButtonKind::Subtle, br.right() + 8.0, y);
        if pick {
            ui.open_menu(id(&format!("{key}-menu")), Rect::new(pr.x, pr.y, 260.0, pr.h), cx.apps.to_vec());
        }
        if let Some(i) = ui.menu_choice(id(&format!("{key}-menu"))) {
            add = cx.apps.get(i).cloned();
        }
    });
    let target = match which {
        0 => &mut cx.cfg.show.exclude,
        1 => &mut cx.cfg.show.include,
        _ => &mut cx.cfg.show.stay_below,
    };
    if let Some(i) = remove {
        target.remove(i);
        cx.changed = true;
    }
    if let Some(a) = add {
        if !target.iter().any(|x| x.eq_ignore_ascii_case(&a)) {
            target.push(a);
            cx.changed = true;
        }
    }
}

// ---------------------------------------------------------------- In-game

fn ingame(ui: &mut Ui, cx: &mut Ctx) {
    ui.note(
        "No window can appear over a game running in exclusive fullscreen. Most games are fine (they run borderless, or Windows does it for them), but some aren't. For those, Delta draws through RivaTuner Statistics Server: a free tool that draws inside games and that anti-cheat systems generally accept. Delta itself never loads anything into a game.",
        pal::DIM,
    );
    let (title, desc, color, label) = match cx.status.rtss_state {
        RtssState::Running => ("RivaTuner is running", "Delta draws inside exclusive-fullscreen games through it.", pal::GOOD, "Running"),
        RtssState::Installed => ("RivaTuner isn't running", "Start it, and Delta can draw inside exclusive-fullscreen games.", pal::WARN, "Not running"),
        RtssState::Missing => ("RivaTuner isn't installed", "It's free from Guru3D. MSI Afterburner isn't needed, just RivaTuner Statistics Server.", pal::WARN, "Not installed"),
    };
    let state = cx.status.rtss_state;
    ui.setting(Some(icon::GAME), title, desc, 220.0, |ui, r| match state {
        RtssState::Running => pill(ui, r, label, color),
        RtssState::Installed => {
            let (b, _) = ui.button(id("rtss-start"), Some(icon::POWER), "Start RivaTuner", ButtonKind::Primary, r.right() - 160.0, r.y + (r.h - 32.0) / 2.0);
            if b {
                cx.actions.push(Action::StartRtss);
            }
        }
        RtssState::Missing => {
            let (b, _) = ui.button(id("rtss-get"), Some(icon::FOLDER), "Get RivaTuner", ButtonKind::Primary, r.right() - 150.0, r.y + (r.h - 32.0) / 2.0);
            if b {
                cx.actions.push(Action::OpenUrl(RTSS_DOWNLOAD.into()));
            }
        }
    });
    let mut mode = match cx.cfg.rtss.bridge {
        Bridge::Off => 0,
        Bridge::Exclusive => 1,
        Bridge::Always => 2,
    };
    ui.setting(Some(icon::VIEW), "Draw through RivaTuner", "Exclusive fullscreen only is best: other games keep the smoother window overlay.", 380.0, |ui, r| {
        if ui.segmented(id("bridge"), &["Off", "Exclusive fullscreen", "All games"], &mut mode, r) {
            cx.cfg.rtss.bridge = [Bridge::Off, Bridge::Exclusive, Bridge::Always][mode];
            cx.changed = true;
        }
    });
    if let Some(n) = &cx.status.exclusive_note {
        ui.note(n, pal::WARN);
    }

    ui.section("Setting up RivaTuner");
    ui.note("1. In RivaTuner, turn off \"Show own statistics\" so only Delta's overlay shows, not RivaTuner's own frame counter.", pal::DIM);
    ui.note("2. Leave \"Show On-Screen Display\" on: it's RivaTuner's master switch, and turning it off hides Delta too.", pal::DIM);
    ui.note("Through RivaTuner, FPS, frame time and the frame time graph update every frame, like Afterburner's. RivaTuner draws those in its own font; the rest of the panel keeps your theme and updates about once a second. In borderless and windowed games Delta uses its own window overlay instead.", pal::DIM);
}

// ---------------------------------------------------------------- Advanced

fn advanced(ui: &mut Ui, cx: &mut Ctx) {
    let mut poll = cx.cfg.poll_ms as f32;
    ui.setting(Some(icon::REFRESH), "Sensor update interval", "How often temperatures, clocks and usage are read. Faster costs a little more CPU.", 300.0, |ui, r| {
        if ui.slider(id("poll"), &mut poll, 250.0, 5000.0, 250.0, &|v| format!("{:.2} s", v / 1000.0), r, 300.0) {
            cx.cfg.poll_ms = poll as u32;
            cx.changed = true;
        }
    });
    let mut hz = cx.cfg.refresh_hz as f32;
    ui.setting(Some(icon::SPEED), "Overlay refresh rate", "How often FPS and the frame time graph redraw.", 300.0, |ui, r| {
        if ui.slider(id("hz"), &mut hz, 5.0, 144.0, 1.0, &|v| format!("{v:.0} Hz"), r, 300.0) {
            cx.cfg.refresh_hz = hz as u32;
            cx.changed = true;
        }
    });
    let mut hist = cx.cfg.history as f32;
    ui.setting(None, "Sensor graph length", "How many readings a sensor graph shows.", 300.0, |ui, r| {
        if ui.slider(id("hist"), &mut hist, 10.0, 300.0, 5.0, &|v| format!("{v:.0}"), r, 300.0) {
            cx.cfg.history = hist as usize;
            cx.changed = true;
        }
    });
    let mut ft = cx.cfg.frametime_samples as f32;
    ui.setting(None, "Frame time graph length", "How many frames the frame time graph shows.", 300.0, |ui, r| {
        if ui.slider(id("ft"), &mut ft, 30.0, 600.0, 10.0, &|v| format!("{v:.0}"), r, 300.0) {
            cx.cfg.frametime_samples = ft as usize;
            cx.changed = true;
        }
    });
    let gpus: Vec<String> = (1..=4).map(|i| format!("GPU {i}")).collect();
    let mut g = (cx.cfg.gpu_index as usize).min(3);
    ui.setting(Some(icon::CHIP), "Graphics card", "Which GPU to read if you have more than one. Takes effect after restarting Delta.", 160.0, |ui, r| {
        if ui.dropdown(id("gpu"), &gpus, &mut g, r, 160.0) {
            cx.cfg.gpu_index = g as u32;
            cx.changed = true;
        }
    });
    let mut rtss = cx.cfg.rtss.compat;
    ui.setting(Some(icon::TEXT), "Support apps made for RTSS", "Lets HWiNFO, AIDA64, OBS HUD and others show their text through Delta while RivaTuner isn't running.", 120.0, |ui, r| {
        if ui.toggle(id("rtss"), &mut rtss, r) {
            cx.changed = true;
        }
    });
    cx.cfg.rtss.compat = rtss;

    ui.section("Files");
    let y = ui.y;
    let (a, r) = ui.button(id("open-cfg"), Some(icon::TEXT), "Open settings file", ButtonKind::Secondary, ui.x, y);
    let (b, _) = ui.button(id("open-themes"), Some(icon::FOLDER), "Open themes folder", ButtonKind::Secondary, r.right() + 8.0, y);
    if a {
        cx.actions.push(Action::OpenConfig);
    }
    if b {
        cx.actions.push(Action::OpenThemes);
    }
    ui.y += 44.0;

    ui.section("Updates");
    let mut auto = cx.cfg.auto_update;
    ui.setting(Some(icon::REFRESH), "Update automatically", "Download new versions from GitHub in the background. They're checked against GitHub's checksum and start the next time Delta does.", 120.0, |ui, r| {
        if ui.toggle(id("auto-update"), &mut auto, r) {
            cx.changed = true;
        }
    });
    cx.cfg.auto_update = auto;
    let (msg, ready) = cx.status.update.clone();
    ui.setting(None, if ready { "Update ready" } else { "Version" }, &msg, 200.0, |ui, r| {
        let (label, action) = if ready { ("Restart now", Action::RestartUpdate) } else { ("Check now", Action::CheckUpdates) };
        let kind = if ready { ButtonKind::Primary } else { ButtonKind::Secondary };
        let (b, _) = ui.button(id("update-btn"), Some(icon::REFRESH), label, kind, r.right() - 140.0, r.y + (r.h - 32.0) / 2.0);
        if b {
            cx.actions.push(action);
        }
    });

    ui.section("About");
    ui.note(&format!("Delta {} · open source under the MIT or Apache-2.0 license.", cx.status.version), pal::DIM);
    let y = ui.y;
    let (gh, _) = ui.button(id("github"), Some(icon::LIST), "Delta on GitHub", ButtonKind::Subtle, ui.x, y);
    if gh {
        cx.actions.push(Action::OpenUrl(format!("https://github.com/{}", crate::update::REPO)));
    }
    ui.y += 40.0;
}
