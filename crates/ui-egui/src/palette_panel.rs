//! Window › Color Palettes: five colours, five gradients and five patterns, from a sentence (AI) or from the
//! built-in themed sets (fruits, plants, animals, ocean, sky, earth, gems, food, seasons), applied to the document.
//!
//! "Apply as" picks what a click on a colour, gradient or pattern does:
//! - **Fill**: a colour fills the selection, or the whole layer (`edit.fill`); a gradient or pattern becomes a fill
//!   layer, masked to the selection when there is one (`layer.newFillLayer.*`).
//! - **Stroke**: a colour strokes the selection (`edit.stroke`), or the layer's edge as a Stroke style.
//! - **Layer Style**: Color, Gradient or Pattern Overlay on the active layer.
//! - **Text**: a colour colours the selected characters while typing, else the selected type layers
//!   (`type.setStyle`); a gradient or pattern becomes an overlay on them.
//!
//! Every action is an engine command, so each is one undoable step. A pattern is first defined as a pattern preset
//! (`edit.definePattern`, from its tile painted in a scratch document), once per pattern.

use std::collections::HashMap;

use base64::Engine as _;
use egui::{Color32, CornerRadius, RichText, Sense, Stroke, StrokeKind, Vec2};
use nephclick_photolab_ai::palette::{Gradient, Palette, Pattern, Rgb, THEMES, categories, search};
use serde_json::{Value, json};

use crate::NephclickPhotoLabApp;
use crate::theme::Tokens;

/// The panel's padding.
const PAD: f32 = 14.0;

/// The panel's id in the dock, and its Window menu command.
pub const PANEL: &str = "colorPalettes";
pub const SHOW: &str = "window.panel.colorPalettes";

/// What a click on a colour, gradient or pattern does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ApplyAs {
    #[default]
    Fill,
    Stroke,
    Style,
    Text,
}

impl ApplyAs {
    const ALL: [ApplyAs; 4] = [ApplyAs::Fill, ApplyAs::Stroke, ApplyAs::Style, ApplyAs::Text];
    fn label(self) -> &'static str {
        match self {
            ApplyAs::Fill => "Fill",
            ApplyAs::Stroke => "Stroke",
            ApplyAs::Style => "Layer Style",
            ApplyAs::Text => "Text",
        }
    }
}

/// The panel's view state.
#[derive(Default)]
pub struct PaletteUi {
    pub apply_as: ApplyAs,
    /// Create from a Colour: the base colour and the harmony rule (`None`: not chosen yet, the foreground and
    /// Complementary).
    base: Option<[u8; 3]>,
    harmony: Option<nephclick_photolab_ai::harmony::Harmony>,
    /// Pattern presets already defined, by [`pattern_key`].
    defined: HashMap<String, String>,
    /// Pattern tile textures, by [`pattern_key`].
    tiles: HashMap<String, egui::TextureHandle>,
}

/// Menu ids handled here.
pub fn handles(id: &str) -> bool {
    id == SHOW
}

/// The Window menu check mark.
pub fn checked(app: &NephclickPhotoLabApp, id: &str) -> Option<bool> {
    (id == SHOW).then(|| crate::panel_docking::is_shown(app, PANEL))
}

/// Window › Color Palettes.
pub fn menu(app: &mut NephclickPhotoLabApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    if id != SHOW {
        return None;
    }
    let show = params.get("show").and_then(Value::as_bool);
    Some(crate::panel_docking::show_panel(app, PANEL, show).map(|visible| json!({"visible": visible})))
}

