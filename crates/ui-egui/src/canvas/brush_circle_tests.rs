//! The painting tools show the brush's circle over the canvas (Preferences › Cursors › Normal Brush Tip, the default):
//! the Brush, the Eraser, the Healing Brush and the Blur tool. Driven through the real app; the assertions read the
//! cursor the tool asked the tool host for.

use craft::behavior::{Cursor, PaintedCursor};
use egui::Modifiers;

use crate::guide_cursor_tests::{harness, hover};
use crate::state::Tool;

fn kind(c: &Option<Cursor>) -> String {
    match c {
        Some(Cursor::Painted(p)) => format!("{:?}", std::mem::discriminant(p)).chars().take(60).collect::<String>() + &format!(" {p:?}").chars().take(120).collect::<String>(),
        other => format!("{other:?}"),
    }
}

#[test]
fn painting_tools_show_the_brush_circle() {
    for tool in [Tool::Brush, Tool::Eraser, Tool::Healing, Tool::Blur] {
        let mut h = harness(tool);
        h.state_mut().run("tools.setBrush", serde_json::json!({"size": 40})).unwrap();
        hover(&mut h, 120.0, 120.0, Modifiers::NONE);
        hover(&mut h, 122.0, 121.0, Modifiers::NONE);
        let cursor = h.state().canvas_tools.last_response.as_ref().and_then(|r| r.cursor.clone());
        let circle = matches!(cursor, Some(Cursor::Painted(PaintedCursor::Outline { .. } | PaintedCursor::Circle { .. })));
        assert!(circle, "{tool:?} shows {}", kind(&cursor));
    }
}
