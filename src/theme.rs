//! Themes: everything about *how* the overlay looks. A theme is a TOML file; it may say
//! `extends = "minimal"` and then only override what it changes. Built-in themes are compiled
//! in and also written to the themes folder on first run so they can be copied and edited.

use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::path::Path;

pub const BUILTIN: &[(&str, &str)] = &[
    ("minimal", include_str!("../themes/minimal.toml")),
    ("strip", include_str!("../themes/strip.toml")),
    ("futuristic", include_str!("../themes/futuristic.toml")),
    ("terminal", include_str!("../themes/terminal.toml")),
    ("hud", include_str!("../themes/hud.toml")),
    ("neon", include_str!("../themes/neon.toml")),
];

/// RGBA, straight (not premultiplied) alpha, 0..1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color(pub [f32; 4]);

impl Serialize for Color {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.hex())
    }
}

impl Color {
    pub fn parse(s: &str) -> Option<Color> {
        let h = s.trim().trim_start_matches('#');
        let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok().map(|b| b as f32 / 255.0);
        match h.len() {
            6 => Some(Color([byte(0)?, byte(2)?, byte(4)?, 1.0])),
            8 => Some(Color([byte(0)?, byte(2)?, byte(4)?, byte(6)?])),
            3 => {
                let n = |i: usize| u8::from_str_radix(&h[i..i + 1], 16).ok().map(|b| (b * 17) as f32 / 255.0);
                Some(Color([n(0)?, n(1)?, n(2)?, 1.0]))
            }
            _ => None,
        }
    }
    pub fn hex(self) -> String {
        let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let [r, g, bl, a] = self.0;
        if b(a) == 255 {
            format!("#{:02X}{:02X}{:02X}", b(r), b(g), b(bl))
        } else {
            format!("#{:02X}{:02X}{:02X}{:02X}", b(r), b(g), b(bl), b(a))
        }
    }
    pub fn with_alpha(self, a: f32) -> Color {
        Color([self.0[0], self.0[1], self.0[2], self.0[3] * a])
    }
    pub fn lerp(self, o: Color, t: f32) -> Color {
        let mut c = [0.0; 4];
        for i in 0..4 {
            c[i] = self.0[i] + (o.0[i] - self.0[i]) * t;
        }
        Color(c)
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Color::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("bad color '{s}' (use #RRGGBB or #RRGGBBAA)")))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Layout {
    /// One row per group, columns aligned across rows.
    Stack,
    /// A single horizontal strip.
    Bar,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum GraphStyle {
    Line,
    Area,
    Bars,
    Step,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CornerStyle {
    Rounded,
    /// Chamfered (45° cut) corners.
    Cut,
    /// Square panel with bracket marks on the corners.
    Brackets,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ColorSource {
    /// Use the theme's plain color for this element.
    Plain,
    /// Use the accent color of the metric's group (gpu, cpu, ...).
    Group,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Font {
    /// Comma-separated list; the first installed family wins.
    pub family: String,
    pub size: f32,
    pub weight: u32,
    pub label_family: String,
    pub label_size: f32,
    pub label_weight: u32,
    pub unit_size: f32,
    /// Extra spacing between label letters, in px.
    pub label_tracking: f32,
    pub uppercase_labels: bool,
    /// Size of the big FPS number (0 = same as `size`).
    pub hero_size: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Colors {
    pub background: Color,
    /// Bottom color of a vertical gradient; same as background for a flat panel.
    pub background_end: Color,
    pub border: Color,
    pub label: Color,
    pub value: Color,
    pub unit: Color,
    pub accent: Color,
    pub good: Color,
    pub warn: Color,
    pub crit: Color,
    pub graph_fill: Color,
    pub graph_grid: Color,
    pub shadow: Color,
    pub separator: Color,
    pub bar_track: Color,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Shape {
    pub corner_style: CornerStyle,
    pub corner_radius: f32,
    pub padding_x: f32,
    pub padding_y: f32,
    pub row_gap: f32,
    pub column_gap: f32,
    pub label_gap: f32,
    pub border_width: f32,
    /// Minimum width of the label column.
    pub label_width: f32,
    /// Draws a thin colored bar at the left edge of each row in its group color.
    pub row_marker: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Graph {
    pub style: GraphStyle,
    pub height: f32,
    /// 0 = as wide as the row.
    pub width: f32,
    pub line_width: f32,
    pub grid: bool,
    /// Background behind the graph area.
    pub background: Color,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Cores {
    pub bar_width: f32,
    pub bar_gap: f32,
    pub height: f32,
    pub radius: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Text {
    pub shadow: bool,
    pub shadow_offset: f32,
    /// Drawn before each row label, e.g. "> " for a terminal look.
    pub label_prefix: String,
    /// Separator between groups in the bar layout.
    pub separator: String,
    /// Optional title line at the top of the panel.
    pub header: String,
    /// Glow strength (0 = off): soft halo drawn behind values.
    pub glow: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Decor {
    pub scanlines: bool,
    pub scanline_alpha: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Theme {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub extends: Option<String>,
    pub layout: Layout,
    pub label_color: ColorSource,
    pub graph_color: ColorSource,
    pub value_color: ColorSource,
    pub font: Font,
    pub colors: Colors,
    pub groups: HashMap<String, Color>,
    pub shape: Shape,
    pub graph: Graph,
    pub cores: Cores,
    pub text: Text,
    pub decor: Decor,
}

impl Theme {
    pub fn group_color(&self, group: &str) -> Color {
        self.groups.get(group).copied().unwrap_or(self.colors.accent)
    }
}

/// Loads `name` from `dir` (or the built-ins), resolving `extends` chains.
pub fn load(dir: &Path, name: &str) -> Result<Theme, String> {
    let value = load_value(dir, name, 0)?;
    value.try_into::<Theme>().map_err(|e| format!("theme '{name}': {e}"))
}

fn load_value(dir: &Path, name: &str, depth: u32) -> Result<toml::Value, String> {
    if depth > 8 {
        return Err("theme `extends` chain is too deep (loop?)".into());
    }
    let text = read_source(dir, name)?;
    let value: toml::Value = toml::from_str(&text).map_err(|e| format!("theme '{name}': {e}"))?;
    // Every theme implicitly extends the base so partial themes always parse.
    let parent = value.get("extends").and_then(|v| v.as_str()).map(str::to_owned);
    let base = match parent {
        Some(p) if !p.eq_ignore_ascii_case(name) => load_value(dir, &p, depth + 1)?,
        _ if name != "minimal" => load_value(dir, "minimal", depth + 1)?,
        _ => toml::Value::Table(Default::default()),
    };
    let mut merged = base;
    merge(&mut merged, value);
    Ok(merged)
}

fn read_source(dir: &Path, name: &str) -> Result<String, String> {
    let file = dir.join(format!("{name}.toml"));
    if let Ok(t) = std::fs::read_to_string(&file) {
        return Ok(t);
    }
    BUILTIN
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, t)| t.to_string())
        .ok_or_else(|| format!("theme '{name}' not found in {}", dir.display()))
}

pub fn merge(base: &mut toml::Value, over: toml::Value) {
    match (base, over) {
        (toml::Value::Table(b), toml::Value::Table(o)) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(existing) if existing.is_table() && v.is_table() => merge(existing, v),
                    _ => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (b, o) => *b = o,
    }
}

/// Theme names available: files in `dir` plus built-ins, sorted, no duplicates.
pub fn list(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = BUILTIN.iter().map(|(n, _)| n.to_string()).collect();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "toml") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    if !names.iter().any(|n| n.eq_ignore_ascii_case(stem)) {
                        names.push(stem.to_string());
                    }
                }
            }
        }
    }
    names.sort();
    names
}

/// Built-in themes load from the exe, so updates reach everyone; the themes folder is for your own
/// themes. A starter file shows how to make one.
pub fn install_builtins(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let f = dir.join("README.txt");
    if !f.exists() {
        let _ = std::fs::write(
            f,
            "Your own Delta themes go here, as .toml files.

             Easiest: in Delta's settings, pick a theme and click Duplicate, then edit the copy.
             A theme only needs what it changes, e.g.:

             name = \"Ocean\"
extends = \"minimal\"

[colors]
accent = \"#4FC3F7\"

             Every option is listed in minimal.toml in Delta's source (themes folder).
",
        );
    }
}

/// Is this one of the themes compiled into Delta?
pub fn is_builtin(name: &str) -> bool {
    BUILTIN.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
}

/// The source text of a theme (file in `dir` or built-in), e.g. to duplicate it.
pub fn source(dir: &Path, name: &str) -> Option<String> {
    read_source(dir, name).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn builtins_parse() {
        let dir = std::env::temp_dir().join("delta-theme-test-nonexistent");
        for (n, _) in BUILTIN {
            load(&dir, n).unwrap_or_else(|e| panic!("{e}"));
        }
    }
    #[test]
    fn colors() {
        assert_eq!(Color::parse("#ff0000"), Some(Color([1.0, 0.0, 0.0, 1.0])));
        assert_eq!(Color::parse("#00000000").unwrap().0[3], 0.0);
        assert!(Color::parse("#zz0000").is_none());
    }
}
