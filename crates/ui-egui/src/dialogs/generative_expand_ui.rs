//! Image › Generative Expand…: makes the canvas larger and has the AI service paint the new area so the picture
//! continues past its old edges. The canvas grows by the width and height asked for (Canvas Size with a transparent
//! extension, the picture where the anchor says), the new border is selected with a few pixels of overlap so the
//! seams blend, and AI Fill paints it from the prompt, or from the picture's surroundings with no prompt. It uses the
//! service and key of Preferences › AI Integrations; the canvas change and the painted layer are each undoable.

use std::sync::{Mutex, PoisonError};

use serde_json::json;

use crate::NephclickPhotoLabApp;

/// Where the picture stays inside the larger canvas.
const ANCHORS: [(&str, &str); 9] = [
    ("center", "Centre (expand every side)"),
    ("left", "Left (expand right)"),
    ("right", "Right (expand left)"),
    ("top", "Top (expand down)"),
    ("bottom", "Bottom (expand up)"),
    ("topLeft", "Top left"),
    ("topRight", "Top right"),
    ("bottomLeft", "Bottom left"),
    ("bottomRight", "Bottom right"),
];
/// Pixels of the old picture the selection takes in, so the painted border blends into it.
const OVERLAP: i32 = 4;
/// The most a side may grow, in pixels.
const MAX_GROW: u32 = 8192;

#[derive(Clone)]
struct Form {
    width: u32,
    height: u32,
    anchor: usize,
    prompt: String,
}

static FORM: Mutex<Option<Form>> = Mutex::new(None);

/// Opens the window.
pub(crate) fn open(app: &mut NephclickPhotoLabApp) {
    let mut held = FORM.lock().unwrap_or_else(PoisonError::into_inner);
    if held.is_none() {
        // A quarter more on each axis, as a start.
        let (w, h) = app.session.active().map_or((0, 0), |st| (st.doc.size.width / 4, st.doc.size.height / 4));
        *held = Some(Form { width: w.max(1), height: h.max(1), anchor: 0, prompt: String::new() });
    }
    app.ctx().request_repaint();
}

/// The offset of the old picture's top left corner inside a canvas grown by (`dw`, `dh`) at `anchor`.
fn offset(anchor: &str, dw: i32, dh: i32) -> (i32, i32) {
    let x = if anchor.contains("Left") || anchor == "left" {
        0
    } else if anchor.contains("Right") || anchor == "right" {
        dw
    } else {
        dw / 2
    };
    let y = if anchor.starts_with("top") {
        0
    } else if anchor.starts_with("bottom") {
        dh
    } else {
        dh / 2
    };
    (x, y)
}

/// Grows the canvas, selects the new border and starts AI Fill on it.
fn generate(app: &mut NephclickPhotoLabApp, ctx: &egui::Context, form: &Form) -> Result<(), String> {
    let st = app.session.active().ok_or("Open a document first.")?;
    let (ow, oh) = (st.doc.size.width as i32, st.doc.size.height as i32);
    let anchor = ANCHORS.get(form.anchor).map_or("center", |a| a.0);
    let (dw, dh) = (form.width.min(MAX_GROW) as i32, form.height.min(MAX_GROW) as i32);
    if dw == 0 && dh == 0 {
        return Err("Give the canvas some width or height to grow by.".into());
    }
    app.run("image.canvasSize", json!({"width": dw, "height": dh, "relative": true, "anchor": anchor, "extensionColor": "transparent"}))?;
    let (x, y) = offset(anchor, dw, dh);
    app.run("select.rect", json!({"x": 0, "y": 0, "width": ow + dw, "height": oh + dh, "antiAlias": false}))?;
    let inner = json!({"x": x + OVERLAP, "y": y + OVERLAP, "width": (ow - 2 * OVERLAP).max(1), "height": (oh - 2 * OVERLAP).max(1), "mode": "subtract", "antiAlias": false});
    app.run("select.rect", inner)?;
    app.ai.fill_prompt = form.prompt.trim().to_string();
    crate::ai_ui::start_fill(app, ctx);
    Ok(())
}

