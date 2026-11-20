//! Layer › New › Light Pose Layer…: relights a photo after it was taken by rebuilding its 3D shape.
//!
//! The offline depth model (`nephclick_photolab_ai::depth_model`, MiDaS v2.1 Small) estimates how near each part of
//! the picture is; that depth becomes a surface whose normals take the new light (direction, colour, intensity),
//! whose relief casts shadows away from it (ray-marched over the depth), and whose glancing angles catch highlights.
//! The result is a new layer above the picture, the whole image relit, so the original stays untouched and the layer
//! can be masked or faded. Without the depth model installed it estimates the shape from brightness instead, and says
//! so. The look is computed at most 2048 pixels on a side (shadows at 512) and applied to the full image in bands, so
//! large photos stay within an 8 GB machine's memory.

use nephclick_photolab_color::{BlendMode, ColorMode};
use nephclick_photolab_doc::Layer;
use nephclick_photolab_geom::Rect;
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

/// The command's id.
pub const ID: &str = "layer.new.lightPoseLayer";

/// Largest side the lighting is worked out at.
const WORK_SIDE: u32 = 2048;
/// Largest side shadows are ray-marched at.
const SHADOW_SIDE: usize = 512;
/// Rows of the full image relit at a time.
const BAND: i32 = 256;

fn has_rgb_doc(s: &Session) -> std::result::Result<(), String> {
    let d = s.active().ok_or("no document open")?;
    if d.doc.mode != ColorMode::Rgb {
        return Err("Light Pose works on RGB documents (Image › Mode › RGB Color)".into());
    }
    Ok(())
}

fn bad(msg: &str) -> EngineError {
    EngineError::BadParams { cmd: ID.to_owned(), msg: msg.to_owned() }
}

/// A number param in `lo..=hi`, `default` when missing; refused when not a finite number.
fn num(p: &Value, key: &str, lo: f32, hi: f32, default: f32) -> Result<f32> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => {
            let n = v.as_f64().filter(|n| n.is_finite()).ok_or_else(|| bad(&format!("{key} must be a number")))? as f32;
            Ok(n.clamp(lo, hi))
        }
    }
}

/// `#rrggbb` as linear-ish 0..=1 RGB.
fn color(p: &Value) -> Result<[f32; 3]> {
    let Some(v) = p.get("color").filter(|v| !v.is_null()) else { return Ok([1.0, 0.96, 0.9]) };
    let hex = v.as_str().ok_or_else(|| bad("color must be \"#rrggbb\""))?.trim().trim_start_matches('#');
    let channel = |i: usize| hex.get(i..i + 2).and_then(|s| u8::from_str_radix(s, 16).ok()).map(|b| f32::from(b) / 255.0);
    match (hex.len(), channel(0), channel(2), channel(4)) {
        (6, Some(r), Some(g), Some(b)) => Ok([r, g, b]),
        _ => Err(bad("color must be \"#rrggbb\"")),
    }
}

/// The light Light Pose casts.
struct Light {
    /// Unit direction toward the light: x right, y down, z toward the viewer.
    dir: [f32; 3],
    color: [f32; 3],
    intensity: f32,
    ambient: f32,
    specular: f32,
    shadows: f32,
    softness: f32,
    depth: f32,
}

/// Specs for Light Pose.
pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: ID,
        label: "Light Pose Layer…",
        menu: &["Layer", "New"],
        shortcut: None,
        params: r##"{"azimuth":0..360=135,"elevation":5..90=40,"intensity":0..3=1.2,"color":"#fff5e6","ambient":0..1=0.35,"specular":0..1=0.3,"shadows":0..1=0.7,"softness":0..10=3,"depth":0..3=1} (azimuth: where the light comes from, 0 from the right, 90 from the top; the offline depth model rebuilds the scene's 3D shape: install it in Select › AI Selection Models…, else the shape is estimated from brightness)"##,
        enabled: has_rgb_doc,
        run: light_pose,
        journal: true,
    }]
}

