//! The Home screen: one centred column of cards in the theme's colours. Start a new document or open one, generate a
//! project idea with AI from a sentence, and reopen a recent file. Also the Home rule the options bar's button, the
//! rail's Home link and `window.home` share.
//!
//! Every colour, radius and stroke comes from `theme::Tokens`; nothing is hard-coded. The recent files' pictures
//! come from the same thumbnail store and source the framework's start page used (`home_recent_ui`).

use craft::behavior::IconId;
use craft::ui::contract::{Show as _, Props as _};
use craft::ui::display::Icon;
use craft::ui::primitives::{IconSpec, Priority, TextureState, ThumbRequest};
use craft::ui::shell::{StartAction, ThumbKey};
use egui::{Align, Color32, CornerRadius, Layout, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};
use serde_json::json;

use super::NephclickPhotoLabApp;
use crate::theme::Tokens;

/// Recent files listed on the Home screen.
const HOME_RECENT: usize = 12;
/// The column's widest width, and the gutter beside it.
const MAX_WIDTH: f32 = 960.0;
const GUTTER: f32 = 32.0;
/// Space between cards, and inside them.
const GAP: f32 = 20.0;
const PAD: f32 = 24.0;
/// A recent file's tile: its picture and the name under it.
const TILE: Vec2 = Vec2::new(176.0, 132.0);
const TILE_LABEL: f32 = 40.0;

/// Ideas the AI card offers as a start.
const SUGGESTIONS: [&str; 4] = ["Poster for a summer music festival", "Instagram post for a coffee shop", "Album cover, dreamy and retro", "Banner for a tech product launch"];

/// What the user asked for on the screen this pass, run once the screen is drawn.
enum Act {
    New,
    Open,
    Recent(StartAction),
    Reveal(std::path::PathBuf),
    Generate,
    CreateIdea,
    UsePalette,
    AiSettings,
}

/// The Home screen in the document area. The recent list is empty on the web, which has no paths to reopen, and with
/// Show Home Recent Files off.
pub(super) fn start_screen(app: &mut NephclickPhotoLabApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let mut acts = Vec::new();
    let full = ui.available_rect_before_wrap();
    ui.painter().rect_filled(full, 0.0, t.canvas);
    egui::ScrollArea::vertical().id_salt("home-screen").auto_shrink([false, false]).show(ui, |ui| {
        let width = (ui.available_width() - 2.0 * GUTTER).clamp(320.0, MAX_WIDTH);
        let side = ((ui.available_width() - width) / 2.0).max(0.0);
        ui.add_space(40.0);
        ui.horizontal(|ui| {
            ui.add_space(side);
            ui.vertical(|ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing = Vec2::new(12.0, 12.0);
                header(ui, &t);
                ui.add_space(8.0);
                start_cards(ui, &t, width, &mut acts);
                ui.add_space(GAP - 12.0);
                idea_card(app, ui, &t, &mut acts);
                ui.add_space(GAP - 12.0);
                recent_card(app, ui, &t, width, &mut acts);
                ui.add_space(8.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(tl!("Drop an image or PSD anywhere to open it.")).color(t.text_faint).size(12.0));
                });
                ui.add_space(32.0);
            });
        });
    });
    let ctx = ui.ctx().clone();
    for act in acts {
        run(app, &ctx, act);
    }
}

fn run(app: &mut NephclickPhotoLabApp, ctx: &egui::Context, act: Act) {
    match act {
        Act::New => {
            crate::new_doc_ui::open(app);
        }
        Act::Open => {
            crate::home_recent_ui::act(app, &StartAction::Open);
        }
        Act::Recent(action) => {
            crate::home_recent_ui::act(app, &action);
        }
        Act::Reveal(path) => reveal(app, &path),
        Act::Generate => crate::ai_ui::start_idea(app, ctx),
        Act::CreateIdea => create_idea(app),
        Act::UsePalette => {
            if let Some(idea) = &app.ai.idea {
                app.ai.palette = Some(idea.palette.clone());
            }
            let _ = crate::menus::invoke(app, ctx, crate::palette_panel::SHOW, json!({}));
        }
        Act::AiSettings => {
            let _ = crate::menus::invoke(app, ctx, "edit.preferences.ai", json!({}));
        }
    }
}

/// Creates the document the idea describes, and puts its palette in the Color Palettes panel.
fn create_idea(app: &mut NephclickPhotoLabApp) {
    let Some(idea) = app.ai.idea.clone() else { return };
    let params = json!({"width": idea.width, "height": idea.height, "name": idea.name, "background": idea.background.hex()});
    match app.run("file.new", params) {
        Ok(_) => {
            app.ai.palette = Some(idea.palette);
            app.ui.status = tl!("Created from your idea. Its colours are in the Color Palettes panel.").into();
            app.ui.status_error = false;
        }
        Err(e) => {
            app.ui.status = e;
            app.ui.status_error = true;
        }
    }
}

