//! View › Split Before and After: compare the image as it is now with the image as it was opened.
//!
//! Three modes, picked in the bar Split View shows over the canvas:
//! - **Side by Side**: the opened image left of a vertical divider, the edited one right of it;
//! - **Stacked**: the opened image above a horizontal divider;
//! - **Toggle**: no divider; Before/After flips the whole image.
//!
//! The divider is dragged anywhere along its line, and zoom, pan and rotation keep working: the "before" image is drawn
//! as a quad in document space over the picture, under the canvas controls. "Before" is the document's opening
//! snapshot (History's first snapshot), shown at up to 4096 pixels on its long side.

use std::sync::{Arc, Mutex, PoisonError};

use craft::behavior::CanvasView;
use craft::behavior::kurbo::Point;
use nephclick_photolab_doc::Document;

use crate::NephclickPhotoLabApp;

/// How the two images share the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    SideBySide,
    Stacked,
    Toggle,
}

/// Split View's state.
#[derive(Clone, Copy, Debug)]
struct State {
    on: bool,
    mode: Mode,
    /// Where the divider stands, as a fraction of the document's width (Side by Side) or height (Stacked).
    at: f64,
    /// Toggle mode: the opened image shows.
    before: bool,
}

static STATE: Mutex<State> = Mutex::new(State { on: false, mode: Mode::SideBySide, at: 0.5, before: true });

/// The opened image's texture, by the snapshot document it was made from.
struct Before {
    doc: std::sync::Weak<Document>,
    texture: egui::TextureHandle,
}
static BEFORE: Mutex<Option<Before>> = Mutex::new(None);

