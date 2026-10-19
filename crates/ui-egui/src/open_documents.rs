//! Several documents open at once. The engine's [`Session`] holds one document, so the app keeps
//! the shown one in [`WordApp::session`] and parks the others, each with its whole session (undo
//! history, selection, view, find…) and the window state it had; switching swaps a parked one in
//! ([`Session::swap_document`] keeps what is the user's rather than a document's in place).

use serde_json::{Value, json};
use wordcraft_engine::Session;

use crate::WordApp;
use crate::canvas::CanvasView;
use crate::dialogs_review::RestrictPane;

/// Most documents open at once: each keeps its whole document and undo history in memory, which
/// a browser tab can't grow without bound.
pub const MAX_DOCUMENTS: usize = 10;

/// A document not shown right now, with what [`WordApp`] keeps for it outside its session.
pub(crate) struct Parked {
    pub(crate) session: Session,
    canvas: CanvasView,
    autosave_path: Option<std::path::PathBuf>,
    untitled: u32,
    restrict: RestrictPane,
}

/// The open documents in tab order: `None` marks the shown one.
pub(crate) struct OpenDocuments {
    slots: Vec<Option<Box<Parked>>>,
    /// The number the next new untitled document is called by ("Document2").
    next_untitled: u32,
}

impl Default for OpenDocuments {
    fn default() -> Self {
        OpenDocuments { slots: vec![None], next_untitled: 2 }
    }
}

impl OpenDocuments {
    pub(crate) fn count(&self) -> usize {
        self.slots.len()
    }

    pub(crate) fn active_index(&self) -> usize {
        self.slots.iter().position(Option::is_none).unwrap_or(0)
    }

    /// The parked documents, by tab index.
    pub(crate) fn parked(&self) -> impl Iterator<Item = (usize, &Parked)> {
        self.slots.iter().enumerate().filter_map(|(i, p)| p.as_deref().map(|p| (i, p)))
    }

    fn parked_index(&self, document: u64) -> Option<usize> {
        self.parked().find(|(_, p)| p.session.document_id() == document).map(|(i, _)| i)
    }
}

/// One tab of the document tab strip.
#[derive(Clone, Debug, PartialEq)]
pub struct DocumentTab {
    pub index: usize,
    pub document: u64,
    pub title: String,
    pub dirty: bool,
    pub active: bool,
}

/// "Document1", "Document2"… in the interface language.
pub(crate) fn untitled_name(number: u32) -> String {
    if number <= 1 { tl!("Document1").into() } else { crate::i18n::fmt(tl!("Document{number}"), &[("number", &number.to_string())]) }
}

/// The title a document is shown and saved under: its file's name, else its title property, else
/// its untitled name.
pub(crate) fn title_of(session: &Session, untitled: u32) -> String {
    match &session.path {
        Some(p) => p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| untitled_name(untitled)),
        None if session.doc.core.title.is_empty() => untitled_name(untitled),
        None => session.doc.core.title.clone(),
    }
}

/// The file name a save suggests: the title, keeping a Word package's own extension.
pub(crate) fn save_name_of(session: &Session, untitled: u32) -> String {
    let ext = session.path.as_ref().and_then(|p| p.extension()).map(|e| e.to_string_lossy().to_string());
    match ext {
        Some(e) if wordcraft_engine::io::is_word_package(&e) => format!("{}.{e}", title_of(session, untitled)),
        _ => format!("{}.docx", title_of(session, untitled)),
    }
}

/// What "Save changes to …?" calls a document: its file name, or its title.
pub(crate) fn prompt_name_of(session: &Session, untitled: u32) -> String {
    session.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| title_of(session, untitled))
}

/// The message an open past [`MAX_DOCUMENTS`] gets.
pub(crate) fn too_many_documents() -> String {
    tl!("You can have up to 10 documents open. Close one to open another.").to_string()
}

impl WordApp {
    /// How many documents are open (the shown one included).
    pub fn document_count(&self) -> usize {
        self.docs.count()
    }

    /// The tab index of the shown document.
    pub fn active_document_index(&self) -> usize {
        self.docs.active_index()
    }

