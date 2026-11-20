//! Edit › Keyboard Shortcuts and Menus: the framework's Keyboard Shortcuts dialog over a draft
//! keymap, with Nephclick PhotoLab's Menus tab beside it.
//!
//! The session holds everything the dialog edits: the draft keymap, the shortcut sets, the Menus
//! draft and the text of a set waiting to be reviewed. Cancel drops it and the runtime never saw
//! a change; OK writes the preferences once ([`save_keys`], then the menus), and the runtime's
//! keymap follows the new preferences revision (`shortcut_dispatch::overrides`).
//!
//! The Toolbar tab is not here: Edit › Toolbar opens the toolbar drawer.

use std::collections::{BTreeMap, BTreeSet};

use craft::keymap::import::kys;
use craft::keymap::{Builtins, CommandRef, ImportOptions, Keymap, NoneDefined, Overrides, Profiles, Scopes, reserved};
use craft::menu::Row;
use craft::menu::shortcuts::{Area, ConflictMode, DialogState, DialogTab, Format, Options, Outcome, ShortcutWords, ShortcutsDialog, kys_error_text};
use craft::ui::runtime::env::Env;
use craft::ui::runtime::windows;
use nephclick_photolab_engine::keymap_defs;
use nephclick_photolab_engine::prefs::{MenuCustomization, Preferences};
use nephclick_photolab_engine::shortcut_sets::DEFAULTS_NAME;
use serde_json::{Map, Value, json};

use crate::NephclickPhotoLabApp;

/// The id of the `.kys` format in the dialog's import and export lists.
const KYS_FORMAT: &str = "kys";

/// The colours Edit › Menus gives an item; `none` is no colour.
const COLORS: [&str; 8] = ["none", "red", "orange", "yellow", "green", "blue", "violet", "gray"];

/// The open Keyboard Shortcuts and Menus dialog, at `app.prefs_rt.shortcuts`.
pub(crate) struct ShortcutsSession {
    /// The dialog's state between passes.
    pub(crate) state: DialogState,
    /// The shortcut sets as the dialog edits them.
    pub(crate) profiles: Profiles,
    /// The draft the dialog edits in place: the rows the runtime catalog binds, with the
    /// preferences' bindings. The runtime's own keymap is not this one.
    pub(crate) keymap: Keymap,
    /// The Menus tab's draft.
    pub(crate) menus: MenusDraft,
    /// The text of a set to review on the next pass, taken by that pass alone: a second pass of
    /// the same frame finds nothing to review.
    pub(crate) pending_import: Option<String>,
    /// The user's answers to earlier import questions, for the next import of this session.
    pub(crate) remembered: BTreeMap<String, CommandRef>,
}

/// What the Menus tab edits: the hidden items and the colour of each item, by command id; and
/// what it shows: the menu picked in its sidebar and the search text.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct MenusDraft {
    pub(crate) hidden: BTreeSet<String>,
    pub(crate) colors: BTreeMap<String, String>,
    /// The top-level menu the sidebar shows ("File", "Edit", …); empty for the first.
    pub(crate) menu: String,
    /// The search over every menu's items; empty shows the picked menu.
    pub(crate) search: String,
}

impl MenusDraft {
    fn of(menus: &MenuCustomization) -> Self {
        MenusDraft { hidden: menus.hidden.iter().cloned().collect(), colors: menus.colors.clone(), menu: String::new(), search: String::new() }
    }

    /// Whether the edits (not the view) differ from `other`'s.
    fn edits_differ(&self, other: &MenusDraft) -> bool {
        self.hidden != other.hidden || self.colors != other.colors
    }
}

