//! The Gradient Editor window of a Gradient Map adjustment layer, as Photoshop opens it: double-clicking the layer's
//! thumbnail (Layer Content Options) or making a new Gradient Map layer pops it up over the canvas, the way the Solid
//! Color layer pops up the Color Picker. Every change previews on the layer at once; OK keeps it as one History step
//! and Cancel puts the layer back as it was.
//!
//! Top to bottom: the preset picker, the stop bar, Reverse and Dither (the framework's Gradient Map editor); the
//! selected colour stop's colour and location, Add and Delete; the gradient's name and New, which saves it as a
//! preset; the gradient type, Solid or Noise. A Solid gradient's Randomize makes new random colour stops. A Noise
//! gradient is made of random colours between the ranges set per channel (RGB or HSB), as rough as Roughness says,
//! with Restrict Colors keeping them from over-saturating; Randomize draws a new one.

use std::sync::{Mutex, PoisonError};

use craft::color_ui::ui::gradient::{ColorStop, GradientStops};
use craft::draw::ui::gradient::{GradientMap, GradientMapEditor, GradientMapOutput};
use craft::theme::Srgba;
use nephclick_photolab_doc::{Adjustment, DocId, LayerContent, LayerId};
use serde_json::{Value, json};

use crate::NephclickPhotoLabApp;
use crate::adjust_editors::values::{read_gradient_map, write_gradient_map};

/// Most stops a gradient takes (the engine's and Photoshop's limit).
const MAX_STOPS: usize = 64;
/// Stops a Noise gradient is made of.
const NOISE_STOPS: usize = 32;

/// How the gradient is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Solid,
    Noise,
}

/// The colour model a Noise gradient's ranges are in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Model {
    Rgb,
    Hsb,
}

/// A Noise gradient's settings.
#[derive(Clone, Debug, PartialEq)]
struct Noise {
    /// 0..=100: how far each colour may jump from the one before.
    roughness: f32,
    model: Model,
    /// Each channel's range, 0..=1.
    min: [f32; 3],
    max: [f32; 3],
    /// Keep the colours from over-saturating.
    restrict: bool,
    seed: u64,
}

impl Default for Noise {
    fn default() -> Self {
        Noise { roughness: 50.0, model: Model::Rgb, min: [0.0; 3], max: [1.0; 3], restrict: false, seed: 0x9e37_79b9_7f4a_7c15 }
    }
}

/// The open editor.
#[derive(Clone)]
struct Editor {
    doc: DocId,
    layer: LayerId,
    /// The layer's settings when the editor opened: the base the edits are written over, and what Cancel restores.
    original: Value,
    map: GradientMap,
    name: String,
    kind: Kind,
    noise: Noise,
    /// The colour stop the stop fields edit.
    selected: usize,
}

static EDITOR: Mutex<Option<Editor>> = Mutex::new(None);

fn held() -> std::sync::MutexGuard<'static, Option<Editor>> {
    EDITOR.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether the editor is open.
#[cfg(test)]
fn is_open() -> bool {
    held().is_some()
}

/// The active layer's Gradient Map settings, when the active layer is a Gradient Map adjustment layer.
fn active_gradient_map(app: &NephclickPhotoLabApp) -> Option<(DocId, LayerId, Value)> {
    let st = app.session.active()?;
    let id = st.active_layer?;
    let layer = st.doc.layer(id)?;
    let LayerContent::Adjustment(adj @ Adjustment::GradientMap { .. }) = &layer.content else { return None };
    Some((st.doc.id, id, nephclick_photolab_engine::adjust_params::to_params(adj)))
}

/// Opens the editor on the active layer when it is a Gradient Map adjustment layer. `false` for any other layer, so
/// the caller goes on as before.
pub(crate) fn open(app: &mut NephclickPhotoLabApp) -> bool {
    let Some((doc, layer, original)) = active_gradient_map(app) else { return false };
    let map = read_gradient_map(&original);
    let editor = Editor { doc, layer, original, map, name: "Custom".into(), kind: Kind::Solid, noise: Noise::default(), selected: 0 };
    *held() = Some(editor);
    app.ctx().request_repaint();
    true
}

/// The layer's settings with `map` written over `base`.
fn params_of(base: &Value, map: &GradientMap) -> Value {
    let mut out = base.as_object().cloned().unwrap_or_default();
    write_gradient_map(&mut out, map);
    Value::Object(out)
}