/// What a click asked for, run after the panel is drawn.
enum Act {
    Generate,
    Load(&'static str),
    Color(Rgb),
    Foreground(Rgb),
    Gradient(Gradient),
    Pattern(Pattern),
}

/// The panel body.
pub fn body(app: &mut NephclickPhotoLabApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let mut acts = Vec::new();
    // The panel opens on a starter theme, so its colours are usable at once.
    if app.ai.palette.is_none() {
        app.ai.palette = THEMES.iter().find(|th| th.name == "Sunset").map(|th| th.palette());
    }
    ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
    // One content width for every row: the panel's visible width less its padding (a scrolled body would otherwise
    // grow to its widest row and push the swatches past the edge).
    let width = (ui.clip_rect().width().min(ui.available_width()) - 2.0 * PAD).max(160.0);
    egui::Frame::new().inner_margin(egui::Margin::symmetric(PAD as i8, 12)).show(ui, |ui| {
        ui.set_width(width);
        ui.set_max_width(width);
        prompt_row(app, ui, &t, width, &mut acts);
        harmony_row(app, ui, &t, width);
        browse(app, ui, &t, width, &mut acts);
        ui.add_space(4.0);
        ui.separator();
        match app.ai.palette.clone() {
            Some(p) => palette(app, ui, &t, width, &p, &mut acts),
            None => {
                ui.label(RichText::new(tl!("Describe a palette above, or pick one of the themes.")).color(t.text_faint).size(12.0));
            }
        }
    });
    let ctx = ui.ctx().clone();
    for act in acts {
        let result = run(app, &ctx, act);
        if let Err(e) = result {
            app.ui.status = e;
            app.ui.status_error = true;
        }
    }
}

fn section(ui: &mut egui::Ui, t: &Tokens, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).size(11.0).strong().color(t.text_dim));
}

/// The AI prompt: a sentence in, a palette out.
fn prompt_row(app: &mut NephclickPhotoLabApp, ui: &mut egui::Ui, t: &Tokens, width: f32, acts: &mut Vec<Act>) {
    let busy = app.ai.is_busy(crate::ai_ui::Task::Palette);
    section(ui, t, tl!("GENERATE WITH AI"));
    ui.horizontal(|ui| {
        let button_w = 84.0;
        let field_w = (width - button_w - ui.spacing().item_spacing.x).max(80.0);
        let edit = egui::TextEdit::singleline(&mut app.ai.palette_prompt)
            .hint_text(tl!("e.g. calm beach at sunrise"))
            .desired_width(field_w)
            .min_size(Vec2::new(field_w, 28.0))
            .align(egui::Align2::LEFT_CENTER)
            .margin(egui::Margin::symmetric(8, 4))
            .char_limit(nephclick_photolab_ai::MAX_PROMPT);
        let resp = ui.add_enabled(!busy, edit);
        let submitted = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let label = if busy { tl!("Working…") } else { tl!("Generate") };
        let b = egui::Button::new(RichText::new(label).color(t.primary_text).strong())
            .fill(t.primary_bg)
            .corner_radius(CornerRadius::same(t.radius.round().clamp(2.0, 12.0) as u8))
            .min_size(Vec2::new(button_w, 28.0));
        if ui.add_enabled(!busy && !app.ai.palette_prompt.trim().is_empty(), b).clicked() || (submitted && !busy) {
            acts.push(Act::Generate);
        }
    });
    if let Some(note) = &app.ai.palette_note {
        ui.label(RichText::new(&note.text).size(11.0).color(if note.error { t.danger } else { t.text_dim }));
    }
}

/// The themed sets: a search field, the categories as chips, and the matching themes.
/// Owner of the panel's screen colour pick.
const PICK_OWNER: &str = "palette-base-colour";

