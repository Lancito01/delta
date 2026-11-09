//! Parses RTSS OSD text (with its `<tag>` markup) into something Delta can draw.
//! Supported: colors `<C=RRGGBB>` / `<C=AARRGGBB>` / `<Cn=..>` / `<Cn>` / `<C>`, sizes `<S=pct>` /
//! `<Sn=..>` / `<Sn>` / `<S>`, layers `<Ln>`, positions `<Pn>` (screen spot 0-8) and `<P=x,y>`
//! (a position before a layer tag places that layer), images `<LI="file">` + `<I=w,h,sx,sy,sw,sh>`,
//! line breaks `<BR>`, and the macros `<FR>`, `<FT>`, `<APP>`. Unknown tags are dropped, as RTSS does.

use crate::theme::Color;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Text { text: String, color: Option<Color>, scale: f32 },
    Newline,
    Image { path: String, w: f32, h: f32, src: Option<[f32; 4]> },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layer {
    /// Screen spot 0..8 (left to right, top to bottom) the layer is pinned to.
    pub spot: Option<u8>,
    /// Absolute position in px (negative = from the right/bottom edge, like RTSS).
    pub pos: Option<(f32, f32)>,
    pub items: Vec<Item>,
}

impl Layer {
    pub fn positioned(&self) -> bool {
        self.spot.is_some() || self.pos.is_some()
    }
}

pub struct Macros {
    pub fps: Option<f64>,
    pub frametime: Option<f64>,
    pub api: String,
}

#[derive(Clone, Copy)]
struct Place {
    spot: Option<u8>,
    pos: Option<(f32, f32)>,
}

struct Parser<'a> {
    layers: Vec<(Option<u32>, Layer)>,
    cur: usize,
    color: Option<Color>,
    scale: f32,
    color_vars: HashMap<u32, Color>,
    size_vars: HashMap<u32, f32>,
    image: Option<String>,
    pending: Option<Place>,
    run: String,
    m: &'a Macros,
}

impl Parser<'_> {
    /// Content is about to be added: a position given without a layer tag starts its own layer.
    fn target(&mut self) -> &mut Layer {
        if let Some(p) = self.pending.take() {
            self.layers.push((None, Layer { spot: p.spot, pos: p.pos, items: vec![] }));
            self.cur = self.layers.len() - 1;
        }
        &mut self.layers[self.cur].1
    }

    fn flush(&mut self) {
        if !self.run.is_empty() {
            let text = std::mem::take(&mut self.run);
            let (color, scale) = (self.color, self.scale);
            self.target().items.push(Item::Text { text, color, scale });
        }
    }

    fn newline(&mut self) {
        self.flush();
        self.target().items.push(Item::Newline);
    }

    fn tag(&mut self, tag: &str) {
        let upper = tag.to_ascii_uppercase();
        match upper.as_str() {
            "FR" => return self.run.push_str(&self.m.fps.map(|f| format!("{f:.0}")).unwrap_or_else(|| "-".into())),
            "FT" => return self.run.push_str(&self.m.frametime.map(|f| format!("{f:.1}")).unwrap_or_else(|| "-".into())),
            "APP" => return self.run.push_str(&self.m.api),
            "BR" => return self.newline(),
            _ => {}
        }
        self.flush();
        let (name, arg) = match tag.find('=') {
            Some(p) => (&upper[..p], Some(&tag[p + 1..])),
            None => (upper.as_str(), None),
        };
        if name == "LI" {
            self.image = arg.map(|a| a.trim().trim_matches('"').to_string());
            return;
        }
        let split = name.find(|c: char| c.is_ascii_digit()).unwrap_or(name.len());
        let (letter, num) = name.split_at(split);
        let num: Option<u32> = num.parse().ok();
        match letter {
            "C" => match (num, arg) {
                (None, None) => self.color = None,
                (None, Some(a)) => self.color = parse_color(a),
                (Some(n), Some(a)) => {
                    if let Some(c) = parse_color(a) {
                        self.color_vars.insert(n, c);
                    }
                }
                (Some(n), None) => self.color = self.color_vars.get(&n).copied(),
            },
            "S" => match (num, arg) {
                (None, None) => self.scale = 1.0,
                (None, Some(a)) => self.scale = a.trim().parse::<f32>().map(|p| p / 100.0).unwrap_or(1.0),
                (Some(n), Some(a)) => {
                    if let Ok(p) = a.trim().parse::<f32>() {
                        self.size_vars.insert(n, p / 100.0);
                    }
                }
                (Some(n), None) => self.scale = self.size_vars.get(&n).copied().unwrap_or(1.0),
            },
            "P" => {
                let spot = if arg.is_none() { num.filter(|n| *n <= 8).map(|n| n as u8) } else { None };
                let pos = arg.and_then(|a| {
                    let mut it = a.split(',').map(|v| v.trim().parse::<f32>());
                    Some((it.next()?.ok()?, it.next()?.ok()?))
                });
                if spot.is_some() || pos.is_some() {
                    self.pending = Some(Place { spot, pos });
                }
            }
            "L" => {
                if let Some(n) = num {
                    let place = self.pending.take();
                    self.cur = match self.layers.iter().position(|(id, _)| *id == Some(n)) {
                        Some(p) => p,
                        None => {
                            self.layers.push((Some(n), Layer::default()));
                            self.layers.len() - 1
                        }
                    };
                    if let Some(p) = place {
                        self.layers[self.cur].1.spot = p.spot;
                        self.layers[self.cur].1.pos = p.pos;
                    }
                }
            }
            "I" => {
                if let (Some(path), Some(a)) = (self.image.clone(), arg) {
                    let v: Vec<f32> = a.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                    if v.len() >= 2 {
                        let src = (v.len() >= 6 && v[4] > 0.0 && v[5] > 0.0).then(|| [v[2], v[3], v[4], v[5]]);
                        self.target().items.push(Item::Image { path, w: v[0], h: v[1], src });
                    }
                }
            }
            _ => {}
        }
    }
}

