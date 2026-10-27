//! Share… (web, ERPCons office shell): hands a copy of the document to the system share sheet,
//! through the browser save's own path so loss warnings and format conversion still apply.

use serde_json::{Value, json};

use crate::WordApp;

/// The share command. Not a registered command, so it has no shortcut or command-search entry:
/// the share sheet only opens from the click that asked for it.
pub(crate) const SHARE: &str = "ui.share";

impl WordApp {
    /// Whether the host can share files ([`Services::share`](crate::Services::share)).
    pub fn can_share(&self) -> bool {
        self.services.share.is_some()
    }

    /// Share the document as the user asked (a loss warning may come first; its buttons share).
    /// `pointer`: a pointer click asked. Enter or Space on a focused button "clicks" a frame later,
    /// without the user activation the share sheet needs, so that downloads the copy instead.
    pub(crate) fn share_document(&mut self, pointer: bool) -> Result<Value, String> {
        self.run(SHARE, share_params(pointer))
    }

    /// `ui.share`: the bytes a save would download, offered to the share sheet; the host
    /// downloads them instead when it can't share.
    pub(crate) fn share_copy(&mut self, params: &Value) -> Result<Value, String> {
        if !self.can_share() {
            return Err("sharing files isn't available here".into());
        }
        self.browser_save(SHARE, params)
    }
}

/// `ui.share`'s parameters: `download` skips the share sheet.
pub(crate) fn share_params(pointer: bool) -> Value {
    json!({"download": !pointer})
}

/// Whether a `ui.share` with these parameters may open the share sheet.
pub(crate) fn may_share(params: &Value) -> bool {
    params.get("download").and_then(Value::as_bool) != Some(true)
}

/// Whether this widget was clicked with the pointer, not activated from the keyboard.
pub(crate) fn pointer_click(response: &egui::Response) -> bool {
    response.clicked_by(egui::PointerButton::Primary)
}

#[cfg(test)]
mod tests;