/// Create from a Colour: a base colour (its swatch, or the eyedropper, which picks anywhere on the screen, other
/// applications included) and one of the thirteen harmony rules make the palette's five colours, with gradients and
/// patterns made from them.
fn harmony_row(app: &mut NephclickPhotoLabApp, ui: &mut egui::Ui, t: &Tokens, width: f32) {
    use nephclick_photolab_ai::harmony::Harmony;
    section(ui, t, tl!("CREATE FROM A COLOR"));
    if let Some(picked) = crate::screen_picker::take(app, PICK_OWNER) {
        let byte = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
        app.palette_ui.base = Some([byte(picked.r), byte(picked.g), byte(picked.b)]);
    }
    let fg = app.session.tools.foreground;
    let byte = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut base = app.palette_ui.base.unwrap_or([byte(fg[0]), byte(fg[1]), byte(fg[2])]);
    let mut rule = app.palette_ui.harmony.unwrap_or(Harmony::Complementary);
    let mut generate = false;
    ui.horizontal(|ui| {
        if ui.color_edit_button_srgb(&mut base).on_hover_text(tl!("Base colour")).changed() {
            app.palette_ui.base = Some(base);
        }
        let pick = ui.button(tl!("Pick from Screen")).on_hover_text(tl!("Pick a colour from anywhere on the screen, other apps included"));
        if pick.clicked() {
            crate::screen_picker::hold(app, ui.ctx(), PICK_OWNER, craft::color_ui::ui::ScreenPickGesture::Clicked);
        }
    });
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("palette-harmony")
            .width((width - 96.0).max(120.0))
            .selected_text(tl!(rule.name()))
            .show_ui(ui, |ui| {
                for h in Harmony::ALL {
                    if ui.selectable_value(&mut rule, h, tl!(h.name())).clicked() {
                        app.palette_ui.harmony = Some(h);
                        generate = true;
                    }
                }
            });
        if ui.button(tl!("Generate")).clicked() {
            generate = true;
        }
    });
    if generate {
        let colors = rule.colors(nephclick_photolab_ai::palette::Rgb(base));
        app.ai.palette = Some(nephclick_photolab_ai::palette::Palette::from_colors(rule.name(), &colors));
    }
}

fn browse(app: &mut NephclickPhotoLabApp, ui: &mut egui::Ui, t: &Tokens, width: f32, acts: &mut Vec<Act>) {
    section(ui, t, tl!("BROWSE THEMES"));
    ui.add(
        egui::TextEdit::singleline(&mut app.ai.theme_query)
            .hint_text(tl!("Search fruits, plants, animals…"))
            .desired_width(width)
            .min_size(Vec2::new(width, 28.0))
            .align(egui::Align2::LEFT_CENTER)
            .margin(egui::Margin::symmetric(8, 4)),
    );
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        for cat in categories() {
            let on = app.ai.theme_query.eq_ignore_ascii_case(cat);
            let chip = egui::Button::new(RichText::new(tl!(cat)).size(11.0).color(if on { t.primary_text } else { t.text_dim }))
                .fill(if on { t.primary_bg } else { t.field })
                .stroke(Stroke::new(1.0, if on { t.primary_bg } else { t.field_border }))
                .corner_radius(CornerRadius::same(255));
            if ui.add(chip).clicked() {
                app.ai.theme_query = if on { String::new() } else { cat.to_string() };
            }
        }
    });
    let found = search(&app.ai.theme_query);
    if app.ai.theme_query.trim().is_empty() {
        return;
    }
    if found.is_empty() {
        ui.label(RichText::new(tl!("No theme matches. Try Generate with AI.")).size(11.0).color(t.text_faint));
        return;
    }
    egui::ScrollArea::vertical().id_salt("palette-themes").max_height(168.0).show(ui, |ui| {
        for theme in found {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::click());
            let p = ui.painter();
            if resp.hovered() {
                p.rect_filled(rect, t.radius_sm, t.hover);
            }
            p.text(rect.left_center() + Vec2::new(6.0, 0.0), egui::Align2::LEFT_CENTER, theme.name, egui::FontId::proportional(12.0), t.text);
            let chip = 16.0;
            let mut x = rect.right() - 6.0 - 5.0 * (chip + 3.0);
            for c in theme.colors.iter().filter_map(|c| Rgb::parse(c)) {
                let r = egui::Rect::from_min_size(egui::pos2(x, rect.center().y - chip / 2.0), Vec2::splat(chip));
                p.rect(r, 3.0, rgb(c), Stroke::new(1.0, t.card_border), StrokeKind::Inside);
                x += chip + 3.0;
            }
            if resp.clicked() {
                acts.push(Act::Load(theme.name));
            }
        }
    });
}

fn rgb(c: Rgb) -> Color32 {
    Color32::from_rgb(c.0[0], c.0[1], c.0[2])
}

