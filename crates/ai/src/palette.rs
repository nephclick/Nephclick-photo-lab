//! Colour palettes: five colours, five gradients and five patterns.
//!
//! A palette comes from a prompt (the model answers in JSON, [`parse`]) or from a built-in themed set ([`THEMES`]:
//! fruits, plants, animals and more, which work without a key). Gradients and patterns a reply leaves out are made
//! from its colours ([`Palette::from_colors`]), so every palette is complete. [`Pattern::render`] paints a pattern
//! tile the app defines as a pattern preset.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AiError;

/// Colours, gradients and patterns in a palette.
pub const COUNT: usize = 5;

/// An sRGB colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgb(pub [u8; 3]);

impl Rgb {
    /// `#rrggbb` or `rrggbb` (also `#rgb`).
    pub fn parse(text: &str) -> Option<Rgb> {
        let h = text.trim().trim_start_matches('#');
        let byte = |s: &str| u8::from_str_radix(s, 16).ok();
        match h.len() {
            6 if h.is_ascii() => Some(Rgb([byte(h.get(0..2)?)?, byte(h.get(2..4)?)?, byte(h.get(4..6)?)?])),
            3 if h.is_ascii() => {
                let d = |i: usize| byte(h.get(i..=i)?).map(|v| v * 17);
                Some(Rgb([d(0)?, d(1)?, d(2)?]))
            }
            _ => None,
        }
    }

    /// `#rrggbb`.
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0[0], self.0[1], self.0[2])
    }

    /// Relative luminance 0..1 (sRGB, approximate): picks readable text over a swatch.
    pub fn luminance(self) -> f32 {
        let [r, g, b] = self.0.map(|c| f32::from(c) / 255.0);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    /// The colour `t` (0..1) of the way to `other`.
    pub fn mix(self, other: Rgb, t: f32) -> Rgb {
        let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
        let m = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round().clamp(0.0, 255.0) as u8;
        Rgb([m(self.0[0], other.0[0]), m(self.0[1], other.0[1]), m(self.0[2], other.0[2])])
    }
}

/// A linear gradient through two or more colours at `angle` degrees.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gradient {
    pub name: String,
    pub stops: Vec<Rgb>,
    pub angle: f32,
}

/// The shape of a pattern tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PatternKind {
    Stripes,
    Diagonal,
    Dots,
    Checker,
    Grid,
    Waves,
}

impl PatternKind {
    pub const ALL: [PatternKind; 6] =
        [PatternKind::Stripes, PatternKind::Diagonal, PatternKind::Dots, PatternKind::Checker, PatternKind::Grid, PatternKind::Waves];

    pub fn name(self) -> &'static str {
        match self {
            PatternKind::Stripes => "stripes",
            PatternKind::Diagonal => "diagonal",
            PatternKind::Dots => "dots",
            PatternKind::Checker => "checker",
            PatternKind::Grid => "grid",
            PatternKind::Waves => "waves",
        }
    }

    pub fn parse(text: &str) -> Option<PatternKind> {
        let t = text.trim().to_ascii_lowercase();
        PatternKind::ALL.into_iter().find(|k| t == k.name() || t.trim_end_matches('s') == k.name().trim_end_matches('s'))
    }
}

/// A two-colour repeating pattern; `scale` is the cell size in pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pattern {
    pub name: String,
    pub kind: PatternKind,
    pub ground: Rgb,
    pub ink: Rgb,
    pub scale: u32,
}

/// Smallest and largest pattern cell, in pixels.
pub const SCALE_RANGE: (u32, u32) = (4, 64);

impl Pattern {
    /// The tile's side: one repeat of the pattern, in pixels.
    pub fn tile_size(&self) -> u32 {
        let s = self.scale.clamp(SCALE_RANGE.0, SCALE_RANGE.1);
        match self.kind {
            PatternKind::Stripes | PatternKind::Diagonal | PatternKind::Dots | PatternKind::Grid => s * 2,
            PatternKind::Checker => s * 2,
            PatternKind::Waves => s * 4,
        }
    }

