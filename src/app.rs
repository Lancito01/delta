//! Ties it together: config + theme, sensors, show rules, the overlay, RTSS compatibility, tray.

use crate::compose::{Frame, compose};
use crate::config::{Bridge, Config, ConfigFile, Monitor, Paths, Position, ShowMode};
use crate::metrics::{self, Sampler, frames, system};
use crate::overlay::gfx::Gfx;
use crate::overlay::{self, Look, Overlay};
use crate::theme::{self, Layout, Theme};
use crate::tray::{self, Entry, Tray};
use crate::ui::Settings;
use crate::ui::pages::{Action, RtssState, Status, ThemeInfo};
use crate::{hotkey, log, osd, rtss, ui, update};
use std::collections::HashSet;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

const HK_TOGGLE: i32 = 1;
const HK_THEME: i32 = 2;
const HK_POSITION: i32 = 3;
const TIMER: usize = 1;
/// Fast timer for the settings window's animations (only while it's open).
const TIMER_UI: usize = 2;

const CMD_TOGGLE: u32 = 100;
const CMD_THEME: u32 = 200;
const CMD_POSITION: u32 = 300;
const CMD_MODE: u32 = 400;
const CMD_EDIT: u32 = 500;
const CMD_FOLDER: u32 = 501;
const CMD_ADMIN: u32 = 502;
const CMD_QUIT: u32 = 503;
const CMD_SETTINGS: u32 = 504;
const CMD_CHECK_UPDATE: u32 = 505;
const CMD_RESTART_UPDATE: u32 = 506;
/// A background update check finished.
const WM_UPDATE_DONE: u32 = WM_APP + 3;
/// Sent by a second copy of Delta (or `--settings`) to open the settings window.
pub const WM_OPEN_SETTINGS: u32 = WM_APP + 2;
const CMD_LAYOUT: u32 = 600;

pub struct App {
    paths: Paths,
    cfg_file: ConfigFile,
    cfg: Config,
    theme: Theme,
    theme_stamp: Option<std::time::SystemTime>,
    sampler: Sampler,
    gfx: Rc<Gfx>,
    overlays: Vec<Overlay>,
    rtss: rtss::Server,
    tray: Tray,
    hwnd: HWND,
    /// The user's show/hide toggle.
    visible: bool,
    elevated: bool,
    fps_note: Option<String>,
    last_cfg_check: Instant,
    last_raise: Instant,
    last_gen: u64,
    last_show: bool,
    last_errors: Vec<String>,
    force: bool,
    render_failures: u32,
    instance: HINSTANCE,
    settings: Option<Settings>,
    /// When to write the config after a change in the settings window.
    save_at: Option<Instant>,
    last_frame: Frame,
    theme_infos: Vec<ThemeInfo>,
    apps_cache: (Option<Instant>, Vec<String>),
    bridge: Option<crate::bridge::Bridge>,
    bridge_active: bool,
    last_rtss_app: Option<rtss::ClientApp>,
    /// Exe names already told about exclusive fullscreen.
    notified: HashSet<String>,
    exclusive_note: Option<String>,
    diag: String,
    diag_key: String,
    diag_at: Instant,
    update: std::sync::Arc<std::sync::Mutex<update::State>>,
    /// When to check for updates next.
    next_update_check: Instant,
    /// The user asked for this check (tell them how it went).
    update_manual: bool,
}