fn header(ui: &mut egui::Ui, t: &Tokens) {
    ui.label(RichText::new(tl!("Welcome to Nephclick PhotoLab")).size(26.0).strong().color(t.text));
    ui.label(RichText::new(tl!("Start something new, pick up where you left off, or let AI suggest an idea.")).size(14.0).color(t.text_dim));
}

/// A card's frame: the theme's card surface, border and large radius, with even padding.
fn card(t: &Tokens) -> egui::Frame {
    egui::Frame::new()
        .fill(t.card)
        .stroke(Stroke::new(1.0, t.card_border))
        .corner_radius(CornerRadius::same(t.radius_lg.round().clamp(4.0, 24.0) as u8))
        .inner_margin(egui::Margin::same(PAD as i8))
}

/// New Document (the primary action) and Open, side by side.
fn start_cards(ui: &mut egui::Ui, t: &Tokens, width: f32, acts: &mut Vec<Act>) {
    let w = ((width - GAP) / 2.0).floor();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        if action_card(ui, t, w, true, "new_file", tl!("New Document"), tl!("Choose a size and start creating")) {
            acts.push(Act::New);
        }
        if action_card(ui, t, w, false, "folder_open", tl!("Open…"), tl!("PSD, PNG, JPEG, TIFF, RAW and more")) {
            acts.push(Act::Open);
        }
    });
}

/// One of the two start cards: an icon tile, a title and a line under it. The primary one is filled with the accent.
fn action_card(ui: &mut egui::Ui, t: &Tokens, width: f32, primary: bool, icon: &'static str, title: &str, line: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 104.0), Sense::click());
    let hovered = resp.hovered();
    let radius = t.radius_lg.round().clamp(4.0, 24.0);
    let (fill, ink, dim) = if primary {
        (if hovered { t.primary_bg.gamma_multiply(0.92) } else { t.primary_bg }, t.primary_text, t.primary_text.gamma_multiply(0.85))
    } else {
        (if hovered { t.hover } else { t.card }, t.text, t.text_dim)
    };
    let p = ui.painter().clone();
    p.rect(rect, radius, fill, Stroke::new(1.0, if primary { fill } else { t.card_border }), StrokeKind::Inside);
    // The icon on a soft tile at the left.
    let tile = Rect::from_min_size(rect.left_center() + Vec2::new(PAD, -22.0), Vec2::splat(44.0));
    let tile_fill = if primary { Color32::from_white_alpha(38) } else { t.accent_soft };
    p.rect_filled(tile, (radius - 4.0).max(4.0), tile_fill);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(tile.shrink(11.0)).layout(Layout::centered_and_justified(egui::Direction::TopDown)));
    child.visuals_mut().override_text_color = Some(if primary { ink } else { t.accent });
    let _ = Icon::new(IconSpec::svg(IconId::new(icon))).id_salt(("home-card-icon", icon)).show(&mut child);
    let text_x = tile.right() + 16.0;
    p.text(egui::pos2(text_x, rect.center().y - 11.0), egui::Align2::LEFT_CENTER, title, egui::FontId::proportional(17.0), ink);
    p.text(egui::pos2(text_x, rect.center().y + 13.0), egui::Align2::LEFT_CENTER, line, egui::FontId::proportional(13.0), dim);
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

/// A filled accent button of the card grammar.
fn primary_button(ui: &mut egui::Ui, t: &Tokens, label: &str, enabled: bool) -> bool {
    let text = RichText::new(label).color(t.primary_text).strong();
    let b = egui::Button::new(text).fill(t.primary_bg).corner_radius(CornerRadius::same(t.radius.round().clamp(2.0, 16.0) as u8)).min_size(Vec2::new(112.0, 36.0));
    ui.add_enabled(enabled, b).clicked()
}

/// A quiet outlined button.
fn quiet_button(ui: &mut egui::Ui, t: &Tokens, label: &str) -> bool {
    let b = egui::Button::new(RichText::new(label).color(t.text))
        .fill(t.field)
        .stroke(Stroke::new(1.0, t.field_border))
        .corner_radius(CornerRadius::same(t.radius.round().clamp(2.0, 16.0) as u8))
        .min_size(Vec2::new(0.0, 36.0));
    ui.add(b).clicked()
}

