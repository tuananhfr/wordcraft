//! What an embedding host lets the user do with the open document (ERPCons online documents
//! opened for viewing or commenting, `erp-host.md` v2). It belongs to the session, never to the
//! file: nothing here is read from or written into the document.

use crate::CommandSpec;

/// The host's grant for one document. The default (full access) is every document's outside the
/// host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Access {
    /// Nothing may change the document: every mutating command, undo and redo are refused.
    pub read_only: bool,
    /// Save As, Export, Print and copying the content to the clipboard are allowed.
    pub can_download: bool,
    /// Opened for commenting: read-only like viewing until comments come to the web, but the
    /// interface says so in its own words.
    pub commenting: bool,
    /// Read-only by the app's edit lock ([`crate::Session::edit_lock`]), not by a host: Save
    /// still writes the document, as it stands, to its own file.
    pub may_save: bool,
}

impl Default for Access {
    fn default() -> Self {
        Access { read_only: false, can_download: true, commenting: false, may_save: false }
    }
}

/// Why a command is refused under an [`Access`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    ReadOnly,
    NoDownload,
}

impl Refusal {
    pub fn reason(self) -> &'static str {
        match self {
            Refusal::ReadOnly => "the document is read-only",
            Refusal::NoDownload => "downloading, printing and copying are turned off for this document",
        }
    }
}

/// Commands that hand the document's content to a file, a printer, another app or the clipboard.
pub fn takes_content_out(id: &str) -> bool {
    matches!(
        id,
        "file.save"
            | "file.saveAs"
            | "file.saveTemplate"
            | "file.exportPdf"
            | "file.exportPng"
            | "file.print"
            | "file.printPdf"
            | "file.printMarkup"
            | "file.share"
            | "edit.copy"
            | "edit.cut"
            | "mailings.finish"
    )
}

/// Commands not marked as mutating that still change the document or what it saves as.
fn edits_anyway(id: &str) -> bool {
    // `file.save` writes over the document's own copy (a viewer only saves a copy); Edit Header
    // and Edit Footer add an empty one to edit when there is none; Undo and Redo swap documents.
    matches!(id, "file.encrypt" | "file.save" | "insert.editHeader" | "insert.editFooter" | "edit.undo" | "edit.undoTo" | "edit.redo")
}

/// Commands that only pick up a tool whose every use edits: a pen, the eraser, the ink lasso.
fn picks_an_editing_tool(id: &str) -> bool {
    matches!(id, "draw.pen" | "draw.pencil" | "draw.highlighter" | "draw.addPen" | "draw.eraser" | "draw.lasso")
}

impl Access {
    /// Whether `spec` may run; `None` lets it.
    pub fn refusal(&self, spec: &CommandSpec) -> Option<Refusal> {
        let edits = spec.mutates || edits_anyway(spec.id) || picks_an_editing_tool(spec.id);
        if self.read_only && edits && !(self.may_save && spec.id == "file.save") {
            return Some(Refusal::ReadOnly);
        }
        if !self.can_download && takes_content_out(spec.id) {
            return Some(Refusal::NoDownload);
        }
        None
    }
}
