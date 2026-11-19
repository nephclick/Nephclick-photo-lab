//! The mark a Layers panel row carries from the document: a warning on an adjustment Nephclick PhotoLab
//! keeps as imported but cannot render (#6541).

use craft::layers::ui::tree::LayerTree;
use nephclick_photolab_doc::{Adjustment, Layer, LayerContent, LayerId};
use serde_json::json;

use crate::NephclickPhotoLabApp;

#[test]
fn an_unsupported_adjustment_row_carries_a_warning_marker() {
    let mut s = nephclick_photolab_engine::Session::new();
    s.execute("file.new", json!({"width": 64, "height": 48})).unwrap();
    let plain = LayerId(s.execute("layer.new.layer", json!({"name": "plain"})).unwrap()["layer"].as_u64().unwrap());
    let mystery = Layer::new("Mystery", LayerContent::Adjustment(Adjustment::Unsupported { psd_key: "xyzw".into(), raw: vec![1, 2, 3].into() }));
    let mystery_id = mystery.id;
    s.edit("unsupported fixture", |doc, active| {
        doc.layers.push(mystery);
        *active = Some(mystery_id);
        Ok(())
    })
    .unwrap();
    let app = NephclickPhotoLabApp::new(s, crate::Services::default());
    let tree = super::layers_tree::PhotoLayers::new(&app, app.session.active().unwrap(), &Default::default(), false, None);
    assert!(tree.info(&mystery_id).warning.is_some(), "the panel marks the row");
    assert!(tree.info(&plain).warning.is_none());
}