/// The built-in sets: Nephclick PhotoLab's defaults.
fn builtins() -> [(&'static str, Overrides); 1] {
    [(DEFAULTS_NAME, Overrides::new())]
}

/// The saved sets of `prefs` as the dialog's `Profiles`. The defaults are the active set when the
/// preferences name none.
pub(crate) fn profiles_of(prefs: &Preferences) -> Profiles {
    let sets: Vec<Value> = prefs.shortcut_sets.iter().map(|(name, set)| json!({"name": name, "overrides": keymap_defs::set_overrides(set)})).collect();
    serde_json::from_value(json!({"active": prefs.shortcut_set, "sets": sets})).unwrap_or_default()
}

/// The keymap the dialog and the offer edit: the rows the runtime catalog binds, the platform the
/// runtime's keymap is built for, and the preferences' bindings as overrides.
pub(crate) fn draft_keymap(app: &NephclickPhotoLabApp) -> Keymap {
    let platform = windows::os_of(app.ctx());
    Keymap::new(crate::shortcut_dispatch::command_rows(app).into_iter().chain(crate::shortcut_dispatch::bridged_rows()).collect(), Scopes::new(), platform)
        .with_reserved(reserved::system(platform))
        .with_overrides(keymap_defs::prefs_overrides(app.session.prefs()))
}

/// How a `.kys` set is read for review and for the offer: rows without a key leave their command
/// alone, and the user's earlier answers come first.
pub(crate) fn import_options(remembered: &BTreeMap<String, CommandRef>) -> ImportOptions {
    ImportOptions { remembered: remembered.clone(), none_defined: NoneDefined::Keep, suggest: true, ..ImportOptions::default() }
}

/// The text of a set file's bytes, or why it is not one.
pub(crate) fn kys_text(name: &str, bytes: &[u8]) -> Result<String, String> {
    std::str::from_utf8(bytes).map(str::to_owned).map_err(|_| format!("{name}: not a keyboard shortcut set (.kys): not UTF-8"))
}

/// Opens the dialog (when it is not open) on tab `tab`: 0 Keyboard Shortcuts, 1 Menus.
pub fn open(app: &mut NephclickPhotoLabApp, tab: u64) {
    if app.prefs_rt.shortcuts.is_some() {
        return;
    }
    let prefs = app.session.prefs();
    let session = ShortcutsSession {
        state: if tab == 1 { DialogState::new().with_tab("menus") } else { DialogState::new() },
        profiles: profiles_of(prefs),
        keymap: draft_keymap(app),
        menus: MenusDraft::of(&prefs.menus),
        pending_import: None,
        remembered: BTreeMap::new(),
    };
    app.prefs_rt.shortcuts = Some(session);
}

/// Whether the dialog is open: the runtime's keys step aside for its modal.
pub fn shortcuts_open(app: &NephclickPhotoLabApp) -> bool {
    app.prefs_rt.shortcuts.is_some()
}

/// Whether the dialog is recording a chord: the key goes to the recorder and runs nothing, and the
/// native menu bar pauses its key equivalents.
pub fn recording(app: &NephclickPhotoLabApp) -> bool {
    app.prefs_rt.shortcuts.as_ref().is_some_and(|session| session.state.is_recording())
}

/// Queues the text of a set for review on the dialog's next pass.
pub(crate) fn queue_import(app: &mut NephclickPhotoLabApp, text: String) {
    open(app, 0);
    if let Some(session) = app.prefs_rt.shortcuts.as_mut() {
        session.pending_import = Some(text);
    }
}

/// The dialog's options: Photoshop's Shortcuts For areas, the conflict bar, `.kys` files in and
/// out, and the summary.
fn options() -> Options {
    let kys = Format::new(KYS_FORMAT, "Photoshop Keyboard Shortcuts (.kys)", &["kys", "psp"]);
    Options::default().with(|o| {
        o.title = Some(tl!("Keyboard Shortcuts and Menus").to_string());
        o.areas = Area::photoshop();
        o.conflicts = ConflictMode::Ask;
        o.import_formats = vec![kys];
        o.export_formats = vec![kys];
        o.summary = true;
    })
}

/// The dialog over the session's parts for one pass.
fn dialog<'a>(keymap: &'a mut Keymap, profiles: &'a mut Profiles, builtins: &'a Builtins<'a>, options: &'a Options) -> ShortcutsDialog<'a> {
    ShortcutsDialog::new(keymap).sets(profiles, builtins).options(options)
}

/// One pass of the dialog while it is open.
pub fn show(app: &mut NephclickPhotoLabApp, ui: &mut egui::Ui) {
    pass(app, ui, false);
}

