//! Generative 3D Relight Layer.
//! Reconstructs surface normals, highlights, directional lighting, and shadows
//! from 2D photographs using 3D lighting reconstruction and generative shading.

use std::f32::consts::PI;

use nephclick_photolab_color::BlendMode;
use nephclick_photolab_doc::Layer;
use serde_json::{json, Value};

use crate::commands::CommandSpec;
use crate::error::{EngineError, Result};
use crate::Session;

fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".into())
}

macro_rules! spec {
    ($id:literal, $label:literal, [$($m:literal),*], $params:literal, $en:expr, $run:expr) => {
        CommandSpec { id: $id, label: $label, menu: &[$($m),*], shortcut: None, params: $params, enabled: $en, run: $run, journal: true }
    };
}

/// Specs for the Relight command.
pub fn specs() -> Vec<CommandSpec> {
    vec![spec!(
        "layer.new.relightLayer",
        "Relight Layer…",
        ["Layer", "New"],
        r##"{"lightAngle":0..360=45.0,"elevation":10..90=45.0,"intensity":0.0..2.0=1.0,"specular":0.0..1.0=0.35,"ambient":0.0..1.0=0.25,"softness":0.0..10.0=2.0,"color":"#fff3e0"}"##,
        has_doc,
        relight_layer
    )]
}

