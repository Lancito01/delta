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
    /// Install, pause, resume or remove the CPU temperature driver.
    Driver(crate::pawnio::Op),
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
    pub cpu_temp: metrics::cputemp::Status,
    /// The current reading, formatted ("58 °C").
    pub cpu_temp_now: Option<String>,
    pub driver: crate::pawnio::Job,
    pub driver_version: Option<String>,
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
    /// The driver's install button was pressed; the warning and the real button are showing.
    pub confirm_install: bool,
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
    ui.tooltip();
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

    cpu_temperature(ui, cx);

    ui.section("Quick start");
    ui.note("Pick what to show under Metrics, how it looks under Appearance, and when it appears under When to show. Every change shows up on the overlay right away.", pal::DIM);
    if !cx.status.diag.is_empty() {
        ui.section("Last app in front");
        ui.note(&cx.status.diag, pal::FAINT);
    }
}

/// The CPU temperature's state, and the buttons that manage the driver it's read through.
fn cpu_temperature(ui: &mut Ui, cx: &mut Ctx) {
    use crate::metrics::cputemp::Status as Temp;
    use crate::pawnio::{Job, Op};
    let st = cx.status;
    ui.section("CPU temperature");
    let version = st.driver_version.as_ref().map_or(String::new(), |v| format!(" {v}"));
    let (label, color, desc) = match &st.cpu_temp {
        Temp::Unknown => ("Checking".to_string(), pal::FAINT, "Looking for the sensor…".to_string()),
        Temp::Working => (
            st.cpu_temp_now.clone().unwrap_or_else(|| "Working".into()),
            pal::GOOD,
            format!("Read from the processor's own sensor, through the PawnIO driver{version}. Add \"CPU temperature\" to a row under Metrics to see it on the overlay."),
        ),
        Temp::NotInstalled => (
            "Needs a driver".to_string(),
            pal::WARN,
            "Windows doesn't let apps read the processor's temperature sensor themselves. Delta can read it through PawnIO, a small open-source driver that other sensor and fan apps use too.".to_string(),
        ),
        Temp::Paused => ("Paused".to_string(), pal::FAINT, "The driver is installed but switched off, so it isn't loaded, also after a restart. Resume it to see the temperature again.".to_string()),
        Temp::NeedsAdmin => ("Needs administrator".to_string(), pal::WARN, "The driver only answers apps that run as administrator. Restart Delta as administrator to see the temperature.".to_string()),
        Temp::Blocked(why) => (
            "Unavailable".to_string(),
            pal::WARN,
            format!("The driver is installed, but {why}. Restarting Windows may fix it; if it doesn't, an anti-cheat or a Windows security setting is blocking the driver. Everything else in Delta works as usual."),
        ),
        Temp::Unsupported => ("Not supported".to_string(), pal::FAINT, "Delta reads the temperature of Intel Core and AMD Ryzen processors. This one isn't supported yet.".to_string()),
    };
    ui.setting(Some(icon::CHIP), "Processor temperature", &desc, 190.0, |ui, r| pill(ui, r, &label, color));

    if let Job::Running(op) = &st.driver {
        ui.note(op.doing(), pal::DIM);
        return;
    }
    let installed = matches!(st.cpu_temp, Temp::Working | Temp::Paused | Temp::NeedsAdmin | Temp::Blocked(_));
    if !matches!(st.cpu_temp, Temp::NotInstalled) {
        cx.ps.confirm_install = false;
    }
    let y = ui.y + 4.0;
    let mut x = ui.x;
    let mut button = |ui: &mut Ui, name: &str, glyph: Option<char>, label: &str, kind: ButtonKind| {
        let (clicked, r) = ui.button(id(name), glyph, label, kind, x, y);
        x = r.right() + 8.0;
        clicked
    };
    match &st.cpu_temp {
        Temp::NotInstalled if cx.ps.confirm_install => {
            ui.note(
                &format!("This installs PawnIO {}, a signed kernel driver, with its own installer: downloaded from PawnIO's GitHub release and checked against a known checksum first. Only administrator apps can use the driver. Some anti-cheats may refuse to start a game while it's loaded; you can pause it here without uninstalling it.", crate::pawnio::VERSION),
                pal::WARN,
            );
            let y = ui.y + 4.0;
            let (go, r) = ui.button(id("drv-install-go"), Some(icon::SHIELD), "Install", ButtonKind::Primary, ui.x, y);
            let (cancel, _) = ui.button(id("drv-install-cancel"), None, "Cancel", ButtonKind::Secondary, r.right() + 8.0, y);
            if go {
                cx.actions.push(Action::Driver(Op::Install));
            }
            if go || cancel {
                cx.ps.confirm_install = false;
            }
            ui.y += 48.0;
        }
        Temp::NotInstalled => {
            if button(ui, "drv-install", Some(icon::SHIELD), "Install the driver…", ButtonKind::Primary) {
                cx.ps.confirm_install = true;
            }
            ui.y += 48.0;
        }
        _ if installed => {
            if st.cpu_temp == Temp::NeedsAdmin && button(ui, "drv-admin", Some(icon::SHIELD), "Restart as administrator", ButtonKind::Primary) {
                cx.actions.push(Action::RestartAdmin);
            }
            if st.cpu_temp == Temp::Paused {
                if button(ui, "drv-resume", Some(icon::POWER), "Resume", ButtonKind::Primary) {
                    cx.actions.push(Action::Driver(Op::Resume));
                }
            } else if button(ui, "drv-pause", Some(icon::POWER), "Pause", ButtonKind::Secondary) {
                cx.actions.push(Action::Driver(Op::Pause));
            }
            if button(ui, "drv-uninstall", Some(icon::DELETE), "Uninstall", ButtonKind::Danger) {
                cx.actions.push(Action::Driver(Op::Uninstall));
            }
            ui.y += 48.0;
        }
        _ => {}
    }
    match &st.driver {
        Job::Done(op, Ok(false)) => ui.note(op.done(), pal::GOOD),
        Job::Done(op, Ok(true)) => ui.note(&format!("{} Restart Windows to finish.", op.done()), pal::WARN),
        Job::Done(_, Err(e)) => ui.note(e, pal::BAD),
        _ => {}
    }
    if installed {
        ui.note("Some anti-cheats may refuse to start a game while this driver is loaded. Pause unloads it and keeps it unloaded until you resume it; Uninstall removes it. Other apps that use PawnIO (fan, RGB and sensor tools) lose it too.", pal::FAINT);
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

fn metric_chip_layout(widths: &[f32], left: f32, right: f32, top: f32) -> (Vec<Rect>, f32, f32) {
    let mut x = left;
    let mut y = top;
    let mut rects = Vec::with_capacity(widths.len());
    for width in widths {
        if x + width > right && x > left {
            x = left;
            y += 38.0;
        }
        rects.push(Rect::new(x, y, *width, 30.0));
        x += width + 8.0;
    }
    (rects, x, y)
}

/// Why a metric can't be read right now (the pill is greyed out and says so on hover).
fn metric_unavailable(cx: &Ctx, metric: &str) -> Option<&'static str> {
    use crate::metrics::cputemp::Status as Temp;
    if metric != "cpu.temp" {
        return None;
    }
    match cx.status.cpu_temp {
        Temp::Unknown | Temp::Working => None,
        Temp::NotInstalled => Some("Unavailable: the CPU temperature driver isn't installed. Set it up on the Overview page."),
        Temp::Paused => Some("Unavailable: the CPU temperature driver is paused. Resume it on the Overview page."),
        Temp::NeedsAdmin => Some("Unavailable: the driver only answers apps running as administrator. Restart Delta as administrator."),
        Temp::Blocked(_) => Some("Unavailable: the CPU temperature driver can't be reached. See the Overview page."),
        Temp::Unsupported => Some("Unavailable: this processor isn't supported yet."),
    }
}

fn draw_metric_pill(ui: &mut Ui, r: Rect, label: &str, color: Color, removable: bool, dim: bool) {
    let hovered = r.contains(ui.inp.mouse);
    ui.fill(r, if hovered { pal::CONTROL_HOVER } else { pal::CONTROL }, 15.0);
    ui.circle(r.x + 13.0, r.y + 15.0, 3.5, if dim { pal::FAINT.with_alpha(0.5) } else { color });
    ui.text_mid(Font::Body, label, r.x + 22.0, r, if dim { pal::FAINT } else { pal::TEXT });
    if removable {
        ui.glyph(icon::CLOSE, r.right() - 15.0, r.y + 15.0, pal::FAINT, false);
    }
}

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
        let unavailable: Vec<(String, &'static str)> = cx.cfg.rows[i].metrics.iter().filter_map(|m| metric_unavailable(cx, m).map(|w| (m.clone(), w))).collect();
        let why_of = |m: &str| unavailable.iter().find(|(k, _)| k == m).map(|(_, w)| *w);
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
            let left = r.x + 18.0;
            let right = r.right() - 18.0;
            let top = r.y + 60.0;
            let widths: Vec<f32> = row.metrics.iter().map(|m| ui.measure(Font::Body, &metrics::display_name(m)).0 + 48.0).collect();
            let (targets, _, _) = metric_chip_layout(&widths, left, right, top);
            let mut visuals = Vec::with_capacity(row.metrics.len());
            let animate_chips = ui.st.metric_drag.is_some_and(|(row_index, _)| row_index == i) || ui.st.metric_reorder == Some(i);
            for (j, metric) in row.metrics.iter().enumerate() {
                let target = targets[j];
                let x_id = id(&format!("metric-chip-x{i}-{metric}"));
                let y_id = id(&format!("metric-chip-y{i}-{metric}"));
                let x = if animate_chips { ui.anim(x_id, target.x, 18.0) } else { ui.snap_anim(x_id, target.x) };
                let y = if animate_chips { ui.anim(y_id, target.y, 18.0) } else { ui.snap_anim(y_id, target.y) };
                visuals.push((metric.clone(), Rect::new(x, y, target.w, target.h)));
            }
            let mut started_drag = false;
            if !ui.blocked && !ui.st.swallow && ui.clip.is_none_or(|clip| clip.contains(ui.inp.mouse)) && ui.inp.pressed {
                if let Some((j, _)) = visuals.iter().enumerate().find(|(_, (_, rect))| {
                    Rect::new(rect.x, rect.y, rect.w - 26.0, rect.h).contains(ui.inp.mouse)
                }) {
                    ui.st.metric_drag = Some((i, j));
                    started_drag = true;
                }
            }
            if let Some((_, source)) = ui.st.metric_drag.filter(|(row_index, _)| *row_index == i) {
                if !started_drag && (ui.inp.down || ui.inp.released) {
                    if let Some((target, (_, rect))) = visuals.iter().enumerate().find(|(target, (_, rect))| {
                        *target != source
                            && Rect::new(rect.x, rect.y, rect.w - 26.0, rect.h).contains(ui.inp.mouse)
                    }) {
                        let insertion = target + usize::from(ui.inp.mouse.0 >= rect.x + rect.w / 2.0);
                        let destination = if insertion > source { insertion - 1 } else { insertion };
                        if destination != source && source < row.metrics.len() {
                            let metric = row.metrics.remove(source);
                            row.metrics.insert(destination.min(row.metrics.len()), metric);
                            ui.st.metric_drag = Some((i, destination));
                            ui.st.metric_reorder = Some(i);
                            cx.changed = true;
                        }
                    }
                }
                if ui.inp.released {
                    ui.st.metric_drag = None;
                }
            }

            let dragging = ui.st.metric_drag.is_some_and(|(row_index, _)| row_index == i);
            let animate_chips = dragging || ui.st.metric_reorder == Some(i);
            let widths: Vec<f32> = row.metrics.iter().map(|m| ui.measure(Font::Body, &metrics::display_name(m)).0 + 48.0).collect();
            let (targets, mut x, mut y) = metric_chip_layout(&widths, left, right, top);
            let mut drop_metric = None;
            let mut settled = true;
            for (j, metric) in row.metrics.iter().enumerate() {
                let target = targets[j];
                let x_id = id(&format!("metric-chip-x{i}-{metric}"));
                let y_id = id(&format!("metric-chip-y{i}-{metric}"));
                let current_x = if animate_chips { ui.anim(x_id, target.x, 18.0) } else { ui.snap_anim(x_id, target.x) };
                let current_y = if animate_chips { ui.anim(y_id, target.y, 18.0) } else { ui.snap_anim(y_id, target.y) };
                if animate_chips && ((current_x - target.x).abs() > 0.5 || (current_y - target.y).abs() > 0.5) {
                    settled = false;
                }
                let rect = Rect::new(current_x, current_y, target.w, target.h);
                let c = group_color_static(cx.themes, &cx.cfg.theme, metric);
                let why = why_of(metric);
                if let Some(why) = why {
                    if !dragging && !ui.blocked && rect.contains(ui.inp.mouse) && ui.clip.is_none_or(|clip| clip.contains(ui.inp.mouse)) {
                        ui.st.tooltip = Some(why.to_string());
                    }
                }
                if dragging {
                    if ui.st.metric_drag == Some((i, j)) {
                        ui.fill(rect, pal::ACCENT_SOFT, 15.0);
                        ui.stroke(rect, pal::ACCENT.with_alpha(0.65), 15.0, 1.0);
                    } else {
                        draw_metric_pill(ui, rect, &metrics::display_name(metric), c, true, why.is_some());
                    }
                } else {
                    let (_, removed, _) = ui.chip_ex(idn(&format!("chip{i}"), j), &metrics::display_name(metric), c, true, why.is_some(), rect.x, rect.y);
                    if removed {
                        drop_metric = Some(j);
                    }
                }
            }
            if ui.st.metric_reorder == Some(i) && !dragging && settled {
                ui.st.metric_reorder = None;
            }
            if !dragging {
                if let Some(j) = drop_metric.take() {
                    let gone = row.metrics.remove(j);
                    row.graph.retain(|g| *g != gone);
                    ui.st.metric_drag = None;
                    cx.changed = true;
                }
            }
            if x + 92.0 > right && x > left {
                x = left;
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
                let mut options: Vec<String> = row.metrics.iter().filter(|m| *m != "cpu.cores" && *m != "app").cloned().collect();
                if !options.iter().any(|m| m == "frametime") && row.metrics.iter().any(|m| m.starts_with("fps")) {
                    options.push("frametime".into());
                }
                if !options.is_empty() {
                    ui.text_mid(Font::Body, "Graph shows", r.x + 18.0, Rect::new(r.x, gy, 0.0, 36.0), pal::DIM);
                    let names: Vec<String> = options.iter().map(|o| metrics::display_name(o)).collect();
                    let mut graph_ids: Vec<String> = row.graph.iter().filter(|g| options.contains(g)).cloned().collect();
                    if graph_ids.is_empty() {
                        graph_ids.push(options[0].clone());
                    }
                    let mut selected: Vec<usize> = options.iter().enumerate().filter_map(|(index, option)| graph_ids.contains(option).then_some(index)).collect();
                    let old_graph_ids = graph_ids.clone();
                    if ui.multi_dropdown(idn("row-graph", i), &names, &mut selected, Rect::new(r.x + 120.0, gy, 220.0, 36.0), 220.0) {
                        if selected.is_empty() {
                            selected = options.iter().position(|option| old_graph_ids.contains(option)).into_iter().collect();
                        }
                        row.graph = selected.iter().filter_map(|index| options.get(*index).cloned()).collect();
                        cx.changed = true;
                    }
                }
            }
            if dragging {
                if let Some((_, source)) = ui.st.metric_drag.filter(|(row_index, _)| *row_index == i) {
                    if let Some(metric) = row.metrics.get(source) {
                        let w = widths[source];
                        let floating = Rect::new(ui.inp.mouse.0 - w / 2.0, ui.inp.mouse.1 - 17.0, w, 30.0);
                        let c = group_color_static(cx.themes, &cx.cfg.theme, metric);
                        let dim = why_of(metric).is_some();
                        draw_metric_pill(ui, floating, &metrics::display_name(metric), c, false, dim);
                        ui.stroke(floating, pal::ACCENT, 15.0, 1.5);
                    }
                }
            }
        });
    }
    if let Some((a, b)) = swap {
        cx.cfg.rows.swap(a, b);
        ui.st.metric_drag = None;
        ui.st.metric_reorder = None;
        cx.changed = true;
    }
    if let Some(i) = remove {
        cx.cfg.rows.remove(i);
        ui.st.metric_drag = None;
        ui.st.metric_reorder = None;
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
            let picked = &mut cx.cfg.rows[row];
            if added {
                picked.metrics.retain(|x| x != mid);
                picked.graph.retain(|x| x != mid);
            } else {
                picked.metrics.push(mid.to_string());
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
