//! Layer › Batch Rename Layers…: a window naming the selected layers at once. A base name numbered from the top of
//! the Layers panel (with the first number and zero padding), find and replace, a prefix and a suffix; the list
//! underneath shows each layer's new name before Rename applies them as one History step.

use std::sync::{Mutex, PoisonError};

use nephclick_photolab_doc::LayerId;
use nephclick_photolab_engine::layer_batch_cmds::RenamePlan;
use serde_json::json;

use crate::NephclickPhotoLabApp;

/// How many layers the preview lists before "and N more".
const PREVIEW_ROWS: usize = 8;

#[derive(Clone)]
struct Form {
    base: String,
    start: u64,
    digits: usize,
    find: String,
    replace: String,
    prefix: String,
    suffix: String,
}

impl Default for Form {
    fn default() -> Self {
        Form { base: String::new(), start: 1, digits: 2, find: String::new(), replace: String::new(), prefix: String::new(), suffix: String::new() }
    }
}

impl Form {
    fn plan(&self) -> RenamePlan {
        RenamePlan {
            base: self.base.clone(),
            start: self.start,
            digits: self.digits,
            find: self.find.clone(),
            replace: self.replace.clone(),
            prefix: self.prefix.clone(),
            suffix: self.suffix.clone(),
        }
    }

    fn does_something(&self) -> bool {
        !(self.base.trim().is_empty() && self.find.is_empty() && self.prefix.is_empty() && self.suffix.is_empty())
    }
}

static FORM: Mutex<Option<Form>> = Mutex::new(None);

/// Opens the window (the settings of the last rename stay for the next).
pub(crate) fn open(app: &mut NephclickPhotoLabApp) {
    let mut form = FORM.lock().unwrap_or_else(PoisonError::into_inner);
    if form.is_none() {
        *form = Some(Form::default());
    }
    app.ctx().request_repaint();
}

/// The selected layers, top of the panel first, with their names.
fn targets(app: &NephclickPhotoLabApp) -> Vec<(LayerId, String)> {
    let Some(st) = app.session.active() else { return Vec::new() };
    let selected = st.selected_layers();
    st.doc.walk().iter().rev().filter(|(_, _, l)| selected.contains(&l.id)).map(|(_, _, l)| (l.id, l.name.clone())).collect()
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.horizontal(|ui| {
        let l = ui.add_sized([96.0, crate::theme::CONTROL_HEIGHT], egui::Label::new(label));
        let width = ui.available_width().max(crate::theme::FIELD_WIDTH);
        ui.add(egui::TextEdit::singleline(value).desired_width(width).char_limit(255)).labelled_by(l.id);
    });
}

/// Draws the window while it is open.
pub(crate) fn show(app: &mut NephclickPhotoLabApp, ctx: &egui::Context) {
    let Some(mut form) = FORM.lock().unwrap_or_else(PoisonError::into_inner).clone() else { return };
    let layers = targets(app);
    let (mut open, mut apply, mut cancel) = (true, false, false);
    egui::Window::new(tl!("Batch Rename Layers"))
        .id(egui::Id::unique("batch-rename-layers"))
        .open(&mut open)
        .collapsible(true)
        .resizable(true)
        .default_width(crate::theme::DIALOG_WIDTH_SM)
        .min_width(crate::theme::DIALOG_WIDTH_SM)
        .show(ctx, |ui| {
            ui.label(crate::i18n::fmt(tl!("{n} layers selected, numbered from the top of the Layers panel."), &[("n", &layers.len().to_string())]));
            ui.add_space(6.0);
            field(ui, tl!("Name:"), &mut form.base);
            ui.horizontal(|ui| {
                let l = ui.add_sized([96.0, crate::theme::CONTROL_HEIGHT], egui::Label::new(tl!("Start at:")));
                ui.add(egui::DragValue::new(&mut form.start).range(0..=1_000_000)).labelled_by(l.id);
                ui.add_space(crate::theme::SPACE_MD);
                let d = ui.label(tl!("Digits:"));
                ui.add(egui::DragValue::new(&mut form.digits).range(0..=6)).labelled_by(d.id);
            });
            field(ui, tl!("Find:"), &mut form.find);
            field(ui, tl!("Replace with:"), &mut form.replace);
            field(ui, tl!("Prefix:"), &mut form.prefix);
            field(ui, tl!("Suffix:"), &mut form.suffix);
            ui.separator();
            ui.strong(tl!("Preview"));
            let plan = form.plan();
            for (i, (_, name)) in layers.iter().take(PREVIEW_ROWS).enumerate() {
                let new = if form.does_something() { plan.name(name, i) } else { name.clone() };
                ui.horizontal_wrapped(|ui| {
                    ui.weak(name.as_str());
                    ui.label("→");
                    ui.label(new);
                });
            }
            if layers.len() > PREVIEW_ROWS {
                ui.weak(crate::i18n::fmt(tl!("and {n} more"), &[("n", &(layers.len() - PREVIEW_ROWS).to_string())]));
            }
            ui.separator();
            let buttons = crate::theme::dialog_buttons(ui, tl!("Rename"), form.does_something() && !layers.is_empty());
            apply = buttons.confirm;
            cancel = buttons.cancel;
        });
    if apply {
        let ids: Vec<u64> = layers.iter().map(|(id, _)| id.0).collect();
        let params = json!({
            "name": form.base.trim(), "start": form.start, "digits": form.digits, "find": form.find, "replace": form.replace,
            "prefix": form.prefix, "suffix": form.suffix, "layers": ids,
        });
        if let Err(e) = app.run("layer.batchRename", params) {
            app.ui.status = e;
            app.ui.status_error = true;
        }
    }
    let mut held = FORM.lock().unwrap_or_else(PoisonError::into_inner);
    *held = if !open || cancel || apply { None } else { Some(form) };
}