/// What the window asked for this pass.
#[derive(Default)]
struct Asked {
    ok: bool,
    cancel: bool,
    import: bool,
    save_preset: bool,
}

/// Draws the editor while it is open.
pub(crate) fn show(app: &mut NephclickPhotoLabApp, ctx: &egui::Context) {
    let Some(mut editor) = held().clone() else { return };
    // The layer went away (closed document, deleted layer, undo): the editor closes with nothing to keep.
    let alive = app.session.active().is_some_and(|st| st.doc.id == editor.doc && st.doc.layer(editor.layer).is_some());
    if !alive {
        app.live_adjust = None;
        *held() = None;
        return;
    }
    let presets = crate::adjust_editors::gradient_presets(app);
    let mut asked = Asked::default();
    let mut open = true;
    egui::Window::new(tl!("Gradient Editor"))
        .id(egui::Id::unique("gradient-map-editor"))
        .open(&mut open)
        .collapsible(true)
        .resizable(true)
        .default_width(crate::theme::DIALOG_WIDTH_LG)
        .default_pos(ctx.content_rect().center() - egui::vec2(230.0, 260.0))
        .show(ctx, |ui| editor_ui(ui, &mut editor, &presets, &mut asked));
    if !open {
        asked.cancel = true;
    }
    if asked.import {
        crate::preset_files_ui::import_gradients_dialog(app);
    }
    if asked.save_preset {
        let stops: Vec<Value> = editor.map.stops.color.iter().map(|s| json!([s.location, hex(s.color)])).collect();
        let name = if editor.name.trim().is_empty() { "Custom".to_owned() } else { editor.name.trim().to_owned() };
        match app.run("gradient.presets.new", json!({"name": name, "stops": stops})) {
            Ok(r) => app.ui.status = crate::i18n::fmt(tl!("Saved the gradient preset {name}"), &[("name", r.get("name").and_then(Value::as_str).unwrap_or(&name))]),
            Err(e) => {
                app.ui.status = e;
                app.ui.status_error = true;
            }
        }
    }
    let params = params_of(&editor.original, &editor.map);
    if asked.cancel {
        app.live_adjust = None;
        *held() = None;
        app.ctx().request_repaint();
        return;
    }
    if asked.ok {
        app.live_adjust = None;
        *held() = None;
        if params != editor.original {
            let mut p = params;
            p["layer"] = json!(editor.layer.0);
            if let Err(e) = app.run("layer.setAdjustment", p) {
                app.ui.status = e;
                app.ui.status_error = true;
            }
        }
        app.ctx().request_repaint();
        return;
    }
    // The canvas shows the edited map on the layer while the editor is open.
    let preview = Some((editor.layer, params));
    if app.live_adjust != preview {
        app.live_adjust = preview;
        app.ctx().request_repaint();
    }
    *held() = Some(editor);
}

fn editor_ui(ui: &mut egui::Ui, editor: &mut Editor, presets: &[craft::draw::ui::gradient::GradientGroup], asked: &mut Asked) {
    // Presets, the stop bar, Reverse and Dither.
    let GradientMapOutput { event, color_requested, import, .. } = GradientMapEditor::new(tl!("Gradient Map"), &editor.map).presets(presets).show(ui);
    if let Some(event) = event {
        event.apply(&mut editor.map);
        editor.kind = Kind::Solid;
    }
    if let Some(stop) = color_requested {
        editor.selected = stop.index;
    }
    asked.import |= import;
    let count = editor.map.stops.color.len();
    editor.selected = editor.selected.min(count.saturating_sub(1));
    ui.separator();

    // Name and New.
    ui.horizontal(|ui| {
        let label = ui.label(tl!("Name:"));
        ui.add(egui::TextEdit::singleline(&mut editor.name).desired_width(crate::theme::FIELD_WIDTH).char_limit(64)).labelled_by(label.id);
        if ui.button(tl!("New")).on_hover_text(tl!("Save this gradient as a preset")).clicked() {
            asked.save_preset = true;
        }
    });

    // Gradient type.
    ui.horizontal(|ui| {
        ui.label(tl!("Gradient Type:"));
        let before = editor.kind;
        ui.radio_value(&mut editor.kind, Kind::Solid, tl!("Solid"));
        ui.radio_value(&mut editor.kind, Kind::Noise, tl!("Noise"));
        if editor.kind == Kind::Noise && before != Kind::Noise {
            editor.map.stops = noise_stops(&editor.noise);
        }
    });
    ui.add_space(4.0);
    match editor.kind {
        Kind::Solid => solid_ui(ui, editor),
        Kind::Noise => noise_ui(ui, editor),
    }

    ui.separator();
    let buttons = crate::theme::dialog_buttons(ui, tl!("OK"), true);
    asked.ok |= buttons.confirm;
    asked.cancel |= buttons.cancel;
}