fn light_pose(s: &mut Session, p: &Value) -> Result<Value> {
    has_rgb_doc(s).map_err(EngineError::Other)?;
    let azimuth = num(p, "azimuth", 0.0, 360.0, 135.0)?.to_radians();
    let elevation = num(p, "elevation", 5.0, 90.0, 40.0)?.to_radians();
    let light = Light {
        dir: [elevation.cos() * azimuth.cos(), -elevation.cos() * azimuth.sin(), elevation.sin()],
        color: color(p)?,
        intensity: num(p, "intensity", 0.0, 3.0, 1.2)?,
        ambient: num(p, "ambient", 0.0, 1.0, 0.35)?,
        specular: num(p, "specular", 0.0, 1.0, 0.3)?,
        shadows: num(p, "shadows", 0.0, 1.0, 0.7)?,
        softness: num(p, "softness", 0.0, 10.0, 3.0)?,
        depth: num(p, "depth", 0.0, 3.0, 1.0)?,
    };
    let d = s.active().ok_or(EngineError::NoDocument)?;
    let doc = std::sync::Arc::clone(&d.doc);
    let bounds = doc.bounds();
    if bounds.is_empty() {
        return Err(EngineError::Other("the document is empty".into()));
    }
    // The lighting, worked out on a reduced picture.
    let small = nephclick_photolab_compose::thumbnail(&doc, WORK_SIDE);
    let (sw, sh) = (small.width.max(1) as usize, small.height.max(1) as usize);
    let (depth, note) = depth_of(&small.pixels, sw, sh);
    let (shade, spec) = shading(&depth, sw, sh, &light);
    drop(depth);
    // The full picture relit in bands of rows.
    let format = doc.pixel_format();
    let mut layer = Layer::raster("Light Pose", format);
    let surface = layer.surface_mut().ok_or_else(|| EngineError::Other("can't make the layer".into()))?;
    if surface.channels() != 4 {
        return Err(EngineError::Other("Light Pose works on RGB documents".into()));
    }
    let (fw, fh) = (bounds.width().max(1) as f32, bounds.height().max(1) as f32);
    let mut y0 = bounds.y0;
    while y0 < bounds.y1 {
        let y1 = (y0 + BAND).min(bounds.y1);
        let band = Rect::new(bounds.x0, y0, bounds.x1, y1);
        let px = nephclick_photolab_compose::render(&doc, band).into_px();
        let mut out = Vec::with_capacity(px.len() * 4);
        let bw = band.width().max(1) as usize;
        for (i, q) in px.iter().enumerate() {
            let (x, y) = ((i % bw) as f32, (y0 - bounds.y0) as f32 + (i / bw) as f32);
            let (u, v) = ((x + 0.5) / fw * sw as f32 - 0.5, (y + 0.5) / fh * sh as f32 - 0.5);
            let k = sample(&shade, sw, sh, u, v);
            let g = sample(&spec, sw, sh, u, v);
            for c in 0..3 {
                let lit = q[c] * (light.ambient + (1.0 - light.ambient) * k * light.color[c]) + g * light.color[c];
                out.push(if lit.is_finite() { lit.clamp(0.0, 1.0) } else { 0.0 });
            }
            out.push(q[3]);
        }
        surface.write_region(band, &out);
        y0 = y1;
    }
    surface.prune();
    let name = doc.next_layer_name("Light Pose");
    layer.name = name.clone();
    layer.blend = BlendMode::Normal;
    let id = s.edit("Light Pose Layer", |doc, active| {
        let id = crate::layer_place::insert_new(doc, *active, layer, crate::layer_place::Place::Above);
        *active = Some(id);
        Ok(id)
    })?;
    let mut out = json!({"layer": id.0, "name": name, "depth": if note.is_none() { "model" } else { "brightness" }});
    if let Some(note) = note {
        out["note"] = json!(note);
    }
    Ok(out)
}

/// The depth of the reduced picture (0..=1, nearest = 1): the depth model's, else one estimated from brightness with
/// a note saying why.
fn depth_of(rgba: &[u8], w: usize, h: usize) -> (Vec<f32>, Option<String>) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if nephclick_photolab_ai::model_install::depth_installed() {
            match nephclick_photolab_ai::depth_model::estimate(rgba, w as u32, h as u32) {
                Ok(depth) if depth.len() == w * h => return (depth, None),
                Ok(_) => {}
                Err(e) => return (brightness_depth(rgba, w, h), Some(format!("the depth model failed ({e}); the shape was estimated from brightness"))),
            }
        }
    }
    let note = "for true 3D lighting, install the Light Pose depth model (Select › AI Selection Models…); the shape was estimated from brightness";
    (brightness_depth(rgba, w, h), Some(note.into()))
}