    /// The open documents in tab order.
    pub fn document_tabs(&self) -> Vec<DocumentTab> {
        let active = self.docs.active_index();
        (0..self.docs.count())
            .filter_map(|i| {
                let (session, untitled) = self.document_at(i)?;
                Some(DocumentTab {
                    index: i,
                    document: session.document_id(),
                    title: title_of(session, untitled),
                    dirty: session.dirty,
                    active: i == active,
                })
            })
            .collect()
    }

    /// The session and untitled number of the document at tab `i`.
    fn document_at(&self, i: usize) -> Option<(&Session, u32)> {
        match self.docs.slots.get(i)? {
            None => Some((&self.session, self.untitled)),
            Some(p) => Some((&p.session, p.untitled)),
        }
    }

    /// The id of the document at tab `i`.
    pub(crate) fn document_id_at(&self, i: usize) -> Option<u64> {
        self.document_at(i).map(|(s, _)| s.document_id())
    }

    /// The open document with this id, shown or parked, with its untitled number.
    pub(crate) fn find_document(&self, document: u64) -> Option<(&Session, u32)> {
        if self.session.document_id() == document {
            return Some((&self.session, self.untitled));
        }
        self.docs.parked().find(|(_, p)| p.session.document_id() == document).map(|(_, p)| (&p.session, p.untitled))
    }

    /// The tab index of the open document with this id.
    pub fn document_index(&self, document: u64) -> Option<usize> {
        if self.session.document_id() == document { Some(self.docs.active_index()) } else { self.docs.parked_index(document) }
    }

    /// Whether any open document has unsaved changes.
    pub fn any_document_dirty(&self) -> bool {
        self.session.dirty || self.docs.parked().any(|(_, p)| p.session.dirty)
    }

    /// Mark the open document with this id as having unsaved changes (a save that didn't happen).
    pub(crate) fn mark_document_unsaved(&mut self, document: u64) {
        if self.session.document_id() == document {
            self.session.dirty = true;
        } else if let Some(p) = self.docs.slots.iter_mut().flatten().find(|p| p.session.document_id() == document) {
            p.session.dirty = true;
        }
    }

    /// Swap the shown document with the one parked at tab `i`; nothing else changes.
    fn swap_with(&mut self, i: usize) {
        let active = self.docs.active_index();
        let Some(mut p) = self.docs.slots.get_mut(i).and_then(Option::take) else { return };
        self.session.swap_document(&mut p.session);
        self.canvas.swap_view(&mut p.canvas);
        std::mem::swap(&mut self.autosave_path, &mut p.autosave_path);
        std::mem::swap(&mut self.untitled, &mut p.untitled);
        std::mem::swap(&mut self.restrict, &mut p.restrict);
        if let Some(slot) = self.docs.slots.get_mut(active) {
            *slot = Some(p);
        }
    }

    /// Run `f` with the document `document` shown, then show the one that was: a save or a
    /// draft of a document in the background. `None` when it isn't open.
    pub(crate) fn in_document<R>(&mut self, document: u64, f: impl FnOnce(&mut WordApp) -> R) -> Option<R> {
        let i = self.document_index(document)?;
        if i == self.docs.active_index() {
            return Some(f(self));
        }
        let back = self.session.document_id();
        self.swap_with(i);
        let r = f(self);
        if let Some(j) = self.document_index(back).filter(|j| *j != self.docs.active_index()) {
            self.swap_with(j);
        }
        Some(r)
    }

    /// Show the document at tab `i` (a tab click, `ui.activateDocument`).
    pub fn activate_document(&mut self, i: usize) -> Result<(), String> {
        if i >= self.docs.count() {
            return Err(format!("no document at index {i}"));
        }
        if i == self.docs.active_index() {
            return Ok(());
        }
        if self.dialog.is_some() || self.colors.is_some() || self.file_dialog_open() {
            return Err("close the open dialog first".into());
        }
        self.swap_with(i);
        self.document_switched();
        Ok(())
    }