fn state() -> State {
    *STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

fn edit(f: impl FnOnce(&mut State)) {
    f(&mut STATE.lock().unwrap_or_else(PoisonError::into_inner));
}

/// Whether Split View is on.
pub fn is_on() -> bool {
    state().on
}

/// View › Split View: turns it on or off; `mode` ("sideBySide", "stacked", "toggle") and `at` (0..1) set it.
pub fn command(app: &mut NephclickPhotoLabApp, params: &serde_json::Value) -> Result<serde_json::Value, String> {
    if app.session.active().is_none() {
        return Err("Split View needs an open document".into());
    }
    let mode = match params.get("mode").and_then(serde_json::Value::as_str) {
        None => None,
        Some("sideBySide") => Some(Mode::SideBySide),
        Some("stacked") => Some(Mode::Stacked),
        Some("toggle") => Some(Mode::Toggle),
        Some(other) => return Err(format!("unknown Split View mode \"{other}\": sideBySide, stacked or toggle")),
    };
    let at = params.get("at").and_then(serde_json::Value::as_f64).filter(|v| v.is_finite());
    let on = params.get("on").and_then(serde_json::Value::as_bool);
    edit(|s| {
        s.on = on.unwrap_or(if mode.is_some() || at.is_some() { true } else { !s.on });
        if let Some(mode) = mode {
            s.mode = mode;
        }
        if let Some(at) = at {
            s.at = at.clamp(0.0, 1.0);
        }
    });
    app.ctx().request_repaint();
    let s = state();
    Ok(serde_json::json!({"on": s.on, "mode": match s.mode { Mode::SideBySide => "sideBySide", Mode::Stacked => "stacked", Mode::Toggle => "toggle" }, "at": s.at}))
}

/// The document's opening snapshot: History's first snapshot, else the oldest state History still holds.
fn opened(app: &NephclickPhotoLabApp, idx: usize) -> Option<Arc<Document>> {
    let st = app.session.documents().get(idx)?;
    st.snapshots.iter().find(|s| s.id == 0).or_else(|| st.snapshots.first()).map(|s| Arc::clone(&s.doc))
}

/// The opened image's texture, made once per snapshot.
fn texture(ctx: &egui::Context, doc: &Arc<Document>) -> egui::TextureHandle {
    let mut held = BEFORE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(before) = held.as_ref()
        && before.doc.upgrade().is_some_and(|d| Arc::ptr_eq(&d, doc))
    {
        return before.texture.clone();
    }
    let side = (ctx.input(|i| i.max_texture_side) as u32).clamp(256, 4096);
    let image = nephclick_photolab_compose::thumbnail(doc, side);
    let color = egui::ColorImage::from_rgba_unmultiplied([image.width as usize, image.height as usize], &image.pixels);
    let texture = ctx.load_texture("split-view-before", color, egui::TextureOptions::LINEAR);
    *held = Some(Before { doc: Arc::downgrade(doc), texture: texture.clone() });
    texture
}

/// The shapes Split View paints over the picture of document `idx` in `view`: the opened image over its part, and the
/// divider.
pub fn shapes(app: &NephclickPhotoLabApp, ctx: &egui::Context, view: &CanvasView, idx: usize) -> Vec<egui::Shape> {
    let s = state();
    if !s.on || app.session.active_index() != Some(idx) {
        return Vec::new();
    }
    let Some(before) = opened(app, idx) else { return Vec::new() };
    let Some(current) = app.session.documents().get(idx) else { return Vec::new() };
    let (w, h) = (f64::from(current.doc.size.width), f64::from(current.doc.size.height));
    if w <= 0.0 || h <= 0.0 {
        return Vec::new();
    }
    // The part of the document the opened image covers, in document units and texture coordinates.
    let (x1, y1) = match s.mode {
        Mode::SideBySide => (w * s.at, h),
        Mode::Stacked => (w, h * s.at),
        Mode::Toggle if s.before => (w, h),
        Mode::Toggle => return Vec::new(),
    };
    let (u1, v1) = ((x1 / w) as f32, (y1 / h) as f32);
    let texture = texture(ctx, &before);
    let corners = [Point::new(0.0, 0.0), Point::new(x1, 0.0), Point::new(x1, y1), Point::new(0.0, y1)].map(|p| view.to_screen(p));
    let uvs = [egui::pos2(0.0, 0.0), egui::pos2(u1, 0.0), egui::pos2(u1, v1), egui::pos2(0.0, v1)];
    let mut mesh = egui::Mesh::with_texture(texture.id());
    for (pos, uv) in corners.iter().zip(uvs) {
        mesh.vertices.push(egui::epaint::Vertex { pos: *pos, uv, color: egui::Color32::WHITE });
    }
    mesh.indices.extend([0, 1, 2, 0, 2, 3]);
    let mut out = vec![egui::Shape::mesh(mesh)];
    let visuals = ctx.global_style().visuals.clone();
    if s.mode != Mode::Toggle {
        let (a, b) = match s.mode {
            Mode::SideBySide => (Point::new(x1, 0.0), Point::new(x1, h)),
            _ => (Point::new(0.0, y1), Point::new(w, y1)),
        };
        let (a, b) = (view.to_screen(a), view.to_screen(b));
        out.push(egui::Shape::line_segment([a, b], egui::Stroke::new(3.0, visuals.extreme_bg_color)));
        out.push(egui::Shape::line_segment([a, b], egui::Stroke::new(1.5, visuals.selection.stroke.color)));
        let mid = a + (b - a) * 0.5;
        out.push(egui::Shape::circle_filled(mid, 9.0, visuals.selection.bg_fill));
        out.push(egui::Shape::circle_stroke(mid, 9.0, egui::Stroke::new(1.5, visuals.extreme_bg_color)));
    }
    out
}

/// The divider's drag strip and Split View's bar, over the canvas of document `idx` in `rect`.
pub fn controls(app: &mut NephclickPhotoLabApp, ctx: &egui::Context, view: &CanvasView, rect: egui::Rect, idx: usize) {
    let s = state();
    if !s.on || app.session.active_index() != Some(idx) {
        return;
    }
    let Some(current) = app.session.documents().get(idx) else { return };
    let (w, h) = (f64::from(current.doc.size.width), f64::from(current.doc.size.height));
    // The drag strip: a band along the divider, on a layer above the canvas so tools never take its press.
    if s.mode != Mode::Toggle && w > 0.0 && h > 0.0 {
        let (a, b) = match s.mode {
            Mode::SideBySide => (Point::new(w * s.at, 0.0), Point::new(w * s.at, h)),
            _ => (Point::new(0.0, h * s.at), Point::new(w, h * s.at)),
        };
        let (a, b) = (view.to_screen(a), view.to_screen(b));
        let band = egui::Rect::from_two_pos(a, b).expand(8.0).intersect(rect);
        if band.is_positive() {
            egui::Area::new(egui::Id::unique(("split-view-divider", idx)))
                .order(egui::Order::Foreground)
                .fixed_pos(band.min)
                .show(ctx, |ui| {
                    let response = ui.allocate_response(band.size(), egui::Sense::drag());
                    let icon = if s.mode == Mode::SideBySide { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::ResizeVertical };
                    if response.hovered() || response.dragged() {
                        ui.ctx().set_cursor_icon(icon);
                    }
                    if response.dragged()
                        && let Some(pointer) = response.interact_pointer_pos()
                    {
                        let doc = view.to_doc(pointer);
                        let at = match s.mode {
                            Mode::SideBySide => doc.x / w,
                            _ => doc.y / h,
                        };
                        if at.is_finite() {
                            edit(|s| s.at = at.clamp(0.0, 1.0));
                        }
                    }
                });
        }
    }
    // The bar: the three modes, Before/After in Toggle, and Done.
    egui::Area::new(egui::Id::unique(("split-view-bar", idx)))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(rect.center().x - 210.0, rect.top() + 10.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(tl!("Before and After"));
                    ui.separator();
                    let mut mode = s.mode;
                    ui.selectable_value(&mut mode, Mode::SideBySide, tl!("Left/Right"));
                    ui.selectable_value(&mut mode, Mode::Stacked, tl!("Top/Bottom"));
                    ui.selectable_value(&mut mode, Mode::Toggle, tl!("Single"));
                    if mode != s.mode {
                        edit(|s| s.mode = mode);
                    }
                    if mode == Mode::Toggle {
                        ui.separator();
                        let label = if s.before { tl!("Showing: Before") } else { tl!("Showing: After") };
                        if ui.button(label).clicked() {
                            edit(|s| s.before = !s.before);
                        }
                    }
                    ui.separator();
                    if ui.button(tl!("Done")).clicked() {
                        edit(|s| s.on = false);
                    }
                });
            });
        });
}

#[cfg(test)]
mod enabled_tests {
    use serde_json::json;

    #[test]
    fn split_view_is_enabled_with_a_document() {
        let mut app = crate::NephclickPhotoLabApp::new(nephclick_photolab_engine::Session::new(), crate::Services::default());
        assert!(!crate::menus::is_enabled(&app, "view.splitView"), "greyed without a document");
        app.run("file.new", json!({"width": 40, "height": 30})).unwrap();
        assert!(crate::menus::is_enabled(&app, "view.splitView"), "enabled with a document");
    }
}