/// A stand-in depth: brighter is nearer, smoothed.
fn brightness_depth(rgba: &[u8], w: usize, h: usize) -> Vec<f32> {
    let l: Vec<f32> = (0..w * h)
        .map(|i| {
            let p = rgba.get(i * 4..i * 4 + 3).unwrap_or(&[0, 0, 0]);
            (0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2])) / 255.0
        })
        .collect();
    blur(&l, w, h, (w.max(h) / 128).max(2))
}

/// Mean of `v` over a `(2r+1)²` box at each pixel (two passes).
fn blur(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    if r == 0 || w == 0 || h == 0 {
        return v.to_vec();
    }
    let pass = |src: &[f32], horizontal: bool| -> Vec<f32> {
        let mut out = vec![0.0f32; w * h];
        let (len, lines) = if horizontal { (w, h) } else { (h, w) };
        for line in 0..lines {
            let at = |i: usize| if horizontal { line * w + i } else { i * w + line };
            let mut sum = 0.0f32;
            let mut n = 0usize;
            for i in 0..=r.min(len - 1) {
                sum += src.get(at(i)).copied().unwrap_or(0.0);
                n += 1;
            }
            for i in 0..len {
                if let Some(slot) = out.get_mut(at(i)) {
                    *slot = sum / n.max(1) as f32;
                }
                if i + r + 1 < len {
                    sum += src.get(at(i + r + 1)).copied().unwrap_or(0.0);
                    n += 1;
                }
                if i >= r {
                    sum -= src.get(at(i - r)).copied().unwrap_or(0.0);
                    n -= 1;
                }
            }
        }
        out
    };
    pass(&pass(v, true), false)
}

/// Bilinear sample of `map` (`w` × `h`) at `(x, y)` in its pixels.
fn sample(map: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let fx = x.clamp(0.0, (w - 1) as f32);
    let fy = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let at = |x: usize, y: usize| map.get(y * w + x).copied().unwrap_or(0.0);
    (at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx) * (1.0 - ty) + (at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx) * ty
}

/// The light on the surface `depth` describes: per pixel, the diffuse light reaching it (shadowed, scaled so the
/// picture keeps its overall brightness) and the highlight it reflects.
fn shading(depth: &[f32], w: usize, h: usize, light: &Light) -> (Vec<f32>, Vec<f32>) {
    let softness = 1 + (light.softness * w.max(h) as f32 / 400.0) as usize;
    let depth = blur(depth, w, h, softness);
    // Relief: the scene's depth range as a share of the picture's width.
    let relief = light.depth * 0.25 * w as f32;
    let height = |x: usize, y: usize| depth.get(y.min(h - 1) * w + x.min(w - 1)).copied().unwrap_or(0.0) * relief;
    let shadow = shadows(&depth, w, h, relief, light);
    let half = {
        let v = [light.dir[0], light.dir[1], light.dir[2] + 1.0];
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
        [v[0] / n, v[1] / n, v[2] / n]
    };
    let mut diffuse = vec![0.0f32; w * h];
    let mut spec = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let dzdx = (height(x + 1, y) - height(x.saturating_sub(1), y)) * 0.5;
            let dzdy = (height(x, y + 1) - height(x, y.saturating_sub(1))) * 0.5;
            let n = [-dzdx, -dzdy, 1.0];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
            let n = [n[0] / len, n[1] / len, n[2] / len];
            let i = y * w + x;
            let sx = (x as f32 + 0.5) / w as f32 * shadow_w(w, h) as f32 - 0.5;
            let sy = (y as f32 + 0.5) / h as f32 * shadow_h(w, h) as f32 - 0.5;
            let occluded = sample(&shadow, shadow_w(w, h), shadow_h(w, h), sx, sy);
            let ndl = (n[0] * light.dir[0] + n[1] * light.dir[1] + n[2] * light.dir[2]).max(0.0);
            if let Some(slot) = diffuse.get_mut(i) {
                *slot = ndl * (1.0 - light.shadows * occluded);
            }
            let ndh = (n[0] * half[0] + n[1] * half[1] + n[2] * half[2]).max(0.0);
            if let Some(slot) = spec.get_mut(i) {
                *slot = ndh.powi(32) * light.specular * light.intensity * (1.0 - occluded);
            }
        }
    }
    // Keep the picture's brightness: the mean light reaching it becomes the intensity asked for.
    let mean = diffuse.iter().sum::<f32>() / (w * h).max(1) as f32;
    let gain = light.intensity / mean.max(0.15);
    for v in &mut diffuse {
        *v *= gain;
    }
    (diffuse, spec)
}

