//! `erp-host.md` v2 rule 12 past the commands: a read-only document's interface doesn't offer
//! what it would refuse (editing dialogs, drags), and a comment draft survives `setMode`.

use egui::Event;
use serde_json::json;
use wordcraft_doc::StoryRef;

use super::HostMode;
use super::tests::{Page, docx, web_app};
use super::tests_access::{READ_ONLY, opened, read_only_text};
use crate::WordApp;
use crate::dialogs::Dialog;
use crate::table_resize::tests::{H, button, cell, screen, steps, table_window};

/// Dialogs whose OK only changes the document.
const EDITING: [&str; 24] = [
    "font",
    "paragraph",
    "tabs",
    "borders",
    "replace",
    "insertTable",
    "pageSetup",
    "link",
    "bookmark",
    "watermark",
    "newStyle",
    "manageStyles",
    "symbol",
    "columns",
    "field",
    "toc",
    "createSource",
    "lineNumbers",
    "hyphenation",
    "language",
    "restrictEditing",
    "pasteSpecial",
    "encryptPassword",
    "newRecipientList",
];

/// Dialogs that only show something.
const VIEWING: [&str; 6] = ["find", "goto", "wordCount", "zoom", "about", "shortcuts"];

fn offered_dialog(app: &WordApp, name: &str) -> bool {
    crate::access_gate::offered(app, "ui.dialog", &json!({"name": name}))
}

#[test]
fn an_editing_dialog_cannot_open_in_view_mode() {
    for mode in READ_ONLY {
        let (mut app, _page) = opened(mode, true);
        for name in EDITING {
            assert!(!offered_dialog(&app, name), "{mode:?}: {name} is offered");
            app.status_msg = None;
            let r = app.run("ui.dialog", json!({"name": name})).unwrap();
            assert!(app.dialog.is_none(), "{mode:?}: {name} opened");
            assert_eq!(r["dialog"], serde_json::Value::Null);
            assert_eq!(app.status_msg.as_ref().map(|(s, _)| s.as_str()), Some(read_only_text(mode)), "{mode:?}: {name} says why");
        }
        for name in VIEWING {
            assert!(offered_dialog(&app, name), "{mode:?}: {name} is not offered");
            app.run("ui.dialog", json!({"name": name})).unwrap();
            assert_eq!(app.dialog.as_ref().map(Dialog::name), Some(name), "{mode:?}");
            app.dialog = None;
        }
        // Opened without `Dialog::open` (a style's Modify…): closed before it is drawn.
        app.dialog = Dialog::modify_style(&app, "Normal");
        assert!(app.dialog.is_some());
        crate::access_gate::close_refused(&mut app);
        assert!(app.dialog.is_none(), "{mode:?}: Modify Style stays closed");
    }
}

/// An editing dialog left open when `setMode` turns editing off closes; Find stays.
#[test]
fn set_mode_closes_an_open_editing_dialog() {
    let (mut app, page) = opened(HostMode::Edit, true);
    for (name, stays) in [("font", false), ("paragraph", false), ("find", true)] {
        page.link.set_mode("d1", HostMode::Edit, true);
        app.host_tick(&page.ctx, 0.0);
        assert!(offered_dialog(&app, name));
        app.run("ui.dialog", json!({"name": name})).unwrap();
        assert_eq!(app.dialog.as_ref().map(Dialog::name), Some(name));
        page.link.set_mode("d1", HostMode::View, true);
        app.host_tick(&page.ctx, 0.0);
        crate::access_gate::close_refused(&mut app);
        assert_eq!(app.dialog.is_some(), stays, "{name}");
        app.dialog = None;
    }
}

/// A real mouse drag from `from` to `to` (screen points), in a few moves, released at the end.
fn drag(h: &mut H, from: egui::Pos2, to: egui::Pos2) {
    h.event(Event::PointerMoved(from));
    steps(h, 2);
    button(h, from, true);
    h.step();
    for k in 1..=5 {
        h.event(Event::PointerMoved(from + (to - from) * (k as f32 / 5.0)));
        h.step();
    }
}

