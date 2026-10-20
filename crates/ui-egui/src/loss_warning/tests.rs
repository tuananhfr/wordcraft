use std::cell::RefCell;
use std::rc::Rc;

use egui_kittest::kittest::Queryable;
use wordcraft_doc::Document;
use wordcraft_engine::Session;

use super::*;
use crate::{Download, Services, i18n};

#[test]
fn lossy_targets_report_each_live_feature() {
    for target in ["rtf", "odt", "txt", "text", "html", "htm", "md", "markdown", "tex", "latex", "ltx", "pdf", "png"] {
        assert_eq!(lost_items(DocFacts { comments: 3, ..Default::default() }, target), vec![LossItem::Comments(3)], "{target}");
        assert_eq!(lost_items(DocFacts { tracked_changes: 12, ..Default::default() }, target), vec![LossItem::TrackedChanges(12)], "{target}");
        assert_eq!(lost_items(DocFacts { vba: true, ..Default::default() }, target), vec![LossItem::Vba], "{target}");
    }
}

#[test]
fn word_and_json_preserve_review_features_and_macro_capability_matches_the_writer() {
    for target in ["docx", "dotx", "docm", "dotm", "xml", "flatopc", "json"] {
        let facts = DocFacts { comments: 3, tracked_changes: 12, ..Default::default() };
        assert!(lost_items(facts, target).is_empty(), "{target}");
        let expected = if matches!(target, "docx" | "dotx" | "xml" | "flatopc") { vec![LossItem::Vba] } else { vec![] };
        assert_eq!(lost_items(DocFacts { vba: true, ..facts }, target), expected, "{target}");
    }
    assert_eq!(lost_items(DocFacts { vba: true, ..Default::default() }, ".DOCX"), vec![LossItem::Vba]);
}

#[test]
fn word_2003_preserves_comments_and_text_revisions_but_drops_format_revisions_and_macros() {
    let facts = DocFacts { comments: 3, tracked_changes: 12, tracked_formatting: 2, vba: true, ..Default::default() };
    assert_eq!(lost_items(facts, "wordml2003"), vec![LossItem::TrackedChanges(2), LossItem::Vba]);
    assert!(lost_items(DocFacts { comments: 3, tracked_changes: 12, ..Default::default() }, "wordml2003").is_empty());
}

#[test]
fn signatures_warn_for_every_save_target_even_the_original_format() {
    for target in wordcraft_engine::io::SAVE_EXTS {
        assert_eq!(lost_items(DocFacts { signed: true, ..Default::default() }, target), vec![LossItem::Signature], "{target}");
    }
}

#[test]
fn empty_documents_never_warn_and_features_are_listed_once() {
    for target in wordcraft_engine::io::SAVE_EXTS {
        assert!(lost_items(DocFacts::default(), target).is_empty(), "{target}");
    }
    assert_eq!(
        lost_items(DocFacts { comments: 3, tracked_changes: 12, vba: true, signed: true, ..Default::default() }, "rtf"),
        vec![LossItem::Comments(3), LossItem::TrackedChanges(12), LossItem::Vba, LossItem::Signature],
    );
}

fn commented(services: Services) -> WordApp {
    let mut app = WordApp::new(Session::new(Document::from_text("Keep this document")), services);
    app.run("review.newComment", json!({"text": "Please review"})).unwrap();
    app
}

