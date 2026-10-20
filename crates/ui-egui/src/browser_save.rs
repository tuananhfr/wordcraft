//! Browser saves and their completion, bound to the document revision written.

use std::sync::mpsc::{Receiver, TryRecvError};

use serde_json::{Value, json};

use crate::file_dialogs::{AfterSave, Download, PickerAnswer};
use crate::{WordApp, i18n, keeps_everything};

#[cfg(test)]
mod tests;

/// A save still in the browser's picker, about the document it saved.
pub(crate) struct PickerSave {
    rx: Receiver<PickerAnswer>,
    name: String,
    document: u64,
    revision: u64,
    keeps_everything: bool,
    /// A save rather than an export: the status bar shows how it goes.
    save: bool,
    /// "Save changes?" answered Save: the command it held back (New, Open, Close…), run once the
    /// file is written.
    then: Option<(String, Value)>,
}

impl WordApp {
    pub(crate) fn browser_save(&mut self, id: &str, params: &Value) -> Result<Value, String> {
        let name = params.get("path").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| self.default_save_name());
        let name = if id == "file.exportPdf" && !name.ends_with(".pdf") { format!("{name}.pdf") } else { name };
        let saving = matches!(id, "file.save" | "file.saveAs");
        if saving && self.picker_saving(self.session.document_id()) {
            return Ok(json!({"downloaded": name, "pending": true}));
        }
        // A save with a password encrypts this and later saves (`file.save`'s `password`).
        if matches!(id, "file.save" | "file.saveAs")
            && let Some(pw) = params.get("password").filter(|v| !v.is_null())
        {
            if !wordcraft_engine::io::can_encrypt(&name) || pw.as_str().is_none_or(str::is_empty) {
                return Err("only Word documents (.docx, .docm, .dotx, .dotm) can be saved with a password, and it can't be empty".into());
            }
            self.session.run("file.encrypt", &json!({"password": pw})).map_err(|e| e.to_string())?;
        }
        // A save is a save, download or not: advance the revision and modified stamp (#262).
        if matches!(id, "file.save" | "file.saveAs") {
            self.session.stamp_save();
        }
        // Word documents are encrypted with the document's password, if it has one.
        let password =
            if matches!(id, "file.save" | "file.saveAs") { self.session.password.as_ref().map(wordcraft_engine::Password::as_str) } else { None };
        let opts = wordcraft_engine::cmd::file::export_options(&self.session, params);
        let document = self.session.document_id();
        let bytes = match wordcraft_engine::io::save_bytes_with(&name, &self.session.doc, password, &opts) {
            Ok(bytes) => bytes,
            Err(e) => {
                if saving {
                    self.note_save_failed(document, &e);
                }
                return Err(e);
            }
        };
        let picker = match self.services.download.as_ref().map(|d| d(&name, &bytes)) {
            Some(Err(e)) => {
                let msg = self.browser_write_failed(saving.then_some(document), &e);
                return Err(msg);
            }
            Some(Ok(Download::Pending(rx))) => Some(rx),
            Some(Ok(Download::Done)) | None => None,
        };
        // An export is a copy, and so is a save in a format that doesn't keep everything (a
        // page image, plain text): the document itself still has unsaved changes.
        let complete_save = saving && keeps_everything(&name);
        if complete_save && picker.is_none() {
            self.session.dirty = false;
            let _ = self.session.doc.passthrough.remove("wordcraft:package.signed");
        }
        let pending = picker.is_some();
        if let Some(rx) = picker {
            self.track_picker_save(rx, &name, complete_save, saving);
        } else if saving {
            self.save_failures.saved(document);
        }
        Ok(json!({"downloaded": name, "bytes": bytes.len(), "pending": pending}))
    }

    /// Remember a save the browser's picker finishes later ([`Download::Pending`]).
    pub(crate) fn track_picker_save(&mut self, rx: Receiver<PickerAnswer>, name: &str, keeps_everything: bool, save: bool) {
        let document = self.session.document_id();
        let revision = self.session.rev();
        if save {
            self.session.dirty = true;
        }
        self.picker_saves.push(PickerSave { rx, name: name.to_string(), document, revision, keeps_everything, save, then: None });
    }

    /// Whether a save of `document` is still in the browser's save picker.
    pub(crate) fn picker_save_pending(&self, document: u64) -> bool {
        self.picker_saves.iter().any(|s| s.document == document)
    }

    /// Whether a save (not an export) of `document` is still being written by the browser's picker.
    pub(crate) fn picker_saving(&self, document: u64) -> bool {
        self.picker_saves.iter().any(|s| s.document == document && s.save)
    }

    /// Hold `then` back until the save just started in the picker has written the file.
    pub(crate) fn continue_after_picker_save(&mut self, document: u64, then: String, params: Value) {
        if let Some(save) = self.picker_saves.iter_mut().find(|s| s.document == document && s.save) {
            save.then = Some((then, params));
            return;
        }
        if let Some(crate::dialogs::Dialog::LossWarning(pending)) = &mut self.dialog
            && pending.document_id() == document
        {
            pending.after = Some(AfterSave::Continue { then, params, document });
        }
    }

    /// Finish the picker saves that have been answered (every frame, from `logic`).
    pub(crate) fn poll_picker_saves(&mut self) {
        let mut i = 0;
        while let Some(save) = self.picker_saves.get(i) {
            let answer = match save.rx.try_recv() {
                Err(TryRecvError::Empty) => {
                    i += 1;
                    continue;
                }
                Ok(answer) => answer,
                // The host dropped the save without an answer: as good as cancelled.
                Err(TryRecvError::Disconnected) => Err(None),
            };
            let save = self.picker_saves.remove(i);
            self.finish_picker_save(save, answer);
        }
    }

    /// The answer is about the document that was saved, shown or not: the others are left alone.
    fn finish_picker_save(&mut self, save: PickerSave, answer: PickerAnswer) {
        match answer {
            Ok(()) => {
                if save.keeps_everything {
                    let _ = self.in_document(save.document, |app| {
                        if app.session.rev() == save.revision {
                            app.session.dirty = false;
                            app.session.doc.passthrough.remove("wordcraft:package.signed");
                        }
                    });
                }
                if save.save {
                    self.save_failures.saved(save.document);
                }
                // Unsaved again (changed since, or a format that doesn't keep everything):
                // carrying on would lose the changes. Closing goes by document; the mailings act
                // on the shown document, so only while that is still the one saved.
                if let Some((then, params)) = save.then {
                    let clean = self.find_document(save.document).is_some_and(|(s, _)| !s.dirty);
                    let by_document = then == "ui.closeDocument" || then == crate::close_many::CLOSE_DOCUMENTS;
                    if clean && (by_document || save.document == self.session.document_id()) {
                        let _ = self.execute_user(&then, params);
                    } else {
                        self.stop_save_all(save.document);
                    }
                }
            }
            Err(error) => {
                self.stop_save_all(save.document);
                if save.save {
                    self.mark_document_unsaved(save.document);
                }
                if let Some(error) = error {
                    let reason = format!("download of {} failed: {error}", save.name);
                    self.browser_write_failed(save.save.then_some(save.document), &reason);
                }
            }
        }
    }

    pub(crate) fn browser_write_failed(&mut self, document: Option<u64>, error: &str) -> String {
        log::error!("browser write failed: {error}");
        let message = i18n::tr(i18n::Lang::from_pref(&self.ui.language), "Couldn't write the file. The document is still unsaved.").to_string();
        if let Some(document) = document {
            self.mark_document_unsaved(document);
            self.note_save_failed(document, &message);
        }
        self.status(message.clone());
        message
    }
}