/// One pass; with `press_ok` the pass presses OK first. The session is out of `app.prefs_rt`
/// during the pass, so nothing in the pass reads it through [`recording`].
fn pass(app: &mut NephclickPhotoLabApp, ui: &mut egui::Ui, press_ok: bool) {
    let Some(mut session) = app.prefs_rt.shortcuts.take() else { return };
    let rows = crate::menus::base_rows(app);
    let options = options();
    let builtins = builtins();
    let ShortcutsSession { state, profiles, keymap, menus, pending_import, remembered } = &mut session;
    if let Some(text) = pending_import.take() {
        review(ui, state, dialog(keymap, profiles, &builtins, &options), &text, remembered);
    }
    if press_ok {
        dialog(keymap, profiles, &builtins, &options).ok(ui, state);
    }
    let mut tabs = [menus_tab(&rows, menus)];
    let output = dialog(keymap, profiles, &builtins, &options).tabs(&mut tabs).show(ui, state);
    drop(tabs);
    match output.outcome {
        Some(Outcome::Ok) => {
            if let Err(e) = save_keys(app, keymap, profiles).and_then(|_| save_menus(app, menus)) {
                app.ui.status = crate::i18n::fmt(tl!("The keyboard shortcuts were not saved: {error}"), &[("error", &e)]);
                app.ui.status_error = true;
            }
        }
        Some(Outcome::Import { .. }) => ask_import(app, state),
        Some(Outcome::Export { name, json, .. }) => save_text(app, state, &format!("{name}.json"), json),
        Some(Outcome::ExportAs { format, name, keymap: exported, .. }) if format == KYS_FORMAT => {
            save_text(app, state, &format!("{name}.kys"), kys::write(&name, &exported));
        }
        Some(Outcome::Summary { name, html, .. }) => save_text(app, state, &format!("{name}.html"), html),
        Some(Outcome::Remembered(answers)) => remembered.extend(answers),
        _ => {}
    }
    if state.is_open() {
        app.prefs_rt.shortcuts = Some(session);
    }
}

/// Opens the review of the set `text` holds (a `.kys` file, or craft-keymap JSON), or shows why it
/// cannot be read.
fn review(ui: &egui::Ui, state: &mut DialogState, dialog: ShortcutsDialog<'_>, text: &str, remembered: &BTreeMap<String, CommandRef>) {
    if text.trim_start().starts_with(['{', '[']) {
        if let Err(e) = dialog.import_json(ui, state, text) {
            state.show_error(e.to_string());
        }
        return;
    }
    match kys::read(text) {
        Ok(set) => dialog.review_import(ui, state, set, import_options(remembered)),
        Err(e) => state.show_error(kys_error_text(&e, &Env::of(ui).words::<ShortcutWords>())),
    }
}

/// Import…: ask for the file; the text is reviewed on the pass after it arrives.
fn ask_import(app: &mut NephclickPhotoLabApp, state: &mut DialogState) {
    let picked = app.pick_file_bytes_filtered(&["kys", "psp", "json"], |app, name, bytes| {
        let text = kys_text(&name, &bytes)?;
        queue_import(app, text);
        Ok(json!({"dialog": "shortcuts"}))
    });
    if let Err(e) = picked {
        state.show_error(e);
    }
}

/// Saves `text` through the save dialog under the suggested file name.
fn save_text(app: &mut NephclickPhotoLabApp, state: &mut DialogState, suggested: &str, text: String) {
    // A set name can hold characters a file name can't.
    let name: String = suggested.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c }).collect();
    let saved = app.pick_save(&name, move |app, path| {
        let write = app.services.write.as_mut().ok_or("no writer configured")?;
        write(&path, text.as_bytes())?;
        Ok(json!({"path": path}))
    });
    if let Err(e) = saved {
        state.show_error(e);
    }
}

