//! Dialogs on a read-only document (`erp-host.md` v2 rule 12): only those that show something or
//! set the app's own preferences open; the ones whose OK edits the document are not offered.

use serde_json::Value;
use wordcraft_engine::access::Refusal;

use crate::WordApp;
use crate::dialogs::Dialog;

/// Dialogs that leave the document as it is. The Find dialog is here too: its Replace tab is
/// hidden while the document is read-only.
fn leaves_document(name: &str) -> bool {
    matches!(
        name,
        "find"
            | "goto"
            | "wordCount"
            | "zoom"
            | "about"
            | "contributors"
            | "models"
            | "commands"
            | "shortcuts"
            | "customizeKeyboard"
            | "customizeRibbon"
            | "customizeQuickAccess"
            | "lossWarning"
            | "legacyDoc"
            | "saveChanges"
            | "saveAll"
            | "password"
    )
}

impl WordApp {
    /// Whether dialog `name` may open on the shown document.
    pub(crate) fn dialog_allowed(&self, name: &str) -> bool {
        !self.read_only() || leaves_document(name)
    }

    /// [`Dialog::open`]'s check: a refused dialog says why in the status bar and doesn't open.
    pub(crate) fn refuse_dialog(&mut self, name: &str) -> bool {
        if self.dialog_allowed(name) {
            return false;
        }
        self.say_refused(Refusal::ReadOnly);
        true
    }
}

/// Whether a button running `id` with `params` is offered: [`crate::widgets::enabled`], and for
/// `ui.dialog` whether that dialog may open.
pub(crate) fn offered(app: &WordApp, id: &str, params: &Value) -> bool {
    if !crate::widgets::enabled(app, id) {
        return false;
    }
    id != "ui.dialog" || params.get("name").and_then(Value::as_str).is_none_or(|name| app.dialog_allowed(name))
}

/// Before the dialogs are drawn: one opened without [`Dialog::open`] (a style's Modify…, a dialog
/// that leads to another) or left open when `setMode` turned editing off closes.
pub(crate) fn close_refused(app: &mut WordApp) {
    let refused = app.dialog.as_ref().is_some_and(|d| !matches!(d, Dialog::Find { .. }) && !app.dialog_allowed(d.name()));
    if refused {
        app.dialog = None;
    }
}