/// Is a Direct3D app running in exclusive fullscreen right now?
fn exclusive_fullscreen() -> bool {
    use windows::Win32::UI::Shell::{QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState};
    unsafe { SHQueryUserNotificationState().is_ok_and(|s| s == QUNS_RUNNING_D3D_FULL_SCREEN) }
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn theme_mtime(paths: &Paths, name: &str) -> Option<std::time::SystemTime> {
    std::fs::metadata(paths.themes.join(format!("{name}.toml"))).and_then(|m| m.modified()).ok()
}

impl App {
    fn new(instance: HINSTANCE, hwnd: HWND) -> Result<App, String> {
        let paths = Paths::resolve();
        let _ = std::fs::create_dir_all(&paths.dir);
        log::init(paths.dir.join("delta.log"));
        theme::install_builtins(&paths.themes);
        let mut cfg_file = ConfigFile::new(paths.config.clone());
        let (cfg, cfg_err) = cfg_file.load();
        let (theme, theme_err) = match theme::load(&paths.themes, &cfg.theme) {
            Ok(t) => (t, None),
            Err(e) => (theme::load(&paths.themes, "minimal").map_err(|e| e.to_string())?, Some(e)),
        };
        let elevated = system::is_elevated();
        log!("Delta {} starting; config {}; elevated: {elevated}", env!("CARGO_PKG_VERSION"), paths.config.display());
        let fps_note = match frames::start() {
            Ok(()) => None,
            Err(e) => {
                log!("frames: {e}");
                Some(if elevated { "FPS unavailable".to_string() } else { "run as admin for FPS".to_string() })
            }
        };
        let sampler = Sampler::start(cfg.poll_ms, cfg.history, cfg.gpu_index);
        let gfx = Rc::new(Gfx::new().map_err(|e| format!("couldn't start graphics: {e}"))?);
        let tray = Tray::new(hwnd);
        let mut app = App {
            theme_stamp: theme_mtime(&paths, &cfg.theme),
            paths,
            cfg_file,
            visible: cfg.show.start_visible,
            cfg,
            theme,
            sampler,
            gfx,
            overlays: vec![],
            rtss: rtss::Server::new(),
            tray,
            hwnd,
            elevated,
            fps_note,
            last_cfg_check: Instant::now(),
            last_raise: Instant::now(),
            last_gen: u64::MAX,
            last_show: false,
            last_errors: vec![],
            force: true,
            render_failures: 0,
            instance,
            settings: None,
            save_at: None,
            last_frame: Frame::default(),
            theme_infos: vec![],
            apps_cache: (None, vec![]),
            bridge: None,
            bridge_active: false,
            last_rtss_app: None,
            notified: HashSet::new(),
            exclusive_note: None,
            diag: String::new(),
            diag_key: String::new(),
            diag_at: Instant::now(),
            update: std::sync::Arc::new(std::sync::Mutex::new(update::State::Idle)),
            next_update_check: Instant::now() + Duration::from_secs(30),
            update_manual: false,
        };
        update::cleanup();
        let mut errors: Vec<String> = cfg_err.into_iter().chain(theme_err).collect();
        errors.extend(app.apply_hotkeys());
        app.report(errors);
        app.set_timer();
        if !app.elevated {
            app.tray.notify("Delta is running", "FPS and frame times need administrator rights. Right-click the tray icon to restart as administrator.");
        }
        Ok(app)
    }

    fn set_timer(&self) {
        unsafe {
            SetTimer(Some(self.hwnd), TIMER, (1000 / self.cfg.refresh_hz.max(1)).max(7), None);
        }
    }

    fn apply_hotkeys(&self) -> Vec<String> {
        let h = &self.cfg.hotkeys;
        hotkey::register(self.hwnd, &[(HK_TOGGLE, &h.toggle), (HK_THEME, &h.cycle_theme), (HK_POSITION, &h.cycle_position)])
    }

    fn report(&mut self, errors: Vec<String>) {
        for e in &errors {
            log!("{e}");
        }
        if !errors.is_empty() && errors != self.last_errors {
            self.tray.notify("Delta settings problem", &errors.join("\n"));
        }
        self.last_errors = errors;
    }

    /// Re-reads config.toml (edited by hand or by the tray menu).
    fn reload(&mut self) {
        let (cfg, err) = self.cfg_file.load();
        self.apply(cfg, err.into_iter().collect(), true);
        if let Some(s) = &mut self.settings {
            if self.save_at.is_none() {
                s.cfg = self.cfg.clone();
                s.needs_redraw = true;
            }
        }
        log!("config reloaded (theme {})", self.cfg.theme);
    }

    /// Switches to `cfg`. `theme_files` = theme files may have changed on disk too.
    fn apply(&mut self, cfg: Config, mut errors: Vec<String>, theme_files: bool) {
        self.sampler.poll_ms.store(cfg.poll_ms, Ordering::Relaxed);
        self.sampler.history.store(cfg.history as u32, Ordering::Relaxed);
        let theme_changed = theme_files || !cfg.theme.eq_ignore_ascii_case(&self.cfg.theme);
        let hotkeys_changed = theme_files || cfg.hotkeys.toggle != self.cfg.hotkeys.toggle || cfg.hotkeys.cycle_theme != self.cfg.hotkeys.cycle_theme || cfg.hotkeys.cycle_position != self.cfg.hotkeys.cycle_position;
        self.cfg = cfg;
        if theme_changed {
            match theme::load(&self.paths.themes, &self.cfg.theme) {
                Ok(t) => self.theme = t,
                Err(e) => errors.push(e),
            }
            self.theme_stamp = theme_mtime(&self.paths, &self.cfg.theme);
        }
        if let Some(b) = &mut self.bridge {
            b.reset();
        }
        for o in &mut self.overlays {
            if theme_changed {
                o.reset_theme();
            } else {
                o.sticky.clear();
            }
        }
        if hotkeys_changed {
            errors.extend(self.apply_hotkeys());
        }
        self.set_timer();
        self.report(errors);
        self.force = true;
        self.tray.tooltip(&format!("Delta: {} theme", self.theme.name));
    }

    fn rules_allow(&self, fg: &system::Foreground, rendering: bool) -> bool {
        let exe = fg.exe.to_ascii_lowercase();
        let listed = |l: &[String]| l.iter().any(|x| x.trim().eq_ignore_ascii_case(&exe));
        if !exe.is_empty() && listed(&self.cfg.show.exclude) {
            return false;
        }
        if !self.cfg.show.include.is_empty() && !listed(&self.cfg.show.include) {
            return false;
        }
        match self.cfg.show.mode {
            ShowMode::Always => true,
            ShowMode::Fullscreen => fg.fullscreen,
            ShowMode::Games => rendering || fg.fullscreen,
        }
    }

    fn tick(&mut self) {
        if self.cfg.auto_update && Instant::now() >= self.next_update_check {
            self.check_updates(false);
        }
        if self.save_at.is_some_and(|t| Instant::now() >= t) {
            self.save_at = None;
            if let Err(e) = self.cfg_file.save(&self.cfg) {
                log!("couldn't save settings: {e}");
            }
        }
        if self.last_cfg_check.elapsed() > Duration::from_millis(500) {
            self.last_cfg_check = Instant::now();
            let theme_changed = theme_mtime(&self.paths, &self.cfg.theme) != self.theme_stamp;
            if self.cfg_file.changed() || theme_changed {
                self.reload();
            }
        }

        let fg = system::foreground();
        let own = fg.pid == std::process::id();
        let stats = if frames::running() { frames::stats(fg.pid, self.cfg.frametime_samples) } else { None };

        // RTSS compatibility: publish who we're drawing over, read what other apps want shown.
        self.rtss.tick(self.cfg.rtss.compat);
        let mut osd_layers: Vec<osd::Layer> = vec![];
        let mut osd_changed = false;
        if self.rtss.serving() {
            let mut apps = vec![];
            for pid in frames::active_pids() {
                if let Some(st) = frames::stats(pid, 0) {
                    let (w, h) = if pid == fg.pid { (fg.rect.2 - fg.rect.0, fg.rect.3 - fg.rect.1) } else { (1, 1) };
                    apps.push(rtss::AppInfo { pid, name: system::process_name(pid).unwrap_or_default(), frames_1s: st.frames_1s, frametime_ms: st.frametime, width: w.max(1) as u32, height: h.max(1) as u32 });
                }
            }
            // Without frame tracing, still announce the fullscreen foreground app so RTSS clients
            // (which check that RTSS "sees" the game) keep drawing.
            if !own && fg.pid != 0 && fg.fullscreen && !apps.iter().any(|a| a.pid == fg.pid) {
                apps.push(rtss::AppInfo { pid: fg.pid, name: fg.exe.clone(), frames_1s: 0, frametime_ms: 0.0, width: (fg.rect.2 - fg.rect.0).max(1) as u32, height: (fg.rect.3 - fg.rect.1).max(1) as u32 });
            }
            self.rtss.publish_apps(&apps);
            let macros = osd::Macros { fps: stats.as_ref().map(|s| s.fps), frametime: stats.as_ref().map(|s| s.frametime), api: "D3D".into() };
            let (slots, changed) = self.rtss.slots();
            osd_changed = changed;
            for s in slots {
                osd_layers.extend(osd::parse(&s.text, &macros));
            }
        }

        // Drawing inside games through RTSS: only while RTSS itself runs and draws in the game.
        let exclusive = !own && exclusive_fullscreen();
        let rtss_running = matches!(self.rtss.status, rtss::Status::RtssRunning);
        let rtss_app = if own {
            self.last_rtss_app
        } else if rtss_running && fg.pid != 0 && !system::never_a_game(&fg.exe, None) {
            rtss::client_app(fg.pid)
        } else {
            None
        };
        self.last_rtss_app = rtss_app;
        let rendering = stats.is_some() || rtss_app.is_some();
        // While the settings window is open the overlay stays up, so changes can be seen.
        let show = if self.settings.is_some() {
            self.visible
        } else if own {
            self.last_show
        } else {
            self.visible && self.rules_allow(&fg, rendering)
        };
        self.last_show = show;
        let bridge_on = show
            && rtss_app.is_some()
            && match self.cfg.rtss.bridge {
                Bridge::Off => false,
                Bridge::Exclusive => if own { self.bridge_active } else { exclusive },
                Bridge::Always => true,
            };
        if !own {
            self.exclusive_notice(&fg, exclusive, rtss_running, rtss_app.is_some(), bridge_on);
            self.diagnostics(&fg, exclusive, &stats, rtss_app, bridge_on);
        }
        if self.bridge_active && !bridge_on {
            rtss::client_clear();
            self.bridge_active = false;
            if let Some(b) = &mut self.bridge {
                b.reset();
            }
        }
        let is_game = rendering || fg.fullscreen;
        if !show {
            for o in &mut self.overlays {
                o.show(false);
            }
            return;
        }

        // Only redraw when something changed: new sensor data, new frames, new OSD text, settings.
        let generation = self.sampler.shared.lock().unwrap().generation;
        let fps_rows = self.cfg.rows.iter().any(|r| r.metrics.iter().any(|m| metrics::group_of(m) == "fps"));
        let frames_shown = stats.is_some() && (fps_rows || !osd_layers.is_empty());
        let targets = self.target_monitors(is_game && !own);
        let moved = targets.len() != self.overlays.len() || self.overlays.iter().zip(&targets).any(|(o, m)| o.monitor != m.rect);
        let bridge_due = bridge_on && (!self.bridge_active || generation != self.last_gen || self.force);
        let dirty = self.force || generation != self.last_gen || frames_shown || osd_changed || moved || bridge_due;
        if !dirty {
            return;
        }
        let sample = self.sampler.shared.lock().unwrap().clone();
        self.last_gen = sample.generation;

        let mut frame = Frame { cores: sample.cores, history: sample.history, app: sample.app, fps_note: self.fps_note.clone(), osd: osd_layers, ..Default::default() };
        frame.values = sample.values;
        if let Some(st) = &stats {
            frame.values.insert("fps".into(), st.fps);
            frame.values.insert("frametime".into(), st.frametime);
            if let Some(l) = st.low1 {
                frame.values.insert("fps.low1".into(), l);
            }
            if let Some(l) = st.low01 {
                frame.values.insert("fps.low01".into(), l);
            }
            frame.frametimes = st.recent.clone();
        } else if let Some(fps) = rtss_app.and_then(|a| a.fps) {
            // Without frame tracing (not admin), RTSS's own count still gives an FPS.
            frame.values.insert("fps".into(), fps);
            frame.values.insert("frametime".into(), 1000.0 / fps.max(0.01));
            frame.fps_note = None;
        }
        let mut skip = None;
        if bridge_on && bridge_due {
            self.draw_through_rtss(&frame, rtss_app.unwrap());
        }
        if bridge_on {
            // RTSS draws the overlay in this game; the window would only double it.
            skip = Some(overlay::foreground_monitor().rect);
        }
        self.render_monitors(&frame, &targets, skip);
        self.last_frame = frame;
    }

    // ------------------------------------------------ drawing through RTSS

    fn draw_through_rtss(&mut self, frame: &Frame, app: rtss::ClientApp) {
        if self.bridge.is_none() {
            match crate::bridge::Bridge::new(&self.gfx) {
                Ok(b) => self.bridge = Some(b),
                Err(e) => {
                    log!("RTSS bridge unavailable: {e}");
                    return;
                }
            }
        }
        // Sized like the window overlay. A fullscreen game rendering below the monitor's
        // resolution is stretched to fit it, so the picture shrinks to match.
        let mon = overlay::foreground_monitor();
        let mon_h = (mon.rect.bottom - mon.rect.top).max(1) as f32;
        let dpi = mon.dpi * (app.height as f32 / mon_h).min(4.0);
        let target = crate::bridge::Target { width: app.width, height: app.height, zoom: app.zoom };
        let b = self.bridge.as_mut().unwrap();
        if let Some((text, objects)) = b.update(&self.gfx, &self.cfg, &self.theme, frame, &target, dpi) {
            if rtss::client_show_with(&text, &objects) {
                if !self.bridge_active {
                    log!("drawing through RTSS ({}x{}, zoom {})", app.width, app.height, app.zoom);
                }
            } else {
                log!("RTSS has no free OSD slot");
            }
        }
        self.bridge_active = true;
    }

    /// Once per game: explain why the overlay can't show in an exclusive-fullscreen game.
    fn exclusive_notice(&mut self, fg: &system::Foreground, exclusive: bool, rtss_running: bool, rtss_sees_it: bool, bridge_on: bool) {
        if !exclusive || bridge_on || fg.exe.is_empty() {
            return;
        }
        let game = fg.exe.trim_end_matches(".exe").to_string();
        let msg = if !rtss_running {
            format!("{game} is in exclusive fullscreen, so the overlay can't show over it. Switch it to Borderless in its video settings, or install RivaTuner Statistics Server (free, from Guru3D; Afterburner isn't needed) and Delta will draw through it.")
        } else if self.cfg.rtss.bridge == Bridge::Off {
            format!("{game} is in exclusive fullscreen, so the overlay can't show over it. Turn on \"Draw through RivaTuner\" in Delta's settings, or switch the game to Borderless.")
        } else if !rtss_sees_it {
            format!("{game} is in exclusive fullscreen, and RivaTuner isn't drawing in it yet. Check that RivaTuner's on-screen display is on for this game, or switch the game to Borderless.")
        } else {
            return;
        };
        self.exclusive_note = Some(msg.clone());
        if self.notified.insert(fg.exe.to_ascii_lowercase()) {
            self.tray.notify("Exclusive fullscreen", &msg);
            log!("{msg}");
        }
    }

    /// A one-line explanation of what the overlay is doing, for "it didn't show" reports.
    fn diagnostics(&mut self, fg: &system::Foreground, exclusive: bool, stats: &Option<frames::FrameStats>, rtss_app: Option<rtss::ClientApp>, bridge_on: bool) {
        let key = format!("{}|{}|{}|{}|{}", fg.exe, fg.fullscreen, exclusive, bridge_on, rtss_app.is_some());
        if key == self.diag_key && self.diag_at.elapsed() < Duration::from_secs(10) {
            return;
        }
        self.diag_key = key;
        self.diag_at = Instant::now();
        let frames = match (stats, rtss_app.and_then(|a| a.fps)) {
            (Some(s), _) => format!("{:.0} FPS (frame tracing)", s.fps),
            (None, Some(f)) => format!("{f:.0} FPS (from RivaTuner)"),
            _ => "no frames seen".into(),
        };
        let overlay = if bridge_on {
            "drawn through RivaTuner".to_string()
        } else {
            match self.overlays.first() {
                Some(o) if o.shown() => format!("window overlay, topmost, {} the app", if overlay::is_above(o.hwnd, fg.hwnd) { "above" } else { "BELOW" }),
                _ => "window overlay hidden".into(),
            }
        };
        let yn = |b: bool| if b { "yes" } else { "no" };
        self.diag = format!("{}: covers its monitor {}, exclusive fullscreen {}, {frames}, {overlay}", if fg.exe.is_empty() { "?" } else { &fg.exe }, yn(fg.fullscreen), yn(exclusive));
        log!("diagnostics: {}", self.diag);
    }

    /// Which monitors to draw on right now.
    fn target_monitors(&self, game_in_front: bool) -> Vec<overlay::MonitorInfo> {
        match self.cfg.monitor() {
            Monitor::Main => vec![overlay::primary_monitor()],
            Monitor::FollowGame => vec![if game_in_front { overlay::foreground_monitor() } else { overlay::primary_monitor() }],
            Monitor::Follow => vec![overlay::foreground_monitor()],
            Monitor::All => overlay::monitors(),
            Monitor::Number(n) => vec![overlay::monitors().get(n as usize - 1).copied().unwrap_or_else(overlay::primary_monitor)],
        }
    }

    fn render_monitors(&mut self, frame: &Frame, targets: &[overlay::MonitorInfo], skip: Option<windows::Win32::Foundation::RECT>) {
        let inst = unsafe { HINSTANCE(GetWindowLongPtrW(self.hwnd, GWLP_HINSTANCE) as _) };
        while self.overlays.len() < targets.len() {
            match Overlay::new(inst, self.gfx.clone()) {
                Ok(o) => self.overlays.push(o),
                Err(e) => {
                    log!("couldn't create an overlay window: {e}");
                    return;
                }
            }
        }
        self.overlays.truncate(targets.len());

        let layout_kind = self.cfg.layout.unwrap_or(self.theme.layout);
        let mut failed = None;
        for (o, mon) in self.overlays.iter_mut().zip(targets) {
            // The game on this monitor shows the overlay itself.
            if skip == Some(mon.rect) {
                o.show(false);
                continue;
            }
            o.place(mon.rect);
            o.set_capture_excluded(self.cfg.show.hide_from_capture);
            let scale = mon.dpi * self.cfg.scale;
            let area = ((mon.rect.right - mon.rect.left) as f32, (mon.rect.bottom - mon.rect.top) as f32);
            let (cfg, theme) = (&self.cfg, &self.theme);
            let Some(placed) = o.with_fonts(theme, scale, |fonts, sticky| compose(cfg, theme, layout_kind, scale, frame, fonts, sticky, area)) else { continue };
            let look = Look { theme, scale, opacity: cfg.opacity, bg: cfg.background_opacity, fg: cfg.text_opacity };
            let panel = placed.panel.as_ref().map(|(s, p)| (s, *p));
            match o.render(panel, &placed.layers, &look) {
                Ok(()) => {
                    if !o.shown() {
                        o.show(true);
                        o.raise(overlay::lowest_topmost_of(&self.cfg.show.stay_below));
                    }
                }
                Err(e) => failed = Some(e),
            }
        }
        match failed {
            None => {
                self.render_failures = 0;
                self.force = false;
            }
            Some(e) => {
                self.render_failures += 1;
                log!("render failed: {e}");
                if self.render_failures > 3 {
                    // Most likely the GPU device was lost (driver update, TDR): start over.
                    match Gfx::new() {
                        Ok(g) => {
                            self.overlays.clear();
                            self.gfx = Rc::new(g);
                            self.render_failures = 0;
                            self.force = true;
                            log!("graphics device recreated");
                        }
                        Err(e) => log!("couldn't recreate the graphics device: {e}"),
                    }
                }
            }
        }
        if self.last_raise.elapsed() > Duration::from_secs(2) {
            self.last_raise = Instant::now();
            let below = overlay::lowest_topmost_of(&self.cfg.show.stay_below);
            for o in &self.overlays {
                o.raise(below);
            }
        }
    }

    fn menu(&self) -> Vec<Entry> {
        let item = |id, text: &str, checked| Entry::Item { id, text: text.to_string(), checked, enabled: true };
        let themes = theme::list(&self.paths.themes);
        let mut status = vec![];
        let fps = match &self.fps_note {
            None => "FPS: on (ETW)".to_string(),
            Some(n) => format!("FPS: {n}"),
        };
        status.push(Entry::Item { id: 0, text: fps, checked: false, enabled: false });
        let rtss = match &self.rtss.status {
            rtss::Status::Serving => "RTSS apps: Delta draws their OSD",
            rtss::Status::RtssRunning => "RTSS apps: RTSS is running, so it draws them",
            rtss::Status::Off => "RTSS apps: off",
            rtss::Status::Failed(e) => &format!("RTSS apps: unavailable ({e})"),
        };
        status.push(Entry::Item { id: 0, text: rtss.to_string(), checked: false, enabled: false });
        let gpu = self.sampler.shared.lock().map(|s| format!("GPU: {} ({})", s.gpu_name, s.gpu_source)).unwrap_or_default();
        status.push(Entry::Item { id: 0, text: gpu, checked: false, enabled: false });
        if !self.diag.is_empty() {
            status.push(Entry::Item { id: 0, text: format!("Last app: {}", self.diag), checked: false, enabled: false });
        }

        let hk = |s: &str| if s.trim().is_empty() { String::new() } else { format!("\t{s}") };
        let mut m = vec![
            Entry::Item { id: 0, text: format!("Delta {}", env!("CARGO_PKG_VERSION")), checked: false, enabled: false },
            Entry::Separator,
            item(CMD_SETTINGS, "Settings…", false),
            item(CMD_TOGGLE, &format!("Show overlay{}", hk(&self.cfg.hotkeys.toggle)), self.visible),
            Entry::Sub { text: "Theme".into(), items: themes.iter().enumerate().map(|(i, t)| item(CMD_THEME + i as u32, t, t.eq_ignore_ascii_case(&self.cfg.theme))).collect() },
            Entry::Sub {
                text: "Layout".into(),
                items: vec![
                    item(CMD_LAYOUT, "Theme default", self.cfg.layout.is_none()),
                    item(CMD_LAYOUT + 1, "Stack (rows)", self.cfg.layout == Some(Layout::Stack)),
                    item(CMD_LAYOUT + 2, "Bar (one line)", self.cfg.layout == Some(Layout::Bar)),
                ],
            },
            Entry::Sub { text: "Position".into(), items: Position::ALL.iter().enumerate().map(|(i, p)| item(CMD_POSITION + i as u32, p.label(), *p == self.cfg.position)).collect() },
            Entry::Sub {
                text: "Show".into(),
                items: vec![
                    item(CMD_MODE, "Always", self.cfg.show.mode == ShowMode::Always),
                    item(CMD_MODE + 1, "Only in games", self.cfg.show.mode == ShowMode::Games),
                    item(CMD_MODE + 2, "Only fullscreen apps", self.cfg.show.mode == ShowMode::Fullscreen),
                ],
            },
            Entry::Separator,
            Entry::Sub { text: "Status".into(), items: status },
            item(CMD_EDIT, "Edit settings…", false),
            item(CMD_FOLDER, "Open themes folder", false),
        ];
        if !self.elevated {
            m.push(item(CMD_ADMIN, "Restart as administrator (enables FPS)", false));
        }
        m.push(Entry::Separator);
        let ready = match &*self.update.lock().unwrap() {
            update::State::Ready(v) => Some(v.clone()),
            _ => None,
        };
        match ready {
            Some(v) => m.push(item(CMD_RESTART_UPDATE, &format!("Restart to update to {v}"), false)),
            None => m.push(item(CMD_CHECK_UPDATE, "Check for updates", false)),
        }
        m.push(item(CMD_QUIT, "Quit", false));
        m
    }

    fn command(&mut self, id: u32) {
        let themes = theme::list(&self.paths.themes);
        match id {
            CMD_TOGGLE => self.toggle(),
            CMD_SETTINGS => self.open_settings(),
            CMD_CHECK_UPDATE => self.check_updates(true),
            CMD_RESTART_UPDATE => self.restart_to_update(),
            _ if (CMD_THEME..CMD_THEME + 100).contains(&id) => {
                if let Some(t) = themes.get((id - CMD_THEME) as usize) {
                    self.cfg_file.set("theme", &format!("\"{t}\""));
                    self.reload();
                }
            }
            _ if (CMD_POSITION..CMD_POSITION + 8).contains(&id) => {
                let p = Position::ALL[(id - CMD_POSITION) as usize];
                self.cfg_file.set("position", &format!("\"{}\"", p.key()));
                self.reload();
            }
            _ if (CMD_MODE..CMD_MODE + 3).contains(&id) => {
                let mode = ["always", "games", "fullscreen"][(id - CMD_MODE) as usize];
                self.set_show_mode(mode);
            }
            _ if (CMD_LAYOUT..CMD_LAYOUT + 3).contains(&id) => {
                let text = std::fs::read_to_string(&self.cfg_file.path).unwrap_or_default();
                let cleaned: String = text.lines().filter(|l| !l.trim_start().starts_with("layout =")).map(|l| format!("{l}\n")).collect();
                let _ = std::fs::write(&self.cfg_file.path, cleaned);
                match id - CMD_LAYOUT {
                    1 => self.cfg_file.set("layout", "\"stack\""),
                    2 => self.cfg_file.set("layout", "\"bar\""),
                    _ => {}
                }
                self.reload();
            }
            CMD_EDIT => open(&self.paths.config.to_string_lossy()),
            CMD_FOLDER => open(&self.paths.themes.to_string_lossy()),
            CMD_ADMIN => {
                if let Ok(exe) = std::env::current_exe() {
                    let r = unsafe { ShellExecuteW(None, w!("runas"), &HSTRING::from(exe.as_os_str()), w!("--wait"), PCWSTR::null(), SW_SHOWNORMAL) };
                    if r.0 as isize > 32 {
                        unsafe { PostQuitMessage(0) };
                    }
                }
            }
            CMD_QUIT => unsafe { PostQuitMessage(0) },
            _ => {}
        }
    }

    fn set_show_mode(&mut self, mode: &str) {
        // `mode` lives in [show]; edit that line in place.
        let text = std::fs::read_to_string(&self.cfg_file.path).unwrap_or_default();
        let mut in_show = false;
        let mut done = false;
        let mut out = String::new();
        for l in text.lines() {
            let t = l.trim_start();
            if t.starts_with('[') {
                in_show = t.starts_with("[show]");
            }
            if in_show && !done && t.starts_with("mode") && t[4..].trim_start().starts_with('=') {
                out.push_str(&format!("mode = \"{mode}\"\n"));
                done = true;
                continue;
            }
            out.push_str(l);
            out.push('\n');
        }
        if !done {
            out.push_str(&format!("\n[show]\nmode = \"{mode}\"\n"));
        }
        let _ = std::fs::write(&self.cfg_file.path, out);
        self.reload();
    }

    fn toggle(&mut self) {
        self.visible = !self.visible;
        self.force = true;
        log!("overlay {}", if self.visible { "shown" } else { "hidden" });
    }

    fn cycle_theme(&mut self) {
        let themes = theme::list(&self.paths.themes);
        let i = themes.iter().position(|t| t.eq_ignore_ascii_case(&self.cfg.theme)).map_or(0, |i| (i + 1) % themes.len());
        self.cfg_file.set("theme", &format!("\"{}\"", themes[i]));
        self.reload();
    }

    fn cycle_position(&mut self) {
        let i = Position::ALL.iter().position(|p| *p == self.cfg.position).map_or(0, |i| (i + 1) % Position::ALL.len());
        self.cfg_file.set("position", &format!("\"{}\"", Position::ALL[i].key()));
        self.reload();
    }

    // ------------------------------------------------ updates

    fn check_updates(&mut self, manual: bool) {
        self.next_update_check = Instant::now() + Duration::from_secs(6 * 3600);
        {
            let mut st = self.update.lock().unwrap();
            if matches!(*st, update::State::Checking | update::State::Ready(_)) {
                return;
            }
            *st = update::State::Checking;
        }
        self.update_manual = manual;
        let state = self.update.clone();
        let hwnd = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            let result = update::check_and_install(env!("CARGO_PKG_VERSION"));
            *state.lock().unwrap() = result;
            unsafe {
                let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_UPDATE_DONE, WPARAM(0), LPARAM(0));
            }
        });
    }

    fn update_done(&mut self) {
        let state = self.update.lock().unwrap().clone();
        match &state {
            update::State::Ready(v) => {
                log!("update: {v} downloaded");
                self.tray.notify("Delta update ready", &format!("Delta {v} is downloaded. It starts next time Delta does, or right-click the tray icon and choose Restart to update."));
            }
            update::State::UpToDate => {
                if self.update_manual {
                    self.tray.notify("Delta is up to date", &format!("You have the latest version ({}).", env!("CARGO_PKG_VERSION")));
                }
            }
            update::State::Failed(e) => {
                log!("update check failed: {e}");
                if self.update_manual {
                    self.tray.notify("Couldn't update Delta", e);
                }
            }
            _ => {}
        }
        if let Some(s) = &mut self.settings {
            s.needs_redraw = true;
        }
    }

    fn restart_to_update(&mut self) {
        match update::restart() {
            Ok(()) => unsafe { PostQuitMessage(0) },
            Err(e) => self.tray.notify("Couldn't restart Delta", &e),
        }
    }

    fn update_status(&self) -> (String, bool) {
        match &*self.update.lock().unwrap() {
            update::State::Idle => (format!("Version {}.", env!("CARGO_PKG_VERSION")), false),
            update::State::Checking => ("Checking for updates…".into(), false),
            update::State::UpToDate => (format!("Version {} is the latest.", env!("CARGO_PKG_VERSION")), false),
            update::State::Ready(v) => (format!("Version {v} is downloaded and starts next time Delta does."), true),
            update::State::Failed(e) => (format!("Couldn't check for updates: {e}"), false),
        }
    }

    // ------------------------------------------------ settings window

    fn open_settings(&mut self) {
        if let Some(s) = &self.settings {
            unsafe {
                let _ = ShowWindow(s.hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(s.hwnd);
            }
            return;
        }
        self.load_theme_infos();
        match Settings::open(self.instance, Some(settings_wndproc), self.gfx.clone(), self.cfg.clone(), 0) {
            Ok(s) => {
                self.settings = Some(s);
                self.force = true;
                unsafe {
                    SetTimer(Some(self.hwnd), TIMER_UI, 15, None);
                }
            }
            Err(e) => log!("couldn't open settings: {e}"),
        }
    }

    fn close_settings(&mut self) {
        self.settings = None;
        self.theme_infos.clear();
        self.force = true;
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_UI);
        }
    }

    fn load_theme_infos(&mut self) {
        self.theme_infos = theme::list(&self.paths.themes)
            .into_iter()
            .filter_map(|name| theme::load(&self.paths.themes, &name).ok().map(|t| ThemeInfo { builtin: theme::is_builtin(&name), name, theme: t }))
            .collect();
        if let Some(s) = &mut self.settings {
            s.themes_changed();
        }
    }

    fn running_apps(&mut self) -> Vec<String> {
        if self.apps_cache.0.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) {
            self.apps_cache = (Some(Instant::now()), system::windowed_apps());
        }
        self.apps_cache.1.clone()
    }

    fn settings_message(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        if msg == WM_CLOSE {
            self.close_settings();
            return Some(LRESULT(0));
        }
        let r = self.settings.as_mut()?.message(msg, wp, lp);
        if self.settings.as_ref().is_some_and(|s| s.needs_redraw) && matches!(msg, WM_LBUTTONDOWN | WM_LBUTTONUP | WM_KEYDOWN | WM_CHAR | WM_SIZE | WM_MOUSEWHEEL) {
            self.render_settings();
        }
        r
    }

    fn render_settings(&mut self) {
        if self.settings.is_none() {
            return;
        }
        let status = Status {
            visible: self.visible,
            elevated: self.elevated,
            fps: self.fps_note.clone(),
            gpu: self.sampler.shared.lock().map(|s| format!("{} · read through {}", s.gpu_name, s.gpu_source)).unwrap_or_default(),
            rtss: match &self.rtss.status {
                rtss::Status::Serving => "Delta draws what they send.".into(),
                rtss::Status::RtssRunning => "RivaTuner is running, so it draws them.".into(),
                rtss::Status::Off => "Turned off.".into(),
                rtss::Status::Failed(e) => format!("Unavailable: {e}"),
            },
            monitors: overlay::monitors().len(),
            version: env!("CARGO_PKG_VERSION"),
            rtss_state: if matches!(self.rtss.status, rtss::Status::RtssRunning) {
                RtssState::Running
            } else if rtss::installed().is_some() {
                RtssState::Installed
            } else {
                RtssState::Missing
            },
            bridge_active: self.bridge_active,
            exclusive_note: self.exclusive_note.clone(),
            diag: self.diag.clone(),
            update: self.update_status(),
        };
        let apps = self.running_apps();
        let mut frame = self.last_frame.clone();
        preview_frame(&mut frame);
        let data = ui::Data { status: &status, themes: &self.theme_infos, frame: &frame, apps: &apps };
        let Some(s) = self.settings.as_mut() else { return };
        let (actions, changed) = s.render(&data);
        if changed {
            let cfg = s.cfg.clone();
            self.apply(cfg, vec![], false);
            self.save_at = Some(Instant::now() + Duration::from_millis(400));
        }
        for a in actions {
            match a {
                Action::ToggleOverlay => self.toggle(),
                Action::RestartAdmin => self.command(CMD_ADMIN),
                Action::OpenThemes => open(&self.paths.themes.to_string_lossy()),
                Action::OpenConfig => {
                    if self.save_at.take().is_some() {
                        let _ = self.cfg_file.save(&self.cfg);
                    }
                    open(&self.paths.config.to_string_lossy())
                }
                Action::DuplicateTheme(name) => self.duplicate_theme(&name),
                Action::OpenUrl(url) => open(&url),
                Action::CheckUpdates => self.check_updates(true),
                Action::RestartUpdate => self.restart_to_update(),
                Action::StartRtss => {
                    if let Some(p) = rtss::installed() {
                        open(&p.to_string_lossy());
                    }
                }
            }
        }
        if let Some(s) = &mut self.settings {
            s.needs_redraw = s.needs_redraw || changed;
        }
    }

    /// Copies a theme into the themes folder under a new name and switches to it.
    fn duplicate_theme(&mut self, name: &str) {
        let Some(src) = theme::source(&self.paths.themes, name) else { return };
        let base = format!("{name}-custom");
        let mut new = base.clone();
        let mut n = 2;
        while self.paths.themes.join(format!("{new}.toml")).exists() || theme::is_builtin(&new) {
            new = format!("{base}-{n}");
            n += 1;
        }
        let display = self.theme_infos.iter().find(|t| t.name.eq_ignore_ascii_case(name)).map_or(name.to_string(), |t| t.theme.name.clone());
        let body: String = src
            .lines()
            .map(|l| if l.trim_start().starts_with("name =") { format!("name = \"{display} (custom)\"") } else { l.to_string() })
            .collect::<Vec<_>>()
            .join("\n");
        let file = self.paths.themes.join(format!("{new}.toml"));
        if std::fs::write(&file, body + "\n").is_ok() {
            self.load_theme_infos();
            let mut cfg = self.cfg.clone();
            cfg.theme = new;
            if let Some(s) = &mut self.settings {
                s.cfg = cfg.clone();
            }
            self.apply(cfg, vec![], false);
            self.save_at = Some(Instant::now());
            open(&file.to_string_lossy());
        }
    }

    fn shutdown(&mut self) {
        if self.bridge_active {
            rtss::client_clear();
        }
        if self.save_at.take().is_some() {
            let _ = self.cfg_file.save(&self.cfg);
        }
        self.tray.remove();
        frames::stop_session();
        log!("exit");
    }
}