    /// One tile as RGBA8 rows, `tile_size()` square, that repeats seamlessly.
    pub fn render(&self) -> (u32, Vec<u8>) {
        let n = self.tile_size();
        let s = n / 2;
        let mut px = Vec::with_capacity((n * n * 4) as usize);
        for y in 0..n {
            for x in 0..n {
                let ink = match self.kind {
                    PatternKind::Stripes => x < s,
                    PatternKind::Diagonal => (x + y) % n < s,
                    PatternKind::Checker => (x < s) != (y < s),
                    PatternKind::Grid => {
                        let line = (s / 6).max(1);
                        x % s < line || y % s < line
                    }
                    PatternKind::Dots => {
                        // One dot centred in each half cell, radius a third of it.
                        let (cx, cy) = ((x % s) as f32 + 0.5 - s as f32 / 2.0, (y % s) as f32 + 0.5 - s as f32 / 2.0);
                        let r = s as f32 / 3.0;
                        cx * cx + cy * cy <= r * r
                    }
                    PatternKind::Waves => {
                        let phase = x as f32 / n as f32 * std::f32::consts::TAU;
                        let mid = n as f32 / 2.0 + phase.sin() * n as f32 / 6.0;
                        let band = n as f32 / 8.0;
                        ((y as f32 + 0.5) - mid).abs() < band
                    }
                };
                let c = if ink { self.ink } else { self.ground };
                px.extend_from_slice(&[c.0[0], c.0[1], c.0[2], 255]);
            }
        }
        (n, px)
    }
}

/// Five colours, five gradients and five patterns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Palette {
    pub name: String,
    pub colors: Vec<Rgb>,
    pub gradients: Vec<Gradient>,
    pub patterns: Vec<Pattern>,
}

impl Palette {
    /// A palette of `colors` (the first five; fewer are filled in with mixes of the ones given) with gradients and
    /// patterns made from them.
    pub fn from_colors(name: &str, colors: &[Rgb]) -> Palette {
        let colors = five(colors);
        Palette { name: name.to_string(), gradients: derived_gradients(&colors), patterns: derived_patterns(&colors), colors }
    }
}

/// Exactly five colours from `given`: extra ones are dropped and missing ones mixed from the ones there are (grey
/// ramps when there are none).
fn five(given: &[Rgb]) -> Vec<Rgb> {
    let mut out: Vec<Rgb> = given.iter().copied().take(COUNT).collect();
    if out.is_empty() {
        out.push(Rgb([40, 40, 48]));
    }
    while out.len() < COUNT {
        let i = out.len();
        let a = out[i % out.len().max(1)];
        let b = out[(i + 1) % out.len()];
        let mixed = a.mix(b, 0.5).mix(if i.is_multiple_of(2) { Rgb([255, 255, 255]) } else { Rgb([0, 0, 0]) }, 0.35);
        out.push(mixed);
    }
    out
}

fn derived_gradients(c: &[Rgb]) -> Vec<Gradient> {
    let at = |i: usize| c.get(i % c.len().max(1)).copied().unwrap_or(Rgb([0, 0, 0]));
    vec![
        Gradient { name: "Blend".into(), stops: vec![at(0), at(1)], angle: 90.0 },
        Gradient { name: "Glow".into(), stops: vec![at(1), at(2), at(3)], angle: 45.0 },
        Gradient { name: "Horizon".into(), stops: vec![at(2), at(4)], angle: 0.0 },
        Gradient { name: "Full Range".into(), stops: c.to_vec(), angle: 135.0 },
        Gradient { name: "Fade".into(), stops: vec![at(3), at(3).mix(Rgb([255, 255, 255]), 0.7)], angle: 90.0 },
    ]
}

fn derived_patterns(c: &[Rgb]) -> Vec<Pattern> {
    let at = |i: usize| c.get(i % c.len().max(1)).copied().unwrap_or(Rgb([0, 0, 0]));
    let p = |name: &str, kind, ground, ink, scale| Pattern { name: name.into(), kind, ground, ink, scale };
    vec![
        p("Stripes", PatternKind::Stripes, at(0), at(1), 12),
        p("Diagonal", PatternKind::Diagonal, at(1), at(2), 10),
        p("Dots", PatternKind::Dots, at(4), at(0), 16),
        p("Checker", PatternKind::Checker, at(2), at(3), 16),
        p("Waves", PatternKind::Waves, at(3), at(4), 12),
    ]
}

/// The instructions sent with a palette prompt: a JSON object of exactly this shape.
pub fn prompt(request: &str) -> String {
    format!(
        "You are a colour designer for an image editor. Create a cohesive palette for: \"{request}\".\n\
         Reply with only a JSON object, no prose, of this shape:\n\
         {{\"name\": \"short palette name\",\n\
           \"colors\": [\"#rrggbb\", … exactly 5],\n\
           \"gradients\": [{{\"name\": \"…\", \"stops\": [\"#rrggbb\", …2 to 4], \"angle\": 0-360}}, … exactly 5],\n\
           \"patterns\": [{{\"name\": \"…\", \"kind\": \"stripes|diagonal|dots|checker|grid|waves\", \"ground\": \"#rrggbb\", \"ink\": \"#rrggbb\", \"scale\": 4-64}}, … exactly 5]}}\n\
         Order the colours from dominant to accent. Use colours that work together."
    )
}

