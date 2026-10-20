//! User-facing outcomes of the read-only legacy Word importer.

use serde_json::Value;

use crate::{WordApp, dialogs::Dialog};

pub(crate) fn after_open(app: &mut WordApp, result: &Result<Value, String>) {
    let opened = match result {
        Ok(v) if v.get("legacy").and_then(Value::as_bool) == Some(true) => true,
        Err(e) if wordcraft_engine::io::legacy::refused(e) => false,
        _ => return,
    };
    app.ui.backstage = false;
    app.status_msg = None;
    app.dialog = Some(Dialog::LegacyDoc { opened });
}

pub(crate) fn title(opened: bool) -> &'static str {
    if opened { "Legacy .doc file" } else { "Cannot open legacy .doc format" }
}

pub(crate) fn body(ui: &mut egui::Ui, opened: bool) -> bool {
    ui.set_max_width((ui.ctx().content_rect().width() - 64.0).clamp(160.0, 420.0));
    ui.label(if opened {
        tl!("The legacy .doc file has been opened. When saved, it will be saved as .docx")
    } else {
        tl!("This file uses the legacy Word format (.doc), which Văn bản cannot read yet. Open it in Word or LibreOffice, save it as .docx, then open it again.")
    });
    ui.add_space(8.0);
    ui.button(tl!("OK")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Escape))
}

#[cfg(test)]
mod tests;