/// Writes the bindings and sets of the dialog (or of an import) to the preferences in one
/// `edit.keyboardShortcuts` call: the one writer of the shortcut preferences. An action's function
/// key belongs to the action, not to a set, so the preferences' own stay whatever set was chosen.
pub(crate) fn save_keys(app: &mut NephclickPhotoLabApp, keymap: &Keymap, profiles: &Profiles) -> Result<Value, String> {
    let (mut set, mut alternates) = keymap_defs::overrides_to_prefs(keymap.overrides());
    let prefs = app.session.prefs();
    let is_action = |id: &String| id.starts_with(nephclick_photolab_engine::actions_cmds::PLAY_SHORTCUT_PREFIX);
    set.retain(|id, _| !is_action(id));
    alternates.retain(|id, _| !is_action(id));
    set.extend(prefs.shortcuts.iter().filter(|(id, _)| is_action(id)).map(|(id, key)| (id.clone(), key.clone())));
    alternates.extend(prefs.alternate_shortcuts.iter().filter(|(id, _)| is_action(id)).map(|(id, keys)| (id.clone(), keys.clone())));
    let builtins = builtins();
    let active = profiles.active_name(&builtins);
    // OK saves the dialog's bindings into the set in use.
    let sets: Map<String, Value> = profiles
        .sets()
        .iter()
        .map(|(name, overrides)| {
            let overrides = if name.as_str() == active { keymap.overrides() } else { overrides };
            let (mut shortcuts, mut alternate_shortcuts) = keymap_defs::overrides_to_prefs(overrides);
            shortcuts.retain(|id, _| !is_action(id));
            alternate_shortcuts.retain(|id, _| !is_action(id));
            (name.clone(), json!({"shortcuts": shortcuts, "alternateShortcuts": alternate_shortcuts}))
        })
        .collect();
    let active = if active == DEFAULTS_NAME { "" } else { active };
    let call = json!({"reset": true, "set": set, "alternates": alternates, "sets": sets, "activeSet": active, "allowUnknown": true, "removeConflicts": false});
    app.run("edit.keyboardShortcuts", call).map_err(|e| e.to_string())
}

/// Writes the Menus tab's draft to the preferences.
fn save_menus(app: &mut NephclickPhotoLabApp, menus: &MenusDraft) -> Result<Value, String> {
    let call = json!({"values": {"menus": {"hidden": menus.hidden, "colors": menus.colors}}});
    app.run("prefs.set", call).map_err(|e| e.to_string())
}

/// The Menus tab: a row per menu item with a Visible check and a colour.
fn menus_tab<'a>(rows: &'a [Row], draft: &'a mut MenusDraft) -> DialogTab<'a> {
    DialogTab::new("menus", tl!("Menus").to_string(), move |ui| menus_body(ui, rows, &mut *draft))
}

/// Longest search text the Menus tab keeps.
const MAX_MENU_SEARCH: usize = 120;

/// The width of the Menus tab's sidebar of menus.
const MENU_SIDEBAR_WIDTH: f32 = 168.0;