/// The palette in a model's `reply`. Colours that do not parse are skipped; gradients and patterns that are missing
/// or broken are made from the colours. A reply with no usable colour is an error.
pub fn parse(reply: &str) -> Result<Palette, AiError> {
    let v = crate::json::object(reply).ok_or_else(|| AiError::BadReply("no palette in the reply".into()))?;
    let name = text_field(&v, "name").unwrap_or_else(|| "AI Palette".into());
    let colors: Vec<Rgb> = v.get("colors").and_then(Value::as_array).map(|a| a.iter().filter_map(|c| c.as_str().and_then(Rgb::parse)).collect()).unwrap_or_default();
    if colors.is_empty() {
        return Err(AiError::BadReply("the reply had no colours".into()));
    }
    let mut palette = Palette::from_colors(&name, &colors);
    let gradients: Vec<Gradient> = v
        .get("gradients")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(gradient).take(COUNT).collect())
        .unwrap_or_default();
    for (slot, g) in palette.gradients.iter_mut().zip(gradients) {
        *slot = g;
    }
    let patterns: Vec<Pattern> =
        v.get("patterns").and_then(Value::as_array).map(|a| a.iter().filter_map(pattern).take(COUNT).collect()).unwrap_or_default();
    for (slot, p) in palette.patterns.iter_mut().zip(patterns) {
        *slot = p;
    }
    Ok(palette)
}

fn text_field(v: &Value, key: &str) -> Option<String> {
    let t = v.get(key)?.as_str()?.trim();
    (!t.is_empty()).then(|| t.chars().take(60).collect())
}

fn gradient(v: &Value) -> Option<Gradient> {
    let stops: Vec<Rgb> = v.get("stops")?.as_array()?.iter().filter_map(|c| c.as_str().and_then(Rgb::parse)).take(8).collect();
    if stops.len() < 2 {
        return None;
    }
    let angle = v.get("angle").and_then(Value::as_f64).filter(|a| a.is_finite()).unwrap_or(90.0).rem_euclid(360.0) as f32;
    Some(Gradient { name: text_field(v, "name").unwrap_or_else(|| "Gradient".into()), stops, angle })
}

fn pattern(v: &Value) -> Option<Pattern> {
    let kind = PatternKind::parse(v.get("kind")?.as_str()?)?;
    let ground = Rgb::parse(v.get("ground")?.as_str()?)?;
    let ink = Rgb::parse(v.get("ink")?.as_str()?)?;
    let scale = v.get("scale").and_then(Value::as_f64).filter(|s| s.is_finite()).unwrap_or(12.0).clamp(SCALE_RANGE.0 as f64, SCALE_RANGE.1 as f64) as u32;
    Some(Pattern { name: text_field(v, "name").unwrap_or_else(|| kind.name().into()), kind, ground, ink, scale })
}

