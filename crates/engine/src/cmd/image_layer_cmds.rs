//! Layer › New › Layer from Image (`layer.new.fromImage`): an image given as bytes (base64 PNG, JPEG or WebP)
//! becomes a new pixel layer over a rectangle of the canvas, fitted to it, optionally masked to the selection.
//!
//! AI Fill places what the image model painted with it: the area it sent (the selection's bounds with some
//! surroundings) comes back as an image, which lands on the same rectangle, masked to the selection, in one
//! history step named after the fill. Agents can place any generated or fetched image the same way.
//!
//! The image's samples are taken as the document's own RGB values, as a pasted image's are: AI Fill sends the
//! area as the document holds it, so what comes back blends with what is there.

use base64::Engine as _;
use nephclick_photolab_color::{ColorMode, PixelFormat};
use nephclick_photolab_doc::{Layer, LayerMask};
use nephclick_photolab_geom::Rect;
use nephclick_photolab_raster::Surface;
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

/// The command id.
pub const ID: &str = "layer.new.fromImage";

/// Largest image accepted, in bytes before base64.
pub const MAX_IMAGE_BYTES: usize = 48 * 1024 * 1024;
/// Largest target rectangle side, and area, in pixels.
pub const MAX_SIDE: u32 = 30_000;
pub const MAX_AREA: u64 = 1 << 27;

fn bad(msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: ID.into(), msg: msg.into() }
}

fn enabled(s: &Session) -> std::result::Result<(), String> {
    let d = s.active().ok_or("no document open")?;
    match d.doc.mode {
        ColorMode::Rgb => Ok(()),
        m => Err(format!("Layer from Image needs an RGB document (this one is {m:?})")),
    }
}

/// The target rectangle: `rect` as `[x, y, width, height]`, else the canvas.
fn target(p: &Value, canvas: Rect) -> Result<Rect> {
    let Some(r) = p.get("rect") else { return Ok(canvas) };
    let a = r.as_array().filter(|a| a.len() == 4).ok_or_else(|| bad("`rect` must be [x, y, width, height]"))?;
    let n: Vec<f64> = a.iter().map(|v| v.as_f64().filter(|x| x.is_finite())).collect::<Option<_>>().ok_or_else(|| bad("`rect` holds numbers"))?;
    let (x, y, w, h) = (n[0], n[1], n[2], n[3]);
    if !(1.0..=MAX_SIDE as f64).contains(&w) || !(1.0..=MAX_SIDE as f64).contains(&h) {
        return Err(bad(format!("`rect` width and height must be 1..{MAX_SIDE}")));
    }
    if x.abs() > 1.0e7 || y.abs() > 1.0e7 {
        return Err(bad("`rect` is too far from the canvas"));
    }
    let (w, h) = (w.round() as u32, h.round() as u32);
    if u64::from(w) * u64::from(h) > MAX_AREA {
        return Err(bad("`rect` is too large"));
    }
    let (x, y) = (x.round() as i32, y.round() as i32);
    Ok(Rect::new(x, y, x.saturating_add(w as i32), y.saturating_add(h as i32)))
}

/// The file name `io::import` reads `bytes` by, from their signature.
fn sniffed_name(bytes: &[u8]) -> Result<&'static str> {
    if bytes.starts_with(b"\x89PNG") {
        Ok("image.png")
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        Ok("image.jpg")
    } else if bytes.len() >= 12 && bytes.get(0..4) == Some(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Ok("image.webp")
    } else {
        Err(bad("`image` must be a PNG, JPEG or WebP image"))
    }
}

/// The image's pixels as RGBA8 rows, `(width, height, pixels)`.
fn decode(bytes: &[u8]) -> Result<(u32, u32, Vec<[u8; 4]>)> {
    let name = sniffed_name(bytes)?;
    let imported = nephclick_photolab_io::import(name, bytes).map_err(|e| bad(format!("the image could not be read: {e}")))?;
    let doc = imported.document;
    let r = doc.bounds();
    let (w, h) = (r.width(), r.height());
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || u64::from(w) * u64::from(h) > MAX_AREA {
        return Err(bad("the image is empty or too large"));
    }
    let flat = nephclick_photolab_compose::flatten_to_surface(&doc, PixelFormat::RGBA8, None);
    let mut px = vec![[0u8; 4]; (w as usize) * (h as usize)];
    flat.read_rgba8_into(r, &mut px);
    Ok((w, h, px))
}

