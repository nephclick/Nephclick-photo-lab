//! The Selection Bar: the contextual task bar under a selection. Left to right:
//!
//! 1. the grip (the bar's);
//! 2. a prompt and **AI Fill**: the selection's area is painted by the image model of Preferences › AI Models and
//!    comes back as a new layer masked to the selection (`ai_ui::start_fill`, `layer.new.fromImage`);
//! 3. Invert, Mask (Reveal Selection as a layer mask) and Deselect;
//! 4. the ⋯ menu (the bar's).
//!
//! Shown while the active document has a selection, Preferences › AI Models › Show the Selection Bar and Window ›
//! Contextual Task Bar are on, and nothing else holds the canvas: no dialog, transform, text edit or drag, and not
//! with the Crop or Frame tool, which show their own bar.

use craft::behavior::IconId;
use craft::ui::buttons::Button;
use craft::ui::canvas_controls::TaskBar;
use craft::ui::contract::{Bind, Props as _, Show as _};
use egui::Vec2;
use craft::behavior::kurbo::Point;
use serde_json::json;

use crate::NephclickPhotoLabApp;
use crate::state::Tool;

/// What a button asked for, run after the bar is drawn.
enum Act {
    Fill,
    Command(&'static str),
}

/// The selection's bounds on the active document, in document pixels, when the bar may show.
fn anchor(app: &NephclickPhotoLabApp) -> Option<nephclick_photolab_geom::Rect> {
    if !app.session.prefs().ai.show_selection_bar || !app.ui.panels.context_task_bar || app.dialog_open() {
        return None;
    }
    if matches!(app.ui.tool, Tool::Crop | Tool::Frame) || app.ui.transform.is_some() || app.ui.text_edit.is_some() {
        return None;
    }
    if crate::canvas::tools::in_gesture(app) {
        return None;
    }
    let st = app.session.active()?;
    let bounds = st.doc.selection.as_ref()?.content_bounds().intersect(&st.doc.bounds());
    (!bounds.is_empty()).then_some(bounds)
}

/// Whether the bar shows now.
#[cfg(test)]
pub fn eligible(app: &NephclickPhotoLabApp) -> bool {
    anchor(app).is_some()
}

/// Shows the bar under the active document's selection, inside `area` (the canvas body's rect).
pub fn show(app: &mut NephclickPhotoLabApp, ui: &mut egui::Ui, area: egui::Rect) {
    let Some(b) = anchor(app) else { return };
    let (x0, y0, x1, y1) = (f64::from(b.x0), f64::from(b.y0), f64::from(b.x1), f64::from(b.y1));
    let corners = [Point::new(x0, y0), Point::new(x1, y0), Point::new(x1, y1), Point::new(x0, y1)];
    let busy = app.ai.is_busy(crate::ai_ui::Task::Fill);
    let note = app.ai.fill_note.clone();
    let mut acts = Vec::new();
    let mut place = app.ui.panels.task_bar;
    let mut prompt = std::mem::take(&mut app.ai.fill_prompt);
    let output = TaskBar::new(tl!("Selection Bar"), &corners, Bind::bound(&mut place)).show(ui, area, |ui, cx| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        let field = egui::TextEdit::singleline(&mut prompt)
            .hint_text(tl!("Describe what to add (optional)"))
            .desired_width(220.0)
            .margin(egui::Margin::symmetric(8, 5))
            .char_limit(nephclick_photolab_ai::MAX_PROMPT);
        let resp = ui.add_enabled(!busy, field);
        let submitted = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let fill_label = if busy { tl!("Painting…") } else { tl!("AI Fill") };
        let fill = Button::new(fill_label).icon(IconId::new("sparkles")).enabled(!busy).show(ui);
        if fill.clicked() || (submitted && !busy) {
            acts.push(Act::Fill);
        }
        ui.separator();
        if Button::new(tl!("Refine Edge")).show(ui).clicked() {
            acts.push(Act::Command("select.selectAndMask"));
        }
        if Button::new(tl!("Invert")).icon(IconId::new("contrast")).show(ui).clicked() {
            acts.push(Act::Command("select.inverse"));
        }
        if Button::new(tl!("Mask")).icon(IconId::new("layer_mask")).show(ui).clicked() {
            acts.push(Act::Command("layer.layerMask.revealSelection"));
        }
        if Button::new(tl!("Copy")).show(ui).clicked() {
            acts.push(Act::Command("layer.new.layerViaCopy"));
        }
        if Button::new(tl!("Deselect")).icon(IconId::new("close")).show(ui).clicked() {
            acts.push(Act::Command("select.deselect"));
        }
        cx.more_menu(ui);
    });
    app.ai.fill_prompt = prompt;
    app.ui.panels.task_bar = place;
    if output.output.hide {
        app.ui.panels.context_task_bar = false;
        return;
    }
    let r = output.output.rect;
    app.ui.task_bar_shown = Some([r.min.x, r.min.y, r.max.x, r.max.y]);
    // The last AI Fill's outcome, just under the bar.
    if let Some(note) = note {
        let t = crate::theme::Tokens::get(ui.ctx());
        let at = egui::pos2(r.min.x + 8.0, r.max.y + 6.0);
        let galley = ui.painter().layout(note.text.clone(), egui::FontId::proportional(12.0), if note.error { t.danger } else { t.text }, (r.width() - 16.0).max(160.0));
        let bg = egui::Rect::from_min_size(at - Vec2::new(6.0, 3.0), galley.size() + Vec2::new(12.0, 6.0));
        ui.painter().rect_filled(bg, t.radius_sm, t.card);
        ui.painter().rect_stroke(bg, t.radius_sm, egui::Stroke::new(1.0, t.card_border), egui::StrokeKind::Inside);
        ui.painter().galley(at, galley, t.text);
    }
    let ctx = ui.ctx().clone();
    for act in acts {
        match act {
            Act::Fill => crate::ai_ui::start_fill(app, &ctx),
            Act::Command(id) => {
                if let Err(e) = crate::menus::invoke(app, &ctx, id, json!({})) {
                    app.ui.status = e;
                    app.ui.status_error = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_only_with_a_selection_and_the_preference_on() {
        let mut app = NephclickPhotoLabApp::new(nephclick_photolab_engine::Session::new(), crate::Services::default());
        assert!(!eligible(&app));
        app.run("file.new", json!({"width": 64, "height": 64})).unwrap();
        app.ui.panels.context_task_bar = true;
        assert!(!eligible(&app), "no selection");
        app.run("select.rect", json!({"x": 4, "y": 4, "width": 10, "height": 10})).unwrap();
        assert!(eligible(&app));
        app.run("prefs.set", json!({"values": {"ai.showSelectionBar": false}})).unwrap();
        assert!(!eligible(&app));
        app.run("prefs.set", json!({"values": {"ai.showSelectionBar": true}})).unwrap();
        app.ui.tool = Tool::Crop;
        assert!(!eligible(&app), "the Crop tool shows its own bar");
    }
}
