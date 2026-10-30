//! `erp-host.md` v2 with a fake `erpHost`: view and comment modes refuse every edit (rules
//! 12–14), `setMode` both ways, `canDownload`, `preserveView` and `setUser`.

use std::cell::RefCell;
use std::rc::Rc;

use egui::{Event, Key, Modifiers};
use serde_json::json;
use wordcraft_doc::{Pos, StoryRef};
use wordcraft_engine::Selection;

use super::tests::{Page, body, docx, type_text, web_app};
use super::{HostMode, HostOpen, SaveReason};
use crate::WordApp;

pub(super) const READ_ONLY: [HostMode; 2] = [HostMode::View, HostMode::Comment];

/// What a refused edit says in `mode`: comment mode is view-only for now and says so.
pub(super) fn read_only_text(mode: HostMode) -> &'static str {
    match mode {
        HostMode::Comment => "Nhận xét trên web sẽ có sau, hiện chỉ xem được",
        _ => "Tài liệu này chỉ được xem, không sửa được.",
    }
}

fn open_as(page: &Page, doc_id: &str, text: &str, mode: HostMode, can_download: bool) {
    page.link.open(HostOpen {
        doc_id: doc_id.into(),
        version: 1,
        title: "Báo cáo".into(),
        format: "docx".into(),
        bytes: docx(text),
        mode,
        can_download,
        ..Default::default()
    });
}

/// The app with a host document open in `mode`, its calls so far taken.
pub(super) fn opened(mode: HostMode, can_download: bool) -> (WordApp, Page) {
    let (mut app, page) = web_app();
    open_as(&page, "d1", "Xin chào", mode, can_download);
    app.host_tick(&page.ctx, 0.0);
    assert_eq!(page.host.take_calls(), vec!["opened d1 true "]);
    (app, page)
}

/// Nothing about the document changed: content, revision, undo history, and nothing told the host.
#[track_caller]
fn unchanged(app: &WordApp, page: &Page, doc: &wordcraft_doc::Document, rev: u64, undo: usize) {
    assert!(app.session.doc == *doc, "the document changed");
    assert_eq!(app.session.rev(), rev);
    assert_eq!(app.session.undo_depth(), undo);
    assert!(!app.session.dirty);
    assert!(page.host.sends().is_empty(), "no save traffic");
}

#[test]
fn every_command_that_edits_is_refused_in_view_and_comment() {
    for mode in READ_ONLY {
        let (mut app, page) = opened(mode, true);
        assert!(app.read_only());
        app.run("select.all", json!({})).unwrap();
        let (doc, rev, undo) = (app.session.doc.clone(), app.session.rev(), app.session.undo_depth());
        let reg = app.session.registry.clone();
        let mut families = std::collections::BTreeSet::new();
        for spec in reg.all().iter().filter(|s| s.mutates) {
            let r = app.run(spec.id, json!({"text": "x", "value": true}));
            assert!(r.is_err(), "{mode:?}: {} ran", spec.id);
            assert!(!crate::widgets::enabled(&app, spec.id), "{mode:?}: {} is offered", spec.id);
            families.insert(spec.id.split('.').next().unwrap_or_default());
        }
        // Undo and redo, find-and-replace, comments, fields and forms, paste.
        for (id, params) in [
            ("edit.undo", json!({})),
            ("edit.redo", json!({})),
            ("edit.replaceAll", json!({"find": "Xin", "replace": "Chào"})),
            ("review.newComment", json!({"text": "Ý kiến"})),
            ("review.editComment", json!({"id": 0, "text": "x"})),
            ("references.updateFields", json!({})),
            ("edit.paste", json!({"text": "dán"})),
            ("ui.changePicture", json!({})),
        ] {
            let e = app.run(id, params).unwrap_err();
            assert_eq!(e, read_only_text(mode), "{mode:?}: {id}");
        }
        unchanged(&app, &page, &doc, rev, undo);
        for family in ["text", "format", "para", "insert", "table", "review", "edit", "references", "design", "layout", "fields", "developer"] {
            assert!(families.contains(family), "the {family} family was covered");
        }
        // Navigation, selection, zoom, find and view switches still work.
        for id in ["caret.docEnd", "select.word", "view.zoom", "view.readMode", "view.printLayout"] {
            app.run(id, json!({"value": 120})).unwrap_or_else(|e| panic!("{mode:?}: {id}: {e}"));
        }
        app.run("edit.find", json!({"text": "chào"})).unwrap();
        unchanged(&app, &page, &doc, rev, undo);
    }
}

