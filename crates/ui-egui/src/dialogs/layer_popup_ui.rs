//! Adjustment and fill layers edited in a floating window, as Photoshop's adjustment dialogs pop up: double-clicking
//! the layer's thumbnail, or making a new one, opens its controls (Curves, Levels, Hue/Saturation, a gradient or
//! pattern fill…) in a window over the canvas, with the same live preview, History steps and Reset as the Properties
//! panel, which notes the layer is open here. Gradient Map layers have their own Gradient Editor
//! (`gradient_map_ui`). Preferences › Interface › "Edit adjustments in a window" (`adjustments_in_window`) turns it off.

use std::cell::Cell;
use std::sync::{Mutex, PoisonError};

use nephclick_photolab_doc::{Adjustment, DocId, LayerContent, LayerId};

use crate::NephclickPhotoLabApp;

static OPEN: Mutex<Option<(DocId, LayerId)>> = Mutex::new(None);

thread_local! {
    /// Set while the window draws the layer's controls, so the shared Properties body knows it is the window's.
    static DRAWING: Cell<bool> = const { Cell::new(false) };
}

/// Whether the window shows layer `id`.
pub(crate) fn shows(id: LayerId) -> bool {
    OPEN.lock().unwrap_or_else(PoisonError::into_inner).is_some_and(|(_, l)| l == id)
}

/// Whether the window is drawing its layer's controls this moment.
pub(crate) fn drawing() -> bool {
    DRAWING.with(Cell::get)
}

/// Closes the window.
pub(crate) fn close() {
    *OPEN.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

/// Whether layer content edits in the window: adjustments (Gradient Map has its own editor) and fills.
fn edits_in_window(content: &LayerContent) -> bool {
    match content {
        LayerContent::Adjustment(Adjustment::GradientMap { .. } | Adjustment::Unsupported { .. }) => false,
        LayerContent::Adjustment(_) | LayerContent::Fill(_) => true,
        _ => false,
    }
}

/// Opens the window on the active layer when it is an adjustment or fill layer and the preference is on. `false`
/// otherwise, so the caller goes on as before (the Properties panel).
pub(crate) fn open(app: &mut NephclickPhotoLabApp) -> bool {
    if !app.ui.adjustments_in_window {
        return false;
    }
    let Some(st) = app.session.active() else { return false };
    let Some(id) = st.active_layer else { return false };
    if !st.doc.layer(id).is_some_and(|l| edits_in_window(&l.content)) {
        return false;
    }
    *OPEN.lock().unwrap_or_else(PoisonError::into_inner) = Some((st.doc.id, id));
    app.ctx().request_repaint();
    true
}

/// Draws the window while it is open.
pub(crate) fn show(app: &mut NephclickPhotoLabApp, ctx: &egui::Context) {
    let Some((doc, id)) = *OPEN.lock().unwrap_or_else(PoisonError::into_inner) else { return };
    let layer = app.session.active().filter(|st| st.doc.id == doc).and_then(|st| st.doc.layer(id).cloned());
    // The layer went (another document, deleted, undone): nothing to edit.
    let Some(layer) = layer.filter(|l| edits_in_window(&l.content)) else {
        close();
        return;
    };
    let mut open = true;
    let mut done = false;
    let title = crate::panels::properties::kind_label(&layer);
    egui::Window::new(title)
        .id(egui::Id::unique("layer-popup-editor"))
        .open(&mut open)
        .collapsible(true)
        .resizable(true)
        .default_width(crate::theme::DIALOG_WIDTH_SM)
        .min_width(crate::theme::DIALOG_WIDTH_SM - 40.0)
        .max_height(ctx.content_rect().height() * 0.85)
        .show(ctx, |ui| {
            ui.weak(layer.name.as_str());
            ui.add_space(4.0);
            egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
                DRAWING.with(|d| d.set(true));
                crate::panels::properties::layer_body(app, ui, &layer);
                DRAWING.with(|d| d.set(false));
            });
            ui.separator();
            ui.horizontal(|ui| {
                let mut keep = app.ui.adjustments_in_window;
                if ui.checkbox(&mut keep, tl!("Edit adjustments in a window")).changed() {
                    app.ui.adjustments_in_window = keep;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let fill = ui.visuals().hyperlink_color;
                    let button = egui::Button::new(egui::RichText::new(tl!("Done")).color(egui::Color32::WHITE))
                        .fill(fill)
                        .min_size(egui::vec2(88.0, crate::theme::CONTROL_HEIGHT));
                    if ui.add(button).clicked() {
                        done = true;
                    }
                });
            });
        });
    if !open || done {
        close();
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_curves_layer_opens_in_a_window_and_a_pixel_layer_does_not() {
        let mut app = NephclickPhotoLabApp::new(nephclick_photolab_engine::Session::new(), crate::Services::default());
        app.run("file.new", json!({"width": 16, "height": 16})).unwrap();
        app.run("layer.new.layer", json!({})).unwrap();
        assert!(!open(&mut app), "a pixel layer stays in Properties");
        app.run("layer.newAdjustmentLayer.curves", json!({})).unwrap();
        assert!(open(&mut app), "a Curves layer opens in the window");
        let id = app.session.active().and_then(|st| st.active_layer).unwrap();
        assert!(shows(id));
        let mut h = egui_kittest::Harness::builder().with_context(app.ctx().clone()).with_size(egui::vec2(1000.0, 800.0)).build_ui_state(
            |ui: &mut egui::Ui, app: &mut Box<NephclickPhotoLabApp>| {
                let ctx = ui.ctx().clone();
                show(app, &ctx);
            },
            app,
        );
        h.run_steps(3);
        assert!(shows(id), "still open after drawing");
        close();
        h.state_mut().ui.adjustments_in_window = false;
        assert!(!open(h.state_mut()), "the preference keeps it in Properties");
    }
}
