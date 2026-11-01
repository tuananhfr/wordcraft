//! Cloud comments through the app with a fake `erpHost`: anchors of the selection, marks of the
//! threads the shell shows (never in the document), clicks, focus, and placing threads again after
//! the other page's edits in the co-editing room.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::{Value, json};
use wordcraft_doc::StoryRef;

use super::tests::{Page, hand_in, web_app};
use super::tests_live::{exchange, page};
use super::tests_offered::click;
use super::{CommentMsg, HostMode, HostOpen};
use crate::WordApp;
use crate::table_resize::tests::{H, steps};

fn select(app: &mut WordApp, from: usize, to: usize) {
    app.run("select.range", json!({"anchor": {"block": 0, "off": from}, "focus": {"block": 0, "off": to}})).expect("select");
}

/// The anchor `anchorOfSelection` gives after a frame.
fn anchor_now(app: &mut WordApp, page: &Page) -> Value {
    app.host_tick(&page.ctx, 0.0);
    app.host_observe(0.0);
    page.link.anchor_of_selection()
}

fn resolve(app: &mut WordApp, page: &Page, threads: Value) -> Value {
    let got: Rc<RefCell<Option<Value>>> = Rc::default();
    let slot = got.clone();
    page.link.comment_message(CommentMsg::Resolve(threads, Box::new(move |v| *slot.borrow_mut() = Some(v))));
    app.host_tick(&page.ctx, 0.0);
    got.take().expect("answered on the next frame")
}

fn commenting() -> (WordApp, Page) {
    let (mut app, page) = web_app();
    page.link.set_comments();
    hand_in(&page, "d1", "Biên bản", "Đoạn một hai");
    app.host_tick(&page.ctx, 0.0);
    page.host.take_calls();
    (app, page)
}

#[test]
fn the_selections_anchor_is_kept_for_the_shell_and_marks_never_reach_the_document() {
    let (mut app, page) = commenting();
    let b = "Đoạn ".len();
    select(&mut app, b, b + "một".len());
    let anchor = anchor_now(&mut app, &page);
    assert_eq!(anchor["v"], 1);
    assert_eq!(anchor["soft"]["quote"], "một");
    assert_eq!(anchor["label"], "“một”");
    let (doc, rev) = (app.session.doc.clone(), app.session.rev());
    page.link.comment_message(CommentMsg::Show(json!([
        {"id": "t1", "anchor": anchor, "status": "open"},
        {"id": "t2", "anchor": anchor, "status": "resolved"},
        {"id": "t3", "anchor": {"kind": "document"}, "status": "open"},
    ])));
    app.host_tick(&page.ctx, 0.0);
    app.host.marks.refresh(&app.session);
    let placed: Vec<&str> = app.host.marks.placed().iter().map(|(id, ..)| id.as_str()).collect();
    assert_eq!(placed, ["t1"], "only open threads with an anchor of this editor");
    assert!(app.session.doc == doc && app.session.rev() == rev, "marks are not part of the document");
    app.run("file.save", json!({})).expect("save");
    let bytes = page.host.sent.borrow()[0].bytes.clone();
    let saved = wordcraft_engine::io::open_bytes("x.docx", &bytes).expect("open");
    assert!(saved.comments.is_empty());
    assert_eq!(saved.plain_text(StoryRef::Body), "Đoạn một hai");
    // Another document in front: nothing to anchor there.
    app.run("file.new", json!({})).expect("new");
    assert_eq!(anchor_now(&mut app, &page), Value::Null);
}

#[test]
fn focusing_a_thread_shows_its_document_and_selects_its_text() {
    let (mut app, page) = commenting();
    let b = "Đoạn một ".len();
    select(&mut app, b, b + "hai".len());
    let anchor = anchor_now(&mut app, &page);
    app.run("file.new", json!({})).expect("new");
    page.link.comment_message(CommentMsg::Focus(anchor, Some("t9".into())));
    app.host_tick(&page.ctx, 0.0);
    assert_eq!(app.session.doc.plain_text(StoryRef::Body), "Đoạn một hai", "the shell's document is in front");
    let (a, f) = app.session.sel.ordered();
    assert_eq!((a.off, f.off), (b, b + "hai".len()));
    assert_eq!(app.host.marks.focused.as_deref(), Some("t9"));
    assert!(app.canvas.scroll_to_caret);
}

/// The web app in a window with the shell's comments and one thread on "hai".
fn window() -> (H, Page) {
    let (mut app, page) = commenting();
    let b = "Đoạn một ".len();
    select(&mut app, b, b + "hai".len());
    let anchor = anchor_now(&mut app, &page);
    app.run("caret.docStart", json!({})).expect("caret");
    page.link.comment_message(CommentMsg::Show(json!([{"id": "t1", "anchor": anchor, "status": "open"}])));
    let mut h: H = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 900.0)).with_step_dt(0.25).build_ui_state(
        |ui, app: &mut WordApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    steps(&mut h, 6);
    (h, page)
}

