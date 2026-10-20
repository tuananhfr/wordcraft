//! One warning before a user save discards document features.

use serde::Serialize;
use serde_json::{Value, json};
use wordcraft_engine::compatibility::DocFacts;

use crate::WordApp;
use crate::dialogs::Dialog;
use crate::file_dialogs::{AfterPick, AfterSave, FileDialogRequest};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum LossItem {
    Comments(usize),
    TrackedChanges(usize),
    Vba,
    Signature,
}

pub fn lost_items(facts: DocFacts, target_format: &str) -> Vec<LossItem> {
    let target = target_format.trim_start_matches('.').to_ascii_lowercase();
    let structured = matches!(target.as_str(), "docx" | "docm" | "dotx" | "dotm" | "xml" | "flatopc" | "wordml2003" | "json");
    let macros = matches!(target.as_str(), "docm" | "dotm" | "json");
    let mut items = Vec::new();
    if !structured {
        if facts.comments > 0 {
            items.push(LossItem::Comments(facts.comments));
        }
        if facts.tracked_changes > 0 {
            items.push(LossItem::TrackedChanges(facts.tracked_changes));
        }
    }
    if target == "wordml2003" && facts.tracked_formatting > 0 {
        items.push(LossItem::TrackedChanges(facts.tracked_formatting));
    }
    if facts.vba && !macros {
        items.push(LossItem::Vba);
    }
    if facts.signed {
        items.push(LossItem::Signature);
    }
    items
}

#[derive(Clone, Debug, Serialize)]
pub struct PendingSave {
    command: String,
    params: Value,
    document: u64,
    revision: u64,
    name: String,
    format: String,
    copy: bool,
    pub(crate) items: Vec<LossItem>,
    pub(crate) after: Option<AfterSave>,
}

impl PendingSave {
    pub(crate) fn document_id(&self) -> u64 {
        self.document
    }
}

impl WordApp {
    pub(crate) fn warn_before_save(&mut self, command: &str, params: &Value) -> bool {
        if !matches!(command, "file.save" | "file.saveAs" | "file.exportPdf" | "file.exportPng" | "file.printPdf") {
            return false;
        }
        let web = self.services.download.is_some();
        let name = params.get("path").and_then(Value::as_str).map(str::to_string).or_else(|| {
            if web { Some(self.default_save_name()) } else { self.session.path.as_ref().map(|p| p.to_string_lossy().into_owned()) }
        });
        let Some(mut name) = name else { return false };
        // Desktop Save As without a path opens the picker before its target is known.
        if command == "file.saveAs" && !web && params.get("path").and_then(Value::as_str).is_none() {
            return false;
        }
        let mut format = match command {
            "file.exportPdf" | "file.printPdf" => "pdf".to_string(),
            "file.exportPng" => "png".to_string(),
            _ if params.get("format").and_then(Value::as_str) == Some("wordml2003") => "wordml2003".to_string(),
            _ => extension(&name),
        };
        if format == "xml" && params.get("format").is_none() && !web
            && wordcraft_engine::cmd::file::is_word_2003_file(std::path::Path::new(&name))
        {
            format = "wordml2003".into();
        }
        if command == "file.exportPdf" && web && !name.ends_with(".pdf") {
            name.push_str(".pdf");
        }
        let items = lost_items(DocFacts::of(&self.session), &format);
        if items.is_empty() {
            return false;
        }
        let mut params = if params.is_object() { params.clone() } else { json!({}) };
        params["path"] = json!(name);
        if format == "wordml2003" {
            params["format"] = json!(format);
        }
        self.dialog = Some(Dialog::LossWarning(PendingSave {
            command: command.into(),
            params,
            document: self.session.document_id(),
            revision: self.session.rev(),
            name,
            format,
            copy: false,
            items,
            after: None,
        }));
        true
    }

    pub(crate) fn accept_loss(&mut self, pending: PendingSave) -> Result<Value, String> {
        if pending.copy {
            let path = pending.params.get("path").and_then(Value::as_str).map(str::to_string);
            return self.write_loss_copy(pending, path);
        }
        let document = pending.document;
        self.in_document(document, |app| {
            // A stale confirmation must never authorize dropping newly added content.
            if app.session.rev() != pending.revision && app.warn_before_save(&pending.command, &pending.params) {
                if let Some(Dialog::LossWarning(next)) = &mut app.dialog {
                    next.after = pending.after;
                }
                return Ok(json!({"pending": "lossWarning"}));
            }
            let result = app.execute(&pending.command, pending.params);
            app.finish_save(pending.after.unwrap_or(AfterSave::Nothing), result)
        })
        .unwrap_or_else(|| Ok(json!({"cancelled": true})))
    }

