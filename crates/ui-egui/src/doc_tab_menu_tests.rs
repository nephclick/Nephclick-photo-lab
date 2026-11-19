//! #11403: each row of the document tab's right-click menu runs a real command on the clicked tab.

use nephclick_photolab_doc::DocId;
use serde_json::json;

use crate::canvas::tabs::tab_context_items;
use crate::{NephclickPhotoLabApp, Services};

/// The file rows name the clicked tab by its index (what `file.close`, `file.closeOthers` and
/// `file.revealInFinder` take); Move to New Window names it by its id.
#[test]
fn each_row_names_its_command_and_the_clicked_tabs_document() {
    let items = tab_context_items(DocId(7), 1, 3, true);
    let menu = crate::menus::menu_rows(&NephclickPhotoLabApp::new(nephclick_photolab_engine::Session::new(), Services::default()));
    let want = [
        ("Consolidate All to Here", "window.arrange.consolidateAllToTabs", json!({})),
        ("Close", "file.close", json!({"document": 1})),
        ("Close All", "file.closeAll", json!({})),
        ("Close Others", "file.closeOthers", json!({"document": 1})),
        ("Move to New Window", "window.arrange.floatInWindow", json!({"document": 7})),
        ("New Document…", "file.new", json!({})),
        ("Open Document…", "file.open", json!({})),
        (crate::menus::reveal_label(), "file.revealInFinder", json!({"document": 1})),
    ];
    for ((label, id, params, _), (wl, wi, wp)) in items.iter().zip(want) {
        assert_eq!((*label, *id, params), (wl, wi, &wp));
        assert!(menu.iter().any(|row| row.command.id == *id) || nephclick_photolab_engine::commands::find(id).is_some(), "{id} is a command");
    }
}

fn app_with(n: usize) -> (Box<NephclickPhotoLabApp>, egui::Context) {
    let mut app = NephclickPhotoLabApp::new(nephclick_photolab_engine::Session::new(), Services::default());
    for i in 0..n {
        app.run("file.new", json!({"width": 100 + i as u32 * 10, "height": 80})).unwrap();
    }
    app.sync_views();
    let ctx = app.ctx().clone();
    (app, ctx)
}

/// Closing a tab once document ids no longer match tab positions closed nothing and reported
/// "no active document": the tab passed its document id where `file.close` takes an index.
#[test]
fn close_row_closes_the_clicked_tab_after_ids_and_indices_diverge() {
    let (mut app, ctx) = app_with(3);
    crate::menus::invoke(&mut app, &ctx, "file.close", json!({"document": 0})).unwrap();
    let docs: Vec<DocId> = app.session.documents().iter().map(|d| d.doc.id).collect();
    assert_eq!(docs.len(), 2);
    // Every remaining id is now larger than the number of open documents.
    assert!(docs.iter().all(|id| id.0 as usize >= docs.len()), "{docs:?}");
    let last = docs[1];
    let items = tab_context_items(last, 1, docs.len(), false);
    let (_, command, params, _) = items.into_iter().find(|(label, ..)| *label == "Close").unwrap();
    crate::menus::invoke(&mut app, &ctx, command, params).unwrap();
    let left: Vec<DocId> = app.session.documents().iter().map(|d| d.doc.id).collect();
    assert_eq!(left, vec![docs[0]], "the clicked tab closed, the other stayed");
}