    /// Another document is shown: what the interface held about the last one would act on the
    /// wrong document.
    fn document_switched(&mut self) {
        self.canvas.document_switched();
        self.controls = Default::default();
        self.dialog = None;
        self.colors = None;
        self.change_picture_target = None;
        self.smart_art_item = None;
        self.smart_art_typing = None;
        self.info_editing = None;
        self.read_aloud_at = None;
        if self.session.read_aloud.open {
            self.session.read_aloud.stop();
        }
    }

    /// Whether opening a document replaces the shown one instead of opening beside it: the
    /// only document is the blank one the app started with (or the Start screen's placeholder),
    /// untouched.
    fn replaces_untouched(&self) -> bool {
        self.docs.count() == 1 && (!self.has_document() || self.session.is_untouched_blank())
    }

    /// Run `id` (an open or a New) in a new document beside the open ones, or in place of an
    /// untouched blank one. The one place [`MAX_DOCUMENTS`] holds; a refused or failed command
    /// leaves the open documents as they were.
    pub(crate) fn run_opening(&mut self, id: &str, params: &Value) -> Result<Value, String> {
        if self.replaces_untouched() {
            return self.session.run(id, params).map_err(|e| e.to_string());
        }
        if self.docs.count() >= MAX_DOCUMENTS {
            let e = too_many_documents();
            // File › Open and New are on the File tab, which hides the status bar that says why.
            self.ui.backstage = false;
            return Err(e);
        }
        let mut fresh = self.session.sibling();
        self.session.swap_document(&mut fresh);
        let mut parked = Box::new(Parked {
            session: fresh,
            canvas: self.canvas.fresh_view(),
            autosave_path: self.autosave_path.take(),
            untitled: 0,
            restrict: Default::default(),
        });
        self.canvas.swap_view(&mut parked.canvas);
        std::mem::swap(&mut self.untitled, &mut parked.untitled);
        std::mem::swap(&mut self.restrict, &mut parked.restrict);
        match self.session.run(id, params) {
            Ok(v) => {
                let active = self.docs.active_index();
                if let Some(slot) = self.docs.slots.get_mut(active) {
                    *slot = Some(parked);
                }
                self.docs.slots.push(None);
                if self.session.path.is_none() {
                    self.untitled = self.take_untitled_number();
                }
                self.document_switched();
                Ok(v)
            }
            Err(e) => {
                self.session.swap_document(&mut parked.session);
                self.canvas.swap_view(&mut parked.canvas);
                self.autosave_path = parked.autosave_path.take();
                std::mem::swap(&mut self.untitled, &mut parked.untitled);
                std::mem::swap(&mut self.restrict, &mut parked.restrict);
                Err(e.to_string())
            }
        }
    }

    /// The number for a new untitled document, counting up through the session as Word does, past
    /// any an open document is already titled with (a restored draft keeps its "Document2").
    fn take_untitled_number(&mut self) -> u32 {
        let taken: Vec<String> = self.docs.parked().map(|(_, p)| title_of(&p.session, p.untitled).to_lowercase()).collect();
        let start = self.docs.next_untitled.max(2);
        let n = (start..=u32::MAX).find(|n| !taken.contains(&untitled_name(*n).to_lowercase())).unwrap_or(start);
        self.docs.next_untitled = n.saturating_add(1);
        n
    }

    /// Close the document at tab `i` without asking; closing another one keeps the shown one,
    /// closing the shown one shows the next (or the last). Closing the only one quits, as Close
    /// did before tabs.
    pub(crate) fn close_document_at(&mut self, i: usize) {
        let active = self.docs.active_index();
        if i >= self.docs.count() {
            return;
        }
        if self.docs.count() == 1 {
            self.quit_requested = true;
            return;
        }
        if i == active {
            let next = if i + 1 < self.docs.count() { i + 1 } else { i - 1 };
            self.swap_with(next);
            self.docs.slots.remove(i);
            self.document_switched();
        } else {
            self.docs.slots.remove(i);
        }
        if self.closing_all {
            self.close_next_for_quit();
        }
    }