/// The selected colour stop's fields, Add, Delete and Randomize.
fn solid_ui(ui: &mut egui::Ui, editor: &mut Editor) {
    let mut stops = editor.map.stops.color.clone();
    let count = stops.len();
    ui.horizontal(|ui| {
        let label = ui.label(tl!("Stop:"));
        let mut shown = editor.selected + 1;
        if ui.add(egui::DragValue::new(&mut shown).range(1..=count.max(1)).speed(0.1)).labelled_by(label.id).changed() {
            editor.selected = shown.saturating_sub(1).min(count.saturating_sub(1));
        }
        ui.weak(crate::i18n::fmt(tl!("of {n}"), &[("n", &count.to_string())]));
    });
    let mut changed = false;
    if let Some(stop) = stops.get_mut(editor.selected) {
        ui.horizontal(|ui| {
            ui.label(tl!("Color:"));
            let mut rgb = [stop.color.r, stop.color.g, stop.color.b];
            if ui.color_edit_button_rgb(&mut rgb).changed() {
                stop.color = Srgba::rgb(rgb[0], rgb[1], rgb[2]);
                changed = true;
            }
            let label = ui.label(tl!("Location:"));
            let mut at = stop.location * 100.0;
            if ui.add(egui::DragValue::new(&mut at).range(0.0..=100.0).suffix("%").speed(0.5)).labelled_by(label.id).changed() {
                stop.location = (at / 100.0).clamp(0.0, 1.0);
                changed = true;
            }
        });
    }
    ui.horizontal(|ui| {
        if ui.add_enabled(count < MAX_STOPS, egui::Button::new(tl!("Add Stop"))).clicked() {
            // Halfway between the selected stop and the next, in the colour the gradient has there.
            let at = match (stops.get(editor.selected), stops.get(editor.selected + 1)) {
                (Some(a), Some(b)) => (a.location + b.location) / 2.0,
                (Some(a), None) => (a.location + 1.0) / 2.0,
                _ => 0.5,
            };
            let color = sample(&stops, at);
            stops.push(ColorStop::new(at, color));
            stops.sort_by(|a, b| a.location.total_cmp(&b.location));
            editor.selected = stops.iter().position(|s| (s.location - at).abs() < f32::EPSILON).unwrap_or(0);
            changed = true;
        }
        if ui.add_enabled(count > 2, egui::Button::new(tl!("Delete Stop"))).clicked() && editor.selected < stops.len() {
            stops.remove(editor.selected);
            editor.selected = editor.selected.min(stops.len().saturating_sub(1));
            changed = true;
        }
        if ui.button(tl!("Randomize")).on_hover_text(tl!("New random colour stops")).clicked() {
            editor.noise.seed = next_seed(editor.noise.seed, ui.input(|i| i.time));
            stops = random_solid(editor.noise.seed);
            editor.selected = 0;
            changed = true;
        }
    });
    if changed {
        stops.sort_by(|a, b| a.location.total_cmp(&b.location));
        editor.map.stops = GradientStops::new(stops);
    }
}