fn open(path: &str) {
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

fn with_app(f: impl FnOnce(&mut App)) {
    APP.with(|a| {
        if let Ok(mut g) = a.try_borrow_mut() {
            if let Some(app) = g.as_mut() {
                f(app);
            }
        }
    });
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    static TASKBAR_CREATED: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    let taskbar = *TASKBAR_CREATED.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) });
    match msg {
        WM_TIMER if wp.0 == TIMER_UI => with_app(|a| {
            if a.settings.as_ref().is_some_and(|s| s.needs_redraw || s.animating()) {
                a.render_settings();
            }
        }),
        WM_TIMER => with_app(|a| {
            a.tick();
            // Live values in the theme previews.
            if let Some(s) = &mut a.settings {
                s.needs_redraw = true;
            }
        }),
        WM_HOTKEY => with_app(|a| match wp.0 as i32 {
            HK_TOGGLE => a.toggle(),
            HK_THEME => a.cycle_theme(),
            HK_POSITION => a.cycle_position(),
            _ => {}
        }),
        tray::CALLBACK => {
            let event = (lp.0 & 0xFFFF) as u32;
            if event == WM_CONTEXTMENU || event == WM_RBUTTONUP {
                // Build the menu, then release the app while the menu's modal loop runs (timers keep ticking).
                let mut entries = None;
                with_app(|a| entries = Some(a.menu()));
                if let Some(e) = entries {
                    let id = tray::popup(hwnd, &e);
                    if id != 0 {
                        with_app(|a| a.command(id));
                    }
                }
            } else if event == WM_LBUTTONUP || event == 0x400 /* NIN_SELECT */ {
                with_app(|a| a.open_settings());
            }
        }
        WM_OPEN_SETTINGS => with_app(|a| a.open_settings()),
        WM_UPDATE_DONE => with_app(|a| a.update_done()),
        m if m == taskbar && m != 0 => with_app(|a| a.tray.add()),
        WM_DESTROY => unsafe { PostQuitMessage(0) },
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
    LRESULT(0)
}

