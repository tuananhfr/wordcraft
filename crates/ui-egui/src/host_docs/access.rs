//! v2 `setMode` and `setUser` (`erp-host.md` rules 12–15) applied to the app: the session's
//! [`wordcraft_engine::Access`] refuses edits; the host's user becomes the author.

use super::{HostLink, HostMode};
use crate::WordApp;

impl WordApp {
    /// `setMode` for `doc_id`: the document and its undo history stay as they are, so turning
    /// editing back on picks up where it stopped. An unknown `doc_id` is ignored.
    pub(super) fn apply_host_mode(&mut self, ctx: &egui::Context, link: &HostLink, doc_id: &str, mode: HostMode, can_download: bool) {
        let Some(document) = link.with_saves(|s| s.by_doc_id(doc_id).map(|d| d.document)).flatten() else { return };
        link.with_saves(|s| s.set_read_only(doc_id, mode.read_only()));
        if mode.read_only() && document == self.session.document_id() {
            self.commit_composition();
            self.keep_comment_draft(ctx);
        }
        let Some(session) = self.session_mut(document) else { return };
        session.access = mode.access(can_download);
    }

    /// A downgrade never drops what was being typed: the input method's composition goes into the
    /// document as one undo step while it is still editable, and a later flush saves it.
    pub(crate) fn commit_composition(&mut self) {
        let text = std::mem::take(&mut self.canvas.ime_preedit);
        if text.is_empty() {
            return;
        }
        self.session.close_typing();
        let id = if self.session.math.is_some() { "equation.type" } else { "text.insert" };
        let _ = self.run(id, serde_json::json!({ "text": text }));
    }

    /// `setUser`: `name` writes the comments and tracked changes this user makes from now on;
    /// `None` gives the preferences' name back.
    pub(super) fn set_host_user(&mut self, name: Option<String>) {
        match name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
            Some(name) => {
                if self.host.prefs_author.is_none() {
                    self.host.prefs_author = Some((self.session.author.clone(), self.session.initials.clone()));
                }
                self.session.author = name;
                // Empty initials are worked out from the name, not left as the preference's.
                self.session.initials.clear();
            }
            None => {
                if let Some((author, initials)) = self.host.prefs_author.take() {
                    self.session.author = author;
                    self.session.initials = initials;
                }
            }
        }
    }

    /// The author name the preferences keep: never the host's user, who is only signed in here.
    pub(crate) fn prefs_author(&self) -> (String, String) {
        self.host.prefs_author.clone().unwrap_or_else(|| (self.session.author.clone(), self.session.initials.clone()))
    }
}
