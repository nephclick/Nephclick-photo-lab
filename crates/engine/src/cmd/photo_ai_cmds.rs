//! One-click photo retouching that runs on this computer, offline, with the AI Selection Models:
//!
//! - **Filter › Blur › Background Blur…** (`filter.blur.backgroundBlur`): the subject model finds the main subject;
//!   everything else is blurred, more where the depth model says it is farther away (a lens's depth of field). The
//!   blur averages background pixels only, so the subject's colours never bleed into it.
//! - **Filter › Skin Retouch…** (`filter.skinRetouch`): skin is found by its colour (inside the subject when the model
//!   is installed) and smoothed by frequency separation: the tones even out while Texture keeps pores and fine
//!   detail.
//!
//! The masks and the smoothing are worked out on the picture reduced to at most 1024 pixels on a side and applied to
//! the full-size layer in bands of rows, so large photos stay within an 8 GB machine's memory. The models' answers are
//! kept for the last picture, so moving a slider in the dialog's live preview does not run them again.

use std::sync::{Mutex, PoisonError};

use nephclick_photolab_color::ColorMode;
use nephclick_photolab_doc::LayerContent;
use nephclick_photolab_geom::Rect;
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

const BLUR: &str = "filter.blur.backgroundBlur";
const SKIN: &str = "filter.skinRetouch";
/// The longest side the masks and the smoothing are worked out at.
const WORK_SIDE: u32 = 1024;
/// Rows of the full-size layer read and written at a time.
const BAND: i32 = 256;

fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}

fn num(p: &Value, cmd: &str, key: &str, lo: f32, hi: f32, default: f32) -> Result<f32> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_f64()
            .map(|v| v as f32)
            .filter(|v| v.is_finite() && (lo..=hi).contains(v))
            .ok_or_else(|| bad(cmd, format!("`{key}` must be a number {lo}..={hi}"))),
    }
}

/// An RGB document whose active layer is an unlocked pixel layer.
fn has_rgb_pixels(s: &Session) -> std::result::Result<(), String> {
    let d = s.active().ok_or("no document open")?;
    if d.doc.mode != ColorMode::Rgb {
        return Err("works on RGB documents (Image › Mode › RGB Color)".into());
    }
    let id = d.active_layer.ok_or("no active layer")?;
    let l = d.doc.layer(id).ok_or("no active layer")?;
    if !matches!(l.content, LayerContent::Raster(_)) {
        return Err(format!("needs a pixel layer (the active layer is {} {} layer)", l.content.article(), l.content.kind_name()));
    }
    let locks = d.doc.effective_locks(id);
    if locks.pixels || locks.all {
        return Err(format!("the layer \"{}\" is locked", l.name));
    }
    Ok(())
}

/// The masks the models made for the last picture: (fingerprint, width, height, subject 0..=1, depth 0..=1 nearest = 1).
type Masks = (u64, usize, usize, Option<Vec<f32>>, Option<Vec<f32>>);
static MASKS: Mutex<Option<Masks>> = Mutex::new(None);