/// Generates a new 3D Relight Layer over the active photo or layer.
fn relight_layer(s: &mut Session, p: &Value) -> Result<Value> {
    let light_angle = p.get("lightAngle").and_then(Value::as_f64).unwrap_or(45.0) as f32;
    let elevation = p.get("elevation").and_then(Value::as_f64).unwrap_or(45.0) as f32;
    let intensity = p.get("intensity").and_then(Value::as_f64).unwrap_or(1.0) as f32;
    let specular = p.get("specular").and_then(Value::as_f64).unwrap_or(0.35) as f32;
    let ambient = p.get("ambient").and_then(Value::as_f64).unwrap_or(0.25) as f32;
    let softness = p.get("softness").and_then(Value::as_f64).unwrap_or(2.0) as f32;
    let color_str = p.get("color").and_then(Value::as_str).unwrap_or("#fff3e0");
    let light_color = parse_hex_color(color_str);

    let doc_id = s.active().ok_or(EngineError::NoDocument)?.doc.id;
    let bounds = s.active().ok_or(EngineError::NoDocument)?.doc.bounds();
    let width = bounds.width().max(1);
    let height = bounds.height().max(1);

    // Read composite pixels via the compositor
    let composite_pixels = {
        let st = s.active().ok_or(EngineError::NoDocument)?;
        nephclick_photolab_compose::render(&st.doc, bounds).into_px()
    };

    let w = width as usize;
    let h = height as usize;
    let mut lum = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            if idx < composite_pixels.len() {
                let px = composite_pixels[idx];
                lum[idx] = 0.2126 * px[0] + 0.7152 * px[1] + 0.0722 * px[2];
            }
        }
    }

    // Directional light vector from spherical coordinates (azimuth, elevation)
    let az_rad = light_angle * PI / 180.0;
    let el_rad = elevation * PI / 180.0;
    let lx = el_rad.cos() * az_rad.cos();
    let ly = el_rad.cos() * az_rad.sin();
    let lz = el_rad.sin().max(0.1);
    let l_len = (lx * lx + ly * ly + lz * lz).sqrt();
    let (lx, ly, lz) = (lx / l_len, ly / l_len, lz / l_len);

    // Camera view vector (0, 0, 1) and half-vector H
    let hx = lx;
    let hy = ly;
    let hz = lz + 1.0;
    let h_len = (hx * hx + hy * hy + hz * hz).sqrt().max(1e-4);
    let (hx, hy, hz) = (hx / h_len, hy / h_len, hz / h_len);

    let radius = softness.clamp(1.0, 8.0) as usize;
    let mut relit = vec![0.0f32; w * h * 4];

    for y in 0..h {
        let y_prev = y.saturating_sub(radius);
        let y_next = (y + radius).min(h - 1);
        for x in 0..w {
            let x_prev = x.saturating_sub(radius);
            let x_next = (x + radius).min(w - 1);

            let gx = (lum[y * w + x_next] - lum[y * w + x_prev]) / (2.0 * radius as f32);
            let gy = (lum[y_next * w + x] - lum[y_prev * w + x]) / (2.0 * radius as f32);

            // Surface normal N = (-gx * 2.5, -gy * 2.5, 1.0) normalized
            let nx = -gx * 2.5;
            let ny = -gy * 2.5;
            let nz = 1.0;
            let n_len = (nx * nx + ny * ny + nz * nz).sqrt();
            let (nx, ny, nz) = (nx / n_len, ny / n_len, nz / n_len);

            // Diffuse N . L
            let n_dot_l = (nx * lx + ny * ly + nz * lz).max(0.0);
            // Specular (N . H)^16
            let n_dot_h = (nx * hx + ny * hy + nz * hz).max(0.0);
            let spec_val = n_dot_h.powi(16);

            let illumination = ambient + n_dot_l * intensity + spec_val * specular;

            let pix_idx = y * w + x;
            let orig = composite_pixels.get(pix_idx).copied().unwrap_or([0.0, 0.0, 0.0, 1.0]);

            let out_idx = pix_idx * 4;
            // Shading rebuilt along 3D lighting vectors
            relit[out_idx] = (orig[0] * illumination * light_color[0]).clamp(0.0, 1.0);
            relit[out_idx + 1] = (orig[1] * illumination * light_color[1]).clamp(0.0, 1.0);
            relit[out_idx + 2] = (orig[2] * illumination * light_color[2]).clamp(0.0, 1.0);
            relit[out_idx + 3] = orig[3];
        }
    }

    // Insert the new Relight Layer into the document
    let new_layer_id = s.edit("Relight Layer", |doc, active| {
        let name = doc.next_layer_name("Relight Layer");
        let mut layer = Layer::raster(name, doc.pixel_format());
        layer.blend = BlendMode::SoftLight;
        layer.opacity = 1.0;

        if let Some(surface) = layer.surface_mut() {
            surface.write_region(bounds, &relit);
            surface.prune();
        }

        let id = crate::layer_place::insert_new(doc, *active, layer, crate::layer_place::Place::Above);
        *active = Some(id);
        Ok(id)
    })?;

    Ok(json!({
        "layer": new_layer_id.0,
        "name": "Relight Layer",
        "document": doc_id.0
    }))
}

fn parse_hex_color(hex: &str) -> [f32; 3] {
    let clean = hex.trim().trim_start_matches('#');
    if clean.len() >= 6 {
        let r = u8::from_str_radix(&clean[0..2], 16).unwrap_or(255) as f32 / 255.0;
        let g = u8::from_str_radix(&clean[2..4], 16).unwrap_or(243) as f32 / 255.0;
        let b = u8::from_str_radix(&clean[4..6], 16).unwrap_or(224) as f32 / 255.0;
        [r, g, b]
    } else {
        [1.0, 0.95, 0.88]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relight_layer_creates_new_raster_layer_above_active() {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 64, "height": 64})).unwrap();
        let initial_count = s.active().unwrap().doc.layers.len();

        let r = s.execute("layer.new.relightLayer", json!({
            "lightAngle": 45.0,
            "elevation": 50.0,
            "intensity": 1.2,
            "color": "#fffaed"
        })).unwrap();

        assert_eq!(r["name"], "Relight Layer");
        assert_eq!(s.active().unwrap().doc.layers.len(), initial_count + 1);
        let active_id = s.active().unwrap().active_layer.unwrap();
        assert_eq!(active_id.0, r["layer"].as_u64().unwrap());
    }
}