/// The Menus tab's body; true when the draft's edits changed. `rows` are the menu items before the
/// customization, so a hidden item is listed here to be shown again.
///
/// A search field and the Show All and Clear Colors buttons over a sidebar of the top-level menus
/// (each with the number of its items hidden) beside the picked menu's items, grouped by submenu:
/// a Visible check, the item's name (dimmed while hidden) and its colour. A search lists the
/// matching items of every menu.
fn menus_body(ui: &mut egui::Ui, rows: &[Row], draft: &mut MenusDraft) -> bool {
    let before = draft.clone();
    let items: Vec<&Row> = rows.iter().filter(|row| row.label != "---" && !row.path.is_empty()).collect();
    let mut menus: Vec<&str> = Vec::new();
    for row in &items {
        if let Some(top) = row.path.first()
            && !menus.contains(&top.as_str())
        {
            menus.push(top.as_str());
        }
    }
    if !menus.contains(&draft.menu.as_str()) {
        draft.menu = menus.first().map(|menu| (*menu).to_owned()).unwrap_or_default();
    }
    let visuals = ui.visuals().clone();

    // The bar: search, then the buttons that act on every item.
    ui.horizontal(|ui| {
        let search = egui::TextEdit::singleline(&mut draft.search).hint_text(tl!("Search menu items")).desired_width(280.0);
        ui.add(search);
        if !draft.search.is_empty() && ui.small_button("×").on_hover_text(tl!("Clear the search")).clicked() {
            draft.search.clear();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(tl!("Clear Colors")).clicked() {
                draft.colors.clear();
            }
            if ui.button(tl!("Show All Menu Items")).clicked() {
                draft.hidden.clear();
            }
        });
    });
    if draft.search.chars().count() > MAX_MENU_SEARCH {
        draft.search = draft.search.chars().take(MAX_MENU_SEARCH).collect();
    }
    ui.add_space(6.0);
    let search = draft.search.trim().to_lowercase();
    let footer = ui.spacing().interact_size.y + 6.0;
    let height = (ui.available_height() - footer).max(120.0);

    ui.horizontal_top(|ui| {
        // The sidebar of menus.
        egui::Frame::new().fill(visuals.faint_bg_color).corner_radius(6.0).inner_margin(6.0).show(ui, |ui| {
            ui.set_width(MENU_SIDEBAR_WIDTH);
            ui.set_height(height - 12.0);
            egui::ScrollArea::vertical().id_salt("menus-sidebar").auto_shrink([false, false]).show(ui, |ui| {
                for menu in &menus {
                    let hidden = items.iter().filter(|row| row.path.first().is_some_and(|top| top == menu) && draft.hidden.contains(row.command.id.as_str())).count();
                    let selected = search.is_empty() && draft.menu == *menu;
                    let text = if hidden > 0 { format!("{menu}  ·  {hidden} {}", tl!("hidden")) } else { (*menu).to_owned() };
                    let response = ui.add_sized([ui.available_width(), 26.0], egui::Button::selectable(selected, text));
                    if response.clicked() {
                        draft.menu = (*menu).to_owned();
                        draft.search.clear();
                    }
                }
            });
        });
        ui.add_space(8.0);
        // The items of the picked menu, or of the search.
        egui::Frame::new().stroke(visuals.widgets.noninteractive.bg_stroke).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
            ui.set_height(height - 16.0);
            ui.set_width(ui.available_width());
            let shown: Vec<&&Row> = items
                .iter()
                .filter(|row| {
                    if search.is_empty() {
                        row.path.first().is_some_and(|top| *top == draft.menu)
                    } else {
                        row.label.to_lowercase().contains(&search) || row.path.iter().any(|part| part.to_lowercase().contains(&search))
                    }
                })
                .collect();
            if shown.is_empty() {
                ui.add_space(12.0);
                ui.vertical_centered(|ui| ui.weak(tl!("No menu items match the search.")));
                return;
            }
            egui::ScrollArea::vertical().id_salt("menus-items").auto_shrink([false, false]).show(ui, |ui| {
                let mut group: Option<String> = None;
                for row in shown {
                    // A heading for each submenu (the menu itself when searching), as the menu bar groups them.
                    let heading = if search.is_empty() { row.path.get(1..).unwrap_or_default().join(" › ") } else { row.path.join(" › ") };
                    if group.as_deref() != Some(heading.as_str()) {
                        if group.is_some() {
                            ui.add_space(6.0);
                        }
                        if !heading.is_empty() {
                            ui.label(egui::RichText::new(&heading).small().strong().color(visuals.weak_text_color()));
                        }
                        group = Some(heading);
                    }
                    menu_item_row(ui, row, draft);
                }
            });
        });
    });

    let hidden = draft.hidden.len();
    let colored = draft.colors.len();
    ui.horizontal(|ui| {
        ui.weak(crate::i18n::fmt(tl!("{hidden} hidden · {colored} with a colour"), &[("hidden", &hidden.to_string()), ("colored", &colored.to_string())]));
    });
    draft.edits_differ(&before)
}

