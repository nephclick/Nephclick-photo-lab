//! Depth from one photo, offline: the MiDaS v2.1 Small model (MIT) run in-process by `tract`. Light Pose turns its
//! relative depth into the scene's 3D shape, then relights that shape.
//!
//! The model sees the picture at 256 × 256 and returns relative inverse depth (larger is nearer); the map is scaled
//! back to the picture and normalised to 0..=1, nearest = 1. About 70 MB on disk, well under 1 GB of memory while
//! it runs. Installed from Select › AI Selection Models… (the Light Pose pack) or on Light Pose's first use.

use std::sync::{Arc, Mutex, PoisonError};

use tract_onnx::prelude::*;

use crate::AiError;
use crate::model_install::{DEPTH_MODEL_BYTES, depth_model_path};

/// The side the model takes its input at.
const INPUT: usize = 256;

type Runnable = Arc<TypedRunnableModel>;
static MODEL: Mutex<Option<Runnable>> = Mutex::new(None);

/// Frees the loaded model's memory.
pub fn unload() {
    *MODEL.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

fn tract_error(e: impl std::fmt::Display) -> AiError {
    AiError::Unsupported(format!("the depth model couldn't run: {e}"))
}

fn model() -> Result<Runnable, AiError> {
    let mut held = MODEL.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(model) = held.as_ref() {
        return Ok(Arc::clone(model));
    }
    let path = depth_model_path();
    let meta = std::fs::metadata(&path)
        .map_err(|_| AiError::NotConfigured("the Light Pose depth model isn't installed (Select › AI Selection Models…)".into()))?;
    if meta.len() != DEPTH_MODEL_BYTES {
        return Err(AiError::NotConfigured("the Light Pose depth model is incomplete: install it again (Select › AI Selection Models…)".into()));
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

/// The depth of `rgba` (`width` × `height`, RGBA8), `width × height` values in 0..=1, nearest = 1.
pub fn estimate(rgba: &[u8], width: u32, height: u32) -> Result<Vec<f32>, AiError> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || rgba.len() != w.saturating_mul(h).saturating_mul(4) {
        return Err(AiError::Invalid("invalid image dimensions or buffer size".into()));
    }
    let model = model()?;
    // ImageNet normalisation, channels apart, bilinear to 256².
    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const STD: [f32; 3] = [0.229, 0.224, 0.225];
    let plane = INPUT * INPUT;
    let mut input = vec![0.0f32; plane * 3];
    let (sx, sy) = (w as f32 / INPUT as f32, h as f32 / INPUT as f32);
    for y in 0..INPUT {
        let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (h - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy - fy.floor());
        let y1 = (y0 + 1).min(h - 1);
        for x in 0..INPUT {
            let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (w - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx - fx.floor());
            let x1 = (x0 + 1).min(w - 1);
            for c in 0..3 {
                let px = |xx: usize, yy: usize| f32::from(rgba.get((yy * w + xx) * 4 + c).copied().unwrap_or(0)) / 255.0;
                let v = (px(x0, y0) * (1.0 - tx) + px(x1, y0) * tx) * (1.0 - ty) + (px(x0, y1) * (1.0 - tx) + px(x1, y1) * tx) * ty;
                if let Some(slot) = input.get_mut(c * plane + y * INPUT + x) {
                    *slot = (v - MEAN[c]) / STD[c];
                }
            }
        }
    }
    let tensor: Tensor = tract_ndarray::Array4::from_shape_vec((1, 3, INPUT, INPUT), input).map_err(tract_error)?.into();
    let outputs = model.run(tvec!(tensor.into())).map_err(tract_error)?;
    let first = outputs.first().ok_or_else(|| AiError::BadReply("the depth model returned nothing".into()))?;
    let raw: Vec<f32> = first.to_plain_array_view::<f32>().map_err(tract_error)?.iter().copied().collect();
    let map = raw.get(..plane).ok_or_else(|| AiError::BadReply("the depth model returned a map of the wrong size".into()))?;
    let (lo, hi) = map.iter().filter(|v| v.is_finite()).fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    let span = (hi - lo).max(1e-6);
    // Back to the picture's size, bilinear.
    let mut out = vec![0.0f32; w * h];
    let (rx, ry) = (INPUT as f32 / w as f32, INPUT as f32 / h as f32);
    let at = |x: usize, y: usize| map.get(y * INPUT + x).copied().filter(|v| v.is_finite()).map_or(0.0, |v| (v - lo) / span);
    for y in 0..h {
        let fy = ((y as f32 + 0.5) * ry - 0.5).clamp(0.0, (INPUT - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy - fy.floor());
        let y1 = (y0 + 1).min(INPUT - 1);
        for x in 0..w {
            let fx = ((x as f32 + 0.5) * rx - 0.5).clamp(0.0, (INPUT - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx - fx.floor());
            let x1 = (x0 + 1).min(INPUT - 1);
            if let Some(slot) = out.get_mut(y * w + x) {
                *slot = (at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx) * (1.0 - ty) + (at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx) * ty;
            }
        }
    }
    Ok(out)
}