/// Keys, typed text, the input method, paste, cut and a file dropped on the page change nothing.
#[test]
fn key_input_paste_and_drop_are_refused_in_view_and_comment() {
    for mode in READ_ONLY {
        let (mut app, page) = web_app();
        let inbox: crate::Inbox = Default::default();
        let intake: Rc<RefCell<Vec<(String, bool)>>> = Rc::default();
        let seen = intake.clone();
        app.services.inbox = Some(inbox.clone());
        app.services.intake_result = Some(Box::new(move |name, _, ok| seen.borrow_mut().push((name.to_string(), ok))));
        app.canvas.want_focus = true;
        open_as(&page, "d1", "Xin chào", mode, true);
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 900.0)).build_ui_state(
            |ui, app: &mut WordApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
        h.run_steps(8);
        assert!(h.state().canvas.focused, "the canvas has the keyboard focus");
        assert!(h.state().read_only());
        let (doc, rev) = (h.state().session.doc.clone(), h.state().session.rev());
        let key = |key, modifiers| Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        let ctrl = Modifiers { command: true, ctrl: true, ..Default::default() };
        for event in [
            Event::Text("abc".into()),
            key(Key::Enter, Modifiers::NONE),
            key(Key::Backspace, Modifiers::NONE),
            key(Key::Delete, Modifiers::NONE),
            key(Key::Tab, Modifiers::NONE),
            Event::Ime(egui::ImeEvent::Preedit { text: "a".into(), active_range_chars: None }),
            Event::Ime(egui::ImeEvent::Commit("á".into())),
            Event::Paste("dán".into()),
            key(Key::A, ctrl),
            Event::Cut,
            key(Key::Z, ctrl),
            key(Key::Y, ctrl),
            key(Key::B, ctrl),
        ] {
            h.event(event);
            h.step();
        }
        inbox.lock().unwrap().extend([("a.png".to_string(), vec![0x89, b'P', b'N', b'G']), ("b.docx".to_string(), docx("khác"))]);
        h.run_steps(2);
        let app = h.state();
        assert!(app.session.doc == doc, "{mode:?}: the document changed");
        assert_eq!(app.session.rev(), rev);
        assert!(app.canvas.ime_preedit.is_empty(), "no composition is drawn");
        assert_eq!(*intake.borrow(), vec![("a.png".to_string(), false), ("b.docx".to_string(), false)]);
        assert_eq!(app.document_count(), 1, "the dropped document didn't open either");
        assert!(page.host.sends().is_empty());
        assert!(page.host.take_calls().iter().all(|c| !c.starts_with("dirty")), "never reported dirty");
    }
}

#[test]
fn view_and_comment_send_nothing_and_flush_resolves_at_once() {
    for mode in READ_ONLY {
        let (mut app, page) = opened(mode, true);
        let e = app.run("file.save", json!({})).unwrap_err();
        assert_eq!(e, read_only_text(mode), "Ctrl+S says why");
        app.host_tick(&page.ctx, 60_000.0);
        let flushed = Rc::new(std::cell::Cell::new(false));
        let f = flushed.clone();
        page.link.flush(Box::new(move || f.set(true)));
        assert!(flushed.get(), "{mode:?}: flush resolved at once");
        assert!(page.host.sends().is_empty());
        assert!(page.host.take_calls().is_empty(), "no dirty report");
    }
}

#[test]
fn set_mode_both_ways_keeps_the_document_and_its_undo_history() {
    for mode in READ_ONLY {
        let (mut app, page) = opened(HostMode::Edit, true);
        type_text(&mut app, "A");
        type_text(&mut app, " ");
        app.run("format.bold", json!({})).unwrap();
        let (text, depth) = (body(&app), app.session.undo_depth());
        assert!(depth >= 2);
        app.host_observe(100.0);
        assert_eq!(page.host.take_calls(), vec!["dirty d1 true"]);

        // The shell flushes before a downgrade; the save is still in flight when setMode lands.
        let flushed = Rc::new(std::cell::Cell::new(false));
        let f = flushed.clone();
        page.link.flush(Box::new(move || f.set(true)));
        assert!(flushed.get());
        assert_eq!(page.host.sends().len(), 1);
        page.link.set_mode("d1", mode, true);
        app.host_tick(&page.ctx, 200.0);
        assert!(app.read_only(), "{mode:?}");
        assert_eq!(body(&app), text);
        assert_eq!(app.session.undo_depth(), depth, "the undo history is kept");
        assert!(app.run("edit.undo", json!({})).is_err());
        assert!(!app.session.can_undo());
        type_text_refused(&mut app);
        page.host.answer(0, super::SaveOutcome::Saved { version: 2 });
        app.host_tick(&page.ctx, 60_000.0);
        assert_eq!(page.host.sends().len(), 1, "nothing more is sent while read-only");

        page.link.set_mode("d1", HostMode::Edit, true);
        app.host_tick(&page.ctx, 61_000.0);
        assert!(!app.read_only());
        assert_eq!(body(&app), text);
        app.run("edit.undo", json!({})).unwrap();
        assert_eq!(app.session.undo_depth(), depth - 1, "Undo picks up where it stopped");
        type_text(&mut app, "B");
        app.host_observe(62_000.0);
        app.host_tick(&page.ctx, 65_100.0);
        assert_eq!(page.host.sends().last().map(|s| s.2), Some(SaveReason::Auto), "AutoSave is back");
    }
}

