//! File › Save As's format step: a small window that asks which kind of file to save (Nephclick PhotoLab's own
//! `.nplab`, Photoshop, an editable PDF, TIFF, PNG…) before the system's save dialog opens on that type. The system
//! dialog doesn't offer a type menu on every platform (macOS shows none), so the choice is made here.

use std::path::Path;
use std::sync::{Mutex, PoisonError};

use crate::NephclickPhotoLabApp;
use crate::services::FormatChoices;

/// The formats offered: (extension, name, what it keeps).
const FORMATS: &[(&str, &str, &str)] = &[
    ("nplab", "Nephclick PhotoLab (.nplab)", "Keeps everything: layers, masks, effects, text and smart objects."),
    ("psd", "Photoshop (.psd)", "Layers and most effects, for Photoshop and other editors."),
    ("psb", "Photoshop Large Document (.psb)", "Like PSD, for documents over 30,000 pixels or 2 GB."),
    ("pdf", "Editable PDF (.pdf)", "Opens in any PDF viewer; reopens here with every layer editable."),
    ("tif", "TIFF (.tif)", "Layers kept for Photoshop; widely used for print."),
    ("ora", "OpenRaster (.ora)", "Layers for Krita, GIMP and MyPaint."),
    ("png", "PNG (.png)", "A flattened copy with transparency."),
    ("jpg", "JPEG (.jpg)", "A flattened copy, small, without transparency."),
    ("webp", "WebP (.webp)", "A flattened copy for the web."),
];

/// The open prompt.
#[derive(Clone)]
struct Prompt {
    doc: nephclick_photolab_doc::DocId,
    suggested: String,
    choices: FormatChoices,
    plain_save: bool,
    picked: String,
}

static PROMPT: Mutex<Option<Prompt>> = Mutex::new(None);
/// The format picked last in this session, offered first next time.
static LAST: Mutex<Option<String>> = Mutex::new(None);

/// Opens the step for document `doc`, whose save would suggest `suggested`.
pub(crate) fn ask(app: &mut NephclickPhotoLabApp, doc: nephclick_photolab_doc::DocId, suggested: String, choices: FormatChoices, plain_save: bool) {
    let ext = Path::new(&suggested).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let ext = match ext.as_str() {
        "npl" | "pcraft" => "nplab".to_owned(),
        "tiff" => "tif".to_owned(),
        "jpeg" => "jpg".to_owned(),
        other => other.to_owned(),
    };
    let last = LAST.lock().unwrap_or_else(PoisonError::into_inner).clone();
    // The document's own format when it is offered, else the last pick, else the native format.
    let picked = if FORMATS.iter().any(|f| f.0 == ext) && ext != "psd" { ext } else { last.unwrap_or_else(|| ext.clone()) };
    let picked = if FORMATS.iter().any(|f| f.0 == picked) { picked } else { "nplab".to_owned() };
    *PROMPT.lock().unwrap_or_else(PoisonError::into_inner) = Some(Prompt { doc, suggested, choices, plain_save, picked });
    app.ctx().request_repaint();
}

/// Whether the step is open.
pub(crate) fn is_open() -> bool {
    PROMPT.lock().unwrap_or_else(PoisonError::into_inner).is_some()
}

/// Draws the step while it is open.
pub(crate) fn show(app: &mut NephclickPhotoLabApp, ctx: &egui::Context) {
    let Some(mut prompt) = PROMPT.lock().unwrap_or_else(PoisonError::into_inner).clone() else { return };
    let mut open = true;
    let mut go = false;
    let mut cancel = false;
    egui::Window::new(tl!("Save As"))
        .id(egui::Id::unique("save-format-step"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .default_width(crate::theme::DIALOG_WIDTH_MD)
        .show(ctx, |ui| {
            ui.label(tl!("Choose the format to save in:"));
            ui.add_space(6.0);
            for (ext, name, note) in FORMATS {
                let selected = prompt.picked == *ext;
                let response = ui.radio(selected, *name);
                if response.clicked() {
                    prompt.picked = (*ext).to_owned();
                }
                if response.double_clicked() {
                    prompt.picked = (*ext).to_owned();
                    go = true;
                }
                ui.indent(*ext, |ui| ui.weak(*note));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(tl!("Other Formats…")).min_size(egui::vec2(0.0, crate::theme::CONTROL_HEIGHT))).clicked() {
                    prompt.picked = String::new();
                    go = true;
                }
                let buttons = crate::theme::dialog_buttons(ui, tl!("Continue…"), true);
                go |= buttons.confirm || ui.input(|i| i.key_pressed(egui::Key::Enter));
                cancel |= buttons.cancel;
            });
        });
    if !open || cancel {
        *PROMPT.lock().unwrap_or_else(PoisonError::into_inner) = None;
        return;
    }
    if !go {
        *PROMPT.lock().unwrap_or_else(PoisonError::into_inner) = Some(prompt);
        return;
    }
    *PROMPT.lock().unwrap_or_else(PoisonError::into_inner) = None;
    let suggested = if prompt.picked.is_empty() {
        prompt.suggested.clone()
    } else {
        *LAST.lock().unwrap_or_else(PoisonError::into_inner) = Some(prompt.picked.clone());
        Path::new(&prompt.suggested).with_extension(&prompt.picked).to_string_lossy().into_owned()
    };
    if let Err(e) = app.pick_document_path(prompt.doc, suggested, prompt.choices, prompt.plain_save)
        && e != crate::file_dialog::CANCELLED
    {
        app.ui.status = e;
        app.ui.status_error = true;
    }
}