unsafe extern "system" fn settings_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let mut handled = None;
    with_app(|a| handled = a.settings_message(msg, wp, lp));
    match handled {
        Some(r) => r,
        None => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Theme previews show sample frame numbers when there's no game running.
fn preview_frame(f: &mut Frame) {
    f.fps_note = None;
    if !f.values.contains_key("fps") {
        f.values.insert("fps".into(), 144.0);
        f.values.insert("frametime".into(), 6.9);
        f.values.insert("fps.low1".into(), 118.0);
        f.values.insert("fps.low01".into(), 97.0);
        f.frametimes = (0..120).map(|i| 6.9 + ((i as f32 * 0.7).sin() * 0.6) + if i % 37 == 0 { 4.0 } else { 0.0 }).collect();
    }
}

pub fn run(instance: HINSTANCE) -> Result<(), String> {
    unsafe {
        let class = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: instance, lpszClassName: w!("DeltaMain"), ..Default::default() };
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(Default::default(), w!("DeltaMain"), w!("Delta"), WS_OVERLAPPED, 0, 0, 0, 0, None, None, Some(instance), None).map_err(|e| e.to_string())?;
        let app = App::new(instance, hwnd)?;
        APP.with(|a| *a.borrow_mut() = Some(app));
        if std::env::args().any(|a| a == "--settings") {
            let _ = PostMessageW(Some(hwnd), WM_OPEN_SETTINGS, WPARAM(0), LPARAM(0));
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        APP.with(|a| {
            if let Some(mut app) = a.borrow_mut().take() {
                app.shutdown();
            }
        });
    }
    Ok(())
}