/// Generate an idea: a sentence in, a document to start from out (name, size, colours, a brief).
fn idea_card(app: &mut NephclickPhotoLabApp, ui: &mut egui::Ui, t: &Tokens, acts: &mut Vec<Act>) {
    let busy = app.ai.is_busy(crate::ai_ui::Task::Idea);
    let has_key = app.ai.masked_key().is_some() || nephclick_photolab_ai::is_loopback(&app.session.prefs().ai.endpoint);
    card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.visuals_mut().override_text_color = Some(t.accent);
            let _ = Icon::new(IconSpec::svg(IconId::new("sparkles"))).id_salt("home-idea-icon").show(ui);
            ui.label(RichText::new(tl!("Generate an idea with AI")).size(17.0).strong().color(t.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let link = if has_key { tl!("AI settings") } else { tl!("Set up AI…") };
                if ui.link(RichText::new(link).color(t.accent_text).size(13.0)).clicked() {
                    acts.push(Act::AiSettings);
                }
            });
        });
        ui.label(RichText::new(tl!("Describe what you want to make. AI suggests a name, a canvas size, a palette and a brief.")).color(t.text_dim).size(13.0));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let button_w = 112.0;
            let field_w = (ui.available_width() - button_w - ui.spacing().item_spacing.x).max(120.0);
            let edit = egui::TextEdit::singleline(&mut app.ai.idea_prompt)
                .hint_text(tl!("A poster, a social post, an album cover…"))
                .desired_width(field_w)
                .min_size(Vec2::new(field_w, 36.0))
                .align(egui::Align2::LEFT_CENTER)
                .margin(egui::Margin::symmetric(12, 8))
                .char_limit(nephclick_photolab_ai::MAX_PROMPT);
            let resp = ui.add_enabled(!busy, edit);
            let submitted = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let label = if busy { tl!("Generating…") } else { tl!("Generate") };
            if primary_button(ui, t, label, !busy && !app.ai.idea_prompt.trim().is_empty()) || (submitted && !busy) {
                acts.push(Act::Generate);
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
            for s in SUGGESTIONS {
                let chip = egui::Button::new(RichText::new(tl!(s)).size(12.0).color(t.text_dim))
                    .fill(t.field)
                    .stroke(Stroke::new(1.0, t.field_border))
                    .corner_radius(CornerRadius::same(255));
                if ui.add_enabled(!busy, chip).clicked() {
                    app.ai.idea_prompt = tl!(s).to_string();
                    acts.push(Act::Generate);
                }
            }
        });
        if let Some(note) = &app.ai.idea_note {
            ui.label(RichText::new(&note.text).size(13.0).color(if note.error { t.danger } else { t.text_dim }));
        }
        if let Some(idea) = &app.ai.idea {
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            ui.label(RichText::new(&idea.name).size(18.0).strong().color(t.text));
            ui.label(RichText::new(format!("{} × {} px", idea.width, idea.height)).size(13.0).color(t.text_dim));
            swatches(ui, t, &idea.palette.colors, 36.0);
            if !idea.brief.is_empty() {
                ui.label(RichText::new(&idea.brief).size(13.0).color(t.text));
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if primary_button(ui, t, tl!("Create Document"), true) {
                    acts.push(Act::CreateIdea);
                }
                if quiet_button(ui, t, tl!("Open in Color Palettes")) {
                    acts.push(Act::UsePalette);
                }
            });
        }
    });
}

/// A row of colour chips with their hex under the pointer.
pub(crate) fn swatches(ui: &mut egui::Ui, t: &Tokens, colors: &[nephclick_photolab_ai::palette::Rgb], side: f32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        for c in colors {
            let (rect, resp) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
            let [r, g, b] = c.0;
            ui.painter().rect(rect, t.radius.clamp(2.0, 10.0), Color32::from_rgb(r, g, b), Stroke::new(1.0, t.card_border), StrokeKind::Inside);
            resp.on_hover_text(c.hex());
        }
    });
}