/// A built-in themed palette: its category, name and five colours.
pub struct Theme {
    pub category: &'static str,
    pub name: &'static str,
    pub colors: [&'static str; COUNT],
}

impl Theme {
    pub fn palette(&self) -> Palette {
        let colors: Vec<Rgb> = self.colors.iter().filter_map(|c| Rgb::parse(c)).collect();
        Palette::from_colors(self.name, &colors)
    }
}

macro_rules! themes {
    ($($cat:literal => [$($name:literal: [$($c:literal),*]),* $(,)?]),* $(,)?) => {
        &[$($(Theme { category: $cat, name: $name, colors: [$($c),*] },)*)*]
    };
}

/// The built-in themed palettes, by category. Original colour picks.
pub const THEMES: &[Theme] = themes![
    "Fruits" => [
        "Mango": ["#ffb627", "#ff8c1a", "#f25c05", "#7fb800", "#2d6a4f"],
        "Watermelon": ["#ff4d6d", "#ff8fa3", "#fff0f3", "#52b788", "#1b4332"],
        "Blueberry": ["#2b2d6e", "#4a4e9c", "#7c83d6", "#c3c7f4", "#5b8c5a"],
        "Lemon": ["#fff275", "#ffe14d", "#f6c90e", "#9bc53d", "#3a5a40"],
        "Fig": ["#4a1942", "#893168", "#c9737c", "#e8c4a2", "#6b8f71"],
        "Peach": ["#ffcdb2", "#ffb4a2", "#e5989b", "#b5838d", "#6d6875"],
    ],
    "Plants" => [
        "Fern": ["#1b4332", "#2d6a4f", "#40916c", "#74c69d", "#d8f3dc"],
        "Lavender": ["#5a4e7c", "#8e7dbe", "#b8a9e3", "#e6e0f8", "#7a9e7e"],
        "Succulent": ["#7a9e87", "#a3c4bc", "#d9e4dd", "#e8a598", "#5c6b73"],
        "Autumn Leaves": ["#7f2704", "#c2410c", "#ea8c00", "#f2c14e", "#5b3a29"],
        "Cherry Blossom": ["#ffe5ec", "#ffc2d1", "#ff8fab", "#fb6f92", "#6b4f4f"],
        "Moss": ["#283618", "#606c38", "#a3b18a", "#dad7cd", "#3a3a3a"],
    ],
    "Animals" => [
        "Tiger": ["#ff7b00", "#ffaa00", "#1a1a1a", "#f5f0e6", "#8a5a00"],
        "Peacock": ["#003f5c", "#00798c", "#2ec4b6", "#9bc53d", "#f4d35e"],
        "Flamingo": ["#ff6f91", "#ff9eb5", "#ffd6e0", "#3d3d3d", "#a7c4a0"],
        "Zebra": ["#111111", "#3a3a3a", "#8c8c8c", "#e0e0e0", "#fafafa"],
        "Parrot": ["#e63946", "#f4a261", "#2a9d8f", "#264653", "#e9c46a"],
        "Fox": ["#d35400", "#e67e22", "#f5cba7", "#fdfefe", "#2c2c2c"],
    ],
    "Ocean" => [
        "Lagoon": ["#03045e", "#0077b6", "#00b4d8", "#90e0ef", "#caf0f8"],
        "Coral Reef": ["#ff7f50", "#ff6f61", "#ffd6a5", "#00a6a6", "#005f73"],
        "Deep Sea": ["#0b132b", "#1c2541", "#3a506b", "#5bc0be", "#e0fbfc"],
        "Seafoam": ["#e0f2e9", "#b5e2d3", "#7cc6b6", "#3a9188", "#1f4e4a"],
    ],
    "Sky" => [
        "Sunset": ["#355070", "#6d597a", "#b56576", "#e56b6f", "#eaac8b"],
        "Sunrise": ["#ffcad4", "#ffd6a5", "#fdffb6", "#a0c4ff", "#6a4c93"],
        "Night": ["#0d1b2a", "#1b263b", "#415a77", "#778da9", "#e0e1dd"],
        "Aurora": ["#011627", "#2ec4b6", "#7bf1a8", "#c77dff", "#e0aaff"],
    ],
    "Earth" => [
        "Desert": ["#c2956b", "#d9b48f", "#efd9b4", "#8c5e3c", "#4a3728"],
        "Canyon": ["#9c3d1f", "#c1663f", "#e09f6b", "#f2d0a4", "#4b2e2a"],
        "Clay": ["#b5651d", "#d08c60", "#e8c1a0", "#7a4e2d", "#f6eee3"],
        "Stone": ["#2f3e46", "#354f52", "#52796f", "#84a98c", "#cad2c5"],
    ],
    "Gems" => [
        "Emerald": ["#004b23", "#007200", "#38b000", "#9ef01a", "#ccff33"],
        "Amethyst": ["#3c096c", "#5a189a", "#7b2cbf", "#9d4edd", "#e0aaff"],
        "Ruby": ["#590d22", "#800f2f", "#a4133c", "#c9184a", "#ff8fa3"],
        "Sapphire": ["#03045e", "#023e8a", "#0353a4", "#4895ef", "#bde0fe"],
    ],
    "Food" => [
        "Coffee": ["#3e2723", "#6d4c41", "#a1887f", "#d7ccc8", "#efebe9"],
        "Macarons": ["#f7cad0", "#cdeac0", "#fde4cf", "#b9d8f0", "#e2cfea"],
        "Spice": ["#9b2226", "#bb3e03", "#ca6702", "#ee9b00", "#e9d8a6"],
        "Mint Chocolate": ["#3b2a20", "#5c4033", "#a8e6cf", "#dcedc1", "#f6fff8"],
    ],
    "Seasons" => [
        "Spring": ["#a8e6cf", "#dcedc1", "#ffd3b6", "#ffaaa5", "#ff8b94"],
        "Summer": ["#ff595e", "#ffca3a", "#8ac926", "#1982c4", "#6a4c93"],
        "Autumn": ["#6f1d1b", "#bb9457", "#432818", "#99582a", "#ffe6a7"],
        "Winter": ["#e3f2fd", "#bbdefb", "#90caf9", "#5c6bc0", "#283593"],
    ],
];

/// The categories of [`THEMES`], in order.
pub fn categories() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for t in THEMES {
        if !out.contains(&t.category) {
            out.push(t.category);
        }
    }
    out
}

