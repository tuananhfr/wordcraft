//! v2 `preserveView`: a newer version of an open document replaces its content in place and keeps
//! what the user was looking at, the first visible text and the selection, where it still exists.

use wordcraft_doc::{Document, Pos};
use wordcraft_engine::Selection;

use crate::WordApp;

/// Where the user was before the reload.
pub(super) struct KeptView {
    sel: Selection,
    /// The text at the top of the window and how far its line starts above the window's edge;
    /// `None` for a background tab.
    top: Option<Option<(Pos, f32)>>,
}

/// `shown`: the document is the one on screen. A background tab has no page geometry of its own
/// to read the top from (the canvas holds the shown one's), so it keeps its selection only.
pub(super) fn keep(app: &mut WordApp, shown: bool) -> KeptView {
    let top = shown.then(|| crate::canvas::first_visible(app));
    KeptView { sel: app.session.sel.clone(), top }
}

/// After the reload: put the selection and the top back where they still exist in the new content.
pub(super) fn restore(app: &mut WordApp, kept: KeptView) {
    let doc = &app.session.doc;
    if exists(doc, &kept.sel.anchor) && exists(doc, &kept.sel.focus) {
        app.session.sel = kept.sel;
    }
    if let Some(top) = kept.top {
        app.canvas.top_anchor = top.filter(|(pos, _)| exists(doc, pos));
        app.canvas.scroll_to_caret = app.canvas.top_anchor.is_none();
    }
}

/// The paragraph is still there and the offset still falls inside it, on a character boundary.
fn exists(doc: &Document, pos: &Pos) -> bool {
    doc.para_at(pos).is_some() && doc.clamp(pos) == *pos
}
