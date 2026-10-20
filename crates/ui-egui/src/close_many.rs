//! Closing several documents at once (Close All, Close Others, closing the window): one question
//! listing every unsaved one, Save All / Don't Save / Cancel, instead of one prompt each. Save All
//! saves them one after another, each save finished before the next starts. In the browser each
//! save picker needs a click of its own, so once one has written its file the question stays up,
//! marks that document saved and offers to save the next. A save that is cancelled or fails stops
//! there and nothing still unsaved is closed. A single unsaved document gets "Save changes?".

use egui::Ui;
use serde_json::{Value, json};

use crate::dialogs::Dialog;
use crate::file_dialogs::{AfterSave, Asked};
use crate::open_documents::prompt_name_of;
use crate::{SaveChoice, WordApp};

/// Saves what is left to save, then closes: what an answered question continues with, also once
/// a save that finishes later has written its file.
pub(crate) const CLOSE_DOCUMENTS: &str = "ui.closeDocuments";

fn ids(p: &Value, key: &str) -> Vec<u64> {
    p.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default()
}

/// Where Save All has got to: the documents it asked about, those saved so far and the one whose
/// save picker is still writing.
struct Progress {
    unsaved: Vec<u64>,
    saved: Vec<u64>,
    saving: Option<u64>,
}

impl WordApp {
    /// Every open document's id, in tab order.
    fn all_document_ids(&self) -> Vec<u64> {
        self.document_tabs().iter().map(|t| t.document).collect()
    }

    /// The documents `ui.closeAll` / `ui.closeOthers` close: all, or all but `document` / `index`.
    pub(crate) fn close_targets(&self, id: &str, p: &Value) -> Result<Vec<u64>, String> {
        let all = self.all_document_ids();
        if id == "ui.closeAll" {
            return Ok(all);
        }
        let keep = self.document_param(p).ok().and_then(|i| self.document_id_at(i)).ok_or("no such document")?;
        Ok(all.into_iter().filter(|d| *d != keep).collect())
    }

    /// The user closes `targets`; `quit` closes the window once they are. Unsaved ones are asked
    /// about first, all in one question.
    pub(crate) fn ask_close(&mut self, targets: Vec<u64>, quit: bool) -> Result<Value, String> {
        if let Some(document) = targets.iter().copied().find(|d| self.picker_saving(*d)) {
            let params = json!({"documents": targets, "quit": quit, "ask": true});
            self.continue_after_picker_save(document, CLOSE_DOCUMENTS.into(), params);
            return Ok(json!({"pending": "save"}));
        }
        let unsaved: Vec<u64> = targets.iter().copied().filter(|d| self.find_document(*d).is_some_and(|(s, _)| s.dirty)).collect();
        match unsaved.as_slice() {
            [] => self.close_documents(&json!({"documents": targets, "quit": quit})),
            [one] => {
                let Some(name) = self.find_document(*one).map(|(s, u)| prompt_name_of(s, u)) else { return Err("no such document".into()) };
                let params = json!({"documents": targets, "quit": quit, "discard": [one]});
                self.dialog = Some(Dialog::SaveChanges { name, then: CLOSE_DOCUMENTS.into(), params, document: *one });
                Ok(json!({"pending": "saveChanges"}))
            }
            _ => {
                self.show_save_all(targets, quit, Progress { unsaved, saved: Vec::new(), saving: None });
                Ok(json!({"pending": "saveAll"}))
            }
        }
    }

    fn show_save_all(&mut self, targets: Vec<u64>, quit: bool, progress: Progress) {
        let names = progress.unsaved.iter().map(|d| self.find_document(*d).map(|(s, u)| prompt_name_of(s, u)).unwrap_or_default()).collect();
        let Progress { unsaved, saved, saving } = progress;
        self.dialog = Some(Dialog::SaveAll { names, documents: unsaved, targets, quit, saved, saving });
    }