/// The themes whose name or category contains every word of `query` (case-insensitive); all of them for an empty
/// query.
pub fn search(query: &str) -> Vec<&'static Theme> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    THEMES
        .iter()
        .filter(|t| {
            let hay = format!("{} {}", t.category, t.name).to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_and_rejects_junk() {
        assert_eq!(Rgb::parse("#ff8800"), Some(Rgb([255, 136, 0])));
        assert_eq!(Rgb::parse("f80"), Some(Rgb([255, 136, 0])));
        assert_eq!(Rgb([1, 2, 255]).hex(), "#0102ff");
        for bad in ["", "#", "#12345", "#gggggg", "ééé", "#ff88001"] {
            assert_eq!(Rgb::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn every_theme_is_five_valid_colours() {
        assert!(THEMES.len() >= 30);
        for t in THEMES {
            assert!(t.colors.iter().all(|c| Rgb::parse(c).is_some()), "{}", t.name);
            let p = t.palette();
            assert_eq!((p.colors.len(), p.gradients.len(), p.patterns.len()), (COUNT, COUNT, COUNT));
        }
        assert!(categories().contains(&"Fruits") && categories().contains(&"Animals") && categories().contains(&"Plants"));
    }

    #[test]
    fn search_matches_names_and_categories() {
        assert!(search("mango").iter().any(|t| t.name == "Mango"));
        assert!(search("FRUITS").len() >= 5);
        assert!(search("animals fox").iter().all(|t| t.name == "Fox"));
        assert_eq!(search("").len(), THEMES.len());
        assert!(search("nothing-like-this").is_empty());
    }

    #[test]
    fn a_full_reply_parses() {
        let reply = r##"```json
        {"name":"Tropical","colors":["#ff6b6b","#feca57","#48dbfb","#1dd1a1","#5f27cd"],
         "gradients":[{"name":"Beach","stops":["#feca57","#48dbfb"],"angle":45}],
         "patterns":[{"name":"Palm","kind":"waves","ground":"#1dd1a1","ink":"#5f27cd","scale":20}]}
        ```"##;
        let p = parse(reply).unwrap();
        assert_eq!(p.name, "Tropical");
        assert_eq!(p.colors[0], Rgb([0xff, 0x6b, 0x6b]));
        assert_eq!(p.gradients[0].name, "Beach");
        assert_eq!(p.patterns[0].kind, PatternKind::Waves);
        // The rest are made from the colours.
        assert_eq!((p.gradients.len(), p.patterns.len()), (COUNT, COUNT));
    }

    #[test]
    fn partial_and_hostile_replies_never_panic() {
        assert!(parse("").is_err());
        assert!(parse("{}").is_err());
        assert!(parse(r#"{"colors":["nope"]}"#).is_err());
        let p = parse(r##"{"colors":["#000000"],"gradients":[{"stops":["#fff"]},{"stops":"x"}],"patterns":[{"kind":"spirals"},5]}"##).unwrap();
        assert_eq!(p.colors.len(), COUNT);
        let p = parse(r##"{"colors":["#000","#111","#222","#333","#444","#555","#666"],"patterns":[{"kind":"dots","ground":"#000","ink":"#fff","scale":1e30}],"gradients":[{"stops":["#000","#fff"],"angle":"NaN"}]}"##).unwrap();
        assert_eq!(p.colors.len(), COUNT);
        assert_eq!(p.patterns[0].scale, SCALE_RANGE.1);
    }

    #[test]
    fn pattern_tiles_are_square_and_two_coloured() {
        for kind in PatternKind::ALL {
            for scale in [0, 4, 13, 64, u32::MAX] {
                let p = Pattern { name: "t".into(), kind, ground: Rgb([0, 0, 0]), ink: Rgb([255, 255, 255]), scale };
                let (n, px) = p.render();
                assert_eq!(px.len(), (n * n * 4) as usize, "{kind:?} {scale}");
                assert!(n <= SCALE_RANGE.1 * 4);
                assert!(px.chunks(4).any(|c| c[0] == 255) && px.chunks(4).any(|c| c[0] == 0), "{kind:?} {scale} shows both colours");
            }
        }
    }
}