/// The palette shown: its name, "Apply as", and its colours, gradients and patterns.
fn palette(app: &mut NephclickPhotoLabApp, ui: &mut egui::Ui, t: &Tokens, width: f32, p: &Palette, acts: &mut Vec<Act>) {
    ui.label(RichText::new(&p.name).size(15.0).strong().color(t.text));
    section(ui, t, tl!("APPLY AS"));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
        for mode in ApplyAs::ALL {
            let on = app.palette_ui.apply_as == mode;
            let b = egui::Button::new(RichText::new(tl!(mode.label())).size(11.0).color(if on { t.primary_text } else { t.text }))
                .fill(if on { t.primary_bg } else { t.field })
                .stroke(Stroke::new(1.0, if on { t.primary_bg } else { t.field_border }))
                .corner_radius(CornerRadius::same(t.radius_sm.round().clamp(2.0, 8.0) as u8))
                .min_size(Vec2::new(0.0, 26.0));
            if ui.add(b).clicked() {
                app.palette_ui.apply_as = mode;
            }
        }
    });
    ui.label(RichText::new(hint(app)).size(11.0).color(t.text_faint));
    let w = width;
    let cell = ((w - 4.0 * 6.0) / 5.0).floor().max(18.0);

    section(ui, t, tl!("COLORS"));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for c in &p.colors {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(cell, cell), Sense::click());
            let hover = resp.hovered();
            ui.painter().rect(rect, t.radius.clamp(2.0, 8.0), rgb(*c), Stroke::new(if hover { 2.0 } else { 1.0 }, if hover { t.accent } else { t.card_border }), StrokeKind::Inside);
            let resp = resp.on_hover_text(format!("{}\n{}", c.hex(), tl!("Click to apply · Right-click to set the foreground colour")));
            if resp.clicked() {
                acts.push(Act::Color(*c));
            }
            if resp.secondary_clicked() {
                acts.push(Act::Foreground(*c));
            }
        }
    });

    section(ui, t, tl!("GRADIENTS"));
    let text_or_style_or_fill = app.palette_ui.apply_as != ApplyAs::Stroke;
    for g in &p.gradients {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 24.0), if text_or_style_or_fill { Sense::click() } else { Sense::hover() });
        paint_gradient(ui, rect, g, t, resp.hovered() && text_or_style_or_fill);
        let resp = resp.on_hover_text(if text_or_style_or_fill { g.name.clone() } else { tl!("Strokes take a colour").to_string() });
        if resp.clicked() {
            acts.push(Act::Gradient(g.clone()));
        }
    }

    section(ui, t, tl!("PATTERNS"));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for pat in &p.patterns {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(cell, cell), if text_or_style_or_fill { Sense::click() } else { Sense::hover() });
            let tex = tile_texture(app, ui.ctx(), pat);
            let radius = t.radius.clamp(2.0, 8.0);
            // The tile repeats over the swatch at its own size.
            let uv_scale = cell / (pat.tile_size() as f32).max(1.0);
            ui.painter().image(tex.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(uv_scale, uv_scale)), Color32::WHITE);
            let hover = resp.hovered() && text_or_style_or_fill;
            ui.painter().rect_stroke(rect, radius, Stroke::new(if hover { 2.0 } else { 1.0 }, if hover { t.accent } else { t.card_border }), StrokeKind::Inside);
            let resp = resp.on_hover_text(if text_or_style_or_fill { pat.name.clone() } else { tl!("Strokes take a colour").to_string() });
            if resp.clicked() {
                acts.push(Act::Pattern(pat.clone()));
            }
        }
    });
}

/// What a click does now, in a line.
fn hint(app: &NephclickPhotoLabApp) -> &'static str {
    let selection = app.session.active().is_some_and(|d| d.doc.selection.is_some());
    match (app.palette_ui.apply_as, selection) {
        (ApplyAs::Fill, true) => tl!("Fills the selection."),
        (ApplyAs::Fill, false) => tl!("Fills the active layer (gradients and patterns add a fill layer)."),
        (ApplyAs::Stroke, true) => tl!("Strokes the selection with a colour."),
        (ApplyAs::Stroke, false) => tl!("Adds a Stroke style to the active layer."),
        (ApplyAs::Style, _) => tl!("Adds an overlay style to the active layer."),
        (ApplyAs::Text, _) => tl!("Colours the selected text, or the selected type layers."),
    }
}