/// A Noise gradient's settings and Randomize.
fn noise_ui(ui: &mut egui::Ui, editor: &mut Editor) {
    let before = editor.noise.clone();
    let noise = &mut editor.noise;
    ui.horizontal(|ui| {
        let label = ui.label(tl!("Roughness:"));
        ui.add(egui::Slider::new(&mut noise.roughness, 0.0..=100.0).suffix("%").integer()).labelled_by(label.id);
    });
    ui.horizontal(|ui| {
        ui.label(tl!("Color Model:"));
        egui::ComboBox::from_id_salt("gradient-noise-model")
            .selected_text(if noise.model == Model::Rgb { "RGB" } else { "HSB" })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut noise.model, Model::Rgb, "RGB");
                ui.selectable_value(&mut noise.model, Model::Hsb, "HSB");
            });
    });
    let names = if noise.model == Model::Rgb { [tl!("R"), tl!("G"), tl!("B")] } else { [tl!("H"), tl!("S"), tl!("B")] };
    for (c, name) in names.into_iter().enumerate() {
        ui.horizontal(|ui| {
            let label = ui.label(name);
            let (mut lo, mut hi) = (noise.min[c] * 100.0, noise.max[c] * 100.0);
            ui.add(egui::DragValue::new(&mut lo).range(0.0..=100.0).suffix("%").speed(0.5)).labelled_by(label.id);
            ui.label("–");
            ui.add(egui::DragValue::new(&mut hi).range(0.0..=100.0).suffix("%").speed(0.5)).labelled_by(label.id);
            noise.min[c] = (lo.min(hi) / 100.0).clamp(0.0, 1.0);
            noise.max[c] = (hi.max(lo) / 100.0).clamp(0.0, 1.0);
        });
    }
    ui.checkbox(&mut noise.restrict, tl!("Restrict Colors"));
    if ui.button(tl!("Randomize")).on_hover_text(tl!("Draw a new random gradient")).clicked() {
        noise.seed = next_seed(noise.seed, ui.input(|i| i.time));
    }
    if editor.noise != before {
        editor.map.stops = noise_stops(&editor.noise);
    }
}

// ---------------------------------------------------------------------------------------------
// Random gradients

/// A small deterministic generator (splitmix64): the same seed draws the same gradient.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// 0..1.
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// A new seed from the last one and the clock, so each Randomize differs.
fn next_seed(seed: u64, time: f64) -> u64 {
    let mut r = Rng(seed ^ time.to_bits().rotate_left(17));
    r.next()
}

/// Two to six random colour stops, spread over the gradient.
pub(crate) fn random_solid(seed: u64) -> Vec<ColorStop> {
    let mut r = Rng(seed);
    let n = 2 + (r.next() % 5) as usize;
    let mut stops: Vec<ColorStop> = (0..n)
        .map(|i| {
            let at = if i == 0 {
                0.0
            } else if i + 1 == n {
                1.0
            } else {
                r.unit()
            };
            ColorStop::new(at, hsb_to_rgb(r.unit(), 0.35 + 0.6 * r.unit(), 0.3 + 0.7 * r.unit()))
        })
        .collect();
    stops.sort_by(|a, b| a.location.total_cmp(&b.location));
    stops
}

/// A Noise gradient as colour stops: a random walk in each channel within its range, the step as large as the
/// roughness allows.
fn noise_stops(noise: &Noise) -> GradientStops {
    let mut r = Rng(noise.seed);
    let step = (noise.roughness / 100.0).clamp(0.0, 1.0);
    let span = |c: usize| (noise.max[c] - noise.min[c]).max(0.0);
    let mut v: [f32; 3] = std::array::from_fn(|c| noise.min[c] + span(c) * r.unit());
    let stops = (0..NOISE_STOPS)
        .map(|i| {
            if i > 0 {
                for (c, value) in v.iter_mut().enumerate() {
                    let jump = (r.unit() * 2.0 - 1.0) * step * span(c);
                    *value = (*value + jump).clamp(noise.min[c], noise.max[c]);
                }
            }
            let color = match noise.model {
                Model::Rgb => Srgba::rgb(v[0], v[1], v[2]),
                Model::Hsb => hsb_to_rgb(v[0], v[1], v[2]),
            };
            let color = if noise.restrict { restrict(color) } else { color };
            ColorStop::new(i as f32 / (NOISE_STOPS - 1) as f32, color)
        })
        .collect();
    GradientStops::new(stops)
}

/// The colour with its saturation held under 80%, as Restrict Colors keeps a noise gradient printable.
fn restrict(c: Srgba) -> Srgba {
    let max = c.r.max(c.g).max(c.b);
    let min = c.r.min(c.g).min(c.b);
    if max <= 0.0 || (max - min) / max <= 0.8 {
        return c;
    }
    let floor = max * 0.2;
    let lift = |x: f32| floor + (x - min) / (max - min).max(f32::EPSILON) * (max - floor);
    Srgba::rgb(lift(c.r), lift(c.g), lift(c.b))
}

