//! The host's [`Access`] at the engine's single edit entry point (`erp-host.md` v2 rule 12).

use serde_json::json;

use crate::{Access, CmdError, Session, cmd};

fn sample() -> Session {
    let mut s = Session::new(crate::sample::sample_document());
    s.run("select.word", &json!({})).ok();
    s
}

/// Pure commands that reach outside the session (files, the system) are left to their own tests,
/// as in `hostile_params_never_panic`; the refused ones never get that far.
fn reaches_outside(id: &str) -> bool {
    id.starts_with("file.") || id == "insert.picture" || id == "insert.textFromFile"
}

#[test]
fn read_only_refuses_every_command_that_edits_and_the_rest_change_nothing() {
    let reg = cmd::registry();
    let params = [json!({}), json!({"text": "x", "value": true, "size": 20, "style": "Heading1"})];
    let mut changed = Vec::new();
    for spec in reg.all() {
        for p in &params {
            let mut s = sample();
            s.access = Access { read_only: true, can_download: true, ..Default::default() };
            let (doc, rev, undo) = (s.doc.clone(), s.rev(), s.undo_depth());
            let id = s.document_id();
            if spec.mutates {
                let r = s.run(spec.id, p);
                assert!(matches!(&r, Err(CmdError::Disabled(m)) if m.contains("read-only")), "{} ran: {r:?}", spec.id);
            } else if reaches_outside(spec.id) {
                continue;
            } else {
                let _ = s.run(spec.id, p);
            }
            // A command that replaces the document (New, Open) isn't an edit of it.
            if s.document_id() == id && (s.doc != doc || s.rev() != rev || s.undo_depth() != undo) {
                changed.push(spec.id);
            }
        }
    }
    changed.dedup();
    assert!(changed.is_empty(), "these changed a read-only document: {changed:?}");
}

#[test]
fn read_only_refuses_undo_redo_and_saving_but_keeps_the_history() {
    let mut s = Session::new(wordcraft_doc::Document::new());
    s.run("text.insert", &json!({"text": "Một"})).unwrap();
    s.run("format.bold", &json!({})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    let (depth, text) = (s.undo_depth(), s.doc.plain_text(wordcraft_doc::StoryRef::Body));
    s.access.read_only = true;
    for id in ["edit.undo", "edit.redo", "edit.undoTo", "edit.repeat", "file.save", "file.encrypt"] {
        assert!(s.run(id, &json!({"count": 1, "password": "x"})).is_err(), "{id}");
    }
    assert!(!s.can_undo() && !s.can_redo());
    assert!(!s.undo(), "the API refuses too");
    assert_eq!(s.undo_depth(), depth);
    // Leaving read-only, the same steps are there to undo and redo.
    s.access.read_only = false;
    s.run("edit.redo", &json!({})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert_ne!(s.doc.plain_text(wordcraft_doc::StoryRef::Body), text);
}

#[test]
fn no_download_refuses_what_takes_the_content_out_only() {
    let mut s = sample();
    s.access = Access { read_only: false, can_download: false, ..Default::default() };
    for id in [
        "file.saveAs",
        "file.saveTemplate",
        "file.exportPdf",
        "file.exportPng",
        "file.print",
        "file.printPdf",
        "file.printMarkup",
        "file.share",
        "edit.copy",
        "edit.cut",
    ] {
        let r = s.run(id, &json!({"path": "/nonexistent/x.docx"}));
        assert!(matches!(&r, Err(CmdError::Disabled(m)) if m.contains("downloading")), "{id}: {r:?}");
    }
    assert!(s.clipboard_text.is_empty());
    for (id, p) in [("caret.docEnd", json!({})), ("text.insert", json!({"text": "!"})), ("view.zoom", json!({"value": 150}))] {
        s.run(id, &p).unwrap_or_else(|e| panic!("{id}: {e}"));
    }
}

#[test]
fn access_belongs_to_its_document_and_is_never_saved() {
    let mut s = sample();
    s.access = Access { read_only: true, can_download: false, ..Default::default() };
    let mut other = s.sibling();
    assert_eq!(other.access, Access::default(), "a new document is fully editable");
    s.swap_document(&mut other);
    assert_eq!(s.access, Access::default());
    assert!(other.access.read_only, "the grant went with its document");
    s.swap_document(&mut other);
    assert!(s.access.read_only);
    let bytes = crate::io::save_bytes_with("a.docx", &s.doc, None, &Default::default()).unwrap();
    let again = crate::io::open_bytes("a.docx", &bytes).unwrap();
    assert!(again.settings.protection.is_none(), "nothing about the grant is written into the file");
}