    /// Answer the Save All question (`ui.saveChanges` while it is open).
    pub(crate) fn answer_save_all(&mut self, choice: SaveChoice) -> Result<Value, String> {
        let Some(Dialog::SaveAll { documents, targets, quit, saved, saving, .. }) = self.dialog.take() else {
            return Err("no Save All prompt is open".into());
        };
        match choice {
            SaveChoice::Cancel => Ok(json!({"done": false})),
            // Its picker's answer carries on; a second one can't open meanwhile.
            SaveChoice::Save | SaveChoice::DontSave if saving.is_some() => {
                self.show_save_all(targets, quit, Progress { unsaved: documents, saved, saving });
                Err("a document is still being saved".into())
            }
            SaveChoice::DontSave => self.close_documents(&json!({"documents": targets, "quit": quit, "discard": documents})),
            SaveChoice::Save => {
                let save: Vec<u64> = documents.iter().copied().filter(|d| !saved.contains(d)).collect();
                self.close_documents(&json!({"documents": targets, "quit": quit, "save": save, "unsaved": documents, "saved": saved}))
            }
        }
    }

    /// A save picker Save All waited for didn't write its document (cancelled, failed, or the
    /// document changed meanwhile): the question goes, closing nothing.
    pub(crate) fn stop_save_all(&mut self, document: u64) {
        if matches!(&self.dialog, Some(Dialog::SaveAll { saving: Some(d), .. }) if *d == document) {
            self.dialog = None;
        }
    }

    /// `ui.closeDocuments`: save each of `save` that is still unsaved, one at a time, then close
    /// `documents` (or, with `quit`, let the window close). A Save As dialog carries on from here
    /// once it has saved; a browser save picker (`wait`) asks again before the next one. A save
    /// that is cancelled or fails stops, closing nothing. Anything unsaved by then that isn't in
    /// `discard` stays open.
    pub(crate) fn close_documents(&mut self, p: &Value) -> Result<Value, String> {
        let (targets, mut save, discard) = (ids(p, "documents"), ids(p, "save"), ids(p, "discard"));
        let quit = p.get("quit").and_then(Value::as_bool) == Some(true);
        if p.get("ask").and_then(Value::as_bool) == Some(true) {
            return self.ask_close(targets, quit);
        }
        let unsaved = Some(ids(p, "unsaved")).filter(|u| !u.is_empty()).unwrap_or_else(|| save.clone());
        let mut saved = ids(p, "saved");
        if let Some(waited) = p.get("wait").and_then(Value::as_u64) {
            // The question was closed while the picker was open: that was a Cancel.
            if !matches!(&self.dialog, Some(Dialog::SaveAll { saving: Some(d), .. }) if *d == waited) {
                return Ok(json!({"done": false}));
            }
            self.dialog = None;
            if save.iter().any(|d| self.find_document(*d).is_some_and(|(s, _)| s.dirty)) {
                self.show_save_all(targets, quit, Progress { unsaved, saved, saving: None });
                return Ok(json!({"pending": "saveAll"}));
            }
        }
        while !save.is_empty() {
            let next = save.remove(0);
            let Some((session, _)) = self.find_document(next) else { continue };
            if !session.dirty {
                saved.push(next);
                continue;
            }
            let untitled_here = session.path.is_none() && self.services.download.is_none();
            let mut done = saved.clone();
            done.push(next);
            let rest = json!({"documents": targets, "save": save, "discard": discard, "quit": quit, "unsaved": unsaved, "saved": done});
            if untitled_here {
                let after = AfterSave::Continue { then: CLOSE_DOCUMENTS.into(), params: rest, document: next };
                return match self.in_document(next, |app| app.save_as(after)) {
                    Some(Asked::Done(r)) => r,
                    Some(Asked::Pending) => Ok(json!({"pending": "saveAs"})),
                    Some(Asked::Busy) | None => Ok(json!({"done": false})),
                };
            }
            match self.in_document(next, WordApp::save_for_prompt).flatten() {
                Some(true) => saved.push(next),
                Some(false) => return Ok(json!({"done": false})),
                None => {
                    if matches!(self.dialog, Some(Dialog::LossWarning(_))) {
                        self.continue_after_picker_save(next, CLOSE_DOCUMENTS.into(), rest);
                        return Ok(json!({"pending": "lossWarning"}));
                    }
                    let mut rest = rest;
                    rest["wait"] = json!(next);
                    self.continue_after_picker_save(next, CLOSE_DOCUMENTS.into(), rest);
                    self.show_save_all(targets, quit, Progress { unsaved, saved, saving: Some(next) });
                    return Ok(json!({"pending": "save"}));
                }
            }
        }
        let kept: Vec<u64> = targets
            .iter()
            .copied()
            .filter(|d| !discard.contains(d) && self.find_document(*d).is_some_and(|(s, _)| s.dirty || self.picker_save_pending(*d)))
            .collect();
        if quit {
            // The window goes with every document in it: nothing to close one by one.
            self.quit_requested |= kept.is_empty();
            return Ok(json!({"done": kept.is_empty()}));
        }
        let mut closed = 0;
        for d in targets.iter().filter(|d| !kept.contains(d)) {
            if let Some(i) = self.document_index(*d) {
                self.close_document_at(i);
                closed += 1;
            }
        }
        Ok(json!({"done": kept.is_empty(), "closed": closed, "documents": self.document_count()}))
    }
}