/// HSB (each 0..1) as RGB.
fn hsb_to_rgb(h: f32, s: f32, b: f32) -> Srgba {
    let h = (h.rem_euclid(1.0)) * 6.0;
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (b * (1.0 - s), b * (1.0 - s * f), b * (1.0 - s * (1.0 - f)));
    let (r, g, bl) = match i as i32 {
        0 => (b, t, p),
        1 => (q, b, p),
        2 => (p, b, t),
        3 => (p, q, b),
        4 => (t, p, b),
        _ => (b, p, q),
    };
    Srgba::rgb(r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), bl.clamp(0.0, 1.0))
}

/// The gradient's colour at `at` (linear between the stops around it).
fn sample(stops: &[ColorStop], at: f32) -> Srgba {
    let Some(first) = stops.first() else { return Srgba::rgb(0.5, 0.5, 0.5) };
    if at <= first.location {
        return first.color;
    }
    for pair in stops.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if at <= b.location {
            let t = if b.location > a.location { (at - a.location) / (b.location - a.location) } else { 0.0 };
            let mix = |x: f32, y: f32| x + (y - x) * t;
            return Srgba::rgb(mix(a.color.r, b.color.r), mix(a.color.g, b.color.g), mix(a.color.b, b.color.b));
        }
    }
    stops.last().map_or(first.color, |s| s.color)
}

fn hex(c: Srgba) -> String {
    nephclick_photolab_engine::adjust_params::hex([c.r, c.g, c.b])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_noise_gradient_stays_in_its_ranges_and_the_seed_repeats_it() {
        let noise = Noise { min: [0.2, 0.0, 0.5], max: [0.4, 1.0, 0.6], ..Noise::default() };
        let a = noise_stops(&noise);
        assert_eq!(a.color.len(), NOISE_STOPS);
        for s in &a.color {
            assert!((0.2..=0.4).contains(&s.color.r) && (0.5..=0.6).contains(&s.color.b), "{:?}", s.color);
        }
        assert_eq!(noise_stops(&noise), a, "the same seed draws the same gradient");
        let other = Noise { seed: next_seed(noise.seed, 1.0), ..noise };
        assert_ne!(noise_stops(&other), a, "Randomize draws another");
    }

    #[test]
    fn zero_roughness_is_one_flat_colour() {
        let noise = Noise { roughness: 0.0, ..Noise::default() };
        let stops = noise_stops(&noise);
        let first = stops.color[0].color;
        assert!(stops.color.iter().all(|s| s.color == first));
    }

    #[test]
    fn random_solid_gradients_span_the_whole_bar() {
        for seed in 0..50u64 {
            let stops = random_solid(seed);
            assert!((2..=6).contains(&stops.len()));
            assert_eq!(stops.first().map(|s| s.location), Some(0.0));
            assert_eq!(stops.last().map(|s| s.location), Some(1.0));
            assert!(stops.windows(2).all(|w| w[0].location <= w[1].location));
        }
    }

    #[test]
    fn restrict_colors_lowers_saturation() {
        let c = restrict(Srgba::rgb(1.0, 0.0, 0.0));
        let (max, min) = (c.r.max(c.g).max(c.b), c.r.min(c.g).min(c.b));
        assert!((max - min) / max <= 0.81, "{c:?}");
    }

    #[test]
    fn the_editor_opens_on_a_gradient_map_layer_and_previews_live() {
        let mut app = NephclickPhotoLabApp::new(nephclick_photolab_engine::Session::new(), crate::Services::default());
        app.run("file.new", json!({"width": 16, "height": 16})).unwrap();
        assert!(!open(&mut app), "no Gradient Map layer yet");
        app.run("layer.newAdjustmentLayer.gradientMap", json!({})).unwrap();
        assert!(open(&mut app), "opens on the Gradient Map layer");
        let mut h = egui_kittest::Harness::builder().with_context(app.ctx().clone()).with_size(egui::vec2(900.0, 700.0)).build_ui_state(
            |ui: &mut egui::Ui, app: &mut Box<NephclickPhotoLabApp>| {
                let ctx = ui.ctx().clone();
                show(app, &ctx);
            },
            app,
        );
        h.run_steps(2);
        assert!(h.state().live_adjust.is_some(), "the layer previews the editor's map");
        // Cancel (Esc): the layer keeps its gradient and the preview goes.
        if let Some(e) = held().as_mut() {
            e.map.stops = GradientStops::new(random_solid(7));
        }
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        let app = h.state();
        assert!(!is_open() && app.live_adjust.is_none());
    }
}