#[test]
fn a_drag_does_not_start_on_a_table_border_or_ruler_marker_in_view_mode() {
    let mut h = table_window();
    h.state_mut().session.access.read_only = true;
    let a = cell(&mut h, 0, 0);
    let (doc, rev) = (h.state().session.doc.clone(), h.state().session.rev());

    // The column border on the page: no resize pointer, no drag, no preview.
    let start = screen(&mut h, a.right(), a.y + a.h / 2.0);
    h.event(Event::PointerMoved(start));
    steps(&mut h, 2);
    assert_ne!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeColumn, "no resize arrow");
    let end = screen(&mut h, a.right() + 30.0, a.y + a.h / 2.0);
    drag(&mut h, start, end);
    assert!(h.state().canvas.table_resize.is_none(), "the border drag didn't start");
    button(&h, end, false);
    steps(&mut h, 3);

    // The same border's marker on the ruler.
    let marker = {
        use egui_kittest::kittest::Queryable;
        h.query_all_by_label("Độ rộng cột bảng")
            .chain(h.query_all_by_label("Table Column Width"))
            .map(|n| n.rect())
            .min_by(|p, q| p.center().x.total_cmp(&q.center().x))
            .expect("a column marker")
    };
    let from = marker.center();
    drag(&mut h, from, from + egui::vec2(60.0, 0.0));
    assert!(h.state().canvas.table_resize.is_none(), "the ruler drag didn't start");
    button(&h, from + egui::vec2(60.0, 0.0), false);
    steps(&mut h, 3);

    assert!(h.state().session.doc == doc, "the table is as it was");
    assert_eq!(h.state().session.rev(), rev);
    assert_eq!(cell(&mut h, 0, 0), a);
}

#[test]
fn a_drag_does_not_start_on_a_shape_in_view_mode() {
    use wordcraft_doc::para::{Float, InlineObject, ShapeKind, Wrap};
    use wordcraft_doc::{Document, Pos};
    let mut a = WordApp::new(wordcraft_engine::Session::new(Document::from_text("Xin chào")), Default::default());
    let shape = InlineObject::Shape {
        kind: ShapeKind::Rectangle,
        w: 120.0,
        h: 60.0,
        fill: None,
        stroke: Some(wordcraft_doc::Rgb(0xFF, 0, 0)),
        stroke_width: 3.0,
        float: Float { wrap: Wrap::InFrontOfText, ..Default::default() },
        story: None,
        freeform: None,
        effects: Default::default(),
        extra: Default::default(),
    };
    a.session.doc.insert_object(&Pos::body(0, 0), shape, &Default::default()).unwrap();
    a.session.access.read_only = true;
    let mut h: H = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 900.0)).with_step_dt(0.25).build_ui_state(
        |ui, app: &mut WordApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        a,
    );
    steps(&mut h, 6);
    let (doc, rev) = (h.state().session.doc.clone(), h.state().session.rev());
    let o = h.state_mut().session.layout().object(&Pos::body(0, 0), 0).expect("laid out");
    let at = |h: &mut H, x: f32, y: f32| crate::canvas::page_to_screen(h.state_mut(), 0, x, y).unwrap();
    let start = at(&mut h, o.rect.x + o.rect.w / 2.0, o.rect.y + 4.0);
    let end = at(&mut h, o.rect.x + o.rect.w / 2.0, o.rect.y + 204.0);
    drag(&mut h, start, end);
    assert!(h.state().canvas.obj_drag.is_none(), "no move started");
    button(&h, end, false);
    steps(&mut h, 3);
    // Selected (that is reading), with no handle to resize it by.
    assert!(crate::objects::selected(h.state()).is_some(), "the click still selects the shape");
    let corner = at(&mut h, o.rect.right(), o.rect.bottom());
    drag(&mut h, corner, corner + egui::vec2(80.0, 80.0));
    assert!(h.state().canvas.obj_drag.is_none(), "no resize started");
    button(&h, corner + egui::vec2(80.0, 80.0), false);
    steps(&mut h, 3);
    assert!(h.state().session.doc == doc, "the shape is where it was");
    assert_eq!(h.state().session.rev(), rev);
}

/// The web app in a window, with `d1` open for editing and one comment on its text, selected.
fn commented(page: &Page, app: WordApp) -> (H, u32) {
    page.link.open(super::HostOpen {
        doc_id: "d1".into(),
        version: 1,
        title: "Báo cáo".into(),
        format: "docx".into(),
        bytes: docx("Xin chào"),
        ..Default::default()
    });
    let mut h: H = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 900.0)).with_step_dt(0.25).build_ui_state(
        |ui, app: &mut WordApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    steps(&mut h, 6);
    h.state_mut().run("select.all", json!({})).unwrap();
    h.state_mut().run("review.newComment", json!({"text": "Gốc"})).unwrap();
    h.state_mut().run("view.commentsPane", json!({"value": false})).unwrap();
    let id = *h.state().session.doc.comments.keys().next().expect("a comment");
    steps(&mut h, 3);
    let card = h.state().canvas.balloon_rects.iter().find(|(i, _)| *i == id).map(|(_, r)| *r).expect("the comment has a balloon");
    let at = card.left_top() + egui::vec2(30.0, 8.0);
    h.event(Event::PointerMoved(at));
    button(&h, at, true);
    h.step();
    button(&h, at, false);
    steps(&mut h, 3);
    assert_eq!(h.state().canvas.balloon, Some(id), "the click selected the balloon");
    (h, id)
}