    pub(crate) fn choose_loss_copy(&mut self, pending: PendingSave) {
        let name = copy_name(&pending.name, &pending.format);
        if self.services.download.is_some() {
            if let Err(error) = self.write_loss_copy(pending, Some(name)) {
                self.status(error);
            }
        } else {
            let _ = self.ask_file(FileDialogRequest::Save { name }, AfterPick::LossCopy(pending));
        }
    }

    pub(crate) fn write_loss_copy(&mut self, pending: PendingSave, path: Option<String>) -> Result<Value, String> {
        let Some(mut path) = path else { return Ok(json!({"cancelled": true})) };
        let ext = file_extension(&pending.format);
        if extension(&path) != ext {
            path.push('.');
            path.push_str(ext);
        }
        self.in_document(pending.document, |app| {
            if app.session.rev() != pending.revision {
                let mut params = pending.params.clone();
                params["path"] = json!(path);
                if app.warn_before_save(&pending.command, &params) {
                    if let Some(Dialog::LossWarning(next)) = &mut app.dialog {
                        next.copy = true;
                    }
                    return Ok(json!({"pending": "lossWarning"}));
                }
            }
            let opts = wordcraft_engine::cmd::file::export_options(&app.session, &pending.params);
            let password = app.session.password.as_ref().map(wordcraft_engine::Password::as_str);
            if let Some(download) = &app.services.download {
                let bytes = wordcraft_engine::io::save_bytes_with(&path, &app.session.doc, password, &opts)?;
                let answer = download(&path, &bytes)?;
                if let crate::Download::Pending(rx) = answer {
                    app.track_picker_save(rx, &path, false, false);
                }
            } else {
                if app.session.path.as_ref().is_some_and(|original| same_file(original, std::path::Path::new(&path))) {
                    return Err(tl!("Choose a different name for the copy.").into());
                }
                if matches!(pending.command.as_str(), "file.printPdf" | "file.exportPng") {
                    let mut copy = wordcraft_engine::Session::new(app.session.doc.clone());
                    copy.prefs = app.session.prefs.clone();
                    copy.sel = app.session.sel.clone();
                    copy.page_hint = app.session.page_hint;
                    let mut params = pending.params;
                    params["path"] = json!(path);
                    copy.run(&pending.command, &params).map_err(|error| error.to_string())?;
                } else {
                    wordcraft_engine::io::save_path_with(std::path::Path::new(&path), &app.session.doc, password, &opts)?;
                }
            }
            Ok(json!({"copied": path}))
        })
        .unwrap_or_else(|| Ok(json!({"cancelled": true})))
    }
}

fn extension(name: &str) -> String {
    name.rsplit(['/', '\\']).next().unwrap_or(name).rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).unwrap_or_default()
}

fn file_extension(format: &str) -> &str {
    match format {
        "wordml2003" | "flatopc" => "xml",
        other => other,
    }
}

fn copy_name(name: &str, format: &str) -> String {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = file.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(file);
    format!("{stem} (1).{}", file_extension(format))
}

fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

pub(crate) fn body(app: &mut WordApp, ui: &mut egui::Ui, pending: &PendingSave) -> bool {
    ui.set_min_width(0.0);
    ui.set_max_width((ui.ctx().content_rect().width() - 64.0).clamp(160.0, 420.0));
    for item in &pending.items {
        let label = match item {
            LossItem::Comments(count) => crate::i18n::fmt(tl!("Comments ({count})"), &[("count", &count.to_string())]),
            LossItem::TrackedChanges(count) => crate::i18n::fmt(tl!("Tracked changes ({count})"), &[("count", &count.to_string())]),
            LossItem::Vba => tl!("VBA macros").to_string(),
            LossItem::Signature => tl!("Digital signature (removed or invalidated)").to_string(),
        };
        ui.label(format!("• {label}"));
    }
    let mut answer = None;
    ui.horizontal_wrapped(|ui| {
        if ui.button(tl!("Save anyway")).clicked() {
            answer = Some(0);
        }
        if ui.button(tl!("Save a copy")).clicked() {
            answer = Some(1);
        }
        if ui.button(tl!("Cancel")).clicked() {
            answer = Some(2);
        }
    });
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        return true;
    }
    match answer {
        Some(0) => {
            if let Err(error) = app.accept_loss(pending.clone()) {
                app.status(error);
            }
            true
        }
        Some(1) => {
            app.choose_loss_copy(pending.clone());
            true
        }
        Some(_) => true,
        None => false,
    }
}
