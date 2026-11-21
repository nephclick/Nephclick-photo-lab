//! Colour harmonies: five colours made from one base colour by a rule of the colour wheel, as palette tools offer
//! them. Pure arithmetic in HSB on the RGB wheel; every rule keeps the base colour first.

use crate::palette::Rgb;

/// A rule that makes a palette from one colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Harmony {
    Complementary,
    Analogous,
    Triadic,
    SplitComplementary,
    DoubleSplitComplementary,
    Square,
    Rectangle,
    Compound,
    Monochromatic,
    Shades,
    Tints,
    Tones,
    WarmCool,
}

impl Harmony {
    /// Every rule, in the order the dropdown lists them.
    pub const ALL: [Harmony; 13] = [
        Harmony::Complementary,
        Harmony::Analogous,
        Harmony::Triadic,
        Harmony::SplitComplementary,
        Harmony::DoubleSplitComplementary,
        Harmony::Square,
        Harmony::Rectangle,
        Harmony::Compound,
        Harmony::Monochromatic,
        Harmony::Shades,
        Harmony::Tints,
        Harmony::Tones,
        Harmony::WarmCool,
    ];

    /// The rule's name.
    pub fn name(self) -> &'static str {
        match self {
            Harmony::Complementary => "Complementary",
            Harmony::Analogous => "Analogous",
            Harmony::Triadic => "Triadic",
            Harmony::SplitComplementary => "Split Complementary",
            Harmony::DoubleSplitComplementary => "Double Split Complementary",
            Harmony::Square => "Square",
            Harmony::Rectangle => "Rectangle (Tetradic)",
            Harmony::Compound => "Compound",
            Harmony::Monochromatic => "Monochromatic",
            Harmony::Shades => "Shades",
            Harmony::Tints => "Tints",
            Harmony::Tones => "Tones",
            Harmony::WarmCool => "Warm and Cool",
        }
    }

    /// Five colours from `base` by this rule, the base first.
    pub fn colors(self, base: Rgb) -> Vec<Rgb> {
        let (h, s, v) = to_hsv(base);
        let hue = |deg: f32| (h + deg / 360.0).rem_euclid(1.0);
        let at = |deg: f32, s: f32, v: f32| from_hsv(hue(deg), s.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
        // A lighter and a darker step of the same hue, to round a rule of fewer hues out to five colours.
        let light = |deg: f32| at(deg, s * 0.6, (v + 0.25).min(1.0));
        let dark = |deg: f32| at(deg, (s + 0.1).min(1.0), v * 0.6);
        let colors = match self {
            Harmony::Complementary => vec![base, light(0.0), at(180.0, s, v), light(180.0), dark(180.0)],
            Harmony::Analogous => vec![base, at(-30.0, s, v), at(30.0, s, v), at(-60.0, s, v), at(60.0, s, v)],
            Harmony::Triadic => vec![base, at(120.0, s, v), at(240.0, s, v), light(120.0), light(240.0)],
            Harmony::SplitComplementary => vec![base, at(150.0, s, v), at(210.0, s, v), light(0.0), dark(180.0)],
            Harmony::DoubleSplitComplementary => vec![base, at(30.0, s, v), at(150.0, s, v), at(210.0, s, v), at(-30.0, s, v)],
            Harmony::Square => vec![base, at(90.0, s, v), at(180.0, s, v), at(270.0, s, v), light(0.0)],
            Harmony::Rectangle => vec![base, at(60.0, s, v), at(180.0, s, v), at(240.0, s, v), light(0.0)],
            Harmony::Compound => vec![base, at(30.0, s, v), at(165.0, s * 0.8, v), at(195.0, s, v * 0.8), dark(0.0)],
            Harmony::Monochromatic => vec![base, at(0.0, s * 0.45, v), at(0.0, s, v * 0.55), at(0.0, s * 0.7, (v + 0.2).min(1.0)), at(0.0, s, v * 0.3)],
            Harmony::Shades => (0..5).map(|i| at(0.0, s, v * (1.0 - i as f32 * 0.18))).collect(),
            Harmony::Tints => (0..5).map(|i| at(0.0, s * (1.0 - i as f32 * 0.2), v + (1.0 - v) * i as f32 * 0.22)).collect(),
            Harmony::Tones => (0..5).map(|i| at(0.0, s * (1.0 - i as f32 * 0.2), v * (1.0 - i as f32 * 0.06))).collect(),
            Harmony::WarmCool => {
                // The base, two warmer and two cooler colours: hues moved toward orange (30°) and toward blue (210°).
                let toward = |target: f32, amount: f32| {
                    let mut d = target / 360.0 - h;
                    if d > 0.5 {
                        d -= 1.0;
                    } else if d < -0.5 {
                        d += 1.0;
                    }
                    from_hsv((h + d * amount).rem_euclid(1.0), s, v)
                };
                vec![base, toward(30.0, 0.4), toward(30.0, 0.8), toward(210.0, 0.4), toward(210.0, 0.8)]
            }
        };
        let mut colors = colors;
        if let Some(first) = colors.first_mut() {
            *first = base;
        }
        colors
    }
}

/// `c` as hue, saturation and brightness, each 0..=1.
fn to_hsv(c: Rgb) -> (f32, f32, f32) {
    let [r, g, b] = c.0.map(|x| f32::from(x) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= f32::EPSILON {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    let s = if max <= f32::EPSILON { 0.0 } else { d / max };
    (h, s, max)
}

/// The colour of hue `h`, saturation `s` and brightness `v`, each 0..=1.
fn from_hsv(h: f32, s: f32, v: f32) -> Rgb {
    let h6 = h.rem_euclid(1.0) * 6.0;
    let f = h6 - h6.floor();
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let (r, g, b) = match h6.floor() as i32 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    let byte = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgb([byte(r), byte(g), byte(b)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rule_gives_five_colours_starting_with_the_base() {
        for base in [Rgb([240, 70, 30]), Rgb([0, 0, 0]), Rgb([255, 255, 255]), Rgb([30, 140, 220])] {
            for rule in Harmony::ALL {
                let colors = rule.colors(base);
                assert_eq!(colors.len(), 5, "{rule:?}");
                assert_eq!(colors[0], base, "{rule:?}");
            }
        }
    }

    #[test]
    fn complementary_is_across_the_wheel() {
        let colors = Harmony::Complementary.colors(Rgb([255, 0, 0]));
        assert_eq!(colors[2], Rgb([0, 255, 255]));
    }

    #[test]
    fn hsv_round_trips() {
        for c in [Rgb([12, 200, 99]), Rgb([255, 128, 0]), Rgb([77, 77, 77])] {
            let (h, s, v) = to_hsv(c);
            let back = from_hsv(h, s, v);
            assert!(c.0.iter().zip(back.0).all(|(a, b)| a.abs_diff(b) <= 1), "{c:?} → {back:?}");
        }
    }
}