/// One item of the Menus tab: its Visible check, its name, and its colour.
fn menu_item_row(ui: &mut egui::Ui, row: &Row, draft: &mut MenusDraft) {
    let id = row.command.id.as_str();
    ui.push_id(id, |ui| {
        let height = ui.spacing().interact_size.y + 4.0;
        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), height), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            let visible = !draft.hidden.contains(id);
            // Items that can't be hidden (File › Close, Save…, Edit › Menus…) have no visibility
            // control at all, as in the reference application (#5096, #14335).
            if MenuCustomization::hideable(id) {
                let mut checked = visible;
                if ui.checkbox(&mut checked, "").on_hover_text(tl!("Visible")).changed() {
                    if checked {
                        draft.hidden.remove(id);
                    } else {
                        draft.hidden.insert(id.to_owned());
                    }
                }
            } else {
                ui.add_space(ui.spacing().interact_size.y + ui.spacing().item_spacing.x);
            }
            let current = draft.colors.get(id).map_or("none", String::as_str).to_owned();
            let current = COLORS.iter().copied().find(|c| *c == current).unwrap_or("none");
            if let Some(tint) = crate::menus::menu_tint(current) {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 5.0, tint);
            }
            let mut name = egui::RichText::new(&row.label);
            if !visible {
                name = name.weak().strikethrough();
            }
            ui.label(name);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut picked = current;
                egui::ComboBox::from_id_salt("menu-color").width(110.0).selected_text(color_name(current)).show_ui(ui, |ui| {
                    for color in COLORS {
                        ui.horizontal(|ui| {
                            match crate::menus::menu_tint(color) {
                                Some(tint) => {
                                    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                                    ui.painter().circle_filled(rect.center(), 5.0, tint);
                                }
                                None => {
                                    ui.add_space(10.0 + ui.spacing().item_spacing.x);
                                }
                            }
                            ui.selectable_value(&mut picked, color, color_name(color));
                        });
                    }
                });
                if picked != current {
                    if picked == "none" {
                        draft.colors.remove(id);
                    } else {
                        draft.colors.insert(id.to_owned(), picked.to_owned());
                    }
                }
                if let Some(shortcut) = &row.shortcut {
                    ui.weak(shortcut.to_string()).on_hover_text(tl!("Shortcut"));
                }
            });
        });
    });
}

/// The shown name of an Edit › Menus colour.
fn color_name(color: &str) -> String {
    match color {
        "none" => tl!("None").to_string(),
        "red" => tl!("Red").to_string(),
        "orange" => tl!("Orange").to_string(),
        "yellow" => tl!("Yellow").to_string(),
        "green" => tl!("Green").to_string(),
        "blue" => tl!("Blue").to_string(),
        "violet" => tl!("Violet").to_string(),
        "gray" => tl!("Gray").to_string(),
        other => other.to_owned(),
    }
}

/// Test hook: adds `binding` to `command` in the open dialog's draft keymap, as the dialog's own edit does.
#[cfg(test)]
pub(crate) fn assign(
    app: &mut NephclickPhotoLabApp,
    command: &CommandRef,
    binding: craft::keymap::Binding,
    policy: craft::keymap::Policy,
) -> Result<craft::keymap::Assigned, craft::keymap::AssignError> {
    let session = app.prefs_rt.shortcuts.as_mut().ok_or(craft::keymap::AssignError::UnknownCommand)?;
    session.keymap.assign(command, binding, policy)
}

/// Test hook: makes `binding` the only binding of `command` in the open dialog's draft keymap, as recording over its
/// key in the dialog does.
#[cfg(test)]
pub(crate) fn rebind(
    app: &mut NephclickPhotoLabApp,
    command: &CommandRef,
    binding: craft::keymap::Binding,
    policy: craft::keymap::Policy,
) -> Result<craft::keymap::Assigned, craft::keymap::AssignError> {
    let session = app.prefs_rt.shortcuts.as_mut().ok_or(craft::keymap::AssignError::UnknownCommand)?;
    session.keymap.set_bindings(command, vec![binding], policy)
}

/// Test hook: presses OK in the open dialog and handles the outcome, in one pass on `ctx`.
#[cfg(test)]
pub(crate) fn press_ok(app: &mut NephclickPhotoLabApp, ctx: &egui::Context) {
    crate::testing::run_ui(ctx, Default::default(), |ui| pass(app, ui, true));
}
