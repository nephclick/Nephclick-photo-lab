//! Select › AI Selection Models…: a small window to download, check and remove the offline AI selection models.
//!
//! Two cards, one per pack: the Select Subject model (in-process, about 170 MB) and the Prompt & Click models (a
//! private Python with SAM 2.1 Tiny and Grounding DINO Tiny, about 2 GB, run only while used). Every button runs a
//! `select.aiModels.*` command; the window polls `select.aiModels` while an install runs to show its progress.

use serde_json::{Value, json};

use crate::NephclickPhotoLabApp;

/// How often the window asks for the install's progress while one runs.
const POLL: std::time::Duration = std::time::Duration::from_millis(400);

/// Opens the window (Select › AI Selection Models…).
pub fn open(app: &mut NephclickPhotoLabApp) {
    app.ai.models_window = true;
    app.ai.models_status = None;
}

/// The status `select.aiModels` answers, read again when it is older than [`POLL`].
fn status(app: &mut NephclickPhotoLabApp, ctx: &egui::Context) -> Value {
    let now = ctx.input(|i| i.time);
    let fresh = app.ai.models_status.as_ref().is_some_and(|(at, _)| now - at < POLL.as_secs_f64());
    if !fresh {
        let value = app.run("select.aiModels", json!({"query": true})).unwrap_or(Value::Null);
        app.ai.models_status = Some((now, value));
    }
    app.ai.models_status.as_ref().map(|(_, v)| v.clone()).unwrap_or(Value::Null)
}

/// Runs an install, cancel or remove, and reads the status again on the next pass.
fn act(app: &mut NephclickPhotoLabApp, id: &str, params: Value) {
    if let Err(e) = app.run(id, params) {
        app.ui.status = e;
        app.ui.status_error = true;
    }
    app.ai.models_status = None;
}

