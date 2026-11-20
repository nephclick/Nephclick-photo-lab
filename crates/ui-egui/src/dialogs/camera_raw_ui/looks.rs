//! RAW Develop's looks: the built-in presets, the presets the user saved, and the LUT library.
//!
//! The user's presets live in `raw_presets.json` in the settings folder; the LUTs the user loads are copied into its
//! `raw_luts` folder as `.cube` files and read into the develop's LUT library (`nephclick_photolab_algo::raw_luts`)
//! the first time RAW Develop opens. The built-in presets and the built-in LUT looks are made here: original looks,
//! none taken from another product.

use std::path::PathBuf;

use craft::imaging::ui::raw_develop::{RawLooks, RawPreset};
use nephclick_photolab_algo::raw_luts;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{CameraRaw, NephclickPhotoLabApp, hosted_mut};

/// Most presets the user may keep.
const MAX_USER_PRESETS: usize = 200;

/// Largest presets file read, in bytes.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
const MAX_PRESETS_FILE: u64 = 8 << 20;

/// Largest `.cube` file loaded into the library, in bytes (a 129³ table is about 60 MB).
const MAX_LUT_FILE: usize = 64 << 20;

/// One saved preset, as `raw_presets.json` holds it.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredPreset {
    name: String,
    settings: CameraRaw,
}

/// The settings folder, where the presets file and the LUT folder are kept; `None` on the web.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn config_dir(app: &NephclickPhotoLabApp) -> Option<PathBuf> {
    if cfg!(target_arch = "wasm32") {
        return None;
    }
    app.services.ai_dir.clone()
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn presets_file(app: &NephclickPhotoLabApp) -> Option<PathBuf> {
    config_dir(app).map(|dir| dir.join("raw_presets.json"))
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn luts_dir(app: &NephclickPhotoLabApp) -> Option<PathBuf> {
    config_dir(app).map(|dir| dir.join("raw_luts"))
}

/// The built-in presets: original looks made from the develop's own controls.
pub(crate) fn builtin_presets() -> Vec<RawPreset> {
    let look = |name: &str, edit: &dyn Fn(&mut CameraRaw)| {
        let mut settings = CameraRaw::default();
        edit(&mut settings);
        RawPreset::new(name, settings, true)
    };
    vec![
        look("Natural Pop", &|s| {
            s.contrast = 12.0;
            s.vibrance = 22.0;
            s.clarity = 8.0;
            s.highlights = -15.0;
            s.shadows = 12.0;
        }),
        look("Vivid", &|s| {
            s.contrast = 25.0;
            s.vibrance = 40.0;
            s.saturation = 10.0;
            s.clarity = 15.0;
            s.dehaze = 6.0;
        }),
        look("Soft Matte", &|s| {
            s.contrast = -18.0;
            s.highlights = -20.0;
            s.shadows = 25.0;
            s.blacks = 22.0;
            s.saturation = -12.0;
            s.grain_amount = 10.0;
        }),
        look("Warm Glow", &|s| {
            s.temperature = 22.0;
            s.tint = 6.0;
            s.exposure = 0.1;
            s.highlights = -25.0;
            s.vibrance = 15.0;
            s.lut = "Warm Filter".into();
            s.lut_amount = 40.0;
        }),
        look("Cool Crisp", &|s| {
            s.temperature = -20.0;
            s.contrast = 15.0;
            s.clarity = 18.0;
            s.whites = 10.0;
            s.lut = "Cool Filter".into();
            s.lut_amount = 35.0;
        }),
        look("Cinematic", &|s| {
            s.contrast = 18.0;
            s.highlights = -30.0;
            s.shadows = 10.0;
            s.vibrance = -8.0;
            s.vignette_amount = -18.0;
            s.lut = "Teal & Orange".into();
            s.lut_amount = 70.0;
        }),
        look("Faded Film", &|s| {
            s.contrast = -10.0;
            s.blacks = 18.0;
            s.grain_amount = 22.0;
            s.grain_size = 30.0;
            s.lut = "Faded Film".into();
            s.lut_amount = 80.0;
        }),
        look("Black & White", &|s| {
            s.saturation = -100.0;
            s.contrast = 22.0;
            s.clarity = 12.0;
            s.whites = 8.0;
            s.blacks = -8.0;
        }),
        look("High-Key Portrait", &|s| {
            s.exposure = 0.35;
            s.contrast = -10.0;
            s.highlights = -35.0;
            s.shadows = 30.0;
            s.texture = -15.0;
            s.vibrance = 10.0;
        }),
        look("Moody Dark", &|s| {
            s.exposure = -0.3;
            s.contrast = 20.0;
            s.highlights = -40.0;
            s.shadows = -10.0;
            s.saturation = -15.0;
            s.vignette_amount = -30.0;
            s.lut = "Bleach Bypass".into();
            s.lut_amount = 45.0;
        }),
    ]
}

/// The user's saved presets, from `raw_presets.json`; none when the file is missing, too large or doesn't parse.
fn user_presets(app: &NephclickPhotoLabApp) -> Vec<StoredPreset> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(path) = presets_file(app) else { return Vec::new() };
        let Ok(meta) = std::fs::metadata(&path) else { return Vec::new() };
        if meta.len() > MAX_PRESETS_FILE {
            log::warn!("{}: larger than {MAX_PRESETS_FILE} bytes, ignored", path.display());
            return Vec::new();
        }
        let Ok(bytes) = std::fs::read(&path) else { return Vec::new() };
        match serde_json::from_slice::<Vec<StoredPreset>>(&bytes) {
            Ok(mut presets) => {
                presets.retain(|preset| !preset.name.trim().is_empty() && preset.settings.validate().is_ok());
                presets.truncate(MAX_USER_PRESETS);
                presets
            }
            Err(e) => {
                log::warn!("{}: presets ignored: {e}", path.display());
                Vec::new()
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = app;
        Vec::new()
    }
}

/// Writes the user's presets to `raw_presets.json`.
fn write_user_presets(app: &NephclickPhotoLabApp, presets: &[StoredPreset]) -> Result<(), String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = presets_file(app).ok_or("no settings folder to keep presets in")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let bytes = serde_json::to_vec_pretty(presets).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (app, presets);
        Err("presets are kept only in the desktop app".into())
    }
}

