use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{Sender, channel};

use serde_json::json;
use wordcraft_engine::Session;

use crate::dialogs::Dialog;
use crate::file_dialogs::{AfterSave, Asked};
use crate::{Download, PickerAnswer, SaveStatus, Services, WordApp};

type Answers = Rc<RefCell<Vec<Sender<PickerAnswer>>>>;

fn app() -> (WordApp, Answers) {
    let answers = Answers::default();
    let queued = answers.clone();
    let services = Services {
        download: Some(Box::new(move |_, _| {
            let (tx, rx) = channel();
            queued.borrow_mut().push(tx);
            Ok(Download::Pending(rx))
        })),
        ..Default::default()
    };
    let mut app = WordApp::new(Session::new(wordcraft_doc::Document::new()), services);
    app.run("ui.language", json!({"value": "vi"})).unwrap();
    app.run("text.insert", json!({"text": "Draft"})).unwrap();
    app.session.path = Some("draft.docx".into());
    (app, answers)
}

fn complete(app: &mut WordApp, answers: &Answers, answer: PickerAnswer) {
    answers.borrow_mut().remove(0).send(answer).unwrap();
    app.poll_picker_saves();
}

#[test]
fn saved_flag_is_set_only_after_write_completion() {
    let (mut app, answers) = app();
    app.run("file.save", json!({})).unwrap();
    assert!(app.session.dirty);
    assert_eq!(app.save_status(), Some(SaveStatus::Saving));
    app.poll_picker_saves();
    assert!(app.session.dirty);
    complete(&mut app, &answers, Ok(()));
    assert!(!app.session.dirty);
    assert_eq!(app.save_status(), None);
}

#[test]
fn failed_write_keeps_dirty_and_hides_javascript_debug_text() {
    let (mut app, answers) = app();
    app.run("file.save", json!({})).unwrap();
    complete(&mut app, &answers, Err(Some("JsValue(Error: NotAllowedError: private details)".into())));
    assert!(app.session.dirty);
    let message = "Không ghi được tệp. Tài liệu vẫn chưa được lưu.";
    assert_eq!(app.save_status(), Some(SaveStatus::Failed(message)));
    assert_eq!(app.status_msg.as_ref().map(|(text, _)| text.as_str()), Some(message));
    let Some(SaveStatus::Failed(reason)) = app.save_status() else { panic!("save failure expected") };
    assert!(!reason.contains("JsValue") && !reason.contains("Error:"));
}

#[test]
fn immediate_write_failure_is_localized_and_keeps_dirty() {
    let (mut app, _) = app();
    app.session.dirty = false;
    app.services.download = Some(Box::new(|_, _| Err("JsValue(Error: SecurityError)".into())));
    let error = app.run("file.save", json!({})).unwrap_err();
    assert_eq!(error, "Không ghi được tệp. Tài liệu vẫn chưa được lưu.");
    assert!(!error.contains("JsValue") && !error.contains("Error:"));
    assert!(app.session.dirty);
}

#[test]
fn cancelled_save_before_close_keeps_document_open() {
    let (mut app, answers) = app();
    let document = app.session.document_id();
    app.run("file.close", json!({})).unwrap();
    app.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
    assert!(app.session.dirty && !app.quit_requested);
    complete(&mut app, &answers, Err(None));
    assert_eq!(app.session.document_id(), document);
    assert!(app.session.dirty && !app.quit_requested);
    assert!(app.dialog.is_none());
    assert_eq!(app.save_status(), None);
}

#[test]
fn close_while_saving_waits_for_the_existing_write() {
    for answer in [Ok(()), Err(None), Err(Some("JsValue(Error: quota)".into()))] {
        let success = answer.is_ok();
        let (mut app, answers) = app();
        app.run("file.save", json!({})).unwrap();
        assert_eq!(app.run("file.close", json!({})).unwrap()["pending"], "save");
        assert!(app.session.dirty && !app.quit_requested);
        assert_eq!(answers.borrow().len(), 1);
        app.poll_picker_saves();
        assert!(!app.quit_requested);
        complete(&mut app, &answers, answer);
        assert_eq!(app.quit_requested, success);
        assert_eq!(app.session.dirty, !success);
        assert!(answers.borrow().is_empty());
    }
}

#[test]
fn close_all_and_exit_wait_for_the_existing_write() {
    for exit in [false, true] {
        for answer in [Ok(()), Err(None), Err(Some("JsValue(Error: quota)".into()))] {
            let success = answer.is_ok();
            let (mut app, answers) = app();
            app.run("file.save", json!({})).unwrap();
            if exit {
                assert!(!app.close_requested());
            } else {
                assert_eq!(app.run("ui.closeAll", json!({})).unwrap()["pending"], "save");
            }
            assert_eq!(answers.borrow().len(), 1);
            assert!(!app.quit_requested);
            complete(&mut app, &answers, answer);
            assert_eq!(app.quit_requested, success);
            assert_eq!(app.session.dirty, !success);
        }
    }
}

