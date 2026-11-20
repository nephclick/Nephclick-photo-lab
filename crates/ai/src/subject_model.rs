//! Select Subject offline: the ISNet salient-object model run in-process by `tract` (pure Rust, no Python, no GPU
//! driver), with a guided filter that snaps the mask's edges to the picture's.
//!
//! The model sees the picture at 1024 × 1024 and returns how likely each pixel is to belong to the main subject; that
//! map is scaled back to the picture and refined against its luminance, so hair and other fine edges follow the
//! picture even on busy backgrounds. The model is loaded on first use and kept until [`unload`] (Remove, or the
//! memory it holds is wanted back); it runs on all cores and needs a few hundred MB while it runs, so an 8 GB machine
//! is enough.

use std::sync::{Arc, Mutex, PoisonError};

use tract_onnx::prelude::*;

use crate::AiError;
use crate::model_install::{SUBJECT_MODEL_BYTES, subject_model_path};
use crate::selection_manager::SelectionMask;

/// The side the model takes its input at.
const INPUT: usize = 1024;

/// The loaded model, kept between runs.
type Runnable = Arc<TypedRunnableModel>;
static MODEL: Mutex<Option<Runnable>> = Mutex::new(None);

/// Frees the loaded model's memory; the next run loads it again.
pub fn unload() {
    *MODEL.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

fn tract_error(e: impl std::fmt::Display) -> AiError {
    AiError::Unsupported(format!("the subject model couldn't run: {e}"))
}

/// The model, loaded and optimised on first use.
fn model() -> Result<Runnable, AiError> {
    let mut held = MODEL.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(model) = held.as_ref() {
        return Ok(Arc::clone(model));
    }
    let path = subject_model_path();
    let meta = std::fs::metadata(&path)
        .map_err(|_| AiError::NotConfigured("the Select Subject model isn't installed (Select › AI Selection Models…)".into()))?;
    if meta.len() != SUBJECT_MODEL_BYTES {
        return Err(AiError::NotConfigured("the Select Subject model is incomplete: install it again (Select › AI Selection Models…)".into()));
    }
    let model = tract_onnx::onnx()
        .model_for_path(&path)
        .map_err(tract_error)?
        .with_input_fact(0, f32::fact([1, 3, INPUT, INPUT]).into())
        .map_err(tract_error)?
        .into_optimized()
        .map_err(tract_error)?
        .into_runnable()
        .map_err(tract_error)?;
    *held = Some(Arc::clone(&model));
    Ok(model)
}

/// The main subject of `rgba` (`width` × `height`, RGBA8): a coverage mask of the same size, 0..=255.
pub fn segment(rgba: &[u8], width: u32, height: u32) -> Result<SelectionMask, AiError> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || rgba.len() != w.saturating_mul(h).saturating_mul(4) {
        return Err(AiError::Invalid("invalid image dimensions or buffer size".into()));
    }
    let model = model()?;
    let input = preprocess(rgba, w, h);
    let tensor: Tensor = tract_ndarray::Array4::from_shape_vec((1, 3, INPUT, INPUT), input).map_err(tract_error)?.into();
    let outputs = model.run(tvec!(tensor.into())).map_err(tract_error)?;
    let first = outputs.first().ok_or_else(|| AiError::BadReply("the subject model returned nothing".into()))?;
    let view = first.to_plain_array_view::<f32>().map_err(tract_error)?;
    let raw: Vec<f32> = view.iter().copied().collect();
    if raw.len() < INPUT * INPUT {
        return Err(AiError::BadReply("the subject model returned a map of the wrong size".into()));
    }
    let map = normalise(raw.get(..INPUT * INPUT).unwrap_or_default());
    let coarse = resize_plane(&map, INPUT, INPUT, w, h);
    let guide = luminance(rgba, w, h);
    let refined = refine(&guide, &coarse, w, h);
    let mask = refined.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
    Ok(SelectionMask { width, height, mask, metadata: Some("Offline subject model (ISNet)".into()) })
}

/// The model's input: the picture at 1024², channels apart, divided by its brightest value and centred on 0.5 (the
/// normalisation the model was trained with). Transparent pixels count as white, as on a white page.
fn preprocess(rgba: &[u8], w: usize, h: usize) -> Vec<f32> {
    let plane = INPUT * INPUT;
    let mut out = vec![0.0f32; plane * 3];
    let mut peak = 0.0f32;
    let sample = |x: f32, y: f32, c: usize| -> f32 {
        // Bilinear, on the picture composited over white.
        let fx = (x - 0.5).clamp(0.0, (w - 1) as f32);
        let fy = (y - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let px = |x: usize, y: usize| -> f32 {
            let i = (y * w + x) * 4;
            let value = rgba.get(i + c).copied().unwrap_or(0) as f32;
            let alpha = rgba.get(i + 3).copied().unwrap_or(255) as f32 / 255.0;
            value * alpha + 255.0 * (1.0 - alpha)
        };
        let top = px(x0, y0) * (1.0 - tx) + px(x1, y0) * tx;
        let bottom = px(x0, y1) * (1.0 - tx) + px(x1, y1) * tx;
        top * (1.0 - ty) + bottom * ty
    };
    let (sx, sy) = (w as f32 / INPUT as f32, h as f32 / INPUT as f32);
    for y in 0..INPUT {
        for x in 0..INPUT {
            let (px, py) = ((x as f32 + 0.5) * sx, (y as f32 + 0.5) * sy);
            for c in 0..3 {
                let v = sample(px, py, c);
                peak = peak.max(v);
                if let Some(slot) = out.get_mut(c * plane + y * INPUT + x) {
                    *slot = v;
                }
            }
        }
    }
    let peak = peak.max(1e-6);
    for v in &mut out {
        *v = *v / peak - 0.5;
    }
    out
}

/// The model's map stretched to 0..=1.
fn normalise(raw: &[f32]) -> Vec<f32> {
    let (lo, hi) = raw.iter().filter(|v| v.is_finite()).fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    let span = hi - lo;
    if !(span.is_finite() && span > 1e-6) {
        return vec![0.0; raw.len()];
    }
    raw.iter().map(|v| if v.is_finite() { (v - lo) / span } else { 0.0 }).collect()
}

/// A plane of `sw` × `sh` scaled to `dw` × `dh`, bilinearly.
fn resize_plane(src: &[f32], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; dw * dh];
    let (sx, sy) = (sw as f32 / dw as f32, sh as f32 / dh as f32);
    let at = |x: usize, y: usize| src.get(y * sw + x).copied().unwrap_or(0.0);
    for y in 0..dh {
        let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (sh - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy - fy.floor());
        let y1 = (y0 + 1).min(sh - 1);
        for x in 0..dw {
            let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (sw - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx - fx.floor());
            let x1 = (x0 + 1).min(sw - 1);
            let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
            let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
            if let Some(slot) = out.get_mut(y * dw + x) {
                *slot = top * (1.0 - ty) + bottom * ty;
            }
        }
    }
    out
}

