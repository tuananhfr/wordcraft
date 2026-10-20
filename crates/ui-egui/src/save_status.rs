//! The status bar's word on the shown document's save: "Saving…" while the browser's save picker
//! is still writing it, "Save failed" (the reason in its tooltip, or a click) from a failed save
//! until the document is saved or edited. Desktop writes a file within the frame that asked, so
//! only its failures show. Draft states belong to the office shell's page and aren't repeated.

use egui::{Align2, Sense, Ui, pos2, vec2};

use crate::WordApp;
use crate::theme::{Tokens, TypeRung, paint_text, text_width};

/// Failed saves, per document, with the revision the document had then.
#[derive(Debug, Default)]
pub(crate) struct SaveFailures(Vec<Failure>);

#[derive(Debug)]
struct Failure {
    document: u64,
    rev: u64,
    reason: String,
}

impl SaveFailures {
    fn failed(&mut self, document: u64, rev: u64, reason: String) {
        self.saved(document);
        self.0.push(Failure { document, rev, reason });
    }

    /// Saved (or closed): its failure is over.
    pub(crate) fn saved(&mut self, document: u64) {
        self.0.retain(|f| f.document != document);
    }

    /// Why the document's last save failed, while it is unchanged since.
    fn reason(&self, document: u64, rev: u64) -> Option<&str> {
        self.0.iter().find(|f| f.document == document && f.rev == rev).map(|f| f.reason.as_str())
    }
}

/// What the status bar says about the shown document's save.
#[derive(Debug, PartialEq)]
pub enum SaveStatus<'a> {
    Saving,
    Failed(&'a str),
}

impl WordApp {
    /// The shown document's save state, when there is one to show.
    pub fn save_status(&self) -> Option<SaveStatus<'_>> {
        let document = self.session.document_id();
        if self.picker_saving(document) {
            return Some(SaveStatus::Saving);
        }
        self.save_failures.reason(document, self.session.rev()).map(SaveStatus::Failed)
    }

    /// A save of `document` failed for `reason`.
    pub(crate) fn note_save_failed(&mut self, document: u64, reason: &str) {
        let rev = self.find_document(document).map_or(0, |(s, _)| s.rev());
        self.save_failures.failed(document, rev, reason.to_string());
    }
}