fn paint_gradient(ui: &egui::Ui, rect: egui::Rect, g: &Gradient, t: &Tokens, hover: bool) {
    let mut mesh = egui::Mesh::default();
    let n = g.stops.len().max(2);
    for (i, c) in g.stops.iter().enumerate() {
        let x = rect.left() + rect.width() * i as f32 / (n - 1) as f32;
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(egui::pos2(x, rect.top()), rgb(*c));
        mesh.colored_vertex(egui::pos2(x, rect.bottom()), rgb(*c));
        if i > 0 {
            mesh.add_triangle(base - 2, base - 1, base);
            mesh.add_triangle(base - 1, base, base + 1);
        }
    }
    ui.painter().add(egui::Shape::mesh(mesh));
    ui.painter().rect_stroke(rect, t.radius_sm.clamp(1.0, 6.0), Stroke::new(if hover { 2.0 } else { 1.0 }, if hover { t.accent } else { t.card_border }), StrokeKind::Inside);
}

/// A pattern's identity: its shape, colours and scale.
fn pattern_key(p: &Pattern) -> String {
    format!("{}-{}-{}-{}", p.kind.name(), p.ground.hex(), p.ink.hex(), p.scale)
}

fn tile_texture(app: &mut NephclickPhotoLabApp, ctx: &egui::Context, p: &Pattern) -> egui::TextureHandle {
    let key = pattern_key(p);
    if let Some(tex) = app.palette_ui.tiles.get(&key) {
        return tex.clone();
    }
    let (n, px) = p.render();
    let image = egui::ColorImage::from_rgba_unmultiplied([n as usize, n as usize], &px);
    let opts = egui::TextureOptions { wrap_mode: egui::TextureWrapMode::Repeat, ..egui::TextureOptions::NEAREST };
    let tex = ctx.load_texture(format!("palette-tile-{key}"), image, opts);
    app.palette_ui.tiles.insert(key, tex.clone());
    tex
}

fn run(app: &mut NephclickPhotoLabApp, ctx: &egui::Context, act: Act) -> Result<(), String> {
    match act {
        Act::Generate => {
            crate::ai_ui::start_palette(app, ctx);
            Ok(())
        }
        Act::Load(name) => {
            if let Some(theme) = THEMES.iter().find(|th| th.name == name) {
                app.ai.palette = Some(theme.palette());
                app.ai.palette_note = None;
            }
            Ok(())
        }
        Act::Foreground(c) => app.run("tools.setColors", json!({"foreground": c.hex()})).map(|_| ()),
        Act::Color(c) => apply_color(app, c),
        Act::Gradient(g) => apply_gradient(app, &g),
        Act::Pattern(p) => apply_pattern(app, &p),
    }
}

fn has_selection(app: &NephclickPhotoLabApp) -> bool {
    app.session.active().is_some_and(|d| d.doc.selection.is_some())
}

/// The selected characters of the type layer being edited: `(layer, start, end)`.
fn text_range(app: &NephclickPhotoLabApp) -> Option<(u64, usize, usize)> {
    app.ui.text_edit.as_ref()?;
    let (layer, range) = crate::type_tool::target(app)?;
    let [a, b] = range?;
    Some((layer, a, b))
}

/// The ids of the selected type layers.
fn selected_text_layers(app: &NephclickPhotoLabApp) -> Vec<u64> {
    let Some(st) = app.session.active() else { return Vec::new() };
    st.selected_layers()
        .into_iter()
        .filter(|id| st.doc.layer(*id).is_some_and(|l| matches!(l.content, nephclick_photolab_doc::LayerContent::Text(_))))
        .map(|id| id.0)
        .collect()
}

fn apply_color(app: &mut NephclickPhotoLabApp, c: Rgb) -> Result<(), String> {
    app.session.active().ok_or_else(|| tl!("Open a document first.").to_string())?;
    let hex = c.hex();
    match app.palette_ui.apply_as {
        ApplyAs::Fill => app.run("edit.fill", json!({"contents": "color", "color": hex})).map(|_| ()),
        ApplyAs::Stroke if has_selection(app) => app.run("edit.stroke", json!({"width": 4, "color": hex, "location": "center"})).map(|_| ()),
        ApplyAs::Stroke => app.run("layer.layerStyle.stroke", json!({"color": hex, "size": 3, "position": "outside"})).map(|_| ()),
        ApplyAs::Style => app.run("layer.layerStyle.colorOverlay", json!({"color": hex})).map(|_| ()),
        ApplyAs::Text => {
            if let Some((layer, a, b)) = text_range(app) {
                return app.run("type.setStyle", json!({"layer": layer, "range": [a, b], "color": hex})).map(|_| ());
            }
            let layers = selected_text_layers(app);
            if layers.is_empty() {
                return Err(tl!("Select a type layer, or select characters while typing.").to_string());
            }
            app.run("type.setStyle", json!({"layers": layers, "color": hex})).map(|_| ())
        }
    }
}

