//! The app side of the AI service (`nephclick_photolab_ai`): the settings and stored key a request uses, the
//! requests themselves on a worker thread (the window never waits on the network), and what their answers do.
//!
//! - A palette answer fills the Color Palettes panel (`palette_panel`).
//! - An idea answer shows on the Home screen (`canvas::start_screen`), whose Create runs `file.new`.
//! - An AI Fill answer is placed with `layer.new.fromImage` over the area that was sent, masked to the selection,
//!   on the document it was asked for.
//!
//! The browser build makes no requests: every start reports that AI needs the desktop app.

use std::sync::mpsc;

use nephclick_photolab_ai::idea::Idea;
use nephclick_photolab_ai::palette::Palette;
use nephclick_photolab_ai::{AiError, Config};
use nephclick_photolab_doc::DocId;
use nephclick_photolab_geom::Rect;
use serde_json::json;

use crate::NephclickPhotoLabApp;

/// What a request is for; one of each runs at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Task {
    Palette,
    Idea,
    Fill,
    Test,
}

/// A finished request.
enum Done {
    Palette(Result<Palette, AiError>),
    Idea(Result<Idea, AiError>),
    Fill { result: Result<Vec<u8>, AiError>, doc: DocId, rect: Rect, label: String },
    Test(Result<String, AiError>),
}

/// A line under a field: what happened, and whether it was a failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub text: String,
    pub error: bool,
}

impl Note {
    fn ok(text: impl Into<String>) -> Self {
        Note { text: text.into(), error: false }
    }
    fn err(text: impl Into<String>) -> Self {
        Note { text: text.into(), error: true }
    }
}

/// The AI service's state in the app.
pub struct AiUi {
    /// The stored key, read once (`None`: not read yet).
    key: Option<Option<String>>,
    tx: mpsc::Sender<Done>,
    rx: mpsc::Receiver<Done>,
    busy: Vec<Task>,
    /// The Color Palettes panel: its prompt, search, and the palette it shows.
    pub palette_prompt: String,
    pub theme_query: String,
    pub palette: Option<Palette>,
    pub palette_note: Option<Note>,
    /// The Home screen's idea field and the idea it shows.
    pub idea_prompt: String,
    pub idea: Option<Idea>,
    pub idea_note: Option<Note>,
    /// The Selection Bar's prompt.
    pub fill_prompt: String,
    pub fill_note: Option<Note>,
    /// The AI Models page's key field (never pre-filled with the stored key) and the button pressed under it. The
    /// Preferences window draws it from a copy of what it reads of the app, so the field is shared with that copy.
    pub key_field: std::sync::Arc<std::sync::Mutex<KeyField>>,
    pub key_note: Option<Note>,
}

/// The API key field of the AI Models page, and what its buttons asked for.
#[derive(Default)]
pub struct KeyField {
    pub draft: String,
    pub action: Option<KeyAction>,
}

/// A button under the API key field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    Save,
    Remove,
    Test,
}

impl Default for AiUi {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        AiUi {
            key: None,
            tx,
            rx,
            busy: Vec::new(),
            palette_prompt: String::new(),
            theme_query: String::new(),
            palette: None,
            palette_note: None,
            idea_prompt: String::new(),
            idea: None,
            idea_note: None,
            fill_prompt: String::new(),
            fill_note: None,
            key_field: Default::default(),
            key_note: None,
        }
    }
}

impl AiUi {
    /// Whether a request for `task` is running.
    pub fn is_busy(&self, task: Task) -> bool {
        self.busy.contains(&task)
    }

    /// The stored key, masked, as of the last read (the Preferences window reads it without the app).
    pub fn masked_key(&self) -> Option<String> {
        self.key.clone().flatten().map(|k| nephclick_photolab_ai::key::masked(&k))
    }
}

/// Runs the button pressed under the AI Models page's key field, if any.
pub fn run_key_action(app: &mut NephclickPhotoLabApp, ctx: &egui::Context) {
    let (action, draft) = {
        let mut field = app.ai.key_field.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let action = field.action.take();
        let draft = if action == Some(KeyAction::Save) { std::mem::take(&mut field.draft) } else { String::new() };
        (action, draft)
    };
    match action {
        Some(KeyAction::Save) => save_key(app, &draft),
        Some(KeyAction::Remove) => clear_key(app),
        Some(KeyAction::Test) => start_test(app, ctx),
        None => {}
    }
}