/// The status bar item for [`WordApp::save_status`], if any.
pub(crate) fn status_item(app: &mut WordApp, ui: &mut Ui, t: &Tokens) {
    let (text, color, reason) = match app.save_status() {
        None => return,
        Some(SaveStatus::Saving) => (tl!("Saving…"), t.text_dim, None),
        Some(SaveStatus::Failed(reason)) => (tl!("Save failed"), t.red, Some(reason.to_string())),
    };
    let font = TypeRung::Caption.regular();
    let track = TypeRung::Caption.tracking();
    let w = text_width(ui.painter(), text, &font, track, color);
    let (r, resp) = ui.allocate_exact_size(vec2(w + 1.0, TypeRung::Caption.line_height()), Sense::click());
    paint_text(ui.painter(), pos2(r.min.x, r.min.y), Align2::LEFT_TOP, text, &font, track, color);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    if let Some(reason) = reason {
        // A touch screen has no hover: a tap says why in the status bar.
        if resp.on_hover_text(&reason).clicked() {
            app.status(reason);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use egui_kittest::kittest::Queryable;
    use serde_json::json;
    use wordcraft_engine::Session;

    use super::SaveStatus;
    use crate::{Download, Services, WordApp};

    fn app() -> WordApp {
        WordApp::new(Session::new(wordcraft_doc::Document::new()), Services::default())
    }

    fn frame(a: &mut WordApp) {
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |ui| a.logic(ui.ctx())).drop_without_applying_deltas();
    }

    #[test]
    fn a_failed_desktop_save_shows_until_the_next_edit_or_save() {
        let dir = std::env::temp_dir().join(format!("wordcraft-save-status-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut a = app();
        a.run("text.insert", json!({"text": "Hello"})).unwrap();
        assert_eq!(a.save_status(), None);
        let nowhere = dir.join("missing-folder").join("a.docx");
        assert!(a.run("file.save", json!({"path": nowhere.to_string_lossy()})).is_err());
        assert!(matches!(a.save_status(), Some(SaveStatus::Failed(reason)) if !reason.is_empty()));

        a.run("text.insert", json!({"text": "!"})).unwrap();
        assert_eq!(a.save_status(), None, "an edit ends it");

        assert!(a.run("file.save", json!({"path": nowhere.to_string_lossy()})).is_err());
        assert!(a.save_status().is_some());
        a.run("file.save", json!({"path": dir.join("a.docx").to_string_lossy()})).unwrap();
        assert_eq!(a.save_status(), None, "so does a save that works");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_picker_save_shows_saving_then_its_outcome() {
        let mut a = app();
        let (tx, rx) = channel();
        let rx = std::cell::RefCell::new(Some(rx));
        a.services.download = Some(Box::new(move |_, _| rx.borrow_mut().take().map(Download::Pending).ok_or_else(|| "disk full".to_string())));
        a.run("text.insert", json!({"text": "Hello"})).unwrap();
        a.run("file.save", json!({})).unwrap();
        assert_eq!(a.save_status(), Some(SaveStatus::Saving));
        tx.send(Err(Some("NotAllowedError".into()))).unwrap();
        frame(&mut a);
        assert_eq!(a.save_status(), Some(SaveStatus::Failed("Couldn't write the file. The document is still unsaved.")));
        // A download that fails outright fails too.
        assert!(a.run("file.save", json!({})).is_err());
        assert_eq!(a.save_status(), Some(SaveStatus::Failed("Couldn't write the file. The document is still unsaved.")));
    }

    #[test]
    fn a_cancelled_picker_is_no_failure_and_a_written_one_clears_it() {
        let mut a = app();
        let answers = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let queued = answers.clone();
        a.services.download = Some(Box::new(move |_, _| {
            let (tx, rx) = channel();
            queued.borrow_mut().push(tx);
            Ok(Download::Pending(rx))
        }));
        a.run("text.insert", json!({"text": "Hello"})).unwrap();
        a.run("file.save", json!({})).unwrap();
        answers.borrow_mut().remove(0).send(Err(None)).unwrap();
        frame(&mut a);
        assert_eq!(a.save_status(), None);
        a.run("file.save", json!({})).unwrap();
        answers.borrow_mut().remove(0).send(Err(Some("quota".into()))).unwrap();
        frame(&mut a);
        assert!(a.save_status().is_some());
        a.run("file.save", json!({})).unwrap();
        answers.borrow_mut().remove(0).send(Ok(())).unwrap();
        frame(&mut a);
        assert_eq!(a.save_status(), None);
    }

    #[test]
    fn the_status_bar_says_it_in_vietnamese() {
        let mut a = app();
        a.run("ui.language", json!({"value": "vi"})).unwrap();
        let (tx, rx) = channel();
        let rx = std::cell::RefCell::new(Some(rx));
        a.services.download = Some(Box::new(move |_, _| rx.borrow_mut().take().map(Download::Pending).ok_or_else(|| "x".to_string())));
        a.run("text.insert", json!({"text": "Hello"})).unwrap();
        a.run("file.save", json!({})).unwrap();
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1366.0, 768.0)).build_ui_state(
            |ui, app: &mut WordApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            a,
        );
        h.run_steps(3);
        assert!(h.query_by_label("Đang lưu…").is_some());
        tx.send(Err(Some("NotAllowedError".into()))).unwrap();
        h.run_steps(3);
        assert!(h.query_by_label("Đang lưu…").is_none());
        assert!(h.query_by_label("Lưu không thành công").is_some());
    }

    /// At phone width the whole label shows, ahead of the view buttons: cut short, "Lưu không"
    /// would read as its opposite.
    #[test]
    fn a_narrow_status_bar_shows_the_whole_label() {
        let mut a = app();
        a.run("ui.language", json!({"value": "vi"})).unwrap();
        let (tx, rx) = channel();
        let rx = std::cell::RefCell::new(Some(rx));
        a.services.download = Some(Box::new(move |_, _| rx.borrow_mut().take().map(Download::Pending).ok_or_else(|| "x".to_string())));
        a.run("text.insert", json!({"text": "Hello"})).unwrap();
        a.run("file.save", json!({})).unwrap();
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(390.0, 844.0)).build_ui_state(
            |ui, app: &mut WordApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            a,
        );
        h.run_steps(4);
        let views = ["Chế độ Đọc", "Bố trí In"]
            .into_iter()
            .flat_map(|l| h.query_all_by_label(l).map(|n| n.rect()).collect::<Vec<_>>())
            .filter(|r| r.min.y > 800.0)
            .map(|r| r.min.x)
            .fold(f32::MAX, f32::min);
        assert!(views < 390.0, "the status bar's view buttons");
        assert!(h.get_by_label("Đang lưu…").rect().max.x < views);
        tx.send(Err(Some("NotAllowedError".into()))).unwrap();
        h.run_steps(4);
        assert!(h.get_by_label("Lưu không thành công").rect().max.x < views);
    }
}