fn stops(g: &Gradient) -> Vec<Value> {
    let n = g.stops.len().max(2) - 1;
    g.stops.iter().enumerate().map(|(i, c)| json!([i as f64 / n as f64, c.hex()])).collect()
}

fn apply_gradient(app: &mut NephclickPhotoLabApp, g: &Gradient) -> Result<(), String> {
    app.session.active().ok_or_else(|| tl!("Open a document first.").to_string())?;
    let angle = g.angle;
    match app.palette_ui.apply_as {
        ApplyAs::Fill => app.run("layer.newFillLayer.gradient", json!({"stops": stops(g), "angle": angle, "style": "linear"})).map(|_| ()),
        ApplyAs::Stroke => Err(tl!("Strokes take a colour: pick one of the colours.").to_string()),
        ApplyAs::Style => app.run("layer.layerStyle.gradientOverlay", json!({"stops": stops(g), "angle": angle})).map(|_| ()),
        ApplyAs::Text => {
            let layers = text_layers_for_style(app)?;
            for l in layers {
                app.run("layer.layerStyle.gradientOverlay", json!({"layer": l, "stops": stops(g), "angle": angle}))?;
            }
            Ok(())
        }
    }
}

/// The type layers a gradient or pattern overlay goes on: the one being edited, else the selected ones.
fn text_layers_for_style(app: &NephclickPhotoLabApp) -> Result<Vec<u64>, String> {
    if let Some(ed) = app.ui.text_edit.as_ref() {
        return Ok(vec![ed.layer]);
    }
    let layers = selected_text_layers(app);
    if layers.is_empty() {
        return Err(tl!("Select a type layer first.").to_string());
    }
    Ok(layers)
}

fn apply_pattern(app: &mut NephclickPhotoLabApp, p: &Pattern) -> Result<(), String> {
    app.session.active().ok_or_else(|| tl!("Open a document first.").to_string())?;
    if app.palette_ui.apply_as == ApplyAs::Stroke {
        return Err(tl!("Strokes take a colour: pick one of the colours.").to_string());
    }
    let id = define_pattern(app, p)?;
    match app.palette_ui.apply_as {
        ApplyAs::Fill => app.run("layer.newFillLayer.pattern", json!({"pattern": id})).map(|_| ()),
        ApplyAs::Style => app.run("layer.layerStyle.patternOverlay", json!({"pattern": id})).map(|_| ()),
        ApplyAs::Text => {
            for l in text_layers_for_style(app)? {
                app.run("layer.layerStyle.patternOverlay", json!({"layer": l, "pattern": id}))?;
            }
            Ok(())
        }
        ApplyAs::Stroke => Ok(()),
    }
}

