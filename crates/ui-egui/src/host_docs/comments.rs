//! Web (a shell whose `erpHost` has `commentClicked`): Cloud comments. `anchorOfSelection`,
//! `showComments`, `focusAnchor` and `resolveAnchors` arrive between frames as [`CommentMsg`]s;
//! a click on a mark goes out through [`super::HostTransport::comment_clicked`]. The threads live
//! on the shell's server: nothing here edits or saves the document. A shell without them never
//! calls [`HostLink::set_comments`] and nothing here runs.

use serde_json::{Value, json};
use wordcraft_engine::live::anchor::{Anchor, Placed};

use super::HostLink;
use crate::WordApp;
use crate::comment_marks::MarkThread;

/// Gets `resolveAnchors`' answer, on the next frame.
pub type CommentAnswer = Box<dyn FnOnce(Value)>;

/// One of the shell's comment calls, for the next frame.
pub enum CommentMsg {
    /// `showComments(threads)`.
    Show(Value),
    /// `focusAnchor(anchor, threadId?)`.
    Focus(Value, Option<String>),
    /// `resolveAnchors(threads)`.
    Resolve(Value, CommentAnswer),
}

fn thread_id(thread: &Value) -> Option<String> {
    thread.get("id").or_else(|| thread.get("threadId")).and_then(Value::as_str).filter(|id| !id.is_empty()).map(str::to_string)
}

/// The threads of `showComments` that get a mark: open ones with an anchor of this editor.
fn open_threads(threads: &Value) -> Vec<MarkThread> {
    threads
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t.get("status").and_then(Value::as_str).is_none_or(|s| s == "open"))
        .filter_map(|t| Some(MarkThread { id: thread_id(t)?, anchor: Anchor::parse(t.get("anchor")?)? }))
        .collect()
}

impl HostLink {
    /// The shell shows Cloud comments (found by feature: `erpHost.commentClicked`).
    pub fn set_comments(&self) {
        self.shared.comments.set(true);
    }

    pub fn comments(&self) -> bool {
        self.shared.comments.get()
    }

    pub fn comment_message(&self, message: CommentMsg) {
        if let Ok(mut inbox) = self.shared.comment_inbox.try_borrow_mut() {
            inbox.push(message);
        }
        self.wake();
    }

    /// `anchorOfSelection()`: the anchor of the selection in the shell's document as of the last
    /// frame, `null` when there is none (another document in front, an empty paragraph).
    pub fn anchor_of_selection(&self) -> Value {
        self.shared.selection_anchor.try_borrow().map_or(Value::Null, |a| a.clone())
    }
}

impl WordApp {
    /// The document the shell's comments are on: the shared one, else the newest host document.
    fn comment_document(&mut self, link: &HostLink) -> Option<u64> {
        if let Some(s) = self.live_session() {
            return Some(s.document_id());
        }
        link.with_saves(|s| s.docs().last().map(|d| d.document)).flatten()
    }

    /// Every frame, after the region calls (so anchors are placed on remote edits already).
    pub(super) fn host_comments_tick(&mut self, link: &HostLink) {
        if !link.comments() {
            return;
        }
        let document = self.comment_document(link);
        self.host.marks.target(document);
        let messages = link.shared.comment_inbox.try_borrow_mut().map(|mut m| std::mem::take(&mut *m)).unwrap_or_default();
        for message in messages {
            match message {
                CommentMsg::Show(threads) => self.host.marks.show(open_threads(&threads)),
                CommentMsg::Focus(anchor, thread) => {
                    self.focus_comment(document, &anchor, thread);
                }
                CommentMsg::Resolve(threads, answer) => answer(self.resolve_comments(document, &threads)),
            }
        }
    }

    /// End of every frame: the selection's anchor for `anchorOfSelection`, and a clicked mark.
    pub(super) fn host_comments_observe(&mut self, link: &HostLink) {
        if !link.comments() {
            return;
        }
        let shown = self.host.marks.document() == Some(self.session.document_id());
        let anchor = shown.then(|| self.session.comment_anchor()).flatten().map_or(Value::Null, |a| a.to_json());
        if let Ok(mut kept) = link.shared.selection_anchor.try_borrow_mut() {
            *kept = anchor;
        }
        if let Some(thread) = self.host.marks.take_clicked() {
            link.transport.comment_clicked(&thread);
        }
    }

    /// `focusAnchor(anchor, threadId)`: shows `document` and selects the anchor's text. `false`
    /// when the anchor has no place in it.
    fn focus_comment(&mut self, document: Option<u64>, anchor: &Value, thread: Option<String>) -> bool {
        let Some((document, anchor)) = document.zip(Anchor::parse(anchor)) else { return false };
        let Some(Placed::At { start, end, .. }) = self.find_document(document).map(|(s, _)| s.place_comment_anchor(&anchor)) else {
            return false;
        };
        if self.session.document_id() != document
            && let Err(e) = self.run("ui.activateDocument", json!({"document": document}))
        {
            log::warn!("erpHost.focusAnchor: couldn't show the document: {e}");
            return false;
        }
        let at = |p: &wordcraft_doc::Pos| json!({"story": "body", "path": p.path.0, "off": p.off});
        if let Err(e) = self.run("select.range", json!({"anchor": at(&start), "focus": at(&end)})) {
            log::warn!("erpHost.focusAnchor: couldn't select: {e}");
            return false;
        }
        self.canvas.scroll_to_caret = true;
        self.host.marks.focused = thread;
        true
    }

    /// `resolveAnchors(threads)`: each thread's place in `document` now, as
    /// `{id, threadId, status: "open" | "orphaned", anchor?}`; `anchor` only when it changed. No
    /// answers before the document is open: its threads aren't orphaned, just not placed yet.
    fn resolve_comments(&self, document: Option<u64>, threads: &Value) -> Value {
        let Some(session) = document.and_then(|d| self.find_document(d)).map(|(s, _)| s) else { return Value::Array(Vec::new()) };
        let updates = threads
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| {
                let id = thread_id(t)?;
                let old = Anchor::parse(t.get("anchor")?)?;
                Some(match session.place_comment_anchor(&old) {
                    Placed::At { anchor, .. } if *anchor != old => json!({"id": id, "threadId": id, "status": "open", "anchor": anchor.to_json()}),
                    Placed::At { .. } => json!({"id": id, "threadId": id, "status": "open"}),
                    Placed::Orphaned => json!({"id": id, "threadId": id, "status": "orphaned"}),
                })
            })
            .collect();
        Value::Array(updates)
    }
}
