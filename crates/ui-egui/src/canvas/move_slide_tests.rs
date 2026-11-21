//! A Move drag of a shape or type layer slides one picture of the layer over the document rendered once without it,
//! instead of rendering the document again at every offset (which fell behind the pointer: the layer flickered or
//! vanished until the release).

use egui::{Modifiers, PointerButton};
use serde_json::json;

use crate::guide_cursor_tests::{harness, screen};
use crate::state::Tool;

#[test]
fn dragging_a_shape_slides_its_picture_over_the_document_without_it() {
    let mut h = harness(Tool::Move);
    h.state_mut().run("shape.create", json!({"kind": "rect", "rect": [40, 40, 100, 80], "fill": "#e02020"})).unwrap();
    let layer = h.state().session.active().and_then(|st| st.active_layer).unwrap();
    h.run_steps(2);
    let from = screen(&h, 90.0, 80.0);
    h.event(egui::Event::PointerMoved(from));
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    for k in 1..=5 {
        h.event(egui::Event::PointerMoved(screen(&h, 90.0 + 8.0 * k as f32, 80.0)));
        h.run_steps(1);
    }
    let app = h.state_mut();
    let idx = app.session.active_index().unwrap();
    let (shown, key) = crate::move_ui::display_doc(app, idx).expect("a preview while dragging");
    assert!(!shown.layer(layer).unwrap().visible, "the shape is hidden under its sliding picture");
    let (_, again) = crate::move_ui::display_doc(app, idx).unwrap();
    assert_eq!(key, again, "one picture for the whole drag: nothing is rendered again as the pointer moves");
    assert_eq!(crate::move_ui::shown_layer_offset(app, shown.id, app.session.active().unwrap().revision, layer), Some((40, 0)));
    let to = screen(&h, 130.0, 80.0);
    h.event(egui::Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    let st = h.state().session.active().unwrap();
    assert!(st.doc.layer(layer).unwrap().visible, "the real layer is back after the release");
}