/// Parses one slot's text into layers; unpositioned text comes first.
pub fn parse(text: &str, m: &Macros) -> Vec<Layer> {
    let mut p = Parser {
        layers: vec![(None, Layer::default())],
        cur: 0,
        color: None,
        scale: 1.0,
        color_vars: HashMap::new(),
        size_vars: HashMap::new(),
        image: None,
        pending: None,
        run: String::new(),
        m,
    };
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c == '<' {
            if let Some(end) = rest.find('>') {
                p.tag(&rest[1..end]);
                rest = &rest[end + 1..];
                continue;
            }
        }
        match c {
            '\n' => p.newline(),
            '\t' => p.run.push_str("    "),
            '\r' => {}
            _ => p.run.push(c),
        }
        rest = &rest[c.len_utf8()..];
    }
    p.flush();
    p.layers.into_iter().map(|(_, l)| l).filter(|l| l.items.iter().any(|i| !matches!(i, Item::Newline))).collect()
}

fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    match s.len() {
        6 => Color::parse(s),
        8 => {
            // RTSS writes AARRGGBB
            let a = u8::from_str_radix(&s[0..2], 16).ok()? as f32 / 255.0;
            Color::parse(&s[2..]).map(|c| c.with_alpha(a))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn m() -> Macros {
        Macros { fps: Some(144.0), frametime: Some(6.94), api: "D3D11".into() }
    }
    #[test]
    fn obs_hud_image() {
        let t = r#"<LI="C:\Temp\obs-hud\rtss-3.png"><P2><L32><C=FFFFFF><I=220,40,0,0,220,40><C>"#;
        let l = parse(t, &m());
        assert_eq!(l.len(), 1, "{l:?}");
        assert_eq!(l[0].spot, Some(2));
        assert!(matches!(&l[0].items[0], Item::Image { path, w, .. } if path.ends_with("rtss-3.png") && *w == 220.0));
    }
    #[test]
    fn colors_and_macros() {
        let l = parse("<C=FF0000>GPU<C> <FR> fps\nline2", &m());
        assert_eq!(l.len(), 1);
        let items = &l[0].items;
        assert!(matches!(&items[0], Item::Text { text, color: Some(_), .. } if text == "GPU"));
        assert!(matches!(&items[1], Item::Text { text, color: None, .. } if text == " 144 fps"));
        assert_eq!(items[2], Item::Newline);
    }
    #[test]
    fn variables() {
        let l = parse("<C0=00FF00><S0=50><C0><S0>x", &m());
        assert!(matches!(&l[0].items[0], Item::Text { color: Some(c), scale, .. } if c.0[1] == 1.0 && *scale == 0.5));
    }
    #[test]
    fn position_without_layer() {
        let l = parse("plain<P=10,20>placed", &m());
        assert_eq!(l.len(), 2);
        assert_eq!(l[1].pos, Some((10.0, 20.0)));
    }
}