/// The picture's luminance, 0..=1.
fn luminance(rgba: &[u8], w: usize, h: usize) -> Vec<f32> {
    (0..w * h)
        .map(|i| {
            let p = rgba.get(i * 4..i * 4 + 3).unwrap_or(&[0, 0, 0]);
            (0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2])) / 255.0
        })
        .collect()
}

/// Mean of `v` over a `(2r+1)²` box at each pixel, from a summed-area table.
fn box_mean(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let stride = w + 1;
    let mut sat = vec![0.0f64; stride * (h + 1)];
    for y in 0..h {
        let mut row = 0.0f64;
        for x in 0..w {
            row += f64::from(v.get(y * w + x).copied().unwrap_or(0.0));
            let above = sat.get(y * stride + x + 1).copied().unwrap_or(0.0);
            if let Some(slot) = sat.get_mut((y + 1) * stride + x + 1) {
                *slot = above + row;
            }
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let s = |yy: usize, xx: usize| sat.get(yy * stride + xx).copied().unwrap_or(0.0);
            let sum = s(y1, x1) - s(y0, x1) - s(y1, x0) + s(y0, x0);
            let n = ((y1 - y0) * (x1 - x0)).max(1) as f64;
            if let Some(slot) = out.get_mut(y * w + x) {
                *slot = (sum / n) as f32;
            }
        }
    }
    out
}

/// The coarse mask `p` refined by a guided filter on the luminance `i`: edges follow the picture's, and areas the
/// model was sure of stay as they were.
fn refine(i: &[f32], p: &[f32], w: usize, h: usize) -> Vec<f32> {
    let r = (w.max(h) / 220).clamp(2, 12);
    let eps = 1e-3f32;
    let mean_i = box_mean(i, w, h, r);
    let mean_p = box_mean(p, w, h, r);
    let ip: Vec<f32> = i.iter().zip(p).map(|(a, b)| a * b).collect();
    let ii: Vec<f32> = i.iter().map(|a| a * a).collect();
    let corr_ip = box_mean(&ip, w, h, r);
    drop(ip);
    let corr_ii = box_mean(&ii, w, h, r);
    drop(ii);
    let mut a = vec![0.0f32; w * h];
    let mut b = vec![0.0f32; w * h];
    for k in 0..w * h {
        let (mi, mp) = (mean_i.get(k).copied().unwrap_or(0.0), mean_p.get(k).copied().unwrap_or(0.0));
        let var = corr_ii.get(k).copied().unwrap_or(0.0) - mi * mi;
        let cov = corr_ip.get(k).copied().unwrap_or(0.0) - mi * mp;
        let ak = cov / (var + eps);
        if let (Some(sa), Some(sb)) = (a.get_mut(k), b.get_mut(k)) {
            *sa = ak;
            *sb = mp - ak * mi;
        }
    }
    let mean_a = box_mean(&a, w, h, r);
    let mean_b = box_mean(&b, w, h, r);
    (0..w * h)
        .map(|k| {
            let guided = mean_a.get(k).copied().unwrap_or(0.0) * i.get(k).copied().unwrap_or(0.0) + mean_b.get(k).copied().unwrap_or(0.0);
            let coarse = p.get(k).copied().unwrap_or(0.0);
            // Where the model was sure (near 0 or 1) its answer stands; the guided edge decides the uncertain band.
            let sure = ((coarse - 0.5).abs() * 2.0).powi(2);
            (coarse * sure + guided * (1.0 - sure)).clamp(0.0, 1.0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_map_normalises_to_zero_and_never_divides_by_zero() {
        assert!(normalise(&[0.3; 16]).iter().all(|v| *v == 0.0));
        let n = normalise(&[0.0, 2.0, f32::NAN, 1.0]);
        assert_eq!(n, vec![0.0, 1.0, 0.0, 0.5]);
    }

    #[test]
    fn resizing_keeps_a_constant_plane_constant() {
        let out = resize_plane(&[0.25; 4 * 4], 4, 4, 7, 3);
        assert!(out.iter().all(|v| (v - 0.25).abs() < 1e-6));
    }

    #[test]
    fn refinement_keeps_a_sure_mask() {
        let (w, h) = (16, 16);
        let p: Vec<f32> = (0..w * h).map(|k| if k % w < 8 { 1.0 } else { 0.0 }).collect();
        let i: Vec<f32> = p.clone();
        let out = refine(&i, &p, w, h);
        assert!(out.iter().zip(&p).all(|(a, b)| (a - b).abs() < 0.05));
    }
}