/// Draws the window while it is open.
pub fn show(app: &mut NephclickPhotoLabApp, ctx: &egui::Context) {
    if !app.ai.models_window {
        return;
    }
    let status = status(app, ctx);
    let progress = status.get("progress").cloned().unwrap_or(Value::Null);
    let running = progress.get("running").and_then(Value::as_str).map(str::to_owned);
    if running.is_some() {
        ctx.request_repaint_after(POLL);
    }
    let installed = |pack: &str| status.get(pack).and_then(|p| p.get("installed")).and_then(Value::as_bool).unwrap_or(false);
    let (subject_ready, prompt_ready, depth_ready) = (installed("subject"), installed("prompt"), installed("depth"));
    let mut open = true;
    let mut action: Option<(&'static str, Value)> = None;
    egui::Window::new(tl!("AI Selection Models"))
        .id(egui::Id::unique("ai-selection-models"))
        .open(&mut open)
        .collapsible(true)
        .resizable(true)
        .default_width(560.0)
        .min_width(crate::theme::DIALOG_WIDTH_MD)
        .max_height(ctx.content_rect().height() * 0.85)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
                let visuals = ui.visuals().clone();
                ui.label(tl!("Selection runs on this computer. After a one-time download no internet connection is needed."));
                ui.add_space(8.0);
                let card = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
                    egui::Frame::new()
                        .fill(visuals.faint_bg_color)
                        .stroke(visuals.widgets.noninteractive.bg_stroke)
                        .corner_radius(8.0)
                        .inner_margin(12.0)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            add(ui);
                        });
                };
                let busy = running.is_some();
                card(ui, &mut |ui| {
                    ui.horizontal(|ui| {
                        ui.strong(tl!("Select Subject model"));
                        badge(ui, subject_ready);
                    });
                    ui.label(tl!(
                        "Finds the main subject, even on busy backgrounds, for Select › Subject, Select Background and All Objects. About 170 MB on disk and under 1 GB of memory while it runs."
                    ));
                    ui.horizontal(|ui| {
                        if !subject_ready && ui.add_enabled(!busy, egui::Button::new(tl!("Download (170 MB)"))).clicked() {
                            action = Some(("select.aiModels.install", json!({"pack": "subject"})));
                        }
                        if subject_ready && ui.add_enabled(!busy, egui::Button::new(tl!("Remove"))).clicked() {
                            action = Some(("select.aiModels.remove", json!({"pack": "subject"})));
                        }
                    });
                });
                ui.add_space(8.0);
                card(ui, &mut |ui| {
                    ui.horizontal(|ui| {
                        ui.strong(tl!("Light Pose depth model"));
                        badge(ui, depth_ready);
                    });
                    ui.label(tl!(
                        "Rebuilds the 3D shape of a photo for Layer › New › Light Pose Layer, so new light casts real shadows and highlights. About 67 MB."
                    ));
                    ui.horizontal(|ui| {
                        if !depth_ready && ui.add_enabled(!busy, egui::Button::new(tl!("Download (67 MB)"))).clicked() {
                            action = Some(("select.aiModels.install", json!({"pack": "depth"})));
                        }
                        if depth_ready && ui.add_enabled(!busy, egui::Button::new(tl!("Remove"))).clicked() {
                            action = Some(("select.aiModels.remove", json!({"pack": "depth"})));
                        }
                    });
                });
                ui.add_space(8.0);
                card(ui, &mut |ui| {
                    ui.horizontal(|ui| {
                        ui.strong(tl!("Prompt & Click models"));
                        badge(ui, prompt_ready);
                    });
                    ui.label(tl!(
                        "Click an object, drag a box around it, or type what to select (Object Selection tool). Installs a private Python with PyTorch, SAM 2.1 Tiny and Grounding DINO Tiny, about 2 GB. It runs only while you use it and quits after three idle minutes."
                    ));
                    ui.horizontal(|ui| {
                        if !prompt_ready && ui.add_enabled(!busy, egui::Button::new(tl!("Install (about 2 GB)"))).clicked() {
                            action = Some(("select.aiModels.install", json!({"pack": "prompt"})));
                        }
                        if prompt_ready && ui.add_enabled(!busy, egui::Button::new(tl!("Remove"))).clicked() {
                            action = Some(("select.aiModels.remove", json!({"pack": "prompt"})));
                        }
                    });
                });
                ui.add_space(10.0);
                if busy {
                    let stage = progress.get("stage").and_then(Value::as_str).unwrap_or("");
                    let done = progress.get("done").and_then(Value::as_u64).unwrap_or(0);
                    let total = progress.get("total").and_then(Value::as_u64).unwrap_or(0);
                    ui.label(stage);
                    if total > 0 {
                        let fraction = (done as f32 / total as f32).clamp(0.0, 1.0);
                        let text = format!("{} / {} MB", done >> 20, total >> 20);
                        ui.add(egui::ProgressBar::new(fraction).show_percentage().text(text));
                    } else {
                        ui.add(egui::ProgressBar::new(0.0).animate(true));
                    }
                    if ui.button(tl!("Cancel")).clicked() {
                        action = Some(("select.aiModels.cancel", json!({})));
                    }
                } else if let Some(error) = progress.get("error").and_then(Value::as_str) {
                    ui.colored_label(visuals.error_fg_color, error);
                }
                if let Some(dir) = status.get("modelsDir").and_then(Value::as_str) {
                    ui.add_space(6.0);
                    ui.weak(crate::i18n::fmt(tl!("Models folder: {dir}"), &[("dir", dir)]));
                }
            });
        });
    if let Some((id, params)) = action {
        act(app, id, params);
    }
    if !open {
        app.ai.models_window = false;
    }
}

/// "Installed" or "Not installed" beside a card's title.
fn badge(ui: &mut egui::Ui, installed: bool) {
    let visuals = ui.visuals();
    let (text, color) = if installed { (tl!("Installed"), visuals.selection.bg_fill) } else { (tl!("Not installed"), visuals.weak_text_color()) };
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(egui::RichText::new(text).small().strong().color(color));
    });
}
