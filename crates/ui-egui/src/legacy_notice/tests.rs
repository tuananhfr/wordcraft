use egui_kittest::kittest::Queryable;
use serde_json::json;

use crate::{WordApp, dialogs::Dialog};

mod fixture;

fn harness(width: f32) -> egui_kittest::Harness<'static, WordApp> {
    let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::from_text("Keep this document")), Default::default());
    app.ui.language = "vi".into();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(width, 844.0)).build_ui_state(
        |ui, app: &mut WordApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    h
}

#[test]
fn ole2_open_shows_one_clear_dialog_and_preserves_the_document() {
    let fixture = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    for width in [390.0, 1366.0] {
        for name in ["old.doc", "renamed.txt", "renamed.html", "renamed.docx"] {
            let mut h = harness(width);
            let before = h.state().session.document_id();
            let data = wordcraft_engine::cmd::insert::base64_encode(&fixture);
            h.state_mut().ui.backstage = true;
            assert!(h.state_mut().run("file.open", json!({"path": name, "data": data})).is_err());
            h.run_steps(6);
            assert!(!h.state().ui.backstage);
            assert_eq!(h.state().session.document_id(), before);
            assert_eq!(h.state().document_count(), 1);
            assert_eq!(h.state().session.doc.plain_text(wordcraft_doc::StoryRef::Body), "Keep this document");
            assert!(matches!(h.state().dialog, Some(Dialog::LegacyDoc { opened: false })));
            h.get_by_label("Không mở được định dạng .doc cũ");
            let text = h.get_by_label("Tệp này dùng định dạng Word cũ (.doc) mà Văn bản chưa đọc được. Hãy mở nó bằng Word hoặc LibreOffice, lưu thành .docx rồi mở lại.").rect();
            assert!(text.left() >= 0.0 && text.right() <= width && text.bottom() <= 844.0, "{text:?}");
            h.get_by_label("OK").click();
            h.run_steps(4);
            assert!(h.state().dialog.is_none());
        }
    }
}

#[test]
fn successful_legacy_open_shows_conversion_information_and_downloads_docx() {
    use std::{cell::RefCell, rc::Rc};

    for width in [390.0, 1366.0] {
        for name in ["old.doc", "old.txt", "old.docx"] {
            let mut h = harness(width);
            let writes = Rc::new(RefCell::new(Vec::new()));
            let received = Rc::clone(&writes);
            h.state_mut().services.download = Some(Box::new(move |name, bytes| {
                received.borrow_mut().push((name.to_string(), bytes.to_vec()));
                Ok(crate::Download::Done)
            }));
            let data = wordcraft_engine::cmd::insert::base64_encode(&fixture::readable_doc());
            let opened = h.state_mut().run("file.open", json!({"path": name, "data": data})).unwrap();
            assert_eq!(opened["legacy"], true);
            assert!(h.state().session.doc.plain_text(wordcraft_doc::StoryRef::Body).contains("Hello"));
            assert!(h.state().session.dirty);
            h.run_steps(6);
            h.get_by_label("Tệp .doc cũ đã được mở. Khi lưu, tệp sẽ được lưu thành .docx");
            h.get_by_label("OK").click();
            h.run_steps(4);
            assert!(h.state().dialog.is_none());
            let proposed = h.state().default_save_name();
            assert!(proposed.ends_with(".docx"));
            assert_ne!(proposed, name);
            for command in ["file.save", "file.saveAs"] {
                h.state_mut().run(command, json!({})).unwrap();
                let writes = writes.borrow();
                let (saved_name, saved_bytes) = writes.last().unwrap();
                assert_eq!(saved_name, &proposed);
                assert!(saved_bytes.starts_with(b"PK\x03\x04"));
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn native_legacy_save_preserves_original_bytes() {
    let dir = std::env::temp_dir().join(format!("wordcraft-w4-legacy-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let original = dir.join("original.doc");
    let converted = dir.join("original.docx");
    let bytes = fixture::readable_doc();
    std::fs::write(&original, &bytes).unwrap();
    std::fs::write(&converted, b"Existing document").unwrap();
    let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::new()), Default::default());
    let opened = app.run("file.open", json!({"path": original.to_string_lossy()})).unwrap();
    assert_eq!(opened["legacy"], true);
    assert_eq!(app.session.path.as_deref(), Some(original.as_path()));
    assert_eq!(app.session.suggested_save_path.as_deref(), Some(converted.as_path()));
    assert_eq!(app.default_save_name(), "original.docx");
    assert_eq!(app.ui.recent.first(), Some(&original.to_string_lossy().into_owned()));
    app.run("file.new", json!({})).unwrap();
    assert!(app.session.suggested_save_path.is_none());
    app.activate_document(0).unwrap();
    assert_eq!(app.session.suggested_save_path.as_deref(), Some(converted.as_path()));
    app.session.run("review.newComment", &json!({"text": "Keep this comment"})).unwrap();
    assert!(!app.warn_before_save("file.save", &json!({})));
    let saved = app.session.run("file.save", &json!({})).unwrap();
    assert_eq!(saved["saved"], false);
    assert_eq!(std::fs::read(&converted).unwrap(), b"Existing document");
    assert!(app.session.ui_requests.iter().any(|request| request["open"] == "saveAs"));
    app.session.ui_requests.clear();
    assert!(app.execute("file.saveAs", json!({"path": original.to_string_lossy()})).is_err());
    assert_eq!(std::fs::read(&original).unwrap(), bytes);
    app.execute("file.saveAs", json!({"path": converted.to_string_lossy()})).unwrap();
    assert_eq!(std::fs::read(&original).unwrap(), bytes);
    assert!(std::fs::read(dir.join("original.docx")).unwrap().starts_with(b"PK\x03\x04"));
    assert_eq!(app.session.run("file.save", &json!({})).unwrap()["saved"], true);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn programmatic_legacy_failure_does_not_open_a_dialog() {
    let mut h = harness(1366.0);
    let data = wordcraft_engine::cmd::insert::base64_encode(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    assert!(h.state_mut().execute("file.open", json!({"path": "old.doc", "data": data})).is_err());
    assert!(h.state().dialog.is_none());
}