/// `src` (`sw` × `sh`) resampled bilinearly to `dw` × `dh`, as interleaved RGBA8.
fn resample(src: &[[u8; 4]], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((dw as usize) * (dh as usize) * 4);
    let at = |x: u32, y: u32| src.get((y.min(sh - 1) as usize) * sw as usize + x.min(sw - 1) as usize).copied().unwrap_or([0; 4]);
    let (fx, fy) = (sw as f32 / dw as f32, sh as f32 / dh as f32);
    for y in 0..dh {
        let sy = ((y as f32 + 0.5) * fy - 0.5).max(0.0);
        let (y0, ty) = (sy.floor() as u32, sy.fract());
        for x in 0..dw {
            let sx = ((x as f32 + 0.5) * fx - 0.5).max(0.0);
            let (x0, tx) = (sx.floor() as u32, sx.fract());
            let (a, b, c, d) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
            for i in 0..4 {
                let top = f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * tx;
                let bottom = f32::from(c[i]) + (f32::from(d[i]) - f32::from(c[i])) * tx;
                out.push((top + (bottom - top) * ty).round().clamp(0.0, 255.0) as u8);
            }
        }
    }
    out
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    if !p.is_object() {
        return Err(bad("params must be a JSON object"));
    }
    enabled(s).map_err(EngineError::Other)?;
    let b64 = p.get("image").and_then(Value::as_str).ok_or_else(|| bad("`image` (base64 PNG, JPEG or WebP) is required"))?;
    if b64.len() > MAX_IMAGE_BYTES / 3 * 4 + 4 {
        return Err(bad("`image` is too large"));
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|e| bad(format!("`image` is not base64: {e}")))?;
    let name = match p.get("name") {
        None | Some(Value::Null) => None,
        Some(Value::String(n)) if n.chars().count() <= 255 => Some(n.trim().to_string()).filter(|n| !n.is_empty()),
        Some(_) => return Err(bad("`name` must be text of at most 255 characters")),
    };
    let mask_to_selection = match p.get("maskToSelection") {
        None => false,
        Some(v) => v.as_bool().ok_or_else(|| bad("`maskToSelection` must be true or false"))?,
    };
    let d = s.active().ok_or(EngineError::NoDocument)?;
    let rect = target(p, d.doc.bounds())?;
    let selection = if mask_to_selection { Some(d.doc.selection.clone().ok_or_else(|| bad("`maskToSelection` needs a selection"))?) } else { None };
    let fmt = PixelFormat::new(d.doc.mode, d.doc.depth, true);
    let (sw, sh, src) = decode(&bytes)?;
    let (dw, dh) = (rect.width() as u32, rect.height() as u32);
    let rgba = resample(&src, sw, sh, dw, dh);
    let surface = Surface::from_interleaved(PixelFormat::RGBA8, rect, &rgba).convert(fmt);
    let label = name.clone().unwrap_or_else(|| "Layer from Image".to_string());
    let nid = s.edit(&label, |doc, active| {
        let layer_name = name.clone().unwrap_or_else(|| doc.next_layer_name("Layer"));
        let mut l = Layer::raster(layer_name, fmt);
        *crate::pixels_mut(&mut l)? = surface;
        if let Some(sel) = selection {
            // The selection becomes the mask and is dropped, as Reveal Selection does: one step.
            l.mask = Some(LayerMask { surface: sel, ..LayerMask::reveal_all() });
            doc.selection = None;
        }
        let nid = crate::layer_place::insert_new(doc, *active, l, crate::layer_place::Place::Above);
        *active = Some(nid);
        Ok(nid)
    })?;
    Ok(json!({"layer": nid.0, "rect": [rect.x0, rect.y0, dw, dh]}))
}