#[test]
fn save_choice_for_exit_waits_until_success() {
    let (mut app, answers) = app();
    assert!(!app.close_requested());
    app.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
    assert!(app.session.dirty && !app.quit_requested);
    app.poll_picker_saves();
    assert!(!app.quit_requested);
    complete(&mut app, &answers, Ok(()));
    assert!(app.quit_requested);
}

#[test]
fn edits_during_a_write_stay_dirty_and_prevent_close() {
    let (mut app, answers) = app();
    app.run("file.save", json!({})).unwrap();
    app.run("file.close", json!({})).unwrap();
    app.run("text.insert", json!({"text": " newer edit"})).unwrap();
    complete(&mut app, &answers, Ok(()));
    assert!(app.session.dirty && !app.quit_requested);
}

#[test]
fn a_second_save_reuses_the_in_flight_write() {
    let (mut app, answers) = app();
    app.run("file.save", json!({})).unwrap();
    assert_eq!(app.run("file.save", json!({})).unwrap()["pending"], true);
    assert_eq!(answers.borrow().len(), 1);
    complete(&mut app, &answers, Ok(()));
    assert!(!app.session.dirty);
}

#[test]
fn background_close_waits_for_its_own_document_save() {
    let (mut app, answers) = app();
    let first = app.session.document_id();
    app.run("file.save", json!({})).unwrap();
    app.run("file.new", json!({})).unwrap();
    app.run("text.insert", json!({"text": "Second"})).unwrap();
    let second = app.session.document_id();
    app.run("file.save", json!({})).unwrap();
    app.run("ui.closeDocument", json!({"document": first})).unwrap();
    complete(&mut app, &answers, Ok(()));
    assert!(app.find_document(first).is_none());
    assert_eq!(app.session.document_id(), second);
    assert!(app.session.dirty);
    assert_eq!(app.save_status(), Some(SaveStatus::Saving));
    complete(&mut app, &answers, Err(None));
    assert!(app.session.dirty && !app.quit_requested);
}

#[test]
fn close_all_waits_then_prompts_for_other_unsaved_documents() {
    let (mut app, answers) = app();
    let first = app.session.document_id();
    app.run("file.save", json!({})).unwrap();
    app.run("file.new", json!({})).unwrap();
    app.run("text.insert", json!({"text": "Second"})).unwrap();
    let second = app.session.document_id();
    app.run("ui.closeAll", json!({})).unwrap();
    assert!(app.dialog.is_none());
    complete(&mut app, &answers, Ok(()));
    assert!(matches!(app.dialog, Some(Dialog::SaveChanges { document, .. }) if document == second));
    assert!(app.find_document(first).is_some_and(|(session, _)| !session.dirty));
    assert_eq!(app.document_count(), 2);
    assert!(!app.quit_requested);
}

#[test]
fn disconnected_write_keeps_document_open_and_unsaved() {
    let (mut app, answers) = app();
    app.run("file.close", json!({})).unwrap();
    app.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
    answers.borrow_mut().clear();
    app.poll_picker_saves();
    assert!(app.session.dirty && !app.quit_requested);
    assert_eq!(app.save_status(), None);
}

#[test]
fn save_as_continuation_waits_for_write_completion() {
    for answer in [Ok(()), Err(None), Err(Some("JsValue(Error: quota)".into()))] {
        let success = answer.is_ok();
        let (mut app, answers) = app();
        let document = app.session.document_id();
        app.services.pick_save = Some(Box::new(|_| Some("chosen.docx".into())));
        let after = AfterSave::Continue { then: "ui.closeDocument".into(), params: json!({"document": document}), document };
        assert!(matches!(app.save_as(after), Asked::Done(Ok(_))));
        assert!(app.session.dirty && !app.quit_requested);
        complete(&mut app, &answers, answer);
        assert_eq!(app.quit_requested, success);
        assert_eq!(app.session.dirty, !success);
    }
}

#[test]
fn loss_confirmation_before_close_waits_for_write_completion() {
    for answer in [Ok(()), Err(None), Err(Some("JsValue(Error: quota)".into()))] {
        let success = answer.is_ok();
        let (mut app, answers) = app();
        app.session.doc.passthrough.insert("wordcraft:package.signed".into(), std::sync::Arc::new(Vec::new()));
        app.run("file.close", json!({})).unwrap();
        app.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
        let Some(Dialog::LossWarning(pending)) = app.dialog.take() else { panic!("loss warning expected") };
        app.accept_loss(pending).unwrap();
        assert!(app.session.dirty && !app.quit_requested);
        assert_eq!(app.save_status(), Some(SaveStatus::Saving));
        complete(&mut app, &answers, answer);
        assert_eq!(app.quit_requested, success);
        assert_eq!(app.session.dirty, !success);
    }
}
