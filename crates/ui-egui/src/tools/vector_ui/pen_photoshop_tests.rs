//! The Pen as Photoshop draws with it: ⌘↩ loads the path being drawn, or the one just closed, as a selection; the
//! pointer's hotspot is the nib of the pen icon; the pointer shows the close circle over the first anchor; and an
//! ⌥-click on the last anchor removes its outgoing handle, so the next segment starts straight.

use craft::behavior::{Cursor, CursorBadge, PaintedCursor};
use egui::{Key, Modifiers};
use egui_kittest::Harness;

use super::pen_draw_tests::{click, drag, hover};
use super::test_input::harness;
use crate::NephclickPhotoLabApp;
use crate::state::Tool;

fn has_selection(h: &Harness<'static, Box<NephclickPhotoLabApp>>) -> bool {
    h.state().session.active().is_some_and(|st| st.doc.selection.is_some())
}

fn cursor_badge(h: &Harness<'static, Box<NephclickPhotoLabApp>>) -> Option<CursorBadge> {
    match h.state().canvas_tools.last_response.as_ref().and_then(|r| r.cursor.clone()) {
        Some(Cursor::Painted(PaintedCursor::Tool { badge, .. })) => badge,
        _ => None,
    }
}

#[test]
fn cmd_enter_after_closing_the_path_makes_a_selection() {
    let mut h = harness(Tool::Pen);
    for p in [[20.0, 20.0], [120.0, 20.0], [120.0, 80.0], [20.0, 20.0]] {
        click(&mut h, p);
    }
    assert!(h.state().ui.pen.is_none(), "clicking the first anchor closed the path");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    h.run_steps(2);
    assert!(has_selection(&h), "⌘↩ loaded the closed path as a selection: {}", h.state().ui.status);
}

#[test]
fn cmd_enter_while_drawing_makes_a_selection() {
    let mut h = harness(Tool::Pen);
    for p in [[20.0, 20.0], [120.0, 20.0], [120.0, 80.0]] {
        click(&mut h, p);
    }
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    h.run_steps(2);
    assert!(has_selection(&h), "⌘↩ loaded the path being drawn as a selection: {}", h.state().ui.status);
    assert!(h.state().ui.pen.is_none(), "and ended it");
}

#[test]
fn the_pointer_shows_the_close_circle_over_the_first_anchor() {
    let mut h = harness(Tool::Pen);
    for p in [[20.0, 20.0], [120.0, 20.0], [120.0, 80.0]] {
        click(&mut h, p);
    }
    hover(&mut h, [60.0, 60.0], Modifiers::NONE);
    assert_eq!(cursor_badge(&h), None, "no badge away from the first anchor");
    hover(&mut h, [21.0, 21.0], Modifiers::NONE);
    assert_eq!(cursor_badge(&h), Some(CursorBadge::Close), "the close circle over the first anchor");
}

#[test]
fn alt_click_on_the_last_anchor_removes_its_outgoing_handle() {
    let mut h = harness(Tool::Pen);
    click(&mut h, [20.0, 50.0]);
    // A smooth anchor with handles, dragged out to the right.
    drag(&mut h, &[[100.0, 50.0], [140.0, 50.0]], None);
    let last = |h: &Harness<'static, Box<NephclickPhotoLabApp>>| h.state().ui.pen.as_ref().and_then(|p| p.draft.knots.last().copied());
    let before = last(&h).expect("a draft");
    assert!((before.out_ctrl.x - before.anchor.x).abs() > 1.0, "the drag pulled out a handle");
    drag(&mut h, &[[100.0, 50.0]], Some(0));
    let after = last(&h).expect("still drawing");
    assert_eq!(h.state().ui.pen.as_ref().map(|p| p.draft.knots.len()), Some(2), "no anchor was added");
    assert!((after.out_ctrl - after.anchor).hypot() < 0.5, "the outgoing handle is gone: {after:?}");
    assert!((after.in_ctrl.x - after.anchor.x).abs() > 1.0, "the incoming handle stays");
}