/// Reads the `.cube` files of the LUT folder into the develop's LUT library, once per run.
fn load_lut_folder(app: &NephclickPhotoLabApp) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        static LOADED: std::sync::Once = std::sync::Once::new();
        let Some(dir) = luts_dir(app) else { return };
        LOADED.call_once(|| {
            let Ok(entries) = std::fs::read_dir(&dir) else { return };
            for entry in entries.flatten().take(raw_luts::MAX_LUTS) {
                let path = entry.path();
                let is_cube = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("cube"));
                let Some(name) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else { continue };
                if !is_cube || entry.metadata().map_or(true, |m| m.len() as usize > MAX_LUT_FILE) {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&path) else { continue };
                if let Err(e) = parse_lut(&name, &bytes).and_then(|lut| raw_luts::insert(&name, lut)) {
                    log::warn!("{}: LUT not loaded: {e}", path.display());
                }
            }
        });
    }
    #[cfg(target_arch = "wasm32")]
    let _ = app;
}

/// Reads the LUT folder into the library, as the first open of RAW Develop would.
pub(super) fn preload(app: &NephclickPhotoLabApp) {
    load_lut_folder(app);
}

/// The LUT a `.cube` file holds; only the plain 0..1 domain is taken.
fn parse_lut(name: &str, bytes: &[u8]) -> Result<raw_luts::RawLut, String> {
    if bytes.len() > MAX_LUT_FILE {
        return Err(format!("larger than {} MB", MAX_LUT_FILE >> 20));
    }
    let file = craft::cms::lutfile::parse(&format!("{name}.cube"), bytes).map_err(|e| e.0)?;
    if !file.domain_is_default() {
        return Err("the LUT declares a custom input domain, which RAW Develop doesn't apply".into());
    }
    raw_luts::RawLut::new(file.size, file.data).ok_or_else(|| "not a usable 3D LUT".to_owned())
}

/// The looks RAW Develop offers now: the built-in presets, the user's, and every LUT's name.
pub(crate) fn looks(app: &NephclickPhotoLabApp) -> RawLooks {
    load_lut_folder(app);
    let mut presets = builtin_presets();
    presets.extend(user_presets(app).into_iter().map(|preset| RawPreset::new(preset.name, preset.settings, false)));
    RawLooks::new(presets, raw_luts::names())
}