/// Recent files as a grid of pictures with their names. Click opens; the menu removes one or shows it in its folder.
fn recent_card(app: &mut NephclickPhotoLabApp, ui: &mut egui::Ui, t: &Tokens, width: f32, acts: &mut Vec<Act>) {
    let show_recent = !cfg!(target_arch = "wasm32") && app.session.prefs().interface.show_home_recent_files;
    let recent = if show_recent { crate::home_recent_ui::recent_files(app, HOME_RECENT) } else { Vec::new() };
    let ctx = ui.ctx().clone();
    let thumbs = crate::home_recent_ui::thumbnails(&ctx);
    let source = crate::home_recent_ui::source(app, &ctx);
    card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(tl!("Recent")).size(17.0).strong().color(t.text));
            if !recent.is_empty() {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.link(RichText::new(tl!("Clear Recent")).color(t.accent_text).size(13.0)).clicked() {
                        acts.push(Act::Recent(StartAction::ClearRecent));
                    }
                });
            }
        });
        if recent.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new(tl!("Files you open or save appear here.")).color(t.text_faint).size(13.0));
            ui.add_space(8.0);
            return;
        }
        ui.add_space(4.0);
        let inner = width - 2.0 * PAD;
        let cols = ((inner + 16.0) / (TILE.x + 16.0)).floor().max(1.0) as usize;
        let tile_w = ((inner - 16.0 * (cols as f32 - 1.0)) / cols as f32).floor();
        let ppp = ctx.pixels_per_point();
        for row in recent.chunks(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                for file in row {
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(tile_w, TILE.y + TILE_LABEL), Sense::click());
                    let pic = Rect::from_min_size(rect.min, Vec2::new(tile_w, TILE.y));
                    let radius = t.radius.clamp(2.0, 12.0);
                    let p = ui.painter();
                    p.rect(pic, radius, t.field, Stroke::new(1.0, if resp.hovered() { t.accent } else { t.card_border }), StrokeKind::Inside);
                    let key = ThumbKey::Recent(file.path.clone());
                    let request = ThumbRequest::for_cell(TILE.x.max(TILE.y), ppp);
                    let texture = match thumbs.state(&ctx, &source, &key, &request, Priority::VISIBLE) {
                        TextureState::Ready(tex) => Some(tex),
                        TextureState::Loading { stand_in, .. } => stand_in,
                        _ => None,
                    };
                    if let Some(tex) = texture {
                        let size = tex.size_vec2();
                        let room = pic.shrink(8.0);
                        let scale = (room.width() / size.x.max(1.0)).min(room.height() / size.y.max(1.0));
                        let shown = Rect::from_center_size(room.center(), size * scale);
                        p.image(tex.id, shown, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                    } else {
                        p.text(pic.center(), egui::Align2::CENTER_CENTER, "…", egui::FontId::proportional(16.0), t.text_faint);
                    }
                    let name = elide(&file.name, tile_w, ui);
                    p.text(egui::pos2(rect.left() + 2.0, pic.bottom() + 12.0), egui::Align2::LEFT_TOP, name, egui::FontId::proportional(13.0), t.text);
                    let path = file.path.clone();
                    // Screen readers and automation name the tile by its file name.
                    let name_for_access = file.name.clone();
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name_for_access));
                    let resp = resp.on_hover_text(path.to_string_lossy());
                    if resp.hovered() {
                        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if resp.clicked() {
                        acts.push(Act::Recent(StartAction::OpenRecent(path.clone())));
                    }
                    resp.context_menu(|ui| {
                        if ui.button(tl!("Open")).clicked() {
                            acts.push(Act::Recent(StartAction::OpenRecent(path.clone())));
                            ui.close();
                        }
                        if ui.button(crate::menus::reveal_label()).clicked() {
                            acts.push(Act::Reveal(path.clone()));
                            ui.close();
                        }
                        if ui.button(tl!("Remove from Recent")).clicked() {
                            acts.push(Act::Recent(StartAction::RemoveRecent(path.clone())));
                            ui.close();
                        }
                    });
                }
            });
            ui.add_space(4.0);
        }
    });
}

/// `name` cut with an ellipsis to fit `width` at the tile label's size.
fn elide(name: &str, width: f32, ui: &egui::Ui) -> String {
    let font = egui::FontId::proportional(13.0);
    let fits = |s: &str| ui.painter().layout_no_wrap(s.to_string(), font.clone(), Color32::WHITE).size().x <= width - 4.0;
    if fits(name) {
        return name.to_string();
    }
    let chars: Vec<char> = name.chars().collect();
    let mut n = chars.len();
    while n > 1 {
        n -= 1;
        let cut: String = chars.iter().take(n).collect::<String>() + "…";
        if fits(&cut) {
            return cut;
        }
    }
    "…".to_string()
}

/// Shows a recent file selected in the system file manager; a failure goes to the status line, as the other open
/// errors do.
fn reveal(app: &mut NephclickPhotoLabApp, path: &std::path::Path) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Err(e) = craft::platform::open::reveal(path) {
        app.ui.status = e;
        app.ui.status_error = true;
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (app, path);
}

/// Shows Home over the open documents, or leaves it for them: the rule of the options bar's Home button, the Home
/// rail's Home link and `window.home`. With no document open there is nothing behind Home, so it does nothing.
pub(crate) fn toggle_home(app: &mut NephclickPhotoLabApp) {
    let documents = app.session.documents().len();
    if documents == 0 {
        return;
    }
    let auto = app.session.prefs().general.auto_show_home_screen;
    let up = app.ui.chrome.shows_home(documents, auto);
    app.ui.chrome.home = if up { None } else { Some(documents) };
}