/// The pattern preset of `p`, defined the first time: its tile is painted into a scratch document, defined with
/// `edit.definePattern`, and the scratch document closed. The active document is active again afterwards.
fn define_pattern(app: &mut NephclickPhotoLabApp, p: &Pattern) -> Result<String, String> {
    let key = pattern_key(p);
    if let Some(id) = app.palette_ui.defined.get(&key).cloned()
        && app.run("pattern.list", json!({})).is_ok_and(|list| list.as_array().is_some_and(|a| a.iter().any(|e| e["id"].to_string().trim_matches('"') == id)))
    {
        return Ok(id);
    }
    let (n, px) = p.render();
    let image = nephclick_photolab_codecs::Image::from_u8(n, n, nephclick_photolab_codecs::ChannelLayout::Rgba, px)
        .and_then(|img| nephclick_photolab_codecs::encode(&img, nephclick_photolab_codecs::Format::Png, &Default::default()))
        .map_err(|e| e.to_string())?;
    let back = app.session.active_index();
    let made = app.run("file.new", json!({"width": n, "height": n, "background": "transparent", "name": "Pattern"}))?;
    let scratch = made.get("document").and_then(Value::as_u64).ok_or("the scratch document did not open")?;
    let defined = (|| {
        app.run(nephclick_photolab_engine::image_layer_cmds::ID, json!({"image": base64::engine::general_purpose::STANDARD.encode(&image), "rect": [0, 0, n, n]}))?;
        app.run("edit.definePattern", json!({"name": format!("{} ({})", p.name, p.kind.name())}))
    })();
    let _ = app.run("file.close", json!({"document": scratch, "save": "discard"}));
    if let Some(i) = back {
        app.session.set_active(i);
    }
    let id = defined?.get("pattern").map(|v| v.to_string().trim_matches('"').to_string()).ok_or("the pattern was not defined")?;
    app.palette_ui.defined.insert(key, id.clone());
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_doc() -> Box<NephclickPhotoLabApp> {
        let mut app = NephclickPhotoLabApp::new(nephclick_photolab_engine::Session::new(), crate::Services::default());
        app.run("file.new", json!({"width": 64, "height": 48})).unwrap();
        app.run("layer.new.layer", json!({})).unwrap();
        app.ai.palette = THEMES.iter().find(|t| t.name == "Mango").map(|t| t.palette());
        app
    }

    fn top_layer(app: &NephclickPhotoLabApp) -> nephclick_photolab_doc::Layer {
        app.session.active().unwrap().doc.layers.last().unwrap().clone()
    }

    #[test]
    fn every_mode_applies_a_colour_or_says_why_not() {
        let mut app = app_with_doc();
        let c = Rgb([255, 0, 0]);
        for mode in [ApplyAs::Fill, ApplyAs::Stroke, ApplyAs::Style] {
            app.palette_ui.apply_as = mode;
            apply_color(&mut app, c).unwrap_or_else(|e| panic!("{mode:?}: {e}"));
        }
        app.palette_ui.apply_as = ApplyAs::Text;
        assert!(apply_color(&mut app, c).unwrap_err().contains("type layer"));
    }

    #[test]
    fn gradients_and_patterns_make_fill_layers_and_overlays() {
        let mut app = app_with_doc();
        let p = app.ai.palette.clone().unwrap();
        let before = app.session.active().unwrap().doc.layers.len();
        apply_gradient(&mut app, &p.gradients[0]).unwrap();
        apply_pattern(&mut app, &p.patterns[0]).unwrap();
        assert_eq!(app.session.active().unwrap().doc.layers.len(), before + 2);
        assert_eq!(app.session.documents().len(), 1, "the scratch document is closed");
        app.palette_ui.apply_as = ApplyAs::Style;
        apply_gradient(&mut app, &p.gradients[1]).unwrap();
        apply_pattern(&mut app, &p.patterns[0]).unwrap();
        assert!(!top_layer(&app).effects.items.is_empty());
        app.palette_ui.apply_as = ApplyAs::Stroke;
        assert!(apply_gradient(&mut app, &p.gradients[0]).is_err());
        assert!(apply_pattern(&mut app, &p.patterns[0]).is_err());
    }

    #[test]
    fn nothing_open_is_an_error_not_a_crash() {
        let mut app = NephclickPhotoLabApp::new(nephclick_photolab_engine::Session::new(), crate::Services::default());
        let p = THEMES[0].palette();
        assert!(apply_color(&mut app, p.colors[0]).is_err());
        assert!(apply_gradient(&mut app, &p.gradients[0]).is_err());
        assert!(apply_pattern(&mut app, &p.patterns[0]).is_err());
    }

    #[test]
    fn the_menu_item_shows_the_panel() {
        let mut app = NephclickPhotoLabApp::new(nephclick_photolab_engine::Session::new(), crate::Services::default());
        assert!(handles(SHOW));
        let r = menu(&mut app, SHOW, &json!({"show": true})).unwrap().unwrap();
        assert_eq!(r, json!({"visible": true}));
        assert_eq!(checked(&app, SHOW), Some(true));
    }
}