fn shadow_dims(w: usize, h: usize) -> (usize, usize) {
    let scale = (SHADOW_SIDE as f32 / w.max(h) as f32).min(1.0);
    (((w as f32 * scale).round() as usize).max(1), ((h as f32 * scale).round() as usize).max(1))
}

fn shadow_w(w: usize, h: usize) -> usize {
    shadow_dims(w, h).0
}

fn shadow_h(w: usize, h: usize) -> usize {
    shadow_dims(w, h).1
}

/// How much each point is shaded from the light by the relief between it and the light, 0..=1, at the shadow size:
/// a march over the depth toward the light, the ray rising at the light's elevation.
fn shadows(depth: &[f32], w: usize, h: usize, relief: f32, light: &Light) -> Vec<f32> {
    let (dw, dh) = shadow_dims(w, h);
    let scale = w as f32 / dw as f32;
    let small: Vec<f32> = (0..dw * dh).map(|i| sample(depth, w, h, ((i % dw) as f32 + 0.5) * scale - 0.5, ((i / dw) as f32 + 0.5) * scale - 0.5) * relief / scale).collect();
    let flat = (light.dir[0] * light.dir[0] + light.dir[1] * light.dir[1]).sqrt();
    if flat < 1e-3 {
        return vec![0.0; dw * dh];
    }
    let (sx, sy) = (light.dir[0] / flat, light.dir[1] / flat);
    let rise = light.dir[2] / flat;
    let steps = dw.max(dh) / 3;
    let soft = 1.0 + light.softness;
    let mut out = vec![0.0f32; dw * dh];
    for y in 0..dh {
        for x in 0..dw {
            let h0 = small.get(y * dw + x).copied().unwrap_or(0.0);
            let mut occ = 0.0f32;
            for t in 1..=steps {
                let t = t as f32;
                let (px, py) = (x as f32 + sx * t, y as f32 + sy * t);
                if px < 0.0 || py < 0.0 || px > (dw - 1) as f32 || py > (dh - 1) as f32 {
                    break;
                }
                let above = sample(&small, dw, dh, px, py) - (h0 + rise * t);
                if above > 0.0 {
                    occ = occ.max((above / soft).min(1.0));
                    if occ >= 1.0 {
                        break;
                    }
                }
            }
            if let Some(slot) = out.get_mut(y * dw + x) {
                *slot = occ;
            }
        }
    }
    blur(&out, dw, dh, 1 + light.softness as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_pose_adds_a_layer_and_refuses_bad_params() {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 48, "height": 32})).unwrap();
        let before = s.active().unwrap().doc.layers.len();
        let r = s.execute(ID, json!({"azimuth": 45, "elevation": 30})).unwrap();
        assert_eq!(s.active().unwrap().doc.layers.len(), before + 1);
        assert!(r.get("layer").is_some());
        for p in [json!({"azimuth": "left"}), json!({"color": "red"}), json!({"color": 3})] {
            assert!(s.execute(ID, p).is_err());
        }
    }

    #[test]
    fn a_flat_surface_lit_from_above_has_no_shadow() {
        let light = Light { dir: [0.0, 0.0, 1.0], color: [1.0; 3], intensity: 1.0, ambient: 0.3, specular: 0.0, shadows: 1.0, softness: 0.0, depth: 1.0 };
        let (shade, _) = shading(&[0.5; 64], 8, 8, &light);
        assert!(shade.iter().all(|v| (v - 1.0).abs() < 1e-3));
    }
}
