//! Select › AI Selection Models…: install, check and remove the offline AI selection models.
//!
//! The installs run in the background (`nephclick_photolab_ai::model_install`); `select.aiModels` answers what is
//! installed and how a running install is doing, so the UI and agents can show its progress.

use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

fn always(_: &Session) -> std::result::Result<(), String> {
    Ok(())
}

/// The pack a command's `pack` param names.
#[cfg(not(target_arch = "wasm32"))]
fn pack(cmd: &str, p: &Value) -> Result<nephclick_photolab_ai::model_install::Pack> {
    let name = p.get("pack").and_then(Value::as_str).unwrap_or("");
    nephclick_photolab_ai::model_install::Pack::parse(name)
        .ok_or_else(|| EngineError::BadParams { cmd: cmd.to_owned(), msg: "\"pack\" must be \"subject\" or \"prompt\"".to_owned() })
}

#[cfg(not(target_arch = "wasm32"))]
fn status(_: &mut Session, _: &Value) -> Result<Value> {
    Ok(nephclick_photolab_ai::model_install::status())
}

#[cfg(not(target_arch = "wasm32"))]
fn install(_: &mut Session, p: &Value) -> Result<Value> {
    let pack = pack("select.aiModels.install", p)?;
    nephclick_photolab_ai::model_install::start(pack).map_err(|e| EngineError::Other(e.to_string()))?;
    Ok(json!({"started": true, "status": nephclick_photolab_ai::model_install::status()}))
}

#[cfg(not(target_arch = "wasm32"))]
fn cancel(_: &mut Session, _: &Value) -> Result<Value> {
    nephclick_photolab_ai::model_install::cancel();
    Ok(json!({"cancelling": true}))
}

#[cfg(not(target_arch = "wasm32"))]
fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    let pack = pack("select.aiModels.remove", p)?;
    nephclick_photolab_ai::model_install::remove(pack).map_err(|e| EngineError::Other(e.to_string()))?;
    // A cached Select Subject made by the removed model is not reused.
    s.subject_cache = Default::default();
    Ok(json!({"removed": true, "status": nephclick_photolab_ai::model_install::status()}))
}

#[cfg(target_arch = "wasm32")]
fn web(_: &mut Session, _: &Value) -> Result<Value> {
    Err(EngineError::Other("AI selection models are installed in the desktop app".into()))
}

#[cfg(target_arch = "wasm32")]
use web as status;
#[cfg(target_arch = "wasm32")]
use web as install;
#[cfg(target_arch = "wasm32")]
use web as cancel;
#[cfg(target_arch = "wasm32")]
use web as remove;

/// The AI selection model commands.
pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "select.aiModels",
            label: "AI Selection Models…",
            menu: &["Select"],
            shortcut: None,
            params: r##"{} → {"modelsDir":str,"subject":{"installed":bool,"bytes":n},"prompt":{"installed":bool,"models":[str]},"progress":{"running":"subject"|"prompt"|null,"stage":str,"done":bytes,"total":bytes,"error":str?,"finished":"subject"|"prompt"|null}} (the menu opens the models window)"##,
            enabled: always,
            run: status,
            journal: false,
        },
        CommandSpec {
            id: "select.aiModels.install",
            label: "Install AI Selection Models",
            menu: &[],
            shortcut: None,
            params: r##"{"pack":"subject|prompt"} (subject: the offline Select Subject model, about 170 MB; prompt: Python, PyTorch, Transformers, SAM 2.1 Tiny and Grounding DINO Tiny for click, rectangle and text selection, about 2 GB. Runs in the background: poll select.aiModels)"##,
            enabled: always,
            run: install,
            journal: false,
        },
        CommandSpec {
            id: "select.aiModels.cancel",
            label: "Cancel AI Model Install",
            menu: &[],
            shortcut: None,
            params: "{}",
            enabled: always,
            run: cancel,
            journal: false,
        },
        CommandSpec {
            id: "select.aiModels.remove",
            label: "Remove AI Selection Models",
            menu: &[],
            shortcut: None,
            params: r##"{"pack":"subject|prompt"}"##,
            enabled: always,
            run: remove,
            journal: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_packs_are_refused_without_a_panic() {
        let mut s = Session::new();
        for p in [json!({}), json!({"pack": 3}), json!({"pack": "everything"})] {
            assert!(install(&mut s, &p).is_err());
            assert!(remove(&mut s, &p).is_err());
        }
        assert!(status(&mut s, &json!({})).is_ok());
    }
}