fn fingerprint(px: &[u8]) -> u64 {
    // FNV-1a over a sparse sample of the picture and its length: enough to tell pictures apart, cheap per preview.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let step = (px.len() / 4096).max(1);
    for b in px.iter().step_by(step).chain(px.len().to_le_bytes().iter()) {
        h = (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// The subject coverage and depth of `rgba` (`w` × `h`, RGBA8): from the cache when it is the same picture.
fn masks(rgba: &[u8], w: usize, h: usize, want_depth: bool) -> (Option<Vec<f32>>, Option<Vec<f32>>) {
    let key = fingerprint(rgba);
    let mut held = MASKS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((k, cw, ch, subject, depth)) = held.as_ref()
        && *k == key
        && (*cw, *ch) == (w, h)
        && (!want_depth || depth.is_some())
    {
        return (subject.clone(), depth.clone());
    }
    let subject = nephclick_photolab_ai::subject_model::segment(rgba, w as u32, h as u32)
        .ok()
        .filter(|m| m.mask.len() == w * h)
        .map(|m| m.mask.iter().map(|v| f32::from(*v) / 255.0).collect::<Vec<f32>>());
    let depth = if want_depth { nephclick_photolab_ai::depth_model::estimate(rgba, w as u32, h as u32).ok().filter(|d| d.len() == w * h) } else { None };
    *held = Some((key, w, h, subject.clone(), depth.clone()));
    (subject, depth)
}

/// The active layer, reduced to `sw` × `sh` (box average), as RGBA floats; and the reduced composite as RGBA8 for the
/// models.
struct Small {
    w: usize,
    h: usize,
    layer: Vec<f32>,
    picture: Vec<u8>,
}

fn small_of(s: &Session, bounds: Rect) -> Result<Small> {
    let d = s.active().ok_or(EngineError::NoDocument)?;
    let id = d.active_layer.ok_or_else(|| EngineError::Other("no active layer".into()))?;
    let surface = d.doc.layer(id).and_then(|l| l.surface()).ok_or_else(|| EngineError::Other("needs a pixel layer".into()))?;
    if surface.channels() != 4 {
        return Err(EngineError::Other("works on RGB layers".into()));
    }
    let thumb = nephclick_photolab_compose::thumbnail(&d.doc, WORK_SIDE);
    let (sw, sh) = (thumb.width.max(1) as usize, thumb.height.max(1) as usize);
    let (bw, bh) = (bounds.width().max(1) as usize, bounds.height().max(1) as usize);
    let mut sum = vec![0.0f32; sw * sh * 4];
    let mut count = vec![0u32; sw * sh];
    let mut y0 = bounds.y0;
    while y0 < bounds.y1 {
        let y1 = (y0 + BAND).min(bounds.y1);
        let band = Rect::new(bounds.x0, y0, bounds.x1, y1);
        let px = surface.read_region(band);
        for (i, q) in px.chunks_exact(4).enumerate() {
            let x = i % bw;
            let y = (y0 - bounds.y0) as usize + i / bw;
            let (sx, sy) = ((x * sw / bw).min(sw - 1), (y * sh / bh).min(sh - 1));
            let at = sy * sw + sx;
            if let (Some(acc), Some(n)) = (sum.get_mut(at * 4..at * 4 + 4), count.get_mut(at)) {
                for (a, v) in acc.iter_mut().zip(q) {
                    *a += v;
                }
                *n += 1;
            }
        }
        y0 = y1;
    }
    for (at, n) in count.iter().enumerate() {
        if *n > 0
            && let Some(acc) = sum.get_mut(at * 4..at * 4 + 4)
        {
            acc.iter_mut().for_each(|a| *a /= *n as f32);
        }
    }
    Ok(Small { w: sw, h: sh, layer: sum, picture: thumb.pixels })
}

/// `v` (`n` channels per pixel, `w` × `h`) blurred by three box passes of radius `r` each way: close to a Gaussian.
fn blur(v: &[f32], n: usize, w: usize, h: usize, r: usize) -> Vec<f32> {
    if r == 0 || w == 0 || h == 0 {
        return v.to_vec();
    }
    let mut a = v.to_vec();
    let mut b = vec![0.0f32; a.len()];
    for _ in 0..3 {
        for horizontal in [true, false] {
            let (lines, len) = if horizontal { (h, w) } else { (w, h) };
            let idx = |line: usize, i: usize| if horizontal { line * w + i } else { i * w + line };
            for line in 0..lines {
                for c in 0..n {
                    let get = |i: isize| a.get(idx(line, i.clamp(0, len as isize - 1) as usize) * n + c).copied().unwrap_or(0.0);
                    let mut acc: f32 = (-(r as isize)..=r as isize).map(get).sum();
                    let span = (2 * r + 1) as f32;
                    for i in 0..len {
                        if let Some(out) = b.get_mut(idx(line, i) * n + c) {
                            *out = acc / span;
                        }
                        acc += get(i as isize + r as isize + 1) - get(i as isize - r as isize);
                    }
                }
            }
            std::mem::swap(&mut a, &mut b);
        }
    }
    a
}

/// `v` (`n` channels) at fractional pixel (`x`, `y`), bilinear, edges held.
fn sample(v: &[f32], n: usize, w: usize, h: usize, x: f32, y: f32, c: usize) -> f32 {
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let at = |xx: usize, yy: usize| v.get((yy * w + xx) * n + c).copied().unwrap_or(0.0);
    (at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx) * (1.0 - ty) + (at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx) * ty
}

/// The colour `c` (RGBA) of `v` averaged with weights `wgt` over radius `r`: blur(c·w)/blur(w), the pixel itself where
/// the weights vanish.
fn weighted_blur(v: &[f32], wgt: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut premul = Vec::with_capacity(v.len() + wgt.len());
    for (q, k) in v.chunks_exact(4).zip(wgt) {
        premul.extend([q[0] * k, q[1] * k, q[2] * k, q[3] * k, *k]);
    }
    let blurred = blur(&premul, 5, w, h, r);
    let mut out = Vec::with_capacity(v.len());
    for (i, q) in blurred.chunks_exact(5).enumerate() {
        if q[4] > 1e-4 {
            out.extend([q[0] / q[4], q[1] / q[4], q[2] / q[4], q[3] / q[4]]);
        } else {
            out.extend(v.get(i * 4..i * 4 + 4).unwrap_or(&[0.0; 4]).iter().copied());
        }
    }
    out
}

/// Writes `f(original, x, y)` over the active layer within `bounds`, in bands, as one History step named `name`.
/// `x`, `y` are in the reduced picture's pixels.
fn apply(s: &mut Session, name: &str, bounds: Rect, small: (usize, usize), f: impl Fn([f32; 4], f32, f32) -> [f32; 4]) -> Result<()> {
    let (sw, sh) = small;
    let (bw, bh) = (bounds.width().max(1) as f32, bounds.height().max(1) as f32);
    s.edit(name, |doc, active| {
        let id = active.ok_or_else(|| EngineError::Other("no active layer".into()))?;
        let surface = doc.layer_mut(id).and_then(|l| l.surface_mut()).ok_or_else(|| EngineError::Other("needs a pixel layer".into()))?;
        let w = bounds.width().max(1) as usize;
        let mut y0 = bounds.y0;
        while y0 < bounds.y1 {
            let y1 = (y0 + BAND).min(bounds.y1);
            let band = Rect::new(bounds.x0, y0, bounds.x1, y1);
            let mut px = surface.read_region(band);
            for (i, q) in px.chunks_exact_mut(4).enumerate() {
                let x = (i % w) as f32;
                let y = (y0 - bounds.y0) as f32 + (i / w) as f32;
                let (u, v) = ((x + 0.5) / bw * sw as f32 - 0.5, (y + 0.5) / bh * sh as f32 - 0.5);
                let out = f([q[0], q[1], q[2], q[3]], u, v);
                for (dst, val) in q.iter_mut().zip(out) {
                    *dst = if val.is_finite() { val.clamp(0.0, 1.0) } else { 0.0 };
                }
            }
            surface.write_region(band, &px);
            y0 = y1;
        }
        Ok(())
    })
}

fn background_blur(s: &mut Session, p: &Value) -> Result<Value> {
    has_rgb_pixels(s).map_err(EngineError::Other)?;
    let amount = num(p, BLUR, "amount", 0.0, 200.0, 24.0)?;
    let feather = num(p, BLUR, "feather", 0.0, 100.0, 12.0)?;
    let use_depth = p.get("depth").and_then(Value::as_bool).unwrap_or(true);
    let bounds = s.active().ok_or(EngineError::NoDocument)?.doc.bounds();
    if bounds.is_empty() {
        return Err(EngineError::Other("the document is empty".into()));
    }
    let small = small_of(s, bounds)?;
    let (sw, sh) = (small.w, small.h);
    let (subject, depth) = masks(&small.picture, sw, sh, use_depth);
    let subject = subject.ok_or_else(|| {
        EngineError::Other("Background Blur finds the subject with the AI Selection Models: install them in Select › AI Selection Models…".into())
    })?;
    let scale = sw as f32 / bounds.width().max(1) as f32;
    // How much each pixel is background: not subject, scaled by distance when the depth model is there.
    let mut weight: Vec<f32> = subject.iter().map(|m| 1.0 - m).collect();
    if let Some(depth) = &depth {
        for (k, d) in weight.iter_mut().zip(depth) {
            *k *= ((1.0 - d) * 1.6 + 0.25).clamp(0.0, 1.0);
        }
    }
    let weight = blur(&weight, 1, sw, sh, ((feather * scale).round() as usize).max(1));
    let background: Vec<f32> = subject.iter().map(|m| (1.0 - m).max(0.0)).collect();
    let radius = ((amount * scale).round() as usize).max(1);
    let blurred = weighted_blur(&small.layer, &background, sw, sh, radius);
    apply(s, "Background Blur", bounds, (sw, sh), |q, u, v| {
        let k = sample(&weight, 1, sw, sh, u, v, 0);
        std::array::from_fn(|c| q[c] + (sample(&blurred, 4, sw, sh, u, v, c) - q[c]) * k)
    })?;
    Ok(json!({"depth": depth.is_some()}))
}

/// How much `rgb` (0..=1) looks like skin, 0..=1: the YCbCr skin box, softened at its edges.
fn skin_likeness(r: f32, g: f32, b: f32) -> f32 {
    let cb = 128.0 + (-0.168_736 * r - 0.331_264 * g + 0.5 * b) * 255.0;
    let cr = 128.0 + (0.5 * r - 0.418_688 * g - 0.081_312 * b) * 255.0;
    let y = (0.299 * r + 0.587 * g + 0.114 * b) * 255.0;
    let inside = |v: f32, lo: f32, hi: f32, soft: f32| ((v - lo) / soft).clamp(0.0, 1.0) * ((hi - v) / soft).clamp(0.0, 1.0);
    inside(cb, 77.0, 127.0, 6.0) * inside(cr, 133.0, 173.0, 6.0) * inside(y, 40.0, 250.0, 20.0)
}

fn skin_retouch(s: &mut Session, p: &Value) -> Result<Value> {
    has_rgb_pixels(s).map_err(EngineError::Other)?;
    let smoothing = num(p, SKIN, "smoothing", 0.0, 100.0, 50.0)? / 100.0;
    let texture = num(p, SKIN, "texture", 0.0, 100.0, 35.0)? / 100.0;
    let bounds = s.active().ok_or(EngineError::NoDocument)?.doc.bounds();
    if bounds.is_empty() {
        return Err(EngineError::Other("the document is empty".into()));
    }
    let small = small_of(s, bounds)?;
    let (sw, sh) = (small.w, small.h);
    let (subject, _) = masks(&small.picture, sw, sh, false);
    let mut skin: Vec<f32> = small.layer.chunks_exact(4).map(|q| skin_likeness(q[0], q[1], q[2])).collect();
    if let Some(subject) = &subject {
        for (k, m) in skin.iter_mut().zip(subject) {
            *k *= m;
        }
    }
    let skin = blur(&skin, 1, sw, sh, 2);
    if skin.iter().all(|k| *k < 0.02) {
        return Err(EngineError::Other("no skin was found in this picture".into()));
    }
    // Frequency separation on the reduced picture: the low frequencies of the skin, averaged over skin only, replace
    // its tones; the high frequencies (pores, fine lines) come back at Texture's strength.
    let side = sw.max(sh) as f32;
    let low_r = ((side / 220.0).round() as usize).max(1);
    let smooth_r = ((side / 90.0 * (0.4 + smoothing)).round() as usize).max(low_r + 1);
    let low = blur(&small.layer, 4, sw, sh, low_r);
    let smooth = weighted_blur(&small.layer, &skin, sw, sh, smooth_r);
    apply(s, "Skin Retouch", bounds, (sw, sh), |q, u, v| {
        let k = sample(&skin, 1, sw, sh, u, v, 0) * smoothing;
        std::array::from_fn(|c| {
            if c == 3 {
                return q[3];
            }
            let high = q[c] - sample(&low, 4, sw, sh, u, v, c);
            let retouched = sample(&smooth, 4, sw, sh, u, v, c) + high * texture;
            q[c] + (retouched - q[c]) * k
        })
    })?;
    Ok(json!({"subject": subject.is_some()}))
}

/// Background Blur and Skin Retouch.
pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: BLUR,
            label: "Background Blur…",
            menu: &["Filter", "Blur"],
            shortcut: None,
            params: r##"{"amount":0..200=24 (blur radius, px),"feather":0..100=12 (edge softness, px),"depth":bool=true (farther is blurrier, with the depth model)} (the subject stays sharp; needs the AI Selection Models)"##,
            enabled: has_rgb_pixels,
            run: background_blur,
            journal: true,
        },
        CommandSpec {
            id: SKIN,
            label: "Skin Retouch…",
            menu: &["Filter"],
            shortcut: None,
            params: r##"{"smoothing":0..100=50,"texture":0..100=35 (how much pore and fine detail stays)} (skin found by colour, inside the subject when the AI Selection Models are installed)"##,
            enabled: has_rgb_pixels,
            run: skin_retouch,
            journal: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skin_tones_look_like_skin_and_others_do_not() {
        assert!(skin_likeness(0.87, 0.67, 0.55) > 0.5, "light skin");
        assert!(skin_likeness(0.55, 0.37, 0.27) > 0.5, "darker skin");
        assert!(skin_likeness(0.1, 0.4, 0.9) < 0.05, "blue");
        assert!(skin_likeness(0.2, 0.7, 0.2) < 0.05, "green");
    }

    #[test]
    fn blur_keeps_a_flat_field_flat_and_spreads_a_spike() {
        let flat = vec![0.5f32; 20 * 10];
        assert!(blur(&flat, 1, 20, 10, 3).iter().all(|v| (v - 0.5).abs() < 1e-5));
        let mut spike = vec![0.0f32; 21 * 21];
        spike[10 * 21 + 10] = 1.0;
        let out = blur(&spike, 1, 21, 21, 2);
        assert!(out[10 * 21 + 10] < 1.0 && out[10 * 21 + 12] > 0.0);
        let total: f32 = out.iter().sum();
        assert!((total - 1.0).abs() < 0.05, "energy kept: {total}");
    }

    #[test]
    fn weighted_blur_ignores_weightless_pixels() {
        // Left half red (weight 1), right half blue (weight 0): the blur stays red.
        let (w, h) = (8, 2);
        let mut v = Vec::new();
        let mut k = Vec::new();
        for _ in 0..h {
            for x in 0..w {
                v.extend(if x < 4 { [1.0, 0.0, 0.0, 1.0] } else { [0.0, 0.0, 1.0, 1.0] });
                k.push(if x < 4 { 1.0 } else { 0.0 });
            }
        }
        let out = weighted_blur(&v, &k, w, h, 2);
        assert!(out[0] > 0.99 && out[2] < 0.01, "red pixel unchanged");
        assert!(out[3 * 4] > 0.99, "edge red pixel takes no blue");
    }

    #[test]
    fn commands_refuse_bad_params_and_wrong_layers() {
        let mut s = Session::new();
        assert!(s.execute(BLUR, json!({})).is_err(), "no document");
        s.execute("file.new", json!({"width": 32, "height": 24})).unwrap();
        for p in [json!({"amount": -1}), json!({"amount": 1e9}), json!({"feather": "x"})] {
            assert!(s.execute(BLUR, p.clone()).is_err(), "{p}");
        }
        for p in [json!({"smoothing": 101}), json!({"texture": -5})] {
            assert!(s.execute(SKIN, p.clone()).is_err(), "{p}");
        }
        s.execute("layer.newAdjustmentLayer.curves", json!({})).unwrap();
        assert!(s.execute(SKIN, json!({})).is_err(), "an adjustment layer is no pixel layer");
    }

    #[test]
    fn skin_retouch_smooths_a_skin_coloured_picture_in_one_step() {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 64, "height": 48})).unwrap();
        s.execute("edit.fill", json!({"color": "#dca88c"})).unwrap();
        let steps = s.journal.len();
        let r = s.execute(SKIN, json!({"smoothing": 80}));
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(s.journal.len(), steps + 1, "one History step");
    }
}