/// Hands the open workspace the looks as they are now.
fn refresh(app: &mut NephclickPhotoLabApp) {
    let looks = looks(app);
    if let Some(hosted) = hosted_mut(app) {
        hosted.workspace_mut().set_looks(looks);
    }
}

/// Save Preset: keeps `settings` as the user's preset `name`, replacing one of that name.
pub(super) fn save_preset(app: &mut NephclickPhotoLabApp, name: &str, settings: &CameraRaw) -> Result<Value, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a preset needs a name".into());
    }
    if builtin_presets().iter().any(|preset| preset.name == name) {
        return Err(format!("\"{name}\" is a built-in preset: choose another name"));
    }
    settings.validate()?;
    let mut presets = user_presets(app);
    presets.retain(|preset| preset.name != name);
    if presets.len() >= MAX_USER_PRESETS {
        return Err(format!("at most {MAX_USER_PRESETS} presets can be kept"));
    }
    presets.push(StoredPreset { name: name.to_owned(), settings: settings.clone() });
    presets.sort_by_key(|preset| preset.name.to_lowercase());
    write_user_presets(app, &presets)?;
    refresh(app);
    app.ui.status = format!("Saved the RAW Develop preset \"{name}\"");
    app.ui.status_error = false;
    Ok(json!({"preset": name}))
}

/// Deletes the user's preset `name`.
pub(super) fn delete_preset(app: &mut NephclickPhotoLabApp, name: &str) -> Result<Value, String> {
    let mut presets = user_presets(app);
    let before = presets.len();
    presets.retain(|preset| preset.name != name);
    if presets.len() != before {
        write_user_presets(app, &presets)?;
    }
    refresh(app);
    Ok(json!({"deleted": name}))
}

/// Load LUT: asks for a `.cube` file, copies it into the LUT folder and the library, and picks it.
pub(super) fn import_lut(app: &mut NephclickPhotoLabApp) -> Result<Value, String> {
    app.pick_file_bytes_filtered(&["cube"], |app, file_name, bytes| {
        let stem = file_name.rsplit(['/', '\\']).next().unwrap_or(&file_name);
        let stem = stem.rsplit_once('.').map_or(stem, |(stem, _)| stem);
        // A name that is also a file name everywhere: no path separators or reserved characters.
        let mut name: String = stem.chars().filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')).collect();
        name = name.trim().chars().take(64).collect();
        if name.is_empty() {
            name = "Custom LUT".into();
        }
        if raw_luts::is_builtin(&name) {
            name.push_str(" (custom)");
        }
        let lut = parse_lut(&name, &bytes)?;
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(dir) = luts_dir(app) {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            std::fs::write(dir.join(format!("{name}.cube")), &bytes).map_err(|e| e.to_string())?;
        }
        raw_luts::insert(&name, lut)?;
        refresh(app);
        if let Some(hosted) = hosted_mut(app) {
            let mut settings = hosted.workspace().session().clone();
            settings.lut = name.clone();
            if settings.lut_amount <= 0.0 {
                settings.lut_amount = 100.0;
            }
            let ctx = app.ctx().clone();
            if let Some(hosted) = hosted_mut(app) {
                hosted.workspace_mut().load(&ctx, settings);
            }
        }
        app.ui.status = format!("Loaded the LUT \"{name}\"");
        app.ui.status_error = false;
        Ok(json!({"lut": name}))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_presets_are_valid_and_name_existing_luts() {
        let names = raw_luts::names();
        for preset in builtin_presets() {
            assert!(preset.settings.validate().is_ok(), "{}", preset.name);
            assert!(preset.settings.lut.is_empty() || names.contains(&preset.settings.lut), "{} names a missing LUT", preset.name);
        }
    }

    #[test]
    fn a_cube_file_parses_and_a_broken_one_is_refused() {
        let cube = "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
        assert!(parse_lut("ok", cube.as_bytes()).is_ok());
        assert!(parse_lut("broken", b"LUT_3D_SIZE 9999\n").is_err());
    }
}