fn comment_text(app: &WordApp, id: u32) -> String {
    let part = app.session.doc.comments[&id].part;
    app.session.doc.plain_text(StoryRef::Part(part))
}

fn draft(h: &H, key: egui::Id) -> Option<String> {
    h.ctx.data(|d| d.get_temp::<String>(key))
}

/// What is typed in a balloon (an edit of the comment, a reply) and not yet posted when
/// `setMode` turns editing off is neither posted nor dropped: it comes back with editing.
#[test]
fn a_comment_draft_waits_through_view_mode_and_comes_back_in_edit() {
    for (replying, mode) in [(false, HostMode::View), (true, HostMode::View), (false, HostMode::Comment), (true, HostMode::Comment)] {
        let (app, page) = web_app();
        let (mut h, id) = commented(&page, app);
        let key = if replying { crate::panes::reply_key(id) } else { crate::panes::edit_key(id) };
        if replying {
            h.state_mut().canvas.balloon_reply = true;
            h.state_mut().canvas.balloon_focus = true;
            steps(&mut h, 2);
        }
        h.event(Event::Text(" thêm".into()));
        steps(&mut h, 2);
        let typed = draft(&h, key).expect("the field keeps a draft");
        assert!(typed.contains("thêm"), "{typed}");
        let comments = h.state().session.doc.comments.len();

        page.link.set_mode("d1", mode, true);
        steps(&mut h, 4);
        assert!(h.state().read_only());
        assert_eq!(comment_text(h.state(), id), "Gốc", "{mode:?}: nothing was posted");
        assert_eq!(h.state().session.doc.comments.len(), comments, "{mode:?}: no reply was created");
        assert_eq!(draft(&h, key), None, "{mode:?}: the draft left the field");
        // Leaving the balloon while read-only posts nothing either.
        let ctx = h.ctx.clone();
        crate::canvas::deselect_balloon(h.state_mut(), &ctx);
        steps(&mut h, 2);
        assert_eq!(comment_text(h.state(), id), "Gốc");
        assert_eq!(h.state().session.doc.comments.len(), comments);
        assert!(page.host.sends().is_empty(), "no save traffic");

        page.link.set_mode("d1", HostMode::Edit, true);
        steps(&mut h, 4);
        assert!(!h.state().read_only());
        assert_eq!(h.state().canvas.balloon, Some(id), "{mode:?}: its balloon is selected again");
        assert_eq!(h.state().canvas.balloon_reply, replying);
        assert_eq!(draft(&h, key).as_deref(), Some(typed.as_str()), "{mode:?}: the draft is back in its field");
        assert_eq!(comment_text(h.state(), id), "Gốc", "still not posted");

        // Leaving the balloon now posts it, as before the downgrade.
        let ctx = h.ctx.clone();
        crate::canvas::deselect_balloon(h.state_mut(), &ctx);
        steps(&mut h, 2);
        if replying {
            let reply = h.state().session.doc.comments.iter().find(|(_, c)| c.parent == Some(id)).map(|(k, _)| *k).expect("the reply");
            assert_eq!(comment_text(h.state(), reply), typed.trim_end());
        } else {
            assert_eq!(comment_text(h.state(), id), typed);
        }
    }
}

/// Pens, the eraser and the ink lasso only edit: not offered, and one picked up before the
/// downgrade draws nothing.
#[test]
fn drawing_tools_are_not_offered_in_view_mode() {
    for mode in READ_ONLY {
        let (mut app, page) = opened(HostMode::Edit, true);
        app.run("draw.pen", json!({})).unwrap();
        page.link.set_mode("d1", mode, true);
        app.host_tick(&page.ctx, 0.0);
        for id in ["draw.pen", "draw.pencil", "draw.highlighter", "draw.addPen", "draw.eraser", "draw.lasso"] {
            assert!(!crate::widgets::enabled(&app, id), "{mode:?}: {id} is offered");
            assert_eq!(app.run(id, json!({})).unwrap_err(), read_only_text(mode), "{mode:?}: {id}");
        }
        assert!(crate::ink::cursor(&app).is_none(), "{mode:?}: no pen pointer");
        app.run("draw.select", json!({})).unwrap();
    }
}