    /// Quitting with unsaved documents: ask about the next one (shown first), or quit once none
    /// is left. Cancel on any prompt stops the quit.
    pub(crate) fn close_next_for_quit(&mut self) {
        let dirty = self.document_tabs().into_iter().find(|t| t.dirty);
        let Some(tab) = dirty else {
            self.closing_all = false;
            self.quit_requested = true;
            return;
        };
        if !tab.active && self.activate_document(tab.index).is_err() {
            self.closing_all = false;
            return;
        }
        let _ = self.run("file.close", json!({}));
    }

    /// The tab a `ui.activateDocument` / `ui.closeDocument` call means: `index`, or `document`
    /// (its id); neither is the shown one.
    pub(crate) fn document_param(&self, p: &Value) -> Result<usize, String> {
        if let Some(d) = p.get("document").and_then(Value::as_u64) {
            return self.document_index(d).ok_or_else(|| format!("no open document with id {d}"));
        }
        match p.get("index") {
            None => Ok(self.docs.active_index()),
            Some(v) => match v.as_u64().and_then(|i| usize::try_from(i).ok()) {
                Some(i) if i < self.docs.count() => Ok(i),
                _ => Err(format!("`index` must be a document's tab index, 0 to {}", self.docs.count().saturating_sub(1))),
            },
        }
    }
}