#[test]
fn save_as_rtf_cancel_writes_nothing_and_save_anyway_writes() {
    let dir = std::env::temp_dir().join(format!("wordcraft-loss-warning-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("review.rtf");
    let _ = std::fs::remove_file(&path);
    let picked = path.to_string_lossy().into_owned();
    let services = Services { pick_save: Some(Box::new(move |_| Some(picked.clone()))), ..Default::default() };
    let mut app = commented(services);
    app.ui.language = "vi".into();
    app.run("file.saveAs", json!({})).unwrap();
    assert!(matches!(app.dialog, Some(Dialog::LossWarning(_))));
    assert!(!path.exists());
    let mut fonts = false;
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(640.0, 480.0)).build_ui_state(
        move |ui, app: &mut WordApp| {
            i18n::set_current(i18n::Lang::from_pref("vi"));
            if fonts {
                crate::dialogs::show(app, ui.ctx());
            } else {
                crate::theme::install_fonts_for(ui.ctx(), false);
                fonts = true;
            }
        }, app,
    );
    h.run_steps(4);
    let _ = h.get_by_label("Lưu sẽ mất một số nội dung");
    let _ = h.get_by_label("• Bình luận (1)");
    h.get_by_label("Hủy").click();
    h.run_steps(4);
    assert!(h.state().dialog.is_none());
    assert!(!path.exists());
    assert!(h.state().session.dirty);
    h.state_mut().run("file.saveAs", json!({})).unwrap();
    h.run_steps(4);
    h.get_by_label("Vẫn lưu").click();
    h.run_steps(4);
    assert!(h.state().dialog.is_none());
    assert!(path.is_file());
    let saved = wordcraft_engine::io::open_path(&path).unwrap();
    assert!(saved.plain_text(wordcraft_doc::StoryRef::Body).contains("Keep this document"));
    assert!(saved.comments.is_empty());
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(dir).unwrap();
    i18n::set_current(i18n::Lang::EN);
}

type Downloads = Rc<RefCell<Vec<String>>>;

fn web_app() -> (WordApp, Downloads) {
    let downloads: Downloads = Rc::default();
    let sink = downloads.clone();
    let app = commented(Services {
        download: Some(Box::new(move |name, _| { sink.borrow_mut().push(name.to_string()); Ok(Download::Done) })),
        ..Default::default()
    });
    (app, downloads)
}

fn take_warning(app: &mut WordApp) -> PendingSave {
    let Some(Dialog::LossWarning(pending)) = app.dialog.take() else { panic!("expected a loss warning") };
    pending
}

#[test]
fn web_warning_precedes_download_and_copy_preserves_document_and_name() {
    let (mut app, downloads) = web_app();
    app.session.path = Some("original.docx".into());
    let before = serde_json::to_value(&app.session.doc).unwrap();
    let revision = app.session.rev();
    assert_eq!(app.run("file.saveAs", json!({"path": "review.rtf"})).unwrap()["pending"], "lossWarning");
    assert!(downloads.borrow().is_empty());
    let pending = take_warning(&mut app);
    app.choose_loss_copy(pending);
    assert_eq!(downloads.borrow().as_slice(), &["review (1).rtf"]);
    assert_eq!(app.session.path.as_deref(), Some(std::path::Path::new("original.docx")));
    assert_eq!(serde_json::to_value(&app.session.doc).unwrap(), before);
    assert_eq!(app.session.rev(), revision);
    assert!(app.session.dirty);
}

#[test]
fn macro_conversion_waits_but_macro_enabled_save_does_not() {
    let (mut app, downloads) = web_app();
    app.session.doc.passthrough.insert("word/vbaProject.bin".into(), std::sync::Arc::new(vec![1]));
    app.session.path = Some("original.docm".into());
    app.run("file.save", json!({})).unwrap();
    assert_eq!(downloads.borrow().len(), 1);
    app.run("file.saveAs", json!({"path": "original.docx"})).unwrap();
    let pending = take_warning(&mut app);
    assert_eq!(pending.items, vec![LossItem::Vba]);
    assert_eq!(downloads.borrow().len(), 1);
    app.accept_loss(pending).unwrap();
    assert_eq!(downloads.borrow().len(), 2);
}

#[test]
fn stale_confirmation_rechecks_the_live_document() {
    let (mut app, downloads) = web_app();
    app.run("file.saveAs", json!({"path": "review.rtf"})).unwrap();
    let pending = take_warning(&mut app);
    app.run("review.newComment", json!({"text": "Another comment"})).unwrap();
    app.accept_loss(pending).unwrap();
    assert!(downloads.borrow().is_empty());
    assert_eq!(take_warning(&mut app).items, vec![LossItem::Comments(2)]);
}

#[test]
fn desktop_copy_keeps_original_bytes_document_name_and_dirty_state() {
    let dir = std::env::temp_dir().join(format!("wordcraft-loss-copy-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let original = dir.join("original.docx");
    let copy = dir.join("review.rtf");
    std::fs::write(&original, b"untouched original").unwrap();
    let mut app = commented(Services::default());
    app.session.path = Some(original.clone());
    let before = serde_json::to_value(&app.session.doc).unwrap();
    app.run("file.saveAs", json!({"path": copy.to_string_lossy()})).unwrap();
    let pending = take_warning(&mut app);
    app.write_loss_copy(pending, Some(copy.to_string_lossy().into_owned())).unwrap();
    assert_eq!(std::fs::read(&original).unwrap(), b"untouched original");
    assert_eq!(app.session.path.as_ref(), Some(&original));
    assert_eq!(serde_json::to_value(&app.session.doc).unwrap(), before);
    assert!(app.session.dirty);
    assert!(copy.is_file());
    std::fs::remove_file(original).unwrap();
    std::fs::remove_file(copy).unwrap();
    std::fs::remove_dir(dir).unwrap();
}

#[test]
fn autosave_cannot_silently_discard_a_new_comment() {
    let dir = std::env::temp_dir().join(format!("wordcraft-loss-autosave-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("original.rtf");
    let mut app = WordApp::new(Session::new(Document::from_text("Saved")), Services::default());
    app.run("file.save", json!({"path": path.to_string_lossy()})).unwrap();
    let before = std::fs::read(&path).unwrap();
    app.run("review.newComment", json!({"text": "New review"})).unwrap();
    app.autosave_tick(crate::now_ms() + 10_000.0);
    assert!(matches!(app.dialog, Some(Dialog::LossWarning(_))));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(app.session.dirty);
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(dir).unwrap();
}

#[test]
fn copy_cannot_overwrite_the_original_file() {
    let dir = std::env::temp_dir().join(format!("wordcraft-loss-copy-original-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("original.rtf");
    std::fs::write(&path, b"original bytes").unwrap();
    let mut app = commented(Services::default());
    app.session.path = Some(path.clone());
    app.run("file.save", json!({})).unwrap();
    let pending = take_warning(&mut app);
    assert!(app.write_loss_copy(pending, Some(path.to_string_lossy().into_owned())).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"original bytes");
    assert!(app.session.dirty);
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(dir).unwrap();
}

#[test]
fn loss_warning_strings_have_vietnamese_translations() {
    let vi = i18n::Lang::from_pref("vi");
    for key in [
        "Saving will lose some content", "Comments ({count})", "Tracked changes ({count})", "VBA macros",
        "Digital signature (removed or invalidated)", "Save anyway", "Save a copy", "Choose a different name for the copy.",
    ] {
        assert!(i18n::has(vi, key), "{key}");
    }
}

#[test]
fn stale_copy_confirmation_keeps_copy_semantics() {
    let (mut app, downloads) = web_app();
    app.session.path = Some("original.docx".into());
    app.run("file.saveAs", json!({"path": "review.rtf"})).unwrap();
    let pending = take_warning(&mut app);
    app.run("review.newComment", json!({"text": "Later comment"})).unwrap();
    app.write_loss_copy(pending, Some("copy.rtf".into())).unwrap();
    assert!(downloads.borrow().is_empty());
    let pending = take_warning(&mut app);
    assert!(pending.copy);
    app.accept_loss(pending).unwrap();
    assert_eq!(downloads.borrow().as_slice(), &["copy.rtf"]);
    assert_eq!(app.session.path.as_deref(), Some(std::path::Path::new("original.docx")));
    assert!(app.session.dirty);
}

#[test]
fn signed_same_format_save_waits_before_downloading() {
    let (mut app, downloads) = web_app();
    app.session.path = Some("signed.docx".into());
    app.session.doc.passthrough.insert("wordcraft:package.signed".into(), std::sync::Arc::new(Vec::new()));
    assert_eq!(app.run("file.save", json!({})).unwrap()["pending"], "lossWarning");
    assert!(downloads.borrow().is_empty());
    let pending = take_warning(&mut app);
    assert_eq!(pending.items, vec![LossItem::Signature]);
    app.accept_loss(pending).unwrap();
    assert_eq!(downloads.borrow().as_slice(), &["signed.docx"]);
    app.run("file.save", json!({})).unwrap();
    assert!(app.dialog.is_none());
    assert_eq!(downloads.borrow().len(), 2);
}

#[test]
fn save_before_close_waits_for_loss_confirmation() {
    let (mut app, downloads) = web_app();
    app.session.path = Some("signed.docx".into());
    app.session.doc.passthrough.insert("wordcraft:package.signed".into(), std::sync::Arc::new(Vec::new()));
    app.run("file.close", json!({})).unwrap();
    app.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
    assert!(!app.quit_requested);
    assert!(downloads.borrow().is_empty());
    let pending = take_warning(&mut app);
    assert!(matches!(&pending.after, Some(AfterSave::Continue { .. })));
    app.accept_loss(pending).unwrap();
    assert_eq!(downloads.borrow().as_slice(), &["signed.docx"]);
    assert!(app.quit_requested);
}

#[test]
fn signature_fact_is_cleared_only_after_the_browser_finishes_writing() {
    for success in [false, true] {
        let (tx, rx) = std::sync::mpsc::channel();
        let rx = RefCell::new(Some(rx));
        let mut app = commented(Services {
            download: Some(Box::new(move |_, _| Ok(Download::Pending(rx.borrow_mut().take().unwrap())))),
            ..Default::default()
        });
        app.session.path = Some("signed.docx".into());
        app.session.doc.passthrough.insert("wordcraft:package.signed".into(), std::sync::Arc::new(Vec::new()));
        app.run("file.save", json!({})).unwrap();
        let pending = take_warning(&mut app);
        app.accept_loss(pending).unwrap();
        assert!(DocFacts::of(&app.session).signed);
        tx.send(if success { Ok(()) } else { Err(None) }).unwrap();
        app.poll_picker_saves();
        assert_eq!(DocFacts::of(&app.session).signed, !success);
        assert_eq!(app.session.dirty, !success);
    }
}