/// The stored key, read from the settings folder the first time it is asked for.
pub fn stored_key(app: &mut NephclickPhotoLabApp) -> Option<String> {
    if app.ai.key.is_none() {
        let loaded = load_key(app);
        app.ai.key = Some(loaded);
    }
    app.ai.key.clone().flatten()
}

#[cfg(not(target_arch = "wasm32"))]
fn load_key(app: &mut NephclickPhotoLabApp) -> Option<String> {
    let dir = app.services.ai_dir.clone()?;
    match nephclick_photolab_ai::key::load(&dir) {
        Ok(key) => key,
        Err(e) => {
            app.ai.key_note = Some(Note::err(e.to_string()));
            None
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn load_key(_app: &mut NephclickPhotoLabApp) -> Option<String> {
    None
}

/// Whether a key is stored.
pub fn has_key(app: &mut NephclickPhotoLabApp) -> bool {
    stored_key(app).is_some()
}

/// The stored key, masked for display.
pub fn masked_key(app: &mut NephclickPhotoLabApp) -> Option<String> {
    stored_key(app).map(|k| nephclick_photolab_ai::key::masked(&k))
}

/// Stores `draft`, the key typed in the AI Models page.
pub fn save_key(app: &mut NephclickPhotoLabApp, draft: &str) {
    app.ai.key_note = Some(match store_key(app, draft) {
        Ok(()) => Note::ok("API key saved on this computer."),
        Err(e) => Note::err(e),
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn store_key(app: &mut NephclickPhotoLabApp, key: &str) -> Result<(), String> {
    let dir = app.services.ai_dir.clone().ok_or("This build has no settings folder to keep a key in.")?;
    nephclick_photolab_ai::key::save(&dir, key).map_err(|e| e.to_string())?;
    app.ai.key = Some(nephclick_photolab_ai::key::check(key).ok().map(str::to_string));
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn store_key(_app: &mut NephclickPhotoLabApp, _key: &str) -> Result<(), String> {
    Err(UNSUPPORTED.into())
}

/// Removes the stored key.
pub fn clear_key(app: &mut NephclickPhotoLabApp) {
    #[cfg(not(target_arch = "wasm32"))]
    let result = match app.services.ai_dir.clone() {
        Some(dir) => nephclick_photolab_ai::key::clear(&dir).map_err(|e| e.to_string()),
        None => Ok(()),
    };
    #[cfg(target_arch = "wasm32")]
    let result: Result<(), String> = Ok(());
    app.ai.key_note = Some(match result {
        Ok(()) => {
            app.ai.key = Some(None);
            Note::ok("API key removed.")
        }
        Err(e) => Note::err(e),
    });
}

/// The browser build's answer to every request.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const UNSUPPORTED: &str = "AI features need the desktop app.";

/// The settings a request uses: Preferences › AI Models and the stored key.
pub fn config(app: &mut NephclickPhotoLabApp) -> Config {
    let key = stored_key(app);
    let ai = &app.session.prefs().ai;
    Config { endpoint: ai.endpoint.clone(), key, text_model: ai.text_model.clone(), image_model: ai.image_model.clone() }
}

/// Runs `work` with the current settings on a worker thread, unless a request for `task` is running. Returns
/// whether it started; a request that cannot start says why through `fail`.
fn spawn(
    app: &mut NephclickPhotoLabApp,
    ctx: &egui::Context,
    task: Task,
    work: impl FnOnce(&Config) -> Done + Send + 'static,
    fail: impl FnOnce(&mut NephclickPhotoLabApp, String),
) -> bool {
    if app.ai.is_busy(task) {
        return false;
    }
    let config = config(app);
    if let Err(e) = config.ready() {
        fail(app, e.to_string());
        return false;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let tx = app.ai.tx.clone();
        let ctx = ctx.clone();
        let started = std::thread::Builder::new().name(format!("ai-{task:?}")).spawn(move || {
            // A panic in a request must not take the window down; it is reported as a failed request.
            let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&config)));
            let done = done.unwrap_or_else(|_| fallback(task));
            let _ = tx.send(done);
            ctx.request_repaint();
        });
        match started {
            Ok(_) => {
                app.ai.busy.push(task);
                true
            }
            Err(e) => {
                fail(app, format!("Could not start the request: {e}"));
                false
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (ctx, work);
        fail(app, UNSUPPORTED.into());
        false
    }
}

/// What a request that panicked reports.
#[cfg(not(target_arch = "wasm32"))]
fn fallback(task: Task) -> Done {
    let e = AiError::BadReply("the request failed unexpectedly".into());
    match task {
        Task::Palette => Done::Palette(Err(e)),
        Task::Idea => Done::Idea(Err(e)),
        Task::Test => Done::Test(Err(e)),
        Task::Fill => Done::Fill { result: Err(e), doc: DocId(0), rect: Rect::EMPTY, label: String::new() },
    }
}

/// Asks for a palette for the Color Palettes panel's prompt.
pub fn start_palette(app: &mut NephclickPhotoLabApp, ctx: &egui::Context) {
    let prompt = app.ai.palette_prompt.trim().to_string();
    if let Err(e) = nephclick_photolab_ai::check_prompt(&prompt) {
        app.ai.palette_note = Some(Note::err(e.to_string()));
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    let work = move |c: &Config| Done::Palette(nephclick_photolab_ai::client::palette(c, &prompt));
    #[cfg(target_arch = "wasm32")]
    let work = move |_: &Config| Done::Palette(Err(AiError::Unsupported(prompt.clone())));
    if spawn(app, ctx, Task::Palette, work, |app, e| app.ai.palette_note = Some(Note::err(e))) {
        app.ai.palette_note = Some(Note::ok("Generating a palette…"));
    }
}

/// Asks for a project idea for the Home screen's prompt.
pub fn start_idea(app: &mut NephclickPhotoLabApp, ctx: &egui::Context) {
    let prompt = app.ai.idea_prompt.trim().to_string();
    if let Err(e) = nephclick_photolab_ai::check_prompt(&prompt) {
        app.ai.idea_note = Some(Note::err(e.to_string()));
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    let work = move |c: &Config| Done::Idea(nephclick_photolab_ai::client::idea(c, &prompt));
    #[cfg(target_arch = "wasm32")]
    let work = move |_: &Config| Done::Idea(Err(AiError::Unsupported(prompt.clone())));
    if spawn(app, ctx, Task::Idea, work, |app, e| app.ai.idea_note = Some(Note::err(e))) {
        app.ai.idea = None;
        app.ai.idea_note = Some(Note::ok("Thinking of an idea…"));
    }
}

/// Checks the service and key from the AI Models page.
pub fn start_test(app: &mut NephclickPhotoLabApp, ctx: &egui::Context) {
    #[cfg(not(target_arch = "wasm32"))]
    let work = |c: &Config| Done::Test(nephclick_photolab_ai::client::test(c));
    #[cfg(target_arch = "wasm32")]
    let work = |_: &Config| Done::Test(Err(AiError::Unsupported(String::new())));
    if spawn(app, ctx, Task::Test, work, |app, e| app.ai.key_note = Some(Note::err(e))) {
        app.ai.key_note = Some(Note::ok("Checking the connection…"));
    }
}

/// The longest side of the image AI Fill sends: enough detail for a model, small enough to send quickly.
const FILL_SIDE: u32 = 1024;

/// The area AI Fill sends: the selection's bounds and a quarter of their size around them (so the model sees what
/// to blend into), within the canvas.
pub fn fill_area(selection: Rect, canvas: Rect) -> Rect {
    let margin = (selection.width().max(selection.height()) / 4).clamp(16, 512) as i32;
    selection.inflate(margin).intersect(&canvas)
}

/// Starts AI Fill on the active document's selection with the Selection Bar's prompt.
pub fn start_fill(app: &mut NephclickPhotoLabApp, ctx: &egui::Context) {
    let prepared = prepare_fill(app);
    let (png, doc, rect) = match prepared {
        Ok(p) => p,
        Err(e) => {
            app.ai.fill_note = Some(Note::err(e));
            return;
        }
    };
    let prompt = app.ai.fill_prompt.trim().to_string();
    let label = if prompt.is_empty() { "AI Fill".to_string() } else { format!("AI Fill: {}", prompt.chars().take(40).collect::<String>()) };
    #[cfg(not(target_arch = "wasm32"))]
    let work = move |c: &Config| Done::Fill { result: nephclick_photolab_ai::client::fill(c, &png, &prompt), doc, rect, label };
    #[cfg(target_arch = "wasm32")]
    let work = move |_: &Config| Done::Fill { result: Err(AiError::Unsupported(String::new())), doc, rect, label: { let _ = (&png, &prompt); label } };
    if spawn(app, ctx, Task::Fill, work, |app, e| app.ai.fill_note = Some(Note::err(e))) {
        app.ai.fill_note = Some(Note::ok("Painting… this can take a little while."));
    }
}

/// The PNG AI Fill sends (the active document's merged pixels over [`fill_area`], at most [`FILL_SIDE`] on a
/// side), the document and the area.
fn prepare_fill(app: &NephclickPhotoLabApp) -> Result<(Vec<u8>, DocId, Rect), String> {
    let st = app.session.active().ok_or("Open a document first.")?;
    if st.doc.mode != nephclick_photolab_color::ColorMode::Rgb {
        return Err("AI Fill works on RGB documents (Image › Mode › RGB Color).".into());
    }
    let selection = st.doc.selection.as_ref().ok_or("Make a selection first.")?;
    let canvas = st.doc.bounds();
    let bounds = selection.content_bounds().intersect(&canvas);
    if bounds.is_empty() {
        return Err("The selection is empty.".into());
    }
    let area = fill_area(bounds, canvas);
    let image = nephclick_photolab_compose::render(&st.doc, area).to_rgba8();
    let (w, h) = (image.width, image.height);
    let scale = (FILL_SIDE as f32 / w.max(h) as f32).min(1.0);
    let (sw, sh) = (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1));
    let pixels = if (sw, sh) == (w, h) { image.pixels } else { shrink(&image.pixels, w, h, sw, sh) };
    let encoded = nephclick_photolab_codecs::Image::from_u8(sw, sh, nephclick_photolab_codecs::ChannelLayout::Rgba, pixels)
        .and_then(|img| nephclick_photolab_codecs::encode(&img, nephclick_photolab_codecs::Format::Png, &Default::default()))
        .map_err(|e| format!("Could not prepare the selected area: {e}"))?;
    Ok((encoded, st.doc.id, area))
}

/// `px` (`w` × `h` RGBA8) reduced to `dw` × `dh` by averaging the source pixels under each target pixel.
fn shrink(px: &[u8], w: u32, h: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((dw as usize) * (dh as usize) * 4);
    for y in 0..dh {
        let (y0, y1) = ((y * h / dh), (((y + 1) * h).div_ceil(dh)).min(h).max(y * h / dh + 1));
        for x in 0..dw {
            let (x0, x1) = ((x * w / dw), (((x + 1) * w).div_ceil(dw)).min(w).max(x * w / dw + 1));
            let mut sum = [0u64; 4];
            let mut n = 0u64;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = ((sy as usize) * (w as usize) + sx as usize) * 4;
                    if let Some(p) = px.get(i..i + 4) {
                        for c in 0..4 {
                            sum[c] += u64::from(p[c]);
                        }
                        n += 1;
                    }
                }
            }
            let n = n.max(1);
            out.extend(sum.map(|s| (s / n) as u8));
        }
    }
    out
}

/// Takes the finished requests and applies them. Called every frame.
pub fn poll(app: &mut NephclickPhotoLabApp) {
    // The stored key is read once, so the Preferences window can show it masked.
    if app.ai.key.is_none() {
        stored_key(app);
    }
    while let Ok(done) = app.ai.rx.try_recv() {
        match done {
            Done::Palette(r) => {
                app.ai.busy.retain(|t| *t != Task::Palette);
                match r {
                    Ok(p) => {
                        app.ai.palette_note = Some(Note::ok(format!("Generated “{}”.", p.name)));
                        app.ai.palette = Some(p);
                    }
                    Err(e) => app.ai.palette_note = Some(Note::err(e.to_string())),
                }
            }
            Done::Idea(r) => {
                app.ai.busy.retain(|t| *t != Task::Idea);
                match r {
                    Ok(i) => {
                        app.ai.idea_note = None;
                        // The idea's colours also fill the Color Palettes panel.
                        app.ai.palette = Some(i.palette.clone());
                        app.ai.idea = Some(i);
                    }
                    Err(e) => app.ai.idea_note = Some(Note::err(e.to_string())),
                }
            }
            Done::Test(r) => {
                app.ai.busy.retain(|t| *t != Task::Test);
                app.ai.key_note = Some(match r {
                    Ok(m) => Note::ok(m),
                    Err(e) => Note::err(e.to_string()),
                });
            }
            Done::Fill { result, doc, rect, label } => {
                app.ai.busy.retain(|t| *t != Task::Fill);
                app.ai.fill_note = Some(match result.map_err(|e| e.to_string()).and_then(|png| place_fill(app, doc, rect, &label, &png)) {
                    Ok(()) => {
                        app.ai.fill_prompt.clear();
                        Note::ok("Added as a new layer, masked to the selection.")
                    }
                    Err(e) => Note::err(e),
                });
            }
        }
    }
}

/// Places AI Fill's image on `doc` over `rect`, masked to the selection, if the document is still open.
fn place_fill(app: &mut NephclickPhotoLabApp, doc: DocId, rect: Rect, label: &str, image: &[u8]) -> Result<(), String> {
    use base64::Engine as _;
    let index = app.session.documents().iter().position(|d| d.doc.id == doc).ok_or("The document was closed before the fill arrived.")?;
    app.session.set_active(index);
    let has_selection = app.session.active().is_some_and(|d| d.doc.selection.is_some());
    let params = json!({
        "image": base64::engine::general_purpose::STANDARD.encode(image),
        "rect": [rect.x0, rect.y0, rect.width(), rect.height()],
        "name": label,
        "maskToSelection": has_selection,
    });
    app.run(nephclick_photolab_engine::image_layer_cmds::ID, params).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fill_area_surrounds_the_selection_within_the_canvas() {
        let canvas = Rect::new(0, 0, 1000, 800);
        assert_eq!(fill_area(Rect::new(400, 300, 600, 500), canvas), Rect::new(350, 250, 650, 550));
        assert_eq!(fill_area(Rect::new(0, 0, 40, 40), canvas), Rect::new(0, 0, 56, 56));
        assert_eq!(fill_area(Rect::new(0, 0, 1000, 800), canvas), canvas);
    }

    #[test]
    fn shrink_averages_and_sizes() {
        let px = [10u8, 20, 30, 255].repeat(16);
        let out = shrink(&px, 4, 4, 2, 2);
        assert_eq!(out, [10u8, 20, 30, 255].repeat(4));
        assert_eq!(shrink(&px, 4, 4, 3, 1).len(), 12);
    }

    #[test]
    fn requests_without_settings_say_what_is_missing() {
        let mut app = NephclickPhotoLabApp::new(nephclick_photolab_engine::Session::new(), crate::Services::default());
        let ctx = app.ctx().clone();
        app.ai.palette_prompt = "sunset".into();
        start_palette(&mut app, &ctx);
        let note = app.ai.palette_note.clone().unwrap();
        assert!(note.error && note.text.contains("API key"), "{note:?}");
        assert!(!app.ai.is_busy(Task::Palette));
        app.ai.palette_prompt.clear();
        start_palette(&mut app, &ctx);
        assert!(app.ai.palette_note.as_ref().unwrap().text.contains("Type what you want"));
    }

    #[test]
    fn fill_needs_a_document_and_a_selection() {
        let mut app = NephclickPhotoLabApp::new(nephclick_photolab_engine::Session::new(), crate::Services::default());
        let ctx = app.ctx().clone();
        start_fill(&mut app, &ctx);
        assert!(app.ai.fill_note.as_ref().unwrap().text.contains("Open a document"));
        app.run("file.new", json!({"width": 64, "height": 64})).unwrap();
        start_fill(&mut app, &ctx);
        assert!(app.ai.fill_note.as_ref().unwrap().text.contains("selection"));
        app.run("select.rect", json!({"x": 8, "y": 8, "width": 16, "height": 16})).unwrap();
        let (png, _, rect) = prepare_fill(&app).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        assert_eq!(rect, Rect::new(0, 0, 40, 40));
    }

    #[test]
    fn a_fill_answer_lands_as_a_masked_layer() {
        let mut app = NephclickPhotoLabApp::new(nephclick_photolab_engine::Session::new(), crate::Services::default());
        app.run("file.new", json!({"width": 64, "height": 64})).unwrap();
        app.run("select.rect", json!({"x": 8, "y": 8, "width": 16, "height": 16})).unwrap();
        let (png, doc, rect) = prepare_fill(&app).unwrap();
        app.ai.busy.push(Task::Fill);
        app.ai.tx.send(Done::Fill { result: Ok(png), doc, rect, label: "AI Fill: test".into() }).unwrap();
        poll(&mut app);
        assert!(!app.ai.is_busy(Task::Fill));
        let note = app.ai.fill_note.clone().unwrap();
        assert!(!note.error, "{note:?}");
        let st = app.session.active().unwrap();
        let top = st.doc.layers.last().unwrap();
        assert_eq!(top.name, "AI Fill: test");
        assert!(top.mask.is_some());
    }
}