#[test]
fn a_click_on_a_mark_names_its_thread_to_the_shell_and_still_moves_the_caret() {
    let (mut h, page) = window();
    let hits = h.state().host.marks.hits().to_vec();
    assert!(hits.len() >= 2, "a tint over the text and a marker in the margin: {hits:?}");
    let text = hits[0].1;
    page.host.take_calls();
    click(&mut h, text.center());
    assert!(page.host.take_calls().contains(&"commentClicked t1".to_string()));
    assert!(h.state().session.sel.focus.off > 0, "the click also placed the caret");
    let marker = hits[1].1;
    click(&mut h, marker.center());
    assert!(page.host.take_calls().contains(&"commentClicked t1".to_string()), "the margin marker too");
    // Away from the marks nothing is named.
    click(&mut h, egui::pos2(text.min.x - 40.0, text.max.y + 200.0));
    assert!(!page.host.take_calls().iter().any(|c| c.starts_with("commentClicked")));
}

#[test]
fn threads_move_with_the_other_pages_edits_and_are_orphaned_when_their_text_goes() {
    let mut pages = [page(), page()];
    for (_, p) in &pages {
        p.link.set_comments();
    }
    let mut locks = Vec::new();
    let b = "Đoạn ".len();
    select(&mut pages[0].0, b, b + "một".len());
    let anchor = {
        let (app, p) = &mut pages[0];
        anchor_now(app, p)
    };
    let threads = json!([{"id": "t1", "anchor": anchor, "status": "open"}]);
    {
        let (app, p) = &mut pages[0];
        assert_eq!(resolve(app, p, threads.clone()), json!([{"id": "t1", "threadId": "t1", "status": "open"}]), "unchanged");
    }
    // The other page adds a paragraph above.
    pages[1].0.run("caret.docStart", json!({})).expect("caret");
    pages[1].0.run("text.newParagraph", json!({})).expect("enter");
    exchange(&mut pages, &mut locks, 0.0);
    exchange(&mut pages, &mut locks, 20_000.0);
    for (app, p) in &mut pages {
        let moved = resolve(app, p, threads.clone());
        assert_eq!(moved[0]["status"], "open");
        assert_eq!(moved[0]["anchor"]["soft"]["block"], 1, "{moved}");
        assert_eq!(moved[0]["anchor"]["region"], anchor["region"], "the same paragraph");
    }
    // The other page replaces the paragraph's text.
    pages[1].0.run("select.range", json!({"anchor": {"block": 1, "off": 0}, "focus": {"block": 1, "off": usize::MAX}})).expect("select");
    pages[1].0.run("text.insert", json!({"text": "Khác hẳn"})).expect("type");
    exchange(&mut pages, &mut locks, 20_000.0);
    exchange(&mut pages, &mut locks, 40_000.0);
    for (app, p) in &mut pages {
        assert_eq!(app.session.doc.plain_text(StoryRef::Body), "\nKhác hẳn");
        assert_eq!(resolve(app, p, threads.clone()), json!([{"id": "t1", "threadId": "t1", "status": "orphaned"}]));
    }
}

#[test]
fn without_the_shells_comments_nothing_changes() {
    let (mut app, page) = web_app();
    hand_in(&page, "d1", "Biên bản", "Đoạn một hai");
    app.host_tick(&page.ctx, 0.0);
    select(&mut app, 0, 4);
    assert_eq!(anchor_now(&mut app, &page), Value::Null);
    page.link.comment_message(CommentMsg::Show(json!([{"id": "t1", "anchor": {"v": 1}, "status": "open"}])));
    app.host_tick(&page.ctx, 0.0);
    assert!(app.host.marks.is_empty());
    // A shell with comments but no document open yet: no answers, nothing orphaned.
    let (mut fresh, page) = web_app();
    page.link.set_comments();
    assert_eq!(resolve(&mut fresh, &page, json!([{"id": "t1", "anchor": {"v": 1}}])), json!([]));
}

#[test]
fn comment_mode_points_to_the_shells_comments_when_it_has_them() {
    for (comments, notice) in
        [(true, "Bạn có thể nhận xét, còn nội dung tài liệu chỉ xem được."), (false, "Nhận xét trên web sẽ có sau, hiện chỉ xem được")]
    {
        let (mut app, page) = web_app();
        if comments {
            page.link.set_comments();
        }
        page.link.open(HostOpen {
            doc_id: "d1".into(),
            version: 1,
            title: "Báo cáo".into(),
            format: "docx".into(),
            bytes: super::tests::docx("Xin chào"),
            mode: HostMode::Comment,
            ..Default::default()
        });
        app.host_tick(&page.ctx, 0.0);
        assert_eq!(app.run("text.insert", json!({"text": "x"})).unwrap_err(), notice);
    }
}
