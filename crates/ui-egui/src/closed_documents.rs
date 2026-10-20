//! Documents closed in this session, kept so Reopen Closed Document (File › Open, a document
//! tab's menu) brings the last one back as it was when closed: a "Don't Save" keeps its
//! unsaved changes, which is the point. Each is kept as the bytes its draft would be, encrypted
//! when it has a password, so no decrypted copy of a protected document stays in memory.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::WordApp;
use crate::open_documents::{save_name_of, title_of};

/// Most closed documents kept.
pub const MAX_CLOSED: usize = 10;

/// Most bytes kept for them in all: they stay in memory, which a browser tab can't grow without
/// bound. The oldest go first.
pub const MAX_CLOSED_BYTES: usize = 64 << 20;

/// A closed document.
#[derive(Clone, Debug)]
pub(crate) struct ClosedDocument {
    /// The Word package name its bytes open under.
    file_name: String,
    path: Option<PathBuf>,
    untitled: u32,
    title: String,
    dirty: bool,
    bytes: Vec<u8>,
}

/// The closed documents, most recent last.
#[derive(Debug, Default)]
pub(crate) struct ClosedDocuments {
    stack: Vec<ClosedDocument>,
    /// An encrypted one waiting for its password (the open happens in the Password dialog), with
    /// the document shown when it began and the recent list from before.
    reopening: Option<(ClosedDocument, u64, Vec<String>)>,
}

impl ClosedDocuments {
    fn push(&mut self, closed: ClosedDocument) {
        if closed.bytes.len() > MAX_CLOSED_BYTES {
            log::warn!("{} is too large to keep for reopening", closed.title);
            return;
        }
        self.stack.push(closed);
        while self.stack.len() > MAX_CLOSED || self.total_bytes() > MAX_CLOSED_BYTES {
            self.stack.remove(0);
        }
    }

    fn total_bytes(&self) -> usize {
        self.stack.iter().map(|c| c.bytes.len()).fold(0, usize::saturating_add)
    }

    /// The title of the document Reopen would bring back.
    pub(crate) fn last_title(&self) -> Option<&str> {
        self.stack.last().map(|c| c.title.as_str())
    }
}

impl WordApp {
    /// Whether Reopen Closed Document has something to reopen.
    pub fn can_reopen_document(&self) -> bool {
        self.closed.reopening.is_none() && !self.closed.stack.is_empty()
    }

    /// Keep the document at tab `i`, which is about to close, for reopening. An untouched blank
    /// document has nothing worth bringing back.
    pub(crate) fn keep_closed(&mut self, i: usize) {
        let Some((session, untitled)) = self.document_at(i) else { return };
        if session.is_untouched_blank() || self.no_document == Some(session.document_id()) {
            return;
        }
        let file_name = save_name_of(session, untitled);
        let password = session.password.as_ref().map(wordcraft_engine::Password::as_str);
        let opts = wordcraft_engine::cmd::file::export_options(&self.session, &json!({}));
        match wordcraft_engine::io::save_bytes_with(&file_name, &session.doc, password, &opts) {
            Ok(bytes) => {
                let closed = ClosedDocument {
                    file_name,
                    path: session.path.clone(),
                    untitled,
                    title: title_of(session, untitled),
                    dirty: session.dirty,
                    bytes,
                };
                self.closed.push(closed);
            }
            Err(e) => log::warn!("couldn't keep {file_name} for reopening: {e}"),
        }
    }

    /// `ui.reopenDocument`: open the last closed document beside the open ones, under its name
    /// and as unsaved as it was. Past the open documents limit it stays closed and can be
    /// reopened later; an encrypted one asks for its password first.
    pub(crate) fn reopen_document(&mut self) -> Result<Value, String> {
        if self.closed.reopening.is_some() {
            return Err(tl!("Type the password of the document being reopened first.").into());
        }
        let Some(closed) = self.closed.stack.pop() else {
            return Err(tl!("No closed document to reopen.").into());
        };
        let shown = self.session.document_id();
        let recent = self.ui.recent.clone();
        let params = json!({"path": closed.file_name, "data": wordcraft_engine::cmd::insert::base64_encode(&closed.bytes)});
        match self.execute_user("file.open", params) {
            Ok(v) if v.get("pending").is_some_and(|p| !p.is_null()) => {
                self.closed.reopening = Some((closed, shown, recent));
                Ok(json!({"pending": "password"}))
            }
            Ok(_) => {
                let title = closed.title.clone();
                self.adopt_reopened(closed, recent);
                Ok(json!({"reopened": title, "dirty": self.session.dirty, "documents": self.document_count()}))
            }
            Err(e) => {
                self.closed.stack.push(closed);
                Err(e)
            }
        }
    }

