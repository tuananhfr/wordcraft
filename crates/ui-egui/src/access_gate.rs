//! The host's grant for the shown document ([`wordcraft_engine::Access`], `erp-host.md` v2 rules
//! 12–14), and the desktop app's edit lock on every document ([`crate::account::EditLock`]), where
//! the interface reaches past the engine's own check: interface commands (`ui.*`), Save As and
//! Export written by the browser, typed text and the input method, pastes, file drops, dialogs,
//! drags on the rulers, and whatever egui copies to the clipboard. Without a host or a lock every
//! document has full access and nothing here refuses anything.

mod dialogs;

use serde_json::Value;
use wordcraft_engine::access::Refusal;

pub(crate) use dialogs::{close_refused, offered};

use crate::{WordApp, i18n};

/// Interface commands that only change the document.
fn ui_edits(id: &str) -> bool {
    matches!(id, "ui.changePicture" | "ui.linkTextBox" | "ui.openRecipientList") || id.starts_with("ui.zotero.")
}

/// Interface commands that print or hand the document to another app.
fn ui_takes_content_out(id: &str) -> bool {
    matches!(id, "ui.print" | "ui.printSubmit" | "ui.printSettings" | crate::share::SHARE)
}

fn message(access: wordcraft_engine::Access, refusal: Refusal) -> &'static str {
    match refusal {
        // Comment mode is view-only until comments come to the web (owner's decision for M5).
        Refusal::ReadOnly if access.commenting => tl!("Commenting on the web is coming later; for now you can only view."),
        Refusal::ReadOnly => tl!("This document is read-only."),
        Refusal::NoDownload => tl!("Downloading, printing and copying are turned off for this document."),
    }
}

impl WordApp {
    /// The shown document can't be changed (opened for viewing or commenting, or editing locked).
    pub fn read_only(&self) -> bool {
        self.session.granted().read_only
    }

    /// The shown document may be saved as a copy, exported, printed and copied from.
    pub fn can_download(&self) -> bool {
        self.session.granted().can_download
    }

    fn access_refusal(&self, id: &str) -> Option<Refusal> {
        let access = self.session.granted();
        match self.session.registry.get(id) {
            Some(spec) => access.refusal(spec),
            None if access.read_only && ui_edits(id) => Some(Refusal::ReadOnly),
            None if !access.can_download && ui_takes_content_out(id) => Some(Refusal::NoDownload),
            None => None,
        }
    }

    /// Whether the host's grant and the edit lock let the user run `id` on the shown document
    /// (for greying out).
    pub(crate) fn access_allows(&self, id: &str) -> bool {
        self.access_refusal(id).is_none()
    }

    /// Says why in the status bar. A refusal by the edit lock gives the lock's reason and tells
    /// the account hook, whose next sync may lift it.
    pub(crate) fn say_refused(&mut self, refusal: Refusal) -> &'static str {
        let access = self.session.granted();
        let lock = self.edit_lock().filter(|_| refusal == Refusal::ReadOnly && access.may_save);
        let text = match lock {
            Some(lock) => i18n::t(lock.reason()),
            None => message(access, refusal),
        };
        if lock.is_some()
            && let Some(hook) = &self.services.account
        {
            hook.edit_refused();
        }
        self.status(text);
        text
    }

    /// The command entry points' check: a refused command says why in the status bar and does
    /// nothing else.
    pub(crate) fn refuse_by_access(&mut self, id: &str) -> Option<Result<Value, String>> {
        let refusal = self.access_refusal(id)?;
        Some(Err(self.say_refused(refusal).to_string()))
    }

    /// A file dropped on a document the host shows read-only: nothing is inserted or opened from
    /// it. Under the edit lock a document still opens; what would insert is refused as a command.
    pub(crate) fn refuse_drop(&mut self) -> bool {
        if !self.session.access.read_only {
            return false;
        }
        self.say_refused(Refusal::ReadOnly);
        true
    }
}

/// The canvas's input for a read-only document: no typed text, composition or paste reaches it
/// (their keys still move and select), and without download no cut or copy either.
pub(crate) fn canvas_input(app: &mut WordApp, events: &mut Vec<egui::Event>) {
    let (read_only, can_download) = (app.read_only(), app.can_download());
    if !read_only && can_download {
        return;
    }
    let mut refused = false;
    events.retain(|e| {
        let keep = match e {
            // The input method's own enable/disable events are no attempt to edit: no notice.
            egui::Event::Ime(_) => return !read_only,
            egui::Event::Text(_) | egui::Event::Paste(_) => !read_only,
            egui::Event::Cut => !read_only && can_download,
            egui::Event::Copy => can_download,
            _ => true,
        };
        refused |= !keep;
        keep
    });
    if read_only {
        app.canvas.ime_preedit.clear();
    }
    if refused {
        let refusal = if read_only { Refusal::ReadOnly } else { Refusal::NoDownload };
        app.say_refused(refusal);
    }
}

/// End of frame: without download, nothing egui copied this frame (a selected label, a text
/// field) reaches the clipboard.
pub(crate) fn end_frame(app: &WordApp, ctx: &egui::Context) {
    if app.can_download() {
        return;
    }
    ctx.output_mut(|o| o.commands.retain(|c| !matches!(c, egui::output::OutputCommand::CopyText(_) | egui::output::OutputCommand::CopyImage(_))));
}

/// A drag handle on a ruler (a margin, an indent, a table border): on a read-only document it
/// only hovers, so no resize pointer shows and no drag starts.
pub(crate) fn drag_handle(app: &WordApp, ui: &egui::Ui, zone: egui::Rect, id: egui::Id, cursor: Option<egui::CursorIcon>) -> egui::Response {
    if app.read_only() {
        return ui.interact(zone, id, egui::Sense::hover());
    }
    let r = ui.interact(zone, id, egui::Sense::drag());
    match cursor {
        Some(c) => r.on_hover_cursor(c),
        None => r,
    }
}