/// A window on "Xin chào" with all of it selected, `read_only` or not.
fn text_window(read_only: bool) -> H {
    let mut a = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::from_text("Xin chào")), Default::default());
    a.run("select.all", json!({})).unwrap();
    a.session.access.read_only = read_only;
    let mut h: H = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 900.0)).with_step_dt(0.25).build_ui_state(
        |ui, app: &mut WordApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        a,
    );
    steps(&mut h, 6);
    h
}

pub(super) fn click(h: &mut H, at: egui::Pos2) {
    h.event(Event::PointerMoved(at));
    h.step();
    button(h, at, true);
    h.step();
    button(h, at, false);
    steps(h, 3);
}

/// Split buttons (Font Color, Highlight, Bullets…) and the Styles gallery are greyed in view mode:
/// a click runs nothing and the arrow opens no menu. Without a host they work as before.
#[test]
fn split_buttons_and_the_styles_gallery_are_greyed_in_view_mode() {
    use egui_kittest::kittest::Queryable;
    for read_only in [false, true] {
        let mut h = text_window(read_only);
        for tip in ["Font Color", "Text Highlight Color", "Bullets", "Numbering"] {
            let rev = h.state().session.rev();
            let r = h.query_all_by_label(crate::tl!(tip)).map(|n| n.rect()).min_by(|p, q| p.min.y.total_cmp(&q.min.y)).expect(tip);
            h.state_mut().status_msg = None;
            click(&mut h, r.center());
            assert_eq!(h.state().session.rev() != rev, !read_only, "read-only {read_only}: {tip} ran");
            assert!(h.state().status_msg.is_none() || !read_only, "{tip}: nothing ran, nothing to refuse");
            if !read_only {
                h.state_mut().run("edit.undo", json!({})).unwrap();
            }
            click(&mut h, r.right_center() + egui::vec2(5.0, 0.0));
            assert_eq!(egui::Popup::is_any_open(&h.ctx), !read_only, "read-only {read_only}: {tip}'s menu");
            h.event(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
            steps(&mut h, 2);
        }
        let heading = {
            let app = h.state();
            crate::i18n::style_name(app.session.doc.styles.get("Heading1").expect("Heading 1"))
        };
        let r = h.get_by_label(&heading).rect();
        let rev = h.state().session.rev();
        click(&mut h, r.center());
        assert_eq!(h.state().session.rev() != rev, !read_only, "read-only {read_only}: the Styles gallery ran");
    }
}

/// A comment edit typed in the Comments pane and not yet posted at a downgrade is kept, and its
/// field gets the keyboard back with editing; leaving it then posts it.
#[test]
fn a_comments_pane_draft_gets_its_field_back_in_edit() {
    let (mut app, page) = web_app();
    page.link.open(super::HostOpen {
        doc_id: "d1".into(),
        version: 1,
        title: "Báo cáo".into(),
        format: "docx".into(),
        bytes: docx("Xin chào"),
        ..Default::default()
    });
    app.host_tick(&page.ctx, 0.0);
    app.run("select.all", json!({})).unwrap();
    app.run("review.newComment", json!({"text": "Gốc"})).unwrap();
    app.run("view.commentsPane", json!({"value": true})).unwrap();
    let id = *app.session.doc.comments.keys().next().expect("a comment");
    let mut h: H = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 900.0)).with_step_dt(0.25).build_ui_state(
        |ui, app: &mut WordApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    steps(&mut h, 4);
    let ctx = h.ctx.clone();
    crate::canvas::deselect_balloon(h.state_mut(), &ctx);
    h.ctx.data_mut(|d| d.insert_temp(crate::panes::edit_focus_key(id), true));
    steps(&mut h, 2);
    h.event(Event::Text(" thêm".into()));
    steps(&mut h, 2);
    let typed = draft(&h, crate::panes::edit_key(id)).expect("the pane's field keeps a draft");

    page.link.set_mode("d1", HostMode::View, true);
    steps(&mut h, 4);
    assert_eq!(comment_text(h.state(), id), "Gốc", "nothing was posted");
    assert!(h.ctx.memory(|m| m.focused()).is_none(), "the field let go of the keyboard");

    page.link.set_mode("d1", HostMode::Edit, true);
    steps(&mut h, 4);
    assert!(h.ctx.memory(|m| m.focused()).is_some(), "the field has the keyboard again");
    assert_eq!(draft(&h, crate::panes::edit_key(id)).as_deref(), Some(typed.as_str()));
    h.event(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
    steps(&mut h, 3);
    assert_eq!(comment_text(h.state(), id), typed, "leaving the field posts it");
}
