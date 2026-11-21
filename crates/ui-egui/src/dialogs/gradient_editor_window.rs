//! The Gradient Editor as a popup window, as Photoshop opens it from the Gradient tool's options bar: the same editor
//! as the Gradient Editor panel (the tool's gradient, or a selected gradient fill layer's), plus Randomize, over the
//! canvas with a Done button. The panel stays in Window › Gradient Editor for those who dock it.

use std::sync::atomic::{AtomicBool, Ordering};

use craft::color_ui::ui::gradient::GradientStops;

use crate::NephclickPhotoLabApp;

static OPEN: AtomicBool = AtomicBool::new(false);

/// Opens the window.
pub(crate) fn open(app: &mut NephclickPhotoLabApp) {
    OPEN.store(true, Ordering::Relaxed);
    app.ctx().request_repaint();
}

/// Draws the window while it is open.
pub(crate) fn show(app: &mut NephclickPhotoLabApp, ctx: &egui::Context) {
    if !OPEN.load(Ordering::Relaxed) {
        return;
    }
    let (mut open, mut done) = (true, false);
    egui::Window::new(tl!("Gradient Editor"))
        .id(egui::Id::unique("gradient-editor-window"))
        .open(&mut open)
        .collapsible(true)
        .resizable(true)
        .default_width(crate::theme::DIALOG_WIDTH_LG)
        .min_width(crate::theme::DIALOG_WIDTH_MD)
        .show(ctx, |ui| {
            crate::tools::gradient_ui::editor::show(app, ui);
            ui.add_space(crate::theme::SPACE_SM);
            // Randomize: new colour stops for the Gradient tool's gradient (a gradient fill layer edits its own rows).
            if crate::tools::gradient_ui::selected_gradient(app).is_none()
                && ui.button(tl!("Randomize")).on_hover_text(tl!("New random colour stops")).clicked()
            {
                let seed = ui.input(|i| i.time).to_bits() ^ app.frame;
                let stops = GradientStops::new(crate::gradient_map_ui::random_solid(seed));
                let id = nephclick_photolab_engine::presets::gradients::STOP;
                if let Err(e) = app.run(id, crate::tools::gradient_ui::stops::preset_params(&stops)) {
                    app.ui.status = e;
                    app.ui.status_error = true;
                }
            }
            ui.separator();
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
    if !open || done || crate::window_keys::take_escape(ctx) {
        OPEN.store(false, Ordering::Relaxed);
    }
}