/// Commands that open or create a document (and so open it beside the others): New, a template,
/// and Open with a file (without one it only shows the file picker).
pub(crate) fn opens_document(id: &str, params: &Value) -> bool {
    match id {
        "file.new" | "file.newFromTemplate" => true,
        "file.open" => params.get("path").is_some(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use serde_json::json;
    use wordcraft_doc::StoryRef;
    use wordcraft_engine::Session;

    use super::MAX_DOCUMENTS;
    use crate::dialogs::Dialog;
    use crate::{Download, PickerAnswer, Services, WordApp};

    fn app() -> WordApp {
        WordApp::new(Session::new(wordcraft_doc::Document::new()), Services::default())
    }

    fn typed(a: &mut WordApp, text: &str) {
        a.run("text.insert", json!({"text": text})).unwrap();
    }

    fn body(a: &WordApp) -> String {
        a.session.doc.plain_text(StoryRef::Body)
    }

    fn asked_about(a: &WordApp) -> Option<(String, u64)> {
        match &a.dialog {
            Some(Dialog::SaveChanges { name, document, .. }) => Some((name.clone(), *document)),
            _ => None,
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("wordcraft-tabs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn docx_on_disk(dir: &std::path::Path, file: &str, text: &str) -> std::path::PathBuf {
        let path = dir.join(file);
        let mut s = Session::new(wordcraft_doc::Document::from_text(text));
        s.run("file.save", &json!({"path": path.to_string_lossy()})).unwrap();
        path
    }

    fn harness(a: WordApp) -> egui_kittest::Harness<'static, WordApp> {
        egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 900.0)).build_ui_state(
            |ui, app: &mut WordApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            a,
        )
    }

    #[test]
    fn the_tab_strip_shows_only_with_two_documents_or_more() {
        let mut h = harness(app());
        h.run_steps(3);
        assert!(h.query_by_label("Close Document1").is_none(), "one document: no tabs");
        h.state_mut().run("file.new", json!({})).unwrap();
        h.run_steps(3);
        assert!(h.query_by_label("Close Document1").is_none(), "the untouched start document was replaced");
        typed(h.state_mut(), "Kept");
        h.state_mut().run("file.new", json!({})).unwrap();
        h.run_steps(3);
        assert!(h.query_by_label("Close Document1").is_some());
        assert!(h.query_by_label("Close Document2").is_some());

        // A click on a tab shows it; × closes it; one document left hides the strip again.
        h.get_by_label("Document1").click();
        h.run_steps(3);
        assert_eq!(h.state().active_document_index(), 0);
        assert!(body(h.state()).contains("Kept"));
        h.get_by_label("Close Document2").click();
        h.run_steps(3);
        assert_eq!(h.state().document_count(), 1);
        assert!(h.state().dialog.is_none(), "an untouched document closes without asking");
        assert!(h.query_by_label("Close Document1").is_none());
    }

    #[test]
    fn a_long_title_is_cut_with_an_ellipsis() {
        let job = crate::doc_tabs::elided(
            "A very long document title that will never fit in one tab",
            egui::FontId::proportional(12.5),
            egui::Color32::WHITE,
            80.0,
        );
        let ctx = egui::Context::default();
        let mut galley = None;
        ctx.run_ui(egui::RawInput::default(), |ui| galley = Some(ui.painter().layout_job(job.clone()))).drop_without_applying_deltas();
        let galley = galley.unwrap();
        assert_eq!(galley.rows.len(), 1);
        assert_eq!(galley.rows[0].row.glyphs.last().map(|g| g.chr), Some('…'));
        assert!(galley.size().x <= 80.5);
    }

    #[test]
    fn switching_keeps_each_documents_text_history_selection_find_and_zoom() {
        let mut a = app();
        typed(&mut a, "Alpha one");
        a.run("view.zoom", json!({"value": 150})).unwrap();
        a.run("edit.find", json!({"text": "one"})).unwrap();
        a.run("select.all", json!({})).unwrap();
        a.canvas.scroll_offset = egui::vec2(0.0, 420.0);
        let first = a.session.document_id();

        a.run("file.new", json!({})).unwrap();
        assert_ne!(a.session.document_id(), first);
        assert!(body(&a).trim().is_empty());
        assert_eq!(a.canvas.scroll_offset, egui::Vec2::ZERO, "a new document starts at the top");
        assert!(a.session.find.query.is_empty());
        typed(&mut a, "Beta");
        a.run("view.zoom", json!({"value": 80})).unwrap();

        a.run("ui.activateDocument", json!({"document": first})).unwrap();
        assert!(body(&a).contains("Alpha one"));
        assert_eq!(a.session.selected_text(), "Alpha one");
        assert!((a.session.view.zoom - 1.5).abs() < 1e-4);
        assert_eq!(a.session.find.query, "one");
        assert_eq!(a.canvas.scroll_offset, egui::vec2(0.0, 420.0));
        a.run("edit.undo", json!({})).unwrap();
        assert!(!body(&a).contains("Alpha one"), "its own history: undo takes back its typing");

        a.run("ui.activateDocument", json!({"index": 1})).unwrap();
        assert!(body(&a).contains("Beta"));
        assert!((a.session.view.zoom - 0.8).abs() < 1e-4);
        a.run("edit.undo", json!({})).unwrap();
        assert!(!body(&a).contains("Beta"));
        a.run("ui.activateDocument", json!({"index": 0})).unwrap();
        a.run("edit.redo", json!({})).unwrap();
        assert!(body(&a).contains("Alpha one"), "redo too");
    }

    #[test]
    fn user_wide_state_stays_put_across_a_switch() {
        let mut a = app();
        typed(&mut a, "Copied");
        a.run("select.all", json!({})).unwrap();
        a.run("edit.copy", json!({})).unwrap();
        a.run("file.new", json!({})).unwrap();
        a.run("edit.paste", json!({})).unwrap();
        assert!(body(&a).contains("Copied"), "the clipboard is the user's, not the document's");
    }

    #[test]
    fn closing_a_tab_closes_that_document_and_asks_when_it_is_unsaved() {
        let mut a = app();
        typed(&mut a, "Unsaved A");
        let first = a.session.document_id();
        a.run("file.new", json!({})).unwrap();
        a.run("file.new", json!({})).unwrap();
        let third = a.session.document_id();
        let second = a.document_id_at(1).unwrap();

        // A clean one in the background closes at once and the shown one stays.
        a.run("ui.closeDocument", json!({"document": second})).unwrap();
        assert_eq!(a.document_count(), 2);
        assert_eq!(a.session.document_id(), third);

        // An unsaved one asks, naming it, while the shown one stays shown.
        a.run("ui.closeDocument", json!({"index": 0})).unwrap();
        assert_eq!(asked_about(&a), Some(("Document1".to_string(), first)));
        assert_eq!(a.session.document_id(), third);
        a.run("ui.saveChanges", json!({"answer": "cancel"})).unwrap();
        assert_eq!(a.document_count(), 2);
        a.run("ui.closeDocument", json!({"index": 0})).unwrap();
        a.run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
        assert_eq!(a.document_count(), 1);
        assert!(a.find_document(first).is_none());
        assert_eq!(a.session.document_id(), third);
        assert!(!a.quit_requested);

        // The shown one closing shows its neighbour.
        typed(&mut a, "x");
        a.run("file.new", json!({})).unwrap();
        a.run("ui.activateDocument", json!({"index": 0})).unwrap();
        a.run("file.close", json!({})).unwrap();
        a.run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
        assert_eq!(a.document_count(), 1);
        assert_ne!(a.session.document_id(), third);
        assert!(!a.quit_requested);
    }

    #[test]
    fn save_in_the_prompt_writes_the_background_document_it_asked_about() {
        let dir = scratch("background-save");
        let path = docx_on_disk(&dir, "a.docx", "Original");
        let mut a = app();
        a.run("file.open", json!({"path": path.to_string_lossy()})).unwrap();
        typed(&mut a, "Edited ");
        a.run("file.new", json!({})).unwrap();
        typed(&mut a, "Shown");
        a.run("ui.closeDocument", json!({"index": 0})).unwrap();
        assert_eq!(asked_about(&a).map(|(n, _)| n), Some("a.docx".to_string()));
        a.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
        let saved = wordcraft_engine::io::open_path(&path).unwrap().plain_text(StoryRef::Body);
        assert!(saved.contains("Edited Original") && !saved.contains("Shown"), "{saved}");
        assert_eq!(a.document_count(), 1);
        assert!(body(&a).contains("Shown") && a.session.dirty);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn closing_the_window_asks_about_each_unsaved_document() {
        let mut a = app();
        typed(&mut a, "One");
        a.run("file.new", json!({})).unwrap();
        a.run("file.new", json!({})).unwrap();
        typed(&mut a, "Three");
        assert!(!a.close_requested());
        assert_eq!(asked_about(&a).map(|(n, _)| n), Some("Document1".to_string()));
        a.run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
        assert_eq!(asked_about(&a).map(|(n, _)| n), Some("Document3".to_string()), "the clean one needs no question");
        a.run("ui.saveChanges", json!({"answer": "cancel"})).unwrap();
        assert!(!a.quit_requested, "Cancel stops closing");
        assert_eq!(a.document_count(), 2);
        assert!(!a.close_requested());
        a.run("ui.saveChanges", json!({"answer": "dontSave"})).unwrap();
        assert!(a.quit_requested);
    }

    #[test]
    fn every_open_path_stops_at_the_limit() {
        let dir = scratch("limit");
        let path = docx_on_disk(&dir, "late.docx", "Late");
        let template = docx_on_disk(&dir, "template.docx", "Template");
        let inbox: crate::Inbox = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut a = app();
        a.services.inbox = Some(inbox.clone());
        typed(&mut a, "x");
        for _ in 1..MAX_DOCUMENTS {
            a.run("file.new", json!({})).unwrap();
        }
        assert_eq!(a.document_count(), MAX_DOCUMENTS);
        let shown = a.session.document_id();
        let refused = |a: &WordApp, what: &str| {
            assert_eq!(a.document_count(), MAX_DOCUMENTS, "{what}");
            assert_eq!(a.session.document_id(), shown, "{what}: the shown document stays");
        };
        for (id, params) in [
            ("file.new", json!({})),
            ("file.new", json!({"template": "letter"})),
            ("file.newFromTemplate", json!({"path": template.to_string_lossy()})),
            ("file.open", json!({"path": path.to_string_lossy()})),
        ] {
            let e = a.run(id, params).unwrap_err();
            assert!(e.contains("up to 10 documents"), "{id}: {e}");
            refused(&a, id);
        }
        // A drop (and the web shell's Open, which arrives as one) says why in the status bar.
        a.status_msg = None;
        inbox.lock().unwrap().push(("dropped.docx".into(), std::fs::read(&path).unwrap()));
        a.drain_inbox();
        refused(&a, "drop");
        assert!(a.status_msg.as_ref().is_some_and(|(m, _)| m.contains("up to 10 documents")), "{:?}", a.status_msg);
        // Closing one makes room.
        a.run("ui.closeDocument", json!({"index": 1})).unwrap();
        a.run("file.open", json!({"path": path.to_string_lossy()})).unwrap();
        assert_eq!(a.document_count(), MAX_DOCUMENTS);
        assert!(body(&a).contains("Late"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_limit_message_is_translated() {
        let vi = crate::i18n::Lang::from_code("vi").unwrap();
        assert_eq!(
            crate::i18n::tr(vi, &super::too_many_documents()),
            "Chỉ mở được tối đa 10 tài liệu. Hãy đóng bớt một tài liệu để mở tài liệu khác."
        );
        assert_eq!(crate::i18n::fmt(crate::i18n::tr(vi, "Document{number}"), &[("number", "3")]), "Tài liệu3");
    }

    #[test]
    fn document_ids_are_unique_across_documents_and_windows() {
        let mut a = app();
        let b = app();
        typed(&mut a, "x");
        let mut ids = vec![b.session.document_id(), a.session.document_id()];
        for _ in 0..4 {
            a.run("file.new", json!({})).unwrap();
            ids.push(a.session.document_id());
        }
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "{ids:?}");
        let tab_ids: Vec<u64> = a.document_tabs().iter().map(|t| t.document).collect();
        assert_eq!(tab_ids.len(), 5);
        assert!(tab_ids.iter().all(|d| ids.contains(d)));
    }

    /// Web: the save picker answers after the user may have switched tabs; its answer is about
    /// the document it wrote.
    #[test]
    fn a_picker_answer_after_a_switch_is_about_the_document_it_wrote() {
        use std::sync::mpsc::{Sender, channel};
        let answers: std::rc::Rc<std::cell::RefCell<Vec<Sender<PickerAnswer>>>> = Default::default();
        let mut a = app();
        let queued = answers.clone();
        a.services.download = Some(Box::new(move |_, _| {
            let (tx, rx) = channel();
            queued.borrow_mut().push(tx);
            Ok(Download::Pending(rx))
        }));
        let answer = |a: PickerAnswer| answers.borrow_mut().remove(0).send(a).unwrap();
        let ctx = egui::Context::default();
        let frame = |a: &mut WordApp| ctx.run_ui(egui::RawInput::default(), |ui| a.logic(ui.ctx())).drop_without_applying_deltas();

        typed(&mut a, "First");
        let first = a.session.document_id();
        a.run("file.save", json!({})).unwrap();
        a.run("file.new", json!({})).unwrap();
        typed(&mut a, "Second");
        a.run("file.save", json!({})).unwrap();
        let second = a.session.document_id();
        a.run("ui.activateDocument", json!({"document": first})).unwrap();
        // The second's save was cancelled: it is unsaved again, the shown first isn't.
        answer(Ok(()));
        answer(Err(None));
        frame(&mut a);
        assert!(!a.session.dirty);
        assert!(a.find_document(second).is_some_and(|(s, _)| s.dirty));

        // Close the second's tab from the first with Save; the picker answers once the user has
        // moved on to a third: the second closes, the shown third stays.
        a.run("ui.closeDocument", json!({"document": second})).unwrap();
        let r = a.run("ui.saveChanges", json!({"answer": "save"})).unwrap();
        assert_eq!(r["pending"], "save");
        typed(&mut a, "More");
        a.run("file.new", json!({})).unwrap();
        let third = a.session.document_id();
        frame(&mut a);
        assert!(a.find_document(second).is_some(), "nothing closes before the file is written");
        answer(Ok(()));
        frame(&mut a);
        assert!(a.find_document(second).is_none());
        assert_eq!(a.session.document_id(), third);
        assert_eq!(a.document_count(), 2);
        assert!(a.find_document(first).is_some_and(|(s, _)| s.dirty), "the first keeps its later edits");
    }
}
