//! A comment being written when `setMode` turns editing off: it is neither posted nor dropped,
//! but put aside in memory and given back, with the keyboard, when editing comes back to that
//! document in this session.

use crate::WordApp;
use crate::panes::{edit_focus_key, edit_key, reply_focus_key, reply_key, reply_open_key};

/// What was being written in the shown document when it turned read-only.
pub(crate) struct KeptDraft {
    document: u64,
    balloon: Option<KeptBalloon>,
    /// The Comments pane field holding a draft, as (comment, its reply field). Its text stays where
    /// it is (the pane shows the comment as saved while read-only); only the keyboard is restored.
    pane: Option<(u32, bool)>,
}

/// The selected balloon's unposted text, taken out of its fields.
struct KeptBalloon {
    id: u32,
    /// The reply field was open.
    replying: bool,
    /// Draft texts by their field's key: the comment and its replies being edited, a new reply.
    texts: Vec<(egui::Id, String)>,
}

fn has_draft(ctx: &egui::Context, key: egui::Id) -> bool {
    ctx.data(|d| d.get_temp::<String>(key).is_some())
}

impl WordApp {
    /// The shown document just turned read-only: the balloon's drafts leave its fields, and the
    /// Comments pane's field with a draft is noted.
    pub(super) fn keep_comment_draft(&mut self, ctx: &egui::Context) {
        let thread = self.canvas.balloon.map(|id| crate::canvas::balloon_thread(self, id)).unwrap_or_default();
        let balloon = self.canvas.balloon.and_then(|id| {
            let keys = thread.iter().copied().map(edit_key).chain([reply_key(id)]);
            let texts: Vec<(egui::Id, String)> = keys.filter_map(|k| ctx.data_mut(|d| d.remove_temp::<String>(k)).map(|t| (k, t))).collect();
            let replying = std::mem::take(&mut self.canvas.balloon_reply);
            (replying || !texts.is_empty()).then_some(KeptBalloon { id, replying, texts })
        });
        let pane = self.session.doc.comments.keys().copied().filter(|id| !thread.contains(id)).find_map(|id| {
            if has_draft(ctx, edit_key(id)) {
                Some((id, false))
            } else {
                let open = ctx.data(|d| d.get_temp::<bool>(reply_open_key(id))).unwrap_or(false);
                (open && has_draft(ctx, reply_key(id))).then_some((id, true))
            }
        });
        if balloon.is_none() && pane.is_none() {
            return;
        }
        let document = self.session.document_id();
        self.host.kept_drafts.retain(|k| k.document != document);
        self.host.kept_drafts.push(KeptDraft { document, balloon, pane });
    }

    /// Every frame: a kept draft goes back to its field, which gets the keyboard, once its
    /// document is shown and editable again (and the comment is still there).
    pub(super) fn restore_comment_draft(&mut self, ctx: &egui::Context) {
        if self.read_only() {
            return;
        }
        let document = self.session.document_id();
        let Some(i) = self.host.kept_drafts.iter().position(|k| k.document == document) else { return };
        let kept = self.host.kept_drafts.swap_remove(i);
        if let Some(b) = kept.balloon.filter(|b| self.session.doc.comments.contains_key(&b.id)) {
            crate::canvas::select_balloon(self, ctx, b.id);
            for (key, text) in b.texts {
                ctx.data_mut(|d| d.insert_temp(key, text));
            }
            self.canvas.balloon_reply = b.replying;
            self.canvas.balloon_focus = true;
        } else if let Some((id, reply)) = kept.pane.filter(|(id, _)| self.session.doc.comments.contains_key(id)) {
            let key = if reply { reply_focus_key(id) } else { edit_focus_key(id) };
            ctx.data_mut(|d| d.insert_temp(key, true));
        }
    }
}
