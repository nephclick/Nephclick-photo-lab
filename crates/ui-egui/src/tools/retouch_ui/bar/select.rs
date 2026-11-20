//! The Retouch options bar's selection family: Quick Selection and Object Selection.

use craft::ui::buttons::Button;
use craft::ui::contract::{Show as _, clicked};
use craft::ui::display::Divider;
use craft::ui::text::RangeAlert;
use serde_json::json;

use super::heal::check;
use crate::NephclickPhotoLabApp;
use crate::option_cells::hint;

/// The Select Subject button both selection tools carry; `empty` is the message when nothing is found. It runs
/// offline: the subject model when installed, else the built-in method, with a hint to install the model.
fn select_subject(ui: &mut egui::Ui, app: &mut NephclickPhotoLabApp, empty: &str) {
    if clicked(&Button::new(tl!("Select Subject")).show(ui)) {
        let r = app.run("select.subject", json!({}));
        let classic = r.as_ref().ok().and_then(|v| v.get("engine")).and_then(serde_json::Value::as_str) == Some("classic");
        crate::retouch_ui::report_smart_result(app, r, empty);
        if classic && !app.ui.status_error && !nephclick_photolab_ai_ready() {
            app.ui.status = tl!("For busy backgrounds, download the Select Subject model: Select › AI Selection Models…").into();
        }
    }
    if clicked(&Button::new(tl!("AI Models…")).show(ui)) {
        crate::ai_models_ui::open(app);
    }
}

/// Whether the offline Select Subject model is installed.
fn nephclick_photolab_ai_ready() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        nephclick_photolab_ai::model_install::subject_installed()
    }
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
}

/// Object Selection's AI controls: AI Click (clicks and rectangles go to the Prompt & Click models) and a text prompt
/// that selects what it names.
fn ai_prompt(ui: &mut egui::Ui, app: &mut NephclickPhotoLabApp) {
    check(ui, tl!("AI Click"), &mut app.ui.tool_options.object_ai, true, Some(tl!("Clicks and rectangles use the Prompt & Click models (Select › AI Selection Models…)")));
    let mut prompt = std::mem::take(&mut app.ai.select_prompt);
    let field = ui.add(egui::TextEdit::singleline(&mut prompt).hint_text(tl!("Type what to select…")).desired_width(170.0));
    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    let go = clicked(&Button::new(tl!("Select")).show(ui)) || enter;
    if prompt.chars().count() > nephclick_photolab_ai::MAX_PROMPT {
        prompt = prompt.chars().take(nephclick_photolab_ai::MAX_PROMPT).collect();
    }
    if go && !prompt.trim().is_empty() {
        let op = crate::tool_feedback::selection_op(
            crate::state::Tool::ObjectSelection,
            app.ui.selection_mode,
            craft::keymap::Mods::default(),
            crate::tool_feedback::has_selection(app),
        );
        let mode = crate::tool_feedback::mode_name(op);
        let r = app.run("select.ai", json!({"type": "text", "text": prompt.trim(), "mode": mode}));
        crate::retouch_ui::report_smart_result(app, r, tl!("Nothing matching the text was found"));
    }
    app.ai.select_prompt = prompt;
}

/// Quick Selection's options.
pub(super) fn quick_selection(ui: &mut egui::Ui, app: &mut NephclickPhotoLabApp) -> Option<RangeAlert> {
    let o = &mut app.ui.tool_options;
    check(ui, tl!("Sample All Layers"), &mut o.sample_all.quick_selection, true, None);
    check(ui, tl!("Enhance Edge"), &mut o.enhance_edge, true, None);
    hint(ui, crate::i18n::fmt(tl!("{key} to subtract"), &[("key", &crate::shortcuts::pretty("Alt"))]));
    Divider::vertical().show(ui);
    select_subject(ui, app, tl!("No subject found — try Object Selection or the Lasso"));
    None
}

/// Object Selection's options: the selection operation (#12853), Object Finder, Sample All Layers
/// and Hard Edge.
pub(super) fn object_selection(ui: &mut egui::Ui, app: &mut NephclickPhotoLabApp) -> Option<RangeAlert> {
    crate::panels::options_bar::selection::operation_segments(app, ui, true);
    Divider::vertical().show(ui);
    let o = &mut app.ui.tool_options;
    check(ui, tl!("Object Finder"), &mut o.object_finder, true, None);
    check(ui, tl!("Sample All Layers"), &mut o.sample_all.object_selection, true, None);
    check(ui, tl!("Hard Edge"), &mut o.object_hard_edge, true, None);
    hint(ui, tl!("Drag a rectangle around the object"));
    Divider::vertical().show(ui);
    ai_prompt(ui, app);
    Divider::vertical().show(ui);
    if clicked(&Button::new(tl!("Background")).show(ui)) {
        let r = app.run("select.ai", json!({"type": "background"}));
        crate::retouch_ui::report_smart_result(app, r, tl!("No background found"));
    }
    if clicked(&Button::new(tl!("All Objects")).show(ui)) {
        let r = app.run("select.ai", json!({"type": "all"}));
        crate::retouch_ui::report_smart_result(app, r, tl!("No objects found"));
    }
    select_subject(ui, app, tl!("No subject found — try a rectangle around the object"));
    None
}
