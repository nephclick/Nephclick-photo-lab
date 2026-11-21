//! The range eyedropper of Hue/Saturation and Selective Color: the button beside Range (Colors) arms it, and the next
//! click on the image shows the range that pixel's colour belongs to (Reds … Magentas by hue; Selective Color's
//! Whites, Neutrals and Blacks for light, grey and dark pixels). It works in the Image › Adjustments dialogs and in
//! an adjustment layer's editor alike, so the range shown is kept here, beside the editors.

use std::sync::{Mutex, PoisonError};

use crate::NephclickPhotoLabApp;

/// Which editor armed the eyedropper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    HueSaturation,
    SelectiveColor,
}

/// The ranges shown and the armed eyedropper.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Shown {
    /// Hue/Saturation: `None` before its first pass, then Master (`None`) or 0 Reds … 5 Magentas.
    pub(crate) hue: Option<Option<usize>>,
    /// Selective Color: 0 Reds … 8 Blacks, once known.
    pub(crate) selective: Option<usize>,
    armed: Option<Kind>,
}

static SHOWN: Mutex<Shown> = Mutex::new(Shown { hue: None, selective: None, armed: None });

fn edit<T>(f: impl FnOnce(&mut Shown) -> T) -> T {
    f(&mut SHOWN.lock().unwrap_or_else(PoisonError::into_inner))
}

/// The ranges shown now.
pub(crate) fn shown() -> Shown {
    edit(|s| *s)
}

/// After a Hue/Saturation pass: the range it shows, and whether its eyedropper was clicked.
pub(crate) fn after_hue(range: Option<usize>, pick: bool) {
    edit(|s| {
        s.hue = Some(range);
        if pick {
            s.armed = Some(Kind::HueSaturation);
        }
    });
}

/// After a Selective Color pass.
pub(crate) fn after_selective(range: usize, pick: bool) {
    edit(|s| {
        s.selective = Some(range);
        if pick {
            s.armed = Some(Kind::SelectiveColor);
        }
    });
}

/// Whether a click on the image picks a range.
pub(crate) fn armed() -> bool {
    edit(|s| s.armed.is_some())
}

/// The range a colour `rgb` (0..=1) belongs to, for `kind`: `None` for Hue/Saturation on a grey pixel (no hue).
pub(crate) fn range_of(kind: Kind, rgb: [f32; 3]) -> Option<usize> {
    let [r, g, b] = rgb.map(|c| c.clamp(0.0, 1.0));
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let s = if max > 0.0 { d / max } else { 0.0 };
    if kind == Kind::SelectiveColor {
        if s < 0.15 {
            return Some(if max > 0.85 {
                6
            } else if max < 0.15 {
                8
            } else {
                7
            });
        }
    } else if s < 0.08 || d <= f32::EPSILON {
        return None;
    }
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    // Six ranges centred on 0° Reds, 60° Yellows … 300° Magentas.
    Some(((h + 0.5).floor() as usize) % 6)
}

/// A click at `at` (document pixels) with the eyedropper armed: shows the range of the pixel's colour there and disarms.
/// Returns whether the click was the eyedropper's.
pub(crate) fn pick(app: &mut NephclickPhotoLabApp, at: [f64; 2]) -> bool {
    let Some(kind) = edit(|s| s.armed.take()) else { return false };
    let Some(rgb) = crate::canvas::composite_color(app, at[0], at[1]) else { return true };
    const NAMES: [&str; 9] = ["Reds", "Yellows", "Greens", "Cyans", "Blues", "Magentas", "Whites", "Neutrals", "Blacks"];
    match range_of(kind, rgb) {
        Some(range) => {
            edit(|s| match kind {
                Kind::HueSaturation => s.hue = Some(Some(range)),
                Kind::SelectiveColor => s.selective = Some(range),
            });
            let name = NAMES.get(range).copied().unwrap_or("Reds");
            app.ui.status = crate::i18n::fmt(tl!("Range: {name}"), &[("name", tl!(name))]);
        }
        None => {
            app.ui.status = tl!("That pixel is grey: pick a coloured part of the image").to_string();
        }
    }
    app.ctx().request_repaint();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_fall_in_their_ranges() {
        let hue = Kind::HueSaturation;
        assert_eq!(range_of(hue, [1.0, 0.05, 0.05]), Some(0), "red");
        assert_eq!(range_of(hue, [0.95, 0.9, 0.1]), Some(1), "yellow");
        assert_eq!(range_of(hue, [0.1, 0.8, 0.2]), Some(2), "green");
        assert_eq!(range_of(hue, [0.1, 0.8, 0.85]), Some(3), "cyan");
        assert_eq!(range_of(hue, [0.1, 0.2, 0.9]), Some(4), "blue");
        assert_eq!(range_of(hue, [0.85, 0.1, 0.8]), Some(5), "magenta");
        assert_eq!(range_of(hue, [0.5, 0.5, 0.5]), None, "grey has no hue");
        let sel = Kind::SelectiveColor;
        assert_eq!(range_of(sel, [0.95, 0.95, 0.95]), Some(6), "white");
        assert_eq!(range_of(sel, [0.5, 0.5, 0.52]), Some(7), "neutral");
        assert_eq!(range_of(sel, [0.05, 0.05, 0.05]), Some(8), "black");
        assert_eq!(range_of(sel, [0.1, 0.2, 0.9]), Some(4), "blue");
    }
}