/// The Save All question's body; true once it is answered.
pub(crate) fn save_all_body(app: &mut WordApp, ui: &mut Ui, d: &Dialog) -> bool {
    let Dialog::SaveAll { names, documents, saved, saving, .. } = d else { return true };
    let count = names.len().to_string();
    ui.label(
        egui::RichText::new(crate::i18n::fmt(tl!("Do you want to save changes to these {count} documents?"), &[("count", &count)]))
            .font(crate::theme::semibold(15.0)),
    );
    ui.add_space(4.0);
    egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
        for (name, document) in names.iter().zip(documents) {
            let row = if saved.contains(document) {
                crate::i18n::fmt(tl!("{name} — Saved"), &[("name", name)])
            } else if *saving == Some(*document) {
                crate::i18n::fmt(tl!("{name} — Saving…"), &[("name", name)])
            } else {
                name.clone()
            };
            ui.label(format!("•  {row}"));
        }
    });
    ui.add_space(4.0);
    ui.label(tl!("Your changes will be lost if you don't save them."));
    ui.add_space(8.0);
    // After the first save in the browser, the button names the document its click saves next.
    let next = names.iter().zip(documents).find(|(_, d)| !saved.contains(d) && *saving != Some(**d)).map(|(n, _)| n);
    let label = match next {
        Some(name) if !saved.is_empty() || saving.is_some() => crate::i18n::fmt(tl!("Save “{name}”"), &[("name", name)]),
        _ => tl!("Save All").to_string(),
    };
    let idle = saving.is_none();
    let buttons = ui
        .horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let cancel = ui.button(tl!("Cancel"));
                let dont_save = ui.add_enabled(idle, egui::Button::new(tl!("Don't Save")));
                let save = ui.add_enabled(idle, crate::widgets::primary_button(ui, &label));
                [(cancel, "cancel"), (dont_save, "dontSave"), (save, "save")]
            })
            .inner
        })
        .inner;
    // As "Save changes?": the save button has focus, Enter clicks the focused button, Esc cancels.
    if idle
        && ui.memory(|m| m.focused().is_none())
        && let Some((save, _)) = buttons.get(2)
    {
        save.request_focus();
    }
    let focused = buttons.iter().any(|(b, _)| b.has_focus());
    let answer = if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        Some("cancel")
    } else if let Some((_, a)) = buttons.iter().find(|(b, _)| b.clicked()) {
        Some(*a)
    } else if idle && !focused && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        Some("save")
    } else {
        None
    };
    let Some(answer) = answer else { return false };
    // `ui.saveChanges` reads the question from `app.dialog`, which `show` has taken out.
    app.dialog = Some(d.clone());
    let _ = app.run("ui.saveChanges", json!({"answer": answer}));
    true
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc::{Sender, channel};

    use egui_kittest::kittest::Queryable;
    use serde_json::json;
    use wordcraft_engine::Session;

    use crate::dialogs::Dialog;
    use crate::{Download, PickerAnswer, Services, WordApp};

    fn app() -> WordApp {
        WordApp::new(Session::new(wordcraft_doc::Document::new()), Services::default())
    }

    fn typed(a: &mut WordApp, text: &str) {
        a.run("text.insert", json!({"text": text})).unwrap();
    }

    /// Three documents: unsaved "A", clean "B" (shown second), unsaved "C" (shown).
    fn three(a: &mut WordApp) -> [u64; 3] {
        typed(a, "A");
        let first = a.session.document_id();
        a.run("file.new", json!({})).unwrap();
        let second = a.session.document_id();
        a.run("file.new", json!({})).unwrap();
        typed(a, "C");
        [first, second, a.session.document_id()]
    }

    fn save_all_names(a: &WordApp) -> Option<Vec<String>> {
        match &a.dialog {
            Some(Dialog::SaveAll { names, .. }) => Some(names.clone()),
            _ => None,
        }
    }

    /// Web: every save goes to a save picker the test answers.
    fn with_pickers(a: &mut WordApp) -> Rc<RefCell<Vec<Sender<PickerAnswer>>>> {
        let answers: Rc<RefCell<Vec<Sender<PickerAnswer>>>> = Rc::default();
        let queued = answers.clone();
        a.services.download = Some(Box::new(move |_, _| {
            let (tx, rx) = channel();
            queued.borrow_mut().push(tx);
            Ok(Download::Pending(rx))
        }));
        answers
    }

    fn frame(a: &mut WordApp) {
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |ui| a.logic(ui.ctx())).drop_without_applying_deltas();
    }

    #[test]
    fn close_all_asks_once_for_every_unsaved_document() {
        let mut a = app();
        let [first, _, third] = three(&mut a);
        assert_eq!(a.run("ui.closeAll", json!({})).unwrap()["pending"], "saveAll");
        assert_eq!(save_all_names(&a), Some(vec!["Document1".to_string(), "Document3".to_string()]));
        // Cancel closes nothing.
        a.run("ui.saveChanges", json!({"answer": "cancel"})).unwrap();
        assert!(a.dialog.is_none());
        assert_eq!(a.document_count(), 3);
        assert!(!a.quit_requested);
        // Don't Save closes them all (the last one quits, as closing the last tab does), and each
        // can be reopened unsaved.
        a.run("ui.closeAll", json!({})).unwrap();
        a.run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
        assert_eq!(a.document_count(), 1);
        assert!(a.quit_requested);
        assert!(a.find_document(first).is_none());
        assert_eq!(a.session.document_id(), third, "the last one is left for the window to take");
        assert!(a.can_reopen_document());
    }

    #[test]
    fn close_others_keeps_the_one_it_was_asked_from() {
        let mut a = app();
        let [first, second, third] = three(&mut a);
        a.run("ui.closeOthers", json!({"document": second})).unwrap();
        assert_eq!(save_all_names(&a), Some(vec!["Document1".to_string(), "Document3".to_string()]));
        a.run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
        assert_eq!(a.document_count(), 1);
        assert_eq!(a.session.document_id(), second);
        assert!(a.find_document(first).is_none() && a.find_document(third).is_none());
        assert!(!a.quit_requested);
    }

    #[test]
    fn one_unsaved_document_gets_the_usual_prompt() {
        let mut a = app();
        typed(&mut a, "A");
        a.run("file.new", json!({})).unwrap();
        a.run("file.new", json!({})).unwrap();
        assert_eq!(a.run("ui.closeOthers", json!({"index": 2})).unwrap()["pending"], "saveChanges");
        assert!(matches!(&a.dialog, Some(Dialog::SaveChanges { name, .. }) if name == "Document1"));
        a.run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
        assert_eq!(a.document_count(), 1);
    }

    #[test]
    fn nothing_unsaved_closes_without_asking() {
        let mut a = app();
        typed(&mut a, "A");
        a.run("file.save", json!({"path": std::env::temp_dir().join(format!("wc-close-many-{}.docx", std::process::id())).to_string_lossy()}))
            .unwrap();
        a.run("file.new", json!({})).unwrap();
        a.run("file.new", json!({})).unwrap();
        let shown = a.session.document_id();
        a.run("ui.closeOthers", json!({})).unwrap();
        assert!(a.dialog.is_none());
        assert_eq!(a.document_count(), 1);
        assert_eq!(a.session.document_id(), shown);
    }

    fn progress(a: &WordApp) -> Option<(Vec<u64>, Option<u64>)> {
        match &a.dialog {
            Some(Dialog::SaveAll { saved, saving, .. }) => Some((saved.clone(), *saving)),
            _ => None,
        }
    }

    /// Web: each save picker needs a click of its own, so after one has written its file the
    /// question stays up, marks it saved and the next click opens the next picker.
    #[test]
    fn save_all_takes_one_click_per_picker_and_then_closes() {
        let mut a = app();
        let answers = with_pickers(&mut a);
        let [first, second, third] = three(&mut a);
        a.run("ui.closeAll", json!({})).unwrap();
        assert_eq!(a.run("ui.saveChanges", json!({"answer": "save"})).unwrap()["pending"], "save");
        assert_eq!(answers.borrow().len(), 1, "one picker at a time");
        assert_eq!(progress(&a), Some((vec![], Some(first))), "the question stays up, its row saving");
        assert!(a.find_document(first).is_some_and(|(s, _)| s.dirty), "not saved before the write completes");
        assert!(a.run("ui.saveChanges", json!({"answer": "save"})).is_err(), "no second picker while one is open");
        frame(&mut a);
        assert_eq!(answers.borrow().len(), 1);
        answers.borrow_mut().remove(0).send(Ok(())).unwrap();
        frame(&mut a);
        assert!(answers.borrow().is_empty(), "the next picker waits for a click");
        assert_eq!(progress(&a), Some((vec![first], None)), "the first is marked saved");
        assert!(a.find_document(first).is_some_and(|(s, _)| !s.dirty));
        assert_eq!(a.document_count(), 3, "nothing closes before every save is written");
        assert_eq!(a.run("ui.saveChanges", json!({"answer": "save"})).unwrap()["pending"], "save");
        assert_eq!(answers.borrow().len(), 1, "the click opened the second picker");
        assert_eq!(progress(&a), Some((vec![first], Some(third))));
        answers.borrow_mut().remove(0).send(Ok(())).unwrap();
        frame(&mut a);
        assert!(a.dialog.is_none());
        assert!(a.find_document(third).is_some_and(|(s, _)| !s.dirty));
        assert_eq!(a.document_count(), 1, "all closed once saved");
        assert!(a.find_document(first).is_none() && a.find_document(second).is_none());
    }

    /// Web: Cancel in any picker stops the rest and closes nothing still unsaved.
    #[test]
    fn cancel_in_a_picker_stops_the_rest_and_closes_nothing() {
        let mut a = app();
        let answers = with_pickers(&mut a);
        let [first, _, third] = three(&mut a);
        a.run("ui.closeAll", json!({})).unwrap();
        a.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
        answers.borrow_mut().remove(0).send(Ok(())).unwrap();
        frame(&mut a);
        a.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
        answers.borrow_mut().remove(0).send(Err(None)).unwrap();
        frame(&mut a);
        frame(&mut a);
        assert!(a.dialog.is_none(), "the question goes");
        assert!(answers.borrow().is_empty(), "no further picker");
        assert_eq!(a.document_count(), 3, "nothing closed");
        assert!(a.find_document(first).is_some_and(|(s, _)| !s.dirty), "the first was saved");
        assert!(a.find_document(third).is_some_and(|(s, _)| s.dirty), "the cancelled one is still unsaved");
        assert!(!a.quit_requested);

        // A failed write in the first picker: nothing more saved, nothing closed.
        a.run("ui.activateDocument", json!({"document": first})).unwrap();
        typed(&mut a, " more");
        a.run("ui.closeAll", json!({})).unwrap();
        assert!(matches!(a.dialog, Some(Dialog::SaveAll { .. })));
        a.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
        answers.borrow_mut().remove(0).send(Err(Some("disk full".into()))).unwrap();
        frame(&mut a);
        assert!(a.dialog.is_none());
        assert!(answers.borrow().is_empty());
        assert_eq!(a.document_count(), 3);

        // Cancel on the question while a picker writes: its answer doesn't carry on.
        a.run("ui.closeAll", json!({})).unwrap();
        a.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
        a.run("ui.saveChanges", json!({"answer": "cancel"})).unwrap();
        answers.borrow_mut().remove(0).send(Ok(())).unwrap();
        frame(&mut a);
        assert!(a.dialog.is_none() && answers.borrow().is_empty());
        assert_eq!(a.document_count(), 3);
    }

    /// Browsers without a save picker download each at once: one click saves them all.
    #[test]
    fn plain_downloads_save_all_in_one_go() {
        let mut a = app();
        let n: Rc<RefCell<u32>> = Rc::default();
        let count = n.clone();
        a.services.download = Some(Box::new(move |_, _| {
            *count.borrow_mut() += 1;
            Ok(Download::Done)
        }));
        three(&mut a);
        a.run("ui.closeAll", json!({})).unwrap();
        a.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
        assert_eq!(*n.borrow(), 2);
        assert_eq!(a.document_count(), 1);
    }

    #[test]
    fn the_rows_and_button_say_how_far_it_has_got_in_vietnamese() {
        let mut a = app();
        let answers = with_pickers(&mut a);
        a.run("ui.language", json!({"value": "vi"})).unwrap();
        three(&mut a);
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1366.0, 768.0)).build_ui_state(
            |ui, app: &mut WordApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            a,
        );
        h.run_steps(3);
        h.state_mut().run("ui.closeAll", json!({})).unwrap();
        h.run_steps(3);
        h.get_by_label("Lưu tất cả").click();
        h.run_steps(3);
        assert!(h.query_by_label("•  Tài liệu1 — Đang lưu…").is_some());
        answers.borrow_mut().remove(0).send(Ok(())).unwrap();
        h.run_steps(3);
        assert!(h.query_by_label("•  Tài liệu1 — Đã lưu").is_some());
        h.get_by_label("Lưu “Tài liệu3”").click();
        h.run_steps(3);
        assert_eq!(answers.borrow().len(), 1, "the click opened the next picker");
    }

    /// Desktop: an untitled document asks where through Save As; cancelling it stops the rest.
    #[test]
    fn a_cancelled_save_as_stops_save_all() {
        let mut a = app();
        let asked: Rc<RefCell<u32>> = Rc::default();
        let count = asked.clone();
        a.services.pick_save = Some(Box::new(move |_| {
            *count.borrow_mut() += 1;
            None
        }));
        three(&mut a);
        a.run("ui.closeAll", json!({})).unwrap();
        a.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
        assert_eq!(*asked.borrow(), 1, "one Save As, cancelled, and no more");
        assert_eq!(a.document_count(), 3);
        assert!(a.any_document_dirty());
    }

    /// Desktop: Save All writes every untitled document where Save As says, then quits.
    #[test]
    fn save_all_on_quit_writes_each_document_then_quits() {
        let dir = std::env::temp_dir().join(format!("wordcraft-save-all-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut a = app();
        let n: Rc<RefCell<u32>> = Rc::default();
        let (count, folder) = (n.clone(), dir.clone());
        a.services.pick_save = Some(Box::new(move |_| {
            *count.borrow_mut() += 1;
            Some(folder.join(format!("doc{}.docx", count.borrow())).to_string_lossy().to_string())
        }));
        three(&mut a);
        assert!(!a.close_requested());
        a.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
        assert!(a.quit_requested);
        assert!(!a.any_document_dirty());
        for f in ["doc1.docx", "doc2.docx"] {
            assert!(dir.join(f).exists(), "{f}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_tab_menu_offers_close_all_close_others_and_reopen() {
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 900.0)).build_ui_state(
            |ui, app: &mut WordApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app(),
        );
        h.state_mut().run("ui.language", json!({"value": "vi"})).unwrap();
        three(h.state_mut());
        h.run_steps(3);
        h.get_by_label("Tài liệu2").click_secondary();
        h.run_steps(3);
        assert!(h.query_by_label("Đóng tất cả").is_some());
        assert!(h.query_by_label("Đóng các tài liệu khác").is_some());
        h.get_by_label("Đóng các tài liệu khác").click();
        h.run_steps(3);
        assert_eq!(save_all_names(h.state()), Some(vec!["Tài liệu1".to_string(), "Tài liệu3".to_string()]));
        assert!(h.query_by_label("Lưu tất cả").is_some());
        h.get_by_label("Không Lưu").click();
        h.run_steps(3);
        assert_eq!(h.state().document_count(), 1);
        // One document left: the strip is gone, File › Open reopens.
        h.state_mut().run("ui.backstage", json!({"page": "open"})).unwrap();
        h.run_steps(3);
        assert!(h.state().can_reopen_document());
        h.get_by_label("Mở lại tài liệu vừa đóng").click();
        h.run_steps(3);
        // It takes the untouched blank one's place, as any open does.
        let a = h.state();
        assert_eq!(a.document_count(), 1);
        assert!(a.session.doc.plain_text(wordcraft_doc::StoryRef::Body).contains('C') && a.session.dirty);
    }
}