    /// Finish a reopen that waited for its password (every frame, from `logic`): adopted once
    /// another document is shown, put back when the password was cancelled.
    pub(crate) fn poll_reopening(&mut self) {
        let Some((closed, shown, recent)) = self.closed.reopening.take() else { return };
        if self.session.document_id() != shown {
            self.adopt_reopened(closed, recent);
        } else if matches!(self.dialog, Some(crate::dialogs::Dialog::Password { .. })) {
            self.closed.reopening = Some((closed, shown, recent));
        } else {
            self.closed.stack.push(closed);
        }
    }

    /// The reopened document, just opened from its bytes, takes back what it was.
    fn adopt_reopened(&mut self, closed: ClosedDocument, recent: Vec<String>) {
        if closed.path.is_none() {
            // Opened under a made-up name, which isn't a file to list in Recent.
            self.ui.recent = recent;
            self.untitled = closed.untitled;
        }
        self.session.path = closed.path;
        self.session.dirty = closed.dirty;
        self.ui.backstage = false;
        self.canvas.want_focus = true;
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wordcraft_doc::StoryRef;
    use wordcraft_engine::Session;

    use super::{ClosedDocument, ClosedDocuments, MAX_CLOSED, MAX_CLOSED_BYTES};
    use crate::dialogs::Dialog;
    use crate::open_documents::MAX_DOCUMENTS;
    use crate::{Services, WordApp};

    fn app() -> WordApp {
        WordApp::new(Session::new(wordcraft_doc::Document::new()), Services::default())
    }

    fn typed(a: &mut WordApp, text: &str) {
        a.run("text.insert", json!({"text": text})).unwrap();
    }

    fn body(a: &WordApp) -> String {
        a.session.doc.plain_text(StoryRef::Body)
    }

    fn record(title: &str, bytes: usize) -> ClosedDocument {
        ClosedDocument { file_name: format!("{title}.docx"), path: None, untitled: 2, title: title.into(), dirty: true, bytes: vec![0; bytes] }
    }

    #[test]
    fn a_document_closed_without_saving_reopens_with_its_changes_unsaved() {
        let mut a = app();
        typed(&mut a, "Kept");
        a.run("file.new", json!({})).unwrap();
        typed(&mut a, "Draft text");
        assert_eq!(a.document_tabs()[1].title, "Document2");
        a.run("file.close", json!({})).unwrap();
        a.run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
        assert_eq!(a.document_count(), 1);
        assert!(a.can_reopen_document());

        let r = a.run("ui.reopenDocument", json!({})).unwrap();
        assert_eq!(r["reopened"], "Document2");
        assert_eq!(a.document_count(), 2);
        assert!(body(&a).contains("Draft text"), "{}", body(&a));
        assert!(a.session.dirty, "unsaved, as it was closed");
        assert!(a.session.path.is_none(), "still untitled");
        assert_eq!(a.document_tabs()[1].title, "Document2", "under its own name");
        assert!(a.ui.recent.is_empty(), "a made-up name isn't a recent file");
        assert!(!a.can_reopen_document(), "nothing left to reopen");
        assert!(a.run("ui.reopenDocument", json!({})).is_err());
    }

    #[test]
    fn a_saved_document_reopens_saved_under_its_file() {
        let dir = std::env::temp_dir().join(format!("wordcraft-reopen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("letter.docx");
        let mut a = app();
        typed(&mut a, "Kept");
        a.run("file.new", json!({})).unwrap();
        typed(&mut a, "Saved text");
        a.run("file.save", json!({"path": path.to_string_lossy()})).unwrap();
        a.run("file.close", json!({})).unwrap();
        assert_eq!(a.document_count(), 1);
        a.run("ui.reopenDocument", json!({})).unwrap();
        assert!(body(&a).contains("Saved text"));
        assert!(!a.session.dirty);
        assert_eq!(a.session.path.as_deref(), Some(path.as_path()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_most_recently_closed_comes_back_first() {
        let mut a = app();
        typed(&mut a, "Kept");
        for text in ["First", "Second"] {
            a.run("file.new", json!({})).unwrap();
            typed(&mut a, text);
        }
        a.run("ui.closeDocument", json!({"index": 1})).unwrap();
        a.run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
        a.run("ui.closeDocument", json!({"index": 1})).unwrap();
        a.run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
        a.run("ui.reopenDocument", json!({})).unwrap();
        assert!(body(&a).contains("Second"));
        a.run("ui.reopenDocument", json!({})).unwrap();
        assert!(body(&a).contains("First"));
    }

    #[test]
    fn an_untouched_blank_document_is_not_kept() {
        let mut a = app();
        typed(&mut a, "Kept");
        a.run("file.new", json!({})).unwrap();
        a.run("file.close", json!({})).unwrap();
        assert!(!a.can_reopen_document());
    }

    #[test]
    fn at_most_ten_and_64_mb_are_kept_oldest_dropped_first() {
        let mut c = ClosedDocuments::default();
        for i in 0..MAX_CLOSED + 3 {
            c.push(record(&format!("d{i}"), 10));
        }
        assert_eq!(c.stack.len(), MAX_CLOSED);
        assert_eq!(c.stack.first().map(|d| d.title.as_str()), Some("d3"), "the oldest went first");
        assert_eq!(c.last_title(), Some("d12"));

        let mut c = ClosedDocuments::default();
        let big = MAX_CLOSED_BYTES / 3;
        for i in 0..4 {
            c.push(record(&format!("big{i}"), big));
        }
        assert!(c.total_bytes() <= MAX_CLOSED_BYTES);
        assert_eq!(c.stack.iter().map(|d| d.title.as_str()).collect::<Vec<_>>(), ["big1", "big2", "big3"]);
        c.push(record("huge", MAX_CLOSED_BYTES + 1));
        assert_eq!(c.last_title(), Some("big3"), "one larger than the whole budget isn't kept");
    }

    #[test]
    fn reopening_respects_the_open_documents_limit() {
        let mut a = app();
        typed(&mut a, "x");
        a.run("file.new", json!({})).unwrap();
        typed(&mut a, "Closed one");
        a.run("file.close", json!({})).unwrap();
        a.run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
        while a.document_count() < MAX_DOCUMENTS {
            a.run("file.new", json!({})).unwrap();
        }
        let e = a.run("ui.reopenDocument", json!({})).unwrap_err();
        assert!(e.contains("up to 10 documents"), "{e}");
        assert!(a.can_reopen_document(), "it stays closed, to reopen later");
        a.run("ui.closeDocument", json!({"index": 1})).unwrap();
        a.run("ui.reopenDocument", json!({})).unwrap();
        assert!(body(&a).contains("Closed one"));
    }

    #[test]
    fn an_encrypted_document_is_kept_encrypted_and_asks_for_its_password() {
        let mut a = app();
        typed(&mut a, "Kept");
        a.run("file.new", json!({})).unwrap();
        typed(&mut a, "Secret words");
        a.run("file.encrypt", json!({"password": "pw123"})).unwrap();
        a.run("file.close", json!({})).unwrap();
        a.run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
        let kept = a.closed.stack.last().unwrap();
        let unlocked = wordcraft_engine::io::open_bytes_with("x.docx", &kept.bytes, None);
        assert!(unlocked.is_err_and(|e| wordcraft_engine::io::needs_password(&e)), "no readable copy is kept");

        // Cancelling the password keeps it for later.
        assert_eq!(a.run("ui.reopenDocument", json!({})).unwrap()["pending"], "password");
        assert!(matches!(a.dialog, Some(Dialog::Password { .. })));
        assert!(!a.can_reopen_document());
        a.dialog = None;
        a.poll_reopening();
        assert!(a.can_reopen_document());
        assert_eq!(a.document_count(), 1);

        // The password opens it, unsaved and under its name.
        a.run("ui.reopenDocument", json!({})).unwrap();
        let Some(Dialog::Password { params, .. }) = a.dialog.take() else { panic!("asks for the password") };
        let mut params = params;
        params["password"] = json!("pw123");
        a.execute("file.open", params).unwrap();
        a.poll_reopening();
        assert!(body(&a).contains("Secret words"));
        assert!(a.session.dirty && a.session.path.is_none());
        assert!(a.session.password.is_some(), "it is saved with its password again");
        assert_eq!(a.document_tabs()[1].title, "Document2");
    }

    #[test]
    fn ctrl_shift_t_keeps_reduce_hanging_indent() {
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 900.0)).build_ui_state(
            |ui, app: &mut WordApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app(),
        );
        h.state_mut().canvas.want_focus = true;
        typed(h.state_mut(), "Kept");
        h.state_mut().run("file.new", json!({})).unwrap();
        typed(h.state_mut(), "Gone");
        h.state_mut().run("file.close", json!({})).unwrap();
        h.state_mut().run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
        h.run_steps(3);
        let mac = cfg!(target_os = "macos");
        let ctrl_shift = egui::Modifiers { shift: true, command: true, ctrl: !mac, mac_cmd: mac, alt: false };
        h.event(egui::Event::Key { key: egui::Key::T, physical_key: None, pressed: true, repeat: false, modifiers: ctrl_shift });
        h.run_steps(3);
        assert_eq!(h.state().document_count(), 1, "the key is Word's, not a browser tab's");
        let rows = crate::keys::shortcut_rows(h.state());
        assert!(rows.iter().any(|r| r.id == "para.removeHanging" && r.key == "Mod+Shift+T"));
        assert!(!rows.iter().any(|r| r.id == "ui.reopenDocument"), "no key of its own");
        h.state_mut().run("ui.reopenDocument", json!({})).unwrap();
        assert!(body(h.state()).contains("Gone"));
        let vi = crate::i18n::Lang::from_code("vi").unwrap();
        assert_eq!(crate::i18n::tr(vi, "Reopen Closed Document"), "Mở lại tài liệu vừa đóng");
    }
}