/// The command spec.
pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: ID,
        label: "Layer from Image",
        menu: &[],
        shortcut: None,
        params: r##"{"image":base64 PNG|JPEG|WebP,"rect":[x,y,width,height]?=canvas (the image is fitted to it),"name":str?,"maskToSelection":bool=false (the selection becomes the layer's mask and is dropped)} → {layer,rect}"##,
        enabled,
        run,
        journal: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w` × `h` PNG of one colour.
    fn png(w: u32, h: u32, rgba: [u8; 4]) -> String {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": w, "height": h, "background": "transparent"})).unwrap();
        let hex = format!("#{:02x}{:02x}{:02x}", rgba[0], rgba[1], rgba[2]);
        s.execute("edit.fill", json!({"contents": "color", "color": hex})).unwrap();
        let doc = s.active().unwrap().doc.clone();
        let out = nephclick_photolab_io::export(&doc, "x.png", &Default::default()).unwrap();
        base64::engine::general_purpose::STANDARD.encode(out.bytes)
    }

    fn session() -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 40, "height": 30})).unwrap();
        s
    }

    #[test]
    fn places_the_image_over_the_rect() {
        let mut s = session();
        let r = s.execute(ID, json!({"image": png(8, 8, [255, 0, 0, 255]), "rect": [10, 5, 16, 12], "name": "AI Fill: red"})).unwrap();
        assert_eq!(r["rect"], json!([10, 5, 16, 12]));
        let d = s.active().unwrap();
        let l = d.doc.layer(nephclick_photolab_doc::LayerId(r["layer"].as_u64().unwrap())).unwrap();
        assert_eq!(l.name, "AI Fill: red");
        let surf = l.surface().unwrap();
        let mut px = [[0u8; 4]; 1];
        surf.read_rgba8_into(Rect::new(12, 7, 13, 8), &mut px);
        assert_eq!(px[0], [255, 0, 0, 255]);
        surf.read_rgba8_into(Rect::new(2, 2, 3, 3), &mut px);
        assert_eq!(px[0][3], 0, "outside the rect stays clear");
        assert_eq!(d.history.undo_label(), Some("AI Fill: red"));
    }

    #[test]
    fn masks_to_the_selection_and_drops_it() {
        let mut s = session();
        s.execute("select.rect", json!({"x": 5, "y": 5, "width": 15, "height": 15})).unwrap();
        let r = s.execute(ID, json!({"image": png(4, 4, [0, 255, 0, 255]), "rect": [0, 0, 30, 30], "maskToSelection": true})).unwrap();
        let d = s.active().unwrap();
        let l = d.doc.layer(nephclick_photolab_doc::LayerId(r["layer"].as_u64().unwrap())).unwrap();
        assert!(l.mask.is_some());
        assert!(d.doc.selection.is_none());
    }

    #[test]
    fn bad_params_are_errors_not_panics() {
        let mut s = session();
        let good = png(2, 2, [0, 0, 255, 255]);
        for p in [
            json!(null),
            json!([]),
            json!({}),
            json!({"image": 5}),
            json!({"image": "%%%"}),
            json!({"image": "aGVsbG8="}),
            json!({"image": good, "rect": [0, 0, 0, 5]}),
            json!({"image": good, "rect": [0, 0, 1e9, 5]}),
            json!({"image": good, "rect": [0, 0, "a", 5]}),
            json!({"image": good, "rect": [1e300, 0, 5, 5]}),
            json!({"image": good, "rect": [0, 0, 30000, 30000]}),
            json!({"image": good, "maskToSelection": true}),
            json!({"image": good, "maskToSelection": "yes"}),
            json!({"image": good, "name": 7}),
        ] {
            assert!(s.execute(ID, p.clone()).is_err(), "{p}");
        }
        assert!(Session::new().execute(ID, json!({"image": good})).is_err(), "no document");
    }

    #[test]
    fn resample_keeps_a_flat_colour_and_handles_tiny_sizes() {
        let src = vec![[9u8, 8, 7, 255]; 4];
        let out = resample(&src, 2, 2, 5, 3);
        assert_eq!(out.len(), 5 * 3 * 4);
        assert!(out.chunks(4).all(|c| c == [9, 8, 7, 255]));
        assert_eq!(resample(&[[1, 2, 3, 4]], 1, 1, 1, 1), vec![1, 2, 3, 4]);
    }
}
