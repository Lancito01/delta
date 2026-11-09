//! User configuration: WHAT to show (rows), WHERE (position), WHEN (show rules, hotkeys).

use crate::theme::{Color, Layout};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const DEFAULT_TEXT: &str = include_str!("default_config.toml");

const SAVED_HEADER: &str = "# Delta settings. The easy way to change them: tray icon > Settings.\n\
# You can still edit this file by hand; changes apply as soon as you save. The README explains every option.\n\n";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Position {
    TopLeft,
    TopCenter,
    TopRight,
    MiddleLeft,
    MiddleRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl Position {
    pub const ALL: [Position; 8] = [
        Position::TopLeft,
        Position::TopCenter,
        Position::TopRight,
        Position::MiddleRight,
        Position::BottomRight,
        Position::BottomCenter,
        Position::BottomLeft,
        Position::MiddleLeft,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Position::TopLeft => "top-left",
            Position::TopCenter => "top-center",
            Position::TopRight => "top-right",
            Position::MiddleLeft => "middle-left",
            Position::MiddleRight => "middle-right",
            Position::BottomLeft => "bottom-left",
            Position::BottomCenter => "bottom-center",
            Position::BottomRight => "bottom-right",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Position::TopLeft => "Top left",
            Position::TopCenter => "Top center",
            Position::TopRight => "Top right",
            Position::MiddleLeft => "Middle left",
            Position::MiddleRight => "Middle right",
            Position::BottomLeft => "Bottom left",
            Position::BottomCenter => "Bottom center",
            Position::BottomRight => "Bottom right",
        }
    }
    /// (horizontal, vertical) anchors: 0 = start, 1 = center, 2 = end.
    pub fn anchors(self) -> (u8, u8) {
        match self {
            Position::TopLeft => (0, 0),
            Position::TopCenter => (1, 0),
            Position::TopRight => (2, 0),
            Position::MiddleLeft => (0, 1),
            Position::MiddleRight => (2, 1),
            Position::BottomLeft => (0, 2),
            Position::BottomCenter => (1, 2),
            Position::BottomRight => (2, 2),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ShowMode {
    Always,
    Games,
    Fullscreen,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Display {
    Text,
    Graph,
    Both,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TempUnit {
    C,
    F,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Row {
    pub label: String,
    pub metrics: Vec<String>,
    pub display: Display,
    pub graph: Option<String>,
    pub group: Option<String>,
    pub color: Option<Color>,
    pub graph_max: Option<f64>,
}

impl Default for Row {
    fn default() -> Self {
        Row { label: String::new(), metrics: vec![], display: Display::Text, graph: None, group: None, color: None, graph_max: None }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Show {
    pub mode: ShowMode,
    pub start_visible: bool,
    pub exclude: Vec<String>,
    pub include: Vec<String>,
    pub hide_from_capture: bool,
    /// Other always-on-top apps (notches, widgets) that stay above the overlay instead of trading
    /// places with it.
    pub stay_below: Vec<String>,
}

impl Default for Show {
    fn default() -> Self {
        Show { mode: ShowMode::Always, start_visible: true, exclude: vec![], include: vec![], hide_from_capture: false, stay_below: vec![] }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct Hotkeys {
    pub toggle: String,
    pub cycle_theme: String,
    pub cycle_position: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Rtss {
    /// Stand in for RTSS (for HWiNFO, OBS HUD, ...) while it isn't running.
    pub compat: bool,
    /// Draw inside games through RTSS while it is running.
    pub bridge: Bridge,
}

impl Default for Rtss {
    fn default() -> Self {
        Rtss { compat: true, bridge: Bridge::Exclusive }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ValueWidth {
    /// Each value gets room for its widest normal reading ("100", "0,000"), so nothing shifts.
    Fixed,
    /// Values take only the room they need (columns still don't shrink for a few seconds).
    Fit,
}

/// Spacing overrides; anything left out uses the theme's value.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Spacing {
    pub values: ValueWidth,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column_gap: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit_gap: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row_gap: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label_gap: Option<f32>,
}

impl Default for Spacing {
    fn default() -> Self {
        Spacing { values: ValueWidth::Fixed, column_gap: None, unit_gap: None, row_gap: None, label_gap: None }
    }
}

/// When Delta draws inside games through RivaTuner Statistics Server.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Bridge {
    Off,
    /// Only for games in exclusive fullscreen, where no window can show.
    Exclusive,
    /// In every game RTSS draws in (the window overlay steps aside there).
    Always,
}

/// Which monitor(s) the overlay is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Monitor {
    /// The main (primary) monitor.
    Main,
    /// The monitor a game is on; the main monitor otherwise.
    FollowGame,
    /// Whichever monitor the active window is on.
    Follow,
    /// Every monitor.
    All,
    /// A specific monitor, 1-based.
    Number(u32),
}

impl Monitor {
    pub fn parse(s: &str) -> Monitor {
        match s.trim().to_ascii_lowercase().as_str() {
            "follow-game" => Monitor::FollowGame,
            "follow" | "foreground" => Monitor::Follow,
            "all" => Monitor::All,
            n => n.parse().ok().filter(|n| *n > 0).map(Monitor::Number).unwrap_or(Monitor::Main),
        }
    }
    pub fn key(self) -> String {
        match self {
            Monitor::Main => "main".into(),
            Monitor::FollowGame => "follow-game".into(),
            Monitor::Follow => "follow".into(),
            Monitor::All => "all".into(),
            Monitor::Number(n) => n.to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default = "Config::fallback")]
pub struct Config {
    pub theme: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<Layout>,
    pub position: Position,
    pub offset_x: i32,
    pub offset_y: i32,
    /// main | follow-game | follow | all | 1, 2, ...
    pub monitor: String,
    pub scale: f32,
    /// Whole overlay.
    pub opacity: f32,
    /// Panel background (and borders, graph backdrops), on top of the theme's own transparency.
    pub background_opacity: f32,
    /// Text, graphs and bars.
    pub text_opacity: f32,
    pub poll_ms: u32,
    pub refresh_hz: u32,
    pub history: usize,
    pub frametime_samples: usize,
    pub temperature: TempUnit,
    pub gpu_index: u32,
    /// Download new versions from GitHub in the background (they run after a restart).
    pub auto_update: bool,
    pub show: Show,
    pub hotkeys: Hotkeys,
    pub rtss: Rtss,
    pub spacing: Spacing,
    pub thresholds: HashMap<String, [f64; 2]>,
    pub rows: Vec<Row>,
}

impl Default for Config {
    fn default() -> Self {
        // Built-in defaults come from the shipped, commented config file so there is one source of truth.
        parse("").unwrap_or_else(|_| Config::fallback())
    }
}

impl Config {
    fn fallback() -> Config {
        Config {
            theme: "minimal".into(),
            layout: None,
            position: Position::TopLeft,
            offset_x: 16,
            offset_y: 16,
            monitor: "main".into(),
            scale: 1.0,
            opacity: 1.0,
            background_opacity: 1.0,
            text_opacity: 1.0,
            poll_ms: 1000,
            refresh_hz: 30,
            history: 60,
            frametime_samples: 180,
            temperature: TempUnit::C,
            gpu_index: 0,
            auto_update: true,
            show: Show::default(),
            hotkeys: Hotkeys::default(),
            rtss: Rtss::default(),
            spacing: Spacing::default(),
            thresholds: HashMap::new(),
            rows: vec![],
        }
    }

    pub fn monitor(&self) -> Monitor {
        Monitor::parse(&self.monitor)
    }

    pub fn sanitize(&mut self) {
        self.scale = self.scale.clamp(0.25, 4.0);
        self.opacity = self.opacity.clamp(0.1, 1.0);
        self.background_opacity = self.background_opacity.clamp(0.0, 1.0);
        self.text_opacity = self.text_opacity.clamp(0.1, 1.0);
        self.poll_ms = self.poll_ms.clamp(100, 10_000);
        self.refresh_hz = self.refresh_hz.clamp(1, 144);
        self.history = self.history.clamp(4, 1000);
        self.frametime_samples = self.frametime_samples.clamp(16, 2000);
    }
}

/// Where Delta keeps its files. Portable mode: a `config.toml` next to the exe wins.
pub struct Paths {
    pub dir: PathBuf,
    pub config: PathBuf,
    pub themes: PathBuf,
}

impl Paths {
    /// `--profile <dir>` on the command line (a separate set of settings, e.g. for testing).
    pub fn profile_arg() -> Option<PathBuf> {
        let args: Vec<String> = std::env::args().collect();
        args.iter().position(|a| a == "--profile").and_then(|i| args.get(i + 1)).map(PathBuf::from)
    }

    pub fn resolve() -> Paths {
        if let Some(dir) = Self::profile_arg() {
            return Paths { config: dir.join("config.toml"), themes: dir.join("themes"), dir };
        }
        let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
        let dir = match exe_dir {
            Some(d) if d.join("config.toml").exists() => d,
            _ => std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("Delta"),
        };
        Paths { config: dir.join("config.toml"), themes: dir.join("themes"), dir }
    }
}

pub struct ConfigFile {
    pub path: PathBuf,
    modified: Option<SystemTime>,
}

impl ConfigFile {
    pub fn new(path: PathBuf) -> ConfigFile {
        if !path.exists() {
            if let Some(p) = path.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            let _ = std::fs::write(&path, DEFAULT_TEXT);
        }
        ConfigFile { path, modified: None }
    }

    fn mtime(&self) -> Option<SystemTime> {
        std::fs::metadata(&self.path).and_then(|m| m.modified()).ok()
    }

    /// Loads the config. Errors keep defaults and are returned for the user to see.
    pub fn load(&mut self) -> (Config, Option<String>) {
        if !self.path.exists() {
            let _ = std::fs::write(&self.path, DEFAULT_TEXT);
        }
        self.modified = self.mtime();
        match std::fs::read_to_string(&self.path) {
            Ok(text) => match parse(&text) {
                Ok(c) => (c, None),
                Err(e) => (Config::default(), Some(format!("config.toml: {e}"))),
            },
            Err(e) => (Config::default(), Some(format!("can't read {}: {e}", self.path.display()))),
        }
    }

    pub fn changed(&self) -> bool {
        self.mtime() != self.modified
    }

    /// Writes the whole config (used by the settings window).
    pub fn save(&mut self, cfg: &Config) -> Result<(), String> {
        let body = toml::to_string_pretty(cfg).map_err(|e| e.to_string())?;
        std::fs::write(&self.path, format!("{SAVED_HEADER}{body}")).map_err(|e| e.to_string())?;
        self.modified = self.mtime();
        Ok(())
    }

    /// Sets a top-level `key = value` line, keeping the rest of the file (and its comments) as is.
    pub fn set(&mut self, key: &str, value: &str) {
        let text = std::fs::read_to_string(&self.path).unwrap_or_else(|_| DEFAULT_TEXT.to_string());
        let new = set_top_level(&text, key, value);
        if std::fs::write(&self.path, new).is_ok() {
            self.modified = self.mtime();
        }
    }
}

/// Parses user config text on top of the shipped defaults: keys the user left out keep their
/// default; tables merge; arrays (like `rows`) are replaced as a whole.
pub fn parse(text: &str) -> Result<Config, String> {
    let mut base: toml::Value = toml::from_str(DEFAULT_TEXT).map_err(|e| e.message().to_string())?;
    let user: toml::Value = toml::from_str(text).map_err(|e| e.message().to_string())?;
    crate::theme::merge(&mut base, user);
    let mut c: Config = base.try_into().map_err(|e: toml::de::Error| e.message().to_string())?;
    c.sanitize();
    Ok(c)
}

fn set_top_level(text: &str, key: &str, value: &str) -> String {
    let line = format!("{key} = {value}");
    let mut out = Vec::new();
    let mut done = false;
    let mut in_table = false;
    let mut insert_at = None;
    for l in text.lines() {
        let t = l.trim_start();
        if t.starts_with('[') {
            if !in_table {
                insert_at = Some(out.len());
            }
            in_table = true;
        }
        if !done && !in_table {
            if let Some(rest) = t.strip_prefix(key) {
                if rest.trim_start().starts_with('=') {
                    // keep a trailing comment if there was one
                    let comment = l.find(" #").map(|i| &l[i..]).unwrap_or("");
                    let pad = if comment.is_empty() { String::new() } else { " ".repeat(26usize.saturating_sub(line.len()).max(1) - 1) };
                    out.push(format!("{line}{pad}{comment}"));
                    done = true;
                    continue;
                }
            }
        }
        out.push(l.to_string());
    }
    if !done {
        out.insert(insert_at.unwrap_or(out.len()), line);
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_parses() {
        let c = Config::default();
        assert_eq!(c.theme, "minimal");
        assert!(c.rows.len() >= 4);
        // Partial user files keep the other defaults.
        let c = parse("theme = \"neon\"
[show]
mode = \"games\"
").unwrap();
        assert_eq!(c.theme, "neon");
        assert_eq!(c.show.mode, ShowMode::Games);
        assert_eq!(c.hotkeys.toggle, "Ctrl+Shift+F10");
        assert!(c.thresholds.contains_key("fps"));
        assert!(parse("theme = 5").is_err());
    }
    #[test]
    fn set_keeps_comments() {
        let t = "theme = \"minimal\"          # pick one\n[show]\ntheme = \"x\"\n";
        let n = set_top_level(t, "theme", "\"neon\"");
        assert!(n.starts_with("theme = \"neon\""));
        assert!(n.contains("# pick one"));
        assert!(n.contains("[show]\ntheme = \"x\""));
        let n = set_top_level("[a]\nb = 1\n", "theme", "\"neon\"");
        assert!(n.starts_with("theme = \"neon\"\n[a]"));
    }
}