/// Draws the window while it is open.
pub(crate) fn show(app: &mut NephclickPhotoLabApp, ctx: &egui::Context) {
    let Some(mut form) = FORM.lock().unwrap_or_else(PoisonError::into_inner).clone() else { return };
    let (mut open, mut go, mut cancel) = (true, false, false);
    egui::Window::new(tl!("Generative Expand"))
        .id(egui::Id::unique("generative-expand"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(crate::theme::DIALOG_WIDTH_MD)
        .show(ctx, |ui| {
            ui.label(tl!("Make the canvas larger; the AI service paints the new area so the picture continues."));
            ui.add_space(crate::theme::SPACE_SM);
            egui::Grid::new("generative-expand-grid").num_columns(2).spacing([crate::theme::SPACE_MD, crate::theme::SPACE_SM]).show(ui, |ui| {
                let l = ui.label(tl!("Add width:"));
                ui.add(egui::DragValue::new(&mut form.width).range(0..=MAX_GROW).suffix(" px")).labelled_by(l.id);
                ui.end_row();
                let l = ui.label(tl!("Add height:"));
                ui.add(egui::DragValue::new(&mut form.height).range(0..=MAX_GROW).suffix(" px")).labelled_by(l.id);
                ui.end_row();
                ui.label(tl!("Picture at:"));
                egui::ComboBox::from_id_salt("generative-expand-anchor")
                    .width(crate::theme::FIELD_WIDTH)
                    .selected_text(tl!(ANCHORS.get(form.anchor).map_or("", |a| a.1)))
                    .show_ui(ui, |ui| {
                        for (i, (_, name)) in ANCHORS.iter().enumerate() {
                            ui.selectable_value(&mut form.anchor, i, tl!(name));
                        }
                    });
                ui.end_row();
                let l = ui.label(tl!("Prompt:"));
                ui.add(egui::TextEdit::singleline(&mut form.prompt).hint_text(tl!("Optional: what to paint")).desired_width(crate::theme::FIELD_WIDTH))
                    .labelled_by(l.id);
                ui.end_row();
            });
            if let Some(note) = &app.ai.fill_note {
                ui.add_space(crate::theme::SPACE_XS);
                if note.error {
                    ui.colored_label(ui.visuals().error_fg_color, note.text.as_str());
                } else {
                    ui.weak(note.text.as_str());
                }
            }
            let buttons = crate::theme::dialog_buttons(ui, tl!("Generate"), form.width > 0 || form.height > 0);
            go = buttons.confirm;
            cancel = buttons.cancel;
        });
    if go {
        if let Err(e) = generate(app, ctx, &form) {
            app.ai.fill_note = Some(crate::ai_ui::Note { text: e, error: true });
            *FORM.lock().unwrap_or_else(PoisonError::into_inner) = Some(form);
            return;
        }
        *FORM.lock().unwrap_or_else(PoisonError::into_inner) = None;
        return;
    }
    *FORM.lock().unwrap_or_else(PoisonError::into_inner) = if !open || cancel { None } else { Some(form) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picture_sits_where_the_anchor_says() {
        assert_eq!(offset("center", 100, 60), (50, 30));
        assert_eq!(offset("left", 100, 60), (0, 30));
        assert_eq!(offset("right", 100, 60), (100, 30));
        assert_eq!(offset("top", 100, 60), (50, 0));
        assert_eq!(offset("bottomRight", 100, 60), (100, 60));
        assert_eq!(offset("topLeft", 100, 60), (0, 0));
    }

    #[test]
    fn generate_grows_the_canvas_and_selects_the_border() {
        let mut app = NephclickPhotoLabApp::new(nephclick_photolab_engine::Session::new(), crate::Services::default());
        app.run("file.new", json!({"width": 100, "height": 80})).unwrap();
        let ctx = app.ctx().clone();
        let form = Form { width: 40, height: 20, anchor: 0, prompt: String::new() };
        // Without an AI service configured, AI Fill reports that; the canvas and the border selection are made first.
        let _ = generate(&mut app, &ctx, &form);
        let st = app.session.active().unwrap();
        assert_eq!((st.doc.size.width, st.doc.size.height), (140, 100));
        assert!(st.doc.selection.is_some(), "the new border is selected");
    }
}