/// Rule 13's exception and `setMode`'s downgrade sequence: edits made before a downgrade, with a
/// composition still being typed, stay dirty; the flush after `setMode` saves them once.
#[test]
fn a_document_downgraded_while_dirty_is_saved_by_the_next_flush() {
    for mode in READ_ONLY {
        let (mut app, page) = opened(HostMode::Edit, true);
        type_text(&mut app, "A");
        app.canvas.ime_preedit = "ế".into();
        let depth = app.session.undo_depth();
        app.host_observe(100.0);
        assert_eq!(page.host.take_calls(), vec!["dirty d1 true"]);

        page.link.set_mode("d1", mode, true);
        app.host_tick(&page.ctx, 200.0);
        assert!(app.read_only());
        assert!(body(&app).starts_with("Aế"), "{mode:?}: the composition went in: {}", body(&app));
        assert_eq!(app.session.undo_depth(), depth + 1, "as one undo step");
        assert!(app.canvas.ime_preedit.is_empty());
        app.host_observe(300.0);
        app.host_tick(&page.ctx, 60_000.0);
        assert!(page.host.sends().is_empty(), "{mode:?}: no AutoSave");
        assert!(page.host.take_calls().is_empty(), "still dirty, nothing new to report");

        let flushed = Rc::new(std::cell::Cell::new(false));
        let f = flushed.clone();
        page.link.flush(Box::new(move || f.set(true)));
        assert!(flushed.get(), "handed over");
        let sends = page.host.sends();
        assert_eq!(sends.len(), 1, "{mode:?}: one save");
        assert_eq!(sends[0].2, SaveReason::Flush);
        assert!(super::tests::sent_text(&page, 0).starts_with("Aế"), "the newest edits");

        page.host.answer(0, super::SaveOutcome::Saved { version: 2 });
        app.host_tick(&page.ctx, 61_000.0);
        app.host_observe(61_100.0);
        assert_eq!(page.host.take_calls(), vec!["dirty d1 false"], "saved: clean");
        app.host_tick(&page.ctx, 200_000.0);
        let again = Rc::new(std::cell::Cell::new(false));
        let f = again.clone();
        page.link.flush(Box::new(move || f.set(true)));
        assert!(again.get());
        assert_eq!(page.host.sends().len(), 1, "{mode:?}: no further saves");
    }
}

fn type_text_refused(app: &mut WordApp) {
    assert!(app.run("text.insert", json!({"text": "x"})).is_err());
}

#[test]
fn can_download_false_disables_save_as_export_print_and_copy() {
    for mode in [HostMode::View, HostMode::Comment, HostMode::Edit] {
        let (mut app, page) = opened(mode, false);
        app.run("select.all", json!({})).unwrap();
        for id in ["file.saveAs", "file.exportPdf", "file.exportPng", "file.print", "file.printPdf", "ui.print", "ui.share", "edit.copy", "edit.cut"]
        {
            assert!(!crate::widgets::enabled(&app, id), "{mode:?}: {id} is offered");
            let e = app.run(id, json!({"path": "x.docx"})).unwrap_err();
            // Cut also edits, so a read-only document says that first.
            let why = if id == "edit.cut" && mode.read_only() {
                read_only_text(mode)
            } else {
                "Tài liệu này không cho phép tải xuống, in hay sao chép."
            };
            assert_eq!(e, why, "{mode:?}: {id}");
        }
        assert!(page.downloads.borrow().is_empty(), "nothing was downloaded");
        assert!(app.session.clipboard_text.is_empty(), "nothing was copied");
        // Whatever else egui would copy this frame (a selected label) doesn't reach the clipboard.
        let mut out = page.ctx.run_ui(Default::default(), |ui| {
            let ctx = ui.ctx();
            ctx.copy_text("Xin chào".into());
            crate::access_gate::end_frame(&app, ctx);
        });
        out.textures_delta.clear();
        assert!(!out.platform_output.commands.iter().any(|c| matches!(c, egui::output::OutputCommand::CopyText(_))));
    }
    // With download allowed, a viewer still saves a copy and copies.
    let (mut app, page) = opened(HostMode::View, true);
    app.run("select.all", json!({})).unwrap();
    app.run("file.saveAs", json!({"path": "Báo cáo.docx"})).unwrap();
    assert_eq!(page.downloads.borrow().len(), 1);
    app.run("edit.copy", json!({})).unwrap();
    assert!(app.session.clipboard_text.contains("Xin chào"));
    assert!(!app.session.dirty && page.host.sends().is_empty());
}

#[test]
fn preserve_view_keeps_the_first_visible_text_and_the_selection() {
    let lines: Vec<String> = (0..300).map(|i| format!("Dòng {i} của báo cáo")).collect();
    let (mut app, page) = web_app();
    app.canvas.want_focus = true;
    page.link.open(HostOpen {
        doc_id: "d1".into(),
        version: 1,
        title: "Dài".into(),
        format: "docx".into(),
        bytes: docx(&lines.join("\n")),
        mode: HostMode::View,
        ..Default::default()
    });
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 900.0)).build_ui_state(
        |ui, app: &mut WordApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    let sel = Selection { anchor: Pos::body(150, 0), focus: Pos::body(150, 4) };
    h.state_mut().session.sel = sel.clone();
    h.state_mut().canvas.nav_to_caret = true;
    h.run_steps(6);
    let (top, _) = crate::canvas::first_visible(h.state_mut()).expect("the page is drawn");
    assert!(top.path.last() > 100, "scrolled down to the selection: {top:?}");

    // A newer version: the first line edited and one appended; block 150 is still there.
    let mut newer = lines.clone();
    newer[0] = "Dòng đầu đã sửa".into();
    newer.push("Dòng mới".into());
    let reload = |preserve_view| HostOpen {
        doc_id: "d1".into(),
        version: 2,
        title: "Dài".into(),
        format: "docx".into(),
        bytes: docx(&newer.join("\n")),
        mode: HostMode::View,
        preserve_view,
        ..Default::default()
    };
    page.link.open(reload(true));
    h.run_steps(6);
    assert!(body(h.state()).contains("Dòng mới"), "the newer bytes are shown");
    assert!(h.state().read_only(), "still a viewer");
    assert_eq!(h.state().session.sel, sel, "the selection is kept");
    let (after, _) = crate::canvas::first_visible(h.state_mut()).expect("the page is drawn");
    assert_eq!(after.path, top.path, "the same block is at the top");
    assert_eq!(after.story, StoryRef::Body);

    // Without preserveView the reload starts at the top, as v1 does.
    page.link.open(reload(false));
    h.run_steps(6);
    assert_eq!(h.state().session.sel, Selection::caret(Pos::body(0, 0)));
    let (fresh, _) = crate::canvas::first_visible(h.state_mut()).expect("the page is drawn");
    assert_eq!(fresh.path.last(), 0);
}

#[test]
fn set_user_becomes_the_author_of_a_new_comment_in_edit_mode() {
    let (mut app, page) = web_app();
    let prefs_name = app.session.author.clone();
    page.link.set_user(Some("Nguyễn Văn An".into()));
    app.host_tick(&page.ctx, 0.0);
    open_as(&page, "d1", "Xin chào", HostMode::Edit, true);
    app.host_tick(&page.ctx, 0.0);
    app.run("select.all", json!({})).unwrap();
    app.run("review.newComment", json!({"text": "Ý kiến"})).unwrap();
    let authors: Vec<&str> = app.session.doc.comments.values().map(|c| c.author.as_str()).collect();
    assert_eq!(authors, vec!["Nguyễn Văn An"]);
    assert_eq!(app.prefs().author, prefs_name, "the preferences keep their own name");
    page.link.set_user(None);
    app.host_tick(&page.ctx, 0.0);
    assert_eq!(app.session.author, prefs_name, "an anonymous viewer: the preferences' name is back");
}
